use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{CreateBatchFeaturesState, CreateBatchFeaturesStep};
use crate::project::AgentKind;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_create_batch_features_dialog(
    frame: &mut Frame,
    state: &CreateBatchFeaturesState,
    message: Option<&str>,
    theme: &Theme,
) {
    match state.step {
        CreateBatchFeaturesStep::WorkspacePath => {
            draw_batch_workspace_path(frame, state, theme);
        }
        CreateBatchFeaturesStep::ProjectName => {
            draw_batch_project_name(frame, state, theme);
        }
        CreateBatchFeaturesStep::FeatureCount => {
            draw_batch_feature_count(frame, state, theme);
        }
        CreateBatchFeaturesStep::FeatureBaseName => {
            draw_batch_feature_base_name(frame, state, theme);
        }
        CreateBatchFeaturesStep::FeatureSettings => {
            draw_batch_feature_settings(frame, state, message, theme);
        }
    }
}

fn dialog_title(state: &CreateBatchFeaturesState) -> String {
    if state.existing_project {
        format!(" Batch Features ({}) ", state.project_name)
    } else {
        " Batch Features (new project) ".to_string()
    }
}

/// Draws the frame shared by every step and returns its inner area.
fn draw_frame(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) -> Rect {
    let area = centered_rect(60, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(dialog_title(state))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

/// "feature-1, feature-2, feature-3" (shortened past five names).
fn names_preview(state: &CreateBatchFeaturesState) -> String {
    let names = state.planned_names();
    if names.len() > 5 {
        format!(
            "{}, ... {}",
            names[..3].join(", "),
            names.last().cloned().unwrap_or_default()
        )
    } else {
        names.join(", ")
    }
}

/// What a batch is and what it makes, shown on its first screen so nobody
/// has to create one to find out. It also says plainly what a batch does not
/// support: per-feature names, prompts or plan interviews.
fn explainer(state: &CreateBatchFeaturesState, theme: &Theme) -> Paragraph<'static> {
    let muted = Style::default().fg(theme.text_muted.to_color());
    let text = Style::default().fg(theme.text.to_color());
    let into = if state.existing_project {
        format!("into '{}'", state.project_name)
    } else {
        "into a new project".to_string()
    };
    Paragraph::new(vec![
        Line::from(Span::styled(
            format!(
                " A batch creates {} features at once {}: {}.",
                state.feature_count,
                into,
                names_preview(state)
            ),
            text,
        )),
        Line::from(Span::styled(
            " Each gets its own git worktree and branch, and starts an agent with the same settings.",
            muted,
        )),
        Line::from(Span::styled(
            " No plan interview, and no per-feature names or prompts: open each feature to give its agent a task (r renames one).",
            muted,
        )),
    ])
    .wrap(Wrap { trim: false })
}

fn key_hints(theme: &Theme, keys: &[(&'static str, &'static str)]) -> Paragraph<'static> {
    let mut spans = Vec::new();
    for (key, label) in keys {
        spans.push(Span::styled(
            format!(" {key}"),
            Style::default().fg(theme.warning.to_color()),
        ));
        spans.push(Span::raw(format!(" {label} ")));
    }
    Paragraph::new(Line::from(spans)).wrap(Wrap { trim: false })
}

/// One step's content: an optional explanation, a labelled one-line input
/// box, key hints, and a note below.
struct InputStep<'a> {
    top: Option<Paragraph<'static>>,
    label: &'a str,
    value: &'a str,
    placeholder: &'a str,
    hints: Paragraph<'static>,
    note: Paragraph<'static>,
}

fn draw_input_step(frame: &mut Frame, inner: Rect, step: InputStep<'_>, theme: &Theme) {
    let InputStep {
        top,
        label,
        value,
        placeholder,
        hints,
        note,
    } = step;
    let top_height = if top.is_some() {
        top.as_ref()
            .map_or(0, |p| p.line_count(inner.width) as u16 + 1)
    } else {
        0
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_height),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(inner);
    if let Some(top) = top {
        frame.render_widget(top, chunks[0]);
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {label}"),
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[1],
    );
    let content = if value.is_empty() {
        Line::from(Span::styled(
            placeholder.to_string(),
            Style::default()
                .fg(theme.text_muted.to_color())
                .add_modifier(Modifier::ITALIC),
        ))
    } else {
        Line::from(vec![
            Span::raw(value.to_string()),
            Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
        ])
    };
    let input = Paragraph::new(content)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color())),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(input, chunks[2]);
    frame.render_widget(hints, chunks[3]);
    frame.render_widget(note, chunks[4]);
}

