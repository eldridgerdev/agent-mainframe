use super::*;

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
