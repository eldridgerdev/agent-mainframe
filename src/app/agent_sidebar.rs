//! Presentation-free assembly of the embedded agent-session sidebar.
//!
//! The TUI draws this panel to the right of an embedded agent pane
//! (`ui::pane::draw_agent_sidebar`); the desktop GUI shows the same panel
//! beside an agent tab (`gui_contract::session_sidebar`). Both build their
//! content here, from [`App::assemble_agent_sidebar`], so the sections, their
//! order, their wording and the sources each one reads stay one
//! implementation.
//!
//! Most inputs come from caches that any AMF process can fill from shared
//! sources (the session-status collector, the background sidebar loads,
//! Codex rollout metadata, the TODO database, usage). The rest are signals
//! only the process that observed them knows. They arrive through
//! [`SidebarRuntime`], which each interface fills from what it can honestly
//! observe: the TUI from its IPC socket and in-memory state
//! ([`App::tui_sidebar_runtime`]), the GUI from the hooks' files on disk.

use super::attention::AttentionState;
use super::util::{ClaudeTaskState, read_claude_task_state};
use super::{App, CodexLiveThreadState};
use crate::context_tracking::SessionContextSnapshot;
use crate::project::{Feature, FeatureSession, Project, SessionKind, TokenUsageSourceMatch};
use crate::token_tracking::{TokenUsageProvider, TokenUsageSource};

const SIDEBAR_PROMPT_PREVIEW_COLS: usize = 32;
const SIDEBAR_PROMPT_PREVIEW_LINES: usize = 2;
const SIDEBAR_SUMMARY_PREVIEW_COLS: usize = 32;
const SIDEBAR_SUMMARY_PREVIEW_LINES: usize = 3;
const SIDEBAR_WORK_VALUE_CHARS: usize = 28;

/// The leader-key hint the TUI appends to a pending diff review's Work text.
/// The GUI offers its own action instead and drops this line.
pub(crate) const DIFF_REVIEW_TUI_HINT: &str =
    "Hint: use leader V if the review prompt is not appearing.";

#[derive(Debug, Clone)]
pub(crate) struct AgentSidebarData {
    pub agent_kind: SessionKind,
    pub status_text: String,
    /// Account-level rate-limit windows for this harness (the same `5h`/`7d`
    /// figures the dashboard status bar shows), one small bar per line. `None` when the
    /// harness has no usage source or the cache is not warm yet — the box is
    /// then omitted entirely.
    pub usage_text: Option<String>,
    #[allow(dead_code)] // populated but not rendered yet
    pub model_text: Option<String>,
    pub prompt_text: String,
    pub work_text: Option<String>,
    pub todos_text: Option<String>,
    /// The current session's TODO-menu-originated reference, resolved from
    /// AMF's TODO DB.
    pub active_todos_text: Option<String>,
    /// Whether the *currently viewed* session itself carries a menu-launched
    /// TODO reference. `leader z` acts only on the current
    /// session, so the header affordance is shown only when this is true.
    pub active_todo_affordance: bool,
    pub summary_text: String,
    pub issue_source_text: Option<String>,
    pub pr_triage_text: Option<String>,
    pub plan_text: String,
    pub context_snapshot: Option<SessionContextSnapshot>,
    pub context_hint_visible: bool,
}

/// [`AgentSidebarData`] plus what an interface needs to act on it, which the
/// TUI's renderer does not use.
#[derive(Debug, Clone)]
pub(crate) struct AgentSidebarAssembly {
    pub data: AgentSidebarData,
    /// The full prompt the Prompt section previews (clamped in `data`).
    pub prompt_full: Option<String>,
    /// The Work section describes a pending diff review or change reason.
    pub pending_diff_review: bool,
}

/// One sidebar section, in the order the panel shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarSectionKind {
    Status,
    Usage,
    Context,
    Plan,
    Issue,
    PrTriage,
    Work,
    Summary,
    Prompt,
    Todos,
    ActiveTodo,
}

impl SidebarSectionKind {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Status => "Status",
            Self::Usage => "Usage",
            Self::Context => "Context",
            Self::Plan => "Plan",
            Self::Issue => "Issue",
            Self::PrTriage => "PR Triage",
            Self::Work => "Work",
            Self::Summary => "Summary",
            Self::Prompt => "Prompt",
            Self::Todos => "Todos",
            Self::ActiveTodo => "Active TODO",
        }
    }
}

