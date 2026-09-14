use super::support::*;
use crate::app::*;
use crate::project::{AgentKind, Feature, Project, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::Utc;
use std::collections::HashMap;
use tempfile::TempDir;

/// Build a fresh final-review `DiffViewerState` over two modified files for the
/// given workdir, install it as the current mode, and return nothing — the test
/// drives `app` afterwards.
#[cfg(test)]
fn enter_review_with_two_files(app: &mut App, workdir: &std::path::Path) {
    let mut state = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibe,
            false,
        ),
        workdir.to_path_buf(),
    );
    state.review = true;
    state.files = vec![
        crate::diff::DiffFile {
            old_path: Some("src/a.rs".into()),
            path: "src/a.rs".into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        },
        crate::diff::DiffFile {
            old_path: Some("src/b.rs".into()),
            path: "src/b.rs".into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        },
    ];
    app.mode = AppMode::DiffViewer(state);
}

/// A `ProjectStore` with one project/feature named to match
/// `enter_review_with_two_files`'s `ViewState` ("proj"/"feat"), so
/// `finish_final_review`'s per-project `final_review_check_command` lookup
/// resolves to the project rooted at `repo`.
#[cfg(test)]
fn store_with_review_project(repo: &std::path::Path) -> ProjectStore {
    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "feat".to_string(),
        branch: "feat".to_string(),
        workdir: repo.to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-feat".to_string(),
        sessions: vec![],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Active,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "proj".to_string(),
        repo: repo.to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    }
}

/// Poll `poll_final_review_check` until the spawned check process finishes
/// (or a generous cap is hit), so tests don't race the child process.
#[cfg(test)]
fn drain_final_review_check(app: &mut App) {
    for _ in 0..200 {
        app.poll_final_review_check().unwrap();
        let still_running =
            matches!(&app.mode, AppMode::DiffViewer(state) if state.finish_check_child.is_some());
        if !still_running {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("final review check did not complete in time");
}

#[test]
fn final_review_check_command_pass_reported_after_all_approved() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".amf")).unwrap();
    std::fs::write(
        repo.path().join(".amf").join("config.json"),
        r#"{"final_review_check_command": "exit 0"}"#,
    )
    .unwrap();

    let mut app = App::new_for_test(
        store_with_review_project(repo.path()),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    enter_review_with_two_files(&mut app, repo.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();

    app.finish_final_review().unwrap();
    assert!(
        matches!(&app.mode, AppMode::DiffViewer(state) if state.finish_check_child.is_some()),
        "the check should be spawned in the background rather than run inline"
    );

    drain_final_review_check(&mut app);

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("check `exit 0` passed"),
        "message should report the passing check: {msg}"
    );
}

#[test]
fn final_review_check_command_failure_blocks_all_approved_fast_path() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".amf")).unwrap();
    std::fs::write(
        repo.path().join(".amf").join("config.json"),
        r#"{"final_review_check_command": "exit 1"}"#,
    )
    .unwrap();

    let mut app = App::new_for_test(
        store_with_review_project(repo.path()),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    enter_review_with_two_files(&mut app, repo.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();

    app.finish_final_review().unwrap();
    drain_final_review_check(&mut app);

    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("check `exit 1` FAILED"),
        "message should report the failing check: {msg}"
    );

    let feedback =
        std::fs::read_to_string(repo.path().join(".claude").join("final-review-feedback.md"))
            .expect("a failing check must still write the feedback file, even with 0 rejections");
    assert!(feedback.contains("**Check:** `exit 1` — FAILED"));
}

