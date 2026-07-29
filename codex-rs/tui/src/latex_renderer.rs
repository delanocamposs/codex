//! Bounded asynchronous rendering for finalized LaTeX formulas.
//!
//! History cells only consult this module's in-memory cache. RaTeX parsing and rasterization run on
//! blocking workers, and successful completions ask the app to rebuild source-backed scrollback.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::app_event::AppEvent;
use crate::latex_image::LatexImageError;
use crate::latex_image::LatexPng;
use crate::latex_image::LatexRenderStyle;
use crate::latex_image::ValidatedLatexFormula;

mod cache;
mod kitty_placeholder;
mod render_handle;
use cache::RenderAdmission;
use cache::RenderCache;
use cache::RenderKey;
use kitty_placeholder::next_image_id;
pub(crate) use render_handle::LatexRenderHandle;

const WORKER_COUNT: usize = 2;
const CACHE_ENTRY_CAPACITY: usize = 64;
const CACHE_BYTE_CAPACITY: usize = 16 * 1024 * 1024;
const CACHE_TERMINAL_BYTE_CAPACITY: usize = 128 * 1024 * 1024;
const RENDER_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
struct RendererLimits {
    worker_count: usize,
    cache_entry_capacity: usize,
    cache_byte_capacity: usize,
    cache_terminal_byte_capacity: usize,
    render_timeout: Duration,
}

impl Default for RendererLimits {
    fn default() -> Self {
        Self {
            worker_count: WORKER_COUNT,
            cache_entry_capacity: CACHE_ENTRY_CAPACITY,
            cache_byte_capacity: CACHE_BYTE_CAPACITY,
            cache_terminal_byte_capacity: CACHE_TERMINAL_BYTE_CAPACITY,
            render_timeout: RENDER_TIMEOUT,
        }
    }
}

struct RenderedImage {
    png: LatexPng,
    image_id: u32,
    // Approximate decoded RGBA storage for the corresponding Kitty definition.
    terminal_bytes: usize,
}

impl RenderedImage {
    #[cfg(test)]
    fn new(png: LatexPng) -> Option<Self> {
        let terminal_bytes = Self::terminal_bytes(&png)?;
        Self::new_with_terminal_bytes(png, terminal_bytes)
    }

    fn new_with_terminal_bytes(png: LatexPng, terminal_bytes: usize) -> Option<Self> {
        Some(Self {
            png,
            image_id: next_image_id()?,
            terminal_bytes,
        })
    }

    fn terminal_bytes(png: &LatexPng) -> Option<usize> {
        let pixels = u64::from(png.width).checked_mul(u64::from(png.height))?;
        usize::try_from(pixels.checked_mul(4)?).ok()
    }
}

struct RenderRequest {
    key: RenderKey,
    formula: ValidatedLatexFormula,
}

type RenderFn = dyn Fn(&ValidatedLatexFormula, LatexRenderStyle, [u8; 3], u32) -> Result<LatexPng, LatexImageError>
    + Send
    + Sync;

struct RendererState {
    generation: u64,
    latest_live_admission_id: u64,
    admission_retry_generation: Option<u64>,
    rendering_disabled: bool,
    cache: RenderCache,
}

impl Default for RendererState {
    fn default() -> Self {
        Self {
            generation: 1,
            latest_live_admission_id: 0,
            admission_retry_generation: None,
            rendering_disabled: false,
            cache: RenderCache::default(),
        }
    }
}

struct RendererInner {
    state: Mutex<RendererState>,
    app_event_tx: mpsc::UnboundedSender<AppEvent>,
    render: Arc<RenderFn>,
    limits: RendererLimits,
    request_tx: mpsc::Sender<RenderRequest>,
}

/// Owns the render generation and bounded cache for one app transcript.
pub(crate) struct LatexRenderer {
    inner: Arc<RendererInner>,
}

impl LatexRenderer {
    pub(crate) fn new(app_event_tx: mpsc::UnboundedSender<AppEvent>) -> Option<Self> {
        if !cfg!(unix) || !crate::terminal_image::kitty_unicode_placeholders_supported() {
            return None;
        }
        Self::start(
            app_event_tx,
            Arc::new(|formula, style, foreground, cell_height| {
                crate::latex_image::render_formula(formula, style, cell_height, foreground)
            }),
        )
    }

    fn start(app_event_tx: mpsc::UnboundedSender<AppEvent>, render: Arc<RenderFn>) -> Option<Self> {
        Self::start_with_limits(app_event_tx, render, RendererLimits::default())
    }

    fn start_with_limits(
        app_event_tx: mpsc::UnboundedSender<AppEvent>,
        render: Arc<RenderFn>,
        limits: RendererLimits,
    ) -> Option<Self> {
        if limits.worker_count == 0 || limits.cache_entry_capacity == 0 {
            return None;
        }
        let runtime = tokio::runtime::Handle::try_current().ok()?;
        let (request_tx, request_rx) = mpsc::channel(limits.cache_entry_capacity);
        let inner = Arc::new(RendererInner {
            state: Mutex::new(RendererState::default()),
            app_event_tx,
            render,
            limits,
            request_tx,
        });
        let request_rx = Arc::new(AsyncMutex::new(request_rx));
        for _ in 0..limits.worker_count {
            let inner = Arc::downgrade(&inner);
            let request_rx = Arc::clone(&request_rx);
            std::mem::drop(runtime.spawn(async move {
                loop {
                    let request = {
                        let mut request_rx = request_rx.lock().await;
                        request_rx.recv().await
                    };
                    let Some(request) = request else {
                        return;
                    };
                    let Some(inner) = inner.upgrade() else {
                        return;
                    };
                    // A stale queued request emits no completion after reset, so receiving it
                    // must still wake the current generation if it observed a full queue.
                    inner.notify_admission_retry();
                    inner.render_request(request).await;
                }
            }));
        }
        Some(Self { inner })
    }

