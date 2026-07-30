use std::time::Duration;

use pretty_assertions::assert_eq;

use super::*;
use crate::terminal_hyperlinks::HyperlinkLine;

const DISPLAY_WIDTH: u16 = 48;
const EVENT_TIMEOUT: Duration = Duration::from_secs(1);

fn install_test_latex_renderer(app: &mut App) -> u64 {
    let renderer =
        crate::latex_renderer::LatexRenderer::new_for_tests(app.app_event_tx.app_event_tx.clone());
    let generation = renderer.generation();
    app.latex_renderer = Some(renderer);
    generation
}

fn has_image(lines: &[HyperlinkLine]) -> bool {
    lines.iter().any(|line| !line.kitty_images.is_empty())
}

fn markdown_cells(app: &App) -> impl DoubleEndedIterator<Item = &Arc<dyn HistoryCell>> {
    app.transcript_cells
        .iter()
        .filter(|cell| cell.as_any().is::<AgentMarkdownCell>())
}

fn completed_agent_turn(index: usize, text: impl Into<String>) -> Turn {
    test_turn(
        &format!("turn-{index}"),
        TurnStatus::Completed,
        vec![ThreadItem::AgentMessage {
            id: format!("assistant-{index}"),
            text: text.into(),
            phase: None,
            memory_citation: None,
        }],
    )
}

async fn next_app_event(
    app_event_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
) -> AppEvent {
    tokio::time::timeout(EVENT_TIMEOUT, app_event_rx.recv())
        .await
        .expect("app event timed out")
        .expect("app event channel closed")
}

async fn finish_initial_replay(
    app: &mut App,
    app_event_rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    tui: &mut crate::tui::Tui,
    app_server: &mut AppServerSession,
) {
    loop {
        let event = next_app_event(app_event_rx).await;
        let replay_finished = matches!(event, AppEvent::EndInitialHistoryReplayBuffer);
        assert!(
            !matches!(event, AppEvent::LatexRenderUpdated { .. }),
            "replay must defer rendering until its tail is known"
        );
        app.handle_event(tui, app_server, event)
            .await
            .expect("handle replay event");
        if replay_finished {
            break;
        }
    }
}

