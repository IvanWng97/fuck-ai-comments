use std::path::Path;

use fuck_ai_comments::{AnalysisError, Finding, SourceFile, analyze_all, analyze_change};

fn analyze(path: &Path, source: &str) -> Result<Vec<Finding>, AnalysisError> {
    analyze_all(SourceFile { path, text: source })
}

fn analyze_line_change(
    path: &Path,
    before: &str,
    after: &str,
) -> Result<Vec<Finding>, AnalysisError> {
    analyze_change(
        SourceFile { path, text: before },
        SourceFile { path, text: after },
    )
}

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "leading comments must not sit outside the function owner: {findings:#?}"
    );
}

#[test]
fn rust_comments_before_attributes_belong_to_the_function() {
    let source = concat!(
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "#[inline]\n",
        "fn work() {\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "outer attributes are part of the function declaration boundary: {findings:#?}"
    );
}

#[test]
fn rust_comments_inside_outer_attributes_belong_to_the_function() {
    let source = concat!(
        "#[allow(\n",
        "    // first\n",
        "    dead_code,\n",
        "    /* second */\n",
        "    unused_variables,\n",
        "    // third\n",
        "    unreachable_code,\n",
        "    /* fourth */\n",
        "    unused_mut\n",
        ")]\n",
        "fn work() {\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "an outer attribute's token tree is part of its function owner: {findings:#?}"
    );
}

#[test]
fn rust_public_docs_can_precede_outer_attributes() {
    let source = concat!(
        "/// First public contract detail.\n",
        "/// Second public contract detail.\n",
        "/// Third public contract detail.\n",
        "/// Fourth public contract detail.\n",
        "#[inline]\n",
        "pub fn work() {\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "outer attributes must not detach rustdoc from public API: {findings:#?}"
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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "public rustdoc belongs to the documentation contract: {findings:#?}"
    );
}

#[test]
fn rust_public_docs_inside_a_private_module_are_narrative() {
    let source = concat!(
        "mod internal {\n",
        "    /// detail one\n",
        "    /// detail two\n",
        "    /// detail three\n",
        "    /// detail four\n",
        "    /// detail five\n",
        "    /// detail six\n",
        "    /// detail seven\n",
        "    /// detail eight\n",
        "    /// detail nine\n",
        "    /// detail ten\n",
        "    pub fn work() {}\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "private module ancestry must bound an item's public reachability: {findings:#?}"
    );
}

#[test]
fn rust_public_docs_inside_a_crate_visible_module_are_narrative() {
    let source = concat!(
        "pub(crate) mod internal {\n",
        "    /// detail one\n",
        "    /// detail two\n",
        "    /// detail three\n",
        "    /// detail four\n",
        "    pub fn work() {}\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "pub(crate) module ancestry is not public API: {findings:#?}"
    );
}

#[test]
fn rust_public_docs_inside_nested_public_modules_are_exempt() {
    let source = concat!(
        "pub mod api {\n",
        "    pub mod nested {\n",
        "        /// detail one\n",
        "        /// detail two\n",
        "        /// detail three\n",
        "        /// detail four\n",
        "        /// detail five\n",
        "        /// detail six\n",
        "        /// detail seven\n",
        "        /// detail eight\n",
        "        /// detail nine\n",
        "        /// detail ten\n",
        "        pub fn work() {}\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "bare-pub items through a bare-pub inline module chain are public API: {findings:#?}"
    );
}

#[test]
fn rust_public_docs_inside_a_public_module_under_a_private_module_are_narrative() {
    let source = concat!(
        "mod internal {\n",
        "    pub mod api {\n",
        "        /// detail one\n",
        "        /// detail two\n",
        "        /// detail three\n",
        "        /// detail four\n",
        "        pub fn work() {}\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "every inline module ancestor must be bare pub: {findings:#?}"
    );
}

#[test]
fn rust_public_method_docs_on_a_private_type_are_narrative() {
    let source = concat!(
        "struct Hidden;\n",
        "impl Hidden {\n",
        "    /// detail one\n",
        "    /// detail two\n",
        "    /// detail three\n",
        "    /// detail four\n",
        "    pub fn work() {}\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "bare pub on an inherent method cannot outgrow its private type: {findings:#?}"
    );
}

#[test]
fn rust_public_method_docs_on_a_reachable_type_are_exempt() {
    let source = concat!(
        "pub mod api {\n",
        "    pub struct Visible;\n",
        "    impl Visible {\n",
        "        /// detail one\n",
        "        /// detail two\n",
        "        /// detail three\n",
        "        /// detail four\n",
        "        /// detail five\n",
        "        /// detail six\n",
        "        /// detail seven\n",
        "        /// detail eight\n",
        "        /// detail nine\n",
        "        /// detail ten\n",
        "        pub fn work() {}\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "a local public type through a public module chain proves method reachability: {findings:#?}"
    );
}

#[test]
fn rust_public_field_docs_on_a_private_type_are_narrative() {
    let source = concat!(
        "struct Hidden {\n",
        "    /// detail one\n",
        "    /// detail two\n",
        "    /// detail three\n",
        "    /// detail four\n",
        "    pub value: usize,\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
        "bare pub on a field cannot outgrow its private type: {findings:#?}"
    );
}

#[test]
fn rust_public_field_docs_on_a_reachable_type_are_exempt() {
    let source = concat!(
        "pub mod api {\n",
        "    pub struct Visible {\n",
        "        /// detail one\n",
        "        /// detail two\n",
        "        /// detail three\n",
        "        /// detail four\n",
        "        pub value: usize,\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "a public field of a reachable public type is public API: {findings:#?}"
    );
}

#[test]
fn rust_crate_root_inner_docs_are_exempt() {
    let source = concat!(
        "//! detail one\n",
        "//! detail two\n",
        "//! detail three\n",
        "//! detail four\n",
        "//! detail five\n",
        "//! detail six\n",
        "//! detail seven\n",
        "//! detail eight\n",
        "//! detail nine\n",
        "//! detail ten\n",
        "pub fn work() {}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "lib.rs inner docs describe the public crate root: {findings:#?}"
    );
}

#[test]
fn rust_arbitrary_file_inner_docs_are_narrative() {
    let source = concat!(
        "//! detail one\n",
        "//! detail two\n",
        "//! detail three\n",
        "//! detail four\n",
        "//! detail five\n",
        "//! detail six\n",
        "//! detail seven\n",
        "//! detail eight\n",
        "//! detail nine\n",
        "pub fn work() {}\n",
    );

    let findings = analyze(Path::new("src/internal.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a standalone module file cannot prove its own public reachability: {findings:#?}"
    );
}

#[test]
fn rust_public_enum_variant_docs_are_public_api_docs() {
    let source = concat!(
        "pub enum State {\n",
        "    /// Waiting for work.\n",
        "    Idle,\n",
        "    /// Processing an item.\n",
        "    Running,\n",
        "    /// Finishing successfully.\n",
        "    Complete,\n",
        "    /// Stopped after an error.\n",
        "    Failed,\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "public enum variants inherit their enum's API visibility: {findings:#?}"
    );
}

#[test]
fn rust_public_enum_variant_docs_require_public_module_ancestry() {
    let source = concat!(
        "mod internal {\n",
        "    pub enum State {\n",
        "        /// detail one\n",
        "        /// detail two\n",
        "        /// detail three\n",
        "        /// detail four\n",
        "        Hidden,\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
        "a public enum inside a private module is not public API: {findings:#?}"
    );
}

#[test]
fn rust_public_enum_variant_docs_in_nested_public_modules_are_exempt() {
    let source = concat!(
        "pub mod api {\n",
        "    pub mod nested {\n",
        "        pub enum State {\n",
        "            /// detail one\n",
        "            /// detail two\n",
        "            /// detail three\n",
        "            /// detail four\n",
        "            Visible,\n",
        "        }\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "public enum variants inherit visibility through a public module chain: {findings:#?}"
    );
}

#[test]
fn rust_public_trait_member_docs_are_public_api_docs() {
    let source = concat!(
        "pub trait Worker {\n",
        "    /// detail one\n",
        "    /// detail two\n",
        "    /// detail three\n",
        "    /// detail four\n",
        "    fn work(&self);\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "trait members inherit a reachable public trait's visibility: {findings:#?}"
    );
}

#[test]
fn rust_trait_member_docs_require_public_module_ancestry() {
    let source = concat!(
        "mod internal {\n",
        "    pub trait Worker {\n",
        "        /// detail one\n",
        "        /// detail two\n",
        "        /// detail three\n",
        "        /// detail four\n",
        "        fn work(&self);\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
        "a trait inside a private module cannot export member docs: {findings:#?}"
    );
}

#[test]
fn rust_inner_docs_in_an_inline_public_module_are_exempt() {
    let source = concat!(
        "pub mod api {\n",
        "    //! detail one\n",
        "    //! detail two\n",
        "    //! detail three\n",
        "    //! detail four\n",
        "    //! detail five\n",
        "    //! detail six\n",
        "    //! detail seven\n",
        "    //! detail eight\n",
        "    //! detail nine\n",
        "    pub fn work() {}\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "inner docs inherit a proven public inline-module chain: {findings:#?}"
    );
}

#[test]
fn rust_inner_docs_in_a_crate_visible_inline_module_are_narrative() {
    let source = concat!(
        "pub(crate) mod internal {\n",
        "    //! detail one\n",
        "    //! detail two\n",
        "    //! detail three\n",
        "    //! detail four\n",
        "    pub fn work() {}\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "inner-doc syntax cannot make a pub(crate) module public: {findings:#?}"
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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "private rustdoc syntax must not bypass narrative policy: {findings:#?}"
    );
}

#[test]
fn rust_inner_doc_syntax_inside_a_function_is_narrative() {
    let source = concat!(
        "fn work() {\n",
        "    //! first\n",
        "    //! second\n",
        "    //! third\n",
        "    //! fourth\n",
        "    run();\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "inner-doc punctuation is not a public-doc escape hatch inside code: {findings:#?}"
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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("limits.js"), source).expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a tool directive must not act as an ownership barrier: {findings:#?}"
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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "a magic SAFETY prefix must not bypass policy without unsafe code: {findings:#?}"
    );
}

#[test]
fn rust_safety_prefix_cannot_exempt_nine_following_comment_lines() {
    let source = concat!(
        "fn read(ptr: *const u8) -> u8 {\n",
        "    // SAFETY: the caller guarantees that ptr is valid.\n",
        "    // detail two\n",
        "    // detail three\n",
        "    // detail four\n",
        "    // detail five\n",
        "    // detail six\n",
        "    // detail seven\n",
        "    // detail eight\n",
        "    // detail nine\n",
        "    // detail ten\n",
        "    unsafe { *ptr }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "a SAFETY prefix must not launder an unbounded prose block: {findings:#?}"
    );
}

#[test]
fn rust_safety_comment_must_not_attach_to_an_unsafe_descendant() {
    let source = concat!(
        "fn read(ptr: *const u8) -> u8 {\n",
        "    // SAFETY: this is not adjacent to the unsafe operation.\n",
        "    // second narrative line\n",
        "    // third narrative line\n",
        "    if ptr.is_null() { 0 } else { unsafe { *ptr } }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "an unsafe descendant of the next statement is not direct attachment: {findings:#?}"
    );
}

#[test]
fn rust_safety_comment_must_be_physically_adjacent_to_unsafe_code() {
    let source = concat!(
        "fn read(ptr: *const u8) -> u8 {\n",
        "    // SAFETY: the blank line detaches this explanation.\n",
        "    // second narrative line\n",
        "    // third narrative line\n",
        "\n",
        "    unsafe { *ptr }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "a blank line must detach a SAFETY proof from unsafe code: {findings:#?}"
    );
}

#[test]
fn rust_safety_proof_cannot_attach_to_safe_reference_dereference() {
    let source = concat!(
        "fn read(value: &u8) -> u8 {\n",
        "    unsafe {\n",
        "        // SAFETY: this safe reference needs no unsafe justification.\n",
        "        // second narrative line\n",
        "        // third narrative line\n",
        "        *value\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "a unary star is not syntactic proof of a raw-pointer dereference: {findings:#?}"
    );
}

#[test]
fn rust_safety_proofs_attach_to_explicit_unsafe_item_boundaries() {
    let sources = [
        concat!(
            "// SAFETY: callers uphold the function contract.\n",
            "// The input is valid for the call.\n",
            "// The input remains live for the call.\n",
            "unsafe fn call() {}\n",
        ),
        concat!(
            "// SAFETY: implementors uphold the marker contract.\n",
            "// Implementations preserve the invariant.\n",
            "// Implementations document their proof.\n",
            "unsafe trait Contract {}\n",
        ),
        concat!(
            "unsafe trait Contract {}\n",
            "struct Worker;\n",
            "// SAFETY: Worker upholds the trait contract.\n",
            "// Its state has the required representation.\n",
            "// Its methods preserve that representation.\n",
            "unsafe impl Contract for Worker {}\n",
        ),
        concat!(
            "// SAFETY: these declarations match the foreign ABI.\n",
            "// The symbol names are provided by the linked library.\n",
            "// Their signatures match the library definitions.\n",
            "unsafe extern \"C\" { fn foreign_call(); }\n",
        ),
        concat!(
            "trait Contract {\n",
            "    // SAFETY: callers uphold the method contract.\n",
            "    // The input is valid for the call.\n",
            "    // The input remains live for the call.\n",
            "    unsafe fn call();\n",
            "}\n",
        ),
    ];

    for source in sources {
        let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");
        assert!(
            findings.is_empty(),
            "an explicit unsafe boundary accepts its adjacent proof: {findings:#?}\n{source}"
        );
    }
}

#[test]
fn rust_safety_comment_before_safe_statement_in_unsafe_block_is_narrative() {
    let source = concat!(
        "unsafe fn read() -> u8 {\n",
        "    unsafe {\n",
        "        // SAFETY: this safe binding needs no unsafe justification.\n",
        "        // second narrative line\n",
        "        // third narrative line\n",
        "        let value = 1;\n",
        "        value\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "a safe statement inside an unsafe block cannot receive a safety exemption: {findings:#?}"
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

    let findings = analyze(Path::new("worker.py"), &source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected Python function budget finding, got {findings:#?}"
    );
}

#[test]
fn python_comments_before_decorators_belong_to_the_function() {
    let source = concat!(
        "# first\n",
        "# second\n",
        "# third\n",
        "# fourth\n",
        "@registered\n",
        "def work():\n",
        "    return 1\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "decorators are part of the function declaration boundary: {findings:#?}"
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

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expected Python docstring to consume the function budget, got {findings:#?}"
    );
}

#[test]
fn python_parenthesized_implicit_docstring_counts_every_physical_line() {
    let source = concat!(
        "def work():\n",
        "    (\"first implementation detail\"\n",
        "     \"second implementation detail\"\n",
        "     \"third implementation detail\"\n",
        "     \"fourth implementation detail\"\n",
        "     \"fifth implementation detail\"\n",
        "     \"sixth implementation detail\"\n",
        "     \"seventh implementation detail\"\n",
        "     \"eighth implementation detail\"\n",
        "     \"ninth implementation detail\"\n",
        "     \"tenth implementation detail\")\n",
        "    return 1\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    let finding = findings
        .iter()
        .find(|finding| finding.rule == "comment-policy/function-comment-budget")
        .expect("the implicit docstring must exceed its function budget");
    assert!(
        finding.message.contains("10 comment lines"),
        "one docstring node must retain its ten-line physical span: {finding:#?}"
    );
}

#[test]
fn python_interpolated_string_is_not_a_docstring() {
    let source = concat!(
        "def work(value):\n",
        "    f\"\"\"first expression line\n",
        "    second expression line\n",
        "    third {value} expression line\n",
        "    fourth expression line\"\"\"\n",
        "    return value\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings.is_empty(),
        "an interpolated expression is executable code, not a docstring: {findings:#?}"
    );
}

#[test]
fn python_class_docstring_uses_a_class_budget() {
    let source = concat!(
        "class Worker:\n",
        "    \"\"\"first implementation detail\n",
        "    second implementation detail\n",
        "    third implementation detail\n",
        "    fourth implementation detail\"\"\"\n",
        "    value = 1\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/type-comment-budget"),
        "class docs must be budgeted by the class rather than file scope: {findings:#?}"
    );
}

#[test]
fn python_ten_tool_directives_cannot_bypass_the_absolute_budget() {
    let source = concat!(
        "def work():\n",
        "    value_1 = missing_1  # noqa: F821\n",
        "    assert value_1  # nosec B101\n",
        "    value_2 = missing_2  # noqa: F821\n",
        "    assert value_2  # nosec B101\n",
        "    value_3 = missing_3  # noqa: F821\n",
        "    assert value_3  # nosec B101\n",
        "    value_4 = missing_4  # noqa: F821\n",
        "    assert value_4  # nosec B101\n",
        "    value_5 = missing_5  # noqa: F821\n",
        "    assert value_5  # nosec B101\n",
        "    return value_5\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "tool-shaped text must not create an unlimited budget bypass: {findings:#?}"
    );
}

#[test]
fn python_one_valid_tool_directive_remains_allowed() {
    let source = concat!(
        "def work():\n",
        "    value = missing_name  # noqa: F821\n",
        "    return value\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings.is_empty(),
        "one operational directive is below the hard cap: {findings:#?}"
    );
}

#[test]
fn python_malformed_noqa_directives_are_narrative() {
    let source = concat!(
        "def work():\n",
        "    first = 1  # noqa: prose\n",
        "    second = 2  # noqa: more prose\n",
        "    third = 3  # noqa: still prose\n",
        "    return first + second + third\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a noqa prefix without rule-code syntax remains narrative: {findings:#?}"
    );
}

#[test]
fn python_detached_line_suppressions_are_narrative() {
    let source = concat!(
        "def work():\n",
        "    # noqa: F401\n",
        "    # type: ignore[name-defined]\n",
        "    # nosec B101\n",
        "    return 1\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "line suppressions without a same-line statement remain narrative: {findings:#?}"
    );
}

#[test]
fn python_valid_trailing_directives_remain_operational() {
    let source = concat!(
        "def work(debug):\n",
        "    missing = unknown  # noqa: F821\n",
        "    typed = unknown  # type: ignore[name-defined]\n",
        "    value = eval('1')  # nosec B307\n",
        "    if debug:  # pragma: no cover\n",
        "        return [ missing, typed, value ]  # fmt: skip\n",
        "    return value\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings.is_empty(),
        "valid statement-attached directives stay outside narrative ratios: {findings:#?}"
    );
}

#[test]
fn python_matching_standalone_fmt_region_remains_operational() {
    let source = concat!(
        "def work():\n",
        "    # fmt: off\n",
        "    value    =    1\n",
        "    # fmt: on\n",
        "    return value\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings.is_empty(),
        "a matched statement-level formatter region is operational: {findings:#?}"
    );
}

#[test]
fn python_standalone_unmatched_fmt_markers_remain_operational() {
    let source = concat!(
        "def work():\n",
        "    # fmt: on\n",
        "    # fmt: off\n",
        "    return 1\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings.is_empty(),
        "exact standalone formatter markers are operational and still capped: {findings:#?}"
    );
}

#[test]
fn python_fmt_markers_inside_expression_are_narrative() {
    let source = concat!(
        "def work():\n",
        "    values = [\n",
        "        # fmt: off\n",
        "        1,\n",
        "        # fmt: on\n",
        "        2,\n",
        "    ]\n",
        "    return values\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "expression-level formatter markers have no operational effect: {findings:#?}"
    );
}

#[test]
fn python_detached_fmt_skip_is_narrative() {
    let source = concat!(
        "def work():\n",
        "    # fmt: skip\n",
        "    # fmt: skip\n",
        "    # fmt: skip\n",
        "    return 1\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-block-budget"),
        "fmt: skip only applies as a trailing statement directive: {findings:#?}"
    );
}

#[test]
fn python_module_docstring_consumes_file_budget() {
    let source = concat!(
        "\"\"\"First module implementation detail.\n",
        "Second module implementation detail.\n",
        "Third module implementation detail.\n",
        "Fourth module implementation detail.\"\"\"\n",
        "VALUE = 1\n",
    );

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
        "module docstrings cannot bypass the file comment budget: {findings:#?}"
    );
}

#[test]
fn python_module_constant_rejects_four_comment_lines() {
    let source = "# first\n# second\n# third\n# fourth\nLIMIT = 4\n";

    let findings = analyze(Path::new("limits.py"), source).expect("valid Python should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "expected Python leaf budget finding, got {findings:#?}"
    );
}

#[test]
fn python_constant_change_rejects_unchanged_comment() {
    let before = "# Coupled to the upstream retry window.\nRETRY_LIMIT = 3\n";
    let after = "# Coupled to the upstream retry window.\nRETRY_LIMIT = 4\n";

    let findings = analyze_line_change(Path::new("limits.py"), before, after)
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

    let findings = analyze(Path::new("worker.ts"), &source).expect("valid TypeScript should parse");

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

    let findings = analyze(Path::new("limits.ts"), source).expect("valid TypeScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "expected TypeScript leaf budget finding, got {findings:#?}"
    );
}

#[test]
fn typescript_const_change_rejects_unchanged_comment() {
    let before = "// Coupled to the upstream retry window.\nconst retryLimit = 3;\n";
    let after = "// Coupled to the upstream retry window.\nconst retryLimit = 4;\n";

    let findings = analyze_line_change(Path::new("limits.ts"), before, after)
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

    let findings = analyze(Path::new("config.toml"), source).expect("valid TOML should parse");

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

    let findings = analyze(Path::new("config.toml"), source).expect("valid TOML should parse");

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

    let findings = analyze(Path::new("config.toml"), source).expect("valid TOML should parse");

    assert!(
        findings.is_empty(),
        "hashes inside TOML strings are data, not comments: {findings:#?}"
    );
}

#[test]
fn toml_multiline_value_rejects_four_internal_comment_lines() {
    let source = concat!(
        "workers = [\n",
        "  # first\n",
        "  \"alpha\",\n",
        "  # second\n",
        "  \"beta\",\n",
        "  # third\n",
        "  \"gamma\",\n",
        "  # fourth\n",
        "  \"delta\",\n",
        "]\n",
    );

    let findings = analyze(Path::new("config.toml"), source).expect("valid TOML should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "comments inside a multiline value belong to that key: {findings:#?}"
    );
}

#[test]
fn detached_toml_comments_fall_back_to_file_budget() {
    let source = concat!(
        "# first\n",
        "# second\n",
        "# third\n",
        "# fourth\n",
        "\n",
        "timeout_ms = 200\n",
    );

    let findings = analyze(Path::new("config.toml"), source).expect("valid TOML should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
        "detached TOML narration must not disappear from policy: {findings:#?}"
    );
}

#[test]
fn toml_key_change_rejects_unchanged_comment() {
    let before = "# Coupled to the upstream retry window.\ntimeout_ms = 200\n";
    let after = "# Coupled to the upstream retry window.\ntimeout_ms = 400\n";

    let findings = analyze_line_change(Path::new("config.toml"), before, after)
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

    let findings = analyze(Path::new("index.html"), source).expect("valid HTML should parse");

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

    let findings = analyze(Path::new("site.css"), source).expect("valid CSS should parse");

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

    let findings = analyze(Path::new("Page.astro"), source).expect("valid Astro should parse");

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

    let findings = analyze(Path::new("Page.astro"), &source).expect("valid Astro should parse");

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

    let findings = analyze(Path::new("index.html"), &source).expect("valid HTML should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings.is_empty(),
        "a longer function can earn separate rationale lines: {findings:#?}"
    );
}

#[test]
fn malformed_rust_fails_closed() {
    let source = "fn broken( {\n";

    let error = analyze(Path::new("src/lib.rs"), source)
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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

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

    let findings = analyze(Path::new("worker.js"), source).expect("valid JavaScript should parse");

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

    let findings = analyze(Path::new("worker.js"), source).expect("valid JavaScript should parse");

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

    let findings = analyze(Path::new("worker.py"), source).expect("valid Python should parse");

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

    let jsx_findings = analyze(Path::new("View.jsx"), jsx).expect("valid JSX should parse");
    let tsx_findings = analyze(Path::new("View.tsx"), tsx).expect("valid TSX should parse");

    assert!(
        jsx_findings.is_empty() && tsx_findings.is_empty(),
        "registered JSX and TSX grammars should accept component syntax"
    );
}

#[test]
fn html_accepts_three_line_template_comment() {
    let source = "<!-- first\nsecond\nthird -->\n<main>hello</main>\n";

    let findings = analyze(Path::new("index.html"), source).expect("valid HTML should parse");

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

    let findings =
        analyze(Path::new("index.html"), source).expect("valid HTML and JavaScript should parse");
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

    let findings = analyze(Path::new("index.html"), source)
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

    let findings = analyze(Path::new("worker.js"), source).expect("valid JavaScript should parse");

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

    let findings = analyze(Path::new("src/lib.rs"), source).expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "closures must not bypass the function budget: {findings:#?}"
    );
}

#[test]
fn javascript_multiline_const_uses_leaf_budget_inside_a_long_function() {
    let source = concat!(
        "function work() {\n",
        "  consume(1);\n",
        "  consume(2);\n",
        "  consume(3);\n",
        "  consume(4);\n",
        "  consume(5);\n",
        "  consume(6);\n",
        "  consume(7);\n",
        "  consume(8);\n",
        "  consume(9);\n",
        "  consume(10);\n",
        "  consume(11);\n",
        "  consume(12);\n",
        "  consume(13);\n",
        "  consume(14);\n",
        "  consume(15);\n",
        "  consume(16);\n",
        "  const\n",
        "  VALUE = {\n",
        "    // first\n",
        "    first: 1,\n",
        "    // second\n",
        "    second: 2,\n",
        "    // third\n",
        "    third: 3,\n",
        "    // fourth\n",
        "    fourth: 4,\n",
        "  };\n",
        "  return VALUE;\n",
        "}\n",
    );

    let findings = analyze(Path::new("worker.js"), source).expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "Tree-sitter's const token must identify a multiline declaration: {findings:#?}"
    );
}

#[test]
fn typescript_multiline_const_uses_leaf_budget_inside_a_long_function() {
    let source = concat!(
        "function work(): Record<string, number> {\n",
        "  consume(1);\n",
        "  consume(2);\n",
        "  consume(3);\n",
        "  consume(4);\n",
        "  consume(5);\n",
        "  consume(6);\n",
        "  consume(7);\n",
        "  consume(8);\n",
        "  consume(9);\n",
        "  consume(10);\n",
        "  consume(11);\n",
        "  consume(12);\n",
        "  consume(13);\n",
        "  consume(14);\n",
        "  consume(15);\n",
        "  consume(16);\n",
        "  const\n",
        "  VALUE: Record<string, number> = {\n",
        "    // first\n",
        "    first: 1,\n",
        "    // second\n",
        "    second: 2,\n",
        "    // third\n",
        "    third: 3,\n",
        "    // fourth\n",
        "    fourth: 4,\n",
        "  };\n",
        "  return VALUE;\n",
        "}\n",
    );

    let findings = analyze(Path::new("worker.ts"), source).expect("valid TypeScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "TypeScript must share the structural const detector: {findings:#?}"
    );
}

#[test]
fn malformed_javascript_and_typescript_directive_prefixes_are_narrative() {
    for path in ["worker.js", "worker.ts"] {
        for candidate in [
            "// eslint-disable-next-line: this is prose",
            "// istanbul ignore totally this is prose",
            "// c8 ignore totally this is prose",
        ] {
            let source = format!(
                "function work() {{\n  {candidate}\n  perform();\n  // ordinary explanation\n  finish();\n}}\n"
            );

            let findings = analyze(Path::new(path), &source).expect("valid source should parse");

            assert!(
                findings
                    .iter()
                    .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
                "magic prefix must not classify malformed prose as metadata ({path}, {candidate}): {findings:#?}"
            );
        }
    }
}

#[test]
fn typescript_suppression_directives_match_compiler_comment_forms() {
    for path in ["worker.js", "worker.ts"] {
        for directive in [
            "// @ts-ignore because the declaration is supplied at runtime",
            "// @ts-expect-error this call intentionally exercises the fallback",
            "// @ts-ignoreThisTokenStillMatchesTheScanner",
            "/// @ts-ignore accepted by the TypeScript scanner",
            "/* @ts-ignore */",
            "/*\n   * @ts-expect-error */",
        ] {
            let source = format!(
                "function work() {{\n  {directive}\n  perform();\n  // ordinary explanation\n  finish();\n}}\n"
            );

            let findings = analyze(Path::new(path), &source).expect("valid source should parse");

            assert!(
                findings.is_empty(),
                "an attached TypeScript suppression directive is compiler metadata ({path}, {directive}): {findings:#?}"
            );
        }
    }
}

#[test]
fn typescript_suppression_block_accepts_scanner_slash_decoration() {
    for path in ["worker.js", "worker.ts"] {
        let source = concat!(
            "function work() {\n",
            "  /*\n",
            "   /// @ts-ignore */\n",
            "  perform();\n",
            "  // ordinary explanation\n",
            "  finish();\n",
            "}\n",
        );

        let findings = analyze(Path::new(path), source).expect("valid source should parse");

        assert!(
            findings.is_empty(),
            "the TypeScript scanner permits slash decoration before a block suppression directive ({path}): {findings:#?}"
        );
    }
}

#[test]
fn typescript_suppression_block_with_prior_prose_is_narrative() {
    for path in ["worker.js", "worker.ts"] {
        let source = concat!(
            "function work() {\n",
            "  /* context\n",
            "   * @ts-expect-error */\n",
            "  perform();\n",
            "  // ordinary explanation\n",
            "  finish();\n",
            "}\n",
        );

        let findings = analyze(Path::new(path), source).expect("valid source should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "prose before a suppression token makes the whole block narrative ({path}): {findings:#?}"
        );
    }
}

#[test]
fn typescript_suppression_block_directive_must_appear_on_the_last_line() {
    let source = concat!(
        "function work() {\n",
        "  /* @ts-ignore\n",
        "   * ordinary explanation */\n",
        "  perform();\n",
        "  // another ordinary explanation\n",
        "  finish();\n",
        "}\n",
    );

    let findings = analyze(Path::new("worker.ts"), source).expect("valid TypeScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "only the last line of a block comment is scanned for suppression directives: {findings:#?}"
    );
}

#[test]
fn typescript_suppression_block_treats_a_carriage_return_as_a_line_break() {
    let source = concat!(
        "function work() {\n",
        "  /*\r   * @ts-ignore */\n",
        "  perform();\n",
        "  // ordinary explanation\n",
        "  finish();\n",
        "}\n",
    );

    let findings = analyze(Path::new("worker.ts"), source).expect("valid TypeScript should parse");

    assert!(
        findings.is_empty(),
        "the TypeScript scanner treats a lone carriage return as a block-comment line break: {findings:#?}"
    );
}

#[test]
fn typescript_check_pragmas_accept_compiler_suffix_forms_in_the_preamble() {
    for path in ["worker.js", "worker.ts"] {
        for pragma in [
            "// @ts-check checked by the JavaScript project",
            "// @ts-nocheck: generated declaration shim",
            "/// @ts-check checked through a triple-slash comment",
        ] {
            let source = format!(
                "{pragma}\n// ordinary explanation\nfunction work() {{\n  perform();\n}}\n"
            );

            let findings = analyze(Path::new(path), &source).expect("valid source should parse");

            assert!(
                findings.is_empty(),
                "a leading TypeScript check pragma accepts the compiler's suffix grammar ({path}, {pragma}): {findings:#?}"
            );
        }
    }
}

#[test]
fn typescript_check_pragma_name_requires_a_compiler_delimiter() {
    for pragma in ["// @ts-checking", "// @ts-nocheck-generated"] {
        let source =
            format!("{pragma}\n// ordinary explanation\nfunction work() {{\n  perform();\n}}\n");

        let findings =
            analyze(Path::new("worker.ts"), &source).expect("valid TypeScript should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "a concatenated pragma-like name is ordinary narrative ({pragma}): {findings:#?}"
        );
    }
}

#[test]
fn unattached_javascript_directive_is_narrative() {
    let source = concat!(
        "function work() {\n",
        "  // @ts-ignore: applies only to the next line\n",
        "\n",
        "  perform();\n",
        "  // ordinary explanation\n",
        "  finish();\n",
        "}\n",
    );

    let findings = analyze(Path::new("worker.js"), source).expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a detached magic comment is prose, not executable metadata: {findings:#?}"
    );
}

#[test]
fn valid_attached_javascript_and_typescript_directives_remain_metadata() {
    for path in ["worker.js", "worker.ts"] {
        for directive in [
            "// eslint-disable-next-line no-console",
            "// @ts-expect-error: intentional invalid call",
            "/* istanbul ignore next */",
            "/* c8 ignore next */",
        ] {
            let source = format!(
                "function work() {{\n  {directive}\n  perform();\n  // ordinary explanation\n  finish();\n}}\n"
            );

            let findings = analyze(Path::new(path), &source).expect("valid source should parse");

            assert!(
                findings.is_empty(),
                "one attached operational directive stays outside the narrative ratio ({path}, {directive}): {findings:#?}"
            );
        }
    }
}

#[test]
fn prettier_ignore_skips_comment_siblings_before_its_next_node() {
    for path in ["render.js", "render.ts"] {
        let source = concat!(
            "function render() {\n",
            "  // prettier-ignore\n",
            "  // ordinary explanation\n",
            "  renderValue();\n",
            "  finish();\n",
            "}\n",
        );

        let findings = analyze(Path::new(path), source).expect("valid source should parse");

        assert!(
            findings.is_empty(),
            "Prettier targets the next non-comment node across comment siblings ({path}): {findings:#?}"
        );
    }
}

#[test]
fn next_line_directives_do_not_skip_comment_siblings() {
    for path in ["render.js", "render.ts"] {
        let source = concat!(
            "function render() {\n",
            "  // @ts-ignore\n",
            "  // ordinary explanation\n",
            "  renderValue();\n",
            "  finish();\n",
            "}\n",
        );

        let findings = analyze(Path::new(path), source).expect("valid source should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "a next-line directive cannot skip an intervening comment ({path}): {findings:#?}"
        );
    }
}

#[test]
fn prettier_ignore_at_frame_end_has_no_next_node() {
    for path in ["render.js", "render.ts"] {
        let source = concat!(
            "function render() {\n",
            "  // ordinary explanation\n",
            "  renderValue();\n",
            "  // prettier-ignore\n",
            "}\n",
            "finish();\n",
        );

        let findings = analyze(Path::new(path), source).expect("valid source should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "a pending next-node directive cannot escape its parent frame ({path}): {findings:#?}"
        );
    }
}

#[test]
fn deeply_nested_javascript_directives_remain_attached() {
    let depth = 400;
    let mut source = String::new();
    for index in 0..depth {
        source.push_str(&format!(
            "function f{index}() {{\n// eslint-disable-next-line no-console\n"
        ));
    }
    source.push_str("return 1;\n");
    source.push_str(&"}\n".repeat(depth));

    let findings = analyze(Path::new("deep.js"), &source).expect("valid nested JavaScript");

    assert!(
        findings.is_empty(),
        "every nested directive must stay attached to its next line: {findings:#?}"
    );
}

#[test]
fn javascript_tool_directives_cannot_bypass_the_absolute_owner_cap() {
    let source = concat!(
        "function work() {\n",
        "  // eslint-disable-next-line no-console\n",
        "  console.log(1);\n",
        "  // @ts-expect-error: intentional invalid call\n",
        "  invalidCall(2);\n",
        "  /* istanbul ignore next */\n",
        "  branch(3);\n",
        "  /* c8 ignore next */\n",
        "  branch(4);\n",
        "  // eslint-disable-next-line no-console\n",
        "  console.log(5);\n",
        "  // @ts-expect-error: intentional invalid call\n",
        "  invalidCall(6);\n",
        "  /* istanbul ignore next */\n",
        "  branch(7);\n",
        "  /* c8 ignore next */\n",
        "  branch(8);\n",
        "  // eslint-disable-next-line no-console\n",
        "  console.log(9);\n",
        "  // @ts-expect-error: intentional invalid call\n",
        "  invalidCall(10);\n",
        "}\n",
    );

    let findings = analyze(Path::new("worker.js"), source).expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "tool metadata must not grant an unlimited comment allowance: {findings:#?}"
    );
}
