use std::path::Path;

use fuck_ai_comments::{SourceFile, analyze_all, analyze_change};

#[test]
fn html_unquoted_spaced_lang_attribute_selects_typescript() {
    let source = concat!(
        "<script lang = ts>\n",
        "function work(): number {\n",
        "  // first\n",
        "  const first: number = 1;\n",
        "  // second\n",
        "  return first;\n",
        "}\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("a literal lang=ts script should use the TypeScript parser");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "the embedded TypeScript function should retain owner policy: {findings:#?}"
    );
}

#[test]
fn unrelated_data_attribute_cannot_suppress_javascript() {
    let source = concat!(
        "<script data-format=application/json>\n",
        "function work() {\n",
        "  // first\n",
        "  const first = 1;\n",
        "  // second\n",
        "  return first;\n",
        "}\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("data-* text must not change the script parser");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "the JavaScript script should still be checked: {findings:#?}"
    );
}

#[test]
fn unrelated_data_attribute_cannot_select_typescript() {
    let source = concat!(
        "<script data-lang=\"ts\">\n",
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "const value = 1;\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("data-* text must not change the script parser");
    let finding = findings
        .iter()
        .find(|finding| finding.rule == "comment-policy/leaf-comment-budget")
        .expect("the JavaScript leaf should exceed its comment allowance");

    assert!(
        finding.message.contains("JavaScript leaf"),
        "data-lang must not select TypeScript: {finding:#?}"
    );
}

#[test]
fn data_mime_wins_over_typescript_text_in_unrelated_attribute() {
    let source = concat!(
        "<script type = text/plain data-note='lang=\"ts\"'>\n",
        "not JavaScript {{ arbitrary template data }}\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("a non-code script type should remain opaque template data");

    assert!(findings.is_empty(), "template data has no comment owners");
}

#[test]
fn module_and_javascript_mime_types_select_javascript() {
    for script_type in ["module", "text/javascript", "APPLICATION/JAVASCRIPT"] {
        let source = format!(
            concat!(
                "<script type=\"{}\">\n",
                "function work() {{\n",
                "  // first\n",
                "  const first = 1;\n",
                "  // second\n",
                "  return first;\n",
                "}}\n",
                "</script>\n",
            ),
            script_type,
        );

        let findings = analyze_all(SourceFile {
            path: Path::new("index.html"),
            text: &source,
        })
        .expect("a JavaScript type should use the JavaScript parser");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
            "script type {script_type:?} should retain owner policy: {findings:#?}"
        );
    }
}

#[test]
fn astro_attribute_free_script_selects_typescript() {
    let source = concat!(
        "<script>\n",
        "function work(): number {\n",
        "  // first\n",
        "  const first: number = 1;\n",
        "  // second\n",
        "  return first;\n",
        "}\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("Astro should process an attribute-free script as TypeScript");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "the processed TypeScript function should retain owner policy: {findings:#?}"
    );
}

#[test]
fn astro_src_only_script_selects_typescript() {
    let source = concat!(
        "<script src='/assets/client.ts'>\n",
        "function work(): number {\n",
        "  // first\n",
        "  const first: number = 1;\n",
        "  // second\n",
        "  return first;\n",
        "}\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("Astro should process a src-only script as TypeScript");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "the processed TypeScript function should retain owner policy: {findings:#?}"
    );
}

#[test]
fn astro_non_src_attribute_uses_browser_javascript_semantics() {
    let source = concat!(
        "<script is:inline>\n",
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "const value = 1;\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("an unprocessed Astro script should use browser JavaScript semantics");
    let finding = findings
        .iter()
        .find(|finding| finding.rule == "comment-policy/leaf-comment-budget")
        .expect("the JavaScript leaf should exceed its comment allowance");

    assert!(
        finding.message.contains("JavaScript leaf"),
        "is:inline must opt out of Astro TypeScript processing: {finding:#?}"
    );
}

#[test]
fn encoded_javascript_mime_cannot_bypass_html_analysis() {
    let source = concat!(
        "<script type='text&#x2f;javascript'>\n",
        "function work() {\n",
        "  // first\n",
        "  const first = 1;\n",
        "  // second\n",
        "  return first;\n",
        "}\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("HTML character references should be decoded before type classification");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "an encoded JavaScript MIME must still be checked: {findings:#?}"
    );
}

#[test]
fn dynamic_astro_script_type_is_scanned_as_javascript() {
    let source = concat!(
        "<script type={contentType}>\n",
        "function work() {\n",
        "  // first\n",
        "  const first = 1;\n",
        "  // second\n",
        "  return first;\n",
        "}\n",
        "</script>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("a nonliteral script type should be checked conservatively as JavaScript");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/function-comment-budget"),
        "a dynamic script type must not bypass owner policy: {findings:#?}"
    );
}

#[test]
fn non_javascript_dynamic_astro_script_fails_closed() {
    let source = concat!(
        "<script type={contentType}>\n",
        "not JavaScript {{ arbitrary template data }}\n",
        "</script>\n",
    );

    let result = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    });

    assert!(
        result.is_err(),
        "unknown dynamic content must not bypass parsing"
    );
}

#[test]
fn separate_one_line_html_comments_share_the_template_budget() {
    let source = concat!(
        "<!-- first -->\n",
        "<header>heading</header>\n",
        "<!-- second -->\n",
        "<main>body</main>\n",
        "<!-- third -->\n",
        "<footer>footer</footer>\n",
        "<!-- fourth -->\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("valid HTML should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
        "splitting comments across elements must not reset the template budget: {findings:#?}"
    );
}

#[test]
fn json_and_data_script_types_remain_opaque() {
    for script_type in [
        "application/json",
        "application/ld+json",
        "application/vnd.example+json",
        "application/javascript; charset=utf-8",
        "importmap",
        "speculationrules",
        "text/plain",
    ] {
        let source = format!(
            concat!(
                "<script type='{}'>\n",
                "not JavaScript {{{{ arbitrary template data }}}}\n",
                "</script>\n",
            ),
            script_type,
        );

        let findings = analyze_all(SourceFile {
            path: Path::new("index.html"),
            text: &source,
        })
        .expect("a data script must remain opaque to the JavaScript parser");

        assert!(
            findings.is_empty(),
            "script type {script_type:?} should not create code owners"
        );
    }
}

#[test]
fn separate_one_line_astro_comments_share_the_template_budget() {
    let source = concat!(
        "<!-- first -->\n",
        "<header>heading</header>\n",
        "<!-- second -->\n",
        "<main>body</main>\n",
        "<!-- third -->\n",
        "<footer>footer</footer>\n",
        "<!-- fourth -->\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("valid Astro should parse");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
        "splitting comments across elements must not reset the template budget: {findings:#?}"
    );
}

#[test]
fn three_separate_template_comment_lines_remain_allowed() {
    let source = concat!(
        "<!-- first -->\n",
        "<header>heading</header>\n",
        "<!-- second -->\n",
        "<main>body</main>\n",
        "<!-- third -->\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("valid HTML should parse");

    assert!(
        findings.is_empty(),
        "three aggregate lines are within the template allowance: {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_comments_do_not_consume_markup_budget() {
    let source = concat!(
        "---\n",
        "function work() {\n",
        "  // first\n",
        "  const first = 1;\n",
        "  // second\n",
        "  const second = 2;\n",
        "  // third\n",
        "  const third = 3;\n",
        "  // fourth\n",
        "  return first + second + third;\n",
        "}\n",
        "---\n",
        "<main>{work()}</main>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("valid Astro should parse");

    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/template-comment-budget"),
        "frontmatter comments belong only to the TypeScript owner: {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_regex_can_match_backticks() {
    let source = concat!(
        "---\n",
        "const cleaned = value.replace(/`/g, '');\n",
        "---\n",
        "<main>{cleaned}</main>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("a valid TypeScript regex must not hide the Astro frontmatter fence");

    assert!(
        findings.is_empty(),
        "a comment-free Astro component should remain clean: {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_regex_and_template_can_share_fake_fence() {
    let source = concat!(
        "---\n",
        "const cleaned = value.replace(/`/g, '');\n",
        "const art = `before\n",
        "---\n",
        "after`;\n",
        "---\n",
        "<main>{cleaned}{art}</main>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("a fence inside a template remains TypeScript during recovery");

    assert!(
        findings.is_empty(),
        "a comment-free Astro component should remain clean: {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_comment_can_contain_many_fake_fences() {
    let source = concat!(
        "---\n",
        "const cleaned = value.replace(/`/g, '');\n",
        "/*\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "---\n",
        "*/\n",
        "---\n",
        "<main>{cleaned}</main>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("fake fences inside one comment do not impose a parse-work cutoff");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
        "the whole block comment must remain in TypeScript policy scope: {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_closing_fence_can_be_indented() {
    let source = concat!(
        "---\n",
        "const pattern = /`/g;\n",
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "const value = 1;\n",
        "  ---\n",
        "<main>{value}</main>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("Astro permits horizontal whitespace before a closing fence");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "indented fences must preserve TypeScript policy scope: {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_expression_starting_with_three_hyphens_is_not_a_fence() {
    let source = concat!(
        "---\n",
        "const pattern = /`/g;\n",
        "let x = 1;\n",
        "  ---x;\n",
        "// first\n",
        "// second\n",
        "// third\n",
        "// fourth\n",
        "const value = x;\n",
        "---\n",
        "<main>{value}</main>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("a TypeScript expression prefixed by three hyphens is not a fence");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/leaf-comment-budget"),
        "comments after the expression must stay in TypeScript policy scope: {findings:#?}"
    );
}

#[test]
fn malformed_astro_frontmatter_still_fails_closed() {
    let source = concat!(
        "---\n",
        "const value = ;\n",
        "---\n",
        "<main>{value}</main>\n",
    );

    let result = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    });

    assert!(
        result.is_err(),
        "malformed frontmatter must not be masked clean"
    );
}

#[test]
fn malformed_astro_frontmatter_cannot_resynchronize_at_a_later_fence() {
    let source = concat!(
        "---\n",
        "const value =\n",
        "---\n",
        "1;\n",
        "---\n",
        "<main>{value}</main>\n",
    );

    let result = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    });

    assert!(
        result.is_err(),
        "the first external fence is authoritative even when its prefix is invalid"
    );
}

#[test]
fn malformed_astro_frontmatter_cannot_fall_back_to_a_resynchronized_outer_tree() {
    let source = concat!(
        "---\n",
        "const pattern = /`/g;\n",
        "const value =\n",
        "---\n",
        "1;\n",
        "// `\n",
        "---\n",
        "<main>{value}</main>\n",
    );

    let result = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    });

    assert!(
        result.is_err(),
        "invalid recovered TypeScript must not fall back to the desynchronized Astro tree"
    );
}

#[test]
fn astro_frontmatter_mask_preserves_crlf_unicode_coordinates() {
    let before = concat!(
        "\u{feff}<!-- leading -->\r\n",
        "---\r\n",
        "const pattern = /`/g;\r\n",
        "function work(): number {\r\n",
        "  // Coupled to 雪 and the returned value.\r\n",
        "  return 1;\r\n",
        "}\r\n",
        "---   \r\n",
        "<main>{work()}</main>\r\n",
    );
    let after = before.replacen("return 1", "return 2", 1);

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Page.astro"),
            text: before,
        },
        SourceFile {
            path: Path::new("Page.astro"),
            text: &after,
        },
    )
    .expect("masking must preserve Astro source coordinates");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 5
        }),
        "the original CRLF line number must survive masking: {findings:#?}"
    );
}

#[test]
fn astro_frontmatter_mask_preserves_lf_embedded_script_coordinates() {
    let before = concat!(
        "---\n",
        "const pattern = /`/g;\n",
        "  ---\n",
        "<script>\n",
        "function work() {\n",
        "  // Coupled to the returned protocol value.\n",
        "  return 1;\n",
        "}\n",
        "</script>\n",
    );
    let after = before.replacen("return 1", "return 2", 1);

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Page.astro"),
            text: before,
        },
        SourceFile {
            path: Path::new("Page.astro"),
            text: &after,
        },
    )
    .expect("masking must preserve post-frontmatter source coordinates");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(
        stale_lines,
        vec![6],
        "the embedded comment must retain its original LF line"
    );
}

#[test]
fn html_markup_change_stales_its_template_comment() {
    let before = "<!-- Coupled to the rendered label. -->\n<main>before</main>\n";
    let after = "<!-- Coupled to the rendered label. -->\n<main>after</main>\n";

    let findings = analyze_change(
        SourceFile {
            path: Path::new("index.html"),
            text: before,
        },
        SourceFile {
            path: Path::new("index.html"),
            text: after,
        },
    )
    .expect("valid HTML snapshots should produce an owner-aware change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "the unchanged markup comment should require re-attestation: {findings:#?}"
    );
}

#[test]
fn embedded_script_change_stales_only_its_inner_comment() {
    let before = concat!(
        "<!-- first outer comment -->\n",
        "<!-- second outer comment -->\n",
        "<!-- third outer comment -->\n",
        "<!-- fourth outer comment -->\n",
        "<script>\n",
        "function work() {\n",
        "  // Coupled to the returned protocol value.\n",
        "  return 1;\n",
        "}\n",
        "</script>\n",
    );
    let after = before.replacen("return 1", "return 2", 1);

    for path in [Path::new("index.html"), Path::new("Page.astro")] {
        let findings = analyze_change(
            SourceFile { path, text: before },
            SourceFile { path, text: &after },
        )
        .expect("embedded script owners should merge into container source coordinates");
        let stale_lines: Vec<_> = findings
            .iter()
            .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
            .map(|finding| finding.line)
            .collect();

        assert_eq!(
            stale_lines,
            vec![7],
            "nested code must not stale its parent template comment in {}",
            path.display()
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.rule != "comment-policy/template-comment-budget"),
            "an inner script change must not activate the outer template budget in {}: {findings:#?}",
            path.display()
        );
    }
}

#[test]
fn top_level_embedded_comment_insertion_cannot_bypass_diff_gate() {
    let cases = [
        (
            Path::new("index.html"),
            "<script>\nlet value = 1;\n</script>\n",
            concat!(
                "<script>\n",
                "// first\n",
                "// second\n",
                "// third\n",
                "// fourth\n",
                "let value = 1;\n",
                "</script>\n",
            ),
        ),
        (
            Path::new("Page.astro"),
            "<script>\nlet value = 1;\n</script>\n",
            concat!(
                "<script>\n",
                "// first\n",
                "// second\n",
                "// third\n",
                "// fourth\n",
                "let value = 1;\n",
                "</script>\n",
            ),
        ),
        (
            Path::new("Page.astro"),
            "---\nlet value = 1;\n---\n<main />\n",
            concat!(
                "---\n",
                "// first\n",
                "// second\n",
                "// third\n",
                "// fourth\n",
                "let value = 1;\n",
                "---\n",
                "<main />\n",
            ),
        ),
    ];
    for (path, before, after) in cases {
        let findings = analyze_change(
            SourceFile { path, text: before },
            SourceFile { path, text: after },
        )
        .expect("top-level embedded comments should retain their file owner");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/file-comment-budget"),
            "a top-level embedded comment insertion must be gated in {}: {findings:#?}",
            path.display()
        );
    }
}

