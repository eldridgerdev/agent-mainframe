use super::*;

fn memory_add_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

fn memory_add_category(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().category,
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_open_memory_add_seeds_finding_with_file_hint_and_default_category() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    // Post-ready like the "done" reply template: opens in the confirm view,
    // not straight into editing.
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    assert_eq!(memory_add_editor_text(&app), "comment 1 (src/file1.rs:1)");
    assert_eq!(memory_add_category(&app), 0, "defaults to General");
}

#[test]
fn pr_review_open_memory_add_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_memory_add();
    assert_eq!(app.pr_review_memory_add_view(), None);
    assert!(app.message.is_some());
}

#[test]
fn pr_review_memory_add_edit_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    // Keys are ignored until edit mode is entered.
    let before = memory_add_editor_text(&app);
    app.pr_review_memory_add_editor_key(KeyEvent::from(KeyCode::Char('Z')));
    assert_eq!(memory_add_editor_text(&app), before);

    app.pr_review_memory_add_edit();
    assert_eq!(app.pr_review_memory_add_view(), Some(true));
    app.pr_review_memory_add_editor_key(KeyEvent::from(KeyCode::Char('!')));
    assert_eq!(memory_add_editor_text(&app), format!("{before}!"));

    app.pr_review_memory_add_stop_edit();
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    app.pr_review_cancel_memory_add();
    assert_eq!(app.pr_review_memory_add_view(), None);
}

#[test]
fn pr_review_cycle_memory_category_wraps() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    let n = crate::app::pr_review::MEMORY_CATEGORIES.len();
    for i in 1..=n {
        app.pr_review_cycle_memory_category();
        assert_eq!(memory_add_category(&app), i % n);
    }
}

#[test]
fn pr_review_append_empty_finding_is_rejected() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().editor = crate::editor::TextEditor::new(String::new());
    }

    // No `expect_repo_root()` was set on the mock: the empty check short-
    // circuits before the doc path is ever resolved.
    app.pr_review_append_memory().unwrap();
    assert_eq!(
        app.pr_review_memory_add_view(),
        Some(false),
        "dialog stays open"
    );
    assert!(app.message.is_some());
}

#[test]
fn pr_review_append_memory_writes_and_dedups() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(move |_| Ok(repo_clone.clone()));

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
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    // Nothing is preselected: ⏎ alone keeps the dialog open and says why.
    app.pr_review_append_memory().unwrap();
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.memory_add.as_ref().unwrap().scope_required),
        _ => panic!("expected PrReview"),
    }
    assert!(!repo.join(".amf").join("review-memory.md").exists());

    app.pr_review_set_memory_scope(crate::app::review_memory::MemoryScope::Project);
    app.pr_review_append_memory().unwrap();
    assert_eq!(
        app.pr_review_memory_add_view(),
        None,
        "dialog closes once appended"
    );

    let doc_path = repo.join(".amf").join("review-memory.md");
    let contents = std::fs::read_to_string(&doc_path).unwrap();
    assert!(contents.contains("## General"));
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));

    // Re-adding the same finding is a dedup no-op, not a duplicate bullet.
    app.pr_review_open_memory_add();
    app.pr_review_set_memory_scope(crate::app::review_memory::MemoryScope::Project);
    app.pr_review_append_memory().unwrap();
    let contents_after = std::fs::read_to_string(&doc_path).unwrap();
    assert_eq!(
        contents_after.matches("comment 1 (src/file1.rs:1)").count(),
        1,
        "dedup should skip the second append"
    );
}

#[test]
fn pr_review_append_memory_honors_project_review_memory_path_override() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    // A project-level override in `{repo}/.amf/config.json` should redirect
    // the append away from the default `.amf/review-memory.md`.
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("config.json"),
        r#"{"review_memory_path": ".amf/team-review-memory.md"}"#,
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(move |_| Ok(repo_clone.clone()));

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
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_set_memory_scope(crate::app::review_memory::MemoryScope::Project);
    app.pr_review_append_memory().unwrap();

    let default_path = repo.join(".amf").join("review-memory.md");
    assert!(
        !default_path.exists(),
        "the default path should be untouched when a project override is set"
    );
    let overridden_path = repo.join(".amf").join("team-review-memory.md");
    let contents = std::fs::read_to_string(&overridden_path).unwrap();
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));
}

