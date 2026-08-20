use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::process::Command as ProcessCommand;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use assert_cmd::Command;
use tempfile::TempDir;

const CLEAN_RUST: &str = "const LIMIT: usize = 4;\n";
const SLOPPY_RUST: &str = "// First explanation.\n// Second explanation.\n// Third explanation.\n// Fourth explanation.\nconst LIMIT: usize = 4;\n";
const STALE_BEFORE: &str =
    "fn limit() -> usize {\n    // This boundary matches the external protocol.\n    1\n}\n";
const STALE_AFTER: &str =
    "fn limit() -> usize {\n    // This boundary matches the external protocol.\n    2\n}\n";
const UNPAIRED_ADD_DELETE_ERROR: &str =
    "error: cannot prove ancestry between supported additions and deletions\n";
const GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT: usize = 1_000;
static RESOURCE_INTENSIVE_GIT_TEST: Mutex<()> = Mutex::new(());

fn serialize_resource_intensive_git_test() -> MutexGuard<'static, ()> {
    RESOURCE_INTENSIVE_GIT_TEST
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn repository() -> TempDir {
    let root = unborn_repository();
    git(
        &root,
        ["commit", "--quiet", "--allow-empty", "-m", "initial"],
    );
    root
}

fn unborn_repository() -> TempDir {
    let root = TempDir::new().expect("temporary repository should be created");
    git(&root, ["init", "--quiet"]);
    git(&root, ["config", "user.email", "tests@example.com"]);
    git(&root, ["config", "user.name", "Test User"]);
    root
}

fn git<I, S>(root: &TempDir, args: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = ProcessCommand::new("git")
        .current_dir(root.path())
        .args(args)
        .output()
        .expect("git should run");
    assert!(
        output.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git test output should be UTF-8")
        .trim()
        .to_owned()
}

fn commit_all(root: &TempDir, message: &str) -> String {
    git(root, ["add", "--all"]);
    git(root, ["commit", "--quiet", "-m", message]);
    git(root, ["rev-parse", "HEAD"])
}

fn command(root: &TempDir) -> Command {
    let mut command = assert_cmd::cargo::cargo_bin_cmd!("fuck-ai-comments");
    command.current_dir(root.path());
    command
}

fn write(root: &TempDir, path: impl AsRef<Path>, source: impl AsRef<[u8]>) {
    let path = root.path().join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("source parent should be created");
    }
    fs::write(path, source).expect("source should be written");
}

fn inexact_rename_source(label: &str, revision: &str, value: usize) -> String {
    let stable_lines = (0..7)
        .map(|index| {
            format!("    let stable_{label}_{index} = \"{label}-anchor-{index:02}-xxxxxxxx\";\n")
        })
        .collect::<String>();
    format!(
        "fn owner_{label}() -> usize {{\n    // This boundary matches the external protocol.\n{stable_lines}    let revision_{label} = \"{revision}-xxxxxxxxxxxxxxxxxxxxxxxx\";\n    {value}\n}}\n"
    )
}

fn stage_inexact_renames(root: &TempDir, count: usize) {
    for index in 0..count {
        let extension = if index == 0 { "rs" } else { "txt" };
        write(
            root,
            format!("old-{index:04}.{extension}"),
            inexact_rename_source(&format!("file_{index}"), "before", 1),
        );
    }
    commit_all(root, "add rename sources");
    for index in 0..count {
        let extension = if index == 0 { "rs" } else { "txt" };
        let old_path = root.path().join(format!("old-{index:04}.{extension}"));
        let new_path = format!("new-{index:04}.{extension}");
        fs::rename(old_path, root.path().join(&new_path)).expect("source should be renamed");
        write(
            root,
            new_path,
            inexact_rename_source(&format!("file_{index}"), "after-", 2),
        );
    }
    git(root, ["add", "--all"]);
}

fn deterministic_padding(mut state: u64) -> String {
    std::iter::repeat_with(|| {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        char::from(b'a' + ((state >> 32) % 26) as u8)
    })
    .take(32 * 1_024)
    .collect()
}

#[test]
fn default_compares_head_to_the_worktree() {
    let root = repository();
    write(&root, "src/lib.rs", STALE_BEFORE);
    commit_all(&root, "add source");
    write(&root, "src/lib.rs", STALE_AFTER);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("src/lib.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn default_analyzes_supported_files_before_the_first_commit() {
    let root = unborn_repository();
    write(&root, "src/lib.rs", SLOPPY_RUST);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("src/lib.rs:1: comment-policy/leaf-comment-budget"));
}

#[test]
fn staged_analyzes_supported_files_before_the_first_commit() {
    let root = unborn_repository();
    write(&root, "src/lib.rs", SLOPPY_RUST);
    git(&root, ["add", "src/lib.rs"]);

    let output = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("src/lib.rs:1: comment-policy/leaf-comment-budget"));
}

#[test]
fn unborn_default_reads_the_worktree_while_staged_reads_the_index() {
    let root = unborn_repository();
    write(&root, "lib.rs", SLOPPY_RUST);
    git(&root, ["add", "lib.rs"]);
    write(&root, "lib.rs", CLEAN_RUST);

    command(&root)
        .arg("check")
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
    let staged = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(
        String::from_utf8(staged.stdout)
            .expect("stdout should be UTF-8")
            .contains("lib.rs:1: comment-policy/leaf-comment-budget")
    );
}

#[test]
fn unborn_modes_accept_an_empty_repository() {
    let root = unborn_repository();

    command(&root)
        .arg("check")
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
    command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn unborn_detection_rejects_a_recursive_nonbranch_symbolic_head() {
    let root = unborn_repository();
    git(
        &root,
        ["symbolic-ref", "refs/meta/alias", "refs/heads/missing"],
    );
    git(&root, ["symbolic-ref", "HEAD", "refs/meta/alias"]);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be UTF-8")
            .contains("could not resolve revision HEAD")
    );
}

