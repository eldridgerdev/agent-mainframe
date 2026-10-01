//! Model/reasoning eligibility, independent of App and generated advice.
//!
//! Inventory: docs/development/model-reasoning-analyzer.md. Discovery must
//! supply effective, project-specific capabilities, not a guessed model list.
//! Cached names, configured defaults and a successful `--version` alone do not
//! establish model access. Rebuild this set immediately before applying a pick.
//!
//! The analyzer uses this boundary; existing manual pickers stay separate.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use sha2::{Digest, Sha256};

use crate::project::AgentKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Availability {
    Available,
    #[allow(dead_code)] // Used by adapters with explicit negative access checks.
    Unavailable,
    Unknown,
}

/// Effective access and reasoning controls for one exact model identifier.
/// For provider-based harnesses the identifier must include its provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelCapability {
    pub model: String,
    pub availability: Availability,
    /// `None` means unknown; an empty list means no explicit reasoning control.
    /// Both permit only an unspecified reasoning setting. Do not infer levels
    /// from the model's name or another model with a similar name.
    pub reasoning_levels: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HarnessCapability {
    pub harness: AgentKind,
    pub availability: Availability,
    /// Installed CLI flags, after any effective policy restrictions. Model
    /// access must be established independently on each ModelCapability.
    pub model_flag: bool,
    pub reasoning_flag: bool,
    pub models: Vec<ModelCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchPath {
    Interactive,
    #[allow(dead_code)] // Explicit reasoning has no headless launch seam yet.
    Headless,
}

impl LaunchPath {
    /// What AMF can actually carry to execution today. CLI support alone must
    /// not make a choice applicable when the AMF launcher drops its settings.
    fn supports_model(self, harness: &AgentKind) -> bool {
        match self {
            Self::Interactive => matches!(harness, AgentKind::Claude | AgentKind::Codex),
            Self::Headless => true,
        }
    }

    fn supports_reasoning(self, harness: &AgentKind) -> bool {
        match self {
            Self::Interactive => matches!(harness, AgentKind::Claude | AgentKind::Codex),
            // HeadlessRunner currently accepts a model but no reasoning arg.
            Self::Headless => false,
        }
    }
}

/// Only EligibleOptions constructs a choice. A generated response may name
/// its id; it cannot create a new applicable harness/model/reasoning tuple.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelChoice {
    id: String,
    harness: AgentKind,
    model: String,
    reasoning: Option<String>,
}

