use super::*;
use pretty_assertions::assert_eq;

fn extract(markdown: &str) -> Vec<(&str, &str)> {
    let prepared = PreparedDisplayMath::new(markdown, b'\x01');
    let (_, spans, _) = prepared.into_parts();
    spans
        .iter()
        .map(|span| (span.raw_block, span.formula))
        .collect()
}

#[test]
fn finds_setext_like_formula_after_rejected_candidates() {
    let source = concat!(
        "```tex\n\\[\n```\n\\]\n\n",
        "<div>\n\\[\nhtml\n\\]\n</div>\n\n",
        "- item\n\n      \\[\n      code\n      \\]\n\n",
        "equation |\n--- |\n$$table$$\n\n",
        "\\[\na\n=\nb\n\\]",
    );
    assert_eq!(extract(source), vec![("\\[\na\n=\nb\n\\]", "\na\n=\nb\n")],);
}

#[test]
fn preparation_accepts_numbered_list_items_but_not_indented_code() {
    let source = concat!(
        "1. First\n\n",
        "   \\[\n   E=mc^2\n   \\]\n\n",
        "2. Second\n\n",
        "   \\[\n   a+b\n   \\]\n\n",
        "3. Code\n\n",
        "       \\[\n       not math\n       \\]\n",
    );
    let first_raw = "\\[\n   E=mc^2\n   \\]";
    let second_raw = "\\[\n   a+b\n   \\]";
    let first_start = source
        .find(first_raw)
        .expect("fixture contains first formula");
    let second_start = source
        .find(second_raw)
        .expect("fixture contains second formula");
    let (_, spans, _) = PreparedDisplayMath::new(source, b'\x01').into_parts();

    assert_eq!(
        spans,
        vec![
            DisplayMathSpan {
                source_range: first_start..first_start + first_raw.len(),
                raw_block: first_raw,
                formula: "\n   E=mc^2\n   ",
            },
            DisplayMathSpan {
                source_range: second_start..second_start + second_raw.len(),
                raw_block: second_raw,
                formula: "\n   a+b\n   ",
            },
        ],
    );
}

#[test]
fn preparation_carries_pending_structural_start_without_changing_source() {
    for (source, expected_start) in [
        ("\\[\na\n=\nb", Some(0)),
        ("Before.\n\n  $$\nlong formula", Some("Before.\n\n".len())),
        ("$$\na\n\n   ", Some(0)),
        ("1. item\n   $$\n   x", Some("1. item\n".len())),
        ("before\n$$\nx", None),
        ("```\n$$\nx", None),
        ("<div>\n$$\nx", None),
        ("$$\na\n\nb", None),
        ("$$\nx\r\r   ", Some(0)),
        ("> $$\nx", None),
        ("    $$\nx", None),
        ("$$x", None),
    ] {
        let (prepared, spans, pending_start) =
            PreparedDisplayMath::new(source, b'\x01').into_parts();

        assert_eq!(&*prepared, source, "{source:?}");
        assert!(spans.is_empty(), "{source:?}");
        assert_eq!(pending_start, expected_start, "{source:?}");
    }
}

#[test]
fn accepted_display_math_cases() {
    let cases: &[(&str, &[(&str, &str)])] = &[
        ("$$α + β$$", &[("$$α + β$$", "α + β")]),
        (
            r"\[\sum_{i=0}^{n} i\]",
            &[(r"\[\sum_{i=0}^{n} i\]", r"\sum_{i=0}^{n} i")],
        ),
        (
            "intro\r\n\r\n  $$\r\n∑_{i=0}^{∞} αᵢ\r\n$$  \r\n",
            &[("$$\r\n∑_{i=0}^{∞} αᵢ\r\n$$", "\r\n∑_{i=0}^{∞} αᵢ\r\n")],
        ),
        (
            "$$a$$\n\nprose\n\n\\[\nb\n\\]\n\n$$c$$",
            &[("$$a$$", "a"), ("\\[\nb\n\\]", "\nb\n"), ("$$c$$", "c")],
        ),
        (
            "1. formulas\n   \\[\n   a\n   \\]\n   $$\n   b\n   $$",
            &[
                ("\\[\n   a\n   \\]", "\n   a\n   "),
                ("$$\n   b\n   $$", "\n   b\n   "),
            ],
        ),
        (
            "- outer\n  - inner\n    \\[\n    n\n    \\]\n\n100. wide\n     $$\n     w\n     $$",
            &[
                ("\\[\n    n\n    \\]", "\n    n\n    "),
                ("$$\n     w\n     $$", "\n     w\n     "),
            ],
        ),
        (
            "1. Formula\n\n   \\[\n   a\n   =\n   b\n   \\]",
            &[("\\[\n   a\n   =\n   b\n   \\]", "\n   a\n   =\n   b\n   ")],
        ),
        (
            "$$\nnot closed\n\n\\[\nrecovered\n\\]\n\n$$broken",
            &[("\\[\nrecovered\n\\]", "\nrecovered\n")],
        ),
        (
            "\\[\n\\text{fluid acceleration}\n=\n\\text{pressure forces}\n\\]",
            &[(
                "\\[\n\\text{fluid acceleration}\n=\n\\text{pressure forces}\n\\]",
                "\n\\text{fluid acceleration}\n=\n\\text{pressure forces}\n",
            )],
        ),
        ("$$x < y > z$$", &[("$$x < y > z$$", "x < y > z")]),
    ];

    for (markdown, expected) in cases {
        let actual = extract(markdown);
        assert_eq!(actual.as_slice(), *expected, "{markdown:?}");
    }
}

#[test]
fn rejected_display_math_cases() {
    for markdown in [
        "$x$",
        "Use $HOME and ${SHELL}.",
        "before $$x$$",
        "$$x$$ after",
        r"before \[x\]",
        "```tex\n$$x$$\n```",
        "    $$x$$",
        "\t\\[x\\]",
        "`$$x$$`",
        "- $$x$$",
        "1. \\[x\\]",
        "1. First\n\n   \\[\n   x\n2. Second\n   \\]",
        "> $$x$$",
        "| equation |\n| --- |\n| $$x$$ |",
        "equation |\n--- |\n$$x$$",
        "<div>\n$$x$$\n</div>",
        "$$<span>x</span>$$",
        "$$$$",
        "$$ $$",
        "$$\nx\n\n=\ny\n$$",
        "$$x",
        "x$$",
        r"\[\]",
        r"\[x",
        r"x\]",
        r"\[x$$",
        "$$a$$\n$$b$$",
        "$$a$$$$",
    ] {
        let (prepared, spans, _) = PreparedDisplayMath::new(markdown, b'\x01').into_parts();
        assert_eq!(&*prepared, markdown, "{markdown:?}");
        assert_eq!(spans, Vec::new(), "{markdown:?}");
    }
}