#[test]
fn multiple_embedded_scripts_keep_owner_ids_isolated() {
    let before = concat!(
        "<script>\n",
        "function first() {\n",
        "  // Coupled to the first value.\n",
        "  return 1;\n",
        "}\n",
        "</script>\n",
        "<script>\n",
        "function second() {\n",
        "  // Coupled to the second value.\n",
        "  return 2;\n",
        "}\n",
        "</script>\n",
    );
    let after = before.replacen("return 2", "return 3", 1);

    let findings = analyze_change(
        SourceFile {
            path: Path::new("index.html"),
            text: before,
        },
        SourceFile {
            path: Path::new("index.html"),
            text: &after,
        },
    )
    .expect("multiple embedded owner trees should merge without id collisions");
    let stale_lines: Vec<_> = findings
        .iter()
        .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
        .map(|finding| finding.line)
        .collect();

    assert_eq!(stale_lines, vec![9], "only the second script owner changed");
}

#[test]
fn astro_frontmatter_change_uses_relocated_typescript_owner() {
    let before = concat!(
        "---\n",
        "function work(): number {\n",
        "  // Coupled to the returned protocol value.\n",
        "  return 1;\n",
        "}\n",
        "---\n",
        "<!-- first outer comment -->\n",
        "<!-- second outer comment -->\n",
        "<!-- third outer comment -->\n",
        "<!-- fourth outer comment -->\n",
        "<main>{work()}</main>\n",
    );
    let after = before.replacen("return 1", "return 2", 1);

    let findings = analyze_change(
        SourceFile {
            path: Path::new("Page.astro"),
            text: before,
        },
        SourceFile {
            path: Path::new("Page.astro"),
            text: &after,
        },
    )
    .expect("Astro frontmatter owners should use container source coordinates");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 3
        }),
        "the relocated frontmatter comment should require re-attestation: {findings:#?}"
    );
    assert!(
        findings
            .iter()
            .all(|finding| finding.rule != "comment-policy/template-comment-budget"),
        "a frontmatter change must not activate the markup template budget: {findings:#?}"
    );
}

