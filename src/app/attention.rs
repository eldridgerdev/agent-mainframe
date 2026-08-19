//! Why a stopped agent session is stopped.
//!
//! The dashboard already knows *that* a session went idle — `sync.rs` watches
//! the thinking transition and raises a generic "waiting for input" pending
//! input. This module layers the *why* on top: is the agent asking a question,
//! has it finished work that wants review, or is it merely parked?
//!
//! Design constraints (see `PLAN.md`):
//! - Nothing here is persisted. The map lives on `App`, is rebuilt from hook
//!   events, and is empty after a restart. No `amf.db` migration.
//! - `ProjectStatus` is untouched; the dashboard composes the persisted status
//!   with this in-memory layer at render time.
//! - A harness that cannot distinguish question-from-done degrades to
//!   [`AttentionState::Waiting`] rather than guessing.

use chrono::{DateTime, Utc};

use super::App;
use crate::project::{AgentKind, ProjectStatus};

/// Why a session is asking to be looked at.
///
/// Ordering is deliberate and load-bearing: `Question` sorts before
/// `CompletedAwaitingReview`, which sorts before `Waiting`. The needs-attention
/// list and the feature-row roll-up both rely on it, so a question always wins
/// over a completion on the same feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AttentionState {
    /// The agent asked something and cannot proceed without an answer —
    /// including a tool-permission prompt, which blocks just as hard as a
    /// prose question does.
    Question,
    /// The agent finished its turn and the work is waiting to be looked at.
    CompletedAwaitingReview,
    /// The session is stopped but the harness could not say why.
    Waiting,
}

impl AttentionState {
    /// Short label for list rows and the needs-attention overlay.
    pub fn label(&self) -> &'static str {
        match self {
            AttentionState::Question => "Question",
            AttentionState::CompletedAwaitingReview => "Completed",
            AttentionState::Waiting => "Waiting",
        }
    }

    /// The status glyph for a dashboard row.
    ///
    /// `nerd_font` picks the icon set; the ASCII fallbacks are chosen to stay
    /// distinguishable from the row glyphs already in use (`✓` for ready, `⚙`
    /// for a pending worktree script, the spinner for thinking).
    pub fn glyph(&self, nerd_font: bool) -> &'static str {
        match (self, nerd_font) {
            // question-circle
            (AttentionState::Question, true) => "\u{f059}",
            (AttentionState::Question, false) => "?",
            // eye — "this wants a look"
            (AttentionState::CompletedAwaitingReview, true) => "\u{f06e}",
            (AttentionState::CompletedAwaitingReview, false) => "*",
            // hourglass
            (AttentionState::Waiting, true) => "\u{f254}",
            (AttentionState::Waiting, false) => "-",
        }
    }

    /// The theme colour for this state: a question is the urgent one and keeps
    /// the established waiting colour, a completion reads as done, and a
    /// generic wait is deliberately quiet so it does not compete with either.
    pub fn color(&self, theme: &crate::theme::Theme) -> ratatui::style::Color {
        match self {
            AttentionState::Question => theme.status_waiting.to_color(),
            AttentionState::CompletedAwaitingReview => theme.success.to_color(),
            AttentionState::Waiting => theme.text_muted.to_color(),
        }
    }

    /// Parse the event kind carried on a hook/notification payload.
    ///
    /// Unknown and absent kinds degrade to [`AttentionState::Waiting`] — a
    /// harness we don't understand still gets a row, just an undifferentiated
    /// one.
    pub fn from_event_kind(kind: Option<&str>) -> Self {
        match kind.unwrap_or_default() {
            "question" => AttentionState::Question,
            "completed" => AttentionState::CompletedAwaitingReview,
            _ => AttentionState::Waiting,
        }
    }
}

