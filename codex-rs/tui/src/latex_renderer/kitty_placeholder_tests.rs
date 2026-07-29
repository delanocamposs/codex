use std::sync::Arc;

use crossterm::terminal::WindowSize;
use pretty_assertions::assert_eq;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;
use unicode_width::UnicodeWidthStr;

use super::kitty_placeholder::MAX_COLOR_ENCODED_IMAGE_ID;
use super::kitty_placeholder::available_image_id_at_offset;
use super::kitty_placeholder::cell_pixels_from_window_size;
use super::kitty_placeholder::image_cell_layout;
use super::kitty_placeholder::inline_image_cell_layout;
use super::kitty_placeholder::inline_placeholder_line;
use super::kitty_placeholder::placeholder_lines;
use super::*;
use crate::latex_image::LatexPng;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::KittyImageAnnotation;
use crate::terminal_image::KittyImage;

fn test_png() -> LatexPng {
    LatexPng {
        bytes: Arc::from([1, 2, 3]),
        width: 80,
        height: 32,
    }
}

#[test]
fn terminal_window_size_converts_to_cell_pixels_with_conservative_fallbacks() {
    let sizes = [
        WindowSize {
            columns: 80,
            rows: 24,
            width: 640,
            height: 384,
        },
        WindowSize {
            columns: 80,
            rows: 24,
            width: 641,
            height: 385,
        },
        WindowSize {
            columns: 0,
            rows: 24,
            width: 640,
            height: 384,
        },
        WindowSize {
            columns: 80,
            rows: 0,
            width: 640,
            height: 384,
        },
        WindowSize {
            columns: 80,
            rows: 24,
            width: 0,
            height: 0,
        },
    ];
    assert_eq!(
        sizes.map(cell_pixels_from_window_size),
        [(8, 16), (9, 17), (8, 16), (8, 16), (8, 16)],
    );
}

#[test]
fn image_layout_preserves_shape_within_bounds() {
    assert_eq!(
        image_cell_layout(
            /*image_width*/ 640,
            /*image_height*/ 80,
            /*max_columns*/ 40,
            /*cell_pixels*/ (8, 16),
        ),
        (40, 3),
    );
    assert_eq!(
        image_cell_layout(
            /*image_width*/ 160,
            /*image_height*/ 640,
            /*max_columns*/ 80,
            /*cell_pixels*/ (8, 16),
        ),
        (6, 12),
    );
}

#[test]
fn inline_layout_is_one_row_and_preserves_aspect_ratio_within_its_cap() {
    let rendered = RenderedImage::new(test_png()).expect("available image ID");

    assert_eq!(
        inline_image_cell_layout(
            rendered.png.width,
            rendered.png.height,
            /*max_columns*/ 12,
            /*cell_pixels*/ (8, 16),
        ),
        5,
    );
    assert_eq!(
        inline_image_cell_layout(
            rendered.png.width,
            rendered.png.height,
            /*max_columns*/ 3,
            /*cell_pixels*/ (8, 16),
        ),
        3,
    );
    assert_eq!(
        inline_image_cell_layout(
            /*image_width*/ 41,
            /*image_height*/ 8,
            /*max_columns*/ 12,
            /*cell_pixels*/ (8, 16),
        ),
        5,
        "inline images wider than one cell are not magnified",
    );
    assert_eq!(
        inline_image_cell_layout(
            /*image_width*/ 82,
            /*image_height*/ 16,
            /*max_columns*/ 12,
            /*cell_pixels*/ (16, 32),
        ),
        5,
        "shared raster and terminal scaling preserves shrink-only formula fitting",
    );
    assert_eq!(
        inline_image_cell_layout(
            /*image_width*/ 7,
            /*image_height*/ 4,
            /*max_columns*/ 12,
            /*cell_pixels*/ (8, 16),
        ),
        1,
        "sub-cell images have an unavoidable one-cell placement",
    );
    let line = inline_placeholder_line(&rendered, /*columns*/ 5);
    assert_eq!(
        (
            line.width(),
            line.kitty_images[0].columns.clone(),
            line.kitty_images[0].image.columns(),
            line.kitty_images[0].image.rows(),
        ),
        (5, 0..5, 5, 1),
    );
}

