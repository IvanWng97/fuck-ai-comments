use std::fmt::Write as _;
use std::path::Path;

use fuck_ai_comments::{AnalysisError, SourceFile, analyze_all, analyze_change, supports_path};

fn assert_parent_type_rename_stales_nested_comment(
    path: &str,
    before: &str,
    after: &str,
    comment_line: usize,
) {
    let findings = analyze_change(
        SourceFile {
            path: Path::new(path),
            text: before,
        },
        SourceFile {
            path: Path::new(path),
            text: after,
        },
    )
    .expect("the unchanged child owner anchors its renamed parent type");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [comment_line],
        "a parent type rename changes the nested owner's semantic identity path"
    );
}

fn assert_leaf_comment_addition_activates_type_budget(path: &str, before: &str, after: &str) {
    let findings = analyze_change(
        SourceFile {
            path: Path::new(path),
            text: before,
        },
        SourceFile {
            path: Path::new(path),
            text: after,
        },
    )
    .expect("valid leaf comment addition");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/type-comment-budget"),
        "a new leaf comment must activate the type budget it consumes ({path}): {findings:#?}"
    );
}

#[test]
fn registered_extensions_share_one_registry() {
    let supported = [
        "file.rs",
        "file.py",
        "file.pyi",
        "file.pyw",
        "file.js",
        "file.cjs",
        "file.mjs",
        "file.jsx",
        "file.ts",
        "file.cts",
        "file.mts",
        "file.tsx",
        "file.m",
        "file.swift",
        "file.kt",
        "file.kts",
        "file.toml",
        "file.html",
        "file.htm",
        "file.css",
        "file.astro",
    ];

    assert!(
        supported.iter().all(|path| supports_path(Path::new(path))),
        "every documented extension must resolve through the registry"
    );
}

#[test]
fn analyze_all_uses_the_public_source_snapshot_seam() {
    let source = "// first\n// second\n// third\n// fourth\nconst LIMIT: usize = 4;\n";

    let findings = analyze_all(SourceFile {
        path: Path::new("src/lib.rs"),
        text: source,
    })
    .expect("valid Rust should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "whole-file analysis should run the same policy authority"
    );
}

#[test]
fn same_line_code_edit_requires_comment_attestation() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "const RETRY_LIMIT: usize = 3; // Coupled to the upstream window.\n",
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "const RETRY_LIMIT: usize = 4; // Coupled to the upstream window.\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust change");
    let stale: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .collect();

    assert_eq!(stale.len(), 1, "the unchanged trailing comment is stale");
    assert_eq!(stale[0].line, 1);
}

#[test]
fn same_line_comment_edit_attests_unchanged_code() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "const RETRY_LIMIT: usize = 3; // Coupled to the old window.\n",
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "const RETRY_LIMIT: usize = 3; // Coupled to the new window.\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "editing only the comment is its own attestation: {findings:#?}"
    );
}

#[test]
fn punctuation_only_comment_edit_is_not_attestation() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// Coupled to the upstream window.\nconst RETRY_LIMIT: usize = 3;\n",
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// Coupled to the upstream window!!!\nconst RETRY_LIMIT: usize = 4;\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "punctuation churn cannot masquerade as semantic review: {findings:#?}"
    );
}

#[test]
fn punctuation_only_comment_edit_does_not_change_the_owner_fingerprint() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// Coupled to the upstream window.\nconst RETRY_LIMIT: usize = 3;\n",
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// Coupled to the upstream window!!!\nconst RETRY_LIMIT: usize = 3;\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust comment change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "comments must stay outside their owner's code fingerprint: {findings:#?}"
    );
}

#[test]
fn editing_one_comment_does_not_attest_its_sibling() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    // First rationale.\n",
            "    first();\n",
            "    // Second rationale.\n",
            "    second();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    // Reviewed first rationale.\n",
            "    first();\n",
            "    // Second rationale.\n",
            "    changed_second();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust change");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(stale_lines, [4], "each comment needs its own attestation");
}

#[test]
fn adding_a_comment_does_not_attest_existing_comments() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    // First rationale.\n",
            "    first();\n",
            "    // Second rationale.\n",
            "    second();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    // First rationale.\n",
            "    first();\n",
            "    // Second rationale.\n",
            "    changed_second();\n",
            "    // New third rationale.\n",
            "    third();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust change");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [2, 4],
        "new comments cannot wash existing comments"
    );
}

#[test]
fn deleting_one_comment_attests_only_that_comment() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    // Removed rationale.\n",
            "    first();\n",
            "    // Surviving rationale.\n",
            "    second();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    changed_first();\n",
            "    // Surviving rationale.\n",
            "    second();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust change");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(stale_lines, [3], "only the surviving comment is stale");
}

