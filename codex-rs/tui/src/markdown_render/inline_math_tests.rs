use pretty_assertions::assert_eq;

use super::InlineMathCursor;
use super::InlineMathSegment;
use super::InlineMathText;
use crate::inline_math::InlineMathSpan;

#[test]
fn substitutes_validated_spans_in_order() {
    let source = r"before \(x\) after";
    let raw = r"\(x\)";
    let start = source.find(raw).expect("fixture contains inline math");
    let mut cursor = InlineMathCursor::new(
        source,
        Some('\u{1}'),
        vec![InlineMathSpan {
            source_range: start..start + raw.len(),
            raw: raw.to_string(),
            formula: "x".to_string(),
        }],
    );
    let masked = format!("before {} after", "\u{1}".repeat(raw.len()));

    let InlineMathText::Segmented(segments) = cursor.consume(&masked, 0..source.len()) else {
        panic!("valid masks should be segmented");
    };
    let rendered = segments
        .into_iter()
        .map(|segment| match segment {
            InlineMathSegment::Text(text) => text.to_string(),
            InlineMathSegment::Math(span) => format!("<{}>", span.formula),
        })
        .collect::<String>();

    assert_eq!(rendered, "before <x> after");
    assert!(cursor.is_complete());
}

#[test]
fn mismatched_masks_fall_back_to_exact_source() {
    let source = r"before \(x\) after";
    let raw = r"\(x\)";
    let start = source.find(raw).expect("fixture contains inline math");
    let mut cursor = InlineMathCursor::new(
        source,
        Some('\u{1}'),
        vec![InlineMathSpan {
            source_range: start..start + raw.len(),
            raw: raw.to_string(),
            formula: "x".to_string(),
        }],
    );
    let malformed_mask = format!("before {} after", "\u{1}".repeat(raw.len() - 1));

    let InlineMathText::SourceLiteral(literal) = cursor.consume(&malformed_mask, 0..source.len())
    else {
        panic!("a mismatched mask should fail closed");
    };

    assert_eq!(literal, source);
    assert!(cursor.is_complete());
}
