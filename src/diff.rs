use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;

use crate::worktree::WorktreeManager;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffSnapshot {
    pub branch: String,
    pub base_ref: String,
    pub base_commit: String,
    pub files: Vec<DiffFile>,
    pub total_additions: usize,
    pub total_deletions: usize,
}

/// A commit offered by the diff scope picker. Commits are limited to the
/// current branch's first-parent history after its resolved base, which keeps
/// the picker focused on changes that belong to the active feature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffCommit {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBase {
    pub branch: String,
    pub base_ref: String,
    pub base_commit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffFile {
    pub old_path: Option<String>,
    pub path: String,
    pub status: DiffFileStatus,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    pub old_content: Option<String>,
    pub new_content: Option<String>,
    pub patch: String,
    pub hunks: Vec<DiffHunk>,
}

/// Location of a commentable diff line: its line number on the base (old)
/// and/or current (new) side. Used to anchor line-level review comments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct DiffLineLocation {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

/// `DiffFile::addressable_lines()` over bare hunks, so a caller holding a
/// freshly built hunk list (context expansion) can see where its lines would
/// land before committing it to the file.
pub fn addressable_lines_in(hunks: &[DiffHunk]) -> Vec<DiffLineLocation> {
    let mut out = Vec::new();
    for hunk in hunks {
        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        for line in &hunk.lines {
            match line.kind {
                DiffLineKind::Context => {
                    out.push(DiffLineLocation {
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    });
                    old_line += 1;
                    new_line += 1;
                }
                DiffLineKind::Removed => {
                    out.push(DiffLineLocation {
                        old_line: Some(old_line),
                        new_line: None,
                    });
                    old_line += 1;
                }
                DiffLineKind::Added => {
                    out.push(DiffLineLocation {
                        old_line: None,
                        new_line: Some(new_line),
                    });
                    new_line += 1;
                }
                DiffLineKind::NoNewlineMarker => {}
            }
        }
    }
    out
}

impl DiffFile {
    /// Commentable diff lines (context / added / removed) across every hunk, in
    /// display order, each tagged with its base/current line numbers. Hunk
    /// headers and no-newline markers are not addressable. This walks the hunks
    /// in the same order the unified patch renderer does, so an index into the
    /// returned vector matches a rendered diff row.
    pub fn addressable_lines(&self) -> Vec<DiffLineLocation> {
        addressable_lines_in(&self.hunks)
    }

    /// The index into `addressable_lines()` of each hunk's first addressable
    /// line, in hunk order. The anchor list for jump-by-hunk navigation of the
    /// review line cursor. Hunks with no addressable lines are skipped so a
    /// jump always lands on a real cursor position.
    pub fn hunk_start_indices(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut idx = 0usize;
        for hunk in &self.hunks {
            let addressable = hunk
                .lines
                .iter()
                .filter(|l| !matches!(l.kind, DiffLineKind::NoNewlineMarker))
                .count();
            if addressable > 0 {
                out.push(idx);
            }
            idx += addressable;
        }
        out
    }

    /// The content of each addressable line, in the same order (and 1:1) as
    /// `addressable_lines()`, with the single-char diff prefix (`+`/`-`/space)
    /// stripped. Used to pre-fill a suggested-change editor with the current
    /// text of the line(s) a comment covers.
    pub fn addressable_line_texts(&self) -> Vec<String> {
        let mut out = Vec::new();
        for hunk in &self.hunks {
            for line in &hunk.lines {
                if matches!(line.kind, DiffLineKind::NoNewlineMarker) {
                    continue;
                }
                // The prefix byte is ASCII, so slicing at 1 is a valid boundary.
                out.push(line.text.get(1..).unwrap_or("").to_string());
            }
        }
        out
    }

    /// Whether opening this file in `$EDITOR` can do anything useful. A deletion
    /// has no file left on disk and a binary blob has nothing an editor can show,
    /// so both footers use this to hide the `E` hint rather than advertise a key
    /// that can only report why it won't work.
    pub fn can_open_in_editor(&self) -> bool {
        !self.is_binary && !matches!(self.status, DiffFileStatus::Deleted)
    }

    /// Whether this file's hunks can be re-rendered with more context. Needs
    /// both blobs: expansion pulls the surrounding lines out of them. Added,
    /// deleted, untracked and binary files never qualify — one side has no
    /// content, and for the first three the diff already shows every line.
    pub fn can_expand_context(&self) -> bool {
        !self.is_binary
            && !self.hunks.is_empty()
            && self.old_content.is_some()
            && self.new_content.is_some()
    }

