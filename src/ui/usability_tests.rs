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
