use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use anyhow::{Context, Result, bail};
use fuck_ai_comments::{AnalysisContext, AnalysisProfile, SourceFile, supports_path};

use super::cargo_context;
use super::check::Report;
use super::source::{self, MAX_SOURCE_BYTES};

// Bounds aggregate blob allocation after every source has passed the shared per-file limit.
const MAX_BATCH_BYTES: u64 = 128 * 1024 * 1024;
const GIT_OBJECT_ID_HEX_LENGTHS: [usize; 2] = [40, 64];
const MAX_GIT_SIMILARITY_SCORE: u16 = 100;
// Git's default keeps its exhaustive O(N^2) rename fallback finite.
const GIT_EXHAUSTIVE_RENAME_LIMIT: usize = 1_000;
const POLICY_CONFIG_PATH: &str = "fuck-ai-comments.toml";

pub(super) enum Mode {
    Worktree,
    Staged,
    Commits { base: String, head: Option<String> },
}

enum HeadState {
    Commit(ObjectId),
    Unborn,
}

enum DiffRequest<'revision> {
    Worktree {
        head: &'revision ObjectId,
    },
    Index {
        head: Option<&'revision ObjectId>,
    },
    Commits {
        base: &'revision ObjectId,
        head: &'revision ObjectId,
    },
}

pub(super) fn scan(
    scope: &Path,
    mode: Mode,
    profile: AnalysisProfile,
    explicit_config: Option<&Path>,
) -> Result<Report> {
    let repository = Repository::discover(scope)?;
    let changes = repository.changes(mode)?;
    if !changes.scope_exists {
        bail!("scope {} does not exist", scope.display());
    }
    let CargoDiscoverySeeds {
        manifests,
        rust_sources,
    } = repository.cargo_discovery_seeds_for_authority(&changes.cargo_authority)?;
    let mut required_manifests: BTreeSet<_> = manifests
        .into_iter()
        .map(|manifest| repository.root.join(manifest))
        .collect();
    required_manifests.extend(
        changes
            .files
            .iter()
            .filter_map(|change| change.after.as_ref())
            .filter(|snapshot| is_cargo_manifest_path(&snapshot.path))
            .map(|snapshot| repository.root.join(&snapshot.path)),
    );
    let authoritative_sources = rust_sources
        .into_iter()
        .map(|source| repository.root.join(source));
    let changed_sources = changes
        .files
        .iter()
        .flat_map(|change| change.before.iter().chain(change.after.iter()))
        .map(|snapshot| repository.root.join(&snapshot.path));
    required_manifests.extend(cargo_context::nearest_manifests_for_rust_sources(
        authoritative_sources.chain(changed_sources),
        &repository.root,
    )?);
    let cargo_context = cargo_context::discover_with_manifests(
        &repository.root,
        &repository.root,
        required_manifests,
    )?;
    repository.validate_cargo_context(&cargo_context, &changes.cargo_authority)?;
    let policy = repository.load_policy(
        cargo_context.analysis(),
        &changes.cargo_authority,
        explicit_config,
    )?;
    let full_static_snapshots = (policy.changed && profile == AnalysisProfile::Full)
        .then(|| repository.snapshots_for_authority(&changes.cargo_authority))
        .transpose()?;
    analyze_changes(
        &repository,
        &policy.analysis,
        changes.files,
        profile,
        &policy.excluded_paths,
        full_static_snapshots,
    )
}

struct LoadedPolicy {
    analysis: AnalysisContext,
    changed: bool,
    excluded_paths: BTreeSet<PathBuf>,
}

struct ScopeChanges {
    files: Vec<FileChange>,
    scope_exists: bool,
    cargo_authority: CargoAuthority,
}

struct CargoDiscoverySeeds {
    manifests: BTreeSet<PathBuf>,
    rust_sources: BTreeSet<PathBuf>,
}

enum CargoAuthority {
    Worktree { before: Option<ObjectId> },
    Index { before: Option<ObjectId> },
    Commits { before: ObjectId, after: ObjectId },
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

