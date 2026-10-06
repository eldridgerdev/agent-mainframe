//! Project-scope prompt overrides — the layer that is **shared and committed**
//! with the repo.
//!
//! The plan originally put these in a `.amf/prompts/` directory, but `.amf/`
//! is gitignored dir-wide in this codebase (see the generated `.amf/README.md`
//! and the repo's own `.gitignore`), so a file there would never be committed.
//! Instead they live under a `prompt_overrides` key in the tracked repo config,
//! `amf.json` (`ExtensionConfig::prompt_overrides`), keyed by stable
//! [`PromptId`] string. Feature- and global-scope overrides are per-user and
//! live in `amf.db` (`crate::db::prompt_overrides`).
//!
//! Shape:
//! ```json
//! "prompt_overrides": {
//!   "pr_review.ai_review": { "template": "…text with {{tokens}}…" },
//!   "learning.answer": {
//!     "template": "…shared…",
//!     "harnesses": { "codex": "…codex-specific…" }
//!   }
//! }
//! ```
//! A per-harness entry beats the shared `template` for that harness. Templates
//! are stored and rendered verbatim — no placeholder validation.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::project::AgentKind;

use super::PromptId;

/// One prompt's project-scope override: an optional shared template plus
/// optional per-harness templates keyed by [`AgentKind::slug`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptOverrideEntry {
    /// Applies to every harness without its own entry. Absent = no shared
    /// override (the entry then only carries per-harness templates).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    /// Per-harness templates, keyed by `"claude"` / `"codex"` / `"opencode"` /
    /// `"pi"`. Each beats [`Self::template`] for that one harness.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub harnesses: HashMap<String, String>,
}

impl PromptOverrideEntry {
    /// Nothing is set — treated as "no override", and skipped on serialize.
    pub fn is_empty(&self) -> bool {
        self.template.is_none() && self.harnesses.is_empty()
    }

    /// The template this entry supplies for `harness`: its per-harness value
    /// if present, else the shared `template`, else `None`.
    pub fn for_harness(&self, harness: &AgentKind) -> Option<&str> {
        self.harnesses
            .get(harness.slug())
            .map(String::as_str)
            .or(self.template.as_deref())
    }

    /// Set (or, with `None`, clear) the shared template.
    pub fn set_shared(&mut self, template: Option<String>) {
        self.template = template.filter(|t| !t.is_empty());
    }

    /// Set (or, with `None`, remove) the template for one harness.
    pub fn set_harness(&mut self, harness: &AgentKind, template: Option<String>) {
        match template {
            Some(text) => {
                self.harnesses.insert(harness.slug().to_string(), text);
            }
            None => {
                self.harnesses.remove(harness.slug());
            }
        }
    }
}

/// The `amf.json` `prompt_overrides` map: prompt-id string → entry.
pub type ProjectPromptOverrides = HashMap<String, PromptOverrideEntry>;

/// The effective project-scope template for `id` under `harness`, or `None`
/// when the repo config has no usable override for it.
pub fn effective<'a>(
    map: &'a ProjectPromptOverrides,
    id: PromptId,
    harness: &AgentKind,
) -> Option<&'a str> {
    map.get(id.as_str())
        .and_then(|entry| entry.for_harness(harness))
}

/// Read `{repo}/amf.json` (or the legacy `.amf/config.json`) and return just
/// its `prompt_overrides` map. Tolerant: a missing file, unreadable file, or
/// malformed JSON yields an empty map, so project scope simply contributes
/// nothing rather than failing a headless run.
///
/// Read live at resolution time (not cached), so hand-editing `amf.json`
/// changes the next prompt AMF sends.
pub fn load_from_repo(repo: &Path) -> ProjectPromptOverrides {
    let Some(path) = crate::extension::resolve_project_config_path(repo) else {
        return ProjectPromptOverrides::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return ProjectPromptOverrides::new();
    };
    // Pull just our key so an unrelated schema change elsewhere in the file
    // can't wipe overrides out from under a read.
    #[derive(Deserialize, Default)]
    struct JustOverrides {
        #[serde(default)]
        prompt_overrides: ProjectPromptOverrides,
    }
    serde_json::from_str::<JustOverrides>(&text)
        .map(|parsed| parsed.prompt_overrides)
        .unwrap_or_default()
}

