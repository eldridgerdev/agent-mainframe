use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use crate::app::{PlaceholderFillState, PromptEditorState, PromptLibraryState};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;
use super::editor_view::editor_lines;

/// Picker over the prompt library: a list of templates on the left with
/// a body preview on the right, plus a search line and footer hints.
pub fn draw_prompt_library(
    frame: &mut Frame,
    state: &PromptLibraryState,
    message: Option<&str>,
    theme: &Theme,
) {
    let area = centered_rect(82, 78, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let total = state.templates.len();
    let shown = state.filtered.len();
    let title = if state.query.is_empty() {
        format!(" Prompt Library ({total}) ")
    } else {
        format!(" Prompt Library ({shown}/{total}) ")
    };

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let msg_height = if message.is_some() { 1 } else { 0 };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),             // search line
            Constraint::Min(1),                // list + preview
            Constraint::Length(1),             // footer hints
            Constraint::Length(msg_height),    // message
        ])
        .split(inner);

    // ── Search line ──────────────────────────────────────────────
    let search_line = if state.search_active {
        Line::from(vec![
            Span::styled("Search: ", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                state.query.clone(),
                Style::default().fg(theme.text.to_color()),
            ),
            Span::styled("█", Style::default().fg(theme.primary.to_color())),
        ])
    } else if state.query.is_empty() {
        Line::from(Span::styled(
            "Press / to search",
            Style::default().fg(theme.text_muted.to_color()),
        ))
    } else {
        Line::from(vec![
            Span::styled("Filter: ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                state.query.clone(),
                Style::default().fg(theme.text.to_color()),
            ),
        ])
    };
    frame.render_widget(Paragraph::new(search_line), chunks[0]);

    if state.templates.is_empty() {
        let empty = Paragraph::new(
            "No saved prompts yet.\n\nPress n to create one, or save the compose box with Ctrl+P.",
        )
        .style(Style::default().fg(theme.text_muted.to_color()))
        .wrap(Wrap { trim: true });
        frame.render_widget(empty, chunks[1]);
        draw_footer(frame, chunks[2], theme, true, true);
        draw_message(frame, chunks[3], message, theme);
        return;
    }

    // ── List + preview ───────────────────────────────────────────
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(chunks[1]);

    let list_width = body[0].width as usize;
    let visible = body[0].height as usize;
    let scroll_offset = if state.selected >= visible {
        state.selected - visible + 1
    } else {
        0
    };

    let items: Vec<ListItem> = state
        .filtered
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible)
        .filter_map(|(pos, template_idx)| {
            let entry = state.templates.get(*template_idx)?;
            let selected = pos == state.selected;
            let marker = if selected { "▸ " } else { "  " };
            let name_style = if selected {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };

            let badge = format!(" [{}]", entry.source.label());
            let fixed = marker.len() + badge.len();
            let name = truncate(&entry.template.name, list_width.saturating_sub(fixed));

            Some(ListItem::new(Line::from(vec![
                Span::styled(marker.to_string(), name_style),
                Span::styled(name, name_style),
                Span::styled(badge, Style::default().fg(badge_color(entry.source, theme))),
            ])))
        })
        .collect();
    frame.render_widget(List::new(items), body[0]);

    // Preview of the selected template's body.
    let preview_block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(theme.border.to_color()));
    let preview_inner = preview_block.inner(body[1]);
    frame.render_widget(preview_block, body[1]);

    if let Some(entry) = state.selected_entry() {
        let mut lines: Vec<Line> = Vec::new();
        if let Some(desc) = &entry.template.description {
            if !desc.trim().is_empty() {
                lines.push(Line::from(Span::styled(
                    desc.clone(),
                    Style::default()
                        .fg(theme.text_muted.to_color())
                        .add_modifier(Modifier::ITALIC),
                )));
                lines.push(Line::from(""));
            }
        }
        for line in entry.template.body.lines() {
            lines.push(Line::from(Span::styled(
                line.to_string(),
                Style::default().fg(theme.text.to_color()),
            )));
        }
        frame.render_widget(
            Paragraph::new(lines).wrap(Wrap { trim: false }),
            preview_inner,
        );
    }

    let can_edit = state.selected_entry().is_some_and(|e| e.source.is_editable());
    let can_delete = state.selected_entry().is_some_and(|e| e.source.is_deletable());
    draw_footer(frame, chunks[2], theme, can_edit, can_delete);
    draw_message(frame, chunks[3], message, theme);
}

