use super::*;
use crate::github::{GhCli, PrResolution};
use crate::project::{AgentKind, SessionKind, TokenUsageSourceMatch};
use crate::summary::SummaryManager;
use crate::tmux::TmuxManager;
use crate::token_tracking::{
    SessionTokenTracker, SessionTokenUsage, TokenPricingConfig, TokenUsageProvider,
    TokenUsageSource, format_token_usage, provider_for_session_kind,
};

use chrono::Utc;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct ActivePrJob {
    feature_id: String,
    branch: String,
    workdir: PathBuf,
}

#[derive(Debug)]
pub(crate) enum ActivePrLookup {
    Found(ActivePrStatus),
    NoPr,
    Failed(String),
}

#[derive(Debug)]
pub(crate) struct ActivePrUpdate {
    pub feature_id: String,
    pub branch: String,
    pub lookup: ActivePrLookup,
}

fn read_status_line(content: &str) -> Option<String> {
    let line = content.lines().next()?.trim().to_string();
    if line.is_empty() { None } else { Some(line) }
}

fn file_mtime_nanos(path: &Path) -> Option<u64> {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
}

fn read_custom_session_status(
    session_id: &str,
    feature_id: &str,
    workdir: &Path,
    db: Option<&crate::db::AmfDb>,
) -> Option<String> {
    let status_path = workdir
        .join(".amf")
        .join("session-status")
        .join(format!("{}.txt", session_id));

    // Cheap stat to get the current file mtime before touching the DB.
    let current_mtime = file_mtime_nanos(&status_path);

    if let Some(db) = db
        && let Ok(Some((text, cached_mtime))) = db.load_session_status_with_mtime(session_id)
    {
        let text = text.trim().to_string();
        if !text.is_empty() {
            // Return cached value when:
            //   - file doesn't exist (current_mtime is None) — use DB as fallback, OR
            //   - file mtime matches what we last cached — file unchanged.
            if current_mtime.is_none() || current_mtime == cached_mtime {
                return Some(text);
            }
        }
    }

    // File exists and is newer than cache (or no cache yet) — read it.
    let file_text = std::fs::read_to_string(&status_path)
        .ok()
        .and_then(|content| read_status_line(&content));

    if let (Some(text), Some(db)) = (file_text.as_ref(), db) {
        let _ = db.upsert_session_status(session_id, feature_id, text, current_mtime);
    }

    file_text
}

// ---------------------------------------------------------------------------
// Background session-status sync types
// ---------------------------------------------------------------------------

struct SessionStatusJob {
    feature_id: String,
    session_id: String,
    kind: SessionKind,
    workdir: PathBuf,
    created_at: chrono::DateTime<chrono::Utc>,
    existing_source: Option<TokenUsageSource>,
    #[allow(dead_code)] // set during job construction, not read yet
    existing_source_match: Option<TokenUsageSourceMatch>,
    claude_session_id: Option<String>,
    custom_status_text: Option<String>,
}

enum SourceAction {
    SetExact(TokenUsageSource),
    SetInferred(TokenUsageSource),
    Clear,
    NoChange,
}

struct SessionStatusUpdate {
    session_id: String,
    source_action: SourceAction,
    status_text: Option<String>,
    token_usage: Option<SessionTokenUsage>,
}

pub(crate) struct SessionStatusBgResult {
    tracker: SessionTokenTracker,
    updates: Vec<SessionStatusUpdate>,
    sources_discovered: bool,
}

fn collect_jobs(
    store: &crate::project::ProjectStore,
    db: Option<&crate::db::AmfDb>,
) -> Vec<SessionStatusJob> {
    store
        .projects
        .iter()
        .flat_map(|project| {
            project.features.iter().flat_map(|feature| {
                feature.sessions.iter().map(|session| SessionStatusJob {
                    feature_id: feature.id.clone(),
                    session_id: session.id.clone(),
                    kind: session.kind.clone(),
                    workdir: feature.workdir.clone(),
                    created_at: session.created_at,
                    existing_source: session.token_usage_source.clone(),
                    existing_source_match: session.token_usage_source_match.clone(),
                    claude_session_id: session.claude_session_id.clone(),
                    custom_status_text: if session.kind == SessionKind::Custom {
                        read_custom_session_status(&session.id, &feature.id, &feature.workdir, db)
                    } else {
                        None
                    },
                })
            })
        })
        .collect()
}