/// The sections that have content, in display order, with each one's body
/// text. A section with nothing to say is left out, never shown empty. The
/// Context body is the shared indicator text
/// (`context_display::format_context_indicator`). OpenCode puts its summary
/// last.
pub(crate) fn sidebar_section_bodies(data: &AgentSidebarData) -> Vec<(SidebarSectionKind, String)> {
    let mut sections = Vec::new();
    let non_empty = |text: &str| !text.trim().is_empty();

    if non_empty(&data.status_text) {
        sections.push((SidebarSectionKind::Status, data.status_text.clone()));
    }
    if let Some(usage) = data.usage_text.as_deref().filter(|text| non_empty(text)) {
        sections.push((SidebarSectionKind::Usage, usage.to_string()));
    }
    if let Some(snapshot) = data.context_snapshot.as_ref() {
        sections.push((
            SidebarSectionKind::Context,
            crate::context_display::format_context_indicator(snapshot).text,
        ));
    }
    if non_empty(&data.plan_text) {
        sections.push((SidebarSectionKind::Plan, data.plan_text.clone()));
    }
    if let Some(issue) = data
        .issue_source_text
        .as_deref()
        .filter(|text| non_empty(text))
    {
        sections.push((SidebarSectionKind::Issue, issue.to_string()));
    }
    if let Some(pr) = data
        .pr_triage_text
        .as_deref()
        .filter(|text| non_empty(text))
    {
        sections.push((SidebarSectionKind::PrTriage, pr.to_string()));
    }

    let is_opencode = matches!(data.agent_kind, SessionKind::Opencode);
    if let Some(work) = data.work_text.as_deref() {
        sections.push((SidebarSectionKind::Work, work.to_string()));
    }
    if !is_opencode && non_empty(&data.summary_text) {
        sections.push((SidebarSectionKind::Summary, data.summary_text.clone()));
    }
    if non_empty(&data.prompt_text) {
        sections.push((SidebarSectionKind::Prompt, data.prompt_text.clone()));
    }
    if let Some(todos) = data.todos_text.as_deref() {
        sections.push((SidebarSectionKind::Todos, todos.to_string()));
    }
    if let Some(active) = data.active_todos_text.as_deref() {
        sections.push((SidebarSectionKind::ActiveTodo, active.to_string()));
    }
    if is_opencode && non_empty(&data.summary_text) {
        sections.push((SidebarSectionKind::Summary, data.summary_text.clone()));
    }
    sections
}

/// The panel's title, per harness.
pub(crate) fn sidebar_title(agent_kind: &SessionKind) -> &'static str {
    match agent_kind {
        SessionKind::Claude => "Claude Sidebar",
        SessionKind::Codex => "Codex Sidebar",
        SessionKind::Opencode => "Opencode Sidebar",
        SessionKind::Pi => "Pi Sidebar",
        _ => "Harness Sidebar",
    }
}

/// How one sidebar value reads, independent of any palette: the TUI maps it
/// to theme colours, the GUI to CSS tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SidebarTone {
    Plain,
    Muted,
    StateActive,
    StateIdle,
    StateStopped,
    Waiting,
    Busy,
    PrWorking,
    Ready,
    Generating,
    Hint,
    Todo,
    Detail,
}

/// The tone of a `label: value` (or bare value, `label` empty) line in the
/// section titled `title`, and whether it is emphasised. One rule for both
/// interfaces.
pub(crate) fn sidebar_value_tone(title: &str, label: &str, value: &str) -> (SidebarTone, bool) {
    let lower = value.to_lowercase();
    let pr_working =
        title == "PR Triage" && (lower.contains("working") || lower.contains("running"));
    let tone = if label == "State" {
        match lower.as_str() {
            "active" => SidebarTone::StateActive,
            "idle" => SidebarTone::StateIdle,
            "stopped" => SidebarTone::StateStopped,
            _ => SidebarTone::Plain,
        }
    } else if lower.contains("waiting") {
        SidebarTone::Waiting
    } else if lower.contains("thinking") || lower.contains("running tool") {
        SidebarTone::Busy
    } else if pr_working {
        SidebarTone::PrWorking
    } else if lower.contains("ready") {
        SidebarTone::Ready
    } else if lower.contains("generating") {
        SidebarTone::Generating
    } else if lower.contains("unavailable") || lower.contains("no summary yet") {
        SidebarTone::Muted
    } else if label == "Hint" {
        SidebarTone::Hint
    } else if title == "Todos" {
        SidebarTone::Todo
    } else if title == "Prompt" || title == "Summary" {
        SidebarTone::Plain
    } else if label == "Usage" {
        SidebarTone::Detail
    } else {
        SidebarTone::Plain
    };
    let emphasised = label == "State"
        || lower.contains("waiting")
        || lower.contains("thinking")
        || lower.contains("running tool")
        || lower.contains("ready")
        || lower.contains("generating")
        || label == "Hint"
        || pr_working;
    (tone, emphasised)
}

/// One Usage line (`5h ┃┃┃┃░░░░░░ 38% · 3h`, as written by
/// [`crate::usage::format_sidebar_usage_windows`]) taken apart.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UsageBarLine<'a> {
    pub label: &'a str,
    pub filled: usize,
    pub empty: usize,
    pub percent_text: &'a str,
    pub percent: f64,
    /// Everything after the percentage, e.g. ` · 3h`.
    pub reset: &'a str,
}

pub(crate) fn parse_usage_bar_line(line: &str) -> Option<UsageBarLine<'_>> {
    use crate::usage::{USAGE_BAR_EMPTY, USAGE_BAR_FILLED};

    let (label, rest) = line.split_once(' ')?;
    let bar_len: usize = rest
        .chars()
        .take_while(|c| *c == USAGE_BAR_FILLED || *c == USAGE_BAR_EMPTY)
        .map(char::len_utf8)
        .sum();
    if bar_len == 0 {
        return None;
    }
    let (bar, tail) = rest.split_at(bar_len);
    let (percent_text, reset) = tail.trim_start().split_once('%')?;
    let percent: f64 = percent_text.parse().ok()?;
    Some(UsageBarLine {
        label,
        filled: bar.chars().filter(|c| *c == USAGE_BAR_FILLED).count(),
        empty: bar.chars().filter(|c| *c == USAGE_BAR_EMPTY).count(),
        percent_text,
        percent,
        reset,
    })
}