#[test]
fn load_prior_agent_responses_populates_state_for_known_files() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // A prior round's feedback file: replies on a known file (src/a.rs) and on a
    // file no longer in the diff (src/gone.rs), which must be filtered out.
    let claude = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("final-review-feedback.md"),
        "# Final Review Feedback\n\n## Review — 2026-07-02T00:00:00Z\n\n\
         ### Line Comments\n\n#### src/a.rs:3 — [suggestion]\n\nRename it.\n\n\
         **Agent:** done, renamed.\n\n#### src/gone.rs:9 — [blocker]\n\nFix it.\n\n\
         **Agent:** removed the file.\n",
    )
    .unwrap();

    enter_review_with_two_files(&mut app, workdir.path());
    app.load_prior_agent_responses();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.prior_agent_responses.len(),
                1,
                "only files still in the diff are kept"
            );
            let a = state.prior_agent_responses.get("src/a.rs").expect("a.rs");
            assert_eq!(a[0].anchor, "src/a.rs:3");
            assert!(a[0].response.contains("renamed"));
            assert!(
                !state.prior_agent_responses.contains_key("src/gone.rs"),
                "a file no longer in the diff is dropped"
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn load_prior_agent_responses_no_feedback_file_is_empty() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    enter_review_with_two_files(&mut app, workdir.path());
    app.load_prior_agent_responses();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.prior_agent_responses.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn final_review_progress_persists_and_resumes() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    enter_review_with_two_files(&mut app, workdir.path());

    // Approve file 0 (advances to file 1), reject file 1 with feedback, and add
    // general feedback. Each action persists progress to disk.
    app.diff_review_approve_current();
    app.diff_review_start_feedback();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("needs work".into());
    }
    app.diff_review_submit_feedback();
    app.diff_review_start_general_feedback();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("overall looks ok".into());
    }
    app.diff_review_submit_general_feedback();

    let progress_path = workdir
        .path()
        .join(".claude")
        .join("final-review-progress.json");
    assert!(progress_path.exists(), "progress file should be written");

    // Simulate closing and reopening the review: a brand-new state for the same
    // workdir, then restore.
    enter_review_with_two_files(&mut app, workdir.path());
    app.restore_review_progress();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("src/a.rs"),
                Some(&ReviewDecision::Approve)
            );
            assert_eq!(
                state.decisions.get("src/b.rs"),
                Some(&ReviewDecision::Reject {
                    feedback: "needs work".into(),
                    // An explicit rejection defaults to Blocker severity.
                    severity: crate::app::Severity::Blocker,
                })
            );
            assert_eq!(state.general_feedback, "overall looks ok");
        }
        _ => panic!("expected diff viewer after restore"),
    }

    // Finishing the review clears the saved progress.
    app.finish_final_review().unwrap();
    assert!(
        !progress_path.exists(),
        "progress file should be removed after finishing"
    );
}

