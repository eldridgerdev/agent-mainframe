use super::support::*;
use crate::app::*;
use crate::project::{AgentKind, Feature, FeatureSession, Project, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::Utc;
use std::collections::HashMap;
use tempfile::NamedTempFile;
use tempfile::TempDir;

#[test]
fn control_view_parser_cursor_move_keeps_relative_redraws_on_tmux_row() {
    let mut parser = vt100::Parser::new(6, 40, 0);
    parser.process(b"\x1b[2;1Hscreen");

    position_parser_cursor(&mut parser, (8, 1), 40, 6);
    parser.process(b"\r\x1b[10Cabc");

    assert_eq!(parser_cursor(&parser), Some((13, 1)));
    assert_eq!(
        parser
            .screen()
            .contents()
            .lines()
            .nth(1)
            .unwrap_or_default()
            .trim_end(),
        "screen    abc"
    );
}

#[test]
fn drain_view_snapshots_updates_rendered_lines_when_content_is_unchanged() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "terminal".to_string(),
        "Terminal".to_string(),
        SessionKind::Terminal,
        VibeMode::default(),
        false,
    ));
    app.pane_content_cols = 4;
    app.pane_content_rows = 1;
    app.pane_content = "same formatted content".to_string();
    app.pane_lines = vec![ratatui::text::Line::from("ABCD")];

    app.view_snapshot_tx.send(ViewSnapshot {
        session: "amf-my-feat".to_string(),
        window: "terminal".to_string(),
        pane_content: Some("same formatted content".to_string()),
        rendered_lines: Some(vec![ratatui::text::Line::from("A  D")]),
        cursor: None,
        capture_duration: None,
        render_duration: None,
        cursor_duration: None,
        pipe_read_duration: None,
    });

    let (pane_changed, cursor_changed) = app.drain_view_snapshots();

    assert!(pane_changed);
    assert!(!cursor_changed);
    assert_eq!(app.pane_content, "same formatted content");
    assert_eq!(app.pane_lines, vec![ratatui::text::Line::from("A  D")]);
}

#[test]
fn snapshot_mailbox_merges_cursor_only_update_with_pending_content() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "terminal".to_string(),
        "Terminal".to_string(),
        SessionKind::Terminal,
        VibeMode::default(),
        false,
    ));
    app.pane_content_cols = 4;
    app.pane_content_rows = 1;

    let base = |content: Option<&str>, cursor: Option<Option<(u16, u16)>>| ViewSnapshot {
        session: "amf-my-feat".to_string(),
        window: "terminal".to_string(),
        pane_content: content.map(|c| c.to_string()),
        rendered_lines: content.map(|_| vec![ratatui::text::Line::from("ABCD")]),
        cursor,
        capture_duration: None,
        render_duration: None,
        cursor_duration: None,
        pipe_read_duration: None,
    };

    // Content snapshot followed by a cursor-only snapshot before the
    // main loop drains: the merged slot must keep both.
    app.view_snapshot_tx.send(base(Some("hello"), None));
    app.view_snapshot_tx.send(base(None, Some(Some((2, 0)))));

    let (pane_changed, cursor_changed) = app.drain_view_snapshots();
    assert!(pane_changed);
    assert!(cursor_changed);
    assert_eq!(app.pane_content, "hello");
    assert_eq!(app.tmux_cursor, Some((2, 0)));

    // Slot is now empty.
    let (pane_changed, cursor_changed) = app.drain_view_snapshots();
    assert!(!pane_changed);
    assert!(!cursor_changed);
}

