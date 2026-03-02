use anyhow::Result;

use super::*;
use super::setup::{ensure_notification_hooks, ensure_review_claude_md};
use super::util::slugify;
use crate::tmux::TmuxManager;

impl App {
    /// Open the custom session picker for the currently
    pub fn open_session_picker(&mut self) -> Result<()> {
        use crate::app::BuiltinSessionOption;
        use crate::app::SessionPickerState;

        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
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

        let feature = self.store.projects[pi].features[fi].clone();
        let agent = feature.agent.clone();

        let vscode_available = std::process::Command::new("code")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();

        let builtin_sessions = vec![
            BuiltinSessionOption {
                kind: SessionKind::Claude,
                label: match agent {
                    AgentKind::Claude => "Claude".to_string(),
                    AgentKind::Opencode => "Opencode (Claude)".to_string(),
                },
                disabled: None,
            },
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
        ];

        let custom_sessions =
            self.active_extension.custom_sessions.clone();

        let total_sessions = builtin_sessions.len() + custom_sessions.len();
        if total_sessions == 0 {
            self.message =
                Some("No sessions available".into());
            return Ok(());
        }

        let from_view = if let AppMode::Viewing(ref view) = self.mode {
            Some((*view).clone())
        } else {
            None
        };

        self.mode = AppMode::SessionPicker(SessionPickerState {
            builtin_sessions,
            custom_sessions,
            selected: 0,
            pi,
            fi,
            from_view,
        });
        Ok(())
    }

