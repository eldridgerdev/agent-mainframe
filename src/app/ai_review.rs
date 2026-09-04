//! AI PR review: AMF generates its own review of a PR's diff (`A` to
//! generate, `W` to post as a GitHub review) as its own workflow, independent
//! of PR Triage.
//!
//! This used to be bolted onto PR Triage (`AppMode::PrReview`), converting
//! each finding into a synthetic `PrComment` merged into the same list real
//! GitHub comments live in. That fought the triage pane's data model
//! repeatedly (see `docs/backlog/pr-comment-review-plan.md`'s "does AI review
//! belong in this pane" open question): a synthetic id range kept clear of
//! real GitHub ids, a bot/human chip special-cased for a third "AI, not yet
//! posted" kind, a `diff_hunk` reconstructed from the full diff instead of
//! coming for free, and a background-job lifecycle that didn't compose with
//! "merge into whichever pane the user is looking at" (a real bug — findings
//! silently dropped after `esc`).
//!
//! This module is fully independent instead: [`AiReviewFinding`] is a
//! first-class, persisted type (its own `ai_review_cache` SQLite table, not a
//! disguised `PrComment`), and posting (`W`) doesn't reconcile anything back
//! onto itself — once posted, a finding simply exists on GitHub and the
//! automatic post-success PR Triage refresh fetches it as an ordinary
//! comment. Reachable from PR Triage (`A`), the dashboard, an agent session
//! (leader key), and the PR picker.

use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::*;
use crate::editor::TextEditor;
use crate::github::{GhCli, PrRef, PrResolution, PrReviewComment as GhPrReviewComment};
use crate::headless::HeadlessRunner;

/// The heading [`ai_review_prompt`] instructs the agent to emit per finding,
/// e.g. `### src/app/sync.rs|RIGHT|42` or `### General`.
/// [`parse_ai_findings`] parses exactly this shape back out.
const AI_FINDING_HEADING_PREFIX: &str = "### ";

/// Lines of context kept on each side of the target line when extracting a
/// windowed hunk for a finding ([`diff_hunk_for_location`]). Deliberately small:
/// unlike a human reviewer's inline comment — which GitHub anchors to a hunk
/// that's already a few lines of context around a small change — an AI
/// finding can point at a line inside a large contiguous block of new code,
/// where the *actual* diff hunk covering it spans the whole block.
/// Reconstructing that whole hunk would defeat the point of showing "the
/// lines this finding is about."
const AI_FINDING_HUNK_CONTEXT_LINES: usize = 6;

/// Provenance for one AI Review pass: which harness and model produced it, and
/// the run's token usage and estimated cost. Captured from the headless run
/// ([`run_ai_pr_review`]), persisted with the findings ([`AiReviewCacheEntry`]),
/// and surfaced everywhere the review appears — the in-app pane and the posted
/// GitHub comment — so a review carries one consistent attribution instead of
/// the bare "— AI review via AMF" marker.
///
/// Every field past the harness is best-effort: a harness that reports no
/// usage degrades to model-only attribution rather than showing a fabricated
/// `$0.00`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AiReviewAttribution {
    /// Display name of the harness that ran the review. Always set for a run
    /// AMF dispatched; `None` only for a legacy cache row written before
    /// attribution existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    /// Model the run used; `None` means the harness's default model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Tokens served from a provider cache. This stays distinct from ordinary
    /// input: some harnesses report it while others do not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    /// Provider-reported total for the run. It is deliberately not derived
    /// from component counts, because providers disagree about which token
    /// categories a total includes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    /// Preformatted USD cost (`token_tracking::format_token_cost`, so the same
    /// configured rates and rounding as AMF's usage meters), or `None` when
    /// the harness reported no usage to price.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_cost: Option<String>,
    /// Wall-clock duration of the completed review, retained in milliseconds
    /// so the persisted representation is harness-neutral and serde-friendly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

impl AiReviewAttribution {
    /// Build from a completed run. `usage` is the harness's last reported
    /// `(input, output)` token counts, absent when it reported none.
    pub fn from_run(
        harness: &AgentKind,
        model: Option<&str>,
        usage: Option<&crate::headless::HeadlessUsage>,
        pricing: &crate::token_tracking::TokenPricingConfig,
        elapsed: std::time::Duration,
    ) -> Self {
        // The configured rates price input/output/cache categories, but a
        // partial provider report would turn unknown categories into a fake
        // zero. Price only a report with both ordinary input and output.
        let estimated_cost = usage.and_then(|usage| {
            let (Some(input_tokens), Some(output_tokens)) =
                (usage.input_tokens, usage.output_tokens)
            else {
                return None;
            };
            Some(crate::token_tracking::format_token_cost(
                &session_usage_from_counts(
                    input_tokens,
                    output_tokens,
                    usage.cached_tokens.unwrap_or(0),
                    // Anthropic's `cache_read_input_tokens` is a separate,
                    // additive billing category on top of `input_tokens`
                    // (priced that way in `token_tracking.rs`). The generic
                    // fallback keys `emit_usage_from` also populates
                    // `cached_tokens` from (`cached_input_tokens`,
                    // `cache_read`, `cached`) commonly report a figure
                    // that's already a *subset* of `input_tokens` for other
                    // providers, so folding it in again would double-count
                    // both the total and the cost.
                    matches!(harness, AgentKind::Claude),
                ),
                pricing,
            ))
        });
        Self {
            harness: Some(harness.display_name().to_string()),
            model: model.map(str::to_string),
            input_tokens: usage.and_then(|usage| usage.input_tokens),
            output_tokens: usage.and_then(|usage| usage.output_tokens),
            cached_tokens: usage.and_then(|usage| usage.cached_tokens),
            total_tokens: usage.and_then(|usage| usage.total_tokens),
            estimated_cost,
            elapsed_ms: Some(elapsed.as_millis().min(u128::from(u64::MAX)) as u64),
        }
    }

    /// Whether any token usage was reported for this run.
    pub fn has_usage(&self) -> bool {
        self.input_tokens.is_some()
            || self.output_tokens.is_some()
            || self.cached_tokens.is_some()
            || self.total_tokens.is_some()
    }

    fn model_label(&self) -> &str {
        self.model.as_deref().unwrap_or("harness default")
    }

    /// `harness claude · model sonnet · ~12.3k in / ~4.5k out · est. $0.08`,
    /// dropping the token clause when usage is unknown and the cost clause when
    /// it could not be priced. Plain text — used by the in-app pane and, inside
    /// [`Self::disclosure_line`], the posted comment.
    pub fn plain_label(&self) -> String {
        let mut parts = vec![
            format!(
                "harness {}",
                self.harness.as_deref().unwrap_or("unreported")
            ),
            format!("model {}", self.model_label()),
        ];
        if let (Some(input), Some(output)) = (self.input_tokens, self.output_tokens) {
            parts.push(format!(
                "~{} in / ~{} out",
                crate::token_tracking::format_token_count(input),
                crate::token_tracking::format_token_count(output)
            ));
        }
        if let Some(cost) = &self.estimated_cost {
            parts.push(format!("est. {cost}"));
        }
        parts.join(" · ")
    }

    /// Deterministic Markdown appended only to the overall GitHub review
    /// body. Each metric is independently reported or called unavailable, so
    /// a partial harness event can never masquerade as a zero-token run.
    pub fn usage_summary(&self) -> String {
        let token = |value: Option<u64>| {
            value
                .map(crate::token_tracking::format_token_count)
                .unwrap_or_else(|| "unavailable".to_string())
        };
        let elapsed = self
            .elapsed_ms
            .map(format_elapsed)
            .unwrap_or_else(|| "unavailable".to_string());
        format!(
            "### AI review usage\n\
             - Harness: {}\n\
             - Model: {}\n\
             - Elapsed: {elapsed}\n\
             - Input tokens: {}\n\
             - Output tokens: {}\n\
             - Cached tokens: {}\n\
             - Total tokens: {}\n\
             - Estimated cost: {}",
            self.harness.as_deref().unwrap_or("unavailable"),
            self.model_label(),
            token(self.input_tokens),
            token(self.output_tokens),
            token(self.cached_tokens),
            token(self.total_tokens),
            self.estimated_cost.as_deref().unwrap_or("unavailable"),
        )
    }
}

fn format_elapsed(elapsed_ms: u64) -> String {
    let seconds = elapsed_ms / 1_000;
    format!("{}m {:02}s", seconds / 60, seconds % 60)
}

/// `cached_is_additive` distinguishes Anthropic's `cache_read_input_tokens`
/// (a separate count on top of `input_tokens`) from other providers' cache
/// figures, which are commonly already a subset of `input_tokens`; only in
/// the former case does folding `cached_tokens` into `cache_read_tokens` and
/// `total_tokens` avoid double-counting them.
fn session_usage_from_counts(
    input_tokens: u64,
    output_tokens: u64,
    cached_tokens: u64,
    cached_is_additive: bool,
) -> crate::token_tracking::SessionTokenUsage {
    let additive_cached = if cached_is_additive { cached_tokens } else { 0 };
    crate::token_tracking::SessionTokenUsage {
        // Only the token counts feed `format_token_cost`; the source label is
        // never read for pricing.
        source: crate::token_tracking::TokenUsageSource {
            provider: crate::token_tracking::TokenUsageProvider::Claude,
            id: "ai-review".to_string(),
        },
        input_tokens,
        output_tokens,
        cache_read_tokens: additive_cached,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: input_tokens
            .saturating_add(output_tokens)
            .saturating_add(additive_cached),
    }
}

/// Attribution for an AI review finding or summary posted to GitHub: the
/// stable `— AI review via AMF` marker, preceded by a
/// [`AiReviewAttribution::usage_summary`] (model, elapsed time, tokens, and
/// estimated cost) when a run's provenance is available. Legacy callers with
/// no attribution still get the bare marker.
fn append_ai_review_attribution(body: &str, attribution: Option<&AiReviewAttribution>) -> String {
    match attribution {
        Some(attribution) => format!(
            "{}\n\n{}\n\n{}",
            body.trim_end(),
            attribution.usage_summary(),
            super::pr_review::AI_REVIEW_ATTRIBUTION_FOOTER
        ),
        None => format!(
            "{}\n\n{}",
            body.trim_end(),
            super::pr_review::AI_REVIEW_ATTRIBUTION_FOOTER
        ),
    }
}

/// Guarantee the attribution survives into the posted body even though the
/// confirm dialog's summary editor is free-form text the user can edit —
/// including deleting the disclosure line and footer [`build_ai_review`]
/// seeded it with. Called right before [`GhCli::create_review`] rather than
/// trusted from dialog build time, so an edited-out attribution is restored
/// instead of silently publishing an unattributed review. Any existing
/// trailing marker (and a recognized disclosure line above it) is stripped
/// first so the attribution is never doubled.
fn ensure_ai_review_attribution(body: &str, attribution: Option<&AiReviewAttribution>) -> String {
    append_ai_review_attribution(strip_ai_review_attribution(body), attribution)
}

/// Remove a trailing [`AI_REVIEW_ATTRIBUTION_FOOTER`] and, if present directly
/// above it, a disclosure line matching `attribution` — leaving the editable
/// core body.
fn strip_ai_review_attribution(body: &str) -> &str {
    let footer = super::pr_review::AI_REVIEW_ATTRIBUTION_FOOTER;
    let trimmed = body.trim_end();
    let core = trimmed.strip_suffix(footer).map_or(trimmed, str::trim_end);
    // The usage block is always its own trailing paragraph, separated by a
    // blank line (see `append_ai_review_attribution`), so only a heading that
    // actually *starts* that last paragraph counts as the real block — not
    // one a finding's own text merely mentions or quotes somewhere earlier.
    // Mirrors the previous line-anchored disclosure check, one level up
    // (paragraph instead of line).
    match core.rsplit_once("\n\n") {
        Some((head, last)) if last.trim_start().starts_with("### AI review usage") => {
            head.trim_end()
        }
        _ if core.trim_start().starts_with("### AI review usage") => "",
        _ => core,
    }
}

/// Soft ceiling on the AI review's assembled prompt (diff + memory doc +
/// instructions): past this, a warning toast fires once the token estimate is
/// known, but the review still runs — chunking or an outright refusal isn't
/// worth the complexity until real use shows it's needed.
const AI_REVIEW_PROMPT_TOKEN_WARN: usize = 40_000;

/// One AI-review finding: parsed from the agent's fixed-format output
/// ([`parse_ai_findings`]), then kept/skipped/edited in the AI Review pane
/// before an optional `W` post. `path` is `None` for the `### General` bucket;
/// `line` is `None` whenever a finding has no validated single-line anchor. An
/// anchored finding's `line` is one-based in the source-file coordinate system named by `side`.
/// `side == None` is accepted only while parsing legacy `path:line` output;
/// validation either resolves it unambiguously to a side or removes the line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiReviewFinding {
    pub path: Option<String>,
    pub line: Option<u32>,
    #[serde(default)]
    pub side: Option<crate::diff::DiffSide>,
    /// Editable before posting (`e` in the pane).
    pub body: String,
    /// The hunk (from the PR diff) covering `path:line`, matching the shape
    /// GitHub's API hands over for free on a real review comment (`@@ ... @@`
    /// header + body). Unlike a GitHub comment, nothing about *generating* a
    /// finding produces this — it's reconstructed after parsing by
    /// re-matching `path:line` back into the already-fetched PR diff
    /// ([`diff_hunk_for_location`]). `None` when there's no anchor, or the line
    /// couldn't be matched to a hunk.
    pub diff_hunk: Option<String>,
    /// Excluded from `W` (post) without discarding it — the user can un-skip.
    pub skipped: bool,
    /// Already posted to GitHub via `W`; blocks a second post of the same
    /// finding and shows a `[posted]` chip. Following up on a published
    /// finding (mark done, reply, resolve, inject fix) happens back in PR
    /// Triage once a refresh picks it up as an ordinary fetched comment —
    /// this pane deliberately doesn't reconcile GitHub identities onto
    /// itself (that reconciliation machinery was most of the friction the
    /// module doc above describes).
    pub published: bool,
}