/// A waiting request the sidebar reports, already matched to the feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarPendingInput {
    pub notification_type: String,
    pub message: String,
}

/// The process-local signals the sidebar reads. Every field is something an
/// interface either observed itself or leaves at its default; nothing here is
/// guessed from another process's state.
#[derive(Debug, Clone, Default)]
pub(crate) struct SidebarRuntime<'a> {
    /// Requests waiting on this feature, oldest first.
    pub pending_inputs: Vec<SidebarPendingInput>,
    /// Why the harness stopped, from the attention layer.
    pub attention: Option<AttentionState>,
    /// A tool call is in flight (IPC `tool-start`).
    pub running_tool: bool,
    pub thinking: bool,
    pub summary_generating: bool,
    /// Live Codex app-server events (IPC).
    pub codex_live: Option<&'a CodexLiveThreadState>,
    /// The dedicated PR-review session is mid-turn.
    pub pr_review_working: bool,
    pub ai_review_running: bool,
}

impl App {
    /// The runtime signals this (TUI) process observed for `feature`, matched
    /// the way the TUI always has: by the viewed tmux session, or by
    /// project and feature name.
    pub(crate) fn tui_sidebar_runtime<'a>(
        &'a self,
        project: &Project,
        feature: &Feature,
        view_session: &str,
    ) -> SidebarRuntime<'a> {
        let pending_inputs = self
            .pending_inputs
            .iter()
            .filter(|input| {
                input.session_id == view_session
                    || (input.project_name.as_deref() == Some(project.name.as_str())
                        && input.feature_name.as_deref() == Some(feature.name.as_str()))
            })
            .map(|input| SidebarPendingInput {
                notification_type: input.notification_type.clone(),
                message: input.message.clone(),
            })
            .collect();
        SidebarRuntime {
            pending_inputs,
            attention: self.feature_attention(&feature.tmux_session),
            running_tool: self.ipc_tool_sessions.contains(&feature.tmux_session),
            thinking: self.is_feature_thinking(&feature.tmux_session),
            summary_generating: self
                .summary_state
                .generating
                .contains(&feature.tmux_session),
            codex_live: self.codex_live_thread(&feature.tmux_session),
            pr_review_working: self
                .dedicated_review_session_working_for_workdir(&feature.workdir)
                .unwrap_or(false),
            ai_review_running: self.ai_review_running_for_workdir(&feature.workdir),
        }
    }

    /// Build the sidebar for `session` (or, when the window has no session
    /// row, the feature's first session of `kind`). `None` for a kind that
    /// has no agent sidebar.
    pub(crate) fn assemble_agent_sidebar(
        &self,
        feature: &Feature,
        session: Option<&FeatureSession>,
        kind: &SessionKind,
        runtime: &SidebarRuntime<'_>,
    ) -> Option<AgentSidebarAssembly> {
        let (context_snapshot, context_hint_visible) = session
            .and_then(|session| self.context_states.get(&session.id))
            .map(|context| {
                (
                    context.snapshot.clone(),
                    session.is_some_and(|session| {
                        self.context_hint_states
                            .is_eligible(&session.id, Some(context))
                    }),
                )
            })
            .unwrap_or((None, false));

        // When the harness said why it stopped, say that instead of counting
        // inputs. The sidebar's status is the one place inside a session that
        // reports the session's own state, and "Waiting for 1 input" is exactly
        // the ambiguity the attention layer exists to remove.
        let status_line = match runtime.attention {
            Some(AttentionState::Question) => "Waiting on your answer".to_string(),
            Some(AttentionState::CompletedAwaitingReview) => {
                "Completed \u{2014} awaiting review".to_string()
            }
            Some(AttentionState::Waiting) => "Waiting for input".to_string(),
            None => match runtime.pending_inputs.len() {
                0 => "Ready".to_string(),
                1 => "Waiting for 1 input".to_string(),
                n => format!("Waiting for {n} inputs"),
            },
        };
        // Resolve sidebar content by the viewed session's stable identity. A
        // TODO reference from another harness must not make this session show an
        // unrelated TODO box.
        let active_todos_text =
            session.and_then(|session| self.active_todos_sidebar_cache.get(&session.id).cloned());
        let active_todo_affordance = session.is_some_and(|session| {
            session
                .todo_reference
                .as_ref()
                .is_some_and(|reference| reference.launched_from_todo_menu)
        });
        let common = SidebarCommon {
            status_line,
            context_snapshot,
            context_hint_visible,
            active_todos_text,
            active_todo_affordance,
        };

        match kind {
            SessionKind::Opencode => Some(self.opencode_sidebar(feature, session, runtime, common)),
            SessionKind::Claude => Some(self.claude_sidebar(feature, session, runtime, common)),
            SessionKind::Codex => Some(self.codex_sidebar(feature, session, runtime, common)),
            SessionKind::Pi => Some(self.pi_sidebar(feature, session, runtime, common)),
            _ => None,
        }
    }

    fn opencode_sidebar(
        &self,
        feature: &Feature,
        session: Option<&FeatureSession>,
        runtime: &SidebarRuntime<'_>,
        common: SidebarCommon,
    ) -> AgentSidebarAssembly {
        let opencode_sidebar = self.opencode_sidebar_cache.get(&feature.tmux_session);
        let usage_line = session
            .and_then(|session| session.status_text.as_deref())
            .map(format_sidebar_usage)
            .filter(|line| line != "Usage: unavailable");
        let prompt_full = opencode_sidebar
            .and_then(|sidebar| sidebar.latest_prompt.as_deref())
            .or_else(|| self.latest_prompt_for_session(&feature.tmux_session));
        let prompt_text = opencode_sidebar_prompt_text(prompt_full);
        let work_text = opencode_sidebar_work_text(opencode_sidebar);
        let todos_text = opencode_sidebar_todos_text(opencode_sidebar);
        let summary_text = opencode_sidebar_summary_text(
            runtime.summary_generating,
            feature.summary.as_deref(),
            opencode_sidebar,
        );
        let model_text = self.sidebar_model_cache.get(&feature.tmux_session).cloned();
        let diff_review = pending_diff_review_work_text(&runtime.pending_inputs);
        let activity_line = if diff_review.is_some() {
            "Waiting for diff review".to_string()
        } else if opencode_sidebar
            .and_then(|sidebar| sidebar.pending_permission.as_ref())
            .is_some()
        {
            "Waiting on permission".to_string()
        } else if runtime.running_tool {
            "Running tool".to_string()
        } else if runtime.thinking {
            "Thinking".to_string()
        } else {
            common.status_line
        };

        let pending_diff_review = diff_review.is_some();
        AgentSidebarAssembly {
            data: AgentSidebarData {
                agent_kind: SessionKind::Opencode,
                status_text: append_model_status_line(
                    opencode_sidebar_status_text(activity_line, usage_line, opencode_sidebar),
                    model_text.as_deref(),
                ),
                usage_text: sidebar_usage_text(self, &SessionKind::Opencode),
                model_text,
                prompt_text,
                work_text: diff_review
                    .or(work_text)
                    .or_else(|| fallback_sidebar_work_text(runtime)),
                todos_text,
                active_todos_text: common.active_todos_text,
                active_todo_affordance: common.active_todo_affordance,
                summary_text,
                issue_source_text: issue_source_sidebar_text(feature),
                pr_triage_text: pr_triage_sidebar_text(self, feature, runtime),
                plan_text: plan_sidebar_text(self, feature),
                context_snapshot: common.context_snapshot,
                context_hint_visible: common.context_hint_visible,
            },
            prompt_full: prompt_full.map(ToOwned::to_owned),
            pending_diff_review,
        }
    }

    fn claude_sidebar(
        &self,
        feature: &Feature,
        session: Option<&FeatureSession>,
        runtime: &SidebarRuntime<'_>,
        common: SidebarCommon,
    ) -> AgentSidebarAssembly {
        let usage_line = session
            .and_then(|session| session.status_text.as_deref())
            .map(format_sidebar_usage);
        let prompt_full = self.latest_prompt_for_session(&feature.tmux_session);
        let prompt_text = sidebar_prompt_text(None, prompt_full);
        let summary_text = if runtime.summary_generating {
            Some("Generating summary...".to_string())
        } else {
            feature.summary.clone()
        };
        let diff_review = pending_diff_review_work_text(&runtime.pending_inputs);
        let pending_diff_review = diff_review.is_some();
        let work_text = diff_review.or_else(|| fallback_sidebar_work_text(runtime));
        let summary_text = compose_sidebar_summary_text(None, summary_text);
        let activity_line = sidebar_status_activity_text(work_text.is_some(), common.status_line);
        let model_text = self.sidebar_model_cache.get(&feature.tmux_session).cloned();
        let status_text = append_model_status_line(
            compose_sidebar_status_text(activity_line, usage_line, None),
            model_text.as_deref(),
        );
        let claude_session_id = session.and_then(|s| s.claude_session_id.as_deref());
        let todos_text = read_claude_task_state(&feature.workdir, claude_session_id)
            .as_ref()
            .and_then(claude_sidebar_todos_text);

        AgentSidebarAssembly {
            data: AgentSidebarData {
                agent_kind: SessionKind::Claude,
                status_text,
                usage_text: sidebar_usage_text(self, &SessionKind::Claude),
                model_text,
                prompt_text,
                work_text,
                todos_text,
                active_todos_text: common.active_todos_text,
                active_todo_affordance: common.active_todo_affordance,
                summary_text,
                issue_source_text: issue_source_sidebar_text(feature),
                pr_triage_text: pr_triage_sidebar_text(self, feature, runtime),
                plan_text: plan_sidebar_text(self, feature),
                context_snapshot: common.context_snapshot,
                context_hint_visible: common.context_hint_visible,
            },
            prompt_full: prompt_full.map(ToOwned::to_owned),
            pending_diff_review,
        }
    }

    fn codex_sidebar(
        &self,
        feature: &Feature,
        session: Option<&FeatureSession>,
        runtime: &SidebarRuntime<'_>,
        common: SidebarCommon,
    ) -> AgentSidebarAssembly {
        let usage_line = session
            .and_then(|session| session.status_text.as_deref())
            .map(format_sidebar_usage);
        let session_prompt = codex_sidebar_source(&SessionKind::Codex, session)
            .and_then(|source| self.cached_codex_session_prompt(&feature.workdir, &source.id));
        let fallback_prompt = self.latest_prompt_for_session(&feature.tmux_session);
        let prompt_text = sidebar_prompt_text(session_prompt, fallback_prompt);
        let prompt_full = select_sidebar_prompt(session_prompt, fallback_prompt);
        let summary_text = if runtime.summary_generating {
            Some("Generating summary...".to_string())
        } else {
            feature.summary.clone()
        };
        let codex_live = runtime.codex_live;
        let diff_review = pending_diff_review_work_text(&runtime.pending_inputs);
        let pending_diff_review = diff_review.is_some();
        let work_text = diff_review
            .or_else(|| codex_live.and_then(|live| live.sidebar_work_text()))
            .or_else(|| fallback_sidebar_work_text(runtime));
        let summary_text = compose_sidebar_summary_text(
            codex_live.and_then(|live| live.summary_prefix()),
            summary_text,
        );
        let activity_line = sidebar_status_activity_text(work_text.is_some(), common.status_line);
        let usage_confidence = format_codex_usage_source_confidence(&SessionKind::Codex, session);
        let model_text = codex_sidebar_source(&SessionKind::Codex, session)
            .and_then(|source| self.cached_codex_session_model(&feature.workdir, &source.id))
            .map(ToOwned::to_owned)
            .or_else(|| self.sidebar_model_cache.get(&feature.tmux_session).cloned())
            .or_else(codex_configured_model_text);
        let status_text = append_model_status_line(
            compose_sidebar_status_text(activity_line, usage_line, usage_confidence),
            model_text.as_deref(),
        );

        AgentSidebarAssembly {
            data: AgentSidebarData {
                agent_kind: SessionKind::Codex,
                status_text,
                usage_text: sidebar_usage_text(self, &SessionKind::Codex),
                model_text,
                prompt_text,
                work_text,
                todos_text: None,
                active_todos_text: common.active_todos_text,
                active_todo_affordance: common.active_todo_affordance,
                summary_text,
                issue_source_text: issue_source_sidebar_text(feature),
                pr_triage_text: pr_triage_sidebar_text(self, feature, runtime),
                plan_text: plan_sidebar_text(self, feature),
                context_snapshot: common.context_snapshot,
                context_hint_visible: common.context_hint_visible,
            },
            prompt_full,
            pending_diff_review,
        }
    }

    fn pi_sidebar(
        &self,
        feature: &Feature,
        session: Option<&FeatureSession>,
        runtime: &SidebarRuntime<'_>,
        common: SidebarCommon,
    ) -> AgentSidebarAssembly {
        let usage_line = session
            .and_then(|session| session.status_text.as_deref())
            .map(format_sidebar_usage);
        let prompt_full = self.latest_prompt_for_session(&feature.tmux_session);
        let prompt_text = sidebar_prompt_text(None, prompt_full);
        let diff_review = pending_diff_review_work_text(&runtime.pending_inputs);
        let pending_diff_review = diff_review.is_some();
        let work_text = diff_review.or_else(|| fallback_sidebar_work_text(runtime));
        let summary_text = compose_sidebar_summary_text(None, feature.summary.clone());
        let activity_line = sidebar_status_activity_text(work_text.is_some(), common.status_line);
        let model_text = self.sidebar_model_cache.get(&feature.tmux_session).cloned();
        let status_text = append_model_status_line(
            compose_sidebar_status_text(activity_line, usage_line, None),
            model_text.as_deref(),
        );

        AgentSidebarAssembly {
            data: AgentSidebarData {
                agent_kind: SessionKind::Pi,
                status_text,
                usage_text: sidebar_usage_text(self, &SessionKind::Pi),
                model_text,
                prompt_text,
                work_text,
                todos_text: None,
                active_todos_text: common.active_todos_text,
                active_todo_affordance: common.active_todo_affordance,
                summary_text,
                issue_source_text: issue_source_sidebar_text(feature),
                pr_triage_text: pr_triage_sidebar_text(self, feature, runtime),
                plan_text: plan_sidebar_text(self, feature),
                context_snapshot: common.context_snapshot,
                context_hint_visible: common.context_hint_visible,
            },
            prompt_full: prompt_full.map(ToOwned::to_owned),
            pending_diff_review,
        }
    }
}

