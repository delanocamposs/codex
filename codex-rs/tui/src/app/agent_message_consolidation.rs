//! Transcript consolidation for finalized streaming agent messages.
//!
//! During streaming, the chat widget emits transient `AgentMessageCell`s so it
//! can animate stable lines into scrollback while keeping the active mutable
//! tail in the bottom pane. Once the answer finishes, the app replaces that
//! trailing run with a single source-backed `AgentMarkdownCell`. This makes the
//! transcript the canonical owner of the raw markdown source used for future
//! resize re-renders.

use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::eyre::Result;

use super::App;
use super::resize_reflow::trailing_run_start;
use crate::app_event::ConsolidationScrollbackReflow;
use crate::history_cell;
use crate::history_cell::HistoryCell;
use crate::inline_visualization::InlineVisualizationContext;
use crate::pager_overlay::Overlay;
use crate::tui;

impl App {
    pub(super) fn handle_consolidate_agent_message(
        &mut self,
        tui: &mut tui::Tui,
        source: String,
        cwd: PathBuf,
        inline_visualization_context: Option<InlineVisualizationContext>,
        scrollback_reflow: ConsolidationScrollbackReflow,
        deferred_history_cell: Option<Box<dyn HistoryCell>>,
    ) -> Result<()> {
        // Some finalize paths must preserve a last provisional stream cell long
        // enough for queue ordering, then fold it into the canonical
        // source-backed cell during consolidation.
        if let Some(cell) = deferred_history_cell {
            let cell: Arc<dyn HistoryCell> = cell.into();
            if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                t.insert_cell(cell.clone());
            }
            self.transcript_cells.push(cell);
        }

        // Walk backward to find the contiguous run of streaming AgentMessageCells that
        // belong to the just-finalized stream.
        let end = self.transcript_cells.len();
        tracing::debug!(
            "ConsolidateAgentMessage: transcript_cells.len()={end}, source_len={}",
            source.len()
        );
        let start = trailing_run_start::<history_cell::AgentMessageCell>(&self.transcript_cells);
        if start < end {
            tracing::debug!(
                "ConsolidateAgentMessage: replacing cells [{start}..{end}] with AgentMarkdownCell"
            );
            let mut math_image_ready = false;
            let has_math = crate::math_source::PreparedMath::new(&source).has_math();
            let latex_renderer = self.latex_renderer.as_ref().filter(|_| has_math);
            let mut consolidated = history_cell::AgentMarkdownCell::new_with_inline_visualizations(
                source,
                &cwd,
                inline_visualization_context,
            );
            if let Some(renderer) = latex_renderer {
                if let Some(buffer) = self.initial_history_replay_buffer.as_mut() {
                    // Resume events arrive oldest-first. Defer math admission until the replay tail
                    // renderer walks newest-first, so a bounded cache cannot be consumed
                    // permanently by the oldest formulas in a long session.
                    consolidated = consolidated.with_latex_renderer(renderer.handle());
                    buffer.retained_lines.clear();
                    buffer.render_from_transcript_tail = true;
                } else {
                    // A stream whose final source matches its committed rows may not otherwise
                    // render this source-backed cell until the next resize. Prime the non-blocking
                    // cache now, allowing this live message to displace historical entries if the
                    // bounded cache is already full.
                    consolidated = consolidated.with_latex_renderer(renderer.live_handle());
                    let width = self
                        .chat_widget
                        .history_wrap_width(tui.terminal.last_known_screen_size.width);
                    let display = consolidated.display_hyperlink_lines(width);
                    math_image_ready = display.iter().any(|line| !line.kitty_images.is_empty());
                    // Keep this handle on the source-backed cell. It remains privileged through
                    // queue or cache retries until a newer math-bearing live message supersedes
                    // it.
                }
            }
            let consolidated: Arc<dyn HistoryCell> = Arc::new(consolidated);
            self.transcript_cells
                .splice(start..end, std::iter::once(consolidated.clone()));

            if let Some(Overlay::Transcript(t)) = &mut self.overlay {
                t.consolidate_cells(start..end, consolidated.clone());
                tui.frame_requester().schedule_frame();
            }

            let scrollback_reflow = if math_image_ready {
                // A repeated formula can already be cached, so priming it queues no completion
                // event. Rebuild now to replace the streamed literal rows with the ready image.
                ConsolidationScrollbackReflow::Required
            } else {
                scrollback_reflow
            };
            self.finish_agent_message_consolidation(tui, scrollback_reflow)?;
        } else {
            tracing::debug!(
                "ConsolidateAgentMessage: no cells to consolidate(start={start}, end={end})",
            );
            self.maybe_finish_stream_reflow(tui)?;
        }

        Ok(())
    }

    fn finish_agent_message_consolidation(
        &mut self,
        tui: &mut tui::Tui,
        scrollback_reflow: ConsolidationScrollbackReflow,
    ) -> Result<()> {
        match scrollback_reflow {
            ConsolidationScrollbackReflow::IfResizeReflowRan => {
                self.maybe_finish_stream_reflow(tui)?;
            }
            ConsolidationScrollbackReflow::Required => {
                self.finish_required_stream_reflow(tui)?;
            }
        }

        Ok(())
    }
}
