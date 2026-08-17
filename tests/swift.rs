use std::path::Path;

use fuck_ai_comments::{SourceFile, analyze_all, analyze_change, supports_path};

#[test]
fn swift_extension_is_registered() {
    assert!(supports_path(Path::new("Renderer.swift")));
}

#[test]
fn swift_short_function_rejects_four_comment_lines() {
    let source = concat!(
        "func render() -> Int {\n",
        "    // one\n",
        "    // two\n",
        "    // three\n",
        "    // four\n",
        "    return 1\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Renderer.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "Swift functions must use the shared function budget: {findings:#?}"
    );
}

#[test]
fn swift_let_rejects_four_leading_comment_lines() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "let retryLimit = 3\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Constants.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a Swift let binding cannot launder a long explanation: {findings:#?}"
    );
}

#[test]
fn swift_callable_container_keeps_the_leaf_comment_budget() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "let handlers = [{ (value: Int) in value + 1 }]\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Handlers.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a closure cannot upgrade a let binding to the function allowance: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/function-comment-budget"),
        "the semantic owner is the let binding, not its contained closure: {findings:#?}"
    );
}

#[test]
fn swift_function_edit_stales_unchanged_comment() {
    let before = concat!(
        "func render() -> Int {\n",
        "    // Coupled to the layout pass.\n",
        "    return 1\n",
        "}\n",
    );
    let after = before.replace("return 1", "return 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Renderer.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Renderer.swift"),
            text: &after,
        },
    )
    .expect("valid Swift change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "Swift code changes must attest each surviving comment: {findings:#?}"
    );
}

#[test]
fn swift_closure_binding_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the transform contract.\n",
        "let transform = { value in value + 1 }\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.swift"),
            text: &after,
        },
    )
    .expect("valid Swift closure binding change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a direct closure binding owns its leading comment and callable body: {findings:#?}"
    );
}

#[test]
fn swift_wrapped_closure_binding_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the configured transform.\n",
        "let transform = makeTransform(configuration) { value in value + 1 }\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.swift"),
            text: &after,
        },
    )
    .expect("valid wrapped Swift closure change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a wrapped closure binding's leading comment must track its body: {findings:#?}"
    );
}

#[test]
fn swift_closure_assignment_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the installed handler.\n",
        "handler = { oldCall() }\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.swift"),
            text: &after,
        },
    )
    .expect("valid Swift assignment change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a callable assignment must own its leading comment and closure body: {findings:#?}"
    );
}

#[test]
fn swift_chained_closure_assignment_stales_its_leading_comment() {
    let before = concat!(
        "root = first = second = {\n",
        "    // Coupled to the innermost installed handler.\n",
        "    oldCall()\n",
        "}\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.swift"),
            text: &after,
        },
    )
    .expect("valid chained Swift assignment change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 2
                && finding.message.contains("leaf `second`")
        }),
        "the innermost assignment must retain the closure body and identity: {findings:#?}"
    );
}

#[test]
fn swift_closure_assignment_keeps_nested_closure_owner_independent() {
    let before = concat!(
        "handler = {\n",
        "    run {\n",
        "        // Coupled only to the nested call.\n",
        "        oldNestedCall()\n",
        "    }\n",
        "    oldOuterCall()\n",
        "}\n",
    );
    let after = before.replace("oldOuterCall", "newOuterCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.swift"),
            text: &after,
        },
    )
    .expect("valid nested Swift assignment change");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed" || finding.line != 3
        }),
        "an assignment owner must not swallow a nested closure owner: {findings:#?}"
    );
}

#[test]
fn swift_closure_assignments_pair_by_canonical_target_across_reordering() {
    let before = concat!(
        "// Same rationale.\n",
        "first = { firstCall() }\n",
        "// Same rationale.\n",
        "second = { secondCall() }\n",
    );
    let after = concat!(
        "// Same rationale.\n",
        "second = { secondCall() }\n",
        "// Same rationale.\n",
        "first = { changedFirstCall() }\n",
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handlers.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handlers.swift"),
            text: after,
        },
    )
    .expect("valid reordered Swift assignments");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(
        stale_lines,
        [3],
        "only the changed assignment should stale its comment"
    );
}

