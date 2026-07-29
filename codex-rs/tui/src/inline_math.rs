//! Source-preserving recognition of explicit inline-math delimiters.
//!
//! Pulldown-cmark 0.10 treats the backslashes in `\(...\)` as Markdown punctuation escapes, and
//! TeX punctuation inside math delimiters can create unrelated Markdown events. This module
//! replaces accepted inline source ranges and lexical display-math contents with same-byte-length
//! control-character runs before the final Markdown parse. The writer can then substitute the
//! original source without changing any parser offsets.

use std::borrow::Cow;
use std::ops::Range;

use pulldown_cmark::Event;
use pulldown_cmark::LinkType;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InlineMathSpan {
    pub(crate) source_range: Range<usize>,
    pub(crate) raw: String,
    pub(crate) formula: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedInlineMath<'a> {
    markdown: Cow<'a, str>,
    mask: Option<char>,
    spans: Vec<InlineMathSpan>,
    display_math: Vec<crate::display_math::DisplayMathSpan<'a>>,
    pending_display_math_start: Option<usize>,
    renders_literal: bool,
}

impl<'a> PreparedInlineMath<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        if !source.contains(r"\(") && !source.contains("$$") && !source.contains(r"\[") {
            return Self {
                markdown: Cow::Borrowed(source),
                mask: None,
                spans: Vec::new(),
                display_math: Vec::new(),
                pending_display_math_start: None,
                renders_literal: false,
            };
        }

        let Some(mask) = crate::display_math::unoccupied_mask_byte(source) else {
            return Self {
                markdown: Cow::Borrowed(source),
                mask: None,
                spans: Vec::new(),
                display_math: Vec::new(),
                pending_display_math_start: None,
                renders_literal: true,
            };
        };
        let prepared_display = crate::display_math::PreparedDisplayMath::new(source, mask);
        let (display_markdown, display_math, pending_display_math_start) =
            prepared_display.into_parts();
        let eligible = eligible_text(&display_markdown);
        let ranges = inline_math_ranges(source, &eligible, &display_math);
        if ranges.is_empty() && matches!(display_markdown, Cow::Borrowed(_)) {
            return Self {
                markdown: Cow::Borrowed(source),
                mask: None,
                spans: Vec::new(),
                display_math,
                pending_display_math_start,
                renders_literal: false,
            };
        }

        let mut masked = display_markdown.into_owned().into_bytes();
        let spans = ranges
            .iter()
            .cloned()
            .map(|range| {
                masked[range.clone()].fill(mask);
                let raw = &source[range.clone()];
                InlineMathSpan {
                    source_range: range,
                    raw: raw.to_string(),
                    formula: raw[2..raw.len() - 2].to_string(),
                }
            })
            .collect();

        let Ok(masked) = String::from_utf8(masked) else {
            unreachable!("ASCII inline-math masking must preserve valid UTF-8");
        };
        Self {
            markdown: Cow::Owned(masked),
            mask: Some(char::from(mask)),
            spans,
            display_math,
            pending_display_math_start,
            renders_literal: false,
        }
    }

    pub(crate) fn contains_math(&self) -> bool {
        // Construction borrows the source only when neither accepted display nor inline math
        // required masking. Reuse that result instead of parsing the same Markdown a second time.
        matches!(&self.markdown, Cow::Owned(_))
    }

    pub(crate) fn pending_display_math_start(&self) -> Option<usize> {
        self.pending_display_math_start
    }

    pub(crate) fn renders_literal(&self) -> bool {
        self.renders_literal
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Cow<'a, str>,
        Option<char>,
        Vec<InlineMathSpan>,
        Vec<crate::display_math::DisplayMathSpan<'a>>,
    ) {
        (self.markdown, self.mask, self.spans, self.display_math)
    }
}