#[test]
fn deleting_an_entire_owner_does_not_create_stale_comments() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn removed() {\n",
            "    // This owner disappears too.\n",
            "    run();\n",
            "}\n",
            "fn kept() {}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "fn kept() {}\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "deleted comments do not survive to become stale: {findings:#?}"
    );
}

#[test]
fn unchanged_comment_left_behind_by_deleted_owner_is_reparented() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "// This rationale must not become an orphan silently.\n",
            "fn removed() {\n",
            "    run();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// This rationale must not become an orphan silently.\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust change");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-reparented"),
        "unchanged text must not drift to a different owner: {findings:#?}"
    );
}

#[test]
fn owner_rename_without_an_exact_anchor_fails_closed() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// Coupled to the worker protocol.\nfn old_name() { run(); }\n",
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// Coupled to the worker protocol.\nfn new_name() { changed_run(); }\n",
    };

    let error = analyze_change(before, after).expect_err("an unanchored rename is ambiguous");

    assert!(
        matches!(error, AnalysisError::AmbiguousChange(_)),
        "renaming cannot guess owner correspondence: {error:#?}"
    );
}

#[test]
fn full_owner_replacement_without_an_exact_anchor_fails_closed() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn removed() {\n",
            "    // Shared rationale.\n",
            "    old_work();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn added() {\n",
            "    // Shared rationale.\n",
            "    new_work();\n",
            "}\n",
        ),
    };

    let error = analyze_change(before, after).expect_err("a full replacement is ambiguous");

    assert!(
        matches!(error, AnalysisError::AmbiguousChange(_)),
        "a same-position replacement must not be reported as a proven reparent: {error:#?}"
    );
}

#[test]
fn owner_rename_with_an_exact_code_anchor_stales_its_comment() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "// Coupled to the worker protocol.\n",
            "fn old_name() {\n",
            "    prepare();\n",
            "    old_work();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "// Coupled to the worker protocol.\n",
            "fn new_name() {\n",
            "    prepare();\n",
            "    new_work();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("the equal body line anchors the rename");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "an anchored rename still requires comment attestation: {findings:#?}"
    );
}

#[test]
fn javascript_arrow_binding_rename_stales_its_comment() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "const oldName = () => {\n",
            "    // Coupled to the callback identity.\n",
            "    prepare();\n",
            "    return work();\n",
            "};\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "const newName = () => {\n",
            "    // Coupled to the callback identity.\n",
            "    prepare();\n",
            "    return work();\n",
            "};\n",
        ),
    };

    let findings = analyze_change(before, after).expect("the body anchors the renamed callback");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "changing the binding changes the arrow function's meaning: {findings:#?}"
    );
}

#[test]
fn javascript_parent_type_rename_stales_nested_method_comment() {
    assert_parent_type_rename_stales_nested_comment(
        "worker.js",
        concat!(
            "class OldWorker {\n",
            "    run() {\n",
            "        // Coupled to the enclosing worker type.\n",
            "        prepare();\n",
            "        return work();\n",
            "    }\n",
            "}\n",
        ),
        concat!(
            "class NewWorker {\n",
            "    run() {\n",
            "        // Coupled to the enclosing worker type.\n",
            "        prepare();\n",
            "        return work();\n",
            "    }\n",
            "}\n",
        ),
        3,
    );
}

#[test]
fn python_parent_type_rename_stales_nested_method_comment() {
    assert_parent_type_rename_stales_nested_comment(
        "worker.py",
        concat!(
            "class OldWorker:\n",
            "    def run(self):\n",
            "        # Coupled to the enclosing worker type.\n",
            "        prepare()\n",
            "        return work()\n",
        ),
        concat!(
            "class NewWorker:\n",
            "    def run(self):\n",
            "        # Coupled to the enclosing worker type.\n",
            "        prepare()\n",
            "        return work()\n",
        ),
        3,
    );
}

#[test]
fn kotlin_parent_type_rename_stales_nested_method_comment() {
    assert_parent_type_rename_stales_nested_comment(
        "Worker.kt",
        concat!(
            "class OldWorker {\n",
            "    fun run() {\n",
            "        // Coupled to the enclosing worker type.\n",
            "        prepare()\n",
            "        work()\n",
            "    }\n",
            "}\n",
        ),
        concat!(
            "class NewWorker {\n",
            "    fun run() {\n",
            "        // Coupled to the enclosing worker type.\n",
            "        prepare()\n",
            "        work()\n",
            "    }\n",
            "}\n",
        ),
        3,
    );
}

