use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{ContextSettingsField, ContextSettingsState};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_context_settings_dialog(frame: &mut Frame, state: &ContextSettingsState, theme: &Theme) {
    let area = centered_rect(56, 42, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Context Window Settings ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Min(0),
        ])
        .split(inner);

    let field_line = |label: &str,
                       value: &str,
                       field: ContextSettingsField,
                       hint: &str|
     -> Vec<Line<'static>> {
        let focused = state.field == field;
        let label_style = if focused {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        let mut spans = vec![
            Span::styled(format!(" {label}: "), label_style),
            Span::styled(value.to_string(), Style::default().fg(theme.text.to_color())),
        ];
        if focused {
            spans.push(Span::styled(
                "\u{2588}",
                Style::default().fg(theme.primary.to_color()),
            ));
        }
        vec![
            Line::from(spans),
            Line::from(Span::styled(
                format!("   {hint}"),
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ]
    };

    let window_limit_display = if state.window_limit_input.is_empty() {
        "(none — use each harness's own default)".to_string()
    } else {
        state.window_limit_input.clone()
    };
    frame.render_widget(
        Paragraph::new(field_line(
            "Context window",
            &window_limit_display,
            ContextSettingsField::WindowLimit,
            "Tokens. Blank clears the override.",
        )),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(field_line(
            "Warning %",
            &state.warning_input,
            ContextSettingsField::WarningPercent,
            "Usage % at which the indicator turns WARNING.",
        )),
        chunks[1],
    );

    frame.render_widget(
        Paragraph::new(field_line(
            "Critical %",
            &state.critical_input,
            ContextSettingsField::CriticalPercent,
            "Usage % at which the indicator turns CRITICAL.",
        )),
        chunks[2],
    );

    if let Some(error) = &state.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(theme.danger.to_color()),
            ))),
            chunks[4],
        );
    }

    let hints = Line::from(vec![
        Span::styled(" Tab", Style::default().fg(theme.primary.to_color())),
        Span::styled(
            " next field  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.primary.to_color())),
        Span::styled(
            " save  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.primary.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]);
    frame.render_widget(Paragraph::new(hints), chunks[5]);
}
