use super::{
    PATCH_JUMP_STEP, handle_diff_viewer_key, handle_diff_viewer_wheel,
    handle_review_harness_pick_key,
};
use crate::app::App;
use crate::app::{
    AppMode, DiffViewerFocus, DiffViewerLayout, DiffViewerState, FileFilter, ViewState,
};
use crate::diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind};
use crate::project::{
    AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// Finish a fully-decided review: the first `q` opens the pre-finish
/// summary, the second (`q` again, now handled by the summary modal)
/// actually writes/dispatches. Tests that only care about the end state
/// use this instead of a single `q` press.
fn finish_review(app: &mut App) {
    handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
    assert!(
        matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open),
        "expected the pre-finish summary to open"
    );
    handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
}

fn make_review_app(workdir: &Path, paths: &[&str]) -> App {
    let mut app = crate::app::App::new_for_test(
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
    let mut state = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            crate::project::SessionKind::Claude,
            VibeMode::Vibeless,
            true,
        ),
        workdir.to_path_buf(),
    );
    state.review = true;
    state.files = paths
        .iter()
        .map(|p| DiffFile {
            old_path: Some((*p).to_string()),
            path: (*p).to_string(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        })
        .collect();
    app.mode = AppMode::DiffViewer(state);
    app
}

/// A review app whose single file carries real hunks parsed from `patch`, so
/// `addressable_lines()` is non-empty (needed to exercise the line cursor and
/// diff search).
fn make_review_app_with_patch(workdir: &Path, patch: &str) -> App {
    let mut app = make_review_app(workdir, &["a.rs"]);
    let files = crate::diff::parse_unified_diff(patch).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = files;
    }
    app
}

/// Matches `SEARCH_PATCH` in the review.rs tests: addressable lines are
/// 0 alpha, 1 beta, 2 beta_two, 3 gamma_alpha, 4 delta.
const SEARCH_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
";

/// A single-file review app with `lines` added lines, focused on the patch.
fn make_long_patch_app(workdir: &Path, lines: usize) -> App {
    let mut patch =
        format!("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -0,0 +1,{lines} @@\n");
    for i in 0..lines {
        patch.push_str(&format!("+line {i}\n"));
    }
    let mut app = make_review_app_with_patch(workdir, &patch);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.focus = DiffViewerFocus::Patch;
    }
    app
}

fn patch_scroll_and_cursor(app: &App) -> (usize, Option<usize>) {
    match &app.mode {
        AppMode::DiffViewer(state) => (state.patch_scroll, state.comment_cursor),
        _ => unreachable!(),
    }
}

fn help_open(app: &App) -> bool {
    matches!(&app.mode, AppMode::DiffViewer(state) if state.help_open)
}

#[test]
fn history_navigation_loads_archive_only_after_live_rounds() {
    let dir = tempfile::TempDir::new().unwrap();
    let claude_dir = dir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("final-review-feedback.md"),
        "\
# Final Review Feedback

## Review — r3

newest

## Review — r2

middle
",
    )
    .unwrap();
    std::fs::write(
        claude_dir.join("final-review-feedback-archive.md"),
        "\
# Final Review Feedback Archive

## Review — r1

oldest
",
    )
    .unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('H'))).unwrap();
    let history = match &app.mode {
        AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
        _ => panic!("not in review"),
    };
    assert_eq!(history.rounds.len(), 2);
    assert_eq!(history.selected, 0);
    assert!(!history.archive_loaded);

    // Current -> newest live -> older live does not touch the archive.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Right)).unwrap();
    let history = match &app.mode {
        AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
        _ => panic!("not in review"),
    };
    assert_eq!(history.selected, 2);
    assert!(!history.archive_loaded);

    // Crossing the loaded tail lazily appends the archive newest-first.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
    let history = match &app.mode {
        AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
        _ => panic!("not in review"),
    };
    assert_eq!(history.selected, 3);
    assert!(history.archive_loaded);
    assert_eq!(history.rounds.len(), 3);
    assert_eq!(history.rounds[2].title, "Review — r1");

    // History owns the keys while open: a verdict binding must not leak to
    // the underlying review.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(state) if state.decisions.is_empty()));

    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(state) if state.review_history.is_none()));
}

#[test]
fn approving_all_files_records_history_without_dispatching_feedback() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a.rs -> b.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve b.rs
    finish_review(&mut app);

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    let history =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(history.contains("**Files reviewed:** 2"));
    assert!(history.contains("**Approved:** 2"));
    assert!(history.contains("**Needs work:** 0"));
    assert!(!history.contains("### Files Needing Revision"));
}

#[test]
fn rejecting_with_feedback_writes_feedback_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // Reject a.rs with feedback "fix it".
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "fix it".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // submit -> advance to b.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve b.rs
    finish_review(&mut app);

    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("### a.rs"));
    assert!(feedback.contains("fix it"));
    assert!(feedback.contains("**Approved:** 1"));
    assert!(feedback.contains("**Needs work:** 1"));
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

/// The index of the first `Dedicated(..)` row in the open destination
/// picker.
fn dedicated_row_index(app: &App) -> usize {
    match &app.mode {
        AppMode::DiffViewer(s) => s
            .destination_pick
            .as_ref()
            .expect("picker open")
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::ReviewDestinationRow::Dedicated(_)))
            .expect("a dedicated row"),
        _ => panic!("not in review"),
    }
}