    /// Rebuild this file's hunks with `context` lines of context around each run
    /// of changed lines, merging hunks whose context regions meet
    /// (`usize::MAX` renders the whole file as one hunk).
    ///
    /// Derived from the *current* hunks plus `old_content`/`new_content`, and
    /// idempotent: expansion only ever adds context lines, so the runs of
    /// added/removed lines it recovers are the same at any starting context.
    /// That means a viewer can step the level up and down without keeping a
    /// pristine copy of the parsed hunks around.
    ///
    /// Returns `None` when expansion isn't possible — including when the blobs
    /// no longer agree with the patch, where guessing would show the reviewer
    /// code that isn't in the diff.
    pub fn hunks_with_context(&self, context: usize) -> Option<Vec<DiffHunk>> {
        if !self.can_expand_context() {
            return None;
        }
        let old_content = self.old_content.as_deref()?;
        let new_content = self.new_content.as_deref()?;
        let old_lines = content_lines(old_content);
        let new_lines = content_lines(new_content);
        let old_total = old_lines.len();
        let new_total = new_lines.len();

        let runs = self.change_runs();
        if runs.is_empty() {
            return None;
        }
        if !runs
            .iter()
            .all(|run| run.matches_content(&old_lines, &new_lines))
        {
            return None;
        }

        // `\ No newline at end of file` is re-emitted rather than carried over:
        // the marker belongs to whichever side's final line lands in a hunk, and
        // expansion changes which lines those are.
        let old_ends_newline = old_content.is_empty() || old_content.ends_with('\n');
        let new_ends_newline = new_content.is_empty() || new_content.ends_with('\n');

        // Group runs whose context regions overlap or touch into one hunk, the
        // same way git merges hunks at a wider `--unified`.
        let mut groups: Vec<Vec<&ChangeRun>> = Vec::new();
        for run in &runs {
            let lead = run.leading_context(context);
            let merged = groups.last_mut().is_some_and(|group| {
                let prev = group.last().expect("groups are never empty");
                let (prev_old_end, prev_new_end) = prev.after();
                let trail =
                    trailing_context(context, prev_old_end, prev_new_end, old_total, new_total);
                if run.old_at.saturating_sub(lead) <= prev_old_end + trail {
                    group.push(run);
                    true
                } else {
                    false
                }
            });
            if !merged {
                groups.push(vec![run]);
            }
        }

        let mut out = Vec::new();
        for group in groups {
            let first = group[0];
            let lead = first.leading_context(context);
            let old_start = first.old_at - lead;
            let new_start = first.new_at - lead;
            let mut old_line = old_start;
            let mut new_line = new_start;
            let mut lines: Vec<DiffLine> = Vec::new();

            let push_context = |lines: &mut Vec<DiffLine>, old_line: usize, new_line: usize| {
                let text = new_lines
                    .get(new_line - 1)
                    .copied()
                    .or_else(|| old_lines.get(old_line - 1).copied())
                    .unwrap_or("");
                lines.push(DiffLine {
                    kind: DiffLineKind::Context,
                    text: format!(" {text}"),
                });
                if (old_line == old_total && !old_ends_newline)
                    || (new_line == new_total && !new_ends_newline)
                {
                    lines.push(no_newline_marker());
                }
            };

            for run in &group {
                while old_line < run.old_at {
                    push_context(&mut lines, old_line, new_line);
                    old_line += 1;
                    new_line += 1;
                }
                let (mut old_idx, mut new_idx) = (run.old_at, run.new_at);
                for line in &run.lines {
                    lines.push(line.clone());
                    match line.kind {
                        DiffLineKind::Removed => {
                            if old_idx == old_total && !old_ends_newline {
                                lines.push(no_newline_marker());
                            }
                            old_idx += 1;
                        }
                        DiffLineKind::Added => {
                            if new_idx == new_total && !new_ends_newline {
                                lines.push(no_newline_marker());
                            }
                            new_idx += 1;
                        }
                        _ => {}
                    }
                }
                let (after_old, after_new) = run.after();
                old_line = after_old;
                new_line = after_new;
            }

            let trail = trailing_context(context, old_line, new_line, old_total, new_total);
            for _ in 0..trail {
                push_context(&mut lines, old_line, new_line);
                old_line += 1;
                new_line += 1;
            }

            let old_count = old_line - old_start;
            let new_count = new_line - new_start;
            out.push(DiffHunk {
                header: hunk_header(old_start, old_count, new_start, new_count),
                old_start,
                old_lines: old_count,
                new_start,
                new_lines: new_count,
                lines,
            });
        }
        Some(out)
    }

    /// Recover the runs of consecutive changed lines from the current hunks —
    /// everything expansion has to preserve verbatim.
    fn change_runs(&self) -> Vec<ChangeRun> {
        let mut runs = Vec::new();
        for hunk in &self.hunks {
            // A zero-length side means the header points at the line *before*
            // the change (git's `@@ -5,0 +6,3 @@`), so the insertion point is
            // the next line. `addressable_lines()` doesn't need this — every
            // location on that side is `None` — but the context math does.
            let mut old_line = if hunk.old_lines == 0 {
                hunk.old_start + 1
            } else {
                hunk.old_start.max(1)
            };
            let mut new_line = if hunk.new_lines == 0 {
                hunk.new_start + 1
            } else {
                hunk.new_start.max(1)
            };
            let mut current: Option<ChangeRun> = None;
            for line in &hunk.lines {
                match line.kind {
                    DiffLineKind::Context => {
                        runs.extend(current.take());
                        old_line += 1;
                        new_line += 1;
                    }
                    DiffLineKind::Removed => {
                        let run = current.get_or_insert_with(|| ChangeRun::new(old_line, new_line));
                        run.lines.push(line.clone());
                        run.old_len += 1;
                        old_line += 1;
                    }
                    DiffLineKind::Added => {
                        let run = current.get_or_insert_with(|| ChangeRun::new(old_line, new_line));
                        run.lines.push(line.clone());
                        run.new_len += 1;
                        new_line += 1;
                    }
                    DiffLineKind::NoNewlineMarker => {}
                }
            }
            runs.extend(current.take());
        }
        runs
    }
}

/// git's `--unified=3` default — the context every freshly parsed hunk carries.
pub const DIFF_DEFAULT_CONTEXT: usize = 3;

/// A run of consecutive added/removed diff lines together with where it starts
/// on each side, recovered from a file's hunks so it can be re-emitted with a
/// different amount of surrounding context.
struct ChangeRun {
    /// 1-based old-side line the run starts at (for a pure insertion, the line
    /// it is inserted before).
    old_at: usize,
    /// 1-based new-side line the run starts at (for a pure deletion, the line
    /// the removal sits before).
    new_at: usize,
    old_len: usize,
    new_len: usize,
    lines: Vec<DiffLine>,
}

impl ChangeRun {
    fn new(old_at: usize, new_at: usize) -> Self {
        Self {
            old_at,
            new_at,
            old_len: 0,
            new_len: 0,
            lines: Vec::new(),
        }
    }

    /// The first line on each side *after* this run.
    fn after(&self) -> (usize, usize) {
        (self.old_at + self.old_len, self.new_at + self.new_len)
    }

    /// How many context lines can precede this run, capped by both file starts
    /// (the two sides advance in lockstep through context, so the tighter bound
    /// wins).
    fn leading_context(&self, context: usize) -> usize {
        context.min(self.old_at - 1).min(self.new_at - 1)
    }

    /// Whether the run's changed lines still match the blobs they were parsed
    /// against.
    fn matches_content(&self, old_lines: &[&str], new_lines: &[&str]) -> bool {
        let (mut old_idx, mut new_idx) = (self.old_at, self.new_at);
        for line in &self.lines {
            // The diff prefix is one ASCII byte, so slicing at 1 is a boundary.
            let text = line.text.get(1..).unwrap_or("");
            match line.kind {
                DiffLineKind::Removed => {
                    if old_idx.checked_sub(1).and_then(|i| old_lines.get(i)) != Some(&text) {
                        return false;
                    }
                    old_idx += 1;
                }
                DiffLineKind::Added => {
                    if new_idx.checked_sub(1).and_then(|i| new_lines.get(i)) != Some(&text) {
                        return false;
                    }
                    new_idx += 1;
                }
                _ => {}
            }
        }
        true
    }
}

