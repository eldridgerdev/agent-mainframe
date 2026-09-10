//! Orchestration for batched headless review of an oversized diff.
//!
//! [`review_batches`] takes the [`ReviewBatch`]es produced by
//! [`crate::diff_split::pack_file_sections`] and drives each one through a
//! harness-neutral [`BatchReviewRunner`]:
//!
//! - a packed group of whole files is reviewed in one prompt;
//! - if the harness rejects it as too long anyway ([`crate::headless::PromptTooLong`]),
//!   the group is halved and each half retried, recursively, down to one file;
//! - a lone file still too long is split hunk-by-hunk
//!   ([`crate::diff_split::split_file_by_hunk`]), each slice reviewed, and the
//!   slice findings merged back into one per-file result;
//! - a single hunk that overflows even alone cannot be divided further — it is
//!   recorded as an [`UncoveredSlice`] rather than dropped, so coverage stays
//!   honest.
//!
//! `review_batches` returns a flat list of per-batch finding texts plus the
//! explicit "not reviewed" list; `synthesize` folds those into one combined
//! payload (`review.synthesis`, with a `review.findings_summary` /
//! halving / deterministic-concat fallback chain). `batched_review` chains the
//! two. The `W` AI PR review (`app/ai_review.rs`) and final-review co-review
//! (`app/review.rs`) call in once they estimate a diff will not fit one
//! prompt.
//!
//! `allow(dead_code)`: a few forward-looking helpers on the public types
//! (`byte_len`, `is_empty`, …) have no caller yet.
#![allow(dead_code)]

use std::path::PathBuf;

use anyhow::Result;

use crate::diff_split::{
    FileSection, HunkFindingPart, HunkOutcome, ReviewBatch, merge_hunk_findings, split_file_by_hunk,
};
use crate::headless::{HeadlessRunner, as_prompt_too_long};
use crate::project::AgentKind;

/// Reason text attached to a hunk that overflows even as the only thing in its
/// prompt. Shared so the [`UncoveredSlice`] and the merged per-file block say
/// the same thing.
const LONE_HUNK_REASON: &str = "exceeds the size budget even as a single hunk";

/// Renders a per-batch review prompt from a slice of diff text. Supplied by
/// the call site from the registry-resolved `review.batch` template.
type BatchPromptRenderer = Box<dyn Fn(&str) -> String + Send + Sync>;

/// Renders a per-hunk-group review prompt from `(slice diff, file path, hunk
/// label)`. Supplied by the call site from the registry-resolved
/// `review.hunk_split` template.
type HunkPromptRenderer = Box<dyn Fn(&str, &str, &str) -> String + Send + Sync>;

/// Renders a synthesis or findings-summary prompt from two string inputs
/// (findings + coverage note, or label + findings).
type SynthesisPromptRenderer = Box<dyn Fn(&str, &str) -> String + Send + Sync>;

/// One reviewed slice of the diff: the harness's raw answer for a batch (or
/// for a hunk-split file, the merged per-slice answers).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchFinding {
    /// Files this block covers, in order.
    pub paths: Vec<String>,
    /// The answer text as returned by the harness / [`merge_hunk_findings`].
    pub text: String,
}

/// A slice of the diff that could not be reviewed even after halving to a
/// single hunk, or whose run failed for another reason. Surfaced verbatim in
/// the combined findings so partial coverage is never silent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UncoveredSlice {
    pub path: String,
    /// `"whole file"`, `"hunk 7"`, `"hunks 3\u{2013}5"`, …
    pub label: String,
    pub reason: String,
}

/// The outcome of [`review_batches`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatchReviewOutput {
    /// Per-batch finding texts, in batch order.
    pub findings: Vec<BatchFinding>,
    /// Slices that were not reviewed.
    pub uncovered: Vec<UncoveredSlice>,
}

impl BatchReviewOutput {
    pub fn is_empty(&self) -> bool {
        self.findings.is_empty() && self.uncovered.is_empty()
    }

    /// Every batch failed to produce findings — the caller should surface a
    /// hard error rather than presenting an all-gaps "review".
    pub fn only_uncovered(&self) -> bool {
        self.findings.is_empty() && !self.uncovered.is_empty()
    }

    /// Distinct file paths that produced at least one finding block.
    pub fn covered_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .findings
            .iter()
            .flat_map(|f| f.paths.iter().cloned())
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

/// Progress emitted as the flow works. The review call sites turn these into
/// the running screen's activity line and the partial-coverage note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchProgress {
    /// Starting batch `index` (1-based) of `total`.
    Batch {
        index: usize,
        total: usize,
        paths: Vec<String>,
    },
    /// A batch overflowed; it is being split in half and retried.
    Halving { paths: Vec<String> },
    /// A lone file is being split hunk-by-hunk.
    SplittingFile { path: String },
    /// Reviewing one hunk slice of a split file.
    Slice { path: String, label: String },
    /// Combining batch findings (emitted by the synthesis task).
    Synthesizing,
    /// A slice could not be reviewed.
    Uncovered { path: String, label: String },
}

/// Runs one bounded review prompt over a slice of diff text and returns the
/// harness's answer. Abstracted so the flow is testable without a real CLI and
/// so all four harnesses share one path.
pub trait BatchReviewRunner {
    fn review(&self, diff_text: &str) -> Result<String>;

    /// Review one hunk-group slice of a file whose own diff was still too large
    /// after file-level batching. `file_path` and `hunk_label` (`"hunk 3"` /
    /// `"hunks 4\u{2013}6"`) name the slice for the prompt. The default
    /// delegates to [`BatchReviewRunner::review`]; [`HeadlessBatchRunner`]
    /// overrides it to render the dedicated `review.hunk_split` prompt so its
    /// `{{file_path}}` / `{{hunk_label}}` placeholders are populated and an
    /// override of that prompt actually takes effect.
    fn review_hunk(
        &self,
        diff_text: &str,
        _file_path: &str,
        _hunk_label: &str,
    ) -> Result<String> {
        self.review(diff_text)
    }
}

