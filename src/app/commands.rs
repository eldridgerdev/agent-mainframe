use std::path::{Path, PathBuf};

use anyhow::Result;

use super::*;

#[derive(Debug, Clone)]
struct FeatureCommandContext {
    project_name: String,
    feature_name: String,
    repo: PathBuf,
    workdir: PathBuf,
    tmux_session: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandExecutionOutcome {
    ReturnToOrigin,
    KeepCurrentMode,
}

impl App {
    pub fn open_command_picker(&mut self, from_view: Option<ViewState>) {
        let commands = self.collect_command_entries(from_view.as_ref());
        self.mode = AppMode::CommandPicker(CommandPickerState {
            selected: 0,
            commands,
            from_view,
        });
    }

    pub fn open_debug_log(&mut self, from_view: Option<ViewState>) {
        self.mode = AppMode::DebugLog(DebugLogState {
            scroll_offset: 0,
            from_view,
        });
    }

    pub fn refresh_status_and_notifications(&mut self) {
        self.sync_statuses();
        if self.ipc.is_some() {
            self.drain_ipc_messages();
        } else {
            self.scan_notifications();
        }
        self.message = Some("Refreshed statuses".into());
    }

    pub fn execute_command_entry(
        &mut self,
        entry: &CommandEntry,
        from_view: Option<ViewState>,
    ) -> Result<CommandExecutionOutcome> {
        match &entry.action {
            CommandAction::SlashCommand { name } => {
                let Some((session, window)) = self.command_tmux_target(from_view.as_ref()) else {
                    self.message = Some("No active session to send to".into());
                    return Ok(CommandExecutionOutcome::ReturnToOrigin);
                };

                let command_text = format!("/{}", name);
                self.tmux.send_literal(&session, &window, &command_text)?;
                self.tmux.send_key_name(&session, &window, "Enter")?;
                self.message = Some(format!("Sent '{}'", command_text));
                Ok(CommandExecutionOutcome::ReturnToOrigin)
            }
            CommandAction::Local { command } => match command {
                LocalCommand::OpenDebugLog => {
                    self.open_debug_log(from_view);
                    Ok(CommandExecutionOutcome::KeepCurrentMode)
                }
                LocalCommand::ClearDebugLog => {
                    self.debug_log.clear();
                    self.message = Some("Cleared debug log".into());
                    Ok(CommandExecutionOutcome::ReturnToOrigin)
                }
                LocalCommand::RefreshNotifications => {
                    self.refresh_status_and_notifications();
                    Ok(CommandExecutionOutcome::ReturnToOrigin)
                }
                LocalCommand::InjectTestInputRequest => {
                    self.inject_test_input_request(from_view.as_ref());
                    Ok(CommandExecutionOutcome::ReturnToOrigin)
                }
                LocalCommand::GenerateSummary => {
                    self.trigger_summary_for_selected()?;
                    Ok(CommandExecutionOutcome::ReturnToOrigin)
                }
                LocalCommand::OpenPlanMarkdown => {
                    self.open_markdown_viewer_for_command(from_view, true)?;
                    Ok(CommandExecutionOutcome::KeepCurrentMode)
                }
                LocalCommand::OpenLatestPrompt => {
                    if let Some(view) = from_view {
                        self.open_latest_prompt_for_view(view);
                        Ok(CommandExecutionOutcome::KeepCurrentMode)
                    } else {
                        self.message = Some("Latest prompt is only available from pane view".into());
                        Ok(CommandExecutionOutcome::ReturnToOrigin)
                    }
                }
            },
        }
    }

