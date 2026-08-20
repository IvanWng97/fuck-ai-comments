use std::fs;
use std::fs::File;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::{MAIN_SEPARATOR, Path};
use std::process::{Command as ProcessCommand, Stdio};

use assert_cmd::Command;
use tempfile::TempDir;

fn command(root: &TempDir) -> Command {
    let mut command = assert_cmd::cargo::cargo_bin_cmd!("fuck-ai-comments");
    command.current_dir(root.path());
    command
}

fn rendered_path(path: &str) -> String {
    if MAIN_SEPARATOR == '\\' {
        path.replace('/', "\\\\")
    } else {
        path.to_owned()
    }
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
    let nested_finding = format!("{}:1:", rendered_path("nested/z.rs"));
    let z = stdout
        .find(&nested_finding)
        .expect("nested z.rs finding should be printed");
    assert!(a < z, "findings were not path-sorted:\n{stdout}");
    assert!(stdout.contains("comment-policy/leaf-comment-budget"));
    assert!(stdout.contains("2 violations in 2 files"));
}

#[test]
fn all_honors_an_unlimited_rustdoc_policy_from_the_repository_config() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "\n",
            "[comments.rustdoc]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("private.rs"),
        concat!(
            "/// First line of internal documentation.\n",
            "/// Second line of internal documentation.\n",
            "pub(crate) fn helper() {}\n",
        ),
    )
    .expect("private Rust source should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn rustdoc_policy_does_not_exempt_unattached_doc_syntax() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "\n",
            "[comments.rustdoc]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("unattached.rs"),
        concat!(
            "/// This syntax is not attached to an item.\n",
            "/// It must remain ordinary narrative.\n",
            "/// A third line still does not make it documentation.\n",
            "/// A fourth line must stay within narrative policy.\n",
            "// This ordinary comment prevents attachment to the item.\n",
            "const VALUE: usize = 2;\n",
        ),
    )
    .expect("Rust source should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("unattached.rs:1: comment-policy/leaf-comment-budget"));
}

#[test]
fn rustdoc_policy_does_not_exempt_inner_doc_syntax_outside_module_bodies() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "[comments.rustdoc]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("nested.rs"),
        concat!(
            "mod internal {\n",
            "    trait Contract {\n",
            "        //! This is not a module-level inner doc.\n",
            "        //! It must remain ordinary narrative.\n",
            "        //! A third line is still not module documentation.\n",
            "        //! A fourth line must stay within narrative policy.\n",
            "        fn call();\n",
            "    }\n",
            "}\n",
        ),
    )
    .expect("Rust source should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("nested.rs:3: comment-policy/comment-block-budget"));
}

#[test]
fn rustdoc_policy_recognizes_inner_docs_on_module_bodies() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "[comments.rustdoc]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("module.rs"),
        concat!(
            "mod internal {\n",
            "    #![allow(dead_code)]\n",
            "    //! First line of module documentation.\n",
            "    //! Second line of module documentation.\n",
            "    //! Third line of module documentation.\n",
            "    //! Fourth line of module documentation.\n",
            "    pub(crate) fn helper() {}\n",
            "}\n",
        ),
    )
    .expect("Rust source should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn rustdoc_policy_does_not_exempt_inner_docs_after_items() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "[comments.rustdoc]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("late-file.rs"),
        concat!(
            "fn before() {}\n",
            "//! Late inner doc line one.\n",
            "//! Late inner doc line two.\n",
            "//! Late inner doc line three.\n",
            "//! Late inner doc line four.\n",
        ),
    )
    .expect("late file docs should be written");
    fs::write(
        root.path().join("late-module.rs"),
        concat!(
            "mod internal {\n",
            "    fn before() {}\n",
            "    //! Late module doc line one.\n",
            "    //! Late module doc line two.\n",
            "    //! Late module doc line three.\n",
            "    //! Late module doc line four.\n",
            "}\n",
        ),
    )
    .expect("late module docs should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("late-file.rs:2: comment-policy/comment-block-budget"));
    assert!(stdout.contains("late-module.rs:3: comment-policy/comment-block-budget"));
}

