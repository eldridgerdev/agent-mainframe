use serde::{Deserialize, Serialize};

use crate::project::AgentKind;

use super::HeadlessCommand;

/// How hard a headless run should think, as one vocabulary over four
/// harnesses that each spell it differently. Not every harness accepts every
/// level: [`ReasoningLevel::supported_for`] is the verified set, and
/// [`reasoning_args`] drops a level the harness cannot express rather than
/// passing a value its CLI would reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningLevel {
    Minimal,
    Low,
    Medium,
    High,
    #[serde(rename = "xhigh")]
    XHigh,
    Max,
}

impl ReasoningLevel {
    /// The levels a harness's headless CLI accepts, weakest first. Verified
    /// against `claude --help` (`--effort`: low, medium, high, xhigh, max),
    /// the Codex config key `model_reasoning_effort`, and `opencode run
    /// --help` (`--variant`: "e.g., high, max, minimal"). Pi has no verified
    /// flag, so it offers none.
    pub fn supported_for(harness: &AgentKind) -> &'static [ReasoningLevel] {
        use ReasoningLevel::*;
        match harness {
            AgentKind::Claude => &[Low, Medium, High, XHigh, Max],
            AgentKind::Codex => &[Low, Medium, High, XHigh],
            AgentKind::Opencode => &[Minimal, High, Max],
            AgentKind::Pi => &[],
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The model and reasoning level a headless run was asked for. Every entry
/// point takes `impl Into<ModelSel>`, so a caller with only a model (an
/// `Option<&str>`) is unchanged.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModelSel<'a> {
    pub model: Option<&'a str>,
    pub reasoning: Option<ReasoningLevel>,
}

impl<'a> ModelSel<'a> {
    pub fn new(model: Option<&'a str>, reasoning: Option<ReasoningLevel>) -> Self {
        Self { model, reasoning }
    }
}

impl<'a> From<Option<&'a str>> for ModelSel<'a> {
    fn from(model: Option<&'a str>) -> Self {
        Self {
            model,
            reasoning: None,
        }
    }
}

/// The CLI args that request `level` from `harness`, or none when the harness
/// cannot express it.
pub fn reasoning_args(harness: &AgentKind, level: ReasoningLevel) -> Vec<String> {
    if !ReasoningLevel::supported_for(harness).contains(&level) {
        return Vec::new();
    }
    let slug = level.slug();
    match harness {
        AgentKind::Claude => vec!["--effort".to_string(), slug.to_string()],
        AgentKind::Codex => vec![
            "-c".to_string(),
            format!("model_reasoning_effort=\"{slug}\""),
        ],
        AgentKind::Opencode => vec!["--variant".to_string(), slug.to_string()],
        AgentKind::Pi => Vec::new(),
    }
}

/// Harnesses whose headless CLI accepts `--model <name>`.
pub(super) fn supports_model_flag(harness: &AgentKind) -> bool {
    match harness {
        AgentKind::Claude | AgentKind::Codex | AgentKind::Opencode | AgentKind::Pi => true,
    }
}

/// `spec.args`, then an optional `--model <name>` (only for harnesses where
/// [`supports_model_flag`] holds), then `spec.trailing` — e.g. Codex's `-`
/// stdin marker must stay last.
pub(super) fn assemble_args(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    sel: ModelSel<'_>,
) -> Vec<String> {
    assemble_args_with(harness, spec, &[], sel)
}

/// [`assemble_args`] with runtime-built `extra` args (e.g. a user's MCP
/// config path) placed after `spec.args` and before `--model`/`trailing`.
pub(super) fn assemble_args_with(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    extra: &[String],
    sel: ModelSel<'_>,
) -> Vec<String> {
    let mut args: Vec<String> = spec.args.iter().map(|arg| arg.to_string()).collect();
    args.extend(extra.iter().cloned());
    if let Some(model) = sel.model
        && supports_model_flag(harness)
    {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    if let Some(level) = sel.reasoning {
        args.extend(reasoning_args(harness, level));
    }
    args.extend(spec.trailing.iter().map(|arg| arg.to_string()));
    args
}
