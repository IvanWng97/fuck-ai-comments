use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use fuck_ai_comments::AnalysisContext;
use serde::Deserialize;

#[cfg(test)]
thread_local! {
    static CARGO_METADATA_DISCOVERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_discovery() {
    CARGO_METADATA_DISCOVERIES.with(|count| count.set(count.get() + 1));
}

#[cfg(not(test))]
fn record_discovery() {}

#[cfg(test)]
pub(super) fn reset_discovery_count() {
    CARGO_METADATA_DISCOVERIES.with(|count| count.set(0));
}

#[cfg(test)]
pub(super) fn cargo_metadata_discoveries() -> usize {
    CARGO_METADATA_DISCOVERIES.with(std::cell::Cell::get)
}

#[derive(Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoPackage>,
    workspace_root: PathBuf,
    version: u32,
}

#[derive(Deserialize)]
struct CargoPackage {
    targets: Vec<CargoTarget>,
}

#[derive(Deserialize)]
struct CargoTarget {
    kind: Vec<String>,
    src_path: PathBuf,
}

pub(super) fn discover(start: &Path, analysis_base: &Path) -> Result<AnalysisContext> {
    let Some(manifest) = nearest_manifest(start)? else {
        return Ok(AnalysisContext::default());
    };
    record_discovery();
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let output = Command::new(cargo)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(&manifest)
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
    let metadata: CargoMetadata = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("Cargo returned invalid metadata for {}", manifest.display()))?;
    if metadata.version != 1 {
        bail!(
            "Cargo returned unsupported metadata version {}",
            metadata.version
        );
    }

    let workspace_root = validated_absolute_path(&metadata.workspace_root, "workspace root")?;
    let analysis_base = std::path::absolute(analysis_base).with_context(|| {
        format!(
            "could not resolve analysis base {}",
            analysis_base.display()
        )
    })?;
    let mut roots = BTreeSet::new();
    for target in metadata
        .packages
        .into_iter()
        .flat_map(|package| package.targets)
        .filter(CargoTarget::is_library)
    {
        let source = validated_absolute_path(&target.src_path, "library target")?;
        roots.insert(source.clone());
        if let Ok(relative) = source.strip_prefix(&workspace_root) {
            roots.insert(relative.to_owned());
        }
        if let Ok(relative) = source.strip_prefix(&analysis_base) {
            roots.insert(relative.to_owned());
        }
    }
    AnalysisContext::from_rust_library_roots(roots).map_err(Into::into)
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

fn nearest_manifest(start: &Path) -> Result<Option<PathBuf>> {
    let absolute = std::path::absolute(start)
        .with_context(|| format!("could not resolve {}", start.display()))?;
    let metadata = fs::symlink_metadata(&absolute)
        .with_context(|| format!("could not inspect {}", start.display()))?;
    let directory = if metadata.file_type().is_dir() {
        absolute.as_path()
    } else {
        absolute
            .parent()
            .with_context(|| format!("{} has no parent directory", start.display()))?
    };
    for ancestor in directory.ancestors() {
        let manifest = ancestor.join("Cargo.toml");
        match fs::symlink_metadata(&manifest) {
            Ok(metadata) if metadata.file_type().is_file() => return Ok(Some(manifest)),
            Ok(_) => bail!(
                "detected Cargo manifest {} is not a regular file",
                manifest.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not inspect {}", manifest.display()));
            }
        }
    }
    Ok(None)
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