#[test]
fn frozen_display_holds_last_frame_until_freeze_lapses() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "terminal".to_string(),
        "Terminal".to_string(),
        SessionKind::Terminal,
        VibeMode::default(),
        false,
    ));
    app.pane_content_cols = 4;
    app.pane_content_rows = 1;
    app.pane_lines = vec![ratatui::text::Line::from("OLD ")];

    app.view_snapshot_tx.send(ViewSnapshot {
        session: "amf-my-feat".to_string(),
        window: "terminal".to_string(),
        pane_content: Some("new".to_string()),
        rendered_lines: Some(vec![ratatui::text::Line::from("NEW ")]),
        cursor: None,
        capture_duration: None,
        render_duration: None,
        cursor_duration: None,
        pipe_read_duration: None,
    });

    // While frozen, the pending frame is held back and the display keeps
    // the previous frame (this is what hides the re-anchor bounce).
    app.freeze_view_display(std::time::Duration::from_secs(30));
    let (pane_changed, _) = app.drain_view_snapshots();
    assert!(!pane_changed);
    assert_eq!(app.pane_lines, vec![ratatui::text::Line::from("OLD ")]);

    // Once the freeze lapses, the freshest frame is revealed.
    app.view_display_frozen_until = Some(std::time::Instant::now());
    let (pane_changed, _) = app.drain_view_snapshots();
    assert!(pane_changed);
    assert_eq!(app.pane_lines, vec![ratatui::text::Line::from("NEW ")]);
}

#[test]
fn redraw_signature_changes_when_view_selection_moves() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    ));

    let initial = app.redraw_signature();
    if let AppMode::Viewing(view) = &mut app.mode {
        view.selection.is_selecting = true;
        view.selection.start_row = 1;
        view.selection.start_col = 2;
        view.selection.end_row = 1;
        view.selection.end_col = 2;
    }
    let selection_started = app.redraw_signature();

    if let AppMode::Viewing(view) = &mut app.mode {
        view.selection.end_col = 8;
        view.selection.has_selection = true;
    }
    let selection_dragged = app.redraw_signature();

    assert_ne!(initial, selection_started);
    assert_ne!(selection_started, selection_dragged);
}

#[test]
fn submit_steering_prompt_pastes_into_running_session() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("coached");
    std::fs::create_dir_all(&workdir).unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_paste_text()
        .withf(|session, window, text| {
            session == "amf-coached"
                && window == "claude"
                && text == "Implement steering coach automatically."
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-coached" && window == "claude" && key == "Enter"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::SteeringPrompt(SteeringPromptState::new(
        ViewState::new(
            "my-project".to_string(),
            "coached".to_string(),
            "amf-coached".to_string(),
            "claude".to_string(),
            "Claude 1".to_string(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        ),
        workdir.clone(),
        "Implement steering coach automatically.".to_string(),
    ));

    app.submit_steering_prompt().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session, "amf-coached");
            assert_eq!(view.window, "claude");
        }
        _ => panic!("expected Viewing mode"),
    }

    let prompt_path = workdir.join(".claude").join("latest-prompt.txt");
    assert_eq!(
        std::fs::read_to_string(prompt_path).unwrap(),
        "Implement steering coach automatically."
    );
}

fn compose_test_view() -> ViewState {
    ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude 1".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    )
}

fn compose_command(name: &str, interactive: bool) -> ComposeCommandEntry {
    ComposeCommandEntry {
        name: name.to_string(),
        description: String::new(),
        source: ComposeCommandSource::BuiltIn,
        interactive,
    }
}

#[test]
fn submit_compose_pastes_prose_into_session() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().to_path_buf();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_paste_text()
        .withf(|session, window, text| {
            session == "amf-my-feat" && window == "claude" && text == "Fix the flaky timer test."
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-my-feat" && window == "claude" && key == "C-u"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-my-feat" && window == "claude" && key == "Enter"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        workdir.clone(),
        "Fix the flaky timer test.".to_string(),
        Vec::new(),
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Viewing(view) if view.session == "amf-my-feat"));
    assert_eq!(
        std::fs::read_to_string(workdir.join(".claude").join("latest-prompt.txt")).unwrap(),
        "Fix the flaky timer test."
    );
}

#[test]
fn submit_compose_types_slash_command_literally() {
    let repo = TempDir::new().unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_send_literal()
        .withf(|session, window, text| {
            session == "amf-my-feat" && window == "claude" && text == "/compact"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "C-u")
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "Enter")
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "/compact".to_string(),
        vec![compose_command("compact", false)],
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Viewing(_)));
    assert!(app.compose_direct_targets.is_empty());
}