/// Production [`BatchReviewRunner`]: renders the per-batch / per-hunk prompt and
/// runs it through [`HeadlessRunner`]. `render_prompt` is supplied by the call
/// site from the registry-resolved `review.batch` template, `render_hunk_prompt`
/// from `review.hunk_split`.
pub struct HeadlessBatchRunner {
    harness: AgentKind,
    workdir: PathBuf,
    model: Option<String>,
    render_prompt: BatchPromptRenderer,
    render_hunk_prompt: HunkPromptRenderer,
}

impl HeadlessBatchRunner {
    pub fn new(
        harness: AgentKind,
        workdir: PathBuf,
        model: Option<String>,
        render_prompt: BatchPromptRenderer,
        render_hunk_prompt: HunkPromptRenderer,
    ) -> Self {
        Self {
            harness,
            workdir,
            model,
            render_prompt,
            render_hunk_prompt,
        }
    }

    fn run(&self, prompt: &str) -> Result<String> {
        // Repo-aware (not `restricted`): per-slice prompts lose cross-file
        // context, so letting the harness read the tree back is worth more
        // than the isolation.
        HeadlessRunner::run(
            &self.harness,
            &self.workdir,
            prompt,
            self.model.as_deref(),
            false,
        )
    }
}

impl BatchReviewRunner for HeadlessBatchRunner {
    fn review(&self, diff_text: &str) -> Result<String> {
        self.run(&(self.render_prompt)(diff_text))
    }

    fn review_hunk(&self, diff_text: &str, file_path: &str, hunk_label: &str) -> Result<String> {
        self.run(&(self.render_hunk_prompt)(diff_text, file_path, hunk_label))
    }
}

/// Full batched review of an oversized unified `diff`: parse into per-file
/// sections, pack them into budgeted batches, review each (splitting on
/// overflow), then synthesize one combined payload. This is the single entry
/// point the review call sites use once they have decided the diff will not
/// fit one prompt.
///
/// `diff` must be a parseable unified diff with at least one file section; on
/// an unparseable or empty diff the returned [`SynthesizedReview`] is empty
/// and the caller should fall back to sending the whole prompt.
pub fn batched_review(
    diff: &str,
    batch_runner: &dyn BatchReviewRunner,
    synth_runner: &dyn SynthesisRunner,
    budget_tokens: usize,
    progress: &mut dyn FnMut(BatchProgress),
) -> SynthesizedReview {
    let files = crate::diff_split::SplitDiff::parse(diff).files;
    let batches = crate::diff_split::pack_file_sections(files, budget_tokens);
    let out = review_batches(batches, batch_runner, budget_tokens, progress);
    synthesize(out, synth_runner, progress)
}

/// Drive every batch through `runner`, adaptively splitting on overflow.
/// `budget_tokens` is only used to size the hunk-split of a lone oversized
/// file; the halving backstop is count-based and needs no budget.
pub fn review_batches(
    batches: Vec<ReviewBatch>,
    runner: &dyn BatchReviewRunner,
    budget_tokens: usize,
    progress: &mut dyn FnMut(BatchProgress),
) -> BatchReviewOutput {
    let total = batches.len();
    let mut out = BatchReviewOutput::default();

    for (i, batch) in batches.into_iter().enumerate() {
        progress(BatchProgress::Batch {
            index: i + 1,
            total,
            paths: batch.paths().into_iter().map(String::from).collect(),
        });
        match batch {
            ReviewBatch::Files(files) => {
                review_files(files, runner, budget_tokens, progress, &mut out)
            }
            ReviewBatch::OversizedFile(file) => {
                review_oversized_file(file, runner, budget_tokens, progress, &mut out)
            }
        }
    }

    out
}

/// Review a packed group of whole files; halve and recurse on overflow, hand a
/// lone overflowing file to the hunk-split path, and record a non-overflow
/// failure as uncovered while continuing.
fn review_files(
    mut files: Vec<FileSection>,
    runner: &dyn BatchReviewRunner,
    budget_tokens: usize,
    progress: &mut dyn FnMut(BatchProgress),
    out: &mut BatchReviewOutput,
) {
    if files.is_empty() {
        return;
    }
    let diff: String = files.iter().map(FileSection::reassemble).collect();
    let paths: Vec<String> = files.iter().map(|f| f.path.clone()).collect();

    match runner.review(&diff) {
        Ok(text) => out.findings.push(BatchFinding { paths, text }),
        Err(err) if as_prompt_too_long(&err).is_some() => {
            if files.len() > 1 {
                progress(BatchProgress::Halving {
                    paths: paths.clone(),
                });
                let right = files.split_off(files.len() / 2);
                review_files(files, runner, budget_tokens, progress, out);
                review_files(right, runner, budget_tokens, progress, out);
            } else {
                let file = files.into_iter().next().expect("len checked above");
                review_oversized_file(file, runner, budget_tokens, progress, out);
            }
        }
        Err(err) => {
            let reason = err.to_string();
            for path in paths {
                out.uncovered.push(UncoveredSlice {
                    path,
                    label: "whole file".to_string(),
                    reason: reason.clone(),
                });
            }
        }
    }
}

/// Split `file` hunk-by-hunk, review each slice, and push one merged per-file
/// finding. A slice the harness still rejects becomes an [`UncoveredSlice`] and
/// a matching `NotReviewed` entry in the merged block.
///
/// [`crate::diff_split::HunkSubunit::oversized`] is only a hint that a slice is
/// as small as it can be made — the runner is still asked, because a real
/// harness may accept a prompt the local estimate flagged.
fn review_oversized_file(
    file: FileSection,
    runner: &dyn BatchReviewRunner,
    budget_tokens: usize,
    progress: &mut dyn FnMut(BatchProgress),
    out: &mut BatchReviewOutput,
) {
    progress(BatchProgress::SplittingFile {
        path: file.path.clone(),
    });
    let split = split_file_by_hunk(&file, budget_tokens);
    let path = split.path.clone();
    let mut parts: Vec<HunkFindingPart> = Vec::new();

    for sub in split.subunits {
        let label = sub.label();
        progress(BatchProgress::Slice {
            path: path.clone(),
            label: label.clone(),
        });
        match review_hunk_section(&sub.section, &label, runner, progress, &path, out) {
            Ok(text) => parts.push(HunkFindingPart {
                label,
                outcome: HunkOutcome::Reviewed(text),
            }),
            Err(reason) => parts.push(HunkFindingPart {
                label,
                outcome: HunkOutcome::NotReviewed(reason),
            }),
        }
    }

    out.findings.push(BatchFinding {
        paths: vec![path.clone()],
        text: merge_hunk_findings(&path, &parts),
    });
}

