use std::path::Path;

use super::*;

impl App {
    pub fn command_picker_codex_target(&self, from_view: Option<&ViewState>) -> Option<String> {
        if let Some(view) = from_view {
            return (view.session_kind == SessionKind::Codex).then(|| view.session.clone());
        }

        self.selected_feature().and_then(|(_, feature)| {
            feature
                .sessions
                .iter()
                .any(|session| session.kind == SessionKind::Codex)
                .then(|| feature.tmux_session.clone())
        })
    }

    pub fn open_command_picker(&mut self, from_view: Option<ViewState>) {
        let (repo, workdir) = match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => {
                let p = self.store.projects.get(*pi);
                (
                    p.map(|p| p.repo.clone()),
                    p.and_then(|p| p.features.get(*fi).map(|f| f.workdir.clone())),
                )
            }
            Selection::Project(pi) => (self.store.projects.get(*pi).map(|p| p.repo.clone()), None),
        };

        let mut commands = if self
            .command_picker_codex_target(from_view.as_ref())
            .is_some()
        {
            codex_debug_commands()
        } else {
            Vec::new()
        };

        let mut global_cmds = Vec::new();
        if let Some(home) = dirs::home_dir() {
            let global_cmd_dir = home.join(".claude").join("commands");
            scan_commands_recursive(&global_cmd_dir, &global_cmd_dir, "Global", &mut global_cmds);
        }
        global_cmds.sort_by(|a, b| a.name.cmp(&b.name));
        commands.extend(global_cmds);

        let mut project_cmds = Vec::new();
        let mut scanned_repo = false;
        if let Some(ref wd) = workdir {
            let workdir_cmd_dir = wd.join(".claude").join("commands");
            if workdir_cmd_dir.exists() {
                scan_commands_recursive(
                    &workdir_cmd_dir,
                    &workdir_cmd_dir,
                    "Project",
                    &mut project_cmds,
                );
                scanned_repo = true;
            }
        }

        if !scanned_repo && let Some(ref repo) = repo {
            let project_cmd_dir = repo.join(".claude").join("commands");
            scan_commands_recursive(
                &project_cmd_dir,
                &project_cmd_dir,
                "Project",
                &mut project_cmds,
            );
        }

        project_cmds.sort_by(|a, b| a.name.cmp(&b.name));
        commands.extend(project_cmds);

        self.mode = AppMode::CommandPicker(CommandPickerState {
            commands,
            selected: 0,
            from_view,
        });
    }
}

pub fn scan_commands_recursive(base: &Path, dir: &Path, source: &str, out: &mut Vec<CommandEntry>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_commands_recursive(base, &path, source, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("md")
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            let name = if let Ok(rel) = dir.strip_prefix(base)
                && !rel.as_os_str().is_empty()
            {
                format!("{}:{}", rel.to_string_lossy(), stem,)
            } else {
                stem.to_string()
            };

            out.push(CommandEntry {
                name,
                source: source.into(),
                path: Some(path),
                action: CommandAction::SlashCommand,
            });
        }
    }
}

fn codex_debug_commands() -> Vec<CommandEntry> {
    vec![
        codex_debug_command("demo-plan", CodexDebugCommand::PlanDemo),
        codex_debug_command("demo-work-command", CodexDebugCommand::WorkCommandDemo),
        codex_debug_command("demo-work-file", CodexDebugCommand::WorkFileDemo),
        codex_debug_command("demo-work-input", CodexDebugCommand::WorkInputDemo),
        codex_debug_command("demo-clear-input", CodexDebugCommand::ClearInputDemo),
    ]
}

fn codex_debug_command(name: &str, action: CodexDebugCommand) -> CommandEntry {
    CommandEntry {
        name: name.to_string(),
        source: "AMF Debug".to_string(),
        path: None,
        action: CommandAction::CodexLiveDemo(action),
    }
}
