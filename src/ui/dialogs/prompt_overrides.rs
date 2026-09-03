use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Padding, Paragraph, Wrap},
};

use crate::app::{PromptOverrideScope, PromptOverrideStep, PromptOverridesState};
use crate::editor::VimMode;
use crate::project::AgentKind;
use crate::prompts::PromptSource;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;
use super::editor_view::editor_lines;

pub fn draw_prompt_overrides(frame: &mut Frame, state: &PromptOverridesState, theme: &Theme) {
    let area = centered_rect(80, 82, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Headless Prompt Overrides ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.help_open {
        draw_help(frame, inner, theme);
        return;
    }
    match &state.edit {
        Some(edit) => match edit.step {
            PromptOverrideStep::Editing => draw_editor(frame, inner, state, theme),
            PromptOverrideStep::ScopePicker => draw_scope_picker(frame, inner, state, theme),
            PromptOverrideStep::HarnessPicker => draw_harness_picker(frame, inner, state, theme),
        },
        None => draw_list(frame, inner, state, theme),
    }
}

fn key(theme: &Theme, s: &str) -> Span<'static> {
    Span::styled(s.to_string(), Style::default().fg(theme.primary.to_color()))
}
fn muted(theme: &Theme, s: &str) -> Span<'static> {
    Span::styled(
        s.to_string(),
        Style::default().fg(theme.text_muted.to_color()),
    )
}

fn source_badge(theme: &Theme, source: PromptSource) -> Span<'static> {
    let (text, color) = match source {
        PromptSource::BuiltIn => ("built-in", theme.text_muted.to_color()),
        PromptSource::Feature => ("feature", theme.info.to_color()),
        PromptSource::Project => ("project", theme.success.to_color()),
        PromptSource::Global => ("global", theme.warning.to_color()),
    };
    Span::styled(format!("{text:>8}"), Style::default().fg(color))
}

fn draw_list(frame: &mut Frame, area: Rect, state: &PromptOverridesState, theme: &Theme) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(muted(
            theme,
            " Effective source per prompt · [F]eature [P]roject [G]lobal override present",
        ))),
        chunks[0],
    );

    let height = chunks[1].height as usize;
    let start = state.scroll.min(state.rows.len().saturating_sub(1));
    let rows: Vec<Line> = state
        .rows
        .iter()
        .enumerate()
        .skip(start)
        .take(height)
        .map(|(i, row)| {
            let selected = i == state.selected;
            let marker = if selected { "› " } else { "  " };
            let name_style = if selected {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let flags = format!(
                "{}{}{}",
                if row.has_feature { "F" } else { "·" },
                if row.has_project { "P" } else { "·" },
                if row.has_global { "G" } else { "·" },
            );
            Line::from(vec![
                Span::styled(marker.to_string(), name_style),
                source_badge(theme, row.source),
                Span::raw("  "),
                Span::styled(format!("{:<30}", row.id.spec().title), name_style),
                muted(theme, &format!(" {flags}  ")),
                muted(theme, row.id.as_str()),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), chunks[1]);

    let footer = Line::from(vec![
        key(theme, " j/k"),
        muted(theme, " move  "),
        key(theme, "Enter/e"),
        muted(theme, " edit  "),
        key(theme, "d"),
        muted(theme, " clear override  "),
        key(theme, "?"),
        muted(theme, " help  "),
        key(theme, "Esc"),
        muted(theme, " close"),
    ]);
    frame.render_widget(Paragraph::new(footer), chunks[2]);
}

fn draw_editor(frame: &mut Frame, area: Rect, state: &PromptOverridesState, theme: &Theme) {
    let Some(edit) = &state.edit else { return };
    let title = state
        .rows
        .get(edit.row)
        .map(|r| r.id.spec().title)
        .unwrap_or("prompt");

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!(" Editing: {title}"),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(muted(
                theme,
                " {{tokens}} are re-filled at run time — no validation; a dropped token renders literally.",
            )),
        ]),
        chunks[0],
    );

    let body_title = match edit.editor.vim_mode() {
        Some(VimMode::Insert) => " Template [Vim Insert] ",
        Some(VimMode::Normal) => " Template [Vim Normal] ",
        None => " Template ",
    };
    let body_block = Block::default()
        .title(Span::styled(
            body_title,
            Style::default().fg(theme.primary.to_color()),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border.to_color()))
        .padding(Padding::horizontal(1));
    frame.render_widget(
        Paragraph::new(editor_lines(&edit.editor, theme, ""))
            .block(body_block)
            .wrap(Wrap { trim: false }),
        chunks[1],
    );

    let footer = Line::from(vec![
        key(theme, " Ctrl+S"),
        muted(theme, " continue → scope  "),
        key(theme, "Ctrl+T"),
        muted(theme, " vim  "),
        key(theme, "Ctrl+Q"),
        muted(theme, " cancel"),
    ]);
    frame.render_widget(Paragraph::new(footer), chunks[2]);
}