    fn collect_command_entries(&self, from_view: Option<&ViewState>) -> Vec<CommandEntry> {
        let feature_context = self
            .feature_context_from_view(from_view)
            .or_else(|| self.selected_feature_context());

        let mut commands = self.local_command_entries(from_view, feature_context.as_ref());

        if let Some(home) = dirs::home_dir() {
            let global_cmd_dir = home.join(".claude").join("commands");
            scan_commands_recursive(
                &global_cmd_dir,
                &global_cmd_dir,
                CommandSection::Global,
                &mut commands,
            );
        }

        let mut scanned_repo = false;
        if let Some(context) = &feature_context {
            let workdir_cmd_dir = context.workdir.join(".claude").join("commands");
            if workdir_cmd_dir.exists() {
                scan_commands_recursive(
                    &workdir_cmd_dir,
                    &workdir_cmd_dir,
                    CommandSection::Project,
                    &mut commands,
                );
                scanned_repo = true;
            }
        }

        if !scanned_repo && let Some(context) = &feature_context {
            let project_cmd_dir = context.repo.join(".claude").join("commands");
            scan_commands_recursive(
                &project_cmd_dir,
                &project_cmd_dir,
                CommandSection::Project,
                &mut commands,
            );
        } else if !scanned_repo
            && let Some(project) = self.selected_project()
        {
            let project_cmd_dir = project.repo.join(".claude").join("commands");
            scan_commands_recursive(
                &project_cmd_dir,
                &project_cmd_dir,
                CommandSection::Project,
                &mut commands,
            );
        }

        commands.sort_by(|a, b| {
            a.section
                .cmp(&b.section)
                .then(a.title.cmp(&b.title))
                .then(a.id.cmp(&b.id))
        });
        commands
    }

    fn local_command_entries(
        &self,
        from_view: Option<&ViewState>,
        feature_context: Option<&FeatureCommandContext>,
    ) -> Vec<CommandEntry> {
        let mut commands = vec![
            local_command_entry(
                LocalCommand::OpenDebugLog,
                CommandSection::AmfDebug,
                "Open Debug Log",
                Some("Open AMF's in-app debug log overlay"),
            ),
            local_command_entry(
                LocalCommand::ClearDebugLog,
                CommandSection::AmfDebug,
                "Clear Debug Log",
                Some("Clear AMF's in-memory debug log buffer"),
            ),
            local_command_entry(
                LocalCommand::RefreshNotifications,
                CommandSection::AmfDebug,
                "Refresh Notifications",
                Some("Re-sync statuses and notification state"),
            ),
        ];

        if feature_context.is_some() {
            commands.push(local_command_entry(
                LocalCommand::InjectTestInputRequest,
                CommandSection::AmfDebug,
                "Inject Test Input Request",
                Some("Add a synthetic pending input for the current feature"),
            ));
            commands.push(local_command_entry(
                LocalCommand::GenerateSummary,
                CommandSection::AmfDev,
                "Generate Summary",
                Some("Trigger summary generation for the selected feature"),
            ));
            commands.push(local_command_entry(
                LocalCommand::OpenPlanMarkdown,
                CommandSection::AmfDev,
                "Open Plan Markdown",
                Some("Open plan-focused markdown files for the current feature"),
            ));
        }

        if from_view.is_some() {
            commands.push(local_command_entry(
                LocalCommand::OpenLatestPrompt,
                CommandSection::AmfDev,
                "Open Latest Prompt",
                Some("Open prompt history for the current pane"),
            ));
        }

        commands
    }

    fn selected_feature_context(&self) -> Option<FeatureCommandContext> {
        let (project, feature) = self.selected_feature()?;
        Some(FeatureCommandContext {
            project_name: project.name.clone(),
            feature_name: feature.name.clone(),
            repo: project.repo.clone(),
            workdir: feature.workdir.clone(),
            tmux_session: feature.tmux_session.clone(),
        })
    }

    fn feature_context_from_view(&self, view: Option<&ViewState>) -> Option<FeatureCommandContext> {
        let view = view?;
        let project = self
            .store
            .projects
            .iter()
            .find(|project| project.name == view.project_name)?;
        let feature = project
            .features
            .iter()
            .find(|feature| feature.name == view.feature_name)?;
        Some(FeatureCommandContext {
            project_name: project.name.clone(),
            feature_name: feature.name.clone(),
            repo: project.repo.clone(),
            workdir: feature.workdir.clone(),
            tmux_session: feature.tmux_session.clone(),
        })
    }

