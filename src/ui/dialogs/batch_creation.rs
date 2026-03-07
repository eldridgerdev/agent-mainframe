use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{CreateBatchFeaturesState, CreateBatchFeaturesStep};
use crate::project::AgentKind;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_create_batch_features_dialog(
    frame: &mut Frame,
    state: &CreateBatchFeaturesState,
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
            draw_batch_feature_settings(frame, state, theme);
        }
    }
}

fn draw_batch_workspace_path(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) {
    let area = centered_rect(60, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create Batch Features ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let label = Paragraph::new(Line::from(Span::styled(
        " Workspace Path:",
        Style::default().fg(theme.primary.to_color()),
    )));
    frame.render_widget(label, chunks[0]);

    let input = Paragraph::new(state.workspace_path.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color())),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(input, chunks[1]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" next  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[2]);

    let info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" Note: ", Style::default().fg(theme.primary.to_color())),
            Span::raw("Path must be a git repository"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default().fg(theme.primary.to_color())),
            Span::raw("This will be root for all features"),
        ]),
    ])
    .style(Style::default().fg(theme.text_muted.to_color()));
    frame.render_widget(info, chunks[3]);
}

fn draw_batch_project_name(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) {
    let area = centered_rect(60, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create Batch Features ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let label = Paragraph::new(Line::from(Span::styled(
        " Project Name:",
        Style::default().fg(theme.primary.to_color()),
    )));
    frame.render_widget(label, chunks[0]);

    let input = Paragraph::new(state.project_name.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color())),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(input, chunks[1]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" next  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[2]);

    let info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" Note: ", Style::default().fg(theme.primary.to_color())),
            Span::raw("Auto-detected from workspace path"),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default().fg(theme.primary.to_color())),
            Span::raw("You can override if needed"),
        ]),
    ])
    .style(Style::default().fg(theme.text_muted.to_color()));
    frame.render_widget(info, chunks[3]);
}

fn draw_batch_feature_base_name(
    frame: &mut Frame,
    state: &CreateBatchFeaturesState,
    theme: &Theme,
) {
    let area = centered_rect(60, 30, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create Batch Features ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let label = Paragraph::new(Line::from(Span::styled(
        " Feature Base Name:",
        Style::default().fg(theme.primary.to_color()),
    )));
    frame.render_widget(label, chunks[0]);

    let input = Paragraph::new(state.feature_prefix.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color())),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(input, chunks[1]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" next  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[2]);

    let features_preview: Vec<String> = (1..=state.feature_count)
        .map(|i| format!("{}-{}", state.feature_prefix, i))
        .collect();
    let features_text = format!("Will create: {}", features_preview.join(", "));

    let info = Paragraph::new(vec![Line::from(vec![
        Span::styled(" Note: ", Style::default().fg(theme.primary.to_color())),
        Span::raw(features_text),
    ])])
    .style(Style::default().fg(theme.text_muted.to_color()));
    frame.render_widget(info, chunks[3]);
}

fn draw_batch_feature_count(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) {
    let area = centered_rect(60, 30, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create Batch Features ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let label = Paragraph::new(Line::from(Span::styled(
        " Number of Features:",
        Style::default().fg(theme.primary.to_color()),
    )));
    frame.render_widget(label, chunks[0]);

    let count_text = format!("  {}", state.feature_count);
    let input = Paragraph::new(count_text.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color())),
        )
        .wrap(Wrap { trim: true });
    frame.render_widget(input, chunks[1]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" adjust  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" next  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[2]);

    let features_preview: Vec<String> = (1..=state.feature_count.min(5))
        .map(|i| format!("{}-{}", state.feature_prefix, i))
        .collect();
    let preview_text = if state.feature_count > 5 {
        format!("Features: {}, ...", features_preview.join(", "))
    } else {
        format!("Features: {}", features_preview.join(", "))
    };

    let info = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" Note: ", Style::default().fg(theme.primary.to_color())),
            Span::raw(preview_text),
        ]),
        Line::from(vec![
            Span::styled("       ", Style::default().fg(theme.primary.to_color())),
            Span::raw("All features will be worktrees"),
        ]),
    ])
    .style(Style::default().fg(theme.text_muted.to_color()));
    frame.render_widget(info, chunks[3]);
}

fn draw_batch_feature_settings(frame: &mut Frame, state: &CreateBatchFeaturesState, theme: &Theme) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Create Batch Features ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let constraints: Vec<Constraint> = if state.agent == AgentKind::Claude {
        vec![
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ]
    } else {
        vec![
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
        ]
    };

    let settings_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(chunks[1]);

    let review_value = if state.review { "On" } else { "Off" };
    let chrome_value = if state.enable_chrome { "On" } else { "Off" };
    let notes_value = if state.enable_notes { "On" } else { "Off" };

    let fields: Vec<(String, String)> = if state.agent == AgentKind::Claude {
        vec![
            ("Agent".to_string(), state.agent.display_name().to_string()),
            ("Mode".to_string(), state.mode.display_name().to_string()),
            ("Review".to_string(), review_value.to_string()),
            ("Chrome".to_string(), chrome_value.to_string()),
            ("Notes".to_string(), notes_value.to_string()),
        ]
    } else {
        vec![
            ("Agent".to_string(), state.agent.display_name().to_string()),
            ("Mode".to_string(), state.mode.display_name().to_string()),
            ("Review".to_string(), review_value.to_string()),
            ("Notes".to_string(), notes_value.to_string()),
        ]
    };

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
            Span::styled(format!(" {}:", label), label_style),
            Span::raw(" "),
            Span::styled(value.as_str(), value_style),
        ]);

        let block = if is_focused {
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color()))
        } else {
            Block::default().borders(Borders::ALL)
        };

        let paragraph = Paragraph::new(text).block(block);
        frame.render_widget(paragraph, settings_chunks[idx]);
    }

    let hints = Paragraph::new(Line::from(vec![
        Span::styled("j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" change  "),
        Span::styled("Tab", Style::default().fg(theme.warning.to_color())),
        Span::raw(" field  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" back"),
    ]));
    frame.render_widget(hints, chunks[2]);
}
