use std::path::Path;

use fuck_ai_comments::{SourceFile, analyze_all, analyze_change, supports_path};

#[test]
fn kotlin_source_and_script_extensions_are_registered() {
    assert!(supports_path(Path::new("Renderer.kt")));
    assert!(supports_path(Path::new("build.gradle.kts")));
}

#[test]
fn kotlin_short_function_rejects_four_comment_lines() {
    let source = concat!(
        "fun render(): Int {\n",
        "    // one\n",
        "    // two\n",
        "    // three\n",
        "    // four\n",
        "    return 1\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Renderer.kt"),
        text: source,
    })
    .expect("valid Kotlin should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "Kotlin functions must use the shared function budget: {findings:#?}"
    );
}

#[test]
fn kotlin_secondary_constructor_uses_its_own_function_budget() {
    let source = concat!(
        "class Large {\n",
        "    var a01 = 1\n",
        "    var a02 = 2\n",
        "    var a03 = 3\n",
        "    var a04 = 4\n",
        "    var a05 = 5\n",
        "    var a06 = 6\n",
        "    var a07 = 7\n",
        "    var a08 = 8\n",
        "    var a09 = 9\n",
        "    var a10 = 10\n",
        "    var a11 = 11\n",
        "    var a12 = 12\n",
        "    constructor(value: Int) {\n",
        "        // first rationale\n",
        "        consume(value)\n",
        "        // second rationale\n",
        "        finish()\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Large.kt"),
        text: source,
    })
    .expect("valid Kotlin secondary constructor should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a secondary constructor must not borrow its class budget: {findings:#?}"
    );
}

#[test]
fn kotlin_body_property_uses_its_own_function_budget() {
    let source = concat!(
        "class Large {\n",
        "    var a01 = 1\n",
        "    var a02 = 2\n",
        "    var a03 = 3\n",
        "    var a04 = 4\n",
        "    var a05 = 5\n",
        "    var a06 = 6\n",
        "    var a07 = 7\n",
        "    var a08 = 8\n",
        "    var a09 = 9\n",
        "    var a10 = 10\n",
        "    var a11 = 11\n",
        "    var a12 = 12\n",
        "    var computed: Int = 0\n",
        "        get() {\n",
        "            // first rationale\n",
        "            prepare()\n",
        "            // second rationale\n",
        "            return field\n",
        "        }\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Large.kt"),
        text: source,
    })
    .expect("valid Kotlin body property should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a body property must not borrow its class budget: {findings:#?}"
    );
}

#[test]
fn kotlin_initializer_uses_its_own_function_budget() {
    let source = concat!(
        "class Large {\n",
        "    var a01 = 1\n",
        "    var a02 = 2\n",
        "    var a03 = 3\n",
        "    var a04 = 4\n",
        "    var a05 = 5\n",
        "    var a06 = 6\n",
        "    var a07 = 7\n",
        "    var a08 = 8\n",
        "    var a09 = 9\n",
        "    var a10 = 10\n",
        "    var a11 = 11\n",
        "    var a12 = 12\n",
        "    init {\n",
        "        // first rationale\n",
        "        prepare()\n",
        "        // second rationale\n",
        "        finish()\n",
        "    }\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Large.kt"),
        text: source,
    })
    .expect("valid Kotlin initializer should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "an initializer must not borrow its class budget: {findings:#?}"
    );
}

#[test]
fn kotlin_val_rejects_four_leading_comment_lines() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "val retryLimit = 3\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Constants.kt"),
        text: source,
    })
    .expect("valid Kotlin should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a Kotlin val binding cannot launder a long explanation: {findings:#?}"
    );
}

#[test]
fn kotlin_function_edit_stales_unchanged_comment() {
    let before = concat!(
        "fun render(): Int {\n",
        "    // Coupled to the layout pass.\n",
        "    return 1\n",
        "}\n",
    );
    let after = before.replace("return 1", "return 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Renderer.kt"),
            text: before,
        },
        SourceFile {
            path: Path::new("Renderer.kt"),
            text: &after,
        },
    )
    .expect("valid Kotlin change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "Kotlin code changes must attest each surviving comment: {findings:#?}"
    );
}