#[test]
fn t_opens_the_destination_picker_and_dedicated_sets_the_target() {
    use crate::app::pr_review::FixTarget;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // Default target is this feature's live session, picker closed.
    assert!(matches!(&app.mode, AppMode::DiffViewer(s)
            if s.fix_target == FixTarget::ExistingLive && s.destination_pick.is_none()));

    // `t` opens the picker rather than toggling in place.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.destination_pick.is_some()));

    // Walk to the dedicated row and choose it.
    let dedicated = dedicated_row_index(&app);
    for _ in 0..dedicated {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(s) => {
            assert_eq!(s.fix_target, FixTarget::DedicatedReview);
            assert!(
                s.review_harness.is_some(),
                "dedicated resolves a harness up front"
            );
            assert!(s.destination_pick.is_none(), "confirming closes the picker");
        }
        _ => panic!("not in review"),
    }

    // Re-open: it pre-highlights the dedicated row it now points at. Walk
    // back up to the first row and pick it → this feature's live session.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
    assert_eq!(
        match &app.mode {
            AppMode::DiffViewer(s) => s.destination_pick.as_ref().unwrap().selected,
            _ => panic!("not in review"),
        },
        dedicated,
        "re-opening pre-highlights the current destination"
    );
    for _ in 0..dedicated {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(s) => {
            assert_eq!(s.fix_target, FixTarget::ExistingLive);
            assert!(s.review_harness.is_none());
        }
        _ => panic!("not in review"),
    }
}

#[test]
fn destination_picker_cancels_without_changing_the_target() {
    use crate::app::pr_review::FixTarget;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(s) => {
            assert!(s.destination_pick.is_none());
            assert_eq!(s.fix_target, FixTarget::ExistingLive);
        }
        _ => panic!("not in review"),
    }
}

#[test]
fn dedicated_target_with_no_session_opens_harness_pick() {
    use crate::app::pr_review::FixTarget;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    // The dispatch resolves the feature by the view's project/feature names,
    // so the store must hold a matching feature with no agent session yet.
    let mut project = Project::new(
        "proj".into(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    project.features.push(Feature::new(
        "feat".into(),
        "branch".into(),
        dir.path().to_path_buf(),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    ));
    app.store.projects.push(project);
    app.store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];

    // Choose the dedicated target via the picker, reject the file, then finish.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
    let dedicated = dedicated_row_index(&app);
    for _ in 0..dedicated {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    if let AppMode::DiffViewer(s) = &app.mode {
        assert_eq!(s.fix_target, FixTarget::DedicatedReview);
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "fix it".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // submit rejection
    finish_review(&mut app);

    // The feedback file is written and, because the dedicated session must be
    // spun up, the harness picker is shown rather than returning to the view.
    assert!(dir.path().join(".claude/final-review-feedback.md").exists());
    match &app.mode {
        AppMode::ReviewHarnessPick(state) => {
            assert_eq!(state.harnesses, vec![AgentKind::Claude, AgentKind::Codex]);
        }
        _ => panic!("expected harness pick mode after finishing"),
    }

    // Cancelling keeps the feedback and returns to the feature view.
    handle_review_harness_pick_key(&mut app, KeyCode::Esc).unwrap();
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

#[test]
fn general_feedback_is_written_even_when_all_approved() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // f opens the general-feedback editor; type, submit, approve, finish.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
    for c in "tighten error handling".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // save general note
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.editing_general);
            assert_eq!(state.selected_file, 0, "general feedback must not advance");
            assert_eq!(state.general_feedback, "tighten error handling");
        }
        _ => panic!("expected diff viewer"),
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a.rs
    finish_review(&mut app);

    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("## General Feedback"));
    assert!(feedback.contains("tighten error handling"));
    assert!(!feedback.contains("## Files Needing Revision"));
}

#[test]
fn destination_picker_lists_other_features_and_routes_to_the_one_picked() {
    use crate::app::ReviewDestinationRow;
    use crate::app::pr_review::FixTarget;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // The reviewed feature ("proj"/"feat") plus a second, unrelated feature.
    let mut project = Project::new(
        "proj".into(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    project.features.push(Feature::new(
        "feat".into(),
        "feat".into(),
        dir.path().to_path_buf(),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    ));
    let mut other = Feature::new(
        "other".into(),
        "other".into(),
        dir.path().to_path_buf(),
        true,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    other.add_session(SessionKind::Claude);
    let other_id = other.id.clone();
    project.features.push(other);
    app.store.projects.push(project);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
    let other_row = match &app.mode {
            AppMode::DiffViewer(s) => s
                .destination_pick
                .as_ref()
                .unwrap()
                .rows
                .iter()
                .position(|r| matches!(r,
                    ReviewDestinationRow::ExistingFeature { feature_id, .. } if *feature_id == other_id))
                .expect("an ExistingFeature row for the other feature"),
            _ => panic!("not in review"),
        };
    for _ in 0..other_row {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(s) => {
            assert_eq!(s.fix_target, FixTarget::ExistingFeature);
            assert_eq!(s.fix_target_feature_id.as_deref(), Some(other_id.as_str()));
            assert!(s.destination_pick.is_none());
        }
        _ => panic!("not in review"),
    }
}

#[test]
fn new_feature_row_opens_the_companion_setup_and_esc_closes_it() {
    use crate::app::ReviewDestinationRow;
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    app.store.available_harnesses = vec![AgentKind::Claude];
    // The companion setup seeds from the reviewed feature's branch, so it
    // has to resolve from the store by workdir.
    let mut project = Project::new(
        "proj".into(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    project.features.push(Feature::new(
        "feat".into(),
        "feat".into(),
        dir.path().to_path_buf(),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    ));
    app.store.projects.push(project);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
    let new_row = match &app.mode {
        AppMode::DiffViewer(s) => s
            .destination_pick
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .position(|r| matches!(r, ReviewDestinationRow::NewFeature))
            .expect("a NewFeature row"),
        _ => panic!("not in review"),
    };
    for _ in 0..new_row {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s)
            if s.review_feature_setup.is_some() && s.destination_pick.is_none()));

    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s)
            if s.review_feature_setup.is_none() && s.destination_pick.is_none()));
}