/// Cache key for [`App::ai_review_finding_fix_costs`]. The per-frame result
/// only changes when the open pane's PR identity or the anchors of its
/// findings change, so the (triage DB load + full cached-review JSON parse +
/// a sibling query per correlated finding) is memoized against this rather
/// than recomputed on every render tick. Cleared outright whenever the pane
/// is (re)opened or a new `A` run lands, so a fix cost attributed in PR
/// Triage between visits is still picked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AiReviewFixCostKey {
    pub pr_number: u32,
    pub head_sha: String,
    pub anchors: Vec<(Option<String>, Option<u32>, Option<crate::diff::DiffSide>)>,
}

impl AiReviewFixCostKey {
    fn for_state(state: &crate::app::AiReviewState) -> Self {
        Self {
            pr_number: state.pr.number,
            head_sha: state.pr.head_sha.clone(),
            anchors: state
                .findings
                .iter()
                .map(|f| (f.path.clone(), f.line, f.side))
                .collect(),
        }
    }
}

/// Progress of the background AI PR review (`A`): the one headless agent pass
/// over the PR diff. `Reviewing` fires once with a token estimate right
/// before the paid call; structured harness activity and usage may follow;
/// `Done` fires exactly once at the end.
pub enum AiReviewProgress {
    Reviewing {
        token_estimate: usize,
    },
    Activity(String),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
    Done(Result<AiReviewOutcome>),
}

/// Successful result of one AI PR review pass: the parsed findings plus the
/// agent's raw response text. The raw text isn't shown in the UI — it exists
/// so [`App::poll_ai_pr_review_bg`] can write it to the debug log, since a
/// model that doesn't follow the fixed-format instruction produces zero
/// parsed findings with no other visible signal that anything went wrong (vs.
/// a genuinely clean diff, which also parses to zero findings).
#[derive(Debug)]
pub struct AiReviewOutcome {
    pub findings: Vec<AiReviewFinding>,
    /// One-to-three sentence overview produced in the same agent pass as the
    /// findings. New runs require this to be `Some`; `None` remains supported
    /// for legacy cache entries created before summary validation.
    pub summary: Option<String>,
    pub raw_output: String,
    /// Which harness/model produced this pass, and what it cost. Filled in by
    /// [`run_ai_pr_review`] after the run; [`process_ai_review_output`] leaves
    /// it at its default since it only sees the response text.
    pub attribution: AiReviewAttribution,
}

/// Record of the most recent `A` run, persisted alongside the findings in
/// `ai_review_cache` so the outcome survives leaving the pane, closing AMF,
/// or a same-head-SHA cache-hit reopen. Without this, a review that already
/// ran (found nothing, or errored) looks identical to one that never ran.
/// Cleared implicitly on a new head SHA — a fresh [`App::open_ai_review_for_pr`]
/// against a new SHA finds no cache row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiReviewRun {
    pub ran_at: DateTime<Local>,
    pub outcome: AiReviewRunOutcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum AiReviewRunOutcome {
    Findings(usize),
    Error(String),
}

/// Presentation status for the AI Review badge in PR Triage. The running
/// variant comes from the live worker; terminal variants come from the exact
/// PR-number/head-SHA cache entry retained on [`PrReviewState`].
#[derive(Debug, Clone, PartialEq)]
pub enum AiReviewTriageStatus {
    NotRun,
    Running,
    Pending(usize),
    NoFindings(AiReviewRun),
    Failed(AiReviewRun),
    /// A successful run found findings, but none remain publishable (for
    /// example because they were skipped or posted). PR Triage shows no badge.
    CompletedWithFindings,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct AiReviewTriageSnapshot {
    pub(crate) pending_findings: usize,
    pub(crate) last_run: Option<AiReviewRun>,
}

/// Stage of the AI PR review's full-screen running view. Mirrors the
/// review-memory lookback bootstrap's `BootstrapStage`: a cheap prep stage,
/// then the one paid pass with a token estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiReviewStage {
    /// Fetching the PR diff and reading the review-memory doc (`gh` + a local
    /// file read — zero agent tokens, but a large diff fetch can take a
    /// moment).
    PreparingDiff,
    Reviewing {
        token_estimate: usize,
    },
}

/// What's persisted in `ai_review_cache`, keyed by `PR# + head SHA`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AiReviewCacheEntry {
    pub findings: Vec<AiReviewFinding>,
    pub last_run: Option<AiReviewRun>,
    /// Overall review summary generated alongside `findings`. The default
    /// keeps cache rows written before this field was introduced readable.
    #[serde(default)]
    pub summary: Option<String>,
    /// Harness/model/token/cost provenance of the run that produced
    /// `findings`. `None` for cache rows written before attribution existed,
    /// or for a run whose latest outcome was an error.
    #[serde(default)]
    pub attribution: Option<AiReviewAttribution>,
}

impl AiReviewCacheEntry {
    /// Number of findings that `W` can still publish. A failed latest run is
    /// deliberately not pending even if an older draft set remains cached.
    pub fn publishable_finding_count(&self) -> usize {
        if !matches!(
            self.last_run.as_ref().map(|run| &run.outcome),
            Some(AiReviewRunOutcome::Findings(_))
        ) {
            return 0;
        }
        self.findings
            .iter()
            .filter(|finding| !finding.skipped && !finding.published)
            .count()
    }
}

/// Assemble the AI code-review prompt: the PR diff, the review-memory doc's
/// content as context (so the agent checks the team's known recurring
/// findings first), and a fixed-format output instruction so the response
/// round-trips through [`parse_ai_findings`] with no further parsing.
///
/// `skill`, from `AppConfig::ai_review_skill`, is a Claude Code skill/command
/// name (no leading `/`, e.g. `"review"`) to run first as the primary review
/// methodology when set. AMF ships no bundled skill for reviewing a PR diff
/// itself, so this lets a user with a richer installed review skill (the
/// built-in `/review`, or a marketplace one) supply the review judgment while
/// AMF still owns parsing the findings back out via the fixed-format
/// instruction that follows. `None` (the default) skips straight to AMF's own
/// review instructions.
/// The full built-in AI PR-review prompt. Production goes through the
/// resolver ([`run_ai_pr_review`] renders the resolved template); this keeps
/// the whole-prompt assembly available to tests.
#[allow(dead_code)]
pub fn ai_review_prompt(diff: &str, memory: &str, skill: Option<&str>) -> String {
    crate::prompts::render_template(
        crate::prompts::PromptId::PrReviewAiReview
            .spec()
            .default_template,
        &ai_review_prompt_context(diff, memory, skill),
    )
}

/// The `{{token}}` context for [`crate::prompts::PromptId::PrReviewAiReview`].
/// `skill_directive` and `recurring_findings` are empty unless a review skill
/// or a non-empty review-memory doc is configured.
pub fn ai_review_prompt_context(
    diff: &str,
    memory: &str,
    skill: Option<&str>,
) -> crate::prompts::PromptContext {
    let skill_directive = match skill {
        Some(skill) => format!(
            "First, use the /{skill} skill/command to review the pull request diff below as \
             your primary review methodology.\n\n"
        ),
        None => String::new(),
    };
    let recurring_findings = if memory.trim().is_empty() {
        String::new()
    } else {
        // Deliberately not "for this project": `memory` may merge the repo's
        // own doc with the user's cross-project one, each labeled inside.
        format!(
            "Known recurring findings to check for:\n{}\n\n",
            memory.trim()
        )
    };
    crate::prompts::PromptContext::new()
        .with("skill_directive", skill_directive)
        .with("recurring_findings", recurring_findings)
        .with("annotated_diff", annotated_diff_for_ai_review(diff))
        .with("finding_heading_prefix", AI_FINDING_HEADING_PREFIX)
}

/// Render a parsed unified diff with an explicit source coordinate on every
/// addressable row. Context rows show both coordinates but lead with RIGHT so
/// findings naturally target the current file; removals expose LEFT only and
/// additions RIGHT only. If parsing fails, retain the raw diff so the review
/// can still run, while response validation will conservatively downgrade any
/// location that cannot be resolved.
fn annotated_diff_for_ai_review(diff: &str) -> String {
    use crate::diff::{DiffLineKind, DiffSide, line_locations_in_hunk};

    let Ok(files) = crate::diff::parse_unified_diff(diff) else {
        return diff.trim_end().to_string();
    };
    let mut out = String::new();
    for file in files {
        out.push_str(&format!("File: {}\n", file.path));
        for hunk in &file.hunks {
            out.push_str(&hunk.header);
            out.push('\n');
            for (line, location) in hunk.lines.iter().zip(line_locations_in_hunk(hunk)) {
                let label = match (line.kind.clone(), location) {
                    (DiffLineKind::Context, Some(location)) => format!(
                        "RIGHT:{} LEFT:{}",
                        location
                            .line_on(DiffSide::New)
                            .expect("context has new line"),
                        location
                            .line_on(DiffSide::Old)
                            .expect("context has old line")
                    ),
                    (DiffLineKind::Added, Some(location)) => format!(
                        "RIGHT:{}",
                        location
                            .line_on(DiffSide::New)
                            .expect("addition has new line")
                    ),
                    (DiffLineKind::Removed, Some(location)) => format!(
                        "LEFT:{}",
                        location
                            .line_on(DiffSide::Old)
                            .expect("removal has old line")
                    ),
                    (DiffLineKind::NoNewlineMarker, None) => "MARKER".to_string(),
                    _ => continue,
                };
                out.push_str(&format!("[{label}] {}\n", line.text));
            }
        }
        out.push('\n');
    }
    out.trim_end().to_string()
}

/// If `output` is entirely wrapped in one fenced code block (a common way
/// models "helpfully" package a response even when told not to add anything
/// outside the requested format, e.g. ` ```markdown ... ``` `), strip the
/// fence and return the inner text. Otherwise returns `output` unchanged.
fn strip_outer_code_fence(output: &str) -> &str {
    let trimmed = output.trim();
    let Some(after_open) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let Some(close_at) = after_open.rfind("```") else {
        return trimmed;
    };
    // Skip an optional language tag on the opening fence line (```markdown).
    let body_start = after_open.find('\n').map_or(0, |i| i + 1);
    if body_start > close_at {
        return trimmed;
    }
    after_open[body_start..close_at].trim()
}

