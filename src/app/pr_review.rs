//! PR comment-review model and normalization (feature-specific).
//!
//! The generic `gh` access lives in [`crate::github`]; this module turns those
//! raw GitHub payloads into a single triage-ready [`PrReview`] and owns the
//! token-saving transforms (bot-boilerplate stripping, one-line snippets,
//! thread-resolution merge). See `docs/backlog/pr-comment-review-plan.md`.

// Some helpers (token estimate for the confirm dialog, the loading-state probe)
// are consumed by later epics; keep them until those land.
#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use chrono::{DateTime, Local};
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::*;
use crate::claude::ClaudeLauncher;
use crate::editor::TextEditor;
use crate::github::{
    GhCli, IssueComment, PrListEntry, PrRef, PrResolution, PrReviewComment as GhPrReviewComment,
    Review, ReviewComment, ReviewThread,
};

/// Snippet length (chars) shown in the comment list.
const SNIPPET_LEN: usize = 80;

/// Label (and de-facto identity) of the dedicated PR-review agent session. The
/// session is found-or-created by this label so the same window is reused for
/// every fix in a PR (plan token principle #4 — pay per-session overhead once).
pub(crate) const REVIEW_SESSION_LABEL: &str = "PR Review";

/// Pause between consecutive prompts when queuing a batch of fixes into one
/// session, so the harness registers each `Enter` as its own submission before
/// the next prompt is pasted (otherwise rapid pastes can merge into one turn).
const BATCH_FIX_SUBMIT_DELAY: std::time::Duration = std::time::Duration::from_millis(150);

/// Soft ceilings for the combined-batch prompt (`B`). Past either, the confirm
/// dialog still opens but a warning toast fires so the user knows a single
/// prompt this large risks blowing the agent's context window (plan: "keep the
/// set bounded"). They gate a warning, not the action.
const BATCH_COMBINED_COMMENT_WARN: usize = 15;
const BATCH_COMBINED_TOKEN_WARN: usize = 6000;

/// Categories offered in the "add to memory" dialog (`Tab` cycles), matching
/// the examples in the review-memory doc's own header template. `General` is
/// the default and also what a blank category falls back to in
/// `review_memory::append_finding`.
pub(crate) const MEMORY_CATEGORIES: &[&str] = &[
    "General",
    "Concurrency",
    "Error handling",
    "Naming",
    "Tests",
    "Performance",
    "API design",
    "Style",
];

/// Practical ceiling for the "All" lookback depth. Not truly unbounded — a
/// repo's full closed-PR history could be thousands deep, and both the `gh`
/// fetch loop and the one-shot distill pass scale with it.
const BOOTSTRAP_ALL_LIMIT: u32 = 500;

/// How far back the review-memory lookback bootstrap (Epic E) looks when
/// seeding `review-memory.md` from history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapDepth {
    Twenty,
    Fifty,
    Hundred,
    All,
}

impl BootstrapDepth {
    pub const ALL: [BootstrapDepth; 4] = [Self::Twenty, Self::Fifty, Self::Hundred, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::Twenty => "20 PRs",
            Self::Fifty => "50 PRs",
            Self::Hundred => "100 PRs",
            Self::All => "All",
        }
    }

    /// The `gh pr list --limit` value this depth fetches.
    pub fn limit(self) -> u32 {
        match self {
            Self::Twenty => 20,
            Self::Fifty => 50,
            Self::Hundred => 100,
            Self::All => BOOTSTRAP_ALL_LIMIT,
        }
    }
}

impl Default for BootstrapDepth {
    /// Matches the plan's mockup, which highlights 50 PRs by default.
    fn default() -> Self {
        Self::Fifty
    }
}

/// Progress of the background lookback-bootstrap fetch + distill (`b` in the
/// PR picker). Two stages: the `gh` fetch loop (zero agent tokens) and the one
/// headless agent pass that clusters the gathered comments into findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStage {
    FetchingComments,
    Distilling {
        pr_count: usize,
        token_estimate: usize,
    },
}

/// Stage of the AI PR review's full-screen running view (Epic E `A`). Mirrors
/// [`BootstrapStage`]: a cheap prep stage, then the one paid pass with a token
/// estimate.
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

/// Outcome of a completed bootstrap run.
#[derive(Debug, Clone, Copy)]
pub struct BootstrapOutcome {
    /// PRs whose comments/reviews contributed non-empty text to the prompt.
    pub pr_count: usize,
    /// Findings newly appended to the memory doc (dedup-aware — re-running
    /// the bootstrap over overlapping history won't double them up).
    pub appended: usize,
}

/// Messages sent back from the background bootstrap thread. `Distilling` fires
/// once, right before the one headless agent call, so the running screen can
/// show a token estimate for that call; `Done` fires exactly once at the end.
pub enum BootstrapProgress {
    Distilling {
        pr_count: usize,
        token_estimate: usize,
    },
    Done(Result<BootstrapOutcome>),
}

/// Past this many estimated prompt tokens, [`App::start_ai_pr_review`] still
/// runs the AI review but warns — a huge PR diff risks blowing the agent's
/// context window (same soft-ceiling-not-hard-gate pattern as the combined-fix
/// batch's [`BATCH_COMBINED_TOKEN_WARN`]).
const AI_REVIEW_TOKEN_WARN: usize = 20_000;

/// Synthetic id range for AI-review draft findings ([`findings_to_comments`]),
/// kept well clear of real GitHub comment ids (ordinary small-ish database
/// ids) so a draft can never collide with a fetched comment. Ids are assigned
/// sequentially from this base per review run — not stable across
/// regenerations, which is fine since each `A` run replaces the prior draft
/// set (see [`App::poll_ai_pr_review_bg`]).
const AI_FINDING_ID_BASE: u64 = 1 << 62;

/// The heading [`ai_review_prompt`] instructs the agent to emit per finding,
/// e.g. `### src/app/sync.rs:42` or `### General`. [`parse_ai_findings`] parses
/// exactly this shape back out.
const AI_FINDING_HEADING_PREFIX: &str = "### ";

/// One AI-review finding, parsed from the agent's fixed-format output
/// ([`parse_ai_findings`]) before being turned into a draft [`PrComment`]
/// ([`findings_to_comments`]). `path`/`line` are `None` for a finding with no
/// single-line anchor (the `### General` bucket).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiFinding {
    pub path: Option<String>,
    pub line: Option<u32>,
    pub body: String,
    /// The hunk (from the PR diff) covering `path:line`, matching the shape
    /// GitHub's API hands over for free on a real review comment
    /// (`@@ ... @@` header + body). Unlike a GitHub comment, nothing about
    /// generating a finding naturally produces this — it's reconstructed
    /// after parsing by re-matching `path:line` back into the diff
    /// ([`diff_hunk_for_line`]) — so the pane can render/inject the same
    /// context a fetched comment would carry. `None` when there's no
    /// anchor, or the line couldn't be matched to a hunk.
    pub diff_hunk: Option<String>,
}

/// Progress of the background AI PR review (`A` in the review pane, Epic E):
/// the one headless agent pass over the PR diff. Mirrors [`BootstrapProgress`]
/// — `Reviewing` fires once with a token estimate right before the paid call,
/// `Done` fires exactly once at the end.
pub enum AiReviewProgress {
    Reviewing { token_estimate: usize },
    Done(Result<AiReviewOutcome>),
}

/// Successful result of one AI PR review pass: the parsed findings plus the
/// agent's raw response text. The raw text isn't shown in the UI — it exists
/// so [`App::poll_ai_pr_review_bg`] can write it to the debug log, since a
/// model that doesn't follow the fixed-format instruction produces zero
/// parsed findings with no other visible signal that anything went wrong
/// (vs. a genuinely clean diff, which also parses to zero findings).
pub struct AiReviewOutcome {
    pub findings: Vec<AiFinding>,
    pub raw_output: String,
}

/// Assemble the AI code-review prompt: the PR diff, the review-memory doc's
/// content as context (so the agent checks the team's known recurring
/// findings first), and a fixed-format output instruction so the response
/// round-trips through [`parse_ai_findings`] with no further parsing — the
/// same "instruct a fixed shape, parse deterministically" approach as
/// [`bootstrap_prompt`].
///
/// `skill`, from `AppConfig::ai_review_skill`, is a Claude Code skill/command
/// name (no leading `/`, e.g. `"review"`) to run first as the primary review
/// methodology when set. AMF ships no bundled skill for reviewing a PR diff
/// itself, so this lets a user with a richer installed review skill (the
/// built-in `/review`, or a marketplace one) supply the review judgment while
/// AMF still owns parsing the findings back out via the fixed-format
/// instruction that follows. `None` (the default) skips straight to AMF's own
/// review instructions.
pub fn ai_review_prompt(diff: &str, memory: &str, skill: Option<&str>) -> String {
    let mut out = String::new();
    if let Some(skill) = skill {
        out.push_str(&format!(
            "First, use the /{skill} skill/command to review the pull request diff below as \
             your primary review methodology.\n\n"
        ));
    }
    out.push_str(
        "You are reviewing a pull request's diff for correctness bugs and quality issues. \
         Check especially for issues matching the team's known recurring findings listed below, \
         if any. Skip praise and style nitpicks the diff already handles well.\n\n",
    );
    if !memory.trim().is_empty() {
        out.push_str("Known recurring findings for this project:\n");
        out.push_str(memory.trim());
        out.push_str("\n\n");
    }
    out.push_str("Diff:\n\n");
    out.push_str(diff.trim_end());
    out.push_str(&format!(
        "\n\n---\n\nOutput ONLY a list of findings, one per heading, in this exact format (no \
         prose outside it; omit entirely if there are no findings):\n\n\
         {AI_FINDING_HEADING_PREFIX}<path>:<line>\n\
         <finding text, 1-3 sentences>\n\n\
         {AI_FINDING_HEADING_PREFIX}General\n\
         <a finding with no single file:line anchor>\n"
    ));
    out
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

/// Parse the AI reviewer's fixed-format output ([`ai_review_prompt`]) into
/// findings. Tolerant of common formatting drift: an outer code fence around
/// the whole response is stripped first ([`strip_outer_code_fence`]), and any
/// small markdown heading level starts a new finding ([`strip_finding_heading`],
/// not just the requested `###`). A `path:line` heading (the line parses as
/// `u32`) anchors it, anything else (`General`, malformed) leaves it pathless.
/// Body lines up to the next heading are joined and trimmed. Empty findings
/// (blank body) are dropped rather than erroring — a partially-malformed
/// response still yields whatever findings did parse.
pub fn parse_ai_findings(output: &str) -> Vec<AiFinding> {
    fn flush(current: Option<(Option<String>, Option<u32>, Vec<&str>)>, out: &mut Vec<AiFinding>) {
        let Some((path, line, lines)) = current else {
            return;
        };
        let body = lines.join("\n").trim().to_string();
        if !body.is_empty() {
            out.push(AiFinding {
                path,
                line,
                body,
                diff_hunk: None,
            });
        }
    }

    let output = strip_outer_code_fence(output);
    let mut findings = Vec::new();
    let mut current: Option<(Option<String>, Option<u32>, Vec<&str>)> = None;
    for raw_line in output.lines() {
        match strip_finding_heading(raw_line) {
            Some(heading) => {
                flush(current.take(), &mut findings);
                let (path, line) = match heading.trim().rsplit_once(':') {
                    Some((p, l)) if !p.is_empty() => match l.trim().parse::<u32>() {
                        Ok(n) => (Some(p.to_string()), Some(n)),
                        Err(_) => (None, None),
                    },
                    _ => (None, None),
                };
                current = Some((path, line, Vec::new()));
            }
            None => {
                if let Some((_, _, lines)) = current.as_mut() {
                    lines.push(raw_line);
                }
            }
        }
    }
    flush(current, &mut findings);
    findings
}

/// Turn parsed AI-review findings into draft [`PrComment`]s, assigning
/// synthetic ids sequentially from [`AI_FINDING_ID_BASE`]. `Inline`-kinded
/// when the finding has both `path` and `line` (renders/fixes like a normal
/// line comment); otherwise `Conversation` (no code anchor — the pathless
/// `General` bucket, or a path-only finding, which is also marked
/// `file_level` so it renders/injects like a file-level GitHub comment).
pub fn findings_to_comments(findings: &[AiFinding]) -> Vec<PrComment> {
    findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let kind = if f.path.is_some() && f.line.is_some() {
                CommentKind::Inline
            } else {
                CommentKind::Conversation
            };
            let snippet = truncate_chars(f.body.lines().next().unwrap_or(""), SNIPPET_LEN);
            PrComment {
                id: AI_FINDING_ID_BASE + i as u64,
                kind,
                author: "AI review".to_string(),
                is_bot: false,
                path: f.path.clone(),
                line: f.line,
                side: Some("RIGHT".into()),
                outdated: false,
                file_level: f.path.is_some() && f.line.is_none(),
                diff_hunk: f.diff_hunk.clone(),
                body: f.body.clone(),
                snippet,
                in_reply_to: None,
                thread_id: None,
                is_resolved: false,
                triage: TriageState::default(),
                local_note: None,
                ai_generated: true,
            }
        })
        .collect()
}

/// Lines of context kept on each side of the target line when extracting a
/// windowed hunk for an AI finding ([`diff_hunk_for_line`]). Deliberately
/// small: unlike a human reviewer's inline comment — which GitHub anchors to
/// a hunk that's already a few lines of context around a small change — an
/// AI finding can point at a line inside a large contiguous block of
/// new/changed code, where the *actual* diff hunk covering it spans the
/// whole block (in the worst case, most of the file). Reconstructing that
/// whole hunk would defeat the point of showing "the lines this finding is
/// about."
const AI_FINDING_HUNK_CONTEXT_LINES: usize = 6;

/// Reconstruct a GitHub-style `diff_hunk` string (the `@@ ... @@` header plus
/// a small window of body lines around the target — not the whole matched
/// hunk, see [`AI_FINDING_HUNK_CONTEXT_LINES`]) for whichever hunk in `files`
/// covers `path:line` on the new (current) side of the diff. An AI-review
/// finding gets no such hunk for free — it's re-derived here by matching the
/// model's `path:line` back into the already-fetched PR diff. `None` when the
/// file isn't in the diff, or no hunk's new-side range covers `line` (a
/// mismatched/hallucinated line number) — the finding still renders and
/// injects fine without one, same as any GitHub comment whose hunk happens to
/// be unavailable.
fn diff_hunk_for_line(files: &[crate::diff::DiffFile], path: &str, line: u32) -> Option<String> {
    let line = line as usize;
    let file = files.iter().find(|f| f.path == path)?;
    let hunk = file.hunks.iter().find(|h| {
        let end = h.new_start + h.new_lines;
        line >= h.new_start && line < end
    })?;

    window_parsed_hunk(hunk, line, false, AI_FINDING_HUNK_CONTEXT_LINES)
}

/// Parse a GitHub-provided hunk and retain only the lines immediately around
/// its comment anchor. The synthetic file headers let the regular unified-diff
/// parser do the fiddly line-kind/header work without maintaining a second
/// parser here.
fn window_github_hunk(text: &str, line: usize, old_side: bool, context: usize) -> Option<String> {
    let synthetic = format!(
        "diff --git a/__amf_comment__ b/__amf_comment__\n\
         --- a/__amf_comment__\n\
         +++ b/__amf_comment__\n{text}\n"
    );
    let files = crate::diff::parse_unified_diff(&synthetic).ok()?;
    let hunk = files.first()?.hunks.first()?;
    window_parsed_hunk(hunk, line, old_side, context)
}

/// Render a bounded slice of a parsed hunk centered on `line`. `old_side`
/// selects base-file numbering for comments on removed lines; otherwise the
/// current-file numbering is used.
fn window_parsed_hunk(
    hunk: &crate::diff::DiffHunk,
    line: usize,
    old_side: bool,
    context: usize,
) -> Option<String> {
    // Walk the hunk tracking the old/new line number *at* each entry (before
    // that line is consumed), both to find the target line's index and to
    // know the old/new start of whatever window we slice out below.
    let mut old_line = hunk.old_start;
    let mut new_line = hunk.new_start;
    let mut line_starts = Vec::with_capacity(hunk.lines.len());
    let mut target_idx = None;
    for (i, l) in hunk.lines.iter().enumerate() {
        line_starts.push((old_line, new_line));
        match l.kind {
            crate::diff::DiffLineKind::Context => {
                let candidate = if old_side { old_line } else { new_line };
                if target_idx.is_none() && candidate == line {
                    target_idx = Some(i);
                }
                old_line += 1;
                new_line += 1;
            }
            crate::diff::DiffLineKind::Added => {
                if !old_side && target_idx.is_none() && new_line == line {
                    target_idx = Some(i);
                }
                new_line += 1;
            }
            crate::diff::DiffLineKind::Removed => {
                if old_side && target_idx.is_none() && old_line == line {
                    target_idx = Some(i);
                }
                old_line += 1;
            }
            crate::diff::DiffLineKind::NoNewlineMarker => {}
        }
    }
    let target_idx = target_idx?;

    let start_idx = target_idx.saturating_sub(context);
    let end_idx = (target_idx + context + 1).min(hunk.lines.len());
    let window = &hunk.lines[start_idx..end_idx];
    let (window_old_start, window_new_start) = line_starts[start_idx];
    let (mut window_old_count, mut window_new_count) = (0usize, 0usize);
    for l in window {
        match l.kind {
            crate::diff::DiffLineKind::Context => {
                window_old_count += 1;
                window_new_count += 1;
            }
            crate::diff::DiffLineKind::Added => window_new_count += 1,
            crate::diff::DiffLineKind::Removed => window_old_count += 1,
            crate::diff::DiffLineKind::NoNewlineMarker => {}
        }
    }

    let mut text = format!(
        "@@ -{window_old_start},{window_old_count} +{window_new_start},{window_new_count} @@"
    );
    for l in window {
        if matches!(l.kind, crate::diff::DiffLineKind::NoNewlineMarker) {
            continue;
        }
        text.push('\n');
        text.push_str(&l.text);
    }
    Some(text)
}

/// Attribution appended to GitHub content the agent harness generated on the
/// user's behalf, as opposed to text the user typed — today that's Epic E
/// AI-review findings, and it's the one shared helper any future AI-drafted
/// reply should also route through, so every AI-authored post reads as one
/// consistent voice. Scoped to AI-authored bodies only: a user-typed reply
/// (the "not needed" reason, a hand-edited "done in `<sha>`" template) is the
/// user's own words and stays unmarked. AI-review generation always runs
/// through `ClaudeLauncher::run_headless`, i.e. always Claude, independent of
/// whichever harness a "fix" gets injected into.
fn append_ai_attribution(body: &str) -> String {
    format!("{}\n\n— drafted by Claude via AMF", body.trim_end())
}