/// Inputs every harness's sidebar shares.
struct SidebarCommon {
    status_line: String,
    context_snapshot: Option<SessionContextSnapshot>,
    context_hint_visible: bool,
    active_todos_text: Option<String>,
    active_todo_affordance: bool,
}

/// Sidebar-box counterpart of the TUI's ambient Viewing-mode PR badge.
pub(crate) fn pr_triage_sidebar_text(
    app: &App,
    feature: &Feature,
    runtime: &SidebarRuntime<'_>,
) -> Option<String> {
    if let Some(pr) = app.active_pr_for_feature(&feature.id) {
        let mut lines = vec![match pr.unresolved_threads {
            Some(count) => format!("PR: #{} · {count} open", pr.number),
            None => format!("PR: #{}", pr.number),
        }];
        if runtime.pr_review_working {
            lines.push("Status: Working".to_string());
        }
        if runtime.ai_review_running {
            lines.push("AI review: Running".to_string());
        }
        return Some(lines.join("\n"));
    }
    let pr = app.terminal_pr_for_feature(&feature.id)?;
    Some(format!("PR: #{} {}", pr.number, pr.state.label()))
}

pub(crate) fn issue_source_sidebar_text(feature: &Feature) -> Option<String> {
    let source = feature.issue_source.as_ref()?;
    let comment = match &source.comment_status {
        crate::project::IssueCommentStatus::Pending => "Pending",
        crate::project::IssueCommentStatus::Posted => "Posted",
        crate::project::IssueCommentStatus::Failed(_) => "Failed",
    };
    Some(format!(
        "Repository: {}\nIssue: #{}\nComment: {comment}",
        source.canonical_repository(),
        source.number
    ))
}

