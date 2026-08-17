use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};
use fuck_ai_comments::{SourceFile, analyze_all, analyze_change, supports_path};

use super::check::Report;
use super::source::{self, MAX_SOURCE_BYTES};

// Bounds aggregate blob allocation after every source has passed the shared per-file limit.
const MAX_BATCH_BYTES: u64 = 128 * 1024 * 1024;

pub(super) enum Mode {
    Worktree,
    Staged,
    Commits { base: String, head: Option<String> },
}

pub(super) fn scan(scope: &Path, mode: Mode) -> Result<Report> {
    let repository = Repository::discover(scope)?;
    let changes = repository.changes(mode)?;
    analyze_changes(&repository, changes)
}

struct Repository {
    root: PathBuf,
    scope: PathBuf,
}

impl Repository {
    fn discover(scope: &Path) -> Result<Self> {
        let absolute_scope = std::path::absolute(scope)
            .with_context(|| format!("could not resolve {}", scope.display()))?;
        let probe = existing_directory(&absolute_scope).ok_or_else(|| {
            anyhow::anyhow!("could not find an existing parent of {}", scope.display())
        })?;
        let canonical_probe = fs::canonicalize(probe)
            .with_context(|| format!("could not resolve {}", probe.display()))?;
        let scope_suffix = absolute_scope
            .strip_prefix(probe)
            .context("could not place path relative to its existing parent")?;
        let normalized_scope = canonical_probe.join(scope_suffix);
        let output = run_git_at(
            probe,
            [OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        )?;
        let root_text = one_line(&output, "repository root")?;
        let root = fs::canonicalize(Path::new(root_text))
            .with_context(|| format!("could not resolve repository root {root_text}"))?;
        let relative_scope = normalized_scope.strip_prefix(&root).with_context(|| {
            format!(
                "{} is outside Git repository {}",
                scope.display(),
                root.display()
            )
        })?;
        if relative_scope
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::RootDir))
        {
            bail!(
                "{} is outside Git repository {}",
                scope.display(),
                root.display()
            );
        }
        let scope = if relative_scope.as_os_str().is_empty() {
            PathBuf::from(".")
        } else {
            relative_scope.to_owned()
        };
        Ok(Self { root, scope })
    }

    fn changes(&self, mode: Mode) -> Result<Vec<FileChange>> {
        match mode {
            Mode::Worktree => self.worktree_changes(),
            Mode::Staged => {
                let head = self.verify_revision("HEAD")?;
                self.diff_changes(DiffTarget::Index, &head, None)
            }
            Mode::Commits { base, head } => {
                let base_id = self.verify_revision(&base)?;
                let head_name = head.as_deref().unwrap_or("HEAD");
                let head_id = self.verify_revision(head_name)?;
                let merge_base = self.merge_base(&base_id, &head_id)?;
                self.diff_changes(DiffTarget::Commit, &merge_base, Some(&head_id))
            }
        }
    }

    fn merge_base(&self, base: &ObjectId, head: &ObjectId) -> Result<ObjectId> {
        let output = self
            .git([
                OsStr::new("merge-base"),
                OsStr::new("--all"),
                OsStr::new(base.as_str()),
                OsStr::new(head.as_str()),
            ])
            .context("could not find a merge base")?;
        ObjectId::parse(one_line(&output, "merge base")?)
    }

    fn worktree_changes(&self) -> Result<Vec<FileChange>> {
        let head = self.verify_revision("HEAD")?;
        let mut changes = self.diff_changes(DiffTarget::Worktree, &head, None)?;
        let output = self.git([
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("--full-name"),
            OsStr::new("-z"),
            OsStr::new("--"),
            self.scope.as_os_str(),
        ])?;
        for path in parse_nul_paths(&output)? {
            if let Some(deletion) = changes.iter_mut().find(|change| {
                change.after.is_none() && change.before.path() == Some(path.as_path())
            }) {
                deletion.after = Some(Snapshot::worktree(path));
            } else {
                changes.push(FileChange {
                    before: None,
                    after: Some(Snapshot::worktree(path)),
                });
            }
        }
        changes.sort_by(|left, right| left.sort_path().cmp(right.sort_path()));
        Ok(changes)
    }

    fn diff_changes(
        &self,
        target: DiffTarget,
        before: &ObjectId,
        after: Option<&ObjectId>,
    ) -> Result<Vec<FileChange>> {
        let mut arguments = vec![
            OsString::from("diff"),
            OsString::from("--raw"),
            OsString::from("-z"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-textconv"),
            OsString::from("--find-renames"),
            OsString::from("--abbrev=64"),
        ];
        if target == DiffTarget::Index {
            arguments.push(OsString::from("--cached"));
        }
        arguments.push(OsString::from(before.as_str()));
        if let Some(after) = after {
            arguments.push(OsString::from(after.as_str()));
        }
        arguments.push(OsString::from("--"));
        arguments.push(self.scope.as_os_str().to_owned());

        let output = self.git(arguments.iter().map(OsString::as_os_str))?;
        parse_raw_changes(&output, target)
    }

    fn verify_revision(&self, revision: &str) -> Result<ObjectId> {
        let commit = format!("{revision}^{{commit}}");
        let output = git_output(
            &self.root,
            [
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--end-of-options"),
                OsStr::new(&commit),
            ],
        )?;
        if !output.status.success() {
            bail!(
                "could not resolve revision {revision}: {}",
                stderr_message(&output)
            );
        }
        ObjectId::parse(one_line(&output.stdout, "revision object ID")?)
            .with_context(|| format!("revision {revision} returned an invalid object ID"))
    }

    fn git<'argument>(
        &self,
        arguments: impl IntoIterator<Item = &'argument OsStr>,
    ) -> Result<Vec<u8>> {
        run_git_at(&self.root, arguments)
    }

    fn read_blobs(&self, object_ids: &BTreeSet<ObjectId>) -> Result<BTreeMap<ObjectId, Vec<u8>>> {
        if object_ids.is_empty() {
            return Ok(BTreeMap::new());
        }

        let metadata_output = self.cat_file("--batch-check", object_ids)?;
        let metadata = parse_blob_metadata(&metadata_output, object_ids)?;
        let output = self.cat_file("--batch", object_ids)?;
        parse_blob_batch(&output, object_ids, &metadata)
    }

    fn cat_file(&self, mode: &str, object_ids: &BTreeSet<ObjectId>) -> Result<Vec<u8>> {
        let mut child = Command::new("git")
            .current_dir(&self.root)
            .arg("--literal-pathspecs")
            .args(["cat-file", mode])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("could not run git cat-file {mode}"))?;
        let mut input = child
            .stdin
            .take()
            .context("git cat-file did not provide stdin")?;
        for object_id in object_ids {
            writeln!(input, "{}", object_id.as_str()).context("could not request Git blob")?;
        }
        drop(input);
        let output = child
            .wait_with_output()
            .with_context(|| format!("could not wait for git cat-file {mode}"))?;
        if !output.status.success() {
            bail!("git cat-file {mode} failed: {}", stderr_message(&output));
        }
        Ok(output.stdout)
    }
}

