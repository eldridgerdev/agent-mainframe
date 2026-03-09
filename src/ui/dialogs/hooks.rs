use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthChar;

use crate::app::{
    ChangeReasonState, HookPromptState, LatestPromptState, RunningHookState, TextSelection,
};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

const LATEST_PROMPT_WIDTH_PERCENT: u16 = 80;
const LATEST_PROMPT_HEIGHT_PERCENT: u16 = 70;

#[derive(Clone, Debug)]
pub struct WrappedPromptLine {
    start: usize,
    end: usize,
    columns: Vec<usize>,
}

impl WrappedPromptLine {
    fn column_count(&self) -> usize {
        self.columns.len()
    }

    fn byte_at_col(&self, col: usize) -> usize {
        self.columns.get(col).copied().unwrap_or(self.end)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LatestPromptDialogLayout {
    pub area: Rect,
    pub content: Rect,
    pub hint: Rect,
}

pub fn latest_prompt_dialog_layout(frame_area: Rect) -> LatestPromptDialogLayout {
    let area = centered_rect(LATEST_PROMPT_WIDTH_PERCENT, LATEST_PROMPT_HEIGHT_PERCENT, frame_area);
    let inner = Block::default().borders(Borders::ALL).inner(area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    LatestPromptDialogLayout {
        area,
        content: chunks[0],
        hint: chunks[1],
    }
}

pub fn wrap_latest_prompt(prompt: &str, width: u16) -> Vec<WrappedPromptLine> {
    let width = width.max(1) as usize;
    let mut lines = Vec::new();
    let mut line_start = 0;
    let mut line_end = 0;
    let mut line_width = 0;
    let mut line_columns = Vec::new();

    for (idx, ch) in prompt.char_indices() {
        if ch == '\n' {
            lines.push(WrappedPromptLine {
                start: line_start,
                end: line_end,
                columns: std::mem::take(&mut line_columns),
            });
            line_start = idx + ch.len_utf8();
            line_end = line_start;
            line_width = 0;
            continue;
        }

        let char_width = UnicodeWidthChar::width(ch).unwrap_or(1).max(1);
        if line_width + char_width > width && !line_columns.is_empty() {
            lines.push(WrappedPromptLine {
                start: line_start,
                end: line_end,
                columns: std::mem::take(&mut line_columns),
            });
            line_start = idx;
            line_width = 0;
        }

        for _ in 0..char_width {
            line_columns.push(idx);
        }
        line_end = idx + ch.len_utf8();
        line_width += char_width;
    }

    if !line_columns.is_empty() || lines.is_empty() || prompt.ends_with('\n') {
        lines.push(WrappedPromptLine {
            start: line_start,
            end: line_end,
            columns: line_columns,
        });
    }

    lines
}

pub fn latest_prompt_selected_text(prompt: &str, width: u16, selection: &TextSelection) -> String {
    if !selection.has_selection {
        return String::new();
    }

    let lines = wrap_latest_prompt(prompt, width);
    let (start_row, start_col, end_row, end_col) = selection.normalized();
    let Some(start_line) = lines.get(start_row as usize) else {
        return String::new();
    };
    let Some(end_line) = lines.get(end_row as usize) else {
        return String::new();
    };

    let start_byte = start_line.byte_at_col(start_col as usize);
    let end_byte = end_line.byte_at_col(end_col as usize);
    if start_byte >= end_byte {
        return String::new();
    }

    prompt[start_byte..end_byte].to_string()
}

fn latest_prompt_lines(
    prompt: &str,
    width: u16,
    height: u16,
    selection: &TextSelection,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let wrapped = wrap_latest_prompt(prompt, width);
    let mut lines = Vec::with_capacity(height as usize);
    let (sel_start_row, sel_start_col, sel_end_row, sel_end_col) = selection.normalized();

    for (row_idx, line) in wrapped.iter().enumerate().take(height as usize) {
        if line.start == line.end {
            lines.push(Line::raw(""));
            continue;
        }

        let row_u16 = row_idx as u16;
        let line_len = line.column_count();
        let selected_range = if selection.has_selection
            && row_u16 >= sel_start_row
            && row_u16 <= sel_end_row
        {
            let start = if row_u16 == sel_start_row {
                (sel_start_col as usize).min(line_len)
            } else {
                0
            };
            let end = if row_u16 == sel_end_row {
                (sel_end_col as usize).min(line_len)
            } else {
                line_len
            };
            (start < end).then_some((start, end))
        } else {
            None
        };

        if let Some((start, end)) = selected_range {
            let before_end = line.byte_at_col(start);
            let selected_end = line.byte_at_col(end);
            let mut spans = Vec::with_capacity(3);
            if before_end > line.start {
                spans.push(Span::styled(
                    prompt[line.start..before_end].to_string(),
                    Style::default().fg(theme.text.to_color()),
                ));
            }
            spans.push(Span::styled(
                prompt[before_end..selected_end].to_string(),
                Style::default()
                    .fg(theme.text.to_color())
                    .bg(theme.effective_selection_bg()),
            ));
            if selected_end < line.end {
                spans.push(Span::styled(
                    prompt[selected_end..line.end].to_string(),
                    Style::default().fg(theme.text.to_color()),
                ));
            }
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(Span::styled(
                prompt[line.start..line.end].to_string(),
                Style::default().fg(theme.text.to_color()),
            )));
        }
    }

    while lines.len() < height as usize {
        lines.push(Line::raw(""));
    }

    lines
}

pub fn draw_change_reason_dialog(frame: &mut Frame, state: &ChangeReasonState, theme: &Theme) {
    let area = centered_rect(80, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Review change ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

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
        Span::styled(" File: ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(
            &state.relative_path,
            Style::default()
                .fg(theme.primary.to_color())
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
        Span::styled(" Tool: ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(tool_label, Style::default().fg(theme.warning.to_color())),
    ]));
    frame.render_widget(tool_line, chunks[1]);

    let mut diff_lines = vec![];

    // Show old content (removed)
    if !state.old_snippet.is_empty() {
        diff_lines.push(Line::from(Span::styled(
            " Removed:",
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        )));
        for line in state.old_snippet.lines().take(3) {
            let truncated = if line.len() > 70 { &line[..70] } else { line };
            diff_lines.push(Line::from(vec![
                Span::styled("  ", Style::default()),
                Span::styled(truncated, Style::default().fg(theme.danger.to_color())),
            ]));
        }
        if state.old_snippet.lines().count() > 3 {
            diff_lines.push(Line::from(Span::styled(
                "  ...",
                Style::default().fg(theme.text_muted.to_color()),
            )));
        }
    }

    // Show new content (added)
    diff_lines.push(Line::from(""));
    diff_lines.push(Line::from(Span::styled(
        " Added:",
        Style::default()
            .fg(theme.success.to_color())
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
        Span::styled(" + ", Style::default().fg(theme.success.to_color())),
        Span::styled(truncated, Style::default().fg(theme.success.to_color())),
    ]));

    let diff_widget = Paragraph::new(diff_lines);
    frame.render_widget(diff_widget, chunks[2]);

    let reason_line = Paragraph::new(Line::from(vec![
        Span::styled(" Reason: ", Style::default().fg(theme.success.to_color())),
        Span::styled(&state.reason, Style::default().fg(theme.text.to_color())),
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(reason_line, chunks[3]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" accept  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" skip  "),
        Span::styled("r", Style::default().fg(theme.danger.to_color())),
        Span::raw(" reject"),
    ]));
    frame.render_widget(hints, chunks[5]);
}

pub fn draw_running_hook_dialog(
    frame: &mut Frame,
    state: &RunningHookState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(90, 70, frame.area());
    frame.render_widget(Clear, area);

    let is_running = state.child.is_some();
    let border_color = if is_running {
        theme.primary.to_color()
    } else if state.success.unwrap_or(false) {
        theme.success.to_color()
    } else {
        theme.danger.to_color()
    };

    let block = Block::default()
        .title(" Running Hook ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
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
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            )
            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
            .use_type(throbber_widgets_tui::WhichUse::Spin);
        let span = throbber.to_symbol_span(throbber_state);
        Line::from(vec![
            Span::styled(" ", Style::default()),
            span,
            Span::styled(
                " Running hook...",
                Style::default().fg(theme.primary.to_color()),
            ),
        ])
    } else if state.success.unwrap_or(false) {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✓ ", Style::default().fg(theme.success.to_color())),
            Span::styled(
                "Hook completed successfully",
                Style::default().fg(theme.success.to_color()),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✗ ", Style::default().fg(theme.danger.to_color())),
            Span::styled("Hook failed", Style::default().fg(theme.danger.to_color())),
        ])
    };
    frame.render_widget(Paragraph::new(status_text), chunks[0]);

    let script_line = Paragraph::new(Line::from(vec![
        Span::styled(
            " Script: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            state.script.clone(),
            Style::default().fg(theme.text.to_color()),
        ),
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
            Style::default().fg(theme.text_muted.to_color()),
        ))]
    } else {
        all_lines[start..]
            .iter()
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {}", l),
                    Style::default().fg(theme.text.to_color()),
                ))
            })
            .collect()
    };
    let output_para = Paragraph::new(output_lines)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.text_muted.to_color()))
                .title(Span::styled(
                    " output ",
                    Style::default().fg(theme.text_muted.to_color()),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(output_para, chunks[2]);

    let hints = if is_running {
        Paragraph::new(Line::from(vec![
            Span::styled(" h", Style::default().fg(theme.warning.to_color())),
            Span::styled(" hide  ", Style::default().fg(theme.text_muted.to_color())),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" continue  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" skip"),
        ]))
    };
    frame.render_widget(hints, chunks[3]);
}

pub fn draw_hook_prompt_dialog(frame: &mut Frame, state: &HookPromptState, theme: &Theme) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", state.title))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

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
                            .fg(theme.primary.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        opt.as_str(),
                        Style::default()
                            .fg(theme.primary.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(opt.as_str(), Style::default().fg(theme.text.to_color())),
                ]))
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" move  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_latest_prompt_dialog(frame: &mut Frame, state: &LatestPromptState, theme: &Theme) {
    let layout = latest_prompt_dialog_layout(frame.area());
    let area = layout.area;
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Latest Prompt ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    frame.render_widget(block, area);

    let paragraph = Paragraph::new(latest_prompt_lines(
        &state.prompt,
        layout.content.width,
        layout.content.height,
        &state.selection,
        theme,
    ))
    .style(Style::default().fg(theme.text.to_color()));
    frame.render_widget(paragraph, layout.content);

    let mut hint_spans = vec![
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled("/q", Style::default().fg(theme.warning.to_color())),
        Span::styled(" close  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("drag", Style::default().fg(theme.warning.to_color())),
        Span::styled(" copy", Style::default().fg(theme.text_muted.to_color())),
    ];
    if state.can_rerun {
        hint_spans.push(Span::styled(
            "  r/Enter",
            Style::default().fg(theme.warning.to_color()),
        ));
        hint_spans.push(Span::styled(
            " rerun",
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }

    let hint = Paragraph::new(Line::from(hint_spans));
    frame.render_widget(hint, layout.hint);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_prompt_selection_preserves_soft_wraps() {
        let prompt = "abcdefgh";
        let selection = TextSelection {
            start_row: 0,
            start_col: 2,
            end_row: 1,
            end_col: 2,
            is_selecting: false,
            has_selection: true,
        };

        assert_eq!(latest_prompt_selected_text(prompt, 4, &selection), "cdef");
    }

    #[test]
    fn latest_prompt_selection_preserves_real_newlines() {
        let prompt = "ab\ncd";
        let selection = TextSelection {
            start_row: 0,
            start_col: 1,
            end_row: 1,
            end_col: 1,
            is_selecting: false,
            has_selection: true,
        };

        assert_eq!(latest_prompt_selected_text(prompt, 4, &selection), "b\nc");
    }
}