/// How many context lines can follow a change ending just before `old_line` /
/// `new_line`, capped by both file ends.
fn trailing_context(
    context: usize,
    old_line: usize,
    new_line: usize,
    old_total: usize,
    new_total: usize,
) -> usize {
    context
        .min(old_total.saturating_sub(old_line - 1))
        .min(new_total.saturating_sub(new_line - 1))
}

fn no_newline_marker() -> DiffLine {
    DiffLine {
        kind: DiffLineKind::NoNewlineMarker,
        text: "\\ No newline at end of file".to_string(),
    }
}

/// Render a unified hunk header, following git's convention of omitting a
/// count of exactly 1.
fn hunk_header(old_start: usize, old_count: usize, new_start: usize, new_count: usize) -> String {
    let side = |start: usize, count: usize| {
        if count == 1 {
            start.to_string()
        } else {
            format!("{start},{count}")
        }
    };
    format!(
        "@@ -{} +{} @@",
        side(old_start, old_count),
        side(new_start, new_count)
    )
}

/// Split file content into lines without the trailing empty element a final
/// newline produces.
fn content_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    content
        .strip_suffix('\n')
        .unwrap_or(content)
        .split('\n')
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffFileStatus {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffHunk {
    pub header: String,
    pub old_start: usize,
    pub old_lines: usize,
    pub new_start: usize,
    pub new_lines: usize,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
    NoNewlineMarker,
}

/// `git diff -w`: treat lines that differ only in whitespace as unchanged.
/// Inserted into the argument list only when the reviewer asks for it, so the
/// default invocation is byte-for-byte what it always was.
const IGNORE_WHITESPACE_FLAG: &str = "--ignore-all-space";

/// Splice the ignore-whitespace flag into a `git diff` argument list.
fn with_whitespace_flag<'a>(args: &[&'a str], ignore_whitespace: bool) -> Vec<&'a str> {
    let mut out = args.to_vec();
    if ignore_whitespace {
        // After the subcommand, before the revision/pathspec operands.
        out.insert(1, IGNORE_WHITESPACE_FLAG);
    }
    out
}

pub fn load_snapshot(
    workdir: &Path,
    override_ref: Option<&str>,
    ignore_whitespace: bool,
) -> Result<DiffSnapshot> {
    let base = resolve_base_ref(workdir, override_ref)?;
    let tracked_patch = git_capture(
        workdir,
        &with_whitespace_flag(
            &[
                "diff",
                "--find-renames",
                "--no-ext-diff",
                "--no-color",
                "--unified=3",
                "--relative",
                &base.base_commit,
            ],
            ignore_whitespace,
        ),
        false,
    )?;

    let mut files = parse_unified_diff(&tracked_patch)?;
    hydrate_file_contents(workdir, &base.base_commit, &mut files)?;

    for rel_path in list_untracked_files(workdir)? {
        let patch = git_capture(
            workdir,
            &with_whitespace_flag(
                &[
                    "diff",
                    "--no-index",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--",
                    "/dev/null",
                    &rel_path,
                ],
                ignore_whitespace,
            ),
            true,
        )?;
        for mut file in parse_unified_diff(&patch)? {
            file.status = DiffFileStatus::Untracked;
            if file.old_path.is_none() {
                file.old_path = None;
            }
            if file.path.is_empty() {
                file.path = rel_path.clone();
            }
            file.old_content = None;
            file.new_content = read_worktree_file(workdir, &file.path)?;
            files.push(file);
        }
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));

    let total_additions = files.iter().map(|file| file.additions).sum();
    let total_deletions = files.iter().map(|file| file.deletions).sum();

    Ok(DiffSnapshot {
        branch: base.branch,
        base_ref: base.base_ref,
        base_commit: base.base_commit,
        files,
        total_additions,
        total_deletions,
    })
}

/// List the commits that make up the active branch diff, newest first.
pub fn list_diff_commits(workdir: &Path) -> Result<Vec<DiffCommit>> {
    let base = resolve_base_ref(workdir, None)?;
    let range = format!("{}..HEAD", base.base_commit);
    let stdout = git_capture(
        workdir,
        &["log", "--first-parent", "--format=%H%x1f%h%x1f%s", &range],
        false,
    )?;

    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.splitn(3, '\u{1f}');
            let hash = fields.next().unwrap_or_default().to_string();
            let short_hash = fields.next().unwrap_or_default().to_string();
            let subject = fields.next().unwrap_or_default().to_string();
            if hash.is_empty() || short_hash.is_empty() {
                bail!("unexpected git log entry: {line}");
            }
            Ok(DiffCommit {
                hash,
                short_hash,
                subject,
            })
        })
        .collect()
}

/// Load only the changes introduced by one commit, excluding later commits and
/// any staged, unstaged, or untracked worktree changes.
pub fn load_commit_snapshot(
    workdir: &Path,
    commit_ref: &str,
    ignore_whitespace: bool,
) -> Result<DiffSnapshot> {
    let verify_ref = format!("{}^{{commit}}", commit_ref.trim());
    let commit = git_capture(workdir, &["rev-parse", "--verify", &verify_ref], false)?
        .trim()
        .to_string();
    if commit.is_empty() {
        bail!("Commit '{commit_ref}' was not found in this repository");
    }

    let branch = WorktreeManager::current_branch(workdir)?
        .filter(|branch| !branch.is_empty())
        .unwrap_or_else(|| "(detached HEAD)".to_string());
    let subject = git_capture(workdir, &["show", "-s", "--format=%s", &commit], false)?
        .trim()
        .to_string();
    let parent = git_optional_trimmed(workdir, &["rev-parse", &format!("{commit}^1")])?;

    let patch = if let Some(parent) = &parent {
        git_capture(
            workdir,
            &with_whitespace_flag(
                &[
                    "diff",
                    "--find-renames",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--relative",
                    parent,
                    &commit,
                ],
                ignore_whitespace,
            ),
            false,
        )?
    } else {
        git_capture(
            workdir,
            &with_whitespace_flag(
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "--find-renames",
                    "--no-ext-diff",
                    "--no-color",
                    "--unified=3",
                    "--relative",
                    "-p",
                    &commit,
                ],
                ignore_whitespace,
            ),
            false,
        )?
    };

    let mut files = parse_unified_diff(&patch)?;
    hydrate_commit_file_contents(workdir, parent.as_deref(), &commit, &mut files)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let total_additions = files.iter().map(|file| file.additions).sum();
    let total_deletions = files.iter().map(|file| file.deletions).sum();

    Ok(DiffSnapshot {
        branch,
        base_ref: subject,
        base_commit: parent.unwrap_or_default(),
        files,
        total_additions,
        total_deletions,
    })
}

