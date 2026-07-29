//! Vertical key/value rendering for markdown tables that no longer scan well as grids.

use super::TABLE_BODY_SEPARATOR_CHAR;
use super::TableCell;
use super::TableColumnKind;
use super::TableColumnMetrics;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::word_wrap_hyperlink_line;
use crate::width::display_width;
use crate::wrapping::RtOptions;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

const FIELD_LEADING_PADDING: usize = 1;
const FIELD_GAP: usize = 2;
const MIN_VALUE_WIDTH: usize = 3;
const MIN_ALIGNED_COMPACT_VALUE_WIDTH: usize = 12;
const MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH: usize = 24;
const MIN_SCANNABLE_NARRATIVE_WIDTH: usize = 12;
const MIN_SCANNABLE_TOKEN_HEAVY_WIDTH: usize = 12;
const CRAMPED_EXPANSIVE_CELL_LINES: usize = 4;
const CATASTROPHIC_NARRATIVE_CELL_LINES: usize = 7;
const STACKED_VALUE_INDENT: usize = 2;

/// Switch modes after enough records contain values the grid can no longer
/// present in useful chunks or expansive content collapses into tall strips.
pub(super) fn should_render_records(
    rows: &[Vec<TableCell>],
    column_widths: &[usize],
    metrics: &[TableColumnMetrics],
) -> bool {
    if rows.is_empty() {
        return false;
    }

    let affected_rows = rows
        .iter()
        .filter(|row| {
            let contains_fragmented_value =
                row.iter()
                    .zip(column_widths)
                    .zip(metrics)
                    .any(|((cell, width), metrics)| {
                        let has_fragmented_token = cell
                            .plain_text()
                            .split_whitespace()
                            .any(|token| display_width(token) > *width);
                        match metrics.kind {
                            TableColumnKind::Compact => has_fragmented_token,
                            TableColumnKind::TokenHeavy => {
                                *width < MIN_SCANNABLE_TOKEN_HEAVY_WIDTH && has_fragmented_token
                            }
                            TableColumnKind::Narrative => false,
                        }
                    });

            contains_fragmented_value || expansive_cells_are_starved(row, column_widths, metrics)
        })
        .count();
    let threshold = if rows.len() == 1 {
        1
    } else {
        2.max(rows.len().div_ceil(3))
    };

    affected_rows >= threshold
}

fn expansive_cells_are_starved(
    row: &[TableCell],
    column_widths: &[usize],
    metrics: &[TableColumnMetrics],
) -> bool {
    let expansive_cells: Vec<(TableColumnKind, usize, usize)> = row
        .iter()
        .zip(column_widths)
        .zip(metrics)
        .filter(|&((_cell, _width), metrics)| metrics.kind != TableColumnKind::Compact)
        .map(|((cell, width), metrics)| (metrics.kind, *width, wrap_cell(cell, *width).len()))
        .collect();

    expansive_cells
        .iter()
        .filter(|(_, _, height)| *height >= CRAMPED_EXPANSIVE_CELL_LINES)
        .count()
        >= 2
        || expansive_cells.iter().any(|(kind, width, height)| {
            *kind == TableColumnKind::Narrative
                && *width < MIN_SCANNABLE_NARRATIVE_WIDTH
                && *height >= CATASTROPHIC_NARRATIVE_CELL_LINES
        })
}

pub(super) fn render_records(
    headers: &[TableCell],
    rows: &[Vec<TableCell>],
    metrics: &[TableColumnMetrics],
    available_width: Option<usize>,
    label_style: Style,
    separator_style: Style,
) -> Vec<HyperlinkLine> {
    let label_width = headers
        .iter()
        .map(|header| display_width(&header.plain_text()))
        .max()
        .unwrap_or(0);
    let minimum_value_width = if metrics
        .iter()
        .any(|metrics| metrics.kind != TableColumnKind::Compact)
    {
        MIN_ALIGNED_EXPANSIVE_VALUE_WIDTH
    } else {
        MIN_ALIGNED_COMPACT_VALUE_WIDTH
    };
    let aligned_fields = available_width.is_none_or(|width| {
        let value_width = width.saturating_sub(FIELD_LEADING_PADDING + label_width + FIELD_GAP);
        FIELD_LEADING_PADDING + label_width + FIELD_GAP + minimum_value_width <= width
            && rows.iter().flatten().all(|cell| {
                cell.lines.iter().all(|line| {
                    line.kitty_images
                        .iter()
                        .all(|image| image.columns.len() <= value_width)
                })
            })
    });
    let mut out = Vec::new();

    for (row_index, row) in rows.iter().enumerate() {
        for (header, value) in headers.iter().zip(row) {
            if aligned_fields {
                render_aligned_field(
                    &mut out,
                    header,
                    value,
                    label_width,
                    available_width,
                    label_style,
                );
            } else {
                render_stacked_field(&mut out, header, value, available_width, label_style);
            }
        }
        if row_index + 1 < rows.len() {
            let width = available_width.unwrap_or_else(|| widest_line_width(&out));
            out.push(HyperlinkLine::new(Line::from(Span::styled(
                TABLE_BODY_SEPARATOR_CHAR.to_string().repeat(width),
                separator_style,
            ))));
        }
    }

    out
}

