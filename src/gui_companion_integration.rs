//! Explicit, revision-bound integration of Final Review companion commits.
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::project::{ReviewSource, TriageSource};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Binding {
    project_id: String,
    companion_id: String,
    companion_created_at: chrono::DateTime<chrono::Utc>,
    companion_name: String,
    companion_branch: String,
    companion_workdir: PathBuf,
    source_id: String,
    source_created_at: chrono::DateTime<chrono::Utc>,
    source_name: String,
    source_branch: String,
    source_workdir: PathBuf,
    link: Link,
}

/// The persisted relationship that makes a feature an integration companion.
/// A Final Review companion lands on its source feature's own branch; a PR
/// Triage companion lands on the PR's recorded head branch, while its
/// cherry-pick still targets whatever the triage source checkout has.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Link {
    Review(ReviewSource),
    Triage(TriageSource),
}

impl Link {
    fn source_feature_id(&self) -> &str {
        match self {
            Link::Review(l) => &l.source_feature_id,
            Link::Triage(l) => &l.source_feature_id,
        }
    }
    fn base_sha(&self) -> &str {
        match self {
            Link::Review(l) => &l.base_sha,
            Link::Triage(l) => &l.base_sha,
        }
    }
    /// The remote branch a push updates.
    fn destination_branch(&self) -> &str {
        match self {
            Link::Review(l) => &l.target_branch,
            Link::Triage(l) => &l.pr_branch,
        }
    }
    fn pr_number(&self) -> Option<u32> {
        match self {
            Link::Review(_) => None,
            Link::Triage(l) => Some(l.pr_number),
        }
    }
}

