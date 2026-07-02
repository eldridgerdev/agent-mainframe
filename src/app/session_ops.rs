use anyhow::Result;

use super::setup::{ensure_notification_hooks, ensure_review_claude_md};
use super::util::slugify;
use super::*;
use crate::project::LaunchOpts;
use crate::tmux::TmuxManager;

fn session_kind_for_agent(agent: &AgentKind) -> SessionKind {
    match agent {
        AgentKind::Claude => SessionKind::Claude,
        AgentKind::Opencode => SessionKind::Opencode,
        AgentKind::Codex => SessionKind::Codex,
        AgentKind::Pi => SessionKind::Pi,
    }
}

fn label_for_agent(agent: &AgentKind) -> String {
    match agent {
        AgentKind::Claude => "Claude".to_string(),
        AgentKind::Opencode => "Opencode".to_string(),
        AgentKind::Codex => "Codex".to_string(),
        AgentKind::Pi => "Pi".to_string(),
    }
}

fn agent_for_session_kind(kind: &SessionKind) -> Option<AgentKind> {
    match kind {
        SessionKind::Claude => Some(AgentKind::Claude),
        SessionKind::Opencode => Some(AgentKind::Opencode),
        SessionKind::Codex => Some(AgentKind::Codex),
        SessionKind::Pi => Some(AgentKind::Pi),
        _ => None,
    }
}

impl App {
    pub(crate) fn ensure_feature_running_for_new_session(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        if self.block_if_feature_pending_worktree_script(pi, fi) {
            anyhow::bail!("feature cannot start while its worktree script is still running");
        }

        let tmux_session = self
            .store
            .projects
            .get(pi)
            .and_then(|project| project.features.get(fi))
            .map(|feature| feature.tmux_session.clone())
            .ok_or_else(|| anyhow::anyhow!("feature not found"))?;

        if !self.tmux.session_exists(&tmux_session) {
            self.ensure_feature_running(pi, fi)?;
        }

        if !self.tmux.session_exists(&tmux_session) {
            anyhow::bail!("failed to start feature session");
        }

        if let Some(feature) = self
            .store
            .projects
            .get_mut(pi)
            .and_then(|project| project.features.get_mut(fi))
        {
            feature.status = ProjectStatus::Idle;
            feature.touch();
        }
        self.save()?;
        Ok(())
    }

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

        let vscode_available = self.vscode_available;

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

        // At most one TODOs session per project; only offer it when none exists.
        if !project.has_todos_session() {
            builtin_sessions.push(BuiltinSessionOption {
                kind: SessionKind::Todos,
                label: "TODOs".to_string(),
                disabled: None,
            });
        }

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

        let vscode_available = self.vscode_available;

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

        // At most one TODOs session per project; only offer it when none exists.
        if !project.has_todos_session() {
            builtin_sessions.push(BuiltinSessionOption {
                kind: SessionKind::Todos,
                label: "TODOs".to_string(),
                disabled: None,
            });
        }

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

    pub fn add_custom_session_type_named(
        &mut self,
        pi: usize,
        fi: usize,
        config: &crate::extension::CustomSessionConfig,
        label: String,
    ) -> Result<bool> {
        self.ensure_feature_running_for_new_session(pi, fi)?;

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
            None => anyhow::bail!("feature not found"),
        };

        let tmux_session = feature.tmux_session.clone();
        let workdir = config
            .working_dir
            .as_ref()
            .map(|rel| feature.workdir.join(rel))
            .unwrap_or_else(|| feature.workdir.clone());

        let session = feature.add_custom_session_named(
            label,
            window_hint,
            config.command.clone(),
            config.on_stop.clone(),
            config.pre_check.clone(),
        );
        let session_id = session.id.clone();
        let window = session.tmux_window.clone();
        let command = session.command.clone();

        if self.tmux.session_exists(&tmux_session) {
            self.tmux.create_window(&tmux_session, &window, &workdir)?;

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
            self.tmux.send_literal(&tmux_session, &window, &shell_cmd)?;
            self.tmux.send_key_name(&tmux_session, &window, "Enter")?;
        }

