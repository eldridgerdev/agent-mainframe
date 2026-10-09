//! Attributable guidance, never fabricated measurements or cross-task scores.
use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

use crate::model_options::{EligibleOptions, ModelChoice};
use crate::project::AgentKind;
use crate::prompts::{PromptContext, render_template};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResearchNote {
    pub id: String,
    pub source: String,
    pub checked_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub statement: String,
}

/// Reviewed repository notes are the only evidence producer in v1. Updating
/// a date requires checking the source again, not reading or saving a row.
pub(crate) fn research_notes() -> Vec<ResearchNote> {
    vec![ResearchNote {
        id: "openai-reasoning-effort-2026-09-29".into(),
        source: "https://developers.openai.com/api/docs/guides/reasoning".into(),
        checked_at: "2026-09-29T00:00:00Z".parse().unwrap(),
        expires_at: "2026-10-29T00:00:00Z".parse().unwrap(),
        statement: "For the same reasoning model, lower effort favors speed and lower reasoning-token use; higher effort favors reasoning depth. This is general provider guidance, not a measurement of implementation quality or a comparison between models. Actual speed, tokens and quality for this plan are unknown.".into(),
    },ResearchNote {
        id:"openai-reasoning-model-selection-2026-09-29".into(),
        source:"https://developers.openai.com/api/docs/guides/model-selection".into(),
        checked_at:"2026-09-29T00:00:00Z".parse().unwrap(),
        expires_at:"2026-10-29T00:00:00Z".parse().unwrap(),
        statement:"OpenAI positions GPT-6 Luna for scoped tasks and efficiency, GPT-6 Sol for everyday coding and work requiring judgment, and GPT-6 Astra for ambiguous problems and demanding analysis. This is qualitative provider guidance; task fit remains a judgment and does not predict measured implementation quality, elapsed time or token totals for this plan.".into(),
    },ResearchNote {
        id:"openai-reasoning-model-selection-gpt-6.1-sol-2026-09-30".into(),
        source:"https://developers.openai.com/api/docs/models/gpt-6.1-sol".into(),
        checked_at:"2026-09-30T00:00:00Z".parse().unwrap(),
        expires_at:"2026-10-30T00:00:00Z".parse().unwrap(),
        statement:"OpenAI positions GPT-6.1 Sol for complex coding, computer use and professional work, and recommends comparing it with Astra on your own tasks. This is qualitative provider guidance; task fit remains a judgment and does not predict measured implementation quality, elapsed time or token totals for this plan.".into(),
    },ResearchNote {
        id:"reviewed-anthropic-effort-2026-09-30".into(),
        source:"https://platform.claude.com/docs/en/build-with-claude/effort".into(),
        checked_at:"2026-09-30T00:00:00Z".parse().unwrap(),
        expires_at:"2026-10-30T00:00:00Z".parse().unwrap(),
        statement:"For the same supported Claude model, lower effort favors speed and token efficiency; higher effort favors thoroughness and deeper reasoning. Effort affects thinking, tool calls and response text, and is a behavioral signal rather than a strict token budget. This provider guidance does not measure this task's quality, elapsed time or token totals; those remain unknown.".into(),
    },ResearchNote {
        id:"reviewed-anthropic-model-selection-2026-09-30".into(),
        source:"https://code.claude.com/docs/en/model-config".into(),
        checked_at:"2026-09-30T00:00:00Z".parse().unwrap(),
        expires_at:"2026-10-30T00:00:00Z".parse().unwrap(),
        statement:"Anthropic positions Sonnet for daily coding, Opus for complex reasoning, and Fable for the hardest and longest-running tasks. These reviewed roles apply to Sonnet 5.5, Opus 5.5, Fable 5.1 and Fable 5 when live Claude Code discovery verifies the exact version and its effort settings. Task fit is a qualified judgment; actual task quality, time and tokens remain unknown. Equal effort names are not calibrated equally across models.".into(),
    },ResearchNote {
        id:"reviewed-anthropic-effort-2026-10-09".into(),
        source:"https://platform.claude.com/docs/en/build-with-claude/effort".into(),
        checked_at:"2026-10-09T00:00:00Z".parse().unwrap(),
        expires_at:"2026-11-09T00:00:00Z".parse().unwrap(),
        statement:"For the same supported Claude model, lower effort favors speed and token efficiency; higher effort favors thoroughness and deeper reasoning. Effort affects thinking, tool calls and response text, and is a behavioral signal rather than a strict token budget. Haiku 5.5 supports effort; Anthropic suggests low for short, simple tasks and notes that at low effort in long agent prompts it is more likely to skip a search, stop early or skip a check. This provider guidance does not measure this task's quality, elapsed time or token totals; those remain unknown.".into(),
    },ResearchNote {
        id:"reviewed-anthropic-model-selection-2026-10-09".into(),
        source:"https://code.claude.com/docs/en/model-config".into(),
        checked_at:"2026-10-09T00:00:00Z".parse().unwrap(),
        expires_at:"2026-11-09T00:00:00Z".parse().unwrap(),
        statement:"Anthropic positions Haiku as fast and efficient for simple tasks, Sonnet for daily coding, Opus for complex reasoning, and Fable for the hardest and longest-running tasks. These reviewed roles apply to Haiku 5.5, Sonnet 5.5, Opus 5.5, Fable 5.1 and Fable 5 when live Claude Code discovery verifies the exact version and its effort settings. Task fit is a qualified judgment; actual task quality, time and tokens remain unknown. Equal effort names are not calibrated equally across models.".into(),
    }]
}