pub fn bind(gui: &mut GuiHandle, target: &FeatureTarget) -> GuiResult<Binding> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Companion feature was deleted or moved"))?;
    let project = &app.store.projects[pi];
    let companion = &project.features[fi];
    let link = match (&companion.review_source, &companion.triage_source) {
        (Some(l), None) => Link::Review(l.clone()),
        (None, Some(l)) => Link::Triage(l.clone()),
        (Some(_), Some(_)) => {
            return Err(GuiError::conflict(
                "This feature carries both Final Review and PR Triage links; refusing to guess",
            ));
        }
        (None, None) => {
            return Err(GuiError::conflict(
                "This feature is not a Final Review or PR Triage companion",
            ));
        }
    };
    let source = project
        .features
        .iter()
        .find(|f| f.id == link.source_feature_id())
        .ok_or_else(|| {
            GuiError::conflict("The linked source feature no longer exists in this project")
        })?;
    if source.id == companion.id {
        return Err(GuiError::conflict("Companion and source must differ"));
    }
    Ok(Binding {
        project_id: project.id.clone(),
        companion_id: companion.id.clone(),
        companion_created_at: companion.created_at,
        companion_name: companion.name.clone(),
        companion_branch: companion.branch.clone(),
        companion_workdir: companion.workdir.clone(),
        source_id: source.id.clone(),
        source_created_at: source.created_at,
        source_name: source.name.clone(),
        source_branch: source.branch.clone(),
        source_workdir: source.workdir.clone(),
        link,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationChoice {
    CherryPick,
    Push,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntegrationPlan {
    binding: Binding,
    pub choice: IntegrationChoice,
    pub revision: String,
    pub companion_name: String,
    pub source_name: String,
    pub companion_path: PathBuf,
    pub source_path: PathBuf,
    pub companion_branch: String,
    pub source_branch: String,
    /// Remote branch a push would update (the PR head branch for PR Triage).
    pub destination_branch: String,
    pub pr_number: Option<u32>,
    pub base_sha: String,
    pub companion_head: String,
    pub source_head: String,
    pub commits: Vec<String>,
    pub source_dirty: bool,
    pub companion_dirty: bool,
    pub remote_tip: Option<String>,
    pub remote_display: Option<String>,
    pub ignored_collisions: Vec<String>,
    pub hidden_tracked: Vec<String>,
    #[serde(skip_serializing)]
    remote: Option<String>,
    companion_identity: CheckoutIdentity,
    source_identity: CheckoutIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct CheckoutIdentity {
    directory: (PathBuf, u64, u64),
    gitdir: (PathBuf, u64, u64),
    common: (PathBuf, u64, u64),
}
fn directory(path: &Path) -> Result<(PathBuf, u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    let p = path.canonicalize()?;
    let m = p.metadata()?;
    ensure!(m.is_dir(), "Checkout identity is not a directory");
    Ok((p, m.dev(), m.ino()))
}
fn identity(path: &Path) -> Result<CheckoutIdentity> {
    let gitdir = git(path, &["rev-parse", "--absolute-git-dir"])?;
    let common = git(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Ok(CheckoutIdentity {
        directory: directory(path)?,
        gitdir: directory(Path::new(gitdir.trim()))?,
        common: directory(Path::new(common.trim()))?,
    })
}

// Own and reap the entire Git process group. Files avoid pipe deadlocks; output
// is bounded when read. A timed-out write may have completed remotely.
fn git(path: &Path, args: &[&str]) -> Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    use std::os::unix::process::CommandExt;
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .process_group(0)
        .spawn()
        .context("Could not start Git")?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(45) => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                bail!(
                    "Git did not finish; inspect the destination before retrying because its outcome may be unknown"
                );
            }
        }
    };
    stdout.seek(SeekFrom::Start(0))?;
    stderr.seek(SeekFrom::Start(0))?;
    let mut out = Vec::new();
    let mut err = Vec::new();
    stdout.take(1_048_577).read_to_end(&mut out)?;
    stderr.take(65_537).read_to_end(&mut err)?;
    ensure!(
        out.len() <= 1_048_576 && err.len() <= 65_536,
        "Git output exceeded its limit"
    );
    if !status.success() {
        if matches!(args.first(), Some(&"push") | Some(&"ls-remote")) {
            bail!(
                "Git {} failed; check the remote and its permissions",
                args[0]
            );
        }
        bail!(
            "Git {} failed: {}",
            args.first().unwrap_or(&"command"),
            String::from_utf8_lossy(&err).trim()
        );
    }
    String::from_utf8(out).context("Git output is not UTF-8")
}
fn value(path: &Path, args: &[&str]) -> Result<String> {
    Ok(git(path, args)?.trim().to_owned())
}
fn sha(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}
fn commit(path: &Path, reference: &str) -> Result<String> {
    let result = value(
        path,
        &["rev-parse", "--verify", &format!("{reference}^{{commit}}")],
    )?;
    ensure!(sha(&result), "Git returned an invalid commit ID");
    Ok(result)
}
fn clean(path: &Path) -> Result<bool> {
    Ok(git(
        path,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?
    .is_empty())
}
fn ancestor(path: &Path, parent: &str, child: &str) -> Result<()> {
    git(path, &["merge-base", "--is-ancestor", parent, child])
        .map(|_| ())
        .context("The destination is not an ancestor of the companion commit")
}
fn operation_clear(path: &Path) -> Result<()> {
    for name in [
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "MERGE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "sequencer",
    ] {
        let p = value(path, &["rev-parse", "--git-path", name])?;
        let p = Path::new(&p);
        let p = if p.is_absolute() {
            p.to_path_buf()
        } else {
            path.join(p)
        };
        ensure!(
            !p.exists(),
            "A Git operation is already in progress; resolve it first"
        );
    }
    Ok(())
}
fn remote_tip(path: &Path, remote: &str, reference: &str) -> Result<Option<String>> {
    let line = git(path, &["ls-remote", "--refs", remote, reference])?;
    let parts: Vec<_> = line.split_whitespace().collect();
    if parts.is_empty() {
        return Ok(None);
    }
    ensure!(
        parts.len() == 2 && parts[1] == reference && sha(parts[0]),
        "Remote returned an invalid branch identity"
    );
    Ok(Some(parts[0].into()))
}

// Every path touched by every pinned commit, not just the net diff: Git may
// overwrite ignored or hidden files when applying an intermediate commit.
fn incoming_paths(companion: &Path, commits: &[String]) -> Result<Vec<String>> {
    let mut paths = std::collections::BTreeSet::new();
    for commit in commits {
        let changed = git(
            companion,
            &[
                "diff-tree",
                "--root",
                "-m",
                "--no-renames",
                "--no-commit-id",
                "--name-only",
                "-r",
                "-z",
                commit,
            ],
        )?;
        for path in changed.split('\0').filter(|p| !p.is_empty()) {
            ensure!(
                !path.starts_with('/') && !path.split('/').any(|part| part == ".."),
                "Git returned an unsafe path"
            );
            paths.insert(format!(":(literal){path}"));
        }
    }
    Ok(paths.into_iter().collect())
}

fn ignored_collisions(source: &Path, paths: &[String]) -> Result<Vec<String>> {
    let mut collisions = std::collections::BTreeSet::new();
    for batch in paths.chunks(128) {
        let mut args = vec![
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "-z",
            "--",
        ];
        args.extend(batch.iter().map(String::as_str));
        let found = git(source, &args)?;
        collisions.extend(
            found
                .split('\0')
                .filter(|p| !p.is_empty())
                .map(str::to_owned),
        );
    }
    Ok(collisions.into_iter().collect())
}

// Tracked paths flagged assume-unchanged (lowercase tag) or skip-worktree
// ('S') are invisible to `git status`, so local edits there would look clean.
// Scoped to incoming paths; unparseable output fails closed.
fn hidden_tracked(source: &Path, paths: &[String]) -> Result<Vec<String>> {
    let mut hidden = std::collections::BTreeSet::new();
    for batch in paths.chunks(128) {
        let mut args = vec!["ls-files", "-v", "-z", "--"];
        args.extend(batch.iter().map(String::as_str));
        let found = git(source, &args)?;
        for entry in found.split('\0').filter(|p| !p.is_empty()) {
            let (tag, path) = entry
                .split_once(' ')
                .context("Git returned an unreadable tracked-file listing")?;
            ensure!(tag.len() == 1, "Git returned an unreadable file tag");
            let tag = tag.as_bytes()[0];
            if tag == b'S' || tag.is_ascii_lowercase() {
                hidden.insert(path.to_owned());
            }
        }
    }
    Ok(hidden.into_iter().collect())
}

fn display_remote(remote: &str) -> String {
    if let Ok(mut url) = url::Url::parse(remote) {
        let _ = url.set_username("");
        let _ = url.set_password(None);
        url.set_query(None);
        url.set_fragment(None);
        return url.to_string();
    }
    if let Some((_, rest)) = remote.rsplit_once('@') {
        return format!("[credentials omitted]@{rest}");
    }
    remote.to_owned()
}

pub fn plan(binding: Binding, choice: IntegrationChoice) -> Result<IntegrationPlan> {
    let c = &binding.companion_workdir;
    let s = &binding.source_workdir;
    let ci = identity(c)?;
    let si = identity(s)?;
    ensure!(
        ci.directory != si.directory,
        "Source and companion resolve to the same checkout"
    );
    ensure!(
        ci.common == si.common,
        "Source and companion no longer belong to the same repository"
    );
    let cb = value(c, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    let sb = value(s, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
    ensure!(
        cb == binding.companion_branch
            && sb == binding.source_branch
            && (matches!(binding.link, Link::Triage(_)) || sb == binding.link.destination_branch()),
        "A checkout branch changed; reopen the integration preview"
    );
    ensure!(
        sha(binding.link.base_sha()),
        "The recorded base is not a valid commit ID"
    );
    let base = commit(c, binding.link.base_sha())?;
    ensure!(
        base == binding.link.base_sha(),
        "The recorded base is not an exact commit ID"
    );
    let head = commit(c, "HEAD")?;
    let source_head = commit(s, "HEAD")?;
    ancestor(c, &base, &head)?;
    operation_clear(s)?;
    operation_clear(c)?;
    let range = format!("{base}..{head}");
    ensure!(
        git(c, &["rev-list", "--min-parents=2", "--count", &range])?.trim() == "0",
        "The companion contains merge commits, which cannot be cherry-picked or previewed exactly; rebase it onto linear history first"
    );
    let listed = git(c, &["rev-list", "--reverse", &range])?;
    let commits: Vec<String> = listed.lines().map(str::to_owned).collect();
    ensure!(
        !commits.is_empty() && commits.iter().all(|x| sha(x)),
        "The companion has no valid commits to integrate"
    );
    let source_dirty = !clean(s)?;
    let companion_dirty = !clean(c)?;
    let (ignored_collisions, hidden_tracked) = if choice == IntegrationChoice::CherryPick {
        let paths = incoming_paths(c, &commits)?;
        (ignored_collisions(s, &paths)?, hidden_tracked(s, &paths)?)
    } else {
        (Vec::new(), Vec::new())
    };
    let (remote, tip) = if choice == IntegrationChoice::Push {
        let reference = format!("refs/heads/{}", binding.link.destination_branch());
        git(c, &["check-ref-format", &reference])?;
        let configured = value(c, &["remote", "get-url", "--push", "origin"])?;
        // Resolve local relative paths now: a later Git config change cannot
        // redirect an argument that happens to name another remote.
        let remote = if !configured.contains(':') && !Path::new(&configured).is_absolute() {
            c.join(&configured)
                .canonicalize()?
                .to_string_lossy()
                .into_owned()
        } else {
            configured
        };
        ensure!(
            !remote.is_empty() && !remote.starts_with('-'),
            "The origin remote is invalid"
        );
        let tip = remote_tip(c, &remote, &reference)?;
        // A PR head that is absent from origin usually lives on a fork or was
        // deleted; creating it would silently retarget the work.
        ensure!(
            tip.is_some() || !matches!(binding.link, Link::Triage(_)),
            "The PR branch does not exist on origin; it may belong to a fork or have been deleted"
        );
        if let Some(tip) = &tip {
            ensure!(
                tip != &head,
                "The remote branch already contains the companion head"
            );
            ancestor(c, tip, &head)?;
            // The push sends tip..head; the preview lists base..head. They
            // are the same set only when the remote tip is not ahead of base.
            ensure!(
                ancestor(c, tip, &base).is_ok(),
                "The remote branch differs from the companion's base, so the push would publish commits not shown in the preview; update the source checkout and re-create the companion"
            );
        }
        (Some(remote), tip)
    } else {
        (None, None)
    };
    let mut plan = IntegrationPlan {
        companion_name: binding.companion_name.clone(),
        source_name: binding.source_name.clone(),
        companion_path: c.clone(),
        source_path: s.clone(),
        companion_branch: cb,
        source_branch: sb,
        destination_branch: binding.link.destination_branch().to_owned(),
        pr_number: binding.link.pr_number(),
        base_sha: base,
        companion_head: head,
        source_head,
        commits,
        source_dirty,
        companion_dirty,
        remote_tip: tip,
        remote_display: remote.as_deref().map(display_remote),
        ignored_collisions,
        hidden_tracked,
        remote,
        companion_identity: ci,
        source_identity: si,
        binding,
        choice,
        revision: String::new(),
    };
    plan.revision = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(&plan, &plan.remote))?)
    );
    Ok(plan)
}

pub fn execute(plan: &IntegrationPlan) -> Result<String> {
    ensure!(
        !plan.source_dirty || plan.choice == IntegrationChoice::Push,
        "Source checkout has uncommitted changes; commit or stash them first"
    );
    ensure!(
        plan.ignored_collisions.is_empty(),
        "Ignored files at incoming paths would be overwritten; move them before cherry-picking"
    );
    ensure!(
        plan.hidden_tracked.is_empty(),
        "Incoming paths are marked assume-unchanged or skip-worktree in the source checkout, so local edits could be hidden; clear those flags first"
    );
    let current = plan_again(plan)?;
    ensure!(
        current.revision == plan.revision,
        "Integration changed; refresh the preview before confirming"
    );
    match plan.choice {
        IntegrationChoice::Push => {
            let remote = plan.remote.as_deref().context("Missing remote")?;
            let tip = plan.remote_tip.as_deref().unwrap_or("");
            let reference = format!("refs/heads/{}", plan.destination_branch);
            // The lease is permitted only after ancestry is proven: even if the
            // remote races, its exact observed tip cannot be overwritten.
            if !tip.is_empty() {
                ancestor(&plan.companion_path, tip, &plan.companion_head)?;
            }
            git(
                &plan.companion_path,
                &[
                    "push",
                    &format!("--force-with-lease={reference}:{tip}"),
                    remote,
                    &format!("{}:{reference}", plan.companion_head),
                ],
            )?;
            Ok(match plan.pr_number {
                Some(pr) => format!(
                    "Pushed {} companion commit(s) to origin/{} for PR #{pr}",
                    plan.commits.len(),
                    plan.destination_branch
                ),
                None => format!(
                    "Pushed {} companion commit(s) to origin/{}",
                    plan.commits.len(),
                    plan.destination_branch
                ),
            })
        }
        IntegrationChoice::CherryPick => {
            let range = format!("{}..{}", plan.base_sha, plan.companion_head);
            pick_with_recovery(
                |args| git(&plan.source_path, args),
                &range,
                plan.commits.len(),
                &plan.source_branch,
                &plan.source_head,
            )
        }
    }
}
fn pick_with_recovery(
    mut run: impl FnMut(&[&str]) -> Result<String>,
    range: &str,
    count: usize,
    branch: &str,
    expected_head: &str,
) -> Result<String> {
    match run(&["cherry-pick", range]) {
        Ok(_) => Ok(format!(
            "Cherry-picked {count} companion commit(s) into {branch}"
        )),
        Err(error) => match run(&["cherry-pick", "--abort"]) {
            Ok(_) => match run(&["rev-parse", "HEAD"]) {
                Ok(head) if head.trim() == expected_head => {
                    bail!("Cherry-pick failed and was aborted: {error}")
                }
                _ => bail!(
                    "Cherry-pick failed: {error}. Abort ran but {branch} could not be confirmed at its original commit; inspect the source checkout."
                ),
            },
            Err(abort) => bail!(
                "Cherry-pick failed: {error}. Abort also failed: {abort}. Inspect the source checkout and resolve the Git operation manually."
            ),
        },
    }
}

fn plan_again(plan: &IntegrationPlan) -> Result<IntegrationPlan> {
    self::plan(plan.binding.clone(), plan.choice)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    fn run(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }
    fn fixture() -> (tempfile::TempDir, Binding) {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let companion = dir.path().join("companion");
        let remote = dir.path().join("remote.git");
        std::fs::create_dir(&source).unwrap();
        run(&source, &["init", "-q", "-b", "source"]);
        run(&source, &["config", "user.name", "AMF Test"]);
        run(&source, &["config", "user.email", "test@example.test"]);
        std::fs::write(source.join("file"), "base\n").unwrap();
        run(&source, &["add", "."]);
        run(&source, &["commit", "-qm", "base"]);
        let base = run(&source, &["rev-parse", "HEAD"]);
        assert!(
            Command::new("git")
                .args(["init", "--bare", "-q"])
                .arg(&remote)
                .status()
                .unwrap()
                .success()
        );
        run(
            &source,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        );
        run(&source, &["push", "-q", "origin", "source"]);
        run(
            &source,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "companion",
                companion.to_str().unwrap(),
            ],
        );
        std::fs::write(companion.join("file"), "one\n").unwrap();
        run(&companion, &["commit", "-qam", "one"]);
        std::fs::write(companion.join("another"), "two\n").unwrap();
        run(&companion, &["add", "."]);
        run(&companion, &["commit", "-qm", "two"]);
        let binding = Binding {
            project_id: "project".into(),
            companion_id: "companion".into(),
            companion_created_at: chrono::Utc::now(),
            companion_name: "Companion".into(),
            companion_branch: "companion".into(),
            companion_workdir: companion,
            source_id: "source".into(),
            source_created_at: chrono::Utc::now(),
            source_name: "Source".into(),
            source_branch: "source".into(),
            source_workdir: source,
            link: Link::Review(ReviewSource {
                source_feature_id: "source".into(),
                target_branch: "source".into(),
                base_sha: base,
            }),
        };
        (dir, binding)
    }
    #[test]
    fn cherry_pick_pins_multiple_commits_and_preserves_uncommitted_companion_content() {
        let (_dir, b) = fixture();
        std::fs::write(b.companion_workdir.join("uncommitted"), "excluded").unwrap();
        let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        assert_eq!(p.commits.len(), 2);
        assert!(p.companion_dirty);
        assert!(!p.source_dirty);
        execute(&p).unwrap();
        assert_eq!(
            std::fs::read_to_string(b.source_workdir.join("file")).unwrap(),
            "one\n"
        );
        assert!(!b.source_workdir.join("uncommitted").exists());
    }
    #[test]
    fn push_uses_pinned_fast_forward_and_refuses_changed_remote() {
        let (_dir, b) = fixture();
        let p = plan(b.clone(), IntegrationChoice::Push).unwrap();
        run(
            &b.source_workdir,
            &["commit", "--allow-empty", "-qm", "remote advance"],
        );
        run(&b.source_workdir, &["push", "-q", "origin", "source"]);
        assert!(execute(&p).is_err());
        assert!(plan(b.clone(), IntegrationChoice::Push).is_err());
        let (_dir, b) = fixture();
        let p = plan(b.clone(), IntegrationChoice::Push).unwrap();
        execute(&p).unwrap();
        assert_eq!(
            remote_tip(
                &b.companion_workdir,
                p.remote.as_ref().unwrap(),
                "refs/heads/source"
            )
            .unwrap(),
            Some(p.companion_head)
        );
    }
    #[test]
    fn preview_changes_and_dirty_source_are_refused() {
        let (_dir, b) = fixture();
        let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        std::fs::write(b.source_workdir.join("untracked"), "retain").unwrap();
        assert!(execute(&p).is_err());
        let dirty = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        assert!(dirty.source_dirty);
        assert!(execute(&dirty).is_err());
        assert_eq!(
            std::fs::read_to_string(b.source_workdir.join("untracked")).unwrap(),
            "retain"
        );
        std::fs::remove_file(b.source_workdir.join("untracked")).unwrap();
        std::fs::write(b.companion_workdir.join("file"), "new\n").unwrap();
        run(&b.companion_workdir, &["commit", "-qam", "new"]);
        assert!(execute(&p).is_err());
    }
    #[test]
    fn conflicting_pick_reports_abort_and_restores_source() {
        let (_dir, b) = fixture();
        std::fs::write(b.source_workdir.join("file"), "conflict\n").unwrap();
        run(&b.source_workdir, &["commit", "-qam", "conflict"]);
        let before = run(&b.source_workdir, &["rev-parse", "HEAD"]);
        let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        let error = execute(&p).unwrap_err().to_string();
        assert!(error.contains("failed and was aborted"), "{error}");
        assert_eq!(run(&b.source_workdir, &["rev-parse", "HEAD"]), before);
        assert!(clean(&b.source_workdir).unwrap());
    }
    #[test]
    fn failed_abort_reports_recovery_without_claiming_restoration() {
        let mut calls = Vec::new();
        let err = pick_with_recovery(
            |args| {
                calls.push(args[1].to_string());
                anyhow::bail!("fixture failure")
            },
            "base..head",
            1,
            "source",
            "head",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(calls, ["base..head", "--abort"]);
        assert!(
            err.contains("Abort also failed") && err.contains("resolve the Git operation manually")
        );
        assert!(!err.contains("restored"));
    }

    #[test]
    fn changed_checkout_and_invalid_git_state_refuse_without_touching_source() {
        let (_dir, b) = fixture();
        let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        let before = run(&b.source_workdir, &["rev-parse", "HEAD"]);
        run(&b.companion_workdir, &["checkout", "-qb", "different"]);
        assert!(execute(&p).is_err());
        assert_eq!(run(&b.source_workdir, &["rev-parse", "HEAD"]), before);
        run(&b.companion_workdir, &["checkout", "-q", "companion"]);
        std::fs::rename(
            &b.companion_workdir,
            b.companion_workdir.with_extension("old"),
        )
        .unwrap();
        std::fs::create_dir(&b.companion_workdir).unwrap();
        run(&b.companion_workdir, &["init", "-q", "-b", "companion"]);
        assert!(execute(&p).is_err());
        assert_eq!(run(&b.source_workdir, &["rev-parse", "HEAD"]), before);
    }
    #[test]
    fn binding_requires_exact_linked_source_in_the_same_project() {
        let (_dir, mut gui, target) = crate::gui_diff::tests::fixture();
        assert!(bind(&mut gui, &target).is_err());
        let app = gui.app_for_workflow();
        let project = &mut app.store.projects[0];
        let mut source = project.features[0].clone();
        source.id = "source-feature".into();
        source.name = "source".into();
        project.features[0].review_source = Some(ReviewSource {
            source_feature_id: source.id.clone(),
            target_branch: source.branch.clone(),
            base_sha: "a".repeat(40),
        });
        project.features.push(source);
        let binding = bind(&mut gui, &target).unwrap();
        assert_eq!(binding.source_id, "source-feature");
        gui.app_for_workflow().store.projects[0]
            .features
            .retain(|feature| feature.id != "source-feature");
        assert!(bind(&mut gui, &target).is_err());
    }
    #[test]
    fn ignored_incoming_file_is_protected_even_when_status_is_clean() {
        let (_dir, b) = fixture();
        // Both worktrees share this excludes file, and this does not modify the source checkout.
        let common = value(
            &b.source_workdir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )
        .unwrap();
        std::fs::write(Path::new(&common).join("info/exclude"), "private.txt\n").unwrap();
        std::fs::write(b.source_workdir.join("private.txt"), "user data").unwrap();
        std::fs::write(b.companion_workdir.join("private.txt"), "incoming").unwrap();
        run(&b.companion_workdir, &["add", "-f", "private.txt"]);
        run(&b.companion_workdir, &["commit", "-qm", "add ignored file"]);
        let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        assert!(!p.source_dirty);
        assert!(p.ignored_collisions.iter().any(|p| p == "private.txt"));
        assert!(execute(&p).is_err());
        assert_eq!(
            std::fs::read_to_string(b.source_workdir.join("private.txt")).unwrap(),
            "user data"
        );
    }

    #[test]
    fn hidden_tracked_incoming_path_is_refused() {
        for flag in ["--assume-unchanged", "--skip-worktree"] {
            let (_dir, b) = fixture();
            run(&b.source_workdir, &["update-index", flag, "file"]);
            std::fs::write(b.source_workdir.join("file"), "hidden edit\n").unwrap();
            let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
            assert!(!p.source_dirty, "status hides the edit");
            assert_eq!(p.hidden_tracked, ["file"], "{flag}");
            assert!(execute(&p).is_err());
            assert_eq!(
                std::fs::read_to_string(b.source_workdir.join("file")).unwrap(),
                "hidden edit\n"
            );
        }
    }

    #[test]
    fn abort_that_leaves_head_moved_is_not_reported_as_restored() {
        let err = pick_with_recovery(
            |args| match args[0] {
                "rev-parse" => Ok("other\n".into()),
                "cherry-pick" if args[1] == "--abort" => Ok(String::new()),
                _ => anyhow::bail!("conflict"),
            },
            "b..h",
            1,
            "source",
            "head",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("could not be confirmed") && !err.contains("was aborted"));
    }

    #[test]
    fn missing_remote_branch_can_be_created_only_while_still_missing() {
        let (_dir, b) = fixture();
        run(
            &b.source_workdir,
            &["push", "-q", "origin", "--delete", "source"],
        );
        let p = plan(b.clone(), IntegrationChoice::Push).unwrap();
        assert!(p.remote_tip.is_none());
        run(&b.source_workdir, &["push", "-q", "origin", "source"]);
        assert!(execute(&p).is_err());
        run(
            &b.source_workdir,
            &["push", "-q", "origin", "--delete", "source"],
        );
        let p = plan(b.clone(), IntegrationChoice::Push).unwrap();
        execute(&p).unwrap();
        assert_eq!(
            remote_tip(
                &b.companion_workdir,
                p.remote.as_ref().unwrap(),
                "refs/heads/source"
            )
            .unwrap(),
            Some(p.companion_head)
        );
    }

    #[test]
    fn remote_display_removes_url_credentials_and_query() {
        let display = display_remote("https://user:secret@example.test/repo?token=secret");
        assert!(!display.contains("secret") && !display.contains("user"));
        assert!(display.contains("example.test/repo"));
    }

    // PR Triage: the push destination is the PR head branch, which is not the
    // branch the triage source checkout has out.
    fn triage_fixture(publish_pr_branch: bool) -> (tempfile::TempDir, Binding) {
        let (dir, mut b) = fixture();
        let base = b.link.base_sha().to_owned();
        if publish_pr_branch {
            run(
                &b.source_workdir,
                &[
                    "push",
                    "-q",
                    "origin",
                    &format!("{base}:refs/heads/pr-head"),
                ],
            );
        }
        b.link = Link::Triage(TriageSource {
            pr_number: 42,
            source_feature_id: "source".into(),
            pr_branch: "pr-head".into(),
            base_sha: base,
        });
        (dir, b)
    }
    fn remote_branch(b: &Binding, branch: &str) -> Option<String> {
        let remote = run(&b.companion_workdir, &["remote", "get-url", "origin"]);
        remote_tip(
            &b.companion_workdir,
            &remote,
            &format!("refs/heads/{branch}"),
        )
        .unwrap()
    }

    #[test]
    fn triage_push_updates_the_pr_branch_not_the_source_branch() {
        let (_dir, b) = triage_fixture(true);
        let source_before = remote_branch(&b, "source");
        let p = plan(b.clone(), IntegrationChoice::Push).unwrap();
        assert_eq!(p.pr_number, Some(42));
        assert_eq!(p.destination_branch, "pr-head");
        assert_ne!(p.destination_branch, p.source_branch);
        assert_eq!(p.remote_tip.as_deref(), Some(p.base_sha.as_str()));
        let message = execute(&p).unwrap();
        assert!(message.contains("PR #42") && message.contains("origin/pr-head"));
        assert_eq!(remote_branch(&b, "pr-head"), Some(p.companion_head.clone()));
        assert_eq!(remote_branch(&b, "source"), source_before);
    }

    #[test]
    fn triage_push_never_creates_a_missing_pr_branch() {
        let (_dir, b) = triage_fixture(false);
        let err = plan(b.clone(), IntegrationChoice::Push)
            .unwrap_err()
            .to_string();
        assert!(err.contains("does not exist on origin"), "{err}");
        assert_eq!(remote_branch(&b, "pr-head"), None);
    }

    #[test]
    fn triage_push_refuses_a_pr_branch_that_moved_or_diverged_after_preview() {
        let (_dir, b) = triage_fixture(true);
        let p = plan(b.clone(), IntegrationChoice::Push).unwrap();
        run(
            &b.source_workdir,
            &["commit", "--allow-empty", "-qm", "other push"],
        );
        run(
            &b.source_workdir,
            &["push", "-q", "origin", "HEAD:refs/heads/pr-head"],
        );
        let moved = remote_branch(&b, "pr-head");
        assert!(execute(&p).is_err());
        assert_eq!(remote_branch(&b, "pr-head"), moved);
        // A divergent PR branch cannot even be previewed.
        let err = plan(b.clone(), IntegrationChoice::Push)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an ancestor"), "{err}");
    }

    #[test]
    fn triage_cherry_pick_lands_in_the_source_checkout_whatever_its_branch() {
        let (_dir, b) = triage_fixture(true);
        let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        assert_eq!(p.source_branch, "source");
        assert_eq!(p.destination_branch, "pr-head");
        execute(&p).unwrap();
        assert_eq!(
            std::fs::read_to_string(b.source_workdir.join("file")).unwrap(),
            "one\n"
        );
        // A source that moved to another branch since the preview is refused.
        let (_dir, b) = triage_fixture(true);
        let p = plan(b.clone(), IntegrationChoice::CherryPick).unwrap();
        run(&b.source_workdir, &["checkout", "-qb", "elsewhere"]);
        let head = run(&b.source_workdir, &["rev-parse", "HEAD"]);
        assert!(execute(&p).is_err());
        assert_eq!(run(&b.source_workdir, &["rev-parse", "HEAD"]), head);
    }

    #[test]
    fn triage_binding_follows_its_own_link_and_refuses_ambiguity() {
        let (_dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let project = &mut gui.app_for_workflow().store.projects[0];
        let mut source = project.features[0].clone();
        source.id = "source-feature".into();
        source.review_source = None;
        source.triage_source = None;
        project.features[0].triage_source = Some(TriageSource {
            pr_number: 7,
            source_feature_id: source.id.clone(),
            pr_branch: "pr".into(),
            base_sha: "a".repeat(40),
        });
        project.features.push(source);
        let binding = bind(&mut gui, &target).unwrap();
        assert_eq!(binding.link.pr_number(), Some(7));
        assert_eq!(binding.link.destination_branch(), "pr");
        gui.app_for_workflow().store.projects[0].features[0].review_source = Some(ReviewSource {
            source_feature_id: "source-feature".into(),
            target_branch: "x".into(),
            base_sha: "a".repeat(40),
        });
        assert!(bind(&mut gui, &target).is_err());
    }

    #[test]
    fn a_checkout_cannot_be_its_own_integration_source() {
        let (_dir, mut b) = fixture();
        b.source_workdir = b.companion_workdir.clone();
        b.source_branch = b.companion_branch.clone();
        b.link = Link::Triage(TriageSource {
            pr_number: 1,
            source_feature_id: "source".into(),
            pr_branch: "companion".into(),
            base_sha: b.link.base_sha().to_owned(),
        });
        let err = plan(b, IntegrationChoice::CherryPick)
            .unwrap_err()
            .to_string();
        assert!(err.contains("same checkout"), "{err}");
    }
}