#[test]
fn comment_auto_reject_survives_pause_and_resume() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // One commentable hunk on src/a.rs: a context line and an added line.
    let hunk = crate::diff::DiffHunk {
        header: "@@ -1,1 +1,2 @@".into(),
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 2,
        lines: vec![
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Context,
                text: " ctx".into(),
            },
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Added,
                text: "+added".into(),
            },
        ],
    };

    // Comment the added line: the file auto-rejects and progress persists.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk.clone()];
        state.comment_cursor = Some(1);
        state.editing_line_comment = true;
        state.feedback_editor = crate::editor::TextEditor::new("bug".into());
    }
    app.diff_review_submit_line_comment();

    // Reopen the review fresh and restore: the verdict comes back still marked
    // as comment-implied, not explicit.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk];
    }
    app.restore_review_progress();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("src/a.rs"),
                Some(&ReviewDecision::Reject {
                    feedback: String::new(),
                    // An auto-rejection carries the neutral default severity;
                    // the real severity lives on the line comment.
                    severity: crate::app::Severity::Suggestion,
                })
            );
            assert!(state.auto_rejected.contains("src/a.rs"));
        }
        _ => panic!("expected diff viewer after restore"),
    }

    // Deleting the restored comment (empty re-submit) still clears the
    // implicit verdict — the distinction survived the round-trip.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(1);
        state.editing_line_comment = true;
        state.feedback_editor = crate::editor::TextEditor::new(String::new());
    }
    app.diff_review_submit_line_comment();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty());
            assert!(state.auto_rejected.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn restore_review_progress_skips_when_decisions_present() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // Write a progress file that, if applied, would approve src/a.rs.
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("final-review-progress.json"),
        r#"{"decisions":{"src/a.rs":"Approve"},"line_comments":{},"general_feedback":"stale","selected_file":0}"#,
    )
    .unwrap();

    // Enter a review that already has an in-memory decision (mimicking an
    // in-review refresh); restore must not clobber it with the disk copy.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state
            .decisions
            .insert("src/b.rs".into(), ReviewDecision::Approve);
    }
    app.restore_review_progress();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(
                !state.decisions.contains_key("src/a.rs"),
                "should not have merged disk progress over in-memory state"
            );
            assert_eq!(state.general_feedback, "");
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_flags_changed_files_and_applies_filter() {
    use crate::app::FileFilter;
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // First round: approve both files and finish. This records the snapshot.
    enter_review_with_two_files(&mut app, workdir.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    let snapshot_path = workdir
        .path()
        .join(".claude")
        .join("final-review-snapshot.json");
    assert!(
        snapshot_path.exists(),
        "snapshot should be written on finish"
    );

    // Re-open with src/b.rs's diff changed (different patch); src/a.rs is
    // untouched.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[1].patch = "@@ -1 +1 @@\n-old\n+new".into();
    }
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.has_prior_review, "a prior snapshot exists");
            assert_eq!(
                state.changed_since_last.iter().cloned().collect::<Vec<_>>(),
                vec!["src/b.rs".to_string()],
                "only the changed file is flagged"
            );
            assert_eq!(
                state.file_filter,
                FileFilter::Changed,
                "a partial re-review auto-applies the changed filter"
            );
            assert_eq!(
                state.selected_file, 1,
                "selection snaps onto the changed file"
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_no_changes_keeps_all_filter() {
    use crate::app::FileFilter;
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    enter_review_with_two_files(&mut app, workdir.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    // Re-open with the identical diff: nothing changed since the last round.
    enter_review_with_two_files(&mut app, workdir.path());
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.has_prior_review);
            assert!(
                state.changed_since_last.is_empty(),
                "no files changed since the last review"
            );
            assert_eq!(
                state.file_filter,
                FileFilter::All,
                "an unchanged re-review leaves the filter untouched"
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_carries_approvals_for_unchanged_files() {
    use crate::app::FileFilter;
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // Round 1: approve src/a.rs, reject src/b.rs, and finish. The snapshot now
    // records a.rs=Approve and b.rs=Reject.
    enter_review_with_two_files(&mut app, workdir.path());
    app.diff_review_approve_current();
    app.diff_review_start_feedback();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("fix this".into());
    }
    app.diff_review_submit_feedback();
    app.finish_final_review().unwrap();

    // Round 2: the fix landed, so b.rs's diff changed; a.rs is untouched.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[1].patch = "@@ -1 +1 @@\n-old\n+new".into();
    }
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            // The unchanged, previously-approved file keeps its verdict — the
            // reviewer resumes from the saved approved state.
            assert_eq!(
                state.decisions.get("src/a.rs"),
                Some(&ReviewDecision::Approve),
                "an unchanged approved file carries its approval into the re-review"
            );
            // The changed (and previously rejected) file is reset for a fresh
            // look, and the review narrows onto it.
            assert!(
                !state.decisions.contains_key("src/b.rs"),
                "a changed file does not carry a stale verdict"
            );
            assert_eq!(state.file_filter, FileFilter::Changed);
            assert_eq!(state.selected_file, 1);
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_carries_an_unresolved_thread_and_reports_its_count() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    let hunk = crate::diff::DiffHunk {
        header: "@@ -1,1 +1,2 @@".into(),
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 2,
        lines: vec![
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Context,
                text: " ctx".into(),
            },
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Added,
                text: "+added".into(),
            },
        ],
    };

    // Round 1: comment on src/a.rs (auto-rejects it), approve src/b.rs, finish.
    // The snapshot now carries the open thread on src/a.rs.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk.clone()];
        state.comment_cursor = Some(1);
        state.editing_line_comment = true;
        state.feedback_editor = crate::editor::TextEditor::new("still needs work".into());
    }
    app.diff_review_submit_line_comment();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    // Round 2: reopen against the same (untouched) diff and restore.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk];
    }
    app.restore_review_progress();
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            let comments = state.line_comments.get("src/a.rs").expect("thread carried");
            assert_eq!(comments.len(), 1);
            assert!(comments[0].carried, "restored thread is marked carried");
            assert!(!comments[0].resolved, "the thread is still open");
            assert_eq!(comments[0].text, "still needs work");
            // The open thread means src/a.rs must not inherit any stale
            // approval, even though nothing about its diff changed.
            assert!(!state.decisions.contains_key("src/a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }
    let message = app.message.as_deref().unwrap_or_default();
    assert!(
        message.contains("no files changed since the last review"),
        "{message}"
    );
    assert!(
        message.contains("1 unresolved thread carried over"),
        "{message}"
    );
}

#[test]
fn interdiff_shows_diff_since_last_reviewed_content() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // Round 1: src/a.rs's worktree content at the time the round finishes is
    // captured into the snapshot.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].new_content = Some("fn a() {}\n".into());
    }
    app.diff_review_approve_current();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    // Round 2: the agent edited src/a.rs since. Its base-ref patch is left
    // empty (as in the fixture) — the interdiff must not depend on it.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].new_content = Some("fn a() {\n    println!(\"hi\");\n}\n".into());
    }

    app.open_interdiff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.interdiff_open, "modal opens when content changed");
            let diff_file = state
                .interdiff_file
                .as_ref()
                .expect("interdiff computed against last round's content");
            assert_eq!(diff_file.path, "src/a.rs");
            assert!(
                !diff_file.hunks.is_empty(),
                "the content actually changed between rounds"
            );
            assert!(diff_file.patch.contains("println"), "{}", diff_file.patch);
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn interdiff_noop_without_prior_review() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // No prior finished review, so no snapshot exists on disk yet.
    enter_review_with_two_files(&mut app, workdir.path());
    app.open_interdiff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.interdiff_open, "nothing to diff against yet");
            assert!(state.interdiff_file.is_none());
        }
        _ => panic!("expected diff viewer"),
    }
    assert_eq!(
        app.message.as_deref(),
        Some("No prior review to diff against")
    );
}

