use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{
    ConfigCategory, ConfigScope, ConfigWizardState, ConfigWizardStep, DiffViewerLayout,
};
use crate::project::{AgentKind, VibeMode};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;
use super::editor_view::{count_wrapped_editor_lines, editor_lines, sync_editor_scroll};

const FIELD_VALUE_PREVIEW_CHARS: usize = 80;
const FIELD_LABEL_WIDTH: usize = 15;

pub fn draw_config_wizard_dialog(frame: &mut Frame, state: &mut ConfigWizardState, theme: &Theme) {
    match state.step {
        ConfigWizardStep::CategoryPicker => draw_category_picker(frame, state, theme),
        ConfigWizardStep::ScopePicker => draw_scope_picker(frame, state, theme),
        ConfigWizardStep::ItemList => draw_item_list(frame, state, theme),
        ConfigWizardStep::EditItem => draw_edit_item(frame, state, theme),
        ConfigWizardStep::ConfirmSave => draw_confirm_save(frame, state, theme),
    }
}

fn draw_category_picker(frame: &mut Frame, state: &ConfigWizardState, theme: &Theme) {
    let area = centered_rect(50, 40, frame.area());
    let items = [
        "Custom Sessions",
        "Feature Presets",
        "Lifecycle Hooks",
        "Keybindings",
        "Allowed Harnesses",
    ];

    draw_modal(frame, area, " Configure ", theme, |frame, inner| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(items.len() as u16),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        let lines: Vec<_> = items
            .iter()
            .enumerate()
            .map(|(index, label)| selection_line(label, state.selected == index, theme))
            .collect();
        frame.render_widget(Paragraph::new(lines), chunks[0]);

        let description = Paragraph::new(category_description(state.selected))
            .style(Style::default().fg(theme.text_muted.to_color()))
            .wrap(Wrap { trim: false });
        frame.render_widget(description, chunks[1]);

        render_error(frame, chunks[2], state.error.as_deref(), theme);
        render_hints(
            frame,
            chunks[3],
            "Enter select  Esc cancel  Ctrl+Q close",
            theme,
        );
    });
}

fn draw_scope_picker(frame: &mut Frame, state: &ConfigWizardState, theme: &Theme) {
    let area = centered_rect(50, 30, frame.area());

    draw_modal(frame, area, " Config Scope ", theme, |frame, inner| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        let project_enabled = state.project_repo.is_some();
        let project_label = state
            .project_name
            .as_ref()
            .map(|name| format!("Project ({}/.amf/config.json)", name))
            .unwrap_or_else(|| "Project (select a project first)".to_string());
        let lines = vec![
            selection_line(
                "Global (~/.config/amf/config.json)",
                state.selected == 0,
                theme,
            ),
            selection_line_with_disabled(
                &project_label,
                state.selected == 1,
                project_enabled,
                theme,
            ),
        ];
        frame.render_widget(Paragraph::new(lines), chunks[0]);

        render_error(frame, chunks[1], state.error.as_deref(), theme);
        render_hints(
            frame,
            chunks[2],
            "Enter select  Esc back  Ctrl+Q close",
            theme,
        );
    });
}

fn draw_item_list(frame: &mut Frame, state: &ConfigWizardState, theme: &Theme) {
    let area = centered_rect(70, 60, frame.area());
    let rows = item_list_lines(state, theme);

    draw_modal(
        frame,
        area,
        &format!(" {} ", category_title(&state.category)),
        theme,
        |frame, inner| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(1),
                    Constraint::Length(1),
                    Constraint::Length(1),
                ])
                .split(inner);

            frame.render_widget(Paragraph::new(rows), chunks[0]);
            render_error(frame, chunks[1], state.error.as_deref(), theme);
            render_hints(frame, chunks[2], item_list_hints(&state.category), theme);
        },
    );
}

