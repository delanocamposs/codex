//! Bounded asynchronous rendering for finalized LaTeX formulas.
//!
//! History cells only consult this module's in-memory cache. RaTeX parsing and rasterization run on
//! blocking workers, and successful completions ask the app to rebuild source-backed scrollback.

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::sync::mpsc;
use tokio::time::timeout;

use crate::app_event::AppEvent;
use crate::latex_image::AdmissibleLatexFormula;
use crate::latex_image::LatexImageError;
use crate::latex_image::LatexPng;
use crate::latex_image::LatexRenderStyle;

mod cache;
mod kitty_placeholder;
mod render_handle;
use cache::CacheCompletion;
use cache::RenderAdmission;
use cache::RenderCache;
use cache::RenderKey;
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
    fn terminal_bytes(png: &LatexPng) -> Option<usize> {
        let pixels = u64::from(png.width).checked_mul(u64::from(png.height))?;
        usize::try_from(pixels.checked_mul(4)?).ok()
    }
}

type RenderFn = dyn Fn(&AdmissibleLatexFormula, LatexRenderStyle, [u8; 3], u32) -> Result<LatexPng, LatexImageError>
    + Send
    + Sync;

struct RendererState {
    generation: u64,
    admission_retry_pending: bool,
    rendering_disabled: bool,
    cache: RenderCache,
}

struct RendererInner {
    state: Mutex<RendererState>,
    app_event_tx: mpsc::UnboundedSender<AppEvent>,
    render: Arc<RenderFn>,
    limits: RendererLimits,
    request_tx: mpsc::Sender<RenderKey>,
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
            state: Mutex::new(RendererState {
                generation: 1,
                admission_retry_pending: false,
                rendering_disabled: false,
                cache: RenderCache::new(limits),
            }),
            app_event_tx,
            render,
            limits,
            request_tx,
        });
        let inner_weak = Arc::downgrade(&inner);
        std::mem::drop(runtime.spawn(async move {
            let workers = Arc::new(Semaphore::new(limits.worker_count));
            let mut request_rx = request_rx;
            loop {
                let Ok(worker) = Arc::clone(&workers).acquire_owned().await else {
                    return;
                };
                let Some(request) = request_rx.recv().await else {
                    return;
                };
                let Some(inner) = inner_weak.upgrade() else {
                    return;
                };
                // A stale queued request emits no completion after reset, so receiving it
                // must still wake the current generation if it observed a full queue.
                inner.notify_admission_retry();
                std::mem::drop(tokio::spawn(async move {
                    inner.render_request(request).await;
                    drop(worker);
                }));
            }
        }));
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
            let generation = state.generation;
            (generation, state.cache.begin_live_admission())
        };
        LatexRenderHandle::new(Arc::clone(&self.inner), generation, admission)
    }

    pub(crate) fn generation(&self) -> u64 {
        self.inner.state().generation
    }

    pub(crate) fn reset(&self) {
        let mut state = self.inner.state();
        state.generation = state.generation.wrapping_add(1);
        state.admission_retry_pending = false;
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
            if !state.admission_retry_pending {
                return;
            }
            state.admission_retry_pending = false;
            state.generation
        };
        self.notify_updated(generation);
    }

    async fn render_request(&self, key: RenderKey) {
        {
            let state = self.state();
            if state.generation != key.generation || !state.cache.contains_pending(&key) {
                return;
            }
        }

        let render = Arc::clone(&self.render);
        let formula = key.formula.clone();
        let foreground = key.foreground;
        let style = key.style;
        let cell_height = key.cell_height;
        let mut render_task =
            tokio::task::spawn_blocking(move || render(&formula, style, foreground, cell_height));
        match timeout(self.limits.render_timeout, &mut render_task).await {
            Ok(Ok(Ok(png))) => self.finish_success(key, png),
            Ok(Ok(Err(error))) => {
                tracing::debug!(%error, "LaTeX rendering failed");
                self.finish_failure(key);
            }
            Ok(Err(error)) => {
                tracing::debug!(%error, "LaTeX worker failed");
                self.finish_failure(key);
            }
            Err(_) => {
                // Blocking tasks cannot be cancelled safely. Disable this renderer after the
                // first timeout so at most one task per worker can remain detached.
                render_task.abort();
                tracing::warn!("LaTeX rendering timed out; disabling image rendering");
                self.disable_rendering(key.generation);
            }
        }
    }

    fn finish_success(&self, key: RenderKey, png: LatexPng) {
        let Some(terminal_bytes) = RenderedImage::terminal_bytes(&png) else {
            self.finish_failure(key);
            return;
        };
        let updated = {
            let mut state = self.state();
            if state.generation != key.generation || state.rendering_disabled {
                return;
            }
            match state.cache.complete(&key, png, terminal_bytes) {
                CacheCompletion::Missing => false,
                CacheCompletion::Updated => true,
                CacheCompletion::ImageIdExhausted => {
                    tracing::warn!("LaTeX image IDs exhausted; disabling image rendering");
                    state.rendering_disabled = true;
                    state.admission_retry_pending = false;
                    state.cache.clear();
                    true
                }
            }
        };
        if updated {
            self.notify_updated(key.generation);
        }
    }

    fn finish_failure(&self, key: RenderKey) {
        let updated = {
            let mut state = self.state();
            if state.generation != key.generation {
                return;
            }
            state.cache.fail(&key)
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
            state.admission_retry_pending = false;
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
