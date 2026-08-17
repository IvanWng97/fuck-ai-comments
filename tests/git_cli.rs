use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::path::Path;
use std::process::Command as ProcessCommand;

use assert_cmd::Command;
use tempfile::TempDir;

const CLEAN_RUST: &str = "const LIMIT: usize = 4;\n";
const SLOPPY_RUST: &str = "// First explanation.\n// Second explanation.\n// Third explanation.\n// Fourth explanation.\nconst LIMIT: usize = 4;\n";
const STALE_BEFORE: &str =
    "fn limit() -> usize {\n    // This boundary matches the external protocol.\n    1\n}\n";
const STALE_AFTER: &str =
    "fn limit() -> usize {\n    // This boundary matches the external protocol.\n    2\n}\n";

fn repository() -> TempDir {
    let root = TempDir::new().expect("temporary repository should be created");
    git(&root, ["init", "--quiet"]);
    git(&root, ["config", "user.email", "tests@example.com"]);
    git(&root, ["config", "user.name", "Test User"]);
    git(
        &root,
        ["commit", "--quiet", "--allow-empty", "-m", "initial"],
    );
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
fn cross_language_rename_analyzes_the_new_file_as_added() {
    let root = repository();
    let shared_source = "# First explanation.\n# Second explanation.\n# Third explanation.\n# Fourth explanation.\nvalue = 1\n";
    write(&root, "config.toml", shared_source);
    commit_all(&root, "add config");
    git(&root, ["mv", "--", "config.toml", "config.py"]);

    let output = command(&root)
        .arg("check")
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(
        stdout.contains("config.py:1: comment-policy/file-comment-budget"),
        "unexpected output: {stdout:?}"
    );
}

#[test]
fn literal_pathspecs_handle_git_metacharacters_and_control_characters() {
    let root = repository();
    let path = "-:(exclude)*雪\nsource.rs";
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
    assert!(stdout.contains("\\nsource.rs:1:"));
}

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
