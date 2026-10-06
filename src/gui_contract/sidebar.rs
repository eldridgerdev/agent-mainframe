//! Sidebar parity with the TUI dashboard tree (`src/ui/list.rs::draw`).
//!
//! The workspace snapshot already carries the persisted `Project`/`Feature`
//! shape, so everything the TUI tree reads straight off the store (nickname,
//! branch, `[repo]`, issue source, mode/review/plan/remote badges, pending
//! worktree script, ready, collapse state, stopped sessions, summary) reaches
//! the GUI without anything here. This module adds what the tree *derives*:
//! display strings the TUI computes at render time and the runtime signals.
//!
//! Every runtime signal is either derived by this GUI process through the
//! same engine or source the TUI uses, or read from a shared persisted form.
//! Nothing here reports a signal only a TUI process could know:
//!
//! | Signal | Source in the GUI |
//! | --- | --- |
//! | Context window, session status text, token usage | The shared session-status collector (`App::sync_session_status_background`), run by this process |
//! | Thinking | The hooks' `/tmp/amf-thinking` marker files (Claude/Codex/Pi) and the OpenCode sidebar cache, the TUI's own non-IPC sources |
//! | Waiting for input / pending input | The hooks' notification files on disk, read without consuming them |
//! | Open PR + unresolved threads | The shared dashboard PR sweep (`App::sync_active_prs_background`), run by this process |
//! | Merged/closed PR | `pr_terminal_state` in SQLite plus this process's own sweep |
//!
//! Deliberately absent (TUI-process state with no shared form): the attention
//! reason, a hook the TUI moved to the background, a TUI-side deletion in
//! progress, TUI summary generation and a TUI AI PR review. `remote_control`
//! and `pending_worktree_script` are on the snapshot's `Feature`, but the
//! store does not persist them (`db/store.rs` loads both as `false`), so only
//! the process that set them ever sees them. While a TUI owns the IPC socket
//! the hooks report to it instead of the marker/notification files, so the GUI
//! under-reports thinking/waiting rather than guessing.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::{GuiError, GuiHandle, GuiResult, WorkspaceSnapshot};
use crate::app::App;
use crate::context_display::format_context_indicator;
use crate::context_tracking::ContextBand;
use crate::custom_session_icons::resolve_custom_session_icon;
use crate::github::TerminalPrState;
use crate::project::{Project, ProjectStatus, ProjectStore, SessionKind};
use crate::token_tracking::{
    aggregate_token_usage, format_feature_token_usage, provider_for_session_kind,
};
use crate::ui::list::{format_age, shorten_path};

/// Mirrors the TUI's `NOTIFICATION_MAX_AGE` (`app/notifications.rs`): the TUI
/// prunes notification files older than this at startup, so the GUI ignores
/// them rather than reporting a request the TUI would discard.
const NOTIFICATION_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// How often this process refreshes context/status/usage, matching the TUI
/// event loop's background session-status cadence.
const SESSION_STATUS_INTERVAL: Duration = Duration::from_secs(5);