#[test]
fn swift_parent_type_rename_stales_nested_method_comment() {
    assert_parent_type_rename_stales_nested_comment(
        "Worker.swift",
        concat!(
            "class OldWorker {\n",
            "    func run() {\n",
            "        // Coupled to the enclosing worker type.\n",
            "        prepare()\n",
            "        work()\n",
            "    }\n",
            "}\n",
        ),
        concat!(
            "class NewWorker {\n",
            "    func run() {\n",
            "        // Coupled to the enclosing worker type.\n",
            "        prepare()\n",
            "        work()\n",
            "    }\n",
            "}\n",
        ),
        3,
    );
}

#[test]
fn objective_c_parent_type_rename_stales_nested_method_comment() {
    assert_parent_type_rename_stales_nested_comment(
        "Worker.m",
        concat!(
            "@implementation OldWorker\n",
            "- (void)run {\n",
            "    // Coupled to the enclosing worker type.\n",
            "    [self prepare];\n",
            "    [self work];\n",
            "}\n",
            "@end\n",
        ),
        concat!(
            "@implementation NewWorker\n",
            "- (void)run {\n",
            "    // Coupled to the enclosing worker type.\n",
            "    [self prepare];\n",
            "    [self work];\n",
            "}\n",
            "@end\n",
        ),
        3,
    );
}

#[test]
fn parent_code_change_does_not_stale_nested_child_comment() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "class Worker extends OldBase {\n",
            "    run() {\n",
            "        // Coupled only to the run method.\n",
            "        return work();\n",
            "    }\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "class Worker extends NewBase {\n",
            "    run() {\n",
            "        // Coupled only to the run method.\n",
            "        return work();\n",
            "    }\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid parent-only JavaScript change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "ancestor code cannot wash into an unchanged nested owner: {findings:#?}"
    );
}

#[test]
fn insertion_at_an_owner_end_does_not_expand_its_anchor_set() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "consume(function () {\n",
            "    // Existing rationale.\n",
            "    keep();\n",
            "});\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "consume(function () {\n",
            "    // Existing rationale.\n",
            "    keep();\n",
            "});\n",
            "consume(function () {\n",
            "    // Added rationale.\n",
            "    added();\n",
            "});\n",
        ),
    };

    let findings = analyze_change(before, after).expect("the original owner has exact anchors");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed"
                && finding.rule != "comment-policy/comment-reparented"
        }),
        "the adjacent insertion must not contaminate the original owner: {findings:#?}"
    );
}

#[test]
fn anonymous_duplicate_owners_without_exact_anchors_fail_closed() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "function outer() {\n",
            "    consume(function () { alpha(); /* Same rationale. */ });\n",
            "    consume(function () { beta(); /* Same rationale. */ });\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "function outer() {\n",
            "    consume(function () { changed_beta(); /* Same rationale. */ });\n",
            "    consume(function () { changed_alpha(); /* Same rationale. */ });\n",
            "}\n",
        ),
    };

    let error = analyze_change(before, after).expect_err("anonymous correspondence is ambiguous");

    assert!(
        matches!(error, AnalysisError::AmbiguousChange(_)),
        "line position cannot identify rewritten anonymous owners: {error:#?}"
    );
}

#[test]
fn flat_anonymous_siblings_pair_through_the_public_change_seam() {
    const CALLBACKS: usize = 256;
    const CHANGED: usize = CALLBACKS / 2;

    let callbacks = |changed| {
        let mut source = String::new();
        for index in 0..CALLBACKS {
            writeln!(source, "consume(() => {{").expect("writing to a String cannot fail");
            writeln!(
                source,
                "    // Callback {index} relies on its configured target."
            )
            .expect("writing to a String cannot fail");
            writeln!(source, "    stable{index}();").expect("writing to a String cannot fail");
            if changed && index == CHANGED {
                writeln!(source, "    changed{index}();").expect("writing to a String cannot fail");
            } else {
                writeln!(source, "    original{index}();")
                    .expect("writing to a String cannot fail");
            }
            writeln!(source, "}});").expect("writing to a String cannot fail");
        }
        source
    };
    let before = callbacks(false);
    let after = callbacks(true);

    let findings = analyze_change(
        SourceFile {
            path: Path::new("callbacks.js"),
            text: &before,
        },
        SourceFile {
            path: Path::new("callbacks.js"),
            text: &after,
        },
    )
    .expect("exact anchors disambiguate flat anonymous callbacks");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(stale_lines, [CHANGED * 5 + 2]);
}

