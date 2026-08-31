use pretty_assertions::assert_eq;

use super::InlineMathCursor;
use super::InlineMathSegment;
use super::InlineMathText;
use crate::math_source::InlineMathSpan;

const MASK: char = '\u{F0000}';
const SECOND_MASK: char = '\u{F0001}';

fn masked(raw: &str, marker: char) -> String {
    format!("{marker}{}", "M".repeat(raw.len() - marker.len_utf8()))
}

#[test]
fn substitutes_validated_spans_in_order() {
    let source = r"before \(x\) after";
    let raw = r"\(x\)";
    let start = source.find(raw).expect("fixture contains inline math");
    let mut cursor = InlineMathCursor::new(
        source,
        vec![InlineMathSpan {
            source_range: start..start + raw.len(),
            marker: MASK,
        }],
    );
    let masked = format!("before {} after", masked(raw, MASK));

    let InlineMathText::Segmented(segments) = cursor.consume(&masked, 0..source.len()) else {
        panic!("valid masks should be segmented");
    };
    let rendered = segments
        .into_iter()
        .map(|segment| match segment {
            InlineMathSegment::Text(text) => text.to_string(),
            InlineMathSegment::Math { formula, .. } => format!("<{formula}>"),
        })
        .collect::<String>();

    assert_eq!(rendered, "before <x> after");
    assert!(cursor.is_complete());
}

#[test]
fn a_mismatched_mask_only_falls_back_for_its_source_event() {
    let source = r"\(x\) \(y\)";
    let first = r"\(x\)";
    let second = r"\(y\)";
    let second_start = source
        .find(second)
        .expect("fixture contains second formula");
    let mut cursor = InlineMathCursor::new(
        source,
        vec![
            InlineMathSpan {
                source_range: 0..first.len(),
                marker: MASK,
            },
            InlineMathSpan {
                source_range: second_start..second_start + second.len(),
                marker: SECOND_MASK,
            },
        ],
    );

    let InlineMathText::SourceLiteral(literal) = cursor.consume(&MASK.to_string(), 0..first.len())
    else {
        panic!("a mismatched mask should fail closed");
    };
    assert_eq!(literal, first);

    let second_masked = masked(second, SECOND_MASK);
    let InlineMathText::Segmented(segments) =
        cursor.consume(&second_masked, second_start..second_start + second.len())
    else {
        panic!("a later valid mask should still be segmented");
    };
    assert_eq!(
        segments
            .into_iter()
            .map(|segment| match segment {
                InlineMathSegment::Text(text) => text.to_string(),
                InlineMathSegment::Math { formula, .. } => formula.to_string(),
            })
            .collect::<String>(),
        "y",
    );
    assert!(cursor.is_complete());
}