/// Everything the TUI tree derives at render time, keyed by stable id so the
/// frontend joins it onto the snapshot's `projects`.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SidebarSnapshot {
    pub projects: BTreeMap<String, SidebarProject>,
    pub features: BTreeMap<String, SidebarFeature>,
    pub sessions: BTreeMap<String, SidebarSession>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SidebarProject {
    /// The repository path with `~` for the home directory, as the TUI shows it.
    pub repo_display: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SidebarFeature {
    pub workdir_display: String,
    /// `just now` / `5m ago` / `3h ago` / `2d ago` / `Oct 01`.
    pub created_age: String,
    /// The GitHub issue this feature was created to fix, as the TUI labels
    /// it (`github.com/owner/repo#N`).
    pub issue: Option<String>,
    /// Age of the persisted AI summary, when there is one.
    pub summary_age: Option<String>,
    /// Aggregate agent-session token usage with configured pricing, exactly
    /// the TUI's `usage … eff · $…` text. Absent until usage is known.
    pub usage: Option<String>,
    pub pr: Option<SidebarPr>,
    /// An agent session is mid-turn, by this process's own observation.
    pub thinking: bool,
    /// A request is waiting on disk for this feature (any kind).
    pub waiting_for_input: bool,
    /// A waiting request other than a diff review: the TUI's `?` marker.
    pub pending_input: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SidebarPr {
    Open {
        number: u32,
        /// `None` when the PR resolved but its threads could not be read.
        unresolved_threads: Option<usize>,
    },
    Merged {
        number: u32,
    },
    Closed {
        number: u32,
    },
}

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct SidebarSession {
    /// The second line under the session row (usage summary, or a custom
    /// session's own status file).
    pub status_text: Option<String>,
    pub context: Option<SidebarContext>,
    /// A custom session's configured `icon` (plain text) and its resolved
    /// `icon_nerd` glyph, which needs a Nerd Font to render.
    pub icon: Option<String>,
    pub icon_nerd: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SidebarContext {
    /// The TUI's complete label, e.g. `Ctx ~74% WARNING STALE`. Empty while a
    /// reset is pending: no old percentage is ever shown for a new window.
    pub text: String,
    pub band: SidebarContextBand,
    pub stale: bool,
    pub pending_reset: bool,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SidebarContextBand {
    Normal,
    Warning,
    Critical,
}

impl From<ContextBand> for SidebarContextBand {
    fn from(band: ContextBand) -> Self {
        match band {
            ContextBand::Normal => Self::Normal,
            ContextBand::Warning => Self::Warning,
            ContextBand::Critical => Self::Critical,
        }
    }
}

/// Which row's persisted collapse flag to set. A feature row carries its
/// project too, for the same stale-snapshot reason `FeatureTarget` does.
#[derive(Debug, Clone, Deserialize)]
pub struct CollapseTarget {
    pub project_id: String,
    pub feature_id: Option<String>,
}

/// Background-source cadence for one GUI process. Owned by the desktop shell
/// rather than [`GuiHandle`], so a test handle never starts a `gh` sweep.
#[derive(Debug, Default)]
pub struct SidebarClock {
    last_session_status: Option<Instant>,
    last_pr_sweep: Option<Instant>,
}

/// Build the sidebar projection for `projects` (the snapshot's own,
/// live-corrected copy) from `app`'s runtime state.
pub(crate) fn project_sidebar(app: &App, projects: &[Project]) -> SidebarSnapshot {
    let mut sidebar = SidebarSnapshot::default();
    let waiting = waiting_requests(projects);
    let terminal_on_disk = app
        .db
        .as_ref()
        .and_then(|db| db.load_all_pr_terminal_state().ok())
        .unwrap_or_default();

    for project in projects {
        sidebar.projects.insert(
            project.id.clone(),
            SidebarProject {
                repo_display: shorten_path(&project.repo),
            },
        );
        // Read the project's config only when a custom session needs its icon.
        let has_custom = project
            .features
            .iter()
            .flat_map(|feature| &feature.sessions)
            .any(|session| session.kind == SessionKind::Custom);
        let custom_sessions = if has_custom {
            app.extension_for_repo(&project.repo).custom_sessions
        } else {
            Vec::new()
        };
        let repo_key = project.repo.to_string_lossy().to_string();

        for feature in &project.features {
            let usage = aggregate_token_usage(
                feature
                    .sessions
                    .iter()
                    .filter(|session| provider_for_session_kind(&session.kind).is_some())
                    .filter_map(|session| session.token_usage.as_ref()),
            )
            .map(|usage| format_feature_token_usage(&usage, &app.config.token_pricing));
            let pr = app
                .active_prs
                .get(&feature.id)
                .map(|pr| SidebarPr::Open {
                    number: pr.number,
                    unresolved_threads: pr.unresolved_threads,
                })
                .or_else(|| {
                    app.terminal_prs
                        .get(&feature.id)
                        .or_else(|| {
                            terminal_on_disk.get(&(repo_key.clone(), feature.branch.clone()))
                        })
                        .map(|pr| match pr.state {
                            TerminalPrState::Merged => SidebarPr::Merged { number: pr.number },
                            TerminalPrState::Closed => SidebarPr::Closed { number: pr.number },
                        })
                });
            let requests = waiting.get(&feature.id);
            sidebar.features.insert(
                feature.id.clone(),
                SidebarFeature {
                    workdir_display: shorten_path(&feature.workdir),
                    created_age: format_age(feature.created_at),
                    issue: feature.issue_source.as_ref().map(|source| {
                        format!("{}#{}", source.canonical_repository(), source.number)
                    }),
                    summary_age: feature
                        .summary
                        .as_ref()
                        .and(feature.summary_updated_at)
                        .map(format_age),
                    usage,
                    pr,
                    thinking: feature.status != ProjectStatus::Stopped
                        && app.thinking_from_shared_sources(&feature.tmux_session, &feature.agent),
                    waiting_for_input: requests.is_some(),
                    pending_input: requests
                        .is_some_and(|kinds| kinds.iter().any(|kind| kind != "diff-review")),
                },
            );

            for session in &feature.sessions {
                let context = session
                    .kind
                    .is_agent_harness()
                    .then(|| app.context_states.get(&session.id))
                    .flatten()
                    .and_then(|state| match state.snapshot.as_ref() {
                        Some(snapshot) => {
                            let indicator = format_context_indicator(snapshot);
                            Some(SidebarContext {
                                text: indicator.text,
                                band: indicator.band.into(),
                                stale: indicator.stale,
                                pending_reset: false,
                            })
                        }
                        None => state.awaiting_post_reset.then_some(SidebarContext {
                            text: String::new(),
                            band: SidebarContextBand::Normal,
                            stale: false,
                            pending_reset: true,
                        }),
                    });
                let custom = (session.kind == SessionKind::Custom)
                    .then(|| {
                        custom_sessions
                            .iter()
                            .find(|config| config.name == session.label)
                    })
                    .flatten();
                sidebar.sessions.insert(
                    session.id.clone(),
                    SidebarSession {
                        status_text: session.status_text.clone(),
                        context,
                        icon: custom.and_then(|config| config.icon.clone()),
                        icon_nerd: custom.and_then(|config| {
                            config
                                .icon_nerd
                                .as_deref()
                                .map(|icon| resolve_custom_session_icon(icon).to_string())
                        }),
                    },
                );
            }
        }
    }
    sidebar
}

#[derive(Deserialize)]
struct NotificationHeader {
    #[serde(alias = "type")]
    notification_type: Option<String>,
    amf_session: Option<String>,
    cwd: Option<String>,
}

/// Notification kinds waiting on disk, per feature id. Reads the same two
/// places the TUI's file scan does (each feature's `.claude/notifications/`,
/// then the global directory matched by AMF session, then cwd) and never
/// removes, answers or collapses anything.
fn waiting_requests(projects: &[Project]) -> HashMap<String, Vec<String>> {
    let mut waiting: HashMap<String, Vec<String>> = HashMap::new();
    let now = SystemTime::now();
    for feature in projects.iter().flat_map(|project| &project.features) {
        for header in read_headers(&feature.workdir.join(".claude").join("notifications"), now) {
            waiting
                .entry(feature.id.clone())
                .or_default()
                .push(header.notification_type.unwrap_or_default());
        }
    }
    let global = crate::project::amf_config_dir().join("notifications");
    for header in read_headers(&global, now) {
        let cwd = PathBuf::from(header.cwd.as_deref().unwrap_or_default());
        if let Some(feature_id) = owning_feature(projects, header.amf_session.as_deref(), &cwd) {
            waiting
                .entry(feature_id)
                .or_default()
                .push(header.notification_type.unwrap_or_default());
        }
    }
    waiting
}

fn read_headers(dir: &Path, now: SystemTime) -> Vec<NotificationHeader> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().extension().and_then(|e| e.to_str()) == Some("json"))
        .filter(|entry| {
            !entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > NOTIFICATION_MAX_AGE)
        })
        .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
        .filter_map(|data| serde_json::from_str(&data).ok())
        .collect()
}

/// `App::project_feature_for_message`'s rule: the feature whose tmux session
/// is the hook's `AMF_SESSION`, else the one whose workdir best contains (or
/// is contained by) the hook's cwd.
fn owning_feature(projects: &[Project], amf_session: Option<&str>, cwd: &Path) -> Option<String> {
    let features = || projects.iter().flat_map(|project| &project.features);
    if let Some(session) = amf_session.filter(|session| !session.is_empty())
        && let Some(feature) = features().find(|feature| feature.tmux_session == session)
    {
        return Some(feature.id.clone());
    }
    if cwd.as_os_str().is_empty() {
        return None;
    }
    features()
        .filter(|feature| cwd.starts_with(&feature.workdir) || feature.workdir.starts_with(cwd))
        .max_by_key(|feature| feature.workdir.as_os_str().len())
        .map(|feature| feature.id.clone())
}

/// Keep the collector's last results across a store reload: `status_text`
/// and `token_usage` are never persisted, so adopting another process's
/// commit would otherwise blank them until the next collection.
pub(crate) fn carry_runtime_session_fields(previous: &ProjectStore, next: &mut ProjectStore) {
    let mut runtime = HashMap::new();
    for session in previous
        .projects
        .iter()
        .flat_map(|project| &project.features)
        .flat_map(|feature| &feature.sessions)
    {
        runtime.insert(
            session.id.as_str(),
            (session.status_text.clone(), session.token_usage.clone()),
        );
    }
    for session in next
        .projects
        .iter_mut()
        .flat_map(|project| &mut project.features)
        .flat_map(|feature| &mut feature.sessions)
    {
        if let Some((status_text, token_usage)) = runtime.remove(session.id.as_str()) {
            session.status_text = status_text;
            session.token_usage = token_usage;
        }
    }
}

impl GuiHandle {
    /// Apply finished background collections and start the next ones when
    /// due. Never blocks on I/O: collection and the `gh` sweep run on their
    /// own threads, exactly as in the TUI's event loop.
    pub fn drive_sidebar_sources(&mut self, clock: &mut SidebarClock) {
        let app = &mut self.app;
        app.poll_session_status_bg();
        app.poll_active_pr_bg();
        app.poll_sidebar_load_results();

        let due =
            |last: Option<Instant>, every: Duration| last.is_none_or(|at| at.elapsed() >= every);
        if app.session_status_bg.is_none()
            && due(clock.last_session_status, SESSION_STATUS_INTERVAL)
        {
            app.sync_session_status_background();
            clock.last_session_status = Some(Instant::now());
        }
        if app.active_pr_bg.is_none()
            && due(clock.last_pr_sweep, crate::app::ACTIVE_PR_SYNC_INTERVAL)
        {
            app.sync_active_prs_background();
            // A workspace with no Git features yet starts no sweep; ask again
            // on the next poll rather than five minutes after the first one.
            if app.active_pr_bg.is_some() {
                clock.last_pr_sweep = Some(Instant::now());
            }
        }
    }

    /// Persist a project or feature row's collapse state in the shared store,
    /// so it round-trips with the TUI's dashboard tree.
    pub fn set_collapsed(
        &mut self,
        target: CollapseTarget,
        collapsed: bool,
    ) -> GuiResult<WorkspaceSnapshot> {
        self.refresh_store()?;
        let apply = |store: &mut ProjectStore| -> bool {
            let Some(project) = store
                .projects
                .iter_mut()
                .find(|project| project.id == target.project_id)
            else {
                return false;
            };
            match target.feature_id.as_deref() {
                None => project.collapsed = collapsed,
                Some(feature_id) => {
                    let Some(feature) = project
                        .features
                        .iter_mut()
                        .find(|feature| feature.id == feature_id)
                    else {
                        return false;
                    };
                    feature.collapsed = collapsed;
                }
            }
            true
        };
        if !apply(&mut self.app.store) {
            return Err(GuiError::not_found(
                "That project or feature was deleted; refresh and retry",
            ));
        }
        match self.app.save_reapplying(apply)? {
            crate::app::ReapplyOutcome::Saved => Ok(self.broadcast_snapshot()),
            crate::app::ReapplyOutcome::TargetGone => Err(GuiError::not_found(
                "That project or feature was deleted; refresh and retry",
            )),
            crate::app::ReapplyOutcome::Conflict => Err(GuiError::conflict(
                crate::app::SAVE_CONFLICT_MESSAGE.to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ActivePrStatus;
    use crate::context_tracking::{
        ContextFreshness, ContextPercentage, ContextProvenance, ContextResetMetadata,
        SessionContextSnapshot, SessionContextState,
    };
    use crate::github::TerminalPr;
    use crate::project::{
        AgentKind, Feature, FeatureSession, IssueCommentStatus, IssueSource, VibeMode,
    };
    use crate::token_tracking::{SessionTokenUsage, TokenUsageProvider, TokenUsageSource};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use chrono::Utc;

    fn session(id: &str, kind: SessionKind, label: &str) -> FeatureSession {
        FeatureSession {
            id: id.to_string(),
            kind,
            label: label.to_string(),
            tmux_window: label.to_ascii_lowercase(),
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
            stopped: false,
        }
    }

    fn feature(id: &str, workdir: &Path, sessions: Vec<FeatureSession>) -> Feature {
        let mut feature = Feature::new_for_project(
            "demo",
            id.to_string(),
            id.to_string(),
            workdir.to_path_buf(),
            true,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        feature.id = id.to_string();
        feature.tmux_session = format!("amf-sidebar-test-{}-{id}", std::process::id());
        feature.status = ProjectStatus::Idle;
        feature.sessions = sessions;
        feature
    }

    fn store_with(features: Vec<Feature>, repo: &Path) -> ProjectStore {
        let mut project = Project::new("demo".into(), repo.to_path_buf(), true, AgentKind::Claude);
        project.id = "proj-1".to_string();
        project.features = features;
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        store
    }

    fn app_for(store: ProjectStore) -> App {
        App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        )
    }

    fn usage(input: u64) -> SessionTokenUsage {
        SessionTokenUsage {
            source: TokenUsageSource {
                provider: TokenUsageProvider::Claude,
                id: "claude-1".into(),
            },
            input_tokens: input,
            output_tokens: 2_000,
            cache_read_tokens: 5_000,
            cache_write_tokens: 1_000,
            reasoning_tokens: 0,
            total_tokens: input + 8_000,
        }
    }

    fn context(percentage: u8, band: ContextBand, stale: bool) -> SessionContextState {
        let now = Utc::now();
        SessionContextState {
            snapshot: Some(SessionContextSnapshot {
                used_tokens: u64::from(percentage) * 1_000,
                context_limit: std::num::NonZeroU64::new(100_000).unwrap(),
                percentage: ContextPercentage::clamped(i64::from(percentage)),
                band,
                provenance: ContextProvenance::Estimated,
                freshness: if stale {
                    ContextFreshness::Stale
                } else {
                    ContextFreshness::Fresh
                },
                sampled_at: now,
                checked_at: now,
                reset: ContextResetMetadata::default(),
            }),
            reset: ContextResetMetadata::default(),
            awaiting_post_reset: false,
        }
    }

    /// The contract the frontend's sidebar is written against: every piece of
    /// information `ui/list.rs::draw` shows reaches the serialized snapshot,
    /// either on the persisted `projects` or in the derived `sidebar`.
    #[test]
    fn the_workspace_snapshot_carries_every_tui_tree_field() {
        let dir = tempfile::tempdir().unwrap();
        let mut agent = session("s-claude", SessionKind::Claude, "Claude 1");
        agent.token_usage = Some(usage(10_000));
        agent.status_text = Some("usage 21.8k eff".into());
        let mut stopped = session("s-term", SessionKind::Terminal, "Shell");
        stopped.stopped = true;
        let mut feat = feature("feat-1", dir.path(), vec![agent, stopped]);
        feat.nickname = Some("Nick".into());
        feat.issue_source = Some(IssueSource {
            host: "github.com".into(),
            owner: "acme".into(),
            repository: "widget".into(),
            number: 42,
            comment_status: IssueCommentStatus::Posted,
        });
        feat.summary = Some("Rounded totals".into());
        feat.summary_updated_at = Some(Utc::now());
        feat.remote_control = true;
        feat.pending_worktree_script = true;
        feat.review = true;
        feat.plan_mode = true;
        let mut app = app_for(store_with(vec![feat], dir.path()));
        app.context_states
            .insert("s-claude".into(), context(74, ContextBand::Warning, true));
        app.active_prs.insert(
            "feat-1".into(),
            ActivePrStatus {
                branch: "feat-1".into(),
                head_sha: "abc".into(),
                number: 321,
                unresolved_threads: Some(4),
            },
        );
        let gui = GuiHandle::from_app(app);

        let json = serde_json::to_value(gui.snapshot()).unwrap();

        let project = &json["projects"][0];
        for key in ["id", "name", "repo", "collapsed", "is_git", "features"] {
            assert!(project.get(key).is_some(), "project.{key} missing");
        }
        let feature = &project["features"][0];
        for key in [
            "id",
            "name",
            "branch",
            "workdir",
            "is_worktree",
            "sessions",
            "collapsed",
            "mode",
            "review",
            "plan_mode",
            "agent",
            "remote_control",
            "pending_worktree_script",
            "ready",
            "status",
            "created_at",
            "summary",
            "summary_updated_at",
            "nickname",
            "issue_source",
        ] {
            assert!(feature.get(key).is_some(), "feature.{key} missing");
        }
        assert_eq!(feature["sessions"][1]["stopped"], true);

        let sidebar = &json["sidebar"];
        assert!(sidebar["projects"]["proj-1"]["repo_display"].is_string());
        let derived = &sidebar["features"]["feat-1"];
        for key in [
            "workdir_display",
            "created_age",
            "issue",
            "summary_age",
            "usage",
            "pr",
            "thinking",
            "waiting_for_input",
            "pending_input",
        ] {
            assert!(derived.get(key).is_some(), "sidebar feature.{key} missing");
        }
        assert_eq!(derived["issue"], "github.com/acme/widget#42");
        assert_eq!(derived["usage"], "usage 21.8k eff · $0.07");
        assert_eq!(derived["created_age"], "just now");
        assert_eq!(
            derived["pr"],
            serde_json::json!({"state": "open", "number": 321, "unresolved_threads": 4})
        );
        let agent = &sidebar["sessions"]["s-claude"];
        assert_eq!(agent["status_text"], "usage 21.8k eff");
        let context = &agent["context"];
        assert!(
            context["text"]
                .as_str()
                .unwrap()
                .starts_with("Ctx ~74% WARNING STALE"),
            "{context}"
        );
        assert_eq!(context["band"], "warning");
        assert_eq!(context["stale"], true);
        assert_eq!(context["pending_reset"], false);
        assert!(sidebar["sessions"]["s-term"]["context"].is_null());
    }

    #[test]
    fn context_is_agent_only_and_a_pending_reset_shows_no_old_percentage() {
        let dir = tempfile::tempdir().unwrap();
        let feat = feature(
            "feat-1",
            dir.path(),
            vec![
                session("s-codex", SessionKind::Codex, "Codex"),
                session("s-nvim", SessionKind::Nvim, "Editor"),
            ],
        );
        let mut app = app_for(store_with(vec![feat], dir.path()));
        app.context_states.insert(
            "s-codex".into(),
            SessionContextState {
                snapshot: None,
                reset: ContextResetMetadata::default(),
                awaiting_post_reset: true,
            },
        );
        app.context_states
            .insert("s-nvim".into(), context(95, ContextBand::Critical, false));

        let sidebar = project_sidebar(&app, &app.store.projects);

        let codex = sidebar.sessions["s-codex"].context.as_ref().unwrap();
        assert!(codex.pending_reset);
        assert!(codex.text.is_empty());
        assert!(sidebar.sessions["s-nvim"].context.is_none());
    }

    #[test]
    fn an_open_pr_outranks_a_terminal_one_and_terminal_state_is_read_from_the_db() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::AmfDb::open(tmp_db.path()).unwrap();
        let repo = dir.path().to_string_lossy().to_string();
        db.save_pr_terminal_state(
            &repo,
            "merged-branch",
            &TerminalPr {
                number: 7,
                state: TerminalPrState::Merged,
                at: "2026-10-01T00:00:00Z".into(),
            },
        )
        .unwrap();
        db.save_pr_terminal_state(
            &repo,
            "reopened",
            &TerminalPr {
                number: 8,
                state: TerminalPrState::Closed,
                at: "2026-10-01T00:00:00Z".into(),
            },
        )
        .unwrap();
        let mut merged = feature("merged-branch", dir.path(), vec![]);
        merged.branch = "merged-branch".into();
        let mut reopened = feature("reopened", dir.path(), vec![]);
        reopened.branch = "reopened".into();
        let mut app = app_for(store_with(vec![merged, reopened], dir.path()));
        app.db = Some(db);
        app.active_prs.insert(
            "reopened".into(),
            ActivePrStatus {
                branch: "reopened".into(),
                head_sha: "abc".into(),
                number: 9,
                unresolved_threads: None,
            },
        );

        let sidebar = project_sidebar(&app, &app.store.projects);

        assert_eq!(
            sidebar.features["merged-branch"].pr,
            Some(SidebarPr::Merged { number: 7 })
        );
        assert_eq!(
            sidebar.features["reopened"].pr,
            Some(SidebarPr::Open {
                number: 9,
                unresolved_threads: None
            })
        );
    }

    #[test]
    fn notification_files_mark_waiting_without_being_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let review_dir = dir.path().join("review");
        let input_dir = dir.path().join("input");
        let stale_dir = dir.path().join("stale");
        for workdir in [&review_dir, &input_dir, &stale_dir] {
            std::fs::create_dir_all(workdir.join(".claude/notifications")).unwrap();
        }
        let review_file = review_dir.join(".claude/notifications/a.json");
        std::fs::write(&review_file, r#"{"type":"diff-review","message":"edit"}"#).unwrap();
        std::fs::write(
            input_dir.join(".claude/notifications/b.json"),
            r#"{"notification_type":"input-request"}"#,
        )
        .unwrap();
        std::fs::write(input_dir.join(".claude/notifications/ignored.txt"), "x").unwrap();
        let stale_file = stale_dir.join(".claude/notifications/c.json");
        std::fs::write(&stale_file, r#"{"type":"input-request"}"#).unwrap();
        let old = SystemTime::now() - NOTIFICATION_MAX_AGE - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&stale_file)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let app = app_for(store_with(
            vec![
                feature("review", &review_dir, vec![]),
                feature("input", &input_dir, vec![]),
                feature("stale", &stale_dir, vec![]),
            ],
            dir.path(),
        ));

        let sidebar = project_sidebar(&app, &app.store.projects);

        let review = &sidebar.features["review"];
        assert!(review.waiting_for_input && !review.pending_input);
        let input = &sidebar.features["input"];
        assert!(input.waiting_for_input && input.pending_input);
        let stale = &sidebar.features["stale"];
        assert!(!stale.waiting_for_input && !stale.pending_input);
        assert!(review_file.exists() && stale_file.exists());
    }

    #[test]
    fn global_notifications_resolve_by_amf_session_then_deepest_workdir() {
        let dir = tempfile::tempdir().unwrap();
        let outer = feature("outer", dir.path(), vec![]);
        let inner = feature("inner", &dir.path().join("nested"), vec![]);
        let session = inner.tmux_session.clone();
        let projects = store_with(vec![outer, inner], dir.path()).projects;

        assert_eq!(
            owning_feature(&projects, Some(&session), Path::new("/elsewhere")),
            Some("inner".into())
        );
        assert_eq!(
            owning_feature(&projects, None, &dir.path().join("nested/src")),
            Some("inner".into())
        );
        assert_eq!(
            owning_feature(&projects, Some(""), &dir.path().join("lib")),
            Some("outer".into())
        );
        assert_eq!(owning_feature(&projects, None, Path::new("")), None);
    }

    #[test]
    fn thinking_follows_the_shared_marker_only_while_the_feature_runs() {
        let dir = tempfile::tempdir().unwrap();
        let running = feature("running", dir.path(), vec![]);
        let mut stopped = feature("stopped", dir.path(), vec![]);
        stopped.status = ProjectStatus::Stopped;
        let markers = [running.tmux_session.clone(), stopped.tmux_session.clone()];
        std::fs::create_dir_all("/tmp/amf-thinking").unwrap();
        for marker in &markers {
            std::fs::write(Path::new("/tmp/amf-thinking").join(marker), "").unwrap();
        }
        let app = app_for(store_with(vec![running, stopped], dir.path()));

        let sidebar = project_sidebar(&app, &app.store.projects);
        for marker in &markers {
            let _ = std::fs::remove_file(Path::new("/tmp/amf-thinking").join(marker));
        }

        assert!(sidebar.features["running"].thinking);
        assert!(!sidebar.features["stopped"].thinking);
    }

    #[test]
    fn custom_sessions_carry_their_configured_icons() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("amf.json"),
            r#"{"custom_sessions":[{"name":"Dev server","icon":"S","icon_nerd":"nf-md-server"}]}"#,
        )
        .unwrap();
        let feat = feature(
            "feat-1",
            dir.path(),
            vec![
                session("s-dev", SessionKind::Custom, "Dev server"),
                session("s-other", SessionKind::Custom, "Unconfigured"),
            ],
        );
        let app = app_for(store_with(vec![feat], dir.path()));

        let sidebar = project_sidebar(&app, &app.store.projects);

        assert_eq!(sidebar.sessions["s-dev"].icon.as_deref(), Some("S"));
        assert_eq!(
            sidebar.sessions["s-dev"].icon_nerd.as_deref(),
            Some("\u{f048b}")
        );
        assert_eq!(sidebar.sessions["s-other"].icon, None);
    }

    fn db_fixture() -> (GuiHandle, tempfile::NamedTempFile, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with(
            vec![feature(
                "feat-1",
                dir.path(),
                vec![session("s-claude", SessionKind::Claude, "Claude")],
            )],
            dir.path(),
        );
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::AmfDb::open(tmp_db.path()).unwrap();
        db.save_store(&store).unwrap();
        let mut tmux = MockTmuxOps::new();
        // Mutations broadcast a live snapshot; nothing is running here.
        tmux.expect_list_sessions().returning(|| Ok(Vec::new()));
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.db = Some(db);
        (GuiHandle::from_app(app), tmp_db, dir)
    }

    #[test]
    fn collapse_state_round_trips_through_the_shared_store() {
        let (mut gui, tmp_db, _dir) = db_fixture();
        let project = CollapseTarget {
            project_id: "proj-1".into(),
            feature_id: None,
        };
        let feature = CollapseTarget {
            project_id: "proj-1".into(),
            feature_id: Some("feat-1".into()),
        };

        let snapshot = gui.set_collapsed(project, true).unwrap();
        assert!(snapshot.projects[0].collapsed);
        gui.set_collapsed(feature, false).unwrap();

        // What a TUI process opening the same database loads.
        let tui = crate::db::AmfDb::open(tmp_db.path())
            .unwrap()
            .load_store()
            .unwrap();
        assert!(tui.projects[0].collapsed);
        assert!(!tui.projects[0].features[0].collapsed);

        // And a TUI's own toggle reaches the GUI on its next refresh.
        let mut changed = tui.clone();
        changed.projects[0].collapsed = false;
        changed.projects[0].features[0].collapsed = true;
        crate::db::AmfDb::open(tmp_db.path())
            .unwrap()
            .save_store(&changed)
            .unwrap();
        let refreshed = gui.refresh_snapshot().unwrap();
        assert!(!refreshed.projects[0].collapsed);
        assert!(refreshed.projects[0].features[0].collapsed);
    }

    #[test]
    fn collapsing_a_deleted_row_is_not_found() {
        let (mut gui, _tmp_db, _dir) = db_fixture();

        let error = gui
            .set_collapsed(
                CollapseTarget {
                    project_id: "proj-1".into(),
                    feature_id: Some("gone".into()),
                },
                true,
            )
            .unwrap_err();

        assert_eq!(error.kind, super::super::GuiErrorKind::NotFound);
    }

    #[test]
    fn a_store_reload_keeps_the_collectors_last_status_and_usage() {
        let dir = tempfile::tempdir().unwrap();
        let mut agent = session("s-claude", SessionKind::Claude, "Claude");
        agent.status_text = Some("usage 1k".into());
        agent.token_usage = Some(usage(1_000));
        let previous = store_with(vec![feature("feat-1", dir.path(), vec![agent])], dir.path());
        let mut next = store_with(
            vec![feature(
                "feat-1",
                dir.path(),
                vec![
                    session("s-claude", SessionKind::Claude, "Claude"),
                    session("s-new", SessionKind::Codex, "Codex"),
                ],
            )],
            dir.path(),
        );

        carry_runtime_session_fields(&previous, &mut next);

        let sessions = &next.projects[0].features[0].sessions;
        assert_eq!(sessions[0].status_text.as_deref(), Some("usage 1k"));
        assert!(sessions[0].token_usage.is_some());
        assert!(sessions[1].status_text.is_none());
    }
}
