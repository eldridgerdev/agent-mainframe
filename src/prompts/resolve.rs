//! Effective-template selection and placeholder interpolation.
//!
//! [`resolve_prompt_layered`] is the entry point every headless call site will
//! use (task 7) instead of assembling its prompt inline. It picks the
//! effective template across four layers and interpolates the caller's
//! context:
//!
//! 1. **feature** — a `prompt_overrides` row in `amf.db` keyed by the
//!    feature's workdir path;
//! 2. **project** — an entry in `amf.json`'s `prompt_overrides`
//!    ([`crate::prompts::project`]);
//! 3. **global** — a keyless `prompt_overrides` row in `amf.db`;
//! 4. **built-in** — the registered [`super::PromptSpec::default_template`].
//!
//! Nearest layer wins. Within the winning layer, a per-harness template beats
//! the shared one — but a nearer *shared* override still beats a farther
//! *per-harness* one, because the layer is chosen before the harness tie-break
//! is applied. When a layer supplies an override, the built-in default for
//! that prompt is never consulted (this is the "default drift" behaviour: a
//! shipped change to a built-in template is silently ignored while an override
//! stands).
//!
//! Interpolation is deliberately unvalidated: [`render_template`] replaces
//! every `{{name}}` for which `ctx` has a value and leaves everything else —
//! a token the caller did not supply, an unknown token an override
//! introduced, a stray `{{` — exactly as written. A user override can drop a
//! required token or add a meaningless one and AMF will render it as-is.

use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::db::prompt_overrides::{OverrideScope, PromptOverrides};
use crate::project::AgentKind;

use super::project::ProjectPromptOverrides;
use super::{PromptId, spec};

/// The dynamic values a call site supplies for one headless run, keyed by
/// placeholder name (no braces). Values are substituted verbatim.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PromptContext {
    values: BTreeMap<String, String>,
}

impl PromptContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builder-style insert: `PromptContext::new().with("diff", d).with(...)`.
    #[must_use]
    pub fn with(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.values.insert(key.into(), value.into());
        self
    }

    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.values.insert(key.into(), value.into());
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl<K, V> FromIterator<(K, V)> for PromptContext
where
    K: Into<String>,
    V: Into<String>,
{
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        Self {
            values: iter
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }
}

/// Which layer supplied the effective template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptSource {
    /// A `prompt_overrides` row keyed by the feature's workdir.
    Feature,
    /// An entry in `amf.json`'s `prompt_overrides`.
    Project,
    /// A keyless (machine-wide) `prompt_overrides` row.
    Global,
    /// The registered built-in default — no override applied.
    BuiltIn,
}

impl PromptSource {
    /// Short label for the manager overlay and debug logs.
    pub fn label(self) -> &'static str {
        match self {
            PromptSource::Feature => "feature override",
            PromptSource::Project => "project override (amf.json)",
            PromptSource::Global => "global override",
            PromptSource::BuiltIn => "built-in default",
        }
    }

    pub fn is_override(self) -> bool {
        !matches!(self, PromptSource::BuiltIn)
    }
}

/// The override sources available when resolving a prompt. Every field is
/// optional: a missing source contributes nothing and resolution falls
/// through to the next layer (and, in the limit, the built-in default). This
/// is what makes the no-database / no-repo-config path just work.
#[derive(Default, Clone, Copy)]
pub struct PromptLayers<'a> {
    /// The feature's workdir path, keying its feature-scope overrides. `None`
    /// disables the feature layer entirely.
    pub feature_workdir: Option<&'a str>,
    /// In-memory view of the `prompt_overrides` table (feature **and** global
    /// rows). `None` when there is no database.
    pub db: Option<&'a PromptOverrides>,
    /// The repo's `amf.json` `prompt_overrides` map. `None` when the feature
    /// has no project config.
    pub project: Option<&'a ProjectPromptOverrides>,
}