    fn changes(&self, mode: Mode) -> Result<ScopeChanges> {
        match mode {
            Mode::Worktree => match self.head_state()? {
                HeadState::Commit(head) => {
                    let in_head = self.scope_exists_in_tree(&head)?;
                    let in_worktree = self.scope_exists_in_worktree()?;
                    Ok(ScopeChanges {
                        files: self.worktree_changes(&head)?,
                        scope_exists: in_head || in_worktree,
                        cargo_authority: CargoAuthority::Worktree { before: Some(head) },
                    })
                }
                HeadState::Unborn => {
                    let in_index = self.scope_exists_in_index()?;
                    let in_worktree = self.scope_exists_in_worktree()?;
                    Ok(ScopeChanges {
                        files: self.unborn_worktree_changes()?,
                        scope_exists: in_index || in_worktree,
                        cargo_authority: CargoAuthority::Worktree { before: None },
                    })
                }
            },
            Mode::Staged => match self.head_state()? {
                HeadState::Commit(head) => {
                    let in_head = self.scope_exists_in_tree(&head)?;
                    let in_index = self.scope_exists_in_index()?;
                    Ok(ScopeChanges {
                        files: self.diff_changes(DiffRequest::Index { head: Some(&head) })?,
                        scope_exists: in_head || in_index,
                        cargo_authority: CargoAuthority::Index { before: Some(head) },
                    })
                }
                HeadState::Unborn => {
                    let in_index = self.scope_exists_in_index()?;
                    Ok(ScopeChanges {
                        files: self.diff_changes(DiffRequest::Index { head: None })?,
                        scope_exists: in_index,
                        cargo_authority: CargoAuthority::Index { before: None },
                    })
                }
            },
            Mode::Commits { base, head } => {
                let base_id = self.verify_revision(&base)?;
                let head_name = head.as_deref().unwrap_or("HEAD");
                let head_id = self.verify_revision(head_name)?;
                let merge_base = self.merge_base(&base_id, &head_id)?;
                let in_merge_base = self.scope_exists_in_tree(&merge_base)?;
                let in_head = self.scope_exists_in_tree(&head_id)?;
                Ok(ScopeChanges {
                    files: self.diff_changes(DiffRequest::Commits {
                        base: &merge_base,
                        head: &head_id,
                    })?,
                    scope_exists: in_merge_base || in_head,
                    cargo_authority: CargoAuthority::Commits {
                        before: merge_base,
                        after: head_id,
                    },
                })
            }
        }
    }

    fn validate_cargo_context(
        &self,
        context: &cargo_context::CargoContext,
        authority: &CargoAuthority,
    ) -> Result<()> {
        match authority {
            CargoAuthority::Worktree { before: None } => Ok(()),
            CargoAuthority::Worktree {
                before: Some(before),
            } => {
                self.require_unchanged_cargo_inputs(
                    context,
                    &[before.as_str()],
                    "HEAD and the worktree",
                )?;
                let tracked_paths = self.tracked_paths_in_tree(before)?;
                self.require_worktree_implicit_inputs_match(
                    context,
                    &tracked_paths,
                    "HEAD and the worktree",
                )?;
                let tracked = cargo_manifest_paths(&tracked_paths)?;
                self.require_matching_cargo_manifests(context, &tracked, "HEAD")
            }
            CargoAuthority::Index { before } => {
                self.require_unchanged_cargo_inputs(context, &[], "the index and the worktree")?;
                let tracked_paths = self.tracked_paths_in_index()?;
                self.require_worktree_implicit_inputs_match(
                    context,
                    &tracked_paths,
                    "the index and the worktree",
                )?;
                let tracked = cargo_manifest_paths(&tracked_paths)?;
                self.require_matching_cargo_manifests(context, &tracked, "the index")?;
                if let Some(before) = before {
                    self.require_unchanged_cargo_inputs(
                        context,
                        &["--cached", before.as_str()],
                        "HEAD and the index",
                    )?;
                }
                Ok(())
            }
            CargoAuthority::Commits { before, after } => {
                self.require_unchanged_cargo_inputs(
                    context,
                    &[before.as_str(), after.as_str()],
                    "the compared commits",
                )?;
                self.require_unchanged_cargo_inputs(
                    context,
                    &[after.as_str()],
                    "the head commit and the worktree",
                )?;
                let tracked_paths = self.tracked_paths_in_tree(after)?;
                self.require_worktree_implicit_inputs_match(
                    context,
                    &tracked_paths,
                    "the head commit and the worktree",
                )?;
                let tracked = cargo_manifest_paths(&tracked_paths)?;
                self.require_matching_cargo_manifests(context, &tracked, "the head commit")
            }
        }
    }

    fn cargo_discovery_seeds_for_authority(
        &self,
        authority: &CargoAuthority,
    ) -> Result<CargoDiscoverySeeds> {
        let paths = match authority {
            CargoAuthority::Worktree {
                before: Some(before),
            } => self.tracked_paths_in_tree(before),
            CargoAuthority::Worktree { before: None } | CargoAuthority::Index { .. } => {
                self.tracked_paths_in_index()
            }
            CargoAuthority::Commits { after, .. } => self.tracked_paths_in_tree(after),
        }?;
        cargo_discovery_seeds(&paths)
    }

    fn require_unchanged_cargo_inputs(
        &self,
        context: &cargo_context::CargoContext,
        arguments: &[&str],
        snapshots: &str,
    ) -> Result<()> {
        let manifest_diff = self.cargo_diff(
            "--glob-pathspecs",
            arguments,
            &[],
            ["Cargo.toml", "**/Cargo.toml"],
        )?;
        require_clean_cargo_diff(&manifest_diff, snapshots)?;

        let implicit_inputs = self.relative_implicit_inputs(context)?;
        if implicit_inputs.is_empty() {
            return Ok(());
        }
        let implicit_diff = self.cargo_diff(
            "--literal-pathspecs",
            arguments,
            &["--diff-filter=ADRTUXB"],
            implicit_inputs,
        )?;
        require_clean_cargo_diff(&implicit_diff, snapshots)
    }