/// Recognize a finding heading: 1-4 leading `#` characters followed by
/// whitespace and the heading text. [`ai_review_prompt`] asks for exactly
/// [`AI_FINDING_HEADING_PREFIX`] (`###`), but models don't reliably hold a
/// specific heading level (`##`/`#` are common substitutions), so the parser
/// accepts any small heading level rather than silently dropping every
/// finding over that one mismatch.
fn strip_finding_heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|&c| c == '#').count();
    if !(1..=4).contains(&hashes) {
        return None;
    }
    let rest = trimmed[hashes..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

/// Parse the AI reviewer's fixed-format output ([`ai_review_prompt`]) into its
/// overall summary and findings. Tolerant of common formatting drift: an
/// outer code fence around the whole response is stripped first
/// ([`strip_outer_code_fence`]), and any small markdown heading level starts a
/// section ([`strip_finding_heading`], not just the requested `###`). A
/// case-insensitive `Summary` section is separated from findings; a
/// `path|RIGHT|line` / `path|LEFT|line` finding heading is explicitly anchored.
/// Legacy `path:line` remains parseable with an unspecified side for safe
/// migration; the validation pass must resolve it unambiguously. Anything else
/// (`General`, malformed) stays pathless. Empty sections are dropped rather
/// than erroring, so a partially-malformed response still yields whatever
/// content did parse.
fn parse_ai_review_output(output: &str) -> (Option<String>, Vec<AiReviewFinding>) {
    fn flush(
        current: Option<(&str, Vec<&str>)>,
        summary: &mut Option<String>,
        out: &mut Vec<AiReviewFinding>,
    ) {
        let Some((heading, lines)) = current else {
            return;
        };
        let body = lines.join("\n").trim().to_string();
        if body.is_empty() {
            return;
        }
        if heading.trim().eq_ignore_ascii_case("summary") {
            if summary.is_none() {
                *summary = Some(body);
            }
            return;
        }
        let heading = heading.trim();
        let explicit = heading.rsplit_once('|').and_then(|(path_and_side, line)| {
            let (path, side) = path_and_side.rsplit_once('|')?;
            let side = match side.trim().to_ascii_uppercase().as_str() {
                "LEFT" => crate::diff::DiffSide::Old,
                "RIGHT" => crate::diff::DiffSide::New,
                _ => return None,
            };
            let line = line.trim().parse::<u32>().ok()?;
            (!path.is_empty()).then(|| (path.to_string(), line, side))
        });
        let (path, line, side) = match explicit {
            Some((path, line, side)) => (Some(path), Some(line), Some(side)),
            None => match heading.rsplit_once(':') {
                Some((path, line)) if !path.is_empty() => match line.trim().parse::<u32>() {
                    Ok(line) => (Some(path.to_string()), Some(line), None),
                    Err(_) => (None, None, None),
                },
                _ => (None, None, None),
            },
        };
        out.push(AiReviewFinding {
            path,
            line,
            side,
            body,
            diff_hunk: None,
            skipped: false,
            published: false,
        });
    }

    let output = strip_outer_code_fence(output);
    let mut summary = None;
    let mut findings = Vec::new();
    let mut current: Option<(&str, Vec<&str>)> = None;
    for raw_line in output.lines() {
        match strip_finding_heading(raw_line) {
            Some(heading) => {
                flush(current.take(), &mut summary, &mut findings);
                current = Some((heading, Vec::new()));
            }
            None => {
                if let Some((_, lines)) = current.as_mut() {
                    lines.push(raw_line);
                }
            }
        }
    }
    flush(current, &mut summary, &mut findings);
    (summary, findings)
}

#[cfg(test)]
pub fn parse_ai_findings(output: &str) -> Vec<AiReviewFinding> {
    parse_ai_review_output(output).1
}

/// Resolve a requested source coordinate through the canonical unified-diff
/// row map. Legacy requests without a side are accepted only when the number
/// identifies one row unambiguously across both sides; if the same integer
/// names different old/new rows, guessing would recreate the original bug.
fn resolve_ai_review_location(
    file: &crate::diff::DiffFile,
    line: u32,
    side: Option<crate::diff::DiffSide>,
) -> Option<(crate::diff::DiffSide, crate::diff::DiffLineLocation)> {
    use crate::diff::DiffSide;

    let line = line as usize;
    if let Some(side) = side {
        return file
            .resolve_source_line(side, line)
            .map(|location| (side, location));
    }

    let old = file.resolve_source_line(DiffSide::Old, line);
    let new = file.resolve_source_line(DiffSide::New, line);
    match (old, new) {
        (Some(old), Some(new)) if old == new => Some((DiffSide::New, new)),
        (Some(_), Some(_)) => None,
        (Some(old), None) => Some((DiffSide::Old, old)),
        (None, Some(new)) => Some((DiffSide::New, new)),
        (None, None) => None,
    }
}

/// Reconstruct a GitHub-style `diff_hunk` string (the `@@ ... @@` header plus
/// a small window around an already-resolved source coordinate).
fn diff_hunk_for_location(
    files: &[crate::diff::DiffFile],
    path: &str,
    side: crate::diff::DiffSide,
    location: crate::diff::DiffLineLocation,
) -> Option<String> {
    let file = files.iter().find(|f| f.path == path)?;
    let hunk = file.hunk_for_location(location)?;
    let line = location.line_on(side)?;

    super::pr_review::window_parsed_hunk(
        hunk,
        line,
        side == crate::diff::DiffSide::Old,
        AI_FINDING_HUNK_CONTEXT_LINES,
    )
}

/// Build the `(summary, inline comments)` GitHub review payload from a set of
/// kept findings (`W` — "post as GitHub review"). A finding with a
/// `path`+`line` anchor *and* a matched `diff_hunk` becomes an inline
/// [`GhPrReviewComment`]; everything else (pathless `General` findings, a
/// path-only file-level one, or an anchored one whose line never matched the
/// diff) folds into the summary body as a bullet instead, so nothing is
/// silently dropped. Inline comments carry their own attribution footer since
/// they can surface on their own (e.g. the Files-changed view) without the
/// review summary in sight; the summary already self-identifies.
///
/// `attribution`, when present, adds the model/token/cost disclosure line
/// above the stable marker on both the summary and every inline comment.
fn build_ai_review(
    findings: &[&AiReviewFinding],
    generated_summary: Option<&str>,
    attribution: Option<&AiReviewAttribution>,
) -> (String, Vec<GhPrReviewComment>) {
    let mut inline = Vec::new();
    let mut general = Vec::new();
    for f in findings {
        match (&f.path, f.side, f.line) {
            // `diff_hunk.is_some()` proves the side-aware source coordinate
            // resolved through the same parsed diff that produced the prompt.
            // Legacy cache rows have no side and invalid/stale coordinates have
            // no hunk, so both conservatively fold into the summary instead of
            // risking a wrong or GitHub-rejected inline anchor.
            (Some(path), Some(side), Some(line)) if f.diff_hunk.is_some() => {
                inline.push(GhPrReviewComment {
                    path: path.clone(),
                    line,
                    side: match side {
                        crate::diff::DiffSide::Old => "LEFT",
                        crate::diff::DiffSide::New => "RIGHT",
                    },
                    start_line: None,
                    start_side: None,
                    // Usage belongs to the one overall review body, never an
                    // inline finding. Keep the generic AMF marker only.
                    body: append_ai_review_attribution(&f.body, None),
                })
            }
            (Some(path), _, _) => general.push(format!("- **{path}**: {}", f.body)),
            (None, _, _) => general.push(format!("- {}", f.body)),
        }
    }

    let generated_summary = generated_summary.filter(|summary| !summary.trim().is_empty());
    let mut body = generated_summary
        .map(|summary| summary.trim().to_string())
        .unwrap_or_else(|| "AI review, via AMF.".to_string());
    if !general.is_empty() {
        body.push_str("\n\n");
        body.push_str(&general.join("\n"));
    }
    if generated_summary.is_some() {
        body = append_ai_review_attribution(&body, attribution);
    }
    (body, inline)
}

/// Rows offered by the AI-review model picker for a given harness: `Default`
/// and `Custom` always appear; presets are a best-effort, *verified* set of
/// model aliases — currently only Claude's, confirmed against `claude
/// --help` ("Provide an alias for the latest model (e.g. 'fable', 'opus', or
/// 'sonnet')"; `haiku` is the fourth well-known tier). Other harnesses don't
/// have a reliably enumerable alias list, so guessing would risk offering a
/// preset that doesn't exist — `Custom` covers them instead.
fn model_pick_rows(harness: &AgentKind) -> Vec<ModelPickRow> {
    let mut rows = vec![ModelPickRow::Default];
    if *harness == AgentKind::Claude {
        rows.extend([
            ModelPickRow::Preset("sonnet"),
            ModelPickRow::Preset("opus"),
            ModelPickRow::Preset("haiku"),
            ModelPickRow::Preset("fable"),
        ]);
    }
    rows.push(ModelPickRow::Custom);
    rows
}

fn model_for_ai_review_run(
    picked: Option<&str>,
    model_picked: bool,
    configured: Option<&str>,
) -> Option<String> {
    if model_picked {
        // `None` is an explicit "Default" choice after the picker has run.
        picked.map(str::to_string)
    } else {
        picked.or(configured).map(str::to_string)
    }
}

/// Validate and enrich a successful harness response. A non-empty Summary is
/// the explicit proof that the model completed the requested response shape;
/// without it, zero parsed findings cannot safely mean a clean review.
fn process_ai_review_output(output: String, diff: &str) -> Result<AiReviewOutcome> {
    let (summary, mut findings) = parse_ai_review_output(&output);
    if summary.is_none() {
        anyhow::bail!("AI review returned malformed output: missing a non-empty Summary section");
    }
    // Validate every requested coordinate against the exact parsed row map.
    // Invalid or ambiguous requests retain their file and prose, but lose the
    // misleading line target and are therefore rendered/posted as file-level
    let files = crate::diff::parse_unified_diff(diff).ok();
    for finding in &mut findings {
        let resolved = match (&finding.path, finding.line, files.as_deref()) {
            (Some(path), Some(line), Some(files)) => files
                .iter()
                .find(|file| file.path == *path)
                .and_then(|file| resolve_ai_review_location(file, line, finding.side)),
            _ => None,
        };
        if let (Some(path), Some((side, location)), Some(files)) =
            (&finding.path, resolved, files.as_deref())
        {
            finding.side = Some(side);
            finding.diff_hunk = diff_hunk_for_location(files, path, side, location);
            if finding.diff_hunk.is_none() {
                finding.line = None;
                finding.side = None;
            }
        } else if finding.path.is_some() && finding.line.is_some() {
            finding.line = None;
            finding.side = None;
            finding.diff_hunk = None;
        }
    }
    Ok(AiReviewOutcome {
        findings,
        summary,
        raw_output: output,
        attribution: AiReviewAttribution::default(),
    })
}

/// Background body of the AI PR review (`A`): assemble the prompt from
/// `diff` + `memory` (+ optional `skill`), report a token estimate, then make
/// **one** headless agent pass and parse its response into findings. Runs off
/// the UI thread; progress and the final result are reported over `tx`.
/// `model`, when set (`AppConfig::review_model_for(ReviewAction::PrReview)`),
/// picks the review's model independent of whichever model the feature's
/// interactive session runs.
#[allow(clippy::too_many_arguments)]
fn run_ai_pr_review(
    harness: AgentKind,
    workdir: PathBuf,
    diff: String,
    memory: String,
    skill: Option<String>,
    model: Option<String>,
    pricing: crate::token_tracking::TokenPricingConfig,
    // The `pr_review.ai_review` template, resolved on the UI thread (built-in
    // default or a feature/project/global override). The diff is only fetched
    // here on the worker thread, so the prompt is rendered here.
    template: String,
    tx: std::sync::mpsc::Sender<AiReviewProgress>,
) {
    let prompt = crate::prompts::render_template(
        &template,
        &ai_review_prompt_context(&diff, &memory, skill.as_deref()),
    );
    let _ = tx.send(AiReviewProgress::Reviewing {
        token_estimate: super::pr_review::estimate_tokens(&prompt),
    });

    // The harness reports usage as one or more `Usage` events during the run;
    // the last one is the run total. Recorded here as well as forwarded so the
    // completed `AiReviewOutcome` carries the model/token/cost attribution,
    // not just the transient running screen.
    let started_at = std::time::Instant::now();
    let last_usage: std::sync::Arc<std::sync::Mutex<Option<crate::headless::HeadlessUsage>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let usage_sink = std::sync::Arc::clone(&last_usage);
    let progress_tx = tx.clone();
    let result = HeadlessRunner::run_with_progress(
        &harness,
        &workdir,
        &prompt,
        model.as_deref(),
        move |progress| {
            let progress = match progress {
                crate::headless::HeadlessProgress::Activity(message) => {
                    AiReviewProgress::Activity(message)
                }
                crate::headless::HeadlessProgress::Usage(usage) => {
                    if let Ok(mut slot) = usage_sink.lock() {
                        *slot = Some(usage.clone());
                    }
                    AiReviewProgress::Usage {
                        input_tokens: usage.input_tokens.unwrap_or(0),
                        output_tokens: usage.output_tokens.unwrap_or(0),
                    }
                }
            };
            let _ = progress_tx.send(progress);
        },
    )
    .and_then(|output| process_ai_review_output(output, &diff))
    .map(|mut outcome| {
        let usage = last_usage.lock().ok().and_then(|slot| slot.clone());
        outcome.attribution = AiReviewAttribution::from_run(
            &harness,
            model.as_deref(),
            usage.as_ref(),
            &pricing,
            started_at.elapsed(),
        );
        outcome
    });
    let _ = tx.send(AiReviewProgress::Done(result));
}

fn pr_review_state_matches_refresh(state: &PrReviewState, workdir: &Path, pr_number: u32) -> bool {
    state.workdir.as_path() == workdir && state.review.pr.number == pr_number
}

/// Replace only the network-backed review snapshot while retaining the
/// pane-local workflow state (filters, fix target, marks, dialogs, token
/// baselines, and return navigation). Keep the same selected comment when it
/// still exists in the refreshed response.
fn apply_refreshed_pr_review_state(
    state: &mut PrReviewState,
    review: crate::app::pr_review::PrReview,
    pending_ai_review_findings: usize,
    ai_review_last_run: Option<AiReviewRun>,
    checked_out_branch: Option<String>,
) {
    let selected_id = state.selected_comment().map(|comment| comment.id);
    let old_selected = state.selected;
    state.review = review;
    state.pending_ai_review_findings = pending_ai_review_findings;
    state.ai_review_last_run = ai_review_last_run;
    state.checked_out_branch = checked_out_branch;
    state.marked.retain(|id| {
        state
            .review
            .comments
            .iter()
            .any(|comment| comment.id == *id)
    });
    if let Some(index) = selected_id.and_then(|id| {
        state
            .review
            .comments
            .iter()
            .position(|comment| comment.id == id)
    }) {
        state.selected = index;
    } else {
        state.selected = old_selected.min(state.review.comments.len().saturating_sub(1));
        state.detail_scroll = 0;
    }
    // The restored (or fallback) selection can land on a row `hide_resolved`
    // now excludes — e.g. the selected comment's thread was resolved on
    // GitHub since the last fetch — so re-apply the same filter this pane
    // already snaps to on an explicit `x` toggle.
    state.snap_selection_to_visible();
}

impl App {
    /// Open the AI Review pane for `pr` in `workdir`, resetting any stashed
    /// return-to from a previous visit. Loads the cached findings for this
    /// PR's head SHA if any (a same-SHA reopen replays them with no token
    /// spend); otherwise starts empty — the user presses `A`. This is the
    /// shared entry used by the dashboard, the PR picker, and an agent
    /// session's leader key; [`Self::open_ai_review_from_triage`] wraps it for
    /// the PR Triage `A` key, which additionally remembers the triage pane to
    /// return to.
    pub fn open_ai_review_for_pr(&mut self, workdir: PathBuf, pr: PrRef) {
        self.ai_review_return_to = None;
        // A fix cost may have been attributed in PR Triage since this pane was
        // last open; a stale memo keyed on the same (unchanged) findings would
        // hide it. Recompute on the first render of the reopened pane.
        self.ai_review_fix_cost_cache = None;
        let cached = self.db.as_ref().and_then(|db| {
            db.load_ai_review_cache(pr.number, &pr.head_sha)
                .ok()
                .flatten()
        });
        let (findings, summary, last_run, attribution) = match cached {
            Some(entry) => (
                entry.findings,
                entry.summary,
                entry.last_run,
                entry.attribution,
            ),
            None => (Vec::new(), None, None, None),
        };
        self.mode = AppMode::AiReview(AiReviewState {
            workdir,
            pr,
            findings,
            summary,
            attribution,
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            last_run,
            harness: None,
            harness_pick: None,
            harness_pick_origin: None,
            model: None,
            model_picked: false,
            model_pick: None,
            finding_editor: None,
            post_confirm: None,
        });
    }

    /// Resolve the current feature's (or `Viewing` session's) branch to a PR
    /// and open AI Review, running the same `gh` preconditions → resolve
    /// chain PR Triage uses. Falls through to the PR picker when the branch
    /// has no auto-detectable open PR.
    pub(crate) fn open_ai_review_for_workdir(&mut self, workdir: PathBuf) {
        if let Err(e) = GhCli::check_available() {
            self.show_error(e);
            return;
        }
        if let Err(e) = GhCli::check_auth() {
            self.show_error(e);
            return;
        }
        match GhCli::resolve_pr(&workdir) {
            Ok(PrResolution::Found(pr)) => self.open_ai_review_for_pr(workdir, pr),
            Ok(PrResolution::NoPrForBranch) => self.open_pr_picker(workdir, None),
            Err(e) => self.show_error(e),
        }
    }

    /// Dashboard entry point: AI Review for the selected feature's branch.
    pub fn open_ai_review(&mut self) {
        let Some((_project, feature)) = self.selected_feature() else {
            self.message = Some("Select a feature to run an AI review".to_string());
            return;
        };
        let workdir = feature.workdir.clone();
        self.open_ai_review_for_workdir(workdir);
    }

    /// PR picker entry point: open AI Review for the highlighted PR (peer to
    /// `App::pr_picker_choose`, which opens PR Triage for it instead).
    pub fn pr_picker_choose_ai_review(&mut self) {
        let (workdir, number) = match &self.mode {
            AppMode::PrPicker(state) => match state.entries.get(state.selected) {
                Some(entry) => (state.workdir.clone(), entry.number),
                None => return,
            },
            _ => return,
        };
        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.open_ai_review_for_pr(workdir, pr),
            Err(e) => {
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.error = Some(e.to_string());
                }
            }
        }
    }

    /// Leader-key entry point from inside an agent session, peer to
    /// `open_pr_review_from_view`.
    pub fn open_ai_review_from_view(&mut self) {
        let AppMode::Viewing(view) = &self.mode else {
            return;
        };
        let Some(workdir) = self.feature_for_view(view).map(|f| f.workdir.clone()) else {
            self.message = Some("No active feature to review".to_string());
            return;
        };
        self.open_ai_review_for_workdir(workdir);
    }

    /// PR Triage's `A` key: open AI Review for the pane's already-resolved PR
    /// (no re-resolve), remembering the triage pane so closing AI Review
    /// (`esc`/`q`, with no post/generation left dangling) returns to it
    /// instead of the dashboard.
    pub fn open_ai_review_from_triage(&mut self) {
        let AppMode::PrReview(_) = &self.mode else {
            return;
        };
        let previous = std::mem::replace(&mut self.mode, AppMode::Normal);
        let AppMode::PrReview(state) = previous else {
            unreachable!()
        };
        let workdir = state.workdir.clone();
        let pr = state.review.pr.clone();
        self.open_ai_review_for_pr(workdir, pr);
        self.ai_review_return_to = Some(Box::new(AppMode::PrReview(state)));
    }

    /// Close the AI Review pane (`esc`/`q`): back to the PR Triage pane it was
    /// opened from, if any, else the dashboard. The background thread, if
    /// running, isn't aborted — [`Self::poll_ai_pr_review_bg`] still surfaces
    /// the result via [`Self::ai_review_pending`].
    pub fn close_ai_review(&mut self) {
        match self.ai_review_return_to.take() {
            Some(return_to) => self.mode = *return_to,
            None => self.mode = AppMode::Normal,
        }
    }

    /// Persist the AI Review pane and synchronize any matching PR Triage
    /// badge. Returns whether the durable cache write succeeded; most edits
    /// treat a write failure as non-fatal, while posting uses it to ensure the
    /// published marker is durable before starting the network refresh.
    fn cache_ai_review(&mut self, state: &AiReviewState) -> bool {
        let entry = AiReviewCacheEntry {
            findings: state.findings.clone(),
            last_run: state.last_run.clone(),
            summary: state.summary.clone(),
            attribution: state.attribution.clone(),
        };
        let saved = match self.db.as_ref() {
            Some(db) => {
                match db.save_ai_review_cache(state.pr.number, &state.pr.head_sha, &entry) {
                    Ok(()) => true,
                    Err(error) => {
                        self.log_warn(
                            "pr_review",
                            format!(
                                "AI Review cache write failed for PR #{}: {error}",
                                state.pr.number
                            ),
                        );
                        false
                    }
                }
            }
            None => false,
        };
        self.update_ai_review_triage_snapshot(
            &state.workdir,
            &state.pr,
            entry.publishable_finding_count(),
            entry.last_run.clone(),
        );
        saved
    }

    /// Publishable cached findings for the exact PR/head SHA. This is read
    /// when PR Triage opens so the pending badge survives pane changes and
    /// process restarts rather than depending on a live background job.
    pub(crate) fn ai_review_triage_snapshot(&self, pr: &PrRef) -> AiReviewTriageSnapshot {
        self.db
            .as_ref()
            .and_then(|db| {
                db.load_ai_review_cache(pr.number, &pr.head_sha)
                    .ok()
                    .flatten()
            })
            .map_or_else(AiReviewTriageSnapshot::default, |entry| {
                AiReviewTriageSnapshot {
                    pending_findings: entry.publishable_finding_count(),
                    last_run: entry.last_run,
                }
            })
    }

    /// Derive the PR Triage badge state with live work taking precedence over
    /// the exact cached terminal result retained by the pane.
    pub(crate) fn ai_review_triage_status(&self, state: &PrReviewState) -> AiReviewTriageStatus {
        let pr = &state.review.pr;
        let running = self.ai_review_bg.is_some()
            && self.ai_review_pending.as_ref().is_some_and(|pending| {
                pending.workdir == state.workdir
                    && pending.pr.number == pr.number
                    && pending.pr.head_sha == pr.head_sha
            });
        if running {
            return AiReviewTriageStatus::Running;
        }
        if state.pending_ai_review_findings > 0 {
            return AiReviewTriageStatus::Pending(state.pending_ai_review_findings);
        }
        match state.ai_review_last_run.clone() {
            Some(run) if matches!(run.outcome, AiReviewRunOutcome::Findings(0)) => {
                AiReviewTriageStatus::NoFindings(run)
            }
            Some(run) if matches!(run.outcome, AiReviewRunOutcome::Error(_)) => {
                AiReviewTriageStatus::Failed(run)
            }
            Some(_) => AiReviewTriageStatus::CompletedWithFindings,
            None => AiReviewTriageStatus::NotRun,
        }
    }

    /// Keep every in-memory copy of the matching PR Triage pane in sync with
    /// the durable cache. The pane may currently be visible, stashed under AI
    /// Review, or stashed while the user watches a fix session.
    fn update_ai_review_triage_snapshot(
        &mut self,
        workdir: &Path,
        pr: &PrRef,
        count: usize,
        last_run: Option<AiReviewRun>,
    ) {
        let matches = |state: &PrReviewState| {
            state.workdir == workdir
                && state.review.pr.number == pr.number
                && state.review.pr.head_sha == pr.head_sha
        };
        if let AppMode::PrReview(state) = &mut self.mode
            && matches(state)
        {
            state.pending_ai_review_findings = count;
            state.ai_review_last_run = last_run.clone();
        }
        if let Some(return_to) = self.ai_review_return_to.as_deref_mut()
            && let AppMode::PrReview(state) = return_to
            && matches(state)
        {
            state.pending_ai_review_findings = count;
            state.ai_review_last_run = last_run.clone();
        }
        if let Some(stash) = &mut self.pr_review_return
            && matches(&stash.state)
        {
            stash.state.pending_ai_review_findings = count;
            stash.state.ai_review_last_run = last_run;
        }
    }

    pub fn ai_review_select_next(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && !state.findings.is_empty()
        {
            state.selected = (state.selected + 1) % state.findings.len();
            state.detail_scroll = 0;
        }
    }

    pub fn ai_review_select_prev(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && !state.findings.is_empty()
        {
            state.selected = (state.selected + state.findings.len() - 1) % state.findings.len();
            state.detail_scroll = 0;
        }
    }

    pub fn ai_review_toggle_skip(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(finding) = state.findings.get_mut(state.selected)
        {
            finding.skipped = !finding.skipped;
        }
        if let AppMode::AiReview(state) = &self.mode {
            let state = state.clone();
            self.cache_ai_review(&state);
        }
    }

    /// For each finding in the open AI Review pane, the `Fix cost (est.): …`
    /// line to show — `Some` only when the finding correlates (by
    /// `path`/`line`/`side`) to a PR comment that was resolved as part of a
    /// combined batch. Indexed 1:1 with `state.findings`; all-`None` without a
    /// DB or a cached review to match findings against.
    ///
    /// The AI Review pane has no fix action of its own — a finding's fix cost
    /// only exists once it has been posted, picked up in PR Triage as an
    /// ordinary comment, and fixed there. This is the read-back of that.
    ///
    /// Called once per render tick from `ui::dashboard::draw`, so the actual
    /// work (a triage DB load, a full `serde_json` parse of the cached PR
    /// review, and a sibling query per correlated finding) is memoized against
    /// [`AiReviewFixCostKey`] and only recomputed when the pane's PR identity
    /// or its findings' anchors change. The cache is cleared outright on pane
    /// (re)open and whenever an `A` run lands, so a fix cost attributed back in
    /// PR Triage between visits is still picked up.
    pub(crate) fn ai_review_finding_fix_costs(&mut self) -> Vec<Option<String>> {
        let AppMode::AiReview(state) = &self.mode else {
            return Vec::new();
        };
        let key = AiReviewFixCostKey::for_state(state);
        if let Some((cached_key, cached)) = &self.ai_review_fix_cost_cache
            && *cached_key == key
        {
            return cached.clone();
        }
        let computed = self.compute_ai_review_finding_fix_costs();
        self.ai_review_fix_cost_cache = Some((key, computed.clone()));
        computed
    }

    /// The uncached body of [`Self::ai_review_finding_fix_costs`] — see that
    /// method for what the result means and why this one isn't called directly
    /// from the render path.
    fn compute_ai_review_finding_fix_costs(&self) -> Vec<Option<String>> {
        let AppMode::AiReview(state) = &self.mode else {
            return Vec::new();
        };
        let none_for_each = || vec![None; state.findings.len()];
        let Some(db) = self.db.as_ref() else {
            return none_for_each();
        };
        let triage = db
            .load_pr_comment_triage(state.pr.number)
            .unwrap_or_default();
        let review = db
            .load_pr_review_cache(state.pr.number, &state.pr.head_sha)
            .ok()
            .flatten();
        let comments: &[crate::app::pr_review::PrComment] =
            review.as_ref().map_or(&[], |r| r.comments.as_slice());

        state
            .findings
            .iter()
            .map(|finding| {
                let fpath = finding.path.as_deref()?;
                let fline = finding.line?;
                let fside = finding.side.map(|side| match side {
                    crate::diff::DiffSide::Old => "LEFT",
                    crate::diff::DiffSide::New => "RIGHT",
                });
                let comment = comments.iter().find(|c| {
                    c.path.as_deref() == Some(fpath)
                        && c.line == Some(fline)
                        && match (fside, c.side.as_deref()) {
                            (Some(a), Some(b)) => a == b,
                            _ => true,
                        }
                })?;
                let row = triage.get(&comment.id)?;
                let batch_id = row.batch_id.as_deref()?;
                // Partial-batch rule: the badge/cost is shown only for a
                // resolved sibling.
                let resolved = comment.is_resolved
                    || matches!(row.state, crate::app::pr_review::TriageState::Done);
                if !resolved {
                    return None;
                }
                let sibling_count = db
                    .pr_comment_triage_batch_siblings(state.pr.number, batch_id)
                    .map(|ids| ids.len())
                    .unwrap_or(1)
                    .max(1);
                Some(crate::app::fix_cost::fix_cost_line(
                    row.batch_fix_cost.as_deref(),
                    Some(crate::app::fix_cost::CombinedBatch { sibling_count }),
                ))
            })
            .collect()
    }

    pub fn ai_review_scroll_detail_up(&mut self, amount: usize) {
        if let AppMode::AiReview(state) = &mut self.mode {
            state.detail_scroll = state.detail_scroll.saturating_sub(amount);
        }
    }

    pub fn ai_review_scroll_detail_down(&mut self, amount: usize) {
        if let AppMode::AiReview(state) = &mut self.mode {
            let max = state.detail_content_lines.saturating_sub(1);
            state.detail_scroll = (state.detail_scroll + amount).min(max);
        }
    }

    /// Open the selected finding's body for editing (`e`).
    pub fn ai_review_edit_finding(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && state.findings.get(state.selected).is_some()
            && state.finding_editor.is_none()
        {
            let body = state.findings[state.selected].body.clone();
            state.finding_editor = Some(TextEditor::new(body));
        }
    }

    pub fn ai_review_finding_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(editor) = &mut state.finding_editor
        {
            editor.handle_key(key);
        }
    }

    /// Commit the edited body back onto the selected finding and close the
    /// editor (`Esc`/`Ctrl+Q` from the finding editor).
    pub fn ai_review_stop_edit_finding(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(editor) = state.finding_editor.take()
            && let Some(finding) = state.findings.get_mut(state.selected)
        {
            finding.body = editor.text().to_string();
        }
        if let AppMode::AiReview(state) = &self.mode {
            let state = state.clone();
            self.cache_ai_review(&state);
        }
    }

    pub fn ai_review_editing_finding(&self) -> bool {
        matches!(&self.mode, AppMode::AiReview(state) if state.finding_editor.is_some())
    }

    /// Kick off the AI PR review (`A`): resolve the review-memory doc
    /// synchronously (cheap, a local file read), then hand the PR-diff fetch
    /// and the one paid agent pass to a background thread and switch to the
    /// full-screen running view. If this pane's review is already running,
    /// reopen its preserved progress view instead of starting another pass.
    pub fn start_ai_pr_review(&mut self) {
        if self.ai_review_bg.is_some() {
            let origin = match &self.mode {
                AppMode::AiReview(state) => state.clone(),
                _ => return,
            };
            let same_run = self.ai_review_pending.as_ref().is_some_and(|pending| {
                pending.workdir == origin.workdir && pending.pr.number == origin.pr.number
            }) && self.ai_review_progress.is_some();
            if same_run {
                self.mode = AppMode::AiReviewRunning(AiReviewRunState {
                    origin,
                    progress: self.ai_review_progress.clone().expect("checked above"),
                });
            } else {
                self.push_toast_warning("Another AI review is already running");
            }
            return;
        }
        let (workdir, harness, model_picked) = match &self.mode {
            AppMode::AiReview(state) => (
                state.workdir.clone(),
                state.harness.clone(),
                state.model_picked,
            ),
            _ => return,
        };
        let Some(harness) = harness else {
            let agents = self.allowed_agents_for_project_path(&workdir);
            if agents.is_empty() {
                self.push_toast_error("No agent harnesses are enabled for this project");
                return;
            }
            let preferred = self
                .feature_indices_for_workdir(&workdir)
                .map(|(pi, _)| self.store.projects[pi].preferred_agent.clone());
            let selected = preferred
                .and_then(|p| agents.iter().position(|a| *a == p))
                .unwrap_or(0);
            if let AppMode::AiReview(state) = &mut self.mode {
                state.harness_pick = Some(AiHarnessPickState {
                    agents,
                    selected,
                    error: None,
                    previous_harness: None,
                });
            }
            return;
        };
        if !model_picked {
            let rows = model_pick_rows(&harness);
            let configured = self.config.review_model_for(ReviewAction::PrReview);
            let preset_match = configured.as_ref().and_then(|configured| {
                rows.iter().position(
                    |row| matches!(row, ModelPickRow::Preset(preset) if preset == configured),
                )
            });
            let (selected, custom_input) = match (preset_match, &configured) {
                (Some(index), _) => (index, String::new()),
                (None, Some(configured)) => (rows.len() - 1, configured.clone()),
                (None, None) => (0, String::new()),
            };
            if let AppMode::AiReview(state) = &mut self.mode {
                state.model_pick = Some(AiModelPickState {
                    rows,
                    selected,
                    custom_input,
                    editing_custom: false,
                });
            }
            return;
        }
        self.begin_ai_pr_review();
    }

    /// Start the background review after a harness has been selected and
    /// validated. Kept separate from [`Self::start_ai_pr_review`] so the
    /// picker can pause before the paid pass without duplicating lifecycle
    /// setup.
    pub(crate) fn begin_ai_pr_review(&mut self) {
        let mut origin = match &self.mode {
            AppMode::AiReview(state) => state.clone(),
            _ => return,
        };
        let Some(harness) = origin.harness.clone() else {
            return;
        };
        origin.harness_pick = None;
        origin.harness_pick_origin = None;
        origin.model_pick = None;
        origin.post_confirm = None;
        origin.finding_editor = None;

        let workdir = origin.workdir.clone();
        let number = origin.pr.number;
        // The pane's own pick (from the model picker) takes priority over the
        // `review_model_for(PrReview)` default it was seeded from — picking
        // "Default" in the picker clears it back to `None` explicitly.
        let model = model_for_ai_review_run(
            origin.model.as_deref(),
            origin.model_picked,
            self.config
                .review_model_for(ReviewAction::PrReview)
                .as_deref(),
        );
        self.log_info(
            "pr_review",
            format!(
                "starting AI review of PR #{number} with {}{}",
                harness.display_name(),
                model
                    .as_deref()
                    .map(|m| format!(" (model: {m})"))
                    .unwrap_or_default()
            ),
        );

        let repo = self.repo_for_project_path(&workdir);
        // Both docs feed the reviewer: the repo's own findings plus the
        // user's cross-project ones, with the overlap between them collapsed
        // so a promoted rule isn't paid for twice in the prompt.
        let memory_paths = self.review_memory_paths(&repo);
        let memory = review_memory::merge_memory_context(
            &std::fs::read_to_string(&memory_paths.project).unwrap_or_default(),
            &std::fs::read_to_string(&memory_paths.global).unwrap_or_default(),
        );
        let skill = self.config.ai_review_skill.clone();
        let pricing = self.config.token_pricing.clone();
        let (template, _) = self.resolve_headless_template(
            crate::prompts::PromptId::PrReviewAiReview,
            &harness,
            &repo,
            &workdir,
        );

        let preview = format!(
            "{template}\n\n[the PR #{number} diff is fetched and spliced into {{{{annotated_diff}}}} when the call runs]"
        );
        if !self.precall_gate(
            crate::app::precall::PrecallAction::PrReviewAiReview,
            &harness,
            &preview,
        ) {
            return;
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_review_bg = Some(rx);
        self.ai_review_pending = Some(origin.clone());
        let thread_workdir = workdir.clone();
        std::thread::spawn(move || match GhCli::pr_diff(&thread_workdir, number) {
            Ok(diff) => run_ai_pr_review(
                harness,
                thread_workdir,
                diff,
                memory,
                skill,
                model,
                pricing,
                template,
                tx,
            ),
            Err(e) => {
                let _ = tx.send(AiReviewProgress::Done(Err(e)));
            }
        });

        let progress = AiReviewRunProgress {
            stage: AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        };
        self.ai_review_progress = Some(progress.clone());
        self.mode = AppMode::AiReviewRunning(AiReviewRunState { origin, progress });
    }

    pub fn ai_review_harness_picking(&self) -> bool {
        matches!(&self.mode, AppMode::AiReview(state) if state.harness_pick.is_some())
    }

    pub fn ai_review_harness_pick_move(&mut self, delta: isize) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(pick) = &mut state.harness_pick
            && !pick.agents.is_empty()
        {
            let len = pick.agents.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
            pick.error = None;
        }
    }

    pub fn ai_review_harness_pick_cancel(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode {
            state.harness_pick = None;
        }
    }

    pub fn ai_review_harness_pick_confirm(&mut self) {
        let (chosen, harness_changed) = match &self.mode {
            AppMode::AiReview(state) => state.harness_pick.as_ref().map_or((None, false), |pick| {
                let chosen = pick.agents.get(pick.selected).cloned();
                let changed = chosen.as_ref().is_some_and(|chosen| {
                    pick.previous_harness
                        .as_ref()
                        .is_some_and(|previous| previous != chosen)
                });
                (chosen, changed)
            }),
            _ => return,
        };
        let Some(chosen) = chosen else {
            return;
        };
        if let Err(error) = HeadlessRunner::check_progress_available(&chosen) {
            if let AppMode::AiReview(state) = &mut self.mode
                && let Some(pick) = &mut state.harness_pick
            {
                pick.error = Some(error.to_string());
            }
            return;
        }
        self.accept_ai_review_harness_pick(chosen, harness_changed);
    }

    /// Apply a harness that has already passed its availability check, reset
    /// any model state belonging to the previous harness, and advance to the
    /// rebuilt model step.
    fn accept_ai_review_harness_pick(&mut self, chosen: AgentKind, harness_changed: bool) {
        if let AppMode::AiReview(state) = &mut self.mode {
            state.harness = Some(chosen.clone());
            state.harness_pick = None;
            state.model = None;
            state.model_picked = false;
            state.model_pick = None;
        }
        self.push_toast_success(format!(
            "AI reviews will run with {}",
            chosen.display_name()
        ));
        if harness_changed {
            if let AppMode::AiReview(state) = &mut self.mode {
                state.model_pick = Some(AiModelPickState {
                    rows: model_pick_rows(&chosen),
                    selected: 0,
                    custom_input: String::new(),
                    editing_custom: false,
                });
            }
            return;
        }
        // Re-enter the same start-up chain rather than jumping straight to
        // `begin_ai_pr_review`: with a harness now chosen but no model picked
        // yet, this opens the model picker next.
        self.start_ai_pr_review();
    }

    #[cfg(test)]
    pub(super) fn accept_selected_ai_review_harness_for_test(&mut self) {
        let (chosen, harness_changed) = match &self.mode {
            AppMode::AiReview(state) => state.harness_pick.as_ref().map_or((None, false), |pick| {
                let chosen = pick.agents.get(pick.selected).cloned();
                let changed = chosen.as_ref().is_some_and(|chosen| {
                    pick.previous_harness
                        .as_ref()
                        .is_some_and(|previous| previous != chosen)
                });
                (chosen, changed)
            }),
            _ => (None, false),
        };
        if let Some(chosen) = chosen {
            self.accept_ai_review_harness_pick(chosen, harness_changed);
        }
    }

    pub fn ai_review_model_picking(&self) -> bool {
        matches!(&self.mode, AppMode::AiReview(state) if state.model_pick.is_some())
    }

    /// Whether the model picker's `Custom` row is currently open for
    /// free-text entry, so the key handler can route chars/backspace to it
    /// instead of list navigation.
    pub fn ai_review_model_pick_editing_custom(&self) -> bool {
        matches!(&self.mode, AppMode::AiReview(state)
            if state.model_pick.as_ref().is_some_and(|pick| pick.editing_custom))
    }

    pub fn ai_review_model_pick_move(&mut self, delta: isize) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(pick) = &mut state.model_pick
            && !pick.editing_custom
            && !pick.rows.is_empty()
        {
            let len = pick.rows.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// `esc`: while typing a custom model, back out to the row list without
    /// losing what's typed so far; from the row list, return to the harness
    /// picker with the current harness highlighted.
    pub fn ai_review_model_pick_cancel(&mut self) {
        let (workdir, current_harness) = match &mut self.mode {
            AppMode::AiReview(state) => {
                let Some(pick) = &mut state.model_pick else {
                    return;
                };
                if pick.editing_custom {
                    pick.editing_custom = false;
                    return;
                }
                (state.workdir.clone(), state.harness.clone())
            }
            _ => return,
        };

        let agents = self.allowed_agents_for_project_path(&workdir);
        if agents.is_empty() {
            self.push_toast_error("No agent harnesses are enabled for this project");
            return;
        }
        let selected = current_harness
            .as_ref()
            .map(|harness| AgentKind::index_in(&agents, harness))
            .unwrap_or(0);
        if let AppMode::AiReview(state) = &mut self.mode {
            // Record the chain's original harness the *first* time it steps
            // back to this picker, and leave it alone on any further
            // back-and-forth. Without this, re-confirming an already-switched
            // harness (switch, back out, reselect the same one) would look
            // unchanged from the immediately preceding screen, letting
            // `start_ai_pr_review` reseed `AppConfig::review_model` (e.g. a
            // Claude preset like "sonnet") as a custom model for a harness
            // it's incompatible with.
            // `model_pick` (which is what routes here) only ever exists once
            // a harness has been chosen, so `current_harness` is always
            // `Some` at this point.
            let previous_harness = state
                .harness_pick_origin
                .get_or_insert_with(|| {
                    current_harness
                        .clone()
                        .expect("model_pick implies a harness is already chosen")
                })
                .clone();
            state.harness = None;
            state.harness_pick = Some(AiHarnessPickState {
                agents,
                selected,
                error: None,
                previous_harness: Some(previous_harness),
            });
            state.model = None;
            state.model_picked = false;
            state.model_pick = None;
        }
    }

    pub fn ai_review_model_pick_push_char(&mut self, c: char) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(pick) = &mut state.model_pick
            && pick.editing_custom
        {
            pick.custom_input.push(c);
        }
    }

    pub fn ai_review_model_pick_backspace(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(pick) = &mut state.model_pick
            && pick.editing_custom
        {
            pick.custom_input.pop();
        }
    }

    /// `⏎`: on `Default`/a `Preset` row, applies the choice and proceeds to
    /// the review. On `Custom`, the first `⏎` opens the text field; a second
    /// `⏎` (while editing) submits the typed model (falling back to `Default`
    /// if left blank).
    pub fn ai_review_model_pick_confirm(&mut self) {
        let row = match &self.mode {
            AppMode::AiReview(state) => state
                .model_pick
                .as_ref()
                .and_then(|pick| pick.rows.get(pick.selected).cloned()),
            _ => return,
        };
        let Some(row) = row else {
            return;
        };
        let editing_custom = matches!(&self.mode, AppMode::AiReview(state) if state.model_pick.as_ref().is_some_and(|p| p.editing_custom));

        let chosen: Option<String> = match row {
            ModelPickRow::Default => None,
            ModelPickRow::Preset(name) => Some(name.to_string()),
            ModelPickRow::Custom if !editing_custom => {
                if let AppMode::AiReview(state) = &mut self.mode
                    && let Some(pick) = &mut state.model_pick
                {
                    pick.editing_custom = true;
                }
                return;
            }
            ModelPickRow::Custom => {
                let typed = match &self.mode {
                    AppMode::AiReview(state) => state
                        .model_pick
                        .as_ref()
                        .map(|pick| pick.custom_input.trim().to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                (!typed.is_empty()).then_some(typed)
            }
        };

        if let AppMode::AiReview(state) = &mut self.mode {
            state.model = chosen.clone();
            state.model_picked = true;
            state.model_pick = None;
        }
        self.push_toast_success(match &chosen {
            Some(model) => format!("AI reviews will use model: {model}"),
            None => "AI reviews will use the harness's default model".to_string(),
        });
        self.begin_ai_pr_review();
    }

    /// Poll the background AI PR review. Progress updates the running
    /// screen's stage; `Done` merges the new findings into whichever pane
    /// state is the right target (see the `Target` enum below) — this may
    /// run well after `esc` ([`Self::cancel_ai_pr_review`]) has already
    /// returned the user to the pane, since the background pass keeps going
    /// — then re-caches and surfaces a toast. Returns `true` when a redraw is
    /// warranted.
    pub fn poll_ai_pr_review_bg(&mut self) -> bool {
        let Some(rx) = self.ai_review_bg.as_ref() else {
            return false;
        };
        let mut changed = false;
        let mut large_diff_warning: Option<usize> = None;
        loop {
            match rx.try_recv() {
                Ok(AiReviewProgress::Reviewing { token_estimate }) => {
                    if let Some(progress) = &mut self.ai_review_progress {
                        progress.stage = AiReviewStage::Reviewing { token_estimate };
                    }
                    if let AppMode::AiReviewRunning(state) = &mut self.mode {
                        state.progress.stage = AiReviewStage::Reviewing { token_estimate };
                    }
                    if token_estimate > AI_REVIEW_PROMPT_TOKEN_WARN {
                        large_diff_warning = Some(token_estimate);
                    }
                    changed = true;
                }
                Ok(AiReviewProgress::Activity(activity)) => {
                    if let Some(progress) = &mut self.ai_review_progress {
                        progress.activity = Some(activity.clone());
                    }
                    if let AppMode::AiReviewRunning(state) = &mut self.mode {
                        state.progress.activity = Some(activity);
                    }
                    changed = true;
                }
                Ok(AiReviewProgress::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    if let Some(progress) = &mut self.ai_review_progress {
                        progress.usage = Some((input_tokens, output_tokens));
                    }
                    if let AppMode::AiReviewRunning(state) = &mut self.mode {
                        state.progress.usage = Some((input_tokens, output_tokens));
                    }
                    changed = true;
                }
                Ok(AiReviewProgress::Done(result)) => {
                    self.ai_review_bg = None;
                    self.ai_review_progress = None;
                    let Some(pending) = self.ai_review_pending.take() else {
                        if let Err(e) = result {
                            self.log_error("pr_review", format!("AI review failed: {e}"));
                            self.push_toast_error(format!("AI review failed: {e}"));
                        }
                        changed = true;
                        break;
                    };
                    let pr_number = pending.pr.number;

                    enum Target {
                        Running,
                        Pane(Box<AiReviewState>),
                        Elsewhere,
                    }
                    let matches_pending = |state: &AiReviewState| {
                        state.workdir == pending.workdir
                            && state.pr.number == pending.pr.number
                            && state.pr.head_sha == pending.pr.head_sha
                    };
                    let target = match &self.mode {
                        AppMode::AiReviewRunning(state) if matches_pending(&state.origin) => {
                            Target::Running
                        }
                        AppMode::AiReview(state) if matches_pending(state) => {
                            Target::Pane(Box::new(state.clone()))
                        }
                        _ => Target::Elsewhere,
                    };

                    let mut landed: Option<AiReviewState> = None;
                    match result {
                        Ok(outcome) => {
                            let count = outcome.findings.len();
                            self.log_debug(
                                "pr_review",
                                format!(
                                    "AI review of PR #{pr_number} parsed {count} finding{} \
                                     from {} chars of output",
                                    if count == 1 { "" } else { "s" },
                                    outcome.raw_output.len()
                                ),
                            );

                            let mut base = match &target {
                                Target::Running => {
                                    let AppMode::AiReviewRunning(state) = &self.mode else {
                                        unreachable!()
                                    };
                                    state.origin.clone()
                                }
                                Target::Pane(state) => (**state).clone(),
                                Target::Elsewhere => pending.clone(),
                            };
                            // A new pass replaces the prior draft set entirely
                            // — published findings are real GitHub history at
                            // this point and following up on them happens in
                            // PR Triage, not here.
                            base.findings = outcome.findings;
                            base.summary = outcome.summary;
                            base.attribution = Some(outcome.attribution);
                            base.selected = 0;
                            base.detail_scroll = 0;
                            base.last_run = Some(AiReviewRun {
                                ran_at: Local::now(),
                                outcome: AiReviewRunOutcome::Findings(count),
                            });
                            self.cache_ai_review(&base);

                            let elsewhere = matches!(target, Target::Elsewhere);
                            if !elsewhere {
                                landed = Some(base);
                            }

                            let note = if elsewhere {
                                format!(" for PR #{pr_number} (re-open to see it)")
                            } else {
                                String::new()
                            };
                            if count == 0 {
                                self.push_toast_success(format!(
                                    "AI review found no findings{note}"
                                ));
                            } else {
                                self.push_toast_success(format!(
                                    "AI review found {count} finding{}{note}",
                                    if count == 1 { "" } else { "s" }
                                ));
                            }
                        }
                        Err(e) => {
                            let mut base = match &target {
                                Target::Running => {
                                    let AppMode::AiReviewRunning(state) = &self.mode else {
                                        unreachable!()
                                    };
                                    state.origin.clone()
                                }
                                Target::Pane(state) => (**state).clone(),
                                Target::Elsewhere => pending.clone(),
                            };
                            base.last_run = Some(AiReviewRun {
                                ran_at: Local::now(),
                                outcome: AiReviewRunOutcome::Error(e.to_string()),
                            });
                            self.cache_ai_review(&base);

                            let elsewhere = matches!(target, Target::Elsewhere);
                            landed = if elsewhere { None } else { Some(base) };

                            self.log_error(
                                "pr_review",
                                format!("AI review of PR #{pr_number} failed: {e}"),
                            );
                            let note = if elsewhere {
                                format!(" for PR #{pr_number} (re-open to see it)")
                            } else {
                                String::new()
                            };
                            self.push_toast_error(format!("AI review failed{note}: {e}"));
                        }
                    }
                    if let Some(state) = landed {
                        self.mode = AppMode::AiReview(state);
                    }
                    // A landed run replaces the finding set (or its anchors),
                    // so the fix-cost memo no longer applies.
                    self.ai_review_fix_cost_cache = None;
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ai_review_bg = None;
                    self.ai_review_progress = None;
                    let pending = self.ai_review_pending.take();
                    let detail = "AI review worker disconnected unexpectedly";
                    let pr_number = pending.as_ref().map(|p| p.pr.number);
                    if let Some(pending) = pending {
                        let matches_pending = |state: &AiReviewState| {
                            state.workdir == pending.workdir
                                && state.pr.number == pending.pr.number
                                && state.pr.head_sha == pending.pr.head_sha
                        };
                        let visible = match &self.mode {
                            AppMode::AiReviewRunning(state) => matches_pending(&state.origin),
                            AppMode::AiReview(state) => matches_pending(state),
                            _ => false,
                        };
                        let mut base = match &self.mode {
                            AppMode::AiReviewRunning(state) if matches_pending(&state.origin) => {
                                state.origin.clone()
                            }
                            AppMode::AiReview(state) if matches_pending(state) => state.clone(),
                            _ => pending,
                        };
                        base.last_run = Some(AiReviewRun {
                            ran_at: Local::now(),
                            outcome: AiReviewRunOutcome::Error(detail.to_string()),
                        });
                        self.cache_ai_review(&base);
                        if visible {
                            self.mode = AppMode::AiReview(base);
                        }
                    }
                    self.log_error(
                        "pr_review",
                        pr_number.map_or_else(
                            || detail.to_string(),
                            |number| format!("AI review of PR #{number}: {detail}"),
                        ),
                    );
                    self.push_toast_error("AI review failed unexpectedly");
                    changed = true;
                    break;
                }
            }
        }
        if let Some(token_estimate) = large_diff_warning {
            self.push_toast_warning(format!(
                "Large diff: ~{token_estimate} tokens in this review — may exceed the agent's context window"
            ));
        }
        changed
    }

    /// Cancel the running screen (`esc`/`q`): return to the AI Review pane.
    /// The background thread isn't aborted — if it finishes later,
    /// [`Self::poll_ai_pr_review_bg`] still surfaces the result (via
    /// [`Self::ai_review_pending`], which survives this).
    pub fn cancel_ai_pr_review(&mut self) {
        if let AppMode::AiReviewRunning(state) = &self.mode {
            self.mode = AppMode::AiReview(state.origin.clone());
        }
    }

    /// Open the post-to-GitHub confirm dialog (`W`) for every kept
    /// (not-skipped, not-already-published) finding.
    pub fn ai_review_open_post_confirm(&mut self) {
        #[allow(clippy::type_complexity)]
        let (findings, generated_summary, attribution): (
            Vec<AiReviewFinding>,
            Option<String>,
            Option<AiReviewAttribution>,
        ) = match &self.mode {
            AppMode::AiReview(state) if state.post_confirm.is_none() => {
                let findings: Vec<AiReviewFinding> = state
                    .findings
                    .iter()
                    .filter(|f| !f.skipped && !f.published)
                    .cloned()
                    .collect();
                // `state.summary` is model prose written over the *complete*
                // finding set, so it can describe a finding the user has since
                // skipped (a false positive, or one too sensitive to post) even
                // though that finding itself is excluded from `findings` below.
                // Once anything's been skipped, drop it in favor of the generic
                // placeholder rather than risk republishing what `skipped` was
                // meant to suppress.
                let any_skipped = state.findings.iter().any(|f| f.skipped);
                let generated_summary = if any_skipped {
                    None
                } else {
                    state.summary.clone()
                };
                (findings, generated_summary, state.attribution.clone())
            }
            _ => return,
        };
        if findings.is_empty() {
            self.push_toast_warning("No findings to post — run A to generate some, or skip fewer");
            return;
        }
        let refs: Vec<&AiReviewFinding> = findings.iter().collect();
        let (summary, inline) =
            build_ai_review(&refs, generated_summary.as_deref(), attribution.as_ref());

        if let AppMode::AiReview(state) = &mut self.mode {
            state.post_confirm = Some(AiReviewPostConfirmState {
                inline,
                editor: TextEditor::new(summary),
                editing: false,
                error: None,
            });
        }
    }

    pub fn ai_review_post_confirm_edit(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(post) = &mut state.post_confirm
        {
            post.editing = true;
        }
    }

    pub fn ai_review_post_confirm_stop_edit(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(post) = &mut state.post_confirm
        {
            post.editing = false;
        }
    }

    pub fn ai_review_post_confirm_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(post) = &mut state.post_confirm
            && post.editing
        {
            post.editor.handle_key(key);
        }
    }

    /// Close the post dialog without posting.
    pub fn ai_review_cancel_post_confirm(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode {
            state.post_confirm = None;
        }
    }

    /// Post dialog status for the key handler: `None` when closed, else
    /// whether it is currently in edit mode.
    pub fn ai_review_post_confirm_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::AiReview(state) => state.post_confirm.as_ref().map(|p| p.editing),
            _ => None,
        }
    }

    /// Post the AI review to GitHub (`event: "COMMENT"` — never
    /// auto-approve/request-changes). On success, every included finding is
    /// simply marked `published` — no reconciliation of GitHub identities:
    /// following up happens in PR Triage once a refresh fetches the real
    /// comments. GitHub rejects the *entire* review if any inline comment
    /// points outside the diff; on that failure the drafts are left
    /// untouched and the dialog stays open with the failure recorded rather
    /// than getting silently closed by `show_error`'s mode reset.
    pub fn ai_review_post(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::AiReview(state) => state.post_confirm.as_ref().map(|post| {
                (
                    state.workdir.clone(),
                    state.pr.clone(),
                    post.inline.clone(),
                    ensure_ai_review_attribution(
                        post.editor.text().trim(),
                        state.attribution.as_ref(),
                    ),
                    state
                        .findings
                        .iter()
                        .filter(|finding| !finding.skipped && !finding.published)
                        .count(),
                )
            }),
            _ => return Ok(()),
        };
        let Some((workdir, pr, inline, body, posted_count)) = prep else {
            return Ok(());
        };

        if let Err(e) = GhCli::create_review(&workdir, &pr, &body, "COMMENT", &inline) {
            self.fail_ai_review_post(e);
            return Ok(());
        }

        if let AppMode::AiReview(state) = &mut self.mode {
            for finding in &mut state.findings {
                if !finding.skipped && !finding.published {
                    finding.published = true;
                }
            }
            state.post_confirm = None;
        }
        let published_marker_saved = if let AppMode::AiReview(state) = &self.mode {
            let state = state.clone();
            self.cache_ai_review(&state)
        } else {
            false
        };
        // The published marker must be durable before any refresh begins, so
        // a network failure cannot make the same findings postable again.
        if published_marker_saved {
            self.start_ai_review_triage_refresh(workdir, pr);
        } else {
            self.push_toast_warning(
                "AI review posted, but its published marker could not be saved; PR Triage was not refreshed",
            );
        }
        let next = if published_marker_saved {
            " — refreshing PR Triage"
        } else {
            ""
        };
        self.push_toast_success(format!(
            "Posted AI review · {posted_count} finding{}{}",
            if posted_count == 1 { "" } else { "s" },
            next,
        ));
        Ok(())
    }

    fn start_ai_review_triage_refresh(&mut self, workdir: PathBuf, pr: PrRef) {
        if let Some(db) = self.db.as_ref()
            && let Err(error) = db.delete_pr_review_cache(pr.number, &pr.head_sha)
        {
            self.log_warn(
                "pr_review",
                format!(
                    "could not invalidate PR #{} cache before post refresh: {error}",
                    pr.number
                ),
            );
        }

        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_review_triage_refresh_bg = Some(rx);
        self.ai_review_triage_refresh_pending = Some(AiReviewTriageRefresh {
            workdir: workdir.clone(),
            pr: pr.clone(),
        });
        std::thread::spawn(move || {
            let result = GhCli::fetch_pr_by_number(&workdir, pr.number).and_then(|fresh_pr| {
                crate::app::pr_review::fetch_and_normalize(&workdir, fresh_pr)
            });
            let _ = tx.send(result);
        });
    }

    /// Apply the automatic post-success PR Triage refresh without changing
    /// the current mode. A matching stashed pane is updated in place; when AI
    /// Review was opened elsewhere, the fresh snapshot is still cached for
    /// the next PR Triage entry.
    pub fn poll_ai_review_triage_refresh_bg(&mut self) -> bool {
        let Some(rx) = self.ai_review_triage_refresh_bg.as_ref() else {
            return false;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.ai_review_triage_refresh_bg = None;
                let pending = self.ai_review_triage_refresh_pending.take();
                let number = pending.as_ref().map(|pending| pending.pr.number);
                self.log_warn(
                    "pr_review",
                    number.map_or_else(
                        || "post-success PR Triage refresh disconnected".to_string(),
                        |number| {
                            format!("post-success PR Triage refresh for PR #{number} disconnected")
                        },
                    ),
                );
                self.push_toast_warning(
                    "AI review posted, but PR Triage refresh failed unexpectedly",
                );
                return true;
            }
        };
        self.ai_review_triage_refresh_bg = None;
        let Some(pending) = self.ai_review_triage_refresh_pending.take() else {
            return true;
        };

        match result {
            Ok(mut review) => {
                self.cache_pr_review(&review);
                self.apply_persisted_triage(&mut review);
                let ai_review = self.ai_review_triage_snapshot(&review.pr);
                let checked_out_branch =
                    crate::worktree::WorktreeManager::current_branch(&pending.workdir)
                        .unwrap_or(None);
                let matches = |state: &PrReviewState| {
                    pr_review_state_matches_refresh(state, &pending.workdir, pending.pr.number)
                };

                if let AppMode::PrReview(state) = &mut self.mode
                    && matches(state)
                {
                    apply_refreshed_pr_review_state(
                        state,
                        review.clone(),
                        ai_review.pending_findings,
                        ai_review.last_run.clone(),
                        checked_out_branch.clone(),
                    );
                }
                if let Some(return_to) = self.ai_review_return_to.as_deref_mut()
                    && let AppMode::PrReview(state) = return_to
                    && matches(state)
                {
                    apply_refreshed_pr_review_state(
                        state,
                        review.clone(),
                        ai_review.pending_findings,
                        ai_review.last_run.clone(),
                        checked_out_branch.clone(),
                    );
                }
                if let Some(stash) = &mut self.pr_review_return
                    && matches(&stash.state)
                {
                    apply_refreshed_pr_review_state(
                        &mut stash.state,
                        review.clone(),
                        ai_review.pending_findings,
                        ai_review.last_run,
                        checked_out_branch,
                    );
                }
                self.log_info(
                    "pr_review",
                    format!(
                        "refreshed PR #{} after AI Review post ({} comments)",
                        review.pr.number,
                        review.comments.len()
                    ),
                );
                self.push_toast_success(format!(
                    "PR Triage refreshed · {} comment{}",
                    review.comments.len(),
                    if review.comments.len() == 1 { "" } else { "s" }
                ));
            }
            Err(error) => {
                self.log_warn(
                    "pr_review",
                    format!(
                        "AI Review posted, but PR #{} refresh failed: {error}",
                        pending.pr.number
                    ),
                );
                self.push_toast_warning(format!(
                    "AI review posted, but PR Triage refresh failed: {error}"
                ));
            }
        }
        true
    }

    /// Record a `W` post failure inline on the still-open post-confirm dialog
    /// and restore the pane — working around `show_error`'s unconditional
    /// `self.mode` reset to `Normal` for any non-Normal/Help/Viewing mode,
    /// which would otherwise silently boot the user back to the dashboard on
    /// a recoverable posting error.
    pub(crate) fn fail_ai_review_post(&mut self, e: anyhow::Error) {
        let detail = e.to_string();
        let mut origin = match &self.mode {
            AppMode::AiReview(state) => Some(state.clone()),
            _ => None,
        };
        if let Some(origin) = &mut origin
            && let Some(post) = &mut origin.post_confirm
        {
            post.error = Some(detail);
        }
        self.show_error(e);
        if let Some(origin) = origin {
            self.mode = AppMode::AiReview(origin);
        }
    }

    /// Whether an `A` AI-review background pass is currently running for the
    /// given feature workdir's PR. The background job can outlive the pane it
    /// was started from, so this checks the pending-review snapshot's own
    /// workdir rather than assuming `self.mode` still points at the pane that
    /// kicked it off.
    pub(crate) fn ai_review_running_for_workdir(&self, workdir: &Path) -> bool {
        self.ai_review_bg.is_some()
            && self
                .ai_review_pending
                .as_ref()
                .is_some_and(|pending| pending.workdir == workdir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE_MAPPING_DIFF: &str = "diff --git a/src/multi.rs b/src/multi.rs\n\
--- a/src/multi.rs\n\
+++ b/src/multi.rs\n\
@@ -1,4 +1,5 @@\n\
\x20start\n\
-old two\n\
+new two a\n\
+new two b\n\
\x20three\n\
\x20four\n\
@@ -18,4 +19,4 @@\n\
\x20eighteen\n\
-nineteen\n\
+nineteen revised\n\
\x20twenty\n\
\x20twenty-one\n\
diff --git a/src/added.rs b/src/added.rs\n\
new file mode 100644\n\
--- /dev/null\n\
+++ b/src/added.rs\n\
@@ -0,0 +1,3 @@\n\
+first\n\
+middle\n\
+last\n\
diff --git a/src/deleted.rs b/src/deleted.rs\n\
deleted file mode 100644\n\
--- a/src/deleted.rs\n\
+++ /dev/null\n\
@@ -1,3 +0,0 @@\n\
-first\n\
-middle\n\
-last\n\
diff --git a/src/boundary.rs b/src/boundary.rs\n\
--- a/src/boundary.rs\n\
+++ b/src/boundary.rs\n\
@@ -1,2 +1,2 @@\n\
-first\n\
+FIRST\n\
\x20middle\n\
@@ -9,2 +9,2 @@\n\
\x20penultimate\n\
-last\n\
+LAST\n";

    #[test]
    fn line_mapping_regression_fixtures_cover_failure_prone_diff_shapes() {
        let files = crate::diff::parse_unified_diff(LINE_MAPPING_DIFF).unwrap();
        assert_eq!(files.len(), 4);

        let multi = files
            .iter()
            .find(|file| file.path == "src/multi.rs")
            .unwrap();
        assert_eq!(multi.hunks.len(), 2);
        assert!(
            multi
                .addressable_lines()
                .iter()
                .any(|location| { location.old_line == Some(19) && location.new_line.is_none() })
        );
        assert!(
            multi
                .addressable_lines()
                .iter()
                .any(|location| { location.old_line == Some(18) && location.new_line == Some(19) })
        );

        let added = files
            .iter()
            .find(|file| file.path == "src/added.rs")
            .unwrap();
        assert!(
            added
                .addressable_lines()
                .iter()
                .all(|location| location.old_line.is_none() && location.new_line.is_some())
        );

        let deleted = files
            .iter()
            .find(|file| file.path == "src/deleted.rs")
            .unwrap();
        assert!(
            deleted
                .addressable_lines()
                .iter()
                .all(|location| location.old_line.is_some() && location.new_line.is_none())
        );

        let boundary = files
            .iter()
            .find(|file| file.path == "src/boundary.rs")
            .unwrap();
        let locations = boundary.addressable_lines();
        assert!(
            locations
                .iter()
                .any(|location| location.new_line == Some(1))
        );
        assert!(
            locations
                .iter()
                .any(|location| location.new_line == Some(10))
        );
    }

    #[test]
    fn parse_ai_findings_parses_path_line_and_general_headings() {
        let output = "### src/app/sync.rs|RIGHT|42\nGuard this with the lock.\n\n### General\nConsider adding integration tests.\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].path.as_deref(), Some("src/app/sync.rs"));
        assert_eq!(findings[0].line, Some(42));
        assert_eq!(findings[0].side, Some(crate::diff::DiffSide::New));
        assert_eq!(findings[0].body, "Guard this with the lock.");
        assert!(!findings[0].skipped);
        assert!(!findings[0].published);
        assert_eq!(findings[1].path, None);
        assert_eq!(findings[1].line, None);
    }

    #[test]
    fn parse_ai_review_output_separates_summary_from_findings() {
        let output = "## Summary\nThe patch has one concurrency risk.\n\n### src/app/sync.rs|LEFT|42\nGuard this with the lock.\n";
        let (summary, findings) = parse_ai_review_output(output);
        assert_eq!(
            summary.as_deref(),
            Some("The patch has one concurrency risk.")
        );
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path.as_deref(), Some("src/app/sync.rs"));
        assert_eq!(findings[0].side, Some(crate::diff::DiffSide::Old));
    }

    #[test]
    fn parse_ai_review_output_allows_summary_without_findings() {
        let (summary, findings) =
            parse_ai_review_output("## Summary\nNo actionable issues found.\n");
        assert_eq!(summary.as_deref(), Some("No actionable issues found."));
        assert!(findings.is_empty());
    }

    #[test]
    fn process_ai_review_output_accepts_summary_only_zero_result() {
        let outcome =
            process_ai_review_output("## Summary\nNo actionable issues found.\n".to_string(), "")
                .unwrap();
        assert!(outcome.findings.is_empty());
        assert_eq!(
            outcome.summary.as_deref(),
            Some("No actionable issues found.")
        );
    }

    #[test]
    fn process_ai_review_output_accepts_summary_with_findings() {
        let outcome = process_ai_review_output(
            "## Summary\nOne risk.\n\n### General\nAdd a regression test.\n".to_string(),
            "",
        )
        .unwrap();
        assert_eq!(outcome.findings.len(), 1);
        assert_eq!(outcome.findings[0].body, "Add a regression test.");
    }

    #[test]
    fn untyped_line_that_names_different_old_and_new_rows_is_not_mapped_to_the_wrong_line() {
        let outcome = process_ai_review_output(
            "## Summary\nOne risk.\n\n### src/multi.rs:19\nThe removed behavior is required.\n"
                .to_string(),
            LINE_MAPPING_DIFF,
        )
        .unwrap();

        assert_eq!(outcome.findings.len(), 1);
        assert_eq!(outcome.findings[0].path.as_deref(), Some("src/multi.rs"));
        assert_eq!(outcome.findings[0].line, None);
        assert_eq!(outcome.findings[0].diff_hunk, None);
    }

    #[test]
    fn process_ai_review_output_rejects_missing_summary() {
        let error =
            process_ai_review_output("### General\nAdd a regression test.\n".to_string(), "")
                .unwrap_err();
        assert!(error.to_string().contains("missing a non-empty Summary"));
    }

    #[test]
    fn process_ai_review_output_rejects_empty_output() {
        let error = process_ai_review_output(String::new(), "").unwrap_err();
        assert!(error.to_string().contains("missing a non-empty Summary"));
    }

    #[test]
    fn parse_ai_findings_malformed_heading_is_pathless() {
        let output = "### not-a-path-line\nSome finding text.\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, None);
        assert_eq!(findings[0].line, None);
    }

    #[test]
    fn parse_ai_findings_drops_empty_findings() {
        let output = "### src/lib.rs:1\n\n### General\nreal finding\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].body, "real finding");
    }

    #[test]
    fn parse_ai_findings_empty_output_yields_no_findings() {
        assert!(parse_ai_findings("").is_empty());
    }

    #[test]
    fn parse_ai_findings_tolerates_a_different_heading_level() {
        let output = "## src/lib.rs:1\nfinding text\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path.as_deref(), Some("src/lib.rs"));
    }

    #[test]
    fn parse_ai_findings_strips_an_outer_code_fence() {
        let output = "```markdown\n### General\nfinding text\n```";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].body, "finding text");
    }

    #[test]
    fn parse_ai_findings_leaves_output_without_a_full_wrap_alone() {
        let output = "before\n```rust\ncode\n```\n### General\nfinding\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn ai_review_prompt_includes_diff_and_memory_context() {
        let prompt = ai_review_prompt("diff content", "known finding", None);
        assert!(prompt.contains("diff content"));
        assert!(prompt.contains("known finding"));
        assert!(prompt.contains(AI_FINDING_HEADING_PREFIX));
        assert!(prompt.contains("## Summary"));
    }

    #[test]
    fn ai_review_prompt_labels_old_and_new_source_coordinates_across_fixtures() {
        let prompt = ai_review_prompt(LINE_MAPPING_DIFF, "", None);

        // The earlier insertion shifts the second hunk: old 18 is new 19,
        // while removed old 19 and replacement new 20 are distinct rows.
        assert!(prompt.contains("[RIGHT:19 LEFT:18]  eighteen"));
        assert!(prompt.contains("[LEFT:19] -nineteen"));
        assert!(prompt.contains("[RIGHT:20] +nineteen revised"));

        // Added-only / deleted-only files expose only their valid side.
        assert!(prompt.contains("[RIGHT:1] +first"));
        assert!(prompt.contains("[RIGHT:3] +last"));
        assert!(prompt.contains("[LEFT:1] -first"));
        assert!(prompt.contains("[LEFT:3] -last"));

        // First and last changed lines remain source coordinates, not rows.
        assert!(prompt.contains("File: src/boundary.rs"));
        assert!(prompt.contains("[RIGHT:1] +FIRST"));
        assert!(prompt.contains("[RIGHT:10] +LAST"));
        assert!(prompt.contains("<path>|<side>|<line>"));
    }

    #[test]
    fn canonical_mapping_covers_hunks_sides_replacements_context_and_boundaries() {
        use crate::diff::{DiffLineLocation, DiffSide};

        let files = crate::diff::parse_unified_diff(LINE_MAPPING_DIFF).unwrap();
        let cases = [
            ("src/multi.rs", DiffSide::Old, 19, Some(19), None),
            ("src/multi.rs", DiffSide::New, 19, Some(18), Some(19)),
            ("src/multi.rs", DiffSide::New, 20, None, Some(20)),
            ("src/multi.rs", DiffSide::New, 21, Some(20), Some(21)),
            ("src/added.rs", DiffSide::New, 1, None, Some(1)),
            ("src/added.rs", DiffSide::New, 3, None, Some(3)),
            ("src/deleted.rs", DiffSide::Old, 1, Some(1), None),
            ("src/deleted.rs", DiffSide::Old, 3, Some(3), None),
            ("src/boundary.rs", DiffSide::New, 1, None, Some(1)),
            ("src/boundary.rs", DiffSide::New, 10, None, Some(10)),
        ];

        for (path, side, line, old_line, new_line) in cases {
            let file = files.iter().find(|file| file.path == path).unwrap();
            assert_eq!(
                resolve_ai_review_location(file, line, Some(side)),
                Some((side, DiffLineLocation { old_line, new_line })),
                "{path} {side:?}:{line}"
            );
        }
    }

    #[test]
    fn explicit_old_and_new_findings_post_to_their_requested_github_sides() {
        let outcome = process_ai_review_output(
            "## Summary\nTwo risks.\n\n\
             ### src/multi.rs|LEFT|19\nRemoved behavior is required.\n\n\
             ### src/multi.rs|RIGHT|19\nThe context call is now unsafe.\n"
                .to_string(),
            LINE_MAPPING_DIFF,
        )
        .unwrap();
        let refs: Vec<&AiReviewFinding> = outcome.findings.iter().collect();
        let (_, inline) = build_ai_review(&refs, outcome.summary.as_deref(), None);

        assert_eq!(inline.len(), 2);
        assert_eq!((inline[0].side, inline[0].line), ("LEFT", 19));
        assert_eq!((inline[1].side, inline[1].line), ("RIGHT", 19));
    }

    #[test]
    fn ai_review_prompt_leads_with_skill_directive_when_configured() {
        let prompt = ai_review_prompt("diff", "", Some("review"));
        assert!(prompt.starts_with("First, use the /review skill/command"));
    }

    #[test]
    fn ai_review_prompt_omits_empty_memory_section() {
        let prompt = ai_review_prompt("diff", "", None);
        assert!(!prompt.contains("Known recurring findings"));
    }

    #[test]
    fn model_pick_rows_offers_verified_presets_for_claude_only() {
        let claude = model_pick_rows(&AgentKind::Claude);
        assert!(claude.contains(&ModelPickRow::Preset("sonnet")));
        let codex = model_pick_rows(&AgentKind::Codex);
        assert!(!codex.iter().any(|r| matches!(r, ModelPickRow::Preset(_))));
        // Pi accepts `--model` (see `HeadlessRunner::supports_model_flag`), so
        // it gets the same Default/Custom picker as the other unenumerable
        // harnesses rather than being skipped.
        assert_eq!(
            model_pick_rows(&AgentKind::Pi),
            vec![ModelPickRow::Default, ModelPickRow::Custom]
        );
    }

    #[test]
    fn explicit_default_model_does_not_fall_back_to_configured_model() {
        assert_eq!(model_for_ai_review_run(None, true, Some("sonnet")), None);
        assert_eq!(
            model_for_ai_review_run(None, false, Some("sonnet")),
            Some("sonnet".to_string())
        );
    }

    fn finding(
        path: Option<&str>,
        line: Option<u32>,
        body: &str,
        hunk: Option<&str>,
    ) -> AiReviewFinding {
        AiReviewFinding {
            path: path.map(String::from),
            line,
            side: line.map(|_| crate::diff::DiffSide::New),
            body: body.to_string(),
            diff_hunk: hunk.map(String::from),
            skipped: false,
            published: false,
        }
    }

    #[test]
    fn build_ai_review_splits_anchored_and_general_findings() {
        let anchored = finding(
            Some("src/lib.rs"),
            Some(10),
            "fix this",
            Some("@@ -1 +1 @@"),
        );
        let general = finding(None, None, "general note", None);
        let (summary, inline) = build_ai_review(
            &[&anchored, &general],
            Some("The patch has an anchored and a broad risk."),
            None,
        );
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].path, "src/lib.rs");
        assert!(inline[0].body.contains("AI review via AMF"));
        assert!(summary.contains("general note"));
        assert!(summary.starts_with("The patch has an anchored and a broad risk."));
        assert!(summary.ends_with("— AI review via AMF"));
    }

    #[test]
    fn build_ai_review_with_no_general_findings_has_bare_summary() {
        let anchored = finding(
            Some("src/lib.rs"),
            Some(10),
            "fix this",
            Some("@@ -1 +1 @@"),
        );
        let (summary, inline) = build_ai_review(&[&anchored], None, None);
        assert_eq!(inline.len(), 1);
        assert_eq!(summary, "AI review, via AMF.");
    }

    #[test]
    fn build_ai_review_folds_an_unmatched_line_into_the_summary_instead_of_posting_it() {
        // No diff_hunk means the line never matched the diff at generation
        // time — GitHub would reject the whole review if this were posted
        // inline, so it must fold into the summary instead.
        let unmatched = finding(Some("src/lib.rs"), Some(999), "miscounted line", None);
        let (summary, inline) = build_ai_review(&[&unmatched], None, None);
        assert!(inline.is_empty());
        assert!(summary.contains("miscounted line"));
    }

    #[test]
    fn cache_entry_counts_only_publishable_findings_from_a_successful_run() {
        let mut skipped = finding(None, None, "skip", None);
        skipped.skipped = true;
        let mut published = finding(None, None, "posted", None);
        published.published = true;
        let pending = finding(None, None, "pending", None);
        let mut entry = AiReviewCacheEntry {
            findings: vec![skipped, published, pending],
            last_run: Some(AiReviewRun {
                ran_at: Local::now(),
                outcome: AiReviewRunOutcome::Findings(3),
            }),
            summary: Some("One finding remains.".to_string()),
            attribution: None,
        };

        assert_eq!(entry.publishable_finding_count(), 1);
        entry.last_run = Some(AiReviewRun {
            ran_at: Local::now(),
            outcome: AiReviewRunOutcome::Error("failed".to_string()),
        });
        assert_eq!(entry.publishable_finding_count(), 0);
    }

    #[test]
    fn append_ai_review_attribution_appends_footer_and_trims_trailing_whitespace() {
        let body = append_ai_review_attribution("finding text  \n\n", None);
        assert_eq!(body, "finding text\n\n— AI review via AMF");
    }

    fn sample_attribution() -> AiReviewAttribution {
        AiReviewAttribution {
            harness: Some("claude".to_string()),
            model: Some("sonnet".to_string()),
            input_tokens: Some(12_300),
            output_tokens: Some(4_500),
            estimated_cost: Some("$0.10".to_string()),
            ..Default::default()
        }
    }

    #[test]
    fn usage_summary_formats_complete_usage_deterministically() {
        let attribution = AiReviewAttribution {
            cached_tokens: Some(3_200),
            total_tokens: Some(20_000),
            elapsed_ms: Some(125_000),
            ..sample_attribution()
        };
        let body = append_ai_review_attribution("finding text", Some(&attribution));
        assert_eq!(
            body,
            "finding text\n\n### AI review usage\n- Harness: claude\n- Model: sonnet\n- Elapsed: 2m 05s\n- Input tokens: 12.3k\n- Output tokens: 4.5k\n- Cached tokens: 3.2k\n- Total tokens: 20.0k\n- Estimated cost: $0.10\n\n— AI review via AMF"
        );
    }

    #[test]
    fn attribution_disclosure_degrades_when_usage_and_cost_are_missing() {
        let attribution = AiReviewAttribution {
            harness: Some("codex".to_string()),
            model: None,
            input_tokens: None,
            output_tokens: None,
            estimated_cost: None,
            ..Default::default()
        };
        assert_eq!(
            attribution.plain_label(),
            "harness codex · model harness default"
        );
        assert!(!attribution.has_usage());
        assert!(
            attribution
                .usage_summary()
                .contains("Input tokens: unavailable")
        );
        assert!(
            attribution
                .usage_summary()
                .contains("Estimated cost: unavailable")
        );
    }

    #[test]
    fn usage_summary_preserves_partial_usage_without_pricing_it() {
        let attribution = AiReviewAttribution {
            harness: Some("pi".to_string()),
            model: None,
            input_tokens: Some(700),
            elapsed_ms: Some(500),
            ..Default::default()
        };
        let summary = attribution.usage_summary();
        assert!(summary.contains("Input tokens: 700"));
        assert!(summary.contains("Output tokens: unavailable"));
        assert!(summary.contains("Estimated cost: unavailable"));
    }

    #[test]
    fn run_attribution_retains_complete_usage_elapsed_time_and_configured_cost() {
        let usage = crate::headless::HeadlessUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(200),
            cached_tokens: Some(50),
            total_tokens: Some(1_250),
        };
        let attribution = AiReviewAttribution::from_run(
            &AgentKind::Claude,
            Some("sonnet"),
            Some(&usage),
            &crate::token_tracking::TokenPricingConfig::default(),
            std::time::Duration::from_secs(3),
        );
        assert_eq!(attribution.total_tokens, Some(1_250));
        assert_eq!(attribution.elapsed_ms, Some(3_000));
        assert!(attribution.estimated_cost.is_some());
    }

    #[test]
    fn partial_run_usage_leaves_cost_unavailable() {
        let usage = crate::headless::HeadlessUsage {
            input_tokens: Some(1_000),
            ..Default::default()
        };
        let attribution = AiReviewAttribution::from_run(
            &AgentKind::Opencode,
            None,
            Some(&usage),
            &crate::token_tracking::TokenPricingConfig::default(),
            std::time::Duration::ZERO,
        );
        assert_eq!(attribution.estimated_cost, None);
        assert!(
            attribution
                .usage_summary()
                .contains("Estimated cost: unavailable")
        );
    }

    #[test]
    fn failed_run_has_no_publishable_findings_or_usage_post() {
        let entry = AiReviewCacheEntry {
            findings: vec![finding(None, None, "stale draft", None)],
            last_run: Some(AiReviewRun {
                ran_at: Local::now(),
                outcome: AiReviewRunOutcome::Error("provider failed".to_string()),
            }),
            ..Default::default()
        };
        assert_eq!(entry.publishable_finding_count(), 0);
    }

    #[test]
    fn ensure_ai_review_attribution_restores_a_marker_the_user_deleted() {
        // The summary body is editable right up to `W`; a user who trims the
        // seeded attribution while editing must still get an attributed post.
        let body = ensure_ai_review_attribution("Fixed the summary text.", None);
        assert_eq!(body, "Fixed the summary text.\n\n— AI review via AMF");
    }

    #[test]
    fn ensure_ai_review_attribution_does_not_duplicate_an_existing_marker() {
        let body = ensure_ai_review_attribution("Summary.\n\n— AI review via AMF", None);
        assert_eq!(body, "Summary.\n\n— AI review via AMF");
    }

    #[test]
    fn ensure_ai_review_attribution_replaces_a_stale_usage_summary() {
        // A re-priced run must not stack a second `_AI review · …_` line on
        // top of the one the dialog was seeded with.
        let attribution = sample_attribution();
        let seeded = append_ai_review_attribution("Summary.", Some(&attribution));
        let repriced = AiReviewAttribution {
            estimated_cost: Some("$0.20".to_string()),
            ..sample_attribution()
        };
        let body = ensure_ai_review_attribution(&seeded, Some(&repriced));
        assert_eq!(body.matches("### AI review usage").count(), 1);
        assert!(body.ends_with("Estimated cost: $0.20\n\n— AI review via AMF"));
    }

    #[test]
    fn strip_ai_review_attribution_ignores_the_heading_mid_paragraph() {
        // A finding that merely quotes or discusses the heading text (not as
        // its own trailing paragraph) must not be truncated as if it were the
        // real deterministic usage block.
        let body = "Findings should avoid emitting a literal \"### AI review usage\" \
                     heading inside generated text.\n\n— AI review via AMF";
        assert_eq!(
            strip_ai_review_attribution(body),
            body.strip_suffix("\n\n— AI review via AMF").unwrap()
        );
    }

    #[test]
    fn non_claude_cached_tokens_are_not_double_counted_into_cost() {
        // Codex/Opencode/Pi report `cached_tokens` via generic fallback keys
        // that are typically already a subset of `input_tokens`, unlike
        // Anthropic's separate, additive `cache_read_input_tokens`.
        let with_cache = crate::headless::HeadlessUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(200),
            cached_tokens: Some(500),
            total_tokens: None,
        };
        let without_cache = crate::headless::HeadlessUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(200),
            cached_tokens: None,
            total_tokens: None,
        };
        let pricing = crate::token_tracking::TokenPricingConfig::default();
        let cost_with_cache = AiReviewAttribution::from_run(
            &AgentKind::Codex,
            None,
            Some(&with_cache),
            &pricing,
            std::time::Duration::ZERO,
        )
        .estimated_cost;
        let cost_without_cache = AiReviewAttribution::from_run(
            &AgentKind::Codex,
            None,
            Some(&without_cache),
            &pricing,
            std::time::Duration::ZERO,
        )
        .estimated_cost;
        assert_eq!(cost_with_cache, cost_without_cache);
    }

    #[test]
    fn usage_is_never_appended_to_inline_findings() {
        let anchored = finding(
            Some("src/lib.rs"),
            Some(10),
            "fix this",
            Some("@@ -1 +1 @@"),
        );
        let attribution = sample_attribution();
        let (_, inline) = build_ai_review(&[&anchored], Some("One issue."), Some(&attribution));
        assert_eq!(inline.len(), 1);
        assert!(!inline[0].body.contains("AI review usage"));
    }

    #[test]
    fn ai_review_finding_footer_is_distinct_from_the_reply_flow_ai_footer() {
        // A finding posted by this module must not be mistaken for a reply
        // AMF's `R`/`n` dialog posted (see `reply_posted_via_amf`), even
        // though both disclose AI authorship.
        let comment = crate::app::pr_review::PrComment {
            id: 1,
            kind: crate::app::pr_review::CommentKind::Inline,
            author: "amf".into(),
            is_bot: false,
            path: Some("src/lib.rs".into()),
            line: Some(10),
            side: Some("RIGHT".into()),
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: append_ai_review_attribution("fix this", None),
            snippet: String::new(),
            in_reply_to: Some(2),
            thread_id: None,
            is_resolved: false,
            triage: crate::app::pr_review::TriageState::Untriaged,
            local_note: None,
            batch_id: None,
            github_id: None,
            github_review_id: None,
        };
        assert!(!crate::app::pr_review::reply_posted_via_amf(&comment));
    }

    fn sample_diff_files() -> Vec<crate::diff::DiffFile> {
        let unified = "diff --git a/src/lib.rs b/src/lib.rs\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -1,3 +1,3 @@\n\
             \x20context one\n\
             -old line\n\
             +new line\n\
             \x20context two\n";
        crate::diff::parse_unified_diff(unified).unwrap()
    }

    #[test]
    fn diff_hunk_for_location_matches_the_covering_hunk() {
        let files = sample_diff_files();
        let file = &files[0];
        let (_, location) =
            resolve_ai_review_location(file, 2, Some(crate::diff::DiffSide::New)).unwrap();
        let hunk =
            diff_hunk_for_location(&files, "src/lib.rs", crate::diff::DiffSide::New, location);
        assert!(hunk.is_some());
        assert!(hunk.unwrap().contains("new line"));
    }

    #[test]
    fn source_location_is_none_outside_the_hunk_or_file() {
        let files = sample_diff_files();
        assert!(
            resolve_ai_review_location(&files[0], 9999, Some(crate::diff::DiffSide::New)).is_none()
        );
        assert!(
            files
                .iter()
                .find(|file| file.path == "src/other.rs")
                .is_none()
        );
    }
}
