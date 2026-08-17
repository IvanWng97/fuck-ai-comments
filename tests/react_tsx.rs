use std::path::Path;

use fuck_ai_comments::{SourceFile, analyze_all, analyze_change};

fn changed(before: &str, after: &str) -> Vec<fuck_ai_comments::Finding> {
    changed_at(Path::new("Card.tsx"), before, after)
}

fn changed_at(path: &Path, before: &str, after: &str) -> Vec<fuck_ai_comments::Finding> {
    analyze_change(
        SourceFile { path, text: before },
        SourceFile { path, text: after },
    )
    .expect("valid TSX change")
}

fn assert_hidden_callable_stales(before: &str, after: &str) {
    let findings = changed(before, after);

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("leaf `Card`")
        }),
        "a callable anywhere in the initializer must make the binding its owner: {findings:#?}"
    );
}

#[test]
fn module_exports_callable_body_change_stales_leading_comment() {
    let findings = changed_at(
        Path::new("module.js"),
        "// why\nmodule.exports = () => oldCall();\n",
        "// why\nmodule.exports = () => newCall();\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a callable assignment must own its leading comment: {findings:#?}"
    );
}

#[test]
fn exported_handler_assignment_body_change_stales_leading_comment() {
    let findings = changed_at(
        Path::new("handler.ts"),
        "// why\nexports.handler = async () => oldCall();\n",
        "// why\nexports.handler = async () => newCall();\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "an async callable assignment must own its leading comment: {findings:#?}"
    );
}

#[test]
fn plain_callable_assignment_body_change_stales_leading_comment() {
    let findings = changed(
        "// why\nhandler = () => oldCall();\n",
        "// why\nhandler = () => newCall();\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a plain callable assignment must own its leading comment: {findings:#?}"
    );
}

#[test]
fn wrapped_callable_assignment_body_change_stales_leading_comment() {
    let findings = changed_at(
        Path::new("handler.ts"),
        "// why\nhandler = wrap(() => oldCall());\n",
        "// why\nhandler = wrap(() => newCall());\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "a wrapper cannot hide a callable assignment from stale detection: {findings:#?}"
    );
}

#[test]
fn nested_assignment_callback_keeps_an_independent_owner() {
    let findings = changed_at(
        Path::new("handler.ts"),
        concat!(
            "oldHandler = () => items.map((item) => {\n",
            "  // Coupled only to this item callback.\n",
            "  return render(item);\n",
            "});\n",
        ),
        concat!(
            "newHandler = () => items.map((item) => {\n",
            "  // Coupled only to this item callback.\n",
            "  return render(item);\n",
            "});\n",
        ),
    );

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "an intervening callable boundary must keep nested callbacks independent: {findings:#?}"
    );
}

#[test]
fn javascript_prettier_ignore_directives_stay_out_of_narrative_budgets() {
    let source = concat!(
        "function render() {\n",
        "  // prettier-ignore\n",
        "  oldCall();\n",
        "  // prettier-ignore\n",
        "  newCall();\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("render.js"),
        text: source,
    })
    .expect("valid JavaScript should parse");

    assert!(
        findings.is_empty(),
        "attached Prettier directives are tool syntax, not narrative: {findings:#?}"
    );
}

#[test]
fn typescript_prettier_ignore_directives_stay_out_of_narrative_budgets() {
    let source = concat!(
        "function render(value: string) {\n",
        "  // prettier-ignore\n",
        "  oldCall(value);\n",
        "  // prettier-ignore\n",
        "  newCall(value);\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("render.ts"),
        text: source,
    })
    .expect("valid TypeScript should parse");

    assert!(
        findings.is_empty(),
        "TypeScript must share the strict Prettier directive contract: {findings:#?}"
    );
}

