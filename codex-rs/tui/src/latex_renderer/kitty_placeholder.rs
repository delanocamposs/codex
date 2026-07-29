//! Kitty Unicode-placeholder layout and image identity.

use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

use crossterm::terminal::WindowSize;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::text::Span;

use super::RenderedImage;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::KittyImageAnnotation;
use crate::terminal_image::KittyImage;

const MAX_PLACEHOLDER_ROWS: u16 = 12;
const MAX_PLACEHOLDER_COLUMNS: u16 = 240;
pub(super) const PLACEHOLDER: char = '\u{10eeee}';
const DEFAULT_CELL_PIXELS: (u32, u32) = (8, 16);
pub(super) const MAX_COLOR_ENCODED_IMAGE_ID: u32 = 0x00ff_ffff;
const RESERVED_PET_IMAGE_IDS: [u32; 2] = [0xC0DE, 0xC0DF];

// Canonical first 240 entries from Kitty's rowcolumn-diacritics.txt. Rendered columns are capped
// to this table:
// https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders
pub(super) const ROW_COLUMN_DIACRITICS: [char; MAX_PLACEHOLDER_COLUMNS as usize] = [
    '\u{0305}', '\u{030d}', '\u{030e}', '\u{0310}', '\u{0312}', '\u{033d}', '\u{033e}', '\u{033f}',
    '\u{0346}', '\u{034a}', '\u{034b}', '\u{034c}', '\u{0350}', '\u{0351}', '\u{0352}', '\u{0357}',
    '\u{035b}', '\u{0363}', '\u{0364}', '\u{0365}', '\u{0366}', '\u{0367}', '\u{0368}', '\u{0369}',
    '\u{036a}', '\u{036b}', '\u{036c}', '\u{036d}', '\u{036e}', '\u{036f}', '\u{0483}', '\u{0484}',
    '\u{0485}', '\u{0486}', '\u{0487}', '\u{0592}', '\u{0593}', '\u{0594}', '\u{0595}', '\u{0597}',
    '\u{0598}', '\u{0599}', '\u{059c}', '\u{059d}', '\u{059e}', '\u{059f}', '\u{05a0}', '\u{05a1}',
    '\u{05a8}', '\u{05a9}', '\u{05ab}', '\u{05ac}', '\u{05af}', '\u{05c4}', '\u{0610}', '\u{0611}',
    '\u{0612}', '\u{0613}', '\u{0614}', '\u{0615}', '\u{0616}', '\u{0617}', '\u{0657}', '\u{0658}',
    '\u{0659}', '\u{065a}', '\u{065b}', '\u{065d}', '\u{065e}', '\u{06d6}', '\u{06d7}', '\u{06d8}',
    '\u{06d9}', '\u{06da}', '\u{06db}', '\u{06dc}', '\u{06df}', '\u{06e0}', '\u{06e1}', '\u{06e2}',
    '\u{06e4}', '\u{06e7}', '\u{06e8}', '\u{06eb}', '\u{06ec}', '\u{0730}', '\u{0732}', '\u{0733}',
    '\u{0735}', '\u{0736}', '\u{073a}', '\u{073d}', '\u{073f}', '\u{0740}', '\u{0741}', '\u{0743}',
    '\u{0745}', '\u{0747}', '\u{0749}', '\u{074a}', '\u{07eb}', '\u{07ec}', '\u{07ed}', '\u{07ee}',
    '\u{07ef}', '\u{07f0}', '\u{07f1}', '\u{07f3}', '\u{0816}', '\u{0817}', '\u{0818}', '\u{0819}',
    '\u{081b}', '\u{081c}', '\u{081d}', '\u{081e}', '\u{081f}', '\u{0820}', '\u{0821}', '\u{0822}',
    '\u{0823}', '\u{0825}', '\u{0826}', '\u{0827}', '\u{0829}', '\u{082a}', '\u{082b}', '\u{082c}',
    '\u{082d}', '\u{0951}', '\u{0953}', '\u{0954}', '\u{0f82}', '\u{0f83}', '\u{0f86}', '\u{0f87}',
    '\u{135d}', '\u{135e}', '\u{135f}', '\u{17dd}', '\u{193a}', '\u{1a17}', '\u{1a75}', '\u{1a76}',
    '\u{1a77}', '\u{1a78}', '\u{1a79}', '\u{1a7a}', '\u{1a7b}', '\u{1a7c}', '\u{1b6b}', '\u{1b6d}',
    '\u{1b6e}', '\u{1b6f}', '\u{1b70}', '\u{1b71}', '\u{1b72}', '\u{1b73}', '\u{1cd0}', '\u{1cd1}',
    '\u{1cd2}', '\u{1cda}', '\u{1cdb}', '\u{1ce0}', '\u{1dc0}', '\u{1dc1}', '\u{1dc3}', '\u{1dc4}',
    '\u{1dc5}', '\u{1dc6}', '\u{1dc7}', '\u{1dc8}', '\u{1dc9}', '\u{1dcb}', '\u{1dcc}', '\u{1dd1}',
    '\u{1dd2}', '\u{1dd3}', '\u{1dd4}', '\u{1dd5}', '\u{1dd6}', '\u{1dd7}', '\u{1dd8}', '\u{1dd9}',
    '\u{1dda}', '\u{1ddb}', '\u{1ddc}', '\u{1ddd}', '\u{1dde}', '\u{1ddf}', '\u{1de0}', '\u{1de1}',
    '\u{1de2}', '\u{1de3}', '\u{1de4}', '\u{1de5}', '\u{1de6}', '\u{1dfe}', '\u{20d0}', '\u{20d1}',
    '\u{20d4}', '\u{20d5}', '\u{20d6}', '\u{20d7}', '\u{20db}', '\u{20dc}', '\u{20e1}', '\u{20e7}',
    '\u{20e9}', '\u{20f0}', '\u{2cef}', '\u{2cf0}', '\u{2cf1}', '\u{2de0}', '\u{2de1}', '\u{2de2}',
    '\u{2de3}', '\u{2de4}', '\u{2de5}', '\u{2de6}', '\u{2de7}', '\u{2de8}', '\u{2de9}', '\u{2dea}',
    '\u{2deb}', '\u{2dec}', '\u{2ded}', '\u{2dee}', '\u{2def}', '\u{2df0}', '\u{2df1}', '\u{2df2}',
    '\u{2df3}', '\u{2df4}', '\u{2df5}', '\u{2df6}', '\u{2df7}', '\u{2df8}', '\u{2df9}', '\u{2dfa}',
];