#[test]
fn submit_compose_interactive_command_enables_direct_input() {
    let repo = TempDir::new().unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_send_literal()
        .withf(|_, _, text| text == "/model")
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "C-u")
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "Enter")
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "/model".to_string(),
        vec![compose_command("model", true)],
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Viewing(_)));
    assert!(app.compose_direct_targets.contains("amf-my-feat:claude"));
}

#[test]
fn submit_compose_rejects_empty_buffer() {
    let repo = TempDir::new().unwrap();
    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "   ".to_string(),
        Vec::new(),
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Compose(_)));
}

#[test]
fn poll_compose_submit_keeps_pasting_indicator_until_delivery_finishes() {
    let repo = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut state = ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "Describe [Image 1]".to_string(),
        Vec::new(),
    );
    state.submit_in_progress = true;
    assert!(state.paste_in_progress());
    app.mode = AppMode::Compose(state);

    let (tx, rx) = std::sync::mpsc::channel();
    app.compose_submit = Some(ComposeSubmit {
        target: "amf-my-feat:claude".to_string(),
        rx,
    });

    assert!(!app.poll_compose_submit());
    assert!(matches!(&app.mode, AppMode::Compose(state) if state.paste_in_progress()));

    tx.send(Ok(())).unwrap();
    assert!(app.poll_compose_submit());
    assert!(matches!(&app.mode, AppMode::Viewing(view) if view.session == "amf-my-feat"));
}

#[test]
fn compose_suggestions_filter_by_prefix_and_complete() {
    let catalog = vec![
        compose_command("clear", false),
        compose_command("compact", false),
        compose_command("config", true),
        compose_command("model", true),
    ];
    let mut state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/c".to_string(),
        catalog,
    );

    let names: Vec<&str> = state
        .suggestions
        .iter()
        .filter_map(|idx| state.catalog.get(*idx))
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["clear", "compact", "config"]);

    state.select_next_suggestion();
    assert!(state.complete_selected_suggestion());
    assert_eq!(state.editor.text(), "/compact");
    assert!(state.exact_command_match().is_some());
}

#[test]
fn compose_suggestions_match_namespaced_commands_fuzzily() {
    let catalog = vec![
        compose_command("clear", false),
        compose_command("stn:commit", false),
        compose_command("config", true),
    ];
    let mut state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/commit".to_string(),
        catalog,
    );

    let names: Vec<&str> = state
        .suggestions
        .iter()
        .filter_map(|idx| state.catalog.get(*idx))
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["stn:commit"]);

    assert!(state.complete_selected_suggestion());
    assert_eq!(state.editor.text(), "/stn:commit");
    assert!(state.exact_command_match().is_some());
}

#[test]
fn compose_suggestions_rank_literal_above_namespaced() {
    let catalog = vec![
        compose_command("stn:commit", false),
        compose_command("commit", false),
    ];
    let state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/commit".to_string(),
        catalog,
    );

    let names: Vec<&str> = state
        .suggestions
        .iter()
        .filter_map(|idx| state.catalog.get(*idx))
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["commit", "stn:commit"]);
}

#[test]
fn compose_parts_interleave_text_and_images() {
    use crate::app::compose::{ComposePart, split_compose_parts};

    let image = |placeholder: &str| ComposeImage {
        placeholder: placeholder.to_string(),
        data: vec![1, 2, 3],
        mime: "image/png".to_string(),
    };
    let images = vec![image("[Image 1]"), image("[Image 2]")];

    let parts = split_compose_parts("look at [Image 1] and [Image 2] please", &images);
    assert_eq!(
        parts,
        vec![
            ComposePart::Text("look at ".to_string()),
            ComposePart::Image(0),
            ComposePart::Text(" and ".to_string()),
            ComposePart::Image(1),
            ComposePart::Text(" please".to_string()),
        ]
    );

    // A deleted placeholder drops its image from delivery.
    let parts = split_compose_parts("only [Image 2] remains", &images);
    assert_eq!(
        parts,
        vec![
            ComposePart::Text("only ".to_string()),
            ComposePart::Image(1),
            ComposePart::Text(" remains".to_string()),
        ]
    );

    // No placeholders: one text part.
    let parts = split_compose_parts("plain prose", &images);
    assert_eq!(parts, vec![ComposePart::Text("plain prose".to_string())]);

    // Image-only submission.
    let parts = split_compose_parts("[Image 1]", &images);
    assert_eq!(parts, vec![ComposePart::Image(0)]);
}