#[test]
fn swift_nested_closure_keeps_an_independent_stale_owner() {
    let before = concat!(
        "let transform = { value in\n",
        "    let nested = { inner in\n",
        "        // Coupled only to the nested transformation.\n",
        "        inner + 1\n",
        "    }\n",
        "    value + 1\n",
        "}\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.swift"),
            text: &after,
        },
    )
    .expect("valid nested Swift closure change");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed" || finding.line != 3
        }),
        "an outer binding must not swallow its nested callable owner: {findings:#?}"
    );
}

#[test]
fn attached_swiftlint_directives_do_not_consume_narrative_budget() {
    let sources = [
        concat!(
            "func work() {\n",
            "    // swiftlint:disable:next force_cast\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    perform() // swiftlint:disable:this force_cast\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    perform()\n",
            "    // swiftlint:disable:previous force_cast\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    // swiftlint:disable force_cast\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    perform()\n",
            "    // swiftlint:enable force_cast\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
    ];

    for source in sources {
        let findings = analyze_all(SourceFile {
            path: Path::new("Worker.swift"),
            text: source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings.is_empty(),
            "an operational SwiftLint directive stays outside the narrative ratio: {findings:#?}"
        );
    }
}

#[test]
fn swiftlint_custom_rule_identifier_is_metadata() {
    let source = concat!(
        "func work() {\n",
        "    // swiftlint:disable:next my-custom-rule\n",
        "    perform()\n",
        "    // ordinary explanation\n",
        "    finish()\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Worker.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings.is_empty(),
        "an operational custom SwiftLint rule stays outside the narrative ratio: {findings:#?}"
    );
}

#[test]
fn swiftlint_trailing_explanation_is_metadata() {
    let source = concat!(
        "func work() {\n",
        "    // swiftlint:disable:next force_cast - required by the API\n",
        "    perform()\n",
        "    // ordinary explanation\n",
        "    finish()\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Worker.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings.is_empty(),
        "a documented SwiftLint directive stays outside the narrative ratio: {findings:#?}"
    );
}

#[test]
fn swiftlint_directives_require_a_rule_identifier() {
    let directives = [
        "// swiftlint:disable:next",
        "// swiftlint:disable:next - explanation",
        "// swiftlint:disable:next - ",
        "// swiftlint:disable:next \u{2003} - explanation",
    ];
    for directive in directives {
        let source = format!(
            "func work() {{\n    {directive}\n    perform()\n    // ordinary explanation\n    finish()\n}}\n"
        );
        let findings = analyze_all(SourceFile {
            path: Path::new("Worker.swift"),
            text: &source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "a SwiftLint command without a rule identifier remains narrative ({directive:?}): {findings:#?}"
        );
    }
}

#[test]
fn trailing_swiftlint_region_directive_is_metadata() {
    for action in ["disable", "enable"] {
        let source = format!(
            "func work() {{\n    perform() // swiftlint:{action} my-custom-rule\n    // ordinary explanation\n    finish()\n}}\n"
        );
        let findings = analyze_all(SourceFile {
            path: Path::new("Worker.swift"),
            text: &source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings.is_empty(),
            "a trailing SwiftLint region directive stays outside the narrative ratio ({action}): {findings:#?}"
        );
    }
}

#[test]
fn deeply_nested_swift_directives_remain_attached() {
    let depth = 400;
    let mut source = String::new();
    for index in 0..depth {
        source.push_str(&format!(
            "class C{index} {{\n// swiftlint:disable:next force_cast\n"
        ));
    }
    source.push_str("let value = 1\n");
    source.push_str(&"}\n".repeat(depth));

    let findings = analyze_all(SourceFile {
        path: Path::new("Deep.swift"),
        text: &source,
    })
    .expect("valid nested Swift");

    assert!(
        findings.is_empty(),
        "every nested directive must stay attached to its next line: {findings:#?}"
    );
}

#[test]
fn attached_swiftformat_directive_does_not_consume_narrative_budget() {
    let source = concat!(
        "func work() {\n",
        "    // swiftformat:disable:next indent braces semicolons linebreaks\n",
        "    self.perform()\n",
        "    // ordinary explanation\n",
        "    finish()\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Worker.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings.is_empty(),
        "an operational SwiftFormat directive stays outside the narrative ratio: {findings:#?}"
    );
}

#[test]
fn malformed_or_unattached_swiftformat_prefixes_remain_narrative() {
    let sources = [
        concat!(
            "func work() {\n",
            "    // swiftformat:disable:next redundantSelf,braces\n",
            "    self.perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    // swiftformat:disbible:next redundantSelf\n",
            "    self.perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    /// swiftformat:disable:next redundantSelf\n",
            "    self.perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    // swiftformat:disable:next redundantSelf\n",
            "\n",
            "    self.perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
    ];

    for source in sources {
        let findings = analyze_all(SourceFile {
            path: Path::new("Worker.swift"),
            text: source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "a malformed, narrative, or detached prefix cannot become metadata: {findings:#?}"
        );
    }
}

#[test]
fn swiftformat_options_directives_follow_their_placement() {
    let sources = [
        concat!(
            "func work() {\n",
            "    // swiftformat:options --indent 2 --allman true\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    perform() // swiftformat:options:this --indent 2\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    // swiftformat:options:next --indent 2\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    perform()\n",
            "    // swiftformat:options:previous --indent 2\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
    ];

    for source in sources {
        let findings = analyze_all(SourceFile {
            path: Path::new("Worker.swift"),
            text: source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings.is_empty(),
            "an attached SwiftFormat option directive is metadata: {findings:#?}"
        );
    }
}

#[test]
fn malformed_or_unattached_swiftformat_options_remain_narrative() {
    let directives = [
        "// swiftformat:options",
        "// swiftformat:options because prose",
        "// swiftformat:options -- 2",
        "// swiftformat:option:next --indent 2",
        "/// swiftformat:options:next --indent 2",
    ];
    for directive in directives {
        let source = format!(
            "func work() {{\n    {directive}\n    perform()\n    // ordinary explanation\n    finish()\n}}\n"
        );
        let findings = analyze_all(SourceFile {
            path: Path::new("Worker.swift"),
            text: &source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "malformed option syntax cannot become metadata ({directive}): {findings:#?}"
        );
    }

    let source = concat!(
        "func work() {\n",
        "    // swiftformat:options:next --indent 2\n",
        "\n",
        "    perform()\n",
        "    // ordinary explanation\n",
        "    finish()\n",
        "}\n",
    );
    let findings = analyze_all(SourceFile {
        path: Path::new("Worker.swift"),
        text: source,
    })
    .expect("valid Swift should parse");
    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a detached option directive remains narrative: {findings:#?}"
    );
}

#[test]
fn malformed_or_unattached_swiftlint_prefixes_remain_narrative() {
    let sources = [
        concat!(
            "func work() {\n",
            "    /// swiftlint:disable:next force_cast\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    /* swiftlint:disable:next force_cast */\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    // swiftlint:disable:next force_cast\n",
            "\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
        concat!(
            "func work() {\n",
            "    // swiftlint:disable:this force_cast\n",
            "    perform()\n",
            "    // ordinary explanation\n",
            "    finish()\n",
            "}\n",
        ),
    ];

    for source in sources {
        let findings = analyze_all(SourceFile {
            path: Path::new("Worker.swift"),
            text: source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "a malformed, narrative, or detached prefix cannot become metadata: {findings:#?}"
        );
    }
}

#[test]
fn swiftlint_directives_cannot_bypass_the_absolute_owner_cap() {
    let source = concat!(
        "func work() {\n",
        "    perform(1) // swiftlint:disable:this force_cast\n",
        "    perform(2) // swiftlint:disable:this force_cast\n",
        "    perform(3) // swiftlint:disable:this force_cast\n",
        "    perform(4) // swiftlint:disable:this force_cast\n",
        "    perform(5) // swiftlint:disable:this force_cast\n",
        "    perform(6) // swiftlint:disable:this force_cast\n",
        "    perform(7) // swiftlint:disable:this force_cast\n",
        "    perform(8) // swiftlint:disable:this force_cast\n",
        "    perform(9) // swiftlint:disable:this force_cast\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Worker.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "tool metadata cannot grant an unlimited comment allowance: {findings:#?}"
    );
}

#[test]
fn swiftformat_directives_cannot_bypass_the_absolute_owner_cap() {
    let source = concat!(
        "func work() {\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(1)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(2)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(3)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(4)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(5)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(6)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(7)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(8)\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.perform(9)\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Worker.swift"),
        text: source,
    })
    .expect("valid Swift should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "SwiftFormat metadata cannot grant an unlimited comment allowance: {findings:#?}"
    );
}

#[test]
fn swiftlint_directive_remains_stale_when_owner_code_changes() {
    let before = concat!(
        "func work(value: Any) {\n",
        "    _ = value as! Int // swiftlint:disable:this force_cast\n",
        "}\n",
    );
    let after = before.replace("as! Int", "as! String");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Worker.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Worker.swift"),
            text: &after,
        },
    )
    .expect("valid Swift change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "operational metadata does not waive stale-comment review: {findings:#?}"
    );
}

#[test]
fn swiftformat_directive_remains_stale_when_owner_code_changes() {
    let before = concat!(
        "func work() {\n",
        "    // swiftformat:options:next --indent 2\n",
        "    self.oldCall()\n",
        "}\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Worker.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Worker.swift"),
            text: &after,
        },
    )
    .expect("valid Swift change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "SwiftFormat metadata does not waive stale-comment review: {findings:#?}"
    );
}

#[test]
fn swift_grouped_closure_binding_owns_every_direct_closure_body() {
    let before = concat!(
        "// Coupled to both transforms.\n",
        "let first = { value in\n",
        "    // Shared by the grouped binding.\n",
        "    value + 1\n",
        "}, second = { value in\n",
        "    value + 2\n",
        "}\n",
    );
    let after = before.replace("value + 2", "value + 3");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transforms.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transforms.swift"),
            text: &after,
        },
    )
    .expect("valid grouped Swift closure change");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(
        stale_lines,
        [1, 3],
        "all comments in a grouped binding share its conservative owner"
    );
}

#[test]
fn swift_overloads_pair_by_signature_across_reordering() {
    let before = concat!(
        "final class Renderer {\n",
        "    func render(value: Int) -> Int {\n",
        "        // Coupled to rendering.\n",
        "        return value + 1\n",
        "    }\n",
        "    func render(value: String) -> Int {\n",
        "        // Coupled to rendering.\n",
        "        return value.count\n",
        "    }\n",
        "}\n",
    );
    let after = concat!(
        "final class Renderer {\n",
        "    func render(value: String) -> Int {\n",
        "        // Coupled to rendering.\n",
        "        return value.count\n",
        "    }\n",
        "    func render(value: Int) -> Int {\n",
        "        // Coupled to rendering.\n",
        "        return value + 2\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Renderer.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Renderer.swift"),
            text: after,
        },
    )
    .expect("Swift overload signatures disambiguate reordered functions");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(
        stale_lines,
        [7],
        "only the changed integer overload should stale its comment"
    );
}

#[test]
fn swift_initializers_pair_by_signature_across_reordering() {
    let before = concat!(
        "final class Store {\n",
        "    init(value: Int) {\n",
        "        // Coupled to construction.\n",
        "        consumeInteger(value)\n",
        "    }\n",
        "    init(value: String) {\n",
        "        // Coupled to construction.\n",
        "        consumeString(value)\n",
        "    }\n",
        "}\n",
    );
    let after = concat!(
        "final class Store {\n",
        "    init(value: String) {\n",
        "        // Coupled to construction.\n",
        "        consumeString(value)\n",
        "    }\n",
        "    init(value: Int) {\n",
        "        // Coupled to construction.\n",
        "        consumeInteger(value + 1)\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Store.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Store.swift"),
            text: after,
        },
    )
    .expect("Swift initializer signatures disambiguate reordered overloads");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(stale_lines, [7]);
}

#[test]
fn swift_subscripts_pair_by_signature_across_reordering() {
    let before = concat!(
        "final class Store {\n",
        "    subscript(index: Int) -> Int {\n",
        "        // Coupled to lookup.\n",
        "        index + 1\n",
        "    }\n",
        "    subscript(key: String) -> Int {\n",
        "        // Coupled to lookup.\n",
        "        key.count\n",
        "    }\n",
        "}\n",
    );
    let after = concat!(
        "final class Store {\n",
        "    subscript(key: String) -> Int {\n",
        "        // Coupled to lookup.\n",
        "        key.count\n",
        "    }\n",
        "    subscript(index: Int) -> Int {\n",
        "        // Coupled to lookup.\n",
        "        index + 2\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Store.swift"),
            text: before,
        },
        SourceFile {
            path: Path::new("Store.swift"),
            text: after,
        },
    )
    .expect("Swift subscript signatures disambiguate reordered overloads");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(stale_lines, [7]);
}

#[test]
fn swift_body_properties_use_local_function_budgets() {
    let properties = [
        concat!(
            "    var shorthand: Int {\n",
            "        // first local rationale\n",
            "        // second local rationale\n",
            "        return 1\n",
            "    }\n",
        ),
        concat!(
            "    var explicit: Int {\n",
            "        // first local rationale\n",
            "        // second local rationale\n",
            "        get { 1 }\n",
            "        set { consume(newValue) }\n",
            "    }\n",
        ),
        concat!(
            "    var observed = 0 {\n",
            "        // first local rationale\n",
            "        // second local rationale\n",
            "        willSet { consume(newValue) }\n",
            "        didSet { consume(oldValue) }\n",
            "    }\n",
        ),
    ];
    let large_type_prefix = concat!(
        "final class Settings {\n",
        "    var field01 = 1\n",
        "    var field02 = 2\n",
        "    var field03 = 3\n",
        "    var field04 = 4\n",
        "    var field05 = 5\n",
        "    var field06 = 6\n",
        "    var field07 = 7\n",
        "    var field08 = 8\n",
        "    var field09 = 9\n",
        "    var field10 = 10\n",
        "    var field11 = 11\n",
        "    var field12 = 12\n",
    );

    for property in properties {
        let source = format!("{large_type_prefix}{property}}}\n");
        let findings = analyze_all(SourceFile {
            path: Path::new("Settings.swift"),
            text: &source,
        })
        .expect("valid Swift should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "a short body-bearing property cannot borrow its large type's budget: {findings:#?}"
        );
    }
}

#[test]
fn swift_protocol_accessor_requirements_remain_bodyless() {
    let requirements = [
        concat!(
            "    var selected: Int {\n",
            "        // first protocol rationale\n",
            "        // second protocol rationale\n",
            "        get\n",
            "        set\n",
            "    }\n",
        ),
        concat!(
            "    subscript(index: Int) -> Int {\n",
            "        // first protocol rationale\n",
            "        // second protocol rationale\n",
            "        get\n",
            "    }\n",
        ),
    ];
    let large_type_prefix = concat!(
        "protocol Requirements {\n",
        "    var field01: Int { get }\n",
        "    var field02: Int { get }\n",
        "    var field03: Int { get }\n",
        "    var field04: Int { get }\n",
        "    var field05: Int { get }\n",
        "    var field06: Int { get }\n",
        "    var field07: Int { get }\n",
        "    var field08: Int { get }\n",
        "    var field09: Int { get }\n",
        "    var field10: Int { get }\n",
        "    var field11: Int { get }\n",
        "    var field12: Int { get }\n",
    );

    for requirement in requirements {
        let source = format!("{large_type_prefix}{requirement}}}\n");
        let findings = analyze_all(SourceFile {
            path: Path::new("Requirements.swift"),
            text: &source,
        })
        .expect("valid Swift protocol should parse");

        assert!(
            findings
                .iter()
                .all(|finding| finding.rule != "comment-policy/function-comment-budget"),
            "an accessor requirement without a body is not a function owner: {findings:#?}"
        );
    }
}

#[test]
fn deeply_nested_swift_types_and_wrappers_analyze_through_the_public_seam() {
    let depth = 64;
    let mut source = String::new();
    for index in 0..depth {
        source.push_str(&format!("class C{index} {{\n"));
    }
    source.push_str("let handler = ");
    source.push_str(&"wrap(".repeat(depth));
    source.push_str("{ 1 }");
    source.push_str(&")".repeat(depth));
    source.push('\n');
    source.push_str(&"}\n".repeat(depth));

    let findings = analyze_all(SourceFile {
        path: Path::new("Deep.swift"),
        text: &source,
    })
    .expect("deep Swift should parse without ancestor rescans");

    assert!(findings.is_empty());
}
