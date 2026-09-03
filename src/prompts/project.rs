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
    map.get(id.as_str()).and_then(|entry| entry.for_harness(harness))
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

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(!json.contains("harnesses\":{}"), "empty maps are skipped: {json}");
        assert!(!json.contains("\"template\":null"), "absent shared is skipped: {json}");

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