/// Reads the sidebar's plan status line from the background-loaded cache
/// (`App::sidebar_effective_plan_cache`) rather than resolving it here.
/// Resolution touches the filesystem (`is_file`, and `canonicalize` for a
/// manually selected plan) and the TUI rebuilds the sidebar on every `draw()`
/// of the pane view — up to ~20x/sec while in Viewing mode — so it must stay
/// off the render thread.
pub(crate) fn plan_sidebar_text(app: &App, feature: &Feature) -> String {
    app.sidebar_effective_plan_cache
        .get(&feature.tmux_session)
        .cloned()
        .unwrap_or_else(|| "No plan selected".to_string())
}

/// Whether the background sidebar load found an effective plan for
/// `feature` — the same cache, and the same off-the-hot-path reason, as
/// [`plan_sidebar_text`].
pub(crate) fn plan_sidebar_has_plan(app: &App, feature: &Feature) -> bool {
    app.sidebar_effective_plan_cache
        .contains_key(&feature.tmux_session)
}

pub(crate) fn append_model_status_line(
    mut status_text: String,
    model_text: Option<&str>,
) -> String {
    let Some(model_text) = model_text.map(str::trim).filter(|line| !line.is_empty()) else {
        return status_text;
    };
    if !status_text.is_empty() {
        status_text.push('\n');
    }
    status_text.push_str(model_text);
    status_text
}

