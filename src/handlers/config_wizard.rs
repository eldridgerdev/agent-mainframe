use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{
    App, AppMode, ConfigCategory, ConfigScope, ConfigWizardFieldEditor, ConfigWizardState,
    ConfigWizardStep, agent_toggles_to_allowed,
};
use crate::editor::TextEditor;
use crate::project::AgentKind;

pub fn handle_config_wizard_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
    {
        app.mode = AppMode::Normal;
        return Ok(());
    }

    let step = match &app.mode {
        AppMode::ConfigWizard(state) => state.step.clone(),
        _ => return Ok(()),
    };

    match step {
        ConfigWizardStep::CategoryPicker => handle_category_picker(app, key.code),
        ConfigWizardStep::ScopePicker => handle_scope_picker(app, key.code)?,
        ConfigWizardStep::ItemList => handle_item_list(app, key)?,
        ConfigWizardStep::EditItem => handle_edit_item(app, key),
        ConfigWizardStep::ConfirmSave => handle_confirm_save(app, key)?,
    }

    Ok(())
}

fn handle_category_picker(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            with_state(app, |state| {
                state.error = None;
                state.selected = (state.selected + 1) % category_count();
            });
        }
        KeyCode::Up | KeyCode::Char('k') => {
            with_state(app, |state| {
                state.error = None;
                state.selected = if state.selected == 0 {
                    category_count() - 1
                } else {
                    state.selected - 1
                };
            });
        }
        KeyCode::Enter => {
            with_state(app, |state| {
                state.category = category_from_index(state.selected);
                state.error = None;
                state.step = ConfigWizardStep::ScopePicker;
                state.selected = match state.scope {
                    ConfigScope::Global => 0,
                    ConfigScope::Project(_) => 1,
                };
            });
        }
        _ => {}
    }
}

fn handle_scope_picker(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            with_state(app, |state| {
                state.error = None;
                state.step = ConfigWizardStep::CategoryPicker;
                state.selected = category_index(&state.category);
            });
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Up | KeyCode::Char('k') => {
            with_state(app, |state| {
                state.error = None;
                state.selected = if state.selected == 0 { 1 } else { 0 };
            });
        }
        KeyCode::Enter => {
            let should_open_project = matches!(
                &app.mode,
                AppMode::ConfigWizard(state) if state.selected == 1 && state.project_repo.is_none()
            );
            if should_open_project {
                with_state(app, |state| {
                    state.error =
                        Some("Select a project or feature first to edit project config".into());
                });
                return Ok(());
            }

            with_state(app, |state| {
                state.scope = if state.selected == 0 {
                    ConfigScope::Global
                } else {
                    ConfigScope::Project(state.project_repo.clone().unwrap_or_default())
                };
            });
            app.config_wizard_select_scope()?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_item_list(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S'))
    {
        app.config_wizard_prepare_confirm()?;
        return Ok(());
    }

    let category = match &app.mode {
        AppMode::ConfigWizard(state) => state.category.clone(),
        _ => return Ok(()),
    };

    match category {
        ConfigCategory::CustomSessions => handle_sessions_list(app, key.code),
        ConfigCategory::FeaturePresets => handle_presets_list(app, key.code),
        ConfigCategory::LifecycleHooks => handle_hooks_list(app, key.code),
        ConfigCategory::Keybindings => handle_keybindings_list(app, key.code),
        ConfigCategory::AllowedAgents => handle_agents_list(app, key.code),
    }

    Ok(())
}

fn handle_sessions_list(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => back_to_scope_picker(app),
        KeyCode::Down | KeyCode::Char('j') => {
            move_list_selection(app, |state| state.sessions.len(), 1)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_list_selection(app, |state| state.sessions.len(), -1)
        }
        KeyCode::Char('a') => app.config_wizard_start_edit(None),
        KeyCode::Enter | KeyCode::Char('e') => {
            let index = selected_index_if_nonempty(app, |state| state.sessions.len());
            if let Some(index) = index {
                app.config_wizard_start_edit(Some(index));
            }
        }
        KeyCode::Char('d') => {
            with_state(app, |state| {
                if state.selected < state.sessions.len() {
                    state.sessions.remove(state.selected);
                    state.selected = adjust_selection(state.selected, state.sessions.len());
                }
            });
        }
        _ => {}
    }
}

fn handle_presets_list(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => back_to_scope_picker(app),
        KeyCode::Down | KeyCode::Char('j') => {
            move_list_selection(app, |state| state.presets.len(), 1)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_list_selection(app, |state| state.presets.len(), -1)
        }
        KeyCode::Char('a') => app.config_wizard_start_edit(None),
        KeyCode::Enter | KeyCode::Char('e') => {
            let index = selected_index_if_nonempty(app, |state| state.presets.len());
            if let Some(index) = index {
                app.config_wizard_start_edit(Some(index));
            }
        }
        KeyCode::Char('d') => {
            with_state(app, |state| {
                if state.selected < state.presets.len() {
                    state.presets.remove(state.selected);
                    state.selected = adjust_selection(state.selected, state.presets.len());
                }
            });
        }
        _ => {}
    }
}