#[tokio::test]
async fn live_display_math_stays_literal_until_message_completion() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    let generation = install_test_latex_renderer(&mut app);
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&app.config)).await?;
    let source = "Before.\n\n$$\nx^2\n$$\n";
    let thread_id = ThreadId::new();
    app.chat_widget
        .handle_thread_session(test_thread_session(thread_id, app.config.cwd.to_path_buf()));
    app.chat_widget.handle_server_notification(
        turn_started_notification(thread_id, "turn-1"),
        /*replay_kind*/ None,
    );
    app.chat_widget.handle_server_notification(
        agent_message_delta_notification(thread_id, "turn-1", "assistant-1", source),
        /*replay_kind*/ None,
    );
    for _ in 0..16 {
        app.chat_widget.on_commit_tick();
    }

    let mut streaming = app
        .chat_widget
        .active_cell_transcript_hyperlink_lines(/*width*/ DISPLAY_WIDTH)
        .unwrap_or_default();
    while let Ok(event) = app_event_rx.try_recv() {
        assert!(
            !matches!(
                event,
                AppEvent::ConsolidateAgentMessage { .. } | AppEvent::LatexRenderUpdated { .. }
            ),
            "streaming source must not enter the display-math renderer"
        );
        if let AppEvent::InsertHistoryCell(cell) = &event {
            streaming.extend(cell.transcript_hyperlink_lines(/*width*/ DISPLAY_WIDTH));
        }
        app.handle_event(&mut tui, &mut app_server, event).await?;
    }
    let streaming_text = streaming
        .iter()
        .map(|line| line.line.to_string())
        .collect::<String>();
    assert_eq!(
        (has_image(&streaming), streaming_text.contains("$$")),
        (false, true),
    );

    app.chat_widget.handle_server_notification(
        ServerNotification::ItemCompleted(codex_app_server_protocol::ItemCompletedNotification {
            thread_id: thread_id.to_string(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: ThreadItem::AgentMessage {
                id: "assistant-1".to_string(),
                text: source.to_string(),
                phase: Some(codex_protocol::models::MessagePhase::FinalAnswer),
                memory_citation: None,
            },
        }),
        /*replay_kind*/ None,
    );

    loop {
        let event = next_app_event(&mut app_event_rx).await;
        assert!(
            !matches!(event, AppEvent::LatexRenderUpdated { .. }),
            "render completion must follow message consolidation"
        );
        let consolidated = matches!(event, AppEvent::ConsolidateAgentMessage { .. });
        app.handle_event(&mut tui, &mut app_server, event).await?;
        if consolidated {
            break;
        }
    }

    let completion = loop {
        match next_app_event(&mut app_event_rx).await {
            event @ AppEvent::LatexRenderUpdated {
                generation: event_generation,
            } => {
                assert_eq!(event_generation, generation);
                break event;
            }
            event => {
                app.handle_event(&mut tui, &mut app_server, event).await?;
            }
        }
    };
    let consolidated = markdown_cells(&app)
        .next()
        .expect("finalized stream should be source-backed");
    assert!(has_image(
        &consolidated.display_hyperlink_lines(/*width*/ DISPLAY_WIDTH)
    ));

    app.handle_event(&mut tui, &mut app_server, completion)
        .await?;
    assert!(
        app.transcript_reflow.has_pending_reflow(),
        "render completion must immediately schedule source-backed scrollback replacement"
    );
    app_server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn cached_display_math_reflows_immediately_when_stream_is_consolidated() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    install_test_latex_renderer(&mut app);
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let source = "$$\nx^2\n$$\n";
    let handle = app
        .latex_renderer
        .as_ref()
        .expect("display-math renderer should be installed")
        .live_handle();

    assert_eq!(handle.render("x^2", /*max_columns*/ DISPLAY_WIDTH), None);
    assert_matches!(
        next_app_event(&mut app_event_rx).await,
        AppEvent::LatexRenderUpdated { .. }
    );

    app.transcript_cells = vec![Arc::new(AgentMessageCell::new(
        vec![Line::from(source)],
        /*is_first_line*/ true,
    ))];
    app.open_transcript_overlay(&mut tui);
    app.handle_consolidate_agent_message(
        &mut tui,
        source.to_string(),
        PathBuf::from("/tmp"),
        /*inline_visualization_context*/ None,
        ConsolidationScrollbackReflow::IfResizeReflowRan,
        /*deferred_history_cell*/ None,
    )?;

    assert!(
        app.transcript_reflow.has_pending_reflow(),
        "a cached image has no completion event, so consolidation must request reflow itself"
    );
    assert!(
        app_event_rx.try_recv().is_err(),
        "a cache hit must not rely on another render-completion event"
    );
    app.close_transcript_overlay(&mut tui);
    Ok(())
}

