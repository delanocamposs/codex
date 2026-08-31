//! Ordered substitution of masked inline-math spans during Markdown rendering.

use std::ops::Range;

use crate::math_source::InlineMathSpan;

pub(super) enum InlineMathText<'text, 'source> {
    Plain(&'text str),
    Segmented(Vec<InlineMathSegment<'text, 'source>>),
    SourceLiteral(&'source str),
}

pub(super) enum InlineMathSegment<'text, 'source> {
    Text(&'text str),
    Math {
        raw: &'source str,
        formula: &'source str,
    },
}

pub(super) struct InlineMathCursor<'a> {
    source: &'a str,
    spans: Vec<InlineMathSpan>,
    next: usize,
}

impl<'a> InlineMathCursor<'a> {
    pub(super) fn new(source: &'a str, spans: Vec<InlineMathSpan>) -> Self {
        Self {
            source,
            spans,
            next: 0,
        }
    }

    pub(super) fn consume<'text>(
        &mut self,
        text: &'text str,
        source_range: Range<usize>,
    ) -> InlineMathText<'text, 'a> {
        self.skip_spans_before(source_range.start);
        if self
            .spans
            .get(self.next)
            .is_none_or(|span| span.source_range.start >= source_range.end)
        {
            return InlineMathText::Plain(text);
        }
        let Some(plan) = self.plan(text, &source_range) else {
            self.skip_spans_through(source_range.end);
            return InlineMathText::SourceLiteral(self.source_literal(source_range));
        };

        let mut segments = Vec::with_capacity(plan.len().saturating_mul(2) + 1);
        let mut text_start = 0usize;
        for run in plan {
            if text_start < run.start {
                segments.push(InlineMathSegment::Text(&text[text_start..run.start]));
            }
            let span = &self.spans[self.next];
            self.next += 1;
            debug_assert_eq!(run.start + span.source_range.len(), run.end);
            let raw = &self.source[span.source_range.clone()];
            segments.push(InlineMathSegment::Math {
                raw,
                formula: &raw[2..raw.len() - 2],
            });
            text_start = run.end;
        }
        if text_start < text.len() {
            segments.push(InlineMathSegment::Text(&text[text_start..]));
        }
        InlineMathText::Segmented(segments)
    }

    pub(super) fn discard(&mut self, text: &str, source_range: Range<usize>) {
        self.skip_spans_before(source_range.start);
        if self
            .spans
            .get(self.next)
            .is_none_or(|span| span.source_range.start >= source_range.end)
        {
            return;
        }
        let Some(plan) = self.plan(text, &source_range) else {
            self.skip_spans_through(source_range.end);
            return;
        };
        self.next += plan.len();
    }

    pub(super) fn is_complete(&self) -> bool {
        self.next == self.spans.len()
    }

    fn skip_spans_before(&mut self, offset: usize) {
        let mut skipped = false;
        while self
            .spans
            .get(self.next)
            .is_some_and(|span| span.source_range.end <= offset)
        {
            self.next += 1;
            skipped = true;
        }
        debug_assert!(
            !skipped,
            "every parser-accepted inline formula must reach a text event"
        );
    }

    fn skip_spans_through(&mut self, offset: usize) {
        while self
            .spans
            .get(self.next)
            .is_some_and(|span| span.source_range.start < offset)
        {
            self.next += 1;
        }
    }

    fn plan(&self, text: &str, source_range: &Range<usize>) -> Option<Vec<MaskRun>> {
        let mut plan = Vec::new();
        let mut pending = self.spans[self.next..].iter();
        let mut remainder = text;
        let mut offset = 0usize;

        for span in pending.by_ref() {
            if span.source_range.start >= source_range.end {
                break;
            }
            if !source_range_contains(source_range, &span.source_range) {
                return None;
            }
            let relative_start = remainder.find(span.marker)?;
            let start = offset + relative_start;
            let end = start.checked_add(span.source_range.len())?;
            let masked = text.get(start..end)?;
            let mut characters = masked.chars();
            if characters.next() != Some(span.marker)
                || !characters.all(|character| character == 'M')
            {
                return None;
            }
            plan.push(MaskRun { start, end });
            offset = end;
            remainder = &text[offset..];
        }
        Some(plan)
    }

    fn source_literal(&self, source_range: Range<usize>) -> &'a str {
        self.source.get(source_range).unwrap_or(self.source)
    }
}

fn source_range_contains(event: &Range<usize>, span: &Range<usize>) -> bool {
    event.start <= span.start && span.end <= event.end
}

struct MaskRun {
    start: usize,
    end: usize,
}

#[cfg(test)]
#[path = "inline_math_tests.rs"]
mod tests;
