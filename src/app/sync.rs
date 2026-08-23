use super::GH_GRAPHQL_BACKOFF;
use super::*;
use crate::context_collectors::{
    ContextCollectionResult, ContextCollectionTarget, ContextCollector, SessionContextCollector,
};
use crate::context_tracking::SessionContextState;
use crate::github::{GhCli, GhGraphqlError};
use crate::project::{AgentKind, SessionKind, TokenUsageSourceMatch};
use crate::summary::SummaryManager;
use crate::tmux::TmuxManager;
use crate::token_tracking::{
    SessionTokenTracker, SessionTokenUsage, TokenPricingConfig, TokenUsageProvider,
    TokenUsageSource, format_token_usage, provider_for_session_kind,
};

use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

#[derive(Debug)]
struct ActivePrJob {
    feature_id: String,
    branch: String,
}

/// One repository's worth of work: a single query answers every feature that
/// lives in it.
struct ActivePrRepoJob {
    repo: PathBuf,
    features: Vec<ActivePrJob>,
}

#[derive(Debug)]
pub(crate) enum ActivePrLookup {
    Found(ActivePrStatus),
    NoPr,
    Failed(String),
    /// Not looked up, because the GraphQL budget ran out earlier in the sweep.
    /// Distinct from `Failed` so the badge keeps its previous value instead of
    /// reporting an error the feature had nothing to do with.
    Skipped,
}

/// What one sweep actually resolved, summarised for the debug log.
///
/// The sweep used to log only failures, which is how a totally broken query
/// stayed invisible for fourteen hours: every call failed, every badge kept its
/// previous value, and the only trace was a `DEBUG` line per feature that
/// nothing aggregated. A line that reports what a sweep *found* makes "0 badged,
/// 9 failed" as legible as an error, and a healthy sweep self-documents its
/// cost.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct ActivePrSweepOutcome {
    pub found: usize,
    pub no_pr: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl ActivePrSweepOutcome {
    pub fn count(&mut self, lookup: &ActivePrLookup) {
        match lookup {
            ActivePrLookup::Found(_) => self.found += 1,
            ActivePrLookup::NoPr => self.no_pr += 1,
            ActivePrLookup::Failed(_) => self.failed += 1,
            ActivePrLookup::Skipped => self.skipped += 1,
        }
    }

    pub fn summary(&self) -> String {
        format!(
            "PR sweep: {} badged, {} without a PR, {} failed, {} skipped",
            self.found, self.no_pr, self.failed, self.skipped
        )
    }
}

/// One completed pass of the dashboard PR refresh.
#[derive(Debug)]
pub(crate) struct ActivePrSweep {
    pub updates: Vec<ActivePrUpdate>,
    /// Set when any job hit GitHub's GraphQL point budget, so the main loop can
    /// stop scheduling sweeps until the budget resets.
    pub rate_limited: bool,
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
    context_conversation_id: Option<String>,
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
    context_result: Option<ContextCollectionResult>,
    context_checked_at: chrono::DateTime<chrono::Utc>,
}

pub(crate) struct SessionStatusBgResult {
    tracker: SessionTokenTracker,
    context_collector: SessionContextCollector,
    updates: Vec<SessionStatusUpdate>,
    sources_discovered: bool,
}