    pub(crate) fn handle(&self) -> LatexRenderHandle {
        let generation = self.inner.state().generation;
        LatexRenderHandle::new(
            Arc::clone(&self.inner),
            generation,
            RenderAdmission::Historical,
        )
    }

    pub(crate) fn live_handle(&self) -> LatexRenderHandle {
        let (generation, admission) = {
            let mut state = self.inner.state();
            state.latest_live_admission_id = state.latest_live_admission_id.wrapping_add(1);
            let admission_id = state.latest_live_admission_id;
            let admission = RenderAdmission::Live(admission_id);
            state.cache.begin_live_admission();
            (state.generation, admission)
        };
        LatexRenderHandle::new(Arc::clone(&self.inner), generation, admission)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.inner.state().generation
    }

    pub(crate) fn reset(&self) {
        let mut state = self.inner.state();
        state.generation = state.generation.wrapping_add(1);
        state.admission_retry_generation = None;
        state.cache.clear();
    }
}

impl RendererInner {
    fn state(&self) -> std::sync::MutexGuard<'_, RendererState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn notify_admission_retry(&self) {
        let generation = {
            let mut state = self.state();
            let active_generation = state.generation;
            state
                .admission_retry_generation
                .take()
                .filter(|generation| *generation == active_generation)
        };
        if let Some(generation) = generation {
            self.notify_updated(generation);
        }
    }

    async fn render_request(&self, request: RenderRequest) {
        {
            let mut state = self.state();
            if state.generation != request.key.generation
                || !state.cache.contains_pending(&request.key)
            {
                return;
            }
            if state.rendering_disabled {
                state.cache.discard_pending(&request.key);
                return;
            }
        }

        let render = Arc::clone(&self.render);
        let formula = request.formula;
        let foreground = request.key.foreground;
        let style = request.key.style;
        let cell_height = request.key.cell_height;
        let mut render_task =
            tokio::task::spawn_blocking(move || render(&formula, style, foreground, cell_height));
        match timeout(self.limits.render_timeout, &mut render_task).await {
            Ok(Ok(Ok(png))) => self.finish_success(request.key, png),
            Ok(Ok(Err(error))) => {
                tracing::debug!(%error, "LaTeX rendering failed");
                self.finish_failure(request.key);
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "LaTeX worker failed");
                self.finish_failure(request.key);
            }
            Err(_) => {
                // Blocking tasks cannot be cancelled safely. Disable this renderer after the
                // first timeout so at most one task per worker can remain detached.
                render_task.abort();
                tracing::warn!("LaTeX rendering timed out; disabling image rendering");
                self.disable_rendering(request.key.generation);
            }
        }
    }

    fn finish_success(&self, key: RenderKey, png: LatexPng) {
        let png_bytes = png.bytes.len();
        let Some(terminal_bytes) = RenderedImage::terminal_bytes(&png) else {
            self.finish_failure(key);
            return;
        };
        if png_bytes > self.limits.cache_byte_capacity
            || terminal_bytes > self.limits.cache_terminal_byte_capacity
        {
            self.finish_failure(key);
            return;
        }
        let updated = {
            let mut state = self.state();
            if state.generation != key.generation {
                return;
            }
            if state.rendering_disabled {
                state.cache.discard_pending(&key);
                return;
            }
            let latest_live_admission_id = state.latest_live_admission_id;
            state.cache.complete(
                &key,
                png,
                terminal_bytes,
                latest_live_admission_id,
                self.limits,
            )
        };
        if updated {
            self.notify_updated(key.generation);
        }
    }

    fn finish_failure(&self, key: RenderKey) {
        let updated = {
            let mut state = self.state();
            if state.generation != key.generation || state.rendering_disabled {
                return;
            }
            let latest_live_admission_id = state.latest_live_admission_id;
            state.cache.fail(&key, latest_live_admission_id)
        };
        if updated {
            self.notify_updated(key.generation);
        }
    }

    fn disable_rendering(&self, generation: u64) {
        {
            let mut state = self.state();
            if state.generation != generation || state.rendering_disabled {
                return;
            }
            state.rendering_disabled = true;
            state.admission_retry_generation = None;
            state.cache.clear();
        }
        self.notify_updated(generation);
    }

    fn notify_updated(&self, generation: u64) {
        let _ = self
            .app_event_tx
            .send(AppEvent::LatexRenderUpdated { generation });
    }
}

#[cfg(test)]
#[path = "latex_renderer/kitty_placeholder_tests.rs"]
mod kitty_placeholder_tests;
#[cfg(test)]
#[path = "latex_renderer_tests.rs"]
mod tests;
