use super::support::*;
use crate::app::ai_review::{
    AiReviewAttribution, AiReviewFinding, AiReviewOutcome, AiReviewProgress,
};
use crate::app::pr_review::{
    github_access::fake::FakeGithub,
    runtime::{PrPostOutcome, ReviewListLoaded},
};
use crate::app::{
    pr_workspace::{PrWorkspace, WorkspacePane, WorkspaceTab},
    *,
};
use crate::diff::DiffSide;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};
use std::{
    path::Path,
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};
use tempfile::TempDir;

struct Fixture {
    app: App,
    _dir: TempDir,
}

fn reviewable() -> crate::github::ReviewablePr {
    crate::github::ReviewablePr {
        number: 7,
        title: "A real review workspace".into(),
        author: "alice".into(),
        is_draft: false,
        updated_at: String::new(),
        base_ref: "main".into(),
        base_oid: "base".into(),
        head_ref: "main".into(),
        head_oid: "sha".into(),
        is_cross_repository: false,
        head_owner: "o".into(),
    }
}

fn finding() -> AiReviewFinding {
    AiReviewFinding {
        path: Some("src/file2.rs".into()),
        line: Some(2),
        side: Some(DiffSide::New),
        body: "Check this change".into(),
        diff_hunk: None,
        skipped: false,
        published: false,
    }
}

impl Fixture {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let mut app = pr_review_test_app();
        app.db = Some(crate::db::AmfDb::open(&dir.path().join("test.db")).unwrap());
        enter_pr_review(&mut app, 3);
        let AppMode::PrReview(mut triage) = std::mem::replace(&mut app.mode, AppMode::Normal)
        else {
            unreachable!()
        };
        triage.workdir = dir.path().to_path_buf();
        let pr = triage.review.pr.clone();
        let mut ai = sample_ai_review_state(dir.path().to_path_buf(), pr.clone());
        ai.findings = vec![finding()];
        let mut files = DiffViewerState::new(
            view_state_for("project", "feature"),
            dir.path().to_path_buf(),
        );
        files.review = true;
        files.scope = DiffScope::PullRequest(Box::new(PrDiffTarget {
            repo: "github.com/o/r".into(),
            pr: reviewable(),
            merge_base_oid: "base".into(),
        }));
        files.files = crate::diff::parse_unified_diff("diff --git a/src/file1.rs b/src/file1.rs\n--- a/src/file1.rs\n+++ b/src/file1.rs\n@@ -1,3 +1,3 @@\n old\n-before\n+after\n context\ndiff --git a/src/file2.rs b/src/file2.rs\n--- a/src/file2.rs\n+++ b/src/file2.rs\n@@ -1,3 +1,3 @@\n old\n-before\n+after\n context\ndiff --git a/src/file3.rs b/src/file3.rs\n--- a/src/file3.rs\n+++ b/src/file3.rs\n@@ -1,3 +1,3 @@\n old\n-before\n+after\n context\n").unwrap();
        let mut workspace = PrWorkspace::new(dir.path().to_path_buf(), AppMode::Normal);
        workspace.pr = Some(pr);
        workspace.parked[WorkspaceTab::Threads.index()] = Some(Box::new(AppMode::PrReview(triage)));
        workspace.parked[WorkspaceTab::Ai.index()] = Some(Box::new(AppMode::AiReview(ai)));
        app.mode = AppMode::DiffViewer(files);
        app.pr_workspace = Some(workspace);
        Self { app, _dir: dir }
    }
}

fn press(app: &mut App, code: KeyCode) {
    crate::handlers::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 40).unwrap();
}

fn files(app: &App) -> &DiffViewerState {
    app.pr_workspace.as_ref().unwrap().files(&app.mode).unwrap()
}

