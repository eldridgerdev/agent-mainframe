use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, CreateProjectStep};

const STEERING_PROMPT_PAGE_SCROLL: usize = 6;

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
            let view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::LatestPrompt(state) => state.view,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.mode = AppMode::Viewing(view);
        }
        KeyCode::Tab | KeyCode::Enter => {
            app.inject_latest_prompt()?;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.latest_prompt_select_next();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.latest_prompt_select_prev();
        }
        KeyCode::Char('y') => {
            app.copy_selected_prompt_to_clipboard()?;
        }
        _ => {}
    }
    Ok(())
}

const MARKDOWN_FAST_SCROLL_STEP: usize = 8;

pub fn handle_markdown_viewer_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(
            key.code,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('k') | KeyCode::Up
        )
    {
        if let AppMode::MarkdownViewer(state) = &mut app.mode {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    state.scroll_offset = state
                        .scroll_offset
                        .saturating_add(MARKDOWN_FAST_SCROLL_STEP);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    state.scroll_offset = state
                        .scroll_offset
                        .saturating_sub(MARKDOWN_FAST_SCROLL_STEP);
                }
                _ => {}
            }
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Char('b') => {
            let picker = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::MarkdownViewer(mut state) => {
                    if let Some(picker) = state.return_to_picker.take() {
                        picker
                    } else {
                        app.mode = AppMode::MarkdownViewer(state);
                        return Ok(());
                    }
                }
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.mode = AppMode::MarkdownFilePicker(picker);
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::MarkdownViewer(state) => state.from_view,
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
            if let AppMode::MarkdownViewer(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_add(1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::MarkdownViewer(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_sub(1);
            }
        }
        KeyCode::PageDown => {
            if let AppMode::MarkdownViewer(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_add(10);
            }
        }
        KeyCode::PageUp => {
            if let AppMode::MarkdownViewer(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_sub(10);
            }
        }
        KeyCode::Home | KeyCode::Char('g') => {
            if let AppMode::MarkdownViewer(state) = &mut app.mode {
                state.scroll_offset = 0;
            }
        }
        KeyCode::End | KeyCode::Char('G') => {
            if let AppMode::MarkdownViewer(state) = &mut app.mode {
                state.scroll_offset = usize::MAX;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_steering_prompt_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.cancel_steering_prompt();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('v') {
        if let AppMode::SteeringPrompt(state) = &mut app.mode {
            state.editor.toggle_vim();
            app.message = Some(if state.editor.vim_mode().is_some() {
                "Vim mode enabled".into()
            } else {
                "Vim mode disabled".into()
            });
        }
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
        if let AppMode::SteeringPrompt(state) = &mut app.mode {
            state.clear_prompt();
            app.message = Some("Steering prompt cleared".into());
        }
        return Ok(());
    }

    if let AppMode::SteeringPrompt(state) = &mut app.mode {
        match key.code {
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.scroll_down(1);
                return Ok(());
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.scroll_up(1);
                return Ok(());
            }
            KeyCode::PageDown => {
                state.scroll_down(STEERING_PROMPT_PAGE_SCROLL);
                return Ok(());
            }
            KeyCode::PageUp => {
                state.scroll_up(STEERING_PROMPT_PAGE_SCROLL);
                return Ok(());
            }
            _ => {}
        }
    }

    match key.code {
        KeyCode::Tab => {
            app.submit_steering_prompt()?;
        }
        KeyCode::Esc if matches!(&app.mode, AppMode::SteeringPrompt(state) if state.editor.vim_mode().is_none()) =>
        {
            app.cancel_steering_prompt();
        }
        _ => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                let outcome = state.editor.handle_key(key);
                if outcome.text_changed {
                    state.refresh_prompt_analysis();
                }
                if outcome.text_changed || outcome.cursor_moved {
                    state.request_cursor_scroll();
                }
            }
        }
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

pub fn handle_theme_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => {
            let preview = if let AppMode::ThemePicker(state) = &mut app.mode
                && state.selected + 1 < state.themes.len()
            {
                state.selected += 1;
                state.themes.get(state.selected).copied()
            } else {
                None
            };
            if let Some(name) = preview {
                app.preview_theme(name);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            let preview = if let AppMode::ThemePicker(state) = &mut app.mode
                && state.selected > 0
            {
                state.selected -= 1;
                state.themes.get(state.selected).copied()
            } else {
                None
            };
            if let Some(name) = preview {
                app.preview_theme(name);
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
        KeyCode::Char('t') => {
            app.toggle_transparent_background();
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            let original = match &app.mode {
                AppMode::ThemePicker(state) => Some(state.original_theme),
                _ => None,
            };
            if let Some(name) = original {
                app.preview_theme(name);
            }
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
        KeyCode::Char('/') => {
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::DebugLog(state) => state.from_view,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.open_command_picker_with_focus(from_view, crate::app::CommandPickerFocus::Local);
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::*;
    use crate::app::{
        CommandAction, MarkdownFilePickerState, MarkdownViewerState, SteeringPromptState, ViewState,
    };
    use crate::project::{AgentKind, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use tempfile::TempDir;

    fn steering_app(workdir: &std::path::Path, prompt: &str) -> App {
        let store = ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        };
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = AppMode::SteeringPrompt(SteeringPromptState::new(
            ViewState::new(
                "demo".to_string(),
                "feature".to_string(),
                "amf-feature".to_string(),
                "claude".to_string(),
                "Claude 1".to_string(),
                crate::project::SessionKind::Claude,
                VibeMode::Vibeless,
                false,
            ),
            workdir.to_path_buf(),
            prompt.to_string(),
        ));
        app
    }

    fn markdown_view() -> ViewState {
        ViewState::new(
            "demo".to_string(),
            "feature".to_string(),
            "amf-feature".to_string(),
            "claude".to_string(),
            "Claude 1".to_string(),
            crate::project::SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        )
    }

    fn markdown_app() -> App {
        let store = ProjectStore {
            version: 5,
            projects: vec![Project::new(
                "demo".into(),
                PathBuf::from("/tmp/demo"),
                true,
                AgentKind::Claude,
            )],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        };
        App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        )
    }

    #[test]
    fn steering_prompt_escape_enters_vim_normal_mode() {
        let repo = TempDir::new().unwrap();
        let mut app = steering_app(repo.path(), "draft");

        handle_steering_prompt_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
            .unwrap();

        match &app.mode {
            AppMode::SteeringPrompt(state) => {
                assert_eq!(
                    state.editor.vim_mode(),
                    Some(crate::editor::VimMode::Normal)
                );
                assert_eq!(state.editor.text(), "draft");
            }
            _ => panic!("expected steering prompt to stay open"),
        }
    }

    #[test]
    fn steering_prompt_ctrl_q_closes_dialog() {
        let repo = TempDir::new().unwrap();
        let mut app = steering_app(repo.path(), "draft");

        handle_steering_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
        )
        .unwrap();

        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn steering_prompt_ctrl_v_toggles_vim_mode() {
        let repo = TempDir::new().unwrap();
        let mut app = steering_app(repo.path(), "draft");

        handle_steering_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
        )
        .unwrap();

        match &app.mode {
            AppMode::SteeringPrompt(state) => {
                assert_eq!(state.editor.vim_mode(), None);
                assert_eq!(state.editor.text(), "draft");
            }
            _ => panic!("expected steering prompt to stay open"),
        }

        handle_steering_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::CONTROL),
        )
        .unwrap();

        match &app.mode {
            AppMode::SteeringPrompt(state) => {
                assert_eq!(
                    state.editor.vim_mode(),
                    Some(crate::editor::VimMode::Insert)
                );
                assert_eq!(state.editor.text(), "draft");
            }
            _ => panic!("expected steering prompt to stay open"),
        }
    }

    #[test]
    fn steering_prompt_scroll_keys_adjust_offset() {
        let repo = TempDir::new().unwrap();
        let mut app = steering_app(repo.path(), "draft");

        if let AppMode::SteeringPrompt(state) = &mut app.mode {
            state.scroll_offset = 4;
        }

        handle_steering_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        )
        .unwrap();
        handle_steering_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
        )
        .unwrap();
        handle_steering_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
        )
        .unwrap();
        handle_steering_prompt_key(&mut app, KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE))
            .unwrap();

        match &app.mode {
            AppMode::SteeringPrompt(state) => assert_eq!(state.scroll_offset, 4),
            _ => panic!("expected steering prompt to stay open"),
        }
    }

    #[test]
    fn steering_prompt_ctrl_l_clears_editor() {
        let repo = TempDir::new().unwrap();
        let mut app = steering_app(repo.path(), "draft\nwith details");

        if let AppMode::SteeringPrompt(state) = &mut app.mode {
            state.scroll_offset = 8;
        }

        handle_steering_prompt_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('l'), KeyModifiers::CONTROL),
        )
        .unwrap();

        match &app.mode {
            AppMode::SteeringPrompt(state) => {
                assert_eq!(state.editor.text(), "");
                assert_eq!(state.scroll_offset, 0);
                assert_eq!(
                    state.prompt_analysis.score,
                    crate::app::analyze_prompt("").score
                );
            }
            _ => panic!("expected steering prompt to stay open"),
        }
        assert_eq!(app.message.as_deref(), Some("Steering prompt cleared"));
    }

    #[test]
    fn markdown_viewer_b_returns_to_picker_when_available() {
        let mut app = markdown_app();
        let view = markdown_view();
        let picker = MarkdownFilePickerState {
            files: vec![PathBuf::from("a.md"), PathBuf::from("b.md")],
            selected: 1,
            plan_only: true,
            workdir: PathBuf::from("/tmp/demo"),
            repo_root: Some(PathBuf::from("/tmp/demo-repo")),
            from_view: Some(view.clone()),
        };
        app.mode = AppMode::MarkdownViewer(MarkdownViewerState {
            title: "b.md".into(),
            source_path: PathBuf::from("b.md"),
            content: "# Title".into(),
            scroll_offset: 0,
            rendered_width: 0,
            rendered_lines: Vec::new(),
            return_to_picker: Some(picker),
            from_view: Some(view),
        });

        handle_markdown_viewer_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        )
        .unwrap();

        match &app.mode {
            AppMode::MarkdownFilePicker(state) => {
                assert_eq!(state.selected, 1);
                assert_eq!(state.files.len(), 2);
            }
            _ => panic!("expected markdown picker after pressing b"),
        }
    }

    #[test]
    fn markdown_viewer_b_is_noop_without_picker_context() {
        let mut app = markdown_app();
        let view = markdown_view();
        app.mode = AppMode::MarkdownViewer(MarkdownViewerState {
            title: "notes.md".into(),
            source_path: PathBuf::from("notes.md"),
            content: "# Title".into(),
            scroll_offset: 0,
            rendered_width: 0,
            rendered_lines: Vec::new(),
            return_to_picker: None,
            from_view: Some(view),
        });

        handle_markdown_viewer_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('b'), KeyModifiers::NONE),
        )
        .unwrap();

        assert!(matches!(app.mode, AppMode::MarkdownViewer(_)));
    }

    #[test]
    fn markdown_viewer_ctrl_j_scrolls_faster() {
        let mut app = markdown_app();
        let view = markdown_view();
        app.mode = AppMode::MarkdownViewer(MarkdownViewerState {
            title: "notes.md".into(),
            source_path: PathBuf::from("notes.md"),
            content: "# Title".into(),
            scroll_offset: 3,
            rendered_width: 0,
            rendered_lines: Vec::new(),
            return_to_picker: None,
            from_view: Some(view),
        });

        handle_markdown_viewer_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
        )
        .unwrap();

        match &app.mode {
            AppMode::MarkdownViewer(state) => {
                assert_eq!(state.scroll_offset, 3 + MARKDOWN_FAST_SCROLL_STEP);
            }
            _ => panic!("expected markdown viewer to stay open"),
        }
    }

    #[test]
    fn markdown_viewer_ctrl_up_scrolls_back_faster() {
        let mut app = markdown_app();
        let view = markdown_view();
        app.mode = AppMode::MarkdownViewer(MarkdownViewerState {
            title: "notes.md".into(),
            source_path: PathBuf::from("notes.md"),
            content: "# Title".into(),
            scroll_offset: 12,
            rendered_width: 0,
            rendered_lines: Vec::new(),
            return_to_picker: None,
            from_view: Some(view),
        });

        handle_markdown_viewer_key(&mut app, KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL))
            .unwrap();

        match &app.mode {
            AppMode::MarkdownViewer(state) => {
                assert_eq!(state.scroll_offset, 12 - MARKDOWN_FAST_SCROLL_STEP);
            }
            _ => panic!("expected markdown viewer to stay open"),
        }
    }

    #[test]
    fn debug_log_slash_opens_local_command_picker() {
        let mut app = markdown_app();
        let view = markdown_view();
        app.mode = AppMode::DebugLog(crate::app::DebugLogState {
            scroll_offset: 0,
            from_view: Some(view.clone()),
        });

        handle_debug_log_key(&mut app, KeyCode::Char('/')).unwrap();

        match &app.mode {
            AppMode::CommandPicker(state) => {
                assert!(matches!(
                    state
                        .commands
                        .get(state.selected)
                        .map(|entry| &entry.action),
                    Some(CommandAction::Local { .. })
                ));
                assert_eq!(
                    state.from_view.as_ref().map(|from_view| &from_view.window),
                    Some(&view.window)
                );
            }
            _ => panic!("expected command picker after pressing / in debug log"),
        }
    }
}
