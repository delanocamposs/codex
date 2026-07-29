use pretty_assertions::assert_eq;
use pulldown_cmark::Event;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;

use super::*;

type PreparedParts = (String, Option<char>, Vec<InlineMathSpan>);

fn prepared(source: &str) -> PreparedParts {
    let (markdown, mask, spans, _) = PreparedInlineMath::new(source).into_parts();
    (markdown.into_owned(), mask, spans)
}

#[test]
fn masks_balanced_explicit_inline_math_without_changing_byte_offsets() {
    let source = r"Given \(x\in\ker f''\), use \(\alpha_i * y\).";
    let (masked, _, spans) = prepared(source);

    assert_eq!(masked.len(), source.len());
    assert_eq!(
        spans
            .iter()
            .map(|span| (span.raw.as_str(), span.formula.as_str()))
            .collect::<Vec<_>>(),
        vec![
            (r"\(x\in\ker f''\)", r"x\in\ker f''"),
            (r"\(\alpha_i * y\)", r"\alpha_i * y"),
        ],
    );
}

#[test]
fn masked_math_cannot_become_markdown_structure() {
    let source = r"before \(\mathbf{x} * [y](z)\) after";
    let (markdown, _, _) = prepared(source);
    assert!(
        !Parser::new(&markdown)
            .any(|event| matches!(event, Event::Start(Tag::Emphasis | Tag::Link { .. }))),
        "{source:?}",
    );

    let source = "\\[\n\\text{fluid acceleration}\n=\n\\text{pressure forces}\n\\]";
    let (markdown, _, _) = prepared(source);
    assert!(
        !Parser::new(&markdown).any(|event| matches!(event, Event::Start(Tag::Heading { .. }))),
        "{source:?}",
    );
}

#[test]
fn rejects_ambiguous_or_non_textual_inline_math() {
    for source in [
        r"$x$",
        r"$$x$$",
        r"\[x\]",
        r"\(\)",
        "\\(\n x\n\\)",
        "\\(x\ry\\)",
        r"\(unclosed",
        r"\\(escaped\)",
        r"`\(code\)`",
        "```\n\\(code\\)\n```",
        r"[label](https://example.com/\(path\))",
        r"<https://example.com/\(path\)>",
        r"<code>\(code\)</code>",
        r"<span title='\(attribute\)'>text</span>",
    ] {
        assert_eq!(prepared(source).2, Vec::new(), "{source:?}");
    }
}

#[test]
fn ordinary_link_labels_can_preserve_literal_inline_math() {
    let source = r"[label \(x\)](https://example.com)";
    assert_eq!(
        prepared(source)
            .2
            .iter()
            .map(|span| span.raw.as_str())
            .collect::<Vec<_>>(),
        vec![r"\(x\)"],
    );
}

#[test]
fn preserves_adjacent_and_multibyte_formulas() {
    let source = "α\\(β\\)\\(γ + δ\\)ω";
    let (markdown, _, spans) = prepared(source);

    assert_eq!(markdown.len(), source.len());
    assert_eq!(
        spans
            .iter()
            .map(|span| span.formula.as_str())
            .collect::<Vec<_>>(),
        vec!["β", "γ + δ"],
    );
}

#[test]
fn exhausted_masks_fail_closed_to_exact_source() {
    let occupied = (1u8..=31)
        .filter(|byte| !byte.is_ascii_whitespace())
        .chain(std::iter::once(127))
        .map(char::from)
        .collect::<String>();
    let source = format!("{occupied} before \\(x\\) after");
    let prepared = PreparedInlineMath::new(&source);

    assert!(prepared.renders_literal());
    let (markdown, mask, spans, _) = prepared.into_parts();
    assert_eq!(
        (&*markdown, mask, spans),
        (source.as_str(), None, Vec::new())
    );
}

#[test]
fn malformed_openers_are_scanned_in_one_pass_and_the_innermost_pair_wins() {
    let source = format!("{}\\(valid\\)", r"\(".repeat(10_000));
    let (_, _, spans) = prepared(&source);

    assert_eq!(
        spans
            .iter()
            .map(|span| span.raw.as_str())
            .collect::<Vec<_>>(),
        vec![r"\(valid\)"],
    );
}

#[test]
fn long_pending_display_math_keeps_exact_source_and_carries_its_start() {
    let prefix = "Before.\n\n";
    let source = format!("{prefix}\\[\n{}", "formula".repeat(10_000));
    let prepared = PreparedInlineMath::new(&source);

    assert_eq!(prepared.pending_display_math_start(), Some(prefix.len()),);
    assert!(!prepared.contains_math());
    let (markdown, mask, spans, _) = prepared.into_parts();
    assert_eq!(&*markdown, source);
    assert_eq!((mask, spans), (None, Vec::new()));
}
