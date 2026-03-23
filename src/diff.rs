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

pub fn load_snapshot(workdir: &Path) -> Result<DiffSnapshot> {
    let base = resolve_base_ref(workdir)?;
    let tracked_patch = git_capture(
        workdir,
        &[
            "diff",
            "--find-renames",
            "--no-ext-diff",
            "--no-color",
            "--unified=3",
            "--relative",
            &base.base_commit,
        ],
        false,
    )?;

    let mut files = parse_unified_diff(&tracked_patch)?;
    hydrate_file_contents(workdir, &base.base_commit, &mut files)?;

    for rel_path in list_untracked_files(workdir)? {
        let patch = git_capture(
            workdir,
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

pub fn resolve_base_ref(workdir: &Path) -> Result<ResolvedBase> {
    let branch = WorktreeManager::current_branch(workdir)?
        .filter(|branch| !branch.is_empty())
        .ok_or_else(|| anyhow!("{} is not on a named branch", workdir.display()))?;

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
    fn load_snapshot_hydrates_old_and_new_file_contents() {
        let repo = init_repo_with_main();
        std::fs::write(repo.path().join("src.txt"), "base\nline two\n").unwrap();
        git(repo.path(), &["add", "src.txt"]);
        git(repo.path(), &["commit", "-m", "add src"]);
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nline changed\n").unwrap();

        let snapshot = load_snapshot(repo.path()).unwrap();
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

        let base = resolve_base_ref(repo.path()).unwrap();

        assert_eq!(base.branch, "feature");
        assert_eq!(base.base_ref, "main");
        assert_eq!(base.base_commit, rev_parse(repo.path(), "main"));
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

        let base = resolve_base_ref(repo.path()).unwrap();

        assert_eq!(base.base_ref, "origin/main");
        assert_eq!(base.base_commit, rev_parse(repo.path(), "origin/main"));
    }

    #[test]
    fn load_snapshot_includes_tracked_and_untracked_changes() {
        let repo = init_repo_with_main();
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\n").unwrap();
        std::fs::write(repo.path().join("notes.md"), "todo\n").unwrap();

        let snapshot = load_snapshot(repo.path()).unwrap();

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