#[test]
fn pr_review_memory_add_starts_with_no_destination_until_one_is_picked() {
    use crate::app::review_memory::MemoryScope;

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    let scope = |app: &App| match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().scope,
        _ => panic!("expected PrReview"),
    };
    assert_eq!(
        scope(&app),
        None,
        "a default was too easy to confirm without noticing where the finding went"
    );

    app.pr_review_set_memory_scope(MemoryScope::Global);
    assert_eq!(scope(&app), Some(MemoryScope::Global));
    app.pr_review_set_memory_scope(MemoryScope::Project);
    assert_eq!(scope(&app), Some(MemoryScope::Project));
}

#[test]
fn pr_review_memory_ai_summary_single_harness_skips_picker_and_gates_the_run() {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|p| Ok(p.to_path_buf()));
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![AgentKind::Claude],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    app.pr_review_open_memory_ai_summary_pick();

    assert!(
        !app.pr_review_memory_ai_summary_picking(),
        "exactly one harness auto-launches, no picker"
    );
    match &app.mode {
        AppMode::PromptPrecall(pending) => {
            assert_eq!(
                pending.prompt_id,
                crate::prompts::PromptId::ReviewMemoryAiSummary
            );
            assert_eq!(pending.harness, AgentKind::Claude);
            match pending.prior_mode.as_ref() {
                AppMode::PrReview(state) => {
                    // `ai_summary` is not switched to `Generating` until the
                    // gate clears, so the stashed mode still shows the
                    // untouched confirm view — a cancel must not leave the
                    // dialog stuck on "Generating" for a run that never
                    // started.
                    assert!(
                        state.memory_add.as_ref().unwrap().ai_summary.is_none(),
                        "prior mode should not show Generating before the gate clears"
                    );
                }
                other => panic!(
                    "expected the stashed mode to be PrReview, got {:?}",
                    std::mem::discriminant(other)
                ),
            }
        }
        other => panic!(
            "expected PromptPrecall, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn pr_review_memory_ai_summary_cancel_precall_leaves_dialog_untouched_and_confirm_still_generates()
{
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|p| Ok(p.to_path_buf()));
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![AgentKind::Claude],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    app.pr_review_open_memory_ai_summary_pick();
    assert!(matches!(app.mode, AppMode::PromptPrecall(_)));

    app.precall_cancel();

    // Cancelling the notice must not leave the dialog stuck showing
    // "Generating" for a run that was never spawned.
    assert!(!app.pr_review_memory_ai_summary_generating());
    assert!(app.memory_ai_summary_bg.is_none());
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.memory_add.as_ref().unwrap().ai_summary.is_none())
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Pressing "summarize with AI" again still gates and, on confirm,
    // actually starts generating this time.
    app.pr_review_open_memory_ai_summary_pick();
    assert!(matches!(app.mode, AppMode::PromptPrecall(_)));
    app.precall_confirm().unwrap();
    assert!(app.pr_review_memory_ai_summary_generating());
    assert!(app.memory_ai_summary_bg.is_some());
}

