use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use fuck_ai_comments::AnalysisContext;
use ignore::WalkBuilder;
use serde::Deserialize;

#[cfg(test)]
thread_local! {
    static CARGO_METADATA_DISCOVERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static CARGO_MANIFEST_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_discovery() {
    CARGO_METADATA_DISCOVERIES.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_discovery() {}

#[cfg(test)]
fn record_manifest_probe() {
    CARGO_MANIFEST_PROBES.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_manifest_probe() {}

#[cfg(test)]
pub(super) fn reset_discovery_count() {
    CARGO_METADATA_DISCOVERIES.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn cargo_metadata_discoveries() -> usize {
    CARGO_METADATA_DISCOVERIES.with(std::cell::Cell::get)
}

#[cfg(test)]
fn reset_manifest_probe_count() {
    CARGO_MANIFEST_PROBES.with(|count| count.set(0));
}

#[cfg(test)]
fn manifest_probe_count() -> usize {
    CARGO_MANIFEST_PROBES.with(std::cell::Cell::get)
}

pub(super) struct CargoContext {
    analysis: AnalysisContext,
    implicit_library_inputs: BTreeSet<PathBuf>,
    manifests: BTreeSet<PathBuf>,
}

impl CargoContext {
    pub(super) fn analysis(&self) -> &AnalysisContext {
        &self.analysis
    }

    pub(super) fn manifests(&self) -> impl Iterator<Item = &Path> {
        self.manifests.iter().map(PathBuf::as_path)
    }

    pub(super) fn implicit_library_inputs(&self) -> impl Iterator<Item = &Path> {
        self.implicit_library_inputs.iter().map(PathBuf::as_path)
    }
}

pub(super) fn discover(start: &Path, analysis_base: &Path) -> Result<CargoContext> {
    discover_with_manifests(start, analysis_base, std::iter::empty::<PathBuf>())
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    workspace_root: PathBuf,
    version: u32,
}

#[derive(Deserialize)]
struct CargoPackage {
    manifest_path: PathBuf,
    targets: Vec<CargoTarget>,
}

#[derive(Deserialize)]
struct CargoTarget {
    kind: Vec<String>,
    src_path: PathBuf,
}

pub(super) fn discover_with_manifests<I, P>(
    start: &Path,
    analysis_base: &Path,
    required_manifests: I,
) -> Result<CargoContext>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let mut candidates: BTreeSet<_> = manifest_candidates(start)?.into_iter().collect();
    for manifest in required_manifests {
        let manifest = validated_absolute_path(manifest.as_ref(), "required Cargo manifest")?;
        validate_regular_manifest(&manifest)?;
        candidates.insert(manifest);
    }
    if candidates.is_empty() {
        return Ok(CargoContext {
            analysis: AnalysisContext::from_rust_library_roots(std::iter::empty::<PathBuf>())?,
            implicit_library_inputs: BTreeSet::new(),
            manifests: BTreeSet::new(),
        });
    }

    let analysis_base = normalized_absolute_path(analysis_base, "analysis base")?;
    let mut discovered_manifests = BTreeSet::new();
    let mut covered_manifests = BTreeSet::new();
    let mut implicit_library_inputs = BTreeSet::new();
    let mut roots = BTreeSet::new();
    let mut candidates: Vec<_> = candidates.into_iter().collect();
    sort_manifests(&mut candidates);
    for manifest in candidates {
        if covered_manifests.contains(&manifest) {
            continue;
        }
        let metadata = cargo_metadata(&manifest)?;
        if metadata.version != 1 {
            bail!(
                "Cargo returned unsupported metadata version {}",
                metadata.version
            );
        }

        let workspace_root = validated_absolute_path(&metadata.workspace_root, "workspace root")?;
        let workspace_manifest = workspace_root.join("Cargo.toml");
        discovered_manifests.insert(manifest);
        discovered_manifests.insert(workspace_manifest.clone());
        covered_manifests.insert(workspace_manifest);
        for package in metadata.packages {
            let package_manifest =
                validated_absolute_path(&package.manifest_path, "package manifest")?;
            let conventional_root = package_manifest
                .parent()
                .context("Cargo returned a package manifest without a parent directory")?
                .join("src/lib.rs");
            discovered_manifests.insert(package_manifest.clone());
            covered_manifests.insert(package_manifest);
            let mut has_library_target = false;
            let mut conventional_root_is_a_target = false;
            for target in package.targets.into_iter().filter(CargoTarget::is_library) {
                let source = validated_absolute_path(&target.src_path, "library target")?;
                has_library_target = true;
                conventional_root_is_a_target |= source == conventional_root;
                roots.insert(source.clone());
                if let Ok(relative) = source.strip_prefix(&analysis_base) {
                    roots.insert(relative.to_owned());
                }
            }
            if conventional_root_is_a_target
                || (!has_library_target && !path_exists(&conventional_root)?)
            {
                implicit_library_inputs.insert(conventional_root);
            }
        }
    }

    let analysis = AnalysisContext::from_rust_library_roots(roots)?;
    Ok(CargoContext {
        analysis,
        implicit_library_inputs,
        manifests: discovered_manifests,
    })
}

pub(super) fn nearest_manifests_for_rust_sources<I, P>(
    sources: I,
    boundary: &Path,
) -> Result<BTreeSet<PathBuf>>
where
    I: IntoIterator<Item = P>,
    P: AsRef<Path>,
{
    let boundary = normalized_absolute_path(boundary, "Cargo manifest search boundary")?;
    let mut manifests = BTreeSet::new();
    let mut nearest_by_directory = BTreeMap::new();
    for source in sources {
        let source = source.as_ref();
        if source.extension() != Some(OsStr::new("rs")) {
            continue;
        }
        let source = normalized_absolute_path(source, "Rust source path")?;
        let directory = source
            .parent()
            .with_context(|| format!("{} has no parent directory", source.display()))?;
        if let Some(manifest) =
            nearest_manifest_within(directory, &boundary, &mut nearest_by_directory)?
        {
            manifests.insert(manifest);
        }
    }
    Ok(manifests)
}

fn nearest_manifest_within(
    directory: &Path,
    boundary: &Path,
    nearest_by_directory: &mut BTreeMap<PathBuf, Option<PathBuf>>,
) -> Result<Option<PathBuf>> {
    let mut uncached = Vec::new();
    let mut nearest = None;
    for ancestor in directory.ancestors() {
        if !ancestor.starts_with(boundary) {
            break;
        }
        if let Some(cached) = nearest_by_directory.get(ancestor) {
            nearest = cached.clone();
            break;
        }
        uncached.push(ancestor.to_owned());
        if let Some(manifest) = manifest_in_directory(ancestor)? {
            nearest = Some(manifest);
            break;
        }
    }
    for directory in uncached {
        nearest_by_directory.insert(directory, nearest.clone());
    }
    Ok(nearest)
}

fn path_exists(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("could not inspect {}", path.display())),
    }
}

fn cargo_metadata(manifest: &Path) -> Result<CargoMetadata> {
    record_discovery();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(manifest)
        .output()
        .with_context(|| {
            format!(
                "could not run Cargo metadata for detected manifest {}",
                manifest.display()
            )
        })?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        bail!(
            "Cargo metadata failed for detected manifest {}: {}",
            manifest.display(),
            detail.trim()
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Cargo returned invalid metadata for {}", manifest.display()))
}

impl CargoTarget {
    fn is_library(&self) -> bool {
        self.kind.iter().any(|kind| {
            matches!(
                kind.as_str(),
                "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"
            )
        })
    }
}

fn manifest_candidates(start: &Path) -> Result<Vec<PathBuf>> {
    let absolute = normalized_absolute_path(start, "Cargo discovery path")?;
    let metadata = fs::symlink_metadata(&absolute)
        .with_context(|| format!("could not inspect {}", start.display()))?;
    let (directory, scan_descendants) = if metadata.file_type().is_dir() {
        (absolute.as_path(), true)
    } else {
        (
            absolute
                .parent()
                .with_context(|| format!("{} has no parent directory", start.display()))?,
            false,
        )
    };

    let mut manifests = BTreeSet::new();
    if let Some(manifest) = nearest_manifest(directory)? {
        manifests.insert(manifest);
    }
    if scan_descendants {
        let mut builder = WalkBuilder::new(directory);
        builder
            .standard_filters(true)
            .hidden(false)
            .require_git(false)
            .follow_links(false)
            .filter_entry(|entry| entry.file_name() != ".git");
        for entry in builder.build() {
            let entry = entry.with_context(|| {
                format!(
                    "could not discover Cargo manifests below {}",
                    directory.display()
                )
            })?;
            if let Some(error) = entry.error() {
                bail!(
                    "could not discover Cargo manifests at {}: {error}",
                    entry.path().display()
                );
            }
            if entry
                .file_type()
                .is_some_and(|file_type| file_type.is_dir())
                && let Some(manifest) = manifest_in_directory(entry.path())?
            {
                manifests.insert(manifest);
            }
            if entry.file_name() != "Cargo.toml" {
                continue;
            }
            validate_regular_manifest(entry.path())?;
            manifests.insert(entry.into_path());
        }
    }

    let mut manifests: Vec<_> = manifests.into_iter().collect();
    sort_manifests(&mut manifests);
    Ok(manifests)
}

fn sort_manifests(manifests: &mut [PathBuf]) {
    manifests.sort_by(|left, right| {
        left.components()
            .count()
            .cmp(&right.components().count())
            .then_with(|| left.cmp(right))
    });
}

fn nearest_manifest(directory: &Path) -> Result<Option<PathBuf>> {
    for ancestor in directory.ancestors() {
        if let Some(manifest) = manifest_in_directory(ancestor)? {
            return Ok(Some(manifest));
        }
    }
    Ok(None)
}

fn manifest_in_directory(directory: &Path) -> Result<Option<PathBuf>> {
    record_manifest_probe();
    let manifest = directory.join("Cargo.toml");
    match fs::symlink_metadata(&manifest) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(Some(manifest)),
        Ok(_) => bail!(
            "detected Cargo manifest {} is not a regular file",
            manifest.display()
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("could not inspect {}", manifest.display()))
        }
    }
}