#[test]
fn finishing_with_an_existing_feature_target_prompts_that_features_agent() {
    let dir = tempfile::TempDir::new().unwrap();

    // The reviewed feature.
    let reviewed = Feature::new(
        "feature".to_string(),
        "feature".to_string(),
        dir.path().to_path_buf(),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    // A separate feature whose agent session must receive the prompt.
    let mut other = Feature::new(
        "other".to_string(),
        "other".to_string(),
        dir.path().to_path_buf(),
        true,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    other.status = ProjectStatus::Active;
    let other_session = other.add_session(SessionKind::Claude).clone();
    let other_tmux = other.tmux_session.clone();
    let other_window = other_session.tmux_window.clone();
    let other_id = other.id.clone();

    let mut project = Project::new(
        "demo".to_string(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    project.features.push(reviewed);
    project.features.push(other);

    let mut tmux = MockTmuxOps::new();
    let (ps, pw) = (other_tmux.clone(), other_window.clone());
    tmux.expect_paste_text()
        .withf(move |session, window, text| {
            session == ps && window == pw && text.contains("final-review-feedback.md")
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    let (ks, kw) = (other_tmux.clone(), other_window.clone());
    tmux.expect_send_key_name()
        .withf(move |session, window, name| session == ks && window == kw && name == "Enter")
        .times(1)
        .returning(|_, _, _| Ok(()));

    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));

    let mut state = DiffViewerState::new(
        ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            true,
        ),
        dir.path().to_path_buf(),
    );
    state.review = true;
    state.fix_target = crate::app::pr_review::FixTarget::ExistingFeature;
    state.fix_target_feature_id = Some(other_id);
    state.files = vec![DiffFile {
        old_path: Some("a.rs".into()),
        path: "a.rs".into(),
        status: DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: vec![],
    }];
    app.mode = AppMode::DiffViewer(state);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "fix".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    finish_review(&mut app);

    assert!(dir.path().join(".claude/final-review-feedback.md").exists());
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    // The .times(1) paste/enter expectations are verified on drop: they
    // targeted the *other* feature's session, not the reviewed one.
}

#[test]
fn finishing_new_feature_with_a_gone_companion_never_touches_the_reviewed_feature() {
    let dir = tempfile::TempDir::new().unwrap();

    // Only the reviewed feature exists — the companion the picker created
    // has been deleted (its id no longer resolves).
    let reviewed = Feature::new(
        "feature".to_string(),
        "feature".to_string(),
        dir.path().to_path_buf(),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    let mut project = Project::new(
        "demo".to_string(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    project.features.push(reviewed);

    // A bare mock: any tmux call (a paste, or spinning up a session) panics.
    let tmux = MockTmuxOps::new();

    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));

    let mut state = DiffViewerState::new(
        ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            true,
        ),
        dir.path().to_path_buf(),
    );
    state.review = true;
    state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    state.fix_target_feature_id = Some("companion-that-was-deleted".to_string());
    state.review_harness = Some(AgentKind::Claude);
    state.files = vec![DiffFile {
        old_path: Some("a.rs".into()),
        path: "a.rs".into(),
        status: DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: vec![],
    }];
    app.mode = AppMode::DiffViewer(state);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "fix".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    finish_review(&mut app);

    // Feedback is still saved and the viewer returns to the feature view…
    assert!(dir.path().join(".claude/final-review-feedback.md").exists());
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("companion review feature is gone")
    );
    // …but no "Final Review" session was created inside the reviewed feature.
    assert!(
        app.store.projects[0].features[0]
            .sessions
            .iter()
            .all(|s| s.label != crate::app::review::FINAL_REVIEW_SESSION_LABEL)
    );
}

#[test]
fn finishing_with_feedback_prompts_the_agent() {
    let dir = tempfile::TempDir::new().unwrap();

    let mut feature = Feature::new(
        "feature".to_string(),
        "feature".to_string(),
        dir.path().to_path_buf(),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    feature.status = ProjectStatus::Active;
    let session = feature.add_session(SessionKind::Claude).clone();
    let agent_session = feature.tmux_session.clone();
    let agent_window = session.tmux_window.clone();

    let mut project = Project::new(
        "demo".to_string(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    project.features.push(feature);

    // The agent must be pasted the feedback prompt and have Enter sent.
    let mut tmux = MockTmuxOps::new();
    let (ps, pw) = (agent_session.clone(), agent_window.clone());
    tmux.expect_paste_text()
        .withf(move |session, window, text| {
            session == ps && window == pw && text.contains("final-review-feedback.md")
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    let (ks, kw) = (agent_session.clone(), agent_window.clone());
    tmux.expect_send_key_name()
        .withf(move |session, window, name| session == ks && window == kw && name == "Enter")
        .times(1)
        .returning(|_, _, _| Ok(()));

    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));

    let mut state = DiffViewerState::new(
        ViewState::new(
            "demo".into(),
            "feature".into(),
            agent_session.clone(),
            agent_window.clone(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            true,
        ),
        dir.path().to_path_buf(),
    );
    state.review = true;
    state.files = vec![DiffFile {
        old_path: Some("a.rs".into()),
        path: "a.rs".into(),
        status: DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: vec![],
    }];
    app.mode = AppMode::DiffViewer(state);

    // Reject the file with feedback, then finish.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "fix".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    finish_review(&mut app);

    assert!(dir.path().join(".claude/final-review-feedback.md").exists());
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    // Mock .times(1) expectations are verified when `app` (and its tmux) drop.
}

#[test]
fn paste_without_submit_does_not_send_enter() {
    let dir = tempfile::TempDir::new().unwrap();

    let mut feature = Feature::new(
        "feature".to_string(),
        "feature".to_string(),
        dir.path().to_path_buf(),
        false,
        VibeMode::Vibeless,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    feature.status = ProjectStatus::Active;
    let session = feature.add_session(SessionKind::Claude).clone();
    let agent_session = feature.tmux_session.clone();
    let agent_window = session.tmux_window.clone();

    let mut project = Project::new(
        "demo".to_string(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    project.features.push(feature);

    // The prompt is pasted but Enter must NOT be sent: no send_key_name
    // expectation is registered, so mockall fails if it is called.
    let mut tmux = MockTmuxOps::new();
    let (ps, pw) = (agent_session.clone(), agent_window.clone());
    tmux.expect_paste_text()
        .withf(move |session, window, text| {
            session == ps && window == pw && text.contains("final-review-feedback.md")
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.config.final_review_submit_prompt = false;

    let mut state = DiffViewerState::new(
        ViewState::new(
            "demo".into(),
            "feature".into(),
            agent_session.clone(),
            agent_window.clone(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            true,
        ),
        dir.path().to_path_buf(),
    );
    state.review = true;
    state.files = vec![DiffFile {
        old_path: Some("a.rs".into()),
        path: "a.rs".into(),
        status: DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: vec![],
    }];
    app.mode = AppMode::DiffViewer(state);

    // Reject the file with feedback, then finish.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    for c in "fix".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    finish_review(&mut app);

    assert!(dir.path().join(".claude/final-review-feedback.md").exists());
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("not submitted"))
    );
}

#[test]
fn reject_then_escape_cancels_feedback_without_recording() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('x'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap(); // cancel feedback editor

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.feedback_editing);
            assert!(state.decisions.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

/// Give the review app's single file a real hunk (one context + one added
/// line) so it has addressable lines to put a cursor on.
fn set_single_hunk(app: &mut App) {
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![DiffHunk {
            header: "@@ -1,1 +1,2 @@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " ctx".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+added line".into(),
                },
            ],
        }];
    }
}

fn run_git(repo: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
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

#[test]
fn base_ref_prompt_opens_types_and_submits_into_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // `b` opens the prompt; verdict keys are suppressed while it is open.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
    assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.editing_base_ref));

    for c in "develop".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Backspace)).unwrap(); // -> "develo"

    // Enter submits: the override is recorded and the viewer re-enters
    // loading so the diff reloads against it.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    match &app.mode {
        AppMode::DiffViewerLoading(s) => {
            assert_eq!(s.override_base_ref.as_deref(), Some("develo"));
            assert!(!s.editing_base_ref);
        }
        _ => panic!("expected loading state after submit"),
    }
}

#[test]
fn base_ref_prompt_escape_cancels_without_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
    for c in "main".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(s) => {
            assert!(!s.editing_base_ref);
            assert!(s.override_base_ref.is_none());
            assert!(s.base_ref_input.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn base_ref_prompt_blank_submit_clears_override() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.override_base_ref = Some("develop".into());
    }

    // Open (pre-fills with the current override), clear it, submit blank.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
    for _ in 0.."develop".len() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Backspace)).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::DiffViewerLoading(s) => assert!(s.override_base_ref.is_none()),
        _ => panic!("expected loading state after submit"),
    }
}