fn draw_edit_item(frame: &mut Frame, state: &mut ConfigWizardState, theme: &Theme) {
    let area = centered_rect(60, 70, frame.area());
    let title = match state.category {
        ConfigCategory::CustomSessions => {
            if state.editing_index.is_some() {
                " Edit Session "
            } else {
                " New Session "
            }
        }
        ConfigCategory::FeaturePresets => {
            if state.editing_index.is_some() {
                " Edit Preset "
            } else {
                " New Preset "
            }
        }
        ConfigCategory::LifecycleHooks => " Edit Hook ",
        ConfigCategory::Keybindings => {
            if state.editing_index.is_some() {
                " Edit Keybinding "
            } else {
                " New Keybinding "
            }
        }
        ConfigCategory::AllowedAgents => " Edit Config ",
    };

    draw_modal(frame, area, title, theme, |frame, inner| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(4),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        frame.render_widget(Paragraph::new(edit_lines(state, theme)), chunks[0]);
        frame.render_widget(
            Paragraph::new(edit_help_lines(state, theme)).wrap(Wrap { trim: false }),
            chunks[1],
        );
        render_error(frame, chunks[2], state.error.as_deref(), theme);
        render_hints(frame, chunks[3], edit_hints(state), theme);
    });

    if state.field_editor.is_some() {
        draw_field_editor(frame, state, theme);
    }
}

fn draw_confirm_save(frame: &mut Frame, state: &ConfigWizardState, theme: &Theme) {
    let area = centered_rect(70, 70, frame.area());
    let scope_label = match &state.scope {
        ConfigScope::Global => "~/.config/amf/config.json".to_string(),
        ConfigScope::Project(repo) => format!("{}/.amf/config.json", repo.display()),
    };

    draw_modal(frame, area, " Confirm Changes ", theme, |frame, inner| {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(inner);

        let scope = Paragraph::new(Line::from(vec![
            Span::styled(" Scope: ", Style::default().fg(theme.primary.to_color())),
            Span::styled(scope_label, Style::default().fg(theme.text.to_color())),
        ]));
        frame.render_widget(scope, chunks[0]);

        super::diff::draw_patch_panel(
            frame,
            chunks[1],
            state.confirm_diff.as_ref(),
            super::diff::PatchPanelOptions {
                layout: DiffViewerLayout::Unified,
                title: "Config Diff".to_string(),
                border_color: theme.primary.to_color(),
                scroll: state.preview_scroll,
                include_prologue: true,
                new_file_presentation: false,
            },
            theme,
        );

        render_error(frame, chunks[2], state.error.as_deref(), theme);
        render_hints(
            frame,
            chunks[3],
            "Enter save  Esc back  q cancel  Ctrl+Q close",
            theme,
        );
    });
}

fn draw_field_editor(frame: &mut Frame, state: &mut ConfigWizardState, theme: &Theme) {
    let Some(editor_state) = &mut state.field_editor else {
        return;
    };
    let area = centered_rect(64, 36, frame.area());
    let lines = editor_lines(
        &editor_state.editor,
        theme,
        "Type the field value. Enter saves, Alt+Enter inserts a newline.",
    );

    let wrap_width = area.width.saturating_sub(2).max(1) as usize;
    let total_visual_lines = count_wrapped_editor_lines(&lines, wrap_width);
    let visible_lines = area.height.saturating_sub(2).max(1) as usize;
    sync_editor_scroll(
        &editor_state.editor,
        &mut editor_state.scroll_offset,
        &mut editor_state.sync_scroll_to_cursor,
        visible_lines,
        wrap_width,
        total_visual_lines,
    );

    let hints = Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" save  "),
        Span::styled("Alt+Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" newline  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel "),
    ]);
    let block = Block::default()
        .title(format!(" Edit {} ", editor_state.label))
        .title_bottom(hints)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((editor_state.scroll_offset.min(u16::MAX as usize) as u16, 0)),
        area,
    );
}

fn draw_modal(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    title: &str,
    theme: &Theme,
    draw_inner: impl FnOnce(&mut Frame, ratatui::layout::Rect),
) {
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    draw_inner(frame, inner);
}

fn selection_line(label: &str, selected: bool, theme: &Theme) -> Line<'static> {
    let style = if selected {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    Line::from(Span::styled(
        format!(" {} {}", if selected { ">" } else { " " }, label),
        style,
    ))
}