#[test]
fn jsx_prettier_ignore_directives_stay_out_of_narrative_budgets() {
    let source = concat!(
        "const Card = () => (\n",
        "  <main>\n",
        "    {/* prettier-ignore */}\n",
        "    <span     data-id='one' />\n",
        "    {/* prettier-ignore */}\n",
        "    <span     data-id='two' />\n",
        "    {/* prettier-ignore */}\n",
        "    <span     data-id='three' />\n",
        "  </main>\n",
        ");\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Card.tsx"),
        text: source,
    })
    .expect("valid TSX should parse");

    assert!(
        findings.is_empty(),
        "attached JSX Prettier directives are tool syntax: {findings:#?}"
    );
}

#[test]
fn prettier_ignore_with_extra_prose_remains_narrative() {
    let source = concat!(
        "function render() {\n",
        "  // prettier-ignore because generated\n",
        "  oldCall();\n",
        "  // prettier-ignore because generated\n",
        "  newCall();\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("render.js"),
        text: source,
    })
    .expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "Prettier syntax must be exact, never prefix-matched: {findings:#?}"
    );
}

#[test]
fn prettier_ignore_targets_the_next_node_across_blank_lines() {
    let source = concat!(
        "function render() {\n",
        "  // prettier-ignore\n",
        "\n",
        "  oldCall();\n",
        "  /* prettier-ignore */\n",
        "\n",
        "  newCall();\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("render.ts"),
        text: source,
    })
    .expect("valid TypeScript should parse");

    assert!(
        findings.is_empty(),
        "Prettier targets the next AST node even across blank lines: {findings:#?}"
    );
}

#[test]
fn jsx_prettier_ignore_with_extra_prose_remains_narrative() {
    let source = concat!(
        "const Card = () => (\n",
        "  <main>\n",
        "    {/* prettier-ignore because generated */}\n",
        "    <span data-id='one' />\n",
        "    {/* prettier-ignore because generated */}\n",
        "    <span data-id='two' />\n",
        "    {/* prettier-ignore because generated */}\n",
        "    <span data-id='three' />\n",
        "    {/* prettier-ignore because generated */}\n",
        "    <span data-id='four' />\n",
        "  </main>\n",
        ");\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Card.tsx"),
        text: source,
    })
    .expect("valid TSX should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "JSX Prettier syntax must be exact, never prefix-matched: {findings:#?}"
    );
}

#[test]
fn prettier_ignore_directives_still_count_toward_the_absolute_cap() {
    let source = concat!(
        "function render() {\n",
        "  // prettier-ignore\n  call1();\n",
        "  // prettier-ignore\n  call2();\n",
        "  // prettier-ignore\n  call3();\n",
        "  // prettier-ignore\n  call4();\n",
        "  // prettier-ignore\n  call5();\n",
        "  // prettier-ignore\n  call6();\n",
        "  // prettier-ignore\n  call7();\n",
        "  // prettier-ignore\n  call8();\n",
        "  // prettier-ignore\n  call9();\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("render.js"),
        text: source,
    })
    .expect("valid JavaScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "tool syntax must not bypass the absolute owner cap: {findings:#?}"
    );
}

#[test]
fn unchanged_prettier_ignore_stales_when_its_owner_changes() {
    let findings = changed_at(
        Path::new("render.ts"),
        "function render() {\n  // prettier-ignore\n  oldCall();\n}\n",
        "function render() {\n  // prettier-ignore\n  newCall();\n}\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "tool syntax still requires stale-comment attestation: {findings:#?}"
    );
}

#[test]
fn const_component_body_change_stales_leading_comment() {
    let findings = changed(
        "// Coupled to the card contract.\nconst Card = () => <Old />;\n",
        "// Coupled to the card contract.\nconst Card = () => <New />;\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("leaf `Card`")
        }),
        "the binding and callable body must be one semantic owner: {findings:#?}"
    );
}

