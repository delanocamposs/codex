use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use std::time::Duration;

use insta::assert_snapshot;
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;
use tokio::time::timeout;

use super::cache::NEGATIVE_KEY_CAPACITY;
use super::kitty_placeholder::PLACEHOLDER;
use super::kitty_placeholder::ROW_COLUMN_DIACRITICS;
use super::*;
use crate::history_cell::AgentMarkdownCell;
use crate::history_cell::HistoryCell;
use crate::terminal_hyperlinks::HyperlinkLine;

impl LatexRenderer {
    pub(crate) fn new_for_tests(app_event_tx: mpsc::UnboundedSender<AppEvent>) -> Self {
        Self::start(app_event_tx, immediate_render())
            .expect("test runtime should provide a LaTeX renderer")
    }
}

fn immediate_render() -> Arc<RenderFn> {
    Arc::new(|_, _, _, _| Ok(test_png(/*byte_len*/ 3)))
}

fn test_png(byte_len: usize) -> LatexPng {
    LatexPng {
        bytes: vec![1; byte_len].into(),
        width: 80,
        height: 32,
    }
}

fn test_limits(cache_entry_capacity: usize) -> RendererLimits {
    RendererLimits {
        worker_count: 1,
        cache_entry_capacity,
        cache_byte_capacity: usize::MAX,
        cache_terminal_byte_capacity: usize::MAX,
        render_timeout: RENDER_TIMEOUT,
    }
}

fn start_test_renderer(
    render: Arc<RenderFn>,
    limits: RendererLimits,
) -> (LatexRenderer, mpsc::UnboundedReceiver<AppEvent>) {
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let renderer = LatexRenderer::start_with_limits(event_tx, render, limits)
        .expect("test runtime should provide a LaTeX renderer");
    (renderer, event_rx)
}

async fn next_render_generation(event_rx: &mut mpsc::UnboundedReceiver<AppEvent>) -> u64 {
    match timeout(Duration::from_secs(1), event_rx.recv())
        .await
        .expect("LaTeX render completion timed out")
        .expect("LaTeX render event channel closed")
    {
        AppEvent::LatexRenderUpdated { generation } => generation,
        event => panic!("unexpected app event: {event:?}"),
    }
}

