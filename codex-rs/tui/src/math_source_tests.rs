use pretty_assertions::assert_eq;

use super::*;

fn prepared(
    source: &str,
) -> (
    String,
    Vec<InlineMathSpan>,
    Vec<DisplayMathSpan>,
    Option<usize>,
) {
    let prepared = PreparedMath::new(source);
    let pending = prepared.pending_display_start();
    let PreparedMathParts {
        markdown,
        inline,
        display,
        ..
    } = prepared.into_parts();
    (markdown.into_owned(), inline, display, pending)
}

fn inline_raw<'a>(source: &'a str, span: &InlineMathSpan) -> &'a str {
    &source[span.source_range.clone()]
}

fn inline_formula<'a>(source: &'a str, span: &InlineMathSpan) -> &'a str {
    let raw = inline_raw(source, span);
    &raw[2..raw.len() - 2]
}

fn inline_raws<'a>(source: &'a str, spans: &[InlineMathSpan]) -> Vec<&'a str> {
    spans.iter().map(|span| inline_raw(source, span)).collect()
}

fn inline_formulas<'a>(source: &'a str, spans: &[InlineMathSpan]) -> Vec<&'a str> {
    spans
        .iter()
        .map(|span| inline_formula(source, span))
        .collect()
}

fn display_formulas(spans: &[DisplayMathSpan]) -> Vec<&str> {
    spans.iter().map(|span| span.formula.as_str()).collect()
}

#[test]
fn recognizes_explicit_math_and_preserves_offsets() {
    let source = "Given \\(α + β\\).\n\\[\nγ = δ\n\\]\n$$x^2$$";
    let (markdown, inline, display, pending) = prepared(source);

    assert_eq!(markdown.len(), source.len());
    assert_eq!(pending, None);
    assert_eq!(
        inline,
        vec![InlineMathSpan {
            source_range: 6..17,
            marker: '\u{F0000}',
        }],
    );
    assert_eq!(
        display,
        vec![
            DisplayMathSpan {
                source_range: 19..32,
                formula: "γ = δ".to_string(),
                quote_depth: 0,
                body_indent: 0,
            },
            DisplayMathSpan {
                source_range: 33..40,
                formula: "x^2".to_string(),
                quote_depth: 0,
                body_indent: 0,
            },
        ],
    );
}

#[test]
fn code_opened_first_owns_math_delimiters() {
    let source = concat!(
        "```tex\n",
        "\\[\n",
        "```\n",
        "\\]\n\n",
        "\\[\n",
        "real\n",
        "\\]\n",
        "~~~text\n",
        "\\(also code\\)\n",
        "$$\n",
        "also code\n",
        "$$\n",
        "~~~\n",
    );
    let (_, inline, display, _) = prepared(source);

    assert_eq!(inline, Vec::new());
    assert_eq!(display_formulas(&display), vec!["real"]);
}

#[test]
fn code_and_math_bodies_do_not_reinterpret_list_markers() {
    let source = concat!(
        "```\n",
        "- ```\n",
        "\\(still code\\)\n",
        "```\n",
        "\\[\n",
        "x\n",
        "- \\]\n",
        "\\]\n",
    );
    let (_, inline, display, _) = prepared(source);

    assert_eq!(inline, Vec::new());
    assert_eq!(display_formulas(&display), vec!["x\n- \\]"]);
}

#[test]
fn multiline_inline_code_stays_inside_its_inline_block() {
    for source in [
        "`code\n\\(not math\\)\n\\[\nalso not math\n\\]\ncode` after \\(math\\)",
        "> `code\n> \\(not math\\)\n> code` after \\(math\\)",
        "> `code\n\\(not math\\)\ncode` after \\(math\\)",
        "- `code\n  \\(not math\\)\n  code` after \\(math\\)",
        "- `code\n\\(not math\\)\ncode` after \\(math\\)",
        "`code\n2. paragraph text\n\\(not math\\)\ncode` after \\(math\\)",
        "`code\n    # paragraph text\n\\(not math\\)\ncode` after \\(math\\)",
        "`code\n** paragraph text\n\\(not math\\)\ncode` after \\(math\\)",
        r"`code \(not math\)\` after \(math\)",
    ] {
        let (_, inline, display, _) = prepared(source);
        assert_eq!(inline_formulas(source, &inline), vec!["math"], "{source:?}");
        assert_eq!(display, Vec::new(), "{source:?}");
    }
}