#[tokio::test]
async fn resumed_math_stays_literal_until_tail_replay_renders_display_and_inline() -> Result<()> {
    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    let generation = install_test_latex_renderer(&mut app);
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&app.config)).await?;
    let source = "Before \\(y^2\\).\n\n$$\nx^2\n$$\n\nAfter.";
    let thread_id = ThreadId::new();
    app.enqueue_primary_thread_session(
        test_thread_session(thread_id, app.config.cwd.to_path_buf()),
        vec![completed_agent_turn(/*index*/ 1, source)],
    )
    .await?;

    finish_initial_replay(&mut app, &mut app_event_rx, &mut tui, &mut app_server).await;

    let agent_cell_index = app
        .transcript_cells
        .iter()
        .position(|cell| cell.as_any().is::<AgentMarkdownCell>())
        .expect("resume should consolidate the agent message into a source-backed cell");
    let literal = app.transcript_cells[agent_cell_index]
        .display_hyperlink_lines(/*width*/ DISPLAY_WIDTH);
    let literal_text = literal
        .iter()
        .map(|line| line.line.to_string())
        .collect::<String>();
    assert_eq!(
        (
            has_image(&literal),
            literal_text.contains("$$"),
            literal_text.contains(r"\(y^2\)"),
        ),
        (false, true, true),
    );
    assert!(app.transcript_reflow.has_pending_reflow());
    app.maybe_run_resize_reflow(&mut tui)?;
    assert!(!app.transcript_reflow.has_pending_reflow());

    loop {
        let event = next_app_event(&mut app_event_rx).await;
        let render_updated = match &event {
            AppEvent::LatexRenderUpdated {
                generation: event_generation,
            } if *event_generation == generation => true,
            AppEvent::LatexRenderUpdated { .. } => {
                panic!("received a stale display-math completion")
            }
            _ => false,
        };
        app.handle_event(&mut tui, &mut app_server, event).await?;
        if render_updated {
            let lines = app.transcript_cells[agent_cell_index]
                .display_hyperlink_lines(/*width*/ DISPLAY_WIDTH);
            let mut image_ids = lines
                .iter()
                .flat_map(|line| &line.kitty_images)
                .map(|annotation| annotation.image.image_id())
                .collect::<Vec<_>>();
            image_ids.sort_unstable();
            image_ids.dedup();
            if image_ids.len() == 2 {
                break;
            }
        }
    }

    assert!(app.transcript_reflow.has_pending_reflow());
    app_server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn thread_switch_tail_replay_admits_newest_equations_when_cache_is_bounded() -> Result<()> {
    const FORMULA_COUNT: usize = 72;
    const TAIL_SAMPLE: usize = 8;

    let (mut app, mut app_event_rx, _op_rx) = make_test_app_with_channels().await;
    install_test_latex_renderer(&mut app);
    app.config.terminal_resize_reflow.max_rows = TerminalResizeReflowMaxRows::Disabled;
    let mut tui = crate::tui::test_support::make_test_tui()?;
    let mut app_server = Box::pin(crate::start_embedded_app_server_for_picker(&app.config)).await?;
    let thread_id = ThreadId::new();
    let turns = (0..FORMULA_COUNT)
        .map(|index| completed_agent_turn(index, format!("$$\nx_{index}\n$$")))
        .collect();
    app.replay_thread_snapshot(
        ThreadEventSnapshot {
            session: Some(test_thread_session(thread_id, app.config.cwd.to_path_buf())),
            turns,
            events: Vec::new(),
            input_state: None,
        },
        /*resume_restored_queue*/ false,
    );

    finish_initial_replay(&mut app, &mut app_event_rx, &mut tui, &mut app_server).await;
    assert!(app.transcript_reflow.has_pending_reflow());
    app.maybe_run_resize_reflow(&mut tui)?;

    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let _ = app.render_transcript_lines_for_reflow(/*width*/ 80);
            let newest_are_ready = markdown_cells(&app)
                .rev()
                .take(TAIL_SAMPLE)
                .all(|cell| has_image(&cell.display_hyperlink_lines(/*width*/ 80)));
            if newest_are_ready {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("tail replay did not fill the display-math cache");

    let rendered = markdown_cells(&app)
        .map(|cell| {
            has_image(&cell.display_hyperlink_lines(/*width*/ 80))
        })
        .collect::<Vec<_>>();
    let edge_rendered = rendered
        .iter()
        .take(TAIL_SAMPLE)
        .chain(rendered.iter().rev().take(TAIL_SAMPLE))
        .copied()
        .collect::<Vec<_>>();
    assert_eq!(
        edge_rendered,
        [vec![false; TAIL_SAMPLE], vec![true; TAIL_SAMPLE],].concat()
    );
    app_server.shutdown().await?;
    Ok(())
}
