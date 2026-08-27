use std::collections::HashMap;

use crate::context_tracking::{ContextBand, SessionContextState};

use super::{App, AppMode, ViewState};

/// Per-session lifecycle state for the context-usage sidebar hint.
///
/// This is deliberately separate from SessionContextState. Context telemetry
/// describes the current measurement; this state records only the user's
/// dismissal for that measurement's reset generation.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ContextHintState {
    pub reset_generation: u64,
    pub dismissed: bool,
}

impl ContextHintState {
    /// Reconcile dismissal state with the latest transient context state.
    ///
    /// A reset generation or a cleared trigger arms the hint again. A stale
    /// snapshot remains a real snapshot, so its warning/critical trigger is
    /// retained and can still be shown with its stale marker.
    pub fn sync(&mut self, context: Option<&SessionContextState>) {
        let generation = context.map_or(0, |state| state.reset.generation);
        if generation != self.reset_generation {
            self.reset_generation = generation;
            self.dismissed = false;
        }

        let trigger_active = context.is_some_and(|state| {
            state
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| context_band_triggers_hint(snapshot.band))
        });
        if !trigger_active {
            self.dismissed = false;
        }
    }

    pub fn dismiss(&mut self) {
        self.dismissed = true;
    }

    pub fn is_eligible(&self, context: Option<&SessionContextState>) -> bool {
        !self.dismissed
            && context.is_some_and(|state| {
                state
                    .snapshot
                    .as_ref()
                    .is_some_and(|snapshot| context_band_triggers_hint(snapshot.band))
            })
    }
}

/// Dismissal state keyed by AMF session ID. It is transient and intentionally
/// not part of the persisted project model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ContextHintStates {
    by_session: HashMap<String, ContextHintState>,
}

impl ContextHintStates {
    pub fn sync_session(&mut self, session_id: &str, context: Option<&SessionContextState>) {
        let Some(context) = context else {
            self.by_session.remove(session_id);
            return;
        };

        self.by_session
            .entry(session_id.to_string())
            .or_default()
            .sync(Some(context));
    }

    pub fn sync_all(&mut self, context_states: &HashMap<String, SessionContextState>) {
        self.by_session
            .retain(|session_id, _| context_states.contains_key(session_id));
        for (session_id, context) in context_states {
            self.sync_session(session_id, Some(context));
        }
    }

    pub fn dismiss(&mut self, session_id: &str, context: Option<&SessionContextState>) {
        let Some(context) = context else {
            return;
        };
        let state = self.by_session.entry(session_id.to_string()).or_default();
        state.sync(Some(context));
        state.dismiss();
    }

    pub fn is_eligible(&self, session_id: &str, context: Option<&SessionContextState>) -> bool {
        self.by_session
            .get(session_id)
            .is_some_and(|hint| hint.is_eligible(context))
    }

    #[cfg(test)]
    fn get(&self, session_id: &str) -> Option<&ContextHintState> {
        self.by_session.get(session_id)
    }
}

impl App {
    pub(crate) fn context_hint_is_visible_in_current_view(&self) -> bool {
        let session_id = match &self.mode {
            AppMode::Viewing(view) => self.context_session_id_for_view(view),
            _ => None,
        };
        let Some(session_id) = session_id else {
            return false;
        };
        self.context_hint_states
            .is_eligible(&session_id, self.context_states.get(&session_id))
    }

    /// Dismiss the hint for the agent session represented by the current view.
    /// The next context reset or cleared trigger re-arms it.
    pub(crate) fn dismiss_context_hint_from_view(&mut self) {
        let session_id = match &self.mode {
            AppMode::Viewing(view) => self.context_session_id_for_view(view),
            _ => None,
        };
        let Some(session_id) = session_id else {
            return;
        };
        self.context_hint_states
            .dismiss(&session_id, self.context_states.get(&session_id));
    }

    fn context_session_id_for_view(&self, view: &ViewState) -> Option<String> {
        self.store
            .projects
            .iter()
            .flat_map(|project| project.features.iter())
            .find(|feature| feature.tmux_session == view.session)
            .and_then(|feature| {
                feature
                    .sessions
                    .iter()
                    .find(|session| session.tmux_window == view.window)
                    .or_else(|| {
                        feature
                            .sessions
                            .iter()
                            .find(|session| session.kind == view.session_kind)
                    })
            })
            .map(|session| session.id.clone())
    }
}

