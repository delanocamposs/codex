//! Terminal annotations carried separately from visible TUI text.
//!
//! Layout code measures and wraps ordinary ratatui lines. Hyperlink and Kitty-image annotations
//! are applied only when text reaches a terminal buffer or scrollback writer, so their protocol
//! bytes never affect geometry.

use std::fmt;
use std::num::NonZeroU16;
use std::ops::Range;

use ratatui::buffer::Buffer;
use ratatui::buffer::CellDiffOption;
use ratatui::buffer::CellWidth;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;
use ratatui::text::Text;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Widget;
use ratatui::widgets::Wrap;
use url::Url;

use crate::line_truncation::line_width;
use crate::render::line_utils::line_to_borrowed;
use crate::render::line_utils::line_to_static;
use crate::terminal_image::KittyImage;
use crate::width::char_width;
use crate::width::display_width;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_line;
use crate::wrapping::word_wrap_line;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalHyperlink {
    pub(crate) columns: Range<usize>,
    pub(crate) destination: String,
    destination_kind: DestinationKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DestinationKind {
    Web,
    TrustedFile,
}

impl TerminalHyperlink {
    pub(crate) fn web(columns: Range<usize>, destination: String) -> Self {
        Self {
            columns,
            destination,
            destination_kind: DestinationKind::Web,
        }
    }

    pub(crate) fn retarget_to_trusted_file(&mut self, destination: &Url) {
        // Keep file URLs out of the general Markdown link path. Only generated visualization links
        // are promoted to this destination kind.
        debug_assert_eq!(destination.scheme(), "file");
        self.destination = destination.to_string();
        self.destination_kind = DestinationKind::TrustedFile;
    }

    fn with_columns(&self, columns: Range<usize>) -> Self {
        Self {
            columns,
            destination: self.destination.clone(),
            destination_kind: self.destination_kind,
        }
    }

    fn terminal_destination(&self) -> Option<String> {
        match self.destination_kind {
            DestinationKind::Web => web_destination(&self.destination),
            DestinationKind::TrustedFile => trusted_file_destination(&self.destination),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct KittyImageAnnotation {
    pub(crate) columns: Range<usize>,
    pub(crate) image: KittyImage,
}

/// A visible ratatui line plus terminal-only annotations that must follow it through layout.
#[derive(Clone, Default, Eq, PartialEq)]
pub(crate) struct HyperlinkLine {
    pub(crate) line: Line<'static>,
    pub(crate) hyperlinks: Vec<TerminalHyperlink>,
    pub(crate) kitty_images: Vec<KittyImageAnnotation>,
}

impl fmt::Debug for HyperlinkLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("HyperlinkLine");
        debug
            .field("line", &self.line)
            .field("hyperlinks", &self.hyperlinks);
        if !self.kitty_images.is_empty() {
            debug.field("kitty_images", &self.kitty_images);
        }
        debug.finish()
    }
}

impl HyperlinkLine {
    pub(crate) fn new(line: Line<'static>) -> Self {
        Self {
            line,
            hyperlinks: Vec::new(),
            kitty_images: Vec::new(),
        }
    }

    pub(crate) fn width(&self) -> usize {
        line_width(&self.line)
    }

    pub(crate) fn push_span(&mut self, span: Span<'static>, destination: Option<&str>) {
        let start = self.width();
        let end = start + display_width(span.content.as_ref());
        self.line.push_span(span);
        if end > start
            && let Some(destination) = destination.and_then(web_destination)
        {
            self.hyperlinks
                .push(TerminalHyperlink::web(start..end, destination));
        }
    }

    pub(crate) fn append_annotated(&mut self, appended: HyperlinkLine) {
        let shift = self.width();
        let mut appended_spans = appended.line.spans;
        for span in &mut appended_spans {
            span.style = appended.line.style.patch(span.style);
        }
        self.line.spans.extend(appended_spans);
        self.hyperlinks
            .extend(appended.hyperlinks.into_iter().map(|mut link| {
                link.columns = link.columns.start + shift..link.columns.end + shift;
                link
            }));
        self.kitty_images
            .extend(appended.kitty_images.into_iter().map(|mut image| {
                image.columns = image.columns.start + shift..image.columns.end + shift;
                image
            }));
    }

    pub(crate) fn style(mut self, style: ratatui::style::Style) -> Self {
        self.line = self.line.style(style);
        self
    }
}

impl From<Line<'static>> for HyperlinkLine {
    fn from(line: Line<'static>) -> Self {
        Self::new(line)
    }
}

impl From<&'static str> for HyperlinkLine {
    fn from(text: &'static str) -> Self {
        Self::new(Line::from(text))
    }
}

impl From<String> for HyperlinkLine {
    fn from(text: String) -> Self {
        Self::new(Line::from(text))
    }
}

pub(crate) fn visible_lines(lines: Vec<HyperlinkLine>) -> Vec<Line<'static>> {
    lines.into_iter().map(|line| line.line).collect()
}

