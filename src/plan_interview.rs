//! Domain model and built-in question bank for plan-mode interviews.
//!
//! The UI state machine is intentionally kept out of this module so question
//! sources and AI response parsing can share these types without depending on
//! TUI state.

#![allow(dead_code)] // Introduced ahead of the Epic 1 UI integration.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanQuestionKind {
    FreeText,
    Select(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuestionSource {
    Builtin,
    GlobalTemplate,
    Template,
    Ai { round: usize },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanQuestion {
    /// Stable slug used to merge questions and associate persisted answers.
    pub id: String,
    pub text: String,
    pub kind: PlanQuestionKind,
    pub source: QuestionSource,
    pub optional: bool,
}

impl PlanQuestion {
    fn builtin(id: &str, text: &str) -> Self {
        Self {
            id: id.to_string(),
            text: text.to_string(),
            kind: PlanQuestionKind::FreeText,
            source: QuestionSource::Builtin,
            optional: true,
        }
    }
}

/// Return the curated questions asked after the required feature brief.
///
/// The order is part of the interview UX: it moves from product scope toward
/// implementation constraints and finishes with acceptance criteria.
pub fn builtin_questions() -> Vec<PlanQuestion> {
    vec![
        PlanQuestion::builtin(
            "scope",
            "What is in scope for this feature, and what is explicitly out of scope?",
        ),
        PlanQuestion::builtin(
            "users-entry-points",
            "Who will use this feature, and where will they enter the workflow?",
        ),
        PlanQuestion::builtin(
            "ui-surface",
            "What user interface or interaction changes should this feature introduce?",
        ),
        PlanQuestion::builtin(
            "data-persistence",
            "What data model or persistence changes does this feature require?",
        ),
        PlanQuestion::builtin(
            "external-integrations",
            "Which external systems, tools, or APIs must this feature integrate with?",
        ),
        PlanQuestion::builtin(
            "risks-unknowns",
            "What risks, constraints, or unknowns should the implementation account for?",
        ),
        PlanQuestion::builtin(
            "definition-of-done",
            "What must be true for this feature to be considered done?",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn builtin_bank_has_stable_order_and_unique_ids() {
        let questions = builtin_questions();
        let ids = questions
            .iter()
            .map(|question| question.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            [
                "scope",
                "users-entry-points",
                "ui-surface",
                "data-persistence",
                "external-integrations",
                "risks-unknowns",
                "definition-of-done",
            ]
        );
        assert_eq!(ids.iter().copied().collect::<HashSet<_>>().len(), ids.len());
    }

    #[test]
    fn builtin_questions_are_optional_free_text_questions() {
        let questions = builtin_questions();

        assert!(questions.iter().all(|question| question.optional));
        assert!(
            questions
                .iter()
                .all(|question| question.source == QuestionSource::Builtin)
        );
        assert!(
            questions
                .iter()
                .all(|question| question.kind == PlanQuestionKind::FreeText)
        );
        assert!(questions.iter().all(|question| !question.text.is_empty()));
    }

    #[test]
    fn question_model_round_trips_all_sources_and_kinds() {
        let questions = vec![
            PlanQuestion {
                id: "deployment-target".into(),
                text: "Where should this run?".into(),
                kind: PlanQuestionKind::Select(vec!["Local".into(), "Cloud".into()]),
                source: QuestionSource::GlobalTemplate,
                optional: false,
            },
            PlanQuestion {
                id: "ai-follow-up".into(),
                text: "How should retries behave?".into(),
                kind: PlanQuestionKind::FreeText,
                source: QuestionSource::Ai { round: 2 },
                optional: true,
            },
        ];

        let json = serde_json::to_string(&questions).unwrap();
        let decoded: Vec<PlanQuestion> = serde_json::from_str(&json).unwrap();

        assert_eq!(decoded, questions);
    }
}