#[test]
fn callable_const_keeps_the_leaf_comment_budget() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "const handlers = [() => render()];\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("handlers.tsx"),
        text: source,
    })
    .expect("valid TSX should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a callback cannot upgrade a const binding to the function allowance: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/function-comment-budget"),
        "the semantic owner is the binding, not its contained callback: {findings:#?}"
    );
}

#[test]
fn exported_and_plain_scalar_consts_keep_the_leaf_budget_in_large_files() {
    for declaration in ["export const VALUE = 1;", "const VALUE = 1;"] {
        let mut source = String::from("// one\n// two\n// three\n// four\n");
        source.push_str(declaration);
        source.push('\n');
        for index in 0..200 {
            source.push_str(&format!("const unrelated{index} = {index};\n"));
        }
        let findings = analyze_all(SourceFile {
            path: Path::new("constants.ts"),
            text: &source,
        })
        .expect("valid TypeScript should parse");
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
            "the scalar declaration owns its leading comments ({declaration}): {findings:#?}"
        );
    }
}

#[test]
fn exported_scalar_const_change_stales_its_leading_comment() {
    let before = "// Coupled to the wire value.\nexport const VALUE = 1;\n";
    let after = "// Coupled to the wire value.\nexport const VALUE = 2;\n";
    let findings = changed_at(Path::new("constant.ts"), before, after);

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "an exported scalar uses one leaf span including its export prefix: {findings:#?}"
    );
}

#[test]
fn ambient_scalar_const_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("constant.ts"),
        "// Coupled to the ambient wire value.\ndeclare const VALUE: 1;\n",
        "// Coupled to the ambient wire value.\ndeclare const VALUE: 2;\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("leaf `VALUE`")
        }),
        "an ambient scalar declaration must be one leaf owner: {findings:#?}"
    );
}

#[test]
fn exported_ambient_scalar_const_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("constant.ts"),
        "// Coupled to the exported ambient wire value.\nexport declare const VALUE: 1;\n",
        "// Coupled to the exported ambient wire value.\nexport declare const VALUE: 2;\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("leaf `VALUE`")
        }),
        "an exported ambient scalar must include its export prefix: {findings:#?}"
    );
}

#[test]
fn ambient_scalar_consts_keep_the_leaf_comment_budget() {
    for declaration in ["declare const VALUE: 1;", "export declare const VALUE: 1;"] {
        let source = format!("// one\n// two\n// three\n// four\n{declaration}\n");
        let findings = analyze_all(SourceFile {
            path: Path::new("constant.ts"),
            text: &source,
        })
        .expect("valid ambient TypeScript should parse");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
            "an ambient const must keep the leaf allowance ({declaration}): {findings:#?}"
        );
    }
}

#[test]
fn ambient_class_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("ambient.ts"),
        "// Coupled to the ambient class shape.\ndeclare class Card { old: string }\n",
        "// Coupled to the ambient class shape.\ndeclare class Card { next: string }\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("type `Card`")
        }),
        "an ambient class must be one type owner: {findings:#?}"
    );
}

#[test]
fn exported_ambient_class_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("ambient.ts"),
        "// Coupled to the exported ambient class shape.\nexport declare class Card { old: string }\n",
        "// Coupled to the exported ambient class shape.\nexport declare class Card { next: string }\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("type `Card`")
        }),
        "an exported ambient class must include its export prefix: {findings:#?}"
    );
}

#[test]
fn ambient_enum_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("ambient.ts"),
        "// Coupled to the ambient enum shape.\ndeclare enum Mode { Old = 1 }\n",
        "// Coupled to the ambient enum shape.\ndeclare enum Mode { Next = 2 }\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("type `Mode`")
        }),
        "an ambient enum must include its declare prefix: {findings:#?}"
    );
}

#[test]
fn ambient_abstract_class_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("ambient.ts"),
        "// Coupled to the abstract ambient shape.\ndeclare abstract class Card { old: string }\n",
        "// Coupled to the abstract ambient shape.\ndeclare abstract class Card { next: string }\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("type `Card`")
        }),
        "an abstract ambient class must include its declare prefix: {findings:#?}"
    );
}

