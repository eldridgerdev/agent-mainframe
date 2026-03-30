use anyhow::Result;
use crossterm::event::KeyCode;
use serde_json::json;

use crate::app::{App, AppMode, CodexDebugCommand, CommandAction, Selection};
use crate::project::SessionKind;
use crate::tmux::TmuxManager;

fn markdown_file_matches_plan_filter(
    state: &crate::app::MarkdownFilePickerState,
    index: usize,
) -> bool {
    state
        .files
        .get(index)
        .map(|path| {
            crate::markdown::markdown_view_relative_label(
                path,
                &state.workdir,
                state.repo_root.as_deref(),
            )
            .to_ascii_lowercase()
            .contains("plan")
        })
        .unwrap_or(false)
}

fn visible_markdown_file_indices(state: &crate::app::MarkdownFilePickerState) -> Vec<usize> {
    (0..state.files.len())
        .filter(|&idx| !state.plan_only || markdown_file_matches_plan_filter(state, idx))
        .collect()
}

fn clamp_markdown_picker_selection(state: &mut crate::app::MarkdownFilePickerState) {
    let visible = visible_markdown_file_indices(state);
    if visible.is_empty() {
        state.selected = 0;
    } else if !visible.contains(&state.selected) {
        state.selected = visible[0];
    }
}