fn settle(app: &mut App, pending: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while pending(app) {
        app.poll_pr_workspace_bg();
        assert!(Instant::now() < deadline, "worker did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn categories_keep_selection_and_restore_manual_reading_position() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    if let AppMode::DiffViewer(files) = &mut app.mode {
        files.selected_file = 2;
        files.patch_scroll = 7;
    }
    press(app, KeyCode::Char('2'));
    press(app, KeyCode::Char('j'));
    assert!(matches!(&app.mode, AppMode::PrReview(state) if state.selected == 1));
    assert_eq!(files(app).selected_file, 1);
    assert!(files(app).comment_cursor.is_some());
    press(app, KeyCode::Char('3'));
    press(app, KeyCode::Char('2'));
    assert!(matches!(&app.mode, AppMode::PrReview(state) if state.selected == 1));
    press(app, KeyCode::Char('1'));
    assert_eq!(files(app).selected_file, 2);
    assert_eq!(files(app).patch_scroll, 7);
}

#[test]
fn feedback_anchors_respect_old_side_and_general_or_outdated_feedback() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('2'));
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[1].side = Some("LEFT".into());
    }
    press(app, KeyCode::Char('j'));
    let files = files(app);
    assert_eq!(
        files.files[files.selected_file].addressable_lines()[files.comment_cursor.unwrap()]
            .old_line,
        Some(2)
    );
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[2].outdated = true;
    }
    press(app, KeyCode::Char('j'));
    assert!(
        app.pr_workspace
            .as_ref()
            .unwrap()
            .anchor_message
            .as_ref()
            .unwrap()
            .contains("Outdated")
    );
    press(app, KeyCode::Char('k'));
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[0].path = None;
    }
    press(app, KeyCode::Char('k'));
    assert!(
        app.pr_workspace
            .as_ref()
            .unwrap()
            .anchor_message
            .as_ref()
            .unwrap()
            .contains("General")
    );
}

#[test]
fn draft_editor_owns_shortcuts_and_close_saves_parked_human_drafts() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('m'));
    for c in "123zpq".chars() {
        press(app, KeyCode::Char(c));
    }
    assert!(
        matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_file_comment && state.feedback_editor.text() == "123zpq")
    );
    assert_eq!(app.pr_workspace.as_ref().unwrap().tab, WorkspaceTab::Files);
    press(app, KeyCode::Tab);
    assert_eq!(files(app).file_comments["src/file1.rs"].text, "123zpq");
    let mut terminal = Terminal::new(TestBackend::new(160, 40)).unwrap();
    terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(
        rendered.contains("123zpq"),
        "saved human drafts should be visible in the context pane"
    );
    press(app, KeyCode::Char('2'));
    press(app, KeyCode::Char('q'));
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.pr_workspace.is_none());
    let draft = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_review_draft("github.com/o/r", 7)
        .unwrap()
        .unwrap();
    assert!(draft.progress.contains("123zpq"));
}

#[test]
fn ai_completion_updates_parked_view_without_moving_the_reader() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('3'));
    let AppMode::AiReview(origin) = &app.mode else {
        unreachable!()
    };
    let origin = origin.clone();
    let (tx, rx) = mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin));
    press(app, KeyCode::Char('1'));
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 2;
        state.patch_scroll = 4;
    }
    tx.send(AiReviewProgress::Done(Ok(AiReviewOutcome {
        findings: vec![finding(), finding()],
        summary: Some("Two findings".into()),
        raw_output: String::new(),
        attribution: AiReviewAttribution::default(),
    })))
    .unwrap();
    assert!(app.poll_pr_workspace_bg());
    assert!(matches!(app.mode, AppMode::DiffViewer(_)));
    assert_eq!(files(app).selected_file, 2);
    assert_eq!(files(app).patch_scroll, 4);
    press(app, KeyCode::Char('3'));
    assert!(matches!(&app.mode, AppMode::AiReview(state) if state.findings.len() == 2));
}