        self.save()?;
        Ok(config.autolaunch.unwrap_or(false))
    }

    pub fn add_builtin_session(&mut self, pi: usize, fi: usize, kind: SessionKind) -> Result<()> {
        self.add_builtin_session_named(pi, fi, kind, None)
    }

    pub fn add_builtin_session_with_label(
        &mut self,
        pi: usize,
        fi: usize,
        kind: SessionKind,
        label: String,
    ) -> Result<()> {
        self.add_builtin_session_named(pi, fi, kind, Some(label))
    }

    fn add_builtin_session_named(
        &mut self,
        pi: usize,
        fi: usize,
        kind: SessionKind,
        label: Option<String>,
    ) -> Result<()> {
        // TODOs is a native overlay with no tmux window, so it must not force
        // the host feature's tmux session to start.
        if kind == SessionKind::Todos {
            return self.add_todos_session_for_picker(pi, fi, label);
        }

        self.ensure_feature_running_for_new_session(pi, fi)?;

        match kind {
            SessionKind::Terminal => self.add_terminal_session_for_picker(pi, fi, label),
            SessionKind::Nvim => self.add_nvim_session_for_picker(pi, fi, label),
            SessionKind::Claude | SessionKind::Opencode | SessionKind::Codex | SessionKind::Pi => {
                self.add_agent_session_for_picker(pi, fi, kind, label)
            }
            SessionKind::Vscode => self.add_vscode_session_for_picker(pi, fi),
            _ => anyhow::bail!("unsupported session type"),
        }
    }

    /// Add a native TODOs session under the given feature and create the
    /// project's `todo_lists` row. Enforces one TODOs session per project.
    /// No tmux window is created.
    pub(crate) fn add_todos_session_for_picker(
        &mut self,
        pi: usize,
        fi: usize,
        label: Option<String>,
    ) -> Result<()> {
        let project = match self.store.projects.get(pi) {
            Some(p) => p,
            None => anyhow::bail!("project not found"),
        };
        if project.has_todos_session() {
            self.message = Some("This project already has a TODOs session".into());
            return Ok(());
        }
        let project_id = project.id.clone();

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => anyhow::bail!("feature not found"),
        };

        let feature_id = feature.id.clone();
        let label = label.unwrap_or_else(|| "TODOs".to_string());
        let session = feature.add_session_named(SessionKind::Todos, label);
        let label = session.label.clone();
        feature.collapsed = false;
        let si = feature.sessions.len() - 1;

        // Create the per-project todo list hosted by this feature. (The
        // `feature` borrow above has ended, so `self.db` is free to access.)
        let list_err = match &self.db {
            Some(db) => db.load_or_create_todo_list(&project_id, &feature_id).err(),
            None => None,
        };
        if let Some(e) = list_err {
            self.log_warn(
                "todos",
                format!("failed to create todo list for project {project_id}: {e}"),
            );
        }

        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    fn add_terminal_session_for_picker(
        &mut self,
        pi: usize,
        fi: usize,
        label: Option<String>,
    ) -> Result<()> {
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => anyhow::bail!("feature not found"),
        };

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = match label {
            Some(label) => feature.add_session_named(SessionKind::Terminal, label),
            None => feature.add_session(SessionKind::Terminal),
        };
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        self.tmux.create_window(&tmux_session, &window, &workdir)?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    fn add_nvim_session_for_picker(
        &mut self,
        pi: usize,
        fi: usize,
        label: Option<String>,
    ) -> Result<()> {
        if std::process::Command::new("nvim")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            anyhow::bail!("nvim is not installed");
        }

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => anyhow::bail!("feature not found"),
        };

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = match label {
            Some(label) => feature.add_session_named(SessionKind::Nvim, label),
            None => feature.add_session(SessionKind::Nvim),
        };
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        self.tmux.create_window(&tmux_session, &window, &workdir)?;
        self.tmux.send_keys(&tmux_session, &window, "nvim")?;

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
            None => anyhow::bail!("feature not found"),
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
        label: Option<String>,
    ) -> Result<()> {
        let repo = self.store.projects[pi].repo.clone();
        let project_name = self.store.projects[pi].name.clone();
        let Some(agent) = agent_for_session_kind(&kind) else {
            anyhow::bail!("unsupported agent session type");
        };

        // Resolve before the mutable borrow of `feature` below.
        let rc_allowed = self.remote_control_allowed();

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => anyhow::bail!("feature not found"),
        };

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let feature_name = feature.name.clone();
        let mode = feature.mode.clone();
        let review = feature.review;
        let use_rc = feature.remote_control && rc_allowed;
        let extra_args: Vec<String> = feature.mode.cli_flags(LaunchOpts {
            enable_chrome: feature.enable_chrome,
            remote_control: use_rc,
            session_name: if use_rc {
                Some(feature.name.clone())
            } else {
                None
            },
        });
        ensure_notification_hooks(&workdir, &repo, &mode, &agent, feature.is_worktree);
        ensure_review_claude_md(&workdir, feature.review);
        let session = match label {
            Some(label) => feature.add_session_named(kind.clone(), label),
            None => feature.add_session(kind.clone()),
        };
        let session_id = session.id.clone();
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        self.tmux.create_window(&tmux_session, &window, &workdir)?;
        match agent {
            AgentKind::Claude => {
                self.tmux
                    .launch_claude(&tmux_session, &window, &session_id, None, extra_args)?;
            }
            AgentKind::Opencode => {
                self.tmux
                    .launch_opencode(&tmux_session, &window, &session_id)?;
            }
            AgentKind::Codex => {
                let codex_args = crate::codex_config::launch_override_args(&workdir);
                self.tmux
                    .launch_codex(&tmux_session, &window, &session_id, None, codex_args)?;
            }
            AgentKind::Pi => {
                self.tmux.launch_pi(&tmux_session, &window, &session_id)?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        let mut view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            window,
            label.clone(),
            kind,
            mode,
            review,
        );
        view.show_startup_mask();
        self.mode = AppMode::Viewing(view);
        self.pane_content.clear();
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    /// Spin up the dedicated PR-review agent session: one agent window, labeled
    /// [`crate::app::pr_review::REVIEW_SESSION_LABEL`], that the PR-review pane
    /// reuses for every fix in a PR. Runs `harness` (the harness the user picked
    /// for the review session) or falls back to the project's preferred agent,
    /// with the feature's mode/flags — just like a picker-launched agent session,
    /// but with a fixed label so it can be found-and-reused. Returns the new
    /// session's index in `feature.sessions`.
    pub(crate) fn create_dedicated_review_session(
        &mut self,
        pi: usize,
        fi: usize,
        label: &str,
        harness: Option<AgentKind>,
    ) -> Result<usize> {
        self.create_agent_session_labeled(pi, fi, label, harness)
    }

    /// Create an agent-harness session in `(pi, fi)` with a caller-provided
    /// `label`, running `harness` (or the project's preferred agent when
    /// `None`) with the feature's mode/flags — the same launch path a
    /// picker-launched agent session uses. Unlike the picker path this leaves
    /// `self.selection` and `self.message` untouched, so callers can route the
    /// new session wherever they need. Returns the new session's index in
    /// `feature.sessions`.
    pub(crate) fn create_agent_session_labeled(
        &mut self,
        pi: usize,
        fi: usize,
        label: &str,
        harness: Option<AgentKind>,
    ) -> Result<usize> {
        self.ensure_feature_running_for_new_session(pi, fi)?;

        let repo = self.store.projects[pi].repo.clone();
        // The caller may pin a harness (e.g. the final review prompts for one);
        // otherwise fall back to the project's preferred agent.
        let agent = harness.unwrap_or_else(|| self.store.projects[pi].preferred_agent.clone());
        let kind = session_kind_for_agent(&agent);

        // Resolve before the mutable borrow of `feature` below.
        let rc_allowed = self.remote_control_allowed();

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => anyhow::bail!("feature not found"),
        };

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let mode = feature.mode.clone();
        let use_rc = feature.remote_control && rc_allowed;
        let extra_args: Vec<String> = feature.mode.cli_flags(LaunchOpts {
            enable_chrome: feature.enable_chrome,
            remote_control: use_rc,
            session_name: if use_rc {
                Some(feature.name.clone())
            } else {
                None
            },
        });
        ensure_notification_hooks(&workdir, &repo, &mode, &agent, feature.is_worktree);
        ensure_review_claude_md(&workdir, feature.review);

        let session = feature.add_session_named(kind.clone(), label.to_string());
        let session_id = session.id.clone();
        let window = session.tmux_window.clone();

        self.tmux.create_window(&tmux_session, &window, &workdir)?;
        match agent {
            AgentKind::Claude => {
                self.tmux
                    .launch_claude(&tmux_session, &window, &session_id, None, extra_args)?;
            }
            AgentKind::Opencode => {
                self.tmux
                    .launch_opencode(&tmux_session, &window, &session_id)?;
            }
            AgentKind::Codex => {
                let codex_args = crate::codex_config::launch_override_args(&workdir);
                self.tmux
                    .launch_codex(&tmux_session, &window, &session_id, None, codex_args)?;
            }
            AgentKind::Pi => {
                self.tmux.launch_pi(&tmux_session, &window, &session_id)?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.save()?;
        Ok(si)
    }

    pub fn remove_session(&mut self) -> Result<()> {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => (*pi, *fi, *si),
            _ => return Ok(()),
        };

        let (tmux_session, workdir, label, on_stop, session_id, is_custom, clear_sidebar) = {
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

            if TmuxManager::session_exists(&tmux_session) {
                let _ = TmuxManager::kill_window(&tmux_session, &window);
            }

            feature.sessions.remove(si);

            let clear_sidebar = if feature.sessions.is_empty() {
                let _ = TmuxManager::kill_session(&tmux_session);
                feature.status = ProjectStatus::Stopped;
                true
            } else {
                false
            };

            (
                tmux_session,
                workdir,
                label,
                on_stop,
                session_id,
                is_custom,
                clear_sidebar,
            )
        };

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

            // Clean up status file and DB entry.
            let status_file = workdir
                .join(".amf")
                .join("session-status")
                .join(format!("{}.txt", session_id));
            let _ = std::fs::remove_file(status_file);
            if let Some(ref db) = self.db {
                let _ = db.delete_session_status(&session_id);
            }
        }

        if clear_sidebar {
            self.clear_sidebar_state_for_session(&tmux_session);
        }

        self.selection = Selection::Feature(pi, fi);
        self.save()?;
        self.message = Some(format!("Removed '{}'", label));

        Ok(())
    }
}