fn selection_line_with_disabled(
    label: &str,
    selected: bool,
    enabled: bool,
    theme: &Theme,
) -> Line<'static> {
    let style = if !enabled {
        Style::default().fg(theme.text_muted.to_color())
    } else if selected {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    Line::from(Span::styled(
        format!(" {} {}", if selected { ">" } else { " " }, label),
        style,
    ))
}

fn render_error(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    error: Option<&str>,
    theme: &Theme,
) {
    let line = error.unwrap_or("");
    frame.render_widget(
        Paragraph::new(line).style(Style::default().fg(theme.danger.to_color())),
        area,
    );
}

fn render_hints(frame: &mut Frame, area: ratatui::layout::Rect, text: &str, theme: &Theme) {
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(theme.warning.to_color())),
        area,
    );
}

fn category_description(selected: usize) -> &'static str {
    match selected {
        1 => {
            "Define reusable feature presets for branch creation, mode, harness, and review behavior."
        }
        2 => "Configure lifecycle scripts that run when worktrees start, stop, or get created.",
        3 => "Override dashboard action keys without editing JSON by hand.",
        4 => "Limit which harnesses are allowed for the selected scope.",
        _ => "Add reusable custom tmux sessions that appear in the session picker for a feature.",
    }
}

fn category_title(category: &ConfigCategory) -> &'static str {
    match category {
        ConfigCategory::CustomSessions => "Custom Sessions",
        ConfigCategory::FeaturePresets => "Feature Presets",
        ConfigCategory::LifecycleHooks => "Lifecycle Hooks",
        ConfigCategory::Keybindings => "Keybindings",
        ConfigCategory::AllowedAgents => "Allowed Harnesses",
    }
}

fn item_list_lines(state: &ConfigWizardState, theme: &Theme) -> Vec<Line<'static>> {
    match state.category {
        ConfigCategory::CustomSessions => {
            if state.sessions.is_empty() {
                vec![Line::from(Span::styled(
                    " (empty - press 'a' to add)",
                    Style::default().fg(theme.text_muted.to_color()),
                ))]
            } else {
                state
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(index, session)| {
                        selection_line(&session.name, state.selected == index, theme)
                    })
                    .collect()
            }
        }
        ConfigCategory::FeaturePresets => {
            if state.presets.is_empty() {
                vec![Line::from(Span::styled(
                    " (empty - press 'a' to add)",
                    Style::default().fg(theme.text_muted.to_color()),
                ))]
            } else {
                state
                    .presets
                    .iter()
                    .enumerate()
                    .map(|(index, preset)| {
                        selection_line(&preset.name, state.selected == index, theme)
                    })
                    .collect()
            }
        }
        ConfigCategory::LifecycleHooks => vec![
            selection_line(
                &format!("on_start: {}", hook_summary(state.hooks.on_start.as_ref())),
                state.selected == 0,
                theme,
            ),
            selection_line(
                &format!("on_stop: {}", hook_summary(state.hooks.on_stop.as_ref())),
                state.selected == 1,
                theme,
            ),
            selection_line(
                &format!(
                    "on_worktree_created: {}",
                    hook_summary(state.hooks.on_worktree_created.as_ref())
                ),
                state.selected == 2,
                theme,
            ),
        ],
        ConfigCategory::Keybindings => state
            .keybinding_actions
            .iter()
            .enumerate()
            .map(|(index, action)| {
                let configured = state.keybindings.get(action).copied();
                let key = configured
                    .or_else(|| crate::handlers::default_key_for_action(action))
                    .map(|key| key.to_string())
                    .unwrap_or_else(|| "?".into());
                let source = if configured.is_some() {
                    "override"
                } else {
                    "default"
                };
                selection_line(
                    &format!("{action:<18} {key:<3} {source}"),
                    state.selected == index,
                    theme,
                )
            })
            .collect(),
        ConfigCategory::AllowedAgents => AgentKind::ALL
            .iter()
            .enumerate()
            .map(|(index, agent)| {
                let enabled = state.agent_toggles.get(index).copied().unwrap_or(true);
                selection_line(
                    &format!(
                        "[{}] {}",
                        if enabled { "x" } else { " " },
                        agent.display_name()
                    ),
                    state.selected == index,
                    theme,
                )
            })
            .collect(),
    }
}