#[test]
fn compose_add_image_numbers_placeholders() {
    let mut state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        String::new(),
        Vec::new(),
    );

    assert_eq!(
        state.add_image(vec![0], "image/png".to_string()),
        "[Image 1]"
    );
    assert_eq!(
        state.add_image(vec![1], "image/jpeg".to_string()),
        "[Image 2]"
    );
    assert_eq!(state.images.len(), 2);

    assert!(state.clear_prompt());
    assert!(state.images.is_empty());
}

#[test]
fn compose_slash_detection_rules() {
    let state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/compact keep recent context".to_string(),
        Vec::new(),
    );
    assert!(state.is_slash_command());
    // Arguments present: the popup should no longer filter.
    assert!(state.pending_command_prefix().is_none());

    let multiline = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/not a command\njust prose".to_string(),
        Vec::new(),
    );
    assert!(!multiline.is_slash_command());
}

#[test]
fn inject_latest_prompt_pastes_into_running_session() {
    let repo = TempDir::new().unwrap();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_paste_text()
        .withf(|session, window, text| {
            session == "amf-coached"
                && window == "claude"
                && text == "Resume from the latest saved prompt."
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-coached" && window == "claude" && key == "Enter"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::LatestPrompt(LatestPromptState {
        view: ViewState::new(
            "my-project".to_string(),
            "coached".to_string(),
            "amf-coached".to_string(),
            "claude".to_string(),
            "Claude 1".to_string(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        ),
        prompts: vec![crate::app::util::PromptEntry {
            text: "Resume from the latest saved prompt.".to_string(),
            timestamp: None,
        }],
        selected: 0,
    });

    app.inject_latest_prompt().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session, "amf-coached");
            assert_eq!(view.window, "claude");
        }
        _ => panic!("expected Viewing mode"),
    }
}

#[test]
fn open_command_picker_prepends_codex_debug_commands() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.open_command_picker(None);

    match &app.mode {
        AppMode::CommandPicker(state) => {
            assert!(!state.commands.is_empty(), "expected commands");
            assert_eq!(state.commands[0].source, "AMF Debug");
            assert_eq!(state.commands[0].name, "demo-plan");
            assert_eq!(state.commands[1].name, "demo-work-change-reason");
            assert_eq!(state.commands[2].name, "demo-work-diff-review");
            assert!(matches!(
                state.commands[0].action,
                CommandAction::CodexLiveDemo(CodexDebugCommand::PlanDemo)
            ));
        }
        _ => panic!("expected command picker"),
    }
}

#[test]
fn custom_diff_review_notification_opens_prompt_while_viewing() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_worktree_agent(
        workdir.path(),
        workdir.path(),
        AgentKind::Claude,
        ProjectStatus::Idle,
        vec![FeatureSession {
            id: "claude-1".to_string(),
            kind: SessionKind::Claude,
            label: "Claude".to_string(),
            tmux_window: "claude".to_string(),
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        }],
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibe,
        false,
    ));

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/main.rs",
        "type": "diff-review",
        "file_path": workdir.path().join("src/main.rs").display().to_string(),
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-1",
        "old_snippet": "old",
        "new_snippet": "new",
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string()
    });
    std::fs::write(
        notify_dir.join("diff-review.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/main.rs");
            assert_eq!(state.tool, "edit");
            assert_eq!(state.old_snippet, "old");
            assert_eq!(state.new_snippet, "new");
        }
        _ => panic!("expected diff review prompt"),
    }
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
}