fn handle_hooks_list(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => back_to_scope_picker(app),
        KeyCode::Down | KeyCode::Char('j') => move_list_selection(app, |_| 3, 1),
        KeyCode::Up | KeyCode::Char('k') => move_list_selection(app, |_| 3, -1),
        KeyCode::Enter | KeyCode::Char('e') => {
            let selected = current_selected(app);
            app.config_wizard_start_edit(Some(selected));
        }
        KeyCode::Char('d') => {
            with_state(app, |state| {
                match state.selected {
                    0 => state.hooks.on_start = None,
                    1 => state.hooks.on_stop = None,
                    _ => state.hooks.on_worktree_created = None,
                }
                state.error = None;
            });
        }
        _ => {}
    }
}

fn handle_keybindings_list(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => back_to_scope_picker(app),
        KeyCode::Down | KeyCode::Char('j') => {
            move_list_selection(app, |state| state.keybinding_actions.len(), 1)
        }
        KeyCode::Up | KeyCode::Char('k') => {
            move_list_selection(app, |state| state.keybinding_actions.len(), -1)
        }
        KeyCode::Enter | KeyCode::Char('e') => {
            let index = selected_index_if_nonempty(app, |state| state.keybinding_actions.len());
            if let Some(index) = index {
                app.config_wizard_start_edit(Some(index));
            }
        }
        KeyCode::Char('d') => {
            with_state(app, |state| {
                if let Some(action) = state.keybinding_actions.get(state.selected).cloned() {
                    state.keybindings.remove(&action);
                    state.keybinding_actions.sort();
                    state.selected =
                        adjust_selection(state.selected, state.keybinding_actions.len());
                    state.error = None;
                }
            });
        }
        _ => {}
    }
}

fn handle_agents_list(app: &mut App, key: KeyCode) {
    match key {
        KeyCode::Esc => back_to_scope_picker(app),
        KeyCode::Down | KeyCode::Char('j') => move_list_selection(app, |_| AgentKind::ALL.len(), 1),
        KeyCode::Up | KeyCode::Char('k') => move_list_selection(app, |_| AgentKind::ALL.len(), -1),
        KeyCode::Enter | KeyCode::Char(' ') => {
            with_state(app, |state| {
                let selected = state.selected;
                if let Some(toggle) = state.agent_toggles.get_mut(selected) {
                    *toggle = !*toggle;
                }
                if !state.agent_toggles.iter().any(|enabled| *enabled) {
                    if let Some(toggle) = state.agent_toggles.get_mut(selected) {
                        *toggle = true;
                    }
                    state.error = Some("Select at least one harness".into());
                    return;
                }
                state.agent_toggles_dirty = true;
                state.allowed_agents = agent_toggles_to_allowed(&state.scope, &state.agent_toggles);
                state.error = None;
            });
        }
        _ => {}
    }
}