/// Build the `(summary, inline comments)` GitHub review payload from a set of
/// AI-review draft findings (Epic E `W` — "post as GitHub review"). A finding
/// with a `path`+`line` anchor (and not `file_level`) becomes an inline
/// [`GhPrReviewComment`]; everything else (pathless `General` findings, or a
/// path-only file-level one — GitHub has no line-less inline review comment)
/// folds into the summary body as a bullet instead, so nothing is silently
/// dropped. Inline comments carry their own attribution footer since they can
/// surface on their own (e.g. the Files-changed view) without the review
/// summary in sight; the summary already self-identifies via its opening line.
fn build_ai_review(findings: &[&PrComment]) -> (String, Vec<GhPrReviewComment>) {
    let mut inline = Vec::new();
    let mut general = Vec::new();
    for f in findings {
        match (&f.path, f.line) {
            (Some(path), Some(line)) if !f.file_level => inline.push(GhPrReviewComment {
                path: path.clone(),
                line,
                side: "RIGHT",
                start_line: None,
                start_side: None,
                body: append_ai_attribution(&f.body),
            }),
            (Some(path), _) => general.push(format!("- **{path}**: {}", f.body)),
            (None, _) => general.push(format!("- {}", f.body)),
        }
    }

    let mut body = String::from("AI review, via AMF.");
    if !general.is_empty() {
        body.push_str("\n\n");
        body.push_str(&general.join("\n"));
    }
    (body, inline)
}

/// Which agent session a "fix" prompt is injected into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FixTarget {
    /// A single dedicated review session, spun up once and reused for every fix
    /// in the PR. The default: per-session overhead (system prompt, tool
    /// definitions, skills) is paid once and file reads amortize across
    /// comments, and review work stays out of the user's working session.
    #[default]
    DedicatedReview,
    /// The feature's existing live agent session — warm in-progress context, at
    /// the cost of carrying that session's unrelated conversation into each fix.
    ExistingLive,
}

impl FixTarget {
    /// Short human label for footers / toasts.
    pub fn label(self) -> &'static str {
        match self {
            FixTarget::DedicatedReview => "dedicated review session",
            FixTarget::ExistingLive => "existing live session",
        }
    }

    /// Compact footer tag.
    pub fn tag(self) -> &'static str {
        match self {
            FixTarget::DedicatedReview => "dedicated",
            FixTarget::ExistingLive => "live",
        }
    }
}

/// Order the comment list is shown in. Cycled with `o`; independent of the
/// `hide_resolved` filter. Sorting is stable, so comments that tie on the sort
/// key (e.g. same file, or all-human/all-bot) keep their original fetch order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrSortMode {
    /// The order `gh` returned them in.
    #[default]
    FetchOrder,
    /// Grouped by file path; comments with no path (conversation/summary) sort
    /// after every file.
    ByFile,
    /// Grouped alphabetically by author login.
    ByAuthor,
    /// Human-authored comments first, bot comments after.
    HumansFirst,
}

impl PrSortMode {
    /// Advance to the next mode, wrapping back to `FetchOrder`.
    pub fn next(self) -> Self {
        match self {
            PrSortMode::FetchOrder => PrSortMode::ByFile,
            PrSortMode::ByFile => PrSortMode::ByAuthor,
            PrSortMode::ByAuthor => PrSortMode::HumansFirst,
            PrSortMode::HumansFirst => PrSortMode::FetchOrder,
        }
    }

    /// Short label for the footer / toast.
    pub fn label(self) -> &'static str {
        match self {
            PrSortMode::FetchOrder => "fetch order",
            PrSortMode::ByFile => "by file",
            PrSortMode::ByAuthor => "by author",
            PrSortMode::HumansFirst => "humans first",
        }
    }
}

/// Index of the session a fix should target within a feature, given the
/// strategy. For [`FixTarget::DedicatedReview`], `None` means no session with
/// `dedicated_label` exists yet and one must be created; for
/// [`FixTarget::ExistingLive`], `None` means there is no live agent session to
/// reuse. `dedicated_label` lets callers reuse this for their own dedicated
/// session (e.g. the final review's "Final Review" window vs PR review's).
pub(crate) fn fix_session_index(
    feature: &Feature,
    target: FixTarget,
    dedicated_label: &str,
) -> Option<usize> {
    match target {
        FixTarget::ExistingLive => feature
            .sessions
            .iter()
            .position(|s| s.kind.is_agent_harness()),
        FixTarget::DedicatedReview => feature
            .sessions
            .iter()
            .position(|s| s.kind.is_agent_harness() && s.label == dedicated_label),
    }
}

/// What kind of GitHub comment this is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentKind {
    /// Inline review comment anchored to a file/line.
    Inline,
    /// A review summary body (Approve / Request changes / Comment).
    ReviewSummary { state: String },
    /// A conversation comment on the PR timeline (no code anchor).
    Conversation,
}

/// Local triage decision, persisted in SQLite (`pr_comment_triage`). GitHub
/// thread resolution is the source of truth for "done"; this is the local layer
/// on top of it (a fix was injected, the user marked it done, skipped it, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriageState {
    #[default]
    Untriaged,
    Fixing,
    Done,
    Skipped,
    Replied,
}

impl TriageState {
    /// Stable token persisted in SQLite. Kept separate from the `Display`/UI
    /// label so the on-disk encoding never shifts with cosmetic changes.
    pub fn as_db_str(self) -> &'static str {
        match self {
            TriageState::Untriaged => "untriaged",
            TriageState::Fixing => "fixing",
            TriageState::Done => "done",
            TriageState::Skipped => "skipped",
            TriageState::Replied => "replied",
        }
    }

    /// Parse the persisted token back into a state; an unknown token (older or
    /// corrupt row) falls back to [`TriageState::Untriaged`].
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "fixing" => TriageState::Fixing,
            "done" => TriageState::Done,
            "skipped" => TriageState::Skipped,
            "replied" => TriageState::Replied,
            _ => TriageState::Untriaged,
        }
    }

    /// One-char list checkbox marker (plan legend: `[ ]` untriaged, `[x]` done,
    /// `[-]` skipped, `[~]` fixing, `[r]` replied).
    pub fn marker(self) -> char {
        match self {
            TriageState::Untriaged => ' ',
            TriageState::Fixing => '~',
            TriageState::Done => 'x',
            TriageState::Skipped => '-',
            TriageState::Replied => 'r',
        }
    }

    /// Short label for the detail chip / toasts (`None` for untriaged — nothing
    /// to show).
    pub fn label(self) -> Option<&'static str> {
        match self {
            TriageState::Untriaged => None,
            TriageState::Fixing => Some("fixing"),
            TriageState::Done => Some("done"),
            TriageState::Skipped => Some("skipped"),
            TriageState::Replied => Some("replied"),
        }
    }
}

/// One normalized, display-ready comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrComment {
    pub id: u64,
    pub kind: CommentKind,
    pub author: String,
    pub is_bot: bool,
    pub path: Option<String>,
    /// Best-known line: the current diff line, falling back to the original.
    pub line: Option<u32>,
    /// GitHub diff side for `line` (`RIGHT`/current or `LEFT`/base).
    /// Older cache rows predate this field and default to the current side.
    #[serde(default)]
    pub side: Option<String>,
    /// True when the comment's anchor line no longer exists in the diff.
    pub outdated: bool,
    /// True when the comment is on the *file* rather than a line (GitHub
    /// `subject_type: "file"`), or when its `diff_hunk` is so large it's
    /// effectively the whole file. Either way the hunk is suppressed in favor of
    /// a bare `File:` reference — see [`PrComment::prompt_hunk`].
    ///
    /// `#[serde(default)]`: cached `pr_review_cache` rows written before this
    /// field existed still deserialize (as `false`, i.e. line-anchored).
    #[serde(default)]
    pub file_level: bool,
    pub diff_hunk: Option<String>,
    /// Original comment body as returned by GitHub.
    pub body: String,
    /// One-line snippet for the list (boilerplate-stripped, truncated).
    pub snippet: String,
    pub in_reply_to: Option<u64>,
    /// GraphQL review-thread node id (inline comments that belong to a thread).
    pub thread_id: Option<String>,
    /// Resolution state from GitHub (source of truth for done/not-done).
    pub is_resolved: bool,
    pub triage: TriageState,
    pub local_note: Option<String>,
    /// `true` for a draft finding AMF's own AI review generated (Epic E `A`),
    /// as opposed to a real comment fetched from GitHub. Drives the `[AI]`
    /// list/detail chip and gates actions that need a real GitHub identity
    /// (reply/resolve) until the finding is posted.
    ///
    /// `#[serde(default)]`: cached `pr_review_cache` rows written before this
    /// field existed still deserialize (as `false`, i.e. a real comment).
    #[serde(default)]
    pub ai_generated: bool,
}

/// A hunk without a usable line anchor longer than this is treated as
/// effectively the whole file. Line-anchored comments are safely windowed
/// around their target instead (see [`COMMENT_HUNK_CONTEXT_LINES`]).
const WHOLE_FILE_HUNK_LINES: usize = 150;

/// Context retained on either side of a line-anchored review comment. GitHub's
/// `diff_hunk` can encompass an entire newly-added function even when the
/// comment itself points at one line; rendering or injecting all of it makes
/// the referenced code hard to spot and wastes prompt context.
const COMMENT_HUNK_CONTEXT_LINES: usize = 3;

impl PrComment {
    /// The diff hunk worth showing and injecting, or `None` when it should be
    /// replaced by a bare `File:` reference — for a file-level comment (whose
    /// hunk is the entire file diff) or an oversized hunk.
    ///
    /// The suppressed case compounds in the combined batch (`B`), where several
    /// whole-file hunks would otherwise land in one prompt.
    pub fn prompt_hunk(&self) -> Option<Cow<'_, str>> {
        let hunk = self.diff_hunk.as_deref()?;
        if self.file_level {
            return None;
        }

        let hunk_lines = hunk.lines().count();
        if hunk_lines > COMMENT_HUNK_CONTEXT_LINES * 2 + 2
            && let Some(line) = self.line
            && let Some(window) = window_github_hunk(
                hunk,
                line as usize,
                self.side.as_deref() == Some("LEFT"),
                COMMENT_HUNK_CONTEXT_LINES,
            )
        {
            return Some(Cow::Owned(window));
        }

        // Keep the old safety net for a malformed/unanchored hunk that cannot
        // be windowed. Valid line-anchored hunks return through the bounded
        // branch above, regardless of their original size.
        if hunk_lines > WHOLE_FILE_HUNK_LINES {
            return None;
        }

        Some(Cow::Borrowed(hunk))
    }

    /// Whether a hunk exists but is being withheld as whole-file-sized. Drives
    /// the "comment on file" note in both the prompt and the detail pane.
    pub fn hunk_suppressed(&self) -> bool {
        self.diff_hunk.is_some() && self.prompt_hunk().is_none()
    }

    /// Text to send to the agent: boilerplate-stripped for bots, verbatim for
    /// humans. Keeps token-heavy bot scaffolding out of prompts.
    pub fn agent_text(&self) -> String {
        if self.is_bot {
            strip_bot_boilerplate(&self.body)
        } else {
            self.body.clone()
        }
    }

    /// Seed text for the "add to memory" dialog: the bot-stripped comment text
    /// with a `file`/`file:line` hint appended, so a finding phrased as a
    /// general rule still carries where it came from. Edited freely before
    /// [`review_memory::append_finding`] writes it as a single bullet
    /// (whitespace/newlines collapsed at that point).
    pub fn memory_finding_seed(&self) -> String {
        let text = self.agent_text().trim().to_string();
        let hint = match &self.path {
            Some(path) if !self.file_level => match self.line {
                Some(line) => Some(format!("{path}:{line}")),
                None => Some(path.clone()),
            },
            Some(path) => Some(path.clone()),
            None => None,
        };
        match hint {
            Some(hint) => format!("{text} ({hint})"),
            None => text,
        }
    }

    /// Assemble the minimal "fix" prompt for this comment: a single instruction
    /// line, the `file:line` pointer, the (bot-stripped) comment text, and the
    /// GitHub-provided diff hunk.
    ///
    /// Deliberately carries **no file contents** — the agent already has the
    /// repo checked out and opens what it needs. This minimal context is the
    /// single biggest token lever (plan token principle #3). The `diff_hunk` is
    /// free: GitHub returns it per inline comment, so including it costs no
    /// extra fetch.
    pub fn fix_prompt(&self) -> String {
        format!(
            "Address this PR review comment.\n{}",
            self.fix_prompt_body()
        )
    }

    /// The per-comment context block shared by the single-comment [`fix_prompt`]
    /// and the combined-batch prompt ([`combined_fix_prompt`]): the `file:line`
    /// pointer, the (bot-stripped) comment text, and the GitHub diff hunk — with
    /// no leading instruction line and no file contents.
    ///
    /// [`fix_prompt`]: Self::fix_prompt
    fn fix_prompt_body(&self) -> String {
        let mut out = String::new();

        if let Some(path) = &self.path {
            match self.line {
                Some(line) if !self.file_level => out.push_str(&format!("File: {path}:{line}")),
                _ => out.push_str(&format!("File: {path}")),
            }
            if self.file_level {
                out.push_str("  (comment on the whole file)");
            } else if self.outdated {
                out.push_str("  (comment is on a line that has since changed)");
            }
            out.push('\n');
        }

        out.push_str(&format!(
            "Comment (@{}): {}\n",
            self.author,
            self.agent_text().trim()
        ));

        match self.prompt_hunk() {
            Some(hunk) => {
                out.push_str("Diff hunk:\n");
                out.push_str(hunk.trim_end());
                out.push('\n');
            }
            // A whole-file-sized hunk is withheld rather than injected: say so,
            // so the agent knows to open the file instead of assuming there was
            // no context to give.
            None if self.hunk_suppressed() => {
                out.push_str(
                    "Diff hunk omitted (it covers effectively the whole file) — \
                     open the file for context.\n",
                );
            }
            None => {}
        }

        out.trim_end().to_string()
    }

    /// How a reply to this comment is posted to GitHub. Inline review comments
    /// reply into their thread (via the thread's root comment id); everything
    /// else (conversation comments, review summaries) posts as a new top-level
    /// conversation comment.
    pub fn reply_target(&self) -> ReplyTarget {
        match self.kind {
            CommentKind::Inline => ReplyTarget::InlineThread {
                root_comment_id: self.in_reply_to.unwrap_or(self.id),
            },
            CommentKind::Conversation | CommentKind::ReviewSummary { .. } => {
                ReplyTarget::Conversation
            }
        }
    }
}

/// Where a reply is delivered on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyTarget {
    /// Reply into an inline review thread, appended under its root comment.
    InlineThread { root_comment_id: u64 },
    /// Post a new top-level comment on the PR conversation timeline.
    Conversation,
}

/// The two contextual replies the pane posts — both tied to a triage decision
/// rather than free-form. A reply is never arbitrary: it either reports a fix
/// or explains why one isn't needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    /// "Done in `<sha>`." after a completed fix → marks the comment `Done`.
    Done,
    /// "Not needed because…" when declining a fix → marks the comment `Skipped`
    /// and keeps the explanation as its local note.
    NotNeeded,
}

impl ReplyKind {
    /// Short label for the reply dialog title.
    pub fn title(self) -> &'static str {
        match self {
            ReplyKind::Done => "Reply · mark done",
            ReplyKind::NotNeeded => "Reply · not needed",
        }
    }
}

/// Short HEAD commit hash of `workdir`, used to seed a "Done in `<sha>`." reply.
/// `None` when the directory isn't a git repo or has no commits yet.
fn latest_commit_short_sha(workdir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Rough token estimate for a prompt preview (~4 chars/token, the usual
/// English-text heuristic). Approximate by design — it backs the "~N tokens"
/// hint in the fix-confirmation dialog, not a billing figure.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Assemble **one** combined prompt that addresses every comment in `comments`
/// — the "fix all of these, then I'll come back" batch. A single shared
/// preamble is followed by a numbered entry per comment, each carrying the same
/// minimal context as [`PrComment::fix_prompt`] (`file:line` pointer,
/// bot-stripped text, diff hunk) and, like it, **no file contents** (token
/// principle #3): the preamble and any repeated file context are paid once
/// across the whole set instead of once per comment. Injected once into the
/// dedicated review session so the agent works the list autonomously.
pub fn combined_fix_prompt(comments: &[&PrComment]) -> String {
    let mut out = String::from(
        "Address these PR review comments. Work through each one in order; \
         open the referenced files yourself as needed.\n",
    );
    for (i, comment) in comments.iter().enumerate() {
        out.push_str(&format!(
            "\nComment {}:\n{}\n",
            i + 1,
            comment.fix_prompt_body()
        ));
    }
    out.trim_end().to_string()
}

/// Build a fresh fix-confirm dialog seeded with `prompt`. The editor opens with
/// the vim keymap when `vim` is set (the pane-level remembered preference) so
/// reopening the dialog for another comment keeps the user's chosen keymap.
/// Build a fresh fix-confirm dialog. `batch` is `None` for an ordinary
/// single-comment fix and `Some(ids)` for the combined-batch flow (`B`), where
/// injecting marks every listed comment `Fixing`.
fn new_fix_confirm(prompt: String, vim: bool, batch: Option<Vec<u64>>) -> FixConfirmState {
    FixConfirmState {
        editor: if vim {
            TextEditor::with_vim(prompt)
        } else {
            TextEditor::new(prompt)
        },
        editing: false,
        scroll: 0,
        // Seed the view scrolled to the cursor (end of the prompt for plain,
        // start for vim) so a tall prompt opens somewhere sensible.
        sync_to_cursor: true,
        batch,
    }
}

/// A fully normalized PR review: the resolved PR plus every triageable comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrReview {
    pub pr: PrRef,
    pub comments: Vec<PrComment>,
    pub fetched_at: DateTime<Local>,
}

impl PrReview {
    /// Number of comments not yet resolved on GitHub.
    pub fn open_count(&self) -> usize {
        self.comments.iter().filter(|c| !c.is_resolved).count()
    }
}

/// Fetch every comment source for a resolved PR and normalize them into one
/// [`PrReview`]. This is the single entry point the UI/state layer calls to
/// (re)load a review; it runs entirely in Rust and spends zero agent tokens.
///
/// Run this off the UI thread — it makes four `gh` calls.
pub fn fetch_and_normalize(workdir: &Path, pr: PrRef) -> Result<PrReview> {
    let review_comments = GhCli::pr_review_comments(workdir, pr.number)?;
    let reviews = GhCli::pr_reviews(workdir, pr.number)?;
    let issue_comments = GhCli::issue_comments(workdir, pr.number)?;
    let threads = GhCli::review_threads(workdir, &pr.owner, &pr.repo, pr.number)?;
    Ok(normalize(
        pr,
        review_comments,
        reviews,
        issue_comments,
        threads,
    ))
}

/// Merge the raw `gh` payloads into a single triage-ready [`PrReview`].
///
/// Inline comments come first (they're the core use case), then non-empty
/// review summaries, then conversation comments. Resolution state is attached
/// from the GraphQL thread map; empty-body review summaries (bare approvals)
/// are dropped since there's nothing to triage.
pub fn normalize(
    pr: PrRef,
    review_comments: Vec<ReviewComment>,
    reviews: Vec<Review>,
    issue_comments: Vec<IssueComment>,
    threads: Vec<ReviewThread>,
) -> PrReview {
    let thread_index = index_threads(&threads);
    let mut comments = Vec::new();

    for c in review_comments {
        let is_bot = c.user.is_bot();
        let (thread_id, is_resolved) = match thread_index.get(&c.id) {
            Some((id, resolved)) => (Some(id.clone()), *resolved),
            None => (None, false),
        };
        let snippet = make_snippet(&c.body, is_bot);
        // A file-level comment has no line by definition — that's not the same
        // thing as an outdated line comment, so don't badge it as one.
        let file_level = c.subject_type.as_deref() == Some("file");
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Inline,
            author: c.user.login,
            is_bot,
            path: c.path,
            line: c.line.or(c.original_line),
            side: c.side,
            outdated: c.line.is_none() && !file_level,
            file_level,
            diff_hunk: c.diff_hunk,
            body: c.body,
            snippet,
            in_reply_to: c.in_reply_to_id,
            thread_id,
            is_resolved,
            triage: TriageState::default(),
            local_note: None,
            ai_generated: false,
        });
    }

    for r in reviews {
        // A review with no body is just an approve/comment action — nothing to
        // triage, so skip it.
        if r.body.trim().is_empty() {
            continue;
        }
        let is_bot = r.user.is_bot();
        let snippet = make_snippet(&r.body, is_bot);
        comments.push(PrComment {
            id: r.id,
            kind: CommentKind::ReviewSummary { state: r.state },
            author: r.user.login,
            is_bot,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: r.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
            ai_generated: false,
        });
    }

    for c in issue_comments {
        let is_bot = c.user.is_bot();
        let snippet = make_snippet(&c.body, is_bot);
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Conversation,
            author: c.user.login,
            is_bot,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: c.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
            ai_generated: false,
        });
    }

    PrReview {
        pr,
        comments,
        fetched_at: Local::now(),
    }
}

