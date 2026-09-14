use ratatui::{
    Frame,
    layout::Alignment,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use crate::app::{PairingDialogStatus, RemotePairingState};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_remote_pairing_dialog(
    frame: &mut Frame,
    state: &RemotePairingState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let qr_rows = state.qr_lines.len() as u16;
    // QR (if any) + blank + code line + blank + status line + blank + hint,
    // clamped so a small terminal still gets a scrollable-looking box
    // rather than an error — the content just won't all fit, which is no
    // worse than any other dialog on a tiny terminal.
    let height_pct = if qr_rows == 0 { 30 } else { 70 };
    let area = centered_rect(64, height_pct, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let border_color = match state.status {
        PairingDialogStatus::Paired { .. } => theme.success.to_color(),
        PairingDialogStatus::Failed(_) => theme.danger.to_color(),
        PairingDialogStatus::Waiting => theme.primary.to_color(),
    };
    let block = Block::default()
        .title(" Pair a Device ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if state.qr_lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "(QR code unavailable — use the number below)",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    } else {
        for row in &state.qr_lines {
            lines.push(
                Line::from(Span::styled(
                    row.clone(),
                    Style::default().fg(theme.text.to_color()),
                ))
                .alignment(Alignment::Center),
            );
        }
    }
    lines.push(Line::from(""));

    let spaced_code: String = state
        .code
        .chars()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join(" ");
    lines.push(
        Line::from(Span::styled(
            spaced_code,
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
    );
    lines.push(Line::from(""));

    let status_line = match &state.status {
        PairingDialogStatus::Waiting => {
            let throbber = throbber_widgets_tui::Throbber::default()
                .throbber_style(Style::default().fg(theme.primary.to_color()))
                .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
                .use_type(throbber_widgets_tui::WhichUse::Spin);
            let spinner = throbber.to_symbol_span(throbber_state);
            Line::from(vec![
                spinner,
                Span::styled(
                    " Waiting for phone to scan…",
                    Style::default().fg(theme.text_muted.to_color()),
                ),
            ])
            .alignment(Alignment::Center)
        }
        PairingDialogStatus::Paired { device_name } => Line::from(Span::styled(
            format!("✓ Paired: {device_name}"),
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        PairingDialogStatus::Failed(msg) => Line::from(Span::styled(
            msg.clone(),
            Style::default().fg(theme.danger.to_color()),
        ))
        .alignment(Alignment::Center),
    };
    lines.push(status_line);
    lines.push(
        Line::from(Span::styled(
            format!("Server: {}", state.addr),
            Style::default().fg(theme.text_muted.to_color()),
        ))
        .alignment(Alignment::Center),
    );
    lines.push(Line::from(""));

    let already_paired = matches!(state.status, PairingDialogStatus::Paired { .. });
    let hint_line = if already_paired {
        Line::from(vec![
            Span::styled(" Enter/Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(" close", Style::default().fg(theme.text_muted.to_color())),
        ])
    } else {
        Line::from(vec![
            Span::styled(" r", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " new code   ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
        ])
    }
    .alignment(Alignment::Center);
    lines.push(hint_line);

    frame.render_widget(Paragraph::new(lines), inner);
}