impl<'a> PromptLayers<'a> {
    /// No overrides — resolution returns the built-in default. Used by tests
    /// and by any call site with neither a database nor project config.
    pub fn none() -> Self {
        Self::default()
    }
}

/// Pick the effective template for `id` under `harness`, and say where it came
/// from. Does not interpolate — see [`resolve_prompt_layered`].
pub fn resolve_template_layered<'a>(
    id: PromptId,
    harness: &AgentKind,
    layers: &PromptLayers<'a>,
) -> (Cow<'a, str>, PromptSource) {
    // 1. feature — DB rows keyed by workdir path
    if let (Some(workdir), Some(db)) = (layers.feature_workdir, layers.db) {
        let scope = OverrideScope::Feature {
            workdir: workdir.to_string(),
        };
        if let Some(template) = db.effective_at(id.as_str(), &scope, harness) {
            return (Cow::Owned(template.to_string()), PromptSource::Feature);
        }
    }

    // 2. project — amf.json prompt_overrides
    if let Some(project) = layers.project
        && let Some(template) = super::project::effective(project, id, harness)
    {
        return (Cow::Owned(template.to_string()), PromptSource::Project);
    }

    // 3. global — keyless DB rows
    if let Some(db) = layers.db
        && let Some(template) = db.effective_at(id.as_str(), &OverrideScope::Global, harness)
    {
        return (Cow::Owned(template.to_string()), PromptSource::Global);
    }

    // 4. built-in — the shipped default (never consulted above)
    (
        Cow::Borrowed(spec(id).default_template_for(harness)),
        PromptSource::BuiltIn,
    )
}

/// Resolve the effective template for `id`/`harness` across `layers` and
/// interpolate `ctx` into it.
pub fn resolve_prompt_layered(
    id: PromptId,
    harness: &AgentKind,
    ctx: &PromptContext,
    layers: &PromptLayers<'_>,
) -> String {
    let (template, _source) = resolve_template_layered(id, harness, layers);
    render_template(&template, ctx)
}

/// Return the built-in default template for `id` under `harness`, with `ctx`
/// interpolated and **no** override layers consulted. Convenience for tests
/// and for the rare call site that has no `App` to source overrides from.
pub fn resolve_prompt(id: PromptId, harness: &AgentKind, ctx: &PromptContext) -> String {
    resolve_prompt_layered(id, harness, ctx, &PromptLayers::none())
}

