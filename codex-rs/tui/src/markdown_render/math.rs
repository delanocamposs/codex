//! Math-aware setup and event handling for the Markdown renderer.

use super::RtOptions;
use super::TABLE_CELL_PADDING;
use super::TABLE_COLUMN_GAP;
use super::Writer;
use super::inline_math::InlineMathCursor;
use super::inline_math::InlineMathSegment;
use super::inline_math::InlineMathText;
use super::word_wrap_hyperlink_line;
use crate::markdown_text_merge::DecodedTextMerge;
use crate::terminal_hyperlinks::HyperlinkLine;
use pulldown_cmark::Event;
use pulldown_cmark::Options;
use pulldown_cmark::Parser;
use pulldown_cmark::TagEnd;
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
    let MarkdownRenderOptions {
        width,
        cwd,
        is_hidden_link_destination,
        latex_renderer,
    } = render_options;
    let mut parser_options = Options::empty();
    parser_options.insert(Options::ENABLE_STRIKETHROUGH);
    parser_options.insert(Options::ENABLE_TABLES);
    let prepared_inline_math = crate::inline_math::PreparedInlineMath::new(input);
    if prepared_inline_math.renders_literal() {
        return render_literal_lines(input, width);
    }
    let (markdown, inline_math_mask, inline_math_spans, display_math) =
        prepared_inline_math.into_parts();
    let parser =
        DecodedTextMerge::new(Parser::new_ext(&markdown, parser_options).into_offset_iter());
    let mut writer = Writer::new(
        input,
        parser,
        width,
        cwd,
        is_hidden_link_destination,
        InlineMathCursor::new(input, inline_math_mask, inline_math_spans),
    );
    writer.latex_renderer = latex_renderer.map(|renderer| renderer.for_render_pass());
    if !writer.run(display_math) {
        return render_literal_lines(input, width);
    }
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
    pub(super) fn run(
        &mut self,
        display_math: Vec<crate::display_math::DisplayMathSpan<'a>>,
    ) -> bool {
        let mut display_math = display_math.into_iter().peekable();
        while let Some((event, range)) = self.iter.next() {
            let is_display_math = matches!(&event, Event::Start(pulldown_cmark::Tag::Paragraph))
                && display_math.peek().is_some_and(|span| {
                    range.start <= span.source_range.start
                        && span.source_range.end <= range.end
                        && self
                            .input
                            .get(range.clone())
                            .is_some_and(|paragraph| paragraph.trim() == span.raw_block)
                });
            if is_display_math {
                let span = display_math
                    .next()
                    .expect("the display-math span was just validated");
                for (paragraph_event, _) in self.iter.by_ref() {
                    if matches!(paragraph_event, Event::End(TagEnd::Paragraph)) {
                        break;
                    }
                }
                self.display_math_block(span.raw_block, span.formula);
                continue;
            }
            self.handle_event(event, range);
        }
        debug_assert!(
            self.inline_math.is_complete(),
            "every inline-math source span must be consumed"
        );
        self.flush_current_line();
        display_math.next().is_none()
    }

    fn display_math_block(&mut self, raw_block: &str, formula: &str) {
        let rendered = self.latex_renderer.as_ref().and_then(|renderer| {
            renderer.render(
                formula,
                self.wrap_width
                    .and_then(|width| u16::try_from(width).ok())
                    .unwrap_or(/*default*/ 80),
            )
        });

        self.flush_current_line();
        if self.needs_newline {
            self.push_blank_line();
        }
        if let Some(lines) = rendered {
            for line in lines {
                self.push_hyperlink_line(line);
                self.flush_current_line();
            }
        } else {
            for line in raw_block.split('\n') {
                self.push_line(Line::from(
                    line.strip_suffix('\r').unwrap_or(line).to_string(),
                ));
                self.flush_current_line();
            }
        }
        self.needs_newline = true;
        self.in_paragraph = false;
        self.pending_marker_line = false;
    }

    pub(super) fn push_text_with_inline_math(&mut self, text: &str, source_range: Range<usize>) {
        match self.inline_math.consume(text, source_range) {
            InlineMathText::Plain(text) => self.push_decoded_text(text),
            InlineMathText::SourceLiteral(text) => self.push_decoded_text(&text),
            InlineMathText::Segmented(segments) => {
                for segment in segments {
                    match segment {
                        InlineMathSegment::Text(text) => self.push_decoded_text(text),
                        InlineMathSegment::Math(span) => self.render_inline_math(span),
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

    fn render_inline_math(&mut self, span: crate::inline_math::InlineMathSpan) {
        if self.link.is_some() {
            self.push_decoded_text(&span.raw);
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
            let indentation = self
                .current_initial_indent
                .iter()
                .map(|span| crate::width::display_width(span.content.as_ref()))
                .sum::<usize>()
                .max(
                    self.current_subsequent_indent
                        .iter()
                        .map(|span| crate::width::display_width(span.content.as_ref()))
                        .sum(),
                );
            self.wrap_width
                .unwrap_or(/*default*/ 80)
                .saturating_sub(indentation)
                .max(1)
        };
        let rendered = self.latex_renderer.as_ref().and_then(|renderer| {
            renderer.render_inline(
                &span.formula,
                u16::try_from(max_columns).unwrap_or(u16::MAX),
            )
        });
        if let Some(rendered) = rendered {
            if let Some(table_state) = self.table_state.as_mut()
                && let Some(cell) = table_state.current_cell.as_mut()
            {
                cell.push_annotated(rendered);
            } else {
                self.push_annotated(rendered);
            }
        } else {
            self.push_decoded_text(&span.raw);
        }
    }
}
