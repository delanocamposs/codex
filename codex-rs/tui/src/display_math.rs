//! Recognition of standalone display-math paragraphs in Markdown source.

use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::Tag;
use std::borrow::Cow;
use std::ops::Range;

/// A display-math block backed by the original Markdown source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DisplayMathBlock<'a> {
    /// The exact, whitespace-trimmed source block, including its delimiters.
    pub(crate) raw_block: &'a str,
    /// The exact source between the opening and closing delimiters.
    pub(crate) formula: &'a str,
}

/// Markdown with the contents of lexical display-math candidates neutralized.
///
/// CommonMark can otherwise interpret TeX lines as Markdown structure before
/// [`DisplayMathExtractor`] sees them. In particular, a formula line containing only `=` turns
/// the preceding line into a Setext heading. A parse of the unmodified source first excludes
/// candidates in lists, block quotes, code blocks, tables, and HTML. A masked discovery parse then
/// accepts complete top-level paragraphs, and only those formulas remain masked in the result.
pub(crate) struct PreparedDisplayMath<'a> {
    markdown: Cow<'a, str>,
    source_ranges: Vec<Range<usize>>,
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
            source_ranges: Vec::new(),
            pending_start,
        };
        if candidates.is_empty() {
            return unchanged();
        }
        let probe_candidates = top_level_text_candidates(source, &candidates);
        if probe_candidates.is_empty() {
            return unchanged();
        }

        let mut probe = source.as_bytes().to_vec();
        for candidate in &probe_candidates {
            mask_formula(&mut probe, candidate.formula_range.clone(), mask);
        }
        let probe =
            String::from_utf8(probe).expect("ASCII display-math masking must preserve valid UTF-8");
        let accepted = accepted_display_math_candidates(source, &probe, &probe_candidates);
        if accepted.is_empty() {
            return unchanged();
        }

        let mut markdown = source.as_bytes().to_vec();
        let source_ranges = accepted
            .iter()
            .map(|candidate| candidate.block_range.clone())
            .collect();
        for candidate in accepted {
            mask_formula(&mut markdown, candidate.formula_range.clone(), mask);
        }
        Self {
            markdown: Cow::Owned(
                String::from_utf8(markdown)
                    .expect("ASCII display-math masking must preserve valid UTF-8"),
            ),
            source_ranges,
            pending_start,
        }
    }

    pub(crate) fn into_parts(self) -> (Cow<'a, str>, Vec<Range<usize>>, Option<usize>) {
        (self.markdown, self.source_ranges, self.pending_start)
    }
}

/// Observes a complete pulldown-cmark event stream and recognizes display math.
///
/// Call [`Self::inspect`] exactly once for every event, in order. The observer
/// uses the nesting of those events to reject paragraphs inside Markdown
/// containers such as lists and block quotes.
pub(crate) struct DisplayMathExtractor<'a> {
    source: &'a str,
    depth: usize,
}

impl<'a> DisplayMathExtractor<'a> {
    pub(crate) fn new(source: &'a str) -> Self {
        Self { source, depth: 0 }
    }

    /// Inspects one event from an offset iterator.
    ///
    /// Returns a block only when this event starts a top-level paragraph whose
    /// entire trimmed source is one nonempty `$$...$$` or `\[...\]` block.
    pub(crate) fn inspect(
        &mut self,
        event: &Event<'_>,
        source_range: Range<usize>,
    ) -> Option<DisplayMathBlock<'a>> {
        let block = if self.depth == 0 && matches!(event, Event::Start(Tag::Paragraph)) {
            parse_display_math_paragraph(self.source, source_range)
        } else {
            None
        };

        match event {
            Event::Start(_) => self.depth += 1,
            Event::End(_) => {
                debug_assert_ne!(
                    self.depth, 0,
                    "display-math extractor observed an unbalanced end event"
                );
                self.depth = self.depth.saturating_sub(1);
            }
            _ => {}
        }

        block
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
    let markdown =
        String::from_utf8(markdown).expect("ASCII display-math masking must preserve valid UTF-8");
    let mut depth = 0usize;

    for (event, range) in Parser::new_ext(&markdown, markdown_options()).into_offset_iter() {
        if depth == 0
            && matches!(event, Event::Start(Tag::Paragraph))
            && range.start <= candidate.delimiter_start
            && candidate.delimiter_start < range.end
            && source[range.start..candidate.delimiter_start]
                .trim()
                .is_empty()
        {
            return Some(range.start.min(candidate.block_start));
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
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
            let Some((content, content_start)) = parsed else {
                open = Some(opening);
                continue;
            };
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

        let Some((trimmed, trimmed_start)) = parsed else {
            continue;
        };
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
    let mut extractor = DisplayMathExtractor::new(source);
    let mut accepted = Vec::new();

    for (event, range) in Parser::new_ext(markdown, markdown_options()).into_offset_iter() {
        if extractor.inspect(&event, range.clone()).is_some()
            && let Some(candidate) = candidates.iter().copied().find(|candidate| {
                range.start <= candidate.block_range.start && candidate.block_range.end <= range.end
            })
        {
            accepted.push(candidate);
        }
    }

    accepted
}

fn top_level_text_candidates<'a>(
    source: &str,
    candidates: &'a [DisplayMathCandidate],
) -> Vec<&'a DisplayMathCandidate> {
    let mut depth = 0usize;
    let mut top_level = Vec::new();

    for (event, range) in Parser::new_ext(source, markdown_options()).into_offset_iter() {
        if depth == 0 && matches!(&event, Event::Start(Tag::Paragraph | Tag::Heading { .. })) {
            top_level.extend(
                candidates
                    .iter()
                    .filter(|candidate| range.contains(&candidate.block_range.start)),
            );
        }
        match event {
            Event::Start(_) => depth += 1,
            Event::End(_) => depth = depth.saturating_sub(1),
            _ => {}
        }
    }

    top_level
}

fn display_line(line: SourceLine<'_>) -> Option<(&str, usize)> {
    let text = line.text.strip_suffix('\r').unwrap_or(line.text);
    let leading_spaces = text.bytes().take_while(|byte| *byte == b' ').count();
    if leading_spaces > 3 || text.as_bytes().get(leading_spaces) == Some(&b'\t') {
        return None;
    }
    let content = &text[leading_spaces..];
    let trimmed = content.trim_end_matches([' ', '\t']);
    Some((trimmed, line.start + leading_spaces))
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

fn parse_display_math_paragraph(
    source: &str,
    paragraph_range: Range<usize>,
) -> Option<DisplayMathBlock<'_>> {
    let paragraph = source.get(paragraph_range)?;
    let raw_block = paragraph.trim();
    let formula = if let Some(formula) = raw_block
        .strip_prefix("$$")
        .and_then(|body| body.strip_suffix("$$"))
    {
        if formula.contains("$$") {
            return None;
        }
        formula
    } else if let Some(formula) = raw_block
        .strip_prefix(r"\[")
        .and_then(|body| body.strip_suffix(r"\]"))
    {
        if formula.contains(r"\[") || formula.contains(r"\]") {
            return None;
        }
        formula
    } else {
        return None;
    };

    if formula.trim().is_empty() || contains_html(raw_block) {
        return None;
    }

    Some(DisplayMathBlock { raw_block, formula })
}

fn contains_html(markdown: &str) -> bool {
    Parser::new(markdown).any(|event| matches!(event, Event::Html(_) | Event::InlineHtml(_)))
}

#[cfg(test)]
#[path = "display_math_tests.rs"]
mod tests;