async fn wait_for_calls(calls: &AtomicUsize, expected: usize) {
    timeout(Duration::from_secs(1), async {
        while calls.load(Ordering::SeqCst) < expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("render call count did not reach its expected value");
}

fn normalized_placeholder_text(line: &HyperlinkLine) -> String {
    line.line
        .to_string()
        .chars()
        .filter(|character| !ROW_COLUMN_DIACRITICS.contains(character))
        .map(|character| {
            if character == PLACEHOLDER {
                '▧'
            } else {
                character
            }
        })
        .collect()
}

#[derive(Default)]
struct BlockingRender {
    calls: AtomicUsize,
    active: AtomicUsize,
    peak_active: AtomicUsize,
    releases: Mutex<usize>,
    release_changed: Condvar,
}

impl BlockingRender {
    fn callback(self: &Arc<Self>) -> Arc<RenderFn> {
        let backend = Arc::clone(self);
        Arc::new(move |_, _, _, _| {
            backend.calls.fetch_add(1, Ordering::SeqCst);
            let active = backend.active.fetch_add(1, Ordering::SeqCst) + 1;
            backend.peak_active.fetch_max(active, Ordering::SeqCst);

            let mut releases = backend.releases.lock().expect("release lock");
            while *releases == 0 {
                releases = backend
                    .release_changed
                    .wait(releases)
                    .expect("release lock");
            }
            *releases -= 1;
            backend.active.fetch_sub(1, Ordering::SeqCst);
            Ok(test_png(/*byte_len*/ 3))
        })
    }

    fn release(&self, count: usize) {
        let mut releases = self.releases.lock().expect("release lock");
        *releases += count;
        self.release_changed.notify_all();
    }

    fn release_all(&self) {
        *self.releases.lock().expect("release lock") = usize::MAX;
        self.release_changed.notify_all();
    }
}

struct BlockingRenderGuard(Arc<BlockingRender>);

impl Drop for BlockingRenderGuard {
    fn drop(&mut self) {
        self.0.release_all();
    }
}

#[tokio::test]
async fn repeated_requests_are_single_flight_and_completion_is_cached() {
    let calls = Arc::new(AtomicUsize::new(0));
    let render_calls = Arc::clone(&calls);
    let render: Arc<RenderFn> = Arc::new(move |_, _, _, _| {
        render_calls.fetch_add(1, Ordering::SeqCst);
        Ok(test_png(/*byte_len*/ 3))
    });
    let (renderer, mut event_rx) =
        start_test_renderer(render, test_limits(/*cache_entry_capacity*/ 4));
    let handle = renderer.handle().for_render_pass_with_cell_pixels((8, 16));

    assert_eq!(handle.render("x^2", /*max_columns*/ 40), None);
    assert_eq!(handle.render("x^2", /*max_columns*/ 40), None);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let first = handle
        .render("x^2", /*max_columns*/ 40)
        .expect("completed image should be cached");
    assert_eq!(handle.render("x^2", /*max_columns*/ 40), Some(first));
    assert!(event_rx.try_recv().is_err());
}

#[tokio::test]
async fn live_capacity_is_stable_and_a_new_live_message_can_replace_it() {
    let calls = Arc::new(AtomicUsize::new(0));
    let render_calls = Arc::clone(&calls);
    let render: Arc<RenderFn> = Arc::new(move |_, _, _, _| {
        render_calls.fetch_add(1, Ordering::SeqCst);
        Ok(test_png(/*byte_len*/ 1))
    });
    let (renderer, mut event_rx) =
        start_test_renderer(render, test_limits(/*cache_entry_capacity*/ 2));
    let first_live = renderer
        .live_handle()
        .for_render_pass_with_cell_pixels((8, 16));

    for formula in ["a", "b"] {
        assert_eq!(first_live.render(formula, /*max_columns*/ 40), None);
    }
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert!(first_live.render("a", /*max_columns*/ 40).is_some());
    assert!(first_live.render("b", /*max_columns*/ 40).is_some());

    for _ in 0..3 {
        assert_eq!(first_live.render("c", /*max_columns*/ 40), None);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert!(event_rx.try_recv().is_err());

    let second_live = renderer
        .live_handle()
        .for_render_pass_with_cell_pixels((8, 16));
    for formula in ["c", "d"] {
        assert_eq!(second_live.render(formula, /*max_columns*/ 40), None);
    }
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert!(second_live.render("c", /*max_columns*/ 40).is_some());
    assert!(second_live.render("d", /*max_columns*/ 40).is_some());
    assert_eq!(first_live.render("a", /*max_columns*/ 40), None);
    assert_eq!(calls.load(Ordering::SeqCst), 4);
}

#[tokio::test]
async fn historical_aggregate_byte_rejections_do_not_alternate_forever() {
    let calls = Arc::new(AtomicUsize::new(0));
    let render_calls = Arc::clone(&calls);
    let render: Arc<RenderFn> = Arc::new(move |_, _, _, _| {
        render_calls.fetch_add(1, Ordering::SeqCst);
        Ok(test_png(/*byte_len*/ 3))
    });
    let mut limits = test_limits(/*cache_entry_capacity*/ 3);
    limits.cache_byte_capacity = 4;
    let (renderer, mut event_rx) = start_test_renderer(render, limits);
    let historical = renderer.handle().for_render_pass_with_cell_pixels((8, 16));

    assert_eq!(historical.render("a", /*max_columns*/ 40), None);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert!(historical.render("a", /*max_columns*/ 40).is_some());

    for formula in ["b", "c"] {
        assert_eq!(historical.render(formula, /*max_columns*/ 40), None);
        assert_eq!(next_render_generation(&mut event_rx).await, 1);
        assert_eq!(historical.render(formula, /*max_columns*/ 40), None);
    }
    for formula in ["b", "c", "b", "c"] {
        assert_eq!(historical.render(formula, /*max_columns*/ 40), None);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(event_rx.try_recv().is_err());
}

#[tokio::test]
async fn exhausted_historical_failure_memory_does_not_starve_new_live_math() {
    let calls = Arc::new(AtomicUsize::new(0));
    let render_calls = Arc::clone(&calls);
    let render: Arc<RenderFn> = Arc::new(move |formula, _, _, _| {
        render_calls.fetch_add(1, Ordering::SeqCst);
        if formula.as_str() == "valid" {
            Ok(test_png(/*byte_len*/ 1))
        } else {
            Err(LatexImageError::InvalidPng("expected test failure".into()))
        }
    });
    let (renderer, mut event_rx) = start_test_renderer(
        render,
        test_limits(/*cache_entry_capacity*/ NEGATIVE_KEY_CAPACITY + 1),
    );
    let historical = renderer.handle().for_render_pass_with_cell_pixels((8, 16));

    for index in 0..=NEGATIVE_KEY_CAPACITY {
        assert_eq!(
            historical.render(&format!("invalid_{index}"), /*max_columns*/ 40),
            None,
        );
        assert_eq!(next_render_generation(&mut event_rx).await, 1);
    }
    assert_eq!(historical.render("valid", /*max_columns*/ 40), None);
    assert_eq!(calls.load(Ordering::SeqCst), NEGATIVE_KEY_CAPACITY + 1);
    assert!(event_rx.try_recv().is_err());

    let live = renderer
        .live_handle()
        .for_render_pass_with_cell_pixels((8, 16));
    assert_eq!(live.render("valid", /*max_columns*/ 40), None);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert!(live.render("valid", /*max_columns*/ 40).is_some());
    assert_eq!(calls.load(Ordering::SeqCst), NEGATIVE_KEY_CAPACITY + 2);
}

#[tokio::test]
async fn style_and_cell_height_key_rasters_but_cell_width_does_not() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let render_calls = Arc::clone(&calls);
    let render: Arc<RenderFn> = Arc::new(move |_, style, _, cell_height| {
        render_calls
            .lock()
            .expect("render calls lock")
            .push((style, cell_height));
        Ok(test_png(/*byte_len*/ 1))
    });
    let (renderer, mut event_rx) =
        start_test_renderer(render, test_limits(/*cache_entry_capacity*/ 4));
    let handle = renderer.handle();
    let initial = handle.for_render_pass_with_cell_pixels((8, 16));

    assert_eq!(initial.render_inline("x", /*max_columns*/ 40), None);
    assert_eq!(initial.render("x", /*max_columns*/ 40), None);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert!(initial.render_inline("x", /*max_columns*/ 40).is_some());
    assert!(initial.render("x", /*max_columns*/ 40).is_some());

    let width_only = handle.for_render_pass_with_cell_pixels((12, 16));
    assert!(width_only.render_inline("x", /*max_columns*/ 40).is_some());
    assert!(width_only.render("x", /*max_columns*/ 40).is_some());
    assert!(event_rx.try_recv().is_err());

    let scaled = handle.for_render_pass_with_cell_pixels((12, 24));
    assert_eq!(scaled.render_inline("x", /*max_columns*/ 40), None);
    assert_eq!(scaled.render("x", /*max_columns*/ 40), None);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert!(scaled.render_inline("x", /*max_columns*/ 40).is_some());
    assert!(scaled.render("x", /*max_columns*/ 40).is_some());
    assert_eq!(
        *calls.lock().expect("render calls lock"),
        vec![
            (LatexRenderStyle::Inline, 16),
            (LatexRenderStyle::Display, 16),
            (LatexRenderStyle::Inline, 24),
            (LatexRenderStyle::Display, 24),
        ],
    );
}

#[tokio::test]
async fn reset_discards_stale_completion_and_stale_handles() {
    let blocking = Arc::new(BlockingRender::default());
    let _guard = BlockingRenderGuard(Arc::clone(&blocking));
    let (renderer, mut event_rx) =
        start_test_renderer(blocking.callback(), test_limits(/*cache_entry_capacity*/ 2));
    let stale = renderer.handle().for_render_pass_with_cell_pixels((8, 16));

    assert_eq!(stale.render("old", /*max_columns*/ 40), None);
    wait_for_calls(&blocking.calls, /*expected*/ 1).await;
    renderer.reset();
    assert_eq!(stale.render("old", /*max_columns*/ 40), None);

    let current = renderer.handle().for_render_pass_with_cell_pixels((8, 16));
    assert_eq!(current.render("new", /*max_columns*/ 40), None);
    blocking.release(/*count*/ 1);
    wait_for_calls(&blocking.calls, /*expected*/ 2).await;
    assert!(event_rx.try_recv().is_err());

    blocking.release(/*count*/ 1);
    assert_eq!(next_render_generation(&mut event_rx).await, 2);
    assert!(current.render("new", /*max_columns*/ 40).is_some());
    assert_eq!(stale.render("old", /*max_columns*/ 40), None);
}

#[tokio::test]
async fn stale_full_queue_after_reset_wakes_current_generation_for_retry() {
    let blocking = Arc::new(BlockingRender::default());
    let _guard = BlockingRenderGuard(Arc::clone(&blocking));
    let (renderer, mut event_rx) =
        start_test_renderer(blocking.callback(), test_limits(/*cache_entry_capacity*/ 1));
    let active = renderer.handle().for_render_pass_with_cell_pixels((8, 16));

    assert_eq!(active.render("active", /*max_columns*/ 40), None);
    wait_for_calls(&blocking.calls, /*expected*/ 1).await;
    let queued = renderer
        .live_handle()
        .for_render_pass_with_cell_pixels((8, 16));
    assert_eq!(queued.render("queued", /*max_columns*/ 40), None);

    renderer.reset();
    let current = renderer.handle().for_render_pass_with_cell_pixels((8, 16));
    assert_eq!(current.render("current", /*max_columns*/ 40), None);
    blocking.release(/*count*/ 1);

    assert_eq!(next_render_generation(&mut event_rx).await, 2);
    assert_eq!(blocking.calls.load(Ordering::SeqCst), 1);
    assert_eq!(current.render("current", /*max_columns*/ 40), None);
    wait_for_calls(&blocking.calls, /*expected*/ 2).await;

    blocking.release(/*count*/ 1);
    assert_eq!(next_render_generation(&mut event_rx).await, 2);
    assert!(current.render("current", /*max_columns*/ 40).is_some());
}

#[tokio::test]
async fn timeout_disables_new_work_and_keeps_worker_concurrency_bounded() {
    let blocking = Arc::new(BlockingRender::default());
    let _guard = BlockingRenderGuard(Arc::clone(&blocking));
    let mut limits = test_limits(/*cache_entry_capacity*/ 4);
    limits.worker_count = 2;
    limits.render_timeout = Duration::from_millis(20);
    let (renderer, mut event_rx) = start_test_renderer(blocking.callback(), limits);
    let live = renderer
        .live_handle()
        .for_render_pass_with_cell_pixels((8, 16));

    assert_eq!(live.render("a", /*max_columns*/ 40), None);
    assert_eq!(live.render("b", /*max_columns*/ 40), None);
    wait_for_calls(&blocking.calls, /*expected*/ 2).await;
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(
        (
            blocking.active.load(Ordering::SeqCst),
            blocking.peak_active.load(Ordering::SeqCst),
        ),
        (2, 2),
    );

    assert_eq!(live.render("after-timeout", /*max_columns*/ 40), None);
    assert_eq!(blocking.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn failed_render_keeps_exact_literal_source_visible_and_copyable() {
    let render: Arc<RenderFn> =
        Arc::new(|_, _, _, _| Err(LatexImageError::InvalidPng("expected test failure".into())));
    let (renderer, mut event_rx) =
        start_test_renderer(render, test_limits(/*cache_entry_capacity*/ 2));
    let cell = AgentMarkdownCell::new(
        "Before.\n\n$$\nx^2\n$$\n\nAfter.".to_string(),
        std::path::Path::new("/tmp"),
    )
    .with_latex_renderer(renderer.handle());

    let pending = cell.display_hyperlink_lines(/*width*/ 48);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(cell.display_hyperlink_lines(/*width*/ 48), pending);
    assert_eq!(cell.transcript_hyperlink_lines(/*width*/ 48), pending);
    assert!(
        pending
            .iter()
            .any(|line| line.line.to_string().contains("$$")),
    );
}

#[tokio::test]
async fn completed_display_math_has_a_visual_snapshot_and_literal_transcript() {
    let (renderer, mut event_rx) =
        start_test_renderer(immediate_render(), test_limits(/*cache_entry_capacity*/ 2));
    let cell = AgentMarkdownCell::new(
        "Before.\n\n$$\nx^2 + y^2 = z^2\n$$\n\nAfter.".to_string(),
        std::path::Path::new("/tmp"),
    )
    .with_latex_renderer(renderer.handle());

    let pending = cell.display_hyperlink_lines(/*width*/ 48);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    let ready = cell.display_hyperlink_lines(/*width*/ 48);
    assert!(ready.iter().any(|line| !line.kitty_images.is_empty()));
    assert_eq!(cell.transcript_hyperlink_lines(/*width*/ 48), pending);

    let snapshot = ready
        .iter()
        .map(|line| {
            let text = normalized_placeholder_text(line);
            if line.kitty_images.is_empty() {
                text
            } else {
                format!("{text} [Kitty image]")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot!("display_math_agent_cell_ready", snapshot);
}

#[tokio::test]
async fn completed_inline_math_has_a_one_row_visual_snapshot_and_literal_transcript() {
    let source = r"Given \(x\in\ker f''\), continue.";
    let (renderer, mut event_rx) =
        start_test_renderer(immediate_render(), test_limits(/*cache_entry_capacity*/ 2));
    let cell = AgentMarkdownCell::new(source.to_string(), std::path::Path::new("/tmp"))
        .with_latex_renderer(renderer.handle());

    let pending = cell.display_hyperlink_lines(/*width*/ 48);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    let ready = cell.display_hyperlink_lines(/*width*/ 48);
    let image = ready
        .iter()
        .flat_map(|line| &line.kitty_images)
        .next()
        .expect("inline image should be ready");
    assert_eq!(image.image.rows(), 1);
    assert_eq!(cell.transcript_hyperlink_lines(/*width*/ 48), pending);

    let snapshot = ready
        .iter()
        .map(normalized_placeholder_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot!("inline_math_agent_cell_ready", snapshot);
}

#[tokio::test]
async fn inline_math_table_snapshot_excludes_code_and_link_labels() {
    let source = concat!(
        "| Kind | Value |\n",
        "| --- | --- |\n",
        "| first | before \\(x^2\\) after |\n",
        "| second | \\(\\int_0^1 x\\,dx\\) |\n",
        "| literal | `\\(code\\)` and [\\(linked\\)](https://example.com) |",
    );
    let (renderer, mut event_rx) =
        start_test_renderer(immediate_render(), test_limits(/*cache_entry_capacity*/ 4));
    let cell = AgentMarkdownCell::new(source.to_string(), std::path::Path::new("/tmp"))
        .with_latex_renderer(renderer.handle());

    assert!(
        cell.display_hyperlink_lines(/*width*/ 100)
            .iter()
            .all(|line| line.kitty_images.is_empty()),
    );
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    assert_eq!(next_render_generation(&mut event_rx).await, 1);
    let ready = cell.display_hyperlink_lines(/*width*/ 100);
    assert_eq!(ready.iter().flat_map(|line| &line.kitty_images).count(), 2,);

    let snapshot = ready
        .iter()
        .map(normalized_placeholder_text)
        .collect::<Vec<_>>()
        .join("\n");
    assert_snapshot!("inline_math_table_ready", snapshot);
}
