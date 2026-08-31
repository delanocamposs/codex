//! Math-aware setup and event handling for the Markdown renderer.

use super::RtOptions;
use super::TABLE_CELL_PADDING;
use super::TABLE_COLUMN_GAP;
use super::Writer;
use super::inline_math::InlineMathCursor;
use super::inline_math::InlineMathSegment;
use super::inline_math::InlineMathText;
use super::inline_math_unicode;
use super::word_wrap_hyperlink_line;
use crate::latex_image::AdmissibleLatexFormula;
use crate::markdown_text_merge::DecodedTextMerge;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::TerminalHyperlink;
use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use ratatui::text::Line;
use std::ops::Range;
use std::path::Path;

pub(crate) struct MarkdownRenderOptions<'a> {
    pub(crate) width: Option<usize>,
    pub(crate) cwd: Option<&'a Path>,
    pub(crate) is_hidden_link_destination: &'a dyn Fn(&str) -> bool,
    pub(crate) latex_renderer: Option<&'a crate::latex_renderer::LatexRenderHandle>,
}

pub(crate) fn render_markdown_lines(
    input: &str,
    render_options: MarkdownRenderOptions<'_>,
) -> Vec<HyperlinkLine> {
    render_prepared_markdown_lines(crate::math_source::PreparedMath::new(input), render_options)
}

pub(crate) fn render_prepared_markdown_lines(
    prepared_math: crate::math_source::PreparedMath<'_>,
    render_options: MarkdownRenderOptions<'_>,
) -> Vec<HyperlinkLine> {
    let MarkdownRenderOptions {
        width,
        cwd,
        is_hidden_link_destination,
        latex_renderer,
    } = render_options;
    if prepared_math.renders_literal() {
        return render_literal_lines(prepared_math.source(), width);
    }
    let mut parser_options = Options::empty();
    parser_options.insert(Options::ENABLE_STRIKETHROUGH);
    parser_options.insert(Options::ENABLE_TABLES);
    let crate::math_source::PreparedMathParts {
        source,
        markdown,
        inline,
        display,
    } = prepared_math.into_parts();
    let input = source.as_ref();
    let parser =
        DecodedTextMerge::new(Parser::new_ext(&markdown, parser_options).into_offset_iter());
    let mut writer = Writer::new(
        input,
        parser,
        width,
        cwd,
        is_hidden_link_destination,
        InlineMathCursor::new(input, inline),
    );
    writer.latex_renderer =
        latex_renderer.map(crate::latex_renderer::LatexRenderHandle::for_render_pass);
    writer.run(display);
    writer.text
}

pub(super) fn render_literal_lines(input: &str, width: Option<usize>) -> Vec<HyperlinkLine> {
    input
        .split('\n')
        .flat_map(|line| {
            let line = HyperlinkLine::new(Line::from(
                line.strip_suffix('\r').unwrap_or(line).to_string(),
            ));
            width
                .map(|width| {
                    word_wrap_hyperlink_line(&line, RtOptions::new(width.max(/*other*/ 1)))
                })
                .unwrap_or_else(|| vec![line])
        })
        .collect()
}

