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
//! onto itself — once posted, a finding simply exists on GitHub and shows up
//! in PR Triage's own fetch on the next refresh. Reachable from PR Triage
//! (`A`), the dashboard, an agent session (leader key), and the PR picker.

use std::path::PathBuf;

use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use super::*;
use crate::editor::TextEditor;
use crate::github::{GhCli, PrRef, PrResolution, PrReviewComment as GhPrReviewComment};
use crate::headless::HeadlessRunner;

/// The heading [`ai_review_prompt`] instructs the agent to emit per finding,
/// e.g. `### src/app/sync.rs:42` or `### General`. [`parse_ai_findings`]
/// parses exactly this shape back out.
const AI_FINDING_HEADING_PREFIX: &str = "### ";

/// Lines of context kept on each side of the target line when extracting a
/// windowed hunk for a finding ([`diff_hunk_for_line`]). Deliberately small:
/// unlike a human reviewer's inline comment — which GitHub anchors to a hunk
/// that's already a few lines of context around a small change — an AI
/// finding can point at a line inside a large contiguous block of new code,
/// where the *actual* diff hunk covering it spans the whole block.
/// Reconstructing that whole hunk would defeat the point of showing "the
/// lines this finding is about."
const AI_FINDING_HUNK_CONTEXT_LINES: usize = 6;

/// Soft ceiling on the AI review's assembled prompt (diff + memory doc +
/// instructions): past this, a warning toast fires once the token estimate is
/// known, but the review still runs — chunking or an outright refusal isn't
/// worth the complexity until real use shows it's needed.
const AI_REVIEW_PROMPT_TOKEN_WARN: usize = 40_000;

