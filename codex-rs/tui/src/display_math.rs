//! Recognition of standalone display-math paragraphs in Markdown source.

use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use pulldown_cmark::TagEnd;
use std::borrow::Cow;
use std::ops::Range;

/// An accepted display-math block backed by ranges in the original Markdown source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DisplayMathSpan<'a> {
    pub(crate) source_range: Range<usize>,
    pub(crate) raw_block: &'a str,
    pub(crate) formula: &'a str,
}

/// Markdown with the contents of lexical display-math candidates neutralized.
///
/// CommonMark can otherwise interpret TeX lines as Markdown structure before
/// the final renderer sees them. In particular, a formula line containing only `=` turns the
/// preceding line into a Setext heading. A parse of the unmodified source first excludes candidates
/// in block quotes, code blocks, tables, and HTML. A masked discovery parse then accepts complete
/// top-level paragraphs and paragraphs whose only ancestors are list/item containers, and only
/// those formulas remain masked in the result.
pub(crate) struct PreparedDisplayMath<'a> {
    markdown: Cow<'a, str>,
    spans: Vec<DisplayMathSpan<'a>>,
    pending_start: Option<usize>,
}

impl<'a> PreparedDisplayMath<'a> {
    pub(crate) fn new(source: &'a str, mask: u8) -> Self {
        let (candidates, pending) = display_math_candidates(source);
        let pending_start = pending
            .as_ref()
            .and_then(|candidate| pending_display_math_start(source, candidate, mask));
        let unchanged = || Self {
            markdown: Cow::Borrowed(source),
            spans: Vec::new(),
            pending_start,
        };
        if candidates.is_empty() {
            return unchanged();
        }
        let probe_candidates = eligible_text_candidates(source, &candidates);
        if probe_candidates.is_empty() {
            return unchanged();
        }

        let mut probe = source.as_bytes().to_vec();
        for candidate in &probe_candidates {
            mask_formula(&mut probe, candidate.formula_range.clone(), mask);
        }
        let Ok(probe) = String::from_utf8(probe) else {
            unreachable!("ASCII display-math masking must preserve valid UTF-8");
        };
        let accepted = accepted_display_math_candidates(source, &probe, &probe_candidates);
        if accepted.is_empty() {
            return unchanged();
        }

        let mut markdown = source.as_bytes().to_vec();
        let mut spans = Vec::with_capacity(accepted.len());
        for candidate in accepted {
            mask_formula(&mut markdown, candidate.formula_range.clone(), mask);
            spans.push(DisplayMathSpan {
                source_range: candidate.block_range.clone(),
                raw_block: &source[candidate.block_range.clone()],
                formula: &source[candidate.formula_range.clone()],
            });
        }
        let Ok(markdown) = String::from_utf8(markdown) else {
            unreachable!("ASCII display-math masking must preserve valid UTF-8");
        };
        Self {
            markdown: Cow::Owned(markdown),
            spans,
            pending_start,
        }
    }

    pub(crate) fn into_parts(self) -> (Cow<'a, str>, Vec<DisplayMathSpan<'a>>, Option<usize>) {
        (self.markdown, self.spans, self.pending_start)
    }
}

fn pending_display_math_start(
    source: &str,
    candidate: &PendingDisplayMathCandidate,
    mask: u8,
) -> Option<usize> {
    if candidate.has_content_after_blank_line {
        return None;
    }

    // Only the source through the opener can affect whether it starts a top-level paragraph.
    // Add one neutral formula byte after the original first line ending, when present, so the
    // structural probe stays bounded by the opener rather than cloning a growing pending formula.
    let mut markdown = source.as_bytes()[..candidate.formula_start].to_vec();
    match source.as_bytes().get(candidate.formula_start..) {
        Some([b'\r', b'\n', ..]) => markdown.extend_from_slice(b"\r\n"),
        Some([line_ending @ (b'\r' | b'\n'), ..]) => markdown.push(*line_ending),
        Some(_) | None => {}
    }
    markdown.push(mask);
    let Ok(markdown) = String::from_utf8(markdown) else {
        unreachable!("ASCII display-math masking must preserve valid UTF-8");
    };
    let mut ancestors = Vec::new();

    for (event, range) in Parser::new_ext(&markdown, markdown_options()).into_offset_iter() {
        if let Some(container_range) = display_math_container(&event, &range, &ancestors)
            && range.start < candidate.formula_start
            && candidate.delimiter_start < range.end
            && (ancestors.is_empty()
                && source[range.start..candidate.delimiter_start]
                    .trim()
                    .is_empty()
                || !ancestors.is_empty()
                    && container_range.start <= candidate.delimiter_start
                    && candidate.delimiter_start < container_range.end)
        {
            return Some(range.start.min(candidate.block_start));
        }
        update_ancestors(&event, &range, &mut ancestors);
    }

    None
}