#[test]
fn css_change_stales_its_stylesheet_comment() {
    let before = "/* Coupled to the contrast requirement. */\n.card { color: red; }\n";
    let after = "/* Coupled to the contrast requirement. */\n.card { color: blue; }\n";

    let findings = analyze_change(
        SourceFile {
            path: Path::new("site.css"),
            text: before,
        },
        SourceFile {
            path: Path::new("site.css"),
            text: after,
        },
    )
    .expect("valid CSS snapshots should produce an owner-aware change");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "the unchanged CSS comment should require re-attestation: {findings:#?}"
    );
}

#[test]
fn data_script_change_stales_its_template_comment() {
    let before = concat!(
        "<!-- Coupled to the embedded data contract. -->\n",
        "<script type=application/json>\n",
        "{\"value\": 1}\n",
        "</script>\n",
    );
    let after = before.replacen("\"value\": 1", "\"value\": 2", 1);

    let findings = analyze_change(
        SourceFile {
            path: Path::new("index.html"),
            text: before,
        },
        SourceFile {
            path: Path::new("index.html"),
            text: &after,
        },
    )
    .expect("data script contents should remain part of the template inventory");

    assert!(
        findings.iter().any(|finding| {
            finding.rule == "comment-policy/comment-owner-changed" && finding.line == 1
        }),
        "opaque script data still belongs to the template owner: {findings:#?}"
    );
}