pub(crate) fn visible_lines_ref(lines: &[HyperlinkLine]) -> Vec<Line<'_>> {
    lines
        .iter()
        .map(|line| line_to_borrowed(&line.line))
        .collect()
}

pub(crate) fn plain_hyperlink_lines(lines: Vec<Line<'static>>) -> Vec<HyperlinkLine> {
    lines.into_iter().map(HyperlinkLine::new).collect()
}

pub(crate) fn prefix_hyperlink_lines(
    lines: Vec<HyperlinkLine>,
    initial_prefix: Span<'static>,
    subsequent_prefix: Span<'static>,
) -> Vec<HyperlinkLine> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, mut line)| {
            let prefix = if index == 0 {
                initial_prefix.clone()
            } else {
                subsequent_prefix.clone()
            };
            let shift = display_width(prefix.content.as_ref());
            let mut spans = Vec::with_capacity(line.line.spans.len() + 1);
            spans.push(prefix);
            spans.extend(line.line.spans);
            line.line = Line::from(spans).style(line.line.style);
            for hyperlink in &mut line.hyperlinks {
                hyperlink.columns = hyperlink.columns.start + shift..hyperlink.columns.end + shift;
            }
            for image in &mut line.kitty_images {
                image.columns = image.columns.start + shift..image.columns.end + shift;
            }
            line
        })
        .collect()
}

pub(crate) fn adaptive_wrap_hyperlink_line(
    line: &HyperlinkLine,
    options: RtOptions<'_>,
) -> Vec<HyperlinkLine> {
    wrap_hyperlink_line(line, options, WrapKind::Adaptive)
}

pub(crate) fn word_wrap_hyperlink_line(
    line: &HyperlinkLine,
    options: RtOptions<'_>,
) -> Vec<HyperlinkLine> {
    wrap_hyperlink_line(line, options, WrapKind::Word)
}

enum WrapKind {
    Adaptive,
    Word,
}

fn wrap_hyperlink_line(
    line: &HyperlinkLine,
    options: RtOptions<'_>,
    wrap_kind: WrapKind,
) -> Vec<HyperlinkLine> {
    let wrapped = match wrap_kind {
        WrapKind::Adaptive => adaptive_wrap_line(&line.line, options),
        WrapKind::Word => word_wrap_line(&line.line, options),
    }
    .into_iter()
    .map(|line| line_to_static(&line))
    .collect();
    let wrapped = remap_wrapped_line(line, wrapped);
    if kitty_images_remain_intact(line, &wrapped) {
        wrapped
    } else {
        // Splitting an image placeholder corrupts both its Kitty metadata and its visible
        // placeholder grid. An over-wide intact row is the safer fail-closed result.
        vec![line.clone()]
    }
}

fn kitty_images_remain_intact(source: &HyperlinkLine, wrapped: &[HyperlinkLine]) -> bool {
    source
        .kitty_images
        .iter()
        .map(|annotation| (&annotation.image, annotation.columns.len()))
        .eq(wrapped
            .iter()
            .flat_map(|line| &line.kitty_images)
            .map(|annotation| (&annotation.image, annotation.columns.len())))
}

pub(crate) fn adaptive_wrap_hyperlink_lines(
    lines: &[HyperlinkLine],
    options: RtOptions<'static>,
) -> Vec<HyperlinkLine> {
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let options = if index == 0 {
            options.clone()
        } else {
            options
                .clone()
                .initial_indent(options.subsequent_indent.clone())
        };
        out.extend(adaptive_wrap_hyperlink_line(line, options));
    }
    out
}

pub(crate) fn annotate_web_urls(lines: Vec<Line<'static>>) -> Vec<HyperlinkLine> {
    lines.into_iter().map(annotate_web_urls_in_line).collect()
}

pub(crate) fn annotate_web_urls_in_line(line: Line<'static>) -> HyperlinkLine {
    let text = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect::<String>();
    let mut out = HyperlinkLine::new(line);
    out.hyperlinks = web_links_in_text(&text);
    out
}