fn existing_directory(path: &Path) -> Option<&Path> {
    let mut candidate = if path.is_dir() { path } else { path.parent()? };
    loop {
        if candidate.is_dir() {
            return Some(candidate);
        }
        candidate = candidate.parent()?;
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DiffTarget {
    Worktree,
    Index,
    Commit,
}

#[derive(Clone)]
struct Snapshot {
    path: PathBuf,
    source: SnapshotSource,
    mode: Option<FileMode>,
}

impl Snapshot {
    fn blob(path: PathBuf, object_id: ObjectId, mode: FileMode) -> Self {
        Self {
            path,
            source: SnapshotSource::Blob(object_id),
            mode: Some(mode),
        }
    }

    fn worktree(path: PathBuf) -> Self {
        Self {
            path,
            source: SnapshotSource::Worktree,
            mode: None,
        }
    }
}

#[derive(Clone)]
enum SnapshotSource {
    Blob(ObjectId),
    Worktree,
}

struct FileChange {
    before: Option<Snapshot>,
    after: Option<Snapshot>,
}

impl FileChange {
    fn sort_path(&self) -> &Path {
        self.after
            .as_ref()
            .or(self.before.as_ref())
            .map_or_else(|| Path::new(""), |snapshot| snapshot.path.as_path())
    }
}

trait SnapshotPath {
    fn path(&self) -> Option<&Path>;
}

impl SnapshotPath for Option<Snapshot> {
    fn path(&self) -> Option<&Path> {
        self.as_ref().map(|snapshot| snapshot.path.as_path())
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum FileMode {
    Missing,
    Regular,
    Symlink,
    Gitlink,
    Other,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ObjectId(String);

impl ObjectId {
    fn parse(text: &str) -> Result<Self> {
        if !matches!(text.len(), 40 | 64) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid Git object ID {text:?}");
        }
        Ok(Self(text.to_ascii_lowercase()))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

fn parse_raw_changes(output: &[u8], target: DiffTarget) -> Result<Vec<FileChange>> {
    let mut cursor = 0;
    let mut changes = Vec::new();
    while cursor < output.len() {
        let header = take_nul(output, &mut cursor, "raw diff header")?;
        let header = std::str::from_utf8(header).context("Git raw diff header was not ASCII")?;
        let fields: Vec<_> = header.split(' ').collect();
        if fields.len() != 5 || !fields[0].starts_with(':') {
            bail!("malformed Git raw diff header {header:?}");
        }
        let old_mode = parse_mode(&fields[0][1..])?;
        let new_mode = parse_mode(fields[1])?;
        let old_id = parse_raw_object_id(fields[2])?;
        let new_id = parse_raw_object_id(fields[3])?;
        let status = parse_status(fields[4])?;
        validate_raw_entry(
            status,
            old_mode,
            new_mode,
            old_id.as_ref(),
            new_id.as_ref(),
            target,
        )?;
        let old_path = parse_git_path(take_nul(output, &mut cursor, "raw diff path")?)?;
        let new_path = if matches!(status, Status::Renamed | Status::Copied) {
            parse_git_path(take_nul(output, &mut cursor, "raw diff destination path")?)?
        } else {
            old_path.clone()
        };

        let before = match status {
            Status::Added | Status::Copied => None,
            Status::Deleted | Status::Modified | Status::Renamed | Status::TypeChanged => {
                Some(Snapshot::blob(
                    old_path,
                    old_id.context("changed entry did not include an old object ID")?,
                    old_mode,
                ))
            }
        };
        let after = match status {
            Status::Deleted => None,
            Status::Added
            | Status::Copied
            | Status::Modified
            | Status::Renamed
            | Status::TypeChanged => Some(match target {
                DiffTarget::Worktree => Snapshot {
                    path: new_path,
                    source: SnapshotSource::Worktree,
                    mode: Some(new_mode),
                },
                DiffTarget::Index | DiffTarget::Commit => Snapshot::blob(
                    new_path,
                    new_id.context("changed entry did not include a new object ID")?,
                    new_mode,
                ),
            }),
        };
        changes.push(FileChange { before, after });
    }
    Ok(changes)
}

#[derive(Clone, Copy)]
enum Status {
    Added,
    Copied,
    Deleted,
    Modified,
    Renamed,
    TypeChanged,
}

fn parse_status(text: &str) -> Result<Status> {
    let bytes = text.as_bytes();
    let Some(code) = bytes.first().copied() else {
        bail!("Git raw diff status was empty");
    };
    let score = &bytes[1..];
    if score.iter().any(|byte| !byte.is_ascii_digit()) {
        bail!("malformed Git raw diff status {text:?}");
    }
    match code {
        b'A' if text.len() == 1 => Ok(Status::Added),
        b'C' if valid_similarity_score(score) => Ok(Status::Copied),
        b'D' if text.len() == 1 => Ok(Status::Deleted),
        b'M' if text.len() == 1 => Ok(Status::Modified),
        b'R' if valid_similarity_score(score) => Ok(Status::Renamed),
        b'T' if text.len() == 1 => Ok(Status::TypeChanged),
        b'U' => bail!("cannot analyze an unmerged Git entry"),
        _ => bail!("unsupported Git raw diff status {text:?}"),
    }
}

fn valid_similarity_score(bytes: &[u8]) -> bool {
    !bytes.is_empty()
        && bytes
            .iter()
            .try_fold(0_u16, |score, digit| {
                score.checked_mul(10)?.checked_add(u16::from(*digit - b'0'))
            })
            .is_some_and(|score| score <= 100)
}

fn validate_raw_entry(
    status: Status,
    old_mode: FileMode,
    new_mode: FileMode,
    old_id: Option<&ObjectId>,
    new_id: Option<&ObjectId>,
    target: DiffTarget,
) -> Result<()> {
    let old_present = old_mode != FileMode::Missing && old_id.is_some();
    let old_missing = old_mode == FileMode::Missing && old_id.is_none();
    let new_present =
        new_mode != FileMode::Missing && (new_id.is_some() || target == DiffTarget::Worktree);
    let new_missing = new_mode == FileMode::Missing && new_id.is_none();
    let valid = match status {
        Status::Added => old_missing && new_present,
        Status::Copied | Status::Modified | Status::Renamed | Status::TypeChanged => {
            old_present && new_present
        }
        Status::Deleted => old_present && new_missing,
    };
    if !valid {
        bail!("Git raw diff modes and object IDs did not match its status");
    }
    Ok(())
}

fn parse_mode(text: &str) -> Result<FileMode> {
    if text.len() != 6 || !text.bytes().all(|byte| matches!(byte, b'0'..=b'7')) {
        bail!("malformed Git file mode {text:?}");
    }
    Ok(match text {
        "000000" => FileMode::Missing,
        "100644" | "100755" => FileMode::Regular,
        "120000" => FileMode::Symlink,
        "160000" => FileMode::Gitlink,
        _ => FileMode::Other,
    })
}

fn parse_raw_object_id(text: &str) -> Result<Option<ObjectId>> {
    if text.bytes().all(|byte| byte == b'0') {
        if matches!(text.len(), 40 | 64) {
            return Ok(None);
        }
        bail!("malformed zero Git object ID");
    }
    ObjectId::parse(text).map(Some)
}

fn parse_nul_paths(output: &[u8]) -> Result<Vec<PathBuf>> {
    let mut cursor = 0;
    let mut paths = Vec::new();
    while cursor < output.len() {
        paths.push(parse_git_path(take_nul(
            output,
            &mut cursor,
            "Git path record",
        )?)?);
    }
    Ok(paths)
}

fn take_nul<'output>(
    output: &'output [u8],
    cursor: &mut usize,
    label: &str,
) -> Result<&'output [u8]> {
    let tail = output
        .get(*cursor..)
        .context("Git record cursor exceeded output")?;
    let length = tail
        .iter()
        .position(|byte| *byte == 0)
        .with_context(|| format!("unterminated {label}"))?;
    let record = &tail[..length];
    *cursor += length + 1;
    Ok(record)
}

fn parse_git_path(bytes: &[u8]) -> Result<PathBuf> {
    if bytes.is_empty() {
        bail!("Git returned an empty path");
    }
    let path = bytes_to_path(bytes)?;
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("Git returned unsafe path {}", path.display());
    }
    Ok(path)
}

#[cfg(unix)]
fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStringExt;

    Ok(PathBuf::from(OsString::from_vec(bytes.to_owned())))
}

#[cfg(not(unix))]
fn bytes_to_path(bytes: &[u8]) -> Result<PathBuf> {
    let text = std::str::from_utf8(bytes).context("Git returned a non-UTF-8 path")?;
    Ok(PathBuf::from(text))
}

fn analyze_changes(repository: &Repository, changes: Vec<FileChange>) -> Result<Report> {
    let mut plans = Vec::new();
    for change in changes {
        validate_modes(&change)?;
        let Some(after) = change.after else {
            continue;
        };
        if !supports_path(&after.path) {
            continue;
        }
        if let Some(before) = change.before.filter(|before| supports_path(&before.path)) {
            plans.push(Plan::Change { before, after });
        } else {
            plans.push(Plan::All(after));
        }
    }

    let object_ids = plans.iter().flat_map(Plan::object_ids).cloned().collect();
    let blobs = repository.read_blobs(&object_ids)?;
    let mut findings = Vec::new();
    for plan in &plans {
        let mut plan_findings = analyze_plan(repository, plan, &blobs)?;
        findings.append(&mut plan_findings);
    }
    findings.sort();
    findings.dedup();
    Ok(Report {
        findings,
        files_scanned: plans.len(),
    })
}

enum Plan {
    All(Snapshot),
    Change { before: Snapshot, after: Snapshot },
}

impl Plan {
    fn object_ids(&self) -> impl Iterator<Item = &ObjectId> {
        let (before, after) = match self {
            Self::All(after) => (None, after),
            Self::Change { before, after } => (Some(before), after),
        };
        before
            .into_iter()
            .chain(std::iter::once(after))
            .filter_map(|snapshot| match &snapshot.source {
                SnapshotSource::Blob(object_id) => Some(object_id),
                SnapshotSource::Worktree => None,
            })
    }
}

fn validate_modes(change: &FileChange) -> Result<()> {
    for snapshot in change.before.iter().chain(change.after.iter()) {
        if supports_path(&snapshot.path)
            && snapshot.mode.is_some_and(|mode| mode != FileMode::Regular)
        {
            bail!(
                "supported path {} is not a regular file",
                snapshot.path.display()
            );
        }
    }
    Ok(())
}

fn analyze_plan(
    repository: &Repository,
    plan: &Plan,
    blobs: &BTreeMap<ObjectId, Vec<u8>>,
) -> Result<Vec<fuck_ai_comments::Finding>> {
    match plan {
        Plan::All(after) => {
            let bytes = read_snapshot(repository, after, blobs)?;
            let text = source::utf8(&after.path, &bytes)?;
            analyze_all(SourceFile {
                path: &after.path,
                text,
            })
            .with_context(|| format!("could not analyze {}", after.path.display()))
        }
        Plan::Change { before, after } => {
            let before_bytes = read_snapshot(repository, before, blobs)?;
            let after_bytes = read_snapshot(repository, after, blobs)?;
            let before_text = source::utf8(&before.path, &before_bytes)?;
            let after_text = source::utf8(&after.path, &after_bytes)?;
            analyze_change(
                SourceFile {
                    path: &before.path,
                    text: before_text,
                },
                SourceFile {
                    path: &after.path,
                    text: after_text,
                },
            )
            .with_context(|| {
                format!(
                    "could not analyze change {} -> {}",
                    before.path.display(),
                    after.path.display()
                )
            })
        }
    }
}

fn read_snapshot<'content>(
    repository: &Repository,
    snapshot: &Snapshot,
    blobs: &'content BTreeMap<ObjectId, Vec<u8>>,
) -> Result<Cow<'content, [u8]>> {
    match &snapshot.source {
        SnapshotSource::Blob(object_id) => blobs
            .get(object_id)
            .map(|bytes| Cow::Borrowed(bytes.as_slice()))
            .with_context(|| format!("Git blob {} was not returned", object_id.as_str())),
        SnapshotSource::Worktree => {
            let disk_path = repository.root.join(&snapshot.path);
            source::read_regular(&disk_path, &snapshot.path).map(Cow::Owned)
        }
    }
}

