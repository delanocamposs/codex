use std::sync::Arc;

use crossterm::terminal::WindowSize;
use pretty_assertions::assert_eq;
use ratatui::style::Color;
use ratatui::style::Style;

use super::kitty_placeholder::MAX_COLOR_ENCODED_IMAGE_ID;
use super::kitty_placeholder::available_image_id_at_offset;
use super::kitty_placeholder::cell_pixels_from_window_size;
use super::kitty_placeholder::image_cell_layout;
use super::kitty_placeholder::inline_image_cell_layout;
use super::kitty_placeholder::inline_placeholder_line;
use super::kitty_placeholder::placeholder_lines;
use super::*;
use crate::latex_image::LatexPng;
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
        (80, 24, 640, 384),
        (80, 24, 641, 385),
        (0, 24, 640, 384),
        (80, 0, 640, 384),
        (80, 24, 0, 0),
    ]
    .map(|(columns, rows, width, height)| WindowSize {
        columns,
        rows,
        width,
        height,
    });
    assert_eq!(
        sizes.map(cell_pixels_from_window_size),
        [(8, 16), (9, 17), (8, 16), (8, 16), (8, 16)],
    );
}

#[test]
fn image_layout_preserves_shape_within_bounds() {
    let cases = [(640, 80, 40, (8, 16)), (160, 640, 80, (8, 16))];
    assert_eq!(
        cases.map(|(image_width, image_height, max_columns, cell_pixels)| {
            image_cell_layout(image_width, image_height, max_columns, cell_pixels)
        }),
        [(40, 3), (6, 12)],
    );
}

#[test]
fn inline_layout_is_one_row_and_preserves_aspect_ratio_within_its_cap() {
    let rendered = RenderedImage::new(test_png()).expect("available image ID");
    let cases = [
        (rendered.png.width, rendered.png.height, 12, (8, 16)),
        (rendered.png.width, rendered.png.height, 3, (8, 16)),
        (41, 8, 12, (8, 16)),
        (82, 16, 12, (16, 32)),
        (7, 4, 12, (8, 16)),
    ];
    assert_eq!(
        cases.map(|(image_width, image_height, max_columns, cell_pixels)| {
            inline_image_cell_layout(image_width, image_height, max_columns, cell_pixels)
        }),
        [5, 3, 5, 5, 1],
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
#[allow(clippy::disallowed_methods)]
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
    let placement_id = expected_image.placement_id();
    let expected_style = Style::new()
        .fg(Color::Rgb(
            ((image_id >> 16) & 0xff) as u8,
            ((image_id >> 8) & 0xff) as u8,
            (image_id & 0xff) as u8,
        ))
        .underline_color(Color::Rgb(
            ((placement_id >> 16) & 0xff) as u8,
            ((placement_id >> 8) & 0xff) as u8,
            (placement_id & 0xff) as u8,
        ));
    let expected_annotation = KittyImageAnnotation {
        columns: 0..2,
        image: expected_image,
    };
    let expected = [
        "\u{10eeee}\u{0305}\u{0305}\u{10eeee}\u{0305}\u{030d}",
        "\u{10eeee}\u{030d}\u{0305}\u{10eeee}\u{030d}\u{030d}",
    ]
    .map(|content| {
        (
            content.to_string(),
            2,
            expected_style,
            vec![expected_annotation.clone()],
        )
    })
    .to_vec();
    assert_eq!(
        lines
            .iter()
            .map(|line| {
                (
                    line.line.to_string(),
                    line.line.width(),
                    line.line.spans[0].style,
                    line.kitty_images.clone(),
                )
            })
            .collect::<Vec<_>>(),
        expected,
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
            (1, 0),
            (MAX_COLOR_ENCODED_IMAGE_ID, 0),
            (MAX_COLOR_ENCODED_IMAGE_ID, 1),
            (0xC0DE, 0),
            (0xC0DE, 1),
            (0xC0DE, 2),
            (1, MAX_COLOR_ENCODED_IMAGE_ID),
        ]
        .map(|(start, offset)| available_image_id_at_offset(start, offset)),
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