#[test]
fn comment_fetch_failure_keeps_files_open_and_can_retry() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    let github = FakeGithub::default();
    github.state().fail_fetch = Some("offline fixture".into());
    app.pr_review_work
        .set_github_for_test(Arc::new(github.clone()));
    press(app, KeyCode::Char('2'));
    app.retry_workspace_comments();
    press(app, KeyCode::Char('1'));
    settle(app, |app| app.pr_review_work.fetch_pending());
    assert!(matches!(app.mode, AppMode::DiffViewer(_)));
    assert!(
        app.pr_workspace.as_ref().unwrap().errors[WorkspaceTab::Threads.index()]
            .as_ref()
            .unwrap()
            .contains("offline fixture")
    );
    github.state().fail_fetch = None;
    github.state().review = Some(pr_review_with_comments(2));
    press(app, KeyCode::Char('2'));
    press(app, KeyCode::Char('r'));
    settle(app, |app| app.pr_review_work.fetch_pending());
    assert!(matches!(&app.mode, AppMode::PrReview(state) if state.review.comments.len() == 2));
    assert!(app.pr_workspace.as_ref().unwrap().errors[WorkspaceTab::Threads.index()].is_none());
}

#[test]
fn human_submission_uses_real_confirmation_and_worker() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    app.pr_review_work
        .set_review_poster_for_test(|_| PrPostOutcome::Posted {
            file_comment_failures: vec![],
        });
    press(app, KeyCode::Char('f'));
    crate::handlers::handle_paste(app, "Human review summary").unwrap();
    press(app, KeyCode::Tab);
    assert_eq!(files(app).general_feedback, "Human review summary");
    press(app, KeyCode::Char('p'));
    assert!(files(app).pr_submit.is_some());
    assert!(
        !app.pr_review_work.review_post_pending(),
        "preview must not post"
    );
    press(app, KeyCode::Enter);
    assert!(
        app.pr_review_work.review_post_pending(),
        "submit status: {:?}; message: {:?}",
        files(app).pr_submit.as_ref().map(|submit| &submit.status),
        app.message
    );
    settle(app, |app| app.pr_review_work.review_post_pending());
    assert!(
        app.message
            .as_ref()
            .is_some_and(|message| message.contains("Posted a"))
    );
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.pr_workspace.is_none());
    let saved = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_review_draft("github.com/o/r", 7)
        .unwrap()
        .unwrap();
    assert_eq!(
        saved.status,
        crate::db::pr_review_drafts::PrReviewDraftStatus::Posted
    );
}

#[test]
fn focus_and_expansion_do_not_change_category_or_selection() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('2'));
    press(app, KeyCode::Tab);
    press(app, KeyCode::Tab);
    assert_eq!(
        app.pr_workspace.as_ref().unwrap().focus,
        WorkspacePane::Context
    );
    press(app, KeyCode::Char('j'));
    assert!(matches!(&app.mode, AppMode::PrReview(state) if state.selected == 0));
    press(app, KeyCode::Char('z'));
    assert!(app.pr_workspace.as_ref().unwrap().expanded);
    press(app, KeyCode::Esc);
    assert!(!app.pr_workspace.as_ref().unwrap().expanded);
    assert!(app.pr_workspace.is_some());
}

#[test]
fn responsive_layout_uses_real_code_and_feedback_at_every_width() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    for width in [160, 100, 60, 20] {
        let mut terminal = Terminal::new(TestBackend::new(width, 32)).unwrap();
        for tab in [KeyCode::Char('1'), KeyCode::Char('2'), KeyCode::Char('3')] {
            press(app, tab);
            for _ in 0..3 {
                terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
                if width == 160 {
                    let text: String = terminal
                        .backend()
                        .buffer()
                        .content()
                        .iter()
                        .map(|cell| cell.symbol())
                        .collect();
                    assert!(
                        text.contains("A real review workspace"),
                        "header should render"
                    );
                    assert!(text.contains("after"), "real diff should stay visible");
                }
                press(app, KeyCode::Tab);
            }
        }
    }
}

