//! Prompt library data model.
//!
//! A prompt library is a named collection of reusable prompt templates.
//! Phase 1 stores user templates in the SQLite `ProjectStore` (mirroring
//! `session_bookmarks`) and injects a chosen template into a session —
//! seeding the compose box when compose interception is on, or pasting
//! straight into the agent window (without sending) when it is off.
//!
//! Templates can carry `{{placeholder}}` slots and explicit placeholder
//! definitions; the fill-in flow that collects values is phase 2, so
//! phase 1 simply delivers the body verbatim. The richer types are kept
//! here so the storage schema is forward-compatible.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a template came from. User templates are runtime-mutable and
/// live in the SQLite store; `Global` / `Project` templates (phase 3)
/// are read-only and merged in from `ExtensionConfig`. The source is
/// attached at load time and never serialized into the user store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptSource {
    #[default]
    User,
    Global,
    Project,
    /// Template lives in the active feature's worktree `.amf/config.json`.
    /// Branch-specific; not yet promoted to the main repo.
    Worktree,
}

impl PromptSource {
    pub fn label(self) -> &'static str {
        match self {
            PromptSource::User => "User",
            PromptSource::Global => "Global",
            PromptSource::Project => "Project",
            PromptSource::Worktree => "Worktree",
        }
    }

    /// All templates can be opened in the editor and saved back to their
    /// source file (or the SQLite store for `User`).
    pub fn is_editable(self) -> bool {
        true
    }

    /// Only `User` templates can be deleted; config-file sources require
    /// manually removing the entry from the file.
    pub fn is_deletable(self) -> bool {
        matches!(self, PromptSource::User)
    }
}

/// How a `{{placeholder}}` slot collects its value. Phase 2 implements
/// the fill-in flow; phase 1 only serializes these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PlaceholderKind {
    Text { default: Option<String> },
    MultiLine { default: Option<String> },
    Select { options: Vec<String> },
}

impl Default for PlaceholderKind {
    fn default() -> Self {
        PlaceholderKind::Text { default: None }
    }
}

/// An explicit placeholder definition. When a template has no explicit
/// definitions the fill-in flow infers `Text` slots from `{{...}}`
/// tokens in the body, so plain templates need none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptPlaceholder {
    /// Matches `{{key}}` in the body.
    pub key: String,
    /// Prompt shown in the fill-in flow; defaults to `key`.
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default, flatten)]
    pub kind: PlaceholderKind,
    #[serde(default)]
    pub required: bool,
}

fn new_template_id() -> String {
    Uuid::new_v4().to_string()
}

/// A reusable prompt template.
///
/// `id`, `created_at`, and `updated_at` have serde defaults so templates
/// hand-authored in (or exported to) `config.json` may omit them — the
/// SQLite store always populates them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptTemplate {
    /// uuid v4.
    #[serde(default = "new_template_id")]
    pub id: String,
    /// Display title.
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// Prompt text; may hold `{{slots}}`.
    pub body: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub placeholders: Vec<PromptPlaceholder>,
    #[serde(default = "Utc::now")]
    pub created_at: DateTime<Utc>,
    #[serde(default = "Utc::now")]
    pub updated_at: DateTime<Utc>,
}

impl PromptTemplate {
    /// Build a new user template with a fresh id and timestamps.
    pub fn new(name: String, body: String) -> Self {
        let now = Utc::now();
        Self {
            id: new_template_id(),
            name,
            description: None,
            body,
            tags: Vec::new(),
            placeholders: Vec::new(),
            created_at: now,
            updated_at: now,
        }
    }
}