/// Re-attach source terminal annotations after visible-text wrapping has split a line.
///
/// Text is matched in display order so repeated labels remain associated with their original
/// source ranges. Whitespace inserted or removed at line boundaries is ignored while matching.
pub(crate) fn remap_wrapped_line(
    source: &HyperlinkLine,
    wrapped: Vec<Line<'static>>,
) -> Vec<HyperlinkLine> {
    let mut out = plain_hyperlink_lines(wrapped);
    if source.hyperlinks.is_empty() && source.kitty_images.is_empty() {
        return out;
    }

    let source_text = line_text(&source.line);
    let mut source_byte = 0usize;
    let mut source_column = 0usize;
    let mut link_index = 0usize;
    let mut image_index = 0usize;
    for (index, line) in out.iter_mut().enumerate() {
        let mut mapped_image_index = None;
        if index > 0 {
            let trimmed = source_text[source_byte..].trim_start_matches(char::is_whitespace);
            let skipped = source_text[source_byte..].len() - trimmed.len();
            source_column += source_text[source_byte..source_byte + skipped]
                .chars()
                .map(char_cell_width)
                .sum::<usize>();
            source_byte += skipped;
        }

        let rendered = line_text(&line.line);
        let remaining = &source_text[source_byte..];
        let Some(rendered_start) = longest_suffix_matching_prefix(&rendered, remaining) else {
            continue;
        };
        let mapped = &rendered[rendered_start..];
        let mut output_column = display_width(&rendered[..rendered_start]);
        for ch in mapped.chars() {
            let width = char_cell_width(ch);
            while source
                .hyperlinks
                .get(link_index)
                .is_some_and(|link| link.columns.end <= source_column)
            {
                link_index += 1;
            }
            while source
                .kitty_images
                .get(image_index)
                .is_some_and(|image| image.columns.end <= source_column)
            {
                image_index += 1;
            }
            if let Some(link) = source
                .hyperlinks
                .get(link_index)
                .filter(|link| link.columns.contains(&source_column))
            {
                push_link_range(line, output_column..output_column + width, link);
            }
            let current_image_index = source
                .kitty_images
                .get(image_index)
                .is_some_and(|image| image.columns.contains(&source_column))
                .then_some(image_index);
            if let Some(current_image_index) = current_image_index {
                let continue_previous = mapped_image_index == Some(current_image_index);
                push_kitty_image_range(
                    line,
                    output_column..output_column + width,
                    continue_previous,
                    &source.kitty_images[current_image_index].image,
                );
            }
            mapped_image_index = current_image_index;
            source_column += width;
            output_column += width;
        }
        source_byte += mapped.len();
    }
    out
}

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

fn longest_suffix_matching_prefix(rendered: &str, source: &str) -> Option<usize> {
    rendered
        .char_indices()
        .map(|(index, _)| index)
        .chain(std::iter::once(rendered.len()))
        .find(|index| source.starts_with(&rendered[*index..]) && *index < rendered.len())
}

fn push_link_range(line: &mut HyperlinkLine, range: Range<usize>, link: &TerminalHyperlink) {
    if range.is_empty() {
        return;
    }
    if let Some(previous) = line.hyperlinks.last_mut()
        && previous.destination == link.destination
        && previous.destination_kind == link.destination_kind
        && previous.columns.end == range.start
    {
        previous.columns.end = range.end;
        return;
    }
    line.hyperlinks.push(link.with_columns(range));
}

fn push_kitty_image_range(
    line: &mut HyperlinkLine,
    range: Range<usize>,
    continue_previous: bool,
    image: &KittyImage,
) {
    if range.is_empty() {
        return;
    }
    if let Some(previous) = line.kitty_images.last_mut()
        && continue_previous
        && previous.columns.end == range.start
    {
        previous.columns.end = range.end;
        return;
    }
    line.kitty_images.push(KittyImageAnnotation {
        columns: range,
        image: image.clone(),
    });
}

pub(crate) fn web_links_in_text(text: &str) -> Vec<TerminalHyperlink> {
    let mut links = Vec::new();
    let mut search_from = 0usize;
    for raw_token in text.split_ascii_whitespace() {
        let Some(relative_start) = text[search_from..].find(raw_token) else {
            continue;
        };
        let raw_start = search_from + relative_start;
        search_from = raw_start + raw_token.len();
        let trimmed_start = raw_token
            .find(|ch: char| !is_leading_punctuation(ch))
            .unwrap_or(raw_token.len());
        let trimmed_end = trailing_url_end(&raw_token[trimmed_start..]) + trimmed_start;
        if trimmed_start >= trimmed_end {
            continue;
        }
        let candidate = &raw_token[trimmed_start..trimmed_end];
        let Some(destination) = web_destination(candidate) else {
            continue;
        };
        let start = display_width(&text[..raw_start + trimmed_start]);
        let end = start + display_width(candidate);
        links.push(TerminalHyperlink::web(start..end, destination));
    }
    links
}

fn is_leading_punctuation(ch: char) -> bool {
    matches!(
        ch,
        '(' | ')' | '[' | ']' | '{' | '}' | '<' | '>' | ',' | '.' | ';' | '!' | '\'' | '"'
    )
}

fn trailing_url_end(candidate: &str) -> usize {
    let mut end = candidate.len();
    while end > 0 {
        let remaining = &candidate[..end];
        let Some(ch) = remaining.chars().next_back() else {
            break;
        };
        let trim = matches!(ch, ',' | '.' | ';' | '!' | '\'' | '"')
            || matches!(ch, ')' | ']' | '}' | '>')
                && has_unmatched_closing_delimiter(remaining, ch);
        if !trim {
            break;
        }
        end -= ch.len_utf8();
    }
    end
}

fn has_unmatched_closing_delimiter(candidate: &str, closing: char) -> bool {
    let opening = match closing {
        ')' => '(',
        ']' => '[',
        '}' => '{',
        '>' => '<',
        _ => return false,
    };
    candidate.chars().filter(|ch| *ch == closing).count()
        > candidate.chars().filter(|ch| *ch == opening).count()
}

