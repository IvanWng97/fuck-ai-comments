use std::fmt::Write as _;
use std::path::Path;
use std::sync::OnceLock;

use divan::Bencher;
use fuck_ai_comments::{SourceFile, analyze_all, analyze_change};

const BENCHMARK_LINES: usize = 10_000;
const OWNER_LINES: usize = 5;
const OWNER_COUNT: usize = BENCHMARK_LINES / OWNER_LINES;
const STATIC_FINDING_COMMENT_LINES: usize = 4;
const ASTRO_FAST_FIXED_LINES: usize = 9;
const ASTRO_FAST_BODY_LINES: usize = BENCHMARK_LINES - ASTRO_FAST_FIXED_LINES;
const ASTRO_RECOVERY_FIXED_LINES: usize = 7;
const ASTRO_FAKE_FENCE_LINES: usize = BENCHMARK_LINES - ASTRO_RECOVERY_FIXED_LINES;
const FUNCTION_COMMENT_BUDGET_RULE: &str = "comment-policy/function-comment-budget";
const LEAF_COMMENT_BUDGET_RULE: &str = "comment-policy/leaf-comment-budget";
const TEMPLATE_COMMENT_BUDGET_RULE: &str = "comment-policy/template-comment-budget";
const STALE_COMMENT_RULE: &str = "comment-policy/comment-owner-changed";

static RUST_SOURCE: OnceLock<String> = OnceLock::new();
static PYTHON_SOURCE: OnceLock<String> = OnceLock::new();
static JAVASCRIPT_SOURCE: OnceLock<String> = OnceLock::new();
static TYPESCRIPT_SOURCE: OnceLock<String> = OnceLock::new();
static TSX_SOURCE: OnceLock<String> = OnceLock::new();
static KOTLIN_SOURCE: OnceLock<String> = OnceLock::new();
static SWIFT_SOURCE: OnceLock<String> = OnceLock::new();
static OBJECTIVE_C_SOURCE: OnceLock<String> = OnceLock::new();
static CSS_SOURCE: OnceLock<String> = OnceLock::new();
static HTML_SOURCE: OnceLock<String> = OnceLock::new();
static TOML_SOURCE: OnceLock<String> = OnceLock::new();
static ASTRO_FAST_SOURCE: OnceLock<String> = OnceLock::new();
static ASTRO_RECOVERY_SOURCE: OnceLock<String> = OnceLock::new();
static TSX_CHANGE: OnceLock<(String, String)> = OnceLock::new();
static RUST_ADVERSARIAL_CHANGE: OnceLock<(String, String)> = OnceLock::new();

fn main() {
    divan::main();
}

#[divan::bench]
fn static_rust_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static(
        bencher,
        Path::new("workers.rs"),
        RUST_SOURCE.get_or_init(rust_source),
    );
}

#[divan::bench]
fn static_python_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static(
        bencher,
        Path::new("workers.py"),
        PYTHON_SOURCE.get_or_init(python_source),
    );
}

#[divan::bench]
fn static_javascript_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static_with_finding(
        bencher,
        Path::new("workers.js"),
        JAVASCRIPT_SOURCE.get_or_init(javascript_source),
        FUNCTION_COMMENT_BUDGET_RULE,
        2,
    );
}

#[divan::bench]
fn static_typescript_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static(
        bencher,
        Path::new("workers.ts"),
        TYPESCRIPT_SOURCE.get_or_init(plain_typescript_source),
    );
}

#[divan::bench]
fn static_tsx_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static(
        bencher,
        Path::new("workers.tsx"),
        TSX_SOURCE.get_or_init(|| typescript_source(None)),
    );
}

#[divan::bench]
fn static_kotlin_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static(
        bencher,
        Path::new("Workers.kt"),
        KOTLIN_SOURCE.get_or_init(kotlin_source),
    );
}

#[divan::bench]
fn static_swift_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static(
        bencher,
        Path::new("Workers.swift"),
        SWIFT_SOURCE.get_or_init(swift_source),
    );
}