fn eligible_text(markdown: &str) -> Vec<Range<usize>> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TABLES);
    let mut text = Vec::new();
    let mut code_block_depth = 0usize;
    let mut autolink_depth = 0usize;
    let mut html_code_depth = 0usize;

    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        match &event {
            Event::Start(Tag::CodeBlock(_)) => code_block_depth += 1,
            Event::Start(Tag::Link {
                link_type: LinkType::Autolink | LinkType::Email,
                ..
            }) => {
                autolink_depth += 1;
            }
            Event::End(pulldown_cmark::TagEnd::CodeBlock) => {
                code_block_depth = code_block_depth.saturating_sub(1);
            }
            Event::End(pulldown_cmark::TagEnd::Link) if autolink_depth > 0 => {
                autolink_depth -= 1;
            }
            Event::InlineHtml(html) => {
                if inline_html_tag(html, "code", /*closing*/ true) {
                    html_code_depth = html_code_depth.saturating_sub(1);
                } else if inline_html_tag(html, "code", /*closing*/ false) {
                    html_code_depth += 1;
                }
            }
            Event::Text(_)
                if code_block_depth == 0 && autolink_depth == 0 && html_code_depth == 0 =>
            {
                text.push(range);
            }
            _ => {}
        }
    }
    text
}

fn inline_html_tag(html: &str, name: &str, closing: bool) -> bool {
    let html = html.trim_start();
    let Some(after_angle) = html.strip_prefix('<') else {
        return false;
    };
    let after_slash = if closing {
        let Some(after_slash) = after_angle.strip_prefix('/') else {
            return false;
        };
        after_slash
    } else {
        if after_angle.starts_with('/') {
            return false;
        }
        after_angle
    };
    let Some(after_name) = after_slash.get(name.len()..) else {
        return false;
    };
    after_slash[..name.len()].eq_ignore_ascii_case(name)
        && after_name
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_whitespace() || character == '>')
        && (closing || !html.trim_end().ends_with("/>"))
}

fn inline_math_ranges(
    source: &str,
    eligible: &[Range<usize>],
    display_math: &[crate::display_math::DisplayMathSpan<'_>],
) -> Vec<Range<usize>> {
    let bytes = source.as_bytes();
    let mut ranges = Vec::new();
    let mut opener = None;
    let mut cursor = 0usize;

    while cursor + 1 < bytes.len() {
        if matches!(bytes[cursor], b'\n' | b'\r') {
            opener = None;
            cursor += 1;
            continue;
        }
        if bytes[cursor] != b'\\'
            || !matches!(bytes[cursor + 1], b'(' | b')')
            || delimiter_is_escaped(bytes, cursor)
        {
            cursor += 1;
            continue;
        }

        match bytes[cursor + 1] {
            b'(' => {
                if position_is_eligible(eligible, cursor + 1) {
                    // A nested opener restarts recognition at the innermost explicit pair. This
                    // keeps malformed prefixes from repeatedly rescanning a streaming line.
                    opener = Some(cursor);
                }
            }
            b')' => {
                if let Some(start) = opener.take() {
                    let range = start..cursor + 2;
                    if !source[start + 2..cursor].trim().is_empty()
                        && position_is_eligible(eligible, cursor + 1)
                        && !overlaps_any(range.clone(), display_math)
                    {
                        ranges.push(range);
                    }
                }
            }
            _ => unreachable!("the delimiter byte was checked above"),
        }
        cursor += 2;
    }

    ranges
}

fn position_is_eligible(ranges: &[Range<usize>], position: usize) -> bool {
    let index = ranges.partition_point(|range| range.end <= position);
    ranges
        .get(index)
        .is_some_and(|range| range.contains(&position))
}

fn delimiter_is_escaped(bytes: &[u8], delimiter_start: usize) -> bool {
    bytes[..delimiter_start]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}

fn overlaps_any(
    candidate: Range<usize>,
    spans: &[crate::display_math::DisplayMathSpan<'_>],
) -> bool {
    let index = spans.partition_point(|span| span.source_range.end <= candidate.start);
    spans
        .get(index)
        .is_some_and(|span| span.source_range.start < candidate.end)
}

#[cfg(test)]
#[path = "inline_math_tests.rs"]
mod tests;