#[test]
fn kotlin_overload_signatures_disambiguate_reordered_functions() {
    let before = SourceFile {
        path: Path::new("Renderers.kt"),
        text: concat!(
            "fun render(value: Int): Int {\n",
            "    // Same rationale.\n",
            "    return value\n",
            "}\n",
            "\n",
            "fun render(value: String): String {\n",
            "    // Same rationale.\n",
            "    return value\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("Renderers.kt"),
        text: concat!(
            "fun render(value: String): String {\n",
            "    // Same rationale.\n",
            "    return value\n",
            "}\n",
            "\n",
            "fun render(value: Int): Int {\n",
            "    // Same rationale.\n",
            "    return value\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Kotlin overload reorder");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "parameter types must qualify an overload identity: {findings:#?}"
    );
}

#[test]
fn kotlin_nested_type_namespaces_disambiguate_reordered_methods() {
    let before = SourceFile {
        path: Path::new("Workers.kt"),
        text: concat!(
            "class Alpha {\n",
            "    class Worker {\n",
            "        fun run() {\n",
            "            // Same rationale.\n",
            "            alphaWork()\n",
            "        }\n",
            "    }\n",
            "}\n",
            "class Beta {\n",
            "    class Worker {\n",
            "        fun run() {\n",
            "            // Same rationale.\n",
            "            betaWork()\n",
            "        }\n",
            "    }\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("Workers.kt"),
        text: concat!(
            "class Beta {\n",
            "    class Worker {\n",
            "        fun run() {\n",
            "            // Same rationale.\n",
            "            betaWork()\n",
            "        }\n",
            "    }\n",
            "}\n",
            "class Alpha {\n",
            "    class Worker {\n",
            "        fun run() {\n",
            "            // Same rationale.\n",
            "            alphaWork()\n",
            "        }\n",
            "    }\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Kotlin reorder");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "the complete nested type path must qualify a method identity: {findings:#?}"
    );
}

#[test]
fn kotlin_enum_entry_namespaces_disambiguate_reordered_methods() {
    let before = SourceFile {
        path: Path::new("Mode.kt"),
        text: concat!(
            "enum class Mode {\n",
            "    A {\n",
            "        override fun run() {\n",
            "            // Same rationale.\n",
            "            alphaWork()\n",
            "        }\n",
            "    },\n",
            "    B {\n",
            "        override fun run() {\n",
            "            // Same rationale.\n",
            "            betaWork()\n",
            "        }\n",
            "    };\n",
            "    abstract fun run()\n",
            "}\n",
        ),
    };
    let after = SourceFile {
        path: Path::new("Mode.kt"),
        text: concat!(
            "enum class Mode {\n",
            "    B {\n",
            "        override fun run() {\n",
            "            // Same rationale.\n",
            "            betaWork()\n",
            "        }\n",
            "    },\n",
            "    A {\n",
            "        override fun run() {\n",
            "            // Same rationale.\n",
            "            alphaWork()\n",
            "        }\n",
            "    };\n",
            "    abstract fun run()\n",
            "}\n",
        ),
    };

    let findings = analyze_change(before, after).expect("valid Kotlin enum reorder");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-reparented"
                && finding.rule != "comment-policy/comment-owner-changed"
        }),
        "an enum entry must qualify its override identity: {findings:#?}"
    );
}

#[test]
fn kotlin_lambda_binding_uses_the_leaf_comment_budget() {
    let source = concat!(
        "val transform = { value: Int ->\n",
        "    // one\n",
        "    // two\n",
        "    // three\n",
        "    // four\n",
        "    value + 1\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Transform.kt"),
        text: source,
    })
    .expect("valid Kotlin lambda should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a callback cannot upgrade a val binding to the function allowance: {findings:#?}"
    );
}

#[test]
fn kotlin_callable_container_keeps_the_leaf_comment_budget() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "val handlers = listOf({ value: Int -> value + 1 })\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Handlers.kt"),
        text: source,
    })
    .expect("valid Kotlin should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a nested lambda cannot raise a val binding's comment allowance: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/function-comment-budget"),
        "the semantic owner is the val binding, not its contained lambda: {findings:#?}"
    );
}

#[test]
fn kotlin_direct_lambda_val_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the transformation body.\n",
        "val transform = { value: Int -> value + 1 }\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.kt"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.kt"),
            text: &after,
        },
    )
    .expect("valid Kotlin lambda change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a direct lambda property's leading comment must track its body: {findings:#?}"
    );
}

#[test]
fn kotlin_wrapped_lambda_val_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the remembered transformation.\n",
        "val transform = remember(Unit) { value: Int -> value + 1 }\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.kt"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.kt"),
            text: &after,
        },
    )
    .expect("valid wrapped Kotlin lambda change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a wrapped lambda property's leading comment must track its body: {findings:#?}"
    );
}

#[test]
fn kotlin_delegated_lambda_val_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the lazy handler.\n",
        "val handler by lazy { oldCall() }\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.kt"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.kt"),
            text: &after,
        },
    )
    .expect("valid delegated Kotlin lambda change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a delegated lambda property's leading comment must track its body: {findings:#?}"
    );
}

#[test]
fn kotlin_anonymous_function_val_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the anonymous function.\n",
        "val handler = fun(value: Int): Int { return value + 1 }\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.kt"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.kt"),
            text: &after,
        },
    )
    .expect("valid Kotlin anonymous function change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "an anonymous function binding's leading comment must track its body: {findings:#?}"
    );
}

