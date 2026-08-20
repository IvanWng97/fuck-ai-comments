use std::fs;
use std::fs::File;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::MAIN_SEPARATOR;
use std::process::{Command as ProcessCommand, Stdio};

use assert_cmd::Command;
use tempfile::TempDir;

fn command(root: &TempDir) -> Command {
    let mut command = assert_cmd::cargo::cargo_bin_cmd!("fuck-ai-comments");
    command.current_dir(root.path());
    command
}

#[test]
fn check_help_describes_the_empty_baseline_for_unborn_branches() {
    let root = TempDir::new().expect("temporary directory should be created");
    let output = command(&root)
        .args(["check", "--help"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    let normalized = stdout.split_whitespace().collect::<Vec<_>>().join(" ");

    assert!(normalized.contains("The baseline is HEAD, or empty on an unborn branch."));
    assert!(normalized.contains(
        "Compare the Git index with HEAD, or with an empty baseline on an unborn branch"
    ));
}

#[cfg(unix)]
fn assert_all_rejects_nonregular(root: &TempDir, path: &str) {
    let output = command(root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains(&format!(
        "error: supported path {path} is not a regular file"
    )));
}

#[test]
fn all_reports_findings_in_stable_path_order() {
    let root = TempDir::new().expect("temporary directory should be created");
    let source = "// first\n// second\n// third\n// fourth\nconst LIMIT: usize = 4;\n";
    fs::create_dir(root.path().join("nested")).expect("nested directory should be created");
    fs::write(root.path().join("nested/z.rs"), source).expect("nested z.rs should be written");
    fs::write(root.path().join("a.rs"), source).expect("a.rs should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    let a = stdout
        .find("a.rs:1:")
        .expect("a.rs finding should be printed");
    let separator = if MAIN_SEPARATOR == '\\' { "\\\\" } else { "/" };
    let nested_finding = format!("nested{separator}z.rs:1:");
    let z = stdout
        .find(&nested_finding)
        .expect("nested z.rs finding should be printed");
    assert!(a < z, "findings were not path-sorted:\n{stdout}");
    assert!(stdout.contains("comment-policy/leaf-comment-budget"));
    assert!(stdout.contains("2 violations in 2 files"));
}

#[test]
fn all_uses_cargo_metadata_for_a_custom_library_root() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir(root.path().join("custom")).expect("custom directory should be created");
    fs::write(
        root.path().join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"custom-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    )
    .expect("Cargo.toml should be written");
    fs::write(
        root.path().join("custom/root.rs"),
        concat!(
            "//! detail one\n",
            "//! detail two\n",
            "//! detail three\n",
            "//! detail four\n",
            "//! detail five\n",
            "//! detail six\n",
            "pub fn work() {}\n",
        ),
    )
    .expect("custom library root should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 2 files scanned\n");
}

#[test]
fn all_uses_workspace_metadata_for_a_member_custom_library_root() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir_all(root.path().join("crates/member/source"))
        .expect("workspace member directory should be created");
    fs::write(
        root.path().join("Cargo.toml"),
        concat!(
            "[workspace]\n",
            "members = [\"crates/member\"]\n",
            "resolver = \"3\"\n",
        ),
    )
    .expect("workspace Cargo.toml should be written");
    fs::write(
        root.path().join("crates/member/Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"workspace-member\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"source/root.rs\"\n",
        ),
    )
    .expect("member Cargo.toml should be written");
    fs::write(
        root.path().join("crates/member/source/root.rs"),
        concat!(
            "//! detail one\n",
            "//! detail two\n",
            "//! detail three\n",
            "//! detail four\n",
            "//! detail five\n",
            "//! detail six\n",
            "pub fn work() {}\n",
        ),
    )
    .expect("member custom library root should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 3 files scanned\n");
}

#[test]
fn all_fails_closed_when_a_detected_cargo_workspace_cannot_be_resolved() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("Cargo.toml"),
        "[package]\nname = \"missing-required-fields\"\n",
    )
    .expect("invalid Cargo manifest should be written");
    fs::write(root.path().join("clean.rs"), "pub fn work() {}\n")
        .expect("clean Rust should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("Cargo metadata failed for detected manifest"));
}

#[test]
fn closed_stdout_preserves_the_violation_exit_code_without_panicking() {
    let root = TempDir::new().expect("temporary directory should be created");
    let source = "// first\n// second\n// third\n// fourth\nconst LIMIT: usize = 4;\n";
    for index in 0..128 {
        fs::write(root.path().join(format!("source-{index:03}.rs")), source)
            .expect("source should be written");
    }
    let mut child = ProcessCommand::new(assert_cmd::cargo::cargo_bin!("fuck-ai-comments"))
        .current_dir(root.path())
        .args(["check", "--all", "."])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("command should start");
    drop(child.stdout.take());

    let output = child
        .wait_with_output()
        .expect("command should finish after stdout closes");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert_eq!(output.status.code(), Some(1), "unexpected stderr: {stderr}");
}

#[test]
fn closed_stdout_preserves_the_clean_exit_code_without_panicking() {
    let root = TempDir::new().expect("temporary directory should be created");
    for index in 0..128 {
        fs::write(
            root.path().join(format!("source-{index:03}.rs")),
            format!("const VALUE_{index}: usize = {index};\n"),
        )
        .expect("source should be written");
    }
    let mut child = ProcessCommand::new(assert_cmd::cargo::cargo_bin!("fuck-ai-comments"))
        .current_dir(root.path())
        .args(["check", "--all", "."])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("command should start");
    drop(child.stdout.take());

    let output = child
        .wait_with_output()
        .expect("command should finish after stdout closes");
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert_eq!(output.status.code(), Some(0), "unexpected stderr: {stderr}");
}

#[test]
fn all_honors_gitignore_and_ignore_filters() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(root.path().join("clean.rs"), "const LIMIT: usize = 4;\n")
        .expect("clean.rs should be written");
    fs::write(root.path().join(".gitignore"), "git-ignored.rs\n")
        .expect(".gitignore should be written");
    fs::write(root.path().join(".ignore"), "ignore-ignored.py\n")
        .expect(".ignore should be written");
    fs::write(root.path().join("git-ignored.rs"), "fn {").expect("ignored Rust should be written");
    fs::write(root.path().join("ignore-ignored.py"), "def (")
        .expect("ignored Python should be written");
    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert_eq!(stdout, "clean: 1 file scanned\n");
}