fn validate_regular_manifest(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("could not inspect detected manifest {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!(
            "detected Cargo manifest {} is not a regular file",
            path.display()
        );
    }
    Ok(())
}

pub(super) fn normalized_absolute_path(path: &Path, label: &str) -> Result<PathBuf> {
    let absolute = std::path::absolute(path)
        .with_context(|| format!("could not resolve {label} {}", path.display()))?;
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(component) => normalized.push(component),
            Component::RootDir | Component::Prefix(_) => {
                normalized.push(component.as_os_str());
            }
            Component::ParentDir if normalized.file_name().is_some() => {
                normalized.pop();
            }
            Component::ParentDir => {
                bail!("could not normalize {label} {}", path.display());
            }
        }
    }
    validated_absolute_path(&normalized, label)
}

fn validated_absolute_path(path: &Path, label: &str) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        bail!("Cargo returned invalid {label} path {}", path.display());
    }
    Ok(path.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{
        manifest_probe_count, nearest_manifests_for_rust_sources, reset_manifest_probe_count,
    };

    #[test]
    fn nearest_manifest_lookup_reuses_ancestor_probes_for_many_sources() {
        let root = TempDir::new().expect("temporary directory should be created");
        fs::write(root.path().join("Cargo.toml"), "[workspace]\n")
            .expect("Cargo manifest should be written");
        let source_directory = root.path().join("nested/src");
        fs::create_dir_all(&source_directory).expect("source directory should be created");
        let sources = (0..64).map(|index| source_directory.join(format!("module_{index}.rs")));
        reset_manifest_probe_count();

        let manifests = nearest_manifests_for_rust_sources(sources, root.path())
            .expect("nearest manifests should be discovered");

        assert_eq!(manifests, [root.path().join("Cargo.toml")].into());
        assert_eq!(manifest_probe_count(), 3);
    }
}