#[test]
fn same_line_anonymous_siblings_fail_closed_through_the_public_change_seam() {
    const CALLBACKS: usize = 256;

    let callbacks = |version| {
        let mut source = format!("const VERSION = {version};\n");
        for index in 0..CALLBACKS {
            write!(source, "(() => {{ stable{index}(); }})();")
                .expect("writing to a String cannot fail");
        }
        source.push('\n');
        source
    };
    let before = callbacks(1);
    let after = callbacks(2);

    let error = analyze_change(
        SourceFile {
            path: Path::new("callbacks.js"),
            text: &before,
        },
        SourceFile {
            path: Path::new("callbacks.js"),
            text: &after,
        },
    )
    .expect_err("one shared line cannot prove anonymous sibling correspondence");

    assert!(
        matches!(error, AnalysisError::AmbiguousChange(_)),
        "ambiguous same-line evidence must fail closed: {error:#?}"
    );
}

#[test]
fn regrouped_anonymous_preference_chain_fails_closed_through_the_public_change_seam() {
    const CALLBACKS: usize = 32;

    fn emit_callback(
        source: &mut String,
        statements: &mut impl Iterator<Item = usize>,
        count: usize,
    ) {
        source.push_str("(() => {\n");
        for statement in statements.take(count) {
            writeln!(source, "  statement_{statement:08}();")
                .expect("writing to a String cannot fail");
        }
        source.push_str("})();\n");
    }

    let statement_count = 2 * CALLBACKS * CALLBACKS - CALLBACKS;
    let mut before = String::new();
    let mut before_statements = 0..statement_count;
    for index in 0..CALLBACKS - 1 {
        emit_callback(
            &mut before,
            &mut before_statements,
            (2 * index + 1) + (2 * index + 2),
        );
    }
    emit_callback(&mut before, &mut before_statements, 2 * (CALLBACKS - 1) + 1);

    let mut after = String::new();
    let mut after_statements = 0..statement_count;
    emit_callback(&mut after, &mut after_statements, 1);
    for index in 1..CALLBACKS {
        emit_callback(
            &mut after,
            &mut after_statements,
            2 * index + (2 * index + 1),
        );
    }
    assert_eq!(before_statements.next(), None);
    assert_eq!(after_statements.next(), None);

    let error = analyze_change(
        SourceFile {
            path: Path::new("callbacks.js"),
            text: &before,
        },
        SourceFile {
            path: Path::new("callbacks.js"),
            text: &after,
        },
    )
    .expect_err("iterative preferences cannot prove anonymous owner correspondence");

    assert!(
        matches!(error, AnalysisError::AmbiguousChange(_)),
        "the analyzer must fail closed instead of peeling guessed pairs: {error:#?}"
    );
}

#[test]
fn format_only_owner_move_does_not_stale_its_comment() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    // Coupled to the worker protocol.\n",
            "    run();\n",
            "}\n",
            "fn neighbor() {}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn neighbor() {}\n",
            "\n",
            "fn work()\n",
            "{\n",
            "    //   Coupled   to the worker protocol!!!\n",
            "    run();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("stable identities survive formatting");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed"
                && finding.rule != "comment-policy/comment-reparented"
        }),
        "format and movement alone are not semantic owner changes: {findings:#?}"
    );
}

#[test]
fn nested_owner_change_does_not_stale_parent_comment() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn outer() {\n",
            "    // Outer rationale.\n",
            "    prepare();\n",
            "    fn inner() {\n",
            "        // Inner rationale.\n",
            "        run();\n",
            "    }\n",
            "    inner();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn outer() {\n",
            "    // Outer rationale.\n",
            "    prepare();\n",
            "    fn inner() {\n",
            "        // Inner rationale.\n",
            "        changed_run();\n",
            "    }\n",
            "    inner();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust change");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [5],
        "tokens belong to the narrowest owner, so child edits do not wash upward"
    );
}

#[test]
fn duplicate_comment_deletion_does_not_attest_the_survivor() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    // Same rationale.\n",
            "    first();\n",
            "    // Same rationale.\n",
            "    second();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn work() {\n",
            "    first();\n",
            "    // Same rationale.\n",
            "    changed_second();\n",
            "}\n",
        ),
    };
    let findings = analyze_change(before, after).expect("valid Rust change");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [3],
        "deleting one duplicate cannot disguise the unchanged survivor"
    );
}

#[test]
fn malformed_before_or_after_snapshot_fails_closed() {
    let valid = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "fn valid() {}\n",
    };
    let malformed = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "fn broken( {\n",
    };

    let before_error =
        analyze_change(malformed, valid).expect_err("malformed old source must fail closed");
    let after_error =
        analyze_change(valid, malformed).expect_err("malformed new source must fail closed");

    assert!(matches!(before_error, AnalysisError::Parse { .. }));
    assert!(matches!(after_error, AnalysisError::Parse { .. }));
}