fn mask_candidates() -> impl Iterator<Item = u8> {
    (1u8..=31)
        .filter(|candidate| !candidate.is_ascii_whitespace())
        .chain(std::iter::once(127))
}

fn markdown_options() -> Options {
    Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES
}

pub(crate) fn unoccupied_mask_byte(source: &str) -> Option<u8> {
    let mut occupied = [false; 128];
    for byte in source.bytes().filter(u8::is_ascii) {
        occupied[usize::from(byte)] = true;
    }
    mask_candidates().find(|candidate| !occupied[usize::from(*candidate)])
}

struct DisplayMathCandidate {
    block_range: Range<usize>,
    formula_range: Range<usize>,
}

struct PendingDisplayMathCandidate {
    block_start: usize,
    delimiter_start: usize,
    formula_start: usize,
    has_content_after_blank_line: bool,
}

#[derive(Clone, Copy)]
struct SourceLine<'a> {
    start: usize,
    text: &'a str,
}

struct OpenDisplayMath {
    block_start: usize,
    delimiter_start: usize,
    formula_start: usize,
    following_start: usize,
    opener: &'static str,
    closer: &'static str,
}

fn display_math_candidates(
    source: &str,
) -> (
    Vec<DisplayMathCandidate>,
    Option<PendingDisplayMathCandidate>,
) {
    let mut complete = Vec::new();
    let mut open: Option<OpenDisplayMath> = None;
    let mut lines = source_lines(source, 0);

    while let Some(line) = lines.next() {
        let parsed = display_line(line);
        if let Some(opening) = open.take() {
            let (content, content_start) = parsed;
            if content == opening.closer {
                let formula_range = opening.formula_start..content_start;
                if !source[formula_range.clone()].trim().is_empty() {
                    complete.push(DisplayMathCandidate {
                        block_range: opening.delimiter_start..content_start + opening.closer.len(),
                        formula_range,
                    });
                }
                continue;
            }
            if !content.contains(opening.opener) && !content.contains(opening.closer) {
                open = Some(opening);
                continue;
            }
            lines = source_lines(source, opening.following_start);
            continue;
        }

        let (trimmed, trimmed_start) = parsed;
        let Some((opener, closer)) = display_delimiters(trimmed) else {
            continue;
        };
        let body = &trimmed[opener.len()..];
        if let Some(formula) = body.strip_suffix(closer) {
            if !formula.trim().is_empty() && !formula.contains(opener) && !formula.contains(closer)
            {
                let start = trimmed_start + opener.len();
                complete.push(DisplayMathCandidate {
                    block_range: trimmed_start..trimmed_start + trimmed.len(),
                    formula_range: start..start + formula.len(),
                });
            }
            continue;
        }
        if !body.trim().is_empty() {
            continue;
        }

        open = Some(OpenDisplayMath {
            block_start: line.start,
            delimiter_start: trimmed_start,
            formula_start: trimmed_start + opener.len(),
            following_start: (line.start + line.text.len() + 1).min(source.len()),
            opener,
            closer,
        });
    }

    let pending = open.map(|opening| PendingDisplayMathCandidate {
        block_start: opening.block_start,
        delimiter_start: opening.delimiter_start,
        formula_start: opening.formula_start,
        has_content_after_blank_line: has_content_after_blank_line(
            &source[opening.following_start..],
        ),
    });
    (complete, pending)
}

fn has_content_after_blank_line(source: &str) -> bool {
    let mut blank_line_seen = false;
    source.split('\n').any(|line| {
        if blank_line_seen {
            return !line.trim().is_empty();
        }
        let Some(content) = content_after_blank_line(line) else {
            return false;
        };
        blank_line_seen = true;
        !content.trim().is_empty()
    })
}

fn content_after_blank_line(line: &str) -> Option<&str> {
    if line.is_empty() {
        return Some(line);
    }
    line.strip_prefix('\r').or_else(|| {
        line.split_once("\r\r")
            .map(|(_, content_after_blank)| content_after_blank)
    })
}

fn source_lines(source: &str, start: usize) -> impl Iterator<Item = SourceLine<'_>> {
    let mut next_start = start;
    source[start..].split('\n').map(move |text| {
        let line = SourceLine {
            start: next_start,
            text,
        };
        next_start += text.len() + 1;
        line
    })
}