fn draw_batch_workspace_path(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) {
    let inner = draw_frame(frame, state, theme);
    let note = Paragraph::new(vec![Line::from(Span::styled(
        " The folder must be a git repository; it becomes the new project's root. To add a batch to an existing project, select it on the dashboard before pressing B.",
        Style::default().fg(theme.text_muted.to_color()),
    ))])
    .wrap(Wrap { trim: false });
    draw_input_step(
        frame,
        inner,
        InputStep {
            top: Some(explainer(state, theme)),
            label: "Repository path:",
            value: &state.workspace_path,
            placeholder: "type the repository's full path",
            hints: key_hints(
                theme,
                &[("Enter", "next"), ("Ctrl+U", "clear"), ("Esc", "cancel")],
            ),
            note,
        },
        theme,
    );
}

fn draw_batch_project_name(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) {
    let inner = draw_frame(frame, state, theme);
    let note = Paragraph::new(Line::from(Span::styled(
        " Defaults to the folder name; it must not match an existing project.",
        Style::default().fg(theme.text_muted.to_color()),
    )))
    .wrap(Wrap { trim: false });
    draw_input_step(
        frame,
        inner,
        InputStep {
            top: None,
            label: "New project name:",
            value: &state.project_name,
            placeholder: "type a project name",
            hints: key_hints(
                theme,
                &[("Enter", "next"), ("Ctrl+U", "clear"), ("Esc", "back")],
            ),
            note,
        },
        theme,
    );
}