/// Build a final-review `DiffViewerState` over `paths` (already in the
/// path-sorted order the diff loader produces) and install it as the mode, so
/// tree navigation can be driven through the same `App` methods the handlers
/// call.
#[cfg(test)]
fn enter_review_with_paths(app: &mut App, paths: &[&str]) {
    let mut state = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibe,
            false,
        ),
        std::path::PathBuf::from("/tmp"),
    );
    state.review = true;
    state.files = paths
        .iter()
        .map(|path| crate::diff::DiffFile {
            old_path: Some((*path).into()),
            path: (*path).into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        })
        .collect();
    app.mode = AppMode::DiffViewer(state);
}

#[cfg(test)]
fn tree_rows(app: &App) -> Vec<FileTreeRow> {
    match &app.mode {
        AppMode::DiffViewer(state) => state.file_tree_rows(),
        _ => panic!("not in the diff viewer"),
    }
}

#[cfg(test)]
fn viewer_state(app: &App) -> &DiffViewerState {
    match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("not in the diff viewer"),
    }
}

/// A 30-line file with line 15 rewritten, hydrated with the blobs context
/// expansion reads from — the same fixture `crate::diff`'s expansion tests use,
/// built here so the viewer can be driven end to end.
#[cfg(test)]
fn expandable_diff_file() -> crate::diff::DiffFile {
    let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
    let new = old.replace("l15\n", "l15 changed\n");
    let mut file = crate::diff::parse_unified_diff(
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
    )
    .unwrap()
    .pop()
    .unwrap();
    file.old_content = Some(old);
    file.new_content = Some(new);
    file
}

