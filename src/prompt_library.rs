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

impl PromptPlaceholder {
    /// The heading shown for this slot in the fill-in flow: an explicit
    /// `label`, else the `key` for text slots. A bare menu (`{{a|b|c}}`) has no
    /// label and its key is the raw token text, so it falls back to a generic
    /// prompt rather than exposing that internal key.
    pub fn display_label(&self) -> &str {
        if let Some(label) = self.label.as_deref().filter(|l| !l.is_empty()) {
            return label;
        }
        match self.kind {
            PlaceholderKind::Select { .. } => "Choose an option",
            _ => &self.key,
        }
    }
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
                // A token may carry inline options (`{{a|b|c}}` or
                // `{{label: a|b|c}}`); `parse_placeholder_token` derives the
                // lookup key.
                let key = parse_placeholder_token(raw).key;
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

/// The result of parsing the inside of a `{{...}}` token: its lookup `key`,
/// an optional display `label`, and any inline `options`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedToken<'a> {
    /// Identifies the slot for substitution and dedup. For a labelled menu it
    /// is the label; for a bare menu it is the whole token text; for a free
    /// text slot it is the trimmed token.
    pub key: &'a str,
    /// Present only when the author wrote a `label:` prefix on a menu.
    pub label: Option<&'a str>,
    /// Inline choices; empty for a free-text slot.
    pub options: Vec<String>,
}

/// Split the inside of a `{{...}}` token into its key, optional label, and
/// inline options. A `|` introduces an option list; a `:` *before the first*
/// `|` names the menu:
///
/// - `{{name}}` → free text, key `"name"`, no options.
/// - `{{dev|staging|prod}}` → menu, every segment is an option, key is the
///   whole token text (`"dev|staging|prod"`), no label.
/// - `{{env: dev|staging|prod}}` → menu labelled `"env"`, key `"env"`, options
///   `["dev", "staging", "prod"]`.
///
/// Whitespace around each part is trimmed and empty options are dropped, so
/// `{{x: a | b |}}` yields `["a", "b"]`.
pub fn parse_placeholder_token(inner: &str) -> ParsedToken<'_> {
    let trimmed = inner.trim();
    let Some(pipe) = trimmed.find('|') else {
        // No options: a plain free-text slot keyed on its name.
        return ParsedToken {
            key: trimmed,
            label: None,
            options: Vec::new(),
        };
    };

    let head = &trimmed[..pipe];
    let tail = &trimmed[pipe + 1..];

    // A `:` in the head before any `|` labels the menu; otherwise the head is
    // itself the first option.
    let (label, first_option) = match head.find(':') {
        Some(colon) => {
            let label = head[..colon].trim();
            let first = head[colon + 1..].trim();
            (
                if label.is_empty() { None } else { Some(label) },
                first,
            )
        }
        None => (None, head.trim()),
    };

    let mut options = Vec::new();
    if !first_option.is_empty() {
        options.push(first_option.to_string());
    }
    options.extend(
        tail.split('|')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToString::to_string),
    );

    // A labelled menu keys on its label so a config-authored placeholder can
    // target it; a bare menu keys on the whole token text (stable + unique).
    let key = label.unwrap_or(trimmed);
    ParsedToken {
        key,
        label,
        options,
    }
}

/// A distinct slot inferred from a body, in first-seen order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InferredSlot {
    pub key: String,
    pub label: Option<String>,
    pub options: Vec<String>,
}

/// Extract the distinct slots from a body in first-seen order. When a key
/// recurs, the label/options from its first occurrence are kept.
pub fn infer_placeholder_slots(body: &str) -> Vec<InferredSlot> {
    let mut slots: Vec<InferredSlot> = Vec::new();
    let mut rest = body;
    while let Some(open) = rest.find("{{") {
        let after = &rest[open + 2..];
        let Some(close) = after.find("}}") else {
            break;
        };
        let parsed = parse_placeholder_token(&after[..close]);
        if !parsed.key.is_empty() && !slots.iter().any(|s| s.key == parsed.key) {
            slots.push(InferredSlot {
                key: parsed.key.to_string(),
                label: parsed.label.map(ToString::to_string),
                options: parsed.options,
            });
        }
        rest = &after[close + 2..];
    }
    slots
}

/// Extract the distinct slot keys from a body, in first-seen order. Inline
/// options and labels are ignored here — see `infer_placeholder_slots` for the
/// full form.
#[allow(dead_code)] // exercised only by unit tests
pub fn infer_placeholder_keys(body: &str) -> Vec<String> {
    infer_placeholder_slots(body)
        .into_iter()
        .map(|s| s.key)
        .collect()
}

/// Parse the editor's raw tag input into a clean tag list. Tags are separated
/// by commas or whitespace; a leading `#` is stripped, original case is kept,
/// and empties and case-insensitive duplicates are dropped. So
/// `"#bug, Frontend  bug"` yields `["bug", "Frontend"]`.
pub fn parse_tags(input: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for raw in input.split(|c: char| c == ',' || c.is_whitespace()) {
        let tag = raw.trim().trim_start_matches('#').trim();
        if tag.is_empty() {
            continue;
        }
        if !tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            tags.push(tag.to_string());
        }
    }
    tags
}

/// Render a tag list back into the comma-separated form shown (and re-parsed)
/// by the editor's tag field.
pub fn format_tags(tags: &[String]) -> String {
    tags.join(", ")
}