fn list_loader(_: &Path, _: bool) -> ReviewListLoaded {
    ReviewListLoaded {
        prs: Ok(vec![reviewable()]),
        current_user: Some("me".into()),
        repo_key: Some("github.com/o/r".into()),
    }
}

#[test]
fn g_opens_workspace_and_leader_u_keeps_the_original_picker() {
    let dir = TempDir::new().unwrap();
    let make_app = || {
        let mut app = App::new_for_test(
            store_with_repo(dir.path().to_path_buf(), ProjectStatus::Idle),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.selection = Selection::Project(0);
        app.pr_review_work
            .set_review_list_loader_for_test(list_loader);
        app
    };
    let mut legacy = make_app();
    legacy.activate_leader();
    press(&mut legacy, KeyCode::Char('U'));
    assert!(legacy.pr_workspace.is_none());
    assert!(matches!(legacy.mode, AppMode::PrReviewList(_)));
    let mut leader = make_app();
    leader.activate_leader();
    press(&mut leader, KeyCode::Char('G'));
    assert!(leader.pr_workspace.is_some());
    assert!(matches!(leader.mode, AppMode::PrReviewList(_)));
    let mut experimental = make_app();
    press(&mut experimental, KeyCode::Char('G'));
    assert!(experimental.pr_workspace.is_some());
    assert!(matches!(experimental.mode, AppMode::PrReviewList(_)));
    press(&mut experimental, KeyCode::Esc);
    assert!(experimental.pr_workspace.is_none());
    assert!(matches!(experimental.mode, AppMode::Normal));
}

#[test]
fn a_failed_draft_save_keeps_the_workspace_and_comments_intact() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('m'));
    crate::handlers::handle_paste(app, "Keep this draft").unwrap();
    press(app, KeyCode::Tab);
    app.db = Some(crate::db::AmfDb::open_read_only(&fixture._dir.path().join("test.db")).unwrap());
    press(app, KeyCode::Char('2'));
    press(app, KeyCode::Char('q'));
    assert!(app.pr_workspace.is_some());
    assert!(matches!(app.mode, AppMode::PrReview(_)));
    assert_eq!(
        files(app).file_comments["src/file1.rs"].text,
        "Keep this draft"
    );
    assert!(
        app.message
            .as_ref()
            .is_some_and(|message| message.contains("Couldn't save"))
    );
}

#[test]
fn closing_during_diff_refresh_saves_the_in_memory_draft() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('f'));
    crate::handlers::handle_paste(app, "Save while refreshing").unwrap();
    press(app, KeyCode::Tab);
    app.refresh_diff_viewer();
    press(app, KeyCode::Char('q'));
    assert!(app.pr_workspace.is_none());
    let draft = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_review_draft("github.com/o/r", 7)
        .unwrap()
        .unwrap();
    assert!(draft.progress.contains("Save while refreshing"));
}

#[test]
fn late_ai_result_for_another_head_cannot_replace_current_findings() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('3'));
    let AppMode::AiReview(state) = &app.mode else {
        unreachable!()
    };
    let mut old = state.clone();
    old.pr.head_sha = "previous-head".into();
    let (tx, rx) = mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(old));
    press(app, KeyCode::Char('1'));
    tx.send(AiReviewProgress::Done(Ok(AiReviewOutcome {
        findings: vec![finding(), finding()],
        summary: None,
        raw_output: String::new(),
        attribution: AiReviewAttribution::default(),
    })))
    .unwrap();
    app.poll_pr_workspace_bg();
    press(app, KeyCode::Char('3'));
    assert!(
        matches!(&app.mode, AppMode::AiReview(state) if state.findings.len() == 1 && state.pr.head_sha == "sha")
    );
}