/// Classify a Claude `Notification` event into an attention state, or into
/// nothing at all.
///
/// `Notification` fires for far more than a blocked agent: a tool-permission
/// prompt blocks the turn, but a 60-second idle nudge, a successful login, and
/// the completion half of an elicitation do not. Only the blocking ones become
/// a `Question`; everything else returns `None` and is reported not at all,
/// because `Question` outranks every other state and a wrong one would mask
/// what the session actually said.
///
/// Newer Claude builds name the notification and older ones only carry the
/// message, so both are consulted. Anything unrecognised is dropped rather than
/// guessed at.
pub fn classify_notification(
    notification_type: Option<&str>,
    message: Option<&str>,
) -> Option<AttentionState> {
    let notification_type = notification_type.unwrap_or_default().to_ascii_lowercase();

    if !notification_type.is_empty() {
        // Blocked on the user: a permission prompt, or an elicitation.
        let blocks = notification_type.contains("permission")
            || notification_type.contains("approval")
            || notification_type.ends_with("_request");
        return blocks.then_some(AttentionState::Question);
    }

    // Untyped: the message is all there is to go on.
    let message = message.unwrap_or_default().to_ascii_lowercase();
    const BLOCKING_PHRASES: &[&str] = &["permission", "approve", "approval", "needs your input"];
    BLOCKING_PHRASES
        .iter()
        .any(|phrase| message.contains(phrase))
        .then_some(AttentionState::Question)
}

/// What a harness is able to tell AMF about a stopped session.
///
/// This is the containment mechanism for the fact that hook fidelity differs
/// per harness: rather than sprinkling `match agent` across the ingestion and
/// clearing paths, each harness declares its capabilities once here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HarnessCapabilities {
    /// The harness can signal "I am asking you something" distinctly from
    /// "I finished".
    pub reports_question: bool,
    /// The harness can signal "my turn ended" as a completion.
    pub reports_completed: bool,
    /// The harness reliably signals that it started producing output again,
    /// so attention can clear itself without the user opening the session.
    pub reports_new_output: bool,
}

impl HarnessCapabilities {
    const NONE: Self = Self {
        reports_question: false,
        reports_completed: false,
        reports_new_output: false,
    };

    /// Capabilities for `agent`, as wired by `app/setup.rs`.
    ///
    /// - **Claude** runs AMF's full hook set: `Notification` marks a question,
    ///   `Stop` marks a completion, and `UserPromptSubmit`/`PreToolUse` mark
    ///   new output.
    /// - **Codex** runs `codex-notify.sh`, which fires once when the turn ends
    ///   and cannot separate a question from a completion — so it can claim
    ///   neither, and everything it reports narrows to `Waiting`. Its
    ///   `thinking-stop` event does give reliable new-output tracking.
    /// - **Opencode** runs the AMF plugins, which raise an explicit
    ///   `input-request`, plus sidebar/pane thinking detection for new output.
    /// - **Pi** has no hook mechanism at all (`src/pi.rs` is a version probe),
    ///   so it reports nothing and clears on open.
    pub fn for_agent(agent: &AgentKind) -> Self {
        match agent {
            AgentKind::Claude => Self {
                reports_question: true,
                reports_completed: true,
                reports_new_output: true,
            },
            AgentKind::Codex => Self {
                reports_question: false,
                reports_completed: false,
                reports_new_output: true,
            },
            AgentKind::Opencode => Self {
                reports_question: true,
                reports_completed: true,
                reports_new_output: true,
            },
            AgentKind::Pi => Self::NONE,
        }
    }

    /// Narrow `state` to what this harness can actually justify claiming.
    ///
    /// A harness that cannot distinguish a question reports `Waiting` instead,
    /// so the UI never shows a distinction the signal doesn't support.
    pub fn resolve(&self, state: AttentionState) -> AttentionState {
        match state {
            AttentionState::Question if !self.reports_question => AttentionState::Waiting,
            AttentionState::CompletedAwaitingReview if !self.reports_completed => {
                AttentionState::Waiting
            }
            other => other,
        }
    }

    /// `true` when attention for this harness can only be cleared by the user
    /// opening the session, because no new-output signal will arrive.
    pub fn clears_on_open(&self) -> bool {
        !self.reports_new_output
    }
}