fn run_jobs(
    mut tracker: SessionTokenTracker,
    jobs: Vec<SessionStatusJob>,
    pricing: &TokenPricingConfig,
) -> SessionStatusBgResult {
    let mut updates = Vec::with_capacity(jobs.len());
    let mut sources_discovered = false;
    let mut reserved_sources: HashSet<(String, TokenUsageProvider, String)> = HashSet::new();

    for job in jobs {
        if job.kind == SessionKind::Custom {
            updates.push(SessionStatusUpdate {
                session_id: job.session_id,
                source_action: SourceAction::NoChange,
                status_text: job.custom_status_text,
                token_usage: None,
            });
            continue;
        }

        let Some(expected_provider) = provider_for_session_kind(&job.kind) else {
            updates.push(SessionStatusUpdate {
                session_id: job.session_id,
                source_action: SourceAction::NoChange,
                status_text: None,
                token_usage: None,
            });
            continue;
        };

        let mut source = job.existing_source.clone();
        let mut action = SourceAction::NoChange;

        // Fast path: Claude session with a known session ID → exact match.
        if source.is_none()
            && matches!(job.kind, SessionKind::Claude)
            && job.claude_session_id.is_some()
            && let Some(id) = job.claude_session_id.as_ref()
        {
            let new_source = TokenUsageSource {
                provider: TokenUsageProvider::Claude,
                id: id.clone(),
            };
            source = Some(new_source.clone());
            action = SourceAction::SetExact(new_source);
            sources_discovered = true;
        }

        // Clear if the cached source belongs to the wrong provider.
        if source
            .as_ref()
            .is_some_and(|s| s.provider != expected_provider)
        {
            source = None;
            action = SourceAction::Clear;
            sources_discovered = true;
        }

        if let Some(existing) = source.as_ref()
            && job.existing_source_match == Some(TokenUsageSourceMatch::Inferred)
            && existing.provider == TokenUsageProvider::Codex
            && tracker
                .source_updated_at_seconds(existing, &job.workdir)
                .is_some_and(|updated| updated < job.created_at.timestamp())
        {
            source = None;
            action = SourceAction::Clear;
            sources_discovered = true;
        }

        if let Some(existing) = source.as_ref() {
            let key = (
                job.feature_id.clone(),
                existing.provider.clone(),
                existing.id.clone(),
            );
            if job.existing_source_match == Some(TokenUsageSourceMatch::Inferred)
                && reserved_sources.contains(&key)
            {
                source = None;
                action = SourceAction::Clear;
                sources_discovered = true;
            } else {
                reserved_sources.insert(key);
            }
        }

        // Infer from the filesystem if we still have no source.
        if source.is_none() {
            let excluded_ids = reserved_sources
                .iter()
                .filter(|(feature_id, provider, _)| {
                    feature_id == &job.feature_id && provider == &expected_provider
                })
                .map(|(_, _, id)| id.clone())
                .collect::<HashSet<_>>();
            if let Some(new_source) = tracker.discover_source_excluding(
                &job.kind,
                &job.workdir,
                job.created_at,
                &excluded_ids,
            ) {
                reserved_sources.insert((
                    job.feature_id.clone(),
                    new_source.provider.clone(),
                    new_source.id.clone(),
                ));
                source = Some(new_source.clone());
                action = SourceAction::SetInferred(new_source);
                sources_discovered = true;
            }
        }

        let token_usage = source
            .as_ref()
            .and_then(|src| tracker.read_usage(src, &job.workdir));
        let status_text = token_usage
            .as_ref()
            .map(|usage| format_token_usage(usage, pricing));

        updates.push(SessionStatusUpdate {
            session_id: job.session_id,
            source_action: action,
            status_text,
            token_usage,
        });
    }

    SessionStatusBgResult {
        tracker,
        updates,
        sources_discovered,
    }
}