/// Substitute `{{name}}` tokens in `template` from `ctx`.
///
/// - `{{name}}` with `ctx.get("name") == Some(v)` → `v`.
/// - `{{name}}` with no value → the literal text `{{name}}` (unchanged),
///   whether or not `name` is a declared placeholder for any prompt.
/// - An unterminated `{{` → the rest of the string, verbatim.
///
/// Substituted values are never re-scanned, so a value that itself contains
/// `{{x}}` cannot trigger a second substitution.
pub fn render_template(template: &str, ctx: &PromptContext) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("}}") else {
            // No closing braces: everything from `{{` on is literal.
            out.push_str(&rest[open..]);
            return out;
        };
        let raw = &after_open[..close];
        match ctx.get(raw.trim()) {
            Some(value) => out.push_str(value),
            None => {
                // Unknown / unsupplied token: emit it exactly as written.
                out.push_str("{{");
                out.push_str(raw);
                out.push_str("}}");
            }
        }
        rest = &after_open[close + 2..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_override_returns_the_builtin_default_interpolated() {
        let ctx = PromptContext::new()
            .with("file_path", "src/lib.rs")
            .with("old_snippet", "a")
            .with("new_snippet", "b");
        let rendered = resolve_prompt(PromptId::ReviewDiffExplain, &AgentKind::Claude, &ctx);
        assert!(rendered.starts_with("Explain these code changes concisely."));
        assert!(rendered.contains("File: src/lib.rs"));
        assert!(rendered.contains("Old:\n```\na\n```"));
        assert!(rendered.contains("New:\n```\nb\n```"));
        assert!(!rendered.contains("{{"));
    }

    #[test]
    fn a_supplied_placeholder_is_substituted() {
        let ctx = PromptContext::new().with("q", "why");
        assert_eq!(render_template("[{{q}}]", &ctx), "[why]");
    }

    #[test]
    fn a_missing_declared_placeholder_is_rendered_literally() {
        // `file_path` is a real placeholder for this prompt, but the caller
        // did not supply it: the token survives verbatim, unvalidated.
        let ctx = PromptContext::new()
            .with("old_snippet", "x")
            .with("new_snippet", "y");
        let rendered = resolve_prompt(PromptId::ReviewDiffExplain, &AgentKind::Claude, &ctx);
        assert!(rendered.contains("File: {{file_path}}"), "{rendered}");
    }

    #[test]
    fn an_unknown_token_is_rendered_literally() {
        let ctx = PromptContext::new().with("known", "K");
        assert_eq!(
            render_template("{{known}} and {{totally_unknown}}", &ctx),
            "K and {{totally_unknown}}"
        );
    }

    #[test]
    fn substituted_values_are_not_rescanned() {
        let ctx = PromptContext::new()
            .with("a", "{{b}}")
            .with("b", "SHOULD_NOT_APPEAR");
        assert_eq!(render_template("{{a}}", &ctx), "{{b}}");
    }

    #[test]
    fn unterminated_open_braces_are_literal() {
        let ctx = PromptContext::new().with("x", "1");
        assert_eq!(
            render_template("value {{x}} then {{oops", &ctx),
            "value 1 then {{oops"
        );
    }

    #[test]
    fn whitespace_inside_a_token_is_tolerated_for_lookup() {
        let ctx = PromptContext::new().with("x", "1");
        assert_eq!(render_template("{{ x }}", &ctx), "1");
    }

    #[test]
    fn empty_context_leaves_a_template_untouched() {
        let ctx = PromptContext::new();
        assert_eq!(render_template("{{a}}/{{b}}", &ctx), "{{a}}/{{b}}");
    }

    #[test]
    fn per_harness_resolution_matches_the_shared_default_today() {
        let ctx = PromptContext::new()
            .with("recent_lines", "l")
            .with("harness_name", "Codex")
            .with("max_chars", "60");
        let claude = resolve_prompt(PromptId::SessionSummary, &AgentKind::Claude, &ctx);
        let codex = resolve_prompt(PromptId::SessionSummary, &AgentKind::Codex, &ctx);
        assert_eq!(claude, codex);
        assert!(claude.contains("Summarize this Codex session in one line (max 60 chars)"));
    }

    // ---- layered resolution: precedence and default drift ----

    use crate::prompts::project::{ProjectPromptOverrides, PromptOverrideEntry};

    const ID: PromptId = PromptId::SessionSummary;

    fn db_with(rows: &[(OverrideScope, Option<AgentKind>, &str)]) -> PromptOverrides {
        let mut set = PromptOverrides::default();
        for (scope, harness, template) in rows {
            set.set(None, ID.as_str(), scope.clone(), harness.clone(), template)
                .unwrap();
        }
        set
    }

    fn project_with(entry: PromptOverrideEntry) -> ProjectPromptOverrides {
        let mut map = ProjectPromptOverrides::new();
        map.insert(ID.as_str().to_string(), entry);
        map
    }

    fn feature(workdir: &str) -> OverrideScope {
        OverrideScope::Feature {
            workdir: workdir.to_string(),
        }
    }

    #[test]
    fn no_layers_resolves_to_the_builtin_default() {
        let (template, source) =
            resolve_template_layered(ID, &AgentKind::Claude, &PromptLayers::none());
        assert_eq!(source, PromptSource::BuiltIn);
        assert_eq!(template, spec(ID).default_template);
    }

    #[test]
    fn precedence_is_feature_then_project_then_global_then_builtin() {
        let db = db_with(&[
            (feature("/w"), None, "FEATURE"),
            (OverrideScope::Global, None, "GLOBAL"),
        ]);
        let project = project_with(PromptOverrideEntry {
            template: Some("PROJECT".into()),
            ..Default::default()
        });

        // All three present → feature wins.
        let all = PromptLayers {
            feature_workdir: Some("/w"),
            db: Some(&db),
            project: Some(&project),
        };
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Claude, &all),
            (Cow::Owned("FEATURE".to_string()), PromptSource::Feature)
        );

        // Drop the feature layer (wrong workdir) → project wins.
        let no_feature = PromptLayers {
            feature_workdir: Some("/other"),
            ..all
        };
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Claude, &no_feature).1,
            PromptSource::Project
        );

        // Drop project too → global wins.
        let only_db = PromptLayers {
            feature_workdir: Some("/other"),
            db: Some(&db),
            project: None,
        };
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Claude, &only_db),
            (Cow::Owned("GLOBAL".to_string()), PromptSource::Global)
        );

        // Nothing applicable → built-in.
        let empty_db = PromptOverrides::default();
        let none = PromptLayers {
            feature_workdir: Some("/w"),
            db: Some(&empty_db),
            project: None,
        };
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Claude, &none).1,
            PromptSource::BuiltIn
        );
    }

    #[test]
    fn per_harness_beats_shared_within_the_winning_layer() {
        let project = project_with(PromptOverrideEntry {
            template: Some("proj shared".into()),
            harnesses: [("codex".to_string(), "proj codex".to_string())]
                .into_iter()
                .collect(),
        });
        let layers = PromptLayers {
            project: Some(&project),
            ..PromptLayers::none()
        };
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Codex, &layers).0,
            "proj codex"
        );
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Claude, &layers).0,
            "proj shared"
        );
    }

    #[test]
    fn a_nearer_shared_override_beats_a_farther_per_harness_override() {
        // Global has a Codex-specific override; project has only a shared one.
        // The plan's rule: the layer is chosen first, so project (nearer)
        // wins even for Codex.
        let db = db_with(&[(
            OverrideScope::Global,
            Some(AgentKind::Codex),
            "global codex",
        )]);
        let project = project_with(PromptOverrideEntry {
            template: Some("project shared".into()),
            ..Default::default()
        });
        let layers = PromptLayers {
            feature_workdir: None,
            db: Some(&db),
            project: Some(&project),
        };
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Codex, &layers),
            (
                Cow::Owned("project shared".to_string()),
                PromptSource::Project
            )
        );
    }

    #[test]
    fn an_override_makes_the_builtin_default_irrelevant_default_drift() {
        // Simulate a shipped change to the built-in: whatever `default_template`
        // becomes, an existing override is served verbatim and the built-in is
        // never consulted.
        let project = project_with(PromptOverrideEntry {
            template: Some("pinned override {{recent_lines}}".into()),
            ..Default::default()
        });
        let layers = PromptLayers {
            project: Some(&project),
            ..PromptLayers::none()
        };
        let (template, source) = resolve_template_layered(ID, &AgentKind::Claude, &layers);
        assert_eq!(source, PromptSource::Project);
        assert_eq!(template, "pinned override {{recent_lines}}");
        assert_ne!(
            template,
            spec(ID).default_template,
            "the override must not coincidentally equal the built-in"
        );

        // And the override's tokens are still interpolated.
        let ctx = PromptContext::new().with("recent_lines", "did things");
        assert_eq!(
            resolve_prompt_layered(ID, &AgentKind::Claude, &ctx, &layers),
            "pinned override did things"
        );
    }

    #[test]
    fn a_missing_db_or_project_source_just_falls_through() {
        let db = db_with(&[(OverrideScope::Global, None, "GLOBAL")]);
        // feature_workdir set but db None → feature layer skipped, no panic.
        let layers = PromptLayers {
            feature_workdir: Some("/w"),
            db: Some(&db),
            project: None,
        };
        assert_eq!(
            resolve_template_layered(ID, &AgentKind::Pi, &layers).1,
            PromptSource::Global
        );
    }
}