#[test]
fn html_and_astro_embedded_styles_enforce_css_comment_budget() {
    let source = concat!(
        "<style>\n",
        "/* first\n",
        "second\n",
        "third\n",
        "fourth */\n",
        ".card { color: red; }\n",
        "</style>\n",
    );

    for path in [Path::new("index.html"), Path::new("Page.astro")] {
        let findings = analyze_all(SourceFile { path, text: source })
            .expect("embedded CSS should use the CSS parser");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
            "embedded CSS comments must be gated in {}: {findings:#?}",
            path.display()
        );
    }
}

#[test]
fn html_global_lang_attribute_does_not_disable_css_analysis() {
    let source = concat!(
        "<style lang='en'>\n",
        "/* first\n",
        "second\n",
        "third\n",
        "fourth */\n",
        ".card { color: red; }\n",
        "</style>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("the global lang attribute must not change HTML style semantics");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
        "HTML style content remains CSS regardless of global lang: {findings:#?}"
    );
}

#[test]
fn encoded_css_mime_cannot_bypass_html_analysis() {
    let source = concat!(
        "<style type='text&#x2f;css'>\n",
        "/* first\n",
        "second\n",
        "third\n",
        "fourth */\n",
        ".card { color: red; }\n",
        "</style>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("index.html"),
        text: source,
    })
    .expect("HTML character references should be decoded before type classification");

    assert!(
        findings
            .iter()
            .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
        "an encoded CSS MIME must still be checked: {findings:#?}"
    );
}

