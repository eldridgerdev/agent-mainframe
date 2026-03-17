use anyhow::Result;

use super::setup::{ensure_notification_hooks, ensure_review_claude_md};
use super::util::slugify;
use super::*;
use crate::tmux::TmuxManager;

fn session_kind_for_agent(agent: &AgentKind) -> SessionKind {
    match agent {
        AgentKind::Claude => SessionKind::Claude,
        AgentKind::Opencode => SessionKind::Opencode,
        AgentKind::Codex => SessionKind::Codex,
    }
}

fn label_for_agent(agent: &AgentKind) -> String {
    match agent {
        AgentKind::Claude => "Claude".to_string(),
        AgentKind::Opencode => "Opencode".to_string(),
        AgentKind::Codex => "Codex".to_string(),
    }
}

fn agent_for_session_kind(kind: &SessionKind) -> Option<AgentKind> {
    match kind {
        SessionKind::Claude => Some(AgentKind::Claude),
        SessionKind::Opencode => Some(AgentKind::Opencode),
        SessionKind::Codex => Some(AgentKind::Codex),
        _ => None,
    }
}

impl App {
    /// Open the custom session picker for the currently
    pub fn open_session_picker(&mut self) -> Result<()> {
        use crate::app::BuiltinSessionOption;
        use crate::app::SessionPickerState;

        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => {
                self.message = Some("Select a feature first".into());
                return Ok(());
            }
        };

        if self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .is_none()
        {
            return Ok(());
        }

        if self.block_if_feature_pending_worktree_script(pi, fi) {
            return Ok(());
        }

        self.reload_extension_config();

        let session_names: Vec<(usize, String)> = self
            .active_extension
            .custom_sessions
            .iter()
            .enumerate()
            .map(|(i, cs)| (i, cs.name.clone()))
            .collect();
        let sessions_count = session_names.len();

        self.log_debug(
            "session_picker",
            format!("Active custom sessions count: {}", sessions_count),
        );
        for (i, name) in session_names {
            self.log_debug("session_picker", format!("  [{}] {}", i, name));
        }

        let project = self.store.projects[pi].clone();

        let vscode_available = std::process::Command::new("code")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();

        let allowed_agents = self.allowed_agents_for_repo(&project.repo);
        let mut builtin_sessions: Vec<BuiltinSessionOption> = allowed_agents
            .iter()
            .map(|agent| BuiltinSessionOption {
                kind: session_kind_for_agent(agent),
                label: label_for_agent(agent),
                disabled: None,
            })
            .collect();

        builtin_sessions.extend(vec![
            BuiltinSessionOption {
                kind: SessionKind::Terminal,
                label: "Terminal".to_string(),
                disabled: None,
            },
            BuiltinSessionOption {
                kind: SessionKind::Nvim,
                label: "Neovim".to_string(),
                disabled: None,
            },
            BuiltinSessionOption {
                kind: SessionKind::Vscode,
                label: "VSCode".to_string(),
                disabled: if vscode_available {
                    None
                } else {
                    Some("code not found in PATH".to_string())
                },
            },
        ]);

        let custom_sessions = self.active_extension.custom_sessions.clone();

        let total_sessions = builtin_sessions.len() + custom_sessions.len();
        if total_sessions == 0 {
            self.message = Some("No sessions available".into());
            return Ok(());
        }

        let from_view = if let AppMode::Viewing(ref view) = self.mode {
            Some((*view).clone())
        } else {
            None
        };

        let selected = builtin_sessions
            .iter()
            .position(|session| session.kind == session_kind_for_agent(&project.preferred_agent))
            .unwrap_or(0);