#[test]
fn leader_g_restores_threads_after_a_fix_session_visit() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('2'));
    press(app, KeyCode::Char('j'));
    let view = view_state_for("my-project", "my-feat");
    let AppMode::PrReview(state) = std::mem::replace(&mut app.mode, AppMode::Viewing(view.clone()))
    else {
        unreachable!()
    };
    app.pr_review_return = Some(PrReviewReturn {
        session: view.session,
        window: view.window,
        state,
    });
    app.activate_leader();
    press(app, KeyCode::Char('G'));
    assert!(matches!(app.mode, AppMode::DiffViewer(_)));
    press(app, KeyCode::Char('2'));
    assert!(matches!(&app.mode, AppMode::PrReview(state) if state.selected == 1));
    assert!(app.pr_review_return.is_none());
}

#[test]
fn refreshed_feedback_at_a_new_head_does_not_highlight_old_code() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('2'));
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.pr.head_sha = "new-head".into();
    }
    app.sync_pr_workspace_anchor();
    assert!(
        app.pr_workspace
            .as_ref()
            .unwrap()
            .anchor_message
            .as_ref()
            .is_some_and(|message| message.contains("different PR head"))
    );
    assert!(files(app).comment_cursor.is_none());
}

#[test]
fn an_unrelated_legacy_diff_is_not_wrapped_in_the_old_workspace() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('2'));
    let unrelated = DiffViewerState::new(
        view_state_for("other", "feature"),
        fixture._dir.path().to_path_buf(),
    );
    app.mode = AppMode::DiffViewer(unrelated);
    app.reconcile_pr_workspace();
    assert!(app.pr_workspace.is_none());
    assert!(
        matches!(&app.mode, AppMode::DiffViewer(state) if matches!(state.scope, DiffScope::CurrentChanges))
    );
}

#[test]
fn ai_fix_handoff_preserves_the_ai_view_and_reuses_the_existing_triage() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    app.store = store_with_repo(fixture._dir.path().to_path_buf(), ProjectStatus::Idle);
    app.store.available_harnesses = vec![crate::project::AgentKind::Claude];
    let root = fixture._dir.path().to_path_buf();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(root.clone()));
    app.worktree = std::sync::Arc::new(worktree);
    app.selection = Selection::Feature(0, 0);
    press(app, KeyCode::Char('3'));
    press(app, KeyCode::Char('f'));
    assert!(matches!(&app.mode, AppMode::PrReview(state) if state.review.comments.len() == 4));
    for _ in 0..4 {
        if !crate::app::pr_workspace::engine_modal(&app.mode) {
            break;
        }
        press(app, KeyCode::Esc);
    }
    press(app, KeyCode::Char('3'));
    assert!(
        matches!(&app.mode, AppMode::AiReview(state) if state.findings.len() == 1 && !state.findings[0].published)
    );
    press(app, KeyCode::Char('2'));
    assert!(matches!(&app.mode, AppMode::PrReview(state) if state.review.comments.len() == 4));
}

fn feature_pr_list(_: &Path, _: bool) -> ReviewListLoaded {
    let mut unrelated = reviewable();
    unrelated.number = 8;
    unrelated.head_ref = "another-feature".into();
    let mut current = reviewable();
    current.head_ref = "my-feat".into();
    ReviewListLoaded {
        prs: Ok(vec![unrelated, current]),
        current_user: None,
        repo_key: None,
    }
}

fn ambiguous_feature_pr_list(workdir: &Path, resolve_user: bool) -> ReviewListLoaded {
    let mut loaded = feature_pr_list(workdir, resolve_user);
    if let Ok(prs) = &mut loaded.prs {
        prs[0].head_ref = "my-feat".into();
        prs[0].is_cross_repository = true;
        prs[0].head_owner = "someone-else".into();
    }
    loaded
}