fn render_aligned_field(
    out: &mut Vec<HyperlinkLine>,
    header: &TableCell,
    value: &TableCell,
    label_width: usize,
    available_width: Option<usize>,
    label_style: Style,
) {
    let value_indent = FIELD_LEADING_PADDING + label_width + FIELD_GAP;
    let value_width = available_width
        .map(|width| width.saturating_sub(value_indent).max(MIN_VALUE_WIDTH))
        .unwrap_or_else(|| cell_width(value).max(MIN_VALUE_WIDTH));
    let wrapped_value = wrap_cell(value, value_width);
    for (line_index, value_line) in wrapped_value.into_iter().enumerate() {
        let mut prefix = HyperlinkLine::new(Line::default());
        if line_index == 0 {
            let label = styled_label(header, label_style);
            let rendered_label_width = label.width();
            prefix.push_span(
                Span::raw(" ".repeat(FIELD_LEADING_PADDING)),
                /*destination*/ None,
            );
            prefix.append_annotated(label);
            prefix.push_span(
                Span::raw(" ".repeat(label_width.saturating_sub(rendered_label_width) + FIELD_GAP)),
                /*destination*/ None,
            );
        } else {
            prefix.push_span(
                Span::raw(" ".repeat(value_indent)),
                /*destination*/ None,
            );
        }
        push_prefixed_value_line(out, prefix, value_line);
    }
}

fn render_stacked_field(
    out: &mut Vec<HyperlinkLine>,
    header: &TableCell,
    value: &TableCell,
    available_width: Option<usize>,
    label_style: Style,
) {
    let label_wrap_width = available_width
        .map(|width| width.saturating_sub(FIELD_LEADING_PADDING).max(1))
        .unwrap_or_else(|| display_width(&header.plain_text()).max(1));
    let label = styled_label(header, label_style);
    let wrapped_labels = word_wrap_hyperlink_line(&label, RtOptions::new(label_wrap_width));
    for label_line in wrapped_labels {
        let leading_padding =
            fitting_leading_padding(FIELD_LEADING_PADDING, label_line.width(), available_width);
        let mut prefix = if leading_padding == 0 {
            HyperlinkLine::new(Line::default())
        } else {
            HyperlinkLine::new(Line::from(Span::raw(" ".repeat(leading_padding))))
        };
        prefix.append_annotated(label_line);
        out.push(prefix);
    }

    let value_width = available_width
        .map(|width| width.saturating_sub(STACKED_VALUE_INDENT).max(1))
        .unwrap_or_else(|| cell_width(value).max(1));
    for value_line in wrap_cell(value, value_width) {
        let leading_padding =
            fitting_leading_padding(STACKED_VALUE_INDENT, value_line.width(), available_width);
        let prefix = if leading_padding == 0 {
            HyperlinkLine::new(Line::default())
        } else {
            HyperlinkLine::new(Line::from(Span::raw(" ".repeat(leading_padding))))
        };
        push_prefixed_value_line(out, prefix, value_line);
    }
}

fn styled_label(header: &TableCell, label_style: Style) -> HyperlinkLine {
    let mut label = HyperlinkLine::new(Line::default());
    for (index, source_line) in header.lines.iter().enumerate() {
        if index > 0 {
            label.push_span(" ".into(), /*destination*/ None);
        }
        let mut source_line = source_line.clone();
        source_line.line.style = label_style.patch(source_line.line.style);
        label.append_annotated(source_line);
    }
    label
}

fn push_prefixed_value_line(
    out: &mut Vec<HyperlinkLine>,
    mut prefix: HyperlinkLine,
    value_line: HyperlinkLine,
) {
    prefix.append_annotated(value_line);
    out.push(prefix);
}

fn fitting_leading_padding(
    preferred: usize,
    content_width: usize,
    available_width: Option<usize>,
) -> usize {
    available_width
        .map(|width| preferred.min(width.saturating_sub(content_width)))
        .unwrap_or(preferred)
}

fn wrap_cell(cell: &TableCell, width: usize) -> Vec<HyperlinkLine> {
    if cell.lines.is_empty() {
        return vec![HyperlinkLine::new(Line::default())];
    }

    cell.lines
        .iter()
        .flat_map(|source_line| word_wrap_hyperlink_line(source_line, RtOptions::new(width.max(1))))
        .collect()
}

fn cell_width(cell: &TableCell) -> usize {
    cell.lines
        .iter()
        .map(HyperlinkLine::width)
        .max()
        .unwrap_or(0)
}

fn widest_line_width(lines: &[HyperlinkLine]) -> usize {
    lines.iter().map(HyperlinkLine::width).max().unwrap_or(0)
}

#[cfg(test)]
#[path = "table_key_value_tests.rs"]
mod tests;
