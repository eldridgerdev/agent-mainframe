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
}

impl PromptSource {
    pub fn label(self) -> &'static str {
        match self {
            PromptSource::User => "User",
            PromptSource::Global => "Global",
            PromptSource::Project => "Project",
        }
    }

    /// User templates can be edited and deleted in place; read-only
    /// sources must be duplicated to the user library first.
    pub fn is_editable(self) -> bool {
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

    /// First non-empty line of the body, for preview rows.
    pub fn preview_line(&self) -> &str {
        self.body
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim()
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
                let key = raw.trim();
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

/// Extract the distinct `{{key}}` token names from a body, in first-seen
/// order. Used by the phase 2 fill-in flow to infer placeholders.
pub fn infer_placeholder_keys(body: &str) -> Vec<String> {
    let mut keys: Vec<String> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            break;
        };
        let key = after[..close].trim();
        if !key.is_empty() && !keys.iter().any(|k| k == key) {
            keys.push(key.to_string());
        }
        rest = &after[close + 2..];
    }
    keys
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
}
