use std::collections::BTreeSet;
use std::path::Path;

use fuck_ai_comments::{Selection, analyze_file};

#[test]
fn rust_short_function_rejects_ten_comment_lines() {
    let source = r#"
fn render() {
    // one
    let one = 1;
    // two
    let two = 2;
    // three
    let three = 3;
    // four
    let four = 4;
    // five
    let five = 5;
    // six
    let six = 6;
    // seven
    let seven = 7;
    // eight
    let eight = 8;
    // nine
    let nine = 9;
    // ten
    let ten = 10;
}
"#;

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected function budget finding, got {findings:#?}"
    );
}

#[test]
fn rust_private_const_rejects_four_comment_lines() {
    let source = r#"
// first
// second
// third
// fourth
const RETRY_LIMIT: usize = 3;
"#;

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "expected leaf budget finding, got {findings:#?}"
    );
}

#[test]
fn rust_private_const_accepts_three_comment_lines() {
    let source = r#"
// first
// second
// third
const RETRY_LIMIT: usize = 3;
"#;

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "three lines are within the leaf allowance: {findings:#?}"
    );
}

#[test]
fn rust_const_change_rejects_unchanged_comment() {
    let source = "// Coupled to the upstream retry window.\nconst RETRY_LIMIT: usize = 4;\n";
    let selection = Selection {
        changed: BTreeSet::from([2]),
        owners: BTreeSet::from([2]),
    };

    let findings =
        analyze_file(Path::new("src/lib.rs"), source, &selection).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-owner-changed"),
        "expected stale-owner finding, got {findings:#?}"
    );
}

#[test]
fn rust_const_change_accepts_edited_comment() {
    let source = "// Coupled to the new upstream retry window.\nconst RETRY_LIMIT: usize = 4;\n";
    let selection = Selection {
        changed: BTreeSet::from([1, 2]),
        owners: BTreeSet::from([1, 2]),
    };

    let findings =
        analyze_file(Path::new("src/lib.rs"), source, &selection).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "editing the comment attests that it was reviewed: {findings:#?}"
    );
}

#[test]
fn rust_safety_block_does_not_consume_narrative_budget() {
    let source = r#"
fn read(ptr: *const u8) -> u8 {
    // SAFETY: the caller guarantees that ptr is valid.
    // It is aligned for u8.
    // It remains alive for this read.
    unsafe { *ptr }
}
"#;

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "a SAFETY proof is executable-contract metadata: {findings:#?}"
    );
}

#[test]
fn python_short_function_rejects_ten_spread_comment_lines() {
    let mut source = String::from("def work():\n");
    for number in 1..=10 {
        source.push_str(&format!(
            "    # narration {number}\n    value_{number} = {number}\n"
        ));
    }
    source.push_str("    return value_10\n");

    let findings = analyze_file(Path::new("worker.py"), &source, &Selection::all(&source))
        .expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected Python function budget finding, got {findings:#?}"
    );
}

#[test]
fn python_module_constant_rejects_four_comment_lines() {
    let source = "# first\n# second\n# third\n# fourth\nLIMIT = 4\n";

    let findings = analyze_file(Path::new("limits.py"), source, &Selection::all(source))
        .expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "expected Python leaf budget finding, got {findings:#?}"
    );
}

#[test]
fn python_constant_change_rejects_unchanged_comment() {
    let source = "# Coupled to the upstream retry window.\nRETRY_LIMIT = 4\n";
    let selection = Selection {
        changed: BTreeSet::from([2]),
        owners: BTreeSet::from([2]),
    };

    let findings = analyze_file(Path::new("limits.py"), source, &selection)
        .expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-owner-changed"),
        "expected Python stale-owner finding, got {findings:#?}"
    );
}

#[test]
fn typescript_short_function_rejects_ten_spread_comment_lines() {
    let mut source = String::from("export function work(): number {\n");
    for number in 1..=10 {
        source.push_str(&format!(
            "  // narration {number}\n  const value{number} = {number};\n"
        ));
    }
    source.push_str("  return value10;\n}\n");

    let findings = analyze_file(Path::new("worker.ts"), &source, &Selection::all(&source))
        .expect("valid TypeScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected TypeScript function budget finding, got {findings:#?}"
    );
}

#[test]
fn typescript_const_rejects_four_comment_lines() {
    let source = "// first\n// second\n// third\n// fourth\nconst limit = 4;\n";

    let findings = analyze_file(Path::new("limits.ts"), source, &Selection::all(source))
        .expect("valid TypeScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "expected TypeScript leaf budget finding, got {findings:#?}"
    );
}

#[test]
fn typescript_const_change_rejects_unchanged_comment() {
    let source = "// Coupled to the upstream retry window.\nconst retryLimit = 4;\n";
    let selection = Selection {
        changed: BTreeSet::from([2]),
        owners: BTreeSet::from([2]),
    };

    let findings = analyze_file(Path::new("limits.ts"), source, &selection)
        .expect("valid TypeScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-owner-changed"),
        "expected TypeScript stale-owner finding, got {findings:#?}"
    );
}

#[test]
fn toml_key_rejects_four_comment_lines() {
    let source = "# first\n# second\n# third\n# fourth\ntimeout_ms = 200\n";

    let findings = analyze_file(Path::new("config.toml"), source, &Selection::all(source))
        .expect("valid TOML should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "expected TOML leaf budget finding, got {findings:#?}"
    );
}

#[test]
fn toml_key_accepts_three_comment_lines() {
    let source = "# first\n# second\n# third\ntimeout_ms = 200\n";

    let findings = analyze_file(Path::new("config.toml"), source, &Selection::all(source))
        .expect("valid TOML should parse");

    assert!(
        findings.is_empty(),
        "three lines are within the TOML leaf allowance: {findings:#?}"
    );
}

#[test]
fn toml_hashes_inside_strings_are_not_comments() {
    let source = concat!(
        "pattern = \"# one # two # three # four # five\"\n",
        "literal = '# six # seven # eight # nine'\n",
        "multiline = \"\"\"# ten\n# eleven\n# twelve\n# thirteen\"\"\"\n",
    );

    let findings = analyze_file(Path::new("config.toml"), source, &Selection::all(source))
        .expect("valid TOML should parse");

    assert!(
        findings.is_empty(),
        "hashes inside TOML strings are data, not comments: {findings:#?}"
    );
}

#[test]
fn toml_key_change_rejects_unchanged_comment() {
    let source = "# Coupled to the upstream retry window.\ntimeout_ms = 400\n";
    let selection = Selection {
        changed: BTreeSet::from([2]),
        owners: BTreeSet::from([2]),
    };

    let findings = analyze_file(Path::new("config.toml"), source, &selection)
        .expect("valid TOML should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-owner-changed"),
        "expected TOML stale-owner finding, got {findings:#?}"
    );
}