    pub fn open_session_picker_from_switcher(
        &mut self,
    ) -> Result<()> {
        use crate::app::{BuiltinSessionOption, SessionPickerState};

        let (project_name, feature_name) = match &self.mode {
            AppMode::SessionSwitcher(state) => (
                state.project_name.clone(),
                state.feature_name.clone(),
            ),
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

        let feature =
            self.store.projects[pi].features[fi].clone();
        let agent = feature.agent.clone();

        let vscode_available = std::process::Command::new("code")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok();

        let builtin_sessions = vec![
            BuiltinSessionOption {
                kind: SessionKind::Claude,
                label: match agent {
                    AgentKind::Claude => {
                        "Claude".to_string()
                    }
                    AgentKind::Opencode => {
                        "Opencode (Claude)".to_string()
                    }
                },
                disabled: None,
            },
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
        ];

        let custom_sessions =
            self.active_extension.custom_sessions.clone();

        self.mode = AppMode::SessionPicker(SessionPickerState {
            builtin_sessions,
            custom_sessions,
            selected: 0,
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
    ) -> Result<()> {
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
            None => return Ok(()),
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
        );
        let window = session.tmux_window.clone();
        let command = session.command.clone();

        if TmuxManager::session_exists(&tmux_session) {
            TmuxManager::create_window(
                &tmux_session,
                &window,
                &workdir,
            )?;
            if let Some(ref cmd) = command {
                TmuxManager::send_literal(
                    &tmux_session,
                    &window,
                    cmd,
                )?;
                TmuxManager::send_key_name(
                    &tmux_session,
                    &window,
                    "Enter",
                )?;
            }
        }

        self.save()?;
        Ok(())
    }

    pub fn add_builtin_session(
        &mut self,
        pi: usize,
        fi: usize,
        kind: SessionKind,
    ) -> Result<()> {
        match kind {
            SessionKind::Terminal => {
                self.add_terminal_session_for_picker(pi, fi)
            }
            SessionKind::Nvim => {
                self.add_nvim_session_for_picker(pi, fi)
            }
            SessionKind::Claude => {
                self.add_claude_session_for_picker(pi, fi)
            }
            SessionKind::Vscode => {
                self.add_vscode_session_for_picker(pi, fi)
            }
            _ => {
                self.message =
                    Some("Unsupported session type".into());
                Ok(())
            }
        }
    }

    fn add_terminal_session_for_picker(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
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
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = feature.add_session(SessionKind::Terminal);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;

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
            self.message = Some(
                "Error: nvim is not installed".into(),
            );
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
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = feature.add_session(SessionKind::Nvim);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        TmuxManager::send_keys(
            &tmux_session,
            &window,
            "nvim",
        )?;

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
            self.message = Some(
                "Error: code (VSCode CLI) is not installed".into(),
            );
            return Ok(());
        }

        let feature = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        let workdir = feature.workdir.clone();
        std::process::Command::new("code")
            .arg(&workdir)
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to launch VSCode: {}", e))?;

        self.message = Some(format!("Opened VSCode in {}", workdir.display()));

        Ok(())
    }

    fn add_claude_session_for_picker(&mut self, pi: usize, fi: usize) -> Result<()> {
        let repo = self.store.projects[pi].repo.clone();

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
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let mode = feature.mode.clone();
        let extra_args: Vec<String> =
            feature.mode.cli_flags(feature.enable_chrome);
        let agent = feature.agent.clone();
        ensure_notification_hooks(
            &workdir,
            &repo,
            &mode,
            &agent,
        );
        ensure_review_claude_md(&workdir, feature.review);
        let session_kind = match feature.agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Opencode => SessionKind::Opencode,
        };
        let session = feature.add_session(session_kind);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        let extra_refs: Vec<&str> =
            extra_args.iter().map(|s| s.as_str()).collect();
        match feature.agent {
            AgentKind::Claude => {
                TmuxManager::launch_claude(
                    &tmux_session,
                    &window,
                    None,
                    &extra_refs,
                )?;
            }
            AgentKind::Opencode => {
                TmuxManager::launch_opencode(
                    &tmux_session,
                    &window,
                )?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn add_terminal_session(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
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

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session =
            feature.add_session(SessionKind::Terminal);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn add_nvim_session(&mut self) -> Result<()> {
        if std::process::Command::new("nvim")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            self.message = Some(
                "Error: nvim is not installed".into(),
            );
            return Ok(());
        }

        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
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

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session =
            feature.add_session(SessionKind::Nvim);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        TmuxManager::send_keys(
            &tmux_session,
            &window,
            "nvim",
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn create_memo(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
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

        if feature.has_notes {
            self.message =
                Some("Memo already exists".into());
            return Ok(());
        }

        let claude_dir = feature.workdir.join(".claude");
        if !claude_dir.exists() {
            let _ = std::fs::create_dir_all(&claude_dir);
        }
        let notes_path = claude_dir.join("notes.md");
        if !notes_path.exists() {
            let _ = std::fs::write(
                &notes_path,
                "# Notes\n\nWrite instructions for Claude here.\n",
            );
        }

        feature.has_notes = true;

        if TmuxManager::session_exists(
            &feature.tmux_session,
        ) {
            let workdir = feature.workdir.clone();
            let tmux_session =
                feature.tmux_session.clone();
            let session =
                feature.add_session(SessionKind::Nvim);
            session.label = "Memo".into();
            let window = session.tmux_window.clone();

            TmuxManager::create_window(
                &tmux_session,
                &window,
                &workdir,
            )?;
            TmuxManager::send_keys(
                &tmux_session,
                &window,
                "nvim .claude/notes.md",
            )?;

            feature.collapsed = false;
        }

        self.save()?;
        self.message = Some("Created memo".into());

        Ok(())
    }

    pub fn add_claude_session(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let repo =
            self.store.projects[pi].repo.clone();

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let mode = feature.mode.clone();
        let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
        let agent = feature.agent.clone();
        ensure_notification_hooks(
            &workdir,
            &repo,
            &mode,
            &agent,
        );
        ensure_review_claude_md(&workdir, feature.review);
        let session_kind = match feature.agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Opencode => SessionKind::Opencode,
        };
        let session = feature.add_session(session_kind);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        let extra_refs: Vec<&str> =
            extra_args.iter().map(|s| s.as_str()).collect();
        match feature.agent {
            AgentKind::Claude => {
                TmuxManager::launch_claude(
                    &tmux_session,
                    &window,
                    None,
                    &extra_refs,
                )?;
            }
            AgentKind::Opencode => {
                TmuxManager::launch_opencode(
                    &tmux_session,
                    &window,
                )?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn remove_session(&mut self) -> Result<()> {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, *si)
            }
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
        let session = match feature.sessions.get(si) {
            Some(s) => s,
            None => return Ok(()),
        };
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        if TmuxManager::session_exists(&tmux_session) {
            let _ = TmuxManager::kill_window(
                &tmux_session,
                &window,
            );
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
