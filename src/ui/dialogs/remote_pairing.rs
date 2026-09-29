use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::remote_tailscale::{RemoteTailscaleState, SetupStep, StepState};
use crate::app::{
    App, PairingDialogStatus, PairingDialogView, PairingUrlSource, RemoteDevicesListState,
    RemotePairingState,
};
use crate::tailscale::ServeOutcome;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_remote_pairing_dialog(frame: &mut Frame, app: &App, state: &RemotePairingState) {
    let theme = &app.theme;
    let throbber_state = &app.throbber_state;
    let tailscale = &app.remote_tailscale;
    match &state.view {
        PairingDialogView::Devices(list) => {
            draw_paired_devices_list(frame, list, theme);
            return;
        }
        PairingDialogView::Setup { scroll } => {
            draw_setup(frame, &app.pairing_setup_steps(), tailscale, *scroll, theme);
            return;
        }
        PairingDialogView::Pairing => {}
    }

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
            format!("Open: {}", state.url),
            Style::default().fg(theme.text_muted.to_color()),
        ))
        .alignment(Alignment::Center),
    );
    let muted = Style::default().fg(theme.text_muted.to_color());
    let warning = Style::default().fg(theme.warning.to_color());
    let offer_serve = tailscale
        .status
        .as_ref()
        .is_some_and(|status| status.can_start_serving());
    if state.url_source == PairingUrlSource::Tailscale {
        lines.push(
            Line::from(Span::styled(
                "via Tailscale — only your tailnet can open it",
                muted,
            ))
            .alignment(Alignment::Center),
        );
    }
    if state.url_unreachable {
        lines.push(
            Line::from(Span::styled(
                "⚠ Phones can't open this (except over USB, adb reverse).",
                warning,
            ))
            .alignment(Alignment::Center),
        );
        lines.push(
            Line::from(Span::styled(
                if offer_serve {
                    "Tailscale is running: press t to share AMF on it."
                } else {
                    "Press s to set up Tailscale for an HTTPS address."
                },
                warning,
            ))
            .alignment(Alignment::Center),
        );
    }
    for line in serve_note_lines(tailscale, theme) {
        lines.push(line.alignment(Alignment::Center));
    }
    lines.push(Line::from(""));

    let already_paired = matches!(state.status, PairingDialogStatus::Paired { .. });
    let hint_line = if already_paired {
        Line::from(vec![
            Span::styled(" Enter/Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " close   ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("v", Style::default().fg(theme.warning.to_color())),
            Span::styled(" devices", Style::default().fg(theme.text_muted.to_color())),
        ])
    } else {
        let mut keys = vec![("r", "new code"), ("v", "devices"), ("s", "setup")];
        if offer_serve {
            keys.push(("t", "serve"));
        }
        keys.push(("Esc", "cancel"));
        hint_spans(&keys, theme)
    }
    .alignment(Alignment::Center);
    lines.push(hint_line);

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// ` key label   key label` in the dialogs' hint style.
fn hint_spans(keys: &[(&str, &str)], theme: &Theme) -> Line<'static> {
    let key_style = Style::default().fg(theme.warning.to_color());
    let label_style = Style::default().fg(theme.text_muted.to_color());
    let mut spans = Vec::new();
    for (i, (key, label)) in keys.iter().enumerate() {
        let gap = if i + 1 == keys.len() { "" } else { "   " };
        spans.push(Span::styled(format!(" {key}"), key_style));
        spans.push(Span::styled(format!(" {label}{gap}"), label_style));
    }
    Line::from(spans)
}

/// What a `t` (tailscale serve) is doing or did, when it needs saying.
fn serve_note_lines(tailscale: &RemoteTailscaleState, theme: &Theme) -> Vec<Line<'static>> {
    if tailscale.serving {
        return vec![Line::from(Span::styled(
            "Asking Tailscale to serve AMF…",
            Style::default().fg(theme.primary.to_color()),
        ))];
    }
    match &tailscale.serve_note {
        Some(ServeOutcome::NeedsApproval(link)) => {
            let warning = Style::default().fg(theme.warning.to_color());
            vec![
                Line::from(Span::styled(
                    "Approve Serve for your tailnet (o opens the link):",
                    warning,
                )),
                Line::from(Span::styled(link.clone(), warning)),
                Line::from(Span::styled("then press t again.", warning)),
            ]
        }
        Some(ServeOutcome::Failed(why)) => vec![Line::from(Span::styled(
            format!("tailscale serve failed: {why}"),
            Style::default().fg(theme.danger.to_color()),
        ))],
        Some(ServeOutcome::Serving) | None => Vec::new(),
    }
}