impl<'a, 'policy, I> Writer<'a, 'policy, I>
where
    I: Iterator<Item = (Event<'a>, Range<usize>)>,
{
    pub(super) fn run(&mut self, display_math: Vec<crate::math_source::DisplayMathSpan>) {
        let mut display_math = display_math.into_iter().peekable();
        let mut active_display_math: Option<Range<usize>> = None;
        while let Some((event, range)) = self.iter.next() {
            if let Some(active) = active_display_math.as_ref() {
                let belongs_to_display = active.start <= range.start
                    && range.start < active.end
                    && (range.end <= active.end
                        || self.input[active.end..range.end].trim().is_empty());
                if belongs_to_display
                    || range.start == active.end
                        && matches!(&event, Event::SoftBreak | Event::HardBreak)
                {
                    continue;
                }
                active_display_math = None;
            }
            let is_leaf_content = matches!(
                &event,
                Event::Text(_)
                    | Event::Code(_)
                    | Event::SoftBreak
                    | Event::HardBreak
                    | Event::InlineHtml(_)
            );
            if is_leaf_content
                && let Some(span) = display_math.peek()
                && range.start < span.source_range.end
                && span.source_range.start < range.end
                && let Some(span) = display_math.next()
            {
                debug_assert_eq!(range.start, span.source_range.start);
                active_display_math = Some(span.source_range.clone());
                self.display_math_block(span);
                continue;
            }
            self.handle_event(event, range);
        }
        debug_assert!(
            self.inline_math.is_complete(),
            "every parser-accepted inline formula must be consumed"
        );
        self.flush_current_line();
        debug_assert!(display_math.next().is_none());
    }

    fn display_math_block(&mut self, span: crate::math_source::DisplayMathSpan) {
        let reuse_empty_line = !self.needs_newline
            && self.current_line_content.as_ref().is_some_and(|line| {
                line.line.spans.is_empty()
                    && line.hyperlinks.is_empty()
                    && line.kitty_images.is_empty()
            });
        let rendered = self.latex_renderer.as_ref().and_then(|renderer| {
            renderer.render(
                &span.formula,
                u16::try_from(self.max_text_math_columns()).unwrap_or(u16::MAX),
            )
        });

        if !reuse_empty_line {
            self.flush_current_line();
        }
        if self.needs_newline {
            self.push_blank_line();
        }
        if let Some(lines) = rendered {
            for (index, line) in lines.into_iter().enumerate() {
                if reuse_empty_line && index == 0 {
                    self.push_annotated(line);
                } else {
                    self.push_hyperlink_line(line);
                }
                self.flush_current_line();
            }
        } else {
            let literal = span.literal(self.input);
            for (index, line) in literal.split('\n').enumerate() {
                let line = Line::from(line.to_string());
                if reuse_empty_line && index == 0 {
                    self.current_line_content = Some(HyperlinkLine::new(line));
                } else {
                    self.push_line(line);
                }
                self.flush_current_line();
            }
        }
        self.needs_newline = true;
        self.pending_marker_line = false;
    }

    pub(super) fn push_text_with_inline_math(&mut self, text: &str, source_range: Range<usize>) {
        match self.inline_math.consume(text, source_range) {
            InlineMathText::Plain(text) => self.push_decoded_text(text),
            InlineMathText::SourceLiteral(text) => self.push_decoded_text(text),
            InlineMathText::Segmented(segments) => {
                for segment in segments {
                    match segment {
                        InlineMathSegment::Text(text) => self.push_decoded_text(text),
                        InlineMathSegment::Math { raw, formula } => {
                            self.render_inline_math(raw, formula);
                        }
                    }
                }
            }
        }
    }

    pub(super) fn discard_inline_math(&mut self, text: &str, source_range: Range<usize>) {
        self.inline_math.discard(text, source_range);
    }

    fn push_decoded_text(&mut self, text: &str) {
        if self.in_table_cell() {
            self.push_text_to_table_cell(text);
            return;
        }
        for (index, line) in text.lines().enumerate() {
            if self.needs_newline {
                self.push_line(Line::default());
                self.needs_newline = false;
            }
            if index > 0 {
                self.push_line(Line::default());
            }
            let content = line.to_string();
            let style = self.inline_styles.last().copied().unwrap_or_default();
            self.push_text_spans(&content, style);
        }
    }

    fn render_inline_math(&mut self, raw: &str, formula: &str) {
        debug_assert!(
            !self.in_code_block,
            "the source parser must not accept math inside code"
        );
        if self.latex_renderer.is_some()
            && let Some(rendered) = AdmissibleLatexFormula::new(formula)
                .ok()
                .and_then(|formula| inline_math_unicode::render(formula.as_str()))
        {
            self.push_decoded_text(&rendered);
            return;
        }
        let in_table_cell = self.in_table_cell();
        let max_columns = if in_table_cell {
            let column_count = self
                .table_state
                .as_ref()
                .map(|table| table.alignments.len())
                .unwrap_or(/*default*/ 1)
                .max(1);
            let table_width = self.available_table_width(column_count).unwrap_or_else(|| {
                let reserved = (column_count.saturating_sub(1) * TABLE_COLUMN_GAP)
                    + (column_count * TABLE_CELL_PADDING * 2);
                80usize.saturating_sub(reserved)
            });
            (table_width / column_count).max(1)
        } else {
            self.max_text_math_columns()
        };
        let rendered = self.latex_renderer.as_ref().and_then(|renderer| {
            renderer.render_inline(formula, u16::try_from(max_columns).unwrap_or(u16::MAX))
        });
        if let Some(mut rendered) = rendered {
            if let Some(destination) = self
                .link
                .as_ref()
                .and_then(|link| super::web_destination(&link.destination))
            {
                let width = rendered.width();
                rendered
                    .hyperlinks
                    .push(TerminalHyperlink::web(0..width, destination));
            }
            if let Some(table_state) = self.table_state.as_mut()
                && let Some(cell) = table_state.current_cell.as_mut()
            {
                cell.push_annotated(rendered);
            } else {
                self.push_annotated(rendered);
            }
        } else {
            self.push_decoded_text(raw);
        }
    }

    fn max_text_math_columns(&self) -> usize {
        let indentation = if self.current_line_content.is_some() {
            Self::spans_display_width(&self.current_initial_indent)
                .max(Self::spans_display_width(&self.current_subsequent_indent))
        } else {
            Self::spans_display_width(&self.prefix_spans(self.pending_marker_line)).max(
                Self::spans_display_width(&self.prefix_spans(/*pending_marker_line*/ false)),
            )
        };
        self.wrap_width
            .unwrap_or(/*default*/ 80)
            .saturating_sub(indentation)
            .max(1)
    }
}