#[test]
fn custom_diff_review_notification_marks_new_files_as_added() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "amf-my-feat");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    ));

    let temp_dir = workdir.path().join("tmp-review");
    std::fs::create_dir_all(&temp_dir).unwrap();
    let original = temp_dir.join("original.md");
    let proposed = temp_dir.join("proposed.md");
    std::fs::write(&original, "").unwrap();
    std::fs::write(&proposed, "# Plan\n").unwrap();

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: plans/new.md",
        "type": "diff-review",
        "file_path": workdir.path().join("plans/new.md").display().to_string(),
        "relative_path": "plans/new.md",
        "tool": "write",
        "change_id": "chg-new",
        "old_snippet": "",
        "new_snippet": "# Plan\n",
        "original_file": original.display().to_string(),
        "proposed_file": proposed.display().to_string(),
        "is_new_file": true,
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string()
    });
    std::fs::write(
        notify_dir.join("diff-review-new-file.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            let file = state.diff_file.as_ref().expect("expected parsed diff file");
            assert_eq!(file.status, crate::diff::DiffFileStatus::Added);
            assert_eq!(file.path, "plans/new.md");
            assert_eq!(state.layout, app.config.diff_viewer_layout);
        }
        _ => panic!("expected diff review prompt"),
    }
}

#[test]
fn custom_diff_review_notification_queues_with_toast_from_normal_mode() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "amf-my-feat");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Normal;

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/lib.rs",
        "type": "diff-review",
        "file_path": workdir.path().join("src/lib.rs").display().to_string(),
        "relative_path": "src/lib.rs",
        "tool": "write",
        "change_id": "chg-2",
        "old_snippet": "",
        "new_snippet": "new body",
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string()
    });
    std::fs::write(
        notify_dir.join("diff-review.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    // From the dashboard the review is queued as a pending input and
    // announced with a toast instead of stealing focus.
    assert!(
        matches!(app.mode, AppMode::Normal),
        "expected to stay on the dashboard"
    );
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("New diff review"))
            .unwrap_or(false),
        "expected a diff review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn check_pending_diff_review_opens_pending_review_from_normal_mode() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "amf-my-feat");
    // The V flow enters the feature view, which checks the session.
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Normal;
    app.selection = Selection::Feature(0, 0);

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/lib.rs",
        "type": "diff-review",
        "file_path": workdir.path().join("src/lib.rs").display().to_string(),
        "relative_path": "src/lib.rs",
        "tool": "write",
        "change_id": "chg-3",
        "old_snippet": "",
        "new_snippet": "new body",
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string()
    });
    std::fs::write(
        notify_dir.join("diff-review.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    // The scan inside check_pending_diff_review queues the review; the
    // V flow then enters the feature view, which opens the prompt.
    app.check_pending_diff_review().unwrap();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/lib.rs");
        }
        _ => panic!("expected diff review prompt"),
    }
    assert!(app.pending_inputs.is_empty());
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("Opened pending diff review"))
            .unwrap_or(false),
        "expected an opened-review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn scan_notifications_pushes_input_request_toast() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Need input before continuing.",
        "type": "input-request",
        "tool": "write",
        "response_file": workdir.path().join("response.json").display().to_string(),
    });
    std::fs::write(
        notify_dir.join("input-request.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.toasts.len(), 1);
    assert!(
        app.toasts[0]
            .message
            .contains("New input request from my-feat")
    );
}

#[test]
fn contextual_syntax_install_returns_to_diff_viewer_and_refreshes() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut diff_viewer = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibe,
            false,
        ),
        workdir.path().to_path_buf(),
    );
    diff_viewer.files = vec![crate::diff::DiffFile {
        old_path: Some("src/main.rs".into()),
        path: "src/main.rs".into(),
        status: crate::diff::DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: Some("fn old() {}\n".into()),
        new_content: Some("fn new() {}\n".into()),
        patch: String::new(),
        hunks: vec![],
    }];

    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(SyntaxOperationEvent::Finished(Ok(
        "Installed Rust parser".to_string()
    )))
    .unwrap();
    drop(tx);

    app.mode = AppMode::SyntaxLanguagePicker(SyntaxLanguagePickerState {
        languages: vec![],
        selected: 0,
        notice: None,
        operation: Some(SyntaxOperationState {
            language: crate::highlight::HighlightLanguage::Rust,
            action: SyntaxOperationAction::Install,
            last_output: None,
            started_at: std::time::Instant::now(),
            output_rx: rx,
        }),
        return_to: Some(Box::new(AppMode::DiffViewer(diff_viewer))),
        auto_return_on_success: true,
        return_language: Some(crate::highlight::HighlightLanguage::Rust),
    });

    app.poll_syntax_language_picker().unwrap();
    // Returning to the diff viewer triggers a refresh (DiffViewerLoading);
    // drive it to completion as the event loop does before asserting.
    app.complete_diff_viewer_loading();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.error.is_some());
            assert!(state.files.is_empty());
        }
        _ => panic!("expected diff viewer after successful install"),
    }
}