#[test]
fn a_block_opener_interrupts_an_unmatched_inline_code_opener() {
    for source in [
        "`unmatched\n```\n`\n\\(code\\)\n```\nafter \\(math\\)",
        "Before ```\n```rust\n\\(code\\)\n```\nafter \\(math\\)",
    ] {
        assert_eq!(
            inline_formulas(source, &prepared(source).1),
            vec!["math"],
            "{source:?}",
        );
    }
}

#[test]
fn inline_code_does_not_cross_blank_lines() {
    for source in [
        "`open\n\n\\(math\\) `",
        "`open\r\n\r\n\\(math\\) `",
        "`open\r\r\\(math\\) `",
        "> `open\n>\n> \\(math\\) `",
    ] {
        assert_eq!(
            inline_formulas(source, &prepared(source).1),
            vec!["math"],
            "{source:?}",
        );
    }
}

#[test]
fn math_opened_first_owns_markdown_punctuation() {
    let source = concat!(
        "\\[\n",
        "```markdown\n",
        "# heading\n",
        "| a | b |\n",
        "```\n",
        "\\]\n",
        "after \\(`code` + <tag>\\).\n",
        "$$\n",
        "~~~text\n",
        "$$\n",
    );
    let (_, inline, display, _) = prepared(source);

    assert_eq!(
        display_formulas(&display),
        vec!["```markdown\n# heading\n| a | b |\n```", "~~~text"],
    );
    assert_eq!(inline_formulas(source, &inline), vec!["`code` + <tag>"]);
}

#[test]
fn math_owned_code_delimiters_do_not_leak_ownership() {
    let cases: &[(&str, &[&str], &[&str])] = &[
        (
            "\\[\n```text\n\\]\nafter \\(x\\)\n```\n",
            &["x"],
            &["```text"],
        ),
        (r"\(a ` b\) then \(x\) `", &["a ` b", "x"], &[]),
        (r"\(a ` b\) ` code \(not math\) `", &["a ` b"], &[]),
        (
            "\\[\n```text\n\\]\n```\ninside \\(code\\)\n",
            &[],
            &["```text"],
        ),
    ];

    for &(source, expected_inline, expected_display) in cases {
        let (_, inline, display, _) = prepared(source);
        assert_eq!(
            (inline_formulas(source, &inline), display_formulas(&display),),
            (expected_inline.to_vec(), expected_display.to_vec()),
            "{source:?}",
        );
    }
}

#[test]
fn fenced_code_obeys_marker_and_container_boundaries() {
    let source = concat!(
        "````text\n",
        "\\(code\\)\n",
        "```\n",
        "\\(still code\\)\n",
        "````\n",
        "\\(math\\)\n",
        "- ```\n",
        "  \\(list code\\)\n",
        "outside \\(outside math\\)\n",
        "> ~~~\n",
        "> \\(quote code\\)\n",
        "after \\(after quote\\)\n",
    );
    let (_, inline, _, _) = prepared(source);

    assert_eq!(
        inline_formulas(source, &inline),
        vec!["math", "outside math", "after quote"],
    );
}

#[test]
fn indented_code_openers_do_not_consume_later_math() {
    let source = "    \\[\noutside \\(x\\)\n\\]";
    let (_, inline, display, _) = prepared(source);

    assert_eq!(inline_formulas(source, &inline), vec!["x"]);
    assert_eq!(display, Vec::new());
    assert_eq!(prepared("    \\[\ncode").3, None);
    assert_eq!(prepared("\t\\(code\\)").1, Vec::new());
}