fn parse_blob_batch(
    output: &[u8],
    requested: &BTreeSet<ObjectId>,
    metadata: &BTreeMap<ObjectId, usize>,
) -> Result<BTreeMap<ObjectId, Vec<u8>>> {
    let mut cursor = 0;
    let mut blobs = BTreeMap::new();
    for object_id in requested {
        let tail = output
            .get(cursor..)
            .context("Git blob batch ended before its header")?;
        let header_length = tail
            .iter()
            .position(|byte| *byte == b'\n')
            .context("Git blob batch header was unterminated")?;
        let header = std::str::from_utf8(&tail[..header_length])
            .context("Git blob batch header was not ASCII")?;
        cursor += header_length + 1;
        let mut fields = header.split(' ');
        let returned_id = fields.next().context("Git blob header had no object ID")?;
        let object_type = fields
            .next()
            .context("Git blob header had no object type")?;
        let size = fields.next().context("Git blob header had no size")?;
        if fields.next().is_some() || returned_id != object_id.as_str() || object_type != "blob" {
            bail!("unexpected Git blob header {header:?}");
        }
        let size: usize = size
            .parse()
            .with_context(|| format!("invalid Git blob size {size:?}"))?;
        if metadata.get(object_id) != Some(&size) {
            bail!("Git blob size changed between batch-check and batch");
        }
        let end = cursor
            .checked_add(size)
            .context("Git blob size overflowed address space")?;
        let bytes = output
            .get(cursor..end)
            .context("Git blob batch ended inside a blob")?;
        if output.get(end) != Some(&b'\n') {
            bail!("Git blob batch did not terminate a blob");
        }
        blobs.insert(object_id.clone(), bytes.to_owned());
        cursor = end + 1;
    }
    if cursor != output.len() {
        bail!("Git blob batch returned unrequested data");
    }
    Ok(blobs)
}