/// Give the review app's single file two hunks (context + added line each),
/// so addressable indices 0,1 belong to the first hunk and 2,3 to the
/// second. Used by the jump-by-hunk tests.
fn set_two_hunks(app: &mut App) {
    let make_hunk = |old_start: usize, new_start: usize| DiffHunk {
        header: format!("@@ -{old_start},1 +{new_start},2 @@"),
        old_start,
        old_lines: 1,
        new_start,
        new_lines: 2,
        lines: vec![
            DiffLine {
                kind: DiffLineKind::Context,
                text: " ctx".into(),
            },
            DiffLine {
                kind: DiffLineKind::Added,
                text: "+added".into(),
            },
        ],
    };
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![make_hunk(1, 1), make_hunk(10, 11)];
    }
}

#[test]
fn bracket_keys_jump_cursor_between_hunks() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_two_hunks(&mut app);

    let cursor = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.comment_cursor,
        _ => panic!("expected diff viewer"),
    };

    // `]` with the cursor off activates it on the first hunk's start.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
    assert_eq!(cursor(&app), Some(0));
    // `]` hops to the second hunk, then clamps there (no hunk after).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
    assert_eq!(cursor(&app), Some(2));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
    assert_eq!(cursor(&app), Some(2), "clamps in the last hunk");

    // From mid-hunk, `[` first snaps back to the current hunk's start…
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    assert_eq!(cursor(&app), Some(3));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
    assert_eq!(cursor(&app), Some(2));
    // …then to the previous hunk, clamping at the first.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
    assert_eq!(cursor(&app), Some(0));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
    assert_eq!(cursor(&app), Some(0), "clamps at the first hunk");
}