/// Score a template against the picker query, returning the best (lowest)
/// fuzzy score across its name, body, and tags, or `None` when nothing
/// matches. A query beginning with `#` is a tag-only filter (the `#` is
/// stripped); a bare `#` surfaces every tagged template (a light "group by
/// tagged" view) and hides untagged ones.
pub fn prompt_filter_score(name: &str, body: &str, tags: &[String], query: &str) -> Option<usize> {
    use crate::app::util::fuzzy_match_score;
    if let Some(rest) = query.strip_prefix('#') {
        let needle = rest.trim();
        if needle.is_empty() {
            return if tags.is_empty() { None } else { Some(0) };
        }
        return tags.iter().filter_map(|t| fuzzy_match_score(t, needle)).min();
    }
    let tag_best = tags.iter().filter_map(|t| fuzzy_match_score(t, query)).min();
    [
        fuzzy_match_score(name, query),
        fuzzy_match_score(body, query),
        tag_best,
    ]
    .into_iter()
    .flatten()
    .min()
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
    fn parse_token_free_text_slot() {
        // No `|`: a plain text slot keyed on its (trimmed) name, colons and all.
        assert_eq!(parse_placeholder_token("name").key, "name");
        assert!(parse_placeholder_token("name").options.is_empty());
        assert_eq!(parse_placeholder_token(" name ").key, "name");
        assert_eq!(parse_placeholder_token("ticket:id").key, "ticket:id");
    }

    #[test]
    fn parse_token_bare_menu_lists_every_segment() {
        // No label: every `|` segment is an option; key is the whole token.
        let parsed = parse_placeholder_token("dev|staging|prod");
        assert_eq!(parsed.label, None);
        assert_eq!(parsed.key, "dev|staging|prod");
        assert_eq!(
            parsed.options,
            vec!["dev".to_string(), "staging".to_string(), "prod".to_string()]
        );
    }

    #[test]
    fn parse_token_labelled_menu_splits_on_colon() {
        // `label:` before the first `|` names the menu and is not an option.
        let parsed = parse_placeholder_token("env: dev|staging|prod");
        assert_eq!(parsed.label, Some("env"));
        assert_eq!(parsed.key, "env");
        assert_eq!(
            parsed.options,
            vec!["dev".to_string(), "staging".to_string(), "prod".to_string()]
        );
        // Whitespace trimmed, empty options dropped.
        let parsed = parse_placeholder_token("x: a | b |");
        assert_eq!(parsed.key, "x");
        assert_eq!(parsed.options, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn infer_slots_carries_label_and_options() {
        let slots = infer_placeholder_slots("Deploy {{env: dev|prod}} as {{user}}");
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].key, "env");
        assert_eq!(slots[0].label.as_deref(), Some("env"));
        assert_eq!(slots[0].options, vec!["dev".to_string(), "prod".to_string()]);
        assert_eq!(slots[1].key, "user");
        assert_eq!(slots[1].label, None);
        assert!(slots[1].options.is_empty());
    }

    #[test]
    fn render_labelled_menu_matches_label_key() {
        let values = vec![("env".to_string(), "staging".to_string())];
        assert_eq!(
            render_template("Deploy to {{env: dev|staging|prod}}", &values),
            "Deploy to staging"
        );
    }

    #[test]
    fn render_bare_menu_matches_raw_token_key() {
        let values = vec![("dev|staging|prod".to_string(), "staging".to_string())];
        assert_eq!(
            render_template("Deploy to {{dev|staging|prod}}", &values),
            "Deploy to staging"
        );
    }

    #[test]
    fn parse_tags_splits_strips_hash_and_dedups() {
        // Commas and whitespace both separate; `#` stripped; case-insensitive
        // dedup keeps the first spelling; empties dropped.
        assert_eq!(
            parse_tags("#bug, Frontend  bug ,, #frontend"),
            vec!["bug".to_string(), "Frontend".to_string()]
        );
        assert!(parse_tags("   ").is_empty());
        assert!(parse_tags("").is_empty());
    }

    #[test]
    fn format_tags_round_trips_through_parse() {
        let tags = vec!["bug".to_string(), "Frontend".to_string()];
        assert_eq!(format_tags(&tags), "bug, Frontend");
        assert_eq!(parse_tags(&format_tags(&tags)), tags);
    }

    #[test]
    fn filter_score_matches_name_body_or_tag() {
        let tags = vec!["frontend".to_string()];
        // Name match.
        assert!(prompt_filter_score("Fix login", "body", &tags, "login").is_some());
        // Body match.
        assert!(prompt_filter_score("name", "refactor auth", &tags, "auth").is_some());
        // Tag match (no `#`).
        assert!(prompt_filter_score("name", "body", &tags, "frontend").is_some());
        // No match anywhere.
        assert!(prompt_filter_score("name", "body", &tags, "zzz").is_none());
    }

    #[test]
    fn filter_score_hash_prefix_matches_tags_only() {
        let tags = vec!["frontend".to_string()];
        // `#`-prefixed query matches the tag but not name/body text.
        assert!(prompt_filter_score("frontend", "frontend", &[], "#frontend").is_none());
        assert!(prompt_filter_score("name", "body", &tags, "#front").is_some());
    }

    #[test]
    fn filter_score_bare_hash_surfaces_only_tagged() {
        // Bare `#` keeps tagged templates, hides untagged ones.
        assert!(prompt_filter_score("n", "b", &["x".to_string()], "#").is_some());
        assert!(prompt_filter_score("n", "b", &[], "#").is_none());
    }
}