fn mask_formula(markdown: &mut [u8], formula_range: Range<usize>, mask: u8) {
    for byte in &mut markdown[formula_range] {
        if !matches!(*byte, b'\r' | b'\n') {
            *byte = mask;
        }
    }
}

fn accepted_display_math_candidates<'a>(
    source: &str,
    markdown: &str,
    candidates: &[&'a DisplayMathCandidate],
) -> Vec<&'a DisplayMathCandidate> {
    let mut accepted = Vec::new();
    let mut ancestors = Vec::new();

    for (event, range) in Parser::new_ext(markdown, markdown_options()).into_offset_iter() {
        if let Some(container_range) = display_math_container(&event, &range, &ancestors) {
            let start = candidates
                .partition_point(|candidate| candidate.formula_range.start <= range.start);
            let end = start
                + candidates[start..]
                    .partition_point(|candidate| candidate.block_range.start < range.end);
            accepted.extend(candidates[start..end].iter().copied().filter(|candidate| {
                container_range.start <= candidate.block_range.start
                    && candidate.block_range.end <= container_range.end
                    && (!ancestors.is_empty()
                        || source[container_range.clone()].trim()
                            == &source[candidate.block_range.clone()])
                    && !contains_html(&source[candidate.block_range.clone()])
            }));
        }
        update_ancestors(&event, &range, &mut ancestors);
    }

    accepted
}

fn eligible_text_candidates<'a>(
    source: &str,
    candidates: &'a [DisplayMathCandidate],
) -> Vec<&'a DisplayMathCandidate> {
    let mut ancestors = Vec::new();
    let mut eligible = Vec::new();

    for (event, range) in Parser::new_ext(source, markdown_options()).into_offset_iter() {
        if let Some(container_range) = display_math_container(&event, &range, &ancestors) {
            let start = candidates
                .partition_point(|candidate| candidate.formula_range.start <= range.start);
            let end = start
                + candidates[start..]
                    .partition_point(|candidate| candidate.block_range.start < range.end);
            eligible.extend(candidates[start..end].iter().filter(|candidate| {
                (ancestors.is_empty()
                    && matches!(&event, Event::Start(Tag::Paragraph | Tag::Heading { .. })))
                    || (container_range.start <= candidate.block_range.start
                        && candidate.block_range.end <= container_range.end)
            }));
        }
        update_ancestors(&event, &range, &mut ancestors);
    }

    eligible
}

type MarkdownAncestor = (TagEnd, Range<usize>);

fn display_math_container(
    event: &Event<'_>,
    event_range: &Range<usize>,
    ancestors: &[MarkdownAncestor],
) -> Option<Range<usize>> {
    if !ancestors
        .iter()
        .all(|(tag, _)| matches!(tag, TagEnd::List(_) | TagEnd::Item))
    {
        return None;
    }
    let item_range = ancestors
        .iter()
        .rev()
        .find_map(|(tag, range)| matches!(tag, TagEnd::Item).then(|| range.clone()));
    match event {
        Event::Start(Tag::Paragraph | Tag::Heading { .. }) => {
            Some(item_range.unwrap_or_else(|| event_range.clone()))
        }
        Event::Text(_) | Event::SoftBreak | Event::HardBreak => item_range,
        _ => None,
    }
}

fn update_ancestors(
    event: &Event<'_>,
    range: &Range<usize>,
    ancestors: &mut Vec<MarkdownAncestor>,
) {
    match event {
        Event::Start(tag) => ancestors.push((tag.to_end(), range.clone())),
        Event::End(_) => {
            ancestors.pop();
        }
        _ => {}
    }
}

fn display_line(line: SourceLine<'_>) -> (&str, usize) {
    let text = line.text.strip_suffix('\r').unwrap_or(line.text);
    let leading_spaces = text.bytes().take_while(|byte| *byte == b' ').count();
    let content = &text[leading_spaces..];
    let trimmed = content.trim_end_matches([' ', '\t']);
    (trimmed, line.start + leading_spaces)
}

fn display_delimiters(line: &str) -> Option<(&'static str, &'static str)> {
    if line.starts_with("$$") {
        Some(("$$", "$$"))
    } else if line.starts_with(r"\[") {
        Some((r"\[", r"\]"))
    } else {
        None
    }
}

fn contains_html(markdown: &str) -> bool {
    Parser::new(markdown).any(|event| matches!(event, Event::Html(_) | Event::InlineHtml(_)))
}

#[cfg(test)]
#[path = "display_math_tests.rs"]
mod tests;