fn settle_workspace_picker(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while app.pr_review_work.review_list_pending() {
        app.poll_pr_review_list_bg();
        assert!(Instant::now() < deadline, "PR picker did not load");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn workspace_picker(app: &App) -> &PrReviewListState {
    let AppMode::PrReviewList(state) = &app.mode else {
        panic!("expected the workspace PR picker");
    };
    state
}

#[test]
fn picker_selects_the_feature_pr_and_keeps_its_badge_after_browsing_and_reload() {
    let dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(dir.path().to_path_buf(), ProjectStatus::Idle),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.pr_review_work
        .set_review_list_loader_for_test(feature_pr_list);
    press(&mut app, KeyCode::Char('G'));
    settle_workspace_picker(&mut app);
    assert_eq!(workspace_picker(&app).selected, 1);
    assert_eq!(workspace_picker(&app).feature_pr_number(), Some(7));

    press(&mut app, KeyCode::Char('k'));
    assert_eq!(workspace_picker(&app).selected, 0);
    let mut terminal = Terminal::new(TestBackend::new(120, 18)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let rows: Vec<String> = terminal
        .backend()
        .buffer()
        .content()
        .chunks(120)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect())
        .collect();
    let marked = rows
        .iter()
        .find(|row| row.contains("current feature"))
        .unwrap();
    assert!(marked.contains("#7"));
    assert!(!marked.contains("#8"));

    press(&mut app, KeyCode::Char('r'));
    settle_workspace_picker(&mut app);
    assert_eq!(
        workspace_picker(&app).selected,
        0,
        "reload must retain the reader's selection"
    );
    assert_eq!(workspace_picker(&app).feature_pr_number(), Some(7));
}

#[test]
fn session_picker_uses_the_viewed_feature_instead_of_dashboard_selection() {
    let dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(dir.path().to_path_buf(), ProjectStatus::Idle),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Project(0);
    app.mode = AppMode::Viewing(view_state_for("my-project", "my-feat"));
    app.pr_review_work
        .set_review_list_loader_for_test(feature_pr_list);
    app.activate_leader();
    press(&mut app, KeyCode::Char('G'));
    settle_workspace_picker(&mut app);
    assert_eq!(
        workspace_picker(&app).feature_branch.as_deref(),
        Some("my-feat")
    );
    assert_eq!(workspace_picker(&app).selected, 1);
}

#[test]
fn picker_does_not_mark_a_project_row_or_a_missing_or_ambiguous_feature_pr() {
    let dir = TempDir::new().unwrap();
    for (selection, branch, ambiguous) in [
        (Selection::Project(0), "my-feat", false),
        (Selection::Feature(0, 0), "missing", false),
        (Selection::Feature(0, 0), "MY-FEAT", false),
        (Selection::Feature(0, 0), "my-feat", true),
    ] {
        let mut app = App::new_for_test(
            store_with_repo(dir.path().to_path_buf(), ProjectStatus::Idle),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.store.projects[0].features[0].branch = branch.into();
        app.selection = selection;
        app.pr_review_work
            .set_review_list_loader_for_test(if ambiguous {
                ambiguous_feature_pr_list
            } else {
                feature_pr_list
            });
        press(&mut app, KeyCode::Char('G'));
        settle_workspace_picker(&mut app);
        assert_eq!(workspace_picker(&app).selected, 0);
        assert_eq!(workspace_picker(&app).feature_pr_number(), None);
    }
}

fn rendered_navigator(app: &mut App, width: u16) -> (String, String) {
    let mut terminal = Terminal::new(TestBackend::new(width, 30)).unwrap();
    terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
    let workspace = app.pr_workspace.as_ref().unwrap();
    let (_, area) = crate::ui::pr_workspace_pane_areas(
        ratatui::layout::Rect::new(0, 0, width, 30),
        workspace.focus,
        workspace.expanded,
    )
    .into_iter()
    .find(|(pane, _)| *pane == WorkspacePane::Navigator)
    .unwrap();
    let buffer = terminal.backend().buffer();
    let mut text = String::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            text.push_str(buffer[(x, y)].symbol());
        }
        text.push('\n');
    }
    (text, buffer[(area.x + 1, area.y + 1)].symbol().to_owned())
}

#[test]
fn ai_running_indicator_animates_in_the_navigator_and_clears_when_the_run_finishes() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('3'));
    let AppMode::AiReview(state) = &mut app.mode else {
        unreachable!();
    };
    state.findings.clear();
    let origin = state.clone();
    let (tx, rx) = mpsc::channel();
    app.ai_review_run.begin(rx, origin);
    app.ai_review_run.show_progress(AiReviewRunProgress {
        stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
        started_at: Instant::now() - Duration::from_secs(65),
        activity: None,
        usage: None,
    });
    app.throbber_state.calc_next();
    for key in ['1', '2', '3'] {
        press(app, KeyCode::Char(key));
        assert!(
            app.has_visible_animation(),
            "spinner must tick in every category"
        );
        for width in [160, 100, 60] {
            let (text, first_frame) = rendered_navigator(app, width);
            assert!(text.contains("AI review running"), "{text}");
            assert!(text.contains("Fetching diff"), "{text}");
            assert!(text.contains("1m "), "{text}");
            app.throbber_state.calc_next();
            let (_, next_frame) = rendered_navigator(app, width);
            assert_ne!(first_frame, next_frame, "spinner did not advance");
        }
    }
    tx.send(AiReviewProgress::Reviewing {
        token_estimate: 100,
    })
    .unwrap();
    app.poll_pr_workspace_bg();
    assert!(rendered_navigator(app, 160).0.contains("Reviewing code"));
    press(app, KeyCode::Char('1'));
    tx.send(AiReviewProgress::Done(Err(anyhow::anyhow!(
        "offline run failed"
    ))))
    .unwrap();
    app.poll_pr_workspace_bg();
    assert!(!app.ai_review_run.is_pending());
    assert!(!rendered_navigator(app, 160).0.contains("AI review running"));
    assert!(matches!(app.mode, AppMode::DiffViewer(_)));
}