#[test]
fn contextual_syntax_install_returns_to_diff_review_prompt() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(SyntaxOperationEvent::Finished(Ok(
        "Installed Rust parser".to_string()
    )))
    .unwrap();
    drop(tx);

    app.mode = AppMode::SyntaxLanguagePicker(SyntaxLanguagePickerState {
        languages: vec![],
        selected: 0,
        notice: None,
        operation: Some(SyntaxOperationState {
            language: crate::highlight::HighlightLanguage::Rust,
            action: SyntaxOperationAction::Install,
            last_output: None,
            started_at: std::time::Instant::now(),
            output_rx: rx,
        }),
        return_to: Some(Box::new(AppMode::DiffReviewPrompt(DiffReviewState {
            session_id: "sess-1".to_string(),
            workdir: workdir.path().to_path_buf(),
            file_path: workdir.path().join("src/main.rs").display().to_string(),
            relative_path: "src/main.rs".to_string(),
            change_id: "chg-1".to_string(),
            tool: "edit".to_string(),
            old_snippet: "old".to_string(),
            new_snippet: "new".to_string(),
            diff_file: None,
            diff_error: None,
            reason: String::new(),
            editing_feedback: false,
            layout: DiffViewerLayout::Unified,
            explanation: None,
            explanation_child: None,
            response_file: workdir.path().join("response.json"),
            proceed_signal: workdir.path().join("proceed"),
            request_id: None,
            reply_socket: None,
            return_to_view: None,
            patch_scroll: 0,
            opened_at: std::time::Instant::now(),
            hold_secs: 0.0,
        }))),
        auto_return_on_success: true,
        return_language: Some(crate::highlight::HighlightLanguage::Rust),
    });

    app.poll_syntax_language_picker().unwrap();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/main.rs");
        }
        _ => panic!("expected diff review prompt after successful install"),
    }
}

#[test]
fn contextual_syntax_install_stays_open_for_non_matching_language() {
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(SyntaxOperationEvent::Finished(Ok(
        "Installed JSON parser".to_string()
    )))
    .unwrap();
    drop(tx);

    app.mode = AppMode::SyntaxLanguagePicker(SyntaxLanguagePickerState {
        languages: vec![],
        selected: 0,
        notice: None,
        operation: Some(SyntaxOperationState {
            language: crate::highlight::HighlightLanguage::Json,
            action: SyntaxOperationAction::Install,
            last_output: None,
            started_at: std::time::Instant::now(),
            output_rx: rx,
        }),
        return_to: Some(Box::new(AppMode::Normal)),
        auto_return_on_success: true,
        return_language: Some(crate::highlight::HighlightLanguage::Rust),
    });

    app.poll_syntax_language_picker().unwrap();

    match &app.mode {
        AppMode::SyntaxLanguagePicker(state) => {
            assert!(state.operation.is_none());
            assert_eq!(state.notice.as_deref(), Some("Installed JSON parser"));
        }
        _ => panic!("expected syntax picker to remain open"),
    }
}

