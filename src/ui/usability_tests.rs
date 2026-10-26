use super::*;
use crate::app::{CreateProjectState, CreateProjectStep, SearchMatch, SearchState, VisibleItem};
use crate::project::AgentKind;
use ratatui::{Terminal, backend::TestBackend};

fn rendered(width: u16, height: u16, mut draw: impl FnMut(&mut Frame)) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame)).unwrap();
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn usability_search_keeps_last_result_visible() {
    let mut state = SearchState {
        query: "result".into(),
        matches: (0..50)
            .map(|i| SearchMatch {
                item: VisibleItem::Project(i),
                label: format!("result-{i:02}"),
                context: String::new(),
            })
            .collect(),
        selected_match: 49,
    };
    for selected in [49, 0, 25] {
        state.selected_match = selected;
        let screen = rendered(80, 24, |frame| {
            dialogs::draw_search_dialog(frame, &state, &Theme::default())
        });
        assert!(screen.contains(&format!("result-{selected:02}")));
    }
}

#[test]
fn usability_project_form_shows_errors_and_cancel_at_standard_size() {
    let state = CreateProjectState {
        step: CreateProjectStep::Path,
        name: "example".into(),
        path: "/missing".into(),
        agent: AgentKind::Claude,
        agent_index: 0,
    };
    let screen = rendered(80, 24, |frame| {
        dialogs::draw_create_project_dialog(
            frame,
            &state,
            &[
                AgentKind::Claude,
                AgentKind::Codex,
                AgentKind::Opencode,
                AgentKind::Pi,
            ],
            Some("Error: Path does not exist: /missing"),
            &Theme::default(),
        )
    });
    for text in [
        "Path does not exist",
        "Shift+Tab",
        "Esc",
        "cancel",
        "Preferred harness:",
    ] {
        assert!(screen.contains(text), "missing {text}");
    }
}

#[test]
fn usability_delete_dialogs_show_consequences_and_confirmation() {
    for (width, height) in [(80, 24), (40, 20)] {
        let project = rendered(width, height, |frame| {
            dialogs::draw_delete_project_confirm(frame, "example", &Theme::default())
        });
        let feature = rendered(width, height, |frame| {
            dialogs::draw_delete_feature_confirm(frame, "example", "feature", &Theme::default())
        });
        for screen in [project, feature] {
            assert!(screen.contains("worktree"));
            assert!(screen.contains("confirm"));
            assert!(screen.contains("cancel"));
        }
    }
}

#[test]
fn usability_help_wraps_and_handles_tiny_terminals() {
    let mut bottom = 0;
    let screen = rendered(80, 24, |frame| {
        bottom = dialogs::draw_help(frame, usize::MAX, &Theme::default())
    });
    assert!(bottom > 0 && bottom < 1000);
    assert!(screen.contains("Close picker"));
    for (width, height) in [(1, 1), (2, 2), (10, 4)] {
        rendered(width, height, |frame| {
            dialogs::draw_help(frame, usize::MAX, &Theme::default());
        });
    }
}

#[test]
fn usability_command_picker_keeps_last_command_visible() {
    let state = crate::app::CommandPickerState {
        commands: (0..50)
            .map(|i| crate::app::CommandEntry {
                name: format!("command-{i:02}"),
                source: format!("source-{}", i / 5),
                path: None,
                action: crate::app::CommandAction::SlashCommand,
            })
            .collect(),
        selected: 49,
        from_view: None,
    };
    let screen = rendered(80, 24, |frame| {
        picker::draw_command_picker(frame, &state, &Theme::default())
    });
    assert!(screen.contains("command-49"));
}

#[test]
fn usability_status_feedback_is_not_overwritten_by_usage() {
    let mut app = crate::app::App::new_for_test(
        crate::project::ProjectStore::empty(),
        Box::new(crate::traits::MockTmuxOps::new()),
        Box::new(crate::traits::MockWorktreeOps::new()),
    );
    let message = "Error: Could not start feature; check the configured harness and retry";
    app.message = Some(message.into());
    let screen = rendered(80, 4, |frame| status::draw(frame, &app, frame.area()));
    assert!(screen.contains(message));
}

fn screen_rows(app: &mut crate::app::App, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| dashboard::draw(frame, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

fn test_app() -> crate::app::App {
    crate::app::App::new_for_test(
        crate::project::ProjectStore::empty(),
        Box::new(crate::traits::MockTmuxOps::new()),
        Box::new(crate::traits::MockWorktreeOps::new()),
    )
}

#[test]
fn usability_stacked_toasts_never_share_a_row_with_the_status_bar() {
    for (w, h) in [(80, 24), (120, 40)] {
        let mut app = test_app();
        app.message = Some("Error: Could not start feature; retry".into());
        for i in 0..6 {
            app.push_toast_error(format!("Failure number {i} with a fairly long explanation"));
            app.push_toast_warning("S opens saved transcripts: select a claude, codex or opencode feature or session first");
        }
        assert!(app.toasts.len() <= crate::app::toast::MAX_TOASTS);
        let rows = screen_rows(&mut app, w, h);
        let status_top = (h - 3) as usize;
        for (y, row) in rows.iter().enumerate() {
            if row.contains("Could not start feature") {
                assert!(
                    !row.contains('┌') && !row.contains('┐') && row.matches('│').count() <= 2,
                    "toast border shares status row {y}: {row:?}"
                );
            }
            if y >= status_top {
                assert!(
                    !row.contains("──") || !row.contains('!'),
                    "toast fragment in status area row {y}: {row:?}"
                );
            }
        }
        assert!(rows.iter().any(|r| r.contains("Failure number 5")));
    }
}

#[test]
fn usability_error_messages_are_never_prefixed_twice() {
    let mut app = test_app();
    app.show_error(anyhow::anyhow!("Error: disk is full"));
    let message = app.message.clone().unwrap();
    assert_eq!(message.matches("Error:").count(), 1, "{message}");
    app.push_toast_error("Error: nope");
    app.push_toast_error("Error: Error: still nope");
    let rows = screen_rows(&mut app, 100, 30).join("\n");
    assert!(!rows.contains("Error: Error:"));
    assert!(!rows.contains("Error: nope"));
    // Actionable messages drop the generic log pointer; others name the key.
    assert!(!crate::app::App::logged_error_message("Could not save; retry").contains("debug log"));
    assert!(crate::app::App::logged_error_message("Boom").ends_with("Press D for the debug log."));
}
