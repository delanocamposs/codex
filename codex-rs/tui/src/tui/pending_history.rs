//! Queued terminal-scrollback rows and resize-race recovery.
//!
//! Transcript rows are source-backed and may be replaced during a resize reflow. One-shot rows
//! are limited to the clear header and startup cwd diagnostic; both precede transcript output and
//! must survive a pending transcript replacement.

use std::io;
use std::io::Write;

use ratatui::backend::Backend;
use ratatui::text::Line;

use crate::custom_terminal::Terminal;
use crate::insert_history::HistoryLineWrapPolicy;
use crate::insert_history::InsertHistoryMode;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::plain_hyperlink_lines;

#[derive(Default)]
pub(super) struct PendingHistory {
    batches: Vec<PendingHistoryBatch>,
    source_reflow_needed: bool,
}

#[derive(Debug, Eq, PartialEq)]
struct PendingHistoryBatch {
    lines: Vec<HyperlinkLine>,
    wrap_policy: HistoryLineWrapPolicy,
    origin: PendingHistoryOrigin,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PendingHistoryOrigin {
    OneShot,
    Transcript,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FlushOutcome {
    Complete,
    SourceReflowNeeded,
}

impl PendingHistory {
    pub(super) fn insert_one_shot(
        &mut self,
        lines: Vec<Line<'static>>,
        wrap_policy: HistoryLineWrapPolicy,
    ) {
        self.insert(
            plain_hyperlink_lines(lines),
            wrap_policy,
            PendingHistoryOrigin::OneShot,
        );
    }

    pub(super) fn insert_transcript(
        &mut self,
        lines: Vec<HyperlinkLine>,
        wrap_policy: HistoryLineWrapPolicy,
    ) {
        self.insert(lines, wrap_policy, PendingHistoryOrigin::Transcript);
    }

    fn insert(
        &mut self,
        lines: Vec<HyperlinkLine>,
        wrap_policy: HistoryLineWrapPolicy,
        origin: PendingHistoryOrigin,
    ) {
        if let Some(last) = self.batches.last_mut()
            && last.wrap_policy == wrap_policy
            && last.origin == origin
        {
            last.lines.extend(lines);
        } else {
            self.batches.push(PendingHistoryBatch {
                lines,
                wrap_policy,
                origin,
            });
        }
    }

    pub(super) fn clear(&mut self) {
        self.batches.clear();
        self.source_reflow_needed = false;
    }

    pub(super) fn discard_transcript(&mut self) {
        discard_transcript(&mut self.batches);
    }

    pub(super) fn take_source_reflow_needed(&mut self) -> bool {
        std::mem::take(&mut self.source_reflow_needed)
    }

    pub(super) fn source_reflow_needed(&self) -> bool {
        self.source_reflow_needed
    }

    pub(super) fn flush<B>(&mut self, terminal: &mut Terminal<B>, is_zellij: bool) -> io::Result<()>
    where
        B: Backend<Error = io::Error> + Write,
    {
        if flush_batches(terminal, &mut self.batches, is_zellij)?
            == FlushOutcome::SourceReflowNeeded
        {
            self.discard_transcript();
            self.source_reflow_needed = true;
        }
        Ok(())
    }
}

fn discard_transcript(batches: &mut Vec<PendingHistoryBatch>) {
    batches.retain(|batch| batch.origin == PendingHistoryOrigin::OneShot);
}

fn flush_batches<B>(
    terminal: &mut Terminal<B>,
    batches: &mut Vec<PendingHistoryBatch>,
    is_zellij: bool,
) -> io::Result<FlushOutcome>
where
    B: Backend<Error = io::Error> + Write,
{
    let viewport_width = usize::from(terminal.viewport_area.width);
    if batches.iter().any(|batch| {
        batch.lines.iter().any(|line| {
            !line.kitty_images.is_empty() && (viewport_width == 0 || line.width() > viewport_width)
        })
    }) {
        // Placeholder coordinates require each logical image row to remain physically intact.
        // Emit none of the queue so App can replace transcript rows atomically while retaining the
        // one-shot prefix.
        return Ok(FlushOutcome::SourceReflowNeeded);
    }

    for batch in batches.iter() {
        let mode = if is_zellij && batch.wrap_policy == HistoryLineWrapPolicy::Terminal {
            InsertHistoryMode::ZellijRaw
        } else {
            InsertHistoryMode::Standard
        };
        crate::insert_history::insert_history_hyperlink_lines_with_mode_and_wrap_policy(
            terminal,
            &batch.lines,
            mode,
            batch.wrap_policy,
        )?;
    }
    batches.clear();
    Ok(FlushOutcome::Complete)
}

#[cfg(test)]
#[path = "pending_history_tests.rs"]
mod tests;