#[test]
fn pr_review_memory_ai_summary_pick_multiple_harnesses_move_cancel_and_confirm() {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|p| Ok(p.to_path_buf()));
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![AgentKind::Claude, AgentKind::Codex],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    app.pr_review_open_memory_ai_summary_pick();
    assert!(app.pr_review_memory_ai_summary_picking());

    // Cancelling the picker starts nothing and returns to the ordinary
    // confirm view.
    app.pr_review_memory_ai_summary_pick_move(1);
    app.pr_review_memory_ai_summary_pick_cancel();
    assert!(!app.pr_review_memory_ai_summary_picking());
    assert!(!app.pr_review_memory_ai_summary_generating());
    assert_eq!(app.pr_review_memory_add_view(), Some(false));

    // Re-open, move to the second harness, confirm: gates on that harness.
    app.pr_review_open_memory_ai_summary_pick();
    app.pr_review_memory_ai_summary_pick_move(1);
    app.pr_review_memory_ai_summary_pick_confirm();
    match &app.mode {
        AppMode::PromptPrecall(pending) => assert_eq!(pending.harness, AgentKind::Codex),
        other => panic!(
            "expected PromptPrecall, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn pr_review_cancel_watching_memory_ai_summary_leaves_raw_text_and_clears_state() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }
    assert!(app.pr_review_memory_ai_summary_generating());

    app.pr_review_cancel_watching_memory_ai_summary();

    assert!(!app.pr_review_memory_ai_summary_generating());
    assert_eq!(
        memory_add_editor_text(&app),
        raw,
        "cancelling never touches the raw/edited text"
    );
    assert_eq!(
        app.pr_review_memory_add_view(),
        Some(false),
        "the dialog stays open, nothing was persisted"
    );
}

#[test]
fn poll_memory_ai_summary_bg_success_overwrites_editor_and_lands_on_review() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Ok("Guard shared state before mutation (src/file1.rs:1).".to_string()),
    })
    .unwrap();

    assert!(app.poll_memory_ai_summary_bg());
    assert!(app.memory_ai_summary_bg.is_none());
    assert_eq!(
        memory_add_editor_text(&app),
        "Guard shared state before mutation (src/file1.rs:1)."
    );
    assert_eq!(
        app.pr_review_memory_add_view(),
        Some(false),
        "success lands back on the ordinary review/edit confirm view"
    );
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.memory_add.as_ref().unwrap().ai_summary.is_none())
        }
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn poll_memory_ai_summary_bg_failure_shows_inline_error_and_keeps_raw_text() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Err("harness not installed".to_string()),
    })
    .unwrap();

    assert!(app.poll_memory_ai_summary_bg());
    assert_eq!(
        memory_add_editor_text(&app),
        raw,
        "a failed run never touches the raw/edited text"
    );
    match &app.mode {
        AppMode::PrReview(state) => assert!(matches!(
            &state.memory_add.as_ref().unwrap().ai_summary,
            Some(crate::app::MemoryAiSummaryState::Failed(m)) if m == "harness not installed"
        )),
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn pr_review_dismiss_memory_ai_summary_error_returns_to_idle_with_raw_text() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Failed("boom".to_string()));
    }
    assert_eq!(
        app.pr_review_memory_ai_summary_error(),
        Some("boom".to_string())
    );

    app.pr_review_dismiss_memory_ai_summary_error();

    assert_eq!(app.pr_review_memory_ai_summary_error(), None);
    assert_eq!(memory_add_editor_text(&app), raw);
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
}

#[test]
fn poll_memory_ai_summary_bg_drops_a_stale_result_when_no_longer_watching() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);
    // The dialog never entered `Generating` (or already stopped watching) —
    // there is nowhere live to land this result.
    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Ok("Some AI text".to_string()),
    })
    .unwrap();

    assert!(app.poll_memory_ai_summary_bg());
    assert_eq!(
        memory_add_editor_text(&app),
        raw,
        "a result with nowhere live to land it must not be applied"
    );
}

#[test]
fn pr_review_append_memory_after_ai_summary_persists_the_generated_text_not_the_raw_seed() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

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
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Ok("Never mutate shared state without the lock (src/file1.rs:1).".to_string()),
    })
    .unwrap();
    assert!(app.poll_memory_ai_summary_bg());

    app.pr_review_set_memory_scope(crate::app::review_memory::MemoryScope::Project);
    app.pr_review_append_memory().unwrap();

    let contents = std::fs::read_to_string(repo.join(".amf").join("review-memory.md")).unwrap();
    assert!(contents.contains("- Never mutate shared state without the lock (src/file1.rs:1)."));
    assert!(
        !contents.contains(&raw),
        "the raw seed must not survive once the AI summary was confirmed"
    );
}

#[test]
fn pr_review_append_memory_global_scope_writes_the_cross_project_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let global_doc = tmp.path().join("global").join("review-memory.md");

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(move |_| Ok(repo_clone.clone()));

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
        Box::new(worktree),
    );
    // An absolute override keeps the test off the developer's real
    // `~/.config/amf/review-memory.md`.
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_set_memory_scope(crate::app::review_memory::MemoryScope::Global);
    app.pr_review_append_memory().unwrap();

    assert_eq!(app.pr_review_memory_add_view(), None, "dialog closes");
    let contents = std::fs::read_to_string(&global_doc).unwrap();
    assert!(
        contents.starts_with("# Review memory (cross-project)"),
        "a freshly created global doc gets the cross-project header, got: {contents}"
    );
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));
    assert!(
        !repo.join(".amf").join("review-memory.md").exists(),
        "the project doc should be untouched when the global scope is picked"
    );
}

#[test]
fn review_memory_paths_resolves_project_override_and_global_together() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("config.json"),
        r#"{"review_memory_path": ".amf/team-review-memory.md"}"#,
    )
    .unwrap();
    let global_doc = tmp.path().join("global-lessons.md");

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
    app.config.global_review_memory_path = Some(global_doc.display().to_string());

    let paths = app.review_memory_paths(&repo);
    assert_eq!(
        paths.project,
        repo.join(".amf").join("team-review-memory.md")
    );
    assert_eq!(paths.global, global_doc);
}
