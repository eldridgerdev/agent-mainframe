//! Read-only GUI diffs use the same Git snapshots and context expansion as
//! the TUI. Requests resolve stable feature IDs afresh; no workflow mode or
//! agent session is needed to browse changes.
use serde::{Deserialize, Serialize};

use crate::diff::{self, DiffFileStatus, DiffLineKind};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DiffContext {
    #[default]
    Standard,
    Expanded,
    Full,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct DiffOptions {
    pub commit: Option<String>,
    pub base_ref: Option<String>,
    #[serde(default)]
    pub ignore_whitespace: bool,
    #[serde(default)]
    pub context: DiffContext,
}

#[derive(Debug, Serialize)]
pub struct DiffCommitView {
    pub hash: String,
    pub short_hash: String,
    pub subject: String,
}

#[derive(Debug, Serialize)]
pub struct DiffLineView {
    pub kind: &'static str,
    pub text: String,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct DiffHunkView {
    pub header: String,
    pub lines: Vec<DiffLineView>,
}

#[derive(Debug, Serialize)]
pub struct DiffFileView {
    pub path: String,
    pub old_path: Option<String>,
    pub status: &'static str,
    pub additions: usize,
    pub deletions: usize,
    pub is_binary: bool,
    pub hunks: Vec<DiffHunkView>,
    /// Retains mode changes and rename metadata even when there are no hunks.
    pub patch: String,
}

#[derive(Debug, Serialize)]
pub struct DiffView {
    pub target: FeatureTarget,
    pub feature_name: String,
    pub branch: String,
    pub base_ref: String,
    pub base_commit: String,
    pub commit: Option<String>,
    pub commits: Vec<DiffCommitView>,
    pub commits_error: Option<String>,
    pub files: Vec<DiffFileView>,
    pub total_additions: usize,
    pub total_deletions: usize,
}

pub fn load(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    options: DiffOptions,
) -> GuiResult<DiffView> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; refresh and retry"))?;
    let project = &app.store.projects[pi];
    if !project.is_git {
        return Err(GuiError::conflict("Diffs require a Git checkout"));
    }
    let feature = &project.features[fi];
    let (commits, commits_error) = match diff::list_diff_commits(&feature.workdir) {
        Ok(commits) => (commits, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let snapshot = if let Some(hash) = &options.commit {
        // Accept only the picker entry's full object ID. A force-push or branch
        // switch cannot turn an old selection into an unrelated revision.
        let commit = commits
            .iter()
            .find(|commit| &commit.hash == hash)
            .ok_or_else(|| {
                GuiError::conflict(
                    "That commit is no longer in the feature's history; choose current changes",
                )
            })?;
        diff::load_commit_snapshot(&feature.workdir, &commit.hash, options.ignore_whitespace)?
    } else {
        diff::load_snapshot(
            &feature.workdir,
            options.base_ref.as_deref(),
            options.ignore_whitespace,
        )?
    };
    let context = match options.context {
        DiffContext::Standard => 3,
        DiffContext::Expanded => 10,
        DiffContext::Full => usize::MAX,
    };
    let files = snapshot
        .files
        .into_iter()
        .map(|file| {
            let hunks = file.hunks_with_context(context).unwrap_or(file.hunks);
            let hunks = hunks
                .into_iter()
                .map(|hunk| {
                    let locations = diff::line_locations_in_hunk(&hunk);
                    DiffHunkView {
                        header: hunk.header,
                        lines: hunk
                            .lines
                            .into_iter()
                            .zip(locations)
                            .map(|(line, location)| DiffLineView {
                                kind: match line.kind {
                                    DiffLineKind::Context => "context",
                                    DiffLineKind::Added => "added",
                                    DiffLineKind::Removed => "removed",
                                    DiffLineKind::NoNewlineMarker => "marker",
                                },
                                text: line.text,
                                old_line: location.and_then(|l| l.old_line),
                                new_line: location.and_then(|l| l.new_line),
                            })
                            .collect(),
                    }
                })
                .collect();
            DiffFileView {
                path: file.path,
                old_path: file.old_path,
                status: match file.status {
                    DiffFileStatus::Added => "added",
                    DiffFileStatus::Modified => "modified",
                    DiffFileStatus::Deleted => "deleted",
                    DiffFileStatus::Renamed => "renamed",
                    DiffFileStatus::Copied => "copied",
                    DiffFileStatus::TypeChanged => "type_changed",
                    DiffFileStatus::Untracked => "untracked",
                },
                additions: file.additions,
                deletions: file.deletions,
                is_binary: file.is_binary,
                hunks,
                patch: file.patch,
            }
        })
        .collect();
    Ok(DiffView {
        target,
        feature_name: feature.name.clone(),
        branch: snapshot.branch,
        base_ref: snapshot.base_ref,
        base_commit: snapshot.base_commit,
        commit: options.commit,
        commits: commits
            .into_iter()
            .map(|c| DiffCommitView {
                hash: c.hash,
                short_hash: c.short_hash,
                subject: c.subject,
            })
            .collect(),
        commits_error,
        files,
        total_additions: snapshot.total_additions,
        total_deletions: snapshot.total_deletions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, AppMode};
    use crate::db::AmfDb;
    use crate::gui_contract::GuiErrorKind;
    use crate::project::{AgentKind, Feature, Project, ProjectStatus, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::Path;
    use std::process::Command;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn fixture() -> (tempfile::TempDir, GuiHandle, FeatureTarget) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        let content = (1..=20).map(|i| format!("line {i}\n")).collect::<String>();
        std::fs::write(repo.join("code.txt"), &content).unwrap();
        std::fs::write(repo.join("old.txt"), "rename me\n").unwrap();
        std::fs::write(repo.join("delete.txt"), "delete me\n").unwrap();
        git(&repo, &["add", "."]);
        git(&repo, &["commit", "-m", "base"]);
        git(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(
            repo.join("code.txt"),
            content.replace("line 8\n", "committed 🦀\n"),
        )
        .unwrap();
        git(&repo, &["add", "code.txt"]);
        git(&repo, &["commit", "-m", "feature commit"]);
        let mut project = Project::new("Demo".into(), repo.clone(), true, AgentKind::Claude);
        let mut feature = Feature::new_for_project(
            "Demo",
            "Feature".into(),
            "feature".into(),
            repo,
            false,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        feature.status = ProjectStatus::Stopped;
        let target = FeatureTarget {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
        };
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let db = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        db.save_store(&store).unwrap();
        let (_, version) = db.load_store_versioned().unwrap();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.db = Some(db);
        app.store_version = Some(version);
        (dir, GuiHandle::from_app(app), target)
    }

    #[test]
    fn current_changes_include_commits_staged_unstaged_and_untracked_without_starting() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let path = repo.join("code.txt");
        let current = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, current.replace("line 9\n", "unstaged\n")).unwrap();
        std::fs::write(repo.join("staged.txt"), "staged\n").unwrap();
        git(&repo, &["add", "staged.txt"]);
        std::fs::write(repo.join("untracked.txt"), "new 🦀\n").unwrap();
        let view = load(&mut gui, target, DiffOptions::default()).unwrap();
        assert_eq!(view.commits.len(), 1);
        let code = view.files.iter().find(|f| f.path == "code.txt").unwrap();
        assert!(
            code.hunks[0]
                .lines
                .iter()
                .any(|l| l.text == "+committed 🦀" && l.new_line == Some(8))
        );
        assert!(
            code.hunks[0]
                .lines
                .iter()
                .any(|l| l.text == "+unstaged" && l.new_line == Some(9))
        );
        assert!(
            view.files
                .iter()
                .any(|f| f.path == "staged.txt" && f.status == "added")
        );
        assert!(
            view.files
                .iter()
                .any(|f| f.path == "untracked.txt" && f.status == "untracked")
        );
        assert!(matches!(gui.app_for_workflow().mode, AppMode::Normal));
        assert_eq!(
            gui.snapshot().projects[0].features[0].status,
            ProjectStatus::Stopped
        );
    }

    #[test]
    fn commit_scope_excludes_working_changes_and_refuses_stale_or_arbitrary_revisions() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let hash = git(&repo, &["rev-parse", "HEAD"]);
        std::fs::write(repo.join("untracked.txt"), "not committed\n").unwrap();
        let options = DiffOptions {
            commit: Some(hash.clone()),
            ..Default::default()
        };
        let view = load(&mut gui, target.clone(), options.clone()).unwrap();
        assert_eq!(view.files.len(), 1);
        assert_eq!(view.files[0].path, "code.txt");
        assert_eq!(view.commit.as_deref(), Some(hash.as_str()));
        assert_eq!(
            load(
                &mut gui,
                target.clone(),
                DiffOptions {
                    commit: Some("HEAD".into()),
                    ..Default::default()
                }
            )
            .unwrap_err()
            .kind,
            GuiErrorKind::Conflict
        );
        git(&repo, &["reset", "--hard", "main"]);
        assert_eq!(
            load(&mut gui, target, options).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
    }

    #[test]
    fn context_expansion_keeps_source_coordinates_and_totals() {
        let (_dir, mut gui, target) = fixture();
        let standard = load(&mut gui, target.clone(), DiffOptions::default()).unwrap();
        let full = load(
            &mut gui,
            target,
            DiffOptions {
                context: DiffContext::Full,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(full.files[0].hunks[0].lines.len() > standard.files[0].hunks[0].lines.len());
        assert_eq!(full.total_additions, standard.total_additions);
        assert_eq!(full.total_deletions, standard.total_deletions);
        let last = full.files[0].hunks[0].lines.last().unwrap();
        assert_eq!((last.old_line, last.new_line), (Some(20), Some(20)));
    }

    #[test]
    fn refresh_and_whitespace_filter_use_current_git_contents() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        git(&repo, &["reset", "--hard", "main"]);
        assert!(
            load(&mut gui, target.clone(), DiffOptions::default())
                .unwrap()
                .files
                .is_empty()
        );
        let path = repo.join("code.txt");
        let content = std::fs::read_to_string(&path).unwrap();
        std::fs::write(path, content.replace("line 8\n", "line    8\n")).unwrap();
        assert_eq!(
            load(&mut gui, target.clone(), DiffOptions::default())
                .unwrap()
                .files
                .len(),
            1
        );
        assert!(
            load(
                &mut gui,
                target,
                DiffOptions {
                    ignore_whitespace: true,
                    ..Default::default()
                }
            )
            .unwrap()
            .files
            .is_empty()
        );
    }

    #[test]
    fn rename_binary_deletion_and_no_newline_metadata_survive_the_contract() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        git(&repo, &["mv", "old.txt", "new.txt"]);
        git(&repo, &["rm", "delete.txt"]);
        std::fs::write(repo.join("binary.bin"), b"\0binary\0").unwrap();
        git(&repo, &["add", "binary.bin"]);
        std::fs::write(repo.join("nonewline.txt"), "no newline").unwrap();
        let view = load(&mut gui, target, DiffOptions::default()).unwrap();
        let renamed = view.files.iter().find(|f| f.path == "new.txt").unwrap();
        assert_eq!(renamed.old_path.as_deref(), Some("old.txt"));
        assert_eq!(renamed.status, "renamed");
        assert!(renamed.patch.contains("rename from"));
        assert!(
            view.files
                .iter()
                .any(|f| f.path == "binary.bin" && f.is_binary)
        );
        let deleted = view.files.iter().find(|f| f.path == "delete.txt").unwrap();
        assert_eq!(deleted.status, "deleted");
        assert!(
            deleted.hunks[0]
                .lines
                .iter()
                .any(|l| l.old_line == Some(1) && l.new_line.is_none())
        );
        let no_newline = view
            .files
            .iter()
            .find(|f| f.path == "nonewline.txt")
            .unwrap();
        assert!(
            no_newline.hunks[0]
                .lines
                .iter()
                .any(|l| l.kind == "marker" && l.old_line.is_none() && l.new_line.is_none())
        );
    }

    #[test]
    fn external_deletion_and_wrong_project_cannot_load_a_stale_feature() {
        let (dir, mut gui, target) = fixture();
        let wrong = FeatureTarget {
            project_id: "wrong".into(),
            ..target.clone()
        };
        assert_eq!(
            load(&mut gui, wrong, DiffOptions::default())
                .unwrap_err()
                .kind,
            GuiErrorKind::NotFound
        );
        let other = AmfDb::open(&dir.path().join("amf.db")).unwrap();
        let mut store = other.load_store().unwrap();
        store.projects[0].features.clear();
        other.save_store(&store).unwrap();
        assert_eq!(
            load(&mut gui, target, DiffOptions::default())
                .unwrap_err()
                .kind,
            GuiErrorKind::NotFound
        );
    }

    #[test]
    fn ordinary_directories_and_invalid_base_refs_report_actionable_errors() {
        let (_dir, mut gui, target) = fixture();
        let error = load(
            &mut gui,
            target.clone(),
            DiffOptions {
                base_ref: Some("missing-base".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.message.contains("missing-base"));
        let app = gui.app_for_workflow();
        app.store.projects[0].is_git = false;
        assert_eq!(
            load(&mut gui, target, DiffOptions::default())
                .unwrap_err()
                .kind,
            GuiErrorKind::Conflict
        );
    }
}
