use std::path::Path;

use fuck_ai_comments::{SourceFile, analyze_all, analyze_change, supports_path};

#[test]
fn objective_c_source_extension_is_registered() {
    assert!(supports_path(Path::new("Renderer.m")));
    assert!(
        !supports_path(Path::new("Header.h")),
        ".h is ambiguous across C, C++, and Objective-C"
    );
    assert!(
        !supports_path(Path::new("Renderer.mm")),
        "the Objective-C grammar does not support complete Objective-C++"
    );
}

#[test]
fn objective_c_short_method_rejects_four_comment_lines() {
    let source = concat!(
        "@implementation Renderer\n",
        "- (NSInteger)render {\n",
        "    // one\n",
        "    // two\n",
        "    // three\n",
        "    // four\n",
        "    return 1;\n",
        "}\n",
        "@end\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Renderer.m"),
        text: source,
    })
    .expect("valid Objective-C should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "method comments must use the function budget: {findings:#?}"
    );
}

#[test]
fn objective_c_const_rejects_four_leading_comment_lines() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "static const NSInteger RetryLimit = 3;\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Constants.m"),
        text: source,
    })
    .expect("valid Objective-C should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a const declaration cannot launder a long explanation: {findings:#?}"
    );
}

#[test]
fn objective_c_callable_container_keeps_the_leaf_comment_budget() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "static NSArray * const Handlers = @[^{ return; }];\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Handlers.m"),
        text: source,
    })
    .expect("valid Objective-C should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a block cannot upgrade a const binding to the function allowance: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/function-comment-budget"),
        "the semantic owner is the const binding, not its contained block: {findings:#?}"
    );
}

#[test]
fn objective_c_method_edit_stales_unchanged_comment() {
    let before = concat!(
        "@implementation Renderer\n",
        "- (NSInteger)render {\n",
        "    // Coupled to the layout pass.\n",
        "    return 1;\n",
        "}\n",
        "@end\n",
    );
    let after = before.replace("return 1", "return 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Renderer.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Renderer.m"),
            text: &after,
        },
    )
    .expect("valid Objective-C change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 3
        }),
        "method code changes must attest each surviving comment: {findings:#?}"
    );
}

#[test]
fn objective_c_class_extension_and_implementation_pair_unambiguously() {
    let before = concat!(
        "@interface Renderer ()\n",
        "- (NSInteger)render;\n",
        "@end\n",
        "\n",
        "@implementation Renderer\n",
        "- (NSInteger)render {\n",
        "    // Coupled to the layout pass.\n",
        "    return 1;\n",
        "}\n",
        "@end\n",
    );
    let after = concat!(
        "@implementation Renderer\n",
        "- (NSInteger)render {\n",
        "    // Coupled to the layout pass.\n",
        "    return 2;\n",
        "}\n",
        "@end\n",
        "\n",
        "@interface Renderer ()\n",
        "- (NSInteger)render;\n",
        "@end\n",
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Renderer.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Renderer.m"),
            text: after,
        },
    )
    .expect("a class extension and implementation have distinct identities");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 3
        }),
        "the implementation method should pair without type ambiguity: {findings:#?}"
    );
}

#[test]
fn objective_c_block_binding_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the transform contract.\n",
        "NSInteger (^transform)(NSInteger) = ^(NSInteger value) {\n",
        "    return value + 1;\n",
        "};\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.m"),
            text: &after,
        },
    )
    .expect("valid Objective-C block binding change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a direct block binding owns its leading comment and callable body: {findings:#?}"
    );
}

#[test]
fn objective_c_wrapped_block_binding_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the wrapped transform.\n",
        "NSInteger (^transform)(NSInteger) = wrap(^(NSInteger value) {\n",
        "    return value + 1;\n",
        "});\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.m"),
            text: &after,
        },
    )
    .expect("valid wrapped Objective-C block change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a wrapped block binding's leading comment must track its body: {findings:#?}"
    );
}

#[test]
fn objective_c_block_assignment_edit_stales_its_leading_comment() {
    let before = concat!(
        "void installHandler(void) {\n",
        "    // Coupled to the installed handler.\n",
        "    handler = ^{ oldCall(); };\n",
        "}\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.m"),
            text: &after,
        },
    )
    .expect("valid Objective-C block assignment change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "a callable assignment must own its leading comment and block body: {findings:#?}"
    );
}

