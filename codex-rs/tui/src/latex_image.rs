//! Bounded in-process LaTeX rendering with RaTeX.
//!
//! Formulas come from model output, so this module constrains source size and checks raster
//! dimensions before allocating the output pixmap.

use std::sync::Arc;

#[cfg(unix)]
use image::ImageFormat;
#[cfg(unix)]
use image::ImageReader;
#[cfg(unix)]
use ratex_font::FontId;
#[cfg(unix)]
use ratex_layout::LayoutOptions;
#[cfg(unix)]
use ratex_layout::layout;
#[cfg(unix)]
use ratex_layout::to_display_list;
#[cfg(unix)]
use ratex_parser::parser::parse;
#[cfg(unix)]
use ratex_render::RenderOptions;
#[cfg(unix)]
use ratex_render::render_to_png;
#[cfg(unix)]
use ratex_types::color::Color;
#[cfg(unix)]
use ratex_types::display_item::DisplayItem;
#[cfg(unix)]
use ratex_types::display_item::DisplayList;
#[cfg(unix)]
use ratex_types::math_style::MathStyle;
#[cfg(unix)]
use std::io::Cursor;
use thiserror::Error;

const MAX_FORMULA_BYTES: usize = 16 * 1024;
#[cfg(unix)]
const REFERENCE_FONT_SIZE: f32 = 44.0;
#[cfg(unix)]
const REFERENCE_IMAGE_PADDING: f32 = 8.0;
#[cfg(unix)]
const REFERENCE_CELL_PIXEL_HEIGHT: u32 = 32;
#[cfg(unix)]
const INLINE_STRUT_HEIGHT: f64 = 0.8;
#[cfg(unix)]
const INLINE_STRUT_DEPTH: f64 = 0.25;
#[cfg(unix)]
const MAX_PNG_BYTES: usize = 4 * 1024 * 1024;
#[cfg(unix)]
const MAX_IMAGE_DIMENSION: u32 = 4096;
#[cfg(unix)]
const MAX_IMAGE_PIXELS: u64 = 8 * 1024 * 1024;
const MACRO_MUTATION_CONTROLS: [&str; 11] = [
    "def",
    "gdef",
    "edef",
    "xdef",
    "global",
    "long",
    "let",
    "futurelet",
    "newcommand",
    "renewcommand",
    "providecommand",
];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ValidatedLatexFormula(Box<str>);

