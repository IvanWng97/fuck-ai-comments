use std::fmt::Write as _;
use std::path::Path;
use std::time::Instant;

use fuck_ai_comments::{SourceFile, analyze_all, analyze_change};

const STATIC_OWNER_COUNT: usize = 4_096;
const CHANGE_OWNER_COUNT: usize = 2_048;

#[test]
fn thousands_of_python_owners_keep_independent_comment_budgets() {
    let source = python_source(STATIC_OWNER_COUNT, None);

    let findings = analyze_all(SourceFile {
        path: Path::new("workers.py"),
        text: &source,
    })
    .expect("generated Python must parse");

    assert!(
        findings.is_empty(),
        "one-line rationales stay within budget"
    );
}

#[test]
fn one_changed_owner_among_thousands_stales_only_its_comment() {
    let changed = CHANGE_OWNER_COUNT / 2;
    let before = python_source(CHANGE_OWNER_COUNT, None);
    let after = python_source(CHANGE_OWNER_COUNT, Some(changed));

    let findings = analyze_change(
        SourceFile {
            path: Path::new("workers.py"),
            text: &before,
        },
        SourceFile {
            path: Path::new("workers.py"),
            text: &after,
        },
    )
    .expect("generated Python change must parse");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(stale_lines, [changed * 4 + 2]);
}

#[test]
fn thousands_of_toml_keys_keep_independent_comment_budgets() {
    let source = toml_source(STATIC_OWNER_COUNT);

    let findings = analyze_all(SourceFile {
        path: Path::new("generated.toml"),
        text: &source,
    })
    .expect("generated TOML must parse");

    assert!(
        findings.is_empty(),
        "one-line rationales stay within budget"
    );
}

#[test]
fn thousands_of_public_rust_methods_keep_docs_exempt() {
    let source = rust_public_method_source(STATIC_OWNER_COUNT);

    let findings = analyze_all(SourceFile {
        path: Path::new("src/lib.rs"),
        text: &source,
    })
    .expect("generated Rust must parse");

    assert!(findings.is_empty(), "reachable public docs stay exempt");
}

#[test]
#[ignore = "manual release-mode scaling evidence"]
fn report_many_owner_release_scaling() {
    for owner_count in [1_000, 2_000, 4_000] {
        let source = python_source(owner_count, None);
        let started = Instant::now();
        let findings = analyze_all(SourceFile {
            path: Path::new("workers.py"),
            text: &source,
        })
        .expect("generated Python must parse");
        assert!(findings.is_empty());
        eprintln!("python owners={owner_count}: {:?}", started.elapsed());

        let before = python_source(owner_count, None);
        let after = python_source(owner_count, Some(owner_count / 2));
        let started = Instant::now();
        let findings = analyze_change(
            SourceFile {
                path: Path::new("workers.py"),
                text: &before,
            },
            SourceFile {
                path: Path::new("workers.py"),
                text: &after,
            },
        )
        .expect("generated Python change must parse");
        assert_eq!(
            findings
                .iter()
                .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
                .count(),
            1
        );
        eprintln!(
            "python change owners={owner_count}: {:?}",
            started.elapsed()
        );

        let source = toml_source(owner_count);
        let started = Instant::now();
        let findings = analyze_all(SourceFile {
            path: Path::new("generated.toml"),
            text: &source,
        })
        .expect("generated TOML must parse");
        assert!(findings.is_empty());
        eprintln!("toml owners={owner_count}: {:?}", started.elapsed());

        let source = rust_public_method_source(owner_count);
        let started = Instant::now();
        let findings = analyze_all(SourceFile {
            path: Path::new("src/lib.rs"),
            text: &source,
        })
        .expect("generated Rust must parse");
        assert!(findings.is_empty());
        eprintln!("rust public methods={owner_count}: {:?}", started.elapsed());
    }
}

fn python_source(owner_count: usize, changed: Option<usize>) -> String {
    let mut source = String::with_capacity(owner_count * 96);
    for owner in 0..owner_count {
        let value = owner + usize::from(changed == Some(owner));
        writeln!(
            source,
            "def task_{owner:05}():\n    # Coupled to this task's return contract.\n    return {value}\n"
        )
        .expect("writing to a String cannot fail");
    }
    source
}

fn toml_source(owner_count: usize) -> String {
    let mut source = String::with_capacity(owner_count * 64);
    for owner in 0..owner_count {
        writeln!(
            source,
            "# Coupled to this key's deployment contract.\nkey_{owner:05} = {owner}"
        )
        .expect("writing to a String cannot fail");
    }
    source
}

fn rust_public_method_source(owner_count: usize) -> String {
    let mut source = String::with_capacity(owner_count * 128);
    for owner in 0..owner_count {
        writeln!(
            source,
            "pub struct Type{owner:05};\nimpl Type{owner:05} {{\n    /// Runs this public type.\n    pub fn run(&self) {{}}\n}}"
        )
        .expect("writing to a String cannot fail");
    }
    source
}
