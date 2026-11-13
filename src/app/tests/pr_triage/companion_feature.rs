use super::*;

fn triage_target_app(store: ProjectStore) -> App {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree))
}

fn triage_setup(app: &App) -> &crate::app::TriageFeatureSetupState {
    match &app.mode {
        AppMode::PrReview(state) => state
            .new_feature_setup
            .as_ref()
            .expect("triage-feature setup overlay should be open"),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

/// Move the fix-target picker's highlight onto the `New feature…` row.
fn select_new_feature_row(app: &mut App) {
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state
            .harness_pick
            .as_mut()
            .expect("fix-target picker should be open");
        pick.selected = pick
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::pr_review::FixTargetPickRow::NewFeature))
            .expect("New feature… row should be present");
    }
}

#[test]
fn pr_review_fix_target_picker_offers_new_feature_last() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let pick = state.harness_pick.as_ref().expect("picker should be open");
            assert_eq!(
                pick.rows.last(),
                Some(&crate::app::pr_review::FixTargetPickRow::NewFeature),
                "the isolated option is last so it reads as the deliberate choice"
            );
            // …and it is not what the cursor lands on by default.
            assert_ne!(pick.selected, pick.rows.len() - 1);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn choosing_new_feature_opens_setup_instead_of_the_fix_confirm() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none(), "picker closes");
            assert!(
                state.fix_confirm.is_none(),
                "nothing is injected until the feature exists"
            );
            assert!(state.new_feature_setup.is_some(), "setup overlay opens");
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    // The branch is pre-filled from the PR's head branch, and is deliberately
    // NOT that branch — git can't check one branch out in two worktrees.
    let setup = triage_setup(&app);
    assert_eq!(setup.branch, "main-triage");
    assert_ne!(setup.branch, "main");
}

#[test]
fn triage_feature_records_the_prs_own_branch_not_the_checked_out_one() {
    // The documented "other PR" flow triages a PR whose head branch is not what
    // the feature has checked out. `TriageSource::pr_branch` is used verbatim as
    // the `git push origin <triage>:<dest>` destination, so reading it off the
    // source feature would push review fixes onto an unrelated remote branch.
    let repo_dir = TempDir::new().unwrap();
    let repo = repo_dir.path().to_path_buf();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
    };
    git(&repo, &["init", "-q", "-b", "my-feat"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "base"]);
    // The PR lives on its own branch, one commit ahead of what's checked out —
    // so `my-feat` does not even contain the PR head.
    git(&repo, &["checkout", "-q", "-b", "pr-head"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "pr work"]);
    let head_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(&repo, &["checkout", "-q", "my-feat"]);

    let triage_workdir = repo.join(".worktrees").join("pr-head-triage");
    std::fs::create_dir_all(&triage_workdir).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_for_root = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_for_root.clone()));
    let triage_workdir_clone = triage_workdir.clone();
    worktree
        .expect_create_from()
        .times(1)
        // The worktree is branched off the PR's branch, not the checkout's.
        .withf(|_, _, branch, base| branch == "pr-head-triage" && base == "pr-head")
        .returning(move |_, _, _, _| Ok(triage_workdir_clone.clone()));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| false);
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].repo = repo.clone();
    store.projects[0].features[0].workdir = repo.clone();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.clone();
        state.review.pr.head_ref = "pr-head".to_string();
        state.review.pr.head_sha = head_sha.clone();
        state.checked_out_branch = Some("my-feat".to_string());
    }

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();
    // The overlay pre-fills from the PR's branch, and the created feature has
    // to agree with it.
    assert_eq!(triage_setup(&app).branch, "pr-head-triage");
    app.pr_review_triage_setup_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => assert!(
            state.new_feature_setup.is_none(),
            "creation failed: {:?}",
            state
                .new_feature_setup
                .as_ref()
                .and_then(|s| s.error.clone())
        ),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    let companion = app.store.projects[0]
        .features
        .iter()
        .find(|f| f.name == "pr-head-triage")
        .expect("the companion feature should have been created");
    let link = companion
        .triage_source
        .as_ref()
        .expect("the companion is linked back to the PR");
    assert_eq!(
        link.pr_branch, "pr-head",
        "fixes land on the PR's branch, not the source feature's `my-feat`"
    );
    assert_eq!(link.source_feature_id, "feat-1");
}

#[test]
fn triage_setup_branch_is_deduplicated_against_existing_features() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A previous triage feature for this branch already claimed the name.
    let mut earlier = store.projects[0].features[0].clone();
    earlier.id = "feat-2".to_string();
    earlier.name = "main-triage".to_string();
    earlier.workdir = PathBuf::from("/tmp/test-workdir-triage");
    store.projects[0].features.push(earlier);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).branch, "main-triage-2");
}