pub fn load_review_file(original: &Path, proposed: &Path, display_path: &str) -> Result<DiffFile> {
    let output = Command::new("git")
        .args([
            "diff",
            "--no-index",
            "--no-ext-diff",
            "--no-color",
            "--unified=3",
            "--",
        ])
        .arg(original)
        .arg(proposed)
        .output()
        .with_context(|| {
            format!(
                "failed to diff review files {} and {}",
                original.display(),
                proposed.display()
            )
        })?;

    let success = output.status.success() || output.status.code() == Some(1);
    if !success {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!(
            "git diff --no-index failed for {} and {}: {}",
            original.display(),
            proposed.display(),
            stderr
        );
    }

    let patch = String::from_utf8_lossy(&output.stdout).into_owned();
    let mut files = parse_unified_diff(&patch)?;
    let mut file = files
        .pop()
        .ok_or_else(|| anyhow!("review diff produced no file entries"))?;
    if !display_path.is_empty() {
        file.path = display_path.to_string();
        file.old_path = Some(display_path.to_string());
    }

    file.old_content = Some(
        std::fs::read_to_string(original)
            .with_context(|| format!("failed to read {}", original.display()))?,
    );
    file.new_content = Some(
        std::fs::read_to_string(proposed)
            .with_context(|| format!("failed to read {}", proposed.display()))?,
    );

    Ok(file)
}
fn hydrate_file_contents(workdir: &Path, base_commit: &str, files: &mut [DiffFile]) -> Result<()> {
    for file in files {
        if file.is_binary {
            file.old_content = None;
            file.new_content = None;
            continue;
        }

        file.old_content = match file.status {
            DiffFileStatus::Added | DiffFileStatus::Untracked => None,
            DiffFileStatus::Deleted
            | DiffFileStatus::Modified
            | DiffFileStatus::Renamed
            | DiffFileStatus::Copied
            | DiffFileStatus::TypeChanged => {
                let path = file.old_path.as_deref().unwrap_or(file.path.as_str());
                git_show_file(workdir, base_commit, path)?
            }
        };

        file.new_content = match file.status {
            DiffFileStatus::Deleted => None,
            DiffFileStatus::Added
            | DiffFileStatus::Untracked
            | DiffFileStatus::Modified
            | DiffFileStatus::Renamed
            | DiffFileStatus::Copied
            | DiffFileStatus::TypeChanged => read_worktree_file(workdir, &file.path)?,
        };
    }

    Ok(())
}

fn hydrate_commit_file_contents(
    workdir: &Path,
    parent: Option<&str>,
    commit: &str,
    files: &mut [DiffFile],
) -> Result<()> {
    for file in files {
        if file.is_binary {
            file.old_content = None;
            file.new_content = None;
            continue;
        }

        file.old_content = match file.status {
            DiffFileStatus::Added | DiffFileStatus::Untracked => None,
            DiffFileStatus::Deleted
            | DiffFileStatus::Modified
            | DiffFileStatus::Renamed
            | DiffFileStatus::Copied
            | DiffFileStatus::TypeChanged => {
                let path = file.old_path.as_deref().unwrap_or(file.path.as_str());
                match parent {
                    Some(parent) => git_show_file(workdir, parent, path)?,
                    None => None,
                }
            }
        };

        file.new_content = match file.status {
            DiffFileStatus::Deleted => None,
            DiffFileStatus::Added
            | DiffFileStatus::Untracked
            | DiffFileStatus::Modified
            | DiffFileStatus::Renamed
            | DiffFileStatus::Copied
            | DiffFileStatus::TypeChanged => git_show_file(workdir, commit, &file.path)?,
        };
    }

    Ok(())
}

pub fn resolve_base_ref(workdir: &Path, override_ref: Option<&str>) -> Result<ResolvedBase> {
    let branch = WorktreeManager::current_branch(workdir)?
        .filter(|branch| !branch.is_empty())
        .ok_or_else(|| anyhow!("{} is not on a named branch", workdir.display()))?;

    // A reviewer-chosen base ref bypasses auto-resolution: verify it exists,
    // then diff against the merge-base/fork-point so picking an ancestor commit
    // diffs directly against it and picking a divergent branch diffs from the
    // common ancestor (matching the auto-resolved semantics below).
    if let Some(reference) = override_ref.map(str::trim).filter(|r| !r.is_empty()) {
        if !git_ref_exists(workdir, reference) {
            bail!("Base ref '{reference}' was not found in this repository");
        }
        let base_commit = fork_point_or_merge_base(workdir, reference)?
            .ok_or_else(|| anyhow!("No common history between '{reference}' and HEAD"))?;
        return Ok(ResolvedBase {
            branch,
            base_ref: reference.to_string(),
            base_commit,
        });
    }

    let mut candidates = Vec::new();

    if let Some(origin_head) = git_optional_trimmed(
        workdir,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )? {
        candidates.push(origin_head);
    }

    if let Some(upstream) = git_optional_trimmed(
        workdir,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )? {
        let upstream_branch = upstream.rsplit('/').next().unwrap_or(&upstream);
        if upstream_branch != branch {
            candidates.push(upstream);
        }
    }

    candidates.extend([
        "origin/main".to_string(),
        "origin/master".to_string(),
        "main".to_string(),
        "master".to_string(),
    ]);

    candidates.retain(|candidate| candidate != &branch);
    dedupe_preserving_order(&mut candidates);

    for candidate in candidates {
        if !git_ref_exists(workdir, &candidate) {
            continue;
        }

        if let Some(base_commit) = fork_point_or_merge_base(workdir, &candidate)? {
            return Ok(ResolvedBase {
                branch: branch.clone(),
                base_ref: candidate,
                base_commit,
            });
        }
    }

    bail!(
        "Could not determine a base branch for '{}'; tried origin/HEAD, origin/main, main, origin/master, and master",
        branch
    );
}