#[test]
fn line_comment_auto_rejects_file_and_removal_clears_it() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Storing a comment defaults the undecided file to "needs revision"
    // with empty feedback (the comment carries the specifics).
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "bug".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("a.rs"),
                Some(&crate::app::ReviewDecision::Reject {
                    feedback: String::new(),
                    severity: crate::app::Severity::Suggestion,
                })
            );
            assert!(state.auto_rejected.contains("a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }

    // Deleting the file's only comment (empty re-submit) clears the
    // auto-set verdict again.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new(String::new());
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty());
            assert!(state.auto_rejected.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn explicit_verdict_wins_over_comment_auto_reject() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Comment (auto-rejects), then explicitly approve: the approval wins.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "note".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    let decision = |app: &App| match &app.mode {
        AppMode::DiffViewer(s) => s.decisions.get("a.rs").cloned(),
        _ => panic!("expected diff viewer"),
    };
    assert_eq!(decision(&app), Some(crate::app::ReviewDecision::Approve));

    // Editing the comment afterwards does not re-default the verdict.
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in " more".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert_eq!(decision(&app), Some(crate::app::ReviewDecision::Approve));
}

#[test]
fn skip_after_comment_clears_implicit_reject() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "note".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('s'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty(), "skip clears the verdict");
            assert!(state.auto_rejected.is_empty());
            // The comment itself is kept — only the verdict was cleared.
            assert!(
                state
                    .line_comments
                    .get("a.rs")
                    .is_some_and(|c| c.len() == 1)
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

/// Seed an unaccepted AI draft comment on the review app's added line
/// (addressable index 1, new line 2 — where `c` places the cursor).
fn seed_draft_comment(app: &mut App) {
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.line_comments.insert(
            "a.rs".into(),
            vec![crate::app::LineComment {
                location: crate::diff::DiffLineLocation {
                    old_line: None,
                    new_line: Some(2),
                },
                start: None,
                text: "AI finding".into(),
                draft: true,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
    }
}

#[test]
fn ctrl_e_cycles_line_comment_severity() {
    use crate::app::Severity;
    let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Open the line-comment editor (defaults to Suggestion), cycle once with
    // Ctrl+E to Nit, type, and submit.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    handle_diff_viewer_key(&mut app, ctrl('e')).unwrap();
    for c in "typo".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            let comment = &state.line_comments.get("a.rs").unwrap()[0];
            assert_eq!(comment.severity, Severity::Nit);
            // A blocker line comment would drive the Blockers filter; a nit
            // does not.
            assert!(!state.file_has_blocker("a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn accepted_draft_auto_rejects_dismissed_draft_does_not() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    // Dismissing a draft leaves the file undecided (a draft was never a
    // kept comment).
    seed_draft_comment(&mut app);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('d'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }

    // Accepting a draft is a human-affirmed finding: it counts like a
    // hand-written comment and defaults the verdict.
    seed_draft_comment(&mut app);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("a.rs"),
                Some(&crate::app::ReviewDecision::Reject {
                    feedback: String::new(),
                    severity: crate::app::Severity::Suggestion,
                })
            );
            assert!(state.auto_rejected.contains("a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn implicit_reject_points_agent_at_line_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);
    set_single_hunk(&mut app);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
    for c in "bug".chars() {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
    }
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    // Auto-rejected, so the review is fully decided: q opens the summary
    // and a second q finishes.
    finish_review(&mut app);

    let feedback =
        std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
    assert!(feedback.contains("**Needs work:** 1"));
    // The rejection heading carries the (auto-reject default) severity tag.
    assert!(feedback.contains("#### a.rs — [suggestion]\n"));
    assert!(feedback.contains("(Needs revision — see this file's line comments below)"));
    assert!(feedback.contains("bug"));
}

#[test]
fn i_opens_syntax_picker_for_selected_diff_file() {
    let mut app = crate::app::App::new_for_test(
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
    let mut state = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            crate::project::SessionKind::Claude,
            VibeMode::Vibe,
            false,
        ),
        PathBuf::from("/tmp/project"),
    );
    state.layout = DiffViewerLayout::Unified;
    state.files = vec![DiffFile {
        old_path: None,
        path: "src/main.rs".into(),
        status: DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: vec![],
    }];
    app.mode = AppMode::DiffViewer(state);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();

    match &app.mode {
        AppMode::SyntaxLanguagePicker(state) => {
            assert_eq!(
                state.languages[state.selected].language,
                crate::highlight::HighlightLanguage::Rust
            );
        }
        other => panic!(
            "expected syntax picker, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}
#[test]
fn file_list_j_k_walk_tree_rows_and_z_folds_a_directory() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["README.md", "src/a.rs", "src/b.rs"]);

    // Rows: README.md, dir src, a.rs, b.rs. j from the first file lands on
    // the directory header without changing the diffed file.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
    assert_eq!(state.selected_file, 0);

    // z folds it; the two files beneath lose their rows.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert!(state.collapsed_dirs.contains("src"));
    assert_eq!(state.file_tree_rows().len(), 2);

    // n keeps its file meaning and re-expands what it lands inside.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert_eq!(state.selected_file, 1);
    assert!(state.collapsed_dirs.is_empty());
}

#[test]
fn tree_fold_keys_are_ignored_while_the_patch_is_focused() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["src/a.rs"]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert_eq!(app.diff_viewer_focus(), Some(DiffViewerFocus::Patch));

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();

    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert!(
        state.collapsed_dirs.is_empty(),
        "folding belongs to the file list, not the patch panel"
    );
}
/// The tree is not a review-mode feature: the plain diff viewer (leader d)
/// shares `draw_file_list` and the same fold bindings, so it groups and
/// folds identically.
#[test]
fn plain_diff_viewer_gets_the_tree_and_fold_keys_too() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["README.md", "src/a.rs", "src/ui/b.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.review = false;
    }

    let rows = match &app.mode {
        AppMode::DiffViewer(state) => state.file_tree_rows(),
        _ => panic!("left the diff viewer"),
    };
    assert_eq!(
        rows.len(),
        5,
        "expected README + src/ + a.rs + ui/ + b.rs outside review mode: {rows:?}"
    );

    // j walks onto the `src` directory row, z folds it.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    let state = match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    };
    assert!(state.collapsed_dirs.contains("src"));
    assert_eq!(state.file_tree_rows().len(), 2);
    assert_eq!(
        state.selected_file, 0,
        "folding must not change the previewed file outside review mode either"
    );
}

/// Two files, each with the same shape: addressable lines are
/// 0 context, 1 removed, 2 added, 3 context.
const TWO_FILE_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,3 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
 fn delta() {}
diff --git a/b.rs b/b.rs
index 3333333..4444444 100644
--- a/b.rs
+++ b/b.rs
@@ -1,3 +1,3 @@
 fn one() {}
-fn two() {}
+fn two_prime() {}
 fn three() {}
";

/// A review over `TWO_FILE_PATCH` with one kept comment on the added line
/// (addressable index 2) of each file.
fn make_review_app_with_comments(workdir: &Path) -> App {
    let mut app = make_review_app(workdir, &["a.rs"]);
    let files = crate::diff::parse_unified_diff(TWO_FILE_PATCH).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = files;
        for path in ["a.rs", "b.rs"] {
            state.line_comments.insert(
                path.to_string(),
                vec![crate::app::LineComment {
                    location: crate::diff::DiffLineLocation {
                        old_line: None,
                        new_line: Some(2),
                    },
                    start: None,
                    text: format!("note on {path}"),
                    draft: false,
                    suggestion: None,
                    severity: crate::app::Severity::default(),
                    anchor_context: None,
                    start_anchor_context: None,
                    anchor_lost: false,
                    resolved: false,
                    carried: false,
                }],
            );
        }
    }
    app
}

fn cursor_position(app: &App) -> (usize, Option<usize>) {
    match &app.mode {
        AppMode::DiffViewer(state) => (state.selected_file, state.comment_cursor),
        _ => panic!("left the diff viewer"),
    }
}

#[test]
fn brace_keys_walk_comments_across_files_and_wrap() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_comments(dir.path());

    // With the cursor off, `}` activates it on the current file's comment.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (0, Some(2)));
    // Then across the file boundary — Tab could never leave a.rs.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (1, Some(2)));
    // And wraps back to the first comment in the review.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (0, Some(2)));
    // Backward from the first comment wraps to the last.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('{'))).unwrap();
    assert_eq!(cursor_position(&app), (1, Some(2)));
}

#[test]
fn brace_keys_report_when_the_review_has_no_comments() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('}'))).unwrap();
    assert_eq!(cursor_position(&app), (0, None));
    assert_eq!(
        app.message.as_deref(),
        Some("No comments in this review yet")
    );
}

