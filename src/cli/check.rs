use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fuck_ai_comments::{Finding, SourceFile, supports_path};
use ignore::WalkBuilder;

use super::{cargo_context, source};

pub(super) struct Report {
    pub(super) findings: Vec<Finding>,
    pub(super) files_scanned: usize,
}

pub(super) fn scan_all(root: &Path) -> Result<Report> {
    let files = supported_files(root)?;
    let current_directory =
        std::env::current_dir().context("could not resolve current directory")?;
    let context = cargo_context::discover(root, &current_directory)?;
    let mut findings = Vec::new();

    for disk_path in &files {
        let source_path = without_dot_prefix(disk_path);
        let bytes = source::read_regular(disk_path, source_path)?;
        let text = source::utf8(source_path, &bytes)?;
        let mut file_findings = context
            .analyze_all(SourceFile {
                path: source_path,
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

fn supported_files(root: &Path) -> Result<Vec<PathBuf>> {
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
        files.push(entry.into_path());
    }
    files.sort();
    Ok(files)
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

        let report = scan_all(root.path()).expect("the Cargo project should scan cleanly");

        assert_eq!(report.files_scanned, 10);
        assert_eq!(cargo_metadata_discoveries(), 1);
    }
}