#[test]
fn display_math_does_not_cross_container_boundaries() {
    for (source, expected_inline) in [
        ("> \\[\noutside \\(x\\)\n\\]", vec!["x"]),
        ("- \\[\noutside \\(x\\)\n\\]", vec!["x"]),
        ("> \\[\n\n> \\]", Vec::new()),
    ] {
        let (_, inline, display, pending) = prepared(source);
        assert_eq!(
            (inline_formulas(source, &inline), display, pending),
            (expected_inline, Vec::new(), None),
            "{source:?}",
        );
    }
}

#[test]
fn display_math_does_not_cross_sibling_list_items() {
    for source in [
        "1. First\n\n   \\[\n   x\n2. Second\n   \\]",
        "# heading\n2. \\[\n   x\n3. sibling\n   \\]",
        "- outer\n  - inner\n    - first\n\n      \\[\n      x\n    - sibling\n      \\]",
    ] {
        let (_, _, display, pending) = prepared(source);
        assert_eq!((display, pending), (Vec::new(), None), "{source:?}");
    }

    let same_item = "1. First\n\n   \\[\n   x\n   2. formula\n   \\]";
    let (_, _, display, _) = prepared(same_item);
    assert_eq!(display_formulas(&display), vec!["x\n2. formula"]);
}

#[test]
fn first_math_opener_owns_nested_openers() {
    let inline_source = r"before \(a \(b\) after";
    let (_, inline, _, _) = prepared(inline_source);
    assert_eq!(inline_raws(inline_source, &inline), vec![r"\(a \(b\)"],);

    let display = prepared(r"\[a \[ b\]").2;
    assert_eq!(display_formulas(&display), vec![r"a \[ b"]);
}

#[test]
fn angle_constructs_only_own_recognized_commonmark_constructs() {
    let constructs = r#"<https://example.test/\(path\)> <https://example.test/'/\(quoted\)> <span title="\(attr\)">\(text\)</span>"#;
    assert_eq!(
        inline_raws(constructs, &prepared(constructs).1),
        vec![r"\(text\)"],
    );

    let comparison = r"left < comparison > and \(x\)";
    assert_eq!(
        inline_formulas(comparison, &prepared(comparison).1),
        vec!["x"]
    );

    let invalid_email = r"<a\(inside\)@b.c> \(outside\)";
    let (_, inline, display, _) = prepared(invalid_email);
    assert_eq!((inline, display), (Vec::new(), Vec::new()));
}

#[test]
fn html_code_elements_own_math_delimiters() {
    let source = concat!(
        "<CoDe\n class=\"language-tex\">\\(code\\)<code>\\(nested\\)</code></cOdE\n>",
        " \\(math\\) <code/> \\(more\\)",
        " <code disabled=/>\\(attribute value\\)</code> \\(last\\)",
    );

    assert_eq!(
        inline_formulas(source, &prepared(source).1),
        vec!["math", "more", "last"],
    );
}

#[test]
fn non_tag_html_constructs_are_opaque_across_lines() {
    let source = concat!(
        "<!--\n<code>\\(hidden\\)</code>\n\\[\nhidden\n\\]\n--> \\(comment\\)\n",
        "<?pi\n<code>\\(hidden\\)</code>\n?> \\(processing\\)\n",
        "<![CDATA[\n<code>\\(hidden\\)</code>\n]]> \\(cdata\\)\n",
        "<!DECL <code>> \\(declaration\\)",
    );
    let (_, inline, display, _) = prepared(source);

    assert_eq!(
        (inline_formulas(source, &inline), display_formulas(&display)),
        (Vec::<&str>::new(), Vec::<&str>::new()),
    );
}

#[test]
fn short_html_comments_do_not_hide_following_math() {
    let source = r"<!--> \(first\) <!---> \(second\)";

    assert_eq!(
        inline_formulas(source, &prepared(source).1),
        vec!["first", "second"],
    );
}