#[test]
fn toml_multiline_value_edit_stales_its_key_comment() {
    let before = SourceFile {
        path: Path::new("config.toml"),
        text: concat!(
            "# Coupled to the ordered worker pool.\n",
            "workers = [\n",
            "    \"alpha\",\n",
            "    \"beta\",\n",
            "]\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("config.toml"),
        text: concat!(
            "# Coupled to the ordered worker pool.\n",
            "workers = [\n",
            "    \"alpha\",\n",
            "    \"gamma\",\n",
            "]\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid TOML change");
    let stale: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .collect();

    assert_eq!(stale.len(), 1, "the key comment should be stale once");
    assert_eq!(stale[0].line, 1);
}

#[test]
fn python_decorated_function_stales_its_leading_comment() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "# Coupled to the registered callback contract.\n",
            "@registered\n",
            "def work():\n",
            "    return 1\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "# Coupled to the registered callback contract.\n",
            "@registered\n",
            "def work():\n",
            "    return 2\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a decorator cannot split a leading comment from its function owner: {findings:#?}"
    );
}

#[test]
fn python_parenthesized_implicit_docstring_stales_as_one_comment() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
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
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
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
            "    return 2\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python change");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [2],
        "the ten-line expression is one docstring attestation"
    );
}

#[test]
fn python_top_level_class_code_change_stales_its_docstring() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Worker:\n",
            "    \"\"\"Coupled to the class-level retry state.\"\"\"\n",
            "    retry_limit = 3\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Worker:\n",
            "    \"\"\"Coupled to the class-level retry state.\"\"\"\n",
            "    retry_limit = 4\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python class change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "top-level class docs must follow their class owner: {findings:#?}"
    );
}

#[test]
fn python_unrelated_file_change_does_not_stale_class_docstring() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Worker:\n",
            "    \"\"\"Coupled only to the class-level retry state.\"\"\"\n",
            "    retry_limit = 3\n",
            "\n",
            "unrelated = 1\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Worker:\n",
            "    \"\"\"Coupled only to the class-level retry state.\"\"\"\n",
            "    retry_limit = 3\n",
            "\n",
            "unrelated = 2\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python file change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "a class docstring must not inherit the whole file as its owner: {findings:#?}"
    );
}

#[test]
fn python_nested_class_code_change_stales_its_docstring() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "def build():\n",
            "    class Worker:\n",
            "        \"\"\"Coupled to the nested class-level retry state.\"\"\"\n",
            "        retry_limit = 3\n",
            "    return Worker\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "def build():\n",
            "    class Worker:\n",
            "        \"\"\"Coupled to the nested class-level retry state.\"\"\"\n",
            "        retry_limit = 4\n",
            "    return Worker\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid nested Python class change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 3
        }),
        "nested class docs must follow the nested class owner: {findings:#?}"
    );
}

#[test]
fn python_method_only_change_does_not_stale_class_docstring() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Worker:\n",
            "    \"\"\"Coupled to the class-level retry state.\"\"\"\n",
            "    def run(self):\n",
            "        return 1\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Worker:\n",
            "    \"\"\"Coupled to the class-level retry state.\"\"\"\n",
            "    def run(self):\n",
            "        return 2\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python method change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "a nested method owner must isolate its code from class docs: {findings:#?}"
    );
}

#[test]
fn python_nested_class_identity_routes_change_after_outer_reordering() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Alpha:\n",
            "    class Worker:\n",
            "        \"\"\"Same rationale.\"\"\"\n",
            "        value = 'alpha'\n",
            "\n",
            "class Beta:\n",
            "    class Worker:\n",
            "        \"\"\"Same rationale.\"\"\"\n",
            "        value = 'changed-beta'\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Beta:\n",
            "    class Worker:\n",
            "        \"\"\"Same rationale.\"\"\"\n",
            "        value = 'beta'\n",
            "\n",
            "class Alpha:\n",
            "    class Worker:\n",
            "        \"\"\"Same rationale.\"\"\"\n",
            "        value = 'alpha'\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python class reorder");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        [3],
        "outer class ancestry must qualify the changed nested class"
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-reparented"),
        "a reorder must preserve nested owner identity: {findings:#?}"
    );
}

#[test]
fn python_valid_directive_remains_stale_when_owner_code_changes() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "def work():\n",
            "    value = missing_name  # noqa: F821\n",
            "    return value\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "def work():\n",
            "    value = another_missing_name  # noqa: F821\n",
            "    return value\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python directive change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "an operational exemption does not waive stale-comment review: {findings:#?}"
    );
}