#[test]
fn rustdoc_policy_recognizes_documented_tuple_fields() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "[comments.rustdoc]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("tuple.rs"),
        concat!(
            "pub(crate) struct Pair(\n",
            "    /// First line of field documentation.\n",
            "    /// Second line of field documentation.\n",
            "    /// Third line of field documentation.\n",
            "    /// Fourth line of field documentation.\n",
            "    pub(crate) u8,\n",
            ");\n",
        ),
    )
    .expect("Rust source should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn all_enforces_a_configured_rustdoc_line_cap() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "\n",
            "[comments.rustdoc]\n",
            "policy = \"capped\"\n",
            "max-lines = 1\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("private.rs"),
        concat!(
            "/// First line of internal documentation.\n",
            "/// Second line of internal documentation.\n",
            "pub(crate) fn helper() {}\n",
        ),
    )
    .expect("private Rust source should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("private.rs:1: comment-policy/comment-type-cap"));
    assert!(stdout.contains("2 rustdoc comment lines; configured allowance is 1"));
    assert!(stdout.contains("1 violation in 1 file"));
}

#[test]
fn configured_type_cap_can_raise_the_builtin_owner_ceiling() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "[comments.rustdoc]\n",
            "policy = \"capped\"\n",
            "max-lines = 10\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("private.rs"),
        concat!(
            "/// Documentation line one.\n",
            "/// Documentation line two.\n",
            "/// Documentation line three.\n",
            "/// Documentation line four.\n",
            "/// Documentation line five.\n",
            "/// Documentation line six.\n",
            "/// Documentation line seven.\n",
            "/// Documentation line eight.\n",
            "/// Documentation line nine.\n",
            "/// Documentation line ten.\n",
            "pub(crate) fn helper() {}\n",
        ),
    )
    .expect("private Rust source should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn rustdoc_relative_policy_can_tighten_public_documentation() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir(root.path().join("src")).expect("source directory should be created");
    fs::write(
        root.path().join("Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"configured-public-docs\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
        ),
    )
    .expect("Cargo manifest should be written");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "[comments.rustdoc]\n",
            "policy = \"relative\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("src/lib.rs"),
        concat!(
            "/// First line of public documentation.\n",
            "/// Second line of public documentation.\n",
            "pub fn helper() {}\n",
        ),
    )
    .expect("Rust source should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");
    let finding = format!(
        "{}:1: comment-policy/function-comment-budget",
        rendered_path("src/lib.rs")
    );

    assert!(stdout.contains(&finding), "unexpected stdout:\n{stdout}");
}

#[test]
fn all_rejects_invalid_repository_policy() {
    let cases = [
        (
            "unknown field",
            "schema-version = 1\nunknown = true\n",
            "unknown field `unknown`",
        ),
        (
            "unsupported schema",
            "schema-version = 2\n",
            "unsupported schema-version 2; expected 1",
        ),
        (
            "missing cap",
            concat!(
                "schema-version = 1\n",
                "[comments.rustdoc]\n",
                "policy = \"capped\"\n",
            ),
            "comments.rustdoc.max-lines is required",
        ),
        (
            "zero cap",
            concat!(
                "schema-version = 1\n",
                "[comments.rustdoc]\n",
                "policy = \"capped\"\n",
                "max-lines = 0\n",
            ),
            "comments.rustdoc.max-lines must be greater than zero",
        ),
        (
            "unused cap",
            concat!(
                "schema-version = 1\n",
                "[comments.rustdoc]\n",
                "policy = \"unlimited\"\n",
                "max-lines = 2\n",
            ),
            "comments.rustdoc.max-lines is only valid when policy = \"capped\"",
        ),
    ];

    for (label, config, expected) in cases {
        let root = TempDir::new().expect("temporary directory should be created");
        fs::write(root.path().join("fuck-ai-comments.toml"), config)
            .expect("policy configuration should be written");

        let output = command(&root)
            .args(["check", "--all", "."])
            .assert()
            .code(2)
            .get_output()
            .clone();
        let stderr = String::from_utf8(output.stderr).expect("stderr should be UTF-8");

        assert!(
            stderr.contains("could not load ./fuck-ai-comments.toml"),
            "{label}: unexpected stderr:\n{stderr}"
        );
        assert!(
            stderr.contains(expected),
            "{label}: unexpected stderr:\n{stderr}"
        );
    }
}

