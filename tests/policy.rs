use std::collections::BTreeSet;
use std::path::Path;

use fuck_ai_comments::{AnalysisError, Selection, analyze_file};

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
fn rust_leading_comments_belong_to_the_function() {
    let source = concat!(
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "fn work() {\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "leading comments must not sit outside the function owner: {findings:#?}"
    );
}

#[test]
fn rust_public_function_docs_are_not_narrative() {
    let source = concat!(
        "/// First public contract detail.\n",
        "/// Second public contract detail.\n",
        "/// Third public contract detail.\n",
        "/// Fourth public contract detail.\n",
        "pub fn work() {\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "public rustdoc belongs to the documentation contract: {findings:#?}"
    );
}

#[test]
fn rust_private_function_rustdoc_is_narrative() {
    let source = concat!(
        "/// first\n",
        "/// second\n",
        "/// third\n",
        "/// fourth\n",
        "fn work() {\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "private rustdoc syntax must not bypass narrative policy: {findings:#?}"
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
fn rust_const_initializer_rejects_four_internal_comment_lines() {
    let source = concat!(
        "const LIMIT: usize = {\n",
        "    // first\n",
        "    // second\n",
        "    // third\n",
        "    // fourth\n",
        "    4\n",
        "};\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "comments inside a leaf initializer must belong to that leaf: {findings:#?}"
    );
}

#[test]
fn blank_line_prevents_file_header_from_becoming_leaf_comment() {
    let source = concat!(
        "// Copyright first line\n",
        "// Copyright second line\n",
        "// Copyright third line\n",
        "// Copyright fourth line\n",
        "\n",
        "const LIMIT: usize = 4;\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/leaf-comment-budget"),
        "a blank line separates file metadata from the next leaf: {findings:#?}"
    );
}

#[test]
fn orphan_file_comment_block_is_still_gated() {
    let source = concat!(
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "\n",
        "fn work() {}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
        "unowned comments must fall back to the file owner: {findings:#?}"
    );
}

#[test]
fn file_owner_accepts_two_narrative_lines() {
    let source = concat!("// first\n", "// second\n", "\n", "fn work() {}\n");

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "two file-level rationale lines are within the minimum allowance: {findings:#?}"
    );
}

#[test]
fn javascript_directive_cannot_hide_leaf_narrative() {
    let source = concat!(
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "// eslint-disable-next-line no-restricted-syntax\n",
        "const limit = 4;\n",
    );

    let findings = analyze_file(Path::new("limits.js"), source, &Selection::all(source))
        .expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a tool directive must not act as an ownership barrier: {findings:#?}"
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
fn changing_one_function_comment_does_not_attest_another() {
    let source = concat!(
        "fn work() {\n",
        "    // reviewed first rationale\n",
        "    first();\n",
        "    // unchanged second rationale\n",
        "    changed_second();\n",
        "}\n",
    );
    let selection = Selection {
        changed: BTreeSet::from([2, 5]),
        owners: BTreeSet::from([2, 5]),
    };

    let findings =
        analyze_file(Path::new("src/lib.rs"), source, &selection).expect("valid Rust should parse");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [4],
        "each surviving comment needs its own attestation"
    );
}

#[test]
fn adding_comment_does_not_attest_existing_function_comments() {
    let source = concat!(
        "fn work() {\n",
        "    // unchanged first rationale\n",
        "    first();\n",
        "    // unchanged second rationale\n",
        "    changed_second();\n",
        "    // new third rationale\n",
        "    third();\n",
        "}\n",
    );
    let selection = Selection {
        changed: BTreeSet::from([5, 6]),
        owners: BTreeSet::from([5, 6]),
    };

    let findings =
        analyze_file(Path::new("src/lib.rs"), source, &selection).expect("valid Rust should parse");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [2, 4],
        "a new comment cannot wash unchanged comments"
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
fn rust_safety_prefix_without_unsafe_code_is_narrative() {
    let source = concat!(
        "fn read() -> u8 {\n",
        "    // SAFETY: this label is not attached to unsafe code.\n",
        "    // second narrative line\n",
        "    // third narrative line\n",
        "    1\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "a magic SAFETY prefix must not bypass policy without unsafe code: {findings:#?}"
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
fn python_function_docstring_consumes_comment_budget() {
    let source = concat!(
        "def work():\n",
        "    \"\"\"Describe the first implementation detail.\n",
        "    Describe the second implementation detail.\n",
        "    Describe the third implementation detail.\"\"\"\n",
        "    return 1\n",
    );

    let findings = analyze_file(Path::new("worker.py"), source, &Selection::all(source))
        .expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected Python docstring to consume the function budget, got {findings:#?}"
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

#[test]
fn html_rejects_four_line_template_comment() {
    let source = "<!-- first\nsecond\nthird\nfourth -->\n<main>hello</main>\n";

    let findings = analyze_file(Path::new("index.html"), source, &Selection::all(source))
        .expect("valid HTML should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
        "expected HTML template budget finding, got {findings:#?}"
    );
}

#[test]
fn css_rejects_four_line_comment() {
    let source = "/* first\nsecond\nthird\nfourth */\nmain { display: block; }\n";

    let findings = analyze_file(Path::new("site.css"), source, &Selection::all(source))
        .expect("valid CSS should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
        "expected CSS comment budget finding, got {findings:#?}"
    );
}

#[test]
fn astro_rejects_four_line_template_comment() {
    let source = "---\nconst title = 'Hello';\n---\n<!-- first\nsecond\nthird\nfourth -->\n<h1>{title}</h1>\n";

    let findings = analyze_file(Path::new("Page.astro"), source, &Selection::all(source))
        .expect("valid Astro should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
        "expected Astro template budget finding, got {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_uses_typescript_owner_policy() {
    let mut source = String::from("---\nfunction work(): number {\n");
    for number in 1..=10 {
        source.push_str(&format!(
            "  // narration {number}\n  const value{number} = {number};\n"
        ));
    }
    source.push_str("  return value10;\n}\n---\n<div>{work()}</div>\n");

    let findings = analyze_file(Path::new("Page.astro"), &source, &Selection::all(&source))
        .expect("valid Astro should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected embedded TypeScript function finding, got {findings:#?}"
    );
}

#[test]
fn html_script_uses_javascript_owner_policy() {
    let mut source = String::from("<script>\nfunction work() {\n");
    for number in 1..=10 {
        source.push_str(&format!(
            "  // narration {number}\n  const value{number} = {number};\n"
        ));
    }
    source.push_str("  return value10;\n}\n</script>\n");

    let findings = analyze_file(Path::new("index.html"), &source, &Selection::all(&source))
        .expect("valid HTML should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected embedded JavaScript function finding, got {findings:#?}"
    );
}

#[test]
fn rust_rejects_three_consecutive_narrative_comments() {
    let source = concat!(
        "fn work() {\n",
        "    // first\n",
        "    // second\n",
        "    // third\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "expected consecutive comment block finding, got {findings:#?}"
    );
}

#[test]
fn rust_trailing_comments_are_not_a_consecutive_comment_block() {
    let source = concat!(
        "fn work() {\n",
        "    let first = 1; // first\n",
        "    let second = 2; // second\n",
        "    consume(first + second); // third\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-block-budget"),
        "code-bearing lines are not a consecutive comment-only block: {findings:#?}"
    );
}

#[test]
fn rust_long_function_accepts_four_separate_rationales() {
    let source = concat!(
        "fn work() {\n",
        "    // first reason\n",
        "    let a = 1;\n",
        "    let b = 2;\n",
        "    let c = 3;\n",
        "    let d = 4;\n",
        "    // second reason\n",
        "    let e = 5;\n",
        "    let f = 6;\n",
        "    let g = 7;\n",
        "    let h = 8;\n",
        "    // third reason\n",
        "    let i = 9;\n",
        "    let j = 10;\n",
        "    let k = 11;\n",
        "    let l = 12;\n",
        "    // fourth reason\n",
        "    let m = 13;\n",
        "    let n = 14;\n",
        "    let o = 15;\n",
        "    consume((a, b, c, d, e, f, g, h, i, j, k, l, m, n, o));\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "a longer function can earn separate rationale lines: {findings:#?}"
    );
}

#[test]
fn malformed_rust_fails_closed() {
    let source = "fn broken( {\n";

    let error = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect_err("malformed Rust must not be analyzed heuristically");

    assert!(
        matches!(
            error,
            AnalysisError::Parse {
                language: "Rust",
                ..
            }
        ),
        "expected a fail-closed Rust parse error, got {error}"
    );
}

#[test]
fn rust_public_const_doc_comment_is_not_a_private_leaf_narrative() {
    let source = concat!(
        "/// First public contract detail.\n",
        "/// Second public contract detail.\n",
        "/// Third public contract detail.\n",
        "/// Fourth public contract detail.\n",
        "pub const LIMIT: usize = 4;\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "public rustdoc is governed by documentation lints, not the private leaf budget: {findings:#?}"
    );
}

#[test]
fn rust_public_const_plain_comments_still_use_leaf_budget() {
    let source = concat!(
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "pub const LIMIT: usize = 4;\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "public visibility must not exempt non-rustdoc narrative: {findings:#?}"
    );
}

#[test]
fn rust_crate_visible_rustdoc_is_not_public_api_docs() {
    let source = concat!(
        "/// first\n",
        "/// second\n",
        "/// third\n",
        "/// fourth\n",
        "pub(crate) const LIMIT: usize = 4;\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "pub(crate) rustdoc must not bypass private narrative policy: {findings:#?}"
    );
}

#[test]
fn javascript_directive_block_does_not_consume_narrative_budget() {
    let source = concat!(
        "function work() {\n",
        "  // eslint-disable-next-line no-console\n",
        "  console.log('work');\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("worker.js"), source, &Selection::all(source))
        .expect("valid JavaScript should parse");

    assert!(
        findings.is_empty(),
        "tool directives are executable metadata, not narrative: {findings:#?}"
    );
}

#[test]
fn javascript_multiline_eslint_prefix_does_not_hide_narrative() {
    let source = concat!(
        "function work() {\n",
        "  /* eslint-disable no-console\n",
        "   * first narrative line\n",
        "   * second narrative line\n",
        "   * third narrative line */\n",
        "  console.log('work');\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("worker.js"), source, &Selection::all(source))
        .expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "a multiline magic prefix must not exempt narrative: {findings:#?}"
    );
}

#[test]
fn python_noqa_prefix_does_not_hide_following_narrative() {
    let source = concat!(
        "def work():\n",
        "    # noqa\n",
        "    # first narrative line\n",
        "    # second narrative line\n",
        "    return 1\n",
    );

    let findings = analyze_file(Path::new("worker.py"), source, &Selection::all(source))
        .expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "one directive must not exempt adjacent narrative comments: {findings:#?}"
    );
}

#[test]
fn jsx_and_tsx_use_their_registered_grammars() {
    let jsx = "export const View = () => <main>Hello</main>;\n";
    let tsx = "export const View = (): JSX.Element => <main>Hello</main>;\n";

    let jsx_findings = analyze_file(Path::new("View.jsx"), jsx, &Selection::all(jsx))
        .expect("valid JSX should parse");
    let tsx_findings = analyze_file(Path::new("View.tsx"), tsx, &Selection::all(tsx))
        .expect("valid TSX should parse");

    assert!(
        jsx_findings.is_empty() && tsx_findings.is_empty(),
        "registered JSX and TSX grammars should accept component syntax"
    );
}

#[test]
fn html_accepts_three_line_template_comment() {
    let source = "<!-- first\nsecond\nthird -->\n<main>hello</main>\n";

    let findings = analyze_file(Path::new("index.html"), source, &Selection::all(source))
        .expect("valid HTML should parse");

    assert!(
        findings.is_empty(),
        "three lines are within the template allowance: {findings:#?}"
    );
}

#[test]
fn embedded_javascript_finding_reports_container_line() {
    let source = concat!(
        "<header>heading</header>\n",
        "<script>\n",
        "function work() {\n",
        "  // one\n",
        "  const one = 1;\n",
        "  // two\n",
        "  const two = 2;\n",
        "}\n",
        "</script>\n",
    );

    let findings = analyze_file(Path::new("index.html"), source, &Selection::all(source))
        .expect("valid HTML and JavaScript should parse");
    let finding = findings
        .iter()
        .find(|finding| finding.rule == "comment-policy/function-comment-budget")
        .expect("short embedded function should exceed its budget");

    assert_eq!(finding.line, 4, "finding line should map to the HTML file");
}

#[test]
fn html_json_script_is_not_parsed_as_javascript() {
    let source = concat!(
        "<script type=\"application/json\">\n",
        "{\"template\": \"{{ this is not JavaScript }}\"}\n",
        "</script>\n",
    );

    let findings = analyze_file(Path::new("index.html"), source, &Selection::all(source))
        .expect("JSON script data should not enter the JavaScript adapter");

    assert!(findings.is_empty(), "JSON data has no comment owners");
}

#[test]
fn javascript_generator_rejects_excessive_comments() {
    let source = concat!(
        "function* work() {\n",
        "  // first\n",
        "  yield 1;\n",
        "  // second\n",
        "  yield 2;\n",
        "}\n",
    );

    let findings = analyze_file(Path::new("worker.js"), source, &Selection::all(source))
        .expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "generator functions must not bypass the function budget: {findings:#?}"
    );
}

#[test]
fn rust_closure_rejects_excessive_comments() {
    let source = concat!(
        "const WORK: fn() = || {\n",
        "    // first\n",
        "    run();\n",
        "    // second\n",
        "    finish();\n",
        "};\n",
    );

    let findings = analyze_file(Path::new("src/lib.rs"), source, &Selection::all(source))
        .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "closures must not bypass the function budget: {findings:#?}"
    );
}