#[test]
fn navigator_does_not_show_an_ai_run_for_a_different_pr_or_head() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('3'));
    let AppMode::AiReview(state) = &app.mode else {
        unreachable!();
    };
    let origin = state.clone();
    let (_tx, rx) = mpsc::channel();
    app.ai_review_run.begin(rx, origin.clone());
    press(app, KeyCode::Char('1'));
    for another_head in [false, true] {
        let mut unrelated = origin.clone();
        if another_head {
            unrelated.pr.head_sha = "another-head".into();
        } else {
            unrelated.pr.number += 1;
        }
        app.ai_review_run.set_origin_for_test(Some(unrelated));
        assert!(!app.has_visible_animation());
        assert!(!rendered_navigator(app, 160).0.contains("AI review running"));
    }
}

#[test]
fn cancel_button_and_shortcut_stop_the_run_and_preserve_existing_review_work() {
    for click in [false, true] {
        let mut fixture = Fixture::new();
        let app = &mut fixture.app;
        app.viewport_cols = 160;
        app.viewport_total_rows = 30;
        press(app, KeyCode::Char('m'));
        crate::handlers::handle_paste(app, "Keep this human draft").unwrap();
        press(app, KeyCode::Tab);
        press(app, KeyCode::Char('3'));
        let AppMode::AiReview(origin) = &app.mode else {
            unreachable!();
        };
        let origin = origin.clone();
        let (tx, rx) = mpsc::channel();
        let cancelled = app.ai_review_run.begin(rx, origin.clone());
        let progress = AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: Instant::now(),
            activity: None,
            usage: None,
        };
        app.ai_review_run.show_progress(progress.clone());
        app.mode = AppMode::AiReviewRunning(AiReviewRunState { origin, progress });
        press(app, KeyCode::Char('1'));
        assert!(rendered_navigator(app, 160).0.contains("[Ctrl+X Cancel]"));
        if click {
            let button = crate::ui::pr_workspace_cancel_button_area(
                ratatui::layout::Rect::new(0, 0, 160, 30),
                WorkspacePane::Navigator,
                false,
            )
            .unwrap();
            crate::handlers::handle_mouse(
                app,
                crossterm::event::MouseEvent {
                    kind: crossterm::event::MouseEventKind::Down(
                        crossterm::event::MouseButton::Left,
                    ),
                    column: button.x + 2,
                    row: button.y,
                    modifiers: KeyModifiers::NONE,
                },
                30,
            )
            .unwrap();
        } else {
            crate::handlers::handle_key(
                app,
                KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL),
                30,
            )
            .unwrap();
        }
        assert!(cancelled.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!app.ai_review_run.is_pending());
        assert!(matches!(app.mode, AppMode::DiffViewer(_)));
        assert_eq!(
            files(app).file_comments["src/file1.rs"].text,
            "Keep this human draft"
        );
        assert!(!rendered_navigator(app, 160).0.contains("[Ctrl+X Cancel]"));
        assert!(
            tx.send(AiReviewProgress::Done(Ok(AiReviewOutcome {
                findings: vec![],
                summary: None,
                raw_output: String::new(),
                attribution: Default::default(),
            })))
            .is_err(),
            "cancelled worker must not be able to deliver a late result"
        );
        press(app, KeyCode::Char('3'));
        assert!(
            matches!(&app.mode, AppMode::AiReview(state) if state.findings.len() == 1 && state.last_run.is_none())
        );
        let AppMode::AiReview(origin) = &app.mode else {
            unreachable!();
        };
        let (next_tx, next_rx) = mpsc::channel();
        let next_token = app.ai_review_run.begin(next_rx, origin.clone());
        assert!(!next_token.load(std::sync::atomic::Ordering::Relaxed));
        app.cancel_workspace_ai_review();
        drop(next_tx);
    }
}