/// Substitute `{{key}}` tokens in `body` with the provided values.
/// Tokens without a matching value collapse to an empty string, so an
/// unfilled optional slot simply disappears. Whitespace inside the
/// braces is tolerated (`{{ key }}` matches `key`).
pub fn render_template(body: &str, values: &[(String, String)]) -> String {
    let mut out = String::with_capacity(body.len());
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(close) = body[i + 2..].find("}}") {
                let raw = &body[i + 2..i + 2 + close];
                // A token may carry inline options (`{{key|a|b}}`); the lookup
                // key is the part before the first `|`.
                let (key, _options) = parse_placeholder_token(raw);
                let replacement = values
                    .iter()
                    .find(|(k, _)| k == key)
                    .map(|(_, v)| v.as_str())
                    .unwrap_or("");
                out.push_str(replacement);
                i = i + 2 + close + 2;
                continue;
            }
        }
        let ch = body[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Split the inside of a `{{...}}` token into its lookup key and any inline
/// options. `{{name}}` → (`"name"`, `[]`); `{{env|dev|prod}}` →
/// (`"env"`, `["dev", "prod"]`). Whitespace around each part is trimmed and
/// empty options are dropped, so `{{x| a | b |}}` yields `["a", "b"]`.
pub fn parse_placeholder_token(inner: &str) -> (&str, Vec<String>) {
    match inner.split_once('|') {
        Some((key, rest)) => {
            let options = rest
                .split('|')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect();
            (key.trim(), options)
        }
        None => (inner.trim(), Vec::new()),
    }
}

/// Extract the distinct slots from a body in first-seen order, each as its
/// lookup key plus any inline options (`{{env|dev|prod}}`). When a key recurs,
/// the options from its first occurrence are kept.
pub fn infer_placeholder_slots(body: &str) -> Vec<(String, Vec<String>)> {
    let mut slots: Vec<(String, Vec<String>)> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            break;
        };
        let (key, options) = parse_placeholder_token(&after[..close]);
        if !key.is_empty() && !slots.iter().any(|(k, _)| k == key) {
            slots.push((key.to_string(), options));
        }
        rest = &after[close + 2..];
    }
    slots
}

/// Extract the distinct `{{key}}` token names from a body, in first-seen
/// order. Inline options (`{{key|a|b}}`) are ignored here — see
/// `infer_placeholder_slots` for the key + options form.
#[allow(dead_code)] // exercised only by unit tests
pub fn infer_placeholder_keys(body: &str) -> Vec<String> {
    infer_placeholder_slots(body)
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_substitutes_filled_slots() {
        let values = vec![("name".to_string(), "Ada".to_string())];
        assert_eq!(render_template("Hi {{name}}!", &values), "Hi Ada!");
    }

    #[test]
    fn render_collapses_missing_optional_slot() {
        assert_eq!(render_template("a{{gone}}b", &[]), "ab");
    }

    #[test]
    fn render_repeats_slot_value() {
        let values = vec![("x".to_string(), "Z".to_string())];
        assert_eq!(render_template("{{x}}-{{x}}", &values), "Z-Z");
    }

    #[test]
    fn render_tolerates_inner_whitespace() {
        let values = vec![("k".to_string(), "v".to_string())];
        assert_eq!(render_template("{{ k }}", &values), "v");
    }

    #[test]
    fn render_no_slots_is_identity() {
        assert_eq!(render_template("plain text", &[]), "plain text");
    }

    #[test]
    fn render_leaves_unclosed_braces() {
        assert_eq!(render_template("a {{ unclosed", &[]), "a {{ unclosed");
    }

    #[test]
    fn infer_keys_returns_distinct_in_order() {
        let keys = infer_placeholder_keys("{{a}} {{b}} {{a}} {{ c }}");
        assert_eq!(keys, vec!["a", "b", "c"]);
    }

    #[test]
    fn parse_token_splits_key_and_options() {
        assert_eq!(parse_placeholder_token("name"), ("name", vec![]));
        assert_eq!(parse_placeholder_token(" name "), ("name", vec![]));
        assert_eq!(
            parse_placeholder_token("env|dev|staging|prod"),
            ("env", vec!["dev".to_string(), "staging".to_string(), "prod".to_string()])
        );
        // Whitespace trimmed, empty options dropped.
        assert_eq!(
            parse_placeholder_token("x| a | b |"),
            ("x", vec!["a".to_string(), "b".to_string()])
        );
    }

    #[test]
    fn infer_slots_carries_options() {
        let slots = infer_placeholder_slots("Deploy {{env|dev|prod}} as {{user}}");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].0, "env");
        assert_eq!(slots[0].1, vec!["dev".to_string(), "prod".to_string()]);
        assert_eq!(slots[1].0, "user");
        assert!(slots[1].1.is_empty());
    }

    #[test]
    fn render_ignores_inline_options_and_matches_key() {
        let values = vec![("env".to_string(), "staging".to_string())];
        assert_eq!(
            render_template("Deploy to {{env|dev|staging|prod}}", &values),
            "Deploy to staging"
        );
    }
}
