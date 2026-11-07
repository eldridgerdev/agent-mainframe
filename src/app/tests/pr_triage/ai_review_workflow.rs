use super::*;

#[test]
fn poll_ai_pr_review_bg_warns_when_reviewing_and_done_arrive_together() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 50_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: String::new(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("Large diff") && t.message.contains("50000")),
        "toasts: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    assert!(
        !app.toasts
            .iter()
            .any(|toast| toast.message.contains("check the debug log")),
        "a valid summary-only response is a clean zero-finding review"
    );
}

#[test]
fn completed_ai_review_updates_stashed_triage_pending_count_and_summary() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![
                sample_ai_review_finding("first"),
                sample_ai_review_finding("second"),
            ],
            summary: Some("Two correctness risks need attention.".to_string()),
            raw_output: "review output".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(
                state.summary.as_deref(),
                Some("Two correctness risks need attention.")
            );
            assert_eq!(state.findings.len(), 2);
        }
        _ => panic!("expected AI Review pane"),
    }
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.pending_ai_review_findings, 2);
            assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(2))
            ));
        }
        _ => panic!("expected stashed PR Triage pane"),
    }

    app.ai_review_toggle_skip();
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => assert_eq!(state.pending_ai_review_findings, 1),
        _ => panic!("expected stashed PR Triage pane"),
    }
}

#[test]
fn completed_ai_review_carries_run_attribution_into_the_pane_and_cache() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let pr = origin.pr.clone();

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    let attribution = crate::app::ai_review::AiReviewAttribution {
        harness: Some("claude".to_string()),
        model: Some("sonnet".to_string()),
        input_tokens: Some(9_000),
        output_tokens: Some(1_200),
        estimated_cost: Some("$0.05".to_string()),
        ..Default::default()
    };
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![sample_ai_review_finding("first")],
            summary: Some("One risk.".to_string()),
            raw_output: "review output".to_string(),
            attribution: attribution.clone(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.attribution.as_ref(), Some(&attribution));
        }
        _ => panic!("expected AI Review pane"),
    }
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_ai_review_cache(pr.number, &pr.head_sha)
        .unwrap()
        .unwrap();
    assert_eq!(cached.attribution.as_ref(), Some(&attribution));
}

#[test]
fn completed_zero_ai_review_updates_visible_triage_immediately() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let origin = match &app.mode {
        AppMode::PrReview(state) => {
            sample_ai_review_state(state.workdir.clone(), state.review.pr.clone())
        }
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin));
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: "## Summary\nNo actionable issues found.".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.pending_ai_review_findings, 0);
            assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(0))
            ));
        }
        _ => panic!("expected visible PR Triage pane"),
    }
}

#[test]
fn ai_review_errors_update_visible_triage_for_agent_and_diff_failures() {
    for detail in ["agent exited with status 1", "failed to fetch PR diff"] {
        let store = store_with_feature(ProjectStatus::Active);
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        enter_pr_review_for_feature(&mut app, 1);
        let origin = match &app.mode {
            AppMode::PrReview(state) => {
                sample_ai_review_state(state.workdir.clone(), state.review.pr.clone())
            }
            _ => unreachable!(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        app.ai_review_run.set_receiver_for_test(Some(rx));
        app.ai_review_run.set_origin_for_test(Some(origin));
        tx.send(crate::app::ai_review::AiReviewProgress::Done(Err(
            anyhow::anyhow!(detail),
        )))
        .unwrap();

        assert!(app.poll_ai_pr_review_bg());
        match &app.mode {
            AppMode::PrReview(state) => assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Error(message))
                    if message == detail
            )),
            _ => panic!("expected visible PR Triage pane"),
        }
    }
}

#[test]
fn disconnected_ai_review_worker_persists_error_and_updates_triage() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    let (workdir, pr) = match &app.mode {
        AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run
        .set_origin_for_test(Some(sample_ai_review_state(workdir, pr.clone())));
    drop(tx);

    assert!(app.poll_ai_pr_review_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Error(message))
                    if message.contains("disconnected")
            )
    ));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_ai_review_cache(pr.number, &pr.head_sha)
        .unwrap()
        .unwrap();
    assert!(matches!(
        cached.last_run.unwrap().outcome,
        crate::app::ai_review::AiReviewRunOutcome::Error(message)
            if message.contains("disconnected")
    ));
}