fn handle_edit_item(app: &mut App, key: KeyEvent) {
    if handle_active_field_editor(app, key) {
        return;
    }
    if handle_keybinding_key_capture(app, key) {
        return;
    }

    let category = match &app.mode {
        AppMode::ConfigWizard(state) => state.category.clone(),
        _ => return,
    };
    let save_focus = save_button_focus(&category);

    match key.code {
        KeyCode::Esc => {
            with_state(app, |state| {
                state.input_mode = false;
                state.capturing_key = false;
                state.field_editor = None;
                state.error = None;
                state.step = ConfigWizardStep::ItemList;
                state.field_values.clear();
                state.field_toggles.clear();
                state.field_focus = 0;
            });
        }
        KeyCode::Tab => {
            with_state(app, |state| {
                state.error = None;
                state.input_mode = false;
                state.capturing_key = false;
                state.field_editor = None;
                state.field_focus = (state.field_focus + 1) % edit_focus_count(&category);
            });
        }
        KeyCode::BackTab => {
            with_state(app, |state| {
                state.error = None;
                state.input_mode = false;
                state.capturing_key = false;
                state.field_editor = None;
                let count = edit_focus_count(&category);
                state.field_focus = if state.field_focus == 0 {
                    count - 1
                } else {
                    state.field_focus - 1
                };
            });
        }
        KeyCode::Enter => {
            let mut should_prepare_confirm = false;
            with_state(app, |state| {
                state.error = None;
                if state.field_focus == save_focus {
                    should_prepare_confirm = true;
                } else if !activate_current_field(state) {
                    if state.category == ConfigCategory::Keybindings && state.field_focus == 1 {
                        state.input_mode = true;
                        state.capturing_key = true;
                        state.error = None;
                    } else {
                        open_field_editor(state);
                    }
                }
            });
            if should_prepare_confirm && app.config_wizard_finish_edit() {
                let _ = app.config_wizard_prepare_confirm();
            }
        }
        KeyCode::Char(' ') => {
            with_state(app, |state| {
                let _ = handle_edit_space(state);
            });
        }
        KeyCode::Down | KeyCode::Char('j') => {
            with_state(app, |state| {
                move_focus_for_arrow(state, 1);
            });
        }
        KeyCode::Up | KeyCode::Char('k') => {
            with_state(app, |state| {
                move_focus_for_arrow(state, -1);
            });
        }
        KeyCode::Left | KeyCode::Char('h') => {
            with_state(app, |state| {
                if !cycle_enum_field(state, -1) {
                    move_focus_for_arrow(state, -1);
                }
            });
        }
        KeyCode::Right | KeyCode::Char('l') => {
            with_state(app, |state| {
                if !cycle_enum_field(state, 1) {
                    move_focus_for_arrow(state, 1);
                }
            });
        }
        KeyCode::Char('i') => {
            with_state(app, |state| {
                if field_accepts_text_input(state) {
                    open_field_editor(state);
                }
            });
        }
        _ => {}
    }
}

fn handle_keybinding_key_capture(app: &mut App, key: KeyEvent) -> bool {
    let capturing = matches!(
        &app.mode,
        AppMode::ConfigWizard(state)
            if state.input_mode
                && state.category == ConfigCategory::Keybindings
                && state.field_focus == 1
    );
    if !capturing {
        return false;
    }

    with_state(app, |state| match key.code {
        KeyCode::Esc | KeyCode::Enter => {
            state.input_mode = false;
            state.capturing_key = false;
            state.error = None;
        }
        KeyCode::Backspace | KeyCode::Delete => {
            if let Some(value) = state.field_values.get_mut(1) {
                value.clear();
            }
            state.error = None;
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            if let Some(value) = state.field_values.get_mut(1) {
                *value = c.to_string();
            }
            state.input_mode = false;
            state.capturing_key = false;
            state.error = None;
        }
        _ => {}
    });
    true
}

fn handle_active_field_editor(app: &mut App, key: KeyEvent) -> bool {
    let has_editor = matches!(
        &app.mode,
        AppMode::ConfigWizard(state) if state.field_editor.is_some()
    );
    if !has_editor {
        return false;
    }

    with_state(app, |state| match key.code {
        KeyCode::Esc => {
            state.field_editor = None;
            state.input_mode = false;
            state.capturing_key = false;
            state.error = None;
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            if let Some(editor) = &mut state.field_editor {
                editor.editor.insert_str("\n");
                editor.sync_scroll_to_cursor = true;
            }
        }
        KeyCode::Enter => {
            commit_field_editor(state);
        }
        _ => {
            if let Some(editor) = &mut state.field_editor {
                let outcome = editor.editor.handle_key(key);
                if outcome.text_changed || outcome.cursor_moved {
                    editor.sync_scroll_to_cursor = true;
                }
            }
        }
    });
    true
}

