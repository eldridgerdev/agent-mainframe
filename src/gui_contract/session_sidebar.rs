//! Session sidebar parity with the TUI's agent sidebar
//! (`ui::pane::draw_agent_sidebar`).
//!
//! The content is assembled by the same function the TUI renders from,
//! [`App::assemble_agent_sidebar`](crate::app::App), so the sections, their
//! order, their wording and which ones are hidden when empty cannot drift.
//! This module only turns that text into typed lines a desktop panel can
//! style, attaches the GUI's equivalents of the TUI's leader actions, and
//! supplies the runtime signals this process can observe.
//!
//! Field by field, where the GUI process gets each input (it never needs a
//! TUI to be running):
//!
//! | Section / field | Source in the GUI process |
//! | --- | --- |
//! | Status tokens, cost | The shared session-status collector (`sync_session_status_background`) |
//! | Status model | The background sidebar load (Claude transcript, OpenCode storage, Codex config) and Codex rollout metadata |
//! | Status activity | Waiting requests read from the hooks' notification files; thinking from the shared marker files / OpenCode storage |
//! | Usage 5h/7d | The shared `UsageManager` (Claude OAuth usage endpoint, Codex rollout files), refreshed while a session sidebar is open |
//! | Context | The shared context collectors, via the session-status collector |
//! | Plan | The background sidebar load (`plan_sidebar_display_text`) |
//! | Issue | The persisted feature |
//! | PR Triage | The shared open-PR sweep and persisted merged/closed state; review-session activity only where shared sources report it |
//! | Work | Notification files (input, diff review), OpenCode storage, thinking markers |
//! | Summary | The persisted feature summary and OpenCode storage |
//! | Prompt | The background sidebar load and Codex rollout metadata |
//! | Todos | Claude task files and OpenCode storage |
//! | Active TODO | The shared TODO database |
//!
//! Never shown, because only the TUI process that owns the IPC socket knows
//! them (see [`TUI_ONLY_NOTE`]): the attention reason, an in-flight tool
//! call, Codex app-server live events (work item and reasoning), the IPC
//! thinking state of a Claude/Codex PR-review session, and a TUI's own
//! summary generation or AI review. The GUI's own summary generation and AI
//! review are this process's state and are reported.

use serde::{Deserialize, Serialize};
use std::io::Read;