// Walk Kitty's 24-bit color-addressable ID space once from a process-random starting point.
// Reusing an ID could make placeholders retained in terminal scrollback display a newer,
// unrelated formula, so exhaustion leaves subsequent formulas as literal text.
static IMAGE_ID_START: LazyLock<u32> =
    LazyLock::new(|| rand::random::<u32>() % MAX_COLOR_ENCODED_IMAGE_ID + 1);
static IMAGE_ID_OFFSET: AtomicU32 = AtomicU32::new(0);

pub(super) fn latex_foreground() -> [u8; 3] {
    if let Some((red, green, blue)) = crate::terminal_palette::default_fg() {
        return [red, green, blue];
    }
    if let Some((red, green, blue)) = crate::terminal_palette::default_bg() {
        let luminance = u32::from(red) * 299 + u32::from(green) * 587 + u32::from(blue) * 114;
        return if luminance < 128_000 {
            [232, 232, 232]
        } else {
            [24, 24, 24]
        };
    }
    [232, 232, 232]
}

pub(super) fn terminal_cell_pixels() -> (u32, u32) {
    crossterm::terminal::window_size()
        .map(cell_pixels_from_window_size)
        .unwrap_or(DEFAULT_CELL_PIXELS)
}

pub(super) fn cell_pixels_from_window_size(size: WindowSize) -> (u32, u32) {
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return DEFAULT_CELL_PIXELS;
    }
    (
        u32::from(size.width).div_ceil(u32::from(size.columns)),
        u32::from(size.height).div_ceil(u32::from(size.rows)),
    )
}