fn handle_confirm_save(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Enter => app.config_wizard_save()?,
        KeyCode::Esc => {
            with_state(app, |state| {
                state.error = None;
                state.step = ConfigWizardStep::ItemList;
            });
        }
        KeyCode::Char('q') => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            with_state(app, |state| {
                state.preview_scroll = state.preview_scroll.saturating_add(1);
            });
        }
        KeyCode::Up | KeyCode::Char('k') => {
            with_state(app, |state| {
                state.preview_scroll = state.preview_scroll.saturating_sub(1);
            });
        }
        _ => {}
    }

    Ok(())
}

fn with_state(app: &mut App, f: impl FnOnce(&mut ConfigWizardState)) {
    if let AppMode::ConfigWizard(state) = &mut app.mode {
        f(state);
    }
}

fn category_count() -> usize {
    5
}

fn category_from_index(index: usize) -> ConfigCategory {
    match index {
        1 => ConfigCategory::FeaturePresets,
        2 => ConfigCategory::LifecycleHooks,
        3 => ConfigCategory::Keybindings,
        4 => ConfigCategory::AllowedAgents,
        _ => ConfigCategory::CustomSessions,
    }
}

fn category_index(category: &ConfigCategory) -> usize {
    match category {
        ConfigCategory::CustomSessions => 0,
        ConfigCategory::FeaturePresets => 1,
        ConfigCategory::LifecycleHooks => 2,
        ConfigCategory::Keybindings => 3,
        ConfigCategory::AllowedAgents => 4,
    }
}

fn back_to_scope_picker(app: &mut App) {
    with_state(app, |state| {
        state.error = None;
        state.step = ConfigWizardStep::ScopePicker;
        state.selected = match state.scope {
            ConfigScope::Global => 0,
            ConfigScope::Project(_) => 1,
        };
    });
}

fn current_selected(app: &App) -> usize {
    match &app.mode {
        AppMode::ConfigWizard(state) => state.selected,
        _ => 0,
    }
}

fn selected_index_if_nonempty(
    app: &App,
    len: impl FnOnce(&ConfigWizardState) -> usize,
) -> Option<usize> {
    match &app.mode {
        AppMode::ConfigWizard(state) if len(state) > 0 => Some(state.selected),
        _ => None,
    }
}

fn move_list_selection(app: &mut App, len: impl FnOnce(&ConfigWizardState) -> usize, delta: isize) {
    with_state(app, |state| {
        let len = len(state);
        if len == 0 {
            state.selected = 0;
            return;
        }
        state.error = None;
        if delta > 0 {
            state.selected = (state.selected + 1) % len;
        } else {
            state.selected = if state.selected == 0 {
                len - 1
            } else {
                state.selected - 1
            };
        }
    });
}

fn adjust_selection(selected: usize, len: usize) -> usize {
    match len {
        0 => 0,
        _ => selected.min(len - 1),
    }
}

fn edit_field_count(category: &ConfigCategory) -> usize {
    match category {
        ConfigCategory::CustomSessions => 10,
        ConfigCategory::FeaturePresets => 8,
        ConfigCategory::LifecycleHooks => 4,
        ConfigCategory::Keybindings => 2,
        ConfigCategory::AllowedAgents => 0,
    }
}

fn edit_focus_count(category: &ConfigCategory) -> usize {
    edit_field_count(category) + 1
}

fn save_button_focus(category: &ConfigCategory) -> usize {
    edit_field_count(category)
}

fn activate_current_field(state: &mut ConfigWizardState) -> bool {
    if handle_edit_space(state) {
        return true;
    }

    cycle_enum_field(state, 1)
}

fn field_accepts_text_input(state: &ConfigWizardState) -> bool {
    field_value_index_for_focus(&state.category, state.field_focus).is_some()
}

fn open_field_editor(state: &mut ConfigWizardState) {
    let Some(field_index) = field_value_index_for_focus(&state.category, state.field_focus) else {
        return;
    };
    let value = state
        .field_values
        .get(field_index)
        .cloned()
        .unwrap_or_default();
    state.input_mode = true;
    state.capturing_key = false;
    state.error = None;
    state.field_editor = Some(ConfigWizardFieldEditor {
        field_index,
        label: field_label_for_focus(&state.category, state.field_focus).to_string(),
        editor: TextEditor::new(value),
        scroll_offset: 0,
        sync_scroll_to_cursor: true,
    });
}