pub(crate) fn codex_configured_model_text() -> Option<String> {
    crate::codex_config::configured_model()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .map(|model| format!("Model: {model}"))
}

pub(crate) fn opencode_sidebar_status_text(
    activity_line: String,
    usage_line: Option<String>,
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> String {
    let mut lines = vec![format!("Activity: {activity_line}")];
    if let Some(usage_line) = usage_line {
        lines.push(usage_line);
    }
    if let Some(reasoning_tokens) = opencode_sidebar
        .and_then(|sidebar| sidebar.reasoning_tokens)
        .filter(|tokens| *tokens > 0)
    {
        lines.push(format!(
            "Reasoning: {}",
            crate::token_tracking::format_token_count(reasoning_tokens)
        ));
    }
    if let Some(change_line) = opencode_sidebar.and_then(|sidebar| sidebar.change_summary_line()) {
        lines.push(change_line);
    }
    lines.join("\n")
}

pub(crate) fn opencode_sidebar_work_text(
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(status) = opencode_sidebar
        .and_then(|sidebar| sidebar.status.as_deref())
        .filter(|status| !status.is_empty())
    {
        lines.push(format!("State: {status}"));
    }
    if let Some(tool) = opencode_sidebar
        .and_then(|sidebar| sidebar.last_tool.as_deref())
        .filter(|tool| !tool.is_empty())
    {
        lines.push(format!("Tool: {tool}"));
    }
    if let Some(permission) = opencode_sidebar
        .and_then(|sidebar| sidebar.pending_permission.as_deref())
        .filter(|permission| !permission.is_empty())
    {
        lines.push(format!(
            "Permission: {}",
            compact_sidebar_text(permission, SIDEBAR_WORK_VALUE_CHARS)
        ));
    }
    if let Some(lsp_summary) = opencode_sidebar
        .and_then(|sidebar| sidebar.lsp_summary.as_deref())
        .filter(|summary| !summary.is_empty())
    {
        lines.push(format!(
            "LSP: {}",
            compact_sidebar_text(lsp_summary, SIDEBAR_WORK_VALUE_CHARS)
        ));
    }
    if let Some(error) = opencode_sidebar
        .and_then(|sidebar| sidebar.last_error.as_deref())
        .filter(|error| !error.is_empty())
    {
        lines.push(format!(
            "Error: {}",
            compact_sidebar_text(error, SIDEBAR_WORK_VALUE_CHARS)
        ));
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub(crate) fn opencode_sidebar_todos_text(
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> Option<String> {
    let sidebar = opencode_sidebar?;
    let todo_count = sidebar
        .todo_count
        .unwrap_or(sidebar.todo_preview.len() as u64) as usize;
    if todo_count == 0 && sidebar.todo_preview.is_empty() {
        return None;
    }

    // Opencode only provides open items — no completed count — so skip the
    // progress bar and render all preview items as pending (○).
    const MAX_SHOWN: usize = 5;
    let mut lines: Vec<String> = sidebar
        .todo_preview
        .iter()
        .take(MAX_SHOWN)
        .map(|item| format!("○ {item}"))
        .collect();

    let remaining = todo_count.saturating_sub(lines.len());
    if remaining > 0 {
        lines.push(format!("+{remaining} more"));
    }

    Some(lines.join("\n"))
}

pub(crate) fn claude_sidebar_todos_text(task_state: &ClaudeTaskState) -> Option<String> {
    let total = task_state.tasks.len();
    if total == 0 {
        return None;
    }
    let completed = task_state.completed_count();

    // Progress bar: 8 filled/empty blocks + " X/Y"
    let bar_width = 8usize;
    let filled = (completed * bar_width).checked_div(total).unwrap_or(0);
    let empty = bar_width - filled;
    let mut lines = vec![format!(
        "{}{} {completed}/{total}",
        "█".repeat(filled),
        "░".repeat(empty),
    )];

    // Sliding window: show up to 5 tasks anchored at the first non-completed
    // task, with one completed item above it for context.
    let (window_start, window_end) = if total <= 5 {
        (0, total)
    } else {
        let first_active = task_state
            .tasks
            .iter()
            .position(|t| t.status != "completed")
            .unwrap_or(total);
        let start = first_active.saturating_sub(1);
        let end = (start + 5).min(total);
        // Re-anchor so we always fill the window when near the end.
        let start = end.saturating_sub(5);
        (start, end)
    };

    for task in &task_state.tasks[window_start..window_end] {
        let label = task.active_form.as_deref().unwrap_or(task.subject.as_str());
        let prefix = match task.status.as_str() {
            "completed" => "✓",
            "in_progress" => "●",
            _ => "○",
        };
        lines.push(format!("{prefix} {label}"));
    }

    let remaining = total - window_end;
    if remaining > 0 {
        lines.push(format!("+{remaining} more"));
    }

    Some(lines.join("\n"))
}

pub(crate) fn opencode_sidebar_summary_text(
    generating: bool,
    feature_summary: Option<&str>,
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> String {
    if generating {
        return "Generating summary...".to_string();
    }

    if let Some(summary) = opencode_sidebar
        .and_then(|sidebar| sidebar.live_summary.as_deref())
        .filter(|summary| !summary.is_empty())
    {
        return compact_sidebar_block(
            summary,
            SIDEBAR_SUMMARY_PREVIEW_COLS,
            SIDEBAR_SUMMARY_PREVIEW_LINES,
        );
    }

    feature_summary
        .map(|summary| {
            compact_sidebar_block(
                summary,
                SIDEBAR_SUMMARY_PREVIEW_COLS,
                SIDEBAR_SUMMARY_PREVIEW_LINES,
            )
        })
        .unwrap_or_default()
}

pub(crate) fn opencode_sidebar_prompt_text(prompt: Option<&str>) -> String {
    prompt
        .map(|prompt| {
            compact_sidebar_block(
                prompt,
                SIDEBAR_PROMPT_PREVIEW_COLS,
                SIDEBAR_PROMPT_PREVIEW_LINES,
            )
        })
        .unwrap_or_default()
}

pub(crate) fn sidebar_status_activity_text(
    has_work_text: bool,
    idle_text: String,
) -> Option<String> {
    if has_work_text { None } else { Some(idle_text) }
}

pub(crate) fn compose_sidebar_status_text(
    activity_line: Option<String>,
    usage_line: Option<String>,
    usage_confidence: Option<String>,
) -> String {
    let mut status_lines = Vec::new();
    if let Some(activity) = activity_line {
        status_lines.push(format!("Activity: {activity}"));
    }
    if let Some(usage_line) = usage_line {
        status_lines.push(usage_line);
    }
    if let Some(confidence) = usage_confidence {
        status_lines.push(confidence);
    }
    status_lines.join("\n")
}

pub(crate) fn compose_sidebar_summary_text(
    reasoning_text: Option<String>,
    summary_text: Option<String>,
) -> String {
    match (reasoning_text, summary_text) {
        (Some(reasoning), Some(summary)) => compact_sidebar_text(
            &format!(
                "Reasoning: {}\n\n{}",
                compact_sidebar_text(&reasoning, 160),
                summary
            ),
            80,
        ),
        (Some(reasoning), None) => compact_sidebar_text(
            &format!("Reasoning: {}", compact_sidebar_text(&reasoning, 160)),
            80,
        ),
        (None, Some(summary)) => compact_sidebar_text(&summary, 80),
        (None, None) => String::new(),
    }
}

pub(crate) fn codex_sidebar_source<'a>(
    sidebar_kind: &SessionKind,
    session: Option<&'a FeatureSession>,
) -> Option<&'a TokenUsageSource> {
    if *sidebar_kind != SessionKind::Codex {
        return None;
    }

    session
        .and_then(|session| session.token_usage_source.as_ref())
        .filter(|source| source.provider == TokenUsageProvider::Codex)
}

pub(crate) fn format_codex_usage_source_confidence(
    sidebar_kind: &SessionKind,
    session: Option<&FeatureSession>,
) -> Option<String> {
    if *sidebar_kind != SessionKind::Codex {
        return None;
    }

    let match_kind = session?.token_usage_source_match.as_ref()?;
    match match_kind {
        TokenUsageSourceMatch::Exact => None,
        TokenUsageSourceMatch::Inferred => Some("Usage source: inferred workdir match".to_string()),
    }
}

pub(crate) fn sidebar_prompt_text(
    session_prompt: Option<&str>,
    fallback_prompt: Option<&str>,
) -> String {
    let prompt = select_sidebar_prompt(session_prompt, fallback_prompt);
    prompt
        .map(|prompt| compact_sidebar_text(&prompt, 48))
        .unwrap_or_default()
}

pub(crate) fn select_sidebar_prompt(
    session_prompt: Option<&str>,
    fallback_prompt: Option<&str>,
) -> Option<String> {
    session_prompt
        .map(ToOwned::to_owned)
        .or_else(|| fallback_prompt.map(ToOwned::to_owned))
}

pub(crate) fn compact_sidebar_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    let truncated: String = compact.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

pub(crate) fn compact_sidebar_block(text: &str, max_cols: usize, max_lines: usize) -> String {
    if max_cols == 0 || max_lines == 0 {
        return String::new();
    }

    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return String::new();
    }

    let words: Vec<&str> = compact.split(' ').collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut index = 0;

    while index < words.len() && lines.len() < max_lines {
        let word = words[index];
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };

        if candidate.chars().count() <= max_cols {
            current = candidate;
            index += 1;
            continue;
        }

        if current.is_empty() {
            lines.push(compact_sidebar_text(word, max_cols));
            index += 1;
        } else {
            lines.push(current);
            current = String::new();
        }
    }

    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current);
    }

    if index < words.len()
        && let Some(last) = lines.pop()
    {
        let trimmed = compact_sidebar_text(&last, max_cols.saturating_sub(1));
        lines.push(format!("{trimmed}…"));
    }

    lines.join("\n")
}