#[test]
fn undo_restores_the_previous_verdict_and_returns_to_the_file() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // Approve a.rs: the verdict lands and the selection advances to b.rs.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(state.selected_file, 1);
            assert!(state.decisions.contains_key("a.rs"));
        }
        _ => panic!("left the diff viewer"),
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.decisions.contains_key("a.rs"));
            assert_eq!(
                state.selected_file, 0,
                "undo must land back on the file whose verdict it took back"
            );
            assert!(state.verdict_undo.is_empty());
        }
        _ => panic!("left the diff viewer"),
    }

    // Nothing left to undo.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    assert_eq!(app.message.as_deref(), Some("No verdict to undo"));
}

#[test]
fn undo_walks_back_through_successive_verdicts() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // a.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('s'))).unwrap(); // skip b.rs
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty(), "both verdicts undone");
            assert_eq!(state.selected_file, 0);
        }
        _ => panic!("left the diff viewer"),
    }
}

#[test]
fn undo_restores_a_comment_implied_rejection_as_implicit() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

    // a.rs is rejected implicitly by a line comment (see
    // `diff_review_sync_auto_reject`), not by a typed rejection.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.decisions.insert(
            "a.rs".to_string(),
            crate::app::ReviewDecision::Reject {
                feedback: String::new(),
                severity: crate::app::Severity::default(),
            },
        );
        state.auto_rejected.insert("a.rs".to_string());
    }

    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('U'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(matches!(
                state.decisions.get("a.rs"),
                Some(crate::app::ReviewDecision::Reject { .. })
            ));
            assert!(
                state.auto_rejected.contains("a.rs"),
                "undo must restore the implicit/explicit distinction, not just the verdict"
            );
        }
        _ => panic!("left the diff viewer"),
    }
}

#[test]
fn repeating_an_identical_verdict_does_not_stack_a_no_op_undo() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &["a.rs"]);

    // Two approvals of the only file: the second changes nothing, so it must
    // not leave a `U` that appears to do something.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
    match &app.mode {
        AppMode::DiffViewer(state) => assert_eq!(state.verdict_undo.len(), 1),
        _ => panic!("left the diff viewer"),
    }
}

// ── AMF's own files, grouped after the reviewer's ──────────────

fn amf_entry(path: &str, status: DiffFileStatus, new: &str) -> DiffFile {
    DiffFile {
        old_path: None,
        path: path.to_string(),
        status,
        additions: new.lines().count(),
        deletions: 0,
        is_binary: false,
        old_content: None,
        new_content: Some(new.to_string()),
        patch: String::new(),
        hunks: vec![],
    }
}

