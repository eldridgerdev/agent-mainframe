//! Read-only catalog of where a PR fix may be handed: the opened feature and
//! the verified triage companion(s) of this PR, with their sessions.
//!
//! Nothing here creates, starts or sends anything. A companion is "verified"
//! exactly as `gui_pr_workspace::is_pr_companion` decides for the fix-target
//! check (`triage_source.pr_number` and `triage_source.source_feature_id`
//! both match), so the catalog can never list a destination the handoff would
//! then refuse. Mirrors the catalog step of `gui_review::destinations`.
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::gui_pr_triage::PrFixTargetView;
use crate::gui_pr_workspace::{CheckoutIdentity, DestinationKind};

#[derive(Debug, Clone, Deserialize)]
pub struct DestinationCatalogRequest {
    /// The feature the PR workflow was opened from.
    pub target: FeatureTarget,
    pub pr_number: u32,
}

/// What the link between a companion and its source records, plus the one
/// derived fact that needs Git.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompanionFacts {
    pub pr_branch: String,
    pub base_sha: String,
    /// Commits on the companion's branch that its recorded base does not
    /// contain. `None` whenever it could not be established; the reason is in
    /// `commits_ahead_error`. Never defaults to zero.
    pub commits_ahead: Option<u32>,
    pub commits_ahead_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DestinationFeature {
    pub kind: DestinationKind,
    pub target: FeatureTarget,
    pub name: String,
    pub branch: String,
    pub workdir: String,
    /// The feature itself is started (a stopped feature may still be listed).
    pub running: bool,
    /// Set when the feature cannot receive a fix and why (for example it
    /// shares its tmux session with another feature).
    pub blocked: Option<String>,
    pub sessions: Vec<PrFixTargetView>,
    /// Present for companions only.
    pub companion: Option<CompanionFacts>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DestinationCatalog {
    pub pr_number: u32,
    pub opened: DestinationFeature,
    pub companions: Vec<DestinationFeature>,
}

/// The off-lock half: Git reads for each companion.
pub struct DestinationCatalogRead {
    catalog: DestinationCatalog,
    reads: Vec<AheadRead>,
}

struct AheadRead {
    index: usize,
    workdir: PathBuf,
    branch: String,
    base_sha: String,
    identity: Option<CheckoutIdentity>,
}

fn session_count_on_tmux(app: &crate::app::App, tmux_session: &str) -> usize {
    app.store
        .projects
        .iter()
        .flat_map(|p| &p.features)
        .filter(|f| f.tmux_session == tmux_session)
        .count()
}

pub fn catalog_read(
    gui: &mut GuiHandle,
    request: DestinationCatalogRequest,
) -> GuiResult<DestinationCatalogRead> {
    gui.refresh_store()?;
    read_from_app(gui.app_for_workflow(), request)
}

fn read_from_app(
    app: &crate::app::App,
    request: DestinationCatalogRequest,
) -> GuiResult<DestinationCatalogRead> {
    if request.pr_number == 0 {
        return Err(GuiError::conflict("Select a PR first"));
    }
    let project_id = &request.target.project_id;
    let opened_id = &request.target.feature_id;
    app.store
        .locate_feature_by_id(Some(project_id), opened_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let rows =
        crate::gui_pr_workspace::fix_target_rows(app, project_id, opened_id, request.pr_number);
    let mut opened = None;
    let mut companions = Vec::new();
    let mut reads = Vec::new();
    for (kind, feature) in
        crate::gui_pr_workspace::destination_features(app, project_id, opened_id, request.pr_number)
    {
        let blocked = (session_count_on_tmux(app, &feature.tmux_session) != 1).then(|| {
            "Shares its tmux session with another feature; choose another destination".to_string()
        });
        let companion = feature
            .triage_source
            .as_ref()
            .filter(|_| kind == DestinationKind::Companion);
        let view = DestinationFeature {
            kind,
            target: FeatureTarget {
                project_id: project_id.clone(),
                feature_id: feature.id.clone(),
            },
            name: feature.name.clone(),
            branch: feature.branch.clone(),
            workdir: feature.workdir.display().to_string(),
            running: feature.status != crate::project::ProjectStatus::Stopped,
            blocked,
            sessions: rows
                .iter()
                .filter(|row| row.target.feature_id == feature.id)
                .cloned()
                .collect(),
            companion: companion.map(|source| CompanionFacts {
                pr_branch: source.pr_branch.clone(),
                base_sha: source.base_sha.clone(),
                commits_ahead: None,
                commits_ahead_error: Some("Not read".into()),
            }),
        };
        match kind {
            DestinationKind::OpenedFeature => opened = Some(view),
            DestinationKind::Companion => {
                if let Some(source) = companion {
                    reads.push(AheadRead {
                        index: companions.len(),
                        workdir: feature.workdir.clone(),
                        branch: feature.branch.clone(),
                        base_sha: source.base_sha.clone(),
                        identity: crate::gui_pr_workspace::checkout_identity(&feature.workdir),
                    });
                }
                companions.push(view);
            }
        }
    }
    Ok(DestinationCatalogRead {
        catalog: DestinationCatalog {
            pr_number: request.pr_number,
            opened: opened.ok_or_else(|| GuiError::not_found("The feature was deleted"))?,
            companions,
        },
        reads,
    })
}

impl DestinationCatalogRead {
    /// Runs the Git reads. Safe to call without any GUI lock held.
    pub fn run(mut self) -> DestinationCatalog {
        for read in &self.reads {
            let outcome = commits_ahead(read);
            if let Some(facts) = self.catalog.companions[read.index].companion.as_mut() {
                match outcome {
                    Ok(count) => {
                        facts.commits_ahead = Some(count);
                        facts.commits_ahead_error = None;
                    }
                    Err(reason) => {
                        facts.commits_ahead = None;
                        facts.commits_ahead_error = Some(reason);
                    }
                }
            }
        }
        self.catalog
    }
}

fn is_object_id(value: &str) -> bool {
    (value.len() == 40 || value.len() == 64) && value.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Every failure is an `Err` with a reason; a Git error never reads as zero.
fn commits_ahead(read: &AheadRead) -> Result<u32, String> {
    let before = read
        .identity
        .as_ref()
        .ok_or("The companion checkout is missing or is not a Git worktree")?;
    if !is_object_id(&read.base_sha) {
        return Err("The recorded base is not a full commit ID".into());
    }
    let head = git(
        &read.workdir,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
    )
    .map_err(|_| "The companion checkout has a detached or unreadable HEAD".to_string())?;
    if head.trim() != read.branch {
        return Err(format!(
            "The companion checkout is on `{}`, not `{}`",
            head.trim(),
            read.branch
        ));
    }
    git(
        &read.workdir,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", read.base_sha),
        ],
    )
    .map_err(|_| "The recorded base commit is not in the companion checkout".to_string())?;
    let count = git(
        &read.workdir,
        &["rev-list", "--count", &format!("{}..HEAD", read.base_sha)],
    )
    .map_err(|error| format!("Could not count commits: {error}"))?;
    let count = count
        .trim()
        .parse::<u32>()
        .map_err(|_| "Git returned an unreadable commit count".to_string())?;
    // The directory must still be the checkout we identified before reading.
    if crate::gui_pr_workspace::checkout_identity(&read.workdir).as_ref() != Some(before) {
        return Err("The companion checkout changed while it was being read".into());
    }
    Ok(count)
}

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut stdout = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut command = Command::new("git");
    command
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
        .stdin(Stdio::null())
        .stdout(stdout.try_clone().map_err(|e| e.to_string())?)
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|_| "Could not start Git".to_string())?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(10) => {
                std::thread::sleep(Duration::from_millis(20))
            }
            _ => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err("Git did not finish in time".into());
            }
        }
    };
    if !status.success() {
        return Err(format!("git {} failed", args.first().unwrap_or(&"")));
    }
    stdout.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    stdout
        .take(65_537)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    if out.len() > 65_536 {
        return Err("Git output exceeded its limit".into());
    }
    String::from_utf8(out).map_err(|_| "Git output is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AgentKind, Feature, ProjectStatus, SessionKind, TriageSource, VibeMode};

    fn run_git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    struct Fixture {
        dir: tempfile::TempDir,
        gui: GuiHandle,
        target: FeatureTarget,
        base_sha: String,
    }

    fn fixture() -> Fixture {
        let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
        let repo = dir.path().join("repo");
        let base_sha = run_git(&repo, &["rev-parse", "HEAD"]);
        let app = gui.app_for_workflow();
        let mut tmux = crate::traits::MockTmuxOps::new();
        tmux.expect_window_exists().returning(|_, _| true);
        app.tmux = Box::new(tmux);
        let opened = &mut app.store.projects[0].features[0];
        opened.status = ProjectStatus::Idle;
        opened.add_session(SessionKind::Claude);
        Fixture {
            dir,
            gui,
            target,
            base_sha,
        }
    }

    impl Fixture {
        /// A real linked worktree on `branch` at the PR head with `commits` extra commits.
        fn add_companion(
            &mut self,
            name: &str,
            pr: u32,
            source: Option<&str>,
            commits: u32,
            status: ProjectStatus,
        ) -> String {
            let repo = self.dir.path().join("repo");
            let workdir = self.dir.path().join(name);
            let branch = format!("{name}-branch");
            run_git(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-b",
                    &branch,
                    workdir.to_str().unwrap(),
                    &self.base_sha,
                ],
            );
            for i in 0..commits {
                std::fs::write(workdir.join(format!("f{i}.txt")), "x").unwrap();
                run_git(&workdir, &["add", "."]);
                run_git(&workdir, &["commit", "-m", &format!("c{i}")]);
            }
            let opened_id = self.target.feature_id.clone();
            let base = self.base_sha.clone();
            let app = self.gui.app_for_workflow();
            let mut feature = Feature::new(
                name.into(),
                branch,
                workdir,
                true,
                VibeMode::default(),
                false,
                false,
                AgentKind::Claude,
                false,
                false,
            );
            feature.status = status;
            feature.triage_source = source.map(|s| TriageSource {
                pr_number: pr,
                source_feature_id: if s.is_empty() { opened_id } else { s.into() },
                pr_branch: "feature".into(),
                base_sha: base,
            });
            feature.add_session(SessionKind::Claude);
            let id = feature.id.clone();
            app.store.projects[0].features.push(feature);
            id
        }

        fn catalog(&mut self) -> DestinationCatalog {
            let app = self.gui.app_for_workflow();
            read_from_app(
                app,
                DestinationCatalogRequest {
                    target: self.target.clone(),
                    pr_number: 42,
                },
            )
            .unwrap()
            .run()
        }
    }

    #[test]
    fn lists_opened_feature_sessions_and_a_running_companion_with_commits_ahead() {
        let mut f = fixture();
        let id = f.add_companion("fix-triage", 42, Some(""), 2, ProjectStatus::Idle);
        let catalog = f.catalog();
        assert_eq!(catalog.opened.kind, DestinationKind::OpenedFeature);
        assert_eq!(catalog.opened.sessions.len(), 1);
        assert_eq!(catalog.companions.len(), 1);
        let c = &catalog.companions[0];
        assert_eq!(c.target.feature_id, id);
        assert!(c.running);
        assert_eq!(c.sessions.len(), 1);
        assert_eq!(c.sessions[0].destination_kind, DestinationKind::Companion);
        assert_eq!(c.branch, "fix-triage-branch");
        let facts = c.companion.as_ref().unwrap();
        assert_eq!(facts.commits_ahead, Some(2));
        assert_eq!(facts.commits_ahead_error, None);
        assert_eq!(facts.pr_branch, "feature");
    }

    #[test]
    fn a_stopped_companion_is_listed_not_hidden() {
        let mut f = fixture();
        f.add_companion("stopped-triage", 42, Some(""), 0, ProjectStatus::Stopped);
        let catalog = f.catalog();
        assert_eq!(catalog.companions.len(), 1);
        let c = &catalog.companions[0];
        assert!(!c.running);
        assert!(c.sessions[0].stopped);
        assert_eq!(c.companion.as_ref().unwrap().commits_ahead, Some(0));
    }

    #[test]
    fn companion_identity_is_exactly_pr_and_source_feature() {
        let mut f = fixture();
        let good = f.add_companion("good", 42, Some(""), 0, ProjectStatus::Idle);
        f.add_companion("other-pr", 41, Some(""), 0, ProjectStatus::Idle);
        f.add_companion(
            "other-source",
            42,
            Some("some-other-feature"),
            0,
            ProjectStatus::Idle,
        );
        f.add_companion("unlinked", 42, None, 0, ProjectStatus::Idle);
        let catalog = f.catalog();
        let ids: Vec<_> = catalog
            .companions
            .iter()
            .map(|c| c.target.feature_id.clone())
            .collect();
        assert_eq!(ids, vec![good]);
    }

    #[test]
    fn a_companion_is_never_listed_for_itself_or_as_the_opened_feature_of_another_pr() {
        let mut f = fixture();
        let id = f.add_companion("c", 42, Some(""), 0, ProjectStatus::Idle);
        // Opening the PR from the companion itself: its own source link points at
        // the original feature, so it has no companions of its own.
        let app = f.gui.app_for_workflow();
        let catalog = read_from_app(
            app,
            DestinationCatalogRequest {
                target: FeatureTarget {
                    project_id: f.target.project_id.clone(),
                    feature_id: id,
                },
                pr_number: 42,
            },
        )
        .unwrap()
        .run();
        assert!(catalog.companions.is_empty());
    }

    #[test]
    fn a_deleted_companion_disappears_and_a_deleted_opened_feature_is_not_found() {
        let mut f = fixture();
        let id = f.add_companion("gone", 42, Some(""), 0, ProjectStatus::Idle);
        f.gui.app_for_workflow().store.projects[0]
            .features
            .retain(|x| x.id != id);
        assert!(f.catalog().companions.is_empty());
        f.gui.app_for_workflow().store.projects[0].features.clear();
        let app = f.gui.app_for_workflow();
        let err = read_from_app(
            app,
            DestinationCatalogRequest {
                target: f.target.clone(),
                pr_number: 42,
            },
        )
        .err()
        .unwrap();
        assert!(err.message.contains("deleted"));
    }

    #[test]
    fn git_failures_fail_closed_instead_of_reading_as_zero() {
        let mut f = fixture();
        f.add_companion("missing-dir", 42, Some(""), 1, ProjectStatus::Idle);
        let wrong_branch = f.add_companion("wrong-branch", 42, Some(""), 1, ProjectStatus::Idle);
        f.add_companion("bad-base", 42, Some(""), 1, ProjectStatus::Idle);
        f.add_companion("short-base", 42, Some(""), 1, ProjectStatus::Idle);
        let missing = f.dir.path().join("missing-dir");
        let wrong_dir = f.dir.path().join("wrong-branch");
        run_git(&wrong_dir, &["checkout", "-b", "elsewhere"]);
        let app = f.gui.app_for_workflow();
        for feature in &mut app.store.projects[0].features {
            if feature.name == "bad-base" {
                feature.triage_source.as_mut().unwrap().base_sha = "0".repeat(40);
            }
            if feature.name == "short-base" {
                feature.triage_source.as_mut().unwrap().base_sha = "abc".into();
            }
        }
        // Remove the checkout last so the identity cannot be computed.
        run_git(
            &f.dir.path().join("repo"),
            &["worktree", "remove", "--force", missing.to_str().unwrap()],
        );
        let catalog = f.catalog();
        assert_eq!(catalog.companions.len(), 4);
        for c in &catalog.companions {
            let facts = c.companion.as_ref().unwrap();
            assert_eq!(facts.commits_ahead, None, "{}", c.name);
            let reason = facts.commits_ahead_error.as_deref().unwrap();
            assert!(!reason.is_empty(), "{}", c.name);
            if c.target.feature_id == wrong_branch {
                assert!(reason.contains("elsewhere"), "{reason}");
            }
        }
    }

    #[test]
    fn a_companion_sharing_a_tmux_session_is_flagged_blocked() {
        let mut f = fixture();
        f.add_companion("shared", 42, Some(""), 0, ProjectStatus::Idle);
        let app = f.gui.app_for_workflow();
        let opened_tmux = app.store.projects[0].features[0].tmux_session.clone();
        app.store.projects[0].features[1].tmux_session = opened_tmux;
        let catalog = f.catalog();
        assert!(catalog.companions[0].blocked.is_some());
        assert!(catalog.opened.blocked.is_some());
        assert!(catalog.companions[0].sessions.is_empty());
    }

    #[test]
    fn the_catalog_is_read_only() {
        let mut f = fixture();
        f.add_companion("ro", 42, Some(""), 1, ProjectStatus::Idle);
        let before =
            serde_json::to_string(&f.gui.app_for_workflow().store.projects[0].features.len())
                .unwrap();
        let head = run_git(&f.dir.path().join("ro"), &["rev-parse", "HEAD"]);
        f.catalog();
        assert_eq!(
            before,
            serde_json::to_string(&f.gui.app_for_workflow().store.projects[0].features.len())
                .unwrap()
        );
        assert_eq!(
            head,
            run_git(&f.dir.path().join("ro"), &["rev-parse", "HEAD"])
        );
        assert_eq!(
            run_git(&f.dir.path().join("ro"), &["status", "--porcelain"]),
            ""
        );
    }
}