#[test]
fn escape_keeps_ai_review_running_through_triage_return_and_completion() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    app.cancel_ai_pr_review();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    assert!(app.ai_review_run.is_pending());
    app.close_ai_review();
    match &app.mode {
        AppMode::PrReview(state) => assert!(matches!(
            app.ai_review_triage_status(state),
            crate::app::ai_review::AiReviewTriageStatus::Running
        )),
        _ => panic!("expected restored PR Triage pane"),
    }

    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: "## Summary\nNo actionable issues found.".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(0))
            )
    ));
}

#[test]
fn poll_ai_pr_review_bg_surfaces_streamed_activity_and_usage() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::Reviewing {
                token_estimate: 42_000,
            },
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Usage {
        input_tokens: 41_000,
        output_tokens: 900,
    })
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
            assert_eq!(state.progress.usage, Some((41_000, 900)));
        }
        _ => panic!("expected running AI review"),
    }
}

#[test]
fn running_ai_review_can_reopen_preserved_progress_after_escape() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let started_at = std::time::Instant::now() - std::time::Duration::from_secs(75);
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.ai_review_run
        .set_progress_for_test(Some(crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at,
            activity: None,
            usage: None,
        }));

    // Progress that lands after leaving the full-screen view is retained in
    // the app-level run state rather than discarded with the old mode.
    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 95_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());

    app.start_ai_pr_review();
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(state.progress.started_at, started_at);
            assert_eq!(
                state.progress.stage,
                crate::app::ai_review::AiReviewStage::Reviewing {
                    token_estimate: 95_000
                }
            );
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
        }
        _ => panic!("expected preserved AI review progress"),
    }
}

#[test]
fn open_ai_review_for_pr_reads_the_memory_docs_the_next_run_would_use() {
    use crate::app::review_memory::{MemoryDocState, MemoryScope};

    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n- Two\n",
    )
    .unwrap();
    let mut app = ai_review_test_app();
    // Keeps the test off the developer's real `~/.config/amf/review-memory.md`.
    app.config.global_review_memory_path =
        Some(tmp.path().join("absent-global.md").display().to_string());

    let pr = crate::github::PrRef {
        number: 1,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/1".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    app.open_ai_review_for_pr(repo.clone(), pr);

    let AppMode::AiReview(state) = &app.mode else {
        panic!("expected AiReview");
    };
    let status = state.memory_status.as_ref().expect("status read on open");
    assert_eq!(status.docs[0].scope, MemoryScope::Project);
    assert_eq!(status.docs[0].state, MemoryDocState::Loaded);
    assert_eq!(status.docs[0].findings, 2);
    assert_eq!(
        status.docs[0].path,
        repo.join(".amf").join("review-memory.md")
    );
    assert_eq!(status.docs[1].state, MemoryDocState::Missing);
}

#[test]
fn ai_review_from_linked_worktree_reads_shared_memory_instead_of_local_copy() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("main");
    std::fs::create_dir_all(&repo).unwrap();
    git_out(&repo, &["init", "--initial-branch=main"]);
    std::fs::write(repo.join(".gitignore"), ".amf/\n").unwrap();
    git_out(&repo, &["add", ".gitignore"]);
    git_out(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-m",
            "initial",
        ],
    );
    let workdir = tmp.path().join("feature");
    git_out(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            workdir.to_str().unwrap(),
        ],
    );
    for (root, text) in [
        (&repo, "- Shared one\n- Shared two\n"),
        (&workdir, "- Stray local finding\n"),
    ] {
        std::fs::create_dir_all(root.join(".amf")).unwrap();
        std::fs::write(root.join(".amf/review-memory.md"), text).unwrap();
    }
    let mut app = ai_review_test_app();
    // Exercise the production git-root lookup rather than an identity mock.
    app.worktree = std::sync::Arc::new(crate::worktree::WorktreeManager);
    app.config.global_review_memory_path = Some(tmp.path().join("absent.md").display().to_string());
    app.open_ai_review_for_pr(workdir.clone(), pr_review_with_comments(1).pr);
    let AppMode::AiReview(state) = &app.mode else {
        panic!("expected AiReview")
    };
    let status = state.memory_status.as_ref().unwrap();
    let shared = repo.canonicalize().unwrap().join(".amf/review-memory.md");
    assert_eq!(status.docs[0].path, shared);
    assert_eq!(status.docs[0].findings, 2);
    // The run resolves its input through this same path, keeping display and delivery aligned.
    let paths = app.review_memory_paths(&app.repo_for_project_path(&workdir));
    assert_eq!(paths.project, shared);
    assert_eq!(
        paths.worktree_local,
        Some(
            workdir
                .canonicalize()
                .unwrap()
                .join(".amf/review-memory.md")
        )
    );
    let loaded = crate::app::review_memory::load_review_memory(&paths);
    assert!(loaded.context.contains("Shared one"));
    assert!(!loaded.context.contains("Stray local"));
}