pub fn parse_unified_diff(patch: &str) -> Result<Vec<DiffFile>> {
    let mut files = Vec::new();
    let mut section = Vec::new();

    for line in patch.lines() {
        if line.starts_with("diff --git ") && !section.is_empty() {
            files.push(parse_file_section(&section)?);
            section.clear();
        }
        section.push(line.to_string());
    }

    if !section.is_empty() {
        files.push(parse_file_section(&section)?);
    }

    Ok(files)
}

fn parse_file_section(lines: &[String]) -> Result<DiffFile> {
    let first = lines
        .first()
        .ok_or_else(|| anyhow!("diff section was empty"))?;
    if !first.starts_with("diff --git ") {
        bail!("unexpected diff header: {first}");
    }

    let (mut old_path, mut new_path) = parse_diff_git_header(first)?;
    let mut status = DiffFileStatus::Modified;
    let mut is_binary = false;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut hunks = Vec::new();
    let mut current_hunk: Option<DiffHunk> = None;

    for line in &lines[1..] {
        if line.starts_with("new file mode ") {
            status = DiffFileStatus::Added;
        } else if line.starts_with("deleted file mode ") {
            status = DiffFileStatus::Deleted;
        } else if let Some(path) = line.strip_prefix("rename from ") {
            old_path = Some(path.to_string());
            status = DiffFileStatus::Renamed;
        } else if let Some(path) = line.strip_prefix("rename to ") {
            new_path = Some(path.to_string());
            status = DiffFileStatus::Renamed;
        } else if let Some(path) = line.strip_prefix("copy from ") {
            old_path = Some(path.to_string());
            status = DiffFileStatus::Copied;
        } else if let Some(path) = line.strip_prefix("copy to ") {
            new_path = Some(path.to_string());
            status = DiffFileStatus::Copied;
        } else if line.starts_with("old mode ") || line.starts_with("new mode ") {
            if matches!(status, DiffFileStatus::Modified) {
                status = DiffFileStatus::TypeChanged;
            }
        } else if line == "GIT binary patch" || line.starts_with("Binary files ") {
            is_binary = true;
        } else if let Some(path) = line.strip_prefix("--- ") {
            old_path = normalize_patch_path(path);
        } else if let Some(path) = line.strip_prefix("+++ ") {
            new_path = normalize_patch_path(path);
        } else if line.starts_with("@@ ") {
            if let Some(hunk) = current_hunk.take() {
                hunks.push(hunk);
            }
            current_hunk = Some(parse_hunk_header(line)?);
        } else if let Some(hunk) = current_hunk.as_mut() {
            let diff_line = if line.starts_with('+') {
                additions += 1;
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: line.to_string(),
                }
            } else if line.starts_with('-') {
                deletions += 1;
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: line.to_string(),
                }
            } else if line.starts_with(' ') {
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: line.to_string(),
                }
            } else if line.starts_with('\\') {
                DiffLine {
                    kind: DiffLineKind::NoNewlineMarker,
                    text: line.to_string(),
                }
            } else {
                continue;
            };
            hunk.lines.push(diff_line);
        }
    }

    if let Some(hunk) = current_hunk.take() {
        hunks.push(hunk);
    }

    let path = match status {
        DiffFileStatus::Deleted => old_path.clone().or_else(|| new_path.clone()),
        _ => new_path.clone().or_else(|| old_path.clone()),
    }
    .ok_or_else(|| anyhow!("could not determine path for diff section '{}'", first))?;

    let mut patch = lines.join("\n");
    if !patch.is_empty() {
        patch.push('\n');
    }

    Ok(DiffFile {
        old_path,
        path,
        status,
        additions,
        deletions,
        is_binary,
        old_content: None,
        new_content: None,
        patch,
        hunks,
    })
}

fn parse_diff_git_header(line: &str) -> Result<(Option<String>, Option<String>)> {
    let rest = line
        .strip_prefix("diff --git ")
        .ok_or_else(|| anyhow!("missing diff header prefix"))?;
    let mut parts = rest.split_whitespace();
    let old_path = parts
        .next()
        .ok_or_else(|| anyhow!("missing old path in diff header"))?;
    let new_path = parts
        .next()
        .ok_or_else(|| anyhow!("missing new path in diff header"))?;
    Ok((
        normalize_patch_path(old_path),
        normalize_patch_path(new_path),
    ))
}

fn parse_hunk_header(line: &str) -> Result<DiffHunk> {
    static HUNK_RE: OnceLock<Regex> = OnceLock::new();
    let re = HUNK_RE.get_or_init(|| {
        Regex::new(r"^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").expect("valid hunk header regex")
    });

    let captures = re
        .captures(line)
        .ok_or_else(|| anyhow!("invalid hunk header: {line}"))?;

    Ok(DiffHunk {
        header: line.to_string(),
        old_start: parse_capture(&captures, 1)?,
        old_lines: parse_optional_capture(&captures, 2)?.unwrap_or(1),
        new_start: parse_capture(&captures, 3)?,
        new_lines: parse_optional_capture(&captures, 4)?.unwrap_or(1),
        lines: Vec::new(),
    })
}

fn parse_capture(captures: &regex::Captures<'_>, index: usize) -> Result<usize> {
    captures
        .get(index)
        .ok_or_else(|| anyhow!("missing regex capture {index}"))?
        .as_str()
        .parse::<usize>()
        .with_context(|| format!("failed to parse capture {index}"))
}

fn parse_optional_capture(captures: &regex::Captures<'_>, index: usize) -> Result<Option<usize>> {
    captures
        .get(index)
        .map(|m| {
            m.as_str()
                .parse::<usize>()
                .with_context(|| format!("failed to parse capture {index}"))
        })
        .transpose()
}