fn parse_blob_metadata(
    output: &[u8],
    requested: &BTreeSet<ObjectId>,
) -> Result<BTreeMap<ObjectId, usize>> {
    let mut cursor = 0;
    let mut total = 0_u64;
    let mut metadata = BTreeMap::new();
    for object_id in requested {
        let header = take_line(output, &mut cursor, "Git blob metadata")?;
        let header = std::str::from_utf8(header).context("Git blob metadata was not ASCII")?;
        let mut fields = header.split(' ');
        let returned_id = fields
            .next()
            .context("Git blob metadata had no object ID")?;
        let object_type = fields
            .next()
            .context("Git blob metadata had no object type")?;
        let size = fields.next().context("Git blob metadata had no size")?;
        if fields.next().is_some() || returned_id != object_id.as_str() || object_type != "blob" {
            bail!("unexpected Git blob metadata {header:?}");
        }
        let size: u64 = size
            .parse()
            .with_context(|| format!("invalid Git blob size {size:?}"))?;
        if size > MAX_SOURCE_BYTES {
            bail!(
                "Git blob {} is {size} bytes; supported source limit is {MAX_SOURCE_BYTES} bytes",
                object_id.as_str()
            );
        }
        total = total
            .checked_add(size)
            .context("Git blob batch size overflowed")?;
        if total > MAX_BATCH_BYTES {
            bail!("Git blob batch is {total} bytes; batch limit is {MAX_BATCH_BYTES} bytes");
        }
        let size = usize::try_from(size).context("Git blob size exceeded this platform")?;
        metadata.insert(object_id.clone(), size);
    }
    if cursor != output.len() {
        bail!("Git blob metadata returned unrequested data");
    }
    Ok(metadata)
}

