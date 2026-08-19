use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use fuck_ai_comments::{Finding, SourceFile, analyze_all, supports_path};
use ignore::WalkBuilder;

use super::source;

pub(super) struct Report {
    pub(super) findings: Vec<Finding>,
    pub(super) files_scanned: usize,
}

pub(super) fn scan_all(root: &Path) -> Result<Report> {
    let files = supported_files(root)?;
    let mut findings = Vec::new();

    for disk_path in &files {
        let source_path = without_dot_prefix(disk_path);
        let bytes = source::read_regular(disk_path, source_path)?;
        let text = source::utf8(source_path, &bytes)?;
        let mut file_findings = analyze_all(SourceFile {
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