pub(crate) const fn context_band_triggers_hint(band: ContextBand) -> bool {
    matches!(band, ContextBand::Warning | ContextBand::Critical)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context_tracking::{
        ContextProvenance, ContextResetMetadata, ContextThresholds, ContextUsageSample,
    };
    use chrono::{DateTime, Utc};

    fn timestamp(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }

    fn context(used_tokens: u64) -> SessionContextState {
        let now = timestamp(1_000);
        let mut state = SessionContextState::default();
        state
            .accept_sample(
                ContextUsageSample {
                    used_tokens,
                    context_limit: Some(100_000),
                    provenance: ContextProvenance::Direct,
                    sampled_at: now,
                    checked_at: now,
                    reset: ContextResetMetadata::default(),
                },
                ContextThresholds::default(),
            )
            .unwrap();
        state
    }

    #[test]
    fn only_warning_and_critical_snapshots_trigger_the_hint() {
        let mut hints = ContextHintStates::default();
        let normal = context(50_000);
        hints.sync_session("session", Some(&normal));
        assert!(!hints.is_eligible("session", Some(&normal)));

        let warning = context(70_000);
        hints.sync_session("session", Some(&warning));
        assert!(hints.is_eligible("session", Some(&warning)));

        let critical = context(85_000);
        hints.sync_session("session", Some(&critical));
        assert!(hints.is_eligible("session", Some(&critical)));
    }

    #[test]
    fn dismissal_is_scoped_to_the_session_and_current_generation() {
        let warning = context(70_000);
        let other_warning = context(85_000);
        let mut hints = ContextHintStates::default();
        hints.sync_session("first", Some(&warning));
        hints.sync_session("second", Some(&other_warning));

        hints.dismiss("first", Some(&warning));
        assert!(!hints.is_eligible("first", Some(&warning)));
        assert!(hints.is_eligible("second", Some(&other_warning)));

        let mut reset = warning.clone();
        reset.reset.generation = 1;
        hints.sync_session("first", Some(&reset));
        assert!(hints.is_eligible("first", Some(&reset)));
        assert_eq!(hints.get("first").unwrap().reset_generation, 1);
    }

    #[test]
    fn a_cleared_trigger_rearms_the_hint() {
        let warning = context(70_000);
        let normal = context(50_000);
        let mut hints = ContextHintStates::default();
        hints.sync_session("session", Some(&warning));
        hints.dismiss("session", Some(&warning));
        assert!(!hints.is_eligible("session", Some(&warning)));

        hints.sync_session("session", Some(&normal));
        assert!(!hints.is_eligible("session", Some(&normal)));
        hints.sync_session("session", Some(&warning));
        assert!(hints.is_eligible("session", Some(&warning)));
    }

    #[test]
    fn unavailable_or_reset_pending_context_never_becomes_a_false_zero_hint() {
        let warning = context(70_000);
        let mut hints = ContextHintStates::default();
        hints.sync_session("session", Some(&warning));
        hints.dismiss("session", Some(&warning));

        let mut reset_pending = warning.clone();
        reset_pending.snapshot = None;
        reset_pending.reset.generation = 1;
        reset_pending.awaiting_post_reset = true;
        hints.sync_session("session", Some(&reset_pending));
        assert!(!hints.is_eligible("session", Some(&reset_pending)));
        assert!(!hints.is_eligible("session", None));
    }

    #[test]
    fn stale_warning_remains_eligible_for_accurate_display() {
        let mut warning = context(70_000);
        warning.mark_unavailable(timestamp(1_005));
        let mut hints = ContextHintStates::default();
        hints.sync_session("session", Some(&warning));
        assert!(hints.is_eligible("session", Some(&warning)));
    }

    #[test]
    fn sync_all_drops_sessions_without_context_state() {
        let warning = context(70_000);
        let mut hints = ContextHintStates::default();
        hints.sync_session("gone", Some(&warning));
        assert!(hints.get("gone").is_some());

        let states = HashMap::new();
        hints.sync_all(&states);
        assert!(hints.get("gone").is_none());
    }
}
