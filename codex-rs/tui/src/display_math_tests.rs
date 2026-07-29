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
fn preparation_leaves_rejected_candidates_exact() {
    for source in [
        "```tex\n\\[\ncode\n\\]\n```",
        "<div>\n\\[\nhtml\n\\]\n</div>",
        "- item\n\n  \\[\n  listed\n  \\]",
        "equation |\n--- |\n$$table$$",
    ] {
        let (prepared, spans, _) = PreparedDisplayMath::new(source, b'\x01').into_parts();

        assert_eq!(&*prepared, source, "{source:?}");
        assert!(spans.is_empty(), "{source:?}");
    }
}

#[test]
fn preparation_masks_a_setext_like_formula_after_rejected_candidates() {
    let source = concat!(
        "```tex\n\\[\n```\n\\]\n\n",
        "<div>\n\\[\nhtml\n\\]\n</div>\n\n",
        "- item\n\n  \\[\n  listed\n  \\]\n\n",
        "equation |\n--- |\n$$table$$\n\n",
        "\\[\na\n=\nb\n\\]",
    );
    let (prepared, spans, _) = PreparedDisplayMath::new(source, b'\x01').into_parts();

    assert_eq!(
        &*prepared,
        concat!(
            "```tex\n\\[\n```\n\\]\n\n",
            "<div>\n\\[\nhtml\n\\]\n</div>\n\n",
            "- item\n\n  \\[\n  listed\n  \\]\n\n",
            "equation |\n--- |\n$$table$$\n\n",
            "\\[\n\x01\n\x01\n\x01\n\\]",
        )
    );
    let expected_start = source
        .find("\\[\na\n=\nb\n\\]")
        .expect("fixture contains display math");
    assert_eq!(spans.len(), 1);
    assert_eq!(
        &spans[0].source_range,
        &(expected_start..expected_start + "\\[\na\n=\nb\n\\]".len())
    );
}

#[test]
fn preparation_carries_pending_structural_start_without_changing_source() {
    for (source, expected_start) in [
        ("\\[\na\n=\nb", Some(0)),
        ("Before.\n\n  $$\nlong formula", Some("Before.\n\n".len())),
        ("$$\n```tex\nnot a real fence", Some(0)),
        ("$$\n--- |\nvalue", Some(0)),
        ("$$\na\n\n   ", Some(0)),
        ("before\n$$\nx", None),
        ("- item\n\n  $$\n  x", None),
        ("```\n$$\nx", None),
        ("<div>\n$$\nx", None),
        ("$$\na\n\nb", None),
        ("$$\n\rx", None),
        ("$$\nx\r\ry", None),
        ("$$\nx\r\r   ", Some(0)),
        ("$$\na\n\nx\r\r", None),
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
            "$$\nnot closed\n\n$$recovered$$",
            &[("$$recovered$$", "recovered")],
        ),
        (
            "$$\nnot closed\n\n\\[\nrecovered\n\\]\n\n$$broken",
            &[("\\[\nrecovered\n\\]", "\nrecovered\n")],
        ),
        (
            "\\[\n\\rho\\left(\n\\frac{\\partial \\mathbf{u}}{\\partial t}\n\\right)\n=\n-\\nabla p\n\\]",
            &[(
                "\\[\n\\rho\\left(\n\\frac{\\partial \\mathbf{u}}{\\partial t}\n\\right)\n=\n-\\nabla p\n\\]",
                "\n\\rho\\left(\n\\frac{\\partial \\mathbf{u}}{\\partial t}\n\\right)\n=\n-\\nabla p\n",
            )],
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
        "That costs $5.",
        "Use $HOME and ${SHELL}.",
        "before $$x$$",
        "$$x$$ after",
        r"before \[x\]",
        r"\[x\] after",
        "```tex\n$$x$$\n```",
        "    $$x$$",
        "\t\\[x\\]",
        "`$$x$$`",
        "- $$x$$",
        "1. \\[x\\]",
        "> $$x$$",
        "| equation |\n| --- |\n| $$x$$ |",
        "equation |\n--- |\n$$x$$",
        "<div>\n$$x$$\n</div>",
        "$$<span>x</span>$$",
        "$$<!-- x -->$$",
        "\\[<em>x</em>\\]",
        "$$$$",
        "$$ $$",
        "$$\n\t\n$$",
        "$$\nx\n\n=\ny\n$$",
        "$$x",
        "x$$",
        r"\[\]",
        "\\[\n \n\\]",
        r"\[x",
        r"x\]",
        r"\[x$$",
        r"$$x\]",
        "$$a$$\n$$b$$",
        "\\[a\\]\n\\[b\\]",
        "$$a$$$$",
    ] {
        assert_eq!(extract(markdown), Vec::new(), "{markdown:?}");
    }
}
