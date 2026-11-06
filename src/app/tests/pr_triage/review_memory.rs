use super::*;

#[test]
fn review_memory_bootstrap_pick_opens_defaulting_to_fifty() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);

    assert!(!app.review_memory_bootstrap_picking());
    app.open_review_memory_bootstrap_pick();
    assert!(app.review_memory_bootstrap_picking());

    match &app.mode {
        AppMode::PrPicker(state) => {
            let pick = state.bootstrap_pick.as_ref().unwrap();
            assert_eq!(
                crate::app::pr_review::BootstrapDepth::ALL[pick.selected],
                crate::app::pr_review::BootstrapDepth::default()
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn review_memory_bootstrap_pick_defaults_to_project_scope_and_toggles() {
    use crate::app::review_memory::MemoryScope;

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();

    let scope = |app: &App| match &app.mode {
        AppMode::PrPicker(state) => state.bootstrap_pick.as_ref().unwrap().scope,
        _ => panic!("expected PrPicker"),
    };
    assert_eq!(
        scope(&app),
        MemoryScope::Project,
        "a bootstrap learns from this repo's history, so it defaults to this repo's doc"
    );

    app.review_memory_bootstrap_toggle_scope();
    assert_eq!(scope(&app), MemoryScope::Global);
    app.review_memory_bootstrap_toggle_scope();
    assert_eq!(scope(&app), MemoryScope::Project);
}

#[test]
fn review_memory_bootstrap_pick_move_wraps() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();

    let selected = |app: &App| match &app.mode {
        AppMode::PrPicker(state) => state.bootstrap_pick.as_ref().unwrap().selected,
        _ => panic!("expected PrPicker"),
    };

    // Starts on the default (Fifty, index 1).
    assert_eq!(selected(&app), 1);
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 2);
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 3);
    // Wraps forward past the last entry.
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 0);
    // Wraps backward past the first entry.
    app.review_memory_bootstrap_pick_move(-1);
    assert_eq!(selected(&app), 3);
}

#[test]
fn review_memory_bootstrap_pick_cancel_closes_the_overlay() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();
    assert!(app.review_memory_bootstrap_picking());

    app.review_memory_bootstrap_pick_cancel();

    assert!(!app.review_memory_bootstrap_picking());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn poll_review_memory_bootstrap_bg_surfaces_result_and_returns_to_picker() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    tx.send(crate::app::pr_review::BootstrapProgress::Distilling {
        pr_count: 3,
        token_estimate: 42,
    })
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    match &app.mode {
        AppMode::ReviewMemoryBootstrapRunning(state) => assert_eq!(
            state.stage,
            crate::app::pr_review::BootstrapStage::Distilling {
                pr_count: 3,
                token_estimate: 42,
            }
        ),
        other => panic!(
            "expected ReviewMemoryBootstrapRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    tx.send(crate::app::pr_review::BootstrapProgress::Done(Ok(
        crate::app::pr_review::BootstrapOutcome {
            pr_count: 3,
            appended: 2,
        },
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(app.review_memory_bootstrap_bg.is_none());
}

#[test]
fn poll_review_memory_bootstrap_bg_error_still_returns_to_picker() {
    // Regression: `show_error` unconditionally resets `self.mode` to `Normal`
    // for any non-Normal/Help/Viewing mode. The `Done(Err(_))` branch used to
    // call it *before* restoring the origin picker, so a failed distill (e.g.
    // the headless `claude` call erroring) dumped the user onto the bare
    // dashboard instead of back to the PR picker like the success path does.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    tx.send(crate::app::pr_review::BootstrapProgress::Done(Err(
        anyhow::anyhow!("claude headless command failed"),
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    match &app.mode {
        AppMode::PrPicker(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("claude headless command failed")),
                "expected the picker's inline error to surface the failure, got {:?}",
                state.error
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.review_memory_bootstrap_bg.is_none());
}

#[test]
fn cancel_review_memory_bootstrap_returns_to_picker_without_dropping_the_bg_result() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    // The user gives up watching...
    app.cancel_review_memory_bootstrap();
    assert!(matches!(app.mode, AppMode::PrPicker(_)));

    // ...but the background run still finishes and reports its result (a real
    // side effect: it wrote findings and spent tokens), even though the user
    // isn't looking at the running screen anymore.
    tx.send(crate::app::pr_review::BootstrapProgress::Done(Ok(
        crate::app::pr_review::BootstrapOutcome {
            pr_count: 5,
            appended: 1,
        },
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    assert!(app.review_memory_bootstrap_bg.is_none());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn open_review_memory_compact_confirm_reads_doc_and_opens() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n- Two\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    assert!(!app.review_memory_compact_confirming());
    app.open_review_memory_compact_confirm();
    assert!(app.review_memory_compact_confirming());

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 2);
            assert_eq!(
                state.confirm.scope,
                crate::app::review_memory::MemoryScope::Project
            );
            assert!(matches!(state.prior_mode.as_ref(), AppMode::PrPicker(_)));
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn open_review_memory_compact_confirm_reachable_from_pr_triage() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo;
    }

    app.open_review_memory_compact_confirm();

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 1);
            assert!(matches!(state.prior_mode.as_ref(), AppMode::PrReview(_)));
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    // Cancelling restores PR Triage, not the picker.
    app.review_memory_compact_confirm_cancel();
    assert!(matches!(app.mode, AppMode::PrReview(_)));
}