#[test]
fn rust_attributed_function_stales_its_leading_comment() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "// Coupled to the inlining contract.\n",
            "#[inline]\n",
            "fn work() { run(); }\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "// Coupled to the inlining contract.\n",
            "#[inline]\n",
            "fn work() { changed_run(); }\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "an outer attribute cannot split a comment from its function: {findings:#?}"
    );
}

#[test]
fn changing_a_rust_function_attribute_stales_its_leading_comment() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "// Coupled to the call-site cost model.\n",
            "#[inline]\n",
            "fn work() { run(); }\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "// Coupled to the call-site cost model.\n",
            "#[cold]\n",
            "fn work() { run(); }\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust attribute change");

    assert!(findings.iter().any(|finding| {
        finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
    }));
}

#[test]
fn rust_function_change_stales_comment_inside_its_outer_attribute() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "#[allow(\n",
            "    /* Coupled to the generated function body. */\n",
            "    dead_code\n",
            ")]\n",
            "fn work() { run(); }\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "#[allow(\n",
            "    /* Coupled to the generated function body. */\n",
            "    dead_code\n",
            ")]\n",
            "fn work() { changed_run(); }\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust function change");

    assert!(findings.iter().any(|finding| {
        finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
    }));
}

#[test]
fn inline_comment_before_a_function_is_not_leading_function_commentary() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "work(); // Coupled to the work call.\n",
            "function next() { return 1; }\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "work(); // Coupled to the work call.\n",
            "function next() { return 2; }\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid JavaScript change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "inline commentary on prior code must not drift to the next owner: {findings:#?}"
    );
}

#[test]
fn reordering_owners_with_identical_comments_preserves_owner_identity() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn alpha() {\n",
            "    // Same rationale.\n",
            "    alpha_work();\n",
            "}\n",
            "fn beta() {\n",
            "    // Same rationale.\n",
            "    beta_work();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn beta() {\n",
            "    // Same rationale.\n",
            "    beta_work();\n",
            "}\n",
            "fn alpha() {\n",
            "    // Same rationale.\n",
            "    alpha_work();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust reorder");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "named owners, not file order, must anchor duplicate comments: {findings:#?}"
    );
}

#[test]
fn deleting_one_duplicate_owner_does_not_steal_the_survivors_comment() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn removed() {\n",
            "    // Same rationale.\n",
            "    removed_work();\n",
            "}\n",
            "fn kept() {\n",
            "    // Same rationale.\n",
            "    kept_work();\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "fn kept() {\n",
            "    // Same rationale.\n",
            "    changed_kept_work();\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust deletion");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(stale_lines, [2], "the surviving owner still needs review");
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-reparented"),
        "the deleted owner's duplicate must not capture the survivor: {findings:#?}"
    );
}

#[test]
fn punctuation_only_block_expansion_still_runs_the_static_budget() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "/* ok */\nconst LIMIT: usize = 4;\n",
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "/* ok\n!!!\n???\n### */\nconst LIMIT: usize = 4;\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust comment edit");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "raw comment growth must trigger budgets even when its normalized words are unchanged: {findings:#?}"
    );
}

#[test]
fn inner_function_change_does_not_activate_outer_function_budget() {
    let before_text = concat!(
        "fn outer() {\n",
        "    // First outer rationale.\n",
        "    first();\n",
        "    // Second outer rationale.\n",
        "    second();\n",
        "    // Third outer rationale.\n",
        "    third();\n",
        "    // Fourth outer rationale.\n",
        "    fourth();\n",
        "    fn inner() { run(); }\n",
        "    inner();\n",
        "}\n",
    );
    let after_text = before_text.replace("inner() { run(); }", "inner() { changed_run(); }");
    let baseline = analyze_all(SourceFile {
        path: Path::new("src/lib.rs"),
        text: before_text,
    })
    .expect("valid Rust baseline");
    assert!(
        baseline
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "the fixture must actually put the outer function over budget"
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("src/lib.rs"),
            text: before_text,
        },
        SourceFile {
            path: Path::new("src/lib.rs"),
            text: &after_text,
        },
    )
    .expect("valid nested Rust change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/function-comment-budget"),
        "an inner owner must not activate its ancestor's pre-existing debt: {findings:#?}"
    );
}

#[test]
fn swift_leaf_comment_addition_activates_its_type_budget() {
    assert_leaf_comment_addition_activates_type_budget(
        "Worker.swift",
        concat!(
            "class Worker {\n",
            "    // First.\n",
            "    let first = { 1 }\n",
            "    let second = { 2 }\n",
            "}\n",
        ),
        concat!(
            "class Worker {\n",
            "    // First.\n",
            "    let first = { 1 }\n",
            "    // Second.\n",
            "    let second = { 2 }\n",
            "}\n",
        ),
    );
}

