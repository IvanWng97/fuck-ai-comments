use std::fs;
use std::fs::File;
use std::path::MAIN_SEPARATOR;

use assert_cmd::Command;
use tempfile::TempDir;

fn command(root: &TempDir) -> Command {
    let mut command = assert_cmd::cargo::cargo_bin_cmd!("fuck-ai-comments");
    command.current_dir(root.path());
    command
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
fn all_honors_gitignore_ignore_hidden_and_symlink_filters() {
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
    fs::write(root.path().join(".hidden.rs"), "fn {").expect("hidden Rust should be written");

    #[cfg(unix)]
    std::os::unix::fs::symlink(
        root.path().join("git-ignored.rs"),
        root.path().join("linked.rs"),
    )
    .expect("symlink should be created");

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