/// The sweep's fresh-repo diff, in `load_snapshot`'s path order: four of
/// AMF's files sorted ahead of the user's README edit, plus a committed
/// `.claude/settings.json` the user changed, which must stay theirs.
fn sweep_files() -> Vec<DiffFile> {
    let mut readme = amf_entry("README.md", DiffFileStatus::Modified, "edited\n");
    readme.old_content = Some("# Demo\n".into());
    let mut settings = amf_entry(".claude/settings.json", DiffFileStatus::Modified, "{}\n");
    settings.old_content = Some("{\"a\":1}\n".into());
    vec![
        amf_entry(
            ".claude/.gitignore",
            DiffFileStatus::Untracked,
            "notifications/\nlatest-prompt.txt\n",
        ),
        amf_entry(
            ".claude/amf-hook-state.json",
            DiffFileStatus::Untracked,
            "{}\n",
        ),
        settings,
        amf_entry(
            ".claude/settings.local.json",
            DiffFileStatus::Untracked,
            &"{}\n".repeat(107),
        ),
        amf_entry(".gitignore", DiffFileStatus::Untracked, "CLAUDE.local.md\n"),
        readme,
    ]
}

fn plain_viewer_with(files: Vec<DiffFile>) -> App {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &[]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.review = false;
        state.files = files;
        state.group_amf_files();
        state.selected_file = 0;
    }
    app
}

fn viewer(app: &App) -> &DiffViewerState {
    match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("left the diff viewer"),
    }
}

fn row_labels(state: &DiffViewerState) -> Vec<String> {
    state
        .file_tree_rows()
        .into_iter()
        .map(|row| match row {
            crate::app::FileTreeRow::Dir {
                label, collapsed, ..
            } => format!("{}{label}", if collapsed { "▸" } else { "▾" }),
            crate::app::FileTreeRow::File { name, .. } => name,
        })
        .collect()
}

#[test]
fn amf_files_sort_after_the_users_under_one_folded_row() {
    let app = plain_viewer_with(sweep_files());
    let state = viewer(&app);
    let order: Vec<&str> = state.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        order,
        vec![
            ".claude/settings.json",
            "README.md",
            ".claude/.gitignore",
            ".claude/amf-hook-state.json",
            ".claude/settings.local.json",
            ".gitignore",
        ],
        "the reviewer's files first, AMF's after, each in path order"
    );
    assert_eq!(state.amf_file_count, 4);
    assert_eq!(state.amf_files_start(), 2);
    assert_eq!(
        row_labels(state),
        vec!["▾.claude", "settings.json", "README.md", "▸AMF files"],
        "a tracked .claude/settings.json is an ordinary change"
    );
    assert_eq!(state.selected_file, 0, "the first file shown is the user's");
}

#[test]
fn dot_shows_and_folds_the_amf_files_from_either_panel() {
    let mut app = plain_viewer_with(sweep_files());
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    assert_eq!(
        row_labels(viewer(&app)),
        vec![
            "▾.claude",
            "settings.json",
            "README.md",
            "▾AMF files",
            ".claude/.gitignore",
            ".claude/amf-hook-state.json",
            ".claude/settings.local.json",
            ".gitignore",
        ]
    );
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("Showing 4 AMF files")
    );

    handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
    assert_eq!(app.diff_viewer_focus(), Some(DiffViewerFocus::Patch));
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    assert_eq!(row_labels(viewer(&app)).last().unwrap(), "▸AMF files");
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("Folded 4 AMF files")
    );
}

#[test]
fn the_amf_row_folds_like_a_directory_and_navigation_reveals_it() {
    let mut app = plain_viewer_with(sweep_files());
    // j walks the rows down onto the folded group; l opens it.
    for _ in 0..3 {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    }
    assert_eq!(
        viewer(&app).tree_cursor_dir.as_deref(),
        Some(crate::app::AMF_GROUP_KEY)
    );
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
    assert!(
        !viewer(&app)
            .collapsed_dirs
            .contains(crate::app::AMF_GROUP_KEY)
    );
    // h on one of its files steps out to the group row, z folds it again.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
    assert_eq!(viewer(&app).selected_file, 2);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('h'))).unwrap();
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
    let state = viewer(&app);
    assert!(state.collapsed_dirs.contains(crate::app::AMF_GROUP_KEY));
    assert_eq!(state.selected_file, 2, "folding never changes the patch");
    let rows = state.file_tree_rows();
    assert!(matches!(
        &rows[state.tree_cursor_row(&rows).unwrap()],
        crate::app::FileTreeRow::Dir { path, .. } if path == crate::app::AMF_GROUP_KEY
    ));

    // n from the user's last file lands in the group and opens it.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
        state.on_file_changed();
    }
    app.diff_viewer_select_next_file();
    let state = viewer(&app);
    assert_eq!(state.files[state.selected_file].path, ".claude/.gitignore");
    assert!(!state.collapsed_dirs.contains(crate::app::AMF_GROUP_KEY));
}

/// `Z` keeps its meaning — the first press folds the tree — even though
/// the AMF row starts folded, and it never changes that row: `.` does.
#[test]
fn shift_z_folds_the_tree_first_and_leaves_the_amf_group_alone() {
    let mut app = plain_viewer_with(sweep_files());
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
    assert_eq!(
        row_labels(viewer(&app)),
        vec!["▸.claude", "README.md", "▸AMF files"],
        "the first Z folds, as it does without AMF files"
    );
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
    assert_eq!(
        row_labels(viewer(&app)),
        vec!["▾.claude", "settings.json", "README.md", "▸AMF files"],
        "unfolding the tree keeps the AMF group folded"
    );

    // Shown with `.`, the group stays shown through both presses.
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    for _ in 0..2 {
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
        assert!(
            !viewer(&app)
                .collapsed_dirs
                .contains(crate::app::AMF_GROUP_KEY)
        );
    }
}