pub(super) fn image_cell_layout(
    image_width: u32,
    image_height: u32,
    max_columns: u16,
    (cell_width, cell_height): (u32, u32),
) -> (u16, u16) {
    let max_columns = max_columns.clamp(1, MAX_PLACEHOLDER_COLUMNS);
    let natural_columns = image_width.div_ceil(cell_width.max(1)).max(1);
    let natural_rows = image_height.div_ceil(cell_height.max(1)).max(1);
    let mut columns = natural_columns.min(u32::from(max_columns));
    let mut rows = if columns < natural_columns {
        natural_rows
            .saturating_mul(columns)
            .div_ceil(natural_columns)
            .max(1)
    } else {
        natural_rows
    };
    if rows > u32::from(MAX_PLACEHOLDER_ROWS) {
        columns = columns
            .saturating_mul(u32::from(MAX_PLACEHOLDER_ROWS))
            .div_ceil(rows)
            .max(1);
        rows = u32::from(MAX_PLACEHOLDER_ROWS);
    }
    (
        u16::try_from(columns).unwrap_or(max_columns),
        u16::try_from(rows).unwrap_or(MAX_PLACEHOLDER_ROWS),
    )
}

pub(super) fn inline_image_cell_layout(
    image_width: u32,
    image_height: u32,
    max_columns: u16,
    (cell_width, cell_height): (u32, u32),
) -> u16 {
    let max_columns = max_columns.clamp(1, MAX_PLACEHOLDER_COLUMNS);
    // Kitty preserves aspect ratio inside the one-row virtual placement. Capping its width at the
    // number of whole cells contained by the raster prevents magnification; a raster narrower
    // than one cell still occupies the unavoidable minimum of one placeholder cell. Formulas
    // taller than the shared inline strut shrink to fit the row.
    let natural_columns = (image_width / cell_width.max(1)).max(1);
    let numerator = u64::from(image_width).saturating_mul(u64::from(cell_height.max(1)));
    let denominator = u64::from(image_height.max(1)).saturating_mul(u64::from(cell_width.max(1)));
    let columns = numerator
        .div_ceil(denominator.max(1))
        .min(u64::from(natural_columns))
        .clamp(1, u64::from(max_columns));
    u16::try_from(columns).unwrap_or(max_columns)
}

pub(super) fn next_image_id() -> Option<u32> {
    loop {
        let offset = IMAGE_ID_OFFSET
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |offset| {
                (offset < MAX_COLOR_ENCODED_IMAGE_ID).then_some(offset + 1)
            })
            .ok()?;
        if let Some(image_id) = available_image_id_at_offset(*IMAGE_ID_START, offset) {
            return Some(image_id);
        }
    }
}

pub(super) fn available_image_id_at_offset(start: u32, offset: u32) -> Option<u32> {
    if offset >= MAX_COLOR_ENCODED_IMAGE_ID {
        return None;
    }
    let image_id = (start - 1).wrapping_add(offset) % MAX_COLOR_ENCODED_IMAGE_ID + 1;
    (!RESERVED_PET_IMAGE_IDS.contains(&image_id)).then_some(image_id)
}

pub(super) fn placeholder_lines(
    rendered: &RenderedImage,
    columns: u16,
    rows: u16,
) -> Vec<HyperlinkLine> {
    let image_id = rendered.image_id;
    let image = KittyImage::new(Arc::clone(&rendered.png.bytes), image_id, columns, rows);
    let red = ((image_id >> 16) & 0xff) as u8;
    let green = ((image_id >> 8) & 0xff) as u8;
    let blue = (image_id & 0xff) as u8;
    let placement_id = image.placement_id();
    let placement_red = ((placement_id >> 16) & 0xff) as u8;
    let placement_green = ((placement_id >> 8) & 0xff) as u8;
    let placement_blue = (placement_id & 0xff) as u8;
    let style = Style::new()
        .fg(crate::terminal_palette::rgb_color((red, green, blue)))
        .underline_color(crate::terminal_palette::rgb_color((
            placement_red,
            placement_green,
            placement_blue,
        )));

    (0..rows)
        .map(|row| {
            let mut placeholders = String::new();
            for column in 0..columns {
                placeholders.push(PLACEHOLDER);
                placeholders.push(ROW_COLUMN_DIACRITICS[usize::from(row)]);
                placeholders.push(ROW_COLUMN_DIACRITICS[usize::from(column)]);
            }
            let mut line = HyperlinkLine::new(Line::from(Span::styled(placeholders, style)));
            line.kitty_images.push(KittyImageAnnotation {
                columns: 0..usize::from(columns),
                image: image.clone(),
            });
            line
        })
        .collect()
}

pub(super) fn inline_placeholder_line(rendered: &RenderedImage, columns: u16) -> HyperlinkLine {
    placeholder_lines(rendered, columns, /*rows*/ 1)
        .into_iter()
        .next()
        .expect("one-row placeholder layout must produce one line")
}
