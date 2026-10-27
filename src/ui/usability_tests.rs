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

fn hint(key: &'static str, label: &'static str, priority: u8) -> Hint {
    Hint {
        key,
        label,
        priority,
    }
}

fn line_text(line: &ratatui::text::Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[test]
fn fit_hints_drops_low_priority_hints_whole_instead_of_clipping() {
    let hints = [
        hint("Enter", "send", 0),
        hint("Ctrl+T", "vim", 1),
        hint("Esc", "close", 0),
        hint("Ctrl+S", "save", 6),
    ];
    let full = line_text(&fit_hints(&hints, 200, Style::default(), Style::default()));
    assert_eq!(full, " Enter send  Ctrl+T vim  Esc close  Ctrl+S save");
    let narrow = line_text(&fit_hints(&hints, 36, Style::default(), Style::default()));
    assert_eq!(narrow, " Enter send  Ctrl+T vim  Esc close");
    // Priority 0 survives even when nothing fits.
    let tiny = line_text(&fit_hints(&hints, 5, Style::default(), Style::default()));
    assert_eq!(tiny, " Enter send  Esc close");
}

fn compose_state() -> crate::app::ComposeState {
    let view = crate::app::ViewState::new(
        "proj".into(),
        "feat".into(),
        "amf-feat".into(),
        "claude".into(),
        "Claude 1".into(),
        crate::project::SessionKind::Claude,
        crate::project::VibeMode::Vibeless,
        false,
    );
    crate::app::ComposeState::new(
        view,
        std::path::PathBuf::from("/tmp"),
        String::new(),
        vec![],
    )
}

#[test]
fn compose_bar_hints_are_whole_words_at_80_and_120_columns() {
    for (width, height) in [(80, 24), (120, 40)] {
        let mut state = compose_state();
        let screen = rendered(width, height, |frame| {
            dialogs::draw_compose_dialog(frame, &mut state, &Theme::default())
        });
        for text in ["Enter send", "Esc close", "Ctrl+T vim"] {
            assert!(screen.contains(text), "missing {text} at {width}x{height}");
        }
        // Whatever is shown is complete: no hint key is cut after its key.
        for partial in ["Ctrl+E dir ", "Ctrl+S sa ", "Ctrl+Space lea"] {
            assert!(!screen.contains(partial), "{partial} at {width}");
        }
    }
}

#[test]
fn session_switcher_footer_wraps_and_keeps_new_and_rename_at_80_and_120() {
    for (width, height) in [(80, 24), (120, 40)] {
        let state = crate::app::SessionSwitcherState {
            project_name: "proj".into(),
            feature_name: "feat".into(),
            tmux_session: "amf-feat".into(),
            sessions: vec![crate::app::SwitcherEntry {
                tmux_window: "claude".into(),
                kind: crate::project::SessionKind::Claude,
                label: "Claude 1".into(),
                icon: None,
                icon_nerd: None,
            }],
            selected: 0,
            return_window: "claude".into(),
            return_label: "Claude 1".into(),
            vibe_mode: crate::project::VibeMode::Vibeless,
            review: false,
        };
        // Key and label are joined by a non-breaking space so a wrap never
        // strands "r" from "rename".
        let screen = rendered(width, height, |frame| {
            picker::draw_session_switcher(frame, &state, false, &Theme::default())
        })
        .replace('\u{a0}', " ");
        for text in ["s new", "r rename", "Esc cancel", "Enter select"] {
            assert!(screen.contains(text), "missing {text} at {width}x{height}");
        }
    }
}

#[test]
fn new_feature_mode_step_fits_or_scrolls_at_80x24() {
    let mut state = feature_form_state();
    state.agent = AgentKind::Claude;
    let agents = [AgentKind::Claude, AgentKind::Codex];
    let draw = |state: &crate::app::CreateFeatureState| {
        rendered(80, 24, |frame| {
            dialogs::draw_create_feature_dialog(
                frame,
                state,
                &[],
                &agents,
                &crate::usage::UsageData::default(),
                &Theme::default(),
            )
        })
    };
    for focus in 0..=6 {
        state.mode_focus = focus;
        let screen = draw(&state);
        // The focused row is on screen and the footer is never pushed off.
        let label = match focus {
            0 => "Harness:",
            1 => "Mode:",
            2 => "Review:",
            3 => "Plan:",
            4 => "Chrome:",
            5 => "Remote Control:",
            _ => "Steering:",
        };
        assert!(screen.contains(label), "focus {focus}: {label} off screen");
        assert!(screen.contains("Ctrl+S create"), "focus {focus}: footer");
    }
    state.mode_focus = 1;
    let screen = draw(&state);
    for mode in crate::project::VibeMode::ALL {
        assert!(screen.contains(mode.display_name()), "mode list clipped");
    }
    state.step = crate::app::CreateFeatureStep::SessionName;
    assert!(draw(&state).contains("Session:"));
}

#[test]
fn syntax_parser_rows_have_no_repeated_available_badge() {
    use crate::highlight::{HighlightInstallState, HighlightLanguage};
    let state = crate::app::SyntaxLanguagePickerState {
        languages: HighlightLanguage::ALL
            .iter()
            .enumerate()
            .map(|(i, language)| crate::app::SyntaxLanguageRow {
                language: *language,
                status: if i == 0 {
                    HighlightInstallState::Installed
                } else {
                    HighlightInstallState::Available
                },
            })
            .collect(),
        selected: 0,
        notice: None,
        operation: None,
        return_to: None,
        auto_return_on_success: false,
        return_language: None,
    };
    let total = state.languages.len();
    for (width, height) in [(80, 24), (120, 40)] {
        let screen = rendered(width, height, |frame| {
            picker::draw_syntax_language_picker(
                frame,
                &state,
                &throbber_widgets_tui::ThrobberState::default(),
                &Theme::default(),
            )
        });
        assert!(!screen.contains("AVAILABLE"), "{width}x{height}");
        assert!(screen.contains("INSTALLED"));
        assert!(screen.contains(&format!("1/{total} installed")));

        // Available rows have no gap between the cursor column and the name.
        let rows = rendered_rows(width, height, |frame| {
            picker::draw_syntax_language_picker(
                frame,
                &state,
                &throbber_widgets_tui::ThrobberState::default(),
                &Theme::default(),
            )
        });
        let name = HighlightLanguage::ALL[1].picker_title();
        let row = rows
            .iter()
            .find(|r| r.contains(name) && r.contains(".") && !r.contains("INSTALLED"))
            .unwrap_or_else(|| panic!("no row for {name}: {rows:#?}"));
        assert!(
            !row.contains(&format!("      {name}")),
            "{width}x{height}: gap before name: {row:?}"
        );

        // A description is whole, ends in an ellipsis, or is left out: it is
        // never cut off at the dialog edge. Small terminals also keep most
        // of the list on screen.
        let mut visible = 0;
        for language in HighlightLanguage::ALL {
            let Some(row) = rows.iter().find(|r| {
                r.contains(language.picker_title()) && r.contains(language.extension_summary())
            }) else {
                continue;
            };
            visible += 1;
            let description = language.picker_description();
            let prefix: String = description.chars().take(4).collect();
            if row.contains(&prefix) {
                assert!(
                    row.contains(description) || row.contains('…'),
                    "{width}x{height}: description cut mid-word: {row:?}"
                );
            }
        }
        if height == 24 {
            assert!(
                visible >= 10,
                "{width}x{height}: only {visible} rows visible"
            );
        }

        // Footer hints are whole: a hint never starts on one line and ends on
        // the next, so a key is always on the same line as its label.
        for (key, label) in [
            ("Enter/i", "install or reinstall"),
            ("x", "uninstall"),
            ("r", "refresh"),
            ("Esc", "cancel"),
        ] {
            let hint = format!("{key} {label}");
            assert!(
                rows.iter().any(|r| r.contains(&hint)),
                "{width}x{height}: hint split or missing: {hint:?} in {rows:#?}"
            );
        }
    }
}

#[test]
fn new_feature_mode_descriptions_are_whole_at_80_and_120_columns() {
    let mut state = feature_form_state();
    state.mode_focus = 1;
    for (width, full) in [(80u16, false), (120, true)] {
        let rows = rendered_rows(width, 40, |frame| {
            dialogs::draw_create_feature_dialog(
                frame,
                &state,
                &[],
                &[AgentKind::Claude, AgentKind::Codex],
                &crate::usage::UsageData::default(),
                &Theme::default(),
            )
        });
        let screen = rows.join("\n");
        for mode in crate::project::VibeMode::ALL {
            let whole = screen.contains(mode.description());
            let short = screen.contains(mode.short_description());
            if full {
                assert!(whole, "{width}: {} missing\n{screen}", mode.description());
            } else {
                assert!(whole || short, "{width}: {mode:?} clipped\n{screen}");
            }
        }
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