pub(crate) fn web_destination(destination: &str) -> Option<String> {
    let safe_destination = sanitized_destination(destination);
    let parsed = Url::parse(&safe_destination).ok()?;
    matches!(parsed.scheme(), "http" | "https")
        .then(|| parsed.host_str())
        .flatten()?;
    Some(safe_destination)
}

fn trusted_file_destination(destination: &str) -> Option<String> {
    let safe_destination = sanitized_destination(destination);
    let parsed = Url::parse(&safe_destination).ok()?;
    (parsed.scheme() == "file" && parsed.to_file_path().is_ok()).then_some(safe_destination)
}

fn sanitized_destination(destination: &str) -> String {
    destination.chars().filter(|ch| !ch.is_control()).collect()
}

pub(crate) fn osc8_hyperlink(destination: &str, text: &str) -> String {
    let Some(safe_destination) = web_destination(destination) else {
        return text.to_string();
    };
    format!("\x1b]8;;{safe_destination}\x07{text}\x1b]8;;\x07")
}

#[cfg(test)]
pub(crate) fn strip_osc8(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut stripped = String::with_capacity(text.len());
    let mut index = 0usize;

    while index < bytes.len() {
        if bytes[index..].starts_with(b"\x1b]8;;") {
            index += 5;
            while index < bytes.len() {
                if bytes[index] == b'\x07' {
                    index += 1;
                    break;
                }
                if index + 1 < bytes.len() && bytes[index] == b'\x1b' && bytes[index + 1] == b'\\' {
                    index += 2;
                    break;
                }
                index += 1;
            }
            continue;
        }
        let ch = text[index..]
            .chars()
            .next()
            .expect("current byte index starts a character");
        stripped.push(ch);
        index += ch.len_utf8();
    }

    stripped
}

pub(crate) fn decorate_spans(line: &HyperlinkLine) -> Vec<Span<'static>> {
    if line.hyperlinks.is_empty() {
        return line.line.spans.clone();
    }

    let mut out = Vec::new();
    let mut column = 0usize;
    let mut link_index = 0usize;
    let mut active_link_index = None;
    let mut active_destination: Option<String> = None;
    for span in &line.line.spans {
        for ch in span.content.chars() {
            let width = char_cell_width(ch);
            while line
                .hyperlinks
                .get(link_index)
                .is_some_and(|link| link.columns.end <= column)
            {
                link_index += 1;
            }
            let selected_link_index = line
                .hyperlinks
                .get(link_index)
                .and_then(|link| link.columns.contains(&column).then_some(link_index));
            if active_link_index != selected_link_index {
                if active_destination.is_some() {
                    append_to_last_span(&mut out, "\x1b]8;;\x07");
                }
                active_destination = selected_link_index
                    .and_then(|index| line.hyperlinks[index].terminal_destination());
                if let Some(destination) = active_destination.as_ref() {
                    push_styled_content(
                        &mut out,
                        &format!("\x1b]8;;{destination}\x07"),
                        span.style,
                    );
                }
                active_link_index = selected_link_index;
            }
            push_styled_content(&mut out, &ch.to_string(), span.style);
            column += width;
        }
    }
    if active_destination.is_some() {
        append_to_last_span(&mut out, "\x1b]8;;\x07");
    }
    out
}

fn push_styled_content(out: &mut Vec<Span<'static>>, content: &str, style: ratatui::style::Style) {
    if let Some(last) = out.last_mut()
        && last.style == style
    {
        last.content.to_mut().push_str(content);
        return;
    }
    out.push(Span::styled(content.to_string(), style));
}