fn item_list_hints(category: &ConfigCategory) -> &'static str {
    match category {
        ConfigCategory::AllowedAgents => "Space toggle  Ctrl+S save  Esc back  Ctrl+Q close",
        ConfigCategory::Keybindings => {
            "Enter/e edit key  d reset override  Ctrl+S save  Esc back  Ctrl+Q close"
        }
        _ => "a add  e edit  d delete  Ctrl+S save  Esc back  Ctrl+Q close",
    }
}

fn edit_hints(state: &ConfigWizardState) -> &'static str {
    if state.capturing_key {
        "Press a key to bind  Backspace clear  Enter/Esc cancel  Ctrl+Q close"
    } else if state.input_mode {
        "Type text  Enter/Esc stop editing  Tab next field  Ctrl+Q close"
    } else {
        "j/k move  Tab next  Shift+Tab prev  Enter edit/toggle/save  Esc back  Ctrl+Q close"
    }
}

fn edit_help_lines(state: &ConfigWizardState, theme: &Theme) -> Vec<Line<'static>> {
    let (title, detail, example) = match state.category {
        ConfigCategory::CustomSessions => custom_session_field_help(state.field_focus),
        ConfigCategory::FeaturePresets => feature_preset_field_help(state.field_focus),
        ConfigCategory::LifecycleHooks => lifecycle_hook_field_help(state.field_focus),
        ConfigCategory::Keybindings => keybinding_field_help(state.field_focus),
        ConfigCategory::AllowedAgents => (
            "Allowed harnesses",
            "Choose which harness backends are permitted for this scope.",
            "All selected means no restriction is written to config.",
        ),
    };

    vec![
        Line::from(vec![
            Span::styled(" Field: ", Style::default().fg(theme.primary.to_color())),
            Span::styled(
                title.to_string(),
                Style::default().fg(theme.text.to_color()),
            ),
        ]),
        Line::from(Span::styled(
            detail.to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            example.to_string(),
            Style::default().fg(theme.info.to_color()),
        )),
    ]
}

fn custom_session_field_help(field_focus: usize) -> (&'static str, &'static str, &'static str) {
    match field_focus {
        0 => (
            "Name",
            "Display name shown in the session picker.",
            "Example: API Server",
        ),
        1 => (
            "Description",
            "Optional short description shown alongside the session entry.",
            "Example: Runs the local Rust API in watch mode",
        ),
        2 => (
            "Command",
            "Shell command started inside the tmux window with `bash -c`.",
            "Example: cargo watch -x run",
        ),
        3 => (
            "Window name",
            "Optional tmux window name. Leave blank to derive one from the session name.",
            "Example: api",
        ),
        4 => (
            "Working dir",
            "Optional path relative to the feature worktree. Leave blank to use the feature root.",
            "Example: backend or apps/web",
        ),
        5 => (
            "Icon",
            "Optional short plain-text icon used when nerd fonts are off.",
            "Example: A",
        ),
        6 => (
            "Nerd icon",
            "Optional nerd-font glyph used when nerd fonts are enabled.",
            "Example: nf-md-server",
        ),
        7 => (
            "On stop",
            "Optional shell command run with `bash -c` before the custom session is stopped.",
            "Example: docker compose stop api",
        ),
        8 => (
            "Pre-check",
            "Optional shell command run with `bash -c` in the feature worktree before launch. It must exit 0 or the session will not start.",
            "Example: command -v cargo >/dev/null && test -f Cargo.toml",
        ),
        9 => (
            "Autolaunch",
            "If enabled, the session is added and launched immediately when possible.",
            "Use Enter to toggle this checkbox.",
        ),
        10 => (
            "Save",
            "Validate this custom session and open the config diff preview.",
            "Press Enter on Save to continue.",
        ),
        _ => (
            "Custom session",
            "Configure how this extra tmux window should be created and launched.",
            "Fields left blank fall back to AMF defaults where supported.",
        ),
    }
}