#[test]
fn ambient_module_changes_stale_their_leading_comments() {
    for (before, after) in [
        (
            concat!(
                "// Coupled to the ambient namespace.\n",
                "declare namespace Old {\n",
                "  export interface Stable {}\n",
                "}\n",
            ),
            concat!(
                "// Coupled to the ambient namespace.\n",
                "declare namespace Next {\n",
                "  export interface Stable {}\n",
                "}\n",
            ),
        ),
        (
            concat!(
                "// Coupled to the external module.\n",
                "declare module \"old\" {\n",
                "  export interface Stable {}\n",
                "}\n",
            ),
            concat!(
                "// Coupled to the external module.\n",
                "declare module \"next\" {\n",
                "  export interface Stable {}\n",
                "}\n",
            ),
        ),
    ] {
        let findings = changed_at(Path::new("ambient.ts"), before, after);

        assert!(
            findings.iter().any(|finding| {
                finding.rule == "comment-policy/comment-owner-changed"
                    && finding.line == 1
                    && finding.message.contains("type `")
            }),
            "an ambient module must include its declare prefix ({after:?}): {findings:#?}"
        );
    }
}

#[test]
fn ambient_function_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("ambient.ts"),
        "// Coupled to the ambient function contract.\ndeclare function load(): 1;\n",
        "// Coupled to the ambient function contract.\ndeclare function load(): 2;\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("function `load`")
        }),
        "an ambient function must be one callable owner: {findings:#?}"
    );
}

#[test]
fn exported_ambient_function_change_stales_its_leading_comment() {
    let findings = changed_at(
        Path::new("ambient.ts"),
        "// Coupled to the exported ambient function contract.\nexport declare function load(): 1;\n",
        "// Coupled to the exported ambient function contract.\nexport declare function load(): 2;\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("function `load`")
        }),
        "an exported ambient function must include its export prefix: {findings:#?}"
    );
}

#[test]
fn declare_global_is_not_an_ambient_leaf() {
    let source = concat!(
        "// one\n",
        "// two\n",
        "// three\n",
        "// four\n",
        "declare global { interface Window { bridge: string } }\n",
    );
    let findings = analyze_all(SourceFile {
        path: Path::new("ambient.ts"),
        text: source,
    })
    .expect("valid global augmentation should parse");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/leaf-comment-budget"),
        "a global augmentation must keep module, type, or file scope: {findings:#?}"
    );
}

#[test]
fn exported_function_body_change_stales_leading_comment() {
    let findings = changed(
        "// Coupled to the card contract.\nexport function Card() { return <Old />; }\n",
        "// Coupled to the card contract.\nexport function Card() { return <New />; }\n",
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "export syntax must not detach a function's leading comment: {findings:#?}"
    );
}