#[test]
fn unborn_detection_rejects_a_symbolic_local_branch_alias() {
    let root = unborn_repository();
    git(
        &root,
        ["symbolic-ref", "refs/heads/alias", "refs/tags/missing"],
    );
    git(&root, ["symbolic-ref", "HEAD", "refs/heads/alias"]);

    command(&root).arg("check").assert().code(2);
}

#[test]
fn unborn_detection_rejects_an_existing_noncommit_branch_object() {
    let root = unborn_repository();
    write(&root, "object.bin", "not a commit\n");
    let object_id = git(&root, ["hash-object", "-w", "object.bin"]);
    fs::write(
        root.path()
            .join(".git")
            .join("refs")
            .join("heads")
            .join("noncommit"),
        format!("{object_id}\n"),
    )
    .expect("non-commit branch ref should be written");
    git(&root, ["symbolic-ref", "HEAD", "refs/heads/noncommit"]);

    command(&root).arg("check").assert().code(2);
}

#[test]
fn unborn_detection_rejects_a_branch_with_a_missing_object() {
    let root = unborn_repository();
    write(
        &root,
        "missing-object.bin",
        "not stored in this repository\n",
    );
    let object_id = git(&root, ["hash-object", "missing-object.bin"]);
    fs::write(
        root.path()
            .join(".git")
            .join("refs")
            .join("heads")
            .join("corrupt"),
        format!("{object_id}\n"),
    )
    .expect("corrupt branch ref should be written");
    git(&root, ["symbolic-ref", "HEAD", "refs/heads/corrupt"]);

    command(&root).arg("check").assert().code(2);
}

