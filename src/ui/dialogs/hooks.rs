use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{ChangeReasonState, HookPromptState, RunningHookState};

use super::super::dashboard::centered_rect;

pub fn draw_change_reason_dialog(frame: &mut Frame, state: &ChangeReasonState) {
    let area = centered_rect(80, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Review change ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // file
            Constraint::Length(1), // tool
            Constraint::Length(1), // separator
            Constraint::Length(6), // diff
            Constraint::Length(1), // separator
            Constraint::Length(2), // reason
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let file_line = Paragraph::new(Line::from(vec![
        Span::styled(" File: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            &state.relative_path,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(file_line, chunks[0]);

    let tool_label = if state.tool == "edit" {
        "EDIT"
    } else {
        "WRITE"
    };
    let tool_line = Paragraph::new(Line::from(vec![
        Span::styled(" Tool: ", Style::default().fg(Color::DarkGray)),
        Span::styled(tool_label, Style::default().fg(Color::Yellow)),
    ]));
    frame.render_widget(tool_line, chunks[1]);

    let mut diff_lines = vec![];

    // Show old content (removed)
    if !state.old_snippet.is_empty() {
        diff_lines.push(Line::from(Span::styled(
            " Removed:",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        )));
        for line in state.old_snippet.lines().take(3) {
            let truncated = if line.len() > 70 { &line[..70] } else { line };
            diff_lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(truncated, Style::default().fg(Color::Red)),
            ]));
        }
        if state.old_snippet.lines().count() > 3 {
            diff_lines.push(Line::from(Span::styled(
                "  ...",
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    // Show new content (added)
    diff_lines.push(Line::from(""));
    diff_lines.push(Line::from(Span::styled(
        " Added:",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    )));

    let new_preview: String = state
        .new_snippet
        .lines()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let truncated = if new_preview.len() > 60 {
        format!("{}...", &new_preview[..57])
    } else {
        new_preview
    };
    diff_lines.push(Line::from(vec![
        Span::styled(" + ", Style::default().fg(Color::Green)),
        Span::styled(truncated, Style::default().fg(Color::Green)),
    ]));

    let diff_widget = Paragraph::new(diff_lines);
    frame.render_widget(diff_widget, chunks[2]);

    let reason_line = Paragraph::new(Line::from(vec![
        Span::styled(" Reason: ", Style::default().fg(Color::Green)),
        Span::styled(&state.reason, Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(reason_line, chunks[3]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" accept  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" skip  "),
        Span::styled("r", Style::default().fg(Color::Red)),
        Span::raw(" reject"),
    ]));
    frame.render_widget(hints, chunks[5]);
}

pub fn draw_running_hook_dialog(
    frame: &mut Frame,
    state: &RunningHookState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let area = centered_rect(90, 70, frame.area());
    frame.render_widget(Clear, area);

    let is_running = state.child.is_some();
    let border_color = if is_running {
        Color::Cyan
    } else if state.success.unwrap_or(false) {
        Color::Green
    } else {
        Color::Red
    };

    let block = Block::default()
        .title(" Running Hook ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let status_text = if is_running {
        let throbber = throbber_widgets_tui::Throbber::default()
            .throbber_style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
            .use_type(throbber_widgets_tui::WhichUse::Spin);
        let span = throbber.to_symbol_span(throbber_state);
        Line::from(vec![
            Span::styled(" ", Style::default()),
            span,
            Span::styled(" Running hook...", Style::default().fg(Color::Cyan)),
        ])
    } else if state.success.unwrap_or(false) {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✓ ", Style::default().fg(Color::Green)),
            Span::styled(
                "Hook completed successfully",
                Style::default().fg(Color::Green),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✗ ", Style::default().fg(Color::Red)),
            Span::styled("Hook failed", Style::default().fg(Color::Red)),
        ])
    };
    frame.render_widget(Paragraph::new(status_text), chunks[0]);

    let script_line = Paragraph::new(Line::from(vec![
        Span::styled(" Script: ", Style::default().fg(Color::DarkGray)),
        Span::styled(state.script.clone(), Style::default().fg(Color::White)),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(script_line, chunks[1]);

    // Show the last N lines of captured stdout/stderr output.
    // Subtract 1 for the Borders::TOP header row.
    let output_height = chunks[2].height.saturating_sub(1) as usize;
    let all_lines: Vec<&str> = state.output.lines().collect();
    let start = all_lines.len().saturating_sub(output_height);
    let output_lines: Vec<Line> = if all_lines.is_empty() {
        vec![Line::from(Span::styled(
            " (no output yet)",
            Style::default().fg(Color::DarkGray),
        ))]
    } else {
        all_lines[start..]
            .iter()
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {}", l),
                    Style::default().fg(Color::White),
                ))
            })
            .collect()
    };
    let output_para = Paragraph::new(output_lines)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(Span::styled(
                    " output ",
                    Style::default().fg(Color::DarkGray),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(output_para, chunks[2]);

    let hints = if is_running {
        Paragraph::new(Line::from(Span::styled(
            " Please wait...",
            Style::default().fg(Color::DarkGray),
        )))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(Color::Yellow)),
            Span::raw(" continue  "),
            Span::styled("Esc", Style::default().fg(Color::Yellow)),
            Span::raw(" skip"),
        ]))
    };
    frame.render_widget(hints, chunks[3]);
}

pub fn draw_hook_prompt_dialog(frame: &mut Frame, state: &HookPromptState) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", state.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let items: Vec<ListItem> = state
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            if i == state.selected {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        " > ",
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        opt.as_str(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(opt.as_str(), Style::default().fg(Color::White)),
                ]))
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(Color::Yellow)),
        Span::raw(" move  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_latest_prompt_dialog(frame: &mut Frame, prompt: &str) {
    let area = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Latest Prompt ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let paragraph = Paragraph::new(prompt)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::White));
    frame.render_widget(paragraph, chunks[0]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::styled("/q", Style::default().fg(Color::Yellow)),
        Span::styled(" close", Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(hint, chunks[1]);
}