#[test]
fn wrapped_component_body_change_stales_leading_comment() {
    let findings = changed(
        concat!(
            "// Coupled to the forwarded ref contract.\n",
            "export const Card = memo(forwardRef(() => <Old />));\n",
        ),
        concat!(
            "// Coupled to the forwarded ref contract.\n",
            "export const Card = memo(forwardRef(() => <New />));\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "transparent call wrappers must retain the binding owner: {findings:#?}"
    );
}

#[test]
fn typed_expression_wrapper_stales_leading_comment() {
    let findings = changed(
        concat!(
            "// Coupled to the typed component contract.\n",
            "export const Card = (() => <Old />) satisfies React.FC;\n",
        ),
        concat!(
            "// Coupled to the typed component contract.\n",
            "export const Card = (() => <New />) satisfies React.FC;\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "typed expression wrappers must retain the callable binding: {findings:#?}"
    );
}

#[test]
fn wrapper_with_one_callable_and_configuration_stales_leading_comment() {
    let findings = changed(
        concat!(
            "// Coupled to the memoized component contract.\n",
            "export const Card = memo(() => <Old />, sameProps);\n",
        ),
        concat!(
            "// Coupled to the memoized component contract.\n",
            "export const Card = memo(() => <New />, sameProps);\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "non-callable wrapper arguments must not hide the callable binding: {findings:#?}"
    );
}

#[test]
fn conditional_initializer_cannot_hide_a_callable_body_change() {
    assert_hidden_callable_stales(
        "// Coupled to the selected renderer.\nconst Card = enabled ? () => <Old /> : fallback;\n",
        "// Coupled to the selected renderer.\nconst Card = enabled ? () => <New /> : fallback;\n",
    );
}

#[test]
fn logical_initializer_cannot_hide_a_callable_body_change() {
    assert_hidden_callable_stales(
        "// Coupled to the optional renderer.\nconst Card = enabled && (() => <Old />);\n",
        "// Coupled to the optional renderer.\nconst Card = enabled && (() => <New />);\n",
    );
}

#[test]
fn object_initializer_cannot_hide_a_callable_body_change() {
    assert_hidden_callable_stales(
        "// Coupled to the renderer table.\nconst Card = { render: () => <Old /> };\n",
        "// Coupled to the renderer table.\nconst Card = { render: () => <New /> };\n",
    );
}

#[test]
fn array_initializer_cannot_hide_a_callable_body_change() {
    assert_hidden_callable_stales(
        "// Coupled to the renderer sequence.\nconst Card = [() => <Old />];\n",
        "// Coupled to the renderer sequence.\nconst Card = [() => <New />];\n",
    );
}

#[test]
fn multi_declarator_callable_edit_stales_shared_leading_comment() {
    let findings = changed(
        concat!(
            "// Coupled to both exported renderers.\n",
            "export const A = () => <Old />, B = () => <Stable />;\n",
        ),
        concat!(
            "// Coupled to both exported renderers.\n",
            "export const A = () => <New />, B = () => <Stable />;\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("leaf `A, B`")
        }),
        "a callable declaration group must be one named semantic owner: {findings:#?}"
    );
}

#[test]
fn multi_declarator_callables_share_the_grouped_identity() {
    let findings = changed(
        concat!(
            "export const OldA = () => <main />,\n",
            "  B = () => {\n",
            "    // Coupled to the declaration group.\n",
            "    return <aside />;\n",
            "  };\n",
        ),
        concat!(
            "export const NewA = () => <main />,\n",
            "  B = () => {\n",
            "    // Coupled to the declaration group.\n",
            "    return <aside />;\n",
            "  };\n",
        ),
    );

    let finding = findings
        .iter()
        .find(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .expect("the declaration rename must stale its shared comment");
    assert!(
        finding.message.contains("leaf `NewA, B`"),
        "contained callables must use the grouped binding identity: {finding:#?}"
    );
}

#[test]
fn default_exported_wrapped_component_stales_leading_comment() {
    let findings = changed(
        concat!(
            "// Coupled to the default component contract.\n",
            "export default memo(() => <Old />);\n",
        ),
        concat!(
            "// Coupled to the default component contract.\n",
            "export default memo(() => <New />);\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "default-export wrappers must not detach the callable owner: {findings:#?}"
    );
}

#[test]
fn conditional_default_export_is_one_fail_closed_owner() {
    let findings = changed(
        concat!(
            "// Coupled to the selected default renderer.\n",
            "export default enabled ? (() => <Old />) : (() => <Stable />);\n",
        ),
        concat!(
            "// Coupled to the selected default renderer.\n",
            "export default enabled ? (() => <New />) : (() => <Stable />);\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed"
                && finding.line == 1
                && finding.message.contains("function `default`")
        }),
        "a callable-bearing default export must fail closed as one owner: {findings:#?}"
    );
}

#[test]
fn class_field_callable_stales_its_leading_comment() {
    let findings = changed(
        concat!(
            "class Card extends React.Component {\n",
            "  // Coupled to the click transition.\n",
            "  handleClick = () => oldTransition();\n",
            "}\n",
        ),
        concat!(
            "class Card extends React.Component {\n",
            "  // Coupled to the click transition.\n",
            "  handleClick = () => newTransition();\n",
            "}\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "class fields and their callable value must be one semantic owner: {findings:#?}"
    );
}

#[test]
fn wrapped_component_uses_its_binding_identity() {
    let findings = changed(
        concat!(
            "const OldCard = memo(forwardRef(() => {\n",
            "  // Coupled to the exported component identity.\n",
            "  return <main />;\n",
            "}));\n",
        ),
        concat!(
            "const NewCard = memo(forwardRef(() => {\n",
            "  // Coupled to the exported component identity.\n",
            "  return <main />;\n",
            "}));\n",
        ),
    );

    let finding = findings
        .iter()
        .find(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .expect("renaming a wrapped callable must stale its comment");
    assert!(
        finding.message.contains("leaf `NewCard`"),
        "the wrapper must not erase the binding identity: {finding:#?}"
    );
}

#[test]
fn nested_callback_does_not_inherit_the_component_binding() {
    let findings = changed(
        concat!(
            "const OldCard = () => items.map((item) => {\n",
            "  // Coupled only to this item callback.\n",
            "  return render(item);\n",
            "});\n",
        ),
        concat!(
            "const NewCard = () => items.map((item) => {\n",
            "  // Coupled only to this item callback.\n",
            "  return render(item);\n",
            "});\n",
        ),
    );

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "an intervening callable boundary must keep the callback independent: {findings:#?}"
    );
}

#[test]
fn wrapper_with_two_callable_arguments_is_one_fail_closed_owner() {
    let findings = changed(
        concat!(
            "// Coupled to the combined callbacks.\n",
            "const Pair = combine(\n",
            "  () => {\n",
            "    // Coupled only to the first callback.\n",
            "    return oldFirst();\n",
            "  },\n",
            "  () => {\n",
            "    // Coupled only to the second callback.\n",
            "    return second();\n",
            "  },\n",
            ");\n",
        ),
        concat!(
            "// Coupled to the combined callbacks.\n",
            "const Pair = combine(\n",
            "  () => {\n",
            "    // Coupled only to the first callback.\n",
            "    return newFirst();\n",
            "  },\n",
            "  () => {\n",
            "    // Coupled only to the second callback.\n",
            "    return second();\n",
            "  },\n",
            ");\n",
        ),
    );

    let stale: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .collect();
    assert_eq!(
        stale.iter().map(|finding| finding.line).collect::<Vec<_>>(),
        [1, 4, 8],
        "all comments in a callable binding must fail closed together: {findings:#?}"
    );
    assert!(
        stale
            .iter()
            .all(|finding| finding.message.contains("leaf `Pair`")),
        "contained callbacks must share the binding owner: {findings:#?}"
    );
}

#[test]
fn decorator_does_not_detach_a_method_comment() {
    let findings = changed(
        concat!(
            "class Card {\n",
            "  // Coupled to the decorated render contract.\n",
            "  @trace\n",
            "  render() { return <Old />; }\n",
            "}\n",
        ),
        concat!(
            "class Card {\n",
            "  // Coupled to the decorated render contract.\n",
            "  @trace\n",
            "  render() { return <New />; }\n",
            "}\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 2
        }),
        "decorator syntax must remain inside the method owner boundary: {findings:#?}"
    );
}

#[test]
fn custom_hook_body_change_stales_leading_comment() {
    let findings = changed(
        concat!(
            "// Coupled to the subscription protocol.\n",
            "export const useSubscription = () => useOldSource();\n",
        ),
        concat!(
            "// Coupled to the subscription protocol.\n",
            "export const useSubscription = () => useNewSource();\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "custom hooks use the same callable owner contract: {findings:#?}"
    );
}

#[test]
fn react_module_directive_participates_in_the_file_fingerprint() {
    let findings = changed(
        concat!(
            "// This module crosses the client boundary.\n",
            "\"use client\";\n",
            "export default function Card() { return <main />; }\n",
        ),
        concat!(
            "// This module crosses the client boundary.\n",
            "\"use server\";\n",
            "export default function Card() { return <main />; }\n",
        ),
    );

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "React string directives are executable code in the file owner: {findings:#?}"
    );
}

#[test]
fn jsx_comment_uses_the_component_binding_budget() {
    let source = concat!(
        "const Card = () => (\n",
        "  <main>\n",
        "    {/* one\n",
        "        two\n",
        "        three\n",
        "        four */}\n",
        "  </main>\n",
        ");\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Card.tsx"),
        text: source,
    })
    .expect("valid TSX should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "JSX comments remain comments owned by the component binding: {findings:#?}"
    );
}

#[test]
fn deeply_nested_react_types_and_wrappers_analyze_through_the_public_seam() {
    let depth = 64;
    let mut source = String::new();
    for index in 0..depth {
        source.push_str(&format!("class C{index} {{ static nested = "));
    }
    source.push_str("class Leaf { render() { return <main />; } }");
    source.push_str(&"; }\n".repeat(depth));
    source.push_str("const handler = ");
    source.push_str(&"wrap(".repeat(depth));
    source.push_str("() => 1");
    source.push_str(&")".repeat(depth));
    source.push_str(";\n");

    let findings = analyze_all(SourceFile {
        path: Path::new("Deep.tsx"),
        text: &source,
    })
    .expect("deep TSX should parse without recursive owner walks");

    assert!(findings.is_empty());
}

#[test]
fn typescript_interface_comment_ignores_unrelated_file_changes() {
    let before = concat!(
        "// Coupled to the public card shape.\n",
        "export interface Card { title: string }\n",
        "const unrelated = 1;\n",
    );
    let after = before.replace("unrelated = 1", "unrelated = 2");

    let findings = changed_at(Path::new("Card.ts"), before, &after);

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/comment-owner-changed"),
        "an interface comment belongs to the interface, not the file: {findings:#?}"
    );
}

#[test]
fn typescript_type_declarations_stale_when_their_own_shape_changes() {
    for (before, after) in [
        (
            "// Shape rationale.\ntype Card = { old: string };\n",
            "// Shape rationale.\ntype Card = { next: string };\n",
        ),
        (
            "// Enum rationale.\nenum Mode { Old }\n",
            "// Enum rationale.\nenum Mode { Next }\n",
        ),
    ] {
        let findings = changed_at(Path::new("types.ts"), before, after);
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/comment-owner-changed"),
            "the declaration's own change must stale its comment ({before:?}): {findings:#?}"
        );
    }
}

#[test]
fn typescript_namespace_has_a_real_type_budget() {
    let source = concat!(
        "namespace Api {\n",
        "  // one\n",
        "  // two\n",
        "  // three\n",
        "  // four\n",
        "\n",
        "  export const value = 1;\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Api.ts"),
        text: source,
    })
    .expect("valid TypeScript namespace should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/type-comment-budget"),
        "namespace comments belong to a Type owner, not file scope: {findings:#?}"
    );
}

#[test]
fn typescript_triple_slash_directives_are_metadata_only_in_the_preamble() {
    for directive in [
        "/// <reference path=\"./types.d.ts\" />",
        "/// <reference path='./quoted types.d.ts' />",
        "/// <reference types=\"node\" resolution-mode='import' preserve=\"true\" />",
        "/// <reference types='node' resolution-mode=\"require\" />",
        "/// <reference lib=\"es2024\"/>",
        "/// <reference no-default-lib=\"true\"/>",
        "/// <amd-module name=\"NamedModule\"/>",
        "/// <amd-dependency path=\"legacy/module\" name=\"legacy\"/>",
    ] {
        let source = format!(
            "{directive}\n// rationale\nfunction value() {{\n  const result = 1;\n  return result;\n}}\n"
        );
        let findings = analyze_all(SourceFile {
            path: Path::new("directives.ts"),
            text: &source,
        })
        .expect("valid TypeScript should parse");

        assert!(
            findings.is_empty(),
            "official preamble directive is compiler metadata ({directive}): {findings:#?}"
        );
    }

    let source = concat!(
        "#!/usr/bin/env node\n",
        "/// <reference types='node' />\n",
        "// rationale\n",
        "function value() {\n",
        "  const result = 1;\n",
        "  return result;\n",
        "}\n",
    );
    let findings = analyze_all(SourceFile {
        path: Path::new("executable.ts"),
        text: source,
    })
    .expect("valid TypeScript should parse");
    assert!(
        findings.is_empty(),
        "the compiler skips a leading shebang before collecting preamble pragmas: {findings:#?}"
    );
}

#[test]
fn malformed_unknown_or_nonpreamble_triple_slash_comments_remain_narrative() {
    for candidate in [
        "/// <reference path=\"\" />",
        "/// <reference unknown=\"node\" />",
        "/// <reference path=\"types.d.ts\">",
        "/// <reference path=\"types.d.ts\" resolution-mode=\"import\" />",
        "/// <reference lib=\"es2024\" resolution-mode=\"require\" />",
        "/// <reference types=\"node\" resolution-mode=\"dynamic\" />",
        "/// <reference no-default-lib=\"true\" resolution-mode=\"import\" />",
        "/// <amd-dependency name=\"missing-path\"/>",
        "/// <unknown value=\"x\"/>",
    ] {
        let source =
            format!("{candidate}\n{candidate}\n{candidate}\n{candidate}\nconst value = 1;\n");
        let findings = analyze_all(SourceFile {
            path: Path::new("invalid.ts"),
            text: &source,
        })
        .expect("valid TypeScript comments should parse");
        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
            "malformed compiler syntax cannot launder narrative ({candidate}): {findings:#?}"
        );
    }

    let source = concat!(
        "const before = 1;\n",
        "/// <reference types=\"node\" />\n",
        "/// <reference types=\"node\" />\n",
        "/// <reference types=\"node\" />\n",
        "/// <reference types=\"node\" />\n",
        "const after = 2;\n",
    );
    let findings = analyze_all(SourceFile {
        path: Path::new("late.ts"),
        text: source,
    })
    .expect("valid TypeScript should parse");
    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "a valid tag after a declaration is ordinary narrative: {findings:#?}"
    );
}

#[test]
fn typescript_triple_slash_directives_keep_absolute_cap_and_stale_teeth() {
    let directive = "/// <reference types=\"node\" />\n";
    let source = format!("{}const value = 1;\n", directive.repeat(9));
    let findings = analyze_all(SourceFile {
        path: Path::new("capped.ts"),
        text: &source,
    })
    .expect("valid TypeScript should parse");
    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/owner-comment-cap"),
        "compiler metadata cannot bypass the absolute cap: {findings:#?}"
    );

    let before = format!("{directive}\"use client\";\n");
    let after = format!("{directive}\"use server\";\n");
    let findings = changed_at(Path::new("stale.ts"), &before, &after);
    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/comment-owner-changed"),
        "compiler metadata still requires stale attestation: {findings:#?}"
    );
}

#[test]
fn typescript_bodyless_signature_has_a_local_function_budget() {
    let source = concat!(
        "interface Renderer {\n",
        "  // one\n",
        "  // two\n",
        "  // three\n",
        "  // four\n",
        "  render(value: string): number;\n",
        "}\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Renderer.ts"),
        text: source,
    })
    .expect("valid TypeScript should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a signature must not borrow its interface's larger budget: {findings:#?}"
    );
}
