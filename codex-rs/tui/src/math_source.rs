//! Source-ordered recognition and masking of explicit TeX math.
//!
//! A small lexer gives Markdown code and explicit math delimiters mutual ownership: while one is
//! open, punctuation belonging to the other is opaque. Accepted math is then replaced by
//! equal-width bytes so the normal CommonMark parse cannot reinterpret TeX as Markdown.

use std::borrow::Cow;
use std::collections::HashMap;
use std::collections::HashSet;
use std::ops::Range;

#[path = "math_source/lexer.rs"]
mod lexer;

pub(crate) fn may_contain_math(source: &str) -> bool {
    source.contains(r"\(") || source.contains(r"\[") || source.contains("$$")
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InlineMathSpan {
    pub(crate) source_range: Range<usize>,
    pub(crate) marker: char,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DisplayMathSpan {
    pub(crate) source_range: Range<usize>,
    pub(crate) formula: String,
    quote_depth: usize,
    body_indent: usize,
}

impl DisplayMathSpan {
    pub(crate) fn literal(&self, source: &str) -> String {
        let Some(raw) = source.get(self.source_range.clone()) else {
            debug_assert!(false, "display math ranges originate from this source");
            return String::new();
        };
        let mut lines = lexer::source_lines(raw);
        let Some(first) = lines.next() else {
            debug_assert!(false, "a display math range contains its delimiters");
            return String::new();
        };
        let mut literal = first.text.to_string();
        for line in lines {
            let (content, quote_prefix_len, _) =
                lexer::strip_quote_prefixes(line.text, self.quote_depth);
            let (content, _) = lexer::strip_indent(content, quote_prefix_len, self.body_indent);
            literal.push('\n');
            literal.push_str(content);
        }
        literal
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DisplayMathDelimiter {
    Bracket,
    Dollar,
}

impl DisplayMathDelimiter {
    pub(crate) fn opener(self) -> &'static str {
        match self {
            Self::Bracket => r"\[",
            Self::Dollar => "$$",
        }
    }

    pub(crate) fn closer(self) -> &'static str {
        match self {
            Self::Bracket => r"\]",
            Self::Dollar => "$$",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedMath<'a> {
    source: Cow<'a, str>,
    markdown: Cow<'a, str>,
    inline: Vec<InlineMathSpan>,
    display: Vec<DisplayMathSpan>,
    pending_display_start: Option<usize>,
    renders_literal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SourceEdit {
    pub(crate) range: Range<usize>,
    pub(crate) replacement: String,
}

pub(crate) struct PreparedMathParts<'a> {
    pub(crate) source: Cow<'a, str>,
    pub(crate) markdown: Cow<'a, str>,
    pub(crate) inline: Vec<InlineMathSpan>,
    pub(crate) display: Vec<DisplayMathSpan>,
}

impl<'a> PreparedMath<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        if !may_contain_math(source) {
            return Self::unchanged(source);
        }
        let lexer::ParsedMath {
            inline,
            display,
            pending_display_start,
            renders_literal,
        } = lexer::parse(source);
        if renders_literal {
            return Self {
                renders_literal: true,
                ..Self::unchanged(source)
            };
        }
        if inline.is_empty() && display.is_empty() {
            return Self {
                pending_display_start,
                ..Self::unchanged(source)
            };
        }
        let inline = assign_inline_markers(source, inline);
        if inline.is_empty() && display.is_empty() {
            return Self {
                pending_display_start,
                ..Self::unchanged(source)
            };
        }

        Self {
            source: Cow::Borrowed(source),
            markdown: Cow::Owned(masked_source(source, &inline, &display)),
            inline,
            display,
            pending_display_start,
            renders_literal: false,
        }
    }

    fn unchanged(source: &'a str) -> Self {
        Self {
            source: Cow::Borrowed(source),
            markdown: Cow::Borrowed(source),
            inline: Vec::new(),
            display: Vec::new(),
            pending_display_start: None,
            renders_literal: false,
        }
    }

    pub(crate) fn pending_display_start(&self) -> Option<usize> {
        self.pending_display_start
    }

    pub(crate) fn has_math(&self) -> bool {
        !self.inline.is_empty() || !self.display.is_empty()
    }

    pub(crate) fn renders_literal(&self) -> bool {
        self.renders_literal
    }

    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    pub(crate) fn markdown(&self) -> &str {
        &self.markdown
    }

    /// Apply edits discovered from [`Self::markdown`] without recognizing math again.
    ///
    /// Callers must derive edits from the masked view. This keeps accepted math opaque to later
    /// Markdown normalizers while remapping its source ranges into the normalized source.
    pub(crate) fn apply_edits(mut self, edits: &[SourceEdit]) -> Self {
        if edits.is_empty() {
            return self;
        }
        let Some(edit_map) = EditMap::new(self.source(), edits) else {
            debug_assert!(false, "normalizer edits must be valid source ranges");
            return self;
        };
        let Some(inline_ranges) = self
            .inline
            .iter()
            .map(|span| edit_map.map_range(&span.source_range))
            .collect::<Option<Vec<_>>>()
        else {
            debug_assert!(
                false,
                "normalizer edits must not overlap accepted inline math"
            );
            return self;
        };
        let Some(display_ranges) = self
            .display
            .iter()
            .map(|span| edit_map.map_range(&span.source_range))
            .collect::<Option<Vec<_>>>()
        else {
            debug_assert!(
                false,
                "normalizer edits must not overlap accepted display math"
            );
            // Keep the authoritative parse rather than demoting the entire message to literal
            // text because an optional normalizer produced unusable coordinates.
            return self;
        };

        for (span, source_range) in self.inline.iter_mut().zip(inline_ranges) {
            span.source_range = source_range;
        }
        for (span, source_range) in self.display.iter_mut().zip(display_ranges) {
            span.source_range = source_range;
        }
        self.pending_display_start = self
            .pending_display_start
            .and_then(|offset| edit_map.map_offset(offset));

        let source = apply_source_edits(self.source(), edits);
        if self.has_math() {
            self.markdown = Cow::Owned(masked_source(&source, &self.inline, &self.display));
        } else {
            self.markdown = Cow::Owned(source.clone());
        }
        self.source = Cow::Owned(source);
        self
    }

    pub(crate) fn into_parts(self) -> PreparedMathParts<'a> {
        PreparedMathParts {
            source: self.source,
            markdown: self.markdown,
            inline: self.inline,
            display: self.display,
        }
    }
}