#[test]
fn unterminated_html_constructs_fail_closed() {
    for source in [
        "<!-- \\(hidden\\)",
        "<?pi \\(hidden\\)",
        "<![CDATA[\\(hidden\\)",
        "<code title=\"unterminated \\(hidden\\)",
    ] {
        assert_eq!(prepared(source).1, Vec::new(), "{source:?}");
    }
}

#[test]
fn display_formula_preserves_content_across_structure_and_line_endings() {
    for (source, formula) in [
        (" \\[\nα\n \\]", "α"),
        ("\\[   \n  x\n   \\]", "  x"),
        ("\\[\na\n\nb\n\\]", "a\n\nb"),
        ("\\[\r\na\r\n\r\nb\r\n\\]", "a\n\nb"),
        ("\\[\rx\r\\]", "x"),
        ("$$\ry\r$$", "y"),
    ] {
        assert_eq!(
            display_formulas(&prepared(source).2),
            vec![formula],
            "{source:?}",
        );
    }
}

#[test]
fn fence_closer_rules_preserve_code_ownership() {
    let cases: &[(&str, &[&str])] = &[
        ("```\n    ```\n\\(still code\\)\n```", &[]),
        ("- ```\n  code\n\n  \\(still code\\)\n  ```", &[]),
        (
            "```\n```\t\n\\(still code\\)\n```\nafter \\(math\\)",
            &["math"],
        ),
        ("```\n   ```\nafter \\(math\\)", &["math"]),
    ];

    for &(source, expected) in cases {
        assert_eq!(
            inline_formulas(source, &prepared(source).1),
            expected.to_vec(),
            "{source:?}",
        );
    }
}

#[test]
fn fenced_code_indentation_is_relative_to_its_list_container() {
    for source in [
        "- item\n\n    ```\n  \\(code\\)\n    ```\nafter \\(math\\)",
        "- \n    ```\n  \\(code\\)\n    ```\nafter \\(math\\)",
        "1.\n     ```\n   \\(code\\)\n     ```\nafter \\(math\\)",
        "paragraph\n2.\n\n    \\(code\\)\nafter \\(math\\)",
        "- - -\n\n    \\(code\\)\nafter \\(math\\)",
    ] {
        assert_eq!(
            inline_formulas(source, &prepared(source).1),
            vec!["math"],
            "{source:?}",
        );
    }
}

#[test]
fn tabbed_list_marker_padding_preserves_indented_code() {
    for source in ["-\t  \\(code\\)", "-\t\t\\(code\\)", "1.\t   \\(code\\)"] {
        assert_eq!(prepared(source).1, Vec::new(), "{source:?}");
    }

    let source = "-\t\\(math\\)";
    assert_eq!(inline_formulas(source, &prepared(source).1), vec!["math"],);
}

#[test]
fn list_indentation_distinguishes_math_from_indented_code() {
    let source = concat!(
        "1. Formula\n\n",
        "   \\[\n",
        "   x\n",
        "   \\]\n\n",
        "2. Code\n\n",
        "       \\[\n",
        "       not math\n",
        "       \\]\n",
    );
    let (_, _, display, _) = prepared(source);
    assert_eq!(display_formulas(&display), vec!["x"]);

    let indented_on_marker_line = "-     \\[\noutside \\(x\\)\n\\]";
    let (_, inline, display, pending) = prepared(indented_on_marker_line);
    assert_eq!(inline_formulas(indented_on_marker_line, &inline), vec!["x"],);
    assert_eq!((display, pending), (Vec::new(), None));
}

#[test]
fn display_math_is_eligible_in_tight_list_items() {
    let source = "1. formulas\n   \\[\n   a\n   \\]\n   $$\n   b\n   $$";
    let (_, _, display, _) = prepared(source);
    assert_eq!(display_formulas(&display), vec!["a", "b"]);
}