fn normalize_patch_path(path: &str) -> Option<String> {
    let token = path.split_whitespace().next().unwrap_or(path);
    if token == "/dev/null" {
        None
    } else if let Some(stripped) = token.strip_prefix("a/") {
        Some(stripped.to_string())
    } else if let Some(stripped) = token.strip_prefix("b/") {
        Some(stripped.to_string())
    } else if let Some(stripped) = token.strip_prefix("1/") {
        Some(stripped.to_string())
    } else if let Some(stripped) = token.strip_prefix("2/") {
        Some(stripped.to_string())
    } else if let Some(stripped) = token.strip_prefix("c/") {
        Some(stripped.to_string())
    } else if let Some(stripped) = token.strip_prefix("i/") {
        Some(stripped.to_string())
    } else if let Some(stripped) = token.strip_prefix("w/") {
        Some(stripped.to_string())
    } else {
        Some(token.to_string())
    }
}

fn list_untracked_files(workdir: &Path) -> Result<Vec<String>> {
    let stdout = git_capture(
        workdir,
        &["ls-files", "--others", "--exclude-standard"],
        false,
    )?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn fork_point_or_merge_base(workdir: &Path, candidate: &str) -> Result<Option<String>> {
    if let Some(commit) =
        git_optional_trimmed(workdir, &["merge-base", "--fork-point", candidate, "HEAD"])?
    {
        return Ok(Some(commit));
    }
    git_optional_trimmed(workdir, &["merge-base", candidate, "HEAD"])
}

fn git_ref_exists(workdir: &Path, reference: &str) -> bool {
    Command::new("git")
        .args(["rev-parse", "--verify", reference])
        .current_dir(workdir)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn git_optional_trimmed(workdir: &Path, args: &[&str]) -> Result<Option<String>> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.is_empty() {
        Ok(None)
    } else {
        Ok(Some(stdout))
    }
}

fn git_capture(workdir: &Path, args: &[&str], allow_diff_exit_code: bool) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(workdir)
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;

    let success =
        output.status.success() || (allow_diff_exit_code && output.status.code() == Some(1));
    if !success {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("git {} failed: {}", args.join(" "), stderr);
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn git_show_file(workdir: &Path, commit: &str, path: &str) -> Result<Option<String>> {
    let spec = format!("{commit}:{path}");
    let output = Command::new("git")
        .args(["show", &spec])
        .current_dir(workdir)
        .output()
        .with_context(|| format!("failed to run git show {spec}"))?;

    if output.status.success() {
        return Ok(Some(String::from_utf8_lossy(&output.stdout).into_owned()));
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("exists on disk, but not in")
        || stderr.contains("does not exist in")
        || stderr.contains("pathspec")
        || stderr.contains("bad object")
    {
        return Ok(None);
    }

    bail!("git show {spec} failed: {}", stderr.trim());
}

fn read_worktree_file(workdir: &Path, rel_path: &str) -> Result<Option<String>> {
    let path = workdir.join(rel_path);
    match std::fs::read(&path) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn dedupe_preserving_order(values: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    values.retain(|value| seen.insert(value.clone()));
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn parse_unified_diff_tracks_status_paths_and_hunks() {
        let files = parse_unified_diff(
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
        .unwrap();

        assert_eq!(files.len(), 1);
        let file = &files[0];
        assert_eq!(file.path, "src/lib.rs");
        assert_eq!(file.status, DiffFileStatus::Modified);
        assert_eq!(file.additions, 2);
        assert_eq!(file.deletions, 1);
        assert_eq!(file.hunks.len(), 1);
        assert_eq!(file.hunks[0].old_start, 1);
        assert_eq!(file.hunks[0].new_start, 1);
        assert_eq!(file.old_content, None);
        assert_eq!(file.new_content, None);
    }

    #[test]
    fn addressable_lines_number_each_commentable_diff_line() {
        let files = parse_unified_diff(
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
        .unwrap();
        let locs = files[0].addressable_lines();

        // One context, one removed, two added — markers/headers excluded.
        assert_eq!(
            locs,
            vec![
                DiffLineLocation {
                    old_line: Some(1),
                    new_line: Some(1)
                },
                DiffLineLocation {
                    old_line: Some(2),
                    new_line: None
                },
                DiffLineLocation {
                    old_line: None,
                    new_line: Some(2)
                },
                DiffLineLocation {
                    old_line: None,
                    new_line: Some(3)
                },
            ]
        );
    }

    #[test]
    fn hunk_start_indices_anchor_each_hunk_and_skip_empty_ones() {
        let make_hunk = |lines: Vec<DiffLine>| DiffHunk {
            header: "@@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines,
        };
        let file = DiffFile {
            old_path: Some("a.rs".into()),
            path: "a.rs".into(),
            status: DiffFileStatus::Modified,
            additions: 2,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![
                // Two addressable lines: indices 0 and 1.
                make_hunk(vec![
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " ctx".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "+one".into(),
                    },
                ]),
                // No addressable lines: must be skipped, not anchored.
                make_hunk(vec![DiffLine {
                    kind: DiffLineKind::NoNewlineMarker,
                    text: "\\ No newline at end of file".into(),
                }]),
                // One addressable line at index 2.
                make_hunk(vec![DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+two".into(),
                }]),
            ],
        };

        assert_eq!(file.hunk_start_indices(), vec![0, 2]);
        // Indices are valid positions in addressable_lines().
        assert_eq!(file.addressable_lines().len(), 3);
    }

    /// `l1\nl2\n…\nl{n}\n`, the base every context-expansion test edits.
    fn numbered_content(n: usize) -> String {
        (1..=n)
            .map(|i| format!("l{i}\n"))
            .collect::<Vec<_>>()
            .concat()
    }

    /// Parse `patch` as a single file and hydrate it with the blobs expansion
    /// reads from, mirroring what `hydrate_file_contents` does after a load.
    fn hydrated(patch: &str, old_content: String, new_content: String) -> DiffFile {
        let mut file = parse_unified_diff(patch).unwrap().pop().unwrap();
        file.old_content = Some(old_content);
        file.new_content = Some(new_content);
        file
    }

    /// A 30-line file with line 15 rewritten, at git's default `--unified=3`.
    fn one_change_in_thirty_lines() -> DiffFile {
        let old = numbered_content(30);
        let new = old.replace("l15\n", "l15 changed\n");
        hydrated(
            "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -12,7 +12,7 @@
 l12
 l13
 l14
-l15
+l15 changed
 l16
 l17
 l18
",
            old,
            new,
        )
    }

    /// A 30-line file with lines 10 and 18 rewritten — two hunks at the default
    /// context, close enough to merge once it widens.
    fn two_changes_in_thirty_lines() -> DiffFile {
        let old = numbered_content(30);
        let new = old
            .replace("l10\n", "l10 changed\n")
            .replace("l18\n", "l18 changed\n");
        hydrated(
            "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -7,7 +7,7 @@
 l7
 l8
 l9
-l10
+l10 changed
 l11
 l12
 l13
@@ -15,7 +15,7 @@
 l15
 l16
 l17
-l18
+l18 changed
 l19
 l20
 l21
",
            old,
            new,
        )
    }

    fn line_texts(hunks: &[DiffHunk]) -> Vec<String> {
        hunks
            .iter()
            .flat_map(|hunk| hunk.lines.iter().map(|line| line.text.clone()))
            .collect()
    }

    #[test]
    fn hunks_with_context_widens_a_hunk_using_the_blobs() {
        let file = one_change_in_thirty_lines();
        let hunks = file.hunks_with_context(10).unwrap();

        // 10 lines each side of the single changed line, clamped by neither
        // file end: lines 5..=25 on both sides.
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 5);
        assert_eq!(hunks[0].old_lines, 21);
        assert_eq!(hunks[0].new_start, 5);
        assert_eq!(hunks[0].new_lines, 21);
        assert_eq!(hunks[0].header, "@@ -5,21 +5,21 @@");

        let texts = line_texts(&hunks);
        assert_eq!(texts.first().unwrap(), " l5");
        assert_eq!(texts.last().unwrap(), " l25");
        // The change itself is preserved verbatim, not re-derived.
        assert!(texts.contains(&"-l15".to_string()));
        assert!(texts.contains(&"+l15 changed".to_string()));
    }

    #[test]
    fn hunks_with_context_merges_hunks_whose_context_regions_meet() {
        let file = two_changes_in_thirty_lines();
        assert_eq!(file.hunks.len(), 2);

        // At 10 lines of context the two regions overlap and become one hunk
        // covering both changes plus everything between them.
        let hunks = file.hunks_with_context(10).unwrap();
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_lines, 28);
        let texts = line_texts(&hunks);
        assert!(texts.contains(&"-l10".to_string()));
        assert!(texts.contains(&"-l18".to_string()));
        // The line that separated the two original hunks is now context.
        assert!(texts.contains(&" l14".to_string()));
    }

    #[test]
    fn hunks_with_context_whole_file_covers_every_line() {
        let file = one_change_in_thirty_lines();
        let hunks = file.hunks_with_context(usize::MAX).unwrap();

        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_lines, 30);
        assert_eq!(hunks[0].new_lines, 30);
        let texts = line_texts(&hunks);
        assert_eq!(texts.first().unwrap(), " l1");
        assert_eq!(texts.last().unwrap(), " l30");
    }

    #[test]
    fn hunks_with_context_round_trips_back_to_the_parsed_hunks() {
        // Idempotence is what lets the viewer step the level up and down
        // without keeping a pristine copy of the parsed hunks.
        for mut file in [one_change_in_thirty_lines(), two_changes_in_thirty_lines()] {
            let original = file.hunks.clone();
            file.hunks = file.hunks_with_context(usize::MAX).unwrap();
            file.hunks = file.hunks_with_context(25).unwrap();
            file.hunks = file.hunks_with_context(DIFF_DEFAULT_CONTEXT).unwrap();
            assert_eq!(file.hunks, original);
        }
    }

    #[test]
    fn hunks_with_context_keeps_comment_anchors_addressable() {
        // Line comments anchor to `DiffLineLocation`s; expansion must leave
        // every one of them present (it only ever adds context lines).
        let mut file = one_change_in_thirty_lines();
        let before = file.addressable_lines();
        file.hunks = file.hunks_with_context(10).unwrap();
        let after = file.addressable_lines();

        assert!(after.len() > before.len());
        for loc in &before {
            assert!(after.contains(loc), "expansion dropped {loc:?}");
        }
    }

    #[test]
    fn hunks_with_context_refuses_when_the_blobs_no_longer_match() {
        let mut file = one_change_in_thirty_lines();
        // The worktree moved on underneath the parsed patch.
        file.new_content = Some(numbered_content(30).replace("l15\n", "l15 rewritten again\n"));

        assert_eq!(file.hunks_with_context(10), None);
    }

    #[test]
    fn hunks_with_context_is_unavailable_without_both_blobs() {
        let mut file = one_change_in_thirty_lines();

        file.old_content = None;
        assert!(!file.can_expand_context());
        assert_eq!(file.hunks_with_context(10), None);

        let mut binary = one_change_in_thirty_lines();
        binary.is_binary = true;
        assert!(!binary.can_expand_context());
        assert_eq!(binary.hunks_with_context(10), None);
    }

    #[test]
    fn hunks_with_context_clamps_at_file_boundaries() {
        // A change on the first line has no room above it.
        let old = numbered_content(6);
        let new = old.replace("l1\n", "l1 changed\n");
        let file = hydrated(
            "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,4 @@
-l1
+l1 changed
 l2
 l3
 l4
",
            old,
            new,
        );

        let hunks = file.hunks_with_context(20).unwrap();
        assert_eq!(hunks[0].old_start, 1);
        assert_eq!(hunks[0].old_lines, 6);
        assert_eq!(line_texts(&hunks).last().unwrap(), " l6");
    }

    #[test]
    fn hunks_with_context_re_emits_the_missing_newline_marker() {
        // The marker belongs to whichever side's final line lands in a hunk,
        // so it is re-derived from the blobs rather than carried over.
        let old = "l1\nl2\nl3\nl4\nl5";
        let new = "l1\nl2 changed\nl3\nl4\nl5";
        let file = hydrated(
            "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,4 @@
 l1
-l2
+l2 changed
 l3
 l4
",
            old.to_string(),
            new.to_string(),
        );

        let hunks = file.hunks_with_context(20).unwrap();
        let kinds: Vec<_> = hunks[0]
            .lines
            .iter()
            .map(|line| line.kind.clone())
            .collect();
        assert_eq!(kinds.last(), Some(&DiffLineKind::NoNewlineMarker));
        // The marker isn't addressable, so it can't shift a comment anchor.
        assert_eq!(hunks[0].lines.len(), hunks[0].old_lines + 2);
    }

    #[test]
    fn load_snapshot_hydrates_old_and_new_file_contents() {
        let repo = init_repo_with_main();
        std::fs::write(repo.path().join("src.txt"), "base\nline two\n").unwrap();
        git(repo.path(), &["add", "src.txt"]);
        git(repo.path(), &["commit", "-m", "add src"]);
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nline changed\n").unwrap();

        let snapshot = load_snapshot(repo.path(), None, false).unwrap();
        let file = snapshot
            .files
            .iter()
            .find(|file| file.path == "src.txt")
            .unwrap();

        assert_eq!(file.old_content.as_deref(), Some("base\nline two\n"));
        assert_eq!(file.new_content.as_deref(), Some("base\nline changed\n"));
    }

    #[test]
    fn resolve_base_ref_falls_back_to_local_main() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\n").unwrap();
        git(repo.path(), &["commit", "-am", "feature change"]);

        let base = resolve_base_ref(repo.path(), None).unwrap();

        assert_eq!(base.branch, "feature");
        assert_eq!(base.base_ref, "main");
        assert_eq!(base.base_commit, rev_parse(repo.path(), "main"));
    }

    #[test]
    fn resolve_base_ref_honors_override() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\n").unwrap();
        git(repo.path(), &["commit", "-am", "feature change"]);
        // A second commit so HEAD differs from the override target.
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\nmore\n").unwrap();
        git(repo.path(), &["commit", "-am", "more"]);

        let target = rev_parse(repo.path(), "HEAD~1");
        let base = resolve_base_ref(repo.path(), Some(&target)).unwrap();

        // The override is used verbatim as the base ref, and since it is an
        // ancestor of HEAD the merge-base is the commit itself.
        assert_eq!(base.base_ref, target);
        assert_eq!(base.base_commit, target);
    }

    #[test]
    fn resolve_base_ref_rejects_unknown_override() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);

        let err = resolve_base_ref(repo.path(), Some("does-not-exist")).unwrap_err();
        assert!(err.to_string().contains("does-not-exist"));
    }

    #[test]
    fn resolve_base_ref_blank_override_falls_back_to_auto() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\n").unwrap();
        git(repo.path(), &["commit", "-am", "feature change"]);

        let base = resolve_base_ref(repo.path(), Some("   ")).unwrap();
        assert_eq!(base.base_ref, "main");
    }

    #[test]
    fn resolve_base_ref_prefers_origin_head_when_available() {
        let remote = TempDir::new().unwrap();
        git(remote.path(), &["init", "--bare", "--initial-branch=main"]);

        let repo = init_repo_with_main();
        git(
            repo.path(),
            &["remote", "add", "origin", remote.path().to_str().unwrap()],
        );
        git(repo.path(), &["push", "-u", "origin", "main"]);
        git(repo.path(), &["remote", "set-head", "origin", "-a"]);
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\n").unwrap();
        git(repo.path(), &["commit", "-am", "feature change"]);

        let base = resolve_base_ref(repo.path(), None).unwrap();

        assert_eq!(base.base_ref, "origin/main");
        assert_eq!(base.base_commit, rev_parse(repo.path(), "origin/main"));
    }

    #[test]
    fn load_snapshot_includes_tracked_and_untracked_changes() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\n").unwrap();
        std::fs::write(repo.path().join("notes.md"), "todo\n").unwrap();

        let snapshot = load_snapshot(repo.path(), None, false).unwrap();

        assert_eq!(snapshot.branch, "feature");
        assert_eq!(snapshot.base_ref, "main");
        assert_eq!(snapshot.files.len(), 2);
        assert_eq!(snapshot.files[0].path, "notes.md");
        assert_eq!(snapshot.files[0].status, DiffFileStatus::Untracked);
        assert_eq!(snapshot.files[1].path, "src.txt");
        assert_eq!(snapshot.files[1].status, DiffFileStatus::Modified);
        assert!(snapshot.total_additions >= 2);
        assert_eq!(snapshot.total_deletions, 0);
    }

    #[test]
    fn list_diff_commits_returns_only_feature_commits_newest_first() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\none\n").unwrap();
        git(repo.path(), &["commit", "-am", "first feature commit"]);
        std::fs::write(repo.path().join("src.txt"), "base\none\ntwo\n").unwrap();
        git(repo.path(), &["commit", "-am", "second feature commit"]);

        let commits = list_diff_commits(repo.path()).unwrap();

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].subject, "second feature commit");
        assert_eq!(commits[1].subject, "first feature commit");
        assert_eq!(commits[0].hash, rev_parse(repo.path(), "HEAD"));
    }

    #[test]
    fn load_commit_snapshot_excludes_other_commits_and_worktree_changes() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfirst\n").unwrap();
        git(repo.path(), &["commit", "-am", "first feature commit"]);
        let first = rev_parse(repo.path(), "HEAD");

        std::fs::write(repo.path().join("src.txt"), "base\nfirst\nsecond\n").unwrap();
        git(repo.path(), &["commit", "-am", "second feature commit"]);
        std::fs::write(
            repo.path().join("src.txt"),
            "base\nfirst\nsecond\nuncommitted\n",
        )
        .unwrap();
        std::fs::write(repo.path().join("untracked.txt"), "not part of commit\n").unwrap();

        let snapshot = load_commit_snapshot(repo.path(), &first, false).unwrap();
        let file = snapshot
            .files
            .iter()
            .find(|file| file.path == "src.txt")
            .unwrap();

        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.base_ref, "first feature commit");
        assert_eq!(file.old_content.as_deref(), Some("base\n"));
        assert_eq!(file.new_content.as_deref(), Some("base\nfirst\n"));
        assert!(file.patch.contains("+first"));
        assert!(!file.patch.contains("second"));
        assert!(!file.patch.contains("uncommitted"));
    }

    fn init_repo_with_main() -> TempDir {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "--initial-branch=main"]);
        git(repo.path(), &["config", "user.name", "AMF Test"]);
        git(repo.path(), &["config", "user.email", "amf@example.com"]);
        std::fs::write(repo.path().join("src.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "src.txt"]);
        git(repo.path(), &["commit", "-m", "initial"]);
        repo
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn rev_parse(repo: &Path, rev: &str) -> String {
        let output = Command::new("git")
            .args(["rev-parse", rev])
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}