#[divan::bench]
fn static_objective_c_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static(
        bencher,
        Path::new("Workers.m"),
        OBJECTIVE_C_SOURCE.get_or_init(objective_c_source),
    );
}

#[divan::bench]
fn static_css_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static_with_finding(
        bencher,
        Path::new("workers.css"),
        CSS_SOURCE.get_or_init(css_source),
        TEMPLATE_COMMENT_BUDGET_RULE,
        2,
    );
}

#[divan::bench]
fn static_html_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static_with_finding(
        bencher,
        Path::new("workers.html"),
        HTML_SOURCE.get_or_init(html_source),
        TEMPLATE_COMMENT_BUDGET_RULE,
        2,
    );
}

#[divan::bench]
fn static_toml_10k_loc(bencher: Bencher<'_, '_>) {
    bench_static_with_finding(
        bencher,
        Path::new("workers.toml"),
        TOML_SOURCE.get_or_init(toml_source),
        LEAF_COMMENT_BUDGET_RULE,
        1,
    );
}

#[divan::bench]
fn astro_fast_path_10k_loc(bencher: Bencher<'_, '_>) {
    let source = ASTRO_FAST_SOURCE.get_or_init(astro_fast_source);
    assert_eq!(
        source.lines().filter(|line| *line == "---").count(),
        2,
        "the Astro fast-path workload must contain only its real frontmatter fences"
    );
    bench_static_with_finding(
        bencher,
        Path::new("Page.astro"),
        source,
        TEMPLATE_COMMENT_BUDGET_RULE,
        BENCHMARK_LINES - STATIC_FINDING_COMMENT_LINES,
    );
}

#[divan::bench]
fn astro_recovery_10k_loc(bencher: Bencher<'_, '_>) {
    let source = ASTRO_RECOVERY_SOURCE.get_or_init(astro_recovery_source);
    assert_eq!(
        source.lines().filter(|line| *line == "---").count(),
        ASTRO_FAKE_FENCE_LINES + 2,
        "the Astro workload must retain every fake fence plus both real fences"
    );
    bench_static(bencher, Path::new("Page.astro"), source);
}

#[divan::bench]
fn change_tsx_10k_loc_per_snapshot(bencher: Bencher<'_, '_>) {
    let (before, after) = TSX_CHANGE.get_or_init(|| {
        (
            typescript_source(None),
            typescript_source(Some(OWNER_COUNT / 2)),
        )
    });
    assert_source_shape(before);
    assert_source_shape(after);
    let path = Path::new("workers.tsx");
    let before_file = SourceFile { path, text: before };
    let after_file = SourceFile { path, text: after };
    let findings = analyze_change(before_file, after_file)
        .expect("generated TypeScript snapshots must parse before benchmarking");
    assert_eq!(findings.len(), 1, "the change workload must stay focused");
    assert_eq!(findings[0].rule, STALE_COMMENT_RULE);
    assert_eq!(findings[0].line, OWNER_COUNT / 2 * OWNER_LINES + 2);

    bencher
        .with_inputs(|| (before_file, after_file))
        .bench_local_values(|(before_file, after_file)| {
            analyze_change(before_file, after_file)
                .expect("generated TypeScript snapshots must parse")
        });
}

#[divan::bench]
fn change_rust_adversarial_10k_loc_per_snapshot(bencher: Bencher<'_, '_>) {
    let (before, after) = RUST_ADVERSARIAL_CHANGE.get_or_init(|| {
        (
            adversarial_rust_change_source(false),
            adversarial_rust_change_source(true),
        )
    });
    assert_source_shape(before);
    assert_source_shape(after);
    let path = Path::new("workers.rs");
    let before_file = SourceFile { path, text: before };
    let after_file = SourceFile { path, text: after };
    let findings = analyze_change(before_file, after_file)
        .expect("adversarial Rust snapshots must parse before benchmarking");
    assert_eq!(findings.len(), 1, "the change workload must stay focused");
    assert_eq!(findings[0].rule, STALE_COMMENT_RULE);
    assert_eq!(findings[0].line, 2);

    bencher
        .with_inputs(|| (before_file, after_file))
        .bench_local_values(|(before_file, after_file)| {
            analyze_change(before_file, after_file).expect("adversarial Rust snapshots must parse")
        });
}