/// Strict counterpart of [`load_from_repo`] for editors: a missing config is
/// an empty map, but an unreadable file, malformed JSON, a non-object root or
/// a malformed `prompt_overrides` value is an error. An editor must report
/// that rather than show "no overrides" and then overwrite the file.
pub fn load_strict(repo: &Path) -> anyhow::Result<ProjectPromptOverrides> {
    Ok(read_config_object(repo)?.1)
}

/// The repo config as a JSON object plus its parsed `prompt_overrides`.
fn read_config_object(
    repo: &Path,
) -> anyhow::Result<(
    serde_json::Map<String, serde_json::Value>,
    ProjectPromptOverrides,
)> {
    let Some(path) = crate::extension::resolve_project_config_path(repo) else {
        return Ok(Default::default());
    };
    let name = path.display();
    let text = std::fs::read_to_string(&path)
        .map_err(|error| anyhow::anyhow!("couldn't read {name}: {error}"))?;
    let value: serde_json::Value = serde_json::from_str(&text)
        .map_err(|error| anyhow::anyhow!("{name} is not valid JSON ({error}); fix it first"))?;
    let serde_json::Value::Object(object) = value else {
        anyhow::bail!("{name} must contain a JSON object; fix it first");
    };
    let overrides = match object.get("prompt_overrides") {
        None | Some(serde_json::Value::Null) => ProjectPromptOverrides::new(),
        Some(value) => serde_json::from_value(value.clone()).map_err(|error| {
            anyhow::anyhow!("{name} has an invalid prompt_overrides value ({error}); fix it first")
        })?,
    };
    Ok((object, overrides))
}