#[test]
fn kotlin_lambda_assignment_edit_stales_its_leading_comment() {
    let before = concat!(
        "// Coupled to the installed handler.\n",
        "handler = { oldCall() }\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.kts"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.kts"),
            text: &after,
        },
    )
    .expect("valid Kotlin script assignment change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a callable assignment must own its leading comment and lambda body: {findings:#?}"
    );
}

#[test]
fn kotlin_nested_branch_assignments_preserve_lambda_ownership() {
    let before = concat!(
        "root = if (enabled) first = if (enabled) second = {\n",
        "    // Coupled to the innermost installed handler.\n",
        "    oldCall()\n",
        "} else fallback else fallback\n",
    );
    let after = before.replace("oldCall", "newCall");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Handler.kts"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.kts"),
            text: &after,
        },
    )
    .expect("valid nested Kotlin branch-assignment change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 2
                && finding.message.contains("leaf `second`")
        }),
        "the innermost assignment must retain the lambda body and identity: {findings:#?}"
    );
}

#[test]
fn kotlin_lambda_assignment_keeps_nested_lambda_owner_independent() {
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
            path: Path::new("Handler.kts"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handler.kts"),
            text: &after,
        },
    )
    .expect("valid nested Kotlin assignment change");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed" || finding.line != 3
        }),
        "an assignment owner must not swallow a nested lambda owner: {findings:#?}"
    );
}

#[test]
fn kotlin_lambda_assignments_pair_by_canonical_target_across_reordering() {
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
            path: Path::new("Handlers.kts"),
            text: before,
        },
        SourceFile {
            path: Path::new("Handlers.kts"),
            text: after,
        },
    )
    .expect("valid reordered Kotlin assignments");

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
fn kotlin_nested_lambda_keeps_an_independent_stale_owner() {
    let before = concat!(
        "val transform = { value: Int ->\n",
        "    val nested = { inner: Int ->\n",
        "        // Coupled only to the nested transformation.\n",
        "        inner + 1\n",
        "    }\n",
        "    value + 1\n",
        "}\n",
    );
    let after = before.replace("value + 1", "value + 2");

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Transform.kt"),
            text: before,
        },
        SourceFile {
            path: Path::new("Transform.kt"),
            text: &after,
        },
    )
    .expect("valid nested Kotlin lambda change");

    assert!(
        findings.iter().all(|finding| {
            finding.rule != "comment-policy/comment-owner-changed" || finding.line != 3
        }),
        "an outer binding must not swallow its nested callable owner: {findings:#?}"
    );
}

#[test]
fn kotlin_block_comment_counts_as_narrative() {
    let source = concat!(
        "fun render(): Int {\n",
        "    /*\n",
        "     * one\n",
        "     * two\n",
        "     * three\n",
        "     * four\n",
        "     */\n",
        "    return 1\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Renderer.kt"),
        text: source,
    })
    .expect("valid Kotlin block comment should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "Kotlin block comments must consume their function budget: {findings:#?}"
    );
}

#[test]
fn kotlin_destructured_val_is_not_a_leaf_owner() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "val (left, right) = pair()\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Pair.kt"),
        text: source,
    })
    .expect("valid Kotlin destructuring should parse");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/leaf-comment-budget"),
        "a destructuring declaration has no single stable leaf identity: {findings:#?}"
    );
}

#[test]
fn kotlin_var_is_not_a_leaf_owner() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "var retryLimit = 3\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("State.kt"),
        text: source,
    })
    .expect("valid mutable Kotlin property should parse");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/leaf-comment-budget"),
        "a mutable property must remain under its enclosing owner: {findings:#?}"
    );
}

#[test]
fn kotlin_enum_entry_is_a_real_type_budget_owner() {
    let source = concat!(
        "enum class Mode {\n",
        "    ALPHA {\n",
        "        // one\n",
        "        // two\n",
        "        // three\n",
        "        // four\n",
        "\n",
        "        override fun run() = 1\n",
        "    };\n",
        "    abstract fun run(): Int\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Mode.kt"),
        text: source,
    })
    .expect("valid Kotlin enum should parse");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/type-comment-budget"
                && finding.message.contains("type `ALPHA`")
        }),
        "an enum entry is the real parent and budget owner of its body: {findings:#?}"
    );
}

#[test]
fn deeply_nested_kotlin_types_and_wrappers_analyze_through_the_public_seam() {
    let depth = 64;
    let mut source = String::new();
    for index in 0..depth {
        source.push_str(&format!("class C{index} {{\n"));
    }
    source.push_str("val handler = ");
    source.push_str(&"wrap(".repeat(depth));
    source.push_str("{ 1 }");
    source.push_str(&")".repeat(depth));
    source.push('\n');
    source.push_str(&"}\n".repeat(depth));

    let findings = analyze_all(SourceFile {
        path: Path::new("Deep.kt"),
        text: &source,
    })
    .expect("deep Kotlin should parse without ancestor rescans");

    assert!(findings.is_empty());
}