        self.mode = AppMode::SessionPicker(SessionPickerState {
            builtin_sessions,
            custom_sessions,
            selected,
            pi,
            fi,
            from_view,
        });
        Ok(())
    }

    pub fn open_session_picker_from_switcher(&mut self) -> Result<()> {
        use crate::app::{BuiltinSessionOption, SessionPickerState};

        let (project_name, feature_name) = match &self.mode {
            AppMode::SessionSwitcher(state) => {
                (state.project_name.clone(), state.feature_name.clone())
            }
            _ => return Ok(()),
        };

        let pi = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name);
        let pi = match pi {
            Some(pi) => pi,
            None => return Ok(()),
        };

        let fi = self.store.projects[pi]
            .features
            .iter()
            .position(|f| f.name == feature_name);
        let fi = match fi {
            Some(fi) => fi,
            None => return Ok(()),
        };

        self.reload_extension_config();

        let project = self.store.projects[pi].clone();

        let vscode_available = std::process::Command::new("code")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();

        let allowed_agents = self.allowed_agents_for_repo(&project.repo);
        let mut builtin_sessions: Vec<BuiltinSessionOption> = allowed_agents
            .iter()
            .map(|agent| BuiltinSessionOption {
                kind: session_kind_for_agent(agent),
                label: label_for_agent(agent),
                disabled: None,
            })
            .collect();

        builtin_sessions.extend(vec![
            BuiltinSessionOption {
                kind: SessionKind::Terminal,
                label: "Terminal".to_string(),
                disabled: None,
            },
            BuiltinSessionOption {
                kind: SessionKind::Nvim,
                label: "Neovim".to_string(),
                disabled: None,
            },
            BuiltinSessionOption {
                kind: SessionKind::Vscode,
                label: "VSCode".to_string(),
                disabled: if vscode_available {
                    None
                } else {
                    Some("code not found in PATH".to_string())
                },
            },
        ]);

        let custom_sessions = self.active_extension.custom_sessions.clone();
        let selected = builtin_sessions
            .iter()
            .position(|session| session.kind == session_kind_for_agent(&project.preferred_agent))
            .unwrap_or(0);

        self.mode = AppMode::SessionPicker(SessionPickerState {
            builtin_sessions,
            custom_sessions,
            selected,
            pi,
            fi,
            from_view: None,
        });
        Ok(())
    }

    /// Add a custom session type as a tracked FeatureSession.
    /// If the feature's tmux session is already running, also
    /// creates the window and sends the command immediately.
    pub fn add_custom_session_type(
        &mut self,
        pi: usize,
        fi: usize,
        config: &crate::extension::CustomSessionConfig,
    ) -> Result<bool> {
        let window_hint = config
            .window_name
            .clone()
            .unwrap_or_else(|| slugify(&config.name));

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(false),
        };

        let tmux_session = feature.tmux_session.clone();
        let workdir = config
            .working_dir
            .as_ref()
            .map(|rel| feature.workdir.join(rel))
            .unwrap_or_else(|| feature.workdir.clone());

        let session = feature.add_custom_session_named(
            config.name.clone(),
            window_hint,
            config.command.clone(),
            config.on_stop.clone(),
            config.pre_check.clone(),
        );
        let session_id = session.id.clone();
        let window = session.tmux_window.clone();
        let command = session.command.clone();

        if TmuxManager::session_exists(&tmux_session) {
            TmuxManager::create_window(&tmux_session, &window, &workdir)?;

            // Set up status directory and env vars for
            // the custom session, wrapped via env+bash
            // for shell portability (fish, zsh, etc.)
            let status_dir = workdir.join(".amf").join("session-status");
            let _ = std::fs::create_dir_all(&status_dir);

            let status_dir_str = status_dir.to_string_lossy().into_owned();
            let env_prefix = TmuxManager::shell_env_prefix(&[
                ("AMF_SESSION_ID", &session_id),
                ("AMF_STATUS_DIR", &status_dir_str),
            ]);
            let shell_cmd = if let Some(ref cmd) = command {
                format!("{} bash -c '{}'", env_prefix, cmd.replace('\'', "'\\''"),)
            } else {
                env_prefix
            };
            TmuxManager::send_literal(&tmux_session, &window, &shell_cmd)?;
            TmuxManager::send_key_name(&tmux_session, &window, "Enter")?;
        }

        self.save()?;
        Ok(config.autolaunch.unwrap_or(false))
    }

    pub fn add_builtin_session(&mut self, pi: usize, fi: usize, kind: SessionKind) -> Result<()> {
        match kind {
            SessionKind::Terminal => self.add_terminal_session_for_picker(pi, fi),
            SessionKind::Nvim => self.add_nvim_session_for_picker(pi, fi),
            SessionKind::Claude | SessionKind::Opencode | SessionKind::Codex => {
                self.add_agent_session_for_picker(pi, fi, kind)
            }
            SessionKind::Vscode => self.add_vscode_session_for_picker(pi, fi),
            _ => {
                self.message = Some("Unsupported session type".into());
                Ok(())
            }
        }
    }

    fn add_terminal_session_for_picker(&mut self, pi: usize, fi: usize) -> Result<()> {
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session) {
            self.message = Some("Error: Feature must be running to add a session".into());
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = feature.add_session(SessionKind::Terminal);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(&tmux_session, &window, &workdir)?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    fn add_nvim_session_for_picker(&mut self, pi: usize, fi: usize) -> Result<()> {
        if std::process::Command::new("nvim")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            self.message = Some("Error: nvim is not installed".into());
            return Ok(());
        }

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session) {
            self.message = Some("Error: Feature must be running to add a session".into());
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = feature.add_session(SessionKind::Nvim);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(&tmux_session, &window, &workdir)?;
        TmuxManager::send_keys(&tmux_session, &window, "nvim")?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    fn add_vscode_session_for_picker(&mut self, pi: usize, fi: usize) -> Result<()> {
        if std::process::Command::new("code")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            self.message = Some("Error: code (VSCode CLI) is not installed".into());
            return Ok(());
        }

        let feature = match self.store.projects.get(pi).and_then(|p| p.features.get(fi)) {
            Some(f) => f,
            None => return Ok(()),
        };

        let workdir = feature.workdir.clone();
        std::process::Command::new("code")
            .arg(&workdir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to launch VSCode: {}", e))?;

        self.message = Some(format!("Opened VSCode in {}", workdir.display()));

        Ok(())
    }

    fn add_agent_session_for_picker(
        &mut self,
        pi: usize,
        fi: usize,
        kind: SessionKind,
    ) -> Result<()> {
        let repo = self.store.projects[pi].repo.clone();
        let Some(agent) = agent_for_session_kind(&kind) else {
            self.message = Some("Unsupported agent session type".into());
            return Ok(());
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session) {
            self.message = Some("Error: Feature must be running to add a session".into());
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let mode = feature.mode.clone();
        let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
        ensure_notification_hooks(&workdir, &repo, &mode, &agent, feature.is_worktree);
        ensure_review_claude_md(&workdir, feature.review);
        let session = feature.add_session(kind.clone());
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(&tmux_session, &window, &workdir)?;
        let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
        match agent {
            AgentKind::Claude => {
                TmuxManager::launch_claude(&tmux_session, &window, None, &extra_refs)?;
            }
            AgentKind::Opencode => {
                TmuxManager::launch_opencode(&tmux_session, &window)?;
            }
            AgentKind::Codex => {
                TmuxManager::launch_codex(&tmux_session, &window, None)?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn add_claude_session(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };
        let Some(kind) = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| session_kind_for_agent(&f.agent))
        else {
            return Ok(());
        };
        self.add_agent_session_for_picker(pi, fi, kind)
    }

    pub fn remove_session(&mut self) -> Result<()> {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => (*pi, *fi, *si),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        let tmux_session = feature.tmux_session.clone();
        let workdir = feature.workdir.clone();
        let session = match feature.sessions.get(si) {
            Some(s) => s,
            None => return Ok(()),
        };
        let window = session.tmux_window.clone();
        let label = session.label.clone();
        let on_stop = session.on_stop.clone();
        let session_id = session.id.clone();
        let is_custom = session.kind == SessionKind::Custom;

        // Run on_stop command for custom sessions before
        // killing the window.
        if is_custom {
            if let Some(ref cmd) = on_stop {
                let _ = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .current_dir(&workdir)
                    .env("AMF_SESSION_ID", &session_id)
                    .env(
                        "AMF_STATUS_DIR",
                        workdir.join(".amf").join("session-status"),
                    )
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }

            // Clean up status file.
            let status_file = workdir
                .join(".amf")
                .join("session-status")
                .join(format!("{}.txt", session_id));
            let _ = std::fs::remove_file(status_file);
        }

        if TmuxManager::session_exists(&tmux_session) {
            let _ = TmuxManager::kill_window(&tmux_session, &window);
        }

        feature.sessions.remove(si);

        if feature.sessions.is_empty() {
            let _ = TmuxManager::kill_session(&tmux_session);
            feature.status = ProjectStatus::Stopped;
        }

        self.selection = Selection::Feature(pi, fi);
        self.save()?;
        self.message = Some(format!("Removed '{}'", label));

        Ok(())
    }
}