    fn command_tmux_target(&self, from_view: Option<&ViewState>) -> Option<(String, String)> {
        if let Some(view) = from_view {
            return Some((view.session.clone(), view.window.clone()));
        }

        let (_, feature) = self.selected_feature()?;
        let window = feature
            .sessions
            .iter()
            .find(|session| {
                matches!(
                    session.kind,
                    SessionKind::Claude | SessionKind::Opencode | SessionKind::Codex
                )
            })
            .map(|session| session.tmux_window.clone())
            .unwrap_or_else(|| "terminal".into());
        Some((feature.tmux_session.clone(), window))
    }

    fn inject_test_input_request(&mut self, from_view: Option<&ViewState>) {
        let Some(context) = self
            .feature_context_from_view(from_view)
            .or_else(|| self.selected_feature_context())
        else {
            self.message = Some("Select a feature to inject a test input request".into());
            return;
        };

        self.pending_inputs.push(PendingInput {
            session_id: context.tmux_session.clone(),
            cwd: context.workdir.to_string_lossy().into_owned(),
            message: "Synthetic input request from AMF debug commands".into(),
            notification_type: "input-request".into(),
            file_path: PathBuf::new(),
            target_file_path: None,
            relative_path: None,
            change_id: None,
            tool: Some("amf-debug".into()),
            old_snippet: None,
            new_snippet: None,
            original_file: None,
            proposed_file: None,
            is_new_file: None,
            reason: Some("debug-command".into()),
            response_file: None,
            project_name: Some(context.project_name),
            feature_name: Some(context.feature_name),
            proceed_signal: None,
            request_id: None,
            reply_socket: None,
        });
        self.message = Some("Injected test input request".into());
    }
}

fn local_command_entry(
    command: LocalCommand,
    section: CommandSection,
    title: &str,
    description: Option<&str>,
) -> CommandEntry {
    CommandEntry {
        id: format!("local:{command:?}"),
        title: title.into(),
        description: description.map(str::to_string),
        section,
        action: CommandAction::Local { command },
        path: None,
    }
}

pub fn scan_commands_recursive(
    base: &Path,
    dir: &Path,
    section: CommandSection,
    out: &mut Vec<CommandEntry>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_commands_recursive(base, &path, section, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            let name = if let Ok(rel) = dir.strip_prefix(base)
                && !rel.as_os_str().is_empty()
            {
                format!("{}:{}", rel.to_string_lossy(), stem)
            } else {
                stem.to_string()
            };

            out.push(CommandEntry {
                id: format!(
                    "{}:{}",
                    match section {
                        CommandSection::Project => "project",
                        CommandSection::Global => "global",
                        CommandSection::AmfDebug => "amf-debug",
                        CommandSection::AmfDev => "amf-dev",
                    },
                    name
                ),
                title: name.clone(),
                description: None,
                section,
                action: CommandAction::SlashCommand { name },
                path: Some(path),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AgentKind, Feature, FeatureSession, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use mockall::predicate::eq;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn command_app() -> App {
        let mut project = Project::new(
            "demo".into(),
            PathBuf::from("/tmp/demo"),
            true,
            AgentKind::Claude,
        );
        let mut feature = Feature::new(
            "feature".into(),
            "feature".into(),
            PathBuf::from("/tmp/demo"),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
        );
        feature.sessions.push(FeatureSession {
            id: Uuid::new_v4().to_string(),
            label: "Claude".into(),
            tmux_window: "claude".into(),
            kind: SessionKind::Claude,
            claude_session_id: None,
            token_usage_source: None,
            created_at: chrono::Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
        });
        project.features.push(feature);

        let store = ProjectStore {
            version: 5,
            projects: vec![project],
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

    fn command_view() -> ViewState {
        ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        )
    }

    #[test]
    fn local_commands_sort_above_project_and_global_commands() {
        let mut commands = vec![
            CommandEntry {
                id: "global:z".into(),
                title: "z".into(),
                description: None,
                section: CommandSection::Global,
                action: CommandAction::SlashCommand { name: "z".into() },
                path: Some(PathBuf::from("/tmp/global/z.md")),
            },
            CommandEntry {
                id: "project:a".into(),
                title: "a".into(),
                description: None,
                section: CommandSection::Project,
                action: CommandAction::SlashCommand { name: "a".into() },
                path: Some(PathBuf::from("/tmp/project/a.md")),
            },
            local_command_entry(
                LocalCommand::RefreshNotifications,
                CommandSection::AmfDebug,
                "Refresh Notifications",
                None,
            ),
            local_command_entry(
                LocalCommand::GenerateSummary,
                CommandSection::AmfDev,
                "Generate Summary",
                None,
            ),
        ];

        commands.sort_by(|a, b| {
            a.section
                .cmp(&b.section)
                .then(a.title.cmp(&b.title))
                .then(a.id.cmp(&b.id))
        });

        assert_eq!(commands[0].section, CommandSection::AmfDebug);
        assert_eq!(commands[1].section, CommandSection::AmfDev);
        assert_eq!(commands[2].section, CommandSection::Project);
        assert_eq!(commands[3].section, CommandSection::Global);
    }

    #[test]
    fn execute_local_inject_test_input_request_adds_pending_input() {
        let mut app = command_app();
        let entry = local_command_entry(
            LocalCommand::InjectTestInputRequest,
            CommandSection::AmfDebug,
            "Inject Test Input Request",
            None,
        );

        let outcome = app.execute_command_entry(&entry, None).unwrap();

        assert_eq!(outcome, CommandExecutionOutcome::ReturnToOrigin);
        assert_eq!(app.pending_inputs.len(), 1);
        assert_eq!(app.pending_inputs[0].notification_type, "input-request");
        assert_eq!(app.pending_inputs[0].project_name.as_deref(), Some("demo"));
        assert_eq!(app.pending_inputs[0].feature_name.as_deref(), Some("feature"));
    }

    #[test]
    fn execute_local_open_debug_log_enters_debug_mode() {
        let mut app = command_app();
        let entry = local_command_entry(
            LocalCommand::OpenDebugLog,
            CommandSection::AmfDebug,
            "Open Debug Log",
            None,
        );
        let view = command_view();

        let outcome = app
            .execute_command_entry(&entry, Some(view.clone()))
            .unwrap();

        assert_eq!(outcome, CommandExecutionOutcome::KeepCurrentMode);
        match &app.mode {
            AppMode::DebugLog(state) => {
                assert_eq!(state.scroll_offset, 0);
                assert_eq!(state.from_view.as_ref().map(|v| &v.window), Some(&view.window));
            }
            _ => panic!("expected debug log mode"),
        }
    }

    #[test]
    fn execute_slash_command_uses_tmux_trait() {
        let mut app = command_app();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_send_literal()
            .with(eq("amf-feature"), eq("claude"), eq("/deploy"))
            .returning(|_, _, _| Ok(()));
        tmux.expect_send_key_name()
            .with(eq("amf-feature"), eq("claude"), eq("Enter"))
            .returning(|_, _, _| Ok(()));
        app.tmux = Box::new(tmux);

        let entry = CommandEntry {
            id: "project:deploy".into(),
            title: "deploy".into(),
            description: None,
            section: CommandSection::Project,
            action: CommandAction::SlashCommand {
                name: "deploy".into(),
            },
            path: Some(PathBuf::from("/tmp/demo/.claude/commands/deploy.md")),
        };

        let outcome = app.execute_command_entry(&entry, None).unwrap();

        assert_eq!(outcome, CommandExecutionOutcome::ReturnToOrigin);
        assert_eq!(app.message.as_deref(), Some("Sent '/deploy'"));
    }

    #[test]
    fn open_command_picker_adds_view_only_latest_prompt_command() {
        let mut app = command_app();
        app.open_command_picker(Some(command_view()));

        let state = match &app.mode {
            AppMode::CommandPicker(state) => state,
            _ => panic!("expected command picker"),
        };

        assert!(state.commands.iter().any(|entry| matches!(
            entry.action,
            CommandAction::Local {
                command: LocalCommand::OpenLatestPrompt
            }
        )));
        assert!(state.commands.iter().any(|entry| matches!(
            entry.action,
            CommandAction::Local {
                command: LocalCommand::InjectTestInputRequest
            }
        )));
    }
}