#[test]
fn blockquote_display_math_is_normalized() {
    let source = "> Before\n> \\[\n>   x > 0\n> \\]\n> After";
    let (markdown, _, display, _) = prepared(source);

    assert_eq!(
        display,
        vec![DisplayMathSpan {
            source_range: 11..28,
            formula: "  x > 0".to_string(),
            quote_depth: 1,
            body_indent: 0,
        }],
    );
    assert_eq!(
        markdown.matches("\n> ").count(),
        source.matches("\n> ").count()
    );
}

#[test]
fn display_dollar_delimiters_are_exact() {
    for source in ["$x$", "$$$x$$$", "$$$$", "$$x$$$$", "before $$x$$"] {
        assert_eq!(prepared(source).2, Vec::new(), "{source:?}");
    }
    assert_eq!(display_formulas(&prepared("$$x$$").2), vec!["x"]);
}

#[test]
fn incomplete_display_math_reports_its_container_start() {
    assert_eq!(prepared("Before.\n\\[\nx").3, Some(0));
    assert_eq!(prepared("Before.\n\n\\[\nx").3, Some("Before.\n\n".len()));
    assert_eq!(
        prepared("> Before\n>\n> \\[\n> x").3,
        Some("> Before\n>\n".len()),
    );
    assert_eq!(
        prepared("1. Before\n\n   \\[\n   x").3,
        Some("1. Before\n\n".len()),
    );
    assert_eq!(prepared("```\n\\[\nx").3, None);
    assert_eq!(prepared("> \\[\noutside \\(x\\)").3, None);
}

#[test]
fn inline_mask_marker_avoids_literal_and_numeric_entity_collisions() {
    let source = "\u{F0000} &#983040; &#xF0001; before \\(x\\) after";
    let prepared = PreparedMath::new(source);

    let PreparedMathParts {
        markdown,
        inline,
        display,
        ..
    } = prepared.into_parts();
    assert_eq!(markdown.len(), source.len());
    assert_eq!(inline[0].marker, '\u{F0002}');
    assert_eq!(inline_formulas(source, &inline), vec!["x"]);
    assert_eq!(display, Vec::new());
}

#[test]
fn source_edits_remap_accepted_math_without_recognizing_new_math() {
    let source = "remove\n\\(accepted\\)\nplaceholder";
    let prepared = PreparedMath::new(source).apply_edits(&[
        SourceEdit {
            range: 0..7,
            replacement: String::new(),
        },
        SourceEdit {
            range: 20..31,
            replacement: r"\(still-code-owned\)".to_string(),
        },
    ]);
    let PreparedMathParts {
        source,
        inline,
        display,
        ..
    } = prepared.into_parts();

    assert_eq!(source, "\\(accepted\\)\n\\(still-code-owned\\)");
    assert_eq!(
        inline,
        vec![InlineMathSpan {
            source_range: 0..12,
            marker: '\u{F0000}',
        }]
    );
    assert_eq!(display, Vec::new());
}

#[test]
fn many_backtick_candidates_still_recognize_math_after_a_boundary() {
    let openers = (1..=100)
        .map(|len| format!("{}x", "`".repeat(len)))
        .collect::<String>();
    let closers = (1..=100)
        .map(|len| format!("{}x", "`".repeat(len)))
        .collect::<String>();
    let source = format!("{openers}\n{}\n\n\\(math\\)\n{closers}", "a".repeat(20_000));
    let prepared = PreparedMath::new(&source);

    let PreparedMathParts {
        inline, display, ..
    } = prepared.into_parts();
    assert_eq!(inline_formulas(&source, &inline), vec!["math"]);
    assert_eq!(display, Vec::new());
}

