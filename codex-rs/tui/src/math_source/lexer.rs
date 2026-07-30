//! Source-order ownership lexer for Markdown code and explicit TeX math.

use crate::table_detect::parse_fence_marker;

use super::DisplayMathDelimiter;
use super::DisplayMathSpan;
use std::ops::Range;

#[path = "lexer/html.rs"]
mod html;
#[path = "lexer/indent.rs"]
mod indent;
#[path = "lexer/inline_code.rs"]
mod inline_code;
#[path = "lexer/link.rs"]
mod link;

use html::HtmlBlock;
use html::HtmlCodeTag;
use html::angle_construct;
use indent::LineView;
use indent::SourceLine;
use indent::leading_indent;
use indent::line_view;
pub(super) use indent::source_lines;
pub(super) use indent::strip_indent;
pub(super) use indent::strip_quote_prefixes;
use inline_code::Backticks;
use link::inline_target_end;
use link::reference_definition_end;
use link::reference_id_end;

pub(super) struct ParsedMath {
    pub(super) inline: Vec<Range<usize>>,
    pub(super) display: Vec<DisplayMathSpan>,
    pub(super) pending_display_start: Option<usize>,
    pub(super) renders_literal: bool,
}

pub(super) fn parse(source: &str) -> ParsedMath {
    let backticks = Backticks::new(source);
    let mut scanner = Scanner {
        source,
        inline: Vec::new(),
        display: Vec::new(),
        open_display: None,
        open_fence: None,
        open_html_block: None,
        inline_code_until: 0,
        html_code_depth: 0,
        html_opaque_until: 0,
        markdown_opaque_until: 0,
        link_label_depth: 0,
        deferred_inline: Vec::new(),
        renders_literal: false,
        mutable_start: 0,
        list_context: None,
        at_block_start: true,
    };
    for line in source_lines(source) {
        while !scanner.scan_line(line, &backticks) {}
    }
    ParsedMath {
        inline: scanner.inline,
        display: scanner.display,
        pending_display_start: scanner.open_display.map(|open| open.pending_start),
        renders_literal: scanner.renders_literal,
    }
}

struct Scanner<'a> {
    source: &'a str,
    inline: Vec<Range<usize>>,
    display: Vec<DisplayMathSpan>,
    open_display: Option<OpenDisplay>,
    open_fence: Option<OpenFence>,
    open_html_block: Option<OpenHtmlBlock>,
    inline_code_until: usize,
    html_code_depth: usize,
    html_opaque_until: usize,
    markdown_opaque_until: usize,
    link_label_depth: usize,
    deferred_inline: Vec<Range<usize>>,
    renders_literal: bool,
    mutable_start: usize,
    list_context: Option<(usize, usize)>,
    at_block_start: bool,
}

