//! What "needs attention" means over Remote Control: exactly the desk's `i`
//! overlay (`App::attention_rows`), folded to one entry per feature.
//!
//! The overlay merges two signals — attention records (why a session
//! stopped) and pending inputs (a Claude Stop hook's wait, diff reviews,
//! review-ready prompts, …). A phone that only read attention records would
//! miss a Claude session finishing its turn, which arrives as a pending
//! input; deriving from the overlay's own rows keeps the phone and the desk
//! from ever disagreeing about what is waiting.

use std::collections::HashMap;

use super::App;
use super::attention::{AttentionRow, AttentionState};

/// One feature's reason to be looked at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteAttention {
    /// Short label: "Question", "Waiting", "Diff review", …
    pub reason: String,
    /// Finishes "<feature> …" in a notification title.
    pub phrase: &'static str,
    /// The agent's own words, when the signal carried any.
    pub detail: Option<String>,
    /// Changes whenever this is new news: a push goes out when a feature's
    /// fingerprint differs from the one last announced.
    pub fingerprint: String,
}

fn attention_phrase(state: AttentionState) -> &'static str {
    match state {
        AttentionState::Question => "is asking you a question",
        AttentionState::CompletedAwaitingReview => "finished and is ready for review",
        AttentionState::Waiting => "is waiting for you",
    }
}

/// Label and phrase for a pending input the attention layer didn't explain.
fn pending_reason(notification_type: &str) -> (&'static str, &'static str) {
    match notification_type {
        "stop" | "input-request" => ("Waiting", "is waiting for you"),
        "diff-review" | "change-reason" => ("Diff review", "wants a change reviewed"),
        "review-ready" => ("Fixes ready", "has review fixes ready"),
        _ => ("Input request", "needs your input"),
    }
}

impl App {
    fn feature_id_by_names(&self, project: &str, feature: &str) -> Option<String> {
        self.store
            .find_project(project)?
            .features
            .iter()
            .find(|f| f.name == feature)
            .map(|f| f.id.clone())
    }

    /// Feature id → why it needs attention, for every feature the `i`
    /// overlay would list. The first row for a feature wins, and attention
    /// rows come first, so a feature both asking a question and holding a
    /// review prompt reads as the question.
    pub(crate) fn remote_attention_by_feature(&self) -> HashMap<String, RemoteAttention> {
        let mut out = HashMap::new();
        for row in self.attention_rows() {
            let (project, feature, attention) = match row {
                AttentionRow::Attention { entry, pending } => {
                    let detail = pending
                        .and_then(|index| self.pending_inputs.get(index))
                        .map(|input| input.message.trim().to_string())
                        .filter(|message| !message.is_empty());
                    (
                        entry.project_name,
                        entry.feature_name,
                        RemoteAttention {
                            reason: entry.record.state.label().to_string(),
                            phrase: attention_phrase(entry.record.state),
                            detail,
                            fingerprint: format!("attention:{}", entry.record.since.to_rfc3339()),
                        },
                    )
                }
                AttentionRow::Pending(index) => {
                    let Some(input) = self.pending_inputs.get(index) else {
                        continue;
                    };
                    let (Some(project), Some(feature)) =
                        (input.project_name.clone(), input.feature_name.clone())
                    else {
                        continue;
                    };
                    let (reason, phrase) = pending_reason(&input.notification_type);
                    let message = input.message.trim().to_string();
                    (
                        project,
                        feature,
                        RemoteAttention {
                            reason: reason.to_string(),
                            phrase,
                            fingerprint: format!(
                                "pending:{}:{}:{}",
                                input.notification_type,
                                message,
                                input.change_id.as_deref().unwrap_or_default()
                            ),
                            detail: Some(message).filter(|m| !m.is_empty()),
                        },
                    )
                }
            };
            if let Some(feature_id) = self.feature_id_by_names(&project, &feature) {
                out.entry(feature_id).or_insert(attention);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::PendingInput;
    use crate::app::attention::AttentionRecord;
    use crate::app::remote_server::tests::test_app_with_feature_and_db;
    use crate::project::AgentKind;

    fn claude_stop() -> PendingInput {
        PendingInput {
            session_id: "amf-my-feature".into(),
            notification_type: "stop".into(),
            message: "Claude is waiting for your input".into(),
            project_name: Some("my-project".into()),
            feature_name: Some("my-feature".into()),
            ..Default::default()
        }
    }

    #[test]
    fn a_claude_stop_hook_alone_needs_attention() {
        let (_db, mut app) = test_app_with_feature_and_db();
        app.pending_inputs.push(claude_stop());
        let id = app.store.projects[0].features[0].id.clone();

        let attention = app.remote_attention_by_feature();

        let entry = &attention[&id];
        assert_eq!(entry.reason, "Waiting");
        assert_eq!(
            entry.detail.as_deref(),
            Some("Claude is waiting for your input")
        );
    }

    #[test]
    fn an_attention_record_wins_and_borrows_the_waits_message() {
        let (_db, mut app) = test_app_with_feature_and_db();
        app.pending_inputs.push(claude_stop());
        app.attention.insert(
            "amf-my-feature".into(),
            AttentionRecord::new(
                AgentKind::Claude,
                AttentionState::Question,
                chrono::Utc::now(),
            ),
        );
        let id = app.store.projects[0].features[0].id.clone();

        let entry = &app.remote_attention_by_feature()[&id];

        assert_eq!(entry.reason, "Question");
        assert_eq!(
            entry.detail.as_deref(),
            Some("Claude is waiting for your input")
        );
        assert!(entry.fingerprint.starts_with("attention:"));
    }
}