impl ModelChoice {
    fn new(harness: &AgentKind, model: &str, reasoning: Option<&str>) -> Self {
        // JSON tuples distinguish embedded delimiters and None from literals.
        let identity = serde_json::to_vec(&(harness.slug(), model, reasoning))
            .expect("string tuple is serializable");
        Self {
            id: format!("option-{:x}", Sha256::digest(identity)),
            harness: harness.clone(),
            model: model.to_string(),
            reasoning: reasoning.map(str::to_string),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn harness(&self) -> &AgentKind {
        &self.harness
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// None means leave reasoning to the harness; its effective value is
    /// unknown, and must not be presented as a measured or explicit level.
    pub fn reasoning(&self) -> Option<&str> {
        self.reasoning.as_deref()
    }
}

#[derive(Debug)]
pub(crate) struct EligibleOptions {
    path: LaunchPath,
    choices: Vec<ModelChoice>,
}

impl EligibleOptions {
    /// `allowed` must already intersect workspace selections and effective
    /// project configuration (App::allowed_agents_for_repo). Empty means none,
    /// not "all": that method has already expanded the config's defaults.
    pub fn new(
        allowed: &[AgentKind],
        capabilities: &[HarnessCapability],
        path: LaunchPath,
    ) -> Self {
        let mut choices = BTreeMap::new();
        for harness in AgentKind::ALL {
            if !allowed.contains(&harness) || !path.supports_model(&harness) {
                continue;
            }
            let mut reports = capabilities.iter().filter(|cap| cap.harness == harness);
            let Some(cap) = reports.next() else {
                continue;
            };
            // Multiple reports with different effective capabilities are
            // ambiguous. Do not merge them into an invented union of access.
            if reports.any(|other| other != cap)
                || cap.availability != Availability::Available
                || !cap.model_flag
            {
                continue;
            }
            for model in &cap.models {
                if model.availability != Availability::Available
                    || !valid_model_identifier(&harness, &model.model)
                    || cap
                        .models
                        .iter()
                        .any(|other| other.model == model.model && other != model)
                {
                    continue;
                }
                let default = ModelChoice::new(&harness, &model.model, None);
                choices.insert(default.id.clone(), default);
                if cap.reasoning_flag && path.supports_reasoning(&harness) {
                    for level in model.reasoning_levels.iter().flatten() {
                        if valid_level(level) {
                            let choice = ModelChoice::new(&harness, &model.model, Some(level));
                            choices.insert(choice.id.clone(), choice);
                        }
                    }
                }
            }
        }
        let mut choices: Vec<_> = choices.into_values().collect();
        choices.sort_by(|a, b| {
            (a.harness.slug(), &a.model, &a.reasoning).cmp(&(
                b.harness.slug(),
                &b.model,
                &b.reasoning,
            ))
        });
        Self { path, choices }
    }

    pub fn choices(&self) -> &[ModelChoice] {
        &self.choices
    }

    pub fn select(&self, id: &str) -> Result<&ModelChoice> {
        self.choices
            .iter()
            .find(|choice| choice.id == id)
            .ok_or_else(|| anyhow::anyhow!("Model option is unavailable or unsupported"))
    }

    /// Call on a freshly discovered set before changing settings or starting
    /// a process. A stale UI selection never authorizes a stale launch.
    pub fn revalidate(&self, selected: &ModelChoice) -> Result<&ModelChoice> {
        let current = self.select(selected.id())?;
        anyhow::ensure!(current == selected, "Model option identity changed");
        Ok(current)
    }

    /// Arguments for the existing Claude/Codex interactive extra_args seam.
    /// The caller must build this set from refreshed capabilities first, then
    /// append these arguments without changing existing permission/hook flags.
    /// No shell command is constructed here; TmuxManager quotes each argument.
    pub fn interactive_args(&self, selected: &ModelChoice) -> Result<Vec<String>> {
        anyhow::ensure!(
            self.path == LaunchPath::Interactive,
            "Not an interactive model selection"
        );
        let current = self.revalidate(selected)?;
        let mut args = vec!["--model".into(), current.model.clone()];
        if let Some(level) = current.reasoning() {
            match current.harness {
                AgentKind::Claude => args.extend(["--effort".into(), level.into()]),
                AgentKind::Codex => args.extend([
                    "-c".into(),
                    format!(
                        "model_reasoning_effort={}",
                        toml::Value::String(level.into())
                    ),
                ]),
                AgentKind::Opencode | AgentKind::Pi => {
                    bail!("Interactive reasoning override unsupported")
                }
            }
        }
        Ok(args)
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && !value.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn valid_model_identifier(harness: &AgentKind, value: &str) -> bool {
    if !valid_identifier(value) {
        return false;
    }
    match harness {
        AgentKind::Claude | AgentKind::Codex => true,
        AgentKind::Opencode | AgentKind::Pi => {
            let qualified = value
                .split_once('/')
                .is_some_and(|(provider, model)| !provider.is_empty() && !model.is_empty());
            // These harnesses also accept a reasoning suffix in --model.
            // Admit plain model identities only, otherwise a headless option
            // could bypass the lack of a reasoning-override launch seam.
            let has_suffix = match harness {
                AgentKind::Opencode => value.contains('#'),
                // A colon-bearing Pi ID is ambiguous without its resolver.
                // Fail closed rather than accepting an implicit thinking pick.
                AgentKind::Pi => value.contains(':'),
                _ => unreachable!(),
            };
            qualified && !has_suffix
        }
    }
}

fn valid_level(value: &str) -> bool {
    value
        .as_bytes()
        .first()
        .is_some_and(u8::is_ascii_alphanumeric)
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}

/// Parse capability metadata from Codex's own catalog, preserving the access
/// assessment supplied by discovery. Visibility is a picker rule, not proof
/// of authenticated access. Cache/NUX/config fallback must remain Unknown.
pub(crate) fn codex_catalog_models(
    value: &serde_json::Value,
    availability: Availability,
) -> Result<Vec<ModelCapability>> {
    let Some(models) = value.get("models").and_then(serde_json::Value::as_array) else {
        bail!("Codex catalog has no models array");
    };
    Ok(models
        .iter()
        .filter(|model| model.get("visibility").and_then(|v| v.as_str()) == Some("list"))
        .filter_map(|model| {
            let slug = model.get("slug")?.as_str()?;
            if !valid_identifier(slug) {
                return None;
            }
            let reasoning_levels = model
                .get("supported_reasoning_levels")
                .and_then(serde_json::Value::as_array)
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|level| level.get("effort")?.as_str())
                        .filter(|level| valid_level(level))
                        .map(str::to_string)
                        .collect()
                });
            Some(ModelCapability {
                model: slug.to_string(),
                availability,
                reasoning_levels,
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn capability(harness: AgentKind) -> HarnessCapability {
        let model = match harness {
            AgentKind::Claude | AgentKind::Codex => "exact-model",
            AgentKind::Opencode | AgentKind::Pi => "provider/exact-model",
        };
        HarnessCapability {
            harness,
            availability: Availability::Available,
            model_flag: true,
            reasoning_flag: true,
            models: vec![ModelCapability {
                model: model.into(),
                availability: Availability::Available,
                reasoning_levels: Some(vec!["low".into(), "high".into()]),
            }],
        }
    }

    fn options(caps: &[HarnessCapability], path: LaunchPath) -> EligibleOptions {
        EligibleOptions::new(&AgentKind::ALL, caps, path)
    }

    #[test]
    fn configured_harness_intersection_is_required_even_with_verified_access() {
        let caps: Vec<_> = AgentKind::ALL.into_iter().map(capability).collect();
        let eligible = EligibleOptions::new(&[AgentKind::Codex], &caps, LaunchPath::Interactive);
        assert_eq!(eligible.choices().len(), 3);
        assert!(
            eligible
                .choices()
                .iter()
                .all(|c| c.harness == AgentKind::Codex)
        );
        assert!(
            EligibleOptions::new(&[], &caps, LaunchPath::Interactive)
                .choices()
                .is_empty()
        );
    }

    #[test]
    fn cli_and_model_access_are_independent_and_unknown_is_not_available() {
        for availability in [Availability::Unavailable, Availability::Unknown] {
            let mut cap = capability(AgentKind::Codex);
            cap.availability = availability;
            assert!(
                options(&[cap], LaunchPath::Interactive)
                    .choices()
                    .is_empty()
            );
            let mut cap = capability(AgentKind::Codex);
            cap.models[0].availability = availability;
            assert!(
                options(&[cap], LaunchPath::Interactive)
                    .choices()
                    .is_empty()
            );
        }
    }

    #[test]
    fn interactive_paths_exclude_overrides_that_amf_cannot_carry() {
        for harness in AgentKind::ALL {
            let cap = capability(harness.clone());
            let eligible = options(&[cap], LaunchPath::Interactive);
            let expected = match harness {
                AgentKind::Claude | AgentKind::Codex => 3,
                AgentKind::Opencode | AgentKind::Pi => 0,
            };
            assert_eq!(eligible.choices().len(), expected, "{harness:?}");
        }
    }

    #[test]
    fn headless_paths_allow_models_but_exclude_explicit_reasoning_for_every_harness() {
        let caps: Vec<_> = AgentKind::ALL.into_iter().map(capability).collect();
        let eligible = options(&caps, LaunchPath::Headless);
        assert_eq!(eligible.choices().len(), 4);
        assert!(eligible.choices().iter().all(|c| c.reasoning().is_none()));
        for choice in eligible.choices() {
            assert!(eligible.interactive_args(choice).is_err());
        }
    }

    #[test]
    fn provider_based_choices_require_an_unambiguous_provider_identity() {
        for harness in [AgentKind::Opencode, AgentKind::Pi] {
            let mut cap = capability(harness);
            for invalid in ["exact-model", "/exact-model", "provider/"] {
                cap.models[0].model = invalid.into();
                assert!(
                    options(std::slice::from_ref(&cap), LaunchPath::Headless)
                        .choices()
                        .is_empty()
                );
            }
            cap.models[0].model = "provider/namespace/exact-model".into();
            assert_eq!(options(&[cap], LaunchPath::Headless).choices().len(), 1);
        }
    }

    #[test]
    fn model_suffixes_cannot_bypass_unsupported_reasoning_overrides() {
        for (harness, model) in [
            (AgentKind::Opencode, "provider/exact-model#high"),
            (AgentKind::Pi, "provider/exact-model:high"),
        ] {
            let mut cap = capability(harness);
            cap.models[0].model = model.into();
            assert!(options(&[cap], LaunchPath::Headless).choices().is_empty());
        }
    }

    #[test]
    fn launch_arguments_use_harness_controls_and_reject_a_stale_pick() {
        for harness in [AgentKind::Claude, AgentKind::Codex] {
            let cap = capability(harness.clone());
            let eligible = options(std::slice::from_ref(&cap), LaunchPath::Interactive);
            let selected = eligible
                .choices()
                .iter()
                .find(|c| c.reasoning() == Some("high"))
                .unwrap();
            let expected = match harness {
                AgentKind::Claude => vec!["--model", "exact-model", "--effort", "high"],
                AgentKind::Codex => vec![
                    "--model",
                    "exact-model",
                    "-c",
                    "model_reasoning_effort=\"high\"",
                ],
                _ => unreachable!(),
            };
            assert_eq!(eligible.interactive_args(selected).unwrap(), expected);
            let default = eligible
                .choices()
                .iter()
                .find(|c| c.reasoning().is_none())
                .unwrap();
            assert_eq!(
                eligible.interactive_args(default).unwrap(),
                vec!["--model", "exact-model"]
            );
            let changed = EligibleOptions::new(&[], &[cap], LaunchPath::Interactive);
            assert!(changed.interactive_args(selected).is_err());
        }
    }

    #[test]
    fn model_specific_levels_and_installed_flags_both_constrain_selection() {
        let mut cap = capability(AgentKind::Claude);
        cap.models.push(ModelCapability {
            model: "no-reasoning-model".into(),
            availability: Availability::Available,
            reasoning_levels: Some(vec![]),
        });
        cap.models.push(ModelCapability {
            model: "unknown-reasoning-model".into(),
            availability: Availability::Available,
            reasoning_levels: None,
        });
        let eligible = options(std::slice::from_ref(&cap), LaunchPath::Interactive);
        assert_eq!(eligible.choices().len(), 5);
        assert!(
            !eligible
                .choices()
                .iter()
                .any(|c| c.reasoning() == Some("max"))
        );
        assert!(
            eligible
                .choices()
                .iter()
                .filter(|c| c.model() != "exact-model")
                .all(|c| c.reasoning().is_none())
        );
        cap.reasoning_flag = false;
        let eligible = options(std::slice::from_ref(&cap), LaunchPath::Interactive);
        assert_eq!(eligible.choices().len(), 3);
        assert!(eligible.choices().iter().all(|c| c.reasoning().is_none()));
        cap.model_flag = false;
        assert!(
            options(&[cap], LaunchPath::Interactive)
                .choices()
                .is_empty()
        );
    }

    #[test]
    fn invented_ids_and_removed_capabilities_cannot_be_applied() {
        let cap = capability(AgentKind::Codex);
        let original = options(std::slice::from_ref(&cap), LaunchPath::Interactive);
        assert!(original.select("codex/exact-model/max").is_err());
        let selected = original
            .choices()
            .iter()
            .find(|c| c.reasoning() == Some("high"))
            .unwrap()
            .clone();
        assert_eq!(original.revalidate(&selected).unwrap(), &selected);
        let mut changed = cap.clone();
        changed.models[0].reasoning_levels = Some(vec!["low".into()]);
        assert!(
            options(&[changed], LaunchPath::Interactive)
                .revalidate(&selected)
                .is_err()
        );
        let mut changed = cap.clone();
        changed.availability = Availability::Unavailable;
        assert!(
            options(&[changed], LaunchPath::Interactive)
                .revalidate(&selected)
                .is_err()
        );
        let changed = EligibleOptions::new(
            &[AgentKind::Claude],
            std::slice::from_ref(&cap),
            LaunchPath::Interactive,
        );
        assert!(changed.revalidate(&selected).is_err());
        assert!(
            options(&[cap], LaunchPath::Headless)
                .revalidate(&selected)
                .is_err()
        );
    }

    #[test]
    fn duplicate_reports_are_deduplicated_but_conflicting_access_is_not_merged() {
        let cap = capability(AgentKind::Codex);
        assert_eq!(
            options(&[cap.clone(), cap.clone()], LaunchPath::Interactive)
                .choices()
                .len(),
            3
        );
        let mut conflicting = cap.clone();
        conflicting.models[0].availability = Availability::Unavailable;
        assert!(
            options(&[cap.clone(), conflicting], LaunchPath::Interactive)
                .choices()
                .is_empty()
        );
        let mut conflicting = cap.clone();
        conflicting.models.push(ModelCapability {
            model: "exact-model".into(),
            availability: Availability::Unknown,
            reasoning_levels: None,
        });
        assert!(
            options(&[conflicting], LaunchPath::Interactive)
                .choices()
                .is_empty()
        );
        let mut duplicates = cap.clone();
        duplicates.models.push(cap.models[0].clone());
        assert_eq!(
            options(&[duplicates], LaunchPath::Interactive)
                .choices()
                .len(),
            3
        );
    }

    #[test]
    fn catalog_does_not_promote_cached_names_or_defaults_to_available_options() {
        let catalog = json!({"models": [
            {"slug": "exact-model", "visibility": "list", "default_reasoning_level": "max",
             "supported_reasoning_levels": [{"effort": "low"}, {"effort": "high"}]},
            {"slug": "hidden-model", "visibility": "hide"},
            {"slug": "missing-visibility"}
        ]});
        let mut cap = capability(AgentKind::Codex);
        cap.models = codex_catalog_models(&catalog, Availability::Unknown).unwrap();
        assert_eq!(cap.models.len(), 1);
        assert!(
            options(std::slice::from_ref(&cap), LaunchPath::Interactive)
                .choices()
                .is_empty()
        );
        cap.models = codex_catalog_models(&catalog, Availability::Available).unwrap();
        let eligible = options(&[cap], LaunchPath::Interactive);
        assert_eq!(eligible.choices().len(), 3);
        assert!(
            !eligible
                .choices()
                .iter()
                .any(|c| c.reasoning() == Some("max"))
        );
    }

    #[test]
    fn malformed_catalog_and_invalid_identifiers_fail_closed() {
        assert!(codex_catalog_models(&json!({}), Availability::Available).is_err());
        assert!(codex_catalog_models(&json!({"models": {}}), Availability::Available).is_err());
        let catalog = json!({"models": [null, {},
            {"slug": "", "visibility": "list"},
            {"slug": "--flag", "visibility": "list"},
            {"slug": "model\nother", "visibility": "list"},
            {"slug": "exact-model", "visibility": "list", "supported_reasoning_levels": [
                null, {}, {"effort": 2}, {"effort": ""}, {"effort": "--flag"},
                {"effort": "high\nlow"}, {"effort": "ultra"}]}
        ]});
        let mut cap = capability(AgentKind::Codex);
        cap.models = codex_catalog_models(&catalog, Availability::Available).unwrap();
        let eligible = options(&[cap], LaunchPath::Interactive);
        assert_eq!(eligible.choices().len(), 2);
        assert_eq!(eligible.choices()[1].reasoning(), Some("ultra"));
    }

    #[test]
    fn unknown_reasoning_metadata_does_not_create_explicit_levels() {
        let catalog = json!({"models": [
            {"slug": "missing-levels", "visibility": "list"},
            {"slug": "bad-levels", "visibility": "list", "supported_reasoning_levels": "high"},
            {"slug": "empty-levels", "visibility": "list", "supported_reasoning_levels": []}
        ]});
        let mut cap = capability(AgentKind::Codex);
        cap.models = codex_catalog_models(&catalog, Availability::Available).unwrap();
        assert_eq!(cap.models[0].reasoning_levels, None);
        assert_eq!(cap.models[1].reasoning_levels, None);
        assert_eq!(cap.models[2].reasoning_levels, Some(vec![]));
        let eligible = options(&[cap], LaunchPath::Interactive);
        assert_eq!(eligible.choices().len(), 3);
        assert!(eligible.choices().iter().all(|c| c.reasoning().is_none()));
    }

    #[test]
    fn identities_are_stable_and_do_not_depend_on_catalog_order() {
        let cap = capability(AgentKind::Codex);
        let first = options(std::slice::from_ref(&cap), LaunchPath::Interactive);
        let mut reordered = cap;
        reordered.models[0]
            .reasoning_levels
            .as_mut()
            .unwrap()
            .reverse();
        assert_eq!(
            first.choices(),
            options(&[reordered], LaunchPath::Interactive).choices()
        );
        let default = ModelChoice::new(&AgentKind::Codex, "model", None);
        let named_default = ModelChoice::new(&AgentKind::Codex, "model", Some("default"));
        let other_harness = ModelChoice::new(&AgentKind::Claude, "model", None);
        assert_ne!(default.id(), named_default.id());
        assert_ne!(default.id(), other_harness.id());
    }
}