#[test]
fn objective_c_chained_block_assignment_stales_its_leading_comment() {
    let before = concat!(
        "void installHandler(void) {\n",
        "    root = first = second = ^{\n",
        "        // Coupled to the innermost installed handler.\n",
        "        oldCall();\n",
        "    };\n",
        "}\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.m"),
            text: &after,
        },
    )
    .expect("valid chained Objective-C assignment change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 3
                && finding.message.contains("leaf `second`")
        }),
        "the innermost assignment must retain the block body and identity: {findings:#?}"
    );
}

#[test]
fn objective_c_block_assignment_keeps_nested_block_owner_independent() {
    let before = concat!(
        "void installHandler(void) {\n",
        "    handler = ^{\n",
        "        run(^{\n",
        "            // Coupled only to the nested call.\n",
        "            oldNestedCall();\n",
        "        });\n",
        "        oldOuterCall();\n",
        "    };\n",
        "}\n",
    );
    let after = before.replace("oldOuterCall", "newOuterCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.m"),
            text: &after,
        },
    )
    .expect("valid nested Objective-C assignment change");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed" || finding.line != 4
        }),
        "an assignment owner must not swallow a nested block owner: {findings:#?}"
    );
}

#[test]
fn objective_c_block_assignments_pair_by_canonical_target_across_reordering() {
    let before = concat!(
        "void installHandlers(void) {\n",
        "    // Same rationale.\n",
        "    first = ^{ firstCall(); };\n",
        "    // Same rationale.\n",
        "    second = ^{ secondCall(); };\n",
        "}\n",
    );
    let after = concat!(
        "void installHandlers(void) {\n",
        "    // Same rationale.\n",
        "    second = ^{ secondCall(); };\n",
        "    // Same rationale.\n",
        "    first = ^{ changedFirstCall(); };\n",
        "}\n",
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handlers.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handlers.m"),
            text: after,
        },
    )
    .expect("valid reordered Objective-C assignments");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(
        stale_lines,
        [4],
        "only the changed assignment should stale its comment"
    );
}

#[test]
fn objective_c_nested_block_keeps_an_independent_stale_owner() {
    let before = concat!(
        "NSInteger (^transform)(NSInteger) = ^(NSInteger value) {\n",
        "    NSInteger (^nested)(NSInteger) = ^(NSInteger inner) {\n",
        "        // Coupled only to the nested transformation.\n",
        "        return inner + 1;\n",
        "    };\n",
        "    return value + 1;\n",
        "};\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.m"),
            text: &after,
        },
    )
    .expect("valid nested Objective-C block change");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed" || finding.line != 3
        }),
        "an outer binding must not swallow its nested callable owner: {findings:#?}"
    );
}

#[test]
fn clang_format_markers_do_not_consume_narrative_budget() {
    for (off, on) in [
        ("// clang-format off", "// clang-format on"),
        ("/* clang-format off */", "/* clang-format on */"),
        (
            "// clang-format off: generated table",
            "// clang-format on: generated table ends",
        ),
    ] {
        let source = format!(
            "@implementation Formatter\n- (NSInteger)work {{\n    {off}\n    NSInteger value    =    1;\n    {on}\n    // ordinary explanation\n    return value;\n}}\n@end\n"
        );

        let findings = analyze_all(SourceFile {
            path: Path::new("Formatter.m"),
            text: &source,
        })
        .expect("valid Objective-C should parse");

        assert!(
            findings.is_empty(),
            "official clang-format markers stay outside the narrative ratio ({off}): {findings:#?}"
        );
    }
}

