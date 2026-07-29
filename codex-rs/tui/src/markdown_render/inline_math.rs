//! Ordered substitution of masked inline-math spans during Markdown rendering.

use std::collections::VecDeque;
use std::ops::Range;

use crate::inline_math::InlineMathSpan;

pub(super) enum InlineMathText<'a> {
    Plain(&'a str),
    Segmented(Vec<InlineMathSegment<'a>>),
    SourceLiteral(String),
}

pub(super) enum InlineMathSegment<'a> {
    Text(&'a str),
    Math(InlineMathSpan),
}

pub(super) struct InlineMathCursor<'a> {
    source: &'a str,
    mask: Option<char>,
    spans: VecDeque<InlineMathSpan>,
    failed: bool,
}

impl<'a> InlineMathCursor<'a> {
    pub(super) fn new(source: &'a str, mask: Option<char>, spans: Vec<InlineMathSpan>) -> Self {
        Self {
            source,
            mask,
            spans: spans.into(),
            failed: false,
        }
    }

    pub(super) fn consume<'text>(
        &mut self,
        text: &'text str,
        source_range: Range<usize>,
    ) -> InlineMathText<'text> {
        let Some(mask) = self.mask.filter(|mask| text.contains(*mask)) else {
            return InlineMathText::Plain(text);
        };
        if self.failed {
            return InlineMathText::SourceLiteral(self.source_literal(source_range));
        }

        let Some(plan) = self.plan(text, source_range.clone(), mask) else {
            self.failed = true;
            return InlineMathText::SourceLiteral(self.source_literal(source_range));
        };

        let mut segments = Vec::with_capacity(plan.len().saturating_mul(2) + 1);
        let mut text_start = 0usize;
        for run in plan {
            if text_start < run.start {
                segments.push(InlineMathSegment::Text(&text[text_start..run.start]));
            }
            let mut substitution_start = run.start;
            for _ in 0..run.substitution_count {
                let Some(span) = self.spans.pop_front() else {
                    unreachable!("inline-math substitutions were validated before substitution");
                };
                let substitution_end = substitution_start + span.source_range.len();
                segments.push(InlineMathSegment::Math(span));
                substitution_start = substitution_end;
            }
            debug_assert_eq!(substitution_start, run.end);
            text_start = run.end;
        }
        if text_start < text.len() {
            segments.push(InlineMathSegment::Text(&text[text_start..]));
        }
        InlineMathText::Segmented(segments)
    }

    pub(super) fn discard(&mut self, text: &str, source_range: Range<usize>) {
        let Some(mask) = self.mask.filter(|mask| text.contains(*mask)) else {
            return;
        };
        if self.failed {
            return;
        }
        let Some(plan) = self.plan(text, source_range, mask) else {
            self.failed = true;
            return;
        };
        let substitution_count = plan.iter().map(|run| run.substitution_count).sum::<usize>();
        self.spans.drain(..substitution_count);
    }

    pub(super) fn is_complete(&self) -> bool {
        self.failed || self.spans.is_empty()
    }

    fn plan(&self, text: &str, source_range: Range<usize>, mask: char) -> Option<Vec<MaskRun>> {
        let mut plan = Vec::new();
        let mut pending = self.spans.iter();
        let mut remainder = text;
        let mut offset = 0usize;

        while let Some(relative_start) = remainder.find(mask) {
            let start = offset + relative_start;
            let run_len = remainder[relative_start..]
                .chars()
                .take_while(|character| *character == mask)
                .map(char::len_utf8)
                .sum::<usize>();
            let mut matched_len = 0usize;
            let mut substitution_count = 0usize;
            while matched_len < run_len {
                let span = pending.next()?;
                if !source_range_contains(&source_range, &span.source_range)
                    || self.source.get(span.source_range.clone()) != Some(span.raw.as_str())
                {
                    return None;
                }
                matched_len = matched_len.checked_add(span.source_range.len())?;
                substitution_count += 1;
            }
            if matched_len != run_len {
                return None;
            }
            plan.push(MaskRun {
                start,
                end: start + run_len,
                substitution_count,
            });
            offset = start + run_len;
            remainder = &text[offset..];
        }

        if pending
            .next()
            .is_some_and(|span| source_range_contains(&source_range, &span.source_range))
        {
            return None;
        }
        Some(plan)
    }

    fn source_literal(&self, source_range: Range<usize>) -> String {
        self.source
            .get(source_range)
            .map(str::to_string)
            .unwrap_or_else(|| self.source.to_string())
    }
}

fn source_range_contains(event: &Range<usize>, span: &Range<usize>) -> bool {
    event.start <= span.start && span.end <= event.end
}

struct MaskRun {
    start: usize,
    end: usize,
    substitution_count: usize,
}

#[cfg(test)]
#[path = "inline_math_tests.rs"]
mod tests;