/// Install a final-review viewer over a single expandable file.
#[cfg(test)]
fn enter_review_with_expandable_file(app: &mut App) {
    enter_review_with_paths(app, &["src/lib.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = vec![expandable_diff_file()];
    }
}

#[test]
fn expanding_context_widens_the_hunk_and_records_the_level() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    let before = viewer_state(&app).files[0].hunks[0].old_lines;
    assert_eq!(app.diff_viewer_context_level(), Some(3));

    app.diff_viewer_expand_context();

    assert_eq!(app.diff_viewer_context_level(), Some(10));
    let state = viewer_state(&app);
    assert!(state.files[0].hunks[0].old_lines > before);
    assert_eq!(state.context_expansion.get("src/lib.rs").copied(), Some(10));
    assert_eq!(app.message.as_deref(), Some("Context: 10 lines"));
}

#[test]
fn the_context_ladder_tops_out_at_the_whole_file_and_returns_to_the_default() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    for _ in 0..4 {
        app.diff_viewer_expand_context();
    }
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));
    assert_eq!(viewer_state(&app).files[0].hunks[0].old_lines, 30);

    // Already at the top: the level holds and the reviewer is told why.
    app.diff_viewer_expand_context();
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));
    assert_eq!(
        app.message.as_deref(),
        Some("Already showing the whole file")
    );

    for _ in 0..4 {
        app.diff_viewer_collapse_context();
    }
    assert_eq!(app.diff_viewer_context_level(), Some(3));
    // Back at the default, the per-file entry is dropped rather than pinned.
    assert!(viewer_state(&app).context_expansion.is_empty());
    assert_eq!(viewer_state(&app).files[0].hunks[0].old_lines, 7);
}

#[test]
fn whole_file_toggle_jumps_both_ways() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_toggle_whole_file_context();
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));

    app.diff_viewer_toggle_whole_file_context();
    assert_eq!(app.diff_viewer_context_level(), Some(3));
}

#[test]
fn expanding_context_keeps_the_line_cursor_on_the_same_diff_line() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    // Park the cursor on the removed line and start a range selection there.
    let removed = viewer_state(&app).files[0]
        .addressable_lines()
        .iter()
        .position(|loc| loc.old_line == Some(15) && loc.new_line.is_none())
        .unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(removed);
        state.comment_anchor = Some(removed);
    }

    app.diff_viewer_expand_context();

    // The index moved (7 context lines were prepended) but it still points at
    // the same line, which is what a comment would anchor to.
    let state = viewer_state(&app);
    let cursor = state.comment_cursor.unwrap();
    assert_ne!(cursor, removed);
    assert_eq!(
        state.files[0].addressable_lines()[cursor],
        crate::diff::DiffLineLocation {
            old_line: Some(15),
            new_line: None
        }
    );
    assert_eq!(state.comment_anchor, Some(cursor));
    assert!(state.cursor_sync_to_view);
}

#[test]
fn expanding_context_raises_the_patch_scroll_bound() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_scroll_patch_bottom();
    let before = viewer_state(&app).patch_scroll;

    app.diff_viewer_toggle_whole_file_context();
    app.diff_viewer_scroll_patch_bottom();

    // The scroll ceiling is derived from the rendered hunks, not the raw
    // `patch` string, so the expanded rows are actually reachable.
    assert!(
        viewer_state(&app).patch_scroll > before,
        "expanded content must be scrollable"
    );
}

/// The index of the whole-file-only context line `l1`, which narrowing the
/// context back to the default hides again.
#[cfg(test)]
fn whole_file_only_line(app: &App) -> usize {
    viewer_state(app).files[0]
        .addressable_lines()
        .iter()
        .position(|loc| loc.old_line == Some(1))
        .expect("the whole file starts at old line 1")
}

#[test]
fn narrowing_context_refuses_while_the_selected_range_would_be_hidden() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_toggle_whole_file_context();

    // A range selection whose start only exists at whole-file context.
    let top = whole_file_only_line(&app);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(top + 4);
        state.comment_anchor = Some(top);
    }

    app.diff_viewer_toggle_whole_file_context();

    // Refused outright: dropping an endpoint would silently re-point a comment
    // made afterwards at lines the reviewer never selected.
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));
    let state = viewer_state(&app);
    assert_eq!(state.comment_cursor, Some(top + 4));
    assert_eq!(state.comment_anchor, Some(top));
    assert_eq!(
        app.message.as_deref(),
        Some(
            "Selected lines would be hidden at that context level — clear the selection (Esc) first"
        )
    );
}