impl Scanner<'_> {
    /// Returns false when a container ended and the same line must be reconsidered.
    fn scan_line(&mut self, line: SourceLine<'_>, backticks: &Backticks) -> bool {
        if let Some(open) = self.open_display.as_ref() {
            if !container_continues(line, open.quote_depth, open.container_indent) {
                self.open_display = None;
                return false;
            }
            if let Some(content) = display_line_content(line, open)
                && content.trim() == open.delimiter.closer()
                && let Some(closer_offset) = content.find(open.delimiter.closer())
            {
                let closer = line.start + line.text.len() - content.len() + closer_offset;
                let source_range = open.source_start..closer + 2;
                let formula = super::display_formula(
                    self.source,
                    &source_range,
                    open.quote_depth,
                    open.body_indent,
                );
                if !formula.trim().is_empty() {
                    self.display.push(DisplayMathSpan {
                        source_range,
                        formula,
                        quote_depth: open.quote_depth,
                        body_indent: open.body_indent,
                    });
                }
                self.open_display = None;
            }
            return true;
        }
        if let Some(open) = self.open_fence.as_ref() {
            if !container_continues(line, open.quote_depth, open.container_indent) {
                self.open_fence = None;
                return false;
            }
            if let Some(content) = fence_content(line, open)
                && let Some((marker, run_len)) = fence_marker(content)
                && marker == open.marker
                && run_len >= open.run_len
                && content[run_len..].bytes().all(|byte| byte == b' ')
            {
                self.open_fence = None;
            }
            return true;
        }
        if let Some(open) = self.open_html_block {
            if !container_continues(line, open.quote_depth, open.container_indent) {
                self.open_html_block = None;
                return false;
            }
            if open.block.closes_on(line.text) {
                self.open_html_block = None;
            }
            return true;
        }

        let line_end = line.start + line.text.len();
        let code_prefix = self.inline_code_until > line.start;
        let html_code_prefix = self.html_code_depth > 0 || self.html_opaque_until > line.start;
        let markdown_opaque_prefix = self.markdown_opaque_until > line.start;
        if (code_prefix && self.inline_code_until >= line_end)
            || (html_code_prefix && self.html_opaque_until >= line_end)
            || (markdown_opaque_prefix && self.markdown_opaque_until >= line_end)
        {
            return true;
        }

        let mut view = line_view(line, self.list_context, self.at_block_start);
        let opaque_prefix = code_prefix || html_code_prefix || markdown_opaque_prefix;
        let starts_list = !opaque_prefix && view.list_indent.is_some();
        if starts_list && let Some(indent) = view.list_indent {
            self.list_context = Some((view.quote_depth, indent));
        }
        if !opaque_prefix && view.content.trim().is_empty() {
            self.mutable_start = line.next_start;
            self.at_block_start = true;
            self.link_label_depth = 0;
            self.deferred_inline.clear();
            return true;
        }
        if !opaque_prefix
            && !starts_list
            && self.list_context.is_some_and(|(depth, indent)| {
                depth != view.quote_depth || view.leading_columns < indent
            })
        {
            self.list_context = None;
            view = line_view(line, None, self.at_block_start);
        }
        let container_indent = self
            .list_context
            .filter(|(depth, _)| *depth == view.quote_depth)
            .map_or(0, |(_, indent)| indent);
        let relative_indent = view
            .list_residual_columns
            .unwrap_or_else(|| view.leading_columns.saturating_sub(container_indent));
        if !opaque_prefix && relative_indent >= 4 {
            self.at_block_start = true;
            self.link_label_depth = 0;
            self.deferred_inline.clear();
            return true;
        }
        if !opaque_prefix
            && relative_indent < 4
            && let Some((marker, run_len)) = fence_marker(view.content)
        {
            self.at_block_start = true;
            self.inline_code_until = 0;
            self.link_label_depth = 0;
            self.deferred_inline.clear();
            self.open_fence = Some(OpenFence {
                marker,
                run_len,
                quote_depth: view.quote_depth,
                container_indent,
            });
            return true;
        }
        if !opaque_prefix && let Some(block) = html::block_start(view.content) {
            self.at_block_start = true;
            self.link_label_depth = 0;
            self.deferred_inline.clear();
            if !block.closes_on(view.content) {
                self.open_html_block = Some(OpenHtmlBlock {
                    block,
                    quote_depth: view.quote_depth,
                    container_indent,
                });
            }
            return true;
        }
        if !opaque_prefix
            && self.link_label_depth == 0
            && let Some(end) = reference_definition_end(&self.source[view.content_start..])
        {
            self.markdown_opaque_until = view.content_start + end;
            self.at_block_start = true;
            self.link_label_depth = 0;
            self.deferred_inline.clear();
            return true;
        }
        if !opaque_prefix
            && self.link_label_depth == 0
            && let Some(display) = display_candidate(&view)
        {
            self.at_block_start = true;
            let body_indent = container_indent.max(view.leading_columns);
            if let Some(formula) = display.formula {
                self.display.push(DisplayMathSpan {
                    source_range: display.source_start..display.source_start + formula.len() + 4,
                    formula: formula.to_string(),
                    quote_depth: view.quote_depth,
                    body_indent,
                });
            } else {
                self.open_display = Some(OpenDisplay {
                    delimiter: display.delimiter,
                    source_start: display.source_start,
                    pending_start: self.mutable_start,
                    quote_depth: view.quote_depth,
                    container_indent,
                    body_indent,
                });
            }
            return true;
        }

        let mut cursor = self
            .inline_code_until
            .max(self.html_opaque_until)
            .max(self.markdown_opaque_until)
            .max(view.content_start);
        let bytes = self.source.as_bytes();
        while cursor < line_end {
            if self.html_code_depth > 0 && bytes[cursor] != b'<' {
                let Some(next_angle) = self.source[cursor..line_end].find('<') else {
                    return true;
                };
                cursor += next_angle;
            }
            if self.html_code_depth == 0 && bytes[cursor] == b'`' && !escaped(bytes, cursor) {
                let run_len = byte_run(&bytes[cursor..], b'`');
                if let Some(closer) = backticks.next(cursor, run_len) {
                    self.inline_code_until = closer + run_len;
                    cursor = self.inline_code_until;
                    continue;
                }
                cursor += run_len;
                continue;
            }
            if bytes[cursor] == b'<' && !escaped(bytes, cursor) {
                if let Some((len, code_tag)) = angle_construct(&self.source[cursor..]) {
                    match code_tag {
                        Some(HtmlCodeTag::Open) => self.html_code_depth += 1,
                        Some(HtmlCodeTag::Close) => {
                            self.html_code_depth = self.html_code_depth.saturating_sub(1);
                        }
                        None => {}
                    }
                    self.html_opaque_until = cursor + len;
                    cursor += len;
                    continue;
                }
                if let Some(end) = self.source[cursor..line_end].find('>') {
                    let end = cursor + end + 1;
                    if self.source[cursor..end].contains(r"\(") {
                        self.renders_literal = true;
                        cursor = end;
                        continue;
                    }
                }
            }
            if self.html_code_depth > 0 {
                cursor += 1;
                continue;
            }
            if bytes[cursor] == b'[' && !escaped(bytes, cursor) {
                if self.link_label_depth == 0 {
                    self.deferred_inline.clear();
                }
                self.link_label_depth += 1;
                cursor += 1;
                continue;
            }
            if bytes[cursor] == b']' && !escaped(bytes, cursor) {
                let closes_label = self.link_label_depth > 0;
                self.link_label_depth = self.link_label_depth.saturating_sub(1);
                if closes_label {
                    let metadata = match bytes.get(cursor + 1) {
                        Some(b'(') => inline_target_end(&self.source[cursor + 1..])
                            .map(|len| (len, /*visible_label*/ true)),
                        Some(b'[') => reference_id_end(&self.source[cursor + 1..])
                            .map(|len| (len, /*visible_label*/ len > 2)),
                        Some(_) | None => None,
                    };
                    if self.link_label_depth == 0 {
                        if metadata.is_some_and(|(_, visible_label)| visible_label) {
                            self.inline.append(&mut self.deferred_inline);
                        } else {
                            self.renders_literal |= !self.deferred_inline.is_empty();
                            self.deferred_inline.clear();
                        }
                    }
                    if let Some((len, _)) = metadata {
                        self.markdown_opaque_until = cursor + 1 + len;
                        cursor = self.markdown_opaque_until;
                        continue;
                    }
                }
                cursor += 1;
                continue;
            }
            if cursor + 1 < line_end
                && bytes[cursor..].starts_with(br"\(")
                && !escaped(bytes, cursor)
            {
                let Some(closer) = inline_closer(bytes, cursor + 2, line_end) else {
                    break;
                };
                if self.source[cursor + 2..closer].trim().is_empty() {
                    cursor = closer + 2;
                } else {
                    let range = cursor..closer + 2;
                    if self.link_label_depth == 0 {
                        self.inline.push(range);
                    } else {
                        self.deferred_inline.push(range);
                    }
                    cursor = closer + 2;
                }
                continue;
            }
            cursor += 1;
        }
        self.at_block_start = false;
        true
    }
}