fn feature_preset_field_help(field_focus: usize) -> (&'static str, &'static str, &'static str) {
    match field_focus {
        0 => (
            "Name",
            "Preset name shown in feature creation.",
            "Example: Quick Fix",
        ),
        1 => (
            "Branch prefix",
            "Optional branch prefix prefilled into the branch name box.",
            "Example: fix/",
        ),
        2 => (
            "Mode",
            "Default vibe mode applied when this preset is chosen.",
            "Use Enter to cycle values.",
        ),
        3 => (
            "Harness",
            "Default harness backend for new features created from this preset.",
            "Use Enter to cycle values.",
        ),
        4 => (
            "Review",
            "Start the feature with review logging enabled.",
            "Use Enter to toggle.",
        ),
        5 => (
            "Plan mode",
            "Enable plan mode for the feature by default.",
            "Use Enter to toggle.",
        ),
        6 => (
            "Chrome",
            "Pass the chrome flag for supported harnesses.",
            "Use Enter to toggle.",
        ),
        7 => (
            "Remote control",
            "Enable remote-control support for features created from this preset.",
            "Use Enter to toggle.",
        ),
        8 => (
            "Save",
            "Validate this preset and open the config diff preview.",
            "Press Enter on Save to continue.",
        ),
        _ => ("Preset", "Reusable defaults for feature creation.", ""),
    }
}

fn lifecycle_hook_field_help(field_focus: usize) -> (&'static str, &'static str, &'static str) {
    match field_focus {
        0 => (
            "Script",
            "Shell script or command to run for this lifecycle hook.",
            "Example: scripts/on-start.sh",
        ),
        1 => (
            "Prompted",
            "If enabled, AMF will ask the user to choose an option before running the hook.",
            "Use Enter to toggle.",
        ),
        2 => (
            "Prompt title",
            "Title shown in the prompt dialog when Prompted is enabled.",
            "Example: Choose environment",
        ),
        3 => (
            "Prompt options",
            "Comma-separated list of options for the hook prompt.",
            "Example: dev, staging, prod",
        ),
        4 => (
            "Save",
            "Validate this hook and open the config diff preview.",
            "Press Enter on Save to continue.",
        ),
        _ => ("Hook", "Configure a lifecycle command.", ""),
    }
}

fn keybinding_field_help(field_focus: usize) -> (&'static str, &'static str, &'static str) {
    match field_focus {
        0 => (
            "Action",
            "Dashboard action to remap.",
            "Use Enter or left/right to cycle known actions.",
        ),
        1 => (
            "Key",
            "Single character used to trigger that action.",
            "Example: r",
        ),
        2 => (
            "Save",
            "Validate this keybinding and open the config diff preview.",
            "Press Enter on Save to continue.",
        ),
        _ => ("Keybinding", "Map an action to a new key.", ""),
    }
}