#[test]
fn open_ai_review_for_pr_starts_empty_with_no_stashed_return() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    // A stash left over from a previous PR Triage-originated visit must not
    // leak into a fresh dashboard/session/picker entry.
    let pr = crate::github::PrRef {
        number: 1,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/1".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    app.ai_review_return_to = Some(Box::new(AppMode::Normal));

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr.clone());

    assert!(app.ai_review_return_to.is_none());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert!(state.findings.is_empty());
            assert_eq!(state.pr.number, 1);
            assert_eq!(state.workdir, PathBuf::from("/tmp/test-workdir"));
        }
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn open_ai_review_for_pr_reopens_cached_findings_and_summary() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let pr = pr_review_with_comments(1).pr;
    app.db
        .as_ref()
        .unwrap()
        .save_ai_review_cache(
            pr.number,
            &pr.head_sha,
            &crate::app::ai_review::AiReviewCacheEntry {
                findings: vec![sample_ai_review_finding("cached finding")],
                last_run: Some(crate::app::ai_review::AiReviewRun {
                    ran_at: chrono::Local::now(),
                    outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
                }),
                summary: Some("Cached review summary.".to_string()),
                attribution: None,
            },
        )
        .unwrap();
    assert_eq!(app.ai_review_triage_snapshot(&pr).pending_findings, 1);

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr);

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.findings[0].body, "cached finding");
            assert_eq!(state.summary.as_deref(), Some("Cached review summary."));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn seed_ai_review_fixture_persists_and_opens_without_running_an_agent() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());

    let response = app
        .seed_ai_review_from_request(&SeedAiReviewRequest {
            pr_number: 55,
            head_sha: "fixture-sha".to_string(),
            summary: "Deterministic completed review.".to_string(),
            findings: vec![SeedAiReviewFinding {
                body: "A deterministic finding.".to_string(),
                ..Default::default()
            }],
            open: true,
            workdir: Some(workdir.path().to_path_buf()),
            repository: Some("owner/repository".to_string()),
            head_ref: Some("fixture-branch".to_string()),
            attribution: None,
        })
        .unwrap();

    assert!(response.ok);
    assert!(response.opened);
    assert_eq!(response.finding_count, 1);
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.pr.number, 55);
            assert_eq!(state.pr.head_sha, "fixture-sha");
            assert_eq!(state.findings[0].body, "A deterministic finding.");
            assert_eq!(
                state.summary.as_deref(),
                Some("Deterministic completed review.")
            );
        }
        _ => panic!("expected seeded AI Review pane"),
    }
}

#[test]
fn pr_triage_zero_result_survives_reopen_and_restart_but_not_new_head() {
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let review = pr_review_with_comments(1);
    let zero_run = crate::app::ai_review::AiReviewRun {
        ran_at: chrono::Local::now(),
        outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(0),
    };

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_path).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&review)
        .unwrap();
    app.db
        .as_ref()
        .unwrap()
        .save_ai_review_cache(
            review.pr.number,
            &review.pr.head_sha,
            &crate::app::ai_review::AiReviewCacheEntry {
                findings: vec![],
                last_run: Some(zero_run.clone()),
                summary: None,
                attribution: None,
            },
        )
        .unwrap();

    app.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));
    app.mode = AppMode::Normal;
    app.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));
    drop(app);

    let store = store_with_feature(ProjectStatus::Idle);
    let mut restarted = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    restarted.db = Some(crate::db::AmfDb::open(&db_path).unwrap());
    restarted.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &restarted.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));

    let mut moved = review;
    moved.pr.head_sha = "new-head".to_string();
    restarted
        .db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&moved)
        .unwrap();
    restarted.enter_pr_review(PathBuf::from("/tmp/test-workdir"), moved.pr.clone());
    assert!(matches!(
        &restarted.mode,
        AppMode::PrReview(state)
            if state.review.pr.head_sha == "new-head"
                && state.ai_review_last_run.is_none()
                && state.pending_ai_review_findings == 0
    ));
}