fn take_line<'output>(
    output: &'output [u8],
    cursor: &mut usize,
    label: &str,
) -> Result<&'output [u8]> {
    let tail = output
        .get(*cursor..)
        .with_context(|| format!("{label} cursor exceeded output"))?;
    let length = tail
        .iter()
        .position(|byte| *byte == b'\n')
        .with_context(|| format!("unterminated {label}"))?;
    let record = &tail[..length];
    *cursor += length + 1;
    Ok(record)
}

fn run_git_at<'argument>(
    directory: &Path,
    arguments: impl IntoIterator<Item = &'argument OsStr>,
) -> Result<Vec<u8>> {
    let output = git_output(directory, arguments)?;
    if !output.status.success() {
        bail!("Git command failed: {}", stderr_message(&output));
    }
    Ok(output.stdout)
}

fn git_output<'argument>(
    directory: &Path,
    arguments: impl IntoIterator<Item = &'argument OsStr>,
) -> Result<Output> {
    Command::new("git")
        .current_dir(directory)
        .arg("--literal-pathspecs")
        .args(arguments)
        .output()
        .context("could not run Git")
}

fn one_line<'output>(output: &'output [u8], label: &str) -> Result<&'output str> {
    let text = std::str::from_utf8(output).with_context(|| format!("{label} was not UTF-8"))?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    let line = line.strip_suffix('\r').unwrap_or(line);
    if line.is_empty() || line.contains(['\n', '\r', '\0']) {
        bail!("{label} was not one line");
    }
    Ok(line)
}