/// Read-modify-write only the `prompt_overrides` key of the repo's config.
///
/// Every other key is preserved exactly as parsed (including keys this build
/// does not know), the file is replaced atomically by
/// [`crate::extension::write_project_config`], and empty entries are dropped.
/// A config that cannot be parsed is refused rather than replaced with a
/// default one.
pub fn update_in_repo(
    repo: &Path,
    edit: impl FnOnce(&mut ProjectPromptOverrides),
) -> anyhow::Result<()> {
    let (mut object, mut overrides) = read_config_object(repo)?;
    edit(&mut overrides);
    overrides.retain(|_, entry| !entry.is_empty());
    if overrides.is_empty() {
        object.remove("prompt_overrides");
    } else {
        // Sorted keys keep the committed file's diff stable.
        let sorted: std::collections::BTreeMap<_, _> = overrides.into_iter().collect();
        object.insert("prompt_overrides".into(), serde_json::to_value(sorted)?);
    }
    let json = serde_json::to_string_pretty(&serde_json::Value::Object(object))?;
    crate::extension::write_project_config(repo, &json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_in_repo_preserves_other_keys_and_drops_empty_entries() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("amf.json"),
            r#"{"future_key":{"kept":true},"allowed_agents":["codex"]}"#,
        )
        .unwrap();
        update_in_repo(dir.path(), |map| {
            map.entry("session.summary".into())
                .or_default()
                .set_harness(&AgentKind::Codex, Some("codex {{recent_lines}}".into()));
        })
        .unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("amf.json")).unwrap())
                .unwrap();
        assert_eq!(raw["future_key"]["kept"], true);
        assert_eq!(raw["allowed_agents"][0], "codex");
        assert_eq!(
            raw["prompt_overrides"]["session.summary"]["harnesses"]["codex"],
            "codex {{recent_lines}}"
        );
        // Defaults of unrelated typed fields are not written back.
        assert!(raw.get("custom_sessions").is_none());

        update_in_repo(dir.path(), |map| {
            map.get_mut("session.summary")
                .unwrap()
                .set_harness(&AgentKind::Codex, None);
        })
        .unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("amf.json")).unwrap())
                .unwrap();
        assert!(raw.get("prompt_overrides").is_none(), "{raw}");
        assert_eq!(raw["future_key"]["kept"], true);
    }

    #[test]
    fn update_in_repo_refuses_to_replace_a_malformed_config() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("amf.json");
        for junk in ["{ not json", "[1, 2]", r#"{"prompt_overrides": 7}"#] {
            std::fs::write(&path, junk).unwrap();
            assert!(load_strict(dir.path()).is_err(), "{junk}");
            let error = update_in_repo(dir.path(), |map| {
                map.entry("session.summary".into())
                    .or_default()
                    .set_shared(Some("x".into()));
            })
            .unwrap_err();
            assert!(error.to_string().contains("fix it first"), "{error}");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), junk);
        }
        // A missing config is simply empty, and the first save creates it.
        std::fs::remove_file(&path).unwrap();
        assert!(load_strict(dir.path()).unwrap().is_empty());
    }

    fn entry(shared: Option<&str>, harnesses: &[(&str, &str)]) -> PromptOverrideEntry {
        PromptOverrideEntry {
            template: shared.map(str::to_string),
            harnesses: harnesses
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn for_harness_prefers_the_specific_template_then_the_shared_one() {
        let e = entry(Some("shared"), &[("codex", "codex text")]);
        assert_eq!(e.for_harness(&AgentKind::Codex), Some("codex text"));
        assert_eq!(e.for_harness(&AgentKind::Claude), Some("shared"));
    }

    #[test]
    fn for_harness_is_none_when_only_a_different_harness_is_set() {
        let e = entry(None, &[("pi", "pi text")]);
        assert_eq!(e.for_harness(&AgentKind::Pi), Some("pi text"));
        assert_eq!(e.for_harness(&AgentKind::Claude), None);
    }

    #[test]
    fn effective_reads_by_prompt_id_string() {
        let mut map = ProjectPromptOverrides::new();
        map.insert(
            "pr_review.ai_review".to_string(),
            entry(Some("repo review prompt {{annotated_diff}}"), &[]),
        );
        assert_eq!(
            effective(&map, PromptId::PrReviewAiReview, &AgentKind::Claude),
            Some("repo review prompt {{annotated_diff}}")
        );
        assert_eq!(
            effective(&map, PromptId::SessionSummary, &AgentKind::Claude),
            None
        );
    }

    #[test]
    fn serde_round_trips_and_skips_empty_pieces() {
        let mut map = ProjectPromptOverrides::new();
        map.insert("session.summary".to_string(), entry(Some("s"), &[]));
        map.insert(
            "learning.answer".to_string(),
            entry(None, &[("codex", "c")]),
        );

        let json = serde_json::to_string(&map).unwrap();
        assert!(
            !json.contains("harnesses\":{}"),
            "empty maps are skipped: {json}"
        );
        assert!(
            !json.contains("\"template\":null"),
            "absent shared is skipped: {json}"
        );

        let back: ProjectPromptOverrides = serde_json::from_str(&json).unwrap();
        assert_eq!(back, map);
    }

    #[test]
    fn load_from_repo_tolerates_absence_and_junk() {
        let dir = tempfile::TempDir::new().unwrap();
        // No amf.json at all.
        assert!(load_from_repo(dir.path()).is_empty());

        // Malformed JSON.
        std::fs::write(dir.path().join("amf.json"), "{ not json").unwrap();
        assert!(load_from_repo(dir.path()).is_empty());
    }

    #[test]
    fn load_from_repo_picks_up_a_hand_authored_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("amf.json"),
            r#"{
                "custom_sessions": [],
                "prompt_overrides": {
                    "review.walkthrough": {
                        "template": "hand-written {{patch}}",
                        "harnesses": { "codex": "hand-written codex {{patch}}" }
                    }
                }
            }"#,
        )
        .unwrap();

        let map = load_from_repo(dir.path());
        assert_eq!(
            effective(&map, PromptId::ReviewWalkthrough, &AgentKind::Claude),
            Some("hand-written {{patch}}")
        );
        assert_eq!(
            effective(&map, PromptId::ReviewWalkthrough, &AgentKind::Codex),
            Some("hand-written codex {{patch}}")
        );
    }

    #[test]
    fn set_helpers_add_and_clear() {
        let mut e = PromptOverrideEntry::default();
        assert!(e.is_empty());
        e.set_shared(Some("x".to_string()));
        e.set_harness(&AgentKind::Pi, Some("pi".to_string()));
        assert_eq!(e.for_harness(&AgentKind::Pi), Some("pi"));
        assert_eq!(e.for_harness(&AgentKind::Claude), Some("x"));

        e.set_harness(&AgentKind::Pi, None);
        e.set_shared(None);
        assert!(e.is_empty());
        // set_shared drops an empty string rather than storing it.
        e.set_shared(Some(String::new()));
        assert!(e.is_empty());
    }
}