#[test]
fn dynamic_astro_style_attributes_are_scanned_as_css() {
    for attribute in ["lang={styleLanguage}", "type={styleType}"] {
        let source = format!(
            concat!(
                "<style {}>\n",
                "/* first\n",
                "second\n",
                "third\n",
                "fourth */\n",
                ".card {{ color: red; }}\n",
                "</style>\n",
            ),
            attribute,
        );

        let findings = analyze_all(SourceFile {
            path: Path::new("Page.astro"),
            text: &source,
        })
        .expect("a nonliteral style attribute should be checked conservatively as CSS");

        assert!(
            findings
                .iter()
                .any(|finding| finding.rule == "comment-policy/template-comment-budget"),
            "attribute {attribute:?} must not bypass CSS policy: {findings:#?}"
        );
    }
}

#[test]
fn non_css_dynamic_astro_style_fails_closed() {
    let source = concat!(
        "<style lang={styleLanguage}>\n",
        "$accent: red;\n",
        ".card { color: $accent; }\n",
        "</style>\n",
    );

    let result = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    });

    assert!(
        result.is_err(),
        "unknown dynamic content must not bypass parsing"
    );
}

#[test]
fn explicit_astro_scss_remains_opaque() {
    let source = concat!(
        "<style lang='scss'>\n",
        "$accent: red;\n",
        ".card { color: $accent; }\n",
        "</style>\n",
    );

    let findings = analyze_all(SourceFile {
        path: Path::new("Page.astro"),
        text: source,
    })
    .expect("an explicit unsupported preprocessor should remain opaque");

    assert!(findings.is_empty(), "SCSS should not create CSS owners");
}

#[test]
fn embedded_css_change_stales_only_its_inner_comment() {
    let before = concat!(
        "<!-- first outer comment -->\n",
        "<!-- second outer comment -->\n",
        "<!-- third outer comment -->\n",
        "<!-- fourth outer comment -->\n",
        "<style>\n",
        "/* Coupled to the contrast requirement. */\n",
        ".card { color: red; }\n",
        "</style>\n",
    );
    let after = before.replacen("color: red", "color: blue", 1);

    for path in [Path::new("index.html"), Path::new("Page.astro")] {
        let findings = analyze_change(
            SourceFile { path, text: before },
            SourceFile { path, text: &after },
        )
        .expect("embedded CSS owners should merge into container source coordinates");
        let stale_lines: Vec<_> = findings
            .iter()
            .filter(|finding| finding.rule == "comment-policy/comment-owner-changed")
            .map(|finding| finding.line)
            .collect();

        assert_eq!(
            stale_lines,
            vec![6],
            "nested CSS must not stale the parent comment in {}",
            path.display()
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.rule != "comment-policy/template-comment-budget"),
            "an inner CSS change must not activate the outer template budget in {}: {findings:#?}",
            path.display()
        );
    }
}