fn stderr_message(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{DiffTarget, ObjectId, parse_blob_batch, parse_raw_changes};

    const OLD_ID: &str = "1111111111111111111111111111111111111111";
    const NEW_ID: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn raw_parser_rejects_inconsistent_status_metadata() {
        let raw = format!(":100644 100644 {OLD_ID} {NEW_ID} A\0file.rs\0");

        assert!(parse_raw_changes(raw.as_bytes(), DiffTarget::Commit).is_err());
    }

    #[test]
    fn raw_parser_rejects_invalid_rename_scores() {
        let raw = format!(":100644 100644 {OLD_ID} {NEW_ID} R101\0old.rs\0new.rs\0");

        assert!(parse_raw_changes(raw.as_bytes(), DiffTarget::Commit).is_err());
    }

    #[test]
    fn raw_parser_rejects_non_ascii_status_without_panicking() {
        let raw = format!(":100644 100644 {OLD_ID} {NEW_ID} 雪\0file.rs\0");

        assert!(parse_raw_changes(raw.as_bytes(), DiffTarget::Commit).is_err());
    }

    #[test]
    fn blob_batch_parser_preserves_arbitrary_blob_bytes() {
        let object_id = ObjectId::parse(OLD_ID).expect("object ID should be valid");
        let requested = BTreeSet::from([object_id.clone()]);
        let mut output = format!("{OLD_ID} blob 4\n").into_bytes();
        output.extend([0, b'\n', 0xff, b'x', b'\n']);
        let metadata = [(object_id.clone(), 4)].into_iter().collect();

        let blobs = parse_blob_batch(&output, &requested, &metadata).expect("batch should parse");

        assert_eq!(blobs.get(&object_id), Some(&vec![0, b'\n', 0xff, b'x']));
    }
}