/// Distinguish template sources at a glance: editable `User` entries are
/// muted, read-only `Project` ones use the success accent, and `Global`
/// ones the info accent.
fn badge_color(source: crate::prompt_library::PromptSource, theme: &Theme) -> ratatui::style::Color {
    use crate::prompt_library::PromptSource;
    match source {
        PromptSource::User => theme.text_muted.to_color(),
        PromptSource::Project => theme.success.to_color(),
        PromptSource::Global => theme.info.to_color(),
        PromptSource::Worktree => theme.warning.to_color(),
    }
}

fn draw_footer(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    can_edit: bool,
    can_delete: bool,
) {
    let hint = |key: &'static str| Span::styled(key, Style::default().fg(theme.warning.to_color()));
    let label = |text: &'static str| Span::styled(text, Style::default().fg(theme.text_muted.to_color()));
    let mut spans = vec![
        hint("Enter"),
        label(" inject  "),
        hint("n"),
        label(" new  "),
    ];
    if can_edit {
        spans.extend([hint("e"), label(" edit  ")]);
    }
    if can_delete {
        spans.extend([hint("d"), label(" del  ")]);
    }
    spans.extend([
        hint("y"),
        label(" dup  "),
        hint("x"),
        label(" export(g/p/w)  "),
        hint("/"),
        label(" search  "),
        hint("Esc"),
        label(" close"),
    ]);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_message(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    message: Option<&str>,
    theme: &Theme,
) {
    if let Some(msg) = message {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                msg.to_string(),
                Style::default().fg(theme.success.to_color()),
            ))),
            area,
        );
    }
}

/// Create/edit dialog: a name field above a multi-line body editor.
pub fn draw_prompt_editor(frame: &mut Frame, state: &PromptEditorState, theme: &Theme) {
    let area = centered_rect(78, 74, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = if state.editing_id.is_some() {
        use crate::prompt_library::PromptSource;
        match state.editing_source {
            PromptSource::User => " Edit Prompt ".to_string(),
            PromptSource::Project => " Edit Prompt [Project config] ".to_string(),
            PromptSource::Global => " Edit Prompt [Global config] ".to_string(),
            PromptSource::Worktree => " Edit Prompt [Worktree config] ".to_string(),
        }
    } else {
        " New Prompt ".to_string()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // name label
            Constraint::Length(1), // name field
            Constraint::Length(1), // spacer
            Constraint::Length(1), // body label
            Constraint::Length(2), // placeholder help (always visible)
            Constraint::Min(3),    // body editor
            Constraint::Length(1), // footer hints
        ])
        .split(inner);

    let active_marker = |active: bool| {
        if active {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted.to_color())
        }
    };

    frame.render_widget(
        Paragraph::new(Span::styled("Name", active_marker(state.name_field_active))),
        chunks[0],
    );
    let name_line = if state.name_field_active {
        Line::from(vec![
            Span::styled(state.name.clone(), Style::default().fg(theme.text.to_color())),
            Span::styled("█", Style::default().fg(theme.primary.to_color())),
        ])
    } else if state.name.is_empty() {
        Line::from(Span::styled(
            "(unnamed)",
            Style::default().fg(theme.text_muted.to_color()),
        ))
    } else {
        Line::from(Span::styled(
            state.name.clone(),
            Style::default().fg(theme.text.to_color()),
        ))
    };
    frame.render_widget(Paragraph::new(name_line), chunks[1]);

    frame.render_widget(
        Paragraph::new(Span::styled(
            "Prompt body",
            active_marker(!state.name_field_active),
        )),
        chunks[3],
    );

    // Always-visible placeholder help, kept off the editor so it stays put
    // while typing. Explains both the text-slot and the option-list forms.
    let help = Line::from(vec![
        Span::styled(
            "{{name}}",
            Style::default().fg(theme.primary.to_color()),
        ),
        Span::styled(
            " fill-in slot \u{00b7} options: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            "{{env|dev|staging|prod}}",
            Style::default().fg(theme.primary.to_color()),
        ),
        Span::styled(
            " shows a menu to pick from",
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]);
    frame.render_widget(Paragraph::new(help).wrap(Wrap { trim: true }), chunks[4]);

    // Help lives on its own line above, so the editor placeholder is empty.
    let body_lines = editor_lines(&state.editor, theme, "");
    frame.render_widget(
        Paragraph::new(body_lines).wrap(Wrap { trim: false }),
        chunks[5],
    );

    let hint = |key: &'static str| Span::styled(key, Style::default().fg(theme.warning.to_color()));
    let label = |text: &'static str| Span::styled(text, Style::default().fg(theme.text_muted.to_color()));
    let footer = Line::from(vec![
        hint("Tab"),
        label(" switch field  "),
        hint("Ctrl+S"),
        label(" save  "),
        hint("Esc"),
        label(" cancel (×2 in body)  "),
        hint("Ctrl+Q"),
        label(" cancel"),
    ]);
    frame.render_widget(Paragraph::new(footer), chunks[6]);
}