fn commit_field_editor(state: &mut ConfigWizardState) {
    let Some(editor) = state.field_editor.take() else {
        return;
    };
    if let Some(value) = state.field_values.get_mut(editor.field_index) {
        *value = editor.editor.text().to_string();
    }
    state.input_mode = false;
    state.capturing_key = false;
    state.error = None;
}

fn field_value_index_for_focus(category: &ConfigCategory, field_focus: usize) -> Option<usize> {
    match category {
        ConfigCategory::CustomSessions if field_focus < 9 => Some(field_focus),
        ConfigCategory::FeaturePresets if matches!(field_focus, 0 | 1) => Some(field_focus),
        ConfigCategory::LifecycleHooks => match field_focus {
            0 => Some(0),
            2 => Some(1),
            3 => Some(2),
            _ => None,
        },
        ConfigCategory::Keybindings if field_focus == 1 => Some(1),
        _ => None,
    }
}

fn field_label_for_focus(category: &ConfigCategory, field_focus: usize) -> &'static str {
    match category {
        ConfigCategory::CustomSessions => match field_focus {
            0 => "Name",
            1 => "Description",
            2 => "Command",
            3 => "Window name",
            4 => "Working dir",
            5 => "Icon",
            6 => "Nerd icon",
            7 => "On stop",
            8 => "Pre-check",
            _ => "Field",
        },
        ConfigCategory::FeaturePresets => match field_focus {
            0 => "Name",
            1 => "Branch prefix",
            _ => "Field",
        },
        ConfigCategory::LifecycleHooks => match field_focus {
            0 => "Script",
            2 => "Prompt title",
            3 => "Prompt options",
            _ => "Field",
        },
        ConfigCategory::Keybindings => "Key",
        ConfigCategory::AllowedAgents => "Field",
    }
}

fn handle_edit_space(state: &mut ConfigWizardState) -> bool {
    state.error = None;
    match state.category {
        ConfigCategory::CustomSessions if state.field_focus == 9 => {
            if let Some(value) = state.field_toggles.get_mut(0) {
                *value = !*value;
            }
            true
        }
        ConfigCategory::FeaturePresets if state.field_focus >= 4 => {
            if let Some(value) = state.field_toggles.get_mut(state.field_focus - 4) {
                *value = !*value;
            }
            true
        }
        ConfigCategory::LifecycleHooks if state.field_focus == 1 => {
            if let Some(value) = state.field_toggles.get_mut(0) {
                *value = !*value;
            }
            true
        }
        _ => false,
    }
}

fn cycle_enum_field(state: &mut ConfigWizardState, delta: isize) -> bool {
    match state.category {
        ConfigCategory::FeaturePresets if state.field_focus == 2 => {
            let current = state.field_values[2].parse::<usize>().unwrap_or(0);
            state.field_values[2] = wrap_index(current, 3, delta).to_string();
            state.error = None;
            true
        }
        ConfigCategory::FeaturePresets if state.field_focus == 3 => {
            let current = state.field_values[3].parse::<usize>().unwrap_or(0);
            state.field_values[3] = wrap_index(current, AgentKind::ALL.len(), delta).to_string();
            state.error = None;
            true
        }
        ConfigCategory::Keybindings if state.field_focus == 0 => {
            let current = state
                .keybinding_actions
                .iter()
                .position(|action| state.field_values.first() == Some(action))
                .unwrap_or(0);
            if let Some(action) = state.keybinding_actions.get(wrap_index(
                current,
                state.keybinding_actions.len(),
                delta,
            )) {
                state.field_values[0] = action.clone();
                state.field_values[1] = state
                    .keybindings
                    .get(action)
                    .copied()
                    .or_else(|| crate::handlers::default_key_for_action(action))
                    .map(|key| key.to_string())
                    .unwrap_or_default();
                state.editing_index = state
                    .keybinding_actions
                    .iter()
                    .position(|known| known == action);
                state.error = None;
                true
            } else {
                false
            }
        }
        _ => false,
    }
}

