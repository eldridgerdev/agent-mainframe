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
        path_is_git: None,
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
        bottom = dialogs::draw_help(frame, usize::MAX, "", false, &Theme::default())
    });
    assert!(bottom > 0 && bottom < 1000);
    // The last row of the last section is reachable at the bottom.
    assert!(screen.contains("PR Triage / Review (legacy)"));
    for (width, height) in [(1, 1), (2, 2), (10, 4)] {
        rendered(width, height, |frame| {
            dialogs::draw_help(frame, usize::MAX, "", false, &Theme::default());
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

#[test]
fn project_form_warns_when_the_folder_is_not_a_git_repository() {
    let state = |path_is_git| CreateProjectState {
        step: CreateProjectStep::Path,
        name: "example".into(),
        path: "/plain".into(),
        agent: AgentKind::Claude,
        agent_index: 0,
        path_is_git,
    };
    let draw = |state: &CreateProjectState| {
        rendered(100, 30, |frame| {
            dialogs::draw_create_project_dialog(
                frame,
                state,
                &[AgentKind::Claude],
                None,
                &Theme::default(),
            )
        })
    };
    let warned = draw(&state(Some(false)));
    for text in ["not a git repository", "worktrees", "Ctrl+G", "git init"] {
        assert!(warned.contains(text), "missing {text}");
    }
    assert!(!draw(&state(Some(true))).contains("not a git repository"));
    assert!(!draw(&state(None)).contains("not a git repository"));
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
                // The status bar's own box owns the first and last column.
                let inner: String = row.chars().skip(1).take(w as usize - 2).collect();
                assert!(
                    !inner.contains(['┌', '┐', '└', '┘']),
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

fn rendered_rows(width: u16, height: u16, mut draw: impl FnMut(&mut Frame)) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
    terminal.draw(|frame| draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn project_form_path_hint_keeps_its_own_row_under_a_long_path() {
    let long_path = format!("/home/someone/{}/my-app", "nested-folder/".repeat(10));
    let state = CreateProjectState {
        step: CreateProjectStep::Path,
        name: "example".into(),
        path: long_path.clone(),
        agent: AgentKind::Claude,
        agent_index: 0,
        path_is_git: None,
    };
    let rows = rendered_rows(100, 30, |frame| {
        dialogs::draw_create_project_dialog(
            frame,
            &state,
            &[AgentKind::Claude],
            None,
            &Theme::default(),
        )
    });
    let hint = rows
        .iter()
        .position(|row| row.contains("Ctrl+B browse folders"))
        .expect("browse hint shown");
    assert!(rows[hint].contains("Ctrl+U clear field"));
    assert!(
        !rows[hint].contains("nested-folder"),
        "hint not on a path row"
    );
    assert!(
        rows[..hint].concat().contains("my-app"),
        "path ends above it"
    );

    let blank = CreateProjectState {
        path: String::new(),
        ..state
    };
    let screen = rendered(100, 30, |frame| {
        dialogs::draw_create_project_dialog(
            frame,
            &blank,
            &[AgentKind::Claude],
            None,
            &Theme::default(),
        )
    });
    assert!(
        screen.contains("type the folder's full path"),
        "placeholder"
    );
}

fn feature_form_state() -> crate::app::CreateFeatureState {
    let mut state = crate::app::CreateFeatureState::new(
        "demo".into(),
        std::path::PathBuf::from("/tmp/demo"),
        Vec::new(),
        true,
    );
    state.branch = "login".into();
    state.step = crate::app::CreateFeatureStep::Mode;
    state
}

fn draw_feature_form(state: &crate::app::CreateFeatureState) -> Vec<String> {
    rendered_rows(120, 40, |frame| {
        dialogs::draw_create_feature_dialog(
            frame,
            state,
            &[],
            &[AgentKind::Claude, AgentKind::Codex],
            &crate::usage::UsageData::default(),
            &Theme::default(),
        )
    })
}

#[test]
fn feature_form_marks_the_focused_row_and_says_what_enter_does() {
    let mut state = feature_form_state();
    state.mode_focus = 1;
    let rows = draw_feature_form(&state);
    let screen = rows.concat();
    assert!(screen.contains("\u{25b8}Mode:"), "focused row is marked");
    assert!(!screen.contains("\u{25b8}Review:"));
    for text in [
        "Enter next field",
        "Ctrl+S create",
        "Tab/Shift+Tab",
        "Session: Claude 1",
    ] {
        assert!(screen.contains(text), "missing {text}");
    }
    // One plain line per mode on what the agent may do without asking.
    for mode in crate::project::VibeMode::ALL {
        assert!(
            screen.contains(mode.permission_summary()),
            "missing {}",
            mode.display_name()
        );
    }

    state.mode_focus = 2;
    let screen = draw_feature_form(&state).concat();
    assert!(screen.contains("\u{25b8}Review:"));
    assert!(screen.contains("Space or j/k toggle [x]"));

    state.step = crate::app::CreateFeatureStep::SessionName;
    let screen = draw_feature_form(&state).concat();
    assert!(screen.contains("\u{25b8}Session:"));
    assert!(screen.contains("Enter create"));
    assert!(!screen.contains("Name Session"), "no separate prompt");
}

#[test]
fn harness_manager_header_and_footer_name_the_same_keys() {
    let state = crate::app::HarnessSetupState::new(false, &[AgentKind::Claude]);
    let screen = rendered(120, 40, |frame| {
        dialogs::draw_harness_setup_dialog(
            frame,
            &state,
            &throbber_widgets_tui::ThrobberState::default(),
            &Theme::default(),
        )
    });
    for text in ["Enter/Space", "toggle", "c saves", "c save", "Esc cancel"] {
        assert!(screen.contains(text), "missing {text}");
    }
    assert!(!screen.contains("q exit"));
    assert!(!screen.contains("Enter to check & enable"));
}

#[test]
fn feature_form_footer_fits_at_80_columns() {
    let state = feature_form_state();
    let screen = rendered(80, 24, |frame| {
        dialogs::draw_create_feature_dialog(
            frame,
            &state,
            &[],
            &[AgentKind::Claude],
            &crate::usage::UsageData::default(),
            &Theme::default(),
        )
    });
    for text in ["Ctrl+S create", "Esc back", "Enter next"] {
        assert!(screen.contains(text), "missing {text}");
    }
}

fn discoverability_app(selection: &str) -> crate::app::App {
    use crate::project::{Feature, Project, ProjectStore, SessionKind, VibeMode};
    let mut store = ProjectStore::empty();
    if selection != "empty" {
        store.add_project(Project::new(
            "demo".into(),
            "/tmp/amf-discoverability-demo".into(),
            true,
            AgentKind::Claude,
        ));
    }
    if matches!(
        selection,
        "project" | "feature" | "bare-feature" | "session"
    ) {
        let mut feature = Feature::new(
            "first".into(),
            "first".into(),
            "/tmp/amf-discoverability-demo".into(),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        if selection != "bare-feature" {
            feature.add_session(SessionKind::Claude);
        }
        store.add_feature("demo", feature);
    }
    let mut app = crate::app::App::new_for_test(
        store,
        Box::new(crate::traits::MockTmuxOps::new()),
        Box::new(crate::traits::MockWorktreeOps::new()),
    );
    app.selection = match selection {
        "feature" | "bare-feature" => crate::app::Selection::Feature(0, 0),
        "session" => crate::app::Selection::Session(0, 0, 0),
        _ => crate::app::Selection::Project(0),
    };
    app
}

const DISCOVERABILITY_ROWS: &[&str] = &[
    "empty",
    "bare-project",
    "project",
    "bare-feature",
    "feature",
    "session",
];

/// The footer's key row: the line above the bottom border.
fn footer_key_row(screen: &str, width: u16, height: u16) -> String {
    let rows: Vec<String> = screen
        .chars()
        .collect::<Vec<_>>()
        .chunks(width as usize)
        .map(|c| c.iter().collect())
        .collect();
    rows[height as usize - 2].clone()
}

#[test]
fn dashboard_footer_shows_a_key_strip_with_help_at_every_size() {
    for (w, h) in [(80u16, 24u16), (120, 40)] {
        for selection in DISCOVERABILITY_ROWS {
            let mut app = discoverability_app(selection);
            let screen = rendered(w, h, |frame| crate::ui::draw(frame, &mut app));
            let row = footer_key_row(&screen, w, h);
            // One line, fully visible: `? help` ends it and the right border
            // is intact.
            assert!(row.contains("? help"), "{w}x{h} {selection}: {row:?}");
            assert!(
                row.trim_end().ends_with('\u{2502}'),
                "{w}x{h} {selection}: {row:?}"
            );
            let expect = match *selection {
                "empty" => "Press N to add a project",
                "bare-project" => "Press n to add a feature",
                "project" => "n new feature",
                "bare-feature" => "Press s to add a session",
                "feature" => "s add session",
                _ => "Enter open",
            };
            assert!(row.contains(expect), "{w}x{h} {selection}: {row:?}");
            // The selection line above it is still there.
            let above = footer_key_row(&screen, w, h - 1);
            if *selection != "empty" {
                assert!(
                    above.contains("demo") || above.contains("first") || above.contains("claude"),
                    "{w}x{h} {selection}: selection line lost: {above:?}"
                );
            }
        }
    }
}

#[test]
fn dashboard_footer_keys_all_have_help_rows() {
    for selection in DISCOVERABILITY_ROWS {
        let app = discoverability_app(selection);
        let strip = status::dashboard_key_strip(&app);
        let lead_keys = strip
            .lead
            .unwrap_or_default()
            .iter()
            .skip(1)
            .step_by(2)
            .copied();
        for key in strip.keys.iter().map(|(k, _)| *k).chain(lead_keys) {
            assert!(
                dialogs::DASHBOARD_KEYS.iter().any(|(_, k, _)| *k == key),
                "{selection}: footer key {key:?} has no help row"
            );
        }
    }
}

#[test]
fn session_picker_names_each_kind_and_its_count() {
    let state = crate::app::SessionPickerState {
        builtin_sessions: [
            (crate::project::SessionKind::Claude, "Claude"),
            (crate::project::SessionKind::Terminal, "Terminal"),
            (crate::project::SessionKind::Nvim, "Neovim"),
            (crate::project::SessionKind::Vscode, "VSCode"),
            (crate::project::SessionKind::Todos, "TODOs"),
        ]
        .into_iter()
        .map(|(kind, label)| crate::app::BuiltinSessionOption {
            kind,
            label: label.into(),
            disabled: None,
        })
        .collect(),
        custom_sessions: vec![],
        selected: 1,
        pi: 0,
        fi: 0,
        from_view: None,
    };
    for (w, h) in [(80u16, 24u16), (120, 40)] {
        let screen = rendered(w, h, |frame| {
            picker::draw_session_picker(
                frame,
                &state,
                &crate::usage::UsageData::default(),
                false,
                &Theme::default(),
            )
        });
        assert!(
            screen.contains("Start a session \u{00b7} 5 options"),
            "{w}x{h}"
        );
        for word in [
            "AI agent",
            "shell",
            "editor in AMF",
            "opens VS Code",
            "TODO list",
        ] {
            assert!(screen.contains(word), "{w}x{h}: missing {word}");
        }
        assert!(screen.contains("Esc cancel"), "{w}x{h}: footer clipped");
        assert!(!screen.contains("  * "), "{w}x{h}: glyph still drawn");
        assert!(!screen.contains("  V "), "{w}x{h}: glyph still drawn");
    }
}
