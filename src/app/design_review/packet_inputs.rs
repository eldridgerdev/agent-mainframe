//! What a Design Review packet is built from, gathered off the UI thread.
//!
//! [`gather`] shells out to git (diff snapshot, commit subjects, the base's
//! directory listing) and reads `AMF_PLAN.md`, so it runs on a background
//! thread; everything else here is pure.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use crate::diff::DiffFile;
use crate::headless::HeadlessUsage;
use crate::project::AgentKind;
use crate::token_cost::cost::TokenCounts;
use crate::token_cost::ledger::{Provenance, estimate_tokens_from_bytes};

/// Bytes of `AMF_PLAN.md` read for the plan-drift excerpt. The context
/// builder budgets it again; this only bounds the read.
pub(crate) const PLAN_EXCERPT_MAX_BYTES: usize = 48 * 1024;

/// Everything git and the worktree contribute to a packet.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GatheredInputs {
    pub files: Vec<DiffFile>,
    pub commit_subjects: Vec<String>,
    pub plan_excerpt: Option<String>,
    /// Directories at the base commit, at the root and under `src/`.
    pub base_dirs: Vec<String>,
}

/// Load the branch diff against its resolved base, the commit subjects since
/// the base, the base's directories and the plan excerpt. Blocking: run it on
/// a worker thread.
pub(crate) fn gather(workdir: &Path) -> Result<GatheredInputs, String> {
    let snapshot = crate::diff::load_snapshot(workdir, None, false)
        .map_err(|err| format!("cannot load the diff: {err}"))?;
    let base = crate::diff::ResolvedBase {
        branch: snapshot.branch.clone(),
        base_ref: snapshot.base_ref.clone(),
        base_commit: snapshot.base_commit.clone(),
        no_base_branch: snapshot.no_base_branch,
    };
    let commit_subjects = if snapshot.no_base_branch {
        Vec::new()
    } else {
        crate::diff::list_diff_commits_since(workdir, &base)
            .map_err(|err| format!("cannot list commits: {err}"))?
            .into_iter()
            .map(|commit| commit.subject)
            .collect()
    };
    Ok(GatheredInputs {
        base_dirs: base_dirs(workdir, &snapshot.base_commit),
        files: snapshot.files,
        commit_subjects,
        plan_excerpt: read_plan_excerpt(workdir),
    })
}

/// `git ls-tree -d --name-only <base>` at the root and under `src/`. A failed
/// listing contributes nothing, which only makes the new-module detector
/// more eager.
fn base_dirs(workdir: &Path, base_commit: &str) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for extra in [None, Some("src/")] {
        let mut command = Command::new("git");
        command
            .arg("-C")
            .arg(workdir)
            .args(["ls-tree", "-d", "--name-only", "-z", base_commit]);
        if let Some(path) = extra {
            command.arg("--").arg(path);
        }
        let Ok(output) = command.output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        for name in output.stdout.split(|b| *b == 0) {
            let name = String::from_utf8_lossy(name);
            let name = name.trim();
            if !name.is_empty() {
                dirs.insert(name.to_owned());
            }
        }
    }
    dirs.into_iter().collect()
}

/// The start of `<workdir>/AMF_PLAN.md`, cut on a char boundary; `None` when
/// the file is absent, unreadable or blank.
pub(crate) fn read_plan_excerpt(workdir: &Path) -> Option<String> {
    let bytes = std::fs::read(workdir.join("AMF_PLAN.md")).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    let mut end = text.len().min(PLAN_EXCERPT_MAX_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let excerpt = text[..end].trim();
    (!excerpt.is_empty()).then(|| excerpt.to_owned())
}

/// The directory of each changed path (`.` for the root), deduplicated and
/// sorted.
pub(crate) fn changed_modules(files: &[DiffFile]) -> Vec<String> {
    files
        .iter()
        .map(|file| match file.path.rsplit_once('/') {
            Some((dir, _)) => dir.to_owned(),
            None => ".".to_owned(),
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Ledger counts for one staff run. Reported usage is `measured`; Claude's
/// `input_tokens` already excludes cache reads, Codex's includes them, so
/// Codex's cached share is taken out of input. With nothing reported the
/// run is `estimated` at bytes/4 of the prompt and the reply.
pub(crate) fn counts_for_run(
    harness: &AgentKind,
    usage: Option<&HeadlessUsage>,
    prompt: &str,
    reply: &str,
) -> (TokenCounts, Provenance) {
    if let Some(usage) = usage.filter(|u| u.input_tokens.is_some() || u.output_tokens.is_some()) {
        let input = match (harness, usage.input_tokens, usage.cached_tokens) {
            (AgentKind::Codex, Some(input), Some(cached)) => Some(input.saturating_sub(cached)),
            (_, input, _) => input,
        };
        let counts = TokenCounts {
            input,
            output: usage.output_tokens,
            cache_read: usage.cached_tokens,
            ..Default::default()
        };
        return (counts, Provenance::Measured);
    }
    let counts = TokenCounts {
        input: Some(estimate_tokens_from_bytes(prompt.len())),
        output: Some(estimate_tokens_from_bytes(reply.len())),
        ..Default::default()
    };
    (counts, Provenance::Estimated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::DiffFileStatus;

    fn file(path: &str) -> DiffFile {
        DiffFile {
            old_path: None,
            path: path.to_owned(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: Vec::new(),
        }
    }

    #[test]
    fn changed_modules_are_parent_directories() {
        let files = [
            file("src/app/a.rs"),
            file("src/app/b.rs"),
            file("Cargo.toml"),
            file("src/db/x.rs"),
        ];
        assert_eq!(changed_modules(&files), [".", "src/app", "src/db"]);
    }

    #[test]
    fn reported_usage_is_measured_and_codex_cache_is_not_counted_twice() {
        let usage = HeadlessUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(50),
            cached_tokens: Some(400),
            total_tokens: None,
        };
        let (claude, prov) = counts_for_run(&AgentKind::Claude, Some(&usage), "p", "r");
        assert_eq!(prov, Provenance::Measured);
        assert_eq!(claude.input, Some(1_000));
        assert_eq!(claude.cache_read, Some(400));
        let (codex, _) = counts_for_run(&AgentKind::Codex, Some(&usage), "p", "r");
        assert_eq!(codex.input, Some(600));
    }

    #[test]
    fn missing_usage_is_estimated_from_bytes() {
        let (counts, prov) = counts_for_run(
            &AgentKind::Claude,
            Some(&HeadlessUsage::default()),
            "abcdefgh",
            "abc",
        );
        assert_eq!(prov, Provenance::Estimated);
        assert_eq!(counts.input, Some(2));
        assert_eq!(counts.output, Some(1));
        assert_eq!(counts.cache_read, None);
    }

    #[test]
    fn plan_excerpt_is_bounded_and_absent_when_blank() {
        let dir = tempfile::TempDir::new().unwrap();
        assert_eq!(read_plan_excerpt(dir.path()), None);
        std::fs::write(dir.path().join("AMF_PLAN.md"), "  \n").unwrap();
        assert_eq!(read_plan_excerpt(dir.path()), None);
        std::fs::write(
            dir.path().join("AMF_PLAN.md"),
            "é".repeat(PLAN_EXCERPT_MAX_BYTES),
        )
        .unwrap();
        let excerpt = read_plan_excerpt(dir.path()).unwrap();
        assert!(excerpt.len() <= PLAN_EXCERPT_MAX_BYTES);
    }
}