/// Final Review keeps asking for a verdict on AMF's files (a deliberate
/// choice), so the finish prompt says how many of the undecided are
/// AMF's, and `u` reaches them through the folded row.
#[test]
fn finishing_final_review_names_the_undecided_amf_files_and_u_reveals_them() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = make_review_app(dir.path(), &[]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = sweep_files();
        state.group_amf_files();
        state.selected_file = 0;
    }
    // Verdicts on both of the reviewer's files.
    app.diff_review_approve_current();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
    }
    app.diff_review_approve_current();
    app.confirm_or_finish_review().unwrap();
    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.starts_with("4 file(s) (4 generated by AMF) have no verdict"),
        "{message}"
    );
    assert!(viewer(&app).finish_confirm);

    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
    }
    app.diff_review_jump_next_undecided();
    let state = viewer(&app);
    assert!(state.is_amf_file(state.selected_file));
    assert!(
        !state.collapsed_dirs.contains(crate::app::AMF_GROUP_KEY),
        "jumping to an undecided AMF file unfolds its row"
    );
}

#[test]
fn shift_z_with_only_amf_files_has_no_tree_to_fold() {
    let only_amf: Vec<DiffFile> = sweep_files()
        .into_iter()
        .filter(|f| f.status == DiffFileStatus::Untracked)
        .collect();
    let mut app = plain_viewer_with(only_amf);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();
    assert!(viewer(&app).collapsed_dirs.is_empty());
    assert_eq!(app.message.as_deref(), Some("No directories to collapse"));
}

/// A failed reload lists nothing, so it must not keep the previous
/// load's AMF count (the `.` hint, the split header).
#[test]
fn a_failed_reload_forgets_the_amf_count() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut app = plain_viewer_with(sweep_files());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        assert_eq!(state.amf_file_count, 4);
        state.workdir = dir.path().join("not-a-repo");
    }
    app.refresh_diff_viewer();
    app.complete_diff_viewer_loading();
    let state = viewer(&app);
    assert!(state.error.is_some(), "the load should fail outside a repo");
    assert!(state.files.is_empty());
    assert_eq!(state.amf_file_count, 0);
}

#[test]
fn a_refresh_keeps_the_reviewers_choice_for_the_amf_group() {
    let mut app = plain_viewer_with(sweep_files());
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = sweep_files();
        state.group_amf_files();
    }
    assert!(
        !viewer(&app)
            .collapsed_dirs
            .contains(crate::app::AMF_GROUP_KEY)
    );
}

#[test]
fn with_only_amf_files_the_group_starts_open() {
    let only_amf: Vec<DiffFile> = sweep_files()
        .into_iter()
        .filter(|f| f.status == DiffFileStatus::Untracked)
        .collect();
    let app = plain_viewer_with(only_amf);
    let state = viewer(&app);
    assert_eq!(state.amf_file_count, 4);
    assert_eq!(row_labels(state).first().unwrap(), "▾AMF files");
}

#[test]
fn dot_says_so_when_there_are_no_amf_files() {
    let mut app = plain_viewer_with(vec![amf_entry(
        "README.md",
        DiffFileStatus::Untracked,
        "x\n",
    )]);
    handle_diff_viewer_key(&mut app, key(KeyCode::Char('.'))).unwrap();
    assert_eq!(viewer(&app).amf_file_count, 0);
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains("No AMF-generated files")
    );
}

/// The real load path, on a real repository: what `r` and opening the
/// viewer run.
#[test]
fn loading_a_fresh_repo_groups_what_amf_wrote() {
    let repo = tempfile::TempDir::new().unwrap();
    let dir = repo.path();
    run_git(dir, &["init", "-q", "--initial-branch=main"]);
    run_git(dir, &["config", "user.name", "AMF Test"]);
    run_git(dir, &["config", "user.email", "amf@example.com"]);
    // A developer's global excludes often ignore `settings.local.json`.
    run_git(dir, &["config", "core.excludesFile", "/dev/null"]);
    std::fs::write(dir.join("README.md"), "# Demo\n").unwrap();
    run_git(dir, &["add", "README.md"]);
    run_git(dir, &["commit", "-q", "-m", "base"]);
    std::fs::create_dir_all(dir.join(".claude")).unwrap();
    std::fs::write(dir.join(".claude/settings.local.json"), "{}\n").unwrap();
    std::fs::write(dir.join(".claude/amf-hook-state.json"), "{}\n").unwrap();
    std::fs::write(dir.join(".gitignore"), "CLAUDE.local.md\n").unwrap();
    std::fs::write(dir.join("README.md"), "# Demo\nEdited\n").unwrap();

    let mut app = make_review_app(dir, &[]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.review = false;
    }
    app.refresh_diff_viewer();
    app.complete_diff_viewer_loading();
    let state = viewer(&app);
    assert_eq!(state.error, None);
    assert_eq!(state.files[0].path, "README.md");
    assert_eq!(state.amf_file_count, 3);
    assert_eq!(row_labels(state), vec!["README.md", "▸AMF files"]);
}

mod viewport_navigation;

mod search;

mod editor_launch;

mod finish_flow;

mod comment_suggestions;

mod overview;

mod file_navigation;

mod editor_behavior;