/// Where an attention state came from, which decides whether it may replace a
/// standing `Question`.
///
/// The distinction matters because AMF has two signals of very different
/// quality. A harness hook is the agent itself saying what happened, so a
/// `Stop` really does end the turn that a permission prompt interrupted.
/// `sync.rs`'s thinking-transition fallback only observes that a session went
/// quiet, which is exactly what a blocked agent looks like — so it must never
/// talk a `Question` down into a completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttentionSource {
    /// A harness lifecycle event (`type: "attention"`), which is authoritative.
    Hook,
    /// Inferred by `sync.rs` from the thinking transition.
    Inferred,
}

/// A session's current attention state plus the provenance needed to age it
/// out and to decide how it clears.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttentionRecord {
    pub state: AttentionState,
    /// When the state was raised, used for stale ageing and for ordering the
    /// needs-attention list oldest-first within a state.
    pub since: DateTime<Utc>,
    /// The harness that produced the signal, so the clearing strategy is
    /// available without re-resolving the feature.
    pub agent: AgentKind,
    /// The capability level that produced `state`, recorded so the UI can
    /// explain a generic `Waiting` as "this harness can't say more" rather
    /// than "nothing is known yet".
    pub capabilities: HarnessCapabilities,
}

impl AttentionRecord {
    /// Build a record for `agent`, narrowing `state` to the harness's
    /// capabilities.
    pub fn new(agent: AgentKind, state: AttentionState, since: DateTime<Utc>) -> Self {
        let capabilities = HarnessCapabilities::for_agent(&agent);
        Self {
            state: capabilities.resolve(state),
            since,
            agent,
            capabilities,
        }
    }
}

/// One row of the needs-attention list: an attention record resolved back to
/// the feature that raised it.
#[derive(Debug, Clone)]
pub struct AttentionEntry {
    pub project_name: String,
    pub feature_name: String,
    pub tmux_session: String,
    pub record: AttentionRecord,
}

/// One row of the needs-attention overlay.
///
/// The overlay shows two things that used to be one list. Attention records
/// say *why* a session stopped; pending inputs are the older, flatter signal
/// and also carry work the attention layer knows nothing about (diff reviews,
/// change reasons, review-ready prompts). Where both describe the same stop
/// they share a row, so a Claude session that asked a question is listed once,
/// as a question — not twice.
#[derive(Debug, Clone)]
pub enum AttentionRow {
    /// A session the attention layer can explain. `pending` is the index of
    /// the input request the same feature raised, when there is one;
    /// selecting the row then goes through the existing input-request
    /// dispatch so the pending input is consumed exactly as before.
    Attention {
        entry: AttentionEntry,
        pending: Option<usize>,
    },
    /// A pending input with no attention record: a review prompt, or a
    /// harness that reports no lifecycle events at all.
    Pending(usize),
}

impl AttentionRow {
    /// The `pending_inputs` index this row dispatches through, if any.
    pub fn pending_index(&self) -> Option<usize> {
        match self {
            AttentionRow::Attention { pending, .. } => *pending,
            AttentionRow::Pending(index) => Some(*index),
        }
    }

    /// The attention state to label the row with, when one is known.
    pub fn state(&self) -> Option<AttentionState> {
        match self {
            AttentionRow::Attention { entry, .. } => Some(entry.record.state),
            AttentionRow::Pending(_) => None,
        }
    }
}

