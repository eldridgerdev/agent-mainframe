use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, CreateProjectStep};

pub fn handle_create_project_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('b') {
        let is_path_step = matches!(
            &app.mode,
            AppMode::CreatingProject(s)
                if matches!(s.step, CreateProjectStep::Path)
        );
        if is_path_step {
            let browse = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::CreatingProject(state) = browse {
                app.start_browse_path(state);
            }
            return Ok(());
        }
    }

    let is_agent_step = matches!(
        &app.mode,
        AppMode::CreatingProject(state)
            if matches!(state.step, CreateProjectStep::Agent)
    );

    match key.code {
        KeyCode::Esc => {
            app.cancel_create();
        }
        KeyCode::Enter => {
            let step = match &app.mode {
                AppMode::CreatingProject(state) => state.step.clone(),
                _ => return Ok(()),
            };

            match step {
                CreateProjectStep::Name => {
                    if let AppMode::CreatingProject(state) = &mut app.mode {
                        state.step = CreateProjectStep::Path;
                    }
                }
                CreateProjectStep::Path => {
                    if let AppMode::CreatingProject(state) = &mut app.mode {
                        state.step = CreateProjectStep::Agent;
                    }
                }
                CreateProjectStep::Agent => {
                    app.create_project()?;
                }
            }
        }
        KeyCode::Tab => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                state.step = match state.step {
                    CreateProjectStep::Name => CreateProjectStep::Path,
                    CreateProjectStep::Path => CreateProjectStep::Agent,
                    CreateProjectStep::Agent => CreateProjectStep::Name,
                };
            }
        }
        KeyCode::Backspace => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                match state.step {
                    CreateProjectStep::Name => {
                        state.name.pop();
                    }
                    CreateProjectStep::Path => {
                        state.path.pop();
                    }
                    CreateProjectStep::Agent => {}
                }
            }
            app.refresh_create_project_agent_selection();
        }
        code if is_agent_step
            && matches!(code, KeyCode::Char('j') | KeyCode::Down | KeyCode::Right) =>
        {
            let (is_agent_step, path, next_index) = match &app.mode {
                AppMode::CreatingProject(state) => (
                    matches!(state.step, CreateProjectStep::Agent),
                    std::path::PathBuf::from(&state.path),
                    state.agent_index.saturating_add(1),
                ),
                _ => (false, std::path::PathBuf::new(), 0),
            };
            if is_agent_step {
                let allowed = app.allowed_agents_for_project_path(&path);
                if next_index < allowed.len()
                    && let AppMode::CreatingProject(state) = &mut app.mode
                {
                    state.agent_index = next_index;
                    state.agent = allowed[next_index].clone();
                }
            }
        }
        code if is_agent_step
            && matches!(code, KeyCode::Char('k') | KeyCode::Up | KeyCode::Left) =>
        {
            let (is_agent_step, path, current_index) = match &app.mode {
                AppMode::CreatingProject(state) => (
                    matches!(state.step, CreateProjectStep::Agent),
                    std::path::PathBuf::from(&state.path),
                    state.agent_index,
                ),
                _ => (false, std::path::PathBuf::new(), 0),
            };
            if is_agent_step && current_index > 0 {
                let next_index = current_index - 1;
                let allowed = app.allowed_agents_for_project_path(&path);
                if let AppMode::CreatingProject(state) = &mut app.mode {
                    state.agent_index = next_index;
                    state.agent = allowed[next_index].clone();
                }
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                match state.step {
                    CreateProjectStep::Name => state.name.push(c),
                    CreateProjectStep::Path => state.path.push(c),
                    CreateProjectStep::Agent => {}
                }
            }
            app.refresh_create_project_agent_selection();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_help_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::Help(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            if let Some(view) = from_view {
                app.mode = AppMode::Viewing(view);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_latest_prompt_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.close_latest_prompt();
        }
        KeyCode::Char('r') | KeyCode::Enter => {
            app.rerun_latest_prompt()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_steering_prompt_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_steering_prompt();
        }
        KeyCode::Tab => {
            app.submit_steering_prompt()?;
        }
        KeyCode::Enter => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                state.prompt.push('\n');
                state.refresh_prompt_analysis();
            }
        }
        KeyCode::Backspace => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                state.prompt.pop();
                state.refresh_prompt_analysis();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                state.prompt.push(c);
                state.refresh_prompt_analysis();
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_delete_project_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('y') => {
            app.delete_project()?;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_delete_feature_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('y') => {
            app.delete_feature()?;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{LatestPromptState, TextSelection, ViewState};
    use crate::project::{
        AgentKind, Feature, FeatureSession, Project, ProjectStatus, ProjectStore, SessionKind,
        VibeMode,
    };
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use chrono::Utc;
    use std::collections::HashMap;

    fn store_with_claude_session() -> ProjectStore {
        let now = Utc::now();
        let session = FeatureSession {
            id: "sess-1".to_string(),
            kind: SessionKind::Claude,
            label: "Claude 1".to_string(),
            tmux_window: "claude".to_string(),
            claude_session_id: None,
            created_at: now,
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
        };
        let feature = Feature {
            id: "feat-1".to_string(),
            name: "my-feat".to_string(),
            branch: "my-feat".to_string(),
            workdir: std::path::PathBuf::from("/tmp/test-workdir"),
            is_worktree: false,
            tmux_session: "amf-my-feat".to_string(),
            sessions: vec![session],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Claude,
            enable_chrome: false,
            has_notes: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        let project = Project {
            id: "proj-1".to_string(),
            name: "my-project".to_string(),
            repo: std::path::PathBuf::from("/tmp/test-repo"),
            collapsed: false,
            features: vec![feature],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: false,
        };
        ProjectStore {
            version: 4,
            projects: vec![project],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        }
    }

    #[test]
    fn latest_prompt_r_key_reruns_prompt() {
        let mut tmux = MockTmuxOps::new();
        tmux.expect_paste_text()
            .withf(|session, window, text| {
                session == "amf-my-feat" && window == "claude" && text == "rerun me"
            })
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_send_key_name()
            .withf(|session, window, key| {
                session == "amf-my-feat" && window == "claude" && key == "Enter"
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        let mut app = App::new_for_test(
            store_with_claude_session(),
            Box::new(tmux),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = AppMode::LatestPrompt(LatestPromptState {
            view: ViewState::new(
                "my-project".to_string(),
                "my-feat".to_string(),
                "amf-my-feat".to_string(),
                "claude".to_string(),
                "Claude 1".to_string(),
                VibeMode::Vibeless,
                false,
            ),
            prompt: "rerun me".to_string(),
            can_rerun: true,
            selection: TextSelection::default(),
        });

        handle_latest_prompt_key(&mut app, KeyCode::Char('r')).unwrap();

        assert!(matches!(app.mode, AppMode::Viewing(_)));
        assert_eq!(app.message.as_deref(), Some("Re-ran latest prompt"));
    }
}

pub fn handle_theme_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::ThemePicker(state) = &mut app.mode
                && state.selected + 1 < state.themes.len()
            {
                state.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::ThemePicker(state) = &mut app.mode
                && state.selected > 0
            {
                state.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let theme_name = match &app.mode {
                AppMode::ThemePicker(state) => state.themes.get(state.selected).copied(),
                _ => None,
            };
            if let Some(name) = theme_name {
                app.apply_theme(name);
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_rename_session_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_rename_session();
        }
        KeyCode::Enter => {
            app.apply_rename_session()?;
        }
        KeyCode::Backspace => {
            if let AppMode::RenamingSession(state) = &mut app.mode {
                state.input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::RenamingSession(state) = &mut app.mode {
                state.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_rename_feature_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_rename_feature();
        }
        KeyCode::Enter => {
            app.apply_rename_feature()?;
        }
        KeyCode::Backspace => {
            if let AppMode::RenamingFeature(state) = &mut app.mode {
                state.input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::RenamingFeature(state) = &mut app.mode {
                state.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_session_config_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => match &mut app.mode {
            AppMode::SessionConfig(state)
                if state.selected_agent + 1 < state.allowed_agents.len() =>
            {
                state.selected_agent += 1;
            }
            AppMode::ProjectAgentConfig(state)
                if state.selected_agent + 1 < state.allowed_agents.len() =>
            {
                state.selected_agent += 1;
            }
            _ => {}
        },
        KeyCode::Char('k') | KeyCode::Up => match &mut app.mode {
            AppMode::SessionConfig(state) if state.selected_agent > 0 => {
                state.selected_agent -= 1;
            }
            AppMode::ProjectAgentConfig(state) if state.selected_agent > 0 => {
                state.selected_agent -= 1;
            }
            _ => {}
        },
        KeyCode::Enter => {
            app.apply_session_config()?;
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_session_config();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_debug_log_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::DebugLog(state) => state.from_view,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            if let Some(view) = from_view {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::DebugLog(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_add(1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::DebugLog(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_sub(1);
            }
        }
        KeyCode::Char('c') => {
            app.debug_log.clear();
            if let AppMode::DebugLog(state) = &mut app.mode {
                state.scroll_offset = 0;
            }
        }
        _ => {}
    }
    Ok(())
}
