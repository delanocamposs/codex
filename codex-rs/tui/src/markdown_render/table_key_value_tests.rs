use pretty_assertions::assert_eq;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use super::render_records;
use crate::markdown_render::TableCell;
use crate::markdown_render::TableColumnKind;
use crate::markdown_render::TableColumnMetrics;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::KittyImageAnnotation;
use crate::terminal_image::KittyImage;

fn metrics() -> Vec<TableColumnMetrics> {
    vec![TableColumnMetrics {
        max_width: 8,
        header_token_width: 4,
        body_token_width: 5,
        kind: TableColumnKind::Compact,
    }]
}

fn image_cell(image: &KittyImage) -> TableCell {
    let image_style = Style::new().underline_color(Color::Rgb(1, 2, 3));
    TableCell {
        lines: vec![HyperlinkLine {
            line: Line::from(Span::styled("MMMM", image_style)),
            hyperlinks: Vec::new(),
            kitty_images: vec![KittyImageAnnotation {
                columns: 0..4,
                image: image.clone(),
            }],
        }],
    }
}

fn text_cell(text: &str) -> TableCell {
    TableCell {
        lines: vec![HyperlinkLine::new(Line::from(text.to_string()))],
    }
}

#[test]
fn aligned_record_header_preserves_image_annotations() {
    let image = KittyImage::new(
        b"png".to_vec(),
        /*image_id*/ 42,
        /*columns*/ 4,
        /*rows*/ 1,
    );
    let label_style = Style::new().add_modifier(Modifier::BOLD);

    let rendered = render_records(
        &[image_cell(&image)],
        &[vec![text_cell("value")]],
        &metrics(),
        /*available_width*/ Some(24),
        label_style,
        Style::default(),
    );

    assert_eq!(
        rendered,
        vec![HyperlinkLine {
            line: Line::from(vec![
                Span::raw(" "),
                Span::styled(
                    "MMMM",
                    label_style.patch(Style::new().underline_color(Color::Rgb(1, 2, 3))),
                ),
                Span::raw("  "),
                Span::raw("value"),
            ]),
            hyperlinks: Vec::new(),
            kitty_images: vec![KittyImageAnnotation {
                columns: 1..5,
                image,
            }],
        }],
    );
}

#[test]
fn stacked_record_header_preserves_image_annotations() {
    let image = KittyImage::new(
        b"png".to_vec(),
        /*image_id*/ 42,
        /*columns*/ 4,
        /*rows*/ 1,
    );
    let label_style = Style::new().add_modifier(Modifier::BOLD);

    let rendered = render_records(
        &[image_cell(&image)],
        &[vec![text_cell("value")]],
        &metrics(),
        /*available_width*/ Some(10),
        label_style,
        Style::default(),
    );

    assert_eq!(
        rendered,
        vec![
            HyperlinkLine {
                line: Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        "MMMM",
                        label_style.patch(Style::new().underline_color(Color::Rgb(1, 2, 3)),),
                    ),
                ]),
                hyperlinks: Vec::new(),
                kitty_images: vec![KittyImageAnnotation {
                    columns: 1..5,
                    image,
                }],
            },
            HyperlinkLine::new(Line::from(vec![Span::raw("  "), Span::raw("value"),])),
        ],
    );
}

#[test]
fn stacked_record_drops_indent_before_viewport_wide_image() {
    let image = KittyImage::new(
        b"png".to_vec(),
        /*image_id*/ 42,
        /*columns*/ 8,
        /*rows*/ 1,
    );
    let value = TableCell {
        lines: vec![HyperlinkLine {
            line: Line::from("MMMMMMMM"),
            hyperlinks: Vec::new(),
            kitty_images: vec![KittyImageAnnotation {
                columns: 0..8,
                image: image.clone(),
            }],
        }],
    };

    let rendered = render_records(
        &[text_cell("Key")],
        &[vec![value]],
        &metrics(),
        /*available_width*/ Some(8),
        Style::default(),
        Style::default(),
    );

    assert_eq!(
        rendered,
        vec![
            HyperlinkLine::new(Line::from(vec![Span::raw(" "), Span::raw("Key"),])),
            HyperlinkLine {
                line: Line::from("MMMMMMMM"),
                hyperlinks: Vec::new(),
                kitty_images: vec![KittyImageAnnotation {
                    columns: 0..8,
                    image,
                }],
            },
        ],
    );
    assert!(rendered.iter().all(|line| line.width() <= 8));
}