impl App {
    /// Record why `tmux_session` is stopped, narrowing the state to what
    /// `agent` can actually report.
    ///
    /// A `Question` already held for the session survives anything `Inferred`
    /// that would talk it down: a session blocked on a permission prompt looks
    /// exactly like one that went quiet, and the fallback must not overwrite
    /// the harness's better signal. A [`AttentionSource::Hook`] event does
    /// replace it — an actual `Stop` means the turn the question interrupted
    /// has since ended, so the question is answered and stale.
    ///
    /// Re-raising the state a session already holds keeps the original
    /// timestamp, so ageing measures how long the user has been ignoring it,
    /// not how chatty the harness is.
    pub fn record_attention(
        &mut self,
        tmux_session: &str,
        agent: &AgentKind,
        state: AttentionState,
        source: AttentionSource,
    ) {
        let incoming = AttentionRecord::new(agent.clone(), state, Utc::now());

        if let Some(existing) = self.attention.get(tmux_session) {
            if existing.state == incoming.state {
                return;
            }
            if source == AttentionSource::Inferred
                && existing.state == AttentionState::Question
                && incoming.state != AttentionState::Question
            {
                return;
            }
        }

        self.attention.insert(tmux_session.to_string(), incoming);
    }

    /// Drop the attention record for `tmux_session`, if any. Returns whether
    /// anything was removed, so callers can skip a redraw when nothing changed.
    pub fn clear_attention(&mut self, tmux_session: &str) -> bool {
        self.attention.remove(tmux_session).is_some()
    }

    /// The attention record for a feature's session, if it has one.
    pub fn attention_for(&self, tmux_session: &str) -> Option<&AttentionRecord> {
        self.attention.get(tmux_session)
    }

    /// Age out records older than `waiting_stale_minutes`, which have stopped
    /// being news. Returns whether anything was dropped.
    ///
    /// The session's generic wait ([`crate::app::PendingInput::is_session_wait`]) goes with
    /// the record. The two are the same stop seen through the old signal and
    /// the new one — dropping only the record would leave the request behind as
    /// a standalone row, and the session would never actually leave the `i`
    /// list. Pending inputs that mean something else (diff reviews, change
    /// reasons, review-ready) are separate work and are left alone.
    ///
    /// `0` disables ageing entirely, in which case this is a no-op.
    pub fn age_out_attention(&mut self) -> bool {
        let Some(threshold) = self.config.waiting_stale_threshold() else {
            return false;
        };
        let Ok(threshold) = chrono::Duration::from_std(threshold) else {
            return false;
        };

        let now = Utc::now();
        let stale: Vec<String> = self
            .attention
            .iter()
            .filter(|(_, record)| now.signed_duration_since(record.since) >= threshold)
            .map(|(session, _)| session.clone())
            .collect();
        if stale.is_empty() {
            return false;
        }

        let stale_features: Vec<(String, String)> = self
            .store
            .projects
            .iter()
            .flat_map(|project| {
                project
                    .features
                    .iter()
                    .filter(|feature| stale.contains(&feature.tmux_session))
                    .map(|feature| (project.name.clone(), feature.name.clone()))
            })
            .collect();

        for session in &stale {
            self.attention.remove(session);
        }
        self.pending_inputs.retain(|input| {
            !input.is_session_wait()
                || !stale_features.iter().any(|(project, feature)| {
                    input.project_name.as_deref() == Some(project.as_str())
                        && input.feature_name.as_deref() == Some(feature.as_str())
                })
        });
        true
    }

    /// Every session that wants looking at, ordered the way the needs-attention
    /// overlay, the leader jump, and the header counts all present it:
    /// questions first, then completions, then generic waits; oldest first
    /// within each group, so the thing that has been ignored longest is the
    /// thing you land on.
    ///
    /// Stopped features are skipped — an attention record can outlive the
    /// session that raised it if the user stops the feature without answering.
    pub fn needs_attention(&self) -> Vec<AttentionEntry> {
        let mut entries: Vec<AttentionEntry> = self
            .store
            .projects
            .iter()
            .flat_map(|project| {
                project.features.iter().filter_map(move |feature| {
                    if feature.status == ProjectStatus::Stopped {
                        return None;
                    }
                    let record = self.attention.get(&feature.tmux_session)?;
                    Some(AttentionEntry {
                        project_name: project.name.clone(),
                        feature_name: feature.name.clone(),
                        tmux_session: feature.tmux_session.clone(),
                        record: record.clone(),
                    })
                })
            })
            .collect();

        entries.sort_by(|a, b| {
            a.record
                .state
                .cmp(&b.record.state)
                .then(a.record.since.cmp(&b.record.since))
                .then(a.feature_name.cmp(&b.feature_name))
        });
        entries
    }

