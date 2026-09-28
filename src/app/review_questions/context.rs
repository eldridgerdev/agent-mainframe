use crate::app::{AiReviewState, DiffScope, DiffViewerState};
use crate::diff::{DiffFile, DiffLineLocation, DiffSide};
use crate::prompts::PromptContext;
use anyhow::{Context, Result, ensure};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) enum ReviewTarget {
    Diff {
        scope: DiffScope,
        base: String,
        files: Arc<Vec<DiffFile>>,
        ignore_whitespace: bool,
    },
    Ai(crate::github::PrRef),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InlineAnchor {
    pub path: String,
    pub start: DiffLineLocation,
    pub end: DiffLineLocation,
    pub side: DiffSide,
}

#[derive(Debug, Clone)]
pub(crate) struct QuestionContext {
    pub workdir: PathBuf,
    pub label: String,
    pub target: ReviewTarget,
    pub focus: String,
    pub anchor: Option<InlineAnchor>,
    pub version: String,
}

#[derive(Debug, Clone)]
pub(crate) struct PreparedContext {
    pub stamp: String,
    pub files: Arc<Vec<DiffFile>>,
    pub revision: String,
}

pub(crate) fn hash(value: impl Hash) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

// Context expansion changes hunks but not file contents. Version the reviewed
// code independently of cursors, scrolling, and expanded context.
fn files_version(files: &[DiffFile]) -> String {
    hash(
        files
            .iter()
            .map(|f| {
                (
                    &f.path,
                    &f.old_path,
                    &f.old_content,
                    &f.new_content,
                    f.is_binary,
                    format!("{:?}", f.status),
                    if f.old_content.is_none() && f.new_content.is_none() {
                        Some(f.patch.as_str())
                    } else {
                        None
                    },
                )
            })
            .collect::<Vec<_>>(),
    )
}

impl QuestionContext {
    pub fn diff_version(state: &DiffViewerState) -> String {
        hash((
            &state.workdir,
            format!("{:?}", state.scope),
            &state.base_commit,
            files_version(&state.files),
        ))
    }
    pub fn ai_version(state: &AiReviewState) -> String {
        hash((
            &state.workdir,
            &state.pr.url,
            state.pr.number,
            &state.pr.head_sha,
        ))
    }
    pub fn from_diff(state: &DiffViewerState) -> Self {
        let file = state.files.get(state.selected_file);
        let anchor = file.and_then(|f| {
            let lines = f.addressable_lines();
            let end = state.comment_cursor?;
            let start = state.comment_anchor.unwrap_or(end);
            let range = start.min(end)..=start.max(end);
            let first = *lines.get(*range.start())?;
            let last = *lines.get(*range.end())?;
            let side = if last.new_line.is_some() {
                DiffSide::New
            } else {
                DiffSide::Old
            };
            if !range.clone().all(|i| lines[i].line_on(side).is_some()) {
                return None;
            }
            Some(InlineAnchor {
                path: f.path.clone(),
                start: first,
                end: last,
                side,
            })
        });
        let focus = file
            .map(|f| {
                format!(
                    "File: {}\nSelected anchor: {:?}\n{}",
                    f.path, anchor, f.patch
                )
            })
            .unwrap_or_else(|| "Whole review; no file or line selection".into());
        let label = match &state.scope {
            DiffScope::PullRequest(t) => {
                format!("{} PR #{} at {}", t.repo, t.pr.number, t.pr.head_oid)
            }
            _ => format!(
                "Final Review: project {}, feature {}",
                state.from_view.project_name, state.from_view.feature_name
            ),
        };
        let version = Self::diff_version(state);
        Self {
            workdir: state.workdir.clone(),
            label,
            target: ReviewTarget::Diff {
                scope: state.scope.clone(),
                base: state.base_commit.clone(),
                files: Arc::new(state.files.clone()),
                ignore_whitespace: state.ignore_whitespace,
            },
            focus,
            anchor,
            version,
        }
    }

    pub fn from_ai(state: &AiReviewState) -> Self {
        let finding = state.findings.get(state.selected);
        let anchor = finding.and_then(|f| {
            let side = f.side?;
            let line = f.line? as usize;
            let location = match side {
                DiffSide::New => DiffLineLocation {
                    old_line: None,
                    new_line: Some(line),
                },
                DiffSide::Old => DiffLineLocation {
                    old_line: Some(line),
                    new_line: None,
                },
            };
            Some(InlineAnchor {
                path: f.path.clone()?,
                start: location,
                end: location,
                side,
            })
        });
        Self {
            workdir: state.workdir.clone(),
            label: format!(
                "{}/{} PR #{}",
                state.pr.owner, state.pr.repo, state.pr.number
            ),
            target: ReviewTarget::Ai(state.pr.clone()),
            focus: finding
                .map(|f| {
                    format!(
                        "Selected finding: {:?}\n{}\n{}",
                        anchor,
                        f.body,
                        f.diff_hunk.as_deref().unwrap_or("")
                    )
                })
                .unwrap_or_else(|| "Whole PR; no finding or line selection".into()),
            anchor,
            version: Self::ai_version(state),
        }
    }

    pub fn tokens(
        &self,
        prepared: &PreparedContext,
        question: &str,
        earlier: &str,
        answer: &str,
    ) -> PromptContext {
        PromptContext::new()
            .with("review_identity", &self.label)
            .with("repository_path", self.workdir.display().to_string())
            .with("review_revision", &prepared.revision)
            .with("context_version", &self.version)
            .with("selection", &self.focus)
            .with("question", question)
            .with("earlier_turns", earlier)
            .with("answer", answer)
            .with(
                "diff",
                prepared
                    .files
                    .iter()
                    .map(|f| f.patch.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
    }
}

pub(crate) fn git(workdir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .context("Repository unavailable")?;
    ensure!(
        output.status.success(),
        "Repository context unavailable: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(String::from_utf8(output.stdout)?.trim_end().to_string())
}

pub(crate) fn repository_stamp(workdir: &Path) -> Result<String> {
    let head = git(workdir, &["rev-parse", "HEAD"])?;
    let patch = git(
        workdir,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            "HEAD",
            "--",
        ],
    )?;
    let staged = git(
        workdir,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--binary",
            "--",
        ],
    )?;
    let paths = git(
        workdir,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )?;
    let mut untracked = Vec::new();
    for path in paths.split('\0').filter(|p| !p.is_empty()) {
        untracked.push((
            path,
            std::fs::read(workdir.join(path)).with_context(|| format!("Cannot inspect {path}"))?,
        ));
    }
    Ok(hash((head, patch, staged, untracked)))
}

pub(crate) type AiDiffLoader = fn(&Path, &crate::github::PrRef) -> Result<Vec<DiffFile>>;

pub(crate) fn load_ai_diff(path: &Path, pr: &crate::github::PrRef) -> Result<Vec<DiffFile>> {
    let current = crate::github::GhCli::fetch_pr_by_number(path, pr.number)?;
    ensure!(
        current.head_sha == pr.head_sha,
        "PR revision changed; reopen the review before asking"
    );
    let diff = crate::github::GhCli::pr_diff(path, pr.number)?;
    let current = crate::github::GhCli::fetch_pr_by_number(path, pr.number)?;
    ensure!(
        current.head_sha == pr.head_sha,
        "PR revision changed while loading its diff; reopen the review"
    );
    crate::diff::parse_unified_diff(&diff)
}

pub(crate) fn prepare(context: &QuestionContext, ai_diff: AiDiffLoader) -> Result<PreparedContext> {
    let before = repository_stamp(&context.workdir)?;
    let head = git(&context.workdir, &["rev-parse", "HEAD"])?;
    let files = match &context.target {
        ReviewTarget::Diff {
            scope: DiffScope::CurrentChanges,
            base,
            files,
            ignore_whitespace,
        } => {
            let snapshot =
                crate::diff::load_snapshot(&context.workdir, Some(base), *ignore_whitespace)?;
            ensure!(
                snapshot.base_commit == *base
                    && files_version(&snapshot.files) == files_version(files),
                "Working-tree content changed since this review loaded; refresh the review and ask again"
            );
            snapshot.files
        }
        ReviewTarget::Diff {
            scope,
            files,
            ignore_whitespace,
            ..
        } => {
            let (base, expected) = match scope {
                DiffScope::PullRequest(t) => (t.merge_base_oid.as_str(), t.pr.head_oid.as_str()),
                DiffScope::Commit(c) => ("", c.hash.as_str()),
                DiffScope::CurrentChanges => unreachable!(),
            };
            ensure!(
                head == expected,
                "Matching checkout unavailable: review is at {expected}, checkout is at {head}. Repository search stopped; return to review or select a matching checkout"
            );
            ensure!(
                git(
                    &context.workdir,
                    &["status", "--porcelain", "--untracked-files=normal"]
                )?
                .is_empty(),
                "Matching checkout has local changes; repository search stopped"
            );
            let actual = if base.is_empty() {
                crate::diff::load_commit_snapshot(&context.workdir, expected, *ignore_whitespace)?
                    .files
            } else {
                crate::diff::load_range_snapshot(
                    &context.workdir,
                    base,
                    expected,
                    *ignore_whitespace,
                )?
                .files
            };
            ensure!(
                files_version(&actual) == files_version(files),
                "Pinned review content changed; reload the review before asking"
            );
            actual
        }
        ReviewTarget::Ai(pr) => {
            ensure!(
                head == pr.head_sha,
                "Matching PR checkout unavailable: review is at {}, checkout is at {head}; repository search stopped",
                pr.head_sha
            );
            ensure!(
                git(
                    &context.workdir,
                    &["status", "--porcelain", "--untracked-files=normal"]
                )?
                .is_empty(),
                "PR checkout has local changes; repository search stopped"
            );
            ai_diff(&context.workdir, pr)?
        }
    };
    ensure!(
        before == repository_stamp(&context.workdir)?,
        "Repository changed while preparing the question; retry"
    );
    Ok(PreparedContext {
        stamp: before,
        files: Arc::new(files),
        revision: head,
    })
}

pub(crate) fn validate_anchor(anchor: &InlineAnchor, files: &[DiffFile]) -> Result<()> {
    let file = files
        .iter()
        .find(|f| f.path == anchor.path)
        .context("Inline destination is outside the reviewed diff")?;
    let lines = file.addressable_lines();
    let start = anchor
        .start
        .line_on(anchor.side)
        .context("Invalid inline start side")?;
    let end = anchor
        .end
        .line_on(anchor.side)
        .context("Invalid inline end side")?;
    ensure!(
        start <= end
            && lines.iter().any(|l| l.line_on(anchor.side) == Some(start))
            && lines.iter().any(|l| l.line_on(anchor.side) == Some(end)),
        "Inline destination is outside the reviewed diff"
    );
    let start_location = file
        .resolve_source_line(anchor.side, start)
        .context("Invalid inline start")?;
    let end_location = file
        .resolve_source_line(anchor.side, end)
        .context("Invalid inline end")?;
    ensure!(
        file.hunk_for_location(start_location)
            .zip(file.hunk_for_location(end_location))
            .is_some_and(|(first, last)| std::ptr::eq(first, last)),
        "Inline range spans separate diff hunks; select lines in a single hunk"
    );
    Ok(())
}