use super::sidebar::waiting_requests;
use super::{GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::app::agent_sidebar::{
    AgentSidebarAssembly, DIFF_REVIEW_TUI_HINT, SidebarPendingInput, SidebarRuntime,
    SidebarSectionKind, SidebarTone, parse_usage_bar_line, sidebar_section_bodies, sidebar_title,
    sidebar_value_tone,
};
use crate::context_tracking::{ContextFreshness, ContextProvenance};
use crate::project::{ProjectStatus, SessionKind};

use super::sidebar::SidebarContextBand;

/// What the panel says about the signals it leaves out.
pub const TUI_ONLY_NOTE: &str = "Attention reasons, running tool calls and Codex live events are \
reported only to a running TUI, so this panel does not show them.";

/// The largest plan file the GUI opens; a bigger one is cut short and says so.
const PLAN_VIEW_MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionSidebarView {
    pub session_id: String,
    pub harness: SessionKind,
    /// `Claude Sidebar`, `Codex Sidebar`, ... as the TUI titles it.
    pub title: String,
    /// Only sections with content, in the TUI's order.
    pub sections: Vec<SessionSidebarSection>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionSidebarSection {
    pub kind: SidebarSectionKind,
    pub title: String,
    pub lines: Vec<SidebarLine>,
    /// The Context section's reading, for a meter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<SidebarContextMeter>,
    pub actions: Vec<SidebarAction>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SidebarLine {
    /// `Label: value`.
    Field {
        label: String,
        value: String,
        tone: SidebarTone,
        emphasised: bool,
    },
    Text {
        text: String,
        tone: SidebarTone,
        emphasised: bool,
    },
    /// A rate-limit window: `5h ┃┃┃░░ 38% · 3h`.
    Bar {
        label: String,
        used_percent: f64,
        level: UsageLevel,
        /// Time to reset, e.g. `3h`; empty when unknown.
        reset: String,
    },
    /// The agent todo list's progress bar.
    Progress {
        done: u32,
        total: u32,
    },
    Item {
        state: TodoItemState,
        text: String,
    },
    /// `+N more`.
    More {
        text: String,
    },
}

/// The dashboard usage bar's colour bands.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UsageLevel {
    Low,
    Medium,
    High,
}

impl UsageLevel {
    /// The thresholds `ui::status::utilization_color` uses.
    fn from_percent(percent: f64) -> Self {
        if percent >= 80.0 {
            Self::High
        } else if percent >= 50.0 {
            Self::Medium
        } else {
            Self::Low
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TodoItemState {
    Done,
    Active,
    Pending,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SidebarContextMeter {
    pub percent: u8,
    pub used_tokens: u64,
    pub limit_tokens: u64,
    pub band: SidebarContextBand,
    pub estimated: bool,
    pub stale: bool,
    /// The TUI would offer its fresh-context prompt (`leader F`) here. The
    /// GUI has no equivalent action yet, so it only says so.
    pub fresh_context_hint: bool,
}

/// The GUI's counterparts of the TUI sidebar's leader actions.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SidebarAction {
    /// TUI `leader n`: read the current plan.
    OpenPlan,
    /// TUI `leader l`: put the last prompt in the composer draft.
    ReusePrompt { prompt: String },
    /// TUI `leader G`.
    PrTriage,
    /// TUI `leader z`: complete the TODO this session was launched for.
    CompleteTodo { todo_id: String },
    /// TUI `leader V`: answer the pending diff review.
    SupervisedEdits,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SessionPlanView {
    /// Relative to the feature's worktree when inside it.
    pub path: String,
    pub markdown: String,
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CompleteSidebarTodo {
    pub todo_id: String,
}

impl GuiHandle {
    /// The agent sidebar for one agent session, from this process's state.
    /// Refreshes the store first so a session deleted elsewhere is
    /// `NotFound`, never a panel for a ghost.
    pub fn session_sidebar(&mut self, target: &SessionTarget) -> GuiResult<SessionSidebarView> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let app = &self.app;
        let project = &app.store.projects[pi];
        let feature = &project.features[fi];
        let session = &feature.sessions[si];
        if !session.kind.is_agent_harness() {
            return Err(GuiError::conflict(format!(
                "'{}' is not an agent session, so it has no agent sidebar",
                session.label
            )));
        }

        let running = feature.status != ProjectStatus::Stopped
            && app.tmux.session_exists(&feature.tmux_session);
        let pending_inputs = waiting_requests(std::slice::from_ref(project))
            .remove(&feature.id)
            .unwrap_or_default()
            .into_iter()
            .map(|request| SidebarPendingInput {
                notification_type: request.kind,
                message: request.message,
            })
            .collect();
        let runtime = SidebarRuntime {
            pending_inputs,
            // Attention, tool calls and Codex live events reach only the IPC
            // socket's owner: see the module docs.
            attention: None,
            running_tool: false,
            codex_live: None,
            thinking: running
                && crate::app::session_ops::agent_for_session_kind(&session.kind).is_some_and(
                    |agent| app.thinking_from_shared_sources(&feature.tmux_session, &agent),
                ),
            summary_generating: app.summary_state.generating.contains(&feature.tmux_session),
            pr_review_working: app
                .dedicated_review_session_working_for_workdir(&feature.workdir)
                .unwrap_or(false),
            ai_review_running: app.ai_review_running_for_workdir(&feature.workdir),
        };
        let assembly = app
            .assemble_agent_sidebar(feature, Some(session), &session.kind, &runtime)
            .ok_or_else(|| GuiError::conflict("This session has no agent sidebar"))?;
        let todo_id = session
            .todo_reference
            .as_ref()
            .filter(|reference| reference.launched_from_todo_menu)
            .map(|reference| reference.todo_id.clone())
            .filter(|_| app.db.is_some());
        let actions = ActionContext {
            has_plan: crate::app::agent_sidebar::plan_sidebar_has_plan(app, feature),
            is_git: project.is_git,
            todo_id,
        };
        Ok(project_view(&session.id, &assembly, &actions))
    }

    /// Bring this process's sidebar caches up to date for `target`'s
    /// feature: the background sidebar load (prompt, model, plan, OpenCode
    /// storage), Codex rollout metadata and the active-TODO cache. Never
    /// blocks on the loads; results land on a later poll.
    pub fn refresh_session_sidebar_sources(&mut self, target: &SessionTarget) -> GuiResult<()> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let app = &mut self.app;
        app.poll_sidebar_load_results();
        app.poll_codex_sidebar_metadata();
        app.schedule_sidebar_load_for_feature(pi, fi);
        let feature = &app.store.projects[pi].features[fi];
        let codex = feature.sessions[si]
            .token_usage_source
            .as_ref()
            .filter(|source| source.provider == crate::token_tracking::TokenUsageProvider::Codex)
            .map(|source| (feature.workdir.clone(), source.id.clone()));
        if let Some((workdir, id)) = codex {
            app.request_codex_sidebar_metadata_for_session(&workdir, &id);
        }
        app.refresh_active_todos_sidebar_cache();
        Ok(())
    }

    /// Refresh the account usage windows (the dashboard status bar's
    /// source). Rate-limited inside `UsageManager`; the Claude half is an
    /// HTTPS call when Claude Code OAuth credentials exist, so the desktop
    /// shell calls this only while a session sidebar is open.
    pub fn refresh_usage_windows(&mut self) {
        self.app.usage.refresh(None);
    }

    /// The feature's current plan (TUI `leader n`), read-only.
    pub fn session_sidebar_plan(&mut self, target: &SessionTarget) -> GuiResult<SessionPlanView> {
        self.refresh_store()?;
        let (pi, fi, _) = self.locate_session(target)?;
        let feature = &self.app.store.projects[pi].features[fi];
        let plan = crate::app::plan::resolve_effective_plan(feature).ok_or_else(|| {
            GuiError::not_found(
                "This feature has no current plan. Choose one from the TUI (leader n) \
                 or add AMF_PLAN.md to its worktree",
            )
        })?;
        let mut bytes = Vec::new();
        std::fs::File::open(plan.path())
            .and_then(|file| {
                file.take((PLAN_VIEW_MAX_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|error| GuiError::not_found(format!("Couldn't read the plan: {error}")))?;
        let truncated = bytes.len() > PLAN_VIEW_MAX_BYTES;
        let mut kept = &bytes[..bytes.len().min(PLAN_VIEW_MAX_BYTES)];
        if truncated {
            kept = &kept[..split_char_start(kept).unwrap_or(kept.len())];
        }
        let markdown = String::from_utf8_lossy(kept).into_owned();
        let path = plan
            .path()
            .strip_prefix(&feature.workdir)
            .unwrap_or(plan.path())
            .display()
            .to_string();
        Ok(SessionPlanView {
            path,
            markdown,
            truncated,
        })
    }

    /// Complete the TODO `target` was launched for (TUI `leader z`). Refuses
    /// a TODO the session no longer references, so a stale panel can never
    /// complete a different item.
    pub fn session_sidebar_complete_todo(
        &mut self,
        target: &SessionTarget,
        request: CompleteSidebarTodo,
    ) -> GuiResult<String> {
        self.refresh_store()?;
        let (pi, fi, si) = self.locate_session(target)?;
        let session = &self.app.store.projects[pi].features[fi].sessions[si];
        let current = session
            .todo_reference
            .as_ref()
            .filter(|reference| reference.launched_from_todo_menu)
            .map(|reference| reference.todo_id.as_str());
        if current != Some(request.todo_id.as_str()) {
            return Err(GuiError::conflict(
                "This session is no longer linked to that TODO; refresh and retry",
            ));
        }
        let message = self
            .app
            .complete_referenced_todo(&request.todo_id)
            .map_err(|error| {
                if error
                    .downcast_ref::<crate::app::ReferencedTodoDeleted>()
                    .is_some()
                {
                    GuiError::not_found(format!("Couldn't complete the TODO: {error}"))
                } else {
                    GuiError::from(error)
                }
            })?;
        self.app.refresh_active_todos_sidebar_cache();
        Ok(message.to_string())
    }
}

/// Where `bytes` ends partway through a UTF-8 character -- what a byte-count
/// cut leaves behind -- the index that character starts at. `None` when the
/// last character is complete, or is invalid in its own right: the file's
/// own bad bytes still decode to U+FFFD rather than being dropped.
fn split_char_start(bytes: &[u8]) -> Option<usize> {
    // A character is at most 4 bytes, so its lead byte is in the last 4.
    let window = bytes.len().saturating_sub(4);
    let lead = window + bytes[window..].iter().rposition(|b| b & 0xC0 != 0x80)?;
    match std::str::from_utf8(&bytes[lead..]) {
        Err(error) if error.error_len().is_none() => Some(lead),
        _ => None,
    }
}

struct ActionContext {
    has_plan: bool,
    is_git: bool,
    todo_id: Option<String>,
}

fn project_view(
    session_id: &str,
    assembly: &AgentSidebarAssembly,
    context: &ActionContext,
) -> SessionSidebarView {
    let data = &assembly.data;
    let sections = sidebar_section_bodies(data)
        .into_iter()
        .map(|(kind, body)| {
            let title = kind.title();
            let mut lines = Vec::new();
            let mut actions = Vec::new();
            let mut meter = None;
            match kind {
                SidebarSectionKind::Usage => {
                    lines.extend(body.lines().map(|line| usage_line(title, line)));
                }
                SidebarSectionKind::Todos => {
                    lines.extend(body.lines().map(|line| todo_line(title, line)));
                }
                SidebarSectionKind::Context => {
                    lines.extend(body.lines().map(|line| field_line(title, line)));
                    meter = data.context_snapshot.as_ref().map(|snapshot| {
                        let indicator = crate::context_display::format_context_indicator(snapshot);
                        SidebarContextMeter {
                            percent: snapshot.percentage.get(),
                            used_tokens: snapshot.used_tokens,
                            limit_tokens: snapshot.context_limit.get(),
                            band: indicator.band.into(),
                            estimated: snapshot.provenance == ContextProvenance::Estimated,
                            stale: snapshot.freshness == ContextFreshness::Stale,
                            fresh_context_hint: data.context_hint_visible,
                        }
                    });
                }
                SidebarSectionKind::Work => {
                    lines.extend(
                        body.lines()
                            .filter(|line| {
                                !(assembly.pending_diff_review && *line == DIFF_REVIEW_TUI_HINT)
                            })
                            .map(|line| field_line(title, line)),
                    );
                    if assembly.pending_diff_review {
                        actions.push(SidebarAction::SupervisedEdits);
                    }
                }
                SidebarSectionKind::ActiveTodo => {
                    let mut completed = false;
                    for (index, line) in body.lines().enumerate() {
                        if index == 0 {
                            // A TODO title is prose, even when it contains a
                            // colon. Keep it eligible for the title clamp.
                            lines.push(SidebarLine::Text {
                                text: line.to_string(),
                                tone: SidebarTone::Plain,
                                emphasised: false,
                            });
                        } else if line == "State: completed" {
                            completed = true;
                            lines.push(SidebarLine::Field {
                                label: "State".into(),
                                value: "completed".into(),
                                tone: SidebarTone::Todo,
                                emphasised: false,
                            });
                        } else {
                            lines.push(field_line(title, line));
                        }
                    }
                    if let Some(todo_id) = context.todo_id.as_ref()
                        && data.active_todo_affordance
                        && !completed
                    {
                        actions.push(SidebarAction::CompleteTodo {
                            todo_id: todo_id.clone(),
                        });
                    }
                }
                _ => lines.extend(body.lines().map(|line| field_line(title, line))),
            }
            match kind {
                SidebarSectionKind::Plan if context.has_plan => {
                    actions.push(SidebarAction::OpenPlan);
                }
                SidebarSectionKind::Prompt => {
                    if let Some(prompt) = assembly
                        .prompt_full
                        .as_deref()
                        .map(str::trim)
                        .filter(|prompt| !prompt.is_empty())
                    {
                        actions.push(SidebarAction::ReusePrompt {
                            prompt: prompt.to_string(),
                        });
                    }
                }
                SidebarSectionKind::PrTriage if context.is_git => {
                    actions.push(SidebarAction::PrTriage);
                }
                _ => {}
            }
            SessionSidebarSection {
                kind,
                title: title.to_string(),
                lines,
                context: meter,
                actions,
            }
        })
        .collect();
    SessionSidebarView {
        session_id: session_id.to_string(),
        harness: data.agent_kind.clone(),
        title: sidebar_title(&data.agent_kind).to_string(),
        sections,
        notes: vec![TUI_ONLY_NOTE.to_string()],
    }
}

/// `Label: value` or a bare line, toned by the shared rule.
fn field_line(title: &str, line: &str) -> SidebarLine {
    match line.split_once(": ") {
        Some((label, value)) => {
            let (tone, emphasised) = sidebar_value_tone(title, label, value);
            SidebarLine::Field {
                label: label.to_string(),
                value: value.to_string(),
                tone,
                emphasised,
            }
        }
        None => {
            let (tone, emphasised) = sidebar_value_tone(title, "", line);
            SidebarLine::Text {
                text: line.to_string(),
                tone,
                emphasised,
            }
        }
    }
}

fn usage_line(title: &str, line: &str) -> SidebarLine {
    match parse_usage_bar_line(line) {
        Some(bar) => SidebarLine::Bar {
            label: bar.label.to_string(),
            used_percent: bar.percent,
            level: UsageLevel::from_percent(bar.percent),
            reset: bar.reset.trim().trim_start_matches('·').trim().to_string(),
        },
        None => field_line(title, line),
    }
}

fn todo_line(title: &str, line: &str) -> SidebarLine {
    if line.starts_with('█') || line.starts_with('░') {
        let counts = line
            .rsplit_once(' ')
            .and_then(|(_, counts)| counts.split_once('/'))
            .and_then(|(done, total)| Some((done.parse().ok()?, total.parse().ok()?)));
        if let Some((done, total)) = counts {
            return SidebarLine::Progress { done, total };
        }
    }
    for (prefix, state) in [
        ("✓ ", TodoItemState::Done),
        ("● ", TodoItemState::Active),
        ("○ ", TodoItemState::Pending),
    ] {
        if let Some(text) = line.strip_prefix(prefix) {
            return SidebarLine::Item {
                state,
                text: text.to_string(),
            };
        }
    }
    if line.starts_with('+') {
        return SidebarLine::More {
            text: line.to_string(),
        };
    }
    field_line(title, line)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::app::opencode_storage::OpencodeSidebarData;
    use crate::context_tracking::{
        ContextBand, ContextPercentage, ContextResetMetadata, SessionContextSnapshot,
        SessionContextState,
    };
    use crate::db::todos::{TodoPriority, TodoScope};
    use crate::project::{
        AgentKind, Feature, FeatureSession, IssueCommentStatus, IssueSource, Project, ProjectStore,
        TodoSessionReference, VibeMode,
    };
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use chrono::Utc;
    use std::path::Path;

    fn session(id: &str, kind: SessionKind) -> FeatureSession {
        FeatureSession {
            id: id.to_string(),
            kind,
            label: id.to_string(),
            tmux_window: id.to_string(),
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

    fn feature(workdir: &Path, agent: AgentKind, sessions: Vec<FeatureSession>) -> Feature {
        let mut feature = Feature::new_for_project(
            "demo",
            "feat".into(),
            "feat".into(),
            workdir.to_path_buf(),
            true,
            VibeMode::default(),
            false,
            false,
            agent,
            false,
            false,
        );
        feature.id = "feat-1".into();
        feature.tmux_session = format!("amf-session-sidebar-{}", std::process::id());
        feature.status = ProjectStatus::Idle;
        feature.sessions = sessions;
        feature
    }

    fn gui_with(feature: Feature, repo: &Path, live: bool) -> GuiHandle {
        let mut project = Project::new("demo".into(), repo.to_path_buf(), true, AgentKind::Claude);
        project.id = "proj-1".into();
        project.features = vec![feature];
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().returning(move |_| live);
        GuiHandle::from_app(App::new_for_test(
            store,
            Box::new(tmux),
            Box::new(MockWorktreeOps::new()),
        ))
    }

    fn target(session_id: &str) -> SessionTarget {
        SessionTarget {
            project_id: "proj-1".into(),
            feature_id: "feat-1".into(),
            session_id: session_id.into(),
        }
    }

    fn context(percentage: u8, band: ContextBand) -> SessionContextState {
        let now = Utc::now();
        SessionContextState {
            snapshot: Some(SessionContextSnapshot {
                used_tokens: u64::from(percentage) * 2_000,
                context_limit: std::num::NonZeroU64::new(200_000).unwrap(),
                percentage: ContextPercentage::clamped(i64::from(percentage)),
                band,
                provenance: ContextProvenance::Estimated,
                freshness: ContextFreshness::Stale,
                sampled_at: now,
                checked_at: now,
                reset: ContextResetMetadata::default(),
            }),
            reset: ContextResetMetadata::default(),
            awaiting_post_reset: false,
        }
    }

    fn kinds(view: &SessionSidebarView) -> Vec<SidebarSectionKind> {
        view.sections.iter().map(|section| section.kind).collect()
    }

    fn section(view: &SessionSidebarView, kind: SidebarSectionKind) -> &SessionSidebarSection {
        view.sections
            .iter()
            .find(|section| section.kind == kind)
            .unwrap_or_else(|| panic!("{kind:?} missing from {:?}", kinds(view)))
    }

    fn field(label: &str, value: &str) -> (String, String) {
        (label.to_string(), value.to_string())
    }

    fn fields(section: &SessionSidebarSection) -> Vec<(String, String)> {
        section
            .lines
            .iter()
            .filter_map(|line| match line {
                SidebarLine::Field { label, value, .. } => Some(field(label, value)),
                _ => None,
            })
            .collect()
    }

    /// The payload the frontend panel is written against: every TUI section,
    /// in the TUI's order, with typed lines and the GUI's leader actions.
    #[test]
    fn a_claude_sidebar_carries_every_section_in_the_tui_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AMF_PLAN.md"), "# Plan\n").unwrap();
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::AmfDb::open(tmp_db.path()).unwrap();
        let list = db
            .create_todo_list(
                &TodoScope::Project {
                    project_id: "proj-1".into(),
                },
                None,
            )
            .unwrap();
        let todo = db
            .add_todo(&list.id, "Ship the panel", None, TodoPriority::Med)
            .unwrap();
        let mut agent = session("claude", SessionKind::Claude);
        agent.status_text = Some("12.3k in · 4.5k out · 16.8k eff · $0.42".into());
        agent.todo_reference = Some(TodoSessionReference {
            todo_id: todo.id.clone(),
            launched_from_todo_menu: true,
        });
        let mut feat = feature(dir.path(), AgentKind::Claude, vec![agent]);
        feat.summary = Some("Builds the GUI agent sidebar".into());
        feat.issue_source = Some(IssueSource {
            host: "github.com".into(),
            owner: "acme".into(),
            repository: "widget".into(),
            number: 42,
            comment_status: IssueCommentStatus::Posted,
        });
        let tmux_session = feat.tmux_session.clone();
        let mut gui = gui_with(feat, dir.path(), true);
        db.save_store(&gui.app.store).unwrap();
        let app = &mut gui.app;
        app.db = Some(db);
        let mut usage = crate::usage::UsageData::default();
        usage.claude.five_hour_pct = Some(62.0);
        usage.claude.seven_day_pct = Some(18.0);
        app.usage.set_data_for_test(usage);
        app.context_states
            .insert("claude".into(), context(85, ContextBand::Critical));
        app.context_hint_states.sync_all(&app.context_states);
        app.sidebar_effective_plan_cache
            .insert(tmux_session.clone(), "Current: AMF_PLAN.md".into());
        app.sidebar_model_cache
            .insert(tmux_session.clone(), "Model: claude-opus".into());
        app.latest_prompt_cache.insert(
            tmux_session.clone(),
            "Add the session sidebar to the GUI agent tabs and keep the draft".into(),
        );
        app.active_prs.insert(
            "feat-1".into(),
            crate::app::ActivePrStatus {
                branch: "feat".into(),
                head_sha: "abc".into(),
                number: 77,
                unresolved_threads: Some(2),
            },
        );
        app.refresh_active_todos_sidebar_cache();

        let view = gui.session_sidebar(&target("claude")).unwrap();

        assert_eq!(view.title, "Claude Sidebar");
        assert_eq!(view.harness, SessionKind::Claude);
        assert_eq!(
            kinds(&view),
            vec![
                SidebarSectionKind::Status,
                SidebarSectionKind::Usage,
                SidebarSectionKind::Context,
                SidebarSectionKind::Plan,
                SidebarSectionKind::Issue,
                SidebarSectionKind::PrTriage,
                SidebarSectionKind::Summary,
                SidebarSectionKind::Prompt,
                SidebarSectionKind::ActiveTodo,
            ]
        );
        assert_eq!(
            fields(section(&view, SidebarSectionKind::Status)),
            vec![
                field("Activity", "Ready"),
                field("Input", "12.3k tokens"),
                field("Output", "4.5k tokens"),
                field("Effective", "16.8k tokens"),
                field("Cost", "$0.42"),
                field("Model", "claude-opus"),
            ]
        );
        let usage = &section(&view, SidebarSectionKind::Usage).lines;
        assert!(matches!(
            &usage[0],
            SidebarLine::Bar { label, used_percent, level: UsageLevel::Medium, .. }
                if label == "5h" && (*used_percent - 62.0).abs() < 0.5
        ));
        assert!(
            matches!(&usage[1], SidebarLine::Bar { label, level: UsageLevel::Low, .. } if label == "7d")
        );
        let context = section(&view, SidebarSectionKind::Context);
        let meter = context.context.as_ref().unwrap();
        assert_eq!(
            (meter.percent, meter.used_tokens, meter.limit_tokens),
            (85, 170_000, 200_000)
        );
        assert_eq!(meter.band, SidebarContextBand::Critical);
        assert!(meter.estimated && meter.stale && meter.fresh_context_hint);
        assert!(matches!(&context.lines[0], SidebarLine::Text { text, .. }
            if text.starts_with("Ctx ~85% CRITICAL STALE")));
        assert_eq!(
            section(&view, SidebarSectionKind::Plan).actions,
            vec![SidebarAction::OpenPlan]
        );
        assert_eq!(
            fields(section(&view, SidebarSectionKind::Issue))[1],
            field("Issue", "#42")
        );
        let pr = section(&view, SidebarSectionKind::PrTriage);
        assert_eq!(fields(pr), vec![field("PR", "#77 · 2 open")]);
        assert_eq!(pr.actions, vec![SidebarAction::PrTriage]);
        let prompt = section(&view, SidebarSectionKind::Prompt);
        assert!(matches!(&prompt.lines[0], SidebarLine::Text { text, .. } if text.ends_with('…')));
        assert_eq!(
            prompt.actions,
            vec![SidebarAction::ReusePrompt {
                prompt: "Add the session sidebar to the GUI agent tabs and keep the draft".into()
            }]
        );
        let active = section(&view, SidebarSectionKind::ActiveTodo);
        assert!(
            matches!(&active.lines[0], SidebarLine::Text { text, .. } if text == "Ship the panel")
        );
        assert_eq!(fields(active), vec![field("State", "open")]);
        assert_eq!(
            active.actions,
            vec![SidebarAction::CompleteTodo {
                todo_id: todo.id.clone()
            }]
        );
        assert_eq!(view.notes, vec![TUI_ONLY_NOTE.to_string()]);

        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["harness"], "claude");
        assert_eq!(json["sections"][0]["kind"], "status");
        assert_eq!(json["sections"][0]["lines"][0]["kind"], "field");
        assert_eq!(json["sections"][0]["lines"][0]["tone"], "ready");
        assert_eq!(json["sections"][1]["lines"][0]["kind"], "bar");
        assert_eq!(json["sections"][2]["context"]["band"], "critical");
        assert_eq!(json["sections"][3]["actions"][0]["kind"], "open_plan");
    }

    #[test]
    fn a_sparse_sidebar_omits_every_empty_section() {
        let dir = tempfile::tempdir().unwrap();
        let feat = feature(
            dir.path(),
            AgentKind::Pi,
            vec![session("pi", SessionKind::Pi)],
        );
        let mut gui = gui_with(feat, dir.path(), true);

        let view = gui.session_sidebar(&target("pi")).unwrap();

        assert_eq!(view.title, "Pi Sidebar");
        // The plan line always reads something ("No plan selected"), as in the TUI.
        assert_eq!(
            kinds(&view),
            vec![SidebarSectionKind::Status, SidebarSectionKind::Plan]
        );
        assert_eq!(
            fields(section(&view, SidebarSectionKind::Status)),
            vec![field("Activity", "Ready")]
        );
        let plan = section(&view, SidebarSectionKind::Plan);
        assert!(
            matches!(&plan.lines[0], SidebarLine::Text { text, .. } if text == "No plan selected")
        );
        assert!(plan.actions.is_empty(), "no plan file, nothing to open");
    }

    #[test]
    fn opencode_puts_its_summary_last_and_reads_todos_from_its_storage() {
        let dir = tempfile::tempdir().unwrap();
        let mut feat = feature(
            dir.path(),
            AgentKind::Opencode,
            vec![session("oc", SessionKind::Opencode)],
        );
        feat.summary = Some("Summarised".into());
        let tmux_session = feat.tmux_session.clone();
        let mut gui = gui_with(feat, dir.path(), true);
        gui.app.opencode_sidebar_cache.insert(
            tmux_session,
            OpencodeSidebarData {
                session_id: "ses_1".into(),
                status: Some("busy".into()),
                last_tool: Some("edit".into()),
                todo_count: Some(7),
                todo_preview: vec!["Write tests".into(), "Wire the panel".into()],
                ..Default::default()
            },
        );

        let view = gui.session_sidebar(&target("oc")).unwrap();

        assert_eq!(view.title, "Opencode Sidebar");
        assert_eq!(kinds(&view).last(), Some(&SidebarSectionKind::Summary));
        let todos = &section(&view, SidebarSectionKind::Todos).lines;
        assert_eq!(
            todos,
            &vec![
                SidebarLine::Item {
                    state: TodoItemState::Pending,
                    text: "Write tests".into()
                },
                SidebarLine::Item {
                    state: TodoItemState::Pending,
                    text: "Wire the panel".into()
                },
                SidebarLine::More {
                    text: "+5 more".into()
                },
            ]
        );
        assert_eq!(
            fields(section(&view, SidebarSectionKind::Work)),
            vec![field("State", "busy"), field("Tool", "edit")]
        );
    }

    #[test]
    fn a_tab_reads_thinking_from_its_own_harness_not_the_features() {
        // An OpenCode tab in a Claude feature: thinking comes from OpenCode
        // storage, which the Claude marker file would never reflect.
        let dir = tempfile::tempdir().unwrap();
        let feat = feature(
            dir.path(),
            AgentKind::Claude,
            vec![session("oc", SessionKind::Opencode)],
        );
        let tmux_session = feat.tmux_session.clone();
        let mut gui = gui_with(feat, dir.path(), true);
        gui.app.opencode_sidebar_cache.insert(
            tmux_session,
            OpencodeSidebarData {
                session_id: "ses_1".into(),
                status: Some("busy".into()),
                ..Default::default()
            },
        );

        let view = gui.session_sidebar(&target("oc")).unwrap();

        assert!(
            fields(section(&view, SidebarSectionKind::Status))
                .contains(&field("Activity", "Thinking"))
        );
    }

    #[test]
    fn claude_todo_lines_parse_into_progress_and_items() {
        assert_eq!(
            todo_line("Todos", "█████░░░ 3/5"),
            SidebarLine::Progress { done: 3, total: 5 }
        );
        assert_eq!(
            todo_line("Todos", "✓ Read the spec"),
            SidebarLine::Item {
                state: TodoItemState::Done,
                text: "Read the spec".into()
            }
        );
        assert_eq!(
            todo_line("Todos", "● Writing the panel"),
            SidebarLine::Item {
                state: TodoItemState::Active,
                text: "Writing the panel".into()
            }
        );
        assert!(matches!(
            usage_line("Usage", "7d ┃┃┃┃┃┃┃┃┃░ 91% · 2d"),
            SidebarLine::Bar { level: UsageLevel::High, reset, .. } if reset == "2d"
        ));
    }

    #[test]
    fn waiting_requests_on_disk_drive_status_and_work_and_a_diff_review_offers_supervised_edits() {
        let dir = tempfile::tempdir().unwrap();
        let notifications = dir.path().join(".claude/notifications");
        std::fs::create_dir_all(&notifications).unwrap();
        std::fs::write(
            notifications.join("a.json"),
            r#"{"type":"diff-review","message":"Edit src/lib.rs"}"#,
        )
        .unwrap();
        let feat = feature(
            dir.path(),
            AgentKind::Claude,
            vec![session("claude", SessionKind::Claude)],
        );
        let mut gui = gui_with(feat, dir.path(), true);

        let view = gui.session_sidebar(&target("claude")).unwrap();

        let work = section(&view, SidebarSectionKind::Work);
        assert_eq!(
            fields(work),
            vec![
                field("State", "waiting for diff review"),
                field("Request", "Edit src/lib.rs"),
            ],
            "the TUI's leader-key hint is replaced by an action"
        );
        assert_eq!(work.actions, vec![SidebarAction::SupervisedEdits]);
        // Work explains the wait, so the TUI drops the Activity line too.
        assert!(!kinds(&view).contains(&SidebarSectionKind::Status));
        assert!(notifications.join("a.json").exists(), "read, not consumed");
    }

    #[test]
    fn tui_only_signals_never_reach_the_gui_panel() {
        let dir = tempfile::tempdir().unwrap();
        let feat = feature(
            dir.path(),
            AgentKind::Claude,
            vec![session("claude", SessionKind::Claude)],
        );
        let tmux_session = feat.tmux_session.clone();
        let mut gui = gui_with(feat, dir.path(), true);
        // State an IPC-owning TUI keeps; a GUI process never fills these,
        // and must not report them even if they were present.
        gui.app.ipc_tool_sessions.insert(tmux_session.clone());
        gui.app.thinking_features.insert(tmux_session.clone());
        gui.app.pending_inputs.push(crate::app::PendingInput {
            session_id: tmux_session,
            message: "From the TUI's queue".into(),
            notification_type: "input-request".into(),
            ..Default::default()
        });

        let view = gui.session_sidebar(&target("claude")).unwrap();

        assert!(
            !view
                .sections
                .iter()
                .any(|s| s.kind == SidebarSectionKind::Work)
        );
        assert_eq!(
            fields(section(&view, SidebarSectionKind::Status)),
            vec![field("Activity", "Ready")]
        );
    }

    #[test]
    fn completing_the_active_todo_refuses_a_stale_id_and_then_reports_completed() {
        let dir = tempfile::tempdir().unwrap();
        let tmp_db = tempfile::NamedTempFile::new().unwrap();
        let db = crate::db::AmfDb::open(tmp_db.path()).unwrap();
        let list = db
            .create_todo_list(
                &TodoScope::Project {
                    project_id: "proj-1".into(),
                },
                None,
            )
            .unwrap();
        let todo = db
            .add_todo(&list.id, "Finish: sidebar parity", None, TodoPriority::Med)
            .unwrap();
        let mut agent = session("claude", SessionKind::Claude);
        agent.todo_reference = Some(TodoSessionReference {
            todo_id: todo.id.clone(),
            launched_from_todo_menu: true,
        });
        let mut gui = gui_with(
            feature(dir.path(), AgentKind::Claude, vec![agent]),
            dir.path(),
            true,
        );
        db.save_store(&gui.app.store).unwrap();
        gui.app.db = Some(db);

        gui.app.refresh_active_todos_sidebar_cache();
        let view = gui.session_sidebar(&target("claude")).unwrap();
        assert!(matches!(
            &section(&view, SidebarSectionKind::ActiveTodo).lines[0],
            SidebarLine::Text { text, .. } if text == "Finish: sidebar parity"
        ));

        let stale = gui
            .session_sidebar_complete_todo(
                &target("claude"),
                CompleteSidebarTodo {
                    todo_id: "other".into(),
                },
            )
            .unwrap_err();
        assert_eq!(stale.kind, super::super::GuiErrorKind::Conflict);

        let message = gui
            .session_sidebar_complete_todo(
                &target("claude"),
                CompleteSidebarTodo {
                    todo_id: todo.id.clone(),
                },
            )
            .unwrap();
        assert_eq!(message, "Marked referenced TODO complete");
        let stored = gui
            .app
            .db
            .as_ref()
            .unwrap()
            .find_todo_by_id(&todo.id)
            .unwrap()
            .unwrap();
        assert!(stored.work.status.is_completed());

        let view = gui.session_sidebar(&target("claude")).unwrap();
        let active = section(&view, SidebarSectionKind::ActiveTodo);
        assert_eq!(fields(active), vec![field("State", "completed")]);
        assert!(active.actions.is_empty());

        // Deleted underneath the panel: NotFound, not an internal error.
        gui.app.db.as_ref().unwrap().delete_todo(&todo.id).unwrap();
        let deleted = gui
            .session_sidebar_complete_todo(
                &target("claude"),
                CompleteSidebarTodo {
                    todo_id: todo.id.clone(),
                },
            )
            .unwrap_err();
        assert_eq!(deleted.kind, super::super::GuiErrorKind::NotFound);
    }

    #[test]
    fn the_plan_opens_read_only_and_a_missing_plan_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let feat = feature(
            dir.path(),
            AgentKind::Codex,
            vec![session("codex", SessionKind::Codex)],
        );
        let mut gui = gui_with(feat, dir.path(), true);

        let missing = gui.session_sidebar_plan(&target("codex")).unwrap_err();
        assert_eq!(missing.kind, super::super::GuiErrorKind::NotFound);

        std::fs::write(dir.path().join("AMF_PLAN.md"), "# Plan\n\n- [ ] step").unwrap();
        let plan = gui.session_sidebar_plan(&target("codex")).unwrap();
        assert_eq!(plan.path, "AMF_PLAN.md");
        assert_eq!(plan.markdown, "# Plan\n\n- [ ] step");
        assert!(!plan.truncated);

        let oversized = "a".repeat(PLAN_VIEW_MAX_BYTES - 1) + "é and more";
        std::fs::write(dir.path().join("AMF_PLAN.md"), &oversized).unwrap();
        let plan = gui.session_sidebar_plan(&target("codex")).unwrap();
        assert!(plan.truncated);
        assert_eq!(plan.markdown, "a".repeat(PLAN_VIEW_MAX_BYTES - 1));
        assert_eq!(
            std::fs::read_to_string(dir.path().join("AMF_PLAN.md")).unwrap(),
            oversized
        );
    }

    #[test]
    fn a_truncated_plan_drops_only_the_character_the_cut_split() {
        // "é" is 2 bytes and "🙂" 4: each cut leaves a partial lead.
        assert_eq!(split_char_start(b"ab\xc3"), Some(2));
        assert_eq!(split_char_start(b"ab\xf0\x9f\x99"), Some(2));
        assert_eq!(split_char_start("abé".as_bytes()), None);
        assert_eq!(split_char_start(b""), None);
        // Replacement characters the file really ends in are content.
        assert_eq!(split_char_start("ab\u{fffd}\u{fffd}".as_bytes()), None);
        // So are invalid bytes of its own, at the end or before a split.
        assert_eq!(split_char_start(b"ab\xff"), None);
        assert_eq!(split_char_start(b"a\xff\xc3"), Some(2));
    }

    #[test]
    fn a_codex_sidebar_uses_its_session_prompt_and_usage_windows() {
        let dir = tempfile::tempdir().unwrap();
        let mut codex = session("codex", SessionKind::Codex);
        codex.token_usage_source = Some(crate::token_tracking::TokenUsageSource {
            provider: crate::token_tracking::TokenUsageProvider::Codex,
            id: "rollout-1".into(),
        });
        let mut gui = gui_with(
            feature(dir.path(), AgentKind::Codex, vec![codex]),
            dir.path(),
            true,
        );
        let mut usage = crate::usage::UsageData::default();
        usage.codex.five_hour_usage_pct = Some(30.0);
        gui.app.usage.set_data_for_test(usage);
        let key = format!("{}::rollout-1", dir.path().display());
        gui.app
            .codex_session_prompt_cache
            .insert(key.clone(), Some("Fix the flaky test".into()));
        gui.app
            .codex_session_model_cache
            .insert(key, Some("Model: gpt-5-codex".into()));

        let view = gui.session_sidebar(&target("codex")).unwrap();

        assert_eq!(view.title, "Codex Sidebar");
        assert!(
            fields(section(&view, SidebarSectionKind::Status))
                .contains(&field("Model", "gpt-5-codex"))
        );
        assert!(matches!(
            &section(&view, SidebarSectionKind::Usage).lines[..],
            [SidebarLine::Bar { label, .. }] if label == "5h"
        ));
        assert_eq!(
            section(&view, SidebarSectionKind::Prompt).actions,
            vec![SidebarAction::ReusePrompt {
                prompt: "Fix the flaky test".into()
            }]
        );
    }

    #[test]
    fn stale_and_non_agent_targets_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let feat = feature(
            dir.path(),
            AgentKind::Claude,
            vec![session("shell", SessionKind::Terminal)],
        );
        let mut gui = gui_with(feat, dir.path(), true);

        let gone = gui.session_sidebar(&target("deleted")).unwrap_err();
        assert_eq!(gone.kind, super::super::GuiErrorKind::NotFound);
        let shell = gui.session_sidebar(&target("shell")).unwrap_err();
        assert_eq!(shell.kind, super::super::GuiErrorKind::Conflict);
    }

    #[test]
    fn refreshing_sources_loads_the_plan_line_off_the_calling_thread() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("AMF_PLAN.md"), "# Plan\n").unwrap();
        let feat = feature(
            dir.path(),
            AgentKind::Claude,
            vec![session("claude", SessionKind::Claude)],
        );
        let mut gui = gui_with(feat, dir.path(), true);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let plan = loop {
            gui.refresh_session_sidebar_sources(&target("claude"))
                .unwrap();
            let view = gui.session_sidebar(&target("claude")).unwrap();
            let plan = section(&view, SidebarSectionKind::Plan).clone();
            if matches!(&plan.lines[0], SidebarLine::Field { value, .. } if value == "AMF_PLAN.md")
                || std::time::Instant::now() > deadline
            {
                break plan;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };

        assert_eq!(fields(&plan), vec![field("Current", "AMF_PLAN.md")]);
        assert_eq!(plan.actions, vec![SidebarAction::OpenPlan]);
    }
}