#[test]
fn refreshed_pr_head_clears_stale_ai_review_completion() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.ai_review_last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(0),
        });
    }
    app.open_ai_review_from_triage();
    let (workdir, old_pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let mut moved = pr_review_with_comments(2);
    moved.pr.head_sha = "new-head".to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir,
        pr: old_pr,
    });
    tx.send(Ok(moved)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.review.pr.head_sha, "new-head");
            assert!(state.ai_review_last_run.is_none());
            assert_eq!(state.pending_ai_review_findings, 0);
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn ai_review_post_dialog_is_seeded_with_generated_summary_and_attribution() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings = vec![sample_ai_review_finding("A general finding")];
        state.summary = Some("The patch has one correctness risk.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(body.starts_with("The patch has one correctness risk."));
            assert!(body.contains("A general finding"));
            assert!(body.ends_with("— AI review via AMF"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_seeds_usage_summary_only_on_the_overall_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        let mut anchored = sample_ai_review_finding("An anchored finding");
        anchored.path = Some("src/lib.rs".to_string());
        anchored.line = Some(10);
        anchored.side = Some(crate::diff::DiffSide::New);
        anchored.diff_hunk = Some("@@ -1 +1 @@".to_string());
        state.findings = vec![anchored];
        state.summary = Some("One risk.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
        });
        state.attribution = Some(crate::app::ai_review::AiReviewAttribution {
            harness: Some("claude".to_string()),
            model: Some("sonnet".to_string()),
            input_tokens: Some(12_300),
            output_tokens: Some(4_500),
            estimated_cost: Some("$0.10".to_string()),
            ..Default::default()
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let post = state.post_confirm.as_ref().unwrap();
            let body = post.editor.text();
            assert!(
                body.contains(
                    "<details>\n<summary>AI review usage · harness claude · model sonnet"
                ) && body.contains("\n\n- Harness: claude\n- Model: sonnet"),
                "summary should carry the usage section: {body}"
            );
            assert!(body.contains("Input tokens: 12.3k"));
            assert!(body.contains("Estimated cost: $0.10"));
            assert!(body.ends_with("— AI review via AMF"));
            assert!(
                !post.inline[0].body.contains("AI review usage"),
                "inline comment must not carry the usage section: {}",
                post.inline[0].body
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_drops_generated_summary_when_a_finding_is_skipped() {
    // Regression: `state.summary` is model prose written over the *complete*
    // finding set, so it can still describe a finding the user has since
    // skipped (a false positive, or one too sensitive to post) even though
    // that finding is excluded from the posted findings below it. Once
    // anything's skipped, the dialog must fall back to the generic
    // placeholder rather than risk republishing what `skipped` was meant to
    // suppress.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        let mut skipped = sample_ai_review_finding("A sensitive finding");
        skipped.skipped = true;
        state.findings = vec![skipped, sample_ai_review_finding("A kept finding")];
        state.summary = Some("The patch has a sensitive issue plus a kept finding.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(2),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(
                !body.contains("sensitive issue"),
                "the model summary describing the skipped finding must not be posted: {body}"
            );
            assert!(body.starts_with("AI review, via AMF."));
            assert!(body.contains("A kept finding"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn open_ai_review_from_triage_stashes_the_pane_and_close_restores_it() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let triage_workdir = match &app.mode {
        AppMode::PrReview(state) => state.workdir.clone(),
        _ => unreachable!(),
    };

    app.open_ai_review_from_triage();

    match &app.mode {
        AppMode::AiReview(state) => assert_eq!(state.workdir, triage_workdir),
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.ai_review_return_to.is_some());

    app.close_ai_review();
    assert!(matches!(&app.mode, AppMode::PrReview(_)));
    assert!(app.ai_review_return_to.is_none());
}

#[test]
fn post_success_refresh_updates_stashed_triage_without_leaving_ai_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        state.hide_resolved = true;
        state.marked.insert(2);
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.review.comments.len(), 3);
            assert_eq!(state.selected_comment().map(|comment| comment.id), Some(2));
            assert!(state.hide_resolved);
            assert!(state.marked.contains(&2));
            assert_eq!(state.pending_ai_review_findings, 0);
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_resolved_comment() {
    // Regression: restoring the selection by id after a refresh can land it
    // on a comment that `hide_resolved` now excludes (its thread resolved
    // upstream since the last fetch), leaving `selected` pointing at a row
    // `visible_indices()` doesn't include.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 3);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2, not yet resolved
        state.hide_resolved = true;
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });

    // The refresh comes back with comment 2's thread now resolved.
    let comments: Vec<crate::github::ReviewComment> = (1..=3u64)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let threads = vec![crate::github::ReviewThread {
        id: "T2".to_string(),
        is_resolved: true,
        comment_ids: vec![2],
    }];
    let refreshed_pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let refreshed =
        crate::app::pr_review::normalize(refreshed_pr, comments, vec![], vec![], threads);
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on a comment hide_resolved now excludes"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_collated_amf_reply() {
    use crate::app::pr_review::AMF_ATTRIBUTION_FOOTER;

    // Regression: `selected` can itself be an orphaned AMF follow-up reply
    // (its root wasn't fetched, so it stood as its own row). If a refresh
    // fetches the root, the reply becomes collated under it and vanishes
    // from `visible_indices()`. `all_sorted_indices()` used to also drop
    // collated replies, so `position()` couldn't find `selected` there
    // either and both neighbor searches short-circuited to `visible[0]`
    // instead of the nearest neighbor (the reply's own root).
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    enter_pr_review_for_feature(&mut app, 1);

    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let user = |login: &str| crate::github::GhUser {
        login: login.to_string(),
        kind: "User".to_string(),
    };

    // An unrelated comment that sorts before the reply's root in fetch order.
    // Its presence is what distinguishes "snap to the nearest neighbor" from
    // "snap to the first visible row" — with only the reply and its root,
    // the root would also happen to be `visible[0]`, masking the bug.
    let unrelated = crate::github::ReviewComment {
        id: 3,
        path: Some("src/other.rs".into()),
        line: Some(4),
        original_line: Some(4),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Unrelated finding.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: None,
    };

    // Initial fetch: the AMF reply came back but its root did not, so it's
    // orphaned and shown as its own row. It's selected.
    let orphan_reply = crate::github::ReviewComment {
        id: 2,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}"),
        user: user("author"),
        in_reply_to_id: Some(1),
        pull_request_review_id: Some(91),
    };
    app.mode = AppMode::PrReview(PrReviewState {
        selected: 1,
        review: crate::app::pr_review::normalize(
            pr.clone(),
            vec![unrelated.clone(), orphan_reply.clone()],
            vec![],
            vec![],
            vec![],
        ),
        ..match app.mode {
            AppMode::PrReview(state) => state,
            _ => unreachable!(),
        }
    });

    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir,
        pr: pr.clone(),
    });

    // The refresh comes back with the reply's root now present, so the
    // reply collates under it and drops out of `visible_indices()`.
    let root = crate::github::ReviewComment {
        id: 1,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Please add a test here.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: Some(90),
    };
    let refreshed = crate::app::pr_review::normalize(
        pr,
        vec![unrelated, root, orphan_reply],
        vec![],
        vec![],
        vec![],
    );
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on the now-collated AMF reply"
            );
            assert_eq!(
                state.selected_comment().map(|comment| comment.id),
                Some(1),
                "selection should snap to the reply's own root, its nearest visible neighbor"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_caches_fresh_triage_when_ai_review_has_no_return_pane() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(worktree_rooted_at_workdir()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let stale = pr_review_with_comments(1);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&stale)
        .unwrap();
    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), stale.pr.clone());

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir: PathBuf::from("/tmp/test-workdir"),
        pr: stale.pr.clone(),
    });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_review_cache(stale.pr.number, &stale.pr.head_sha)
        .unwrap()
        .unwrap();
    assert_eq!(cached.comments.len(), 3);
}

#[test]
fn close_ai_review_with_no_stash_returns_to_the_dashboard() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    assert!(app.ai_review_return_to.is_none());

    app.close_ai_review();

    assert!(matches!(&app.mode, AppMode::Normal));
}
