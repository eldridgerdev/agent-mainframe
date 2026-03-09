use crate::steering_config::{
    SteeringCoachConfig, SteeringConstraintConfig, SteeringDetectionMatcher,
    default_detection_for,
};

pub use crate::steering_config::SteeringConstraintKind as PromptConstraint;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCheck {
    pub constraint: PromptConstraint,
    pub label: String,
    pub missing_explanation: String,
    pub teaching_tip: String,
    pub present: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptAnalysis {
    pub score: u8,
    pub max_score: u8,
    pub summary: String,
    pub checks: Vec<PromptCheck>,
    pub teaching_tips: Vec<String>,
}

impl PromptAnalysis {
    pub fn present_checks(&self) -> impl Iterator<Item = &PromptCheck> {
        self.checks.iter().filter(|check| check.present)
    }

    pub fn missing_checks(&self) -> impl Iterator<Item = &PromptCheck> {
        self.checks.iter().filter(|check| !check.present)
    }
}

pub fn analyze_prompt(prompt: &str) -> PromptAnalysis {
    analyze_prompt_with_config(prompt, &SteeringCoachConfig::default())
}

pub fn analyze_prompt_with_config(prompt: &str, config: &SteeringCoachConfig) -> PromptAnalysis {
    let trimmed = prompt.trim();
    let lowercase = trimmed.to_lowercase();

    let checks = config
        .constraints
        .iter()
        .map(|constraint| PromptCheck {
            constraint: constraint.constraint,
            label: constraint.label.clone(),
            missing_explanation: constraint.missing_explanation.clone(),
            teaching_tip: constraint.teaching_tip.clone(),
            present: check_constraint(constraint, trimmed, &lowercase),
        })
        .collect::<Vec<_>>();

    let present_count = checks.iter().filter(|check| check.present).count();
    let missing = checks.iter().filter(|check| !check.present).count();
    let score = scaled_score(present_count);
    let max_score = scaled_score(checks.len());

    let summary = if checks.is_empty() {
        if trimmed.is_empty() {
            "No steering constraints are configured for this repo. Add some in .amf/steering.json or write the task directly."
                .to_string()
        } else {
            "No steering constraints are configured for this repo. Launch when the prompt wording is ready."
                .to_string()
        }
    } else if trimmed.is_empty() {
        "Start with the concrete task, then add scope, success criteria, validation, and watch-outs before launch."
            .to_string()
    } else if missing == 0 {
        "Strong steering draft. The prompt names the work, the boundaries, and how to validate it."
            .to_string()
    } else if missing <= 2 {
        "Usable draft. Add the missing constraints so the agent does less guessing once it starts."
            .to_string()
    } else {
        "Thin draft. Add missing constraints now so the first agent pass is less likely to wander or regress behavior."
            .to_string()
    };

    let teaching_tips = checks
        .iter()
        .filter(|check| !check.present)
        .map(|check| check.teaching_tip.clone())
        .take(3)
        .collect();

    PromptAnalysis {
        score,
        max_score,
        summary,
        checks,
        teaching_tips,
    }
}

fn scaled_score(count: usize) -> u8 {
    let scaled = count.saturating_mul(2);
    u8::try_from(scaled).unwrap_or(u8::MAX)
}

fn check_constraint(
    constraint: &SteeringConstraintConfig,
    prompt: &str,
    lowercase: &str,
) -> bool {
    let matcher = constraint
        .detection
        .as_ref()
        .cloned()
        .unwrap_or_else(|| default_detection_for(constraint.constraint));
    matches_detection(&matcher, prompt, lowercase)
}

fn matches_detection(matcher: &SteeringDetectionMatcher, prompt: &str, lowercase: &str) -> bool {
    match matcher {
        SteeringDetectionMatcher::Any { matchers } => matchers
            .iter()
            .any(|matcher| matches_detection(matcher, prompt, lowercase)),
        SteeringDetectionMatcher::All { matchers } => matchers
            .iter()
            .all(|matcher| matches_detection(matcher, prompt, lowercase)),
        SteeringDetectionMatcher::ContainsAny { phrases } => phrases
            .iter()
            .map(|phrase| phrase.to_lowercase())
            .any(|phrase| lowercase.contains(&phrase)),
        SteeringDetectionMatcher::TokenContainsAny { snippets } => prompt_tokens(prompt).any(|token| {
            snippets
                .iter()
                .map(|snippet| snippet.to_lowercase())
                .any(|snippet| token.contains(&snippet))
        }),
        SteeringDetectionMatcher::TokenStartsWithAny { prefixes } => prompt_tokens(prompt).any(|token| {
            prefixes
                .iter()
                .map(|prefix| prefix.to_lowercase())
                .any(|prefix| token.starts_with(&prefix))
        }),
        SteeringDetectionMatcher::TokenEndsWithAny { suffixes } => prompt_tokens(prompt).any(|token| {
            suffixes
                .iter()
                .map(|suffix| suffix.to_lowercase())
                .any(|suffix| token.ends_with(&suffix))
        }),
        SteeringDetectionMatcher::PromptContains { text } => lowercase.contains(&text.to_lowercase()),
    }
}

fn prompt_tokens(prompt: &str) -> impl Iterator<Item = String> + '_ {
    prompt.split_whitespace().map(|word| {
        word.trim_matches(|c: char| "`'\",:;()[]{}".contains(c))
            .to_lowercase()
    })
}