    /// The strongest attention state among a feature's sessions, for the
    /// feature-row roll-up. Attention is tracked per tmux session and a
    /// feature owns exactly one, so this is a lookup today; it exists as a
    /// named concept so the roll-up rule lives in one place.
    pub fn feature_attention(&self, tmux_session: &str) -> Option<AttentionState> {
        self.attention.get(tmux_session).map(|record| record.state)
    }

    /// The needs-attention overlay's rows: everything [`Self::needs_attention`]
    /// returns, in that order, followed by any pending input the attention
    /// layer did not account for.
    ///
    /// A pending input is folded into an attention row when it is the same
    /// feature's generic wait ([`crate::app::PendingInput::is_session_wait`]) — the two are
    /// the same stop seen through the old signal and the new one. Everything
    /// else (diff reviews, change reasons, review-ready) stays a row of its
    /// own, because those are separate pieces of work rather than a description
    /// of why a session stopped.
    pub fn attention_rows(&self) -> Vec<AttentionRow> {
        let mut claimed = vec![false; self.pending_inputs.len()];
        let mut rows: Vec<AttentionRow> = Vec::new();

        for entry in self.needs_attention() {
            let pending = self
                .pending_inputs
                .iter()
                .enumerate()
                .find(|(index, input)| {
                    !claimed[*index]
                        && input.is_session_wait()
                        && input.project_name.as_deref() == Some(entry.project_name.as_str())
                        && input.feature_name.as_deref() == Some(entry.feature_name.as_str())
                })
                .map(|(index, _)| index);
            if let Some(index) = pending {
                claimed[index] = true;
            }
            rows.push(AttentionRow::Attention { entry, pending });
        }

        rows.extend(
            claimed
                .iter()
                .enumerate()
                .filter(|(_, claimed)| !**claimed)
                .map(|(index, _)| AttentionRow::Pending(index)),
        );
        rows
    }

