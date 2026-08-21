use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use fuck_ai_comments::{Finding, SourceFile, supports_path};
use ignore::WalkBuilder;

use super::{cargo_context, git, source};

pub(super) struct Report {
    pub(super) findings: Vec<Finding>,
    pub(super) files_scanned: usize,
}

pub(super) fn scan_all(root: &Path, explicit_config: Option<&Path>) -> Result<Report> {
    let (default_config, config_path) = match explicit_config {
        Some(config_path) => (scan_root_config_path(root), config_path.to_owned()),
        None => {
            let config_path = default_config_path(root)?;
            (config_path.clone(), config_path)
        }
    };
    let files = supported_files(root, [&default_config, &config_path])?;
    let current_directory =
        std::env::current_dir().context("could not resolve current directory")?;
    let cargo_context = cargo_context::discover(root, &current_directory)?;
    let analysis = analysis_with_repository_policy(
        cargo_context.analysis(),
        &config_path,
        explicit_config.is_some(),
    )?;
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
    config_paths: impl IntoIterator<Item = &'path PathBuf>,
) -> Result<Vec<PathBuf>> {
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
        files.push(entry.into_path());
    }
    files.sort();
    Ok(files)
}

fn analysis_with_repository_policy(
    analysis: &fuck_ai_comments::AnalysisContext,
    config_path: &Path,
    required: bool,
) -> Result<fuck_ai_comments::AnalysisContext> {
    match fs::symlink_metadata(config_path) {
        Ok(_) => {
            let bytes = source::read_regular(config_path, config_path)?;
            let text = source::utf8(config_path, &bytes)?;
            analysis
                .clone()
                .with_policy_toml(text)
                .with_context(|| format!("could not load {}", config_path.display()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound && !required => Ok(analysis.clone()),
        Err(error) => {
            Err(error).with_context(|| format!("could not inspect {}", config_path.display()))
        }
    }
}

fn default_config_path(root: &Path) -> Result<PathBuf> {
    if let Some(repository_root) = git::discover_worktree_root(root)? {
        return Ok(repository_root.join("fuck-ai-comments.toml"));
    }
    Ok(scan_root_config_path(root))
}

fn scan_root_config_path(root: &Path) -> PathBuf {
    if root.is_dir() {
        root.join("fuck-ai-comments.toml")
    } else {
        root.parent()
            .unwrap_or_else(|| Path::new("."))
            .join("fuck-ai-comments.toml")
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