#[test]
fn unborn_head_does_not_become_an_implicit_commit_range_fallback() {
    let root = repository();
    git(&root, ["branch", "baseline", "HEAD"]);
    git(&root, ["symbolic-ref", "HEAD", "refs/heads/unborn"]);

    let output = command(&root)
        .args(["check", "--base", "baseline"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be UTF-8")
            .contains("could not resolve revision HEAD")
    );
}

#[test]
fn unborn_staged_still_fails_closed_on_parse_errors() {
    let root = unborn_repository();
    write(&root, "broken.rs", "fn broken( {\n");
    git(&root, ["add", "broken.rs"]);

    let output = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    assert!(
        String::from_utf8(output.stderr)
            .expect("stderr should be UTF-8")
            .contains("could not parse broken.rs as Rust")
    );
}

#[test]
fn attestation_profile_skips_static_worktree_findings() {
    let root = repository();
    write(&root, "lib.rs", CLEAN_RUST);
    commit_all(&root, "add source");
    write(&root, "lib.rs", SLOPPY_RUST);

    command(&root)
        .args(["check", "--profile", "attestation"])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn attestation_profile_reports_stale_worktree_comments() {
    let root = repository();
    write(&root, "lib.rs", STALE_BEFORE);
    commit_all(&root, "add source");
    write(&root, "lib.rs", STALE_AFTER);

    let output = command(&root)
        .args(["check", "--profile", "attestation"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("lib.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn default_includes_untracked_files() {
    let root = repository();
    write(&root, "untracked.rs", SLOPPY_RUST);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("untracked.rs:1: comment-policy/leaf-comment-budget"));
}

#[test]
fn default_rejects_an_untracked_manifest_hidden_by_ignore_rules() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(&root, ".ignore", "nested/Cargo.toml\n");
    write(
        &root,
        "nested/custom/root.rs",
        concat!(
            "//! before detail one\n",
            "//! before detail two\n",
            "//! before detail three\n",
            "//! before detail four\n",
            "//! before detail five\n",
            "//! before detail six\n",
            "pub fn work() -> usize { 1 }\n",
        ),
    );
    commit_all(&root, "add ordinary Rust source");
    write(
        &root,
        "nested/Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"hidden-new-manifest\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(
        &root,
        "nested/custom/root.rs",
        concat!(
            "//! after detail one\n",
            "//! after detail two\n",
            "//! after detail three\n",
            "//! after detail four\n",
            "//! after detail five\n",
            "//! after detail six\n",
            "pub fn work() -> usize { 2 }\n",
        ),
    );

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(
        stderr.contains("manifest nested/Cargo.toml is absent from HEAD"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn default_rejects_a_hidden_untracked_manifest_without_a_rust_change() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(&root, ".ignore", "nested/\n");
    write(&root, "nested/custom/root.rs", CLEAN_RUST);
    commit_all(&root, "add ignored ordinary Rust source");
    write(
        &root,
        "nested/Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"hidden-new-manifest\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(
        stderr.contains("manifest nested/Cargo.toml is absent from HEAD"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn default_rejects_a_gitignored_manifest_above_tracked_rust() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(&root, "nested/custom/root.rs", CLEAN_RUST);
    commit_all(&root, "add ordinary tracked Rust source");
    write(&root, ".gitignore", "nested/\n");
    commit_all(&root, "ignore the nested directory");
    write(
        &root,
        "nested/Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"gitignored-new-manifest\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(
        stderr.contains("manifest nested/Cargo.toml is absent from HEAD"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn default_rejects_a_gitignored_implicit_library_addition() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"gitignored-implicit-library\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
        ),
    );
    write(&root, "src/main.rs", "fn main() {}\n");
    write(&root, ".gitignore", "src/lib.rs\n");
    commit_all(&root, "add binary package");
    write(&root, "src/lib.rs", CLEAN_RUST);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(
        stderr.contains("Cargo target inputs differ between HEAD and the worktree"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn attestation_profile_validates_a_clean_added_file() {
    let root = repository();
    write(&root, "untracked.rs", SLOPPY_RUST);

    command(&root)
        .args(["check", "--profile", "attestation"])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn attestation_profile_rejects_an_invalid_added_file() {
    let root = repository();
    write(&root, "broken.rs", "fn broken( {\n");

    let output = command(&root)
        .args(["check", "--profile", "attestation"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("could not parse broken.rs as Rust"));
}

#[test]
fn staged_reads_the_index_instead_of_the_worktree() {
    let root = repository();
    write(&root, "lib.rs", CLEAN_RUST);
    commit_all(&root, "add clean source");
    write(&root, "lib.rs", SLOPPY_RUST);
    git(&root, ["add", "lib.rs"]);
    write(&root, "lib.rs", CLEAN_RUST);

    let staged = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(
        String::from_utf8(staged.stdout)
            .expect("stdout should be UTF-8")
            .contains("comment-policy/leaf-comment-budget")
    );
    command(&root)
        .arg("check")
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn attestation_profile_skips_static_staged_findings() {
    let root = repository();
    write(&root, "lib.rs", CLEAN_RUST);
    commit_all(&root, "add clean source");
    write(&root, "lib.rs", SLOPPY_RUST);
    git(&root, ["add", "lib.rs"]);
    write(&root, "lib.rs", "fn broken( {\n");

    command(&root)
        .args(["check", "--staged", "--profile", "attestation"])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn staged_ignores_unstaged_changes() {
    let root = repository();
    write(&root, "lib.rs", CLEAN_RUST);
    commit_all(&root, "add clean source");
    write(&root, "lib.rs", SLOPPY_RUST);

    command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
    let worktree = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(
        String::from_utf8(worktree.stdout)
            .expect("stdout should be UTF-8")
            .contains("comment-policy/leaf-comment-budget")
    );
}

#[test]
fn default_reconciles_an_index_deletion_with_the_untracked_worktree_file() {
    let root = repository();
    write(&root, "lib.rs", STALE_BEFORE);
    commit_all(&root, "add source");
    git(&root, ["rm", "--quiet", "--cached", "--", "lib.rs"]);
    write(&root, "lib.rs", STALE_AFTER);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("lib.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn default_reconciles_an_out_of_scope_same_path_replacement_before_guarding() {
    let root = repository();
    write(&root, "outside.rs", CLEAN_RUST);
    commit_all(&root, "add source");
    git(&root, ["rm", "--quiet", "--cached", "--", "outside.rs"]);
    write(&root, "scoped/inside.rs", CLEAN_RUST);

    command(&root)
        .args(["check", "scoped"])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn default_pairs_a_rename_before_checking_staleness() {
    let root = repository();
    write(&root, "old.rs", STALE_BEFORE);
    commit_all(&root, "add source");
    git(&root, ["mv", "--", "old.rs", "new.rs"]);
    write(&root, "new.rs", STALE_AFTER);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("new.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn default_pairs_a_rename_into_scope_before_checking_staleness() {
    let root = repository();
    write(&root, "outside.rs", STALE_BEFORE);
    commit_all(&root, "add source");
    fs::create_dir(root.path().join("scoped")).expect("scope should be created");
    git(&root, ["mv", "--", "outside.rs", "scoped/inside.rs"]);
    write(&root, "scoped/inside.rs", STALE_AFTER);

    let output = command(&root)
        .args(["check", "scoped"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("scoped/inside.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn staged_pairs_a_rename_into_scope_before_checking_staleness() {
    let root = repository();
    write(&root, "outside.rs", STALE_BEFORE);
    commit_all(&root, "add source");
    fs::create_dir(root.path().join("scoped")).expect("scope should be created");
    git(&root, ["mv", "--", "outside.rs", "scoped/inside.rs"]);
    write(&root, "scoped/inside.rs", STALE_AFTER);
    git(&root, ["add", "--all"]);

    let output = command(&root)
        .args(["check", "--staged", "scoped"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("scoped/inside.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn staged_breaks_a_scoped_rewrite_before_pairing_its_deleted_source() {
    let root = repository();
    write(
        &root,
        "outside.rs",
        inexact_rename_source("moved", "before", 1),
    );
    write(
        &root,
        "scoped/inside.rs",
        format!(
            "const ORIGINAL_INSIDE: &str = \"{}\";\n",
            deterministic_padding(7)
        ),
    );
    commit_all(&root, "add sources");
    fs::remove_file(root.path().join("outside.rs")).expect("outside source should be removed");
    write(
        &root,
        "scoped/inside.rs",
        inexact_rename_source("moved", "after-", 2),
    );
    git(&root, ["add", "--all"]);
    let limit_argument = format!("-l{GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT}");
    let raw_without_break = git(
        &root,
        [
            "diff",
            "--cached",
            "--raw",
            "--find-renames=1%",
            &limit_argument,
            "HEAD",
        ],
    );
    assert!(
        raw_without_break.contains(" D\toutside.rs")
            && raw_without_break.contains(" M\tscoped/inside.rs")
            && !raw_without_break.contains(" R"),
        "unexpected fixture without rewrite breaking: {raw_without_break}"
    );
    let raw_with_break = git(
        &root,
        [
            "diff",
            "--cached",
            "--raw",
            "--find-renames=1%",
            "--break-rewrites",
            &limit_argument,
            "HEAD",
        ],
    );
    assert!(
        raw_with_break.contains(" R") && raw_with_break.contains("\toutside.rs\tscoped/inside.rs"),
        "unexpected fixture with rewrite breaking: {raw_with_break}"
    );

    let output = command(&root)
        .args(["check", "--staged", "scoped"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("scoped/inside.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn staged_accepts_a_standalone_complete_rewrite() {
    let root = repository();
    write(
        &root,
        "lib.rs",
        format!("const BEFORE: &str = \"{}\";\n", deterministic_padding(11)),
    );
    commit_all(&root, "add source");
    write(
        &root,
        "lib.rs",
        format!("static AFTER: &str = \"{}\";\n", deterministic_padding(12)),
    );
    git(&root, ["add", "--all"]);
    let limit_argument = format!("-l{GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT}");
    let raw = git(
        &root,
        [
            "diff",
            "--cached",
            "--raw",
            "--find-renames=1%",
            "-B",
            &limit_argument,
            "HEAD",
        ],
    );
    assert!(
        raw.contains(" M100\tlib.rs"),
        "unexpected standalone rewrite fixture: {raw}"
    );

    command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_pairs_a_rename_into_scope_before_checking_staleness() {
    let root = repository();
    write(&root, "outside.rs", STALE_BEFORE);
    let base = commit_all(&root, "add source");
    fs::create_dir(root.path().join("scoped")).expect("scope should be created");
    git(&root, ["mv", "--", "outside.rs", "scoped/inside.rs"]);
    write(&root, "scoped/inside.rs", STALE_AFTER);
    let head = commit_all(&root, "rename source into scope");

    let output = command(&root)
        .args(["check", "--base", &base, "--head", &head, "scoped"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("scoped/inside.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn default_ignores_a_rename_out_of_scope() {
    let root = repository();
    write(&root, "scoped/inside.rs", CLEAN_RUST);
    commit_all(&root, "add source in scope");
    git(&root, ["mv", "--", "scoped/inside.rs", "outside.rs"]);

    command(&root)
        .args(["check", "scoped"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn default_scope_matching_uses_path_components() {
    let root = repository();
    write(&root, "scoped/unchanged.rs", CLEAN_RUST);
    commit_all(&root, "add scoped source");
    write(&root, "scoped-other/outside.rs", SLOPPY_RUST);

    command(&root)
        .args(["check", "scoped"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn default_pairs_a_unique_low_similarity_rename() {
    let root = repository();
    let before_prefix = (0..32)
        .map(|index| format!("const BEFORE_{index}: usize = {index};\n"))
        .collect::<String>();
    let after_prefix = (0..32)
        .map(|index| format!("const AFTER_{index}: &str = \"value-{index}\";\n"))
        .collect::<String>();
    write(&root, "old.rs", format!("{before_prefix}{STALE_BEFORE}"));
    commit_all(&root, "add source");
    git(&root, ["mv", "--", "old.rs", "new.rs"]);
    write(&root, "new.rs", format!("{after_prefix}{STALE_AFTER}"));

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("new.rs:34: comment-policy/comment-owner-changed"));
}

#[test]
fn default_ignores_a_tiny_git_limit_for_three_inexact_supported_renames() {
    let root = repository();
    for label in ["alpha", "beta", "gamma"] {
        write(
            &root,
            format!("old-{label}.rs"),
            inexact_rename_source(label, "before", 1),
        );
    }
    commit_all(&root, "add sources");
    git(&root, ["config", "diff.renameLimit", "1"]);
    for label in ["alpha", "beta", "gamma"] {
        git(
            &root,
            [
                "mv",
                "--",
                &format!("old-{label}.rs"),
                &format!("new-{label}.rs"),
            ],
        );
        write(
            &root,
            format!("new-{label}.rs"),
            inexact_rename_source(label, "after-", 2),
        );
    }
    let limit_argument = format!("-l{GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT}");
    let raw = git(
        &root,
        [
            "diff",
            "--raw",
            "--find-renames=1%",
            &limit_argument,
            "HEAD",
        ],
    );
    assert_eq!(raw.matches("R087").count(), 3, "unexpected fixture: {raw}");

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert_eq!(
        stdout
            .matches("comment-policy/comment-owner-changed")
            .count(),
        3
    );
}

#[test]
fn staged_fails_closed_when_a_surviving_comment_falls_below_rename_similarity() {
    let root = repository();
    let before_prefix = format!("const BEFORE: &str = \"{}\";\n", deterministic_padding(1));
    let after_prefix = format!("static AFTER: &str = \"{}\";\n", deterministic_padding(2));
    write(&root, "old.rs", format!("{before_prefix}{STALE_BEFORE}"));
    commit_all(&root, "add source");
    git(&root, ["mv", "--", "old.rs", "new.rs"]);
    write(&root, "new.rs", format!("{after_prefix}{STALE_AFTER}"));
    git(&root, ["add", "--all"]);
    let limit_argument = format!("-l{GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT}");
    let raw = git(
        &root,
        [
            "diff",
            "--cached",
            "--raw",
            "--find-renames=1%",
            &limit_argument,
            "HEAD",
        ],
    );
    assert!(!raw.contains(" R"), "unexpected fixture: {raw}");

    command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .stderr(UNPAIRED_ADD_DELETE_ERROR);
}

#[test]
fn staged_fails_closed_on_unrelated_supported_addition_and_deletion() {
    let root = repository();
    write(&root, "removed.py", "removed_value = 41\n");
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("removed.py")).expect("old source should be removed");
    write(&root, "added.rs", "const ADDED_VALUE: usize = 42;\n");
    git(&root, ["add", "--all"]);

    command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .stderr(UNPAIRED_ADD_DELETE_ERROR);
}

#[test]
fn attestation_profile_keeps_the_addition_deletion_ancestry_guard() {
    let root = repository();
    write(&root, "removed.py", "removed_value = 41\n");
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("removed.py")).expect("old source should be removed");
    write(&root, "added.rs", "const ADDED_VALUE: usize = 42;\n");
    git(&root, ["add", "--all"]);

    command(&root)
        .args(["check", "--staged", "--profile", "attestation"])
        .assert()
        .code(2)
        .stderr(UNPAIRED_ADD_DELETE_ERROR);
}

#[test]
fn staged_pairs_candidates_at_the_explicit_exhaustive_rename_limit() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    stage_inexact_renames(&root, GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT);
    git(&root, ["config", "diff.renameLimit", "1"]);
    let limit_argument = format!("-l{GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT}");
    let raw = git(
        &root,
        [
            "diff",
            "--cached",
            "--raw",
            "--find-renames=1%",
            &limit_argument,
            "HEAD",
        ],
    );
    assert_eq!(
        raw.matches(" R").count(),
        GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT,
        "unexpected fixture: {raw}"
    );

    let output = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("new-0000.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn staged_fails_closed_above_the_explicit_exhaustive_rename_limit() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    let candidate_count = GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT + 1;
    stage_inexact_renames(&root, candidate_count);
    git(&root, ["config", "diff.renameLimit", "0"]);
    let limit_argument = format!("-l{GIT_DEFAULT_EXHAUSTIVE_RENAME_LIMIT}");
    let raw = git(
        &root,
        [
            "diff",
            "--cached",
            "--raw",
            "--find-renames=1%",
            &limit_argument,
            "HEAD",
        ],
    );
    assert_eq!(
        (
            raw.matches(" R").count(),
            raw.matches(" A\t").count(),
            raw.matches(" D\t").count(),
        ),
        (0, candidate_count, candidate_count),
        "unexpected fixture: {raw}"
    );

    command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .stderr(UNPAIRED_ADD_DELETE_ERROR);
}

#[test]
fn default_fails_closed_on_a_tracked_deletion_outside_an_untracked_addition_scope() {
    let root = repository();
    write(&root, "outside.rs", CLEAN_RUST);
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("outside.rs")).expect("tracked source should be removed");
    write(&root, "scoped/inside.rs", CLEAN_RUST);

    command(&root)
        .args(["check", "scoped"])
        .assert()
        .code(2)
        .stderr(UNPAIRED_ADD_DELETE_ERROR);
}

#[test]
fn deleting_a_file_does_not_lint_the_old_snapshot() {
    let root = repository();
    write(&root, "deleted.rs", SLOPPY_RUST);
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("deleted.rs")).expect("source should be removed");

    command(&root)
        .arg("check")
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn attestation_profile_does_not_scan_a_deleted_file() {
    let root = repository();
    write(&root, "deleted.rs", SLOPPY_RUST);
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("deleted.rs")).expect("source should be removed");

    command(&root)
        .args(["check", "--profile", "attestation"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn base_and_head_read_committed_blobs() {
    let root = repository();
    write(&root, "lib.rs", STALE_BEFORE);
    let base = commit_all(&root, "add source");
    write(&root, "lib.rs", STALE_AFTER);
    let head = commit_all(&root, "change source");
    write(
        &root,
        "lib.rs",
        "fn limit() -> usize {\n    // This boundary now matches version two.\n    2\n}\n",
    );

    let output = command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("lib.rs:2: comment-policy/comment-owner-changed"));
}

#[test]
fn base_uses_cargo_metadata_for_a_custom_library_root() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"custom-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(
        &root,
        "custom/root.rs",
        concat!(
            "//! before detail one\n",
            "//! before detail two\n",
            "//! before detail three\n",
            "//! before detail four\n",
            "//! before detail five\n",
            "//! before detail six\n",
            "pub fn work() -> usize { 1 }\n",
        ),
    );
    let base = commit_all(&root, "add custom library root");
    write(
        &root,
        "custom/root.rs",
        concat!(
            "//! after detail one\n",
            "//! after detail two\n",
            "//! after detail three\n",
            "//! after detail four\n",
            "//! after detail five\n",
            "//! after detail six\n",
            "pub fn work() -> usize { 2 }\n",
        ),
    );
    let head = commit_all(&root, "change custom library root");

    command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_allows_an_ordinary_src_lib_addition_without_a_cargo_manifest() {
    let root = repository();
    let base = git(&root, ["rev-parse", "HEAD"]);
    write(&root, "ordinary/src/lib.rs", CLEAN_RUST);
    let head = commit_all(&root, "add ordinary src lib file");

    command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_allows_an_ordinary_src_lib_addition_with_a_custom_library_root() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"custom-with-ordinary-src-lib\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(&root, "custom/root.rs", CLEAN_RUST);
    let base = commit_all(&root, "add custom library root");
    write(&root, "src/lib.rs", CLEAN_RUST);
    let head = commit_all(&root, "add ordinary src lib file");

    command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_allows_an_ordinary_src_lib_addition_when_autolib_is_disabled() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"autolib-disabled\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "autolib = false\n",
        ),
    );
    write(&root, "src/main.rs", "fn main() {}\n");
    let base = commit_all(&root, "add binary package");
    write(&root, "src/lib.rs", CLEAN_RUST);
    let head = commit_all(&root, "add ordinary src lib file");

    command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_discovers_a_nested_cargo_project_without_a_root_manifest() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "rust/Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"nested-custom-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(
        &root,
        "rust/custom/root.rs",
        concat!(
            "//! before detail one\n",
            "//! before detail two\n",
            "//! before detail three\n",
            "//! before detail four\n",
            "//! before detail five\n",
            "//! before detail six\n",
            "pub fn work() -> usize { 1 }\n",
        ),
    );
    let base = commit_all(&root, "add nested custom library root");
    write(
        &root,
        "rust/custom/root.rs",
        concat!(
            "//! after detail one\n",
            "//! after detail two\n",
            "//! after detail three\n",
            "//! after detail four\n",
            "//! after detail five\n",
            "//! after detail six\n",
            "pub fn work() -> usize { 2 }\n",
        ),
    );
    let head = commit_all(&root, "change nested custom library root");

    command(&root)
        .args(["check", "--base", &base, "--head", &head, "rust"])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_ignores_an_unrelated_cargo_project_outside_the_repository() {
    let _guard = serialize_resource_intensive_git_test();
    let outer = TempDir::new().expect("outer Cargo project should be created");
    write(
        &outer,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"outer-project\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
        ),
    );
    write(&outer, "src/lib.rs", CLEAN_RUST);
    let root = tempfile::Builder::new()
        .prefix("nested-repository-")
        .tempdir_in(outer.path())
        .expect("nested repository should be created");
    git(&root, ["init", "--quiet"]);
    git(&root, ["config", "user.email", "tests@example.com"]);
    git(&root, ["config", "user.name", "Test User"]);
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"nested-repository\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[workspace]\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(
        &root,
        "custom/root.rs",
        concat!(
            "//! before detail one\n",
            "//! before detail two\n",
            "//! before detail three\n",
            "//! before detail four\n",
            "//! before detail five\n",
            "//! before detail six\n",
            "pub fn work() -> usize { 1 }\n",
        ),
    );
    let base = commit_all(&root, "add nested repository");
    write(
        &root,
        "custom/root.rs",
        concat!(
            "//! after detail one\n",
            "//! after detail two\n",
            "//! after detail three\n",
            "//! after detail four\n",
            "//! after detail five\n",
            "//! after detail six\n",
            "pub fn work() -> usize { 2 }\n",
        ),
    );
    let head = commit_all(&root, "change nested repository");

    command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_discovers_a_tracked_nested_manifest_hidden_by_dirty_ignore_rules() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "ignored/Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"ignored-custom-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(
        &root,
        "ignored/custom/root.rs",
        concat!(
            "//! before detail one\n",
            "//! before detail two\n",
            "//! before detail three\n",
            "//! before detail four\n",
            "//! before detail five\n",
            "//! before detail six\n",
            "pub fn work() -> usize { 1 }\n",
        ),
    );
    let base = commit_all(&root, "add ignored nested Cargo project");
    write(
        &root,
        "ignored/custom/root.rs",
        concat!(
            "//! after detail one\n",
            "//! after detail two\n",
            "//! after detail three\n",
            "//! after detail four\n",
            "//! after detail five\n",
            "//! after detail six\n",
            "pub fn work() -> usize { 2 }\n",
        ),
    );
    let head = commit_all(&root, "change ignored nested library root");
    write(&root, ".ignore", "ignored/\n");

    command(&root)
        .args(["check", "--base", &base, "--head", &head, "ignored"])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_rejects_a_cargo_root_change_between_snapshots() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"moving-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"old/root.rs\"\n",
        ),
    );
    write(&root, "old/root.rs", "pub fn work() -> usize { 1 }\n");
    let base = commit_all(&root, "add old library root");
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"moving-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"new/root.rs\"\n",
        ),
    );
    fs::remove_file(root.path().join("old/root.rs")).expect("old root should be removed");
    write(&root, "new/root.rs", "pub fn work() -> usize { 2 }\n");
    let head = commit_all(&root, "move library root");

    let output = command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains(
        "Cargo target roles cannot be proven because Cargo target inputs differ between the compared commits"
    ));
}

#[test]
fn base_rejects_a_nested_cargo_manifest_change() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "rust/Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"nested-change\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(
        &root,
        "rust/custom/root.rs",
        "pub fn work() -> usize { 1 }\n",
    );
    let base = commit_all(&root, "add nested Cargo project");
    write(
        &root,
        "rust/Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"nested-change\"\n",
            "version = \"0.1.1\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(
        &root,
        "rust/custom/root.rs",
        "pub fn work() -> usize { 2 }\n",
    );
    let head = commit_all(&root, "change nested Cargo project");

    let output = command(&root)
        .args(["check", "--base", &base, "--head", &head, "rust"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains(
        "Cargo target roles cannot be proven because Cargo target inputs differ between the compared commits"
    ));
}

#[test]
fn base_rejects_metadata_from_a_worktree_newer_than_the_requested_head() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"historical-head\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"historical/root.rs\"\n",
        ),
    );
    write(
        &root,
        "historical/root.rs",
        "pub fn work() -> usize { 1 }\n",
    );
    let base = commit_all(&root, "add historical library root");
    write(
        &root,
        "historical/root.rs",
        "pub fn work() -> usize { 2 }\n",
    );
    let historical_head = commit_all(&root, "change historical library root");
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"historical-head\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"current/root.rs\"\n",
        ),
    );
    fs::remove_file(root.path().join("historical/root.rs"))
        .expect("historical root should be removed");
    write(&root, "current/root.rs", "pub fn work() -> usize { 3 }\n");
    commit_all(&root, "move current library root");

    let output = command(&root)
        .args(["check", "--base", &base, "--head", &historical_head])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains(
        "Cargo target roles cannot be proven because Cargo target inputs differ between the head commit and the worktree"
    ));
}

#[test]
fn staged_rejects_metadata_from_an_unstaged_manifest() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"staged-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    );
    write(&root, "custom/root.rs", "pub fn work() -> usize { 1 }\n");
    commit_all(&root, "add staged library root");
    write(&root, "custom/root.rs", "pub fn work() -> usize { 2 }\n");
    git(&root, ["add", "custom/root.rs"]);
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"staged-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"unstaged/root.rs\"\n",
        ),
    );
    write(&root, "unstaged/root.rs", "pub fn work() -> usize { 3 }\n");

    let output = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains(
        "Cargo target roles cannot be proven because Cargo target inputs differ between the index and the worktree"
    ));
}

#[test]
fn base_rejects_metadata_after_an_unstaged_implicit_library_deletion() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"implicit-library\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
        ),
    );
    write(&root, "src/main.rs", "fn main() {}\n");
    write(&root, "src/lib.rs", "pub fn work() -> usize { 1 }\n");
    let base = commit_all(&root, "add implicit library");
    write(&root, "src/lib.rs", "pub fn work() -> usize { 2 }\n");
    let head = commit_all(&root, "change implicit library");
    fs::remove_file(root.path().join("src/lib.rs")).expect("library root should be removed");

    let output = command(&root)
        .args(["check", "--base", &base, "--head", &head])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(
        stderr.contains("Cargo target inputs differ between the head commit and the worktree"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn staged_rejects_metadata_after_an_unstaged_implicit_library_deletion() {
    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    write(
        &root,
        "Cargo.toml",
        concat!(
            "[package]\n",
            "name = \"staged-implicit-library\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
        ),
    );
    write(&root, "src/main.rs", "fn main() {}\n");
    write(&root, "src/lib.rs", "pub fn work() -> usize { 1 }\n");
    commit_all(&root, "add implicit library");
    write(&root, "src/lib.rs", "pub fn work() -> usize { 2 }\n");
    git(&root, ["add", "src/lib.rs"]);
    fs::remove_file(root.path().join("src/lib.rs")).expect("library root should be removed");

    let output = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(
        stderr.contains("Cargo target inputs differ between the index and the worktree"),
        "unexpected stderr: {stderr}"
    );
}

#[test]
fn attestation_profile_skips_static_commit_findings() {
    let root = repository();
    write(&root, "lib.rs", CLEAN_RUST);
    let base = commit_all(&root, "add source");
    write(&root, "lib.rs", SLOPPY_RUST);
    let head = commit_all(&root, "add comments");

    command(&root)
        .args([
            "check",
            "--base",
            &base,
            "--head",
            &head,
            "--profile",
            "attestation",
        ])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn base_defaults_head_to_the_head_commit() {
    let root = repository();
    write(&root, "lib.rs", STALE_BEFORE);
    let base = commit_all(&root, "add source");
    write(&root, "lib.rs", STALE_AFTER);
    commit_all(&root, "change source");

    let output = command(&root)
        .args(["check", "--base", &base])
        .assert()
        .code(1)
        .get_output()
        .clone();
    assert!(
        String::from_utf8(output.stdout)
            .expect("stdout should be UTF-8")
            .contains("comment-policy/comment-owner-changed")
    );
}

#[test]
fn base_compares_the_merge_base_to_head() {
    let root = repository();
    let base_branch = git(&root, ["branch", "--show-current"]);
    write(&root, "lib.rs", STALE_BEFORE);
    commit_all(&root, "add source");
    git(&root, ["switch", "--quiet", "-c", "feature"]);
    write(&root, "feature.rs", CLEAN_RUST);
    let feature = commit_all(&root, "change feature");
    git(&root, ["switch", "--quiet", &base_branch]);
    write(&root, "lib.rs", STALE_AFTER);
    let base = commit_all(&root, "change base only");

    command(&root)
        .args(["check", "--base", &base, "--head", &feature])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn full_profile_rejects_a_cross_language_rename() {
    let root = repository();
    let shared_source = "value = 1\n";
    write(&root, "config.toml", shared_source);
    commit_all(&root, "add config");
    git(&root, ["mv", "--", "config.toml", "config.py"]);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("cannot attest a change across language adapters"));
}

#[test]
fn attestation_profile_rejects_a_cross_language_rename() {
    let root = repository();
    let shared_source = "value = 1\n";
    write(&root, "config.toml", shared_source);
    commit_all(&root, "add config");
    git(&root, ["mv", "--", "config.toml", "config.py"]);

    let output = command(&root)
        .args(["check", "--profile", "attestation"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("cannot attest a change across language adapters"));
}

#[test]
fn literal_pathspecs_handle_git_metacharacters_and_unicode() {
    let root = repository();
    let path = "-[literal]雪.rs";
    write(&root, path, SLOPPY_RUST);

    let output = command(&root)
        .args(["check", "--"])
        .arg(path)
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("comment-policy/leaf-comment-budget"));
    assert!(stdout.contains("-[literal]雪.rs:1:"));
}

#[cfg(unix)]
#[test]
fn finding_paths_cannot_inject_github_workflow_commands() {
    let root = repository();
    write(&root, "::warning::pwn.rs", SLOPPY_RUST);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.starts_with("./::warning::pwn.rs:1:"));
    assert!(!stdout.lines().any(|line| line.starts_with("::")));
}

#[test]
fn finding_paths_escape_unicode_bidi_controls() {
    let root = repository();
    write(&root, "safe\u{202e}evil.rs", SLOPPY_RUST);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("safe\\u{202e}evil.rs:1:"));
    assert!(!stdout.contains('\u{202e}'));
}

#[test]
fn parse_errors_fail_closed() {
    let root = repository();
    write(&root, "broken.rs", "fn {");

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: could not analyze broken.rs"));
    assert!(stderr.contains("could not parse broken.rs as Rust"));
}

#[cfg(unix)]
#[test]
fn parse_errors_escape_control_characters_in_paths() {
    let root = repository();
    write(&root, "broken\n::warning::pwn.rs", "fn {");

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("broken\\n::warning::pwn.rs"));
    assert_eq!(stderr.lines().count(), 1);
    assert!(!stderr.lines().any(|line| line.starts_with("::")));
}

#[test]
fn invalid_utf8_errors_fail_closed() {
    let root = repository();
    write(&root, "broken.rs", [0xff, 0xfe]);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: broken.rs is not valid UTF-8"));
}

#[cfg(unix)]
#[test]
fn invalid_utf8_errors_escape_control_characters_in_paths() {
    let root = repository();
    write(&root, "::error::broken\nsource.rs", [0xff, 0xfe]);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("::error::broken\\nsource.rs is not valid UTF-8"));
    assert_eq!(stderr.lines().count(), 1);
    assert!(!stderr.lines().any(|line| line.starts_with("::")));
}

#[test]
fn error_paths_escape_unicode_bidi_controls() {
    let root = repository();
    write(&root, "broken\u{2067}source.rs", "fn {");

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("broken\\u{2067}source.rs"));
    assert!(!stderr.contains('\u{2067}'));
}

#[test]
fn invalid_revision_fails_closed() {
    let root = repository();

    let output = command(&root)
        .args(["check", "--base", "missing-revision"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: could not resolve revision missing-revision"));
}

#[test]
fn nonexistent_scope_fails_closed() {
    let root = repository();

    let output = command(&root)
        .args(["check", "missing-directory"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: scope missing-directory does not exist"));
}

#[test]
fn staged_scope_must_exist_in_head_or_index() {
    let root = repository();
    write(&root, "worktree-only.rs", CLEAN_RUST);

    let output = command(&root)
        .args(["check", "--staged", "worktree-only.rs"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: scope worktree-only.rs does not exist"));
}

#[test]
fn staged_scope_can_exist_in_head_and_index_without_a_worktree_file() {
    let root = repository();
    write(&root, "index-only.rs", CLEAN_RUST);
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("index-only.rs")).expect("source should be removed");

    command(&root)
        .args(["check", "--staged", "index-only.rs"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn commit_scope_must_exist_in_the_merge_base_or_head_tree() {
    let root = repository();
    write(&root, "worktree-only.rs", CLEAN_RUST);

    let output = command(&root)
        .args([
            "check",
            "--base",
            "HEAD",
            "--head",
            "HEAD",
            "worktree-only.rs",
        ])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: scope worktree-only.rs does not exist"));
}

#[test]
fn commit_scope_can_exist_only_in_the_commit_trees() {
    let root = repository();
    write(&root, "tree-only.rs", CLEAN_RUST);
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("tree-only.rs")).expect("source should be removed");

    command(&root)
        .args(["check", "--base", "HEAD", "--head", "HEAD", "tree-only.rs"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn deleted_file_remains_a_valid_scope() {
    let root = repository();
    write(&root, "deleted.rs", SLOPPY_RUST);
    commit_all(&root, "add source");
    fs::remove_file(root.path().join("deleted.rs")).expect("source should be removed");

    command(&root)
        .args(["check", "deleted.rs"])
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[test]
fn supported_index_blob_with_invalid_utf8_fails_closed() {
    let root = repository();
    write(&root, "broken.rs", [0xff, 0xfe]);
    git(&root, ["add", "broken.rs"]);

    let output = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: broken.rs is not valid UTF-8"));
}

#[test]
fn oversized_supported_worktree_file_fails_before_reading() {
    let root = repository();
    let file = File::create(root.path().join("huge.rs")).expect("source should be created");
    file.set_len(17 * 1024 * 1024)
        .expect("large sparse source should be sized");

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("huge.rs is 17825792 bytes; supported source limit is 16777216 bytes"));
}

#[test]
fn oversized_supported_index_blob_fails_after_batch_check() {
    let root = repository();
    let file = File::create(root.path().join("huge.rs")).expect("source should be created");
    file.set_len(17 * 1024 * 1024)
        .expect("large sparse source should be sized");
    git(&root, ["add", "huge.rs"]);

    let output = command(&root)
        .args(["check", "--staged"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("is 17825792 bytes; supported source limit is 16777216 bytes"));
}

#[test]
fn staged_drains_cat_file_while_requesting_more_than_a_pipe_of_unique_blobs() {
    const UNIQUE_BLOB_COUNT: usize = 4_096;

    let _guard = serialize_resource_intensive_git_test();
    let root = repository();
    for index in 0..UNIQUE_BLOB_COUNT {
        write(
            &root,
            format!("source-{index:04}.rs"),
            format!("const VALUE_{index}: usize = {index};\n"),
        );
    }
    git(&root, ["add", "--all"]);

    let mut command = command(&root);
    command.timeout(Duration::from_secs(30));
    command
        .args(["check", "--staged"])
        .assert()
        .code(0)
        .stdout(format!("clean: {UNIQUE_BLOB_COUNT} files scanned\n"));
}

#[test]
fn unsupported_untracked_binary_is_skipped() {
    let root = repository();
    write(&root, "asset.bin", [0xff, 0xfe]);

    command(&root)
        .arg("check")
        .assert()
        .code(0)
        .stdout("clean: 0 files scanned\n");
}

#[cfg(unix)]
#[test]
fn supported_untracked_symlink_fails_closed() {
    let root = repository();
    write(&root, "target.txt", CLEAN_RUST);
    std::os::unix::fs::symlink("target.txt", root.path().join("linked.rs"))
        .expect("symlink should be created");

    let output = command(&root)
        .arg("check")
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: supported path linked.rs is not a regular file"));
}

#[test]
fn check_modes_are_mutually_exclusive_and_head_requires_base() {
    let root = repository();

    command(&root)
        .args(["check", "--all", "--staged"])
        .assert()
        .code(2);
    command(&root)
        .args(["check", "--staged", "--base", "HEAD"])
        .assert()
        .code(2);
    command(&root)
        .args(["check", "--head", "HEAD"])
        .assert()
        .code(2);
}