#[test]
fn link_metadata_is_opaque_while_visible_labels_remain_eligible() {
    let cases: &[(&str, &[&str])] = &[
        (
            r#"[label \(shown\)](<https://example.test/\(path\)> "\(title\)") \(tail\)"#,
            &["shown", "tail"],
        ),
        (
            r#"![alt \(shown\)](image-\(hidden\).png 'title \(hidden\)') \(tail\)"#,
            &["shown", "tail"],
        ),
        (
            r#"[label \(shown\)](nested_(one_(two)) "\(title\)") \(tail\)"#,
            &["shown", "tail"],
        ),
        (
            r#"[label \(shown\)][\(hidden id\)] \(tail\)"#,
            &["shown", "tail"],
        ),
    ];

    for &(source, expected) in cases {
        assert_eq!(
            inline_formulas(source, &prepared(source).1),
            expected.to_vec(),
            "{source:?}",
        );
    }
}

#[test]
fn potentially_structural_reference_labels_force_literal_fallback() {
    let source = concat!(
        "[shortcut \\(literal\\)]\n\n",
        "[shortcut \\(literal\\)]: /shortcut\n",
        "[collapsed \\(literal\\)][]\n\n",
        "[collapsed \\(literal\\)]: /collapsed\n",
        "[ordinary brackets \\(also literal\\)]\n",
        "After \\(math\\)",
    );

    let prepared = PreparedMath::new(source);
    assert!(prepared.renders_literal());
    assert!(!prepared.has_math());
}

#[test]
fn reference_definitions_own_identifiers_destinations_and_titles() {
    for source in [
        "[\\(id\\)]: /url/\\(destination\\) \"\\(title\\)\"\nAfter \\(math\\)",
        "[\\(id\\)]:\n  <https://example.test/\\(destination\\)>\nAfter \\(math\\)",
        "[\\(id\\)]: /url\n  '\\(continued title\\)'\nAfter \\(math\\)",
    ] {
        assert_eq!(
            inline_formulas(source, &prepared(source).1),
            vec!["math"],
            "{source:?}",
        );
    }
}

#[test]
fn raw_html_blocks_own_their_bodies_but_inline_html_does_not() {
    let cases: &[(&str, &[&str])] = &[
        ("<div>\n\\(hidden\\)\n</div>\n\nAfter \\(math\\)", &["math"]),
        (
            "<script>\n\n\\(hidden across blank\\)\n</SCRIPT>\nAfter \\(math\\)",
            &["math"],
        ),
        (
            "<x-widget>\n\\(hidden\\)\n</x-widget>\n\nAfter \\(math\\)",
            &["math"],
        ),
        (
            r#"Before <span title="\(attribute\)">\(visible\)</span> \(tail\)"#,
            &["visible", "tail"],
        ),
    ];

    for &(source, expected) in cases {
        assert_eq!(
            inline_formulas(source, &prepared(source).1),
            expected.to_vec(),
            "{source:?}",
        );
    }
}

#[test]
fn masking_preserves_commonmark_link_metadata() {
    use pulldown_cmark::Event;
    use pulldown_cmark::Options;
    use pulldown_cmark::Parser;
    use pulldown_cmark::Tag;

    fn metadata(source: &str) -> Vec<(String, String, String)> {
        Parser::new_ext(source, Options::ENABLE_TABLES)
            .filter_map(|event| match event {
                Event::Start(
                    Tag::Link {
                        dest_url,
                        title,
                        id,
                        ..
                    }
                    | Tag::Image {
                        dest_url,
                        title,
                        id,
                        ..
                    },
                ) => Some((
                    dest_url.into_string(),
                    title.into_string(),
                    id.into_string(),
                )),
                _ => None,
            })
            .collect()
    }

    let source = concat!(
        "[visible \\(x\\)](https://example.test/\\(path\\) \"title \\(hidden\\)\")\n",
        "\n",
        "[reference \\(y\\)][id]\n\n",
        "[id]: /destination/\\(hidden\\) 'title \\(hidden\\)'\n",
        "[math id][\\(id\\)]\n\n",
        "[\\(id\\)]: /math-id\n",
    );
    let prepared = PreparedMath::new(source);
    let original = metadata(source);

    assert_eq!(original.len(), 3);
    assert_eq!(metadata(prepared.markdown()), original);
}