fn edit_lines(state: &ConfigWizardState, theme: &Theme) -> Vec<Line<'static>> {
    match state.category {
        ConfigCategory::CustomSessions => {
            let labels = [
                "Name",
                "Description",
                "Command",
                "Window name",
                "Working dir",
                "Icon",
                "Nerd icon",
                "On stop",
                "Pre-check",
            ];
            let mut lines = Vec::new();
            for (index, label) in labels.iter().enumerate() {
                lines.push(text_field_line(
                    label,
                    state
                        .field_values
                        .get(index)
                        .map(String::as_str)
                        .unwrap_or(""),
                    state.field_focus == index,
                    state.input_mode && state.field_focus == index,
                    theme,
                ));
            }
            lines.push(toggle_field_line(
                "Autolaunch",
                state.field_toggles.first().copied().unwrap_or(false),
                state.field_focus == 9,
                theme,
            ));
            lines.push(button_line("Save", state.field_focus == 10, theme));
            lines
        }
        ConfigCategory::FeaturePresets => {
            let mut lines = vec![
                text_field_line(
                    "Name",
                    state.field_values.first().map(String::as_str).unwrap_or(""),
                    state.field_focus == 0,
                    state.input_mode && state.field_focus == 0,
                    theme,
                ),
                text_field_line(
                    "Branch prefix",
                    state.field_values.get(1).map(String::as_str).unwrap_or(""),
                    state.field_focus == 1,
                    state.input_mode && state.field_focus == 1,
                    theme,
                ),
                enum_field_line(
                    "Mode",
                    &VibeMode::ALL
                        .iter()
                        .map(|mode| mode.display_name().to_ascii_lowercase())
                        .collect::<Vec<_>>(),
                    state
                        .field_values
                        .get(2)
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0),
                    state.field_focus == 2,
                    theme,
                ),
                enum_field_line(
                    "Harness",
                    &AgentKind::ALL
                        .iter()
                        .map(|agent| agent.display_name().to_ascii_lowercase())
                        .collect::<Vec<_>>(),
                    state
                        .field_values
                        .get(3)
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(0),
                    state.field_focus == 3,
                    theme,
                ),
                toggle_field_line(
                    "Review",
                    state.field_toggles.first().copied().unwrap_or(false),
                    state.field_focus == 4,
                    theme,
                ),
                toggle_field_line(
                    "Plan mode",
                    state.field_toggles.get(1).copied().unwrap_or(false),
                    state.field_focus == 5,
                    theme,
                ),
                toggle_field_line(
                    "Chrome",
                    state.field_toggles.get(2).copied().unwrap_or(false),
                    state.field_focus == 6,
                    theme,
                ),
                toggle_field_line(
                    "Remote control",
                    state.field_toggles.get(3).copied().unwrap_or(false),
                    state.field_focus == 7,
                    theme,
                ),
            ];
            lines.push(button_line("Save", state.field_focus == 8, theme));
            lines
        }
        ConfigCategory::LifecycleHooks => {
            let mut lines = vec![
                text_field_line(
                    "Script",
                    state.field_values.first().map(String::as_str).unwrap_or(""),
                    state.field_focus == 0,
                    state.input_mode && state.field_focus == 0,
                    theme,
                ),
                toggle_field_line(
                    "Prompted",
                    state.field_toggles.first().copied().unwrap_or(false),
                    state.field_focus == 1,
                    theme,
                ),
                text_field_line(
                    "Prompt title",
                    state.field_values.get(1).map(String::as_str).unwrap_or(""),
                    state.field_focus == 2,
                    state.input_mode && state.field_focus == 2,
                    theme,
                ),
                text_field_line(
                    "Prompt options",
                    state.field_values.get(2).map(String::as_str).unwrap_or(""),
                    state.field_focus == 3,
                    state.input_mode && state.field_focus == 3,
                    theme,
                ),
            ];
            lines.push(button_line("Save", state.field_focus == 4, theme));
            lines
        }
        ConfigCategory::Keybindings => {
            let selected_action = state
                .keybinding_actions
                .iter()
                .position(|action| state.field_values.first() == Some(action))
                .unwrap_or(0);
            let mut lines = vec![
                select_field_line(
                    "Action",
                    state
                        .keybinding_actions
                        .get(selected_action)
                        .map(String::as_str)
                        .unwrap_or(""),
                    state.field_focus == 0,
                    theme,
                ),
                text_field_line(
                    "Key",
                    state.field_values.get(1).map(String::as_str).unwrap_or(""),
                    state.field_focus == 1,
                    state.input_mode && state.field_focus == 1,
                    theme,
                ),
            ];
            lines.push(button_line("Save", state.field_focus == 2, theme));
            lines
        }
        ConfigCategory::AllowedAgents => Vec::new(),
    }
}