#[test]
fn all_scans_new_language_adapters_in_one_folder() {
    let root = TempDir::new().expect("temporary directory should be created");
    let sources = [
        ("Renderer.m", "@implementation Renderer\n@end\n"),
        ("Renderer.swift", "let value = 1\n"),
        ("Renderer.kt", "val value = 1\n"),
        (
            "Renderer.tsx",
            "export const Renderer = () => <main>Hello</main>;\n",
        ),
    ];
    for (path, source) in sources {
        fs::write(root.path().join(path), source).expect("source should be written");
    }

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert_eq!(stdout, "clean: 4 files scanned\n");
}

#[test]
fn all_scans_hidden_supported_files() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir(root.path().join(".github")).expect("hidden directory should be created");
    fs::write(root.path().join(".github/broken.js"), "function {")
        .expect("hidden JavaScript should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: could not analyze"));
    assert!(stderr.contains("broken.js"));
}

#[cfg(unix)]
#[test]
fn all_fails_closed_on_supported_symlinks() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(root.path().join("target.txt"), "const LIMIT = 4;\n")
        .expect("target should be written");
    std::os::unix::fs::symlink("target.txt", root.path().join("linked.js"))
        .expect("symlink should be created");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: supported path linked.js is not a regular file"));
}

#[cfg(unix)]
#[test]
fn all_fails_closed_on_supported_fifo_and_socket_paths() {
    let fifo_root = TempDir::new().expect("temporary directory should be created");
    let fifo_path = fifo_root.path().join("events.rs");
    let status = ProcessCommand::new("mkfifo")
        .arg(&fifo_path)
        .status()
        .expect("mkfifo should run");
    assert!(status.success(), "mkfifo should create the fixture");
    assert_all_rejects_nonregular(&fifo_root, "events.rs");

    let socket_root = TempDir::new().expect("temporary directory should be created");
    let socket_path = socket_root.path().join("events.py");
    let _listener = UnixListener::bind(&socket_path).expect("Unix socket should be bound");
    assert_all_rejects_nonregular(&socket_root, "events.py");
}

#[test]
fn all_fails_closed_on_parse_errors() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(root.path().join("broken.rs"), "fn {").expect("broken.rs should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: could not analyze broken.rs"));
    assert!(stderr.contains("could not parse broken.rs as Rust"));
}

#[test]
fn all_fails_closed_on_invalid_ignore_rules() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(root.path().join(".ignore"), "[z-a]\n").expect(".ignore should be written");
    fs::write(root.path().join("clean.rs"), "const LIMIT: usize = 4;\n")
        .expect("clean.rs should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: could not apply ignore rules at ."));
    assert!(stderr.contains("invalid range"));
}

#[test]
fn all_fails_closed_on_non_utf8_supported_files() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(root.path().join("broken.py"), [0xff, 0xfe]).expect("broken.py should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("error: broken.py is not valid UTF-8"));
}

#[test]
fn all_skips_unsupported_files_before_reading_them() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(root.path().join("binary.bin"), [0xff, 0xfe]).expect("binary file should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert_eq!(stdout, "clean: 0 files scanned\n");
}

#[test]
fn all_rejects_the_attestation_profile() {
    let root = TempDir::new().expect("temporary directory should be created");

    let output = command(&root)
        .args(["check", "--all", "--profile", "attestation", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("--all cannot use the attestation profile"));
}

#[test]
fn cli_rejects_an_unknown_analysis_profile() {
    let root = TempDir::new().expect("temporary directory should be created");

    let output = command(&root)
        .args(["check", "--profile", "unknown"])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("invalid value 'unknown' for '--profile <PROFILE>'"));
}

#[test]
fn all_fails_closed_before_reading_an_oversized_supported_file() {
    let root = TempDir::new().expect("temporary directory should be created");
    let file = File::create(root.path().join("huge.rs")).expect("source should be created");
    file.set_len(17 * 1024 * 1024)
        .expect("large sparse source should be sized");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(2)
        .get_output()
        .clone();
    let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

    assert!(stderr.contains("huge.rs is 17825792 bytes; supported source limit is 16777216 bytes"));
}