fn draw_batch_feature_base_name(
    frame: &mut Frame,
    state: &CreateBatchFeaturesState,
    theme: &Theme,
) {
    let inner = draw_frame(frame, state, theme);
    let note = Paragraph::new(Line::from(vec![
        Span::styled(
            " Will create: ",
            Style::default().fg(theme.primary.to_color()),
        ),
        Span::styled(
            names_preview(state),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]))
    .wrap(Wrap { trim: false });
    draw_input_step(
        frame,
        inner,
        InputStep {
            top: None,
            label: "Name prefix (a number is appended):",
            value: &state.feature_prefix,
            placeholder: "type a prefix, e.g. feature-",
            hints: key_hints(
                theme,
                &[("Enter", "next"), ("Ctrl+U", "clear"), ("Esc", "back")],
            ),
            note,
        },
        theme,
    );
}

fn draw_batch_feature_count(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) {
    let inner = draw_frame(frame, state, theme);
    // A batch into an existing project starts on this step, so it carries the
    // explanation; one creating a project showed it on the path step.
    let top = state.existing_project.then(|| explainer(state, theme));
    let esc_label = if state.existing_project {
        "cancel"
    } else {
        "back"
    };
    let count = state.feature_count.to_string();
    draw_input_step(
        frame,
        inner,
        InputStep {
            top,
            label: "Number of features:",
            value: &count,
            placeholder: "",
            hints: key_hints(
                theme,
                &[
                    ("j/k", "adjust"),
                    ("0-9", "type"),
                    ("Enter", "next"),
                    ("Esc", esc_label),
                ],
            ),
            note: Paragraph::new(Line::from(Span::styled(
                format!(" Up to {}.", CreateBatchFeaturesState::MAX_FEATURES),
                Style::default().fg(theme.text_muted.to_color()),
            ))),
        },
        theme,
    );
}

fn draw_batch_feature_settings(
    frame: &mut Frame,
    state: &CreateBatchFeaturesState,
    message: Option<&str>,
    theme: &Theme,
) {
    let inner = draw_frame(frame, state, theme);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(2),
        ])
        .split(inner);

    let plan_notice = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" Note: ", Style::default().fg(theme.warning.to_color())),
            Span::raw("Plan interviews are skipped for batch creation."),
        ]),
        Line::from(Span::raw(format!(" Creates: {}", names_preview(state)))),
    ])
    .style(Style::default().fg(theme.text_muted.to_color()));
    frame.render_widget(plan_notice, chunks[0]);

    let row_count = if state.agent == AgentKind::Claude {
        4
    } else {
        3
    };
    let settings_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Length(3); row_count])
        .split(chunks[1]);

    let review_value = if state.review { "On" } else { "Off" };
    let chrome_value = if state.enable_chrome { "On" } else { "Off" };
    let mode_value = format!(
        "{} - {}",
        state.mode.display_name(),
        state.mode.permission_summary()
    );

    let mut fields: Vec<(&str, String)> = vec![
        ("Harness", state.agent.display_name().to_string()),
        ("Mode", mode_value),
        ("Review", review_value.to_string()),
    ];
    if state.agent == AgentKind::Claude {
        fields.push(("Chrome", chrome_value.to_string()));
    }

    for (idx, (label, value)) in fields.iter().enumerate() {
        let is_focused = state.mode_focus == idx;

        let label_style = if is_focused {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };

        let value_style = if is_focused {
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };

        let text = Line::from(vec![
            Span::styled(
                format!("{}{}:", if is_focused { "\u{25b8}" } else { " " }, label),
                label_style,
            ),
            Span::raw(" "),
            Span::styled(value.clone(), value_style),
        ]);

        let block = if is_focused {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color()))
        } else {
            Block::default().borders(Borders::ALL)
        };

        let paragraph = Paragraph::new(text).block(block);
        if let Some(area) = settings_chunks.get(idx) {
            frame.render_widget(paragraph, *area);
        }
    }

    // Why the last attempt failed (an existing feature name, no git, ...).
    // Without this the error was set but hidden behind the dialog.
    if let Some(message) = message {
        let style = if message.starts_with("Error") {
            Style::default().fg(theme.danger.to_color())
        } else {
            Style::default().fg(theme.text.to_color())
        };
        frame.render_widget(
            Paragraph::new(format!(" {message}"))
                .style(style)
                .wrap(Wrap { trim: false }),
            chunks[2],
        );
    }

    let last_field = state.mode_focus + 1 >= row_count;
    let enter_label = if last_field { "create" } else { "next field" };
    frame.render_widget(
        key_hints(
            theme,
            &[
                ("j/k/Space", "change"),
                ("Tab/Shift+Tab", "field"),
                ("Enter", enter_label),
                ("Esc", "back"),
            ],
        ),
        chunks[3],
    );
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn render(state: &CreateBatchFeaturesState, message: Option<&str>) -> String {
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_create_batch_features_dialog(frame, state, message, &Theme::default())
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn feature_settings_explain_that_plan_interviews_are_skipped() {
        let mut state = CreateBatchFeaturesState::with_workspace(Some("/tmp/repo".to_string()));
        state.step = CreateBatchFeaturesStep::FeatureSettings;
        assert!(render(&state, None).contains("Plan interviews are skipped for batch creation."));
    }

    #[test]
    fn a_batch_into_the_selected_project_explains_itself_on_its_first_screen() {
        let state =
            CreateBatchFeaturesState::for_project("demo".into(), std::path::Path::new("/tmp/demo"));
        assert_eq!(state.step, CreateBatchFeaturesStep::FeatureCount);
        let screen = render(&state, None);
        for text in [
            "Batch Features (demo)",
            "into 'demo'",
            "feature-1, feature-2, feature-3",
            "worktree",
            "No plan interview",
            "per-feature names or prompts",
            "cancel",
        ] {
            assert!(screen.contains(text), "missing {text}:\n{screen}");
        }
    }

    #[test]
    fn the_preview_names_match_what_the_batch_creates() {
        let mut state = CreateBatchFeaturesState::with_workspace(None);
        assert!(state.workspace_path.is_empty(), "never prefilled from cwd");
        state.feature_count = 2;
        assert_eq!(state.planned_names(), vec!["feature-1", "feature-2"]);
        state.feature_prefix = "plan".into();
        assert_eq!(state.planned_names(), vec!["plan1", "plan2"]);
    }

    #[test]
    fn a_failed_batch_shows_its_error_inside_the_dialog() {
        let mut state =
            CreateBatchFeaturesState::for_project("demo".into(), std::path::Path::new("/tmp/demo"));
        state.step = CreateBatchFeaturesStep::FeatureSettings;
        let screen = render(
            &state,
            Some("Error: Feature 'feature-1' already exists in 'demo'; choose another base name"),
        );
        assert!(screen.contains("already exists"), "{screen}");
    }
}