impl ResearchNote {
    pub fn validate_provenance(&self) -> Result<()> {
        ensure!(
            research_notes().contains(self),
            "unrecognized or altered research provenance"
        );
        Ok(())
    }

    fn is_model_guidance(&self) -> bool {
        matches!(
            self.id.as_str(),
            "openai-reasoning-model-selection-2026-09-29"
                | "openai-reasoning-model-selection-gpt-6.1-sol-2026-09-30"
                | "reviewed-anthropic-model-selection-2026-09-30"
                | "reviewed-anthropic-model-selection-2026-10-09"
        )
    }
    fn is_effort_guidance(&self) -> bool {
        matches!(
            self.id.as_str(),
            "openai-reasoning-effort-2026-09-29"
                | "reviewed-anthropic-effort-2026-09-30"
                | "reviewed-anthropic-effort-2026-10-09"
        )
    }
    pub fn applies(&self, choice: &ModelChoice, now: DateTime<Utc>) -> bool {
        let claude = *choice.harness() == AgentKind::Claude;
        let codex = *choice.harness() == AgentKind::Codex;
        self.validate_provenance().is_ok()
            && self.checked_at <= now && now < self.expires_at
            && match self.id.as_str() {
                "openai-reasoning-effort-2026-09-29" => codex,
                "openai-reasoning-model-selection-2026-09-29" => codex && matches!(choice.model(),"gpt-6-luna"|"gpt-6-sol"|"gpt-6-astra"),
                "openai-reasoning-model-selection-gpt-6.1-sol-2026-09-30" => codex && choice.model() == "gpt-6.1-sol",
                "reviewed-anthropic-effort-2026-09-30" => claude && matches!(choice.model(), "claude-sonnet-5-5" | "claude-sonnet-5" | "claude-sonnet-4-6" | "claude-opus-5-5" | "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" | "claude-fable-5-1" | "claude-fable-5"),
                "reviewed-anthropic-model-selection-2026-09-30" => claude && matches!(choice.model(), "claude-sonnet-5-5" | "claude-opus-5-5" | "claude-fable-5-1" | "claude-fable-5"),
                // These supersede the 2026-09-30 pair, which stays registered
                // so research persisted before this review still loads.
                "reviewed-anthropic-effort-2026-10-09" => claude && matches!(choice.model(), "claude-haiku-5-5" | "claude-sonnet-5-5" | "claude-sonnet-5" | "claude-sonnet-4-6" | "claude-opus-5-5" | "claude-opus-5" | "claude-opus-4-8" | "claude-opus-4-7" | "claude-opus-4-6" | "claude-fable-5-1" | "claude-fable-5"),
                "reviewed-anthropic-model-selection-2026-10-09" => claude && matches!(choice.model(), "claude-haiku-5-5" | "claude-sonnet-5-5" | "claude-opus-5-5" | "claude-fable-5-1" | "claude-fable-5"),
                _ => false,
            }
            // Deliberately bounded to documented conventional effort levels.
            // Do not infer performance of new levels from an ordinal name.
            && matches!(choice.reasoning(), Some("low" | "medium" | "high"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Priority {
    Speed,
    Balance,
    Depth,
}

impl Priority {
    pub fn label(self) -> &'static str {
        match self {
            Self::Speed => "Favor speed / fewer reasoning tokens",
            Self::Balance => "Balance speed and reasoning depth",
            Self::Depth => "Favor reasoning depth",
        }
    }
    fn level(self) -> &'static str {
        match self {
            Self::Speed => "low",
            Self::Balance => "medium",
            Self::Depth => "high",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Response {
    status: Status,
    choices: Vec<Proposal>,
}
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Status {
    Qualified,
    Insufficient,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    option_id: String,
    evidence_ids: Vec<String>,
    priority: Priority,
}

#[derive(Debug, Clone)]
pub(crate) struct Recommendation {
    pub choice: ModelChoice,
    pub evidence: Vec<ResearchNote>,
    pub priority: Priority,
}

/// Generated text cannot add measured values, source URLs, explanations or
/// persisted evidence. It only selects trusted IDs and a supported tradeoff.
pub(crate) fn validate_response(
    raw: &str,
    options: &EligibleOptions,
    notes: &[ResearchNote],
    now: DateTime<Utc>,
) -> Result<Vec<Recommendation>> {
    let response: Response = serde_json::from_str(raw)?;
    if response.status == Status::Insufficient {
        ensure!(
            response.choices.is_empty(),
            "insufficient response contains choices"
        );
        return Ok(vec![]);
    }
    ensure!(
        (1..=3).contains(&response.choices.len()),
        "expected one to three qualified choices"
    );
    let mut seen = BTreeSet::new();
    let mut recommendations: Vec<Recommendation> = vec![];
    for proposal in response.choices {
        let choice = options.select(&proposal.option_id)?.clone();
        ensure!(seen.insert(choice.id().to_string()), "duplicate choice");
        ensure!(
            choice.reasoning() == Some(proposal.priority.level()),
            "tradeoff does not match reasoning setting"
        );
        ensure!(
            !proposal.evidence_ids.is_empty(),
            "missing evidence reference"
        );
        let mut refs = BTreeSet::new();
        let mut evidence = vec![];
        for id in proposal.evidence_ids {
            ensure!(refs.insert(id.clone()), "duplicate evidence reference");
            let note = notes
                .iter()
                .find(|n| n.id == id)
                .ok_or_else(|| anyhow::anyhow!("unknown evidence reference"))?;
            ensure!(
                note.applies(&choice, now),
                "irrelevant, stale or altered evidence"
            );
            evidence.push(note.clone());
        }
        ensure!(
            evidence.iter().any(ResearchNote::is_effort_guidance),
            "reasoning tradeoff requires effort evidence"
        );
        for previous in &recommendations {
            if previous.choice.model() != choice.model()
                || previous.choice.harness() != choice.harness()
            {
                ensure!(
                    previous
                        .evidence
                        .iter()
                        .any(ResearchNote::is_model_guidance)
                        && evidence.iter().any(ResearchNote::is_model_guidance),
                    "evidence does not support a comparison between models"
                );
            }
        }
        recommendations.push(Recommendation {
            choice,
            evidence,
            priority: proposal.priority,
        });
    }
    Ok(recommendations)
}

pub(crate) fn prompt_context(
    plan: &str,
    options: &EligibleOptions,
    notes: &[ResearchNote],
    now: DateTime<Utc>,
) -> Result<PromptContext> {
    prompt_context_for_task(plan, "implementation", options, notes, now)
}

pub(crate) fn prompt_context_for_task(
    task_context: &str,
    task_phase: &str,
    options: &EligibleOptions,
    notes: &[ResearchNote],
    now: DateTime<Utc>,
) -> Result<PromptContext> {
    ensure!(!task_context.trim().is_empty(), "task context is missing");
    ensure!(!task_phase.trim().is_empty(), "task phase is missing");
    let choices: Vec<_> = options.choices().iter().filter_map(|c| {
        let evidence_ids: Vec<_> = notes.iter().filter(|n| n.applies(c, now)).map(|n| &n.id).collect();
        (!evidence_ids.is_empty()).then(|| serde_json::json!({"option_id":c.id(),"harness":c.harness().slug(),"model":c.model(),"reasoning":c.reasoning(),"evidence_ids":evidence_ids}))
    }).collect();
    let evidence: Vec<_> = notes
        .iter()
        .filter(|n| options.choices().iter().any(|c| n.applies(c, now)))
        .collect();
    Ok(PromptContext::new()
        .with("task_phase", task_phase)
        .with("task_context", task_context)
        .with("eligible_options", serde_json::to_string(&choices)?)
        .with("evidence", serde_json::to_string(&evidence)?))
}

/// Other prompts intentionally permit unresolved tokens. This workflow must
/// check the effective override before rendering; plan text may contain braces.
pub(crate) fn render_analysis_prompt(template: &str, ctx: &PromptContext) -> Result<String> {
    let mut tokens = BTreeSet::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        rest = &rest[start + 2..];
        let end = rest
            .find("}}")
            .ok_or_else(|| anyhow::anyhow!("unclosed analyzer placeholder"))?;
        let token = rest[..end].trim();
        ensure!(ctx.get(token).is_some(), "unresolved analyzer placeholder");
        tokens.insert(token);
        rest = &rest[end + 2..];
    }
    for required in ["task_phase", "task_context", "eligible_options", "evidence"] {
        ensure!(
            ctx.get(required).is_some_and(|s| !s.trim().is_empty()),
            "missing analyzer context: {required}"
        );
        ensure!(
            tokens.contains(required),
            "analyzer template omits required context: {required}"
        );
    }
    Ok(render_template(template, ctx))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_options::{Availability, HarnessCapability, LaunchPath, ModelCapability};
    pub(super) fn options() -> EligibleOptions {
        EligibleOptions::new(
            &[AgentKind::Codex],
            &[HarnessCapability {
                harness: AgentKind::Codex,
                availability: Availability::Available,
                model_flag: true,
                reasoning_flag: true,
                models: vec![ModelCapability {
                    model: "test-model".into(),
                    availability: Availability::Available,
                    reasoning_levels: Some(vec!["low".into(), "high".into()]),
                }],
            }],
            LaunchPath::Interactive,
        )
    }
    fn response(options: &EligibleOptions) -> String {
        let c = options
            .choices()
            .iter()
            .find(|c| c.reasoning() == Some("low"))
            .unwrap();
        serde_json::json!({"status":"qualified","choices":[{"option_id":c.id(),"evidence_ids":[research_notes()[0].id],"priority":"speed"}]}).to_string()
    }
    fn now() -> DateTime<Utc> {
        "2026-09-29T12:00:00Z".parse().unwrap()
    }
    #[test]
    fn qualified_guidance_has_no_measurement_fields() {
        let options = options();
        let raw = response(&options);
        assert_eq!(
            validate_response(&raw, &options, &research_notes(), now())
                .unwrap()
                .len(),
            1
        );
        assert!(
            validate_response(
                &raw.replace("\"priority\":", "\"tokens\":10,\"priority\":"),
                &options,
                &research_notes(),
                now()
            )
            .is_err()
        );
    }
    #[test]
    fn invented_options_citations_and_stale_notes_fail() {
        let options = options();
        let raw = response(&options);
        assert!(
            validate_response(
                &raw.replace(
                    options
                        .choices()
                        .iter()
                        .find(|c| c.reasoning() == Some("low"))
                        .unwrap()
                        .id(),
                    "invented"
                ),
                &options,
                &research_notes(),
                now()
            )
            .is_err()
        );
        assert!(
            validate_response(
                &raw.replace(&research_notes()[0].id, "invented"),
                &options,
                &research_notes(),
                now()
            )
            .is_err()
        );
        assert!(
            validate_response(
                &raw,
                &options,
                &research_notes(),
                "2026-10-29T00:00:00Z".parse().unwrap()
            )
            .is_err()
        );
        let mut altered = research_notes();
        altered[0].statement = "This model guarantees success".into();
        assert!(validate_response(&raw, &options, &altered, now()).is_err());
    }
    #[test]
    fn malformed_and_insufficient_are_distinct() {
        let options = options();
        assert!(validate_response("oops", &options, &research_notes(), now()).is_err());
        assert!(
            validate_response(
                r#"{"status":"insufficient","choices":[]}"#,
                &options,
                &[],
                now()
            )
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn model_alternatives_require_matching_model_and_effort_sources() {
        let options = EligibleOptions::new(
            &[AgentKind::Codex],
            &[HarnessCapability {
                harness: AgentKind::Codex,
                availability: Availability::Available,
                model_flag: true,
                reasoning_flag: true,
                models: ["gpt-6-luna", "gpt-6-astra", "unresearched"]
                    .into_iter()
                    .map(|model| ModelCapability {
                        model: model.into(),
                        availability: Availability::Available,
                        reasoning_levels: Some(vec!["low".into()]),
                    })
                    .collect(),
            }],
            LaunchPath::Interactive,
        );
        let proposal = |model: &str, refs: Vec<String>| serde_json::json!({"option_id":options.choices().iter().find(|c|c.model()==model && c.reasoning()==Some("low")).unwrap().id(),"priority":"speed","evidence_ids":refs});
        let notes = research_notes();
        let both = notes[..2].iter().map(|n| n.id.clone()).collect::<Vec<_>>();
        let raw=serde_json::json!({"status":"qualified","choices":[proposal("gpt-6-luna",both.clone()),proposal("gpt-6-astra",both.clone())]}).to_string();
        assert_eq!(
            validate_response(&raw, &options, &notes, now())
                .unwrap()
                .len(),
            2
        );
        let unsupported=serde_json::json!({"status":"qualified","choices":[proposal("gpt-6-luna",vec![notes[0].id.clone()]),proposal("gpt-6-astra",vec![notes[0].id.clone()])]}).to_string();
        assert!(validate_response(&unsupported, &options, &notes, now()).is_err());
        let irrelevant =
            serde_json::json!({"status":"qualified","choices":[proposal("unresearched",both)]})
                .to_string();
        assert!(validate_response(&irrelevant, &options, &notes, now()).is_err());
        let duplicate=serde_json::json!({"status":"qualified","choices":[proposal("gpt-6-luna",vec![notes[0].id.clone()]),proposal("gpt-6-luna",vec![notes[0].id.clone()])]}).to_string();
        assert!(validate_response(&duplicate, &options, &notes, now()).is_err());
    }
    #[test]
    fn new_sol_comparisons_require_version_specific_fresh_guidance_and_effort() {
        let options = EligibleOptions::new(
            &[AgentKind::Codex],
            &[HarnessCapability {
                harness: AgentKind::Codex,
                availability: Availability::Available,
                model_flag: true,
                reasoning_flag: true,
                models: ["gpt-6.1-sol", "gpt-6-astra", "gpt-6.2-sol"]
                    .into_iter()
                    .map(|model| ModelCapability {
                        model: model.into(),
                        availability: Availability::Available,
                        reasoning_levels: Some(vec!["medium".into()]),
                    })
                    .collect(),
            }],
            LaunchPath::Interactive,
        );
        let notes = research_notes();
        let checked = "2026-09-30T12:00:00Z".parse().unwrap();
        let proposal = |model: &str, refs: &[usize]| {
            serde_json::json!({
                "option_id":options.choices().iter().find(|c|c.model()==model && c.reasoning()==Some("medium")).unwrap().id(),
                "priority":"balance",
                "evidence_ids":refs.iter().map(|i| &notes[*i].id).collect::<Vec<_>>()
            })
        };
        let response = |model: &str, refs: &[usize]| {
            serde_json::json!({"status":"qualified","choices":[
                proposal(model, refs), proposal("gpt-6-astra", &[0,1])
            ]})
            .to_string()
        };
        let raw = response("gpt-6.1-sol", &[0, 2]);
        let recommendations = validate_response(&raw, &options, &notes, checked).unwrap();
        assert_eq!(recommendations.len(), 2);
        assert_eq!(recommendations[0].choice.model(), "gpt-6.1-sol");
        // Old Sol guidance cannot establish the new version's task role.
        assert!(
            validate_response(&response("gpt-6.1-sol", &[0, 1]), &options, &notes, checked)
                .is_err()
        );
        // Model guidance alone must not be mistaken for effort evidence.
        assert!(
            validate_response(&response("gpt-6.1-sol", &[2]), &options, &notes, checked).is_err()
        );
        assert!(
            validate_response(&response("gpt-6.2-sol", &[0, 2]), &options, &notes, checked)
                .is_err()
        );
        assert!(validate_response(&raw, &options, &notes, now()).is_err());
        let new_sol = &recommendations[0].choice;
        assert!(!notes[2].applies(new_sol, notes[2].expires_at));
        assert!(notes[2].applies(new_sol, notes[2].checked_at));

        let ctx = prompt_context("reviewed task", &options, &notes, checked).unwrap();
        let prompt_options: serde_json::Value =
            serde_json::from_str(ctx.get("eligible_options").unwrap()).unwrap();
        let new_option = prompt_options
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["option_id"] == new_sol.id())
            .unwrap();
        assert_eq!(
            new_option["evidence_ids"],
            serde_json::json!([notes[0].id, notes[2].id])
        );
        let prompt = render_analysis_prompt(
            crate::prompts::PromptId::ModelAnalysis
                .spec()
                .default_template,
            &ctx,
        )
        .unwrap();
        assert!(prompt.contains(new_sol.id()));
        assert!(prompt.contains(&notes[2].source));
        // Research never supplies eligibility if live discovery removes it.
        assert!(
            validate_response(
                &raw,
                &EligibleOptions::new(&[], &[], LaunchPath::Interactive),
                &notes,
                checked
            )
            .is_err()
        );
    }
    #[test]
    fn claude_requires_its_own_fresh_version_scoped_effort_and_model_sources() {
        let caps = [AgentKind::Claude, AgentKind::Codex].map(|harness| HarnessCapability {
            harness,
            availability: Availability::Available,
            model_flag: true,
            reasoning_flag: true,
            models: [
                "claude-sonnet-5-5",
                "claude-opus-5-5",
                "claude-sonnet-6",
                "sonnet",
            ]
            .into_iter()
            .map(|model| ModelCapability {
                model: model.into(),
                availability: Availability::Available,
                reasoning_levels: Some(vec![
                    "low".into(),
                    "medium".into(),
                    "high".into(),
                    "xhigh".into(),
                ]),
            })
            .collect(),
        });
        let options = EligibleOptions::new(
            &[AgentKind::Claude, AgentKind::Codex],
            &caps,
            LaunchPath::Interactive,
        );
        let notes = research_notes();
        let checked = "2026-09-30T12:00:00Z".parse().unwrap();
        let proposal = |harness: AgentKind, model, level, priority, refs: &[usize]| {
            let choice = options
                .choices()
                .iter()
                .find(|c| {
                    *c.harness() == harness && c.model() == model && c.reasoning() == Some(level)
                })
                .unwrap();
            serde_json::json!({"option_id":choice.id(),"priority":priority,"evidence_ids":refs.iter().map(|i| &notes[*i].id).collect::<Vec<_>>()})
        };
        let valid = serde_json::json!({"status":"qualified","choices":[
            proposal(AgentKind::Claude,"claude-sonnet-5-5","medium","balance",&[3,4]),
            proposal(AgentKind::Claude,"claude-opus-5-5","high","depth",&[3,4]),
        ]})
        .to_string();
        assert_eq!(
            validate_response(&valid, &options, &notes, checked)
                .unwrap()
                .len(),
            2
        );
        for choice in [
            proposal(
                AgentKind::Claude,
                "claude-sonnet-5-5",
                "medium",
                "balance",
                &[0],
            ),
            proposal(
                AgentKind::Claude,
                "claude-sonnet-5-5",
                "medium",
                "balance",
                &[4],
            ),
            proposal(
                AgentKind::Codex,
                "claude-sonnet-5-5",
                "medium",
                "balance",
                &[3, 4],
            ),
            proposal(
                AgentKind::Claude,
                "claude-sonnet-6",
                "medium",
                "balance",
                &[3, 4],
            ),
            proposal(AgentKind::Claude, "sonnet", "medium", "balance", &[3, 4]),
            proposal(
                AgentKind::Claude,
                "claude-sonnet-5-5",
                "xhigh",
                "depth",
                &[3, 4],
            ),
        ] {
            let raw = serde_json::json!({"status":"qualified","choices":[choice]}).to_string();
            assert!(validate_response(&raw, &options, &notes, checked).is_err());
        }
        assert!(validate_response(&valid, &options, &notes, now()).is_err());
        assert!(validate_response(&valid, &options, &notes, notes[3].expires_at).is_err());
        assert!(
            validate_response(
                &valid.replace(&notes[4].id, &notes[3].id),
                &options,
                &notes,
                checked
            )
            .is_err()
        );
        let mut altered = notes.clone();
        altered[3].statement = "Guaranteed faster".into();
        assert!(validate_response(&valid, &options, &altered, checked).is_err());
    }
    #[test]
    fn haiku_qualifies_only_from_the_review_that_covers_it() {
        let caps = [HarnessCapability {
            harness: AgentKind::Claude,
            availability: Availability::Available,
            model_flag: true,
            reasoning_flag: true,
            models: ["claude-haiku-5-5", "claude-sonnet-5-5", "claude-haiku-4-5"]
                .into_iter()
                .map(|model| ModelCapability {
                    model: model.into(),
                    availability: Availability::Available,
                    reasoning_levels: Some(vec!["low".into(), "medium".into()]),
                })
                .collect(),
        }];
        let options = EligibleOptions::new(&[AgentKind::Claude], &caps, LaunchPath::Interactive);
        let notes = research_notes();
        let id = |id: &str| notes.iter().position(|n| n.id == id).unwrap();
        let (old_effort, old_model) = (
            id("reviewed-anthropic-effort-2026-09-30"),
            id("reviewed-anthropic-model-selection-2026-09-30"),
        );
        let (effort, model) = (
            id("reviewed-anthropic-effort-2026-10-09"),
            id("reviewed-anthropic-model-selection-2026-10-09"),
        );
        let checked = "2026-10-09T12:00:00Z".parse().unwrap();
        let proposal = |model: &str, level, priority, refs: &[usize]| {
            let choice = options
                .choices()
                .iter()
                .find(|c| c.model() == model && c.reasoning() == Some(level))
                .unwrap();
            serde_json::json!({"option_id":choice.id(),"priority":priority,"evidence_ids":refs.iter().map(|i| &notes[*i].id).collect::<Vec<_>>()})
        };
        let respond = |choices: Vec<serde_json::Value>| {
            serde_json::json!({"status":"qualified","choices":choices}).to_string()
        };

        let valid = respond(vec![
            proposal("claude-haiku-5-5", "low", "speed", &[effort, model]),
            proposal(
                "claude-sonnet-5-5",
                "medium",
                "balance",
                &[old_effort, old_model],
            ),
        ]);
        let recommendations = validate_response(&valid, &options, &notes, checked).unwrap();
        assert_eq!(recommendations[0].choice.model(), "claude-haiku-5-5");

        for refs in [
            // The earlier review did not cover Haiku.
            &[old_effort, old_model][..],
            &[effort, old_model][..],
            // Model guidance alone is not effort evidence.
            &[model][..],
        ] {
            let raw = respond(vec![proposal("claude-haiku-5-5", "low", "speed", refs)]);
            assert!(validate_response(&raw, &options, &notes, checked).is_err());
        }
        // Haiku 4.5 has no documented effort control.
        let older = respond(vec![proposal(
            "claude-haiku-4-5",
            "low",
            "speed",
            &[effort, model],
        )]);
        assert!(validate_response(&older, &options, &notes, checked).is_err());
        // The new review is not retroactive and expires like the others.
        assert!(validate_response(&valid, &options, &notes, notes[old_effort].checked_at).is_err());
        assert!(validate_response(&valid, &options, &notes, notes[effort].expires_at).is_err());

        let ctx = prompt_context("reviewed task", &options, &notes, checked).unwrap();
        let prompt_options: serde_json::Value =
            serde_json::from_str(ctx.get("eligible_options").unwrap()).unwrap();
        let haiku = prompt_options
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["model"] == "claude-haiku-5-5" && v["reasoning"] == "low")
            .unwrap();
        assert_eq!(
            haiku["evidence_ids"],
            serde_json::json!([notes[effort].id, notes[model].id])
        );
        assert!(
            prompt_options
                .as_array()
                .unwrap()
                .iter()
                .all(|v| v["model"] != "claude-haiku-4-5")
        );
    }
    #[test]
    fn missing_context_and_unknown_override_placeholders_fail() {
        let options = options();
        let ctx = prompt_context("plan {{literal}}", &options, &research_notes(), now()).unwrap();
        let template = crate::prompts::PromptId::ModelAnalysis
            .spec()
            .default_template;
        assert!(
            render_analysis_prompt(template, &ctx)
                .unwrap()
                .contains("plan {{literal}}")
        );
        assert!(render_analysis_prompt(template, &PromptContext::new()).is_err());
        assert!(render_analysis_prompt("no context", &ctx).is_err());
        assert!(render_analysis_prompt(&format!("{template} {{{{invented}}}}"), &ctx).is_err());
    }
}
