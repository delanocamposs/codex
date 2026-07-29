use pretty_assertions::assert_eq;
use ratatui::layout::Position;
use ratatui::layout::Rect;
use ratatui::text::Line;

use super::*;
use crate::terminal_hyperlinks::KittyImageAnnotation;
use crate::terminal_image::KittyImage;
use crate::test_backend::VT100Backend;

fn image_history_line(columns: u16) -> HyperlinkLine {
    let mut line = HyperlinkLine::new(Line::from("\u{10eeee}".repeat(usize::from(columns))));
    line.kitty_images.push(KittyImageAnnotation {
        columns: 0..usize::from(columns),
        image: KittyImage::new(
            b"png".to_vec(),
            /*image_id*/ 42,
            columns,
            /*rows*/ 1,
        ),
    });
    line
}

#[test]
fn overwide_transcript_is_deferred_without_dropping_one_shot_prefix() {
    let width = 6;
    let height = 4;
    let backend = VT100Backend::new(width, height);
    let mut terminal = Terminal::with_options_and_cursor_position(backend, Position { x: 0, y: 3 })
        .expect("terminal");
    terminal.set_viewport_area(Rect::new(
        /*x*/ 0,
        /*y*/ height - 1,
        width,
        /*height*/ 1,
    ));
    let mut pending = PendingHistory::default();
    pending.insert_one_shot(vec![Line::from("header")], HistoryLineWrapPolicy::PreWrap);
    pending.insert_transcript(
        vec![
            Line::from("source-backed prose").into(),
            image_history_line(width + 1),
        ],
        HistoryLineWrapPolicy::PreWrap,
    );

    pending
        .flush(&mut terminal, /*is_zellij*/ false)
        .expect("defer overwide transcript");
    {
        let first_output =
            std::str::from_utf8(&terminal.backend().raw_output).expect("UTF-8 output");
        assert_eq!(
            (
                pending.source_reflow_needed(),
                &pending.batches,
                first_output.matches("\x1b_Ga=T").count(),
                first_output.matches('\u{10eeee}').count(),
            ),
            (
                true,
                &vec![PendingHistoryBatch {
                    lines: vec![Line::from("header").into()],
                    wrap_policy: HistoryLineWrapPolicy::PreWrap,
                    origin: PendingHistoryOrigin::OneShot,
                }],
                0,
                0,
            ),
        );
    }

    pending
        .flush(&mut terminal, /*is_zellij*/ false)
        .expect("flush retained one-shot prefix");
    let output = std::str::from_utf8(&terminal.backend().raw_output).expect("UTF-8 output");
    assert_eq!(
        (
            pending.batches.is_empty(),
            pending.take_source_reflow_needed(),
            pending.take_source_reflow_needed(),
            output.contains("header"),
            output.contains("source-backed prose"),
            output.matches("\x1b_Ga=T").count(),
            output.matches('\u{10eeee}').count(),
        ),
        (true, true, false, true, false, 0, 0),
    );
}