#[test]
fn z_and_enter_fold_a_files_folder_but_keep_shell_meanings_on_file_rows() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    if let AppMode::DiffViewer(files) = &mut app.mode {
        files.tree_cursor_dir = Some("src".into());
    }
    press(app, KeyCode::Char('z'));
    assert!(files(app).collapsed_dirs.contains("src"));
    assert!(!app.pr_workspace.as_ref().unwrap().expanded);
    press(app, KeyCode::Enter);
    assert!(!files(app).collapsed_dirs.contains("src"));
    assert_eq!(
        app.pr_workspace.as_ref().unwrap().focus,
        WorkspacePane::Navigator
    );
    if let AppMode::DiffViewer(files) = &mut app.mode {
        files.tree_cursor_dir = None;
    }
    press(app, KeyCode::Char('z'));
    assert!(app.pr_workspace.as_ref().unwrap().expanded);
    press(app, KeyCode::Esc);
    press(app, KeyCode::Enter);
    assert_eq!(
        app.pr_workspace.as_ref().unwrap().focus,
        WorkspacePane::Diff
    );
}

#[test]
fn submitting_from_an_outdated_thread_clears_its_code_placeholder() {
    let mut fixture = Fixture::new();
    let app = &mut fixture.app;
    press(app, KeyCode::Char('2'));
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[1].outdated = true;
    }
    press(app, KeyCode::Char('j'));
    assert!(app.pr_workspace.as_ref().unwrap().anchor_message.is_some());
    press(app, KeyCode::Char('p'));
    assert!(files(app).pr_submit.is_some());
    press(app, KeyCode::Esc);
    let workspace = app.pr_workspace.as_ref().unwrap();
    assert_eq!(workspace.tab, WorkspaceTab::Files);
    assert!(workspace.anchor_message.is_none());
    press(app, KeyCode::Tab);
    app.message = None;
    press(app, KeyCode::Char('j'));
    assert!(
        app.message
            .as_ref()
            .is_none_or(|message| !message.contains("Press 1 to browse code")),
        "Diff pane still swallows keys: {:?}",
        app.message
    );
}