#[test]
fn narrowing_context_keeps_a_selection_that_stays_visible() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_toggle_whole_file_context();

    // The changed line survives at any context level, so the range travels.
    let changed = viewer_state(&app).files[0]
        .addressable_lines()
        .iter()
        .position(|loc| loc.old_line == Some(15) && loc.new_line.is_none())
        .unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(changed);
        state.comment_anchor = Some(changed);
    }

    app.diff_viewer_toggle_whole_file_context();

    assert_eq!(app.diff_viewer_context_level(), Some(3));
    let state = viewer_state(&app);
    let cursor = state.comment_cursor.unwrap();
    assert_eq!(
        state.files[0].addressable_lines()[cursor],
        crate::diff::DiffLineLocation {
            old_line: Some(15),
            new_line: None
        }
    );
    assert_eq!(state.comment_anchor, Some(cursor));
}

#[test]
fn narrowing_context_moves_a_hidden_cursor_to_the_nearest_line_and_reports_it() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_toggle_whole_file_context();

    let top = whole_file_only_line(&app);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(top);
        state.comment_anchor = None;
    }

    app.diff_viewer_toggle_whole_file_context();

    // No selection to protect, so the change goes ahead — but the cursor lands
    // on the nearest line still rendered (old line 12, the hunk's first
    // context line) rather than silently snapping to index 0 of a file whose
    // first rendered line has moved.
    assert_eq!(app.diff_viewer_context_level(), Some(3));
    let state = viewer_state(&app);
    let cursor = state.comment_cursor.unwrap();
    assert_eq!(
        state.files[0].addressable_lines()[cursor],
        crate::diff::DiffLineLocation {
            old_line: Some(12),
            new_line: Some(12)
        }
    );
    assert!(state.cursor_sync_to_view);
    let message = app.message.clone().unwrap();
    assert!(
        message.contains("cursor moved to line 12"),
        "the move must be reported: {message}"
    );
}

#[test]
fn narrowing_context_clamps_the_patch_scroll_into_the_shorter_patch() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_toggle_whole_file_context();
    app.diff_viewer_scroll_patch_bottom();
    let expanded_scroll = viewer_state(&app).patch_scroll;

    app.diff_viewer_toggle_whole_file_context();

    // The patch is short again; leaving the old offset would render a blank
    // panel until the reviewer scrolled back up.
    let max_scroll = app.diff_viewer_patch_line_count().saturating_sub(1);
    let scroll = viewer_state(&app).patch_scroll;
    assert!(
        scroll <= max_scroll,
        "scroll {scroll} is past the new last line {max_scroll}"
    );
    assert!(
        expanded_scroll > max_scroll,
        "the test must actually start out of range: {expanded_scroll} vs {max_scroll}"
    );
}

#[test]
fn context_expansion_survives_a_diff_reload() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_expand_context();

    // A refresh replaces `files` with freshly parsed (unexpanded) hunks.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = vec![expandable_diff_file()];
        assert_eq!(state.files[0].hunks[0].old_lines, 7);
        state.reapply_context_expansion();
    }

    let state = viewer_state(&app);
    assert_eq!(state.files[0].hunks[0].old_lines, 21);
    assert_eq!(state.context_expansion.get("src/lib.rs").copied(), Some(10));
}

#[test]
fn context_expansion_is_dropped_for_a_file_that_left_the_changeset() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_expand_context();

    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files.clear();
        state.reapply_context_expansion();
        assert!(state.context_expansion.is_empty());
    }
}