fn apply_bg_result(app: &mut App, result: SessionStatusBgResult) {
    app.token_tracker = result.tracker;

    for update in result.updates {
        'outer: for project in &mut app.store.projects {
            for feature in &mut project.features {
                for session in &mut feature.sessions {
                    if session.id != update.session_id {
                        continue;
                    }
                    match update.source_action {
                        SourceAction::SetExact(ref src) => {
                            session.set_token_usage_source_exact(src.clone())
                        }
                        SourceAction::SetInferred(ref src) => {
                            session.set_token_usage_source_inferred(src.clone())
                        }
                        SourceAction::Clear => session.clear_token_usage_source(),
                        SourceAction::NoChange => {}
                    }
                    session.status_text = update.status_text;
                    session.token_usage = update.token_usage;
                    break 'outer;
                }
            }
        }
    }

    if result.sources_discovered
        && let Err(err) = app.save()
    {
        app.log_warn(
            "usage",
            format!("Failed to persist discovered token tracking sources: {err}"),
        );
    }

    if app.has_active_sidebar() {
        app.refresh_sidebar_for_current_view();
    } else if !matches!(app.mode, AppMode::Viewing(_)) {
        app.schedule_sidebar_loads_for_polling_fallback();
    }
    app.flush_token_cache_to_db();
}

pub(super) fn pane_shows_thinking_hint(content: &str) -> bool {
    let lower = content.to_lowercase();
    ["esc interrupt", "esc to interrupt", "ctrl+c to interrupt"]
        .iter()
        .any(|marker| lower.contains(marker))
}

fn opencode_sidebar_thinking_state(
    sidebar: &crate::app::opencode_storage::OpencodeSidebarData,
) -> Option<bool> {
    if sidebar
        .pending_permission
        .as_deref()
        .is_some_and(|permission| !permission.trim().is_empty())
    {
        return Some(false);
    }

    let status = sidebar.status.as_deref()?.trim().to_ascii_lowercase();
    if status.is_empty() {
        return None;
    }

    Some(!matches!(
        status.as_str(),
        "idle"
            | "ready"
            | "done"
            | "completed"
            | "closed"
            | "cancelled"
            | "canceled"
            | "stopped"
            | "waiting"
    ))
}

impl App {
    /// Refresh dashboard PR badges without ever blocking rendering or input.
    /// The main loop starts this on the existing feature-status cadence and
    /// refuses to overlap jobs, so a slow `gh` invocation cannot build up a
    /// queue of redundant work.
    pub fn sync_active_prs_background(&mut self) {
        if self.active_pr_bg.is_some() {
            return;
        }

        let jobs = self
            .store
            .projects
            .iter()
            .filter(|project| project.is_git)
            .flat_map(|project| {
                project.features.iter().map(|feature| ActivePrJob {
                    feature_id: feature.id.clone(),
                    branch: feature.branch.clone(),
                    workdir: feature.workdir.clone(),
                })
            })
            .collect::<Vec<_>>();
        if jobs.is_empty() {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.active_pr_bg = Some(rx);
        std::thread::spawn(move || {
            let updates = jobs
                .into_iter()
                .map(|job| {
                    let lookup = match GhCli::resolve_pr(&job.workdir) {
                        Ok(PrResolution::Found(pr)) => {
                            let unresolved_threads =
                                GhCli::review_threads(&job.workdir, &pr.owner, &pr.repo, pr.number)
                                    .ok()
                                    .map(|threads| {
                                        threads.iter().filter(|thread| !thread.is_resolved).count()
                                    });
                            ActivePrLookup::Found(ActivePrStatus {
                                branch: job.branch.clone(),
                                head_sha: pr.head_sha,
                                number: pr.number,
                                unresolved_threads,
                            })
                        }
                        Ok(PrResolution::NoPrForBranch) => ActivePrLookup::NoPr,
                        Err(error) => ActivePrLookup::Failed(error.to_string()),
                    };
                    ActivePrUpdate {
                        feature_id: job.feature_id,
                        branch: job.branch,
                        lookup,
                    }
                })
                .collect();
            let _ = tx.send(updates);
        });
    }

    /// Apply a completed dashboard PR refresh. Transient failures deliberately
    /// preserve the previous value; a confirmed no-PR result removes it.
    pub fn poll_active_pr_bg(&mut self) -> bool {
        let Some(rx) = self.active_pr_bg.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(updates) => {
                self.active_pr_bg = None;
                self.apply_active_pr_updates(updates)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.active_pr_bg = None;
                false
            }
        }
    }