/// Fill-in flow: one slot at a time with a `current/total` progress
/// counter, the slot label, the field editor, and footer hints.
pub fn draw_placeholder_fill(frame: &mut Frame, state: &PlaceholderFillState, theme: &Theme) {
    let area = centered_rect(70, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let total = state.placeholders.len();
    let pos = state.current + 1;
    let title = format!(" Fill \u{2014} {} ({pos}/{total}) ", state.template.name);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // slot label
            Constraint::Length(2), // help line (always visible)
            Constraint::Length(1), // spacer
            Constraint::Min(3),    // field editor / option list
            Constraint::Length(1), // footer hints
        ])
        .split(inner);

    let label = state
        .current_placeholder()
        .map(|p| p.label.as_deref().unwrap_or(&p.key).to_string())
        .unwrap_or_default();
    let required = state
        .current_placeholder()
        .map(|p| p.required)
        .unwrap_or(false);
    let mut label_spans = vec![Span::styled(
        label,
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )];
    if required {
        label_spans.push(Span::styled(
            " *",
            Style::default().fg(theme.warning.to_color()),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(label_spans)), chunks[0]);

    // Always-visible help, tailored to the slot kind. Kept separate from the
    // editor so it stays put while the user types.
    let help_text = if state.is_select() {
        "Choose an option with \u{2191}/\u{2193} (or j/k), then Tab to confirm and continue."
    } else if state.current_is_multiline() {
        "Type a value. Enter adds a new line; Tab continues to the next field."
    } else {
        "Type a value, then Tab or Enter to continue to the next field."
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            help_text,
            Style::default().fg(theme.text_muted.to_color()),
        ))
        .wrap(Wrap { trim: true }),
        chunks[1],
    );

    if state.is_select() {
        // Select slot: render the options as a navigable list.
        let items: Vec<Line> = state
            .current_options()
            .iter()
            .enumerate()
            .map(|(i, opt)| {
                let selected = i == state.select_index;
                let marker = if selected { "\u{25b8} " } else { "  " };
                let style = if selected {
                    Style::default()
                        .fg(theme.primary.to_color())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text.to_color())
                };
                Line::from(vec![
                    Span::styled(marker.to_string(), style),
                    Span::styled(opt.clone(), style),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(items), chunks[3]);
    } else {
        // Help lives on its own line above, so the editor placeholder is empty.
        let body_lines = editor_lines(&state.input, theme, "");
        frame.render_widget(
            Paragraph::new(body_lines).wrap(Wrap { trim: false }),
            chunks[3],
        );
    }

    let hint = |key: &'static str| Span::styled(key, Style::default().fg(theme.warning.to_color()));
    let label = |text: &'static str| Span::styled(text, Style::default().fg(theme.text_muted.to_color()));
    let last = state.current + 1 >= total;
    let mut footer_spans = Vec::new();
    if state.is_select() {
        footer_spans.extend([hint("\u{2191}/\u{2193}"), label(" choose  ")]);
    }
    footer_spans.extend([
        hint("Tab"),
        label(if last { " inject  " } else { " next  " }),
        hint("Shift+Tab"),
        label(" prev  "),
        hint("Ctrl+S"),
        label(" inject  "),
        hint("Esc"),
        label(" cancel"),
    ]);
    frame.render_widget(Paragraph::new(Line::from(footer_spans)), chunks[4]);
}

fn truncate(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    if max_chars == 1 {
        return "…".to_string();
    }
    let mut out: String = s.chars().take(max_chars - 1).collect();
    out.push('…');
    out
}