pub fn handle_command_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let old_mode = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::CommandPicker(state) = old_mode
                && let Some(view) = state.from_view
            {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::CommandPicker(ref mut state) = app.mode {
                let len = state.commands.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::CommandPicker(ref mut state) = app.mode {
                let len = state.commands.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            let old_mode = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::CommandPicker(state) = old_mode {
                let selected_command = state.commands.get(state.selected).cloned();
                let return_view = state.from_view.clone();

                if let Some(command) = selected_command {
                    match command.action {
                        CommandAction::Local { command } => match command {
                            crate::app::LocalCommand::OpenDebugLog => {
                                app.open_debug_log(return_view.clone());
                            }
                            crate::app::LocalCommand::RefreshNotifications => {
                                app.refresh_status_and_notifications();
                                if let Some(view) = return_view.clone() {
                                    app.mode = AppMode::Viewing(view);
                                }
                            }
                        },
                        CommandAction::CodexLiveDemo(debug_command) => {
                            if let Some(session_id) =
                                app.command_picker_codex_target(state.from_view.as_ref())
                            {
                                let event = codex_debug_event(debug_command);
                                app.apply_codex_live_event(&session_id, &event);
                                app.message = Some(format!("Applied '{}'", command.name.as_str()));
                            } else {
                                app.message = Some("No Codex session selected".into());
                            }
                        }
                        CommandAction::SlashCommand => {
                            let command_text = format!("/{}", command.name);

                            let tmux_info = if let Some(ref view) = state.from_view {
                                Some((view.session.clone(), view.window.clone()))
                            } else if let Some((_, feature)) = app.selected_feature() {
                                let window = feature
                                    .sessions
                                    .iter()
                                    .find(|s| {
                                        matches!(
                                            s.kind,
                                            SessionKind::Claude
                                                | SessionKind::Opencode
                                                | SessionKind::Codex
                                        )
                                    })
                                    .map(|s| s.tmux_window.clone())
                                    .unwrap_or_else(|| "terminal".into());
                                Some((feature.tmux_session.clone(), window))
                            } else {
                                None
                            };

                            if let Some((session, window)) = &tmux_info {
                                let _ = TmuxManager::send_literal(session, window, &command_text);
                                let _ = TmuxManager::send_key_name(session, window, "Enter");
                                app.message = Some(format!("Sent '{}'", command_text));
                            } else {
                                app.message = Some("No active session to send to".into());
                            }
                        }
                    }
                }

                if let Some(view) = return_view
                    && matches!(app.mode, AppMode::Normal)
                {
                    app.mode = AppMode::Viewing(view);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn codex_debug_event(command: CodexDebugCommand) -> serde_json::Value {
    match command {
        CodexDebugCommand::PlanDemo => json!({
            "type": "plan",
            "thread_id": "thread-demo",
            "payload": {
                "text": "1. Inspect reducer\n2. Patch sidebar\n3. Re-run tests"
            }
        }),
        CodexDebugCommand::WorkChangeReasonDemo => json!({
            "type": "fileChange",
            "payload": {
                "relative_path": "src/app/codex_live.rs",
                "status": "needs-reason",
                "tool": "Edit"
            }
        }),
        CodexDebugCommand::WorkDiffReviewDemo => json!({
            "type": "fileChange",
            "payload": {
                "relative_path": "src/ui/dashboard.rs",
                "status": "needs-review",
                "tool": "Edit",
                "message": "Review the change before continuing."
            }
        }),
        CodexDebugCommand::WorkCommandDemo => json!({
            "type": "commandExecution",
            "payload": {
                "command": "cargo test codex_sidebar -- --nocapture",
                "phase": "running"
            }
        }),
        CodexDebugCommand::WorkFileDemo => json!({
            "type": "fileChange",
            "payload": {
                "relative_path": "src/ui/dashboard.rs",
                "status": "proposed"
            }
        }),
        CodexDebugCommand::WorkInputDemo => json!({
            "type": "requestUserInput",
            "payload": {
                "prompt": "Need approval before applying the patch."
            }
        }),
        CodexDebugCommand::ClearInputDemo => json!({
            "type": "inputResolved"
        }),
    }
}

pub fn handle_syntax_language_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    let operation_running = matches!(
        &app.mode,
        AppMode::SyntaxLanguagePicker(state) if state.operation.is_some()
    );

    if operation_running {
        if matches!(key, KeyCode::Esc | KeyCode::Char('q')) {
            app.message = Some("Wait for the syntax operation to finish".into());
        }
        return Ok(());
    }

    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.close_syntax_language_picker();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::SyntaxLanguagePicker(state) = &mut app.mode {
                let len = state.languages.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::SyntaxLanguagePicker(state) = &mut app.mode {
                let len = state.languages.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter | KeyCode::Char('i') => {
            app.syntax_picker_install_selected();
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            app.syntax_picker_uninstall_selected();
        }
        KeyCode::Char('r') => {
            app.refresh_syntax_language_picker();
            if let AppMode::SyntaxLanguagePicker(state) = &mut app.mode {
                state.notice = Some("Refreshed syntax parser status".into());
            }
        }
        _ => {}
    }

    Ok(())
}

pub fn handle_markdown_file_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let old_mode = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::MarkdownFilePicker(state) = old_mode
                && let Some(view) = state.from_view
            {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::MarkdownFilePicker(ref mut state) = app.mode {
                clamp_markdown_picker_selection(state);
                let visible = visible_markdown_file_indices(state);
                if !visible.is_empty() {
                    let pos = visible
                        .iter()
                        .position(|&idx| idx == state.selected)
                        .unwrap_or(0);
                    state.selected = visible[(pos + 1) % visible.len()];
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::MarkdownFilePicker(ref mut state) = app.mode {
                clamp_markdown_picker_selection(state);
                let visible = visible_markdown_file_indices(state);
                if !visible.is_empty() {
                    let pos = visible
                        .iter()
                        .position(|&idx| idx == state.selected)
                        .unwrap_or(0);
                    state.selected = if pos == 0 {
                        *visible.last().unwrap()
                    } else {
                        visible[pos - 1]
                    };
                }
            }
        }
        KeyCode::Char('p') => {
            if let AppMode::MarkdownFilePicker(ref mut state) = app.mode {
                state.plan_only = !state.plan_only;
                clamp_markdown_picker_selection(state);
            }
        }
        KeyCode::Enter => {
            let old_mode = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::MarkdownFilePicker(mut state) = old_mode {
                clamp_markdown_picker_selection(&mut state);
                let path = state.files.get(state.selected).cloned();
                if let (Some(path), Some(view)) = (path, state.from_view.clone()) {
                    let return_to_picker = Some(crate::app::MarkdownFilePickerState {
                        files: state.files,
                        selected: state.selected,
                        plan_only: state.plan_only,
                        workdir: state.workdir.clone(),
                        repo_root: state.repo_root.clone(),
                        from_view: Some(view.clone()),
                    });
                    return app.open_markdown_viewer_path(
                        path,
                        state.workdir,
                        state.repo_root,
                        view,
                        return_to_picker,
                    );
                }
                app.mode = AppMode::MarkdownFilePicker(state);
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{CommandEntry, CommandPickerState, MarkdownFilePickerState, ViewState};
    use crate::project::{
        AgentKind, Feature, FeatureSession, Project, ProjectStatus, ProjectStore, SessionKind,
        VibeMode,
    };
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use chrono::Utc;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn picker_app() -> App {
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

    fn picker_view() -> ViewState {
        ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "claude".into(),
            "Claude 1".into(),
            crate::project::SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        )
    }

    fn codex_picker_app() -> App {
        let now = Utc::now();
        let feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![FeatureSession {
                id: "codex-1".into(),
                kind: SessionKind::Codex,
                label: "Codex".into(),
                tmux_window: "codex".into(),
                claude_session_id: None,
                token_usage_source: None,
                token_usage_source_match: None,
                created_at: now,
                command: None,
                on_stop: None,
                pre_check: None,
                status_text: None,
            }],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Codex,
            enable_chrome: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        let store = ProjectStore {
            version: 5,
            projects: vec![Project {
                id: "proj-1".into(),
                name: "demo".into(),
                repo: PathBuf::from("/tmp/demo"),
                collapsed: false,
                features: vec![feature],
                created_at: now,
                preferred_agent: AgentKind::Codex,
                is_git: false,
            }],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        };

        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.selection = Selection::Feature(0, 0);
        app
    }

    #[test]
    fn markdown_picker_plan_toggle_keeps_only_plan_matches() {
        let mut app = picker_app();
        app.mode = AppMode::MarkdownFilePicker(MarkdownFilePickerState {
            files: vec![
                PathBuf::from("/tmp/demo/docs/guide.md"),
                PathBuf::from("/tmp/demo/.claude/plan.md"),
                PathBuf::from("/tmp/demo/PLAN.md"),
            ],
            selected: 0,
            plan_only: true,
            workdir: PathBuf::from("/tmp/demo"),
            repo_root: None,
            from_view: Some(picker_view()),
        });

        handle_markdown_file_picker_key(&mut app, KeyCode::Char('p')).unwrap();

        match &app.mode {
            AppMode::MarkdownFilePicker(state) => {
                assert!(!state.plan_only);
                assert_eq!(state.selected, 0);
            }
            _ => panic!("expected markdown picker to stay open"),
        }
    }

    #[test]
    fn markdown_picker_navigation_uses_filtered_rows() {
        let mut app = picker_app();
        app.mode = AppMode::MarkdownFilePicker(MarkdownFilePickerState {
            files: vec![
                PathBuf::from("/tmp/demo/docs/guide.md"),
                PathBuf::from("/tmp/demo/.claude/plan.md"),
                PathBuf::from("/tmp/demo/docs/plan-notes.md"),
            ],
            selected: 1,
            plan_only: true,
            workdir: PathBuf::from("/tmp/demo"),
            repo_root: None,
            from_view: Some(picker_view()),
        });

        handle_markdown_file_picker_key(&mut app, KeyCode::Char('j')).unwrap();

        match &app.mode {
            AppMode::MarkdownFilePicker(state) => assert_eq!(state.selected, 2),
            _ => panic!("expected markdown picker to stay open"),
        }
    }

    #[test]
    fn command_picker_debug_demo_updates_live_codex_state_locally() {
        let mut app = codex_picker_app();
        app.mode = AppMode::CommandPicker(CommandPickerState {
            commands: vec![CommandEntry {
                name: "demo-plan".into(),
                source: "AMF Debug".into(),
                path: None,
                action: CommandAction::CodexLiveDemo(CodexDebugCommand::PlanDemo),
            }],
            selected: 0,
            from_view: None,
        });

        handle_command_picker_key(&mut app, KeyCode::Enter).unwrap();

        assert_eq!(
            app.codex_live_thread("amf-feature")
                .unwrap()
                .plan_text
                .as_deref(),
            Some("1. Inspect reducer\n2. Patch sidebar\n3. Re-run tests")
        );
        assert_eq!(app.message.as_deref(), Some("Applied 'demo-plan'"));
        assert!(matches!(app.mode, AppMode::Normal));
    }

    #[test]
    fn command_picker_change_reason_demo_updates_live_codex_state_locally() {
        let mut app = codex_picker_app();
        app.mode = AppMode::CommandPicker(CommandPickerState {
            commands: vec![CommandEntry {
                name: "demo-work-change-reason".into(),
                source: "AMF Debug".into(),
                path: None,
                action: CommandAction::CodexLiveDemo(CodexDebugCommand::WorkChangeReasonDemo),
            }],
            selected: 0,
            from_view: None,
        });

        handle_command_picker_key(&mut app, KeyCode::Enter).unwrap();

        assert_eq!(
            app.codex_live_thread("amf-feature")
                .and_then(|live| live.sidebar_work_text())
                .as_deref(),
            Some(
                "State: waiting for change reason\nFile: src/app/codex_live.rs\nTool: Edit\nRequest: Explain why this change is needed."
            )
        );
        assert_eq!(
            app.message.as_deref(),
            Some("Applied 'demo-work-change-reason'")
        );
        assert!(matches!(app.mode, AppMode::Normal));
    }
}

pub fn handle_notification_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::NotificationPicker(_, v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            if let Some(view) = from_view {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::NotificationPicker(ref mut idx, _) = app.mode {
                let len = app.pending_inputs.len();
                if len > 0 {
                    *idx = (*idx + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::NotificationPicker(ref mut idx, _) = app.mode {
                let len = app.pending_inputs.len();
                if len > 0 {
                    *idx = if *idx == 0 { len - 1 } else { *idx - 1 };
                }
            }
        }
        KeyCode::Enter => {
            app.handle_notification_select()?;
        }
        KeyCode::Char('x') | KeyCode::Delete => {
            if let AppMode::NotificationPicker(ref mut idx, _) = app.mode {
                let i = *idx;
                if i < app.pending_inputs.len() {
                    let input = app.pending_inputs.remove(i);
                    let _ = std::fs::remove_file(&input.file_path);
                    app.message = Some("Input request deleted".into());
                    if app.pending_inputs.is_empty() {
                        let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                            AppMode::NotificationPicker(_, v) => v,
                            other => {
                                app.mode = other;
                                return Ok(());
                            }
                        };
                        if let Some(view) = from_view {
                            app.mode = AppMode::Viewing(view);
                        }
                    } else if *idx >= app.pending_inputs.len() {
                        *idx = app.pending_inputs.len() - 1;
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_session_switcher_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_session_switcher();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::SessionSwitcher(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::SessionSwitcher(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            app.switch_from_switcher();
        }
        KeyCode::Char('r') => {
            app.start_rename_from_switcher();
        }
        KeyCode::Char('s') => {
            app.open_session_picker_from_switcher()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_opencode_session_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_opencode_session_picker();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::OpencodeSessionPicker(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::OpencodeSessionPicker(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            app.confirm_opencode_session();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_claude_session_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_claude_session_picker();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::ClaudeSessionPicker(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::ClaudeSessionPicker(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            app.confirm_claude_session();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_codex_session_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_codex_session_picker();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::CodexSessionPicker(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::CodexSessionPicker(ref mut state) = app.mode {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            app.confirm_codex_session();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_claude_session_confirm_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('n') => {
            app.cancel_claude_session_confirm();
        }
        KeyCode::Char('y') => {
            app.confirm_and_start_claude()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_codex_session_confirm_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('n') => {
            app.cancel_codex_session_confirm();
        }
        KeyCode::Char('y') => {
            app.confirm_and_start_codex()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_opencode_session_confirm_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('n') => {
            app.cancel_opencode_session_confirm();
        }
        KeyCode::Char('y') => {
            app.confirm_and_start_opencode()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_session_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let old_mode = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::SessionPicker(state) = old_mode
                && let Some(view) = state.from_view
            {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::SessionPicker(ref mut state) = app.mode {
                let total = state.builtin_sessions.len() + state.custom_sessions.len();
                if total > 0 {
                    let start = state.selected;
                    loop {
                        state.selected = (state.selected + 1) % total;
                        if state.selected == start {
                            break;
                        }
                        if state.selected < state.builtin_sessions.len() {
                            if state.builtin_sessions[state.selected].disabled.is_none() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::SessionPicker(ref mut state) = app.mode {
                let total = state.builtin_sessions.len() + state.custom_sessions.len();
                if total > 0 {
                    let start = state.selected;
                    loop {
                        state.selected = if state.selected == 0 {
                            total - 1
                        } else {
                            state.selected - 1
                        };
                        if state.selected == start {
                            break;
                        }
                        if state.selected < state.builtin_sessions.len() {
                            if state.builtin_sessions[state.selected].disabled.is_none() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        KeyCode::Enter => {
            let old_mode = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::SessionPicker(state) = old_mode {
                let builtin_len = state.builtin_sessions.len();
                if state.selected < builtin_len {
                    let builtin = &state.builtin_sessions[state.selected];
                    if let Some(ref reason) = builtin.disabled {
                        app.message = Some(format!("Cannot start: {}", reason));
                        app.mode = AppMode::SessionPicker(state);
                        return Ok(());
                    }
                    match app.add_builtin_session(state.pi, state.fi, builtin.kind.clone()) {
                        Ok(()) => {
                            app.message = Some(format!("Added '{}'", builtin.label));
                        }
                        Err(e) => {
                            app.message = Some(format!("Error: {}", e));
                        }
                    }
                } else {
                    let custom_idx = state.selected - builtin_len;
                    if let Some(cfg) = state.custom_sessions.get(custom_idx).cloned() {
                        // Resolve working directory for the
                        // pre_check (same logic as session_ops).
                        let check_dir = app
                            .store
                            .projects
                            .get(state.pi)
                            .and_then(|p| p.features.get(state.fi))
                            .map(|f| {
                                cfg.working_dir
                                    .as_ref()
                                    .map(|rel| f.workdir.join(rel))
                                    .unwrap_or_else(|| f.workdir.clone())
                            });
                        let pre_ok = match &check_dir {
                            Some(dir) => cfg.run_pre_check(dir),
                            None => Ok(()),
                        };
                        if let Err(reason) = pre_ok {
                            app.message = Some(format!("{}: {}", cfg.name, reason));
                        } else {
                            match app.add_custom_session_type(state.pi, state.fi, &cfg) {
                                Ok(autolaunch) => {
                                    app.message = Some(format!("Added '{}'", cfg.name));
                                    if autolaunch {
                                        // Point selection to the newly added session
                                        // (last in the sessions list).
                                        if let Some(feature) = app
                                            .store
                                            .projects
                                            .get(state.pi)
                                            .and_then(|p| p.features.get(state.fi))
                                        {
                                            let si = feature.sessions.len().saturating_sub(1);
                                            app.selection =
                                                Selection::Session(state.pi, state.fi, si);
                                        }
                                        let _ = app.enter_view();
                                    }
                                }
                                Err(e) => {
                                    app.message = Some(format!("Error: {}", e));
                                }
                            }
                        }
                    }
                }
                if let Some(view) = state.from_view {
                    app.mode = AppMode::Viewing(view);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_bookmark_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let old_mode = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::BookmarkPicker(state) = old_mode
                && let Some(view) = state.from_view
            {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::BookmarkPicker(ref mut state) = app.mode {
                let len = app.store.session_bookmarks.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::BookmarkPicker(ref mut state) = app.mode {
                let len = app.store.session_bookmarks.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            let slot = if let AppMode::BookmarkPicker(state) = &app.mode {
                if app.store.session_bookmarks.is_empty() {
                    app.message = Some("No bookmarks yet".into());
                    return Ok(());
                }
                state.selected + 1
            } else {
                return Ok(());
            };
            app.jump_to_bookmark(slot)?;
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            let slot = if let AppMode::BookmarkPicker(state) = &app.mode {
                if app.store.session_bookmarks.is_empty() {
                    app.message = Some("No bookmarks to remove".into());
                    return Ok(());
                }
                state.selected + 1
            } else {
                return Ok(());
            };
            app.remove_bookmark_slot(slot)?;
            if let AppMode::BookmarkPicker(state) = &mut app.mode {
                let len = app.store.session_bookmarks.len();
                if len == 0 {
                    state.selected = 0;
                } else if state.selected >= len {
                    state.selected = len - 1;
                }
            }
        }
        KeyCode::Char(c @ '1'..='9') => {
            let slot = (c as u8 - b'0') as usize;
            app.jump_to_bookmark(slot)?;
        }
        _ => {}
    }
    Ok(())
}