#[test]
fn all_uses_an_explicit_policy_configuration() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        "this default config is intentionally invalid\n",
    )
    .expect("default policy configuration should be written");
    fs::write(
        root.path().join("custom-policy.toml"),
        concat!(
            "schema-version = 1\n",
            "\n",
            "[comments.rustdoc]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("private.rs"),
        concat!(
            "/// First line of internal documentation.\n",
            "/// Second line of internal documentation.\n",
            "pub(crate) fn helper() {}\n",
        ),
    )
    .expect("private Rust source should be written");

    command(&root)
        .args(["check", "--all", "--config", "custom-policy.toml", "."])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn all_applies_narrative_policy_to_non_rust_languages() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "\n",
            "[comments.narrative]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("module.py"),
        concat!(
            "# First narrative line.\n",
            "# Second narrative line.\n",
            "# Third narrative line.\n",
            "# Fourth narrative line.\n",
            "VALUE = 4\n",
        ),
    )
    .expect("Python source should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn all_applies_narrative_policy_to_container_and_toml_owners() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "[comments.narrative]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("style.css"),
        concat!(
            "/* First narrative line. */\n",
            "/* Second narrative line. */\n",
            "/* Third narrative line. */\n",
            "/* Fourth narrative line. */\n",
            ".item { color: black; }\n",
        ),
    )
    .expect("CSS source should be written");
    fs::write(
        root.path().join("data.toml"),
        concat!(
            "# First narrative line.\n",
            "# Second narrative line.\n",
            "# Third narrative line.\n",
            "# Fourth narrative line.\n",
            "value = 4\n",
        ),
    )
    .expect("TOML source should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 2 files scanned\n");
}

