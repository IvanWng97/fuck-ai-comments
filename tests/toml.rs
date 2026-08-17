use std::path::Path;

use fuck_ai_comments::{AnalysisError, SourceFile, analyze_all, analyze_change};

fn source(text: &str) -> SourceFile<'_> {
    SourceFile {
        path: Path::new("config.toml"),
        text,
    }
}

fn stale_lines(before: &str, after: &str) -> Vec<usize> {
    analyze_change(source(before), source(after))
        .expect("valid TOML change")
        .into_iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect()
}

#[test]
fn quoted_dotted_key_change_stales_its_comment() {
    let before = concat!(
        "# Coupled to the release retry contract.\n",
        "\"release.config\".retry.count = 3\n",
    );
    let after = concat!(
        "# Coupled to the release retry contract.\n",
        "\"release.config\".retry.count = 4\n",
    );

    assert_eq!(stale_lines(before, after), [1]);
}

#[test]
fn table_header_change_stales_the_owned_key_comment() {
    let before = concat!(
        "[primary]\n",
        "# Coupled to the primary service contract.\n",
        "timeout = 200\n",
    );
    let after = concat!(
        "[fallback]\n",
        "# Coupled to the primary service contract.\n",
        "timeout = 200\n",
    );

    assert_eq!(stale_lines(before, after), [2]);
}

#[test]
fn repeated_array_table_change_stales_only_its_occurrence() {
    let before = concat!(
        "[[servers]]\n",
        "# Coupled to the public listener.\n",
        "port = 8000\n",
        "\n",
        "[[servers]]\n",
        "# Coupled to the admin listener.\n",
        "port = 9000\n",
    );
    let after = concat!(
        "[[servers]]\n",
        "# Coupled to the public listener.\n",
        "port = 8000\n",
        "\n",
        "[[servers]]\n",
        "# Coupled to the admin listener.\n",
        "port = 9001\n",
    );

    assert_eq!(stale_lines(before, after), [6]);
}

#[test]
fn dotted_key_and_standard_table_siblings_keep_distinct_key_owners() {
    let input = concat!(
        "service.primary = true\n",
        "[service.fallback]\n",
        "# first\n",
        "# second\n",
        "# third\n",
        "enabled = true\n",
    );

    let findings = analyze_all(source(input)).expect("valid mixed dotted and table TOML");

    assert!(
        findings.is_empty(),
        "three comments belong to the table key's leaf allowance: {findings:#?}"
    );
}

#[test]
fn multiline_key_change_stales_leading_internal_and_trailing_comments() {
    let before = concat!(
        "# Coupled to worker ordering.\n",
        "workers = [\n",
        "    # The fallback must remain last.\n",
        "    \"alpha\",\n",
        "    \"fallback\",\n",
        "] # Consumed in declared order.\n",
        "\n",
        "# Applies to the whole file.\n",
        "\n",
        "enabled = true\n",
    );
    let after = concat!(
        "# Coupled to worker ordering.\n",
        "workers = [\n",
        "    # The fallback must remain last.\n",
        "    \"beta\",\n",
        "    \"fallback\",\n",
        "] # Consumed in declared order.\n",
        "\n",
        "# Applies to the whole file.\n",
        "\n",
        "enabled = true\n",
    );

    assert_eq!(stale_lines(before, after), [1, 3, 6]);
}

#[test]
fn malformed_toml_fails_closed() {
    let error = analyze_all(source("workers = [\"alpha\"\n"))
        .expect_err("malformed TOML must not be analyzed heuristically");

    assert!(matches!(error, AnalysisError::Toml { .. }));
}

#[test]
fn semantically_invalid_toml_fails_closed() {
    let error = analyze_all(source("timeout = 200\ntimeout = 400\n"))
        .expect_err("duplicate TOML keys must not produce a partial inventory");

    assert!(matches!(error, AnalysisError::Toml { .. }));
}