fn move_focus_for_arrow(state: &mut ConfigWizardState, delta: isize) {
    let count = edit_focus_count(&state.category);
    if count == 0 {
        return;
    }
    state.error = None;
    state.field_focus = wrap_index(state.field_focus, count, delta);
    if matches!(state.category, ConfigCategory::Keybindings) {
        state.capturing_key = state.field_focus == 1;
    }
}

fn wrap_index(current: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    if delta >= 0 {
        (current + 1) % len
    } else if current == 0 {
        len - 1
    } else {
        current - 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extension::LifecycleHooks;
    use crate::project::ProjectStore;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;

    fn test_state(
        category: ConfigCategory,
        field_focus: usize,
        field_values: Vec<String>,
    ) -> ConfigWizardState {
        ConfigWizardState {
            step: ConfigWizardStep::EditItem,
            category,
            scope: ConfigScope::Global,
            selected: 0,
            field_focus,
            input_mode: false,
            sessions: Vec::new(),
            presets: Vec::new(),
            hooks: LifecycleHooks::default(),
            keybindings: HashMap::new(),
            allowed_agents: None,
            editing_index: None,
            field_values,
            field_editor: None,
            field_toggles: Vec::new(),
            agent_toggles: Vec::new(),
            agent_toggles_dirty: false,
            keybinding_actions: Vec::new(),
            capturing_key: false,
            original_json: String::new(),
            modified_json: String::new(),
            confirm_diff: None,
            preview_scroll: 0,
            project_repo: None,
            project_name: None,
            error: None,
        }
    }

    #[test]
    fn custom_session_command_opens_wrapped_field_editor_and_commits() {
        let mut state = test_state(
            ConfigCategory::CustomSessions,
            2,
            vec![
                "Release".into(),
                String::new(),
                "cargo run --release".into(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ],
        );

        open_field_editor(&mut state);
        let editor = state.field_editor.as_mut().expect("field editor");
        assert_eq!(editor.label, "Command");
        assert_eq!(editor.editor.text(), "cargo run --release");

        editor.editor.insert_str(" -- --dry-run");
        commit_field_editor(&mut state);

        assert!(!state.input_mode);
        assert!(state.field_editor.is_none());
        assert_eq!(state.field_values[2], "cargo run --release -- --dry-run");
    }

    #[test]
    fn lifecycle_prompt_title_commits_to_mapped_field_value() {
        let mut state = test_state(
            ConfigCategory::LifecycleHooks,
            2,
            vec!["script.sh".into(), "Old title".into(), "One, Two".into()],
        );

        open_field_editor(&mut state);
        let editor = state.field_editor.as_mut().expect("field editor");
        assert_eq!(editor.label, "Prompt title");
        editor.editor.clear();
        editor.editor.insert_str("New title");
        commit_field_editor(&mut state);

        assert_eq!(state.field_values[1], "New title");
    }

    #[test]
    fn keybinding_action_cycles_through_known_actions() {
        let mut state = test_state(
            ConfigCategory::Keybindings,
            0,
            vec!["quit".into(), "q".into()],
        );
        state.keybinding_actions = crate::handlers::DASHBOARD_KEYBINDING_ACTIONS
            .iter()
            .map(|(action, _)| (*action).to_string())
            .collect();

        assert!(cycle_enum_field(&mut state, 1));

        assert_eq!(state.field_values[0], "create_project");
        assert_eq!(state.field_values[1], "N");
    }

    #[test]
    fn keybinding_key_capture_replaces_existing_key() {
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = AppMode::ConfigWizard(test_state(
            ConfigCategory::Keybindings,
            1,
            vec!["refresh".into(), "r".into()],
        ));
        if let AppMode::ConfigWizard(state) = &mut app.mode {
            state.input_mode = true;
            state.capturing_key = true;
        }

        assert!(handle_keybinding_key_capture(
            &mut app,
            KeyEvent::new(KeyCode::Char('R'), KeyModifiers::SHIFT)
        ));

        let AppMode::ConfigWizard(state) = app.mode else {
            panic!("expected config wizard mode");
        };
        assert_eq!(state.field_values[1], "R");
        assert!(!state.input_mode);
        assert!(!state.capturing_key);
    }
}