#[test]
fn open_review_memory_compact_confirm_reachable_from_the_dashboard() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n- Two\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].workdir = repo;
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.selection = Selection::Feature(0, 0);
    assert!(matches!(app.mode, AppMode::Normal));

    app.open_review_memory_compact_confirm();

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 2);
            assert!(matches!(state.prior_mode.as_ref(), AppMode::Normal));
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    // Cancelling restores the dashboard.
    app.review_memory_compact_confirm_cancel();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn open_review_memory_compact_confirm_uses_global_when_only_global_has_findings() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        &global_doc,
        "# Review memory (cross-project)\n\n## Tests\n- One\n- Two\n- Three\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    app.open_review_memory_compact_confirm();

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 3);
            assert_eq!(
                state.confirm.scope,
                crate::app::review_memory::MemoryScope::Global
            );
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn review_memory_compact_toggle_scope_rereads_the_selected_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- Project one\n- Project two\n",
    )
    .unwrap();
    std::fs::write(
        &global_doc,
        "# Review memory (cross-project)\n\n## Tests\n- Global one\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    let compact_selection = |app: &App| match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            (state.confirm.scope, state.confirm.existing_findings)
        }
        _ => panic!("expected ReviewMemoryCompactConfirm"),
    };

    app.open_review_memory_compact_confirm();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Project, 2)
    );

    app.review_memory_compact_toggle_scope();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Global, 1)
    );

    app.review_memory_compact_toggle_scope();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Project, 2)
    );
}

#[test]
fn open_review_memory_compact_confirm_bails_when_doc_missing() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    app.open_review_memory_compact_confirm();

    assert!(!app.review_memory_compact_confirming());
    assert!(app.message.is_some());
}

#[test]
fn review_memory_compact_confirm_cancel_closes_the_overlay() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let prior_mode = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));
    app.mode = AppMode::ReviewMemoryCompactConfirm(Box::new(
        crate::app::ReviewMemoryCompactConfirmState {
            workdir: std::path::PathBuf::from("/tmp/test-workdir"),
            confirm: crate::app::CompactConfirmState {
                existing_findings: 3,
                scope: crate::app::review_memory::MemoryScope::Project,
            },
            prior_mode,
        },
    ));
    assert!(app.review_memory_compact_confirming());

    app.review_memory_compact_confirm_cancel();

    assert!(!app.review_memory_compact_confirming());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn review_memory_compact_confirm_run_targets_the_selected_global_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    let prior_mode = Box::new(AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    }));
    app.mode = AppMode::ReviewMemoryCompactConfirm(Box::new(
        crate::app::ReviewMemoryCompactConfirmState {
            workdir: tmp.path().join("repo"),
            // The missing file makes the spawned background read return
            // immediately, while this cached count still lets us exercise the
            // selected-path handoff without invoking an agent in the test.
            confirm: crate::app::CompactConfirmState {
                existing_findings: 1,
                scope: crate::app::review_memory::MemoryScope::Global,
            },
            prior_mode,
        },
    ));

    app.review_memory_compact_confirm_run();
    // The pre-call notice interposes; continuing dispatches the run.
    assert!(matches!(&app.mode, AppMode::PromptPrecall(p)
        if p.prompt_id == crate::prompts::PromptId::ReviewMemoryCompact));
    app.precall_confirm().unwrap();

    match &app.mode {
        AppMode::ReviewMemoryCompactRunning(view) => {
            assert_eq!(view.scope, crate::app::review_memory::MemoryScope::Global);
        }
        other => panic!(
            "expected ReviewMemoryCompactRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert_eq!(
        app.review_memory_compact_pending
            .as_ref()
            .map(|state| (&state.path, state.scope)),
        Some((&global_doc, crate::app::review_memory::MemoryScope::Global))
    );
    assert!(matches!(
        app.review_memory_compact_pending
            .as_ref()
            .map(|state| state.origin.as_ref()),
        Some(AppMode::PrPicker(_))
    ));
}

