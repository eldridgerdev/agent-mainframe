use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Padding, Paragraph, Wrap},
};

use crate::app::{PlaceholderFillState, PromptEditorFocus, PromptEditorState, PromptLibraryState};
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
            "Press / to search (use #tag to filter by tag)",
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
    let preview_outer = preview_block.inner(body[1]);
    frame.render_widget(preview_block, body[1]);

    // Reserve the last preview line for the resolved source path, so the
    // user always sees where the selected entry lives on disk.
    let preview_split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(preview_outer);
    let preview_inner = preview_split[0];

    if let Some(entry) = state.selected_entry() {
        let path_line = match &entry.source_path {
            Some(path) => Line::from(Span::styled(
                crate::app::util::shorten_path(path),
                Style::default()
                    .fg(theme.text_muted.to_color())
                    .add_modifier(Modifier::ITALIC),
            )),
            None => Line::from(Span::styled(
                "(no saved location)",
                Style::default()
                    .fg(theme.text_muted.to_color())
                    .add_modifier(Modifier::ITALIC),
            )),
        };
        frame.render_widget(Paragraph::new(path_line), preview_split[1]);
    }

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
        // Tag chips, e.g. `#bug #frontend`, above the body preview.
        if !entry.template.tags.is_empty() {
            let chips: Vec<Span> = entry
                .template
                .tags
                .iter()
                .flat_map(|tag| {
                    [
                        Span::styled(
                            format!("#{tag}"),
                            Style::default().fg(theme.info.to_color()),
                        ),
                        Span::raw(" "),
                    ]
                })
                .collect();
            lines.push(Line::from(chips));
            lines.push(Line::from(""));
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

    use crate::prompt_library::PromptSource;

    let name_focused = state.focus == PromptEditorFocus::Name;
    let tags_focused = state.focus == PromptEditorFocus::Tags;
    let body_focused = state.focus == PromptEditorFocus::Body;

    let hint = |key: &'static str| Span::styled(key, Style::default().fg(theme.warning.to_color()));
    let label = |text: &'static str| Span::styled(text, Style::default().fg(theme.text_muted.to_color()));

    // ── Outer dialog frame: title above, key hints on the bottom border.
    let title = if state.editing_id.is_some() {
        match state.editing_source {
            PromptSource::User => " Edit Prompt ".to_string(),
            PromptSource::Project => " Edit Prompt — Project config ".to_string(),
            PromptSource::Global => " Edit Prompt — Global config ".to_string(),
            PromptSource::Worktree => " Edit Prompt — Worktree config ".to_string(),
        }
    } else {
        " New Prompt ".to_string()
    };
    let footer = Line::from(vec![
        label(" "),
        hint("Tab"),
        label(" switch  "),
        hint("Ctrl+S"),
        label(" save  "),
        hint("Esc"),
        label(" cancel (×2 in body)  "),
        hint("Ctrl+Q"),
        label(" cancel "),
    ]);
    let block = Block::default()
        .title(title)
        .title_bottom(footer)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // name box
            Constraint::Length(1), // spacer
            Constraint::Length(3), // tags box
            Constraint::Length(1), // spacer
            Constraint::Min(4),    // body box
            Constraint::Length(1), // spacer
            Constraint::Length(2), // destination hint
        ])
        .split(inner);

    // A focus-aware bordered field box: the label is the box title, the
    // border lights up in the primary colour while the field has focus.
    let field_block = |title_text: String, focused: bool| {
        let border = if focused {
            theme.primary.to_color()
        } else {
            theme.border.to_color()
        };
        let title_style = if focused {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        Block::default()
            .title(Span::styled(format!(" {title_text} "), title_style))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border))
            .padding(Padding::horizontal(1))
    };

    // A single-line field value: cursor when focused, muted placeholder when
    // empty + unfocused, else the plain value.
    let single_line_field = |value: &str, focused: bool, placeholder: &'static str| {
        if focused {
            Line::from(vec![
                Span::styled(value.to_string(), Style::default().fg(theme.text.to_color())),
                Span::styled("█", Style::default().fg(theme.primary.to_color())),
            ])
        } else if value.is_empty() {
            Line::from(Span::styled(
                placeholder,
                Style::default().fg(theme.text_muted.to_color()),
            ))
        } else {
            Line::from(Span::styled(
                value.to_string(),
                Style::default().fg(theme.text.to_color()),
            ))
        }
    };

    // Name field.
    frame.render_widget(
        Paragraph::new(single_line_field(&state.name, name_focused, "(unnamed)"))
            .block(field_block("Name".to_string(), name_focused)),
        chunks[0],
    );

    // Tags field (the hint rides in the box title to keep the value clean).
    frame.render_widget(
        Paragraph::new(single_line_field(&state.tags, tags_focused, "none"))
            .block(field_block(
                "Tags — comma-separated, optional".to_string(),
                tags_focused,
            )),
        chunks[2],
    );

    // Body editor box, with the placeholder-syntax legend on its bottom border.
    let key_span = |s: &'static str| Span::styled(s, Style::default().fg(theme.primary.to_color()));
    let muted = |s: &'static str| Span::styled(s, Style::default().fg(theme.text_muted.to_color()));
    let help = Line::from(vec![
        muted(" "),
        key_span("{{name}}"),
        muted(" slot \u{00b7} "),
        key_span("{{a|b|c}}"),
        muted(" menu \u{00b7} "),
        key_span("{{label: a|b|c}}"),
        muted(" labelled menu "),
    ]);
    let body_border = if body_focused {
        theme.primary.to_color()
    } else {
        theme.border.to_color()
    };
    let body_title_style = if body_focused {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let body_block = Block::default()
        .title(Span::styled(" Prompt body ", body_title_style))
        .title_bottom(help)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(body_border))
        .padding(Padding::horizontal(1));
    // The placeholder-syntax legend on the box's bottom border already
    // explains slots, so the empty body just shows the cursor.
    let body_lines = editor_lines(&state.editor, theme, "");
    frame.render_widget(
        Paragraph::new(body_lines)
            .block(body_block)
            .wrap(Wrap { trim: false }),
        chunks[4],
    );

    // Destination hint at the bottom: where a save will land. User templates
    // live in the local SQLite store (not version-controlled); config sources
    // show the target config.json path.
    let muted_style = Style::default()
        .fg(theme.text_muted.to_color())
        .add_modifier(Modifier::ITALIC);
    let mut dest_lines: Vec<Line> = Vec::new();
    match state.editing_source {
        PromptSource::User => {
            dest_lines.push(Line::from(Span::styled(
                "Saves to your local store — not version-controlled",
                muted_style,
            )));
            if let Some(path) = &state.dest_path {
                dest_lines.push(Line::from(Span::styled(
                    crate::app::util::shorten_path(path),
                    Style::default().fg(theme.text_muted.to_color()),
                )));
            }
        }
        other => match &state.dest_path {
            Some(path) => {
                dest_lines.push(Line::from(Span::styled(
                    "Saves to this config file:",
                    muted_style,
                )));
                dest_lines.push(Line::from(Span::styled(
                    crate::app::util::shorten_path(path),
                    Style::default().fg(theme.text_muted.to_color()),
                )));
            }
            None => dest_lines.push(Line::from(Span::styled(
                format!("Saves to {} config", other.label().to_lowercase()),
                muted_style,
            ))),
        },
    }
    frame.render_widget(
        Paragraph::new(dest_lines).wrap(Wrap { trim: true }),
        chunks[6],
    );
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
        .map(|p| p.display_label().to_string())
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