fn bench_static(bencher: Bencher<'_, '_>, path: &Path, source: &str) {
    bench_static_with_expected(bencher, path, source, &[]);
}

fn bench_static_with_finding(
    bencher: Bencher<'_, '_>,
    path: &Path,
    source: &str,
    expected_rule: &'static str,
    expected_line: usize,
) {
    bench_static_with_expected(bencher, path, source, &[(expected_rule, expected_line)]);
}

fn bench_static_with_expected(
    bencher: Bencher<'_, '_>,
    path: &Path,
    source: &str,
    expected_findings: &[(&str, usize)],
) {
    assert_source_shape(source);
    let file = SourceFile { path, text: source };
    let findings =
        analyze_all(file).expect("generated source must parse before benchmarking begins");
    let finding_shape: Vec<_> = findings
        .iter()
        .map(|finding| (finding.rule, finding.line))
        .collect();
    assert_eq!(
        finding_shape, expected_findings,
        "static benchmark findings changed: {findings:#?}"
    );
    bencher
        .with_inputs(|| file)
        .bench_local_values(|file| analyze_all(file).expect("generated source must parse"));
}

fn assert_source_shape(source: &str) {
    assert_eq!(source.lines().count(), BENCHMARK_LINES);
}

fn rust_source() -> String {
    generate_owners(None, |source, owner, _| {
        writeln!(
            source,
            "pub struct Worker{owner:05};\nimpl Worker{owner:05} {{\n    pub fn run(&self) -> usize {{\n        /* Coupled to this worker's return contract. */ {owner}\n    }} }}"
        )
    })
}

fn python_source() -> String {
    generate_owners(None, |source, owner, _| {
        writeln!(
            source,
            "def task_{owner:05}():\n    # Coupled to this task's return contract.\n    value = {owner}\n    result = value\n    return result"
        )
    })
}

fn javascript_source() -> String {
    generate_owners(None, |source, owner, _| {
        if owner == 0 {
            writeln!(
                source,
                "function task{owner:05}() {{\n  // Coupled to this task's returned value.\n  const value = {owner};\n  // Coupled to the same return contract.\n  return value; }}"
            )
        } else {
            writeln!(
                source,
                "function task{owner:05}() {{\n  // Coupled to this task's return contract.\n  const value = {owner};\n  const result = value;\n  return result; }}"
            )
        }
    })
}

fn plain_typescript_source() -> String {
    generate_owners(None, |source, owner, _| {
        writeln!(
            source,
            "export function task{owner:05}(): number {{\n  // Coupled to this task's typed return contract.\n  const value: number = {owner};\n  const result: number = value;\n  return result; }}"
        )
    })
}

fn typescript_source(changed: Option<usize>) -> String {
    generate_owners(changed, |source, owner, is_changed| {
        let value = owner + usize::from(is_changed);
        writeln!(
            source,
            "export function Card{owner:05}() {{\n  // Coupled to this component's rendered value.\n  const value = {value};\n  return <span>{{value}}</span>;\n}}"
        )
    })
}

fn adversarial_rust_change_source(reverse: bool) -> String {
    let statement_count = BENCHMARK_LINES - 3;
    let mut source = String::with_capacity(statement_count * 32);
    source.push_str("fn work() {\n    // Coupled to the execution order.\n");
    if reverse {
        for statement in (0..statement_count).rev() {
            writeln!(source, "    step_{statement:05}();")
                .expect("writing to a String cannot fail");
        }
    } else {
        for statement in 0..statement_count {
            writeln!(source, "    step_{statement:05}();")
                .expect("writing to a String cannot fail");
        }
    }
    source.push_str("}\n");
    source
}