/// Review one hunk-group slice. On overflow: if it holds more than one hunk,
/// halve the hunks and retry each half; a lone hunk that still overflows is
/// recorded uncovered. A non-overflow error is returned as the not-reviewed
/// reason.
fn review_hunk_section(
    section: &FileSection,
    label: &str,
    runner: &dyn BatchReviewRunner,
    progress: &mut dyn FnMut(BatchProgress),
    path: &str,
    out: &mut BatchReviewOutput,
) -> std::result::Result<String, String> {
    match runner.review_hunk(&section.reassemble(), path, label) {
        Ok(text) => Ok(text),
        Err(err) if as_prompt_too_long(&err).is_some() => {
            if section.hunks.len() <= 1 {
                progress(BatchProgress::Uncovered {
                    path: path.to_string(),
                    label: label.to_string(),
                });
                out.uncovered.push(UncoveredSlice {
                    path: path.to_string(),
                    label: label.to_string(),
                    reason: LONE_HUNK_REASON.to_string(),
                });
                return Err(LONE_HUNK_REASON.to_string());
            }

            let mut left = section.hunks.clone();
            let right = left.split_off(left.len() / 2);
            let mk = |hunks| FileSection {
                path: section.path.clone(),
                header: section.header.clone(),
                hunks,
            };
            let mut merged = String::new();
            for (n, half) in [mk(left), mk(right)].into_iter().enumerate() {
                let half_label = format!("{label} (part {})", n + 1);
                progress(BatchProgress::Slice {
                    path: path.to_string(),
                    label: half_label.clone(),
                });
                match review_hunk_section(&half, &half_label, runner, progress, path, out) {
                    Ok(text) => {
                        if !merged.is_empty() {
                            merged.push('\n');
                        }
                        merged.push_str(text.trim());
                    }
                    Err(reason) => {
                        if !merged.is_empty() {
                            merged.push('\n');
                        }
                        merged.push_str(&format!("_{half_label} not reviewed: {reason}_"));
                    }
                }
            }
            Ok(merged)
        }
        Err(err) => {
            let reason = err.to_string();
            progress(BatchProgress::Uncovered {
                path: path.to_string(),
                label: label.to_string(),
            });
            out.uncovered.push(UncoveredSlice {
                path: path.to_string(),
                label: label.to_string(),
                reason: reason.clone(),
            });
            Err(reason)
        }
    }
}

// ---------------------------------------------------------------------------
// Synthesis
//
// [`synthesize`] folds the per-batch finding texts into one review. It runs
// the `review.synthesis` prompt; if that overflows it shrinks each batch's
// findings through `review.findings_summary` and retries; if it still
// overflows it halves the finding list, synthesizes each half, and combines.
// Every failure path falls back to a deterministic concatenation so a
// combined payload is always produced — synthesis is a quality step, never a
// single point of failure.
// ---------------------------------------------------------------------------

/// Hard cap for the local truncation fallback when `review.findings_summary`
/// itself fails for a batch. The prompt is told the same number by the call
/// site's render closure.
pub const FALLBACK_SUMMARY_CHARS: usize = 1_500;

/// The two prompt operations synthesis needs, kept behind a trait for the same
/// reasons as [`BatchReviewRunner`]: testable without a CLI, one path for all
/// harnesses.
pub trait SynthesisRunner {
    /// Run `review.synthesis` over the concatenated batch findings.
    fn synthesize(&self, batch_findings: &str, uncovered_note: &str) -> Result<String>;
    /// Run `review.findings_summary` to shrink one batch's findings.
    fn summarize(&self, batch_label: &str, findings: &str) -> Result<String>;
}

/// Production [`SynthesisRunner`]: renders the `review.synthesis` /
/// `review.findings_summary` prompts (closures supplied by the call site from
/// the registry) and runs them through [`HeadlessRunner`].
pub struct HeadlessSynthesisRunner {
    harness: AgentKind,
    workdir: PathBuf,
    model: Option<String>,
    render_synthesis: SynthesisPromptRenderer,
    render_summary: SynthesisPromptRenderer,
}

impl HeadlessSynthesisRunner {
    pub fn new(
        harness: AgentKind,
        workdir: PathBuf,
        model: Option<String>,
        render_synthesis: SynthesisPromptRenderer,
        render_summary: SynthesisPromptRenderer,
    ) -> Self {
        Self {
            harness,
            workdir,
            model,
            render_synthesis,
            render_summary,
        }
    }

    fn run(&self, prompt: &str) -> Result<String> {
        HeadlessRunner::run(
            &self.harness,
            &self.workdir,
            prompt,
            self.model.as_deref(),
            true,
        )
    }
}

impl SynthesisRunner for HeadlessSynthesisRunner {
    fn synthesize(&self, batch_findings: &str, uncovered_note: &str) -> Result<String> {
        self.run(&(self.render_synthesis)(batch_findings, uncovered_note))
    }
    fn summarize(&self, batch_label: &str, findings: &str) -> Result<String> {
        self.run(&(self.render_summary)(batch_label, findings))
    }
}

/// One combined review payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SynthesizedReview {
    /// The combined text handed to the review UI / destination picker.
    pub text: String,
    /// `false` when the synthesis prompt could not run and `text` is a
    /// deterministic concatenation of the batch findings — the `W` review's
    /// coverage note calls this out so the reviewer knows the combine step
    /// was skipped.
    pub synthesis_ran: bool,
    /// Passed through from [`review_batches`] for the partial-coverage panel.
    pub uncovered: Vec<UncoveredSlice>,
}