#[test]
fn inexact_clang_format_prefixes_remain_narrative() {
    for candidate in [
        "// clang-format off because prose",
        "/// clang-format off",
        "// clang-format off:",
        "/* clang-format off: generated table */",
        "// clang-format OFF",
        "NSInteger inlineValue = 0; // clang-format off",
        "/* clang-format off */ NSInteger inlineValue = 0;",
    ] {
        let source = format!(
            "@implementation Formatter\n- (NSInteger)work {{\n    {candidate}\n    NSInteger value = 1;\n    // ordinary explanation\n    return value;\n}}\n@end\n"
        );

        let findings = analyze_all(SourceFile {
            path: Path::new("Formatter.m"),
            text: &source,
        })
        .expect("valid Objective-C should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "an inexact marker cannot launder narrative ({candidate}): {findings:#?}"
        );
    }
}

#[test]
fn clang_format_markers_cannot_bypass_the_absolute_owner_cap() {
    let source = concat!(
        "@implementation Formatter\n",
        "- (NSInteger)work {\n",
        "    // clang-format off\n",
        "    // clang-format on\n",
        "    // clang-format off\n",
        "    // clang-format on\n",
        "    // clang-format off\n",
        "    // clang-format on\n",
        "    // clang-format off\n",
        "    // clang-format on\n",
        "    // clang-format off\n",
        "    return 1;\n",
        "}\n",
        "@end\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Formatter.m"),
        text: source,
    })
    .expect("valid Objective-C should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "tool metadata cannot grant an unlimited comment allowance: {findings:#?}"
    );
}

#[test]
fn clang_format_marker_remains_stale_when_owner_code_changes() {
    let before = concat!(
        "@implementation Formatter\n",
        "- (NSInteger)work {\n",
        "    // clang-format off\n",
        "    return 1;\n",
        "}\n",
        "@end\n",
    );
    let after = before.replace("return 1", "return 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Formatter.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Formatter.m"),
            text: &after,
        },
    )
    .expect("valid Objective-C change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 3
        }),
        "operational metadata does not waive stale-comment review: {findings:#?}"
    );
}

#[test]
fn objective_c_grouped_block_binding_owns_every_direct_block_body() {
    let before = concat!(
        "// Coupled to both transforms.\n",
        "NSInteger (^first)(NSInteger) = ^(NSInteger value) {\n",
        "    // Shared by the grouped binding.\n",
        "    return value + 1;\n",
        "}, (^second)(NSInteger) = ^(NSInteger value) {\n",
        "    return value + 2;\n",
        "};\n",
    );
    let after = before.replace("value + 2", "value + 3");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transforms.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transforms.m"),
            text: &after,
        },
    )
    .expect("valid grouped Objective-C block change");

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
fn objective_c_methods_pair_by_kind_and_full_selector_across_reordering() {
    let before = concat!(
        "@implementation Renderer\n",
        "+ (NSInteger)render:(NSInteger)value scale:(NSInteger)scale {\n",
        "    // Coupled to class rendering.\n",
        "    return value * scale;\n",
        "}\n",
        "- (NSInteger)render:(NSInteger)value offset:(NSInteger)offset {\n",
        "    // Coupled to offset rendering.\n",
        "    return value + offset;\n",
        "}\n",
        "- (NSInteger)render:(NSInteger)value scale:(NSInteger)scale {\n",
        "    // Coupled to instance rendering.\n",
        "    return value + scale;\n",
        "}\n",
        "@end\n",
    );
    let after = concat!(
        "@implementation Renderer\n",
        "- (NSInteger)render:(NSInteger)value scale:(NSInteger)scale {\n",
        "    // Coupled to instance rendering.\n",
        "    return value + scale;\n",
        "}\n",
        "- (NSInteger)render:(NSInteger)value offset:(NSInteger)offset {\n",
        "    // Coupled to offset rendering.\n",
        "    return value + offset;\n",
        "}\n",
        "+ (NSInteger)render:(NSInteger)value scale:(NSInteger)scale {\n",
        "    // Coupled to class rendering.\n",
        "    return value * scale + 1;\n",
        "}\n",
        "@end\n",
    );

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Renderer.m"),
            text: before,
        },
        SourceFile {
            path: Path::new("Renderer.m"),
            text: after,
        },
    )
    .expect("Objective-C method kind and selector disambiguate reordered methods");

    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();
    assert_eq!(
        stale_lines,
        [11],
        "only the changed class method should stale its comment"
    );
}

#[test]
fn deeply_wrapped_objective_c_block_analyzes_through_the_public_seam() {
    let depth = 64;
    let mut source = String::from("id handler = ");
    source.push_str(&"wrap(".repeat(depth));
    source.push_str("^{ return; }");
    source.push_str(&")".repeat(depth));
    source.push_str(";\n");

    let findings = analyze_all(SourceFile {
        path: Path::new("Deep.m"),
        text: &source,
    })
    .expect("deep Objective-C should parse without reverse parent probes");

    assert!(findings.is_empty());
}