#[test]
fn kotlin_leaf_comment_addition_activates_its_type_budget() {
    assert_leaf_comment_addition_activates_type_budget(
        "Worker.kt",
        concat!(
            "class Worker {\n",
            "    // First.\n",
            "    val first = { 1 }\n",
            "    val second = { 2 }\n",
            "}\n",
        ),
        concat!(
            "class Worker {\n",
            "    // First.\n",
            "    val first = { 1 }\n",
            "    // Second.\n",
            "    val second = { 2 }\n",
            "}\n",
        ),
    );
}

#[test]
fn tsx_leaf_comment_addition_activates_its_type_budget() {
    assert_leaf_comment_addition_activates_type_budget(
        "Worker.tsx",
        concat!(
            "class Worker {\n",
            "    // First.\n",
            "    static first = () => 1;\n",
            "    static second = () => 2;\n",
            "}\n",
        ),
        concat!(
            "class Worker {\n",
            "    // First.\n",
            "    static first = () => 1;\n",
            "    // Second.\n",
            "    static second = () => 2;\n",
            "}\n",
        ),
    );
}

#[test]
fn leaf_inside_function_selects_nearest_budget_without_activating_type_debt() {
    let before_text = concat!(
        "class Worker {\n",
        "    // First type rationale.\n",
        "    first = 1;\n",
        "    // Second type rationale.\n",
        "    second = 2;\n",
        "    // Third type rationale.\n",
        "    third = 3;\n",
        "    // Fourth type rationale.\n",
        "    fourth = 4;\n",
        "    run() {\n",
        "        // First local rationale.\n",
        "        const first = () => 1;\n",
        "        const second = () => 2;\n",
        "    }\n",
        "}\n",
    );
    let after_text = before_text.replace(
        "        const second = () => 2;",
        "        // Local rationale.\n        const second = () => 2;",
    );
    let baseline = analyze_all(SourceFile {
        path: Path::new("Worker.tsx"),
        text: before_text,
    })
    .expect("valid TSX baseline");
    assert!(
        baseline
            .iter()
            .any(|finding| finding.rule == "comment-policy/type-comment-budget"),
        "the fixture must put the outer type over budget"
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Worker.tsx"),
            text: before_text,
        },
        SourceFile {
            path: Path::new("Worker.tsx"),
            text: &after_text,
        },
    )
    .expect("valid nested TSX change");
    let has_function_budget = findings
        .iter()
        .any(|finding| finding.rule == "comment-policy/function-comment-budget");
    let has_type_budget = findings
        .iter()
        .any(|finding| finding.rule == "comment-policy/type-comment-budget");

    assert!(
        has_function_budget && !has_type_budget,
        "the nearest function budget must be selected without activating the type parent: {findings:#?}"
    );
}

#[test]
fn javascript_automatic_semicolon_insertion_changes_the_owner_fingerprint() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "function read() {\n",
            "    // Returning the shared value is part of the protocol.\n",
            "    return value;\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "function read() {\n",
            "    // Returning the shared value is part of the protocol.\n",
            "    return\n",
            "    value;\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid JavaScript change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "AST structure, not only leaf tokens, must fingerprint owner code: {findings:#?}"
    );
}

#[test]
fn rust_impl_namespaces_disambiguate_reordered_methods() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "impl Alpha {\n",
            "    fn run() {\n",
            "        // Same rationale.\n",
            "        alpha_work();\n",
            "    }\n",
            "}\n",
            "impl Beta {\n",
            "    fn run() {\n",
            "        // Same rationale.\n",
            "        beta_work();\n",
            "    }\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "impl Beta {\n",
            "    fn run() {\n",
            "        // Same rationale.\n",
            "        beta_work();\n",
            "    }\n",
            "}\n",
            "impl Alpha {\n",
            "    fn run() {\n",
            "        // Same rationale.\n",
            "        alpha_work();\n",
            "    }\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust reorder");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "the impl target must qualify a method identity: {findings:#?}"
    );
}

#[test]
fn rust_impl_namespace_ignores_generic_header_trivia() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "impl<T> Contract<T> for Worker<T> {\n",
            "    fn run(&self) {\n",
            "        // Coupled to this implementation.\n",
            "        work();\n",
            "    }\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "impl<T> Contract /* separator */ < T > for Worker < T > {\n",
            "    fn run(&self) {\n",
            "        // Coupled to this implementation.\n",
            "        work();\n",
            "    }\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust formatting change");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "whitespace and comments cannot change an impl namespace identity: {findings:#?}"
    );
}

