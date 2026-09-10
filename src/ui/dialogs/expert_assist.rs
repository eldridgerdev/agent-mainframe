use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use super::super::dashboard::centered_rect;
use crate::app::expert_assist::{ExpertAssistField, ExpertAssistPhase, ExpertAssistState};
use crate::theme::Theme;

pub fn draw_expert_assist_dialog(frame: &mut Frame, state: &ExpertAssistState, theme: &Theme) {
    let area = centered_rect(72, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(" Expert Assist ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(2),
            Constraint::Length(1),
        ])
        .split(inner);
    let label = |name: &str, active: bool| {
        Line::from(vec![Span::styled(
            name.to_string(),
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(if active {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        )])
    };
    let field = |name: &str, value: &str, active: bool| {
        Paragraph::new(vec![
            label(name, active),
            Line::from(Span::styled(
                value.to_string(),
                Style::default().fg(theme.text.to_color()),
            )),
        ])
        .wrap(Wrap { trim: false })
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![Span::styled(
            "Ask a bounded expert for advice or a proposed patch.",
            Style::default().fg(theme.text.to_color()),
        )])),
        chunks[0],
    );
    frame.render_widget(
        field(
            "Question",
            &state.question,
            state.field == ExpertAssistField::Question,
        ),
        chunks[1],
    );
    frame.render_widget(
        field(
            "Acceptance criteria",
            &state.acceptance_criteria,
            state.field == ExpertAssistField::Criteria,
        ),
        chunks[2],
    );
    frame.render_widget(
        field(
            "Attempted fixes (optional)",
            &state.attempted_fixes,
            state.field == ExpertAssistField::AttemptedFixes,
        ),
        chunks[3],
    );
    let phase = match state.phase {
        ExpertAssistPhase::Draft => "Draft",
        ExpertAssistPhase::Running => "Ready for pre-call confirmation",
        ExpertAssistPhase::MissingEvidence => "Missing evidence",
        ExpertAssistPhase::Failed => "Failed",
        ExpertAssistPhase::Ready => "Ready handoff",
    };
    let response_text = if state.editing_handoff {
        Some(state.handoff_buffer.as_str())
    } else {
        state.response.as_deref()
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(
                    format!("{phase}: "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::raw(state.status.clone()),
            ]),
            response_text
                .map(|r| Line::from(r.to_string()))
                .unwrap_or_else(|| Line::raw("")),
        ])
        .wrap(Wrap { trim: false }),
        chunks[4],
    );
    let footer = if state.editing_handoff {
        "Type to edit handoff  Enter: save  Esc: cancel"
    } else if state.phase == ExpertAssistPhase::Ready {
        "v: view  d: dismiss  s: send (requires target validation)  Esc: close"
    } else {
        "Tab/j/k: field  Enter: validate  Esc: cancel"
    };
    frame.render_widget(Paragraph::new(Line::from(footer)), chunks[5]);
}