fn valid_edits(source: &str, edits: &[SourceEdit]) -> bool {
    let mut previous_end = 0;
    edits.iter().all(|edit| {
        let valid = previous_end <= edit.range.start
            && edit.range.start <= edit.range.end
            && edit.range.end <= source.len()
            && source.is_char_boundary(edit.range.start)
            && source.is_char_boundary(edit.range.end);
        previous_end = edit.range.end;
        valid
    })
}

struct EditMap {
    entries: Vec<MappedEdit>,
}

struct MappedEdit {
    source_range: Range<usize>,
    output_end: usize,
}

impl EditMap {
    fn new(source: &str, edits: &[SourceEdit]) -> Option<Self> {
        valid_edits(source, edits).then_some(())?;
        let mut entries = Vec::with_capacity(edits.len());
        let mut source_end = 0;
        let mut output_end = 0usize;
        for edit in edits {
            output_end = output_end
                .checked_add(edit.range.start - source_end)?
                .checked_add(edit.replacement.len())?;
            source_end = edit.range.end;
            entries.push(MappedEdit {
                source_range: edit.range.clone(),
                output_end,
            });
        }
        Some(Self { entries })
    }

    fn map_range(&self, range: &Range<usize>) -> Option<Range<usize>> {
        let next = self
            .entries
            .partition_point(|edit| edit.source_range.end <= range.start);
        if self.entries.get(next).is_some_and(|edit| {
            range.start < edit.source_range.end && edit.source_range.start < range.end
        }) {
            return None;
        }
        Some(self.map_offset(range.start)?..self.map_offset(range.end)?)
    }

    fn map_offset(&self, offset: usize) -> Option<usize> {
        let preceding = self
            .entries
            .partition_point(|edit| edit.source_range.start < offset);
        let Some(edit) = preceding
            .checked_sub(1)
            .and_then(|index| self.entries.get(index))
        else {
            return Some(offset);
        };
        if offset < edit.source_range.end {
            return None;
        }
        edit.output_end.checked_add(offset - edit.source_range.end)
    }
}

fn apply_source_edits(source: &str, edits: &[SourceEdit]) -> String {
    let output_len = edits.iter().fold(source.len(), |len, edit| {
        len + edit.replacement.len() - edit.range.len()
    });
    let mut output = String::with_capacity(output_len);
    let mut copied_until = 0;
    for edit in edits {
        output.push_str(&source[copied_until..edit.range.start]);
        output.push_str(&edit.replacement);
        copied_until = edit.range.end;
    }
    output.push_str(&source[copied_until..]);
    output
}