#[test]
fn sync_session_status_none_when_file_missing() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-456";
    // No status file created

    let store = store_with_custom_session(workdir.path(), session_id);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        None,
    );
}

#[test]
fn sync_session_status_none_when_file_empty() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-789";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(status_dir.join(format!("{}.txt", session_id)), "").unwrap();

    let store = store_with_custom_session(workdir.path(), session_id);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        None,
    );
}

#[test]
fn sync_session_status_skips_non_custom_sessions() {
    let workdir = TempDir::new().unwrap();
    let now = Utc::now();

    // Create a Claude session (not Custom)
    let session = FeatureSession {
        id: "claude-sess".to_string(),
        kind: SessionKind::Claude,
        label: "Claude 1".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: None,
        todo_reference: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    let store = ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    // Even if a status file exists for this ID, it should
    // be ignored because the session is not Custom.
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(status_dir.join("claude-sess.txt"), "should be ignored").unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        None,
    );
}

#[test]
fn on_stop_persists_on_feature_session() {
    let mut feat = crate::project::Feature::new(
        "test".to_string(),
        "test".to_string(),
        PathBuf::from("/tmp/test"),
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::default(),
        false,
        false,
    );
    let s = feat.add_custom_session_named(
        "Dev Servers".to_string(),
        "devservers".to_string(),
        Some("docker compose up".to_string()),
        Some("docker compose down".to_string()),
        None,
    );
    assert_eq!(s.on_stop, Some("docker compose down".to_string()));
    assert_eq!(s.command, Some("docker compose up".to_string()));
}

#[test]
fn on_stop_none_when_not_provided() {
    let mut feat = crate::project::Feature::new(
        "test".to_string(),
        "test".to_string(),
        PathBuf::from("/tmp/test"),
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::default(),
        false,
        false,
    );
    let s =
        feat.add_custom_session_named("Terminal".to_string(), "term".to_string(), None, None, None);
    assert_eq!(s.on_stop, None);
}

#[test]
fn status_file_cleanup_during_remove() {
    let workdir = TempDir::new().unwrap();
    let session_id = "cleanup-test-sess";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    let status_file = status_dir.join(format!("{}.txt", session_id));
    std::fs::write(&status_file, "running").unwrap();
    assert!(status_file.exists());

    // Build a store with a custom session
    let store = store_with_custom_session(workdir.path(), session_id);
    let feature_id = store.projects[0].features[0].id.clone();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();
    db.upsert_session_status(session_id, &feature_id, "running", None)
        .unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions().returning(|| Ok(vec![]));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(db);
    app.pending_sidebar_loads
        .insert("amf-custom-cleanup-test-sess".to_string());
    app.latest_prompt_cache
        .insert("amf-my-feat".to_string(), prompt_entry("cached prompt"));
    app.opencode_sidebar_cache.insert(
        "amf-my-feat".to_string(),
        crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".to_string(),
            title: Some("cached".to_string()),
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: None,
            todo_preview: Vec::new(),
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: None,
            model: None,
            provider: None,
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        },
    );

    // Selecting the session and removing it should clean
    // up the status file.
    app.selection = Selection::Session(0, 0, 0);
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.remove_session().unwrap();

    assert!(
        !status_file.exists(),
        "status file should be removed on session removal"
    );
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .load_session_status(session_id)
            .unwrap()
            .is_none(),
        "status row should be removed on session removal"
    );
    assert!(app.latest_prompt_for_session("amf-my-feat").is_none());
    assert!(!app.opencode_sidebar_cache.contains_key("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn sync_session_status_trims_whitespace() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-trim";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(
        status_dir.join(format!("{}.txt", session_id)),
        "  API :3000  \n",
    )
    .unwrap();

    let store = store_with_custom_session(workdir.path(), session_id);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("API :3000".to_string()),
    );
}
