//! Desktop steering coach: the TUI's `analyze_prompt` checklist for a draft
//! startup prompt. The analysis is a pure local keyword/path scan; it never
//! calls a harness, reads the repository or touches the shared GUI handle,
//! so it needs no pre-call notice. The result is advisory and bound to the
//! exact text it describes by a digest, so the frontend can tell when the
//! displayed checklist no longer matches the draft being sent.

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::app::{PromptConstraint, analyze_prompt};

/// Drafts longer than this are refused rather than scanned; a prompt this
/// large is a pasted file, not a task statement.
pub const MAX_STEERING_TEXT_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SteeringCheckView {
    pub key: &'static str,
    pub label: &'static str,
    pub present: bool,
    /// "covered", or the TUI's explanation of what is missing.
    pub detail: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SteeringAnalysisView {
    /// Lowercase SHA-256 of the exact analyzed bytes (untrimmed).
    pub digest: String,
    pub score: u8,
    pub max_score: u8,
    pub summary: String,
    pub checks: Vec<SteeringCheckView>,
    /// Up to three teaching tips for the missing constraints (TUI wording).
    pub tips: Vec<String>,
    /// True when the draft is blank, so the checklist is guidance only.
    pub blank: bool,
}

/// Digest that identifies the exact text an analysis describes.
pub fn text_digest(text: &str) -> String {
    let hash = Sha256::digest(text.as_bytes());
    hash.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn constraint_key(constraint: PromptConstraint) -> &'static str {
    match constraint {
        PromptConstraint::FileScope => "file_scope",
        PromptConstraint::AcceptanceCriteria => "acceptance_criteria",
        PromptConstraint::Invariants => "invariants",
        PromptConstraint::ValidationCommands => "validation_commands",
        PromptConstraint::Risks => "risks",
    }
}

pub fn analyze(text: &str) -> Result<SteeringAnalysisView, String> {
    if text.len() > MAX_STEERING_TEXT_BYTES {
        return Err(format!(
            "The draft is {} KiB; the steering check only reads drafts up to {} KiB. Trim it or check a shorter statement of the task.",
            text.len().div_ceil(1024),
            MAX_STEERING_TEXT_BYTES / 1024
        ));
    }
    let analysis = analyze_prompt(text);
    Ok(SteeringAnalysisView {
        digest: text_digest(text),
        score: analysis.score,
        max_score: analysis.max_score,
        summary: analysis.summary,
        checks: analysis
            .checks
            .iter()
            .map(|check| SteeringCheckView {
                key: constraint_key(check.constraint),
                label: check.constraint.label(),
                present: check.present,
                detail: if check.present {
                    "covered"
                } else {
                    check.constraint.missing_explanation()
                },
            })
            .collect(),
        tips: analysis.teaching_tips,
        blank: text.trim().is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const STRONG: &str = "Touch only src/app/feature_ops.rs. Done when the dialog shows. \
        Do not change the create path. Run `cargo test`. Careful about tmux regressions.";

    #[test]
    fn matches_the_tui_analysis_and_wording() {
        let view = analyze(STRONG).unwrap();
        let tui = analyze_prompt(STRONG);
        assert_eq!((view.score, view.max_score), (tui.score, tui.max_score));
        assert_eq!(view.summary, tui.summary);
        assert_eq!(view.checks.len(), 5);
        assert!(view.checks.iter().all(|check| check.present));
        assert!(view.checks.iter().all(|check| check.detail == "covered"));
        assert!(view.tips.is_empty());
        assert!(!view.blank);
    }

    #[test]
    fn missing_constraints_carry_the_tui_explanations_and_tips() {
        let view = analyze("Make the thing better").unwrap();
        assert_eq!(view.score, 0);
        let scope = &view.checks[0];
        assert_eq!(
            (scope.key, scope.label, scope.present),
            ("file_scope", "File scope", false)
        );
        assert_eq!(
            scope.detail,
            "The prompt does not clearly name what files or directories are in scope."
        );
        assert_eq!(view.tips.len(), 3);
        assert!(view.tips[0].starts_with("Add a line like: `Touch only"));
    }

    #[test]
    fn blank_drafts_get_guidance_not_a_verdict() {
        let view = analyze("  \n").unwrap();
        assert!(view.blank);
        assert!(view.summary.starts_with("Start with the concrete task"));
    }

    #[test]
    fn digest_binds_the_exact_text() {
        let a = analyze("Fix the bug").unwrap();
        assert_eq!(a.digest, analyze("Fix the bug").unwrap().digest);
        // Any edit, even trailing whitespace the scan trims away, changes it.
        assert_ne!(a.digest, analyze("Fix the bug ").unwrap().digest);
        assert_ne!(a.digest, analyze("Fix the bug!").unwrap().digest);
        assert_eq!(a.digest.len(), 64);
        assert_eq!(
            text_digest(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn oversized_drafts_are_refused_with_a_reason() {
        let big = "a".repeat(MAX_STEERING_TEXT_BYTES + 1);
        let err = analyze(&big).unwrap_err();
        assert!(err.contains("up to 256 KiB"), "{err}");
        assert!(analyze(&"a".repeat(MAX_STEERING_TEXT_BYTES)).is_ok());
    }
}