fn masked_source(source: &str, inline: &[InlineMathSpan], display: &[DisplayMathSpan]) -> String {
    let mut markdown = source.as_bytes().to_vec();
    for span in inline {
        let mut marker_bytes = [0; 4];
        let marker_bytes = span.marker.encode_utf8(&mut marker_bytes).as_bytes();
        let start = span.source_range.start;
        markdown[start..start + marker_bytes.len()].copy_from_slice(marker_bytes);
        markdown[start + marker_bytes.len()..span.source_range.end].fill(b'M');
    }
    for span in display {
        markdown[span.source_range.start..span.source_range.start + 2].fill(b'M');
        for range in display_payload_ranges(source, span) {
            markdown[range].fill(b'M');
        }
        markdown[span.source_range.end - 2..span.source_range.end].fill(b'M');
    }
    String::from_utf8(markdown).unwrap_or_else(|error| {
        debug_assert!(false, "ASCII masking preserves valid UTF-8: {error}");
        String::from_utf8_lossy(error.as_bytes()).into_owned()
    })
}

fn display_payload_ranges<'a>(
    source: &'a str,
    span: &'a DisplayMathSpan,
) -> impl Iterator<Item = Range<usize>> + 'a {
    let body_start = span.source_range.start + 2;
    let body_end = span.source_range.end - 2;
    let mut first_line = true;

    lexer::source_lines(&source[body_start..body_end]).filter_map(move |line| {
        let absolute_start = body_start + line.start;
        let payload_start = if first_line {
            first_line = false;
            absolute_start
        } else {
            let (after_quotes, quote_prefix_len, _) =
                lexer::strip_quote_prefixes(line.text, span.quote_depth);
            let (_, indentation) =
                lexer::strip_indent(after_quotes, quote_prefix_len, span.body_indent);
            absolute_start + quote_prefix_len + indentation
        };
        let payload_end = absolute_start + line.text.len();
        (payload_start < payload_end).then_some(payload_start..payload_end)
    })
}

pub(super) fn display_formula(
    source: &str,
    range: &Range<usize>,
    quote_depth: usize,
    body_indent: usize,
) -> String {
    let body_start = range.start + 2;
    let body_end = range.end - 2;
    let mut formula = String::with_capacity(body_end - body_start);
    let mut pending_blank_lines = 0;
    for line in lexer::source_lines(&source[body_start..body_end]) {
        let (content, quote_prefix_len, _) = lexer::strip_quote_prefixes(line.text, quote_depth);
        let content = lexer::strip_indent(content, quote_prefix_len, body_indent).0;
        if content.trim().is_empty() {
            if !formula.is_empty() {
                pending_blank_lines += 1;
            }
            continue;
        }
        if !formula.is_empty() {
            for _ in 0..=pending_blank_lines {
                formula.push('\n');
            }
        }
        pending_blank_lines = 0;
        formula.push_str(content);
    }
    formula
}

fn assign_inline_markers(source: &str, ranges: Vec<Range<usize>>) -> Vec<InlineMathSpan> {
    // A four-byte private-use scalar is ordinary CommonMark text, preserves byte offsets when
    // followed by ASCII filler, and cannot collide with ordinary prose. Distinct raw formulas get
    // distinct markers so masking cannot make unrelated reference labels compare equal.
    let mut occupied = source.chars().collect::<HashSet<_>>();
    for entity in source.split("&#").skip(1) {
        let (radix, digits) = entity
            .strip_prefix(['x', 'X'])
            .map_or((10, entity), |digits| (16, digits));
        if let Some((digits, _)) = digits.split_once(';')
            && let Ok(value) = u32::from_str_radix(digits, radix)
            && let Some(character) = char::from_u32(value)
        {
            occupied.insert(character);
        }
    }
    let mut available = (0xF_0000..=0xF_FFFD)
        .chain(0x10_0000..=0x10_FFFD)
        .filter_map(char::from_u32)
        .filter(|character| !occupied.contains(character));
    let mut assigned = HashMap::<&str, char>::new();
    ranges
        .into_iter()
        .filter_map(|source_range| {
            let raw = &source[source_range.clone()];
            let marker = assigned.get(raw).copied().or_else(|| {
                available.next().inspect(|marker| {
                    assigned.insert(raw, *marker);
                })
            })?;
            Some(InlineMathSpan {
                source_range,
                marker,
            })
        })
        .collect()
}

#[cfg(test)]
#[path = "math_source_tests.rs"]
mod tests;