#[test]
fn all_applies_unlimited_policy_to_structural_safety_proofs() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "\n",
            "[comments.safety-proof]\n",
            "policy = \"unlimited\"\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("safety.rs"),
        concat!(
            "fn read(pointer: *const u8) -> u8 {\n",
            "    // SAFETY: the caller keeps the pointer readable.\n",
            "    // The allocation remains live.\n",
            "    // The pointer is aligned.\n",
            "    // Reading one byte stays in bounds.\n",
            "    // No mutable reference aliases it.\n",
            "    // The address is non-null.\n",
            "    // The provenance remains valid.\n",
            "    // The pointee is initialized.\n",
            "    // The read does not cross the allocation.\n",
            "    unsafe { core::ptr::read(pointer) }\n",
            "}\n",
        ),
    )
    .expect("Rust source should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn all_applies_a_line_cap_to_structural_tool_directives() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::write(
        root.path().join("fuck-ai-comments.toml"),
        concat!(
            "schema-version = 1\n",
            "\n",
            "[comments.tool-directive]\n",
            "policy = \"capped\"\n",
            "max-lines = 1\n",
        ),
    )
    .expect("policy configuration should be written");
    fs::write(
        root.path().join("directives.js"),
        concat!(
            "function report() {\n",
            "  // eslint-disable-next-line no-console\n",
            "  console.log('one');\n",
            "  // eslint-disable-next-line no-console\n",
            "  console.log('two');\n",
            "}\n",
        ),
    )
    .expect("JavaScript source should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    assert!(stdout.contains("directives.js:2: comment-policy/comment-type-cap"));
    assert!(stdout.contains("2 tool-directive comment lines; configured allowance is 1"));
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
fn all_does_not_infer_a_library_root_without_cargo_metadata() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir(root.path().join("src")).expect("source directory should be created");
    fs::write(
        root.path().join("src/lib.rs"),
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
    .expect("ordinary Rust source should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    let finding = format!(
        "{}:1: comment-policy/comment-block-budget",
        rendered_path("src/lib.rs")
    );
    assert!(stdout.contains(&finding), "unexpected stdout:\n{stdout}");
}

#[test]
fn all_discovers_a_manifest_hidden_by_a_file_only_ignore_rule() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir_all(root.path().join("rust/custom"))
        .expect("nested custom directory should be created");
    fs::write(root.path().join(".ignore"), "**/Cargo.toml\n")
        .expect("ignore rules should be written");
    fs::write(
        root.path().join("rust/Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"file-ignored-manifest\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    )
    .expect("nested Cargo.toml should be written");
    fs::write(
        root.path().join("rust/custom/root.rs"),
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
        .stdout("clean: 1 file scanned\n");
}

#[test]
fn all_discovers_a_nested_cargo_project_without_a_root_manifest() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir_all(root.path().join("rust/custom"))
        .expect("nested custom directory should be created");
    fs::write(
        root.path().join("rust/Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"nested-custom-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    )
    .expect("nested Cargo.toml should be written");
    fs::write(
        root.path().join("rust/custom/root.rs"),
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
    .expect("nested custom library root should be written");

    command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(0)
        .stdout("clean: 2 files scanned\n");
}

#[test]
fn all_keeps_nested_workspace_roots_in_repository_coordinates() {
    let root = TempDir::new().expect("temporary directory should be created");
    fs::create_dir_all(root.path().join("rust/custom"))
        .expect("nested custom directory should be created");
    fs::create_dir(root.path().join("custom"))
        .expect("top-level custom directory should be created");
    fs::write(
        root.path().join("rust/Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"nested-coordinate-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    )
    .expect("nested Cargo.toml should be written");
    let inner_docs = concat!(
        "//! detail one\n",
        "//! detail two\n",
        "//! detail three\n",
        "//! detail four\n",
        "//! detail five\n",
        "//! detail six\n",
        "pub fn work() {}\n",
    );
    fs::write(root.path().join("rust/custom/root.rs"), inner_docs)
        .expect("nested custom library root should be written");
    fs::write(root.path().join("custom/root.rs"), inner_docs)
        .expect("ordinary top-level module should be written");

    let output = command(&root)
        .args(["check", "--all", "."])
        .assert()
        .code(1)
        .get_output()
        .clone();
    let stdout = String::from_utf8(output.stdout).expect("stdout should be UTF-8");

    let ordinary_finding = format!("{}:1:", rendered_path("custom/root.rs"));
    let library_finding = format!("{}:1:", rendered_path("rust/custom/root.rs"));
    assert!(
        stdout.contains(&ordinary_finding),
        "unexpected stdout:\n{stdout}"
    );
    assert!(!stdout.contains(&library_finding));
}

#[test]
fn all_normalizes_a_parent_relative_repository_path() {
    let parent = TempDir::new().expect("temporary directory should be created");
    fs::create_dir_all(parent.path().join("repo/custom"))
        .expect("custom directory should be created");
    fs::create_dir(parent.path().join("sibling")).expect("sibling directory should be created");
    fs::write(
        parent.path().join("repo/Cargo.toml"),
        concat!(
            "[package]\n",
            "name = \"parent-relative-root\"\n",
            "version = \"0.1.0\"\n",
            "edition = \"2024\"\n",
            "\n",
            "[lib]\n",
            "path = \"custom/root.rs\"\n",
        ),
    )
    .expect("Cargo.toml should be written");
    fs::write(
        parent.path().join("repo/custom/root.rs"),
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
    let mut command = assert_cmd::cargo::cargo_bin_cmd!("fuck-ai-comments");
    command.current_dir(parent.path().join("sibling"));

    command
        .args(["check", "--all"])
        .arg(Path::new("..").join("repo"))
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