fn append_to_last_span(out: &mut [Span<'static>], content: &str) {
    if let Some(last) = out.last_mut() {
        last.content.to_mut().push_str(content);
    }
}

fn char_cell_width(ch: char) -> usize {
    if ch.is_control() {
        return 0;
    }
    char_width(ch)
}

pub(crate) fn mark_buffer_hyperlinks(
    buf: &mut Buffer,
    area: Rect,
    lines: &[HyperlinkLine],
    scroll_rows: usize,
) {
    if area.width == 0 {
        return;
    }
    let mut logical_row = 0usize;
    for line in lines {
        let paragraph =
            Paragraph::new(Text::from(line_to_borrowed(&line.line))).wrap(Wrap { trim: false });
        let rendered_height = paragraph.line_count(area.width).max(/*other*/ 1);
        if line.hyperlinks.is_empty() {
            logical_row += rendered_height;
            continue;
        }

        let layout_area = Rect::new(
            /*x*/ 0,
            /*y*/ 0,
            area.width,
            u16::try_from(rendered_height).unwrap_or(u16::MAX),
        );
        let mut layout = Buffer::empty(layout_area);
        paragraph.render(layout_area, &mut layout);
        let rendered_lines = (0..layout_area.height)
            .map(|row| {
                let mut trailing_columns = 0usize;
                let text = (0..layout_area.width)
                    .filter_map(|column| {
                        if trailing_columns > 0 {
                            trailing_columns -= 1;
                            return None;
                        }
                        let cell = &layout[(column, row)];
                        if cell.diff_option == CellDiffOption::Skip {
                            return None;
                        }
                        trailing_columns = usize::from(cell.cell_width()).saturating_sub(1);
                        Some(cell.symbol())
                    })
                    .collect::<String>();
                Line::from(text.trim_end().to_string())
            })
            .collect();
        for (row, rendered) in remap_wrapped_line(line, rendered_lines).iter().enumerate() {
            for link in &rendered.hyperlinks {
                let mut trailing_columns = 0usize;
                for column in link.columns.clone() {
                    if trailing_columns > 0 {
                        trailing_columns -= 1;
                        continue;
                    }
                    let row = logical_row + row;
                    if row < scroll_rows || row - scroll_rows >= usize::from(area.height) {
                        continue;
                    }
                    let x = area.x + column as u16;
                    let y = area.y + (row - scroll_rows) as u16;
                    let cell = &mut buf[(x, y)];
                    if cell.diff_option == CellDiffOption::Skip {
                        continue;
                    }
                    trailing_columns = usize::from(cell.cell_width()).saturating_sub(1);
                    let symbol = link.terminal_destination().map_or_else(
                        || cell.symbol().to_string(),
                        |destination| {
                            format!("\x1b]8;;{destination}\x07{}\x1b]8;;\x07", cell.symbol())
                        },
                    );
                    let width = NonZeroU16::new(cell.cell_width()).unwrap_or(NonZeroU16::MIN);
                    cell.set_symbol(&symbol)
                        .set_diff_option(CellDiffOption::ForcedWidth(width));
                }
            }
        }
        logical_row += rendered_height;
    }
}

pub(crate) fn mark_url_hyperlink(buf: &mut Buffer, area: Rect, destination: &str) {
    mark_matching_cells(buf, area, destination, |cell| {
        cell.fg == Color::Cyan && cell.modifier.contains(Modifier::UNDERLINED)
    });
}

pub(crate) fn mark_underlined_hyperlink(buf: &mut Buffer, area: Rect, destination: &str) {
    mark_matching_cells(buf, area, destination, |cell| {
        cell.modifier.contains(Modifier::UNDERLINED)
    });
}

fn mark_matching_cells(
    buf: &mut Buffer,
    area: Rect,
    destination: &str,
    matches: impl Fn(&ratatui::buffer::Cell) -> bool,
) {
    if web_destination(destination).is_none() {
        return;
    }
    for position in area.positions() {
        let cell = &mut buf[position];
        if cell.diff_option != CellDiffOption::Skip
            && !cell.symbol().trim().is_empty()
            && matches(cell)
        {
            let width = NonZeroU16::new(cell.cell_width()).unwrap_or(NonZeroU16::MIN);
            let symbol = osc8_hyperlink(destination, cell.symbol());
            cell.set_symbol(&symbol)
                .set_diff_option(CellDiffOption::ForcedWidth(width));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wrapping::RtOptions;
    use pretty_assertions::assert_eq;
    use ratatui::style::Style;

    #[test]
    fn only_web_destinations_receive_osc8() {
        assert!(osc8_hyperlink("https://example.com/a", "a").contains("\x1b]8;;"));
        assert_eq!(osc8_hyperlink("mailto:a@example.com", "a"), "a");
        assert_eq!(
            osc8_hyperlink("https://example.com/\u{7}safe", "a"),
            "\x1b]8;;https://example.com/safe\x07a\x1b]8;;\x07"
        );
        assert_eq!(
            strip_osc8(&osc8_hyperlink("https://example.com/a", "visible")),
            "visible"
        );
    }

    #[test]
    fn discovers_punctuated_web_url_columns() {
        assert_eq!(
            web_links_in_text("See (https://example.com/a)."),
            vec![TerminalHyperlink::web(
                /*columns*/ 5..26,
                "https://example.com/a".to_string(),
            )]
        );
    }

    #[test]
    fn hyperlink_columns_follow_a_long_prefix_without_wrapping() {
        let prefix = "a".repeat(65_536);
        let destination = "https://example.com/long-prefix";
        let text = format!("{prefix} {destination}");

        assert_eq!(
            HyperlinkLine::new(Line::from(text.clone())).width(),
            text.len()
        );
        assert_eq!(
            web_links_in_text(&text),
            vec![TerminalHyperlink::web(
                /*columns*/ 65_537..65_537 + destination.len(),
                destination.to_string(),
            )]
        );
    }

    #[test]
    fn preserves_balanced_parentheses_in_bare_web_urls() {
        let destination = "https://en.wikipedia.org/wiki/Function_(mathematics)";
        assert_eq!(
            web_links_in_text(&format!("See ({destination}).")),
            vec![TerminalHyperlink::web(
                /*columns*/ 5..5 + usize::from(destination.cell_width()),
                destination.to_string(),
            )]
        );
    }

    #[test]
    fn decorates_a_contiguous_web_link_with_one_osc8_pair() {
        let destination = "https://example.com/a/very/long/path";
        let line = HyperlinkLine {
            line: Line::from(destination),
            hyperlinks: vec![TerminalHyperlink::web(
                /*columns*/ 0..usize::from(destination.cell_width()),
                destination.to_string(),
            )],
            kitty_images: Vec::new(),
        };

        assert_eq!(
            decorate_spans(&line),
            vec![Span::from(osc8_hyperlink(destination, destination))]
        );
        assert_eq!(
            decorate_spans(&HyperlinkLine::new(Line::from("not linked"))),
            vec![Span::from("not linked")]
        );
    }

    #[test]
    fn wrapping_maps_repeated_link_labels_by_source_position() {
        let mut source = HyperlinkLine::new(Line::from("here here"));
        source.hyperlinks.push(TerminalHyperlink::web(
            /*columns*/ 5..9,
            "https://example.com".to_string(),
        ));

        let wrapped = remap_wrapped_line(&source, vec![Line::from("here here")]);

        assert_eq!(
            wrapped[0].hyperlinks,
            vec![TerminalHyperlink::web(
                /*columns*/ 5..9,
                "https://example.com".to_string(),
            )]
        );
    }

    #[test]
    fn image_annotation_moves_to_one_wrapped_row() {
        let image = KittyImage::new(
            b"png".to_vec(),
            /*image_id*/ 42,
            /*columns*/ 6,
            /*rows*/ 1,
        );
        let mut source = HyperlinkLine::new(Line::from(vec![
            "before ".into(),
            "MMMMMM".into(),
            ", after".into(),
        ]));
        source.kitty_images.push(KittyImageAnnotation {
            columns: 7..13,
            image: image.clone(),
        });

        let wrapped = adaptive_wrap_hyperlink_line(&source, RtOptions::new(/*width*/ 10));

        assert_eq!(
            wrapped
                .iter()
                .map(|line| line.line.to_string().trim_end().to_string())
                .collect::<Vec<_>>(),
            vec!["before", "MMMMMM,", "after"],
        );
        assert_eq!(
            wrapped
                .iter()
                .map(|line| line.kitty_images.clone())
                .collect::<Vec<_>>(),
            vec![
                Vec::new(),
                vec![KittyImageAnnotation {
                    columns: 0..6,
                    image,
                }],
                Vec::new(),
            ],
        );
    }

    #[test]
    fn adjacent_image_annotations_stay_distinct_across_rewraps() {
        let first_image = KittyImage::new(
            b"first".to_vec(),
            /*image_id*/ 41,
            /*columns*/ 4,
            /*rows*/ 1,
        );
        let second_image = KittyImage::new(
            b"second".to_vec(),
            /*image_id*/ 42,
            /*columns*/ 4,
            /*rows*/ 1,
        );
        let mut source = HyperlinkLine::new(Line::from(vec!["AAAA".into(), "BBBB".into()]));
        source.kitty_images = vec![
            KittyImageAnnotation {
                columns: 0..4,
                image: first_image.clone(),
            },
            KittyImageAnnotation {
                columns: 4..8,
                image: second_image.clone(),
            },
        ];

        let once = adaptive_wrap_hyperlink_line(&source, RtOptions::new(/*width*/ 8));
        assert_eq!(once.len(), 1);
        assert_eq!(
            once[0].kitty_images,
            vec![
                KittyImageAnnotation {
                    columns: 0..4,
                    image: first_image.clone(),
                },
                KittyImageAnnotation {
                    columns: 4..8,
                    image: second_image.clone(),
                },
            ],
        );

        let twice = adaptive_wrap_hyperlink_line(&once[0], RtOptions::new(/*width*/ 4));
        assert_eq!(
            twice
                .iter()
                .map(|line| {
                    (
                        line.line.to_string(),
                        line.kitty_images
                            .iter()
                            .map(|annotation| annotation.image.image_id())
                            .collect::<Vec<_>>(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                ("AAAA".to_string(), vec![first_image.image_id()]),
                ("BBBB".to_string(), vec![second_image.image_id()]),
            ],
        );
    }

    #[test]
    fn word_wrap_fails_closed_when_an_image_would_split() {
        let image = KittyImage::new(
            b"png".to_vec(),
            /*image_id*/ 42,
            /*columns*/ 8,
            /*rows*/ 1,
        );
        let source = HyperlinkLine {
            line: Line::from("MMMMMMMM"),
            hyperlinks: Vec::new(),
            kitty_images: vec![KittyImageAnnotation {
                columns: 0..8,
                image,
            }],
        };

        let once = word_wrap_hyperlink_line(&source, RtOptions::new(/*width*/ 4));
        let twice = word_wrap_hyperlink_line(&once[0], RtOptions::new(/*width*/ 4));

        assert_eq!(once, vec![source]);
        assert_eq!(twice, once);
    }

    #[test]
    fn word_wrap_with_an_image_still_breaks_an_ordinary_long_token() {
        let image = KittyImage::new(
            b"png".to_vec(),
            /*image_id*/ 42,
            /*columns*/ 4,
            /*rows*/ 1,
        );
        let source = HyperlinkLine {
            line: Line::from(vec!["MMMM".into(), " ".into(), "abcdefghij".into()]),
            hyperlinks: Vec::new(),
            kitty_images: vec![KittyImageAnnotation {
                columns: 0..4,
                image,
            }],
        };

        let wrapped = word_wrap_hyperlink_line(&source, RtOptions::new(/*width*/ 5));

        assert_eq!(
            wrapped
                .iter()
                .map(|line| line.line.to_string())
                .collect::<Vec<_>>(),
            vec!["MMMM", "abcde", "fghij"],
        );
        assert!(wrapped.iter().all(|line| line.width() <= 5));
        assert_eq!(
            wrapped
                .iter()
                .map(|line| {
                    line.kitty_images
                        .iter()
                        .map(|annotation| annotation.columns.clone())
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>(),
            vec![vec![0..4], Vec::new(), Vec::new()],
        );
    }

    #[test]
    fn wrapping_maps_multiple_links_across_indented_unicode_lines() {
        let text = "alpha 😀here middle there end";
        let first_start = text.find("here").expect("first link");
        let second_start = text.find("there").expect("second link");
        let first_column = usize::from(text[..first_start].cell_width());
        let second_column = usize::from(text[..second_start].cell_width());
        let mut source = HyperlinkLine::new(Line::from(text));
        source.hyperlinks.push(TerminalHyperlink::web(
            first_column..first_column + usize::from("here".cell_width()),
            "https://example.com/first".to_string(),
        ));
        source.hyperlinks.push(TerminalHyperlink::web(
            second_column..second_column + usize::from("there".cell_width()),
            "https://example.com/second".to_string(),
        ));

        let wrapped = remap_wrapped_line(
            &source,
            vec![
                Line::from("  alpha 😀here"),
                Line::from("    middle there end"),
            ],
        );

        assert_eq!(
            wrapped,
            vec![
                HyperlinkLine {
                    line: Line::from("  alpha 😀here"),
                    hyperlinks: vec![TerminalHyperlink::web(
                        /*columns*/ 10..14,
                        "https://example.com/first".to_string(),
                    )],
                    kitty_images: Vec::new(),
                },
                HyperlinkLine {
                    line: Line::from("    middle there end"),
                    hyperlinks: vec![TerminalHyperlink::web(
                        /*columns*/ 11..16,
                        "https://example.com/second".to_string(),
                    )],
                    kitty_images: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn buffer_hyperlinks_follow_word_wrapping() {
        let destination = "https://example.com/path";
        let mut line = HyperlinkLine::new(Line::from(format!("See {destination} now")));
        line.hyperlinks.push(TerminalHyperlink::web(
            /*columns*/ 4..4 + usize::from(destination.cell_width()),
            destination.to_string(),
        ));
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 18, /*height*/ 4,
        );
        let mut buf = Buffer::empty(area);

        Paragraph::new(Text::from(line.line.clone()))
            .wrap(Wrap { trim: false })
            .render(area, &mut buf);
        mark_buffer_hyperlinks(&mut buf, area, &[line], /*scroll_rows*/ 0);

        let linked_text = area
            .positions()
            .filter_map(|position| {
                let symbol = buf[position].symbol();
                symbol
                    .contains(&format!("\x1b]8;;{destination}\x07"))
                    .then(|| strip_osc8(symbol))
            })
            .collect::<String>();
        assert_eq!(linked_text, destination);
    }

    #[test]
    fn buffer_hyperlinks_follow_wrapped_wide_glyphs() {
        let destination = "https://example.com/wide";
        let mut line = HyperlinkLine::new(Line::from("前文 "));
        line.push_span("漢字漢字".into(), Some(destination));
        line.push_span(" 後文".into(), /*destination*/ None);
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 6, /*height*/ 4,
        );
        let mut buf = Buffer::empty(area);

        Paragraph::new(Text::from(line.line.clone()))
            .wrap(Wrap { trim: false })
            .render(area, &mut buf);
        mark_buffer_hyperlinks(&mut buf, area, &[line], /*scroll_rows*/ 0);

        let linked_text = area
            .positions()
            .filter_map(|position| {
                let symbol = buf[position].symbol();
                symbol
                    .contains(&format!("\x1b]8;;{destination}\x07"))
                    .then(|| strip_osc8(symbol))
            })
            .collect::<String>();
        assert_eq!(linked_text, "漢字漢字");
    }

    #[test]
    fn buffer_hyperlinks_follow_wrapped_halfwidth_dakuten() {
        let destination = "https://example.com/dakuten";
        let mut line = HyperlinkLine::new(Line::from("ｶﾞ "));
        line.push_span("ﾊﾟlink".into(), Some(destination));
        line.push_span(" tail".into(), /*destination*/ None);
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 5, /*height*/ 4,
        );
        let mut buf = Buffer::empty(area);

        Paragraph::new(Text::from(line.line.clone()))
            .wrap(Wrap { trim: false })
            .render(area, &mut buf);
        mark_buffer_hyperlinks(&mut buf, area, &[line], /*scroll_rows*/ 0);

        let linked_text = area
            .positions()
            .filter_map(|position| {
                let symbol = buf[position].symbol();
                symbol
                    .contains(&format!("\x1b]8;;{destination}\x07"))
                    .then(|| strip_osc8(symbol))
            })
            .collect::<String>();
        assert_eq!(linked_text, "ﾊﾟlink");
    }

    #[test]
    fn forced_width_hyperlinks_render_wide_and_halfwidth_cells_snapshot() {
        let destination = "https://example.com/rendered";
        let mut line = HyperlinkLine::new(Line::from("prefix "));
        line.push_span("漢字 ｶﾞ".into(), Some(destination));
        line.push_span(" tail".into(), /*destination*/ None);

        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 14, /*height*/ 3,
        );
        let backend = crate::test_backend::VT100Backend::new(area.width, area.height);
        let mut terminal =
            crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
        terminal.set_viewport_area(area);

        terminal
            .draw(|frame| {
                Paragraph::new(Text::from(line.line.clone()))
                    .wrap(Wrap { trim: false })
                    .render(area, frame.buffer_mut());
                mark_buffer_hyperlinks(
                    frame.buffer_mut(),
                    area,
                    &[line.clone()],
                    /*scroll_rows*/ 0,
                );
            })
            .expect("render hyperlinks");

        insta::assert_snapshot!(
            "forced_width_hyperlinks_render_wide_and_halfwidth_cells",
            terminal.backend()
        );
    }

    #[test]
    fn buffer_hyperlinks_preserve_visible_cell_width_for_ratatui_diff() {
        let destination = "https://example.com/dakuten";
        let mut line = HyperlinkLine::new(Line::from("ｶﾞ tail"));
        line.hyperlinks.push(TerminalHyperlink::web(
            /*columns*/ 0..2,
            destination.to_string(),
        ));
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 7, /*height*/ 1,
        );
        let previous = Buffer::with_lines(["       "]);
        let mut next = Buffer::empty(area);

        Paragraph::new(Text::from(line.line.clone())).render(area, &mut next);
        mark_buffer_hyperlinks(&mut next, area, &[line], /*scroll_rows*/ 0);

        assert_eq!(next[(0, 0)].cell_width(), 2);
        assert!(matches!(
            next[(0, 0)].diff_option,
            CellDiffOption::ForcedWidth(width) if width.get() == 2
        ));
        assert_eq!(
            previous
                .diff_iter(&next)
                .map(|(x, _, cell)| (x, strip_osc8(cell.symbol())))
                .collect::<Vec<_>>(),
            vec![
                (0, "ｶﾞ".to_string()),
                (3, "t".to_string()),
                (4, "a".to_string()),
                (5, "i".to_string()),
                (6, "l".to_string()),
            ]
        );
    }

    #[test]
    fn matching_hyperlinks_preserve_visible_cell_width_for_ratatui_diff() {
        let destination = "https://example.com/dakuten";
        let area = Rect::new(
            /*x*/ 0, /*y*/ 0, /*width*/ 7, /*height*/ 1,
        );
        let previous = Buffer::with_lines(["       "]);
        let mut next = Buffer::empty(area);
        next.set_string(
            /*x*/ 0,
            /*y*/ 0,
            "ｶﾞ tail",
            Style::default().add_modifier(Modifier::UNDERLINED),
        );

        mark_underlined_hyperlink(&mut next, area, destination);

        assert_eq!(next[(0, 0)].cell_width(), 2);
        assert!(matches!(
            next[(0, 0)].diff_option,
            CellDiffOption::ForcedWidth(width) if width.get() == 2
        ));
        assert_eq!(
            previous
                .diff_iter(&next)
                .map(|(x, _, cell)| (x, strip_osc8(cell.symbol())))
                .collect::<Vec<_>>(),
            vec![
                (0, "ｶﾞ".to_string()),
                (2, " ".to_string()),
                (3, "t".to_string()),
                (4, "a".to_string()),
                (5, "i".to_string()),
                (6, "l".to_string()),
            ]
        );
    }

    #[test]
    fn trusted_file_destination_receives_osc8_without_enabling_plain_file_links() {
        let temp_dir = tempfile::tempdir().expect("temp directory");
        let file_url = Url::from_file_path(temp_dir.path().join("viewer.html"))
            .expect("test path should convert to file URL");
        let mut link = TerminalHyperlink::web(
            /*columns*/ 0..4,
            "https://codex.invalid/viewer".to_string(),
        );
        link.retarget_to_trusted_file(&file_url);
        let line = HyperlinkLine {
            line: Line::from("view"),
            hyperlinks: vec![link],
            kitty_images: Vec::new(),
        };

        assert_eq!(
            decorate_spans(&line),
            vec![Span::from(format!(
                "\x1b]8;;{file_url}\x07view\x1b]8;;\x07"
            ))]
        );
        assert_eq!(osc8_hyperlink(file_url.as_str(), "view"), "view");
    }
}