fn inline_closer(bytes: &[u8], mut cursor: usize, end: usize) -> Option<usize> {
    while cursor + 1 < end {
        if bytes[cursor..].starts_with(br"\)") && !escaped(bytes, cursor) {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

struct OpenDisplay {
    delimiter: DisplayMathDelimiter,
    source_start: usize,
    pending_start: usize,
    quote_depth: usize,
    container_indent: usize,
    body_indent: usize,
}

struct OpenFence {
    marker: u8,
    run_len: usize,
    quote_depth: usize,
    container_indent: usize,
}

#[derive(Clone, Copy)]
struct OpenHtmlBlock {
    block: HtmlBlock,
    quote_depth: usize,
    container_indent: usize,
}

struct DisplayCandidate<'a> {
    delimiter: DisplayMathDelimiter,
    source_start: usize,
    formula: Option<&'a str>,
}

fn display_candidate<'a>(line: &'a LineView<'a>) -> Option<DisplayCandidate<'a>> {
    let content = line.content.trim();
    let start = line.content_start + line.content.find(content)?;
    for delimiter in [DisplayMathDelimiter::Bracket, DisplayMathDelimiter::Dollar] {
        if content == delimiter.opener() {
            return Some(DisplayCandidate {
                delimiter,
                source_start: start,
                formula: None,
            });
        }
        let Some(body) = content
            .strip_prefix(delimiter.opener())
            .and_then(|body| body.strip_suffix(delimiter.closer()))
        else {
            continue;
        };
        if body.trim().is_empty()
            || body.contains(delimiter.closer())
            || delimiter == DisplayMathDelimiter::Dollar
                && (body.starts_with('$') || body.ends_with('$'))
        {
            return None;
        }
        return Some(DisplayCandidate {
            delimiter,
            source_start: start,
            formula: Some(body),
        });
    }
    None
}

fn container_continues(line: SourceLine<'_>, quote_depth: usize, indent: usize) -> bool {
    let (content, quote_bytes, actual_depth) = strip_quote_prefixes(line.text, quote_depth);
    actual_depth >= quote_depth
        && (content.trim().is_empty() || leading_indent(content, quote_bytes).1 >= indent)
}

fn display_line_content<'a>(line: SourceLine<'a>, open: &OpenDisplay) -> Option<&'a str> {
    let (content, quote_bytes, depth) = strip_quote_prefixes(line.text, open.quote_depth);
    let (leading_bytes, leading_columns) = leading_indent(content, quote_bytes);
    (depth >= open.quote_depth && leading_columns >= open.container_indent)
        .then(|| &content[leading_bytes..])
}

fn fence_content<'a>(line: SourceLine<'a>, open: &OpenFence) -> Option<&'a str> {
    let (content, quote_bytes, depth) = strip_quote_prefixes(line.text, open.quote_depth);
    let (leading_bytes, leading_columns) = leading_indent(content, quote_bytes);
    let extra = leading_columns.saturating_sub(open.container_indent);
    (depth >= open.quote_depth && leading_columns >= open.container_indent && extra <= 3)
        .then(|| &content[leading_bytes..])
}

fn fence_marker(source: &str) -> Option<(u8, usize)> {
    let content = source.trim_end();
    let (marker, run_len) = parse_fence_marker(content)?;
    (marker != '`' || !content[run_len..].contains('`')).then_some((marker as u8, run_len))
}

fn byte_run(bytes: &[u8], needle: u8) -> usize {
    bytes.iter().take_while(|byte| **byte == needle).count()
}

fn escaped(bytes: &[u8], start: usize) -> bool {
    bytes[..start]
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\\')
        .count()
        % 2
        == 1
}