#[test]
fn toggling_ignore_whitespace_flips_the_flag_and_reloads() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_toggle_ignore_whitespace();

    // `-w` changes what git emits, so the toggle must go through the loader
    // rather than just re-rendering what's already in memory.
    assert!(
        matches!(&app.mode, AppMode::DiffViewerLoading(s) if s.ignore_whitespace),
        "toggle should flip the flag and enter the loading state"
    );
    assert_eq!(
        app.message.as_deref(),
        Some("Ignoring whitespace-only changes (git diff -w)")
    );
}

#[test]
fn a_file_with_no_surrounding_context_reports_why_instead_of_no_opping() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    // `enter_review_with_paths` builds hunk-less, blob-less files — exactly the
    // added/binary shape that can't be expanded.
    enter_review_with_paths(&mut app, &["src/new.rs"]);

    app.diff_viewer_expand_context();

    assert_eq!(app.diff_viewer_context_level(), Some(3));
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("no surrounding context"),
        "got {:?}",
        app.message
    );
}

#[test]
fn ancestor_dirs_lists_each_level_shallowest_first() {
    assert_eq!(ancestor_dirs("README.md"), Vec::<String>::new());
    assert_eq!(ancestor_dirs("src/main.rs"), vec!["src".to_string()]);
    assert_eq!(
        ancestor_dirs("src/app/dialogs/diff.rs"),
        vec![
            "src".to_string(),
            "src/app".to_string(),
            "src/app/dialogs".to_string()
        ]
    );
}

#[test]
fn file_tree_rows_group_by_directory_without_reordering_the_file_list() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    // Exactly the order `crate::diff` produces (files sorted by full path).
    enter_review_with_paths(
        &mut app,
        &[
            "README.md",
            "src/a.rs",
            "src/app/mod.rs",
            "src/app/state.rs",
        ],
    );

    let rows = tree_rows(&app);
    let described: Vec<String> = rows
        .iter()
        .map(|row| match row {
            FileTreeRow::Dir { path, depth, .. } => format!("dir {path} @{depth}"),
            FileTreeRow::File { index, depth, name } => format!("file {name} #{index} @{depth}"),
        })
        .collect();
    assert_eq!(
        described,
        vec![
            "file README.md #0 @0",
            "dir src @0",
            "file a.rs #1 @1",
            "dir src/app @1",
            "file mod.rs #2 @2",
            "file state.rs #3 @2",
        ]
    );
    // File rows keep the original `files` order, so n/p and the tree agree.
    let file_indices: Vec<usize> = rows
        .iter()
        .filter_map(|row| match row {
            FileTreeRow::File { index, .. } => Some(*index),
            FileTreeRow::Dir { .. } => None,
        })
        .collect();
    assert_eq!(file_indices, vec![0, 1, 2, 3]);
}

#[test]
fn collapsing_a_directory_hides_its_files_but_not_from_filters() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(
        &mut app,
        &["src/a.rs", "src/app/mod.rs", "src/app/state.rs"],
    );

    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.toggle_dir_collapsed("src/app");
    }

    let rows = tree_rows(&app);
    assert!(
        matches!(&rows[2], FileTreeRow::Dir { path, collapsed, files, .. }
            if path == "src/app" && *collapsed && *files == 2),
        "collapsed directory row should summarise the two files it hides: {rows:?}"
    );
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, FileTreeRow::File { .. }))
            .count(),
        1,
        "only the file outside the collapsed directory should have a row"
    );
    // Folding is a view concern only: filters and counts still see every file.
    assert_eq!(viewer_state(&app).visible_file_indices(), vec![0, 1, 2]);
}

#[test]
fn file_order_navigation_reveals_a_file_inside_a_collapsed_directory() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/a.rs", "src/app/mod.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.toggle_dir_collapsed("src/app");
    }

    // n (file-order navigation) must never strand the selection behind a fold.
    app.diff_viewer_select_next_file();

    let state = viewer_state(&app);
    assert_eq!(state.selected_file, 1);
    assert!(state.collapsed_dirs.is_empty());
    assert!(state.tree_cursor_dir.is_none());
}

