use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use fuck_ai_comments::{Finding, RepositoryConfig, SourceFile, supports_path};
use ignore::WalkBuilder;

use super::{cargo_context, git, source};

pub(super) struct Report {
    pub(super) findings: Vec<Finding>,
    pub(super) files_scanned: usize,
}

pub(super) fn scan_all(root: &Path, explicit_config: Option<&Path>) -> Result<Report> {
    let (default_config, config_source) = match explicit_config {
        Some(config_path) => (
            scan_root_config_path(root),
            ConfigSource {
                path: config_path.to_owned(),
                authority_root: scan_root(root).to_owned(),
            },
        ),
        None => {
            let config_source = default_config_source(root)?;
            (config_source.path.clone(), config_source)
        }
    };
    let config = load_repository_config(&config_source.path, explicit_config.is_some())?;
    let files = supported_files(
        root,
        &config_source.authority_root,
        [&default_config, &config_source.path],
        &config,
    )?;
    let current_directory =
        std::env::current_dir().context("could not resolve current directory")?;
    let cargo_context = cargo_context::discover(root, &current_directory)?;
    let analysis = cargo_context
        .analysis()
        .clone()
        .with_repository_config(&config);
    let mut findings = Vec::new();

    for disk_path in &files {
        let source_path = without_dot_prefix(disk_path);
        let analysis_path = if source_path
            .components()
            .any(|component| component == Component::ParentDir)
        {
            Cow::Owned(cargo_context::normalized_absolute_path(
                source_path,
                "source path",
            )?)
        } else {
            Cow::Borrowed(source_path)
        };
        let bytes = source::read_regular(disk_path, source_path)?;
        let text = source::utf8(source_path, &bytes)?;
        let mut file_findings = analysis
            .analyze_all(SourceFile {
                path: &analysis_path,
                text,
            })
            .with_context(|| format!("could not analyze {}", source_path.display()))?;
        findings.append(&mut file_findings);
    }
    findings.sort();

    Ok(Report {
        findings,
        files_scanned: files.len(),
    })
}

fn supported_files<'path>(
    root: &Path,
    authority_root: &Path,
    config_paths: impl IntoIterator<Item = &'path PathBuf>,
    config: &RepositoryConfig,
) -> Result<Vec<PathBuf>> {
    let exclusion_coordinates = ExclusionCoordinates::new(root, authority_root)?;
    let absolute_configs: BTreeSet<_> = config_paths
        .into_iter()
        .map(|path| cargo_context::normalized_absolute_path(path, "policy config"))
        .collect::<Result<_>>()?;
    let mut builder = WalkBuilder::new(root);
    builder
        .standard_filters(true)
        .hidden(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git");

    let mut files = Vec::new();
    for entry in builder.build() {
        let entry = entry.with_context(|| format!("could not walk {}", root.display()))?;
        if let Some(error) = entry.error() {
            bail!(
                "could not apply ignore rules at {}: {error}",
                entry.path().display()
            );
        }
        if !supports_path(entry.path()) {
            continue;
        }
        let absolute_entry = cargo_context::normalized_absolute_path(entry.path(), "source path")?;
        if absolute_configs.contains(&absolute_entry) {
            continue;
        }
        let relative_entry = exclusion_coordinates.relative_path(&absolute_entry)?;
        if config.excludes_path(&relative_entry, false) {
            continue;
        }
        files.push(entry.into_path());
    }
    files.sort();
    Ok(files)
}

struct ExclusionCoordinates {
    lexical_scan_root: PathBuf,
    authority_relative_scan_root: PathBuf,
}

impl ExclusionCoordinates {
    fn new(root: &Path, authority_root: &Path) -> Result<Self> {
        let lexical_scan_root =
            cargo_context::normalized_absolute_path(scan_root(root), "scan root")?;
        let canonical_scan_root = fs::canonicalize(scan_root(root)).with_context(|| {
            format!("could not resolve scan root {}", scan_root(root).display())
        })?;
        let canonical_authority = fs::canonicalize(authority_root).with_context(|| {
            format!(
                "could not resolve config authority {}",
                authority_root.display()
            )
        })?;
        let authority_relative_scan_root = canonical_scan_root
            .strip_prefix(&canonical_authority)
            .with_context(|| {
                format!(
                    "scan root {} is outside config authority {}",
                    scan_root(root).display(),
                    authority_root.display()
                )
            })?
            .to_owned();
        Ok(Self {
            lexical_scan_root,
            authority_relative_scan_root,
        })
    }

    fn relative_path<'path>(&self, absolute_path: &'path Path) -> Result<Cow<'path, Path>> {
        let scan_relative = absolute_path
            .strip_prefix(&self.lexical_scan_root)
            .context("source path is outside the scan root")?;
        if self.authority_relative_scan_root.as_os_str().is_empty() {
            return Ok(Cow::Borrowed(scan_relative));
        }
        Ok(Cow::Owned(
            self.authority_relative_scan_root.join(scan_relative),
        ))
    }
}

