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