/// The setup walkthrough (`s`): every step from installing Tailscale to
/// scanning the QR, ticked from what the latest probe saw.
fn draw_setup(
    frame: &mut Frame,
    steps: &[SetupStep],
    tailscale: &RemoteTailscaleState,
    scroll: u16,
    theme: &Theme,
) {
    let area = centered_rect(80, 85, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(" AMF Remote setup — Tailscale ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [body, hints] = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

    let text = Style::default().fg(theme.text.to_color());
    let muted = Style::default().fg(theme.text_muted.to_color());
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "Tailscale gives your phone a private HTTPS address for AMF, which it",
            text,
        )),
        Line::from(Span::styled(
            "needs to install AMF Remote and get notifications. Ticks are checked",
            text,
        )),
        Line::from(Span::styled(
            if tailscale.probing {
                "against this computer's Tailscale — checking now…"
            } else {
                "against this computer's Tailscale; r checks again."
            },
            text,
        )),
    ];
    lines.extend(serve_note_lines(tailscale, theme));
    lines.push(Line::from(""));

    for (i, step) in steps.iter().enumerate() {
        let (marker, marker_style, title_style) = match step.state {
            StepState::Done => ("✓", Style::default().fg(theme.success.to_color()), muted),
            StepState::Todo => (
                "○",
                Style::default().fg(theme.warning.to_color()),
                text.add_modifier(Modifier::BOLD),
            ),
            StepState::Unknown => ("·", muted, text),
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {marker} "), marker_style),
            Span::styled(format!("{}. {}", i + 1, step.title), title_style),
        ]));
        for line in &step.lines {
            lines.push(Line::from(Span::styled(format!("     {line}"), muted)));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "`amf doctor` runs the same checks from a shell. Full guide: docs/remote-control.md",
        muted,
    )));

    let max_scroll = (lines.len() as u16).saturating_sub(body.height);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll.min(max_scroll), 0)),
        body,
    );

    let mut keys = vec![
        ("j/k", "scroll"),
        ("t", "tailscale serve"),
        ("c", "copy policy"),
    ];
    if matches!(tailscale.serve_note, Some(ServeOutcome::NeedsApproval(_))) {
        keys.push(("o", "open link"));
    }
    keys.extend([("r", "re-check"), ("Esc", "back")]);
    frame.render_widget(
        Paragraph::new(hint_spans(&keys, theme).alignment(Alignment::Center)),
        hints,
    );
}

/// The paired-devices sub-screen (`v` from the pairing dialog): one row per
/// device with paired/last-seen times and revoke status, cursor-navigable,
/// `d`/`d` to revoke the selected row.
fn draw_paired_devices_list(frame: &mut Frame, list: &RemoteDevicesListState, theme: &Theme) {
    let area = centered_rect(70, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Paired Devices ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();

    if list.devices.is_empty() {
        lines.push(Line::from(Span::styled(
            "No devices paired yet",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    } else {
        for (i, device) in list.devices.iter().enumerate() {
            let selected = i == list.selected;
            let marker = if selected { "> " } else { "  " };
            let name_style = if device.revoked {
                Style::default()
                    .fg(theme.text_muted.to_color())
                    .add_modifier(Modifier::CROSSED_OUT)
            } else if selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let last_seen = device
                .last_seen_at
                .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_else(|| "never".to_string());
            let status = if device.revoked { " (revoked)" } else { "" };
            lines.push(Line::from(vec![
                Span::styled(marker, Style::default().fg(theme.warning.to_color())),
                Span::styled(device.name.clone(), name_style),
                Span::styled(
                    format!(
                        "  paired {}  last seen {}{}",
                        device.paired_at.format("%Y-%m-%d"),
                        last_seen,
                        status
                    ),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
            ]));
        }
    }
    lines.push(Line::from(""));

    if list.confirm_revoke {
        lines.push(Line::from(Span::styled(
            "Press d again to confirm revoke",
            Style::default().fg(theme.danger.to_color()),
        )));
        lines.push(Line::from(""));
    }

    lines.push(
        Line::from(vec![
            Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
            Span::styled(" move   ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled("d", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " revoke   ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(" back", Style::default().fg(theme.text_muted.to_color())),
        ])
        .alignment(Alignment::Center),
    );

    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::AppMode;
    use crate::tailscale::{TailnetNode, TailscaleStatus};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use ratatui::{Terminal, backend::TestBackend};

    fn app() -> App {
        let mut app = App::new_for_test(
            crate::project::ProjectStore::empty(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.open_pairing_dialog_for_test("127.0.0.1:47800".parse().unwrap());
        app
    }

    /// One string per screen row, so assertions can't match across rows.
    fn screen(app: &App) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
        terminal
            .draw(|frame| {
                if let AppMode::RemotePairing(state) = &app.mode {
                    draw_remote_pairing_dialog(frame, app, state);
                }
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    fn shows(rows: &[String], text: &str) -> bool {
        rows.iter().any(|row| row.contains(text))
    }

    #[test]
    fn offers_t_when_tailscale_runs_but_does_not_serve_amf() {
        let mut app = app();
        app.remote_tailscale.status = Some(TailscaleStatus::Running(TailnetNode {
            dns_name: "pc.tail1.ts.net".into(),
            https_enabled: true,
            tagged_for_amf: false,
            serve_url: None,
        }));
        let rows = screen(&app);
        assert!(shows(&rows, "Phones can't open this"));
        assert!(shows(&rows, "press t to share AMF on it"));
        assert!(shows(&rows, " t serve "));
        assert!(shows(&rows, "s setup"));
    }

    #[test]
    fn the_setup_view_lists_the_steps_with_keys() {
        let mut app = app();
        app.remote_tailscale.status = Some(TailscaleStatus::NotInstalled);
        app.open_pairing_setup_view();
        let rows = screen(&app);
        assert!(shows(&rows, "AMF Remote setup"));
        assert!(shows(
            &rows,
            "○ 1. Install Tailscale on this computer and your phone"
        ));
        assert!(shows(&rows, "c copy policy"));
        assert!(shows(&rows, "Esc back"));
    }
}
