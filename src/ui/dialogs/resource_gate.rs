use crate::app::resource_gate::{LowMemory, OverLimit};
use crate::app::{PendingStart, ResourceConfirmState};
use crate::theme::Theme;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};


/// Pre-start warning: the machine is at the agent cap and/or low on memory.
/// Advisory only — confirming starts the agent anyway.
pub fn draw_resource_confirm_dialog(frame: &mut Frame, state: &ResourceConfirmState, theme: &Theme) {
    let mut body: Vec<Line> = Vec::new();
    if let Some(over) = state.over_limit {
        body.extend(over_limit_lines(&over, theme));
    }
    if state.over_limit.is_some() && state.low_memory.is_some() {
        body.push(Line::from(""));
    }
    if let Some(low) = state.low_memory {
        body.extend(low_memory_lines(&low, theme));
    }

    // Sized to what it actually says: heading, the tripped gate(s), a blank
    // line, the question, and the key hints, inside the border. A fixed
    // percentage would leave this dialog mostly empty, since one tripped gate
    // is three short lines.
    let height = (body.len() as u16) + 7;
    let area = centered_rect_height(64, height, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Resource Check ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let heading = Paragraph::new(Line::from(Span::styled(
        format!(" {}", pending_summary(&state.pending)),
        Style::default()
            .fg(theme.warning.to_color())
            .add_modifier(Modifier::BOLD),
    )));
    frame.render_widget(heading, chunks[0]);

    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), chunks[1]);

    let prompt = Paragraph::new(Line::from(vec![
        Span::styled(
            " Start anyway? ",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled("(y/n)", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(prompt, chunks[2]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" y", Style::default().fg(theme.warning.to_color())),
        Span::raw(" start anyway  "),
        Span::styled("n/Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[3]);
}

/// A centered box `percent_x` wide and exactly `height` rows tall (clamped to
/// the screen).
fn centered_rect_height(percent_x: u16, height: u16, area: Rect) -> Rect {
    let height = height.min(area.height);
    let width = area.width * percent_x / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

fn pending_summary(pending: &PendingStart) -> String {
    match pending {
        PendingStart::Feature { .. } => "Starting a feature's agent".to_string(),
        PendingStart::BuiltinSession { label, .. } => match label {
            Some(label) => format!("Adding agent session '{label}'"),
            None => "Adding an agent session".to_string(),
        },
    }
}

fn over_limit_lines<'a>(over: &OverLimit, theme: &Theme) -> Vec<Line<'a>> {
    vec![
        Line::from(Span::styled(
            format!(
                " {} agent{} already running (limit {}).",
                over.active,
                if over.active == 1 { "" } else { "s" },
                over.limit
            ),
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            " Counts harness sessions across all projects, plus".to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            " any headless review or plan run in flight.".to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ]
}

fn low_memory_lines<'a>(low: &LowMemory, theme: &Theme) -> Vec<Line<'a>> {
    let mut lines = vec![Line::from(Span::styled(
        format!(
            " {} MiB memory available, below the {} MiB floor.",
            low.snapshot.available_mb, low.threshold_mb
        ),
        Style::default().fg(theme.text.to_color()),
    ))];
    if let (Some(free), Some(total)) = (low.snapshot.swap_free_mb, low.snapshot.swap_total_mb) {
        lines.push(Line::from(Span::styled(
            format!(" Swap: {free} MiB free of {total} MiB."),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    lines.push(Line::from(Span::styled(
        format!(" Measured from {}.", low.snapshot.source.label()),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    lines
}
