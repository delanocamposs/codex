use pretty_assertions::assert_eq;
use pulldown_cmark::Event;
use pulldown_cmark::Parser;

use super::*;

type PreparedParts = (String, Option<char>, Vec<InlineMathSpan>);

fn prepared(source: &str) -> PreparedParts {
    let (markdown, mask, spans) = PreparedInlineMath::new(source).into_parts();
    (markdown.into_owned(), mask, spans)
}

fn raw_and_formula(spans: &[InlineMathSpan]) -> Vec<(String, String)> {
    spans
        .iter()
        .map(|span| (span.raw.clone(), span.formula.clone()))
        .collect()
}

#[test]
fn masks_balanced_explicit_inline_math_without_changing_byte_offsets() {
    let source = r"Given \(x\in\ker f''\), use \(\alpha_i * y\).";
    let (masked, mask, spans) = prepared(source);

    assert_eq!(masked.len(), source.len());
    assert_eq!(
        raw_and_formula(&spans),
        vec![
            (r"\(x\in\ker f''\)".to_string(), r"x\in\ker f''".to_string()),
            (r"\(\alpha_i * y\)".to_string(), r"\alpha_i * y".to_string()),
        ],
    );
    assert_eq!(
        masked
            .chars()
            .filter(|character| Some(*character) == mask)
            .count(),
        spans.iter().map(|span| span.raw.len()).sum::<usize>(),
    );
}

#[test]
fn masked_tex_punctuation_stays_in_plain_text_events() {
    let source = r"before \(\mathbf{x} * [y](z)\) after";
    let (markdown, mask, _) = prepared(source);
    let events = Parser::new(&markdown).collect::<Vec<_>>();

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, Event::Start(Tag::Emphasis | Tag::Link { .. }))),
        "{events:?}"
    );
    assert!(events.iter().any(
        |event| matches!(event, Event::Text(text) if mask.is_some_and(|mask| text.contains(mask)))
    ));
}

#[test]
fn display_math_contents_cannot_become_markdown_structure() {
    let source = "\\[\n\\text{fluid acceleration}\n=\n\\text{pressure forces}\n\\]";
    let (markdown, _, spans) = prepared(source);
    let events = Parser::new(&markdown).collect::<Vec<_>>();

    assert!(
        events
            .iter()
            .all(|event| !matches!(event, Event::Start(Tag::Heading { .. }))),
        "{events:?}"
    );
    assert_eq!(spans, Vec::new());
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
        r"<CODE class='language-tex'>\(code\)</CODE>",
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
fn occupied_primary_masks_fall_back_to_another_absent_byte() {
    let source = format!(
        "before {}{}{}{} and \\(x\\)",
        char::from(0x1c),
        char::from(0x1d),
        char::from(0x1e),
        char::from(0x1f),
    );
    let prepared = PreparedInlineMath::new(&source);
    assert!(prepared.contains_math());
    let (_, mask, spans) = prepared.into_parts();

    assert_eq!(spans[0].raw, r"\(x\)");
    assert!(
        mask.is_some_and(|mask| !['\u{001c}', '\u{001d}', '\u{001e}', '\u{001f}'].contains(&mask))
    );
}

#[test]
fn exhausted_control_masks_disable_math_rendering() {
    let occupied_masks = (1u8..=31)
        .filter(|byte| !byte.is_ascii_whitespace())
        .chain(std::iter::once(127))
        .map(char::from)
        .collect::<String>();
    let source = format!("{occupied_masks} before \\(x\\) after");
    let prepared = PreparedInlineMath::new(&source);

    assert!(prepared.renders_literal());
    let (markdown, mask, spans) = prepared.into_parts();
    assert_eq!(&*markdown, source);
    assert_eq!((mask, spans), (None, Vec::new()));
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
    let (markdown, mask, spans) = prepared.into_parts();
    assert_eq!(&*markdown, source);
    assert_eq!((mask, spans), (None, Vec::new()));
}