fn load_repository_config(config_path: &Path, required: bool) -> Result<RepositoryConfig> {
    match fs::symlink_metadata(config_path) {
        Ok(_) => {
            let bytes = source::read_regular(config_path, config_path)?;
            let text = source::utf8(config_path, &bytes)?;
            let config = RepositoryConfig::from_toml(text)
                .with_context(|| format!("could not load {}", config_path.display()))?;
            Ok(config)
        }
        Err(error) if error.kind() == ErrorKind::NotFound && !required => {
            Ok(RepositoryConfig::default())
        }
        Err(error) => {
            Err(error).with_context(|| format!("could not inspect {}", config_path.display()))
        }
    }
}

struct ConfigSource {
    path: PathBuf,
    authority_root: PathBuf,
}

fn default_config_source(root: &Path) -> Result<ConfigSource> {
    if let Some(repository_root) = git::discover_worktree_root(root)? {
        return Ok(ConfigSource {
            path: repository_root.join("fuck-ai-comments.toml"),
            authority_root: repository_root,
        });
    }
    let authority_root = scan_root(root).to_owned();
    Ok(ConfigSource {
        path: authority_root.join("fuck-ai-comments.toml"),
        authority_root,
    })
}

fn scan_root_config_path(root: &Path) -> PathBuf {
    scan_root(root).join("fuck-ai-comments.toml")
}

fn scan_root(root: &Path) -> &Path {
    if root.is_dir() {
        root
    } else {
        root.parent().unwrap_or_else(|| Path::new("."))
    }
}

fn without_dot_prefix(path: &Path) -> &Path {
    path.strip_prefix(".").unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::super::cargo_context::{cargo_metadata_discoveries, reset_discovery_count};
    use super::scan_all;

    #[test]
    fn scan_all_discovers_cargo_targets_once_for_many_files() {
        let root = TempDir::new().expect("temporary directory should be created");
        fs::write(
            root.path().join("Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"discovery-count\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
            ),
        )
        .expect("Cargo.toml should be written");
        fs::create_dir(root.path().join("src")).expect("source directory should be created");
        fs::write(root.path().join("src/lib.rs"), "pub fn work() {}\n")
            .expect("library root should be written");
        for index in 0..8 {
            fs::write(
                root.path().join(format!("src/module_{index}.rs")),
                format!("pub const VALUE_{index}: usize = {index};\n"),
            )
            .expect("module should be written");
        }
        reset_discovery_count();

        let report = scan_all(root.path(), None).expect("the Cargo project should scan cleanly");

        assert_eq!(report.files_scanned, 10);
        assert_eq!(cargo_metadata_discoveries(), 1);
    }

    #[test]
    fn scan_all_queries_one_workspace_once_including_member_manifests() {
        let root = TempDir::new().expect("temporary directory should be created");
        fs::write(
            root.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"member\"]\nresolver = \"3\"\n",
        )
        .expect("workspace Cargo.toml should be written");
        fs::create_dir_all(root.path().join("member/custom"))
            .expect("member source directory should be created");
        fs::write(
            root.path().join("member/Cargo.toml"),
            concat!(
                "[package]\n",
                "name = \"discovery-member\"\n",
                "version = \"0.1.0\"\n",
                "edition = \"2024\"\n",
                "\n",
                "[lib]\n",
                "path = \"custom/root.rs\"\n",
            ),
        )
        .expect("member Cargo.toml should be written");
        fs::write(
            root.path().join("member/custom/root.rs"),
            "pub fn work() {}\n",
        )
        .expect("member root should be written");
        for index in 0..8 {
            fs::write(
                root.path().join(format!("member/custom/module_{index}.rs")),
                format!("pub const VALUE_{index}: usize = {index};\n"),
            )
            .expect("module should be written");
        }
        reset_discovery_count();

        let report = scan_all(root.path(), None).expect("the Cargo workspace should scan cleanly");

        assert_eq!(report.files_scanned, 11);
        assert_eq!(cargo_metadata_discoveries(), 1);
    }
}