/// What the agent is doing when nothing more specific (a diff review, an
/// OpenCode or Codex work item) says so: the oldest waiting request, a tool
/// call or a turn in progress.
pub(crate) fn fallback_sidebar_work_text(runtime: &SidebarRuntime<'_>) -> Option<String> {
    if let Some(first) = runtime.pending_inputs.first() {
        let message = first.message.trim();
        let mut text = format!(
            "State: waiting for input\nRequest: {}",
            if message.is_empty() {
                "Harness is waiting for input"
            } else {
                message
            }
        );
        if runtime.pending_inputs.len() > 1 {
            text.push_str(&format!(
                "\nQueue: {} pending",
                runtime.pending_inputs.len()
            ));
        }
        return Some(text);
    }

    if runtime.running_tool {
        return Some("State: running tool".to_string());
    }

    if runtime.thinking {
        return Some("State: thinking".to_string());
    }

    None
}

pub(crate) fn pending_diff_review_work_text(inputs: &[SidebarPendingInput]) -> Option<String> {
    let matching_inputs = inputs
        .iter()
        .filter(|input| {
            matches!(
                input.notification_type.as_str(),
                "diff-review" | "change-reason"
            )
        })
        .collect::<Vec<_>>();

    let first = matching_inputs.first()?;
    let message = first.message.trim();
    let (state, default_request) = match first.notification_type.as_str() {
        "change-reason" => (
            "waiting for change reason",
            "Explain why this change is needed.",
        ),
        _ => (
            "waiting for diff review",
            "Review the proposed change before continuing.",
        ),
    };
    let mut text = format!(
        "State: {state}\nRequest: {}",
        if message.is_empty() {
            default_request
        } else {
            message
        }
    );
    if matching_inputs.len() > 1 {
        text.push_str(&format!("\nQueue: {} pending", matching_inputs.len()));
    }
    text.push('\n');
    text.push_str(DIFF_REVIEW_TUI_HINT);
    Some(text)
}