fn kotlin_source() -> String {
    generate_owners(None, |source, owner, _| {
        writeln!(
            source,
            "fun task{owner:05}(): Int {{\n    // Coupled to this task's return contract.\n    val value = {owner}\n    return value\n}}"
        )
    })
}

fn swift_source() -> String {
    generate_owners(None, |source, owner, _| {
        writeln!(
            source,
            "func task{owner:05}() -> Int {{\n    // Coupled to this task's return contract.\n    let value = {owner}\n    return value\n}}"
        )
    })
}

fn objective_c_source() -> String {
    generate_owners(None, |source, owner, _| {
        writeln!(
            source,
            "NSInteger Task{owner:05}(void) {{\n    // Coupled to this task's return contract.\n    NSInteger value = {owner};\n    return value;\n}}"
        )
    })
}

fn css_source() -> String {
    let mut source = String::new();
    source.push_str(".workers {\n");
    for comment in 0..STATIC_FINDING_COMMENT_LINES {
        writeln!(
            source,
            "  /* Narrative contract detail {comment} for the generated worker styles. */"
        )
        .expect("writing to a String cannot fail");
    }
    for worker in 0..BENCHMARK_LINES - STATIC_FINDING_COMMENT_LINES - 2 {
        writeln!(source, "  --worker-{worker:05}: {worker};")
            .expect("writing to a String cannot fail");
    }
    source.push_str("}\n");
    source
}

fn html_source() -> String {
    let mut source = String::new();
    source.push_str("<main>\n");
    for comment in 0..STATIC_FINDING_COMMENT_LINES {
        writeln!(
            source,
            "  <!-- Narrative contract detail {comment} for the generated worker list. -->"
        )
        .expect("writing to a String cannot fail");
    }
    for worker in 0..BENCHMARK_LINES - STATIC_FINDING_COMMENT_LINES - 2 {
        writeln!(
            source,
            "  <span data-worker=\"{worker:05}\">{worker}</span>"
        )
        .expect("writing to a String cannot fail");
    }
    source.push_str("</main>\n");
    source
}

fn toml_source() -> String {
    let mut source = String::new();
    for comment in 0..STATIC_FINDING_COMMENT_LINES {
        writeln!(
            source,
            "# Narrative contract detail {comment} for the first worker."
        )
        .expect("writing to a String cannot fail");
    }
    for worker in 0..BENCHMARK_LINES - STATIC_FINDING_COMMENT_LINES {
        writeln!(source, "worker_{worker:05} = {worker}").expect("writing to a String cannot fail");
    }
    source
}

fn astro_fast_source() -> String {
    let mut source = String::new();
    source.push_str("---\nconst art = `before\n");
    for row in 0..ASTRO_FAST_BODY_LINES {
        writeln!(source, "worker-{row:05}").expect("writing to a String cannot fail");
    }
    source.push_str("after`;\n---\n");
    for comment in 0..STATIC_FINDING_COMMENT_LINES {
        writeln!(
            source,
            "<!-- Narrative contract detail {comment} for the rendered worker art. -->"
        )
        .expect("writing to a String cannot fail");
    }
    source.push_str("<main>{art}</main>\n");
    source
}

fn astro_recovery_source() -> String {
    let mut source = String::with_capacity(BENCHMARK_LINES * 4);
    source.push_str("---\nconst pattern = /`/g;\nconst art = `before\n");
    for _ in 0..ASTRO_FAKE_FENCE_LINES {
        source.push_str("---\n");
    }
    source.push_str(
        "after`;\n---\n<!-- Coupled to the rendered output. -->\n<main>{pattern.source}{art}</main>\n",
    );
    source
}

fn generate_owners(
    changed: Option<usize>,
    mut write_owner: impl FnMut(&mut String, usize, bool) -> std::fmt::Result,
) -> String {
    let mut source = String::with_capacity(OWNER_COUNT * 160);
    for owner in 0..OWNER_COUNT {
        write_owner(&mut source, owner, changed == Some(owner))
            .expect("writing to a String cannot fail");
    }
    source
}