/// Build `comment_id -> (thread_node_id, is_resolved)` from the GraphQL threads.
fn index_threads(threads: &[ReviewThread]) -> HashMap<u64, (String, bool)> {
    let mut map = HashMap::new();
    for t in threads {
        for &cid in &t.comment_ids {
            map.insert(cid, (t.id.clone(), t.is_resolved));
        }
    }
    map
}

/// Produce a one-line list snippet, stripping bot boilerplate first so the
/// snippet reflects the actual content, not scaffolding.
fn make_snippet(body: &str, is_bot: bool) -> String {
    let cleaned = if is_bot {
        strip_bot_boilerplate(body)
    } else {
        body.to_string()
    };
    let first = cleaned
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    truncate_chars(first, SNIPPET_LEN)
}

/// Strip the heavy scaffolding bots (CodeRabbit, Copilot, …) wrap around their
/// actual point: `<details>` blocks, HTML comments, `<summary>` tags, markdown
/// image badges, fenced quoted-diff/suggestion blocks, and leading `> `
/// quoted-diff lines. Cheap and lossy-by-design — only the actionable prose
/// needs to survive for the agent prompt and snippet. The comment's own
/// `diff_hunk` (plus the checked-out repo) already gives the agent this
/// context, so a bot re-quoting the diff inline is pure repetition.
pub fn strip_bot_boilerplate(body: &str) -> String {
    static DETAILS: OnceLock<Regex> = OnceLock::new();
    static HTML_COMMENT: OnceLock<Regex> = OnceLock::new();
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static QUOTED_DIFF_FENCE: OnceLock<Regex> = OnceLock::new();
    static QUOTED_LINES: OnceLock<Regex> = OnceLock::new();
    static BLANKS: OnceLock<Regex> = OnceLock::new();

    let details = DETAILS.get_or_init(|| Regex::new(r"(?is)<details>.*?</details>").unwrap());
    let html_comment = HTML_COMMENT.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    let summary = SUMMARY.get_or_init(|| Regex::new(r"(?is)</?summary>").unwrap());
    let image = IMAGE.get_or_init(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    // Fenced ```diff / ```suggestion blocks: bots paste the same hunk back as
    // a code fence, which repeats context the agent already gets for free
    // from `diff_hunk`.
    let quoted_diff_fence = QUOTED_DIFF_FENCE
        .get_or_init(|| Regex::new(r"(?ims)^```(?:diff|suggestion)\s*\n.*?\n```\s*$").unwrap());
    // Leading `> ` blockquote lines (bots sometimes quote the diff as a
    // blockquote instead of a fence).
    let quoted_lines = QUOTED_LINES.get_or_init(|| Regex::new(r"(?m)^>.*$\n?").unwrap());
    let blanks = BLANKS.get_or_init(|| Regex::new(r"\n{3,}").unwrap());

    let s = details.replace_all(body, "");
    let s = html_comment.replace_all(&s, "");
    let s = summary.replace_all(&s, "");
    let s = image.replace_all(&s, "");
    let s = quoted_diff_fence.replace_all(&s, "");
    let s = quoted_lines.replace_all(&s, "");
    let s = blanks.replace_all(&s, "\n\n");
    s.trim().to_string()
}

/// Truncate to `max` characters (not bytes), appending an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Flatten one PR's review comments + review summaries into plain-text lines
/// for the lookback-bootstrap prompt (Epic E). Bot bodies are stripped like
/// everywhere else in this module; empty bodies (bare approvals, blank
/// comments) are dropped. Returns an empty string when the PR has nothing
/// worth feeding to the distiller.
fn bootstrap_pr_text(comments: &[ReviewComment], reviews: &[Review]) -> String {
    let mut lines = Vec::new();
    for c in comments {
        let text = if c.user.is_bot() {
            strip_bot_boilerplate(&c.body)
        } else {
            c.body.clone()
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let loc = match &c.path {
            Some(p) => match c.line.or(c.original_line) {
                Some(l) => format!("{p}:{l}"),
                None => p.clone(),
            },
            None => "general".to_string(),
        };
        lines.push(format!("- ({loc}) {}", text.replace('\n', " ")));
    }
    for r in reviews {
        let text = if r.user.is_bot() {
            strip_bot_boilerplate(&r.body)
        } else {
            r.body.clone()
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        lines.push(format!("- (review) {}", text.replace('\n', " ")));
    }
    lines.join("\n")
}

/// Assemble the one distill prompt from every PR's gathered text. Instructs
/// the agent to output the same `## Category` / `- bullet` shape
/// [`review_memory::append_finding`] writes, so the response can be fed
/// straight back through [`review_memory::parse_findings_markdown`] with no
/// further parsing.
fn bootstrap_prompt(pr_bodies: &[(u32, String, String)]) -> String {
    let mut out = String::from(
        "You are distilling recurring code-review findings from a project's PR \
         history into a durable list of lessons for future reviews.\n\n\
         Below are review comments and review summaries from several recent, \
         already-merged/closed pull requests. Identify findings that recur across \
         multiple PRs, or that state a general rule the team clearly cares about — \
         not a one-off nitpick specific to a single PR's code. Ignore praise, \
         procedural comments (\"LGTM\", \"done\"), and anything that reads as already \
         resolved.\n\n\
         Output ONLY a Markdown list grouped under `## Category` headings (categories \
         like General, Concurrency, Error handling, Naming, Tests, Performance, API \
         design, Style), one finding per `- ` bullet, phrased as a general rule (not \
         tied to a specific file, PR, or person). No prose outside the headings and \
         bullets.\n\n---\n\n",
    );
    for (number, title, body) in pr_bodies {
        out.push_str(&format!("### PR #{number}: {title}\n{body}\n\n"));
    }
    out.trim_end().to_string()
}

/// Background body of the lookback bootstrap (Epic E): fetch comments/reviews
/// for every listed PR (zero agent tokens), then make **one** headless agent
/// pass to cluster them into findings and append the new ones to the memory
/// doc. Runs off the UI thread; progress and the final result are reported
/// over `tx`. A single PR's fetch failure is skipped rather than aborting the
/// whole run — one stale/deleted PR shouldn't sink the batch.
fn run_review_memory_bootstrap(
    workdir: PathBuf,
    memory_path: PathBuf,
    entries: Vec<PrListEntry>,
    tx: std::sync::mpsc::Sender<BootstrapProgress>,
) {
    let mut pr_bodies = Vec::new();
    for entry in &entries {
        let comments = GhCli::pr_review_comments(&workdir, entry.number).unwrap_or_default();
        let reviews = GhCli::pr_reviews(&workdir, entry.number).unwrap_or_default();
        let text = bootstrap_pr_text(&comments, &reviews);
        if !text.is_empty() {
            pr_bodies.push((entry.number, entry.title.clone(), text));
        }
    }

    if pr_bodies.is_empty() {
        let _ = tx.send(BootstrapProgress::Done(Ok(BootstrapOutcome {
            pr_count: 0,
            appended: 0,
        })));
        return;
    }

    let prompt = bootstrap_prompt(&pr_bodies);
    let _ = tx.send(BootstrapProgress::Distilling {
        pr_count: pr_bodies.len(),
        token_estimate: estimate_tokens(&prompt),
    });

    let result = ClaudeLauncher::run_headless(&workdir, &prompt).and_then(|output| {
        let findings = review_memory::parse_findings_markdown(&output);
        let mut appended = 0;
        for (category, finding) in &findings {
            if review_memory::append_finding(&memory_path, category, finding)? {
                appended += 1;
            }
        }
        Ok(BootstrapOutcome {
            pr_count: pr_bodies.len(),
            appended,
        })
    });
    let _ = tx.send(BootstrapProgress::Done(result));
}

/// Background body of the AI PR review (`A` in the review pane, Epic E):
/// assemble the prompt from `diff` + `memory` (+ optional `skill`), report a
/// token estimate, then make **one** headless agent pass and parse its
/// response into findings. Runs off the UI thread; progress and the final
/// result are reported over `tx`.
fn run_ai_pr_review(
    workdir: PathBuf,
    diff: String,
    memory: String,
    skill: Option<String>,
    tx: std::sync::mpsc::Sender<AiReviewProgress>,
) {
    let prompt = ai_review_prompt(&diff, &memory, skill.as_deref());
    let _ = tx.send(AiReviewProgress::Reviewing {
        token_estimate: estimate_tokens(&prompt),
    });

    let result = ClaudeLauncher::run_headless(&workdir, &prompt).map(|output| {
        let mut findings = parse_ai_findings(&output);
        // Attach each anchored finding's diff hunk by re-matching its
        // `path:line` into the already-fetched PR diff — nothing about
        // generating a finding produces one the way GitHub's API does for a
        // fetched comment. A parse failure (malformed diff) just leaves every
        // finding without a hunk rather than failing the whole review.
        if let Ok(files) = crate::diff::parse_unified_diff(&diff) {
            for finding in &mut findings {
                if let (Some(path), Some(line)) = (&finding.path, finding.line) {
                    finding.diff_hunk = diff_hunk_for_line(&files, path, line);
                }
            }
        }
        AiReviewOutcome {
            findings,
            raw_output: output,
        }
    });
    let _ = tx.send(AiReviewProgress::Done(result));
}

impl App {
    /// Open the PR comment-review pane for the selected feature's branch.
    ///
    /// Runs the `gh` preconditions and resolves the PR synchronously (cheap),
    /// then kicks the comment fetch onto a background thread. All of this
    /// spends zero agent tokens.
    pub fn open_pr_review(&mut self) {
        let Some((_project, feature)) = self.selected_feature() else {
            self.message = Some("Select a feature to review its PR".to_string());
            return;
        };
        let workdir = feature.workdir.clone();

        if let Err(e) = GhCli::check_available() {
            self.show_error(e);
            return;
        }
        if let Err(e) = GhCli::check_auth() {
            self.show_error(e);
            return;
        }

        match GhCli::resolve_pr(&workdir) {
            Ok(PrResolution::Found(pr)) => self.enter_pr_review(workdir, pr),
            // No PR for this branch: offer a list of the repo's PRs to pick from
            // (the picker falls through to the number prompt on its own if the
            // list can't be fetched).
            Ok(PrResolution::NoPrForBranch) => self.open_pr_picker(workdir, None),
            Err(e) => self.show_error(e),
        }
    }

    /// Open the review pane for a resolved PR, preferring the SQLite cache.
    ///
    /// A cache hit (same `PR# + head SHA`) skips the four `gh` calls entirely and
    /// shows the stored comments instantly; a miss falls back to the background
    /// fetch. Either path spends zero agent tokens. Manual refresh
    /// ([`refresh_pr_review`](Self::refresh_pr_review)) bypasses the cache.
    fn enter_pr_review(&mut self, workdir: PathBuf, pr: PrRef) {
        if let Some(mut review) = self.load_cached_pr_review(&pr) {
            self.log_info(
                "pr_review",
                format!("cache hit for PR #{} @ {}", pr.number, pr.head_sha),
            );
            self.apply_persisted_triage(&mut review);
            self.mode = AppMode::PrReview(PrReviewState {
                workdir,
                review,
                selected: 0,
                detail_scroll: 0,
                detail_content_lines: 0,
                hide_resolved: false,
                sort_mode: PrSortMode::default(),
                fix_target: FixTarget::default(),
                review_harness: None,
                harness_pick: None,
                fix_confirm: None,
                fix_vim_enabled: false,
                reply: None,
                memory_add: None,
                marked: std::collections::HashSet::new(),
                pending_batch: false,
                ai_review_post: None,
            });
            return;
        }
        self.start_pr_review_fetch(workdir, pr);
    }

    /// Look up a cached, normalized review for this PR's head SHA. Returns `None`
    /// on a miss, when there's no DB, or when the cache read fails (non-fatal —
    /// the caller just re-fetches).
    fn load_cached_pr_review(&self, pr: &PrRef) -> Option<PrReview> {
        self.db
            .as_ref()?
            .load_pr_review_cache(pr.number, &pr.head_sha)
            .ok()
            .flatten()
    }

    /// Persist a freshly-fetched review under its `PR# + head SHA` key so the
    /// next open is a cache hit. A write failure is non-fatal (logged, not shown).
    fn cache_pr_review(&mut self, review: &PrReview) {
        let result = match self.db.as_ref() {
            Some(db) => db.save_pr_review_cache(review),
            None => return,
        };
        if let Err(e) = result {
            self.log_warn("pr_review", format!("cache write failed: {e}"));
        }
    }

    /// Overlay the persisted local triage (`Fixing`/`Done`/skip notes) onto a
    /// freshly-loaded review. The `pr_comment_triage` table — keyed by
    /// `PR# + comment id` (not the head SHA, so marks survive a push that moves
    /// the PR's head) — is authoritative for local triage, so it wins over
    /// whatever the cache blob happened to serialize. A read failure (or no DB)
    /// is non-fatal: comments just stay [`TriageState::Untriaged`].
    fn apply_persisted_triage(&mut self, review: &mut PrReview) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let triage = match db.load_pr_comment_triage(review.pr.number) {
            Ok(map) => map,
            Err(e) => {
                self.log_warn("pr_review", format!("triage load failed: {e}"));
                return;
            }
        };
        for comment in &mut review.comments {
            if let Some((state, note)) = triage.get(&comment.id) {
                comment.triage = *state;
                comment.local_note = note.clone();
            }
        }
    }

    /// Persist one comment's triage state (with an optional note) to SQLite. A
    /// write failure is non-fatal (logged, not surfaced).
    fn persist_triage(
        &mut self,
        pr_number: u32,
        head_sha: &str,
        comment_id: u64,
        state: TriageState,
        note: Option<&str>,
    ) {
        let result = match self.db.as_ref() {
            Some(db) => db.save_pr_comment_triage(pr_number, head_sha, comment_id, state, note),
            None => return,
        };
        if let Err(e) = result {
            self.log_warn("pr_review", format!("triage persist failed: {e}"));
        }
    }

    /// Set the selected comment's triage state in-memory and persist it. The
    /// comment keeps its existing `local_note`. No-op outside the review pane or
    /// with no selection.
    fn pr_review_set_triage(&mut self, state: TriageState) {
        let Some((pr_number, head_sha, comment_id, note)) = ({
            let AppMode::PrReview(s) = &mut self.mode else {
                return;
            };
            s.review.comments.get_mut(s.selected).map(|c| {
                c.triage = state;
                (
                    s.review.pr.number,
                    s.review.pr.head_sha.clone(),
                    c.id,
                    c.local_note.clone(),
                )
            })
        }) else {
            return;
        };
        self.persist_triage(pr_number, &head_sha, comment_id, state, note.as_deref());
    }

    /// Mark the selected comment done (toggles back to untriaged if it already
    /// is). Manual, with **no auto-advance** — the user stays on the comment so
    /// they can review the agent's work before moving on (plan: Epic B).
    pub fn pr_review_mark_done(&mut self) {
        let next = match self.pr_review_selected_triage() {
            Some(TriageState::Done) => TriageState::Untriaged,
            Some(_) => TriageState::Done,
            None => return,
        };
        self.pr_review_set_triage(next);
        let msg = match next {
            TriageState::Done => "Marked done",
            _ => "Cleared — back to untriaged",
        };
        self.push_toast_success(msg.to_string());
    }

    /// Skip the selected comment locally (toggles back to untriaged if already
    /// skipped). Local-only — no GitHub write, no agent tokens.
    pub fn pr_review_skip(&mut self) {
        let next = match self.pr_review_selected_triage() {
            Some(TriageState::Skipped) => TriageState::Untriaged,
            Some(_) => TriageState::Skipped,
            None => return,
        };
        self.pr_review_set_triage(next);
        let msg = match next {
            TriageState::Skipped => "Skipped",
            _ => "Cleared — back to untriaged",
        };
        self.push_toast_success(msg.to_string());
    }

    /// Triage state of the currently-selected comment, if any.
    fn pr_review_selected_triage(&self) -> Option<TriageState> {
        match &self.mode {
            AppMode::PrReview(s) => s.selected_comment().map(|c| c.triage),
            _ => None,
        }
    }

    /// Toggle whether the selected comment is marked for a batch fix (`space`).
    /// Marks are kept by comment id, so they survive the hide-resolved filter
    /// shifting the visible rows. No-op with no selection.
    pub fn pr_review_toggle_mark(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        let Some(id) = state.selected_comment().map(|c| c.id) else {
            return;
        };
        let now_marked = if state.marked.remove(&id) {
            false
        } else {
            state.marked.insert(id);
            true
        };
        let count = state.marked.len();
        self.message = Some(if now_marked {
            format!("Marked for batch fix ({count} marked)")
        } else if count == 0 {
            "Unmarked — nothing marked".to_string()
        } else {
            format!("Unmarked ({count} still marked)")
        });
    }

    /// Queue a fix prompt for every marked comment into the **one** review
    /// session, in list order, without leaving the pane — the throughput loop:
    /// the harness works through them (pasted + submitted, so they queue while
    /// it's busy) while the user keeps triaging. Each is a separate prompt
    /// (distinct from the combined-prompt batch), sharing the session's warm
    /// file context. Marked comments that are already GitHub-resolved are skipped
    /// (token principle #6). Requires the review session to already exist — the
    /// first fix (`f`) establishes and warms it; this never cold-starts a session
    /// to auto-submit into. Each queued comment is marked `Fixing` and persisted;
    /// the marked set is cleared on success.
    pub fn pr_review_queue_marked_fixes(&mut self) -> Result<()> {
        // Assemble the queue: marked, not-yet-resolved comments in list order.
        let (pr_number, head_sha, workdir, target, queue) = match &self.mode {
            AppMode::PrReview(state) => {
                if state.marked.is_empty() {
                    self.message = Some("No comments marked — press space to mark".into());
                    return Ok(());
                }
                let queue: Vec<(u64, String)> = state
                    .review
                    .comments
                    .iter()
                    .filter(|c| state.marked.contains(&c.id) && !c.is_resolved)
                    .map(|c| (c.id, c.fix_prompt()))
                    .collect();
                (
                    state.review.pr.number,
                    state.review.pr.head_sha.clone(),
                    state.workdir.clone(),
                    state.fix_target,
                    queue,
                )
            }
            _ => return Ok(()),
        };
        if queue.is_empty() {
            self.message = Some("Marked comments are all resolved — nothing to queue".into());
            return Ok(());
        }

        // Resolve the warm session — must already exist (no cold-start submit).
        let Some((pi, fi)) = self.feature_indices_for_workdir(&workdir) else {
            self.message = Some("Could not find the feature for this PR".into());
            return Ok(());
        };
        let feature = &self.store.projects[pi].features[fi];
        let Some(si) = fix_session_index(feature, target, REVIEW_SESSION_LABEL) else {
            self.message = Some(
                "No review session yet — press f on a comment to start one, then F to queue the rest"
                    .into(),
            );
            return Ok(());
        };
        let session = feature.tmux_session.clone();
        let window = feature.sessions[si].tmux_window.clone();

        // Send each prompt (clear stray input, paste, submit). The pause lets the
        // harness register each submission before the next paste.
        let count = queue.len();
        for (i, (_id, prompt)) in queue.iter().enumerate() {
            if i > 0 {
                std::thread::sleep(BATCH_FIX_SUBMIT_DELAY);
            }
            self.tmux.send_key_name(&session, &window, "C-u")?;
            self.tmux.paste_text(&session, &window, prompt)?;
            self.tmux.send_key_name(&session, &window, "Enter")?;
        }

        // Mark each queued comment `Fixing` and persist; then clear the marks.
        for (id, _) in &queue {
            if let AppMode::PrReview(state) = &mut self.mode
                && let Some(c) = state.review.comments.iter_mut().find(|c| c.id == *id)
            {
                c.triage = TriageState::Fixing;
            }
            self.persist_triage(pr_number, &head_sha, *id, TriageState::Fixing, None);
        }
        if let AppMode::PrReview(state) = &mut self.mode {
            state.marked.clear();
        }
        self.push_toast_success(format!(
            "Queued {count} fix{} into the review session",
            if count == 1 { "" } else { "es" }
        ));
        Ok(())
    }

    /// Open the PR picker: a selectable list of the repo's PRs. `seed_number`
    /// pre-highlights that PR when present (e.g. the branch's auto-detected one,
    /// or the PR already open in the pane). Lists open PRs by default. If `gh pr
    /// list` fails outright, falls back to the manual number prompt so the user
    /// is never stuck. Zero agent tokens.
    pub fn open_pr_picker(&mut self, workdir: PathBuf, seed_number: Option<u32>) {
        match GhCli::list_prs(&workdir, false) {
            Ok(entries) => {
                let selected = seed_number
                    .and_then(|n| entries.iter().position(|e| e.number == n))
                    .unwrap_or(0);
                let current_user = self.resolve_gh_current_user(&workdir);
                self.mode = AppMode::PrPicker(PrPickerState {
                    workdir,
                    entries,
                    selected,
                    include_closed: false,
                    error: None,
                    bootstrap_pick: None,
                    current_user,
                });
            }
            Err(e) => {
                self.log_warn("pr_review", format!("pr list failed: {e}"));
                self.prompt_pr_number(workdir, Some(e.to_string()));
            }
        }
    }

    /// Resolve the authenticated `gh` user's login, memoized in
    /// [`App::gh_current_user`] for the session so the PR picker doesn't
    /// repeat the `gh api user` call on every open/refresh. A failed
    /// resolution (e.g. `gh` unauthenticated) is cached too, rather than
    /// retried on every call.
    pub(crate) fn resolve_gh_current_user(&mut self, workdir: &Path) -> Option<String> {
        if let Some(cached) = &self.gh_current_user {
            return cached.clone();
        }
        let resolved = match GhCli::current_user(workdir) {
            Ok(login) => Some(login),
            Err(e) => {
                self.log_warn("pr_review", format!("could not resolve gh user: {e}"));
                None
            }
        };
        self.gh_current_user = Some(resolved.clone());
        resolved
    }

    /// Open the PR picker from inside the review pane (the `g` key), seeded on the
    /// PR currently being reviewed so it starts highlighted.
    pub fn open_pr_picker_from_pane(&mut self) {
        let (workdir, current) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), Some(state.review.pr.number)),
            AppMode::PrReviewLoading(state) => (state.workdir.clone(), Some(state.pr.number)),
            _ => return,
        };
        self.open_pr_picker(workdir, current);
    }

    /// Move the picker highlight down one row (clamped).
    pub fn pr_picker_select_next(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode
            && !state.entries.is_empty()
        {
            state.selected = (state.selected + 1).min(state.entries.len() - 1);
        }
    }

    /// Move the picker highlight up one row (clamped).
    pub fn pr_picker_select_prev(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.selected = state.selected.saturating_sub(1);
        }
    }

    /// Toggle whether the picker list includes closed/merged PRs, re-fetching
    /// with the new filter. Keeps the highlight on the same PR number when it
    /// survives the toggle.
    pub fn pr_picker_toggle_closed(&mut self) {
        let (workdir, include_closed, current) = match &self.mode {
            AppMode::PrPicker(state) => (
                state.workdir.clone(),
                !state.include_closed,
                state.entries.get(state.selected).map(|e| e.number),
            ),
            _ => return,
        };
        match GhCli::list_prs(&workdir, include_closed) {
            Ok(entries) => {
                let selected = current
                    .and_then(|n| entries.iter().position(|e| e.number == n))
                    .unwrap_or(0);
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.entries = entries;
                    state.selected = selected;
                    state.include_closed = include_closed;
                    state.error = None;
                }
            }
            Err(e) => {
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.error = Some(e.to_string());
                }
            }
        }
    }

    /// Resolve the highlighted PR (by number) and open it for review. On a
    /// resolve failure the picker stays open with an inline error.
    pub fn pr_picker_choose(&mut self) {
        let (workdir, number) = match &self.mode {
            AppMode::PrPicker(state) => match state.entries.get(state.selected) {
                Some(entry) => (state.workdir.clone(), entry.number),
                None => return,
            },
            _ => return,
        };
        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.enter_pr_review(workdir, pr),
            Err(e) => {
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.error = Some(e.to_string());
                }
            }
        }
    }

    /// Switch from the picker to the manual PR-number prompt (the `#` key), so
    /// "pick a PR" and "type a number" live behind one entry point.
    pub fn pr_picker_to_number_prompt(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrPicker(state) => state.workdir.clone(),
            _ => return,
        };
        self.prompt_pr_number(workdir, None);
    }

    /// Open the manual PR-number override prompt. Used when the branch has no
    /// auto-detectable open PR; `error` seeds an inline message after a failed
    /// resolve so the user can correct the number and retry.
    fn prompt_pr_number(&mut self, workdir: PathBuf, error: Option<String>) {
        self.mode = AppMode::PrNumberPrompt(PrNumberPromptState {
            workdir,
            input: String::new(),
            error,
        });
    }

    /// Append a digit to the PR-number prompt (non-digits are ignored).
    pub fn pr_number_prompt_push(&mut self, c: char) {
        if let AppMode::PrNumberPrompt(state) = &mut self.mode
            && c.is_ascii_digit()
        {
            state.input.push(c);
        }
    }

    /// Delete the last digit from the PR-number prompt.
    pub fn pr_number_prompt_backspace(&mut self) {
        if let AppMode::PrNumberPrompt(state) = &mut self.mode {
            state.input.pop();
        }
    }

    /// Resolve the typed PR number and, on success, start the comment fetch.
    /// On failure the prompt stays open with an inline error so the user can
    /// retry. Spends zero agent tokens (one `gh pr view <n>` call).
    pub fn submit_pr_number(&mut self) {
        let AppMode::PrNumberPrompt(state) = &self.mode else {
            return;
        };
        let workdir = state.workdir.clone();
        let Ok(number) = state.input.trim().parse::<u32>() else {
            self.prompt_pr_number(workdir, Some("Enter a PR number, e.g. 321".to_string()));
            return;
        };

        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.enter_pr_review(workdir, pr),
            Err(e) => self.prompt_pr_number(workdir, Some(e.to_string())),
        }
    }

    /// Re-fetch the currently-open PR, bypassing the cache. Re-resolves the PR
    /// first so a new head SHA (e.g. after pushing fixes) is picked up, then
    /// fetches fresh comments and overwrites the cache row. Zero agent tokens.
    pub fn refresh_pr_review(&mut self) {
        let (workdir, number) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.number),
            _ => return,
        };
        self.log_info("pr_review", format!("refreshing PR #{number}"));
        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.start_pr_review_fetch(workdir, pr),
            Err(e) => self.show_error(e),
        }
    }

    /// Spawn the off-thread comment fetch and enter the loading mode.
    fn start_pr_review_fetch(&mut self, workdir: PathBuf, pr: PrRef) {
        self.log_info(
            "pr_review",
            format!("fetching comments for PR #{}", pr.number),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        self.pr_review_bg = Some(rx);

        let thread_workdir = workdir.clone();
        let thread_pr = pr.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_and_normalize(&thread_workdir, thread_pr));
        });

        self.mode = AppMode::PrReviewLoading(PrReviewLoadState { workdir, pr });
    }

    /// Whether a PR comment fetch is in flight.
    pub fn pr_review_loading(&self) -> bool {
        matches!(self.mode, AppMode::PrReviewLoading(_))
    }

    /// Poll the background PR fetch. On completion, transition to the review
    /// pane (or report the error and return to the dashboard). Returns `true`
    /// when state changed and a redraw is warranted.
    pub fn poll_pr_review_bg(&mut self) -> bool {
        let Some(rx) = self.pr_review_bg.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.pr_review_bg = None;
                // If the user navigated away from the loading screen, drop it.
                let AppMode::PrReviewLoading(state) = &self.mode else {
                    return false;
                };
                let workdir = state.workdir.clone();
                match result {
                    Ok(mut review) => {
                        self.log_info(
                            "pr_review",
                            format!("loaded {} comments", review.comments.len()),
                        );
                        self.carry_forward_ai_drafts(&mut review);
                        self.cache_pr_review(&review);
                        self.apply_persisted_triage(&mut review);
                        self.mode = AppMode::PrReview(PrReviewState {
                            workdir,
                            review,
                            selected: 0,
                            detail_scroll: 0,
                            detail_content_lines: 0,
                            hide_resolved: false,
                            sort_mode: PrSortMode::default(),
                            fix_target: FixTarget::default(),
                            review_harness: None,
                            harness_pick: None,
                            fix_confirm: None,
                            fix_vim_enabled: false,
                            reply: None,
                            memory_add: None,
                            marked: std::collections::HashSet::new(),
                            pending_batch: false,
                            ai_review_post: None,
                        });
                    }
                    Err(e) => {
                        self.mode = AppMode::Normal;
                        self.show_error(e);
                    }
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_review_bg = None;
                if matches!(self.mode, AppMode::PrReviewLoading(_)) {
                    self.mode = AppMode::Normal;
                    self.message = Some("PR fetch failed unexpectedly".to_string());
                    return true;
                }
                false
            }
        }
    }

    /// Close the review pane / cancel a pending load and return to the dashboard.
    pub fn close_pr_review(&mut self) {
        self.pr_review_bg = None;
        self.mode = AppMode::Normal;
    }

    /// Before overwriting the cache with a freshly-fetched review, carry
    /// forward any AI-review draft findings ([`findings_to_comments`]) from
    /// the previous cache row at the *same* `PR# + head SHA` — a manual
    /// refresh ([`refresh_pr_review`]) re-fetches from GitHub but shouldn't
    /// silently drop unposted AI drafts the user hasn't triaged yet. A
    /// different head SHA (the PR moved, e.g. after a push) means the old
    /// drafts reviewed code that's since changed, so they're intentionally
    /// left behind — re-run the AI review (`A`) against the new diff instead.
    ///
    /// [`refresh_pr_review`]: Self::refresh_pr_review
    fn carry_forward_ai_drafts(&self, review: &mut PrReview) {
        let Some(previous) = self.load_cached_pr_review(&review.pr) else {
            return;
        };
        let drafts = previous.comments.into_iter().filter(|c| c.ai_generated);
        review.comments.extend(drafts);
    }

    /// Kick off the AI PR review (`A` in the review pane, Epic E): resolve the
    /// review-memory doc synchronously (cheap, a local file read), then hand
    /// the PR-diff fetch and the one paid agent pass to a background thread
    /// and switch to the full-screen running view. Any dialog open on the
    /// pane is cleared before stashing — the same precaution as the `f`/`P`
    /// stash (reopening a since-actioned dialog on return would be stale).
    /// No-op (with a hint) if a review is already running — e.g. the user
    /// cancelled back to the pane with `esc` and pressed `A` again before the
    /// first pass finished — rather than orphaning it for a second one.
    pub fn start_ai_pr_review(&mut self) {
        if self.ai_review_bg.is_some() {
            self.push_toast_warning("AI review already running — wait for it to finish");
            return;
        }
        let mut origin = match &self.mode {
            AppMode::PrReview(state) => state.clone(),
            _ => return,
        };
        origin.harness_pick = None;
        origin.fix_confirm = None;
        origin.reply = None;
        origin.memory_add = None;

        let workdir = origin.workdir.clone();
        let number = origin.review.pr.number;
        self.log_info("pr_review", format!("starting AI review of PR #{number}"));

        let repo = self.repo_for_project_path(&workdir);
        let memory_path =
            review_memory::review_memory_path(&repo, self.config.review_memory_path.as_deref());
        let memory = std::fs::read_to_string(&memory_path).unwrap_or_default();
        let skill = self.config.ai_review_skill.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_review_bg = Some(rx);
        self.ai_review_pending = Some(origin.clone());
        let thread_workdir = workdir.clone();
        std::thread::spawn(move || match GhCli::pr_diff(&thread_workdir, number) {
            Ok(diff) => run_ai_pr_review(thread_workdir, diff, memory, skill, tx),
            Err(e) => {
                let _ = tx.send(AiReviewProgress::Done(Err(e)));
            }
        });

        self.mode = AppMode::AiPrReviewRunning(AiReviewRunState {
            origin,
            stage: AiReviewStage::PreparingDiff,
        });
    }

    /// Poll the background AI PR review. Progress updates the running
    /// screen's stage; `Done` merges the new draft findings into whichever
    /// pane state is the right target (see the `Target` enum below) — this
    /// may run well after `esc` (`cancel_ai_pr_review`) has already returned
    /// the user to the pane, since the background pass keeps going — then
    /// re-caches and surfaces a toast (success/warning) or error (a real
    /// side effect, tokens spent, even if the user already navigated away).
    /// Returns `true` when a redraw is warranted.
    pub fn poll_ai_pr_review_bg(&mut self) -> bool {
        let Some(rx) = self.ai_review_bg.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(AiReviewProgress::Reviewing { token_estimate }) => {
                    if let AppMode::AiPrReviewRunning(state) = &mut self.mode {
                        state.stage = AiReviewStage::Reviewing { token_estimate };
                    }
                    changed = true;
                }
                Ok(AiReviewProgress::Done(result)) => {
                    self.ai_review_bg = None;
                    let Some(pending) = self.ai_review_pending.take() else {
                        // Invariant: always set alongside `ai_review_bg` in
                        // `start_ai_pr_review`. If it's ever missing there's
                        // nowhere safe to merge into — just surface the error.
                        if let Err(e) = result {
                            self.show_error(e);
                        }
                        changed = true;
                        break;
                    };
                    let pr_number = pending.review.pr.number;

                    // Where to land the result, resolved *before* any
                    // mode-mutating side effect (`show_error` resets
                    // `self.mode` to `Normal` for anything but
                    // Normal/Help/Viewing) so both the success and error
                    // paths can restore sensibly:
                    // - `Running`: still on the running screen for this PR —
                    //   merge and land in the pane.
                    // - `Pane`: the user already backed out (`esc`) to this
                    //   PR's pane — possibly triaging further in the
                    //   meantime, so merge into *that* live state, not the
                    //   stale pre-`A` snapshot (`pending`). This is the case
                    //   that used to silently drop the findings entirely.
                    // - `Elsewhere`: navigated away — a different PR, closed
                    //   the pane, opened another dialog. Nowhere live to
                    //   show it; cache only and leave `self.mode` alone.
                    enum Target {
                        Running,
                        Pane(Box<PrReviewState>),
                        Elsewhere,
                    }
                    let target = match &self.mode {
                        AppMode::AiPrReviewRunning(state)
                            if state.origin.review.pr.number == pr_number =>
                        {
                            Target::Running
                        }
                        AppMode::PrReview(state) if state.review.pr.number == pr_number => {
                            Target::Pane(Box::new(state.clone()))
                        }
                        _ => Target::Elsewhere,
                    };

                    let mut landed: Option<PrReviewState> = None;
                    match result {
                        Ok(outcome) => {
                            let count = outcome.findings.len();
                            // A model that ignores the fixed-format
                            // instruction parses to zero findings with no
                            // other signal — log the raw response so that
                            // case is diagnosable (press `D` for the debug
                            // log) instead of looking identical to "the diff
                            // was just clean".
                            if count == 0 && !outcome.raw_output.trim().is_empty() {
                                self.log_warn(
                                    "pr_review",
                                    format!(
                                        "AI review of PR #{pr_number} parsed 0 findings from a \
                                         non-empty response ({} chars) — raw output:\n{}",
                                        outcome.raw_output.len(),
                                        outcome.raw_output
                                    ),
                                );
                            } else {
                                self.log_debug(
                                    "pr_review",
                                    format!(
                                        "AI review of PR #{pr_number} parsed {count} finding{} \
                                         from {} chars of output",
                                        if count == 1 { "" } else { "s" },
                                        outcome.raw_output.len()
                                    ),
                                );
                            }

                            let mut base = match &target {
                                Target::Running => {
                                    let AppMode::AiPrReviewRunning(state) = &self.mode else {
                                        unreachable!()
                                    };
                                    state.origin.clone()
                                }
                                Target::Pane(state) => (**state).clone(),
                                Target::Elsewhere => pending.clone(),
                            };
                            base.review.comments.retain(|c| !c.ai_generated);
                            base.review
                                .comments
                                .extend(findings_to_comments(&outcome.findings));
                            self.cache_pr_review(&base.review);
                            self.apply_persisted_triage(&mut base.review);

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
                                self.push_toast_warning(format!(
                                    "AI review found 0 findings{note} — press D to check the \
                                     debug log"
                                ));
                            } else {
                                self.push_toast_success(format!(
                                    "AI review found {count} finding{}{note}",
                                    if count == 1 { "" } else { "s" }
                                ));
                            }
                        }
                        Err(e) => {
                            // Preserve whatever live pane state exists before
                            // `show_error` resets `self.mode` to `Normal`.
                            landed = match &target {
                                Target::Running => {
                                    let AppMode::AiPrReviewRunning(state) = &self.mode else {
                                        unreachable!()
                                    };
                                    Some(state.origin.clone())
                                }
                                Target::Pane(state) => Some((**state).clone()),
                                Target::Elsewhere => None,
                            };
                            self.show_error(e);
                        }
                    }
                    if let Some(state) = landed {
                        self.mode = AppMode::PrReview(state);
                    }
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ai_review_bg = None;
                    let pending = self.ai_review_pending.take();
                    let pr_number = pending.as_ref().map(|p| p.review.pr.number);
                    match &self.mode {
                        AppMode::AiPrReviewRunning(state)
                            if Some(state.origin.review.pr.number) == pr_number =>
                        {
                            self.mode = AppMode::PrReview(state.origin.clone());
                            self.message = Some("AI review failed unexpectedly".to_string());
                            changed = true;
                        }
                        AppMode::PrReview(state) if Some(state.review.pr.number) == pr_number => {
                            self.message = Some("AI review failed unexpectedly".to_string());
                            changed = true;
                        }
                        _ => {}
                    }
                    break;
                }
            }
        }
        changed
    }

    /// Cancel the running screen (`esc`/`q`): return to the review pane. The
    /// background thread isn't aborted — if it finishes later,
    /// [`App::poll_ai_pr_review_bg`] still surfaces the result (via
    /// [`App::ai_review_pending`], which survives this).
    pub fn cancel_ai_pr_review(&mut self) {
        if let AppMode::AiPrReviewRunning(state) = &self.mode {
            self.mode = AppMode::PrReview(state.origin.clone());
        }
    }

    pub fn pr_review_select_next(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            let next = match visible.iter().position(|&i| i == state.selected) {
                Some(pos) => visible.get(pos + 1).copied(),
                None => visible.first().copied(),
            };
            if let Some(next) = next {
                state.selected = next;
                state.detail_scroll = 0;
            }
        }
    }

    pub fn pr_review_select_prev(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            let prev = match visible.iter().position(|&i| i == state.selected) {
                Some(0) => None,
                Some(pos) => visible.get(pos - 1).copied(),
                None => visible.last().copied(),
            };
            if let Some(prev) = prev {
                state.selected = prev;
                state.detail_scroll = 0;
            }
        }
    }

    /// Toggle hiding GitHub-resolved comments. When the current selection
    /// becomes hidden, snap to its nearest remaining visible neighbor in sort
    /// order (falling back to the closest one before it, then the first
    /// visible comment).
    pub fn pr_review_toggle_resolved(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.hide_resolved = !state.hide_resolved;
            let visible = state.visible_indices();
            if visible.is_empty() {
                return;
            }
            if !visible.contains(&state.selected) {
                let order = state.all_sorted_indices();
                let pos = order.iter().position(|&i| i == state.selected);
                let snapped = pos
                    .and_then(|p| order[p..].iter().find(|i| visible.contains(i)))
                    .or_else(|| {
                        pos.and_then(|p| order[..p].iter().rev().find(|i| visible.contains(i)))
                    })
                    .copied()
                    .unwrap_or(visible[0]);
                state.selected = snapped;
                state.detail_scroll = 0;
            }
        }
    }

    /// Cycle the comment list's sort order (`o`): fetch order → by file → by
    /// author → humans-first → back to fetch order. Independent of the
    /// `hide_resolved` filter.
    pub fn pr_review_cycle_sort(&mut self) {
        let label = {
            let AppMode::PrReview(state) = &mut self.mode else {
                return;
            };
            state.sort_mode = state.sort_mode.next();
            state.sort_mode.label()
        };
        self.push_toast_success(format!("Sort: {label}"));
    }

    /// Toggle which agent session "fix" prompts are injected into: the default
    /// dedicated review session, or the feature's existing live session.
    pub fn pr_review_toggle_fix_target(&mut self) {
        let label = {
            let AppMode::PrReview(state) = &mut self.mode else {
                return;
            };
            state.fix_target = match state.fix_target {
                FixTarget::DedicatedReview => FixTarget::ExistingLive,
                FixTarget::ExistingLive => FixTarget::DedicatedReview,
            };
            state.fix_target.label()
        };
        self.push_toast_success(format!("Fixes target the {label}"));
    }

    /// Open the fix confirm/edit dialog for the selected comment. Assembles the
    /// minimal fix prompt and shows it for review (with a `~N tokens` preview)
    /// before anything reaches the agent — nothing is injected until the user
    /// confirms. Editing is opt-in (`e`) from the dialog.
    ///
    /// For the dedicated-review target, the first fix of a PR first opens the
    /// harness picker (which harness the review session should run) — the fix
    /// confirm follows once the user picks. Subsequent fixes (harness already
    /// chosen, or a dedicated session already exists) go straight to the dialog.
    pub fn pr_review_open_fix_confirm(&mut self) {
        if self.pr_review_needs_harness_pick() {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.pending_batch = false;
            }
            self.pr_review_open_harness_pick();
            return;
        }
        self.pr_review_show_fix_confirm();
    }

    /// Build and open the single-comment fix confirm dialog for the selected
    /// comment. Assumes any harness pick has already happened (callers gate it),
    /// so it never re-opens the picker.
    fn pr_review_show_fix_confirm(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        state.pending_batch = false;
        let Some(comment) = state.selected_comment() else {
            self.message = Some("No comment selected".into());
            return;
        };
        let prompt = comment.fix_prompt();
        let vim = state.fix_vim_enabled;
        state.fix_confirm = Some(new_fix_confirm(prompt, vim, None));
    }

    /// Open the **combined-batch** confirm dialog (`B`): assemble one numbered
    /// prompt from every marked, not-yet-resolved comment and show it (with a
    /// `~N tokens` preview and editing) before injecting it once into the
    /// dedicated review session — the "fix all of these, then I'll come back"
    /// flow. Requires a non-empty marked set (`space` to mark); like a single
    /// fix, the first fix of a dedicated-review PR picks the harness first.
    pub fn pr_review_open_batch_confirm(&mut self) {
        // A marked, not-all-resolved set is required before we touch the harness
        // picker or build anything.
        let valid = match &self.mode {
            AppMode::PrReview(state) => {
                if state.marked.is_empty() {
                    self.message = Some("No comments marked — press space to mark".into());
                    return;
                }
                state
                    .review
                    .comments
                    .iter()
                    .filter(|c| state.marked.contains(&c.id) && !c.is_resolved)
                    .count()
            }
            _ => return,
        };
        if valid == 0 {
            self.message = Some("Marked comments are all resolved — nothing to batch".into());
            return;
        }
        // The dedicated-review target picks a harness before the first fix, same
        // as a single fix; route the picker's continuation back to the batch.
        if self.pr_review_needs_harness_pick() {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.pending_batch = true;
            }
            self.pr_review_open_harness_pick();
            return;
        }
        self.pr_review_show_batch_confirm();
    }

    /// Build and open the combined-batch confirm dialog. Assumes the marked set
    /// was already validated and any harness pick has happened.
    fn pr_review_show_batch_confirm(&mut self) {
        let built = match &self.mode {
            AppMode::PrReview(state) => {
                let selected: Vec<&PrComment> = state
                    .review
                    .comments
                    .iter()
                    .filter(|c| state.marked.contains(&c.id) && !c.is_resolved)
                    .collect();
                (!selected.is_empty()).then(|| {
                    let ids: Vec<u64> = selected.iter().map(|c| c.id).collect();
                    (combined_fix_prompt(&selected), ids)
                })
            }
            _ => return,
        };
        let Some((prompt, ids)) = built else {
            self.message = Some("Marked comments are all resolved — nothing to batch".into());
            return;
        };

        // Keep the set bounded: warn (but don't block) past the soft ceilings so
        // the user knows a single prompt this large may exceed the context window.
        let count = ids.len();
        let tokens = estimate_tokens(&prompt);
        if count > BATCH_COMBINED_COMMENT_WARN || tokens > BATCH_COMBINED_TOKEN_WARN {
            self.push_toast_warning(format!(
                "Large batch: {count} comments (~{tokens} tokens) in one prompt — may exceed the agent's context window"
            ));
        }

        if let AppMode::PrReview(state) = &mut self.mode {
            state.pending_batch = false;
            let vim = state.fix_vim_enabled;
            state.fix_confirm = Some(new_fix_confirm(prompt, vim, Some(ids)));
        }
    }

    /// Whether the first `f` should pick a harness before injecting: only for
    /// the dedicated-review target, when no harness has been chosen yet *and* no
    /// dedicated session already exists (a cache re-open inherits the running
    /// session's harness, so don't ask again).
    fn pr_review_needs_harness_pick(&self) -> bool {
        let AppMode::PrReview(state) = &self.mode else {
            return false;
        };
        if state.fix_target != FixTarget::DedicatedReview || state.review_harness.is_some() {
            return false;
        }
        match self.feature_indices_for_workdir(&state.workdir) {
            Some((pi, fi)) => {
                let feature = &self.store.projects[pi].features[fi];
                fix_session_index(feature, FixTarget::DedicatedReview, REVIEW_SESSION_LABEL)
                    .is_none()
            }
            // No feature resolved yet — let the inject path surface the error.
            None => false,
        }
    }

    /// Open the single-select harness picker for the dedicated review session,
    /// highlighting the project's preferred agent by default. No-op if a comment
    /// isn't selected or no harnesses are available.
    fn pr_review_open_harness_pick(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrReview(state) => state.workdir.clone(),
            _ => return,
        };
        let agents = self.allowed_agents_for_project_path(&workdir);
        if agents.is_empty() {
            // Nothing to choose — fall back to the default and inject directly.
            return self.pr_review_skip_harness_pick();
        }
        let preferred = self
            .feature_indices_for_workdir(&workdir)
            .map(|(pi, _)| self.store.projects[pi].preferred_agent.clone());
        let selected = preferred
            .and_then(|p| agents.iter().position(|a| *a == p))
            .unwrap_or(0);
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = Some(HarnessPickState { agents, selected });
        }
    }

    /// Skip harness selection (e.g. no choices available): continue straight to
    /// the confirm dialog, leaving `review_harness` at its default fallback.
    fn pr_review_skip_harness_pick(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = None;
        }
        self.pr_review_continue_after_harness();
    }

    /// After the harness is chosen (or skipped), open the dialog the pending
    /// action wanted: the combined-batch confirm for the `B` flow, otherwise the
    /// single-comment fix confirm. Neither re-checks the harness pick, so this
    /// can't loop back into the picker.
    fn pr_review_continue_after_harness(&mut self) {
        let batch = matches!(&self.mode, AppMode::PrReview(state) if state.pending_batch);
        if batch {
            self.pr_review_show_batch_confirm();
        } else {
            self.pr_review_show_fix_confirm();
        }
    }

    /// Whether the harness picker is currently open over the review pane.
    pub fn pr_review_harness_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.harness_pick.is_some()
        )
    }

    /// Move the harness-picker highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_harness_pick_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.harness_pick
            && !pick.agents.is_empty()
        {
            let len = pick.agents.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Confirm the harness picker: remember the choice for the rest of the PR
    /// and continue into the fix confirm dialog.
    pub fn pr_review_harness_pick_confirm(&mut self) {
        let chosen = match &self.mode {
            AppMode::PrReview(state) => state
                .harness_pick
                .as_ref()
                .and_then(|p| p.agents.get(p.selected).cloned()),
            _ => return,
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = None;
            state.review_harness = chosen.clone();
        }
        if let Some(agent) = &chosen {
            self.push_toast_success(format!("Review session will run {}", agent.display_name()));
        }
        // Continue into the dialog the pending action wanted (single or batch).
        self.pr_review_continue_after_harness();
    }

    /// Cancel the harness picker without choosing — aborts this fix; the user
    /// can press `f`/`B` again. `review_harness` stays unset so the picker
    /// reappears, and any pending batch is discarded.
    pub fn pr_review_harness_pick_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = None;
            state.pending_batch = false;
        }
    }

    /// Whether the fix confirm/edit dialog is currently open, and if so whether
    /// it is in edit mode. `None` means no dialog is open.
    pub fn pr_review_fix_editing(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.fix_confirm.as_ref().map(|c| c.editing),
            _ => None,
        }
    }

    /// Close the fix confirm/edit dialog without injecting anything.
    pub fn pr_review_cancel_fix(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.fix_confirm = None;
        }
    }

    /// Switch the open fix dialog into edit mode so keystrokes flow to the
    /// prompt editor. No-op when the dialog is closed or already editing.
    pub fn pr_review_fix_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the prompt is kept).
    pub fn pr_review_fix_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.editing = false;
        }
    }

    /// Forward a key to the open fix-prompt editor (only meaningful in edit
    /// mode). Returns `true` when a dialog editor consumed the key. Requests a
    /// cursor-follow scroll when the edit moved the cursor or changed the text.
    pub fn pr_review_fix_editor_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
            && confirm.editing
        {
            let outcome = confirm.editor.handle_key(key);
            if outcome.text_changed || outcome.cursor_moved {
                confirm.sync_to_cursor = true;
            }
            return true;
        }
        false
    }

    /// Toggle the vim keymap on the open fix-prompt editor, remembering the
    /// choice on the pane so reopening the dialog keeps it. No-op when closed.
    pub fn pr_review_fix_toggle_vim(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        let Some(confirm) = &mut state.fix_confirm else {
            return;
        };
        confirm.editor.toggle_vim();
        confirm.sync_to_cursor = true;
        let on = confirm.editor.vim_mode().is_some();
        state.fix_vim_enabled = on;
        self.message = Some(if on {
            "Vim mode enabled".into()
        } else {
            "Vim mode disabled".into()
        });
    }

    /// Scroll the fix-prompt editor by `delta` visual rows (positive = down).
    /// Clears cursor-follow so the user can scroll away from the cursor; the
    /// final clamp to content happens during rendering.
    pub fn pr_review_fix_scroll(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.scroll = confirm.scroll.saturating_add_signed(delta);
            confirm.sync_to_cursor = false;
        }
    }

    /// The vim mode of the open fix-prompt editor, or `None` when the dialog is
    /// closed or the editor is in plain (non-vim) mode. Drives `Esc` handling
    /// (vim consumes `Esc` for Insert→Normal) and the dialog's mode label.
    pub fn pr_review_fix_vim_mode(&self) -> Option<crate::editor::VimMode> {
        match &self.mode {
            AppMode::PrReview(state) => {
                state.fix_confirm.as_ref().and_then(|c| c.editor.vim_mode())
            }
            _ => None,
        }
    }

    /// Confirm the dialog: inject the (possibly edited) prompt into the chosen
    /// agent session and switch the user into that session to watch it (no
    /// auto-advance). The dedicated review session is spun up on first use and
    /// reused thereafter; the existing-live target reuses the feature's running
    /// agent session. Delivery goes through the shared compose / prompt-library
    /// seam: pasted without sending so the user reviews before it runs.
    ///
    /// Handles both a single-comment fix and a **combined batch** (the `B` flow,
    /// `FixConfirmState::batch`): the batch injects one numbered prompt and marks
    /// every included comment `Fixing`, then clears the marked set.
    pub fn pr_review_inject_fix(&mut self) -> Result<()> {
        let (prompt, pr_number, head_sha, fixing_ids, is_batch) = match &self.mode {
            AppMode::PrReview(state) => {
                let pr_number = state.review.pr.number;
                let head_sha = state.review.pr.head_sha.clone();
                let (prompt, ids, is_batch) = match &state.fix_confirm {
                    // Confirming the open dialog uses its edited buffer. A batch
                    // dialog carries every included comment id; a single one
                    // targets the current selection.
                    Some(confirm) => {
                        let prompt = confirm.editor.text().trim().to_string();
                        match &confirm.batch {
                            Some(batch_ids) => (prompt, batch_ids.clone(), true),
                            None => (
                                prompt,
                                state.selected_comment().map(|c| c.id).into_iter().collect(),
                                false,
                            ),
                        }
                    }
                    // No dialog open (e.g. empty pane): fall back to the selection.
                    None => match state.selected_comment() {
                        Some(c) => (c.fix_prompt(), vec![c.id], false),
                        None => {
                            self.message = Some("No comment selected".into());
                            return Ok(());
                        }
                    },
                };
                (prompt, pr_number, head_sha, ids, is_batch)
            }
            _ => return Ok(()),
        };

        if prompt.is_empty() {
            self.message = Some("Nothing to inject — the prompt is empty".into());
            return Ok(());
        }

        let (pi, fi, si) = match self.resolve_fix_session() {
            Ok(target) => target,
            Err(e) => {
                self.show_error(e);
                return Ok(());
            }
        };

        // The fix is committed: mark every targeted comment `Fixing` and persist
        // before we leave the pane, so re-opening the review (cache hit) shows
        // the state.
        for id in &fixing_ids {
            if let AppMode::PrReview(state) = &mut self.mode
                && let Some(c) = state.review.comments.iter_mut().find(|c| c.id == *id)
            {
                c.triage = TriageState::Fixing;
            }
            self.persist_triage(pr_number, &head_sha, *id, TriageState::Fixing, None);
        }
        // A batch consumes the marked set once it's committed.
        if is_batch {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.marked.clear();
            }
            self.push_toast_success(format!(
                "Injected a combined fix for {} comments",
                fixing_ids.len()
            ));
        }

        // Stash the pane's exact state so leader+P can jump straight back to
        // it without re-fetching — the same mechanism the `P` toggle uses.
        // Leaving the pane is still intentional (the user watches the agent),
        // but the round trip back to triage the next comment no longer has to
        // go through the dashboard and a re-resolve. The confirm dialog has
        // already served its purpose (the prompt above was read from it), so
        // clear it before stashing — otherwise returning would reopen the
        // same "inject fix" dialog instead of the plain comment list.
        let AppMode::PrReview(mut state) = std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return Ok(());
        };
        state.fix_confirm = None;
        let feature = &self.store.projects[pi].features[fi];
        self.pr_review_return = Some(PrReviewReturn {
            session: feature.tmux_session.clone(),
            window: feature.sessions[si].tmux_window.clone(),
            state,
        });

        // Switch into the target session, then deliver the prompt via the
        // shared seam (seeds the compose box when interception is on, else
        // pastes without sending).
        self.selection = Selection::Session(pi, fi, si);
        self.enter_view_without_auto_compose()?;
        let AppMode::Viewing(view) = &self.mode else {
            return Ok(());
        };
        let view = view.clone();
        self.deliver_prompt(prompt, Some(view))
    }

    /// Jump from the review pane straight into the linked fix session (`P`),
    /// stashing the pane's exact state (selection, scroll, open dialogs) so
    /// `pr_review_return_to_pane` can pop back to it without re-fetching.
    /// Unlike `f`, this never spins up the dedicated session — it only jumps
    /// to one that already exists, so a quick "peek at the agent" doesn't
    /// have the side effect of starting a review session on its own.
    pub fn pr_review_toggle_to_session(&mut self) -> Result<()> {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PrReview(state) => state,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let Some((pi, fi)) = self.feature_indices_for_workdir(&state.workdir) else {
            self.mode = AppMode::PrReview(state);
            self.push_toast_warning("Could not find the feature for this PR");
            return Ok(());
        };
        let feature = &self.store.projects[pi].features[fi];
        let Some(si) = fix_session_index(feature, state.fix_target, REVIEW_SESSION_LABEL) else {
            self.mode = AppMode::PrReview(state);
            self.push_toast_warning("No review session yet — press f to start one");
            return Ok(());
        };
        let session = feature.tmux_session.clone();
        let window = feature.sessions[si].tmux_window.clone();

        self.selection = Selection::Session(pi, fi, si);
        self.pr_review_return = Some(PrReviewReturn {
            session,
            window,
            state,
        });
        self.enter_view_without_auto_compose()
    }

    /// Jump back from a Viewing session to the review pane stashed by
    /// `pr_review_toggle_to_session` (`leader+P`), restoring the exact prior
    /// state — no re-fetch. Only restores when the current session is the one
    /// the stash was jumped from; a stash left behind after navigating
    /// elsewhere is not popped into an unrelated session's view.
    pub fn pr_review_return_to_pane(&mut self) {
        let Some(stash) = &self.pr_review_return else {
            self.push_toast_warning("No review pane to return to");
            return;
        };
        let matches_current = matches!(
            &self.mode,
            AppMode::Viewing(view) if view.session == stash.session && view.window == stash.window
        );
        if !matches_current {
            self.push_toast_warning("No review pane linked to this session");
            return;
        }
        if let Some(stash) = self.pr_review_return.take() {
            self.mode = AppMode::PrReview(stash.state);
        }
    }

    /// Open a **"Done in `<sha>`"** reply for the selected comment, seeded from
    /// the feature workdir's latest commit (the fix the user just made). Editable
    /// before posting; posting marks the comment `Done`.
    pub fn pr_review_open_reply_done(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrReview(state) if state.reply.is_none() && state.fix_confirm.is_none() => {
                state.workdir.clone()
            }
            _ => return,
        };
        let seed = match latest_commit_short_sha(&workdir) {
            Some(sha) => format!("Done in `{sha}`."),
            None => "Done.".to_string(),
        };
        self.open_reply(ReplyKind::Done, seed);
    }

    /// Open a **"not needed"** reply for the selected comment: an empty editor
    /// for the user to explain *why* a fix isn't needed. Posting marks the
    /// comment `Skipped` and stores the explanation as its local note.
    pub fn pr_review_open_reply_not_needed(&mut self) {
        self.open_reply(ReplyKind::NotNeeded, String::new());
    }

    /// Shared entry: open the reply dialog for the selected comment with a kind
    /// and a seeded body. No-op if a fix/reply dialog is already open or nothing
    /// is selected.
    fn open_reply(&mut self, kind: ReplyKind, seed: String) {
        let comment_id = match &self.mode {
            AppMode::PrReview(state) if state.reply.is_none() && state.fix_confirm.is_none() => {
                state.selected_comment().map(|c| c.id)
            }
            _ => return,
        };
        let Some(comment_id) = comment_id else {
            self.message = Some("No comment selected".into());
            return;
        };
        // An AI-review draft finding has no real GitHub thread to reply into
        // until it's posted (Epic E's "post as GitHub review" action) — bail
        // with a clear message instead of a confusing GitHub 404/422.
        if let AppMode::PrReview(state) = &self.mode
            && state
                .review
                .comments
                .iter()
                .any(|c| c.id == comment_id && c.ai_generated)
        {
            self.push_toast_warning(
                "This is an unposted AI-review draft — post it first, or skip/add to memory instead",
            );
            return;
        }
        // Not-needed replies start in edit mode (the user must type a reason);
        // the done template is post-ready, so it opens in the confirm view.
        let editing = matches!(kind, ReplyKind::NotNeeded);
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply = Some(ReplyState {
                comment_id,
                kind,
                editor: TextEditor::new(seed),
                editing,
            });
        }
    }

    /// Enter edit mode so keystrokes flow to the reply editor.
    pub fn pr_review_reply_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
        {
            reply.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the body is kept).
    pub fn pr_review_reply_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
        {
            reply.editing = false;
        }
    }

    /// Forward a key to the open reply editor (only meaningful in edit mode).
    pub fn pr_review_reply_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
            && reply.editing
        {
            reply.editor.handle_key(key);
        }
    }

    /// Close the reply dialog without posting.
    pub fn pr_review_cancel_reply(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply = None;
        }
    }

    /// Reply-dialog status for the key handler: `None` when closed, else whether
    /// it is currently in edit mode.
    pub fn pr_review_reply_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.reply.as_ref().map(|r| r.editing),
            _ => None,
        }
    }

    /// Post the (possibly edited) reply to GitHub and close the dialog. Inline
    /// comments reply into their thread; conversation comments and review
    /// summaries post a new conversation comment. On success the comment is
    /// marked by the reply's kind — `Done` for a "done in `<sha>`" reply,
    /// `Skipped` (with the body kept as the local note) for a "not needed" one.
    /// The GitHub write runs only on the user's explicit confirm.
    pub fn pr_review_post_reply(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::PrReview(state) => state.reply.as_ref().and_then(|reply| {
                let comment = state
                    .review
                    .comments
                    .iter()
                    .find(|c| c.id == reply.comment_id)?;
                Some((
                    state.workdir.clone(),
                    state.review.pr.clone(),
                    comment.reply_target(),
                    reply.kind,
                    reply.comment_id,
                    reply.editor.text().trim().to_string(),
                ))
            }),
            _ => return Ok(()),
        };
        let Some((workdir, pr, target, kind, comment_id, body)) = prep else {
            return Ok(());
        };

        if body.is_empty() {
            let hint = match kind {
                ReplyKind::NotNeeded => "Explain why a fix isn't needed, or esc to cancel",
                ReplyKind::Done => "Reply is empty — type something or esc to cancel",
            };
            self.message = Some(hint.into());
            return Ok(());
        }

        let result = match target {
            ReplyTarget::InlineThread { root_comment_id } => GhCli::reply_to_review_comment(
                &workdir,
                &pr.owner,
                &pr.repo,
                pr.number,
                root_comment_id,
                &body,
            ),
            ReplyTarget::Conversation => {
                GhCli::post_issue_comment(&workdir, &pr.owner, &pr.repo, pr.number, &body)
            }
        };
        if let Err(e) = result {
            self.show_error(e);
            return Ok(());
        }

        // Apply the triage outcome for this reply kind and close the dialog.
        let (triage, note) = match kind {
            ReplyKind::Done => (TriageState::Done, None),
            ReplyKind::NotNeeded => (TriageState::Skipped, Some(body.clone())),
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            if let Some(c) = state
                .review
                .comments
                .iter_mut()
                .find(|c| c.id == comment_id)
            {
                c.triage = triage;
                c.local_note = note.clone();
            }
            state.reply = None;
        }
        self.persist_triage(pr.number, &pr.head_sha, comment_id, triage, note.as_deref());
        // Posting can flip a thread's resolution (e.g. GitHub auto-resolves, or
        // the reviewer resolved meanwhile), so re-pull thread state to keep the
        // `✓` marker honest. Zero agent tokens — one GraphQL call.
        self.refresh_thread_resolution();
        let toast = match kind {
            ReplyKind::Done => "Posted reply · marked done",
            ReplyKind::NotNeeded => "Posted reply · marked skipped",
        };
        self.push_toast_success(toast.to_string());
        Ok(())
    }

    /// Open the AI-review post confirm dialog (Epic E `W`): gather every
    /// `ai_generated` draft finding that hasn't been skipped or already
    /// posted, build the review payload ([`build_ai_review`]), and seed an
    /// editable preview of the summary body. No-op (with a hint) if there are
    /// no eligible findings, or if another dialog is already open.
    pub fn pr_review_open_ai_review_post_confirm(&mut self) {
        let findings: Vec<PrComment> = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.memory_add.is_none()
                    && state.ai_review_post.is_none() =>
            {
                state
                    .review
                    .comments
                    .iter()
                    .filter(|c| {
                        c.ai_generated
                            && !matches!(c.triage, TriageState::Skipped | TriageState::Replied)
                    })
                    .cloned()
                    .collect()
            }
            _ => return,
        };
        if findings.is_empty() {
            self.push_toast_warning(
                "No AI findings to post — run A to generate some, or skip fewer",
            );
            return;
        }
        let refs: Vec<&PrComment> = findings.iter().collect();
        let (summary, inline) = build_ai_review(&refs);
        let comment_ids = findings.iter().map(|c| c.id).collect();

        if let AppMode::PrReview(state) = &mut self.mode {
            state.ai_review_post = Some(AiReviewPostConfirmState {
                comment_ids,
                inline,
                editor: TextEditor::new(summary),
                editing: false,
                error: None,
            });
        }
    }

    pub fn pr_review_ai_review_post_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(post) = &mut state.ai_review_post
        {
            post.editing = true;
        }
    }

    pub fn pr_review_ai_review_post_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(post) = &mut state.ai_review_post
        {
            post.editing = false;
        }
    }

    pub fn pr_review_ai_review_post_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(post) = &mut state.ai_review_post
            && post.editing
        {
            post.editor.handle_key(key);
        }
    }

    /// Close the AI-review post dialog without posting.
    pub fn pr_review_cancel_ai_review_post(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.ai_review_post = None;
        }
    }

    /// AI-review post dialog status for the key handler: `None` when closed,
    /// else whether it is currently in edit mode.
    pub fn pr_review_ai_review_post_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.ai_review_post.as_ref().map(|p| p.editing),
            _ => None,
        }
    }

    /// Post the AI review to GitHub (`event: "COMMENT"` — never
    /// auto-approve/request-changes) and, on success, mark every included
    /// finding `Replied` so a re-post doesn't duplicate them. GitHub rejects
    /// the *entire* review if any inline comment points outside the diff
    /// (documented on [`GhCli::create_review`]); on that failure the drafts
    /// are left untouched so nothing triaged is lost, and the post-confirm
    /// dialog stays open with the failure recorded on
    /// [`AiReviewPostConfirmState::error`] rather than getting silently
    /// closed by `show_error`'s mode reset (from real use — pressing `W`
    /// into a rejected review used to boot the user back to the dashboard).
    pub fn pr_review_post_ai_review(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::PrReview(state) => state.ai_review_post.as_ref().map(|post| {
                (
                    state.workdir.clone(),
                    state.review.pr.clone(),
                    post.comment_ids.clone(),
                    post.inline.clone(),
                    post.editor.text().trim().to_string(),
                )
            }),
            _ => return Ok(()),
        };
        let Some((workdir, pr, comment_ids, inline, body)) = prep else {
            return Ok(());
        };

        if let Err(e) = GhCli::create_review(&workdir, &pr, &body, "COMMENT", &inline) {
            self.fail_ai_review_post(e);
            return Ok(());
        }

        if let AppMode::PrReview(state) = &mut self.mode {
            for c in &mut state.review.comments {
                if comment_ids.contains(&c.id) {
                    c.triage = TriageState::Replied;
                }
            }
            state.ai_review_post = None;
        }
        for id in &comment_ids {
            self.persist_triage(pr.number, &pr.head_sha, *id, TriageState::Replied, None);
        }
        self.recache_current_review();
        self.push_toast_success(format!(
            "Posted AI review · {} comment{}",
            comment_ids.len(),
            if comment_ids.len() == 1 { "" } else { "s" }
        ));
        Ok(())
    }

    /// Record a `W` post failure inline on the still-open post-confirm
    /// dialog and restore the review pane — working around `show_error`'s
    /// unconditional `self.mode` reset to `Normal` for any
    /// non-Normal/Help/Viewing mode, which would otherwise silently boot the
    /// user back to the dashboard on a recoverable posting error (from real
    /// use — see the bug backlog).
    pub(crate) fn fail_ai_review_post(&mut self, e: anyhow::Error) {
        let detail = e.to_string();
        let mut origin = match &self.mode {
            AppMode::PrReview(state) => Some(state.clone()),
            _ => None,
        };
        if let Some(origin) = &mut origin
            && let Some(post) = &mut origin.ai_review_post
        {
            post.error = Some(detail);
        }
        self.show_error(e);
        if let Some(origin) = origin {
            self.mode = AppMode::PrReview(origin);
        }
    }

    /// Open the "add to memory" dialog for the selected comment, seeded from
    /// [`PrComment::memory_finding_seed`] and defaulting to the `General`
    /// category. Editable before it's appended. No-op if a fix/reply/memory
    /// dialog is already open or nothing is selected.
    pub fn pr_review_open_memory_add(&mut self) {
        let seed = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.memory_add.is_none() =>
            {
                state
                    .selected_comment()
                    .map(|c| (c.id, c.memory_finding_seed()))
            }
            _ => return,
        };
        let Some((comment_id, seed)) = seed else {
            self.message = Some("No comment selected".into());
            return;
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = Some(MemoryAddState {
                comment_id,
                category: 0,
                editor: TextEditor::new(seed),
                editing: false,
            });
        }
    }

    /// Enter edit mode so keystrokes flow to the finding editor.
    pub fn pr_review_memory_add_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the text is kept).
    pub fn pr_review_memory_add_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.editing = false;
        }
    }

    /// Forward a key to the open finding editor (only meaningful in edit mode).
    pub fn pr_review_memory_add_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
            && memory_add.editing
        {
            memory_add.editor.handle_key(key);
        }
    }

    /// Cycle the category (confirm view only) through [`MEMORY_CATEGORIES`].
    pub fn pr_review_cycle_memory_category(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.category = (memory_add.category + 1) % MEMORY_CATEGORIES.len();
        }
    }

    /// Close the "add to memory" dialog without appending.
    pub fn pr_review_cancel_memory_add(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = None;
        }
    }

    /// Memory-add dialog status for the key handler: `None` when closed, else
    /// whether it is currently in edit mode.
    pub fn pr_review_memory_add_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.memory_add.as_ref().map(|m| m.editing),
            _ => None,
        }
    }

    /// Append the (possibly edited) finding to the review-memory doc and close
    /// the dialog. Whitespace/newlines in the finding text are collapsed to a
    /// single line first, since the doc stores each finding as one bullet.
    /// Dedup-aware and append-only (`review_memory::append_finding`) — never
    /// touches existing prose. Zero agent tokens; a local file write only.
    pub fn pr_review_append_memory(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::PrReview(state) => state.memory_add.as_ref().map(|memory_add| {
                (
                    state.workdir.clone(),
                    MEMORY_CATEGORIES[memory_add.category],
                    memory_add.editor.text(),
                )
            }),
            _ => return Ok(()),
        };
        let Some((workdir, category, finding)) = prep else {
            return Ok(());
        };

        let finding = finding.split_whitespace().collect::<Vec<_>>().join(" ");
        if finding.is_empty() {
            self.message = Some("Finding is empty — type something or esc to cancel".into());
            return Ok(());
        }

        let repo = self.repo_for_project_path(&workdir);
        let path =
            review_memory::review_memory_path(&repo, self.config.review_memory_path.as_deref());
        let appended = review_memory::append_finding(&path, category, &finding)?;

        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = None;
        }
        let toast = if appended {
            format!("Added to memory · {category}")
        } else {
            "Already in memory · skipped".to_string()
        };
        self.push_toast_success(toast);
        Ok(())
    }

    /// Toggle GitHub resolution of the selected comment's review thread via the
    /// GraphQL `resolveReviewThread` / `unresolveReviewThread` mutation. Only
    /// inline comments that belong to a thread can be resolved; conversation
    /// comments and review summaries have no thread, so this is a no-op with a
    /// hint. Independent of replying (the user may resolve without commenting).
    ///
    /// On success the new state is applied to every comment in that thread and
    /// the SQLite cache is refreshed so a later cache-hit re-open reflects it.
    /// Zero agent tokens.
    pub fn pr_review_toggle_resolve(&mut self) {
        let info = match &self.mode {
            AppMode::PrReview(state) => state
                .selected_comment()
                .map(|c| (state.workdir.clone(), c.thread_id.clone(), c.is_resolved)),
            _ => return,
        };
        let Some((workdir, thread_id, is_resolved)) = info else {
            self.message = Some("No comment selected".into());
            return;
        };
        let Some(thread_id) = thread_id else {
            self.message = Some("This comment has no resolvable review thread".into());
            return;
        };

        let desired = !is_resolved;
        let now_resolved = match GhCli::set_thread_resolved(&workdir, &thread_id, desired) {
            Ok(state) => state,
            Err(e) => {
                self.show_error(e);
                return;
            }
        };

        if let AppMode::PrReview(state) = &mut self.mode {
            for c in &mut state.review.comments {
                if c.thread_id.as_deref() == Some(thread_id.as_str()) {
                    c.is_resolved = now_resolved;
                }
            }
        }
        self.recache_current_review();
        let msg = if now_resolved {
            "Thread resolved"
        } else {
            "Thread reopened"
        };
        self.push_toast_success(msg.to_string());
    }

    /// Re-fetch GitHub review-thread resolution state and apply it to the
    /// in-memory review (and cache). One GraphQL call, zero agent tokens. A
    /// failure is non-fatal — the existing markers just stay as they were.
    fn refresh_thread_resolution(&mut self) {
        let (workdir, pr) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.clone()),
            _ => return,
        };
        let threads = match GhCli::review_threads(&workdir, &pr.owner, &pr.repo, pr.number) {
            Ok(threads) => threads,
            Err(e) => {
                self.log_warn("pr_review", format!("thread refresh failed: {e}"));
                return;
            }
        };
        let index = index_threads(&threads);
        if let AppMode::PrReview(state) = &mut self.mode {
            for c in &mut state.review.comments {
                if let Some((tid, resolved)) = index.get(&c.id) {
                    c.thread_id = Some(tid.clone());
                    c.is_resolved = *resolved;
                }
            }
        }
        self.recache_current_review();
    }

    /// Re-persist the current in-memory review to the SQLite cache so a later
    /// cache-hit re-open reflects resolution changes made in the pane.
    fn recache_current_review(&mut self) {
        let review = match &self.mode {
            AppMode::PrReview(state) => state.review.clone(),
            _ => return,
        };
        self.cache_pr_review(&review);
    }

    /// Resolve (and, for the dedicated strategy, lazily create) the agent
    /// window that fix prompts target. Returns `(project, feature, session)`
    /// indices. Ensures the feature's tmux session is running first.
    fn resolve_fix_session(&mut self) -> Result<(usize, usize, usize)> {
        let (workdir, target, harness) = match &self.mode {
            AppMode::PrReview(state) => (
                state.workdir.clone(),
                state.fix_target,
                state.review_harness.clone(),
            ),
            _ => anyhow::bail!("not reviewing a PR"),
        };
        let (pi, fi) = self
            .feature_indices_for_workdir(&workdir)
            .ok_or_else(|| anyhow::anyhow!("could not find the feature for this PR"))?;

        self.ensure_feature_running_for_new_session(pi, fi)?;

        let feature = &self.store.projects[pi].features[fi];
        if let Some(si) = fix_session_index(feature, target, REVIEW_SESSION_LABEL) {
            return Ok((pi, fi, si));
        }

        match target {
            FixTarget::DedicatedReview => {
                let si =
                    self.create_dedicated_review_session(pi, fi, REVIEW_SESSION_LABEL, harness)?;
                Ok((pi, fi, si))
            }
            FixTarget::ExistingLive => {
                anyhow::bail!("no live agent session to reuse — switch to the dedicated target (t)")
            }
        }
    }

    /// Find the `(project, feature)` indices of the feature whose workdir
    /// matches `workdir`.
    fn feature_indices_for_workdir(&self, workdir: &Path) -> Option<(usize, usize)> {
        self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            p.features
                .iter()
                .position(|f| f.workdir == workdir)
                .map(|fi| (pi, fi))
        })
    }

    /// Token usage for the session the pane's current fix target resolves to,
    /// for a header display. Read-only — unlike [`App::resolve_fix_session`] it
    /// never creates a session, so this is safe to call on every frame just to
    /// render a number. `None` before any fix has spun up the target session.
    pub(crate) fn pr_review_fix_session_usage(
        &self,
    ) -> Option<crate::token_tracking::SessionTokenUsage> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        let (pi, fi) = self.feature_indices_for_workdir(&state.workdir)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = fix_session_index(feature, state.fix_target, REVIEW_SESSION_LABEL)?;
        feature.sessions[si].token_usage.clone()
    }

    pub fn pr_review_scroll_detail_up(&mut self, amount: usize) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.detail_scroll = state.detail_scroll.saturating_sub(amount);
        }
    }

    pub fn pr_review_scroll_detail_down(&mut self, amount: usize) {
        if let AppMode::PrReview(state) = &mut self.mode {
            // The renderer records how many lines it last drew; clamp against
            // that so scrolling can't run past the rendered detail content.
            let max_scroll = state.detail_content_lines.saturating_sub(1);
            state.detail_scroll = (state.detail_scroll + amount).min(max_scroll);
        }
    }

    /// Open the lookback-bootstrap depth picker (`b` in the PR picker): an
    /// overlay on the picker, not a separate mode, mirroring how the fix
    /// harness picker overlays the review pane.
    pub fn open_review_memory_bootstrap_pick(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.bootstrap_pick = Some(BootstrapPickState {
                selected: BootstrapDepth::ALL
                    .iter()
                    .position(|d| *d == BootstrapDepth::default())
                    .unwrap_or(0),
            });
        }
    }

    /// Whether the bootstrap depth picker is currently open over the PR picker.
    pub fn review_memory_bootstrap_picking(&self) -> bool {
        matches!(&self.mode, AppMode::PrPicker(state) if state.bootstrap_pick.is_some())
    }

    /// Move the depth-picker highlight (`+1`/`-1`, wrapping).
    pub fn review_memory_bootstrap_pick_move(&mut self, delta: isize) {
        if let AppMode::PrPicker(state) = &mut self.mode
            && let Some(pick) = &mut state.bootstrap_pick
        {
            let len = BootstrapDepth::ALL.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Close the depth picker without running anything, staying on the PR
    /// picker.
    pub fn review_memory_bootstrap_pick_cancel(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.bootstrap_pick = None;
        }
    }

    /// Confirm the chosen depth: resolve the recent closed/merged PRs
    /// synchronously (one cheap `gh` call), then hand the heavy work — the
    /// per-PR comment fetch loop and the one distill pass — to a background
    /// thread and switch to the full-screen running view.
    pub fn review_memory_bootstrap_pick_confirm(&mut self) {
        let (workdir, depth, mut origin) = match &self.mode {
            AppMode::PrPicker(state) => {
                let Some(pick) = &state.bootstrap_pick else {
                    return;
                };
                let depth = BootstrapDepth::ALL[pick.selected];
                (state.workdir.clone(), depth, state.clone())
            }
            _ => return,
        };
        origin.bootstrap_pick = None;

        let entries = match GhCli::list_recent_closed_prs(&workdir, depth.limit()) {
            Ok(entries) => entries,
            Err(e) => {
                self.mode = AppMode::PrPicker(origin);
                self.show_error(e);
                return;
            }
        };
        if entries.is_empty() {
            self.mode = AppMode::PrPicker(origin);
            self.message = Some("No merged/closed PRs found to learn from".into());
            return;
        }

        let repo = self.repo_for_project_path(&workdir);
        let memory_path =
            review_memory::review_memory_path(&repo, self.config.review_memory_path.as_deref());

        self.log_info(
            "pr_review",
            format!(
                "bootstrapping review memory from {} PRs (depth: {})",
                entries.len(),
                depth.label()
            ),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        self.review_memory_bootstrap_bg = Some(rx);
        let thread_workdir = workdir.clone();
        std::thread::spawn(move || {
            run_review_memory_bootstrap(thread_workdir, memory_path, entries, tx);
        });

        self.mode = AppMode::ReviewMemoryBootstrapRunning(BootstrapRunState {
            origin,
            depth,
            stage: BootstrapStage::FetchingComments,
        });
    }

    /// Poll the background bootstrap. Progress messages update the running
    /// screen's stage; `Done` always surfaces a toast (success) or error (the
    /// run has a real side effect — tokens spent, findings written — even if
    /// the user already navigated away), and restores the PR picker only if
    /// the running screen is still showing. An error is also written onto the
    /// restored picker's own inline `error` field so it's visible immediately
    /// on return, not just logged. Returns `true` when a redraw is warranted.
    pub fn poll_review_memory_bootstrap_bg(&mut self) -> bool {
        let Some(rx) = self.review_memory_bootstrap_bg.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(BootstrapProgress::Distilling {
                    pr_count,
                    token_estimate,
                }) => {
                    if let AppMode::ReviewMemoryBootstrapRunning(state) = &mut self.mode {
                        state.stage = BootstrapStage::Distilling {
                            pr_count,
                            token_estimate,
                        };
                    }
                    changed = true;
                }
                Ok(BootstrapProgress::Done(result)) => {
                    self.review_memory_bootstrap_bg = None;
                    // Capture the origin before any mode-mutating side effect
                    // below: `show_error` unconditionally resets `self.mode` to
                    // `Normal` for any non-Normal/Help/Viewing mode, which would
                    // otherwise clobber the running screen's stashed picker
                    // before we get a chance to restore it.
                    let mut origin = match &self.mode {
                        AppMode::ReviewMemoryBootstrapRunning(state) => Some(state.origin.clone()),
                        _ => None,
                    };
                    match result {
                        Ok(outcome) => {
                            self.push_toast_success(format!(
                                "Bootstrapped review memory from {} PR{} · {} new finding{}",
                                outcome.pr_count,
                                if outcome.pr_count == 1 { "" } else { "s" },
                                outcome.appended,
                                if outcome.appended == 1 { "" } else { "s" },
                            ));
                        }
                        Err(e) => {
                            // `show_error` only logs and surfaces via the
                            // dashboard's status bar, which the PR picker's
                            // full-screen render doesn't draw — also set the
                            // picker's own inline `error` (the same field
                            // `pr_picker_choose` uses) so the failure is
                            // actually visible on return, not just logged.
                            let detail = e.to_string();
                            if let Some(origin) = &mut origin {
                                origin.error = Some(detail.clone());
                            }
                            self.show_error(e);
                        }
                    }
                    if let Some(origin) = origin {
                        self.mode = AppMode::PrPicker(origin);
                    }
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.review_memory_bootstrap_bg = None;
                    if let AppMode::ReviewMemoryBootstrapRunning(state) = &self.mode {
                        self.mode = AppMode::PrPicker(state.origin.clone());
                        self.message = Some("Bootstrap failed unexpectedly".to_string());
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// Cancel the running screen (`esc`/`q`): return to the PR picker. The
    /// background thread isn't aborted — if it finishes later,
    /// [`App::poll_review_memory_bootstrap_bg`] still surfaces the result.
    pub fn cancel_review_memory_bootstrap(&mut self) {
        if let AppMode::ReviewMemoryBootstrapRunning(state) = &self.mode {
            self.mode = AppMode::PrPicker(state.origin.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GhUser;

    fn pr() -> PrRef {
        PrRef {
            number: 1,
            head_sha: "sha".into(),
            url: "https://github.com/o/r/pull/1".into(),
            owner: "o".into(),
            repo: "r".into(),
        }
    }

    fn user(login: &str, kind: &str) -> GhUser {
        GhUser {
            login: login.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn strips_details_comments_and_images() {
        let body = "Real point here.\n\n<details>\n<summary>Prompt for AI agents</summary>\n\
            lots of tokens\n</details>\n<!-- internal note -->\n![badge](http://x/y.png)";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, "Real point here.");
    }

    #[test]
    fn strips_quoted_diff_fence() {
        let body = "This can race with the poller.\n\n```diff\n@@ -40,6 +40,7 @@\n-old\n+new\n```\n\nGuard it behind the lock.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(
            out,
            "This can race with the poller.\n\nGuard it behind the lock."
        );
    }

    #[test]
    fn strips_quoted_suggestion_fence() {
        let body = "Consider this:\n\n```suggestion\nlet x = 1;\n```\n\nSaves a line.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, "Consider this:\n\nSaves a line.");
    }

    #[test]
    fn strips_leading_quoted_diff_lines() {
        let body = "> -old line\n> +new line\n\nActual comment text.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, "Actual comment text.");
    }

    #[test]
    fn leaves_non_diff_fences_untouched() {
        let body = "Use this instead:\n\n```rust\nlet x = 1;\n```\n\nCleaner.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, body);
    }

    #[test]
    fn snippet_truncates_with_ellipsis() {
        let long = "x".repeat(200);
        let s = truncate_chars(&long, SNIPPET_LEN);
        assert_eq!(s.chars().count(), SNIPPET_LEN);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn snippet_skips_leading_blank_lines() {
        assert_eq!(make_snippet("\n\n  hello world  \n", false), "hello world");
    }

    #[test]
    fn normalize_attaches_resolution_and_outdated() {
        let comments = vec![
            ReviewComment {
                id: 11,
                path: Some("a.rs".into()),
                line: None, // outdated
                original_line: Some(7),
                side: Some("RIGHT".into()),
                diff_hunk: Some("@@".into()),
                subject_type: None,
                body: "race condition".into(),
                user: user("alice", "User"),
                in_reply_to_id: None,
                pull_request_review_id: Some(99),
            },
            ReviewComment {
                id: 12,
                path: Some("b.rs".into()),
                line: Some(3),
                original_line: Some(3),
                side: Some("RIGHT".into()),
                diff_hunk: None,
                subject_type: None,
                body: "nit".into(),
                user: user("coderabbitai", "Bot"),
                in_reply_to_id: None,
                pull_request_review_id: None,
            },
        ];
        let threads = vec![ReviewThread {
            id: "T1".into(),
            is_resolved: true,
            comment_ids: vec![11],
        }];

        let review = normalize(pr(), comments, vec![], vec![], threads);
        assert_eq!(review.comments.len(), 2);

        let c11 = &review.comments[0];
        assert_eq!(c11.line, Some(7)); // fell back to original_line
        assert!(c11.outdated);
        assert!(c11.is_resolved);
        assert_eq!(c11.thread_id.as_deref(), Some("T1"));
        assert!(!c11.is_bot);

        let c12 = &review.comments[1];
        assert!(!c12.outdated);
        assert!(!c12.is_resolved);
        assert!(c12.is_bot);

        assert_eq!(review.open_count(), 1); // only c12 is unresolved
    }

    #[test]
    fn normalize_marks_file_level_comments_and_not_as_outdated() {
        // A file-level comment has no `line` by definition — that must not be
        // mistaken for an outdated line comment.
        let comments = vec![ReviewComment {
            id: 21,
            path: Some("src/big.rs".into()),
            line: None,
            original_line: None,
            side: None,
            diff_hunk: Some("@@ -1,400 +1,420 @@\n+ enormous".into()),
            subject_type: Some("file".into()),
            body: "This module does too much.".into(),
            user: user("alice", "User"),
            in_reply_to_id: None,
            pull_request_review_id: Some(99),
        }];

        let review = normalize(pr(), comments, vec![], vec![], vec![]);
        let c = &review.comments[0];
        assert!(c.file_level);
        assert!(!c.outdated);
        assert_eq!(c.prompt_hunk(), None);
    }

    #[test]
    fn normalize_drops_empty_review_summaries() {
        let reviews = vec![
            Review {
                id: 1,
                body: "".into(),
                state: "APPROVED".into(),
                user: user("bob", "User"),
            },
            Review {
                id: 2,
                body: "Please add a test.".into(),
                state: "CHANGES_REQUESTED".into(),
                user: user("bob", "User"),
            },
        ];
        let review = normalize(pr(), vec![], reviews, vec![], vec![]);
        assert_eq!(review.comments.len(), 1);
        assert_eq!(
            review.comments[0].kind,
            CommentKind::ReviewSummary {
                state: "CHANGES_REQUESTED".into()
            }
        );
    }

    fn inline_comment(body: &str, is_bot: bool) -> PrComment {
        PrComment {
            id: 1,
            kind: CommentKind::Inline,
            author: "alice".into(),
            is_bot,
            path: Some("src/app/sync.rs".into()),
            line: Some(42),
            side: Some("RIGHT".into()),
            outdated: false,
            file_level: false,
            diff_hunk: Some("@@ -38,4 +38,5 @@\n  poll = 250;\n+ self.sync();".into()),
            body: body.into(),
            snippet: String::new(),
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::Untriaged,
            local_note: None,
            ai_generated: false,
        }
    }

    #[test]
    fn fix_prompt_includes_file_line_comment_and_hunk() {
        let c = inline_comment("Guard this behind the lock.", false);
        let prompt = c.fix_prompt();
        assert!(prompt.starts_with("Address this PR review comment."));
        assert!(prompt.contains("File: src/app/sync.rs:42"));
        assert!(prompt.contains("Comment (@alice): Guard this behind the lock."));
        assert!(prompt.contains("Diff hunk:"));
        assert!(prompt.contains("+ self.sync();"));
        // No file contents are ever injected — only the comment + hunk.
        assert!(!prompt.contains("fn "));
    }

    #[test]
    fn fix_prompt_omits_hunk_for_file_level_comment() {
        let mut c = inline_comment("Split this module up.", false);
        c.file_level = true;
        c.line = None;
        // GitHub hands a file-level comment the entire file diff as its hunk.
        c.diff_hunk = Some("@@ -1,400 +1,420 @@\n+ a\n+ b".into());

        assert_eq!(c.prompt_hunk(), None);
        assert!(c.hunk_suppressed());

        let prompt = c.fix_prompt();
        assert!(prompt.contains("File: src/app/sync.rs  (comment on the whole file)"));
        assert!(!prompt.contains("Diff hunk:"));
        assert!(!prompt.contains("+ a"));
        // The agent is told the hunk was withheld, not that there was none.
        assert!(prompt.contains("Diff hunk omitted"));
    }

    #[test]
    fn whole_file_sized_hunk_is_dropped_but_ordinary_ones_are_kept() {
        let hunk_of = |n: usize| {
            Some(
                std::iter::repeat_n("+ line", n)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        };

        // Real line comments run to ~90 hunk lines; those keep their context.
        let mut c = inline_comment("This block is wrong.", false);
        c.diff_hunk = hunk_of(93);
        assert!(c.prompt_hunk().is_some());
        assert!(!c.hunk_suppressed());
        assert!(c.fix_prompt().contains("Diff hunk:"));

        // Only a pathological, whole-file-sized hunk trips the backstop.
        c.diff_hunk = hunk_of(WHOLE_FILE_HUNK_LINES + 1);
        assert_eq!(c.prompt_hunk(), None);
        let prompt = c.fix_prompt();
        // Still line-anchored, so the pointer keeps its line — only the wall of
        // diff is dropped, and not as a "whole file" comment.
        assert!(prompt.contains("File: src/app/sync.rs:42"));
        assert!(!prompt.contains("(comment on the whole file)"));
        assert!(!prompt.contains("Diff hunk:"));
        assert!(prompt.contains("Diff hunk omitted"));
    }

    #[test]
    fn line_comment_windows_githubs_large_hunk_around_its_anchor() {
        let mut c = inline_comment("Only this line is relevant.", false);
        c.line = Some(20);
        c.outdated = true;
        c.diff_hunk = Some(
            std::iter::once("@@ -1,0 +1,40 @@".to_string())
                .chain((1..=40).map(|i| format!("+line{i}")))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let hunk = c.prompt_hunk().expect("the anchor is in the hunk");
        let lines: Vec<_> = hunk.lines().collect();
        assert_eq!(lines.len(), COMMENT_HUNK_CONTEXT_LINES * 2 + 2);
        assert!(hunk.contains("+line20"));
        assert!(!hunk.contains("+line1\n"));
        assert!(!hunk.contains("+line40"));
    }

    #[test]
    fn combined_fix_prompt_drops_whole_file_hunks() {
        let ordinary = inline_comment("Guard this behind the lock.", false);
        let mut file_level = inline_comment("Split this module up.", false);
        file_level.path = Some("src/big.rs".into());
        file_level.file_level = true;
        file_level.line = None;
        file_level.diff_hunk = Some("@@ -1,400 +1,420 @@\n+ enormous".into());

        let prompt = combined_fix_prompt(&[&ordinary, &file_level]);

        // The line-anchored comment keeps its (small) hunk...
        assert!(prompt.contains("+ self.sync();"));
        // ...while the whole-file hunk never lands in the shared prompt, where
        // several of them would otherwise compound.
        assert!(!prompt.contains("+ enormous"));
        assert!(prompt.contains("File: src/big.rs  (comment on the whole file)"));
    }

    #[test]
    fn fix_prompt_strips_bot_boilerplate() {
        let c = inline_comment("<details>noise</details>Real point.", true);
        let prompt = c.fix_prompt();
        assert!(prompt.contains("Comment (@alice): Real point."));
        assert!(!prompt.contains("<details>"));
    }

    #[test]
    fn combined_fix_prompt_numbers_comments_under_one_preamble() {
        let mut a = inline_comment("Guard this behind the lock.", false);
        a.path = Some("src/a.rs".into());
        a.line = Some(10);
        let mut b = inline_comment("Rename this field.", false);
        b.path = Some("src/b.rs".into());
        b.line = Some(20);

        let prompt = combined_fix_prompt(&[&a, &b]);

        // One shared preamble, not repeated per comment.
        assert!(prompt.starts_with("Address these PR review comments."));
        assert!(!prompt.contains("Address this PR review comment."));
        assert_eq!(prompt.matches("Address these").count(), 1);

        // Each comment appears as a numbered entry with its own file:line + text.
        assert!(prompt.contains("Comment 1:"));
        assert!(prompt.contains("Comment 2:"));
        assert!(prompt.contains("File: src/a.rs:10"));
        assert!(prompt.contains("Guard this behind the lock."));
        assert!(prompt.contains("File: src/b.rs:20"));
        assert!(prompt.contains("Rename this field."));

        // Still no file contents — only the comment text + diff hunks.
        assert!(!prompt.contains("fn "));
    }

    #[test]
    fn reply_target_inline_uses_thread_root() {
        // A reply (in_reply_to set) targets the thread root, not its own id.
        let mut leaf = inline_comment("thanks", false);
        leaf.id = 55;
        leaf.in_reply_to = Some(40);
        assert_eq!(
            leaf.reply_target(),
            ReplyTarget::InlineThread {
                root_comment_id: 40
            }
        );

        // A root inline comment (no in_reply_to) replies to itself.
        let root = inline_comment("nit", false);
        assert_eq!(
            root.reply_target(),
            ReplyTarget::InlineThread { root_comment_id: 1 }
        );
    }

    #[test]
    fn reply_target_conversation_and_summary_post_issue_comment() {
        let mut conv = inline_comment("hi", false);
        conv.kind = CommentKind::Conversation;
        assert_eq!(conv.reply_target(), ReplyTarget::Conversation);

        let mut summary = inline_comment("changes", false);
        summary.kind = CommentKind::ReviewSummary {
            state: "CHANGES_REQUESTED".into(),
        };
        assert_eq!(summary.reply_target(), ReplyTarget::Conversation);
    }

    #[test]
    fn fix_prompt_marks_outdated_and_omits_missing_pieces() {
        let mut c = inline_comment("Still relevant?", false);
        c.outdated = true;
        c.diff_hunk = None;
        let prompt = c.fix_prompt();
        assert!(prompt.contains("(comment is on a line that has since changed)"));
        assert!(!prompt.contains("Diff hunk:"));

        // Conversation/summary comments have no path: no File line at all.
        c.path = None;
        c.line = None;
        c.outdated = false;
        let prompt = c.fix_prompt();
        assert!(!prompt.contains("File:"));
        assert!(prompt.contains("Comment (@alice): Still relevant?"));
    }

    #[test]
    fn estimate_tokens_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn triage_state_db_str_roundtrips() {
        for state in [
            TriageState::Untriaged,
            TriageState::Fixing,
            TriageState::Done,
            TriageState::Skipped,
            TriageState::Replied,
        ] {
            assert_eq!(TriageState::from_db_str(state.as_db_str()), state);
        }
        // Unknown tokens degrade to untriaged rather than erroring.
        assert_eq!(TriageState::from_db_str("garbage"), TriageState::Untriaged);
    }

    #[test]
    fn triage_state_labels_and_markers() {
        assert_eq!(TriageState::Untriaged.label(), None);
        assert_eq!(TriageState::Untriaged.marker(), ' ');
        assert_eq!(TriageState::Done.label(), Some("done"));
        assert_eq!(TriageState::Done.marker(), 'x');
        assert_eq!(TriageState::Skipped.marker(), '-');
        assert_eq!(TriageState::Fixing.marker(), '~');
    }

    #[test]
    fn fix_target_defaults_to_dedicated_and_has_tags() {
        assert_eq!(FixTarget::default(), FixTarget::DedicatedReview);
        assert_eq!(FixTarget::DedicatedReview.tag(), "dedicated");
        assert_eq!(FixTarget::ExistingLive.tag(), "live");
    }

    #[test]
    fn fix_session_index_prefers_dedicated_else_creates() {
        use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
        let mut feature = Feature::new(
            "feat".into(),
            "branch".into(),
            std::path::PathBuf::from("/tmp/wd"),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );

        // Nothing running yet: both strategies report "must create / nothing to
        // reuse".
        assert_eq!(
            fix_session_index(&feature, FixTarget::DedicatedReview, REVIEW_SESSION_LABEL),
            None
        );
        assert_eq!(
            fix_session_index(&feature, FixTarget::ExistingLive, REVIEW_SESSION_LABEL),
            None
        );

        // A regular live agent session satisfies existing-live but not dedicated.
        feature.add_session_named(SessionKind::Claude, "Claude".into());
        assert_eq!(
            fix_session_index(&feature, FixTarget::ExistingLive, REVIEW_SESSION_LABEL),
            Some(0)
        );
        assert_eq!(
            fix_session_index(&feature, FixTarget::DedicatedReview, REVIEW_SESSION_LABEL),
            None
        );

        // Once the dedicated review session exists it is reused by label, while
        // existing-live still resolves to the first agent session.
        feature.add_session_named(SessionKind::Claude, REVIEW_SESSION_LABEL.into());
        assert_eq!(
            fix_session_index(&feature, FixTarget::DedicatedReview, REVIEW_SESSION_LABEL),
            Some(1)
        );
        assert_eq!(
            fix_session_index(&feature, FixTarget::ExistingLive, REVIEW_SESSION_LABEL),
            Some(0)
        );
    }

    #[test]
    fn fix_session_index_ignores_non_agent_sessions() {
        use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
        let mut feature = Feature::new(
            "feat".into(),
            "branch".into(),
            std::path::PathBuf::from("/tmp/wd"),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        // A terminal window is not an agent harness, so it is never a fix target.
        feature.add_session_named(SessionKind::Terminal, "Terminal".into());
        assert_eq!(
            fix_session_index(&feature, FixTarget::ExistingLive, REVIEW_SESSION_LABEL),
            None
        );
        assert_eq!(
            fix_session_index(&feature, FixTarget::DedicatedReview, REVIEW_SESSION_LABEL),
            None
        );
    }

    #[test]
    fn agent_text_strips_only_for_bots() {
        let human = PrComment {
            id: 1,
            kind: CommentKind::Inline,
            author: "alice".into(),
            is_bot: false,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            ai_generated: false,
            diff_hunk: None,
            body: "<details>keep?</details>plain".into(),
            snippet: String::new(),
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::Untriaged,
            local_note: None,
        };
        let mut bot = human.clone();
        bot.is_bot = true;
        assert!(human.agent_text().contains("<details>"));
        assert_eq!(bot.agent_text(), "plain");
    }

    fn review_comment(id: u64, path: Option<&str>, line: Option<u32>, body: &str) -> ReviewComment {
        bot_review_comment(id, path, line, body, false)
    }

    fn bot_review_comment(
        id: u64,
        path: Option<&str>,
        line: Option<u32>,
        body: &str,
        is_bot: bool,
    ) -> ReviewComment {
        ReviewComment {
            id,
            path: path.map(String::from),
            line,
            original_line: line,
            side: Some("RIGHT".into()),
            diff_hunk: None,
            subject_type: None,
            body: body.to_string(),
            user: if is_bot {
                user("coderabbitai", "Bot")
            } else {
                user("alice", "User")
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        }
    }

    fn review(id: u64, body: &str, is_bot: bool) -> Review {
        Review {
            id,
            body: body.to_string(),
            state: "COMMENTED".into(),
            user: if is_bot {
                user("coderabbitai", "Bot")
            } else {
                user("alice", "User")
            },
        }
    }

    #[test]
    fn bootstrap_depth_default_is_fifty() {
        assert_eq!(BootstrapDepth::default(), BootstrapDepth::Fifty);
        assert_eq!(BootstrapDepth::Fifty.limit(), 50);
        assert_eq!(BootstrapDepth::Twenty.limit(), 20);
        assert_eq!(BootstrapDepth::Hundred.limit(), 100);
        assert!(BootstrapDepth::All.limit() > 100);
    }

    #[test]
    fn bootstrap_pr_text_includes_location_and_review_lines() {
        let comments = vec![review_comment(
            1,
            Some("src/app/sync.rs"),
            Some(42),
            "Guard this behind the lock.",
        )];
        let reviews = vec![review(2, "Looks solid overall.", false)];
        let text = bootstrap_pr_text(&comments, &reviews);
        assert_eq!(
            text,
            "- (src/app/sync.rs:42) Guard this behind the lock.\n- (review) Looks solid overall."
        );
    }

    #[test]
    fn bootstrap_pr_text_strips_bot_boilerplate_and_skips_empty() {
        let comments = vec![
            bot_review_comment(
                1,
                Some("a.rs"),
                None,
                "<details><summary>Prompt for AI agents</summary>noise</details>Real point.",
                true,
            ),
            review_comment(2, None, None, "   "),
        ];
        let text = bootstrap_pr_text(&comments, &[]);
        assert_eq!(text, "- (a.rs) Real point.");
    }

    #[test]
    fn bootstrap_prompt_lists_every_pr_and_instructs_category_format() {
        let bodies = vec![
            (
                1,
                "Fix race".to_string(),
                "- (a.rs:1) Guard the lock".to_string(),
            ),
            (
                2,
                "Add tests".to_string(),
                "- (review) Needs tests".to_string(),
            ),
        ];
        let prompt = bootstrap_prompt(&bodies);
        assert!(prompt.contains("## Category"));
        assert!(prompt.contains("### PR #1: Fix race"));
        assert!(prompt.contains("- (a.rs:1) Guard the lock"));
        assert!(prompt.contains("### PR #2: Add tests"));
        assert!(prompt.contains("- (review) Needs tests"));
    }

    #[test]
    fn parse_ai_findings_parses_path_line_and_general_headings() {
        let output = "### src/app/sync.rs:42\n\
                       This can race with the poller.\n\
                       Guard it behind the lock.\n\n\
                       ### General\n\
                       Prefer explicit error types over anyhow here.\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].path.as_deref(), Some("src/app/sync.rs"));
        assert_eq!(findings[0].line, Some(42));
        assert_eq!(
            findings[0].body,
            "This can race with the poller.\nGuard it behind the lock."
        );
        assert_eq!(findings[1].path, None);
        assert_eq!(findings[1].line, None);
        assert_eq!(
            findings[1].body,
            "Prefer explicit error types over anyhow here."
        );
    }

    #[test]
    fn parse_ai_findings_malformed_heading_is_pathless() {
        // No colon, and a non-numeric "line" both degrade to pathless rather
        // than erroring — a partially-malformed response still parses.
        let output = "### no colon here\nSome finding.\n\n### src/x.rs:notanumber\nAnother.\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].path, None);
        assert_eq!(findings[0].body, "Some finding.");
        assert_eq!(findings[1].path, None);
        assert_eq!(findings[1].body, "Another.");
    }

    #[test]
    fn parse_ai_findings_drops_empty_findings() {
        // A heading with no body (or only blank lines) yields nothing to
        // triage, so it's dropped rather than becoming an empty draft.
        let output = "### a.rs:1\n\n### b.rs:2\nReal finding.\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path.as_deref(), Some("b.rs"));
    }

    #[test]
    fn parse_ai_findings_empty_output_yields_no_findings() {
        assert!(parse_ai_findings("").is_empty());
        assert!(parse_ai_findings("no headings at all, just prose").is_empty());
    }

    #[test]
    fn parse_ai_findings_tolerates_a_different_heading_level() {
        // The prompt asks for `###`, but models don't reliably hold a
        // specific heading level — `##` and bare `#` should still parse.
        let output = "## src/x.rs:5\nDouble-hash heading.\n\n# General\nSingle-hash heading.\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].path.as_deref(), Some("src/x.rs"));
        assert_eq!(findings[0].body, "Double-hash heading.");
        assert_eq!(findings[1].path, None);
        assert_eq!(findings[1].body, "Single-hash heading.");
    }

    #[test]
    fn parse_ai_findings_strips_an_outer_code_fence() {
        let output = "```markdown\n### a.rs:1\nFenced finding.\n```\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].body, "Fenced finding.");
    }

    #[test]
    fn parse_ai_findings_leaves_output_without_a_full_wrap_alone() {
        // A fence that doesn't wrap the *entire* response (e.g. a fenced
        // suggestion inside a finding's own body) must not be treated as the
        // outer-wrap case, or its content would be silently dropped.
        let output = "### a.rs:1\nSee below:\n```rust\nfoo();\n```\nmore text\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].body.contains("foo();"));
        assert!(findings[0].body.contains("more text"));
    }

    #[test]
    fn findings_to_comments_assigns_synthetic_ids_kind_and_ai_flag() {
        let findings = vec![
            AiFinding {
                path: Some("a.rs".into()),
                line: Some(10),
                body: "line finding".into(),
                diff_hunk: None,
            },
            AiFinding {
                path: Some("b.rs".into()),
                line: None,
                body: "file-level finding".into(),
                diff_hunk: None,
            },
            AiFinding {
                path: None,
                line: None,
                body: "general finding".into(),
                diff_hunk: None,
            },
        ];
        let comments = findings_to_comments(&findings);
        assert_eq!(comments.len(), 3);

        assert_eq!(comments[0].id, AI_FINDING_ID_BASE);
        assert_eq!(comments[0].kind, CommentKind::Inline);
        assert!(!comments[0].file_level);

        assert_eq!(comments[1].id, AI_FINDING_ID_BASE + 1);
        assert_eq!(comments[1].kind, CommentKind::Conversation);
        assert!(comments[1].file_level);

        assert_eq!(comments[2].id, AI_FINDING_ID_BASE + 2);
        assert_eq!(comments[2].kind, CommentKind::Conversation);
        assert!(!comments[2].file_level);

        for c in &comments {
            assert!(c.ai_generated);
            assert!(!c.is_bot);
            assert_eq!(c.author, "AI review");
            assert_eq!(c.triage, TriageState::Untriaged);
        }
    }

    #[test]
    fn ai_review_prompt_includes_diff_and_memory_context() {
        let prompt = ai_review_prompt(
            "diff --git a/x b/x\n+foo",
            "- Always guard shared state",
            None,
        );
        assert!(prompt.contains("diff --git a/x b/x"));
        assert!(prompt.contains("Always guard shared state"));
        assert!(prompt.contains(AI_FINDING_HEADING_PREFIX));
        assert!(!prompt.contains("skill/command"));
    }

    #[test]
    fn ai_review_prompt_leads_with_skill_directive_when_configured() {
        let prompt = ai_review_prompt("diff", "", Some("review"));
        assert!(prompt.starts_with("First, use the /review skill/command"));
    }

    #[test]
    fn ai_review_prompt_omits_empty_memory_section() {
        let prompt = ai_review_prompt("diff", "   \n", None);
        assert!(!prompt.contains("Known recurring findings"));
    }

    #[test]
    fn build_ai_review_splits_anchored_and_general_findings() {
        let comments = findings_to_comments(&[
            AiFinding {
                path: Some("a.rs".into()),
                line: Some(10),
                body: "Guard the lock here.".into(),
                diff_hunk: None,
            },
            AiFinding {
                path: Some("b.rs".into()),
                line: None,
                body: "Whole file needs a rewrite.".into(),
                diff_hunk: None,
            },
            AiFinding {
                path: None,
                line: None,
                body: "Consider consistent error handling.".into(),
                diff_hunk: None,
            },
        ]);
        let refs: Vec<&PrComment> = comments.iter().collect();
        let (summary, inline) = build_ai_review(&refs);

        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].path, "a.rs");
        assert_eq!(inline[0].line, 10);
        assert_eq!(inline[0].side, "RIGHT");
        assert_eq!(
            inline[0].body,
            "Guard the lock here.\n\n— drafted by Claude via AMF"
        );

        assert!(summary.contains("AI review, via AMF."));
        assert!(summary.contains("**b.rs**: Whole file needs a rewrite."));
        assert!(summary.contains("Consider consistent error handling."));
        // The anchored finding's own text shouldn't be duplicated into the
        // summary — it already posts as its own inline comment.
        assert!(!summary.contains("Guard the lock here."));
    }

    #[test]
    fn build_ai_review_with_no_general_findings_has_bare_summary() {
        let comments = findings_to_comments(&[AiFinding {
            path: Some("a.rs".into()),
            line: Some(1),
            body: "x".into(),
            diff_hunk: None,
        }]);
        let refs: Vec<&PrComment> = comments.iter().collect();
        let (summary, inline) = build_ai_review(&refs);
        assert_eq!(inline.len(), 1);
        assert_eq!(summary, "AI review, via AMF.");
    }

    #[test]
    fn append_ai_attribution_appends_footer_and_trims_trailing_whitespace() {
        assert_eq!(
            append_ai_attribution("Guard the lock here."),
            "Guard the lock here.\n\n— drafted by Claude via AMF"
        );
        assert_eq!(
            append_ai_attribution("Trailing newline.\n\n"),
            "Trailing newline.\n\n— drafted by Claude via AMF"
        );
    }

    fn sample_diff_files() -> Vec<crate::diff::DiffFile> {
        crate::diff::parse_unified_diff(
            "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 line1
-line2
+line2 updated
+line3
",
        )
        .unwrap()
    }

    #[test]
    fn diff_hunk_for_line_matches_the_covering_hunk() {
        let files = sample_diff_files();
        let hunk = diff_hunk_for_line(&files, "src/lib.rs", 1).expect("line 1 is in range");
        assert!(hunk.starts_with("@@ -1,2 +1,3 @@"));
        assert!(hunk.contains(" line1"));
        assert!(hunk.contains("-line2"));
        assert!(hunk.contains("+line2 updated"));
        assert!(hunk.contains("+line3"));

        // The last new-side line (3) is also covered.
        assert!(diff_hunk_for_line(&files, "src/lib.rs", 3).is_some());
    }

    #[test]
    fn diff_hunk_for_line_is_none_outside_the_hunk_or_file() {
        let files = sample_diff_files();
        assert!(diff_hunk_for_line(&files, "src/lib.rs", 999).is_none());
        assert!(diff_hunk_for_line(&files, "src/other.rs", 1).is_none());
    }

    #[test]
    fn diff_hunk_for_line_windows_a_large_hunk_instead_of_returning_it_whole() {
        // Regression: an AI finding can point at a line inside a large
        // contiguous block of new/changed code — e.g. a whole function this
        // PR added — where the *actual* diff hunk covering it spans the
        // entire block. Reconstructing that whole hunk read as "the whole
        // file" rather than "the lines this finding is about".
        // A pure-insertion hunk (no leading context line, so the added
        // lines' content numbers line up exactly with their new-file line
        // numbers) — as a whole new function this PR added would diff.
        let mut diff = String::from(
            "diff --git a/big.rs b/big.rs\n\
             index 1111111..2222222 100644\n\
             --- a/big.rs\n\
             +++ b/big.rs\n\
             @@ -1,0 +1,40 @@\n",
        );
        for i in 1..=40 {
            diff.push_str(&format!("+line{i}\n"));
        }
        let files = crate::diff::parse_unified_diff(&diff).unwrap();

        // Target a line comfortably in the middle of the 40-line hunk.
        let hunk = diff_hunk_for_line(&files, "big.rs", 20).expect("line 20 is in range");
        let lines: Vec<&str> = hunk.lines().collect();

        // Bounded to context-on-each-side + the target line + the header,
        // regardless of how large the real hunk is.
        assert!(
            lines.len() <= 2 * AI_FINDING_HUNK_CONTEXT_LINES + 2,
            "expected a small window, got {} lines:\n{hunk}",
            lines.len()
        );
        assert!(lines[0].starts_with("@@ "));
        assert!(hunk.contains("+line20"));
        // Lines far from the target on either side must not be included.
        assert!(!hunk.contains("+line1\n"));
        assert!(!hunk.contains("+line40"));
    }
}