/// Fold a [`BatchReviewOutput`] into one review. Never fails: on any synthesis
/// error it returns the deterministic concatenation with `synthesis_ran`
/// `false`.
pub fn synthesize(
    output: BatchReviewOutput,
    runner: &dyn SynthesisRunner,
    progress: &mut dyn FnMut(BatchProgress),
) -> SynthesizedReview {
    let uncovered_note = render_uncovered_note(&output.uncovered);

    if output.findings.is_empty() {
        // Nothing to combine — the payload is just the coverage note (or
        // empty). Not a synthesis "run".
        return SynthesizedReview {
            text: uncovered_note.trim_end().to_string(),
            synthesis_ran: false,
            uncovered: output.uncovered,
        };
    }

    progress(BatchProgress::Synthesizing);
    let (body, ran) =
        synthesize_findings(output.findings, &uncovered_note, runner, progress, false);
    let text = if ran {
        body
    } else {
        deterministic_fallback(&body, &uncovered_note)
    };

    SynthesizedReview {
        text,
        synthesis_ran: ran,
        uncovered: output.uncovered,
    }
}

/// Recursive core: try to synthesize `findings`; on overflow summarize (once)
/// then halve. Returns `(text, synthesis_ran)`; `synthesis_ran` is `false`
/// when a deterministic concatenation was used. `uncovered_note` is threaded
/// into the prompt only — the deterministic branches leave it to the caller
/// so it is not duplicated across halves.
fn synthesize_findings(
    findings: Vec<BatchFinding>,
    uncovered_note: &str,
    runner: &dyn SynthesisRunner,
    progress: &mut dyn FnMut(BatchProgress),
    already_summarized: bool,
) -> (String, bool) {
    if findings.is_empty() {
        return (String::new(), false);
    }

    let joined = join_findings(&findings);
    match runner.synthesize(&joined, uncovered_note) {
        Ok(text) => (text, true),
        Err(err) if as_prompt_too_long(&err).is_some() => {
            if !already_summarized {
                progress(BatchProgress::Synthesizing);
                let summarized = findings
                    .iter()
                    .map(|f| BatchFinding {
                        paths: f.paths.clone(),
                        text: summarize_or_truncate(runner, f),
                    })
                    .collect();
                return synthesize_findings(summarized, uncovered_note, runner, progress, true);
            }

            if findings.len() == 1 {
                // Already summarized and un-splittable: keep the block as-is.
                return (join_findings(&findings), false);
            }

            progress(BatchProgress::Synthesizing);
            let mut left = findings;
            let right = left.split_off(left.len() / 2);
            let (a, _) = synthesize_findings(left, "", runner, progress, true);
            let (b, _) = synthesize_findings(right, "", runner, progress, true);
            let combined = format!("{}\n\n{}", a.trim(), b.trim());
            match runner.synthesize(&combined, uncovered_note) {
                Ok(text) => (text, true),
                Err(_) => (combined, false),
            }
        }
        Err(_) => (joined, false),
    }
}

/// Shrink one batch's findings with `review.findings_summary`, falling back to
/// a hard character truncation if that call fails.
fn summarize_or_truncate(runner: &dyn SynthesisRunner, finding: &BatchFinding) -> String {
    let label = if finding.paths.is_empty() {
        "the changeset".to_string()
    } else {
        finding.paths.join(", ")
    };
    match runner.summarize(&label, &finding.text) {
        Ok(text) => text,
        Err(_) => hard_truncate(&finding.text, FALLBACK_SUMMARY_CHARS),
    }
}

/// One finding block per entry, `\n\n`-separated. A block that does not
/// already open with a markdown heading gets a `## <paths>` one so the
/// combined text keeps a consistent shape.
fn join_findings(findings: &[BatchFinding]) -> String {
    let mut out = String::new();
    for finding in findings {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let trimmed = finding.text.trim();
        if trimmed.starts_with('#') || finding.paths.is_empty() {
            out.push_str(trimmed);
        } else {
            out.push_str(&format!("## {}\n{trimmed}", finding.paths.join(", ")));
        }
    }
    out
}

/// The "not reviewed" section, or empty when nothing was skipped. Ends with a
/// blank line so it slots in front of the findings in the synthesis prompt.
fn render_uncovered_note(uncovered: &[UncoveredSlice]) -> String {
    if uncovered.is_empty() {
        return String::new();
    }
    let mut out = String::from(
        "## Not reviewed\n\nThese slices were too large to review even after splitting:\n",
    );
    for slice in uncovered {
        out.push_str(&format!(
            "- `{}` {} \u{2014} {}\n",
            slice.path, slice.label, slice.reason
        ));
    }
    out.push('\n');
    out
}

/// Wrap the deterministic concatenation with a stub summary and the coverage
/// note so the payload still has the `## Summary` / findings shape the review
/// parser expects.
fn deterministic_fallback(body: &str, uncovered_note: &str) -> String {
    let mut out = String::from(
        "## Summary\nAutomatic synthesis was unavailable; the per-slice findings are combined \
         verbatim below.\n\n",
    );
    if !uncovered_note.is_empty() {
        out.push_str(uncovered_note.trim_end());
        out.push_str("\n\n");
    }
    out.push_str(body.trim());
    out
}