#[test]
fn rust_impl_namespace_detects_semantic_target_rename() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "impl<T> Worker<T> {\n",
            "    fn run(&self) {\n",
            "        // Coupled to this implementation.\n",
            "        work();\n",
            "    }\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: concat!(
            "impl<T> RenamedWorker<T> {\n",
            "    fn run(&self) {\n",
            "        // Coupled to this implementation.\n",
            "        work();\n",
            "    }\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Rust semantic change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 3
        }),
        "a renamed impl target must stale its unchanged method comment: {findings:#?}"
    );
}

#[test]
fn python_class_namespaces_disambiguate_reordered_methods() {
    let before = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Alpha:\n",
            "    def run(self):\n",
            "        # Same rationale.\n",
            "        return alpha_work()\n",
            "\n",
            "class Beta:\n",
            "    def run(self):\n",
            "        # Same rationale.\n",
            "        return beta_work()\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.py"),
        text: concat!(
            "class Beta:\n",
            "    def run(self):\n",
            "        # Same rationale.\n",
            "        return beta_work()\n",
            "\n",
            "class Alpha:\n",
            "    def run(self):\n",
            "        # Same rationale.\n",
            "        return alpha_work()\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Python reorder");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "the class path must qualify a method identity: {findings:#?}"
    );
}

#[test]
fn javascript_class_namespaces_disambiguate_reordered_methods() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "class Alpha {\n",
            "    run() {\n",
            "        // Same rationale.\n",
            "        return alphaWork();\n",
            "    }\n",
            "}\n",
            "class Beta {\n",
            "    run() {\n",
            "        // Same rationale.\n",
            "        return betaWork();\n",
            "    }\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "class Beta {\n",
            "    run() {\n",
            "        // Same rationale.\n",
            "        return betaWork();\n",
            "    }\n",
            "}\n",
            "class Alpha {\n",
            "    run() {\n",
            "        // Same rationale.\n",
            "        return alphaWork();\n",
            "    }\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid JavaScript reorder");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "the class path must qualify a method identity: {findings:#?}"
    );
}

#[test]
fn changing_an_internal_operator_is_meaningful_comment_attestation() {
    let before = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "function allowed(x, limit) {\n",
            "    // Valid while x < limit.\n",
            "    return x < limit;\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("worker.js"),
        text: concat!(
            "function allowed(x, limit) {\n",
            "    // Valid while x > limit.\n",
            "    return x > limit;\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid JavaScript change");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "operators carry meaning and must survive attestation normalization: {findings:#?}"
    );
}

#[test]
fn whitespace_and_terminal_punctuation_churn_is_not_comment_attestation() {
    let before = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "// Coupled to the upstream window.\nconst RETRY_LIMIT: usize = 3;\n",
    };
    let after = SourceFile {
        path: Path::new("src/lib.rs"),
        text: "//   Coupled   to the upstream window!!!\nconst RETRY_LIMIT: usize = 4;\n",
    };

    let findings = analyze_change(before, after).expect("valid Rust change");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-owner-changed"),
        "format-only churn cannot attest an owner change: {findings:#?}"
    );
}

#[test]
fn deeply_nested_syntax_is_walked_without_using_the_call_stack() {
    const DEPTH: usize = 4_096;
    let source = format!(
        "const VALUE = {}0{};\n",
        "[".repeat(DEPTH),
        "]".repeat(DEPTH)
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("worker.js"),
        text: &source,
    })
    .expect("tree-sitter accepts deeply nested JavaScript");

    assert!(
        findings.is_empty(),
        "the fixture has no comments: {findings:#?}"
    );
}

#[test]
fn deeply_nested_kotlin_change_preserves_owner_correspondence() {
    const DEPTH: usize = 200;

    let nested_source = |value| {
        let mut source = String::new();
        for level in 0..DEPTH {
            writeln!(source, "class Level{level} {{").expect("writing to a String cannot fail");
        }
        writeln!(source, "fun run(): Int {{").expect("writing to a String cannot fail");
        writeln!(source, "// Coupled to the deepest implementation.")
            .expect("writing to a String cannot fail");
        writeln!(source, "return {value}").expect("writing to a String cannot fail");
        source.push_str("}\n");
        for _ in 0..DEPTH {
            source.push_str("}\n");
        }
        source
    };
    let before = nested_source(1);
    let after = nested_source(2);

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Deep.kt"),
            text: &before,
        },
        SourceFile {
            path: Path::new("Deep.kt"),
            text: &after,
        },
    )
    .expect("valid deeply nested Kotlin change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == DEPTH + 2
        }),
        "the deepest unchanged comment must remain attached to its changed function: {findings:#?}"
    );
}