#[test]
fn triage_setup_rows_cycle_their_values() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Row 0 is Preset; move to Mode and cycle it. This is the setting the
    // whole target exists for — triaging in a different mode than the PR was
    // built in.
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);
    app.pr_review_triage_setup_move(2);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Mode
    );
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibe);
    app.pr_review_triage_setup_adjust(-1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);

    // Booleans toggle regardless of direction.
    app.pr_review_triage_setup_move(1);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Review
    );
    assert!(!triage_setup(&app).review);
    app.pr_review_triage_setup_adjust(1);
    assert!(triage_setup(&app).review);

    // Row movement wraps: from Review (index 3), -4 lands on Branch (index 5).
    app.pr_review_triage_setup_move(-4);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Branch
    );
}

#[test]
fn triage_setup_branch_row_takes_typed_input() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Not on the branch row: typing is not routed here at all.
    assert!(!app.pr_review_triage_setup_on_branch_row());

    app.pr_review_triage_setup_move(6);
    assert!(app.pr_review_triage_setup_on_branch_row());
    app.pr_review_triage_setup_branch_backspace();
    app.pr_review_triage_setup_branch_push('X');
    assert_eq!(triage_setup(&app).branch, "main-triagX");

    // Cycling is a no-op on the text row (nothing to cycle through).
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).branch, "main-triagX");
}

#[test]
fn triage_setup_preset_fills_in_the_rows_below_it() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    app.active_extension.feature_presets = vec![crate::extension::FeaturePreset {
        name: "Careful triage".to_string(),
        branch_prefix: Some("triage/".to_string()),
        mode: VibeMode::Vibeless,
        agent: AgentKind::Codex,
        review: true,
        design_review: false,
        plan_mode: true,
        quick_plan: false,
        enable_chrome: true,
        remote_control: false,
    }];
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).preset_label(), "Manual");
    // Row 0 is the preset row; cycling forward selects the only preset.
    app.pr_review_triage_setup_adjust(1);

    let setup = triage_setup(&app);
    assert_eq!(setup.preset_label(), "Careful triage");
    assert_eq!(setup.agent(), AgentKind::Codex);
    assert!(setup.review);
    assert!(setup.enable_chrome);
    assert_eq!(
        setup.branch, "triage/main-triage",
        "the preset's branch prefix applies to the companion branch too"
    );
}

#[test]
fn cancelling_triage_setup_leaves_the_fix_target_unresolved() {
    // Backing out of the setup means no target was chosen — the next `f` must
    // re-offer every option rather than silently using the default.
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    app.pr_review_triage_setup_cancel();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.new_feature_setup.is_none());
            assert!(state.fix_confirm.is_none());
            assert!(!state.fix_target_picked);
            assert!(!state.pending_batch);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "the picker is offered again"
    );
}

/// Attach a companion triage feature for PR #7 to the store, linked back to
/// the source feature the way `create_triage_feature` persists it.
fn push_companion_feature(store: &mut ProjectStore, agent: AgentKind, mode: VibeMode) {
    let source_id = store.projects[0].features[0].id.clone();
    let mut companion = store.projects[0].features[0].clone();
    companion.id = "feat-triage".to_string();
    companion.name = "main-triage".to_string();
    companion.branch = "main-triage".to_string();
    companion.workdir = PathBuf::from("/tmp/test-workdir/.worktrees/main-triage");
    companion.is_worktree = true;
    companion.agent = agent;
    companion.mode = mode;
    companion.sessions = vec![];
    companion.add_session_named(SessionKind::Claude, "PR Triage".to_string());
    companion.triage_source = Some(crate::project::TriageSource {
        pr_number: 7,
        source_feature_id: source_id,
        // The PR's head branch (`pr_review_with_comments` puts the PR on
        // `main`), not the source feature's own branch — that's what
        // `create_triage_feature` records, and it's the push destination.
        pr_branch: "main".to_string(),
        base_sha: "basesha".to_string(),
    });
    store.projects[0].features.push(companion);
}

#[test]
fn companion_target_resolves_sessions_in_the_triage_feature() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A same-named session in the *source* feature must not be mistaken for
    // the companion's — that would inject fixes into the wrong worktree.
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    assert_eq!(
        app.pr_review_target_feature(),
        Some((0, 1)),
        "the companion feature, not the source one"
    );
}