#[test]
fn placeholder_grid_has_exact_coordinates_and_metadata() {
    let rendered = RenderedImage::new(test_png()).expect("available image ID");

    let lines = placeholder_lines(&rendered, /*columns*/ 2, /*rows*/ 2);
    let image_id = rendered.image_id;
    let expected_image = KittyImage::new(
        Arc::clone(&rendered.png.bytes),
        image_id,
        /*columns*/ 2,
        /*rows*/ 2,
    );
    let style = Style::new()
        .fg(crate::terminal_palette::rgb_color((
            ((image_id >> 16) & 0xff) as u8,
            ((image_id >> 8) & 0xff) as u8,
            (image_id & 0xff) as u8,
        )))
        .underline_color(crate::terminal_palette::rgb_color((
            ((expected_image.placement_id() >> 16) & 0xff) as u8,
            ((expected_image.placement_id() >> 8) & 0xff) as u8,
            (expected_image.placement_id() & 0xff) as u8,
        )));
    let expected = [
        "\u{10eeee}\u{0305}\u{0305}\u{10eeee}\u{0305}\u{030d}",
        "\u{10eeee}\u{030d}\u{0305}\u{10eeee}\u{030d}\u{030d}",
    ]
    .map(|content| {
        let mut line = HyperlinkLine::new(Line::from(Span::styled(content, style)));
        line.kitty_images.push(KittyImageAnnotation {
            columns: 0..2,
            image: expected_image.clone(),
        });
        line
    })
    .to_vec();

    assert_eq!(lines, expected);
    assert!(
        lines
            .iter()
            .all(|line| line.line.spans[0].content.width() == 2)
    );
}

#[test]
fn image_id_is_stable_and_placement_id_tracks_placeholder_geometry() {
    let rendered = RenderedImage::new(test_png()).expect("available image ID");

    let first = placeholder_lines(&rendered, /*columns*/ 5, /*rows*/ 3);
    let repeated = placeholder_lines(&rendered, /*columns*/ 5, /*rows*/ 3);
    let resized = placeholder_lines(&rendered, /*columns*/ 4, /*rows*/ 3);
    let first = &first[0].kitty_images[0].image;
    let repeated = &repeated[0].kitty_images[0].image;
    let resized = &resized[0].kitty_images[0].image;

    assert_eq!(first, repeated);
    assert_eq!(
        (first.image_id(), resized.image_id()),
        (rendered.image_id, rendered.image_id),
    );
    assert_ne!(first.placement_id(), resized.placement_id());
}

#[test]
fn image_id_allocation_wraps_skips_pet_ids_and_exhausts() {
    assert_eq!(
        [
            available_image_id_at_offset(/*start*/ 1, /*offset*/ 0),
            available_image_id_at_offset(
                /*start*/ MAX_COLOR_ENCODED_IMAGE_ID,
                /*offset*/ 0,
            ),
            available_image_id_at_offset(
                /*start*/ MAX_COLOR_ENCODED_IMAGE_ID,
                /*offset*/ 1,
            ),
            available_image_id_at_offset(/*start*/ 0xC0DE, /*offset*/ 0),
            available_image_id_at_offset(/*start*/ 0xC0DE, /*offset*/ 1),
            available_image_id_at_offset(/*start*/ 0xC0DE, /*offset*/ 2),
            available_image_id_at_offset(
                /*start*/ 1,
                /*offset*/ MAX_COLOR_ENCODED_IMAGE_ID,
            ),
        ],
        [
            Some(1),
            Some(MAX_COLOR_ENCODED_IMAGE_ID),
            Some(1),
            None,
            None,
            Some(0xC0E0),
            None,
        ],
    );
}
