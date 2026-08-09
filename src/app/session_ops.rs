use anyhow::Result;

use super::setup::{ensure_notification_hooks, ensure_review_claude_md};
use super::util::slugify;
use super::*;
use crate::project::LaunchOpts;
use crate::tmux::TmuxManager;

/// How long to wait for a launched VS Code window to appear as a local
/// process, and how often to look. Generous because a cold VS Code start is
/// several seconds; the wait happens on a background thread, so it costs the
/// UI nothing.
const VSCODE_OWNER_RESOLVE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const VSCODE_OWNER_RESOLVE_POLL: std::time::Duration = std::time::Duration::from_millis(500);

pub(crate) fn session_kind_for_agent(agent: &AgentKind) -> SessionKind {
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

fn kind_label(kind: &SessionKind) -> &'static str {
    match kind {
        SessionKind::Claude => "Claude",
        SessionKind::Opencode => "Opencode",
        SessionKind::Codex => "Codex",
        SessionKind::Pi => "Pi",
        SessionKind::Terminal => "terminal",
        SessionKind::Nvim => "Neovim",
        SessionKind::Vscode => "VSCode",
        SessionKind::Custom => "custom",
        SessionKind::Todos => "TODOs",
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

/// The harness whose saved-transcript picker (`S`) covers `kind`, if any.
/// Pi has no resume support, so it has no picker.
fn harness_session_picker_kind(kind: &SessionKind) -> Option<AgentKind> {
    match kind {
        SessionKind::Claude => Some(AgentKind::Claude),
        SessionKind::Codex => Some(AgentKind::Codex),
        SessionKind::Opencode => Some(AgentKind::Opencode),
        _ => None,
    }
}

fn persisted_resume_id(session: &FeatureSession) -> Option<String> {
    let id = match session.kind {
        SessionKind::Claude => session.claude_session_id.clone().or_else(|| {
            session
                .token_usage_source
                .as_ref()
                .filter(|source| {
                    source.provider == crate::token_tracking::TokenUsageProvider::Claude
                })
                .map(|source| source.id.clone())
        }),
        SessionKind::Opencode => session
            .token_usage_source
            .as_ref()
            .filter(|source| source.provider == crate::token_tracking::TokenUsageProvider::Opencode)
            .map(|source| source.id.clone()),
        SessionKind::Codex => session
            .token_usage_source
            .as_ref()
            .filter(|source| source.provider == crate::token_tracking::TokenUsageProvider::Codex)
            .map(|source| source.id.clone()),
        _ => None,
    };
    id.filter(|id| !id.trim().is_empty())
}

impl App {
    /// Intercept opening a persisted agent pane whose tmux session has
    /// disappeared *and* that AMF can offer a real choice about. Returns `true`
    /// when the stopped-session dialog was opened, allowing callers to preserve
    /// their normal running-session behavior.
    ///
    /// Deliberately narrow: the dialog only earns its keypress when the pane
    /// vanished behind the user's back (a crash, a reboot, an external
    /// `tmux kill-server`) *and* AMF holds a saved harness ID, so "resume" and
    /// "clear" actually differ. Everything else — a feature stopped from the
    /// dashboard with `x`, a feature created but never started, a harness with
    /// no resume support such as Pi — falls through to the ordinary start path
    /// it has always used.
    pub fn open_stopped_session_dialog(&mut self) -> Result<bool> {
        let (pi, fi, si) = match self.selection {
            Selection::Session(pi, fi, si) => (pi, fi, si),
            _ => return Ok(false),
        };

        let Some((project_id, feature_id, session_id, tmux_session, kind, has_resume_id)) =
            self.store.projects.get(pi).and_then(|project| {
                project.features.get(fi).and_then(|feature| {
                    feature.sessions.get(si).map(|session| {
                        (
                            project.id.clone(),
                            feature.id.clone(),
                            session.id.clone(),
                            feature.tmux_session.clone(),
                            session.kind.clone(),
                            persisted_resume_id(session).is_some(),
                        )
                    })
                })
            })
        else {
            return Ok(false);
        };

        // Without a saved harness ID both branches start the same clear
        // session, so there is nothing to ask.
        if !kind.is_agent_harness() || !has_resume_id {
            return Ok(false);
        }

        // The user stopped this feature from the dashboard in this run, so its
        // missing tmux session is expected: restart and resume in one keypress
        // the way `x` then `Enter` always has.
        if self.user_stopped_features.contains(&feature_id) {
            return Ok(false);
        }

        if self.tmux.session_exists(&tmux_session) {
            return Ok(false);
        }

        if self.block_if_feature_pending_worktree_script(pi, fi) {
            return Ok(true);
        }

        let mut choices = vec![StoppedSessionChoice::Resume, StoppedSessionChoice::Clear];
        if harness_session_picker_kind(&kind).is_some() {
            choices.push(StoppedSessionChoice::PickSession);
        }
        choices.push(StoppedSessionChoice::Cancel);

        self.mode = AppMode::StoppedSessionDialog(StoppedSessionDialogState {
            project_id,
            feature_id,
            session_id,
            selected: 0,
            choices,
            harness_label: kind_label(&kind).to_string(),
        });
        self.message = None;
        Ok(true)
    }

    pub fn confirm_stopped_session_choice(&mut self, choice: StoppedSessionChoice) {
        if choice == StoppedSessionChoice::Cancel {
            self.mode = AppMode::Normal;
            return;
        }

        let state = match &self.mode {
            AppMode::StoppedSessionDialog(state) => state.clone(),
            _ => return,
        };

        let resolved = self
            .store
            .projects
            .iter()
            .position(|project| project.id == state.project_id)
            .and_then(|pi| {
                self.store.projects[pi]
                    .features
                    .iter()
                    .position(|feature| feature.id == state.feature_id)
                    .map(|fi| (pi, fi))
            })
            .and_then(|(pi, fi)| {
                self.store.projects[pi].features[fi]
                    .sessions
                    .iter()
                    .position(|session| session.id == state.session_id)
                    .map(|si| (pi, fi, si))
            });

        let Some((pi, fi, si)) = resolved else {
            self.show_error(anyhow::anyhow!(
                "The selected session no longer exists; recovery was cancelled"
            ));
            return;
        };

        let (tmux_session, kind, agent, resume_id) = {
            let feature = &self.store.projects[pi].features[fi];
            let session = &feature.sessions[si];
            let Some(agent) = agent_for_session_kind(&session.kind) else {
                self.show_error(anyhow::anyhow!(
                    "The selected session is not an agent session"
                ));
                return;
            };
            (
                feature.tmux_session.clone(),
                session.kind.clone(),
                agent,
                persisted_resume_id(session),
            )
        };

        self.selection = Selection::Session(pi, fi, si);

        // Hand off to the harness's own transcript picker, which lists every
        // saved session on disk (not just the one AMF recorded) and knows how
        // to start a stopped feature against the chosen one.
        if choice == StoppedSessionChoice::PickSession {
            self.mode = AppMode::Normal;
            self.open_harness_session_picker(&kind);
            return;
        }

        let resume_id = match choice {
            StoppedSessionChoice::Resume => {
                let Some(resume_id) = resume_id else {
                    self.show_error(anyhow::anyhow!(
                        "No saved {} session identifier is available to resume",
                        agent.display_name()
                    ));
                    return;
                };
                Some(resume_id)
            }
            StoppedSessionChoice::Clear => None,
            StoppedSessionChoice::PickSession | StoppedSessionChoice::Cancel => unreachable!(),
        };

        // A sync tick or another AMF process may have recreated the tmux
        // session while the dialog was open. In that case, open it without
        // launching a duplicate harness.
        if self.tmux.session_exists(&tmux_session) {
            self.mode = AppMode::Normal;
            if let Err(error) = self.enter_view_without_auto_compose() {
                self.show_error(error);
            }
            return;
        }

        if let Err(error) = self.tmux.check_harness_available(&agent) {
            self.show_error(error);
            return;
        }

        let mut created_session = false;
        if let Err(error) = self.ensure_feature_running_for_recovery(
            pi,
            fi,
            state.session_id.clone(),
            resume_id,
            &mut created_session,
            // Mid-recovery: the picked session id lives in a mode the dialog
            // would replace, so warn rather than park and lose it.
            StartIntent::Warn("the recovered agent session"),
        ) {
            // This tmux session was created solely for this recovery attempt.
            // Remove a partially launched session so the same dialog can be
            // reached and retried from a clean stopped state.
            if created_session {
                let _ = self.tmux.kill_session(&tmux_session);
            }
            if let Some(feature) = self
                .store
                .projects
                .get_mut(pi)
                .and_then(|project| project.features.get_mut(fi))
            {
                feature.status = ProjectStatus::Stopped;
            }
            self.show_error(anyhow::anyhow!(
                "Failed to start the {} session: {error:#}",
                agent.display_name()
            ));
            return;
        }

        if choice == StoppedSessionChoice::Clear
            && let Some(session) = self
                .store
                .projects
                .get_mut(pi)
                .and_then(|project| project.features.get_mut(fi))
                .and_then(|feature| feature.sessions.get_mut(si))
        {
            session.claude_session_id = None;
            session.clear_token_usage_source();
        }

        self.mode = AppMode::Normal;
        if let Err(error) = self.enter_view_without_auto_compose() {
            self.show_error(error);
            return;
        }
        if let AppMode::Viewing(view) = &mut self.mode {
            view.show_startup_mask();
        }
        self.message = Some(match choice {
            StoppedSessionChoice::Resume => format!("Resumed {} session", kind_label(&kind)),
            StoppedSessionChoice::Clear => {
                format!("Started clear {} session", kind_label(&kind))
            }
            StoppedSessionChoice::PickSession | StoppedSessionChoice::Cancel => unreachable!(),
        });
    }

    /// Open the saved-transcript picker for `kind` — the same picker `S`
    /// reaches from the dashboard.
    fn open_harness_session_picker(&mut self, kind: &SessionKind) {
        match harness_session_picker_kind(kind) {
            Some(AgentKind::Claude) => self.pick_claude_session(),
            Some(AgentKind::Codex) => self.pick_codex_session(),
            Some(AgentKind::Opencode) => self.pick_opencode_session(),
            _ => {}
        }
    }

    /// Bring `(pi, fi)` up so a new session can be added to it.
    ///
    /// `intent` covers the *feature's own* saved agents, which come up with
    /// the tmux session: adding even a terminal to a stopped feature can start
    /// several harnesses. Callers that already gated the whole operation pass
    /// [`StartIntent::Approved`].
    pub(crate) fn ensure_feature_running_for_new_session(
        &mut self,
        pi: usize,
        fi: usize,
        intent: StartIntent,
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

        if !self.tmux.session_exists(&tmux_session)
            && self.ensure_feature_running(pi, fi, intent)? == Started::Parked
        {
            // Unreachable for the callers that exist: adding a session is
            // either pre-approved or warn-only, because there is no way to
            // hand the caller its new session back after a dialog.
            anyhow::bail!("this start cannot wait on a confirmation dialog");
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
        // A custom session is not itself an agent, but adding one to a stopped
        // feature brings that feature's saved agents up with it. The picker
        // has no state to resume from, so this warns rather than parks.
        self.ensure_feature_running_for_new_session(
            pi,
            fi,
            StartIntent::Warn("this feature's saved agents"),
        )?;

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
            self.tmux
                .run_shell_command(&tmux_session, &window, &shell_cmd)?;
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
        // A harness session obviously spends the machine's agent budget. So
        // does a terminal or an editor added to a *stopped* feature: bringing
        // its tmux session up launches every agent it has saved. Only an add
        // that starts nothing — a second window on a feature already running,
        // or the tmux-less TODOs list — goes through without a word.
        if (kind.is_agent_harness() || self.add_would_start_feature(pi, fi, &kind))
            && self.gate_start(PendingStart::BuiltinSession {
                pi,
                fi,
                kind: kind.clone(),
                label: label.clone(),
            })
        {
            return Ok(());
        }

        self.add_builtin_session_unchecked(pi, fi, kind, label)
    }

    /// Whether adding a `kind` session to `(pi, fi)` would bring the
    /// feature's tmux session — and so its saved agents — up.
    fn add_would_start_feature(&self, pi: usize, fi: usize, kind: &SessionKind) -> bool {
        kind.is_tmux_backed()
            && self
                .store
                .projects
                .get(pi)
                .and_then(|project| project.features.get(fi))
                .is_some_and(|feature| !self.tmux.session_exists(&feature.tmux_session))
    }

    /// Add a session that has already cleared the resource gate (or never
    /// needed it).
    pub(crate) fn add_builtin_session_unchecked(
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

        self.ensure_feature_running_for_new_session(pi, fi, StartIntent::Approved)?;

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
        let feature_id = feature.id.clone();

        // Windows already open on this worktree, captured *before* the launch:
        // whichever match appears afterwards is the one AMF opened, and that
        // difference is the only proof of ownership worth killing on.
        let before = crate::resources::procs::existing_vscode_windows(&workdir);

        // `--new-window` is what makes the instance AMF's own. Without it the
        // folder is handed to whatever window happens to be running, which AMF
        // must never close on the user's behalf.
        let command = format!("code --new-window {}", workdir.display());
        std::process::Command::new("code")
            .arg("--new-window")
            .arg(&workdir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| anyhow::anyhow!("Failed to launch VSCode: {}", e))?;

        self.record_vscode_launch(&feature_id, &workdir, &command, before);

        self.message = Some(format!("Opened VSCode in {}", workdir.display()));

        Ok(())
    }

    /// Record the launch, then resolve which process it produced in the
    /// background.
    ///
    /// The `code` CLI hands the request off and exits immediately, so its own
    /// PID is worthless — the window process shows up seconds later, and on a
    /// remote/WSL setup never shows up locally at all. The record therefore
    /// starts as not-owned and is upgraded only if a new local window is
    /// actually found; anything still not-owned is skipped at stop time.
    ///
    /// Because VS Code is a singleton application, "a new local window" only
    /// exists when this launch is what *started* VS Code. Handing a folder to a
    /// running instance produces a window but no new eligible process, and that
    /// launch stays not-owned for good — correctly, since AMF cannot close that
    /// window without closing the instance around it.
    ///
    /// A launch still resolving when its feature is stopped is not lost: the
    /// stop marks it for reclamation (see
    /// [`crate::app::editor_ops::PendingEditorLaunch`]) and the resolver below
    /// closes the window instead of recording it.
    fn record_vscode_launch(
        &mut self,
        feature_id: &str,
        workdir: &std::path::Path,
        command: &str,
        before: Vec<i64>,
    ) {
        let Some(db) = self.db.as_ref() else {
            return;
        };

        // Drop records whose process is gone (closed window, reboot) so
        // repeated launches don't pile up rows for one worktree.
        if let Ok(existing) = db.launched_editors_for_feature(feature_id) {
            for editor in existing {
                if editor.worktree_path == workdir
                    && !crate::resources::procs::pid_alive(editor.pid)
                {
                    let _ = db.delete_launched_editor(&editor.id);
                }
            }
        }

        let record = db.record_launched_editor(
            feature_id,
            None,
            crate::db::editors::EditorKind::Vscode,
            0,
            workdir,
            false,
            command,
        );
        let Ok(record) = record else {
            self.log_warn("editor", "failed to record the VSCode launch".to_string());
            return;
        };

        let db_path = db.path.clone();
        let workdir = workdir.to_path_buf();

        use crate::app::editor_ops::{PendingEditorLaunch, PendingLaunchState, lock_state};
        let state = std::sync::Arc::new(std::sync::Mutex::new(PendingLaunchState::Resolving));
        self.prune_resolved_editor_launches();
        self.pending_editor_launches.push(PendingEditorLaunch {
            feature_id: feature_id.to_string(),
            record_id: record.id.clone(),
            kind: crate::db::editors::EditorKind::Vscode,
            state: state.clone(),
        });

        std::thread::spawn(move || {
            let found = crate::resources::procs::find_new_vscode_window(
                &workdir,
                &before,
                VSCODE_OWNER_RESOLVE_TIMEOUT,
                VSCODE_OWNER_RESOLVE_POLL,
            );
            // Held across the decision *and* the write, so a stop arriving in
            // the middle either claims the launch (and this closes the window)
            // or finds the row already owned (and closes it itself). There is no
            // ordering where the window survives with nobody responsible.
            let mut state = lock_state(&state);
            let reclaim = *state == PendingLaunchState::Reclaim;
            *state = PendingLaunchState::Done;

            let Some(found) = found else {
                // Left not-owned on purpose: a reused window, or a remote one
                // with no local process, is not AMF's to close.
                return;
            };

            if reclaim {
                // The feature was stopped while this window was still opening.
                crate::resources::procs::terminate_tree(
                    found.pid,
                    std::time::Duration::from_secs(2),
                );
                if let Ok(db) = crate::db::AmfDb::open(&db_path) {
                    let _ = db.delete_launched_editor(&record.id);
                }
                return;
            }

            // The process's own start time, recorded now so that a later
            // process inheriting this PID cannot pass the kill-time check.
            let started =
                crate::resources::procs::start_time_for_pid(found.pid).unwrap_or_default();
            if let Ok(db) = crate::db::AmfDb::open(&db_path) {
                let _ = db.set_launched_editor_owner(&record.id, found.pid, true, &started);
            }
        });
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
                let codex_args = crate::codex_config::launch_override_args(&workdir, &mode);
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

    /// Spin up the dedicated PR-triage agent session: one agent window, labeled
    /// [`crate::app::pr_review::TRIAGE_SESSION_LABEL`], that the PR-triage pane
    /// reuses for every fix in a PR. Runs `harness` (the harness the user picked
    /// for the triage session) or falls back to the project's preferred agent,
    /// with the feature's mode/flags — just like a picker-launched agent session,
    /// but with a fixed label so it can be found-and-reused. Returns the new
    /// session's index in `feature.sessions`.
    pub(crate) fn create_dedicated_review_session(
        &mut self,
        pi: usize,
        fi: usize,
        label: &str,
        harness: Option<AgentKind>,
        intent: StartIntent,
    ) -> Result<usize> {
        self.create_agent_session_labeled(pi, fi, label, harness, intent)
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
        intent: StartIntent,
    ) -> Result<usize> {
        // This always launches a harness, so the gate runs unconditionally --
        // once, here, covering both the new session and any of the feature's
        // own agents that come up with it.
        if self.gate_launch(intent) == Started::Parked {
            // See `ensure_feature_running_for_new_session`: this primitive
            // owes its caller a session index, which a parked start cannot
            // produce.
            anyhow::bail!("this start cannot wait on a confirmation dialog");
        }
        self.ensure_feature_running_for_new_session(pi, fi, StartIntent::Approved)?;

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
                let codex_args = crate::codex_config::launch_override_args(&workdir, &mode);
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