    pub(crate) fn apply_active_pr_updates(&mut self, updates: Vec<ActivePrUpdate>) -> bool {
        let mut changed = false;
        for update in updates {
            let branch_is_current = self.store.projects.iter().any(|project| {
                project.features.iter().any(|feature| {
                    feature.id == update.feature_id && feature.branch == update.branch
                })
            });
            if !branch_is_current {
                continue;
            }

            match update.lookup {
                ActivePrLookup::Found(status) => {
                    if self.active_prs.get(&update.feature_id) != Some(&status) {
                        self.active_prs.insert(update.feature_id, status);
                        changed = true;
                    }
                }
                ActivePrLookup::NoPr => {
                    changed |= self.active_prs.remove(&update.feature_id).is_some();
                }
                ActivePrLookup::Failed(error) => {
                    self.log_debug(
                        "github",
                        format!(
                            "Dashboard PR lookup failed for {} ({}): {}",
                            update.feature_id, update.branch, error
                        ),
                    );
                }
            }
        }
        changed
    }

    pub(crate) fn active_pr_for_feature(&self, feature_id: &str) -> Option<&ActivePrStatus> {
        self.active_prs.get(feature_id)
    }

    /// Kick off a background thread to do the expensive token-usage I/O.
    /// The thread takes ownership of `self.token_tracker` so the cache is
    /// preserved; it is swapped back in when `poll_session_status_bg` applies
    /// the results.
    pub fn sync_session_status_background(&mut self) {
        let jobs = collect_jobs(&self.store, self.db.as_ref());
        let pricing = self.config.token_pricing.clone();
        let tracker = std::mem::take(&mut self.token_tracker);

        let (tx, rx) = std::sync::mpsc::channel();
        self.session_status_bg = Some(rx);

        std::thread::spawn(move || {
            let result = run_jobs(tracker, jobs, &pricing);
            let _ = tx.send(result);
        });
    }

    /// Check whether the background session-status thread has finished and,
    /// if so, apply its results.  Returns `true` when state changed.
    pub fn poll_session_status_bg(&mut self) -> bool {
        let ready = self
            .session_status_bg
            .as_ref()
            .and_then(|rx| rx.try_recv().ok());

        match ready {
            Some(result) => {
                self.session_status_bg = None;
                apply_bg_result(self, result);
                true
            }
            None => {
                // Clean up a disconnected sender.
                if self.session_status_bg.as_ref().is_some_and(|rx| {
                    matches!(
                        rx.try_recv(),
                        Err(std::sync::mpsc::TryRecvError::Disconnected)
                    )
                }) {
                    self.session_status_bg = None;
                }
                false
            }
        }
    }