    /// What the header badge counts: the attention breakdown plus the number
    /// of pending inputs the attention layer did *not* explain.
    ///
    /// Both halves are needed. A diff review, a change reason, or a
    /// review-ready prompt is real work the badge has always advertised, and
    /// it must not disappear from the count just because some other session
    /// happens to hold an attention record. Derived from
    /// [`Self::attention_rows`] so the badge and the `i` overlay can never
    /// disagree about how many things are waiting.
    pub fn attention_badge_counts(&self) -> ((usize, usize, usize), usize) {
        let mut counts = (0usize, 0usize, 0usize);
        let mut unexplained = 0usize;
        for row in self.attention_rows() {
            match row.state() {
                Some(AttentionState::Question) => counts.0 += 1,
                Some(AttentionState::CompletedAwaitingReview) => counts.1 += 1,
                Some(AttentionState::Waiting) => counts.2 += 1,
                None => unexplained += 1,
            }
        }
        (counts, unexplained)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(agent: AgentKind, state: AttentionState) -> AttentionRecord {
        AttentionRecord::new(agent, state, Utc::now())
    }

    #[test]
    fn unsupported_harness_resolves_to_generic_waiting() {
        // Pi has no hooks: every state it could be handed collapses to Waiting.
        for state in [
            AttentionState::Question,
            AttentionState::CompletedAwaitingReview,
            AttentionState::Waiting,
        ] {
            assert_eq!(
                record(AgentKind::Pi, state).state,
                AttentionState::Waiting,
                "Pi should not claim {state:?}"
            );
        }

        // Codex fires one hook at turn end that cannot separate "asking" from
        // "done", so neither claim survives: both narrow to Waiting.
        assert_eq!(
            record(AgentKind::Codex, AttentionState::Question).state,
            AttentionState::Waiting
        );
        assert_eq!(
            record(AgentKind::Codex, AttentionState::CompletedAwaitingReview).state,
            AttentionState::Waiting
        );
    }

    #[test]
    fn full_fidelity_harnesses_keep_their_state() {
        for agent in [AgentKind::Claude, AgentKind::Opencode] {
            assert_eq!(
                record(agent.clone(), AttentionState::Question).state,
                AttentionState::Question
            );
            assert_eq!(
                record(agent, AttentionState::CompletedAwaitingReview).state,
                AttentionState::CompletedAwaitingReview
            );
        }
    }

    #[test]
    fn only_hookless_harnesses_clear_on_open() {
        assert!(HarnessCapabilities::for_agent(&AgentKind::Pi).clears_on_open());
        for agent in [AgentKind::Claude, AgentKind::Codex, AgentKind::Opencode] {
            assert!(!HarnessCapabilities::for_agent(&agent).clears_on_open());
        }
    }

    #[test]
    fn typed_notifications_that_block_the_turn_are_questions() {
        for typed in [
            "tool_permission_request",
            "permission_request",
            "Permission",
            "elicitation_request",
            "permission_approval",
        ] {
            assert_eq!(
                classify_notification(Some(typed), None),
                Some(AttentionState::Question),
                "{typed} blocks the turn and should be a question"
            );
        }
    }

    #[test]
    fn typed_notifications_that_do_not_block_report_nothing() {
        // Reporting a state here would claim the session stopped when it has
        // not, and `Question` outranks whatever it actually said.
        for typed in [
            "idle_timeout",
            "auth_success",
            "elicitation_complete",
            "task_finished",
            "some_new_thing_we_have_never_seen",
        ] {
            assert_eq!(
                classify_notification(Some(typed), None),
                None,
                "{typed} does not block the turn"
            );
        }
    }

    #[test]
    fn untyped_notifications_fall_back_to_the_message() {
        // Older Claude builds send no notification_type at all.
        for message in [
            "Claude needs your permission to use Bash",
            "Claude needs your input",
            "Approve this action?",
            "Waiting for APPROVAL",
        ] {
            assert_eq!(
                classify_notification(None, Some(message)),
                Some(AttentionState::Question),
                "{message:?} should read as blocking"
            );
        }

        for message in ["Task finished", "Logged in successfully", ""] {
            assert_eq!(
                classify_notification(None, Some(message)),
                None,
                "{message:?} should not read as blocking"
            );
        }
    }

    #[test]
    fn a_recognised_type_wins_over_a_misleading_message() {
        // The type is the more reliable signal, so a non-blocking type is not
        // second-guessed by prose that happens to contain "permission".
        assert_eq!(
            classify_notification(Some("idle_timeout"), Some("permission")),
            None
        );
    }

    #[test]
    fn nothing_at_all_reports_nothing() {
        assert_eq!(classify_notification(None, None), None);
    }

    #[test]
    fn event_kind_parsing_defaults_to_waiting() {
        assert_eq!(
            AttentionState::from_event_kind(Some("question")),
            AttentionState::Question
        );
        assert_eq!(
            AttentionState::from_event_kind(Some("completed")),
            AttentionState::CompletedAwaitingReview
        );
        assert_eq!(
            AttentionState::from_event_kind(Some("waiting")),
            AttentionState::Waiting
        );
        assert_eq!(
            AttentionState::from_event_kind(Some("who-knows")),
            AttentionState::Waiting
        );
        assert_eq!(
            AttentionState::from_event_kind(None),
            AttentionState::Waiting
        );
    }

    #[test]
    fn question_sorts_ahead_of_completion_and_waiting() {
        let mut states = [
            AttentionState::Waiting,
            AttentionState::CompletedAwaitingReview,
            AttentionState::Question,
        ];
        states.sort();
        assert_eq!(
            states,
            [
                AttentionState::Question,
                AttentionState::CompletedAwaitingReview,
                AttentionState::Waiting,
            ]
        );
    }
}