    fn require_worktree_implicit_inputs_match(
        &self,
        context: &cargo_context::CargoContext,
        tracked_paths: &[u8],
        snapshots: &str,
    ) -> Result<()> {
        let implicit_inputs = self.relative_implicit_inputs(context)?;
        if implicit_inputs.is_empty() {
            return Ok(());
        }
        let tracked: BTreeSet<_> = parse_nul_paths(tracked_paths)?.into_iter().collect();
        for input in implicit_inputs {
            let live = match fs::symlink_metadata(self.root.join(&input)) {
                Ok(_) => true,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!(
                            "could not inspect implicit library input {}",
                            input.display()
                        )
                    });
                }
            };
            if live != tracked.contains(&input) {
                bail!(
                    "Cargo target roles cannot be proven because Cargo target inputs differ between {snapshots}"
                );
            }
        }
        Ok(())
    }

    fn relative_implicit_inputs(
        &self,
        context: &cargo_context::CargoContext,
    ) -> Result<Vec<PathBuf>> {
        context
            .implicit_library_inputs()
            .map(|path| {
                path.strip_prefix(&self.root)
                    .map(Path::to_owned)
                    .with_context(|| {
                        format!(
                            "Cargo target roles cannot be proven because implicit library input {} is outside Git repository {}",
                            path.display(),
                            self.root.display()
                        )
                    })
            })
            .collect()
    }

    fn cargo_diff<I, S>(
        &self,
        pathspec_mode: &str,
        arguments: &[&str],
        options: &[&str],
        paths: I,
    ) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Command::new("git")
            .current_dir(&self.root)
            .arg(pathspec_mode)
            .args(["diff", "--quiet", "--no-ext-diff", "--no-textconv"])
            .args(options)
            .args(arguments)
            .arg("--")
            .args(paths)
            .output()
            .context("could not compare Cargo target inputs")
    }

    fn tracked_paths_in_tree(&self, tree: &ObjectId) -> Result<Vec<u8>> {
        self.git([
            OsStr::new("ls-tree"),
            OsStr::new("-r"),
            OsStr::new("-z"),
            OsStr::new("--name-only"),
            OsStr::new(tree.as_str()),
        ])
    }

    fn tracked_paths_in_index(&self) -> Result<Vec<u8>> {
        self.git([
            OsStr::new("ls-files"),
            OsStr::new("--cached"),
            OsStr::new("--full-name"),
            OsStr::new("-z"),
        ])
    }

    fn require_matching_cargo_manifests(
        &self,
        context: &cargo_context::CargoContext,
        tracked: &BTreeSet<PathBuf>,
        snapshot: &str,
    ) -> Result<()> {
        let mut discovered = BTreeSet::new();
        for manifest in context.manifests() {
            let relative = manifest.strip_prefix(&self.root).with_context(|| {
                format!(
                    "Cargo target roles cannot be proven because manifest {} is outside Git repository {}",
                    manifest.display(),
                    self.root.display()
                )
            })?;
            discovered.insert(relative.to_owned());
        }
        if let Some(manifest) = discovered.difference(tracked).next() {
            bail!(
                "Cargo target roles cannot be proven because manifest {} is absent from {snapshot}",
                manifest.display()
            );
        }
        if let Some(manifest) = tracked.difference(&discovered).next() {
            bail!(
                "Cargo target roles cannot be proven because manifest {} from {snapshot} was not discovered",
                manifest.display()
            );
        }
        Ok(())
    }

    fn scope_exists_in_worktree(&self) -> Result<bool> {
        let path = self.root.join(&self.scope);
        match fs::symlink_metadata(&path) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => {
                Err(error).with_context(|| format!("could not inspect {}", path.display()))
            }
        }
    }

    fn scope_exists_in_index(&self) -> Result<bool> {
        if self.scope == Path::new(".") {
            return Ok(true);
        }
        let output = self.git([
            OsStr::new("ls-files"),
            OsStr::new("--cached"),
            OsStr::new("--full-name"),
            OsStr::new("-z"),
            OsStr::new("--"),
            self.scope.as_os_str(),
        ])?;
        Ok(!parse_nul_paths(&output)?.is_empty())
    }

    fn scope_exists_in_tree(&self, tree: &ObjectId) -> Result<bool> {
        if self.scope == Path::new(".") {
            return Ok(true);
        }
        let output = self.git([
            OsStr::new("ls-tree"),
            OsStr::new("-z"),
            OsStr::new("--name-only"),
            OsStr::new(tree.as_str()),
            OsStr::new("--"),
            self.scope.as_os_str(),
        ])?;
        Ok(!parse_nul_paths(&output)?.is_empty())
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

    fn worktree_changes(&self, head: &ObjectId) -> Result<Vec<FileChange>> {
        let mut changes = self.diff_changes(DiffRequest::Worktree { head })?;
        let output = self.git([
            OsStr::new("ls-files"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("--full-name"),
            OsStr::new("-z"),
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

    fn unborn_worktree_changes(&self) -> Result<Vec<FileChange>> {
        let output = self.git([
            OsStr::new("ls-files"),
            OsStr::new("--cached"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("--full-name"),
            OsStr::new("-z"),
        ])?;
        let mut changes = Vec::new();
        for path in parse_nul_paths(&output)? {
            match fs::symlink_metadata(self.root.join(&path)) {
                Ok(_) => changes.push(FileChange {
                    before: None,
                    after: Some(Snapshot::worktree(path)),
                }),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("could not inspect {}", path.display()));
                }
            }
        }
        changes.sort_by(|left, right| left.sort_path().cmp(right.sort_path()));
        Ok(changes)
    }

    fn diff_changes(&self, request: DiffRequest<'_>) -> Result<Vec<FileChange>> {
        let mut arguments = vec![
            OsString::from("diff"),
            OsString::from("--raw"),
            OsString::from("-z"),
            OsString::from("--no-ext-diff"),
            OsString::from("--no-textconv"),
            OsString::from("--find-renames=1%"),
            OsString::from("-B"),
            OsString::from(format!("-l{GIT_EXHAUSTIVE_RENAME_LIMIT}")),
            OsString::from("--no-abbrev"),
        ];
        let target = match request {
            DiffRequest::Worktree { head } => {
                arguments.push(OsString::from(head.as_str()));
                DiffTarget::Worktree
            }
            DiffRequest::Index { head } => {
                arguments.push(OsString::from("--cached"));
                if let Some(head) = head {
                    arguments.push(OsString::from(head.as_str()));
                }
                DiffTarget::Index
            }
            DiffRequest::Commits { base, head } => {
                arguments.push(OsString::from(base.as_str()));
                arguments.push(OsString::from(head.as_str()));
                DiffTarget::Commit
            }
        };

        let output = self.git(arguments.iter().map(OsString::as_os_str))?;
        parse_raw_changes(&output, target)
    }

    fn head_state(&self) -> Result<HeadState> {
        let output = git_output(
            &self.root,
            [
                OsStr::new("rev-parse"),
                OsStr::new("--verify"),
                OsStr::new("--quiet"),
                OsStr::new("--end-of-options"),
                OsStr::new("HEAD^{commit}"),
            ],
        )?;
        if output.status.success() {
            return ObjectId::parse(one_line(&output.stdout, "revision object ID")?)
                .map(HeadState::Commit)
                .context("revision HEAD returned an invalid object ID");
        }

        let symbolic = git_output(
            &self.root,
            [
                OsStr::new("symbolic-ref"),
                OsStr::new("--quiet"),
                OsStr::new("--no-recurse"),
                OsStr::new("HEAD"),
            ],
        )?;
        if !symbolic.status.success() {
            bail!("could not resolve revision HEAD");
        }
        let branch = one_line(&symbolic.stdout, "HEAD symbolic reference")?;
        if !matches!(
            branch.strip_prefix("refs/heads/"),
            Some(branch_name) if !branch_name.is_empty()
        ) {
            bail!("could not resolve revision HEAD: symbolic reference is not a local branch");
        }

        let symbolic_branch = git_output(
            &self.root,
            [
                OsStr::new("symbolic-ref"),
                OsStr::new("--quiet"),
                OsStr::new("--no-recurse"),
                OsStr::new(branch),
            ],
        )?;
        if symbolic_branch.status.success() {
            bail!("revision HEAD does not name a commit");
        }
        if symbolic_branch.status.code() != Some(1) {
            bail!(
                "could not inspect HEAD branch: {}",
                stderr_message(&symbolic_branch)
            );
        }

        let existing = git_output(
            &self.root,
            [
                OsStr::new("show-ref"),
                OsStr::new("--verify"),
                OsStr::new("--quiet"),
                OsStr::new(branch),
            ],
        )?;
        match existing.status.code() {
            Some(1) => Ok(HeadState::Unborn),
            Some(0) => bail!("revision HEAD does not name a commit"),
            _ => bail!("could not inspect HEAD: {}", stderr_message(&existing)),
        }
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

    fn is_supported_regular(&self, snapshot: &Snapshot) -> Result<bool> {
        if !supports_path(&snapshot.path) {
            return Ok(false);
        }
        match snapshot.mode {
            Some(mode) => Ok(mode == FileMode::Regular),
            None => fs::symlink_metadata(self.root.join(&snapshot.path))
                .with_context(|| format!("could not inspect {}", snapshot.path.display()))
                .map(|metadata| metadata.file_type().is_file()),
        }
    }

    fn in_scope(&self, path: &Path) -> bool {
        self.scope == Path::new(".") || path.starts_with(&self.scope)
    }

    fn load_policy(
        &self,
        analysis: &AnalysisContext,
        authority: &CargoAuthority,
        explicit_config: Option<&Path>,
    ) -> Result<LoadedPolicy> {
        if let Some(config_path) = explicit_config {
            let absolute = cargo_context::normalized_absolute_path(config_path, "policy config")?;
            let bytes = source::read_regular(&absolute, config_path)?;
            let text = source::utf8(config_path, &bytes)?;
            let analysis = analysis
                .clone()
                .with_policy_toml(text)
                .with_context(|| format!("could not load {}", config_path.display()))?;
            let explicit_path = absolute
                .strip_prefix(&self.root)
                .ok()
                .filter(|path| !path.as_os_str().is_empty())
                .map(Path::to_owned);
            return Ok(LoadedPolicy {
                analysis,
                // An explicit config is read from the live filesystem rather than the
                // selected Git authority, so it has no reliable before snapshot.
                changed: true,
                excluded_paths: std::iter::once(PathBuf::from(POLICY_CONFIG_PATH))
                    .chain(explicit_path)
                    .collect(),
            });
        }

        let config_path = Path::new(POLICY_CONFIG_PATH);
        let (before, after) = self.policy_snapshots(authority)?;
        for snapshot in before.iter().chain(after.iter()) {
            validate_policy_snapshot(snapshot)?;
        }
        let object_ids: BTreeSet<_> = before
            .iter()
            .chain(after.iter())
            .filter_map(|snapshot| match &snapshot.source {
                SnapshotSource::Blob(object_id) => Some(object_id.clone()),
                SnapshotSource::Worktree => None,
            })
            .collect();
        let blobs = self.read_blobs(&object_ids)?;
        let before_bytes = before
            .as_ref()
            .map(|snapshot| read_snapshot(self, snapshot, &blobs))
            .transpose()?;
        let after_bytes = after
            .as_ref()
            .map(|snapshot| read_snapshot(self, snapshot, &blobs))
            .transpose()?;
        let changed = before_bytes.as_deref() != after_bytes.as_deref();
        let analysis = match after_bytes {
            Some(bytes) => {
                let text = source::utf8(config_path, &bytes)?;
                analysis
                    .clone()
                    .with_policy_toml(text)
                    .with_context(|| format!("could not load {POLICY_CONFIG_PATH}"))?
            }
            None => analysis.clone(),
        };
        Ok(LoadedPolicy {
            analysis,
            changed,
            excluded_paths: std::iter::once(config_path.to_owned()).collect(),
        })
    }

    fn policy_snapshots(
        &self,
        authority: &CargoAuthority,
    ) -> Result<(Option<Snapshot>, Option<Snapshot>)> {
        let path = Path::new(POLICY_CONFIG_PATH);
        match authority {
            CargoAuthority::Worktree { before } => Ok((
                before
                    .as_ref()
                    .map(|tree| self.snapshot_in_tree(tree, path))
                    .transpose()?
                    .flatten(),
                self.snapshot_in_worktree(path)?,
            )),
            CargoAuthority::Index { before } => Ok((
                before
                    .as_ref()
                    .map(|tree| self.snapshot_in_tree(tree, path))
                    .transpose()?
                    .flatten(),
                self.snapshot_in_index(path)?,
            )),
            CargoAuthority::Commits { before, after } => Ok((
                self.snapshot_in_tree(before, path)?,
                self.snapshot_in_tree(after, path)?,
            )),
        }
    }

    fn snapshot_in_worktree(&self, path: &Path) -> Result<Option<Snapshot>> {
        match fs::symlink_metadata(self.root.join(path)) {
            Ok(_) => Ok(Some(Snapshot::worktree(path.to_owned()))),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => {
                Err(error).with_context(|| format!("could not inspect {}", path.display()))
            }
        }
    }

    fn snapshot_in_tree(&self, tree: &ObjectId, path: &Path) -> Result<Option<Snapshot>> {
        let output = self.git([
            OsStr::new("ls-tree"),
            OsStr::new("-z"),
            OsStr::new(tree.as_str()),
            OsStr::new("--"),
            path.as_os_str(),
        ])?;
        one_snapshot(parse_tree_snapshots(&output)?, path, "Git tree")
    }

    fn snapshot_in_index(&self, path: &Path) -> Result<Option<Snapshot>> {
        let output = self.git([
            OsStr::new("ls-files"),
            OsStr::new("--stage"),
            OsStr::new("--full-name"),
            OsStr::new("-z"),
            OsStr::new("--"),
            path.as_os_str(),
        ])?;
        one_snapshot(parse_index_snapshots(&output)?, path, "Git index")
    }

    fn snapshots_for_authority(&self, authority: &CargoAuthority) -> Result<Vec<Snapshot>> {
        match authority {
            CargoAuthority::Worktree { .. } => self.worktree_snapshots(),
            CargoAuthority::Index { .. } => {
                let output = self.git([
                    OsStr::new("ls-files"),
                    OsStr::new("--stage"),
                    OsStr::new("--full-name"),
                    OsStr::new("-z"),
                ])?;
                parse_index_snapshots(&output)
            }
            CargoAuthority::Commits { after, .. } => {
                let output = self.git([
                    OsStr::new("ls-tree"),
                    OsStr::new("-r"),
                    OsStr::new("-z"),
                    OsStr::new(after.as_str()),
                ])?;
                parse_tree_snapshots(&output)
            }
        }
    }

    fn worktree_snapshots(&self) -> Result<Vec<Snapshot>> {
        let output = self.git([
            OsStr::new("ls-files"),
            OsStr::new("--cached"),
            OsStr::new("--others"),
            OsStr::new("--exclude-standard"),
            OsStr::new("--full-name"),
            OsStr::new("-z"),
        ])?;
        let mut snapshots = Vec::new();
        for path in parse_nul_paths(&output)? {
            match fs::symlink_metadata(self.root.join(&path)) {
                Ok(_) => snapshots.push(Snapshot::worktree(path)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error)
                        .with_context(|| format!("could not inspect {}", path.display()));
                }
            }
        }
        Ok(snapshots)
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
        self.cat_file_with_writer_builder(mode, object_ids, std::thread::Builder::new())
    }

    fn cat_file_with_writer_builder(
        &self,
        mode: &str,
        object_ids: &BTreeSet<ObjectId>,
        writer_builder: std::thread::Builder,
    ) -> Result<Vec<u8>> {
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
        let (output, input_result) = std::thread::scope(|scope| -> Result<_> {
            let writer = match writer_builder.spawn_scoped(scope, move || {
                for object_id in object_ids {
                    writeln!(input, "{}", object_id.as_str())
                        .context("could not request Git blob")?;
                }
                Ok::<(), anyhow::Error>(())
            }) {
                Ok(writer) => writer,
                Err(error) => {
                    let cleanup_error = terminate_and_reap(&mut child, mode).err();
                    let error = anyhow::Error::new(error)
                        .context("could not spawn Git blob request writer");
                    return Err(match cleanup_error {
                        Some(cleanup_error) => error.context(format!(
                            "could not clean up git cat-file {mode}: {cleanup_error:#}"
                        )),
                        None => error,
                    });
                }
            };
            let output = child.wait_with_output();
            let input_result = writer
                .join()
                .map_err(|_| anyhow::anyhow!("Git blob request writer panicked"));
            Ok((output, input_result))
        })?;
        let output = output.with_context(|| format!("could not wait for git cat-file {mode}"))?;
        if !output.status.success() {
            bail!("git cat-file {mode} failed: {}", stderr_message(&output));
        }
        input_result??;
        Ok(output.stdout)
    }
}

fn terminate_and_reap(child: &mut Child, mode: &str) -> Result<()> {
    let kill_result = child.kill();
    let wait_result = child.wait();
    wait_result.with_context(|| format!("could not reap git cat-file {mode}"))?;
    kill_result.with_context(|| format!("could not terminate git cat-file {mode}"))
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
        if !GIT_OBJECT_ID_HEX_LENGTHS.contains(&text.len())
            || !text.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
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
        b'M' if text.len() == 1 || valid_similarity_score(score) => Ok(Status::Modified),
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
            .is_some_and(|score| score <= MAX_GIT_SIMILARITY_SCORE)
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

fn parse_tree_snapshots(output: &[u8]) -> Result<Vec<Snapshot>> {
    parse_snapshot_records(output, "Git tree entry", |header| {
        let mut fields = header.split_ascii_whitespace();
        let mode = parse_mode(fields.next().context("Git tree entry had no mode")?)?;
        let _object_type = fields.next().context("Git tree entry had no object type")?;
        let object_id = ObjectId::parse(fields.next().context("Git tree entry had no object ID")?)?;
        if fields.next().is_some() {
            bail!("malformed Git tree entry header {header:?}");
        }
        Ok((mode, object_id))
    })
}

fn parse_index_snapshots(output: &[u8]) -> Result<Vec<Snapshot>> {
    parse_snapshot_records(output, "Git index entry", |header| {
        let mut fields = header.split_ascii_whitespace();
        let mode = parse_mode(fields.next().context("Git index entry had no mode")?)?;
        let object_id =
            ObjectId::parse(fields.next().context("Git index entry had no object ID")?)?;
        let stage = fields.next().context("Git index entry had no stage")?;
        if stage != "0" || fields.next().is_some() {
            bail!("cannot analyze an unmerged Git index entry");
        }
        Ok((mode, object_id))
    })
}

fn parse_snapshot_records(
    output: &[u8],
    label: &str,
    mut parse_header: impl FnMut(&str) -> Result<(FileMode, ObjectId)>,
) -> Result<Vec<Snapshot>> {
    let mut cursor = 0;
    let mut snapshots = Vec::new();
    while cursor < output.len() {
        let record = take_nul(output, &mut cursor, label)?;
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .with_context(|| format!("{label} had no path separator"))?;
        let header = std::str::from_utf8(&record[..tab])
            .with_context(|| format!("{label} header was not ASCII"))?;
        let path = parse_git_path(&record[tab + 1..])?;
        let (mode, object_id) = parse_header(header)?;
        snapshots.push(Snapshot::blob(path, object_id, mode));
    }
    Ok(snapshots)
}

fn one_snapshot(
    mut snapshots: Vec<Snapshot>,
    expected_path: &Path,
    label: &str,
) -> Result<Option<Snapshot>> {
    if snapshots.len() > 1 {
        bail!(
            "{label} returned duplicate entries for {}",
            expected_path.display()
        );
    }
    let snapshot = snapshots.pop();
    if snapshot
        .as_ref()
        .is_some_and(|snapshot| snapshot.path != expected_path)
    {
        bail!("{label} returned an unexpected path");
    }
    Ok(snapshot)
}

fn validate_policy_snapshot(snapshot: &Snapshot) -> Result<()> {
    if snapshot.mode.is_some_and(|mode| mode != FileMode::Regular) {
        bail!(
            "policy config {} is not a regular file",
            snapshot.path.display()
        );
    }
    Ok(())
}

fn parse_raw_object_id(text: &str) -> Result<Option<ObjectId>> {
    if text.bytes().all(|byte| byte == b'0') {
        if GIT_OBJECT_ID_HEX_LENGTHS.contains(&text.len()) {
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

fn cargo_manifest_paths(output: &[u8]) -> Result<BTreeSet<PathBuf>> {
    Ok(parse_nul_paths(output)?
        .into_iter()
        .filter(|path| is_cargo_manifest_path(path))
        .collect())
}

fn cargo_discovery_seeds(output: &[u8]) -> Result<CargoDiscoverySeeds> {
    let mut manifests = BTreeSet::new();
    let mut rust_sources = BTreeSet::new();
    for path in parse_nul_paths(output)? {
        if is_cargo_manifest_path(&path) {
            manifests.insert(path.clone());
        }
        if path.extension() == Some(OsStr::new("rs")) {
            rust_sources.insert(path);
        }
    }
    Ok(CargoDiscoverySeeds {
        manifests,
        rust_sources,
    })
}

fn is_cargo_manifest_path(path: &Path) -> bool {
    path.file_name() == Some(OsStr::new("Cargo.toml"))
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

fn analyze_changes(
    repository: &Repository,
    context: &AnalysisContext,
    mut changes: Vec<FileChange>,
    profile: AnalysisProfile,
    excluded_paths: &BTreeSet<PathBuf>,
    full_static_snapshots: Option<Vec<Snapshot>>,
) -> Result<Report> {
    for change in &mut changes {
        if change
            .before
            .path()
            .is_some_and(|path| excluded_paths.contains(path))
        {
            change.before = None;
        }
        if change
            .after
            .path()
            .is_some_and(|path| excluded_paths.contains(path))
        {
            change.after = None;
        }
    }
    changes.retain(|change| change.before.is_some() || change.after.is_some());
    reject_unpaired_supported_addition_and_deletion(repository, &changes)?;
    changes.retain(|change| {
        change
            .after
            .as_ref()
            .is_some_and(|after| repository.in_scope(&after.path))
    });
    for change in &changes {
        validate_modes(change)?;
    }

    let mut plans = Vec::new();
    for change in changes {
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

    let static_plans = full_static_snapshots
        .map(|snapshots| {
            snapshots
                .into_iter()
                .filter(|snapshot| {
                    repository.in_scope(&snapshot.path)
                        && !excluded_paths.contains(&snapshot.path)
                        && supports_path(&snapshot.path)
                })
                .map(|snapshot| {
                    validate_snapshot_mode(&snapshot)?;
                    Ok(Plan::All(snapshot))
                })
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    let object_ids = plans
        .iter()
        .chain(static_plans.iter().flatten())
        .flat_map(Plan::object_ids)
        .cloned()
        .collect();
    let blobs = repository.read_blobs(&object_ids)?;
    let mut findings = Vec::new();
    let files_scanned = if let Some(static_plans) = &static_plans {
        for plan in static_plans {
            let mut plan_findings =
                analyze_plan(repository, context, plan, &blobs, AnalysisProfile::Full)?;
            findings.append(&mut plan_findings);
        }
        for plan in plans
            .iter()
            .filter(|plan| matches!(plan, Plan::Change { .. }))
        {
            let mut plan_findings = analyze_plan(
                repository,
                context,
                plan,
                &blobs,
                AnalysisProfile::Attestation,
            )?;
            findings.append(&mut plan_findings);
        }
        static_plans.len()
    } else {
        for plan in &plans {
            let mut plan_findings = analyze_plan(repository, context, plan, &blobs, profile)?;
            findings.append(&mut plan_findings);
        }
        plans.len()
    };
    findings.sort();
    findings.dedup();
    Ok(Report {
        findings,
        files_scanned,
    })
}

fn reject_unpaired_supported_addition_and_deletion(
    repository: &Repository,
    changes: &[FileChange],
) -> Result<()> {
    let mut has_addition = false;
    let mut has_deletion = false;
    for change in changes {
        if change.before.is_none()
            && let Some(after) = &change.after
            && repository.in_scope(&after.path)
            && repository.is_supported_regular(after)?
        {
            has_addition = true;
        }
        if change.after.is_none()
            && let Some(before) = &change.before
            && repository.is_supported_regular(before)?
        {
            has_deletion = true;
        }
        if has_addition && has_deletion {
            bail!("cannot prove ancestry between supported additions and deletions");
        }
    }
    Ok(())
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

fn validate_snapshot_mode(snapshot: &Snapshot) -> Result<()> {
    if snapshot.mode.is_some_and(|mode| mode != FileMode::Regular) {
        bail!(
            "supported path {} is not a regular file",
            snapshot.path.display()
        );
    }
    Ok(())
}

fn analyze_plan(
    repository: &Repository,
    context: &AnalysisContext,
    plan: &Plan,
    blobs: &BTreeMap<ObjectId, Vec<u8>>,
    profile: AnalysisProfile,
) -> Result<Vec<fuck_ai_comments::Finding>> {
    match plan {
        Plan::All(after) => {
            let bytes = read_snapshot(repository, after, blobs)?;
            let text = source::utf8(&after.path, &bytes)?;
            context
                .analyze_all_with_profile(
                    SourceFile {
                        path: &after.path,
                        text,
                    },
                    profile,
                )
                .with_context(|| format!("could not analyze {}", after.path.display()))
        }
        Plan::Change { before, after } => {
            let before_bytes = read_snapshot(repository, before, blobs)?;
            let after_bytes = read_snapshot(repository, after, blobs)?;
            let before_text = source::utf8(&before.path, &before_bytes)?;
            let after_text = source::utf8(&after.path, &after_bytes)?;
            context
                .analyze_change_with_profile(
                    SourceFile {
                        path: &before.path,
                        text: before_text,
                    },
                    SourceFile {
                        path: &after.path,
                        text: after_text,
                    },
                    profile,
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

fn require_clean_cargo_diff(output: &Output, snapshots: &str) -> Result<()> {
    match output.status.code() {
        Some(0) => Ok(()),
        Some(1) => bail!(
            "Cargo target roles cannot be proven because Cargo target inputs differ between {snapshots}"
        ),
        _ => bail!(
            "could not compare Cargo target inputs between {snapshots}: {}",
            stderr_message(output)
        ),
    }
}

fn stderr_message(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).trim().to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::io;
    use std::path::PathBuf;
    use std::thread;

    use tempfile::TempDir;

    use super::{
        DiffTarget, ObjectId, Repository, Status, parse_blob_batch, parse_raw_changes,
        parse_status, run_git_at,
    };

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
    fn status_parser_accepts_modified_with_an_optional_valid_rewrite_score() {
        for status in ["M", "M0", "M100"] {
            assert!(
                matches!(parse_status(status), Ok(Status::Modified)),
                "valid modified status {status:?} should parse"
            );
        }
    }

    #[test]
    fn status_parser_rejects_invalid_modified_rewrite_scores() {
        for status in ["", "Mnot-a-score", "M101"] {
            assert!(
                parse_status(status).is_err(),
                "invalid modified status {status:?} should fail"
            );
        }
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

    #[test]
    fn cat_file_returns_the_writer_spawn_error() {
        let root = TempDir::new().expect("temporary repository should be created");
        run_git_at(root.path(), [std::ffi::OsStr::new("init")])
            .expect("temporary Git repository should be initialized");
        let repository = Repository {
            root: root.path().to_owned(),
            scope: PathBuf::from("."),
        };
        let object_id = ObjectId::parse(OLD_ID).expect("object ID should be valid");
        let requested = BTreeSet::from([object_id]);

        let error = repository
            .cat_file_with_writer_builder(
                "--batch-check",
                &requested,
                thread::Builder::new().stack_size(usize::MAX),
            )
            .expect_err("an impossible stack size should fail to spawn the writer");

        assert!(
            format!("{error:#}").contains("could not spawn Git blob request writer")
                && error.root_cause().downcast_ref::<io::Error>().is_some(),
            "unexpected error: {error:#}"
        );
    }
}