#[test]
fn poll_review_memory_compact_bg_success_opens_review_dialog() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Global,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Global,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    tx.send(crate::app::pr_review::CompactProgress::Compacting { token_estimate: 99 })
        .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    match &app.mode {
        AppMode::ReviewMemoryCompactRunning(view) => assert_eq!(
            view.stage,
            crate::app::pr_review::CompactStage::Compacting { token_estimate: 99 }
        ),
        other => panic!(
            "expected ReviewMemoryCompactRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(Some(
        crate::app::pr_review::CompactOutcome {
            original_findings: 5,
            proposed_findings: 3,
            proposed_content: "# Review memory\n\n## Tests\n- Merged finding\n".to_string(),
            original_content: "# Review memory\n\n## Tests\n- One\n- Two\n".to_string(),
        },
    ))))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(app.review_memory_compact_bg.is_none());
    match &app.mode {
        AppMode::ReviewMemoryCompactReview(state) => {
            assert_eq!(state.scope, crate::app::review_memory::MemoryScope::Global);
            assert_eq!(state.original_findings, 5);
            assert_eq!(state.proposed_findings, 3);
            assert_eq!(
                state.editor.text(),
                "# Review memory\n\n## Tests\n- Merged finding\n"
            );
        }
        other => panic!(
            "expected ReviewMemoryCompactReview, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn poll_review_memory_compact_bg_nothing_to_compact_returns_to_picker_with_message() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(None)))
        .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(app.message.is_some());
}

#[test]
fn poll_review_memory_compact_bg_error_still_returns_to_picker() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    tx.send(crate::app::pr_review::CompactProgress::Done(Err(
        anyhow::anyhow!("claude headless command failed"),
    )))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    match &app.mode {
        AppMode::PrPicker(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("claude headless command failed"))
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.review_memory_compact_bg.is_none());
}

#[test]
fn cancel_review_memory_compact_does_not_reopen_review_dialog_over_the_user() {
    // Regression guard for the analogous AI-review bug (`ai_review_pending`):
    // a late `Done` after the user already backed out (`esc`) must not force
    // them into a full-screen dialog they didn't ask to see, and must not
    // panic when `self.mode` is no longer `ReviewMemoryCompactRunning`.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    app.cancel_review_memory_compact();
    assert!(matches!(app.mode, AppMode::PrPicker(_)));

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(Some(
        crate::app::pr_review::CompactOutcome {
            original_findings: 2,
            proposed_findings: 1,
            proposed_content: "# Review memory\n".to_string(),
            original_content: "# Review memory\n\n## Tests\n- One\n- Two\n".to_string(),
        },
    ))))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(app.review_memory_compact_bg.is_none());
    assert!(app.review_memory_compact_pending.is_none());
    // Stays on the picker the user cancelled back to — not yanked into the
    // review dialog, and not silently dropped either (a toast explains it).
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("navigated away"))
    );
}

/// Enter the compact review dialog with `content` proposed for `path`, taking
/// the conflict baseline from whatever is on disk at `path` right now — the
/// same snapshot the background pass would have read.
fn enter_compact_review_for_test(app: &mut App, path: std::path::PathBuf, content: &str) {
    let original_content = std::fs::read_to_string(&path).unwrap_or_default();
    let origin = Box::new(AppMode::PrPicker(crate::app::PrPickerState {
        workdir: std::path::PathBuf::from("/tmp/test-workdir"),
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    }));
    app.mode = AppMode::ReviewMemoryCompactReview(crate::app::CompactReviewState {
        origin,
        path,
        scope: crate::app::review_memory::MemoryScope::Project,
        original_findings: 3,
        proposed_findings: 2,
        editor: crate::editor::TextEditor::new(content.to_string()),
        original_content,
        overwrite_confirmed: false,
        editing: false,
        scroll: 0,
        sync_to_cursor: false,
        error: None,
    });
}

#[test]
fn pr_review_compact_write_overwrites_file_and_returns_to_picker() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "# Review memory\n\n## Tests\n- Merged finding\n");
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("3") && t.message.contains("2"))
    );
}

#[test]
fn pr_review_compact_write_keeps_findings_appended_while_the_dialog_was_open() {
    // The compact proposal rewrites a snapshot taken before the agent pass. Any
    // AMF session can append to the same doc in that window (every session on
    // the machine shares the global one), so those findings have to survive the
    // rewrite instead of being clobbered by a stale snapshot.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    // Another session appends after the snapshot was taken.
    crate::app::review_memory::append_finding(
        &path,
        crate::app::review_memory::MemoryScope::Project,
        "Concurrency",
        "Guard the shared doc",
    )
    .unwrap();

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("- Merged finding"));
    assert!(contents.contains("- Guard the shared doc"));
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("kept 1 finding added elsewhere"))
    );
}

#[test]
fn pr_review_compact_write_refuses_once_when_the_doc_diverged_then_overwrites() {
    // A change no append can explain (prose hand-edited, findings deleted)
    // can't be replayed onto the rewrite, so the first confirm reports it
    // inline and writes nothing; confirming again is a deliberate overwrite.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    let hand_edited = "# Review memory\n\nHand-written note.\n\n## Tests\n- Stale one\n";
    std::fs::write(&path, hand_edited).unwrap();

    app.pr_review_compact_write().unwrap();

    match &app.mode {
        AppMode::ReviewMemoryCompactReview(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("changed on disk"))
            );
            assert!(state.overwrite_confirmed);
        }
        other => panic!(
            "expected the dialog to stay open, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), hand_edited);

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Review memory\n\n## Tests\n- Merged finding\n"
    );
}

#[test]
fn pr_review_compact_discard_leaves_file_untouched() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    let original = "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n";
    std::fs::write(&path, original).unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(&mut app, path.clone(), "# Review memory\n\n- edited away\n");

    app.pr_review_compact_discard();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, original);
}