fn draw_scope_picker(frame: &mut Frame, area: Rect, state: &PromptOverridesState, theme: &Theme) {
    let Some(edit) = &state.edit else { return };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Save this override to which scope?",
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );

    let rows: Vec<Line> = edit
        .scopes
        .iter()
        .enumerate()
        .map(|(i, scope)| {
            let sel = i == edit.scope_index;
            let (marker, style) = if sel {
                (
                    "› ",
                    Style::default()
                        .fg(theme.primary.to_color())
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("  ", Style::default().fg(theme.text.to_color()))
            };
            let hint = match scope {
                PromptOverrideScope::Feature => "amf.db · this checkout only",
                PromptOverrideScope::Project => "amf.json · committed, shared with the repo",
                PromptOverrideScope::Global => "amf.db · every project on this machine",
            };
            Line::from(vec![
                Span::styled(format!("{marker}{}", scope.label()), style),
                muted(theme, &format!("   {hint}")),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), chunks[1]);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            key(theme, " j/k"),
            muted(theme, " move  "),
            key(theme, "Enter"),
            muted(theme, " next → harness  "),
            key(theme, "Esc"),
            muted(theme, " back"),
        ])),
        chunks[2],
    );
}

fn draw_harness_picker(frame: &mut Frame, area: Rect, state: &PromptOverridesState, theme: &Theme) {
    let Some(edit) = &state.edit else { return };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Apply to which harness?",
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ))),
        chunks[0],
    );

    let mut labels: Vec<String> = vec!["Shared (all harnesses)".to_string()];
    labels.extend(AgentKind::ALL.iter().map(|h| h.display_name().to_string()));
    let rows: Vec<Line> = labels
        .iter()
        .enumerate()
        .map(|(i, label)| {
            let sel = i == edit.harness_index;
            let (marker, style) = if sel {
                (
                    "› ",
                    Style::default()
                        .fg(theme.primary.to_color())
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("  ", Style::default().fg(theme.text.to_color()))
            };
            Line::from(Span::styled(format!("{marker}{label}"), style))
        })
        .collect();
    frame.render_widget(Paragraph::new(rows), chunks[1]);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            key(theme, " j/k"),
            muted(theme, " move  "),
            key(theme, "Enter"),
            muted(theme, " save  "),
            key(theme, "Esc"),
            muted(theme, " back"),
        ])),
        chunks[2],
    );
}

fn draw_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    let lines = vec![
        Line::from(Span::styled(
            " Headless Prompt Overrides",
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(muted(
            theme,
            " Every headless AI call AMF makes runs one of these templates. Override any of",
        )),
        Line::from(muted(
            theme,
            " them at feature, project, or global scope; the nearest scope wins, then the",
        )),
        Line::from(muted(
            theme,
            " built-in default. Within a scope a per-harness template beats the shared one.",
        )),
        Line::raw(""),
        Line::from(vec![
            key(theme, " Enter / e"),
            muted(theme, "  edit the effective template"),
        ]),
        Line::from(vec![
            key(theme, " d, d"),
            muted(theme, "       clear the effective override"),
        ]),
        Line::from(vec![
            key(theme, " Ctrl+S"),
            muted(
                theme,
                "     in the editor: choose scope, then harness, then save",
            ),
        ]),
        Line::from(vec![
            key(theme, " Ctrl+T"),
            muted(theme, "     toggle Vim keys in the editor"),
        ]),
        Line::raw(""),
        Line::from(muted(
            theme,
            " {{token}} placeholders are re-filled with live context at run time. There is no",
        )),
        Line::from(muted(
            theme,
            " validation: a dropped or unknown token is saved and rendered verbatim.",
        )),
        Line::raw(""),
        Line::from(vec![
            key(theme, " Esc"),
            muted(theme, "        close this help"),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}