/// One AI-review finding: parsed from the agent's fixed-format output
/// ([`parse_ai_findings`]), then kept/skipped/edited in the AI Review pane
/// before an optional `W` post. `path`/`line` are `None` for a finding with no
/// single-line anchor (the `### General` bucket).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AiReviewFinding {
    pub path: Option<String>,
    pub line: Option<u32>,
    /// Editable before posting (`e` in the pane).
    pub body: String,
    /// The hunk (from the PR diff) covering `path:line`, matching the shape
    /// GitHub's API hands over for free on a real review comment (`@@ ... @@`
    /// header + body). Unlike a GitHub comment, nothing about *generating* a
    /// finding produces this — it's reconstructed after parsing by
    /// re-matching `path:line` back into the already-fetched PR diff
    /// ([`diff_hunk_for_line`]). `None` when there's no anchor, or the line
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
pub struct AiReviewOutcome {
    pub findings: Vec<AiReviewFinding>,
    pub raw_output: String,
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
pub fn parse_ai_findings(output: &str) -> Vec<AiReviewFinding> {
    fn flush(
        current: Option<(Option<String>, Option<u32>, Vec<&str>)>,
        out: &mut Vec<AiReviewFinding>,
    ) {
        let Some((path, line, lines)) = current else {
            return;
        };
        let body = lines.join("\n").trim().to_string();
        if !body.is_empty() {
            out.push(AiReviewFinding {
                path,
                line,
                body,
                diff_hunk: None,
                skipped: false,
                published: false,
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

    super::pr_review::window_parsed_hunk(hunk, line, false, AI_FINDING_HUNK_CONTEXT_LINES)
}

/// Attribution appended to GitHub content the agent harness generated on the
/// user's behalf, as opposed to text the user typed. AI-review generation can
/// run through any supported headless harness, independent of whichever
/// harness a PR Triage "fix" gets injected into, so the marker stays provider
/// neutral rather than incorrectly attributing another harness to Claude.
fn append_ai_attribution(body: &str) -> String {
    format!("{}\n\n— drafted by AI via AMF", body.trim_end())
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
fn build_ai_review(findings: &[&AiReviewFinding]) -> (String, Vec<GhPrReviewComment>) {
    let mut inline = Vec::new();
    let mut general = Vec::new();
    for f in findings {
        match (&f.path, f.line) {
            // `diff_hunk.is_some()` gates whether the model's self-reported
            // line actually landed inside a hunk of the diff GitHub will
            // validate the review against (`diff_hunk_for_line`, computed
            // from the very same diff at generation time) — models count
            // lines from the raw unified-diff text themselves and can get
            // this wrong independent of whether the PR has since moved, so a
            // `None` hunk here means GitHub's create-review API would reject
            // this line too. Fold it into the summary instead of a doomed
            // inline comment.
            (Some(path), Some(line)) if f.diff_hunk.is_some() => inline.push(GhPrReviewComment {
                path: path.clone(),
                line,
                side: "RIGHT",
                start_line: None,
                start_side: None,
                body: append_ai_attribution(&f.body),
            }),
            (Some(path), Some(line)) => general.push(format!("- **{path}:{line}**: {}", f.body)),
            (Some(path), None) => general.push(format!("- **{path}**: {}", f.body)),
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

/// Background body of the AI PR review (`A`): assemble the prompt from
/// `diff` + `memory` (+ optional `skill`), report a token estimate, then make
/// **one** headless agent pass and parse its response into findings. Runs off
/// the UI thread; progress and the final result are reported over `tx`.
/// `model`, when set (`AppConfig::review_model`), picks the review's model
/// independent of whichever model the feature's interactive session runs.
fn run_ai_pr_review(
    harness: AgentKind,
    workdir: PathBuf,
    diff: String,
    memory: String,
    skill: Option<String>,
    model: Option<String>,
    tx: std::sync::mpsc::Sender<AiReviewProgress>,
) {
    let prompt = ai_review_prompt(&diff, &memory, skill.as_deref());
    let _ = tx.send(AiReviewProgress::Reviewing {
        token_estimate: super::pr_review::estimate_tokens(&prompt),
    });

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
                crate::headless::HeadlessProgress::Usage {
                    input_tokens,
                    output_tokens,
                } => AiReviewProgress::Usage {
                    input_tokens,
                    output_tokens,
                },
            };
            let _ = progress_tx.send(progress);
        },
    )
    .map(|output| {
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
        let cached = self.db.as_ref().and_then(|db| {
            db.load_ai_review_cache(pr.number, &pr.head_sha)
                .ok()
                .flatten()
        });
        let (findings, last_run) = match cached {
            Some(entry) => (entry.findings, entry.last_run),
            None => (Vec::new(), None),
        };
        self.mode = AppMode::AiReview(AiReviewState {
            workdir,
            pr,
            findings,
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            last_run,
            harness: None,
            harness_pick: None,
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

    fn cache_ai_review(&self, state: &AiReviewState) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let entry = AiReviewCacheEntry {
            findings: state.findings.clone(),
            last_run: state.last_run.clone(),
        };
        let _ = db.save_ai_review_cache(state.pr.number, &state.pr.head_sha, &entry);
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
            self.cache_ai_review(state);
        }
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
            self.cache_ai_review(state);
        }
    }

    pub fn ai_review_editing_finding(&self) -> bool {
        matches!(&self.mode, AppMode::AiReview(state) if state.finding_editor.is_some())
    }

    /// Kick off the AI PR review (`A`): resolve the review-memory doc
    /// synchronously (cheap, a local file read), then hand the PR-diff fetch
    /// and the one paid agent pass to a background thread and switch to the
    /// full-screen running view. No-op (with a hint) if a review is already
    /// running.
    pub fn start_ai_pr_review(&mut self) {
        if self.ai_review_bg.is_some() {
            self.push_toast_warning("AI review already running — wait for it to finish");
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
                });
            }
            return;
        };
        if !model_picked {
            // Pi's headless model flag isn't verified (see `HeadlessRunner`),
            // so a picker that can't do anything for it would just be
            // friction — skip straight to "default" for that harness.
            if harness == AgentKind::Pi {
                if let AppMode::AiReview(state) = &mut self.mode {
                    state.model_picked = true;
                }
                self.begin_ai_pr_review();
                return;
            }
            let rows = model_pick_rows(&harness);
            let configured = self.config.review_model.clone();
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
    fn begin_ai_pr_review(&mut self) {
        let mut origin = match &self.mode {
            AppMode::AiReview(state) => state.clone(),
            _ => return,
        };
        let Some(harness) = origin.harness.clone() else {
            return;
        };
        origin.harness_pick = None;
        origin.model_pick = None;
        origin.post_confirm = None;
        origin.finding_editor = None;

        let workdir = origin.workdir.clone();
        let number = origin.pr.number;
        // The pane's own pick (from the model picker) takes priority over the
        // `AppConfig::review_model` default it was seeded from — picking
        // "Default" in the picker clears it back to `None` explicitly.
        let model = origin
            .model
            .clone()
            .or_else(|| self.config.review_model.clone());
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
        let memory_path = review_memory::review_memory_path(
            &repo,
            self.configured_review_memory_path(&repo).as_deref(),
        );
        let memory = std::fs::read_to_string(&memory_path).unwrap_or_default();
        let skill = self.config.ai_review_skill.clone();

        let (tx, rx) = std::sync::mpsc::channel();
        self.ai_review_bg = Some(rx);
        self.ai_review_pending = Some(origin.clone());
        let thread_workdir = workdir.clone();
        std::thread::spawn(move || match GhCli::pr_diff(&thread_workdir, number) {
            Ok(diff) => run_ai_pr_review(harness, thread_workdir, diff, memory, skill, model, tx),
            Err(e) => {
                let _ = tx.send(AiReviewProgress::Done(Err(e)));
            }
        });

        self.mode = AppMode::AiReviewRunning(AiReviewRunState {
            origin,
            stage: AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        });
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
        let chosen = match &self.mode {
            AppMode::AiReview(state) => state
                .harness_pick
                .as_ref()
                .and_then(|pick| pick.agents.get(pick.selected).cloned()),
            _ => return,
        };
        let Some(chosen) = chosen else {
            return;
        };
        if let Err(error) = HeadlessRunner::check_available(&chosen) {
            if let AppMode::AiReview(state) = &mut self.mode
                && let Some(pick) = &mut state.harness_pick
            {
                pick.error = Some(error.to_string());
            }
            return;
        }
        if let AppMode::AiReview(state) = &mut self.mode {
            state.harness = Some(chosen.clone());
            state.harness_pick = None;
        }
        self.push_toast_success(format!(
            "AI reviews will run with {}",
            chosen.display_name()
        ));
        // Re-enter the same start-up chain rather than jumping straight to
        // `begin_ai_pr_review`: with a harness now chosen but no model picked
        // yet, this opens the model picker next.
        self.start_ai_pr_review();
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
    /// losing what's typed so far; otherwise cancel the picker outright (the
    /// harness stays chosen — pressing `A` again reopens this step).
    pub fn ai_review_model_pick_cancel(&mut self) {
        if let AppMode::AiReview(state) = &mut self.mode
            && let Some(pick) = &mut state.model_pick
        {
            if pick.editing_custom {
                pick.editing_custom = false;
            } else {
                state.model_pick = None;
            }
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
                    if let AppMode::AiReviewRunning(state) = &mut self.mode {
                        state.stage = AiReviewStage::Reviewing { token_estimate };
                    }
                    if token_estimate > AI_REVIEW_PROMPT_TOKEN_WARN {
                        large_diff_warning = Some(token_estimate);
                    }
                    changed = true;
                }
                Ok(AiReviewProgress::Activity(activity)) => {
                    if let AppMode::AiReviewRunning(state) = &mut self.mode {
                        state.activity = Some(activity);
                    }
                    changed = true;
                }
                Ok(AiReviewProgress::Usage {
                    input_tokens,
                    output_tokens,
                }) => {
                    if let AppMode::AiReviewRunning(state) = &mut self.mode {
                        state.usage = Some((input_tokens, output_tokens));
                    }
                    changed = true;
                }
                Ok(AiReviewProgress::Done(result)) => {
                    self.ai_review_bg = None;
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
                    let target = match &self.mode {
                        AppMode::AiReviewRunning(state) if state.origin.pr.number == pr_number => {
                            Target::Running
                        }
                        AppMode::AiReview(state) if state.pr.number == pr_number => {
                            Target::Pane(Box::new(state.clone()))
                        }
                        _ => Target::Elsewhere,
                    };

                    let mut landed: Option<AiReviewState> = None;
                    match result {
                        Ok(outcome) => {
                            let count = outcome.findings.len();
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
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ai_review_bg = None;
                    let pending = self.ai_review_pending.take();
                    let pr_number = pending.as_ref().map(|p| p.pr.number);
                    let detail = pr_number.map_or_else(
                        || "AI review failed unexpectedly".to_string(),
                        |number| format!("AI review of PR #{number} failed unexpectedly"),
                    );
                    self.log_error("pr_review", detail);
                    self.push_toast_error("AI review failed unexpectedly");
                    match &self.mode {
                        AppMode::AiReviewRunning(state)
                            if Some(state.origin.pr.number) == pr_number =>
                        {
                            self.mode = AppMode::AiReview(state.origin.clone());
                            changed = true;
                        }
                        AppMode::AiReview(state) if Some(state.pr.number) == pr_number => {
                            changed = true;
                        }
                        _ => {}
                    }
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
        let findings: Vec<AiReviewFinding> = match &self.mode {
            AppMode::AiReview(state) if state.post_confirm.is_none() => state
                .findings
                .iter()
                .filter(|f| !f.skipped && !f.published)
                .cloned()
                .collect(),
            _ => return,
        };
        if findings.is_empty() {
            self.push_toast_warning("No findings to post — run A to generate some, or skip fewer");
            return;
        }
        let refs: Vec<&AiReviewFinding> = findings.iter().collect();
        let (summary, inline) = build_ai_review(&refs);

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
                    post.editor.text().trim().to_string(),
                )
            }),
            _ => return Ok(()),
        };
        let Some((workdir, pr, inline, body)) = prep else {
            return Ok(());
        };
        let posted_count = inline.len().max(1); // at least the summary itself

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
        if let AppMode::AiReview(state) = &self.mode {
            self.cache_ai_review(state);
        }
        self.push_toast_success(format!(
            "Posted AI review · {posted_count} finding{} — follow up in PR Triage after a refresh",
            if posted_count == 1 { "" } else { "s" }
        ));
        Ok(())
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

    #[test]
    fn parse_ai_findings_parses_path_line_and_general_headings() {
        let output = "### src/app/sync.rs:42\nGuard this with the lock.\n\n### General\nConsider adding integration tests.\n";
        let findings = parse_ai_findings(output);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].path.as_deref(), Some("src/app/sync.rs"));
        assert_eq!(findings[0].line, Some(42));
        assert_eq!(findings[0].body, "Guard this with the lock.");
        assert!(!findings[0].skipped);
        assert!(!findings[0].published);
        assert_eq!(findings[1].path, None);
        assert_eq!(findings[1].line, None);
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
        let (summary, inline) = build_ai_review(&[&anchored, &general]);
        assert_eq!(inline.len(), 1);
        assert_eq!(inline[0].path, "src/lib.rs");
        assert!(inline[0].body.contains("drafted by AI via AMF"));
        assert!(summary.contains("general note"));
        assert!(summary.starts_with("AI review, via AMF."));
    }

    #[test]
    fn build_ai_review_with_no_general_findings_has_bare_summary() {
        let anchored = finding(
            Some("src/lib.rs"),
            Some(10),
            "fix this",
            Some("@@ -1 +1 @@"),
        );
        let (summary, inline) = build_ai_review(&[&anchored]);
        assert_eq!(inline.len(), 1);
        assert_eq!(summary, "AI review, via AMF.");
    }

    #[test]
    fn build_ai_review_folds_an_unmatched_line_into_the_summary_instead_of_posting_it() {
        // No diff_hunk means the line never matched the diff at generation
        // time — GitHub would reject the whole review if this were posted
        // inline, so it must fold into the summary instead.
        let unmatched = finding(Some("src/lib.rs"), Some(999), "miscounted line", None);
        let (summary, inline) = build_ai_review(&[&unmatched]);
        assert!(inline.is_empty());
        assert!(summary.contains("miscounted line"));
    }

    #[test]
    fn append_ai_attribution_appends_footer_and_trims_trailing_whitespace() {
        let body = append_ai_attribution("finding text  \n\n");
        assert_eq!(body, "finding text\n\n— drafted by AI via AMF");
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
    fn diff_hunk_for_line_matches_the_covering_hunk() {
        let files = sample_diff_files();
        let hunk = diff_hunk_for_line(&files, "src/lib.rs", 2);
        assert!(hunk.is_some());
        assert!(hunk.unwrap().contains("new line"));
    }

    #[test]
    fn diff_hunk_for_line_is_none_outside_the_hunk_or_file() {
        let files = sample_diff_files();
        assert!(diff_hunk_for_line(&files, "src/lib.rs", 9999).is_none());
        assert!(diff_hunk_for_line(&files, "src/other.rs", 1).is_none());
    }
}