#[test]
fn opening_a_pr_with_an_existing_companion_adopts_it() {
    // The companion is reused for every fix in the PR, across pane re-opens
    // and restarts — so entering the pane must not re-ask for a target.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.adopt_existing_triage_feature();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::NewFeature
            );
            assert!(state.fix_target_picked, "no re-ask on the next fix");
            assert_eq!(state.review_harness, Some(AgentKind::Codex));
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_none()
            && state.new_feature_setup.is_none()
            && state.fix_confirm.is_some()),
        "the fix goes straight to the confirm dialog in the adopted feature"
    );
}

#[test]
fn adopt_is_a_no_op_without_a_companion_for_this_pr() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    // Same feature, different PR: the link is keyed on the PR number too.
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .pr_number = 99;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if !state.fix_target_picked),
        "an unrelated PR's triage feature is not adopted"
    );
}

#[test]
fn a_vanished_companion_really_does_reopen_the_picker() {
    // The error text promises "press f and pick a target again", so the pane
    // must actually un-resolve the target — both `fix_target_picked` and
    // `review_harness` short-circuit the picker.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    // The companion is deleted mid-visit (from the dashboard, say).
    app.store.projects[0].features.remove(1);
    app.pr_review_inject_fix().unwrap();

    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("no longer exists")),
        "the pane reports it: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(!state.fix_target_picked);
            assert!(
                state.review_harness.is_none(),
                "the dead harness is dropped"
            );
        }
        other => panic!(
            "the pane stays open so `f` is still reachable, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "`f` re-offers every target instead of resolving against the dead one"
    );
}

#[test]
fn fix_confirm_names_the_companion_feature_and_its_mode() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert_eq!(
        app.pr_review_triage_feature_summary().as_deref(),
        Some("main-triage · Codex · Vibeless")
    );

    // The in-feature targets say nothing extra — they run where the user
    // already is.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::DedicatedReview;
    }
    assert!(app.pr_review_triage_feature_summary().is_none());
}

#[test]
fn integrate_is_rejected_for_the_in_feature_targets() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_integrate();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.integrate.is_none()),
        "the dedicated/live targets already commit on the PR branch"
    );
    assert!(!app.toasts.is_empty(), "the user is told why");
}

#[test]
fn integrate_reports_the_commits_and_blocks_cherry_pick_on_a_dirty_source() {
    use crate::app::TriageIntegration;

    // Real git: a source repo with a commit, and a companion worktree branched
    // from it carrying one triage commit.
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("a.txt"), "one\ntwo\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    // Dirty the source worktree — the cherry-pick must refuse rather than
    // clobber in-progress work.
    std::fs::write(source.join("b.txt"), "wip\n").unwrap();

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    app.pr_review_open_integrate();

    let (commits, source_dirty) = match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().expect("overlay should be open");
            assert_eq!(integrate.triage_branch, "main-triage");
            assert_eq!(integrate.pr_branch, "main");
            (integrate.commits.clone(), integrate.source_dirty.clone())
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    };
    assert_eq!(commits.len(), 1, "one triage commit to land");
    assert!(commits[0].contains("apply review comment"));
    assert!(
        source_dirty.is_some(),
        "the dirty source worktree disables the cherry-pick"
    );

    // Confirming the cherry-pick refuses instead of running it, and leaves the
    // source worktree exactly as it was.
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        integrate.selected = TriageIntegration::ALL
            .iter()
            .position(|o| *o == TriageIntegration::CherryPick)
            .unwrap();
    }
    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(
                integrate
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("Cherry-pick refused")),
                "got {:?}",
                integrate.error
            );
            assert!(integrate.done.is_none());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert_eq!(
        std::fs::read_to_string(source.join("a.txt")).unwrap(),
        "one\n",
        "the source worktree is untouched"
    );
}

#[test]
fn integrate_cherry_picks_into_a_clean_source_worktree() {
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("b.txt"), "fix\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }
    app.pr_review_open_integrate();
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        assert!(integrate.source_dirty.is_none());
        integrate.selected = crate::app::TriageIntegration::ALL
            .iter()
            .position(|o| *o == crate::app::TriageIntegration::CherryPick)
            .unwrap();
    }

    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(integrate.error.is_none(), "got {:?}", integrate.error);
            assert!(
                integrate
                    .done
                    .as_deref()
                    .is_some_and(|d| d.contains("Cherry-picked 1 commit")),
                "got {:?}",
                integrate.done
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(
        source.join("b.txt").exists(),
        "the triage commit landed in the source worktree"
    );
}