/// Truncate `s` to at most `max_chars` characters on a char boundary, adding
/// an ellipsis marker when it actually cut.
fn hard_truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}\u{2026}", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;
    use crate::diff_split::{HunkGroup, SplitDiff, pack_file_sections};

    /// A runner that fails with `PromptTooLong` whenever the diff text exceeds
    /// `limit` bytes and otherwise echoes a deterministic answer.
    struct FakeRunner {
        limit: usize,
        calls: RefCell<Vec<String>>,
    }

    impl FakeRunner {
        fn new(limit: usize) -> Self {
            Self {
                limit,
                calls: RefCell::new(Vec::new()),
            }
        }
        fn call_count(&self) -> usize {
            self.calls.borrow().len()
        }
    }

    impl BatchReviewRunner for FakeRunner {
        fn review(&self, diff_text: &str) -> Result<String> {
            self.calls.borrow_mut().push(diff_text.to_string());
            if diff_text.len() > self.limit {
                Err(crate::headless::prompt_too_long_error(
                    &AgentKind::Claude,
                    "prompt is too long",
                ))
            } else {
                Ok(format!("REVIEWED {} bytes", diff_text.len()))
            }
        }
    }

    /// A runner that always fails, with an overflow error or a plain one.
    struct AlwaysFail {
        overflow: bool,
    }
    impl BatchReviewRunner for AlwaysFail {
        fn review(&self, _diff_text: &str) -> Result<String> {
            if self.overflow {
                Err(crate::headless::prompt_too_long_error(
                    &AgentKind::Claude,
                    "prompt is too long",
                ))
            } else {
                Err(anyhow::anyhow!("401 Unauthorized"))
            }
        }
    }

    fn sink() -> impl FnMut(BatchProgress) {
        |_| {}
    }

    fn section(path: &str, hunk_bytes: &[usize]) -> FileSection {
        let header = format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n");
        let hunks = hunk_bytes
            .iter()
            .enumerate()
            .map(|(i, &b)| HunkGroup {
                header: format!("@@ -{0},1 +{0},1 @@\n", i * 10 + 1),
                body: format!("+{}\n", "x".repeat(b.max(1))),
            })
            .collect();
        FileSection {
            path: path.to_string(),
            header,
            hunks,
        }
    }

    #[test]
    fn small_batches_each_produce_one_finding() {
        let batches = vec![
            ReviewBatch::Files(vec![section("a.rs", &[20]), section("b.rs", &[20])]),
            ReviewBatch::Files(vec![section("c.rs", &[20])]),
        ];
        let runner = FakeRunner::new(100_000);
        let mut events = Vec::new();
        let out = review_batches(batches, &runner, 1_000, &mut |p| events.push(p));

        assert_eq!(out.findings.len(), 2);
        assert!(out.uncovered.is_empty());
        assert_eq!(out.findings[0].paths, ["a.rs", "b.rs"]);
        assert_eq!(out.findings[1].paths, ["c.rs"]);
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e, BatchProgress::Batch { .. }))
                .count(),
            2
        );
    }

    #[test]
    fn an_overflowing_group_is_halved_until_it_fits() {
        // Four files; the whole group and each pair overflow, singletons fit.
        let files = vec![
            section("a.rs", &[400]),
            section("b.rs", &[400]),
            section("c.rs", &[400]),
            section("d.rs", &[400]),
        ];
        let one = files[0].reassemble().len();
        let runner = FakeRunner::new(one + 10); // only a single file fits
        let mut halvings = 0;
        let out = review_batches(vec![ReviewBatch::Files(files)], &runner, 10_000, &mut |p| {
            if matches!(p, BatchProgress::Halving { .. }) {
                halvings += 1;
            }
        });

        assert!(out.uncovered.is_empty());
        assert_eq!(out.covered_paths(), ["a.rs", "b.rs", "c.rs", "d.rs"]);
        assert!(halvings >= 1, "the group was split at least once");
    }

    #[test]
    fn a_lone_file_that_overflows_is_split_by_hunk() {
        // One file, three small hunks; the whole file overflows but each hunk
        // (with the header) fits.
        let file = section("big.rs", &[300, 300, 300]);
        let header_plus_one = {
            let h = file.header.len() + file.hunks[0].reassemble().len();
            h + 10
        };
        let runner = FakeRunner::new(header_plus_one);
        let mut split_events = 0;
        let out = review_batches(
            vec![ReviewBatch::Files(vec![file])],
            &runner,
            // Token budget that fits header + one hunk (~92 tokens) but not
            // two, so each slice is a single hunk.
            120,
            &mut |p| {
                if matches!(p, BatchProgress::SplittingFile { .. }) {
                    split_events += 1;
                }
            },
        );

        assert_eq!(split_events, 1);
        assert_eq!(out.findings.len(), 1);
        assert_eq!(out.findings[0].paths, ["big.rs"]);
        assert!(out.findings[0].text.starts_with("## big.rs"));
        assert!(out.uncovered.is_empty());
    }

    #[test]
    fn hunk_slices_go_through_review_hunk_with_the_file_path_and_label() {
        // A runner that keeps `review` and `review_hunk` calls apart so we can
        // assert the hunk-split path uses the dedicated seam (and so its
        // `review.hunk_split` prompt / `{{file_path}}` / `{{hunk_label}}`
        // placeholders are actually populated by the production runner).
        struct SeamRunner {
            hunk_calls: RefCell<Vec<(String, String)>>,
        }
        impl BatchReviewRunner for SeamRunner {
            fn review(&self, diff_text: &str) -> Result<String> {
                // The whole-file prompt always overflows so the flow hunk-splits.
                Err(crate::headless::prompt_too_long_error(
                    &AgentKind::Claude,
                    &format!("prompt is too long ({} bytes)", diff_text.len()),
                ))
            }
            fn review_hunk(
                &self,
                _diff_text: &str,
                file_path: &str,
                hunk_label: &str,
            ) -> Result<String> {
                self.hunk_calls
                    .borrow_mut()
                    .push((file_path.to_string(), hunk_label.to_string()));
                Ok(format!("### {file_path}|RIGHT|1\nlooked at {hunk_label}"))
            }
        }

        let file = section("src/big.rs", &[120, 120, 120]);
        let runner = SeamRunner {
            hunk_calls: RefCell::new(Vec::new()),
        };
        let out = review_batches(
            vec![ReviewBatch::Files(vec![file])],
            &runner,
            // Budget that fits the header plus a single hunk but not two.
            80,
            &mut sink(),
        );

        let calls = runner.hunk_calls.borrow();
        assert!(!calls.is_empty(), "the hunk seam was used");
        assert!(calls.iter().all(|(path, _)| path == "src/big.rs"));
        assert!(
            calls.iter().all(|(_, label)| label.starts_with("hunk")),
            "each slice carries a hunk label: {calls:?}"
        );
        assert_eq!(out.findings.len(), 1);
        assert!(out.findings[0].text.contains("looked at hunk"));
        assert!(out.uncovered.is_empty());
    }

    #[test]
    fn a_single_hunk_over_budget_even_alone_is_recorded_uncovered() {
        let file = section("mixed.rs", &[40, 50_000, 40]);
        let runner = FakeRunner::new(2_000);
        let out = review_batches(
            vec![ReviewBatch::OversizedFile(file)],
            &runner,
            120,
            &mut sink(),
        );

        assert_eq!(out.uncovered.len(), 1);
        assert_eq!(out.uncovered[0].path, "mixed.rs");
        assert_eq!(out.uncovered[0].label, "hunk 2");
        assert!(out.uncovered[0].reason.contains("single hunk"));
        // The surrounding hunks still produced a merged block.
        assert_eq!(out.findings.len(), 1);
        assert!(out.findings[0].text.contains("NOT REVIEWED"));
    }

    #[test]
    fn a_non_overflow_error_is_recorded_and_the_flow_continues() {
        let batches = vec![
            ReviewBatch::Files(vec![section("a.rs", &[10]), section("b.rs", &[10])]),
            ReviewBatch::Files(vec![section("c.rs", &[10])]),
        ];
        let out = review_batches(batches, &AlwaysFail { overflow: false }, 1_000, &mut sink());

        assert!(out.findings.is_empty());
        assert_eq!(out.uncovered.len(), 3);
        assert!(out.uncovered.iter().all(|u| u.reason.contains("401")));
        assert!(out.only_uncovered());
    }

    #[test]
    fn review_hunk_section_halves_a_multi_hunk_group_that_overflows() {
        // Force `split_file_by_hunk` to emit one multi-hunk group (budget high
        // enough to group all four), but the runner rejects any diff with more
        // than one hunk's worth of bytes — so the group must be halved.
        let file = section("g.rs", &[200, 200, 200, 200]);
        let one_hunk = file.header.len() + file.hunks[0].reassemble().len();
        let runner = FakeRunner::new(one_hunk + 5);
        // Big token budget => split_file_by_hunk keeps all 4 hunks in one slice.
        let out = review_batches(
            vec![ReviewBatch::OversizedFile(file)],
            &runner,
            100_000,
            &mut sink(),
        );

        assert_eq!(out.findings.len(), 1);
        assert!(out.uncovered.is_empty(), "every hunk fit once isolated");
        // Each of the 4 hunks was reviewed on its own eventually.
        assert!(runner.call_count() >= 4);
    }

    #[test]
    fn an_always_overflowing_single_hunk_terminates_and_is_uncovered() {
        let file = section("stuck.rs", &[100]);
        let runner = FakeRunner::new(0); // nothing ever fits
        let out = review_batches(
            vec![ReviewBatch::OversizedFile(file)],
            &runner,
            0,
            &mut sink(),
        );

        assert_eq!(out.uncovered.len(), 1);
        assert!(out.findings.len() <= 1);
    }

    #[test]
    fn end_to_end_from_a_raw_diff_covers_every_file() {
        // Build a multi-file diff, pack it tiny so most files are their own
        // batch, and confirm the flow reviews every path.
        let raw = format!(
            "{a}{b}{c}",
            a = section("one.rs", &[120, 120]).reassemble(),
            b = section("two.rs", &[500]).reassemble(),
            c = section("three.rs", &[60]).reassemble(),
        );
        let split = SplitDiff::parse(&raw);
        assert_eq!(split.files.len(), 3);
        let batches = pack_file_sections(split.files, 40);
        let runner = FakeRunner::new(100_000);
        let out = review_batches(batches, &runner, 40, &mut sink());

        assert_eq!(out.covered_paths(), ["one.rs", "three.rs", "two.rs"]);
        assert!(out.uncovered.is_empty());
    }

    // --- synthesis --------------------------------------------------------

    /// A synthesis runner: `synthesize` fails with `PromptTooLong` when its
    /// input exceeds `synth_limit` bytes (or always, with `synth_plain_err`);
    /// `summarize` returns a short string unless `summary_ok` is false.
    struct FakeSynth {
        synth_limit: usize,
        synth_plain_err: bool,
        summary_ok: bool,
        synth_calls: RefCell<usize>,
        summary_calls: RefCell<usize>,
    }

    impl FakeSynth {
        fn new(synth_limit: usize) -> Self {
            Self {
                synth_limit,
                synth_plain_err: false,
                summary_ok: true,
                synth_calls: RefCell::new(0),
                summary_calls: RefCell::new(0),
            }
        }
    }

    impl SynthesisRunner for FakeSynth {
        fn synthesize(&self, batch_findings: &str, _uncovered_note: &str) -> Result<String> {
            *self.synth_calls.borrow_mut() += 1;
            if self.synth_plain_err {
                return Err(anyhow::anyhow!("provider unreachable"));
            }
            if batch_findings.len() > self.synth_limit {
                Err(crate::headless::prompt_too_long_error(
                    &AgentKind::Claude,
                    "prompt is too long",
                ))
            } else {
                Ok(format!(
                    "## Summary\nsynthesized {} bytes\n\n### combined\nok",
                    batch_findings.len()
                ))
            }
        }
        fn summarize(&self, label: &str, _findings: &str) -> Result<String> {
            *self.summary_calls.borrow_mut() += 1;
            if self.summary_ok {
                // Keep the file coordinate, per the real prompt's contract.
                Ok(format!("### {label}|RIGHT|1\nshort"))
            } else {
                Err(anyhow::anyhow!("summarize failed"))
            }
        }
    }

    fn findings(n: usize, bytes_each: usize) -> Vec<BatchFinding> {
        (0..n)
            .map(|i| BatchFinding {
                paths: vec![format!("f{i}.rs")],
                text: format!(
                    "### f{i}.rs|RIGHT|1\n{}",
                    "detail ".repeat(bytes_each / 7 + 1)
                ),
            })
            .collect()
    }

    #[test]
    fn synthesis_returns_the_model_output_when_it_fits() {
        let out = BatchReviewOutput {
            findings: findings(3, 40),
            uncovered: vec![],
        };
        let runner = FakeSynth::new(100_000);
        let review = synthesize(out, &runner, &mut sink());

        assert!(review.synthesis_ran);
        assert!(review.text.starts_with("## Summary"));
        assert_eq!(*runner.synth_calls.borrow(), 1);
        assert_eq!(*runner.summary_calls.borrow(), 0);
    }

    #[test]
    fn synthesis_summarizes_each_batch_and_retries_on_overflow() {
        let out = BatchReviewOutput {
            findings: findings(4, 400),
            uncovered: vec![],
        };
        // First pass overflows; summarized findings are tiny and fit.
        let runner = FakeSynth::new(600);
        let review = synthesize(out, &runner, &mut sink());

        assert!(review.synthesis_ran);
        assert_eq!(*runner.summary_calls.borrow(), 4, "one summary per batch");
        assert!(
            *runner.synth_calls.borrow() >= 2,
            "retried after summarizing"
        );
    }

    #[test]
    fn synthesis_halves_when_even_summaries_overflow_then_combines() {
        let out = BatchReviewOutput {
            findings: findings(4, 400),
            uncovered: vec![],
        };
        // Anything over ~30 bytes overflows, so even the summarized set of 4
        // (`### f|RIGHT|1\nshort` × 4 joined) is too big and must be halved
        // down to single blocks, which fit.
        let runner = FakeSynth::new(30);
        let review = synthesize(out, &runner, &mut sink());

        // Every leaf synthesize of a single summarized block fits, so the
        // final combine of two leaves may or may not fit — either way a
        // payload comes back.
        assert!(!review.text.is_empty());
        assert_eq!(*runner.summary_calls.borrow(), 4);
    }

    #[test]
    fn synthesis_falls_back_to_deterministic_concat_when_nothing_fits() {
        let out = BatchReviewOutput {
            findings: findings(3, 100),
            uncovered: vec![UncoveredSlice {
                path: "huge.rs".to_string(),
                label: "hunk 2".to_string(),
                reason: "exceeds the size budget even as a single hunk".to_string(),
            }],
        };
        let runner = FakeSynth::new(0); // no synthesize input ever fits
        let review = synthesize(out, &runner, &mut sink());

        assert!(!review.synthesis_ran);
        assert!(review.text.starts_with("## Summary"));
        assert!(review.text.contains("Automatic synthesis was unavailable"));
        // Coverage note and every batch's findings survive the fallback.
        assert!(review.text.contains("## Not reviewed"));
        assert!(review.text.contains("`huge.rs` hunk 2"));
        assert!(review.text.contains("### f0.rs|RIGHT|1"));
        assert!(review.text.contains("### f2.rs|RIGHT|1"));
        assert_eq!(review.uncovered.len(), 1);
    }

    #[test]
    fn synthesis_with_no_findings_returns_just_the_coverage_note() {
        let out = BatchReviewOutput {
            findings: vec![],
            uncovered: vec![UncoveredSlice {
                path: "big.rs".to_string(),
                label: "hunk 9".to_string(),
                reason: "still too long".to_string(),
            }],
        };
        let runner = FakeSynth::new(100_000);
        let review = synthesize(out, &runner, &mut sink());

        assert!(!review.synthesis_ran);
        assert!(review.text.starts_with("## Not reviewed"));
        assert!(review.text.contains("`big.rs` hunk 9"));
        assert_eq!(*runner.synth_calls.borrow(), 0, "nothing to synthesize");
    }

    #[test]
    fn synthesis_with_nothing_at_all_is_empty() {
        let review = synthesize(
            BatchReviewOutput::default(),
            &FakeSynth::new(100_000),
            &mut sink(),
        );
        assert!(review.text.is_empty());
        assert!(!review.synthesis_ran);
    }

    #[test]
    fn a_non_overflow_synthesis_error_also_falls_back() {
        let out = BatchReviewOutput {
            findings: findings(2, 50),
            uncovered: vec![],
        };
        let mut runner = FakeSynth::new(100_000);
        runner.synth_plain_err = true;
        let review = synthesize(out, &runner, &mut sink());

        assert!(!review.synthesis_ran);
        assert!(review.text.contains("### f0.rs|RIGHT|1"));
        assert_eq!(*runner.summary_calls.borrow(), 0, "not an overflow");
    }

    #[test]
    fn a_failed_summary_falls_back_to_truncation_and_still_completes() {
        let out = BatchReviewOutput {
            findings: findings(3, 4_000),
            uncovered: vec![],
        };
        let mut runner = FakeSynth::new(500);
        runner.summary_ok = false;
        let review = synthesize(out, &runner, &mut sink());

        // Truncated blocks are still large, so this ends in the deterministic
        // fallback — but it completes and carries every file.
        assert!(!review.text.is_empty());
        assert!(review.text.contains("f0.rs"));
        assert!(review.text.contains("f2.rs"));
        assert_eq!(*runner.summary_calls.borrow(), 3);
    }

    #[test]
    fn batched_review_parses_packs_reviews_and_synthesizes() {
        // A combined fake: batch review always fits; synthesis always fits.
        struct BothFit;
        impl BatchReviewRunner for BothFit {
            fn review(&self, diff_text: &str) -> Result<String> {
                Ok(format!("### x|RIGHT|1\nreviewed {} bytes", diff_text.len()))
            }
        }
        impl SynthesisRunner for BothFit {
            fn synthesize(&self, batch_findings: &str, _note: &str) -> Result<String> {
                Ok(format!("## Summary\ncombined\n\n{batch_findings}"))
            }
            fn summarize(&self, _l: &str, f: &str) -> Result<String> {
                Ok(f.to_string())
            }
        }

        let raw = format!(
            "{a}{b}{c}",
            a = section("one.rs", &[80]).reassemble(),
            b = section("two.rs", &[80]).reassemble(),
            c = section("three.rs", &[80]).reassemble(),
        );
        let both = BothFit;
        let review = batched_review(&raw, &both, &both, 30, &mut sink());

        assert!(review.synthesis_ran);
        assert!(review.text.starts_with("## Summary"));
        assert!(review.uncovered.is_empty());
        // Every file reached the reviewer.
        for path in ["one.rs", "two.rs", "three.rs"] {
            assert!(review.text.contains(path), "{path} missing from payload");
        }
    }

    #[test]
    fn batched_review_of_an_unparseable_diff_is_empty() {
        struct Never;
        impl BatchReviewRunner for Never {
            fn review(&self, _d: &str) -> Result<String> {
                panic!("should not be called for an empty parse")
            }
        }
        impl SynthesisRunner for Never {
            fn synthesize(&self, _a: &str, _b: &str) -> Result<String> {
                Ok(String::new())
            }
            fn summarize(&self, _a: &str, _b: &str) -> Result<String> {
                Ok(String::new())
            }
        }
        let n = Never;
        let review = batched_review("not a diff at all\njust text\n", &n, &n, 100, &mut sink());
        assert!(review.text.is_empty());
        assert!(!review.synthesis_ran);
    }

    #[test]
    fn hard_truncate_respects_char_boundaries() {
        let s = "\u{e9}".repeat(50); // 50 two-byte chars
        let t = hard_truncate(&s, 10);
        assert!(t.chars().count() <= 10);
        assert!(t.ends_with('\u{2026}'));
        assert_eq!(hard_truncate("short", 100), "short");
    }

    // --- end-to-end -----------------------------------------------------

    /// A `diff --git` section for a pure rename, no hunks.
    fn rename_section(old: &str, new: &str) -> String {
        format!(
            "diff --git a/{old} b/{new}\nsimilarity index 100%\nrename from {old}\nrename to {new}\n"
        )
    }

    /// End-to-end over a ~100k-line, move-heavy synthetic diff: many renames
    /// (some with edits), one oversized file, run through
    /// `SplitDiff::parse` → `pack_file_sections` → `review_batches` →
    /// `synthesize` with fakes that force batching, hunk-splitting, and the
    /// synthesis summarize-then-halve fallback. Asserts full coverage and that
    /// each path is either reviewed or explicitly listed as not reviewed.
    #[test]
    fn end_to_end_100k_line_move_heavy_diff() {
        let mut raw = String::new();
        let mut expected_paths: Vec<String> = Vec::new();

        // 300 pure renames (header only) + 300 renames with a real edit.
        for i in 0..300 {
            raw.push_str(&rename_section(
                &format!("old/mod{i}/a.rs"),
                &format!("new/mod{i}/a.rs"),
            ));
            expected_paths.push(format!("new/mod{i}/a.rs"));

            let edited = format!("new/mod{i}/b.rs");
            raw.push_str(&format!(
                "diff --git a/old/mod{i}/b.rs b/{edited}\nsimilarity index 92%\nrename from old/mod{i}/b.rs\nrename to {edited}\n--- a/old/mod{i}/b.rs\n+++ b/{edited}\n"
            ));
            for h in 0..80 {
                raw.push_str(&format!(
                    "@@ -{0},3 +{0},3 @@\n-    let old_{h} = 1;\n+    let renamed_{h} = 1;\n     ok();\n",
                    h * 8 + 1
                ));
            }
            expected_paths.push(edited);
        }

        // One oversized file: ~1500 hunks so its own diff dwarfs any budget.
        raw.push_str(
            "diff --git a/src/giant.rs b/src/giant.rs\n--- a/src/giant.rs\n+++ b/src/giant.rs\n",
        );
        for h in 0..1500 {
            raw.push_str(&format!(
                "@@ -{0},4 +{0},4 @@\n context\n-old line {h} aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n+new line {h} aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n more\n",
                h * 6 + 1
            ));
        }
        expected_paths.push("src/giant.rs".to_string());

        assert!(
            raw.lines().count() > 90_000,
            "fixture should be ~100k lines, got {}",
            raw.lines().count()
        );

        // Parse must round-trip the whole thing losslessly.
        let split = crate::diff_split::SplitDiff::parse(&raw);
        assert_eq!(split.reassemble(), raw);
        assert_eq!(split.files.len(), expected_paths.len());

        // Batch runner accepts any prompt (batching is driven purely by the
        // token budget here); synthesis input over 2 KB overflows, forcing the
        // per-batch summarize-then-halve fallback.
        let batch_runner = FakeRunner::new(1_000_000);
        let synth_runner = FakeSynth::new(2_000);

        // --- review stage: run the batches directly so coverage is visible ---
        let budget = 5_000; // edit files pack; only `giant.rs` must hunk-split
        let batches = crate::diff_split::pack_file_sections(split.files, budget);
        let mut splitting_files = 0usize;
        let mut batch_events = 0usize;
        let out = review_batches(batches, &batch_runner, budget, &mut |p| match p {
            BatchProgress::SplittingFile { .. } => splitting_files += 1,
            BatchProgress::Batch { .. } => batch_events += 1,
            _ => {}
        });

        assert_eq!(splitting_files, 1, "only `giant.rs` needed hunk-splitting");
        assert!(batch_events > 10, "the diff was reviewed in many batches");

        // Every file is accounted for: reviewed, or explicitly not reviewed.
        let covered: std::collections::HashSet<String> = out.covered_paths().into_iter().collect();
        let uncovered: std::collections::HashSet<String> =
            out.uncovered.iter().map(|u| u.path.clone()).collect();
        for path in &expected_paths {
            assert!(
                covered.contains(path) || uncovered.contains(path),
                "{path} is neither reviewed nor listed as uncovered"
            );
        }
        // Nothing was dropped silently: coverage + gaps == the whole file set.
        assert_eq!(
            covered.union(&uncovered).count(),
            expected_paths.len(),
            "coverage plus gaps must equal every file in the diff"
        );

        // --- synthesis stage: including the summarize-then-halve fallback ---
        let review = synthesize(out, &synth_runner, &mut sink());
        assert!(review.synthesis_ran, "synthesis produced the combined text");
        assert!(
            *synth_runner.summary_calls.borrow() > 0,
            "the per-batch summarize fallback ran"
        );
        assert!(review.text.starts_with("## Summary"));
    }
}