fn collect_jobs(
    store: &crate::project::ProjectStore,
    db: Option<&crate::db::AmfDb>,
    context_states: &HashMap<String, SessionContextState>,
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
                    context_conversation_id: context_states
                        .get(&session.id)
                        .and_then(|state| state.reset.conversation_id.clone()),
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
    mut context_collector: SessionContextCollector,
    jobs: Vec<SessionStatusJob>,
    pricing: &TokenPricingConfig,
) -> SessionStatusBgResult {
    let mut updates = Vec::with_capacity(jobs.len());
    let mut sources_discovered = false;
    let mut reserved_sources: HashSet<(String, TokenUsageProvider, String)> = HashSet::new();
    let mut reserved_context_ids: HashSet<(String, u8, String)> = HashSet::new();

    for job in jobs {
        let context_checked_at = Utc::now();
        let harness_key = session_kind_key(&job.kind);
        let excluded_context_ids = reserved_context_ids
            .iter()
            .filter(|(feature_id, kind, _)| feature_id == &job.feature_id && *kind == harness_key)
            .map(|(_, _, id)| id.clone())
            .collect::<Vec<_>>();
        if job.kind == SessionKind::Custom {
            updates.push(SessionStatusUpdate {
                session_id: job.session_id,
                source_action: SourceAction::NoChange,
                status_text: job.custom_status_text,
                token_usage: None,
                context_result: None,
                context_checked_at,
            });
            continue;
        }

        let Some(expected_provider) = provider_for_session_kind(&job.kind) else {
            let context_result = job.kind.is_agent_harness().then(|| {
                context_collector.collect(ContextCollectionTarget {
                    session_kind: &job.kind,
                    workdir: &job.workdir,
                    conversation_id: job
                        .context_conversation_id
                        .as_deref()
                        .or(job.claude_session_id.as_deref()),
                    excluded_conversation_ids: &excluded_context_ids,
                    session_created_at: job.created_at,
                    provider_id: None,
                    model_id: None,
                    runtime_payload: None,
                    fallback_usage: None,
                    fallback_context_limit: None,
                    collected_at: context_checked_at,
                })
            });
            reserve_context_result(
                &mut reserved_context_ids,
                &job.feature_id,
                harness_key,
                context_result.as_ref(),
            );
            updates.push(SessionStatusUpdate {
                session_id: job.session_id,
                source_action: SourceAction::NoChange,
                status_text: None,
                token_usage: None,
                context_result,
                context_checked_at,
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
        let conversation_id = job
            .context_conversation_id
            .as_deref()
            .or(job.claude_session_id.as_deref())
            .or_else(|| source.as_ref().map(|source| source.id.as_str()));
        let context_result = Some(context_collector.collect(ContextCollectionTarget {
            session_kind: &job.kind,
            workdir: &job.workdir,
            conversation_id,
            excluded_conversation_ids: &excluded_context_ids,
            session_created_at: job.created_at,
            provider_id: None,
            model_id: None,
            runtime_payload: None,
            fallback_usage: token_usage.as_ref(),
            fallback_context_limit: None,
            collected_at: context_checked_at,
        }));
        reserve_context_result(
            &mut reserved_context_ids,
            &job.feature_id,
            harness_key,
            context_result.as_ref(),
        );

        updates.push(SessionStatusUpdate {
            session_id: job.session_id,
            source_action: action,
            status_text,
            token_usage,
            context_result,
            context_checked_at,
        });
    }

    SessionStatusBgResult {
        tracker,
        context_collector,
        updates,
        sources_discovered,
    }
}

fn session_kind_key(kind: &SessionKind) -> u8 {
    match kind {
        SessionKind::Claude => 0,
        SessionKind::Opencode => 1,
        SessionKind::Codex => 2,
        SessionKind::Pi => 3,
        SessionKind::Terminal => 4,
        SessionKind::Nvim => 5,
        SessionKind::Vscode => 6,
        SessionKind::Custom => 7,
        SessionKind::Todos => 8,
    }
}

fn reserve_context_result(
    reserved: &mut HashSet<(String, u8, String)>,
    feature_id: &str,
    kind: u8,
    result: Option<&ContextCollectionResult>,
) {
    let conversation_id = match result {
        Some(ContextCollectionResult::Collected(sample)) => sample.reset.conversation_id.as_ref(),
        Some(ContextCollectionResult::ResetPending {
            conversation_id, ..
        }) => conversation_id.as_ref(),
        _ => None,
    };
    if let Some(conversation_id) = conversation_id {
        reserved.insert((feature_id.to_string(), kind, conversation_id.clone()));
    }
}

fn apply_bg_result(app: &mut App, result: SessionStatusBgResult) {
    app.token_tracker = result.tracker;
    app.context_collector = result.context_collector;
    let live_session_ids = result
        .updates
        .iter()
        .map(|update| update.session_id.as_str())
        .collect::<HashSet<_>>();
    app.context_states
        .retain(|session_id, _| live_session_ids.contains(session_id.as_str()));

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
                    apply_context_result(
                        &mut app.context_states,
                        &session.id,
                        update.context_result,
                        update.context_checked_at,
                    );
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

fn apply_context_result(
    states: &mut HashMap<String, SessionContextState>,
    session_id: &str,
    result: Option<ContextCollectionResult>,
    checked_at: chrono::DateTime<Utc>,
) {
    let Some(result) = result else {
        states.remove(session_id);
        return;
    };
    let state = states.entry(session_id.to_string()).or_default();
    match result {
        ContextCollectionResult::Collected(sample) => {
            if state.accept_sample(sample).is_err() {
                state.mark_unavailable(checked_at);
            }
        }
        ContextCollectionResult::ResetPending {
            conversation_id,
            event,
        } => state.begin_reset(conversation_id, event),
        ContextCollectionResult::Unavailable(_) => state.mark_unavailable(checked_at),
    }
}

pub(super) fn pane_shows_thinking_hint(content: &str) -> bool {
    let lower = content.to_lowercase();
    ["esc interrupt", "esc to interrupt", "ctrl+c to interrupt"]
        .iter()
        .any(|marker| lower.contains(marker))
}

pub(super) fn opencode_sidebar_thinking_state(
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
    /// Record that GitHub's GraphQL budget is exhausted, and say so once.
    ///
    /// Announced rather than silent: until this, a depleted budget showed up
    /// only as PR badges quietly going blank, and the first *visible* symptom
    /// was PR Triage failing — which is the one path that did not cause it.
    pub(crate) fn note_gh_graphql_rate_limited(&mut self) {
        let already_backing_off = self.gh_graphql_backoff_remaining().is_some();
        self.gh_graphql_limited_at = Some(Instant::now());
        if already_backing_off {
            return;
        }
        self.log_warn(
            "sync",
            format!(
                "GitHub GraphQL rate limit exhausted; pausing PR badge refresh for {} minutes",
                GH_GRAPHQL_BACKOFF.as_secs() / 60
            ),
        );
        self.push_toast_warning(format!(
            "GitHub's hourly API budget is used up — PR badges paused for {} min",
            GH_GRAPHQL_BACKOFF.as_secs() / 60
        ));
    }

    /// How much of the GraphQL backoff is left, or `None` when it has expired
    /// (or never started).
    pub(crate) fn gh_graphql_backoff_remaining(&self) -> Option<Duration> {
        let since = self.gh_graphql_limited_at?.elapsed();
        GH_GRAPHQL_BACKOFF
            .checked_sub(since)
            .filter(|d| !d.is_zero())
    }

    /// Refresh dashboard PR badges without ever blocking rendering or input.
    /// The main loop starts this on [`ACTIVE_PR_SYNC_INTERVAL`] and refuses to
    /// overlap jobs, so a slow `gh` invocation cannot build up a queue of
    /// redundant work.
    ///
    /// Work is batched **per repository**, not per feature: one query returns
    /// every open PR in a repo with its unresolved-thread count, and each
    /// feature's branch is matched against that locally. A worktree-heavy
    /// project is then the same cost as a single-feature one.
    pub fn sync_active_prs_background(&mut self) {
        if self.active_pr_bg.is_some() {
            return;
        }
        // A depleted budget stays depleted for the rest of the hour, so a
        // sweep launched now would spend nothing but failures.
        if self.gh_graphql_backoff_remaining().is_some() {
            return;
        }

        // Group by repository root. Features of one project share a repo, and
        // two projects can point at the same one, so the key is the repo path
        // rather than the project.
        let mut repos: Vec<ActivePrRepoJob> = Vec::new();
        for project in self.store.projects.iter().filter(|project| project.is_git) {
            if project.features.is_empty() {
                continue;
            }
            let features = project
                .features
                .iter()
                .map(|feature| ActivePrJob {
                    feature_id: feature.id.clone(),
                    branch: feature.branch.clone(),
                })
                .collect::<Vec<_>>();

            match repos.iter_mut().find(|job| job.repo == project.repo) {
                Some(existing) => existing.features.extend(features),
                None => repos.push(ActivePrRepoJob {
                    repo: project.repo.clone(),
                    features,
                }),
            }
        }
        if repos.is_empty() {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.active_pr_bg = Some(rx);
        std::thread::spawn(move || {
            let mut rate_limited = false;
            let mut updates = Vec::new();

            for job in repos {
                // One depleted budget means every remaining repository would
                // fail the same way, so the first to see it stops the rest.
                if rate_limited {
                    updates.extend(job.features.into_iter().map(|feature| ActivePrUpdate {
                        feature_id: feature.feature_id,
                        branch: feature.branch,
                        lookup: ActivePrLookup::Skipped,
                    }));
                    continue;
                }

                // No GitHub remote is a settled answer, not a failure: the
                // repository simply has no PRs to badge. Costs no API call.
                let Some((owner, repo)) = crate::github::owner_repo_from_remote(&job.repo) else {
                    updates.extend(job.features.into_iter().map(|feature| ActivePrUpdate {
                        feature_id: feature.feature_id,
                        branch: feature.branch,
                        lookup: ActivePrLookup::NoPr,
                    }));
                    continue;
                };

                let open = match GhCli::open_prs(&job.repo, &owner, &repo) {
                    Ok(open) => open,
                    Err(GhGraphqlError::RateLimited) => {
                        rate_limited = true;
                        updates.extend(job.features.into_iter().map(|feature| ActivePrUpdate {
                            feature_id: feature.feature_id,
                            branch: feature.branch,
                            lookup: ActivePrLookup::Skipped,
                        }));
                        continue;
                    }
                    Err(GhGraphqlError::Failed(error)) => {
                        // One failed repository must not blank the badges of
                        // features in the repositories that did answer.
                        updates.extend(job.features.into_iter().map(|feature| ActivePrUpdate {
                            feature_id: feature.feature_id,
                            branch: feature.branch,
                            lookup: ActivePrLookup::Failed(error.clone()),
                        }));
                        continue;
                    }
                };

                updates.extend(job.features.into_iter().map(|feature| {
                    let lookup = match open.iter().find(|pr| pr.head_ref == feature.branch) {
                        Some(pr) => ActivePrLookup::Found(ActivePrStatus {
                            branch: feature.branch.clone(),
                            head_sha: pr.head_sha.clone(),
                            number: pr.number,
                            unresolved_threads: Some(pr.unresolved_threads),
                        }),
                        // The repository answered and this branch was not in
                        // its open PRs: a confirmed absence, not a failure.
                        None => ActivePrLookup::NoPr,
                    };
                    ActivePrUpdate {
                        feature_id: feature.feature_id,
                        branch: feature.branch,
                        lookup,
                    }
                }));
            }

            let _ = tx.send(ActivePrSweep {
                updates,
                rate_limited,
            });
        });
    }

    /// Apply a completed dashboard PR refresh. Transient failures deliberately
    /// preserve the previous value; a confirmed no-PR result removes it.
    pub fn poll_active_pr_bg(&mut self) -> bool {
        let Some(rx) = self.active_pr_bg.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(sweep) => {
                self.active_pr_bg = None;
                if sweep.rate_limited {
                    self.note_gh_graphql_rate_limited();
                }
                self.apply_active_pr_updates(sweep.updates)
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
        let mut outcome = ActivePrSweepOutcome::default();
        for update in updates {
            outcome.count(&update.lookup);
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
                    let previous_pr_number = self
                        .active_prs
                        .get(&update.feature_id)
                        .map(|previous| previous.number);
                    if let Some(previous_pr_number) =
                        previous_pr_number.filter(|number| *number != status.number)
                    {
                        let workdir = self.store.projects.iter().find_map(|project| {
                            project
                                .features
                                .iter()
                                .find(|feature| feature.id == update.feature_id)
                                .map(|feature| feature.workdir.clone())
                        });
                        if let Some(workdir) = workdir {
                            changed |= self
                                .invalidate_pr_context_for_transition(&workdir, previous_pr_number);
                        }
                    }
                    if self.active_prs.get(&update.feature_id) != Some(&status) {
                        self.active_prs.insert(update.feature_id, status);
                        changed = true;
                    }
                }
                ActivePrLookup::NoPr => {
                    changed |= self.active_prs.remove(&update.feature_id).is_some();
                }
                // Nothing was looked up, so nothing is known: leave the
                // previous badge in place rather than blanking or erroring it.
                ActivePrLookup::Skipped => {}
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
        self.log_debug("github", outcome.summary());
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
        let jobs = collect_jobs(&self.store, self.db.as_ref(), &self.context_states);
        let pricing = self.config.token_pricing.clone();
        let tracker = std::mem::take(&mut self.token_tracker);
        let context_collector = std::mem::take(&mut self.context_collector);

        let (tx, rx) = std::sync::mpsc::channel();
        self.session_status_bg = Some(rx);

        std::thread::spawn(move || {
            let result = run_jobs(tracker, context_collector, jobs, &pricing);
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
            self.clear_attention(&tmux_session);
            self.clear_sidebar_state_for_session(&tmux_session);
        }

        // A question nobody has answered for `waiting_stale_minutes` has
        // stopped being news; ageing it out keeps the needs-attention list
        // showing what actually wants attention now.
        self.age_out_attention();

        // A feature that came back up through any path (picker restart, another
        // AMF window) is no longer "stopped on purpose", so a future missing
        // session should reach the recovery dialog again.
        if !self.user_stopped_features.is_empty() {
            let live_feature_ids: HashSet<String> = self
                .store
                .projects
                .iter()
                .flat_map(|project| project.features.iter())
                .filter(|feature| live_sessions.contains(feature.tmux_session.as_str()))
                .map(|feature| feature.id.clone())
                .collect();
            self.user_stopped_features
                .retain(|id| !live_feature_ids.contains(id));
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
        let jobs = collect_jobs(&self.store, self.db.as_ref(), &self.context_states);
        let owned_tracker = std::mem::take(tracker);
        let context_collector = std::mem::take(&mut self.context_collector);
        let pricing = self.config.token_pricing.clone();
        let result = run_jobs(owned_tracker, context_collector, jobs, &pricing);
        apply_bg_result(self, result);
        *tracker = std::mem::take(&mut self.token_tracker);
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
                // Only the *transition* back into thinking means the session
                // started producing output again. Clearing on every poll while
                // thinking would retire attention the session raised while it
                // was already busy — Claude keeps its thinking marker set
                // through a permission prompt, and OpenCode stays busy while
                // its `question` tool is open, so a real Question would be
                // dropped within one poll of being raised.
                //
                // This is the "clear on new output" path for every harness
                // whose thinking state AMF can observe; harnesses that report
                // no thinking at all clear on open instead (see
                // `HarnessCapabilities::clears_on_open`).
                if !was_thinking && self.clear_attention(&sid) {
                    pending_inputs_changed = true;
                }
                if let Some(watch) = self.awaiting_review_fixes.get_mut(&sid) {
                    watch.started_thinking = true;
                }
                let before = self.pending_inputs.len();
                self.pending_inputs.retain(|p| {
                    !(p.is_session_wait()
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
                // Agent-agnostic completion signal, mirroring the pending-input
                // fallback below: a session that stops working has finished a
                // turn. Harness hooks report the same thing more promptly and
                // more precisely; this covers the gap when a hook is missing or
                // slow. It is `Inferred`, so it cannot downgrade a Question a
                // harness raised: a blocked agent looks exactly like a quiet
                // one from here.
                self.record_attention(
                    &sid,
                    &agent,
                    crate::app::attention::AttentionState::CompletedAwaitingReview,
                    crate::app::attention::AttentionSource::Inferred,
                );

                // A session we dispatched review-fix feedback to, that we've
                // since observed actually start working, going idle means
                // "fixes ready" — a more specific and actionable signal than
                // the generic waiting-for-input notification below, so it
                // takes priority and skips that path entirely.
                let review_ready = self
                    .awaiting_review_fixes
                    .get(&sid)
                    .is_some_and(|w| w.started_thinking);
                if review_ready {
                    self.awaiting_review_fixes.remove(&sid);
                    let any_pending_for_feature = self.pending_inputs.iter().any(|p| {
                        p.notification_type == "review-ready"
                            && p.project_name.as_deref() == Some(&project_name)
                            && p.feature_name.as_deref() == Some(&feature_name)
                    });
                    if !any_pending_for_feature {
                        pending_inputs_changed = true;
                        self.queue_pending_input(PendingInput {
                            session_id: sid.clone(),
                            cwd,
                            message: "Fixes ready — re-review?".to_string(),
                            notification_type: "review-ready".to_string(),
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
                                "Detected review fixes ready for {} (agent={}, session={})",
                                feature_name,
                                agent.display_name(),
                                sid
                            ),
                        );
                    }
                    continue;
                }

                let any_pending_for_feature = self.pending_inputs.iter().any(|p| {
                    p.project_name.as_deref() == Some(&project_name)
                        && p.feature_name.as_deref() == Some(&feature_name)
                });
                if !any_pending_for_feature {
                    pending_inputs_changed = true;
                    self.queue_pending_input(PendingInput {
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

    pub(super) fn is_session_marked_thinking(tmux_session: &str) -> bool {
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
        let mut matched: Option<(String, String, String)> = None;
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.tmux_session != tmux_session {
                    continue;
                }
                let codex_session_id = feature
                    .sessions
                    .iter()
                    .find(|s| s.kind == SessionKind::Codex && s.tmux_window == tmux_window)
                    .map(|s| s.id.clone());
                if let Some(codex_session_id) = codex_session_id {
                    matched = Some((project.name.clone(), feature.name.clone(), codex_session_id));
                    break;
                }
            }
            if matched.is_some() {
                break;
            }
        }

        if let Some((project_name, feature_name, feature_session_id)) = matched {
            self.ipc_thinking_sessions.insert(tmux_session.to_string());
            self.ipc_thinking_feature_sessions
                .insert(feature_session_id);
            self.pending_inputs.retain(|p| {
                !(p.is_session_wait()
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