pub(crate) fn format_sidebar_usage(status: &str) -> String {
    let mut input = None;
    let mut output = None;
    let mut effective = None;
    let mut cost = None;

    for part in status
        .split(" · ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if let Some(value) = part.strip_suffix(" in") {
            input = Some(value.to_string());
        } else if let Some(value) = part.strip_suffix(" out") {
            output = Some(value.to_string());
        } else if let Some(value) = part.strip_suffix(" eff") {
            effective = Some(value.to_string());
        } else if part.starts_with('$') || part.starts_with("<$") {
            cost = Some(part.to_string());
        }
    }

    let mut lines = Vec::new();
    if let Some(value) = input {
        lines.push(format!("Input: {value} tokens"));
    }
    if let Some(value) = output {
        lines.push(format!("Output: {value} tokens"));
    }
    if let Some(value) = effective {
        lines.push(format!("Effective: {value} tokens"));
    }
    if let Some(cost_value) = cost {
        lines.push(format!("Cost: {cost_value}"));
    }

    if lines.is_empty() {
        format!("Usage: {status}")
    } else {
        lines.join("\n")
    }
}

/// Body for the sidebar's **Usage** box: this harness's account-level
/// rate-limit windows, read from the same cached `UsageData` the dashboard
/// status bar uses. `None` (box omitted) when the harness has no usage
/// source or nothing has been fetched yet.
pub(crate) fn sidebar_usage_text(app: &App, kind: &SessionKind) -> Option<String> {
    let windows = crate::usage::usage_windows_for_session_kind(kind, &app.usage.get_data());
    crate::usage::format_sidebar_usage_windows(&windows)
}
