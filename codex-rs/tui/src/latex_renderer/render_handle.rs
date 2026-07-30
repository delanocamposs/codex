//! Generation-scoped, non-blocking views of the math render cache.

use std::fmt;
use std::sync::Arc;

use super::RenderedImage;
use super::RendererInner;
use super::cache::CacheLookup;
use super::cache::RenderAdmission;
use super::cache::RenderKey;
use super::kitty_placeholder::image_cell_layout;
use super::kitty_placeholder::inline_image_cell_layout;
use super::kitty_placeholder::inline_placeholder_line;
use super::kitty_placeholder::latex_foreground;
use super::kitty_placeholder::placeholder_lines;
use super::kitty_placeholder::terminal_cell_pixels;
use crate::latex_image::AdmissibleLatexFormula;
use crate::latex_image::LatexRenderStyle;
use crate::terminal_hyperlinks::HyperlinkLine;

/// A generation-scoped, non-blocking view of the LaTeX render cache.
#[derive(Clone)]
pub(crate) struct LatexRenderHandle {
    pub(super) inner: Arc<RendererInner>,
    pub(super) generation: u64,
    pub(super) admission: RenderAdmission,
    pub(super) cell_pixels: Option<(u32, u32)>,
}

impl fmt::Debug for LatexRenderHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LatexRenderHandle")
            .field("generation", &self.generation)
            .field("admission", &self.admission)
            .finish()
    }
}

impl LatexRenderHandle {
    pub(super) fn new(
        inner: Arc<RendererInner>,
        generation: u64,
        admission: RenderAdmission,
    ) -> Self {
        Self {
            inner,
            generation,
            admission,
            cell_pixels: None,
        }
    }

    /// Samples terminal cell geometry once for a complete Markdown render pass.
    pub(crate) fn for_render_pass(&self) -> Self {
        self.for_render_pass_with_cell_pixels(terminal_cell_pixels())
    }

    pub(super) fn for_render_pass_with_cell_pixels(&self, cell_pixels: (u32, u32)) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            generation: self.generation,
            admission: self.admission,
            cell_pixels: Some(cell_pixels),
        }
    }

    pub(crate) fn render(&self, formula: &str, max_columns: u16) -> Option<Vec<HyperlinkLine>> {
        let cell_pixels = self.cell_pixels.unwrap_or_else(terminal_cell_pixels);
        let image = self.ready_image(formula, LatexRenderStyle::Display, cell_pixels)?;
        let (columns, rows) =
            image_cell_layout(image.png.width, image.png.height, max_columns, cell_pixels);
        Some(placeholder_lines(&image, columns, rows))
    }

    pub(crate) fn render_inline(&self, formula: &str, max_columns: u16) -> Option<HyperlinkLine> {
        let cell_pixels = self.cell_pixels.unwrap_or_else(terminal_cell_pixels);
        let image = self.ready_image(formula, LatexRenderStyle::Inline, cell_pixels)?;
        let columns =
            inline_image_cell_layout(image.png.width, image.png.height, max_columns, cell_pixels);
        Some(inline_placeholder_line(&image, columns))
    }

    fn ready_image(
        &self,
        formula: &str,
        style: LatexRenderStyle,
        cell_pixels: (u32, u32),
    ) -> Option<Arc<RenderedImage>> {
        let formula = AdmissibleLatexFormula::new(formula).ok()?;
        let foreground = latex_foreground();
        let key = RenderKey {
            generation: self.generation,
            formula,
            foreground,
            style,
            cell_height: cell_pixels.1,
        };
        let mut state = self.inner.state();
        if state.rendering_disabled || state.generation != self.generation {
            return None;
        }

        // A font-size change invalidates every raster together. Width remains a layout-only
        // input, so resizes at the same cell height reuse the image.
        state.cache.select_cell_height(cell_pixels.1);
        match state.cache.lookup(&key, self.admission) {
            CacheLookup::Ready(image) => return Some(image),
            CacheLookup::Pending => return None,
            CacheLookup::Missing => {}
        }

        if !state.cache.can_admit(&key, self.admission) {
            return None;
        }

        // Reserve while holding the state lock. A worker that frees a full queue must acquire
        // the same lock before notifying us, so it cannot race ahead of this retry marker.
        let request_permit = match self.inner.request_tx.try_reserve() {
            Ok(request_permit) => request_permit,
            Err(_) => {
                state.admission_retry_pending = true;
                return None;
            }
        };
        state.cache.insert_pending(key.clone(), self.admission);
        request_permit.send(key);
        None
    }
}