impl ValidatedLatexFormula {
    pub(crate) fn new(formula: &str) -> Result<Self, LatexImageError> {
        let formula = formula.trim();
        if formula.is_empty() {
            return Err(LatexImageError::InvalidFormula("formula is empty"));
        }
        if formula.len() > MAX_FORMULA_BYTES {
            return Err(LatexImageError::InvalidFormula("formula is too long"));
        }
        if formula_contains_macro_definition(formula) {
            // RaTeX bounds expansion count but not the number of tokens produced by each
            // expansion. A bounded formula can therefore define a large body and expand it
            // hundreds of times before reaching that limit.
            return Err(LatexImageError::InvalidFormula(
                "custom macro definitions are not allowed",
            ));
        }
        Ok(Self(formula.into()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

fn formula_contains_macro_definition(formula: &str) -> bool {
    let bytes = formula.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }
        index += 1;
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_alphabetic() {
            index += 1;
        }
        if start == index {
            // A TeX control symbol consumes the one non-letter after the backslash. In
            // particular, the second backslash in `\\def` is not the start of `\def`.
            index = index.saturating_add(1);
            continue;
        }
        if MACRO_MUTATION_CONTROLS.contains(&&formula[start..index]) {
            return true;
        }
    }
    false
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LatexPng {
    pub(crate) bytes: Arc<[u8]>,
    pub(crate) width: u32,
    pub(crate) height: u32,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum LatexRenderStyle {
    Display,
    Inline,
}

#[derive(Debug, Eq, Error, PartialEq)]
pub(crate) enum LatexImageError {
    #[error("invalid LaTeX formula: {0}")]
    InvalidFormula(&'static str),
    #[error("RaTeX could not render LaTeX: {0}")]
    Render(String),
    #[error("rendered PNG has invalid byte size {0}")]
    InvalidOutputSize(usize),
    #[error("rendered PNG is invalid: {0}")]
    InvalidPng(String),
}

#[cfg(unix)]
pub(crate) fn render_formula(
    formula: &ValidatedLatexFormula,
    style: LatexRenderStyle,
    cell_height: u32,
    foreground: [u8; 3],
) -> Result<LatexPng, LatexImageError> {
    let raster_metrics = raster_metrics(cell_height);
    let ast =
        parse(formula.as_str()).map_err(|error| LatexImageError::Render(error.to_string()))?;
    let layout = layout(
        &ast,
        &LayoutOptions {
            color: color(foreground, /*alpha*/ 1.0),
            style: match style {
                LatexRenderStyle::Display => MathStyle::Display,
                LatexRenderStyle::Inline => MathStyle::Text,
            },
            ..LayoutOptions::default()
        },
    );
    let mut display_list = to_display_list(&layout);
    match style {
        LatexRenderStyle::Display => {}
        LatexRenderStyle::Inline => apply_inline_strut(&mut display_list),
    }
    validate_display_list_dimensions(&display_list, raster_metrics)?;
    reject_system_font_fallback(&display_list)?;
    let bytes = render_to_png(
        &display_list,
        &RenderOptions {
            font_size: raster_metrics.font_size,
            padding: raster_metrics.padding,
            background_color: Color::new(0.0, 0.0, 0.0, 0.0),
            font_dir: String::new(),
            device_pixel_ratio: 1.0,
        },
    )
    .map_err(LatexImageError::Render)?;
    validate_png(bytes)
}

#[cfg(unix)]
fn apply_inline_strut(display_list: &mut DisplayList) {
    let height = display_list.height.max(INLINE_STRUT_HEIGHT);
    let shift = height - display_list.height;
    if shift > 0.0 {
        for item in &mut display_list.items {
            match item {
                DisplayItem::GlyphPath { y, .. }
                | DisplayItem::Line { y, .. }
                | DisplayItem::Rect { y, .. }
                | DisplayItem::Path { y, .. } => *y += shift,
            }
        }
    }
    display_list.height = height;
    display_list.depth = display_list.depth.max(INLINE_STRUT_DEPTH);
}

#[cfg(not(unix))]
pub(crate) fn render_formula(
    _formula: &ValidatedLatexFormula,
    _style: LatexRenderStyle,
    _cell_height: u32,
    _foreground: [u8; 3],
) -> Result<LatexPng, LatexImageError> {
    Err(LatexImageError::Render(
        "LaTeX rendering is unavailable on this platform".into(),
    ))
}

#[cfg(unix)]
fn reject_system_font_fallback(display_list: &DisplayList) -> Result<(), LatexImageError> {
    // RaTeX 0.1.14 writes system-font discovery diagnostics directly to stderr. Rendering runs
    // in-process while the TUI owns that terminal, so reject fallback before it can corrupt the
    // screen. Host fonts themselves are not a trust or licensing boundary; remove this guard once
    // RaTeX makes discovery silent or configurable.
    for item in &display_list.items {
        let DisplayItem::GlyphPath {
            font, char_code, ..
        } = item
        else {
            continue;
        };
        let Some(font_id) = FontId::parse(font) else {
            return Err(LatexImageError::Render(format!(
                "unknown RaTeX font {font}"
            )));
        };
        if matches!(
            font_id,
            FontId::CjkRegular | FontId::CjkFallback | FontId::EmojiFallback
        ) || (*char_code > 0x7f && ratex_font::get_char_metrics(font_id, *char_code).is_none())
        {
            return Err(LatexImageError::Render(
                "formula requires a system font fallback".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn color([red, green, blue]: [u8; 3], alpha: f32) -> Color {
    Color::new(
        f32::from(red) / 255.0,
        f32::from(green) / 255.0,
        f32::from(blue) / 255.0,
        alpha,
    )
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq)]
struct RasterMetrics {
    font_size: f32,
    padding: f32,
}

#[cfg(unix)]
fn raster_metrics(cell_height: u32) -> RasterMetrics {
    // Calibrate the approved RaTeX size to a normal 32px-high terminal cell. The power-of-two
    // reference also gives the 16px fallback the same formula-to-cell ratio at exactly half size.
    let terminal_scale = cell_height.max(1) as f32 / REFERENCE_CELL_PIXEL_HEIGHT as f32;
    RasterMetrics {
        font_size: REFERENCE_FONT_SIZE * terminal_scale,
        padding: REFERENCE_IMAGE_PADDING * terminal_scale,
    }
}

#[cfg(unix)]
fn validate_display_list_dimensions(
    display_list: &DisplayList,
    raster_metrics: RasterMetrics,
) -> Result<(), LatexImageError> {
    let width = display_list.width * f64::from(raster_metrics.font_size)
        + 2.0 * f64::from(raster_metrics.padding);
    let height = (display_list.height + display_list.depth) * f64::from(raster_metrics.font_size)
        + 2.0 * f64::from(raster_metrics.padding);
    if !width.is_finite()
        || !height.is_finite()
        || width <= 0.0
        || height <= 0.0
        || width.ceil() > f64::from(MAX_IMAGE_DIMENSION)
        || height.ceil() > f64::from(MAX_IMAGE_DIMENSION)
        || width.ceil() * height.ceil() > MAX_IMAGE_PIXELS as f64
    {
        return Err(LatexImageError::Render(format!(
            "dimensions {width:.1}x{height:.1} exceed limits"
        )));
    }
    Ok(())
}

#[cfg(unix)]
fn validate_png(bytes: Vec<u8>) -> Result<LatexPng, LatexImageError> {
    if bytes.len() > MAX_PNG_BYTES {
        return Err(LatexImageError::InvalidOutputSize(bytes.len()));
    }
    let (width, height) = ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Png)
        .into_dimensions()
        .map_err(|error| LatexImageError::InvalidPng(error.to_string()))?;
    if width == 0
        || height == 0
        || width > MAX_IMAGE_DIMENSION
        || height > MAX_IMAGE_DIMENSION
        || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS
    {
        return Err(LatexImageError::InvalidPng(format!(
            "dimensions {width}x{height} exceed limits"
        )));
    }
    Ok(LatexPng {
        bytes: bytes.into(),
        width,
        height,
    })
}

#[cfg(all(test, unix))]
#[path = "latex_image_tests.rs"]
mod tests;