#[test]
fn tree_move_walks_directory_rows_and_leaves_the_patch_alone() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["README.md", "src/a.rs"]);

    // Row 0 is README.md; row 1 is the `src` directory header.
    app.diff_viewer_tree_move(1);
    let state = viewer_state(&app);
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
    assert_eq!(
        state.selected_file, 0,
        "parking on a directory must not change which file the patch shows"
    );

    // Folding from the directory row keeps the cursor there.
    app.diff_viewer_tree_toggle_collapsed();
    let state = viewer_state(&app);
    assert!(state.collapsed_dirs.contains("src"));
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));

    // Stepping down onto a file selects it and drops the directory cursor.
    app.diff_viewer_tree_expand();
    app.diff_viewer_tree_move(1);
    let state = viewer_state(&app);
    assert_eq!(state.selected_file, 1);
    assert!(state.tree_cursor_dir.is_none());
}

#[test]
fn toggle_all_folds_every_directory_then_unfolds_them() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/a.rs", "src/app/mod.rs"]);

    app.diff_viewer_tree_toggle_all();
    let state = viewer_state(&app);
    assert!(state.collapsed_dirs.contains("src"));
    assert!(state.collapsed_dirs.contains("src/app"));
    // The selection is folded away, so the cursor parks on its outermost dir
    // and the list still highlights a row.
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
    let rows = state.file_tree_rows();
    assert_eq!(
        rows.len(),
        1,
        "everything should fold into one row: {rows:?}"
    );
    assert!(state.tree_cursor_row(&rows).is_some());

    app.diff_viewer_tree_toggle_all();
    assert!(viewer_state(&app).collapsed_dirs.is_empty());
}

#[test]
fn tree_cursor_row_falls_back_to_the_deepest_visible_ancestor() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/app/mod.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        // Fold the inner directory directly, without moving the selection.
        state.toggle_dir_collapsed("src/app");
    }

    let state = viewer_state(&app);
    let rows = state.file_tree_rows();
    let cursor = state
        .tree_cursor_row(&rows)
        .expect("a row stays highlighted");
    assert!(
        matches!(&rows[cursor], FileTreeRow::Dir { path, .. } if path == "src/app"),
        "the highlight should fall back to the fold hiding the selected file"
    );
}

#[test]
fn tree_move_enters_the_list_when_the_filter_hides_the_selection() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["README.md", "src/a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        // Select a file the Undecided filter then drops, leaving a single
        // root-level row and nothing highlighted.
        state.selected_file = 1;
        state
            .decisions
            .insert("src/a.rs".to_string(), crate::app::ReviewDecision::Approve);
        state.file_filter = crate::app::FileFilter::Undecided;
    }
    assert!(
        viewer_state(&app)
            .tree_cursor_row(&tree_rows(&app))
            .is_none(),
        "the test needs a state with no highlighted row"
    );

    // j must reach the only row rather than stepping past it.
    app.diff_viewer_tree_move(1);
    assert_eq!(viewer_state(&app).selected_file, 0);

    // ...and so must k, from the other end.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
    }
    app.diff_viewer_tree_move(-1);
    assert_eq!(viewer_state(&app).selected_file, 0);
}

#[test]
fn tree_toggle_folds_the_highlighted_row_not_the_hidden_selections_directory() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/a.rs", "src/app/mod.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
        state.decisions.insert(
            "src/app/mod.rs".to_string(),
            crate::app::ReviewDecision::Approve,
        );
        state.file_filter = crate::app::FileFilter::Undecided;
    }
    // With src/app/mod.rs filtered out, `src/app` has no row and the highlight
    // falls back to `src`.
    let rows = tree_rows(&app);
    let cursor = viewer_state(&app)
        .tree_cursor_row(&rows)
        .expect("a row stays highlighted");
    assert!(matches!(&rows[cursor], FileTreeRow::Dir { path, .. } if path == "src"));

    app.diff_viewer_tree_toggle_collapsed();

    let state = viewer_state(&app);
    assert!(
        state.collapsed_dirs.contains("src"),
        "z should fold the directory the list highlights"
    );
    assert!(
        !state.collapsed_dirs.contains("src/app"),
        "z must not fold a directory that has no row: {:?}",
        state.collapsed_dirs
    );
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
}