fn text_field_line(
    label: &str,
    value: &str,
    focused: bool,
    editing: bool,
    theme: &Theme,
) -> Line<'static> {
    let marker = if focused { ">" } else { " " };
    let label_style = if focused {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::BOLD)
    };
    let value_preview = truncate_field_preview(value, FIELD_VALUE_PREVIEW_CHARS);
    let value_style = if value_preview.is_empty() {
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::ITALIC)
    } else {
        Style::default().fg(theme.status_detail.to_color())
    };
    let mut spans = vec![
        Span::styled(format!("{marker} {label:<FIELD_LABEL_WIDTH$}"), label_style),
        Span::styled(" | ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(
            if value_preview.is_empty() {
                "empty".to_string()
            } else {
                value_preview
            },
            value_style,
        ),
    ];
    if editing {
        spans.push(Span::styled(
            "\u{2588}",
            Style::default().fg(theme.primary.to_color()),
        ));
    }
    Line::from(spans)
}

fn truncate_field_preview(value: &str, max_chars: usize) -> String {
    let mut normalized = value.replace('\n', " ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    if max_chars <= 3 {
        return "...".chars().take(max_chars).collect();
    }
    normalized = normalized
        .chars()
        .take(max_chars.saturating_sub(3))
        .collect();
    normalized.push_str("...");
    normalized
}

fn toggle_field_line(label: &str, value: bool, active: bool, theme: &Theme) -> Line<'static> {
    let marker = if active { ">" } else { " " };
    let label_style = if active {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::BOLD)
    };
    Line::from(vec![
        Span::styled(format!("{marker} {label:<FIELD_LABEL_WIDTH$}"), label_style),
        Span::styled(" | ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(
            format!("[{}]", if value { "x" } else { " " }),
            Style::default().fg(theme.status_detail.to_color()),
        ),
    ])
}

fn select_field_line(label: &str, value: &str, active: bool, theme: &Theme) -> Line<'static> {
    let marker = if active { ">" } else { " " };
    let label_style = if active {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::BOLD)
    };
    let value_style = if active {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.status_detail.to_color())
    };
    Line::from(vec![
        Span::styled(format!("{marker} {label:<FIELD_LABEL_WIDTH$}"), label_style),
        Span::styled(" | ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("< ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(value.to_string(), value_style),
        Span::styled(" >", Style::default().fg(theme.text_muted.to_color())),
    ])
}

#[cfg(test)]
mod tests {
    use super::truncate_field_preview;

    #[test]
    fn field_preview_replaces_newlines() {
        assert_eq!(truncate_field_preview("one\ntwo", 20), "one two");
    }

    #[test]
    fn field_preview_uses_ascii_ellipsis_when_truncated() {
        assert_eq!(truncate_field_preview("abcdefghij", 8), "abcde...");
    }
}

fn button_line(label: &str, active: bool, theme: &Theme) -> Line<'static> {
    let style = if active {
        Style::default()
            .fg(theme.shortcut_text.to_color())
            .bg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let marker = if active { ">" } else { " " };
    Line::from(vec![
        Span::styled(
            format!("{marker} {:<FIELD_LABEL_WIDTH$}", ""),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("   ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(format!(" {label} "), style),
    ])
}

fn enum_field_line(
    label: &str,
    values: &[String],
    selected: usize,
    active: bool,
    theme: &Theme,
) -> Line<'static> {
    let marker = if active { ">" } else { " " };
    let label_style = if active {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::BOLD)
    };
    let mut spans = vec![
        Span::styled(format!("{marker} {label:<FIELD_LABEL_WIDTH$}"), label_style),
        Span::styled(" | ", Style::default().fg(theme.text_muted.to_color())),
    ];
    for (index, value) in values.iter().enumerate() {
        let style = if selected == index {
            if active {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.status_detail.to_color())
            }
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        let prefix = if selected == index { "> " } else { "  " };
        spans.push(Span::styled(format!("{prefix}{value} "), style));
    }
    Line::from(spans)
}

fn hook_summary(hook: Option<&crate::extension::HookConfig>) -> String {
    match hook {
        Some(crate::extension::HookConfig::Script(script)) => script.clone(),
        Some(crate::extension::HookConfig::WithPrompt { script, .. }) => {
            format!("{script} (prompt)")
        }
        None => "(not set)".into(),
    }
}