    pub fn sync_statuses(&mut self) {
        let live_sessions: HashSet<String> = self
            .tmux
            .list_sessions()
            .unwrap_or_default()
            .into_iter()
            .collect();
        let mut stopped_sessions = Vec::new();
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                if live_sessions.contains(feature.tmux_session.as_str()) {
                    if feature.status == ProjectStatus::Stopped {
                        feature.status = ProjectStatus::Idle;
                    }
                } else {
                    if feature.status != ProjectStatus::Stopped {
                        stopped_sessions.push(feature.tmux_session.clone());
                    }
                    feature.status = ProjectStatus::Stopped;
                }
            }
        }
        for tmux_session in stopped_sessions {
            self.clear_sidebar_state_for_session(&tmux_session);
        }
    }

    #[allow(dead_code)] // exercised only by unit tests
    pub fn sync_session_status(&mut self) {
        let mut tracker = std::mem::take(&mut self.token_tracker);
        self.sync_session_status_with_tracker(&mut tracker);
        self.token_tracker = tracker;
    }

    #[allow(dead_code)] // exercised only by unit tests
    pub(crate) fn sync_session_status_with_tracker(&mut self, tracker: &mut SessionTokenTracker) {
        let pricing = self.config.token_pricing.clone();
        let mut discovered_sources = false;

        for project in &mut self.store.projects {
            for feature in &mut project.features {
                let mut reserved_sources: HashSet<(TokenUsageProvider, String)> = HashSet::new();
                for session in &mut feature.sessions {
                    if session.kind == crate::project::SessionKind::Custom {
                        session.status_text = read_custom_session_status(
                            &session.id,
                            &feature.id,
                            &feature.workdir,
                            self.db.as_ref(),
                        );
                        session.token_usage = None;
                        continue;
                    }

                    let Some(expected_provider) = provider_for_session_kind(&session.kind) else {
                        session.status_text = None;
                        session.token_usage = None;
                        continue;
                    };

                    if session.token_usage_source.is_none()
                        && matches!(session.kind, SessionKind::Claude)
                        && session.claude_session_id.is_some()
                        && let Some(id) = session.claude_session_id.as_ref()
                    {
                        session.set_token_usage_source_exact(TokenUsageSource {
                            provider: TokenUsageProvider::Claude,
                            id: id.clone(),
                        });
                        discovered_sources = true;
                    }

                    if session
                        .token_usage_source
                        .as_ref()
                        .is_some_and(|source| source.provider != expected_provider)
                    {
                        session.clear_token_usage_source();
                        discovered_sources = true;
                    }

                    if let Some(source) = session.token_usage_source.as_ref()
                        && session.token_usage_source_match == Some(TokenUsageSourceMatch::Inferred)
                        && source.provider == TokenUsageProvider::Codex
                        && tracker
                            .source_updated_at_seconds(source, &feature.workdir)
                            .is_some_and(|updated| updated < session.created_at.timestamp())
                    {
                        session.clear_token_usage_source();
                        discovered_sources = true;
                    }

                    if let Some(source) = session.token_usage_source.as_ref() {
                        let key = (source.provider.clone(), source.id.clone());
                        if session.token_usage_source_match == Some(TokenUsageSourceMatch::Inferred)
                            && reserved_sources.contains(&key)
                        {
                            session.clear_token_usage_source();
                            discovered_sources = true;
                        } else {
                            reserved_sources.insert(key);
                        }
                    }

                    if session.token_usage_source.is_none() {
                        let excluded_ids = reserved_sources
                            .iter()
                            .filter(|(provider, _)| provider == &expected_provider)
                            .map(|(_, id)| id.clone())
                            .collect::<HashSet<_>>();
                        if let Some(source) = tracker.discover_source_excluding(
                            &session.kind,
                            &feature.workdir,
                            session.created_at,
                            &excluded_ids,
                        ) {
                            reserved_sources.insert((source.provider.clone(), source.id.clone()));
                            session.set_token_usage_source_inferred(source);
                            discovered_sources = true;
                        }
                    }

                    session.token_usage = session
                        .token_usage_source
                        .as_ref()
                        .and_then(|source| tracker.read_usage(source, &feature.workdir));
                    session.status_text = session
                        .token_usage
                        .as_ref()
                        .map(|usage| format_token_usage(usage, &pricing));
                }
            }
        }

        if self.has_active_sidebar() {
            self.refresh_sidebar_for_current_view();
        } else if !matches!(self.mode, AppMode::Viewing(_)) {
            self.schedule_sidebar_loads_for_polling_fallback();
        }

        if discovered_sources && let Err(err) = self.save() {
            self.log_warn(
                "usage",
                format!("Failed to persist discovered token tracking sources: {err}"),
            );
        }
        self.flush_token_cache_to_db();
    }

    pub fn sync_thinking_status(&mut self) -> bool {
        // Background sidebar loads warm Opencode status/prompt data for the
        // dashboard. Drain them here too, not just when a sidebar is open, so
        // thinking sync can use cached state instead of falling back to
        // `tmux capture-pane` across many features.
        self.poll_sidebar_load_results();

        // Opencode features without sidebar cache need a capture-pane
        // fallback, which requires &mut self — collect probe targets up
        // front instead of borrowing the store across the loop.
        struct ThinkingProbe {
            tmux_session: String,
            agent: AgentKind,
            opencode_window: Option<String>,
        }
        let probes: Vec<ThinkingProbe> = self
            .store
            .projects
            .iter()
            .flat_map(|project| {
                project.features.iter().filter_map(|feature| {
                    if feature.status == ProjectStatus::Stopped {
                        return None;
                    }
                    Some(ThinkingProbe {
                        tmux_session: feature.tmux_session.clone(),
                        agent: feature.agent.clone(),
                        opencode_window: feature
                            .sessions
                            .iter()
                            .find(|s| s.kind == SessionKind::Opencode)
                            .map(|s| s.tmux_window.clone()),
                    })
                })
            })
            .collect();

        let old_thinking = std::mem::take(&mut self.thinking_features);
        let ipc_mode = self.ipc.is_some();
        for probe in &probes {
            let thinking = match probe.agent {
                AgentKind::Claude => {
                    if ipc_mode {
                        self.ipc_thinking_sessions.contains(&probe.tmux_session)
                            || self.ipc_tool_sessions.contains(&probe.tmux_session)
                    } else {
                        Self::is_session_marked_thinking(&probe.tmux_session)
                    }
                }
                AgentKind::Opencode => self
                    .opencode_sidebar_cache
                    .get(&probe.tmux_session)
                    .and_then(opencode_sidebar_thinking_state)
                    .or_else(|| {
                        probe.opencode_window.as_ref().and_then(|window| {
                            self.opencode_thinking_pane_fallback(&probe.tmux_session, window)
                        })
                    })
                    .unwrap_or(false),
                AgentKind::Codex => {
                    if ipc_mode {
                        self.ipc_thinking_sessions.contains(&probe.tmux_session)
                    } else {
                        Self::is_session_marked_thinking(&probe.tmux_session)
                    }
                }
                AgentKind::Pi => Self::is_session_marked_thinking(&probe.tmux_session),
            };
            if thinking {
                self.thinking_features.insert(probe.tmux_session.clone());
            }
        }

        // Agent-agnostic fallback: if a feature transitions from
        // thinking to not-thinking, treat it as waiting for user
        // input unless another pending notification already exists.
        let active_features: Vec<(String, String, String, String, AgentKind)> = self
            .store
            .projects
            .iter()
            .flat_map(|project| {
                project.features.iter().filter_map(|feature| {
                    if feature.status == ProjectStatus::Stopped {
                        return None;
                    }
                    Some((
                        project.name.clone(),
                        feature.name.clone(),
                        feature.tmux_session.clone(),
                        feature.workdir.to_string_lossy().into_owned(),
                        feature.agent.clone(),
                    ))
                })
            })
            .collect();

        let mut pending_inputs_changed = false;
        for (project_name, feature_name, sid, cwd, agent) in active_features {
            let was_thinking = old_thinking.contains(&sid);
            let is_thinking = self.thinking_features.contains(&sid);

            if is_thinking {
                let before = self.pending_inputs.len();
                self.pending_inputs.retain(|p| {
                    !(p.notification_type == "input-request"
                        && p.project_name.as_deref() == Some(&project_name)
                        && p.feature_name.as_deref() == Some(&feature_name))
                });
                let removed = before.saturating_sub(self.pending_inputs.len());
                if removed > 0 {
                    pending_inputs_changed = true;
                    self.log_debug(
                        "sync",
                        format!(
                            "Cleared {removed} input notification(s) for {} (agent={}, session={})",
                            feature_name,
                            agent.display_name(),
                            sid
                        ),
                    );
                }
                continue;
            }

            if was_thinking && !is_thinking {
                let any_pending_for_feature = self.pending_inputs.iter().any(|p| {
                    p.project_name.as_deref() == Some(&project_name)
                        && p.feature_name.as_deref() == Some(&feature_name)
                });
                if !any_pending_for_feature {
                    pending_inputs_changed = true;
                    self.pending_inputs.push(PendingInput {
                        session_id: sid.clone(),
                        cwd,
                        message: "Agent finished and is waiting for input".to_string(),
                        notification_type: "input-request".to_string(),
                        file_path: std::path::PathBuf::new(),
                        target_file_path: None,
                        relative_path: None,
                        change_id: None,
                        tool: None,
                        old_snippet: None,
                        new_snippet: None,
                        original_file: None,
                        proposed_file: None,
                        is_new_file: None,
                        reason: None,
                        response_file: None,
                        project_name: Some(project_name),
                        feature_name: Some(feature_name.clone()),
                        proceed_signal: None,
                        request_id: None,
                        reply_socket: None,
                    });
                    self.log_info(
                        "sync",
                        format!(
                            "Detected waiting-for-input for {} (agent={}, session={})",
                            feature_name,
                            agent.display_name(),
                            sid
                        ),
                    );
                }
            }
        }

        old_thinking != self.thinking_features || pending_inputs_changed
    }

    /// Capture-pane is a subprocess; cache its thinking verdict per
    /// session so the 500ms thinking tick spawns it at most every 5s.
    fn opencode_thinking_pane_fallback(
        &mut self,
        tmux_session: &str,
        window: &str,
    ) -> Option<bool> {
        const FALLBACK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

        if let Some((checked_at, result)) = self.opencode_thinking_pane_cache.get(tmux_session)
            && checked_at.elapsed() < FALLBACK_INTERVAL
        {
            return *result;
        }

        let result = TmuxManager::capture_pane(tmux_session, window)
            .ok()
            .map(|content| pane_shows_thinking_hint(&content));
        self.opencode_thinking_pane_cache
            .insert(tmux_session.to_string(), (Instant::now(), result));
        result
    }

    fn is_session_marked_thinking(tmux_session: &str) -> bool {
        let path_str = format!("/tmp/amf-thinking/{}", tmux_session);
        let path = std::path::Path::new(&path_str);
        if !path.exists() {
            return false;
        }

        match std::fs::metadata(path) {
            Ok(metadata) => match metadata.modified() {
                Ok(modified) => match modified.elapsed() {
                    Ok(elapsed) => elapsed < std::time::Duration::from_secs(2),
                    Err(_) => false,
                },
                Err(_) => false,
            },
            Err(_) => false,
        }
    }

    pub fn cleanup_stale_thinking_files() {
        let Ok(entries) = std::fs::read_dir("/tmp/amf-thinking") else {
            return;
        };

        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata()
                && let Ok(modified) = metadata.modified()
                && let Ok(elapsed) = modified.elapsed()
                && elapsed > std::time::Duration::from_secs(10)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    pub fn is_feature_thinking(&self, tmux_session: &str) -> bool {
        self.thinking_features.contains(tmux_session)
    }

    pub(crate) fn note_codex_prompt_submit(&mut self, tmux_session: &str, tmux_window: &str) {
        let mut matched: Option<(String, String)> = None;
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.tmux_session != tmux_session || feature.agent != AgentKind::Codex {
                    continue;
                }
                let has_codex_window = feature
                    .sessions
                    .iter()
                    .any(|s| s.kind == SessionKind::Codex && s.tmux_window == tmux_window);
                if has_codex_window {
                    matched = Some((project.name.clone(), feature.name.clone()));
                    break;
                }
            }
            if matched.is_some() {
                break;
            }
        }

        if let Some((project_name, feature_name)) = matched {
            self.ipc_thinking_sessions.insert(tmux_session.to_string());
            self.pending_inputs.retain(|p| {
                !(p.notification_type == "input-request"
                    && p.project_name.as_deref() == Some(&project_name)
                    && p.feature_name.as_deref() == Some(&feature_name))
            });
            self.log_debug(
                "ipc",
                format!(
                    "Codex prompt submit captured locally; marked thinking (session={}, feature={})",
                    tmux_session, feature_name
                ),
            );
        } else {
            self.log_debug(
                "ipc",
                format!(
                    "Ignored codex prompt submit marker for non-worktree/non-codex window (session={}, window={})",
                    tmux_session, tmux_window
                ),
            );
        }
    }

    pub fn is_feature_waiting_for_input(&self, feature_name: &str) -> bool {
        self.pending_inputs
            .iter()
            .any(|input| input.feature_name.as_deref() == Some(feature_name))
    }

    pub fn trigger_summary_for_selected(&mut self) -> Result<()> {
        let selected = match &self.selection {
            Selection::Feature(pi, fi) => {
                let feature = &self.store.projects[*pi].features[*fi];
                Some((
                    feature.tmux_session.clone(),
                    feature.workdir.clone(),
                    feature.agent.clone(),
                ))
            }
            Selection::Session(pi, fi, _si) => {
                let feature = &self.store.projects[*pi].features[*fi];
                Some((
                    feature.tmux_session.clone(),
                    feature.workdir.clone(),
                    feature.agent.clone(),
                ))
            }
            _ => None,
        };

        if let Some((tmux_session, workdir, agent)) = selected {
            if self.summary_state.generating.contains(&tmux_session) {
                self.message = Some("Summary already generating...".into());
                return Ok(());
            }

            let window = self.get_window_for_session(&tmux_session, &agent);
            if let Some(w) = window {
                self.summary_state.generating.insert(tmux_session.clone());
                self.message = Some("Generating summary...".into());

                let (tx, rx) = std::sync::mpsc::channel();
                self.summary_rx = Some(rx);

                let tmux_session_clone = tmux_session.clone();
                std::thread::spawn(move || {
                    let result =
                        SummaryManager::generate_summary(&tmux_session_clone, &w, &workdir, agent);
                    let _ = tx.send((tmux_session_clone, result));
                });
            } else {
                self.message = Some("No agent window found".into());
            }
        } else {
            self.message = Some("Select a feature to summarize".into());
        }

        Ok(())
    }

    pub fn poll_summary_result(&mut self) -> Result<()> {
        if let Some(ref rx) = self.summary_rx {
            match rx.try_recv() {
                Ok((tmux_session, result)) => {
                    self.summary_rx = None;
                    self.summary_state.generating.remove(&tmux_session);

                    match result {
                        Ok(summary) => {
                            for project in &mut self.store.projects {
                                for feature in &mut project.features {
                                    if feature.tmux_session == tmux_session {
                                        feature.summary = Some(summary.clone());
                                        feature.summary_updated_at = Some(Utc::now());
                                        break;
                                    }
                                }
                            }
                            self.save()?;
                            self.message = Some(format!("Summary: {}", summary));
                        }
                        Err(e) => {
                            self.message = Some(format!("Failed to generate summary: {}", e));
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.summary_rx = None;
                }
            }
        }
        Ok(())
    }

    pub fn poll_codex_sidebar_metadata(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.codex_sidebar_metadata_rx.try_recv() {
            changed = true;
            self.codex_sidebar_metadata_inflight
                .remove(&result.cache_key);
            self.codex_session_title_cache
                .insert(result.cache_key.clone(), result.title);
            self.codex_session_prompt_cache
                .insert(result.cache_key.clone(), result.prompt);
            self.codex_session_model_cache
                .insert(result.cache_key, result.model_text);
        }
        changed
    }

    fn get_window_for_session(&self, tmux_session: &str, _agent: &AgentKind) -> Option<String> {
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.tmux_session == tmux_session {
                    return Self::get_agent_window(feature);
                }
            }
        }
        None
    }

    fn get_agent_window(feature: &Feature) -> Option<String> {
        let target_kind = match feature.agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Opencode => SessionKind::Opencode,
            AgentKind::Codex => SessionKind::Codex,
            AgentKind::Pi => SessionKind::Pi,
        };
        feature
            .sessions
            .iter()
            .find(|s| s.kind == target_kind)
            .map(|s| s.tmux_window.clone())
    }
}
