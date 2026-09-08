use super::support::*;
use crate::app::attention::{AttentionSource, AttentionState};
use crate::app::sync::pane_shows_thinking_hint;
use crate::app::util::{latest_prompt_path, read_latest_prompt};
use crate::app::*;
use crate::project::{
    AgentKind, Feature, FeatureSession, Project, SessionKind, TokenUsageSourceMatch,
};
use crate::token_tracking::{SessionTokenTracker, TokenUsageProvider, TokenUsageSource};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::{TimeZone, Utc};
use std::collections::HashMap;
use tempfile::NamedTempFile;
use tempfile::TempDir;

#[test]
fn read_latest_prompt_prefers_claude_path() {
    let workdir = TempDir::new().unwrap();
    let claude_path = latest_prompt_path(workdir.path());
    let codex_path = workdir.path().join(".codex").join("latest-prompt.txt");
    std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    std::fs::write(&claude_path, "claude prompt").unwrap();
    std::fs::write(&codex_path, "codex prompt").unwrap();

    assert_eq!(
        read_latest_prompt(workdir.path()).as_deref(),
        Some("claude prompt")
    );
}

#[test]
fn read_latest_prompt_falls_back_to_codex_path() {
    let workdir = TempDir::new().unwrap();
    let codex_path = workdir.path().join(".codex").join("latest-prompt.txt");
    std::fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    std::fs::write(&codex_path, "codex prompt").unwrap();

    assert_eq!(
        read_latest_prompt(workdir.path()).as_deref(),
        Some("codex prompt")
    );
}

#[test]
fn poll_sidebar_load_results_updates_feature_caches() {
    let repo = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.pending_sidebar_loads.insert("amf-my-feat".to_string());
    app.sidebar_load_tx
        .send(crate::app::SidebarLoadResult {
            tmux_session: "amf-my-feat".to_string(),
            signature: 7,
            changed: true,
            latest_prompt: Some("lazy prompt".to_string()),
            model_text: Some("Model: openai/gpt-5.5".to_string()),
            opencode_sidebar: Some(crate::app::opencode_storage::OpencodeSidebarData {
                session_id: "ses-1".to_string(),
                title: Some("Loaded later".to_string()),
                latest_prompt: Some("lazy prompt".to_string()),
                status: Some("busy".to_string()),
                last_tool: Some("edit".to_string()),
                todo_count: Some(2),
                todo_preview: vec!["finish parser".to_string(), "wire UI".to_string()],
                pending_permission: None,
                last_error: None,
                lsp_summary: Some("ready".to_string()),
                live_summary: Some("live summary".to_string()),
                model: Some("gpt-5.5".to_string()),
                provider: Some("openai".to_string()),
                reasoning_tokens: Some(12),
                additions: Some(3),
                deletions: Some(1),
                files: Some(1),
            }),
            plan_text: "Current: AMF_PLAN.md".to_string(),
        })
        .unwrap();

    app.poll_sidebar_load_results();

    assert_eq!(
        app.latest_prompt_for_session("amf-my-feat"),
        Some("lazy prompt")
    );
    assert_eq!(
        app.sidebar_effective_plan_cache
            .get("amf-my-feat")
            .map(String::as_str),
        Some("Current: AMF_PLAN.md")
    );
    assert_eq!(
        app.opencode_sidebar_cache
            .get("amf-my-feat")
            .and_then(|data| data.title.as_deref()),
        Some("Loaded later")
    );
    assert_eq!(
        app.sidebar_model_cache
            .get("amf-my-feat")
            .map(String::as_str),
        Some("Model: openai/gpt-5.5")
    );
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

// ── thinking hint parsing ─────────────────────────────────

#[test]
fn pane_shows_thinking_hint_detects_supported_markers() {
    assert!(pane_shows_thinking_hint("Esc to interrupt"));
    assert!(pane_shows_thinking_hint("press ESC interrupt to stop"));
    assert!(pane_shows_thinking_hint("Ctrl+C to interrupt"));
}

#[test]
fn pane_shows_thinking_hint_ignores_unrelated_text() {
    assert!(!pane_shows_thinking_hint("waiting for input"));
    assert!(!pane_shows_thinking_hint("all done"));
}

// ── sync_statuses ─────────────────────────────────────────────

#[test]
fn sync_statuses_stopped_becomes_idle_when_session_live() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions()
        .times(1)
        .returning(|| Ok(vec!["amf-my-feat".to_string()]));

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.sync_statuses();

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Idle
    );
}

#[test]
fn sync_statuses_active_becomes_stopped_when_session_gone() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions()
        .times(1)
        .returning(|| Ok(vec![]));

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.pending_sidebar_loads.insert("amf-my-feat".to_string());
    app.latest_prompt_cache
        .insert("amf-my-feat".to_string(), prompt_entry("cached prompt"));
    app.opencode_sidebar_cache.insert(
        "amf-my-feat".to_string(),
        crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".to_string(),
            title: Some("cached".to_string()),
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: None,
            todo_preview: Vec::new(),
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: None,
            model: None,
            provider: None,
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        },
    );
    app.sync_statuses();

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    assert!(app.latest_prompt_for_session("amf-my-feat").is_none());
    assert!(!app.opencode_sidebar_cache.contains_key("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn sync_statuses_idle_stays_idle_when_session_live() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions()
        .times(1)
        .returning(|| Ok(vec!["amf-my-feat".to_string()]));

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.sync_statuses();

    // Already Idle; stays Idle (not overwritten)
    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Idle
    );
}

/// A sweep cut short by the rate limit must leave the badges it never looked
/// at exactly as they were. Blanking them would report "no PR" for features
/// that certainly still have one.
#[test]
fn a_skipped_pr_lookup_leaves_the_previous_badge_untouched() {
    use crate::app::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();

    app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch: branch.clone(),
            head_sha: "abc123".to_string(),
            number: 321,
            unresolved_threads: Some(4),
        }),
    }]);

    let changed = app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: branch.clone(),
        lookup: ActivePrLookup::Skipped,
    }]);

    assert!(!changed, "a skipped lookup is not a change");
    let pr = app.active_pr_for_feature(&feature_id).unwrap();
    assert_eq!(pr.number, 321);
    assert_eq!(pr.unresolved_threads, Some(4), "the count is preserved");
}

/// Once the budget is known gone, the sweep must stand down — otherwise the
/// 5-minute timer keeps spending calls that can only fail, and keeps the
/// budget at zero as it tries to refill.
#[test]
fn hitting_the_graphql_limit_pauses_the_pr_sweep_and_says_so() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    assert!(app.gh_graphql_backoff_remaining().is_none());

    app.note_gh_graphql_rate_limited();

    let remaining = app
        .gh_graphql_backoff_remaining()
        .expect("backoff is running");
    assert!(remaining <= crate::app::GH_GRAPHQL_BACKOFF);
    assert!(!app.toasts.is_empty(), "the pause is announced, not silent");

    // The sweep refuses to start while backing off.
    app.sync_active_prs_background();
    assert!(
        app.active_pr_bg.is_none(),
        "no worker is spawned during backoff"
    );
}

/// Being rate-limited twice must not stack up toasts; the user already knows.
#[test]
fn a_repeated_rate_limit_extends_the_pause_without_re_announcing_it() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.note_gh_graphql_rate_limited();
    let announced = app.toasts.len();
    app.note_gh_graphql_rate_limited();

    assert_eq!(app.toasts.len(), announced, "announced once, not twice");
    assert!(app.gh_graphql_backoff_remaining().is_some());
}

/// The line has to distinguish a healthy quiet sweep from a broken one. Both
/// badge nothing; only one of them failed, and for fourteen hours nothing said
/// which was happening.
#[test]
fn the_sweep_summary_tells_a_quiet_sweep_apart_from_a_broken_one() {
    use crate::app::sync::ActivePrSweepOutcome;

    let mut healthy = ActivePrSweepOutcome::default();
    for _ in 0..9 {
        healthy.count(&crate::app::sync::ActivePrLookup::NoPr);
    }
    let healthy = healthy.summary();
    assert!(healthy.contains("0 badged"));
    assert!(healthy.contains("9 without a PR"));
    assert!(healthy.contains("0 failed"), "{healthy}");

    let mut broken = ActivePrSweepOutcome::default();
    for _ in 0..9 {
        broken.count(&crate::app::sync::ActivePrLookup::Failed(
            "bad query".into(),
        ));
    }
    let broken = broken.summary();
    assert!(broken.contains("9 failed"), "{broken}");
    assert_ne!(healthy, broken, "the two states must not read alike");
}

#[test]
fn active_pr_updates_cache_current_branch_and_remove_confirmed_absence() {
    use crate::app::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();

    assert!(app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch: branch.clone(),
            head_sha: "abc123".to_string(),
            number: 321,
            unresolved_threads: Some(4),
        }),
    }]));
    assert_eq!(app.active_pr_for_feature(&feature_id).unwrap().number, 321);
    assert_eq!(
        app.active_pr_for_feature(&feature_id)
            .unwrap()
            .unresolved_threads,
        Some(4)
    );

    // A result started for an old branch cannot overwrite the current badge.
    assert!(!app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: "old-branch".to_string(),
        lookup: ActivePrLookup::NoPr,
    }]));
    assert!(app.active_pr_for_feature(&feature_id).is_some());

    assert!(app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch,
        lookup: ActivePrLookup::NoPr,
    }]));
    assert!(app.active_pr_for_feature(&feature_id).is_none());
}

#[test]
fn failed_active_pr_refresh_preserves_last_known_badge() {
    use crate::app::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "abc123".to_string(),
            number: 321,
            unresolved_threads: Some(1),
        },
    );

    assert!(!app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch,
        lookup: ActivePrLookup::Failed("network unavailable".to_string()),
    }]));
    assert_eq!(app.active_pr_for_feature(&feature_id).unwrap().number, 321);
}

#[test]
fn active_pr_successor_replaces_badge_and_invalidates_old_pr_targets() {
    use crate::app::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let old_state = match &mut app.mode {
        AppMode::PrReview(state) => {
            state.review.pr.number = 449;
            state.clone()
        }
        _ => unreachable!(),
    };
    app.pr_review_return = Some(PrReviewReturn {
        session: "amf-my-feat".to_string(),
        window: "pr-triage".to_string(),
        state: old_state.clone(),
    });
    let mut old_ai_pr = old_state.review.pr.clone();
    old_ai_pr.number = 449;
    app.ai_review_pending = Some(sample_ai_review_state(old_state.workdir.clone(), old_ai_pr));

    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "old-head".to_string(),
            number: 449,
            unresolved_threads: Some(0),
        },
    );
    assert!(app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch,
            head_sha: "new-head".to_string(),
            number: 450,
            unresolved_threads: Some(2),
        }),
    }]));

    assert_eq!(app.active_pr_for_feature(&feature_id).unwrap().number, 450);
    assert!(app.pr_review_return.is_none());
    assert!(app.ai_review_pending.is_none());
    // Explicitly opened closed PRs remain viewable; only implicit restore
    // targets are invalidated by discovering the current open successor.
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state) if state.review.pr.number == 449
    ));
}

#[test]
fn active_pr_successor_prevents_returning_to_stashed_predecessor() {
    use crate::app::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let old_state = match &mut app.mode {
        AppMode::PrReview(state) => {
            state.review.pr.number = 449;
            state.clone()
        }
        _ => unreachable!(),
    };
    app.pr_review_return = Some(PrReviewReturn {
        session: "amf-my-feat".to_string(),
        window: "pr-triage".to_string(),
        state: old_state,
    });
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "pr-triage".to_string(),
        "PR Triage".to_string(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    ));

    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "old-head".to_string(),
            number: 449,
            unresolved_threads: Some(0),
        },
    );
    app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id,
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch,
            head_sha: "new-head".to_string(),
            number: 450,
            unresolved_threads: Some(0),
        }),
    }]);
    app.pr_review_return_to_pane();

    assert!(matches!(&app.mode, AppMode::Viewing(_)));
    assert!(app.pr_review_return.is_none());
}

#[test]
fn unchanged_active_pr_badge_does_not_cancel_explicit_closed_pr_fetch() {
    use crate::app::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "open-head".to_string(),
            number: 450,
            unresolved_threads: Some(0),
        },
    );
    let (_tx, rx) = std::sync::mpsc::channel();
    app.pr_review_bg = Some(rx);
    app.mode = AppMode::PrReviewLoading(crate::app::PrReviewLoadState {
        workdir: feature.workdir.clone(),
        pr: crate::github::PrRef {
            number: 449,
            head_sha: "closed-head".to_string(),
            url: "https://github.com/o/r/pull/449".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            head_ref: "main".to_string(),
        },
        usage_baselines: std::collections::HashMap::new(),
    });

    assert!(!app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id,
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch,
            head_sha: "open-head".to_string(),
            number: 450,
            unresolved_threads: Some(0),
        }),
    }]));

    assert!(matches!(
        &app.mode,
        AppMode::PrReviewLoading(state) if state.pr.number == 449
    ));
    assert!(app.pr_review_bg.is_some());
}

/// A `Found` terminal result is durable: it lands in the in-memory cache and,
/// when a DB is attached, is persisted so a restart doesn't lose it.
#[test]
fn terminal_pr_found_updates_cache_and_persists_to_db() {
    use crate::app::sync::{TerminalPrLookup, TerminalPrUpdate};
    use crate::db::AmfDb;
    use crate::github::{TerminalPr, TerminalPrState};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = NamedTempFile::new().unwrap();
    app.db = Some(AmfDb::open(db_file.path()).unwrap());

    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    let repo = app.store.projects[0].repo.clone();

    let pr = TerminalPr {
        number: 42,
        state: TerminalPrState::Merged,
        at: "2026-01-01T00:00:00Z".to_string(),
    };

    assert!(app.apply_terminal_pr_updates(vec![TerminalPrUpdate {
        feature_id: feature_id.clone(),
        repo: repo.clone(),
        branch: branch.clone(),
        lookup: TerminalPrLookup::Found(pr.clone()),
    }]));
    assert_eq!(app.terminal_pr_for_feature(&feature_id), Some(&pr));

    let saved = app
        .db
        .as_ref()
        .unwrap()
        .load_all_pr_terminal_state()
        .unwrap();
    assert_eq!(
        saved.get(&(repo.to_string_lossy().to_string(), branch)),
        Some(&pr)
    );
}

/// A result addressed to a branch the feature no longer has (it was renamed,
/// or the update raced a branch switch) must not overwrite the current
/// terminal badge — mirrors the same guard on `apply_active_pr_updates`.
#[test]
fn terminal_pr_update_for_stale_branch_does_not_overwrite_current_badge() {
    use crate::app::sync::{TerminalPrLookup, TerminalPrUpdate};
    use crate::github::{TerminalPr, TerminalPrState};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let repo = app.store.projects[0].repo.clone();

    let changed = app.apply_terminal_pr_updates(vec![TerminalPrUpdate {
        feature_id: feature_id.clone(),
        repo,
        branch: "old-branch".to_string(),
        lookup: TerminalPrLookup::Found(TerminalPr {
            number: 1,
            state: TerminalPrState::Merged,
            at: "2026-01-01T00:00:00Z".to_string(),
        }),
    }]);

    assert!(!changed);
    assert!(app.terminal_pr_for_feature(&feature_id).is_none());
}

/// `Skipped` and `Failed` terminal lookups must leave a previously confirmed
/// merged/closed badge exactly as it was — the sweep never had new evidence.
#[test]
fn skipped_or_failed_terminal_lookup_leaves_previous_badge_untouched() {
    use crate::app::sync::{TerminalPrLookup, TerminalPrUpdate};
    use crate::github::{TerminalPr, TerminalPrState};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    let repo = app.store.projects[0].repo.clone();
    let pr = TerminalPr {
        number: 7,
        state: TerminalPrState::Closed,
        at: "2026-01-01T00:00:00Z".to_string(),
    };
    app.terminal_prs.insert(feature_id.clone(), pr.clone());

    let changed = app.apply_terminal_pr_updates(vec![
        TerminalPrUpdate {
            feature_id: feature_id.clone(),
            repo: repo.clone(),
            branch: branch.clone(),
            lookup: TerminalPrLookup::Skipped,
        },
        TerminalPrUpdate {
            feature_id: feature_id.clone(),
            repo,
            branch,
            lookup: TerminalPrLookup::Failed("network unavailable".to_string()),
        },
    ]);

    assert!(!changed);
    assert_eq!(app.terminal_pr_for_feature(&feature_id), Some(&pr));
}

/// The bug this pins down: without a negative cache, a branch that never has
/// a PR would trigger a fresh `GhCli::terminal_prs` call on every sweep,
/// forever. `NoPr` must settle into `confirmed_no_terminal_pr` so the next
/// sweep's `needs_terminal` computation (gated on `known_terminal_ids`) skips
/// it — and that settled answer must be cleared the moment the branch shows
/// an open PR again, since a *new* PR on that branch can still reach a
/// terminal state later.
#[test]
fn confirmed_no_terminal_pr_is_cached_and_cleared_when_branch_reopens() {
    use crate::app::sync::{ActivePrLookup, ActivePrUpdate, TerminalPrLookup, TerminalPrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    let repo = app.store.projects[0].repo.clone();

    let changed = app.apply_terminal_pr_updates(vec![TerminalPrUpdate {
        feature_id: feature_id.clone(),
        repo,
        branch: branch.clone(),
        lookup: TerminalPrLookup::NoPr,
    }]);
    assert!(!changed, "a settled negative isn't a badge change");
    assert!(app.confirmed_no_terminal_pr.contains(&feature_id));
    assert!(app.terminal_pr_for_feature(&feature_id).is_none());

    // The branch gets a PR again; the settled "never had one" answer must not
    // survive that, or a later merge/close on this new PR would never be
    // looked up again.
    app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch,
            head_sha: "abc123".to_string(),
            number: 99,
            unresolved_threads: Some(0),
        }),
    }]);
    assert!(!app.confirmed_no_terminal_pr.contains(&feature_id));
}

/// Seeded once at startup, before the first sweep: a matching `(repo,
/// branch)` row lands in `terminal_prs`, and a row for a repo/branch this
/// store doesn't have is simply not applied.
#[test]
fn load_terminal_prs_from_db_seeds_only_matching_features() {
    use crate::db::AmfDb;
    use crate::github::{TerminalPr, TerminalPrState};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature_id = app.store.projects[0].features[0].id.clone();
    let repo = app.store.projects[0].repo.to_string_lossy().to_string();
    let branch = app.store.projects[0].features[0].branch.clone();

    let db_file = NamedTempFile::new().unwrap();
    let db = AmfDb::open(db_file.path()).unwrap();
    let pr = TerminalPr {
        number: 5,
        state: TerminalPrState::Merged,
        at: "2026-01-01T00:00:00Z".to_string(),
    };
    db.save_pr_terminal_state(&repo, &branch, &pr).unwrap();
    db.save_pr_terminal_state("/some/other/repo", "unrelated-branch", &pr)
        .unwrap();
    app.db = Some(db);

    assert!(app.terminal_pr_for_feature(&feature_id).is_none());
    app.load_terminal_prs_from_db();

    assert_eq!(app.terminal_pr_for_feature(&feature_id), Some(&pr));
    assert_eq!(app.terminal_prs.len(), 1, "the unrelated row isn't applied");
}

#[test]
fn predecessor_invalidation_preserves_live_successor_ai_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let successor = match &mut app.mode {
        AppMode::PrReview(state) => {
            state.review.pr.number = 450;
            state.clone()
        }
        _ => unreachable!(),
    };
    app.pr_review_return = Some(PrReviewReturn {
        session: "amf-my-feat".to_string(),
        window: "pr-triage".to_string(),
        state: successor.clone(),
    });
    let ai_origin = sample_ai_review_state(successor.workdir.clone(), successor.review.pr.clone());
    let (_tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    app.ai_review_pending = Some(ai_origin.clone());
    app.ai_review_progress = Some(crate::app::AiReviewRunProgress {
        stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
        started_at: std::time::Instant::now(),
        activity: None,
        usage: None,
    });
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin: ai_origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    assert!(
        !app.invalidate_pr_context_for_transition(std::path::Path::new("/tmp/test-workdir"), 449,)
    );

    assert!(app.pr_review_return.is_some());
    assert!(app.ai_review_pending.is_some());
    assert!(app.ai_review_bg.is_some());
    assert!(app.ai_review_progress.is_some());
    assert!(matches!(
        &app.mode,
        AppMode::AiReviewRunning(state) if state.origin.pr.number == 450
    ));
}

#[test]
fn sync_thinking_status_drains_sidebar_results_for_opencode_features() {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].agent = AgentKind::Opencode;

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.pending_sidebar_loads.insert("amf-my-feat".to_string());
    app.sidebar_load_tx
        .send(crate::app::SidebarLoadResult {
            tmux_session: "amf-my-feat".to_string(),
            signature: 9,
            changed: true,
            latest_prompt: Some("warm prompt".to_string()),
            model_text: None,
            opencode_sidebar: Some(crate::app::opencode_storage::OpencodeSidebarData {
                session_id: "ses-1".to_string(),
                title: Some("Warm cache".to_string()),
                latest_prompt: Some("warm prompt".to_string()),
                status: Some("busy".to_string()),
                last_tool: Some("edit".to_string()),
                todo_count: Some(1),
                todo_preview: vec!["finish parser".to_string()],
                pending_permission: None,
                last_error: None,
                lsp_summary: None,
                live_summary: None,
                model: None,
                provider: None,
                reasoning_tokens: None,
                additions: None,
                deletions: None,
                files: None,
            }),
            plan_text: "No plan selected".to_string(),
        })
        .unwrap();

    assert!(app.sync_thinking_status());
    assert_eq!(
        app.latest_prompt_for_session("amf-my-feat"),
        Some("warm prompt")
    );
    assert_eq!(
        app.opencode_sidebar_cache
            .get("amf-my-feat")
            .and_then(|data| data.title.as_deref()),
        Some("Warm cache")
    );
    assert!(app.is_feature_thinking("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn ipc_opencode_sidebar_update_queues_sidebar_refresh() {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].agent = AgentKind::Opencode;

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "opencode-sidebar-updated",
        "source": "opencode-sidebar",
        "session_id": "ses-1",
        "cwd": repo.path(),
    }));

    assert!(app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn sync_thinking_status_keeps_a_question_raised_while_the_session_is_busy() {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].agent = AgentKind::Opencode;

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    let sidebar_data = |status: &str| crate::app::opencode_storage::OpencodeSidebarData {
        session_id: "ses-1".to_string(),
        title: None,
        latest_prompt: None,
        status: Some(status.to_string()),
        last_tool: None,
        todo_count: None,
        todo_preview: vec![],
        pending_permission: None,
        last_error: None,
        lsp_summary: None,
        live_summary: None,
        model: None,
        provider: None,
        reasoning_tokens: None,
        additions: None,
        deletions: None,
        files: None,
    };

    app.opencode_sidebar_cache
        .insert("amf-my-feat".to_string(), sidebar_data("busy"));
    app.sync_thinking_status();

    // The harness asks something without going idle first: OpenCode stays busy
    // while its `question` tool is open, and Claude keeps its thinking marker
    // through a permission prompt. Polling must not retire it.
    app.record_attention(
        "amf-my-feat",
        &AgentKind::Opencode,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.sync_thinking_status();
    app.sync_thinking_status();
    assert_eq!(
        app.attention_for("amf-my-feat").map(|r| r.state),
        Some(AttentionState::Question),
        "a question must survive polls while the session is still busy"
    );

    // Going idle does not retire it either — that is what a blocked agent
    // looks like from here, so the inferred completion must not win.
    app.opencode_sidebar_cache
        .insert("amf-my-feat".to_string(), sidebar_data("idle"));
    app.sync_thinking_status();
    assert_eq!(
        app.attention_for("amf-my-feat").map(|r| r.state),
        Some(AttentionState::Question)
    );

    // Working again is the actual new-output transition, and clears it.
    app.opencode_sidebar_cache
        .insert("amf-my-feat".to_string(), sidebar_data("busy"));
    app.sync_thinking_status();
    assert!(app.attention_for("amf-my-feat").is_none());
}

#[test]
fn sync_thinking_status_raises_review_ready_after_watched_session_finishes() {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].agent = AgentKind::Opencode;

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.awaiting_review_fixes.insert(
        "amf-my-feat".to_string(),
        AwaitingReviewFix {
            started_thinking: false,
        },
    );

    let sidebar_data = |status: &str| crate::app::opencode_storage::OpencodeSidebarData {
        session_id: "ses-1".to_string(),
        title: None,
        latest_prompt: None,
        status: Some(status.to_string()),
        last_tool: None,
        todo_count: None,
        todo_preview: vec![],
        pending_permission: None,
        last_error: None,
        lsp_summary: None,
        live_summary: None,
        model: None,
        provider: None,
        reasoning_tokens: None,
        additions: None,
        deletions: None,
        files: None,
    };

    // Still working: `started_thinking` flips true, no notification yet.
    app.opencode_sidebar_cache
        .insert("amf-my-feat".to_string(), sidebar_data("busy"));
    app.sync_thinking_status();
    assert!(
        app.awaiting_review_fixes
            .get("amf-my-feat")
            .is_some_and(|w| w.started_thinking),
        "watched session should be marked as having started thinking"
    );
    assert!(
        app.pending_inputs.is_empty(),
        "no notification should fire while still thinking"
    );

    // Goes idle: the watched session raises a distinct "review-ready"
    // notification instead of (not in addition to) the generic one, and the
    // watch entry is cleared.
    app.opencode_sidebar_cache
        .insert("amf-my-feat".to_string(), sidebar_data("idle"));
    app.sync_thinking_status();

    assert!(!app.awaiting_review_fixes.contains_key("amf-my-feat"));
    assert_eq!(app.pending_inputs.len(), 1);
    let notif = &app.pending_inputs[0];
    assert_eq!(notif.notification_type, "review-ready");
    assert_eq!(notif.message, "Fixes ready — re-review?");
    assert_eq!(notif.feature_name.as_deref(), Some("my-feat"));
}

#[test]
fn visible_animation_is_enabled_for_dashboard_thinking_features() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    assert!(!app.has_visible_animation());
    app.thinking_features.insert("amf-my-feat".to_string());
    assert!(app.has_visible_animation());
}

#[test]
fn visible_animation_is_enabled_for_dashboard_summary_generation() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.summary_state
        .generating
        .insert("amf-my-feat".to_string());
    assert!(app.has_visible_animation());
}

#[test]
fn visible_animation_is_enabled_while_ai_pr_review_runs_in_the_background() {
    // AI Review is its own pane/mode now (independent of PR Triage), so the
    // background-throbber animation only applies while sitting in
    // `AppMode::AiReview` — not `PrReview`, even though the same
    // `ai_review_bg` background job can outlive either.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    assert!(!app.has_visible_animation());

    let (_tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    assert!(app.has_visible_animation());

    app.ai_review_bg = None;
    assert!(!app.has_visible_animation());
}

#[test]
fn visible_animation_is_enabled_for_pr_review_running_screens() {
    // Regression: `redraw_signature()` only hashes the mode's discriminant,
    // not its stage, so these full-screen loading/running views need an
    // unconditional `has_visible_animation` arm — otherwise the throbber only
    // advances on the rare frame something else forces a redraw, and just
    // sits frozen for the (potentially long) blocking `gh`/`claude` call in
    // between, reading as "nothing is happening".
    let mut app = pr_review_test_app();

    enter_pr_review(&mut app, 1);
    let pr = match &app.mode {
        AppMode::PrReview(state) => state.review.pr.clone(),
        _ => unreachable!(),
    };
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin: sample_ai_review_state(std::path::PathBuf::from("/tmp/wd"), pr),
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    assert!(app.has_visible_animation());

    app.mode = AppMode::PrReviewLoading(crate::app::PrReviewLoadState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        pr: crate::github::PrRef {
            number: 1,
            head_sha: "sha".to_string(),
            url: "https://github.com/o/r/pull/1".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            head_ref: "main".to_string(),
        },
        usage_baselines: std::collections::HashMap::new(),
    });
    assert!(app.has_visible_animation());

    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });
    assert!(app.has_visible_animation());
}

fn store_with_single_agent_session(
    workdir: &std::path::Path,
    agent: AgentKind,
    kind: SessionKind,
    session_id: &str,
    window: &str,
) -> ProjectStore {
    let now = Utc::now();
    let session = FeatureSession {
        id: session_id.to_string(),
        kind,
        label: window.to_string(),
        tmux_window: window.to_string(),
        claude_session_id: None,
        todo_reference: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: agent.clone(),
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
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
        name: "my-project".to_string(),
        repo: workdir.to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: agent,
        is_git: false,
    };
    ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    }
}

#[test]
fn sync_session_status_reads_first_line() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-123";

    let store = store_with_custom_session(workdir.path(), session_id);
    let feature_id = store.projects[0].features[0].id.clone();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();
    db.upsert_session_status(session_id, &feature_id, "API :3000 | DB :5432", None)
        .unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(db);
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("API :3000 | DB :5432".to_string()),
    );
}

#[test]
fn sync_session_status_migrates_legacy_file_to_db() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-migrate";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(
        status_dir.join(format!("{}.txt", session_id)),
        "API :3000 | DB :5432\nextra line\n",
    )
    .unwrap();

    let store = store_with_custom_session(workdir.path(), session_id);
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(db);
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("API :3000 | DB :5432".to_string()),
    );
    assert_eq!(
        app.db
            .as_ref()
            .unwrap()
            .load_session_status(session_id)
            .unwrap(),
        Some("API :3000 | DB :5432".to_string())
    );
}

#[test]
fn sync_session_status_shows_agent_token_usage() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let encoded = workdir
        .path()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let claude_dir = home.path().join(".claude").join("projects").join(encoded);
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("claude-123.jsonl"),
        "{\"requestId\":\"req-1\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":12,\"output_tokens\":4}}}\n",
    )
    .unwrap();

    let now = Utc::now();
    let session = FeatureSession {
        id: "claude-sess".to_string(),
        kind: SessionKind::Claude,
        label: "Claude 1".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: None,
        todo_reference: None,
        token_usage_source: Some(TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "claude-123".to_string(),
        }),
        token_usage_source_match: Some(TokenUsageSourceMatch::Exact),
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: true,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Claude,
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
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
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::Claude,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    let mut tracker = SessionTokenTracker::new(
        Some(home.path().to_path_buf()),
        Some(data.path().to_path_buf()),
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.context_collector = crate::context_collectors::SessionContextCollector::with_roots(
        home.path(),
        data.path(),
        data.path(),
    );
    app.sync_session_status_with_tracker(&mut tracker);

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("12 in · 4 out · 32 eff · <$0.01".to_string()),
    );
    let usage = app.store.projects[0].features[0].sessions[0]
        .token_usage
        .as_ref()
        .unwrap();
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.total_tokens, 16);
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact),
    );
    let context = app
        .context_states
        .get("claude-sess")
        .and_then(|state| state.snapshot.as_ref())
        .expect("the five-second session sync should collect context usage");
    assert_eq!(context.used_tokens, 12);
    assert_eq!(
        context.provenance,
        crate::context_tracking::ContextProvenance::Estimated
    );
}

#[test]
fn sync_session_status_collects_isolated_pi_context_without_a_token_usage_provider() {
    let home = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let mut store = store_with_repo(workdir.path().to_path_buf(), ProjectStatus::Idle);
    let first_created = Utc.with_ymd_and_hms(2026, 8, 23, 11, 0, 0).unwrap();
    let second_created = Utc.with_ymd_and_hms(2026, 8, 23, 12, 0, 0).unwrap();
    let first_session =
        store.projects[0].features[0].add_session_named(SessionKind::Pi, "Pi".to_string());
    first_session.created_at = first_created;
    let first_amf_session_id = first_session.id.clone();
    let second_session =
        store.projects[0].features[0].add_session_named(SessionKind::Pi, "Pi 2".to_string());
    second_session.created_at = second_created;
    let second_amf_session_id = second_session.id.clone();
    let session_dir = home.path().join(".pi/agent/sessions/repo");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("pi-1.jsonl"),
        format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"pi-1\",\"timestamp\":\"2026-08-23T11:00:00Z\",\"cwd\":{}}}\n\
             {{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"provider\":\"test\",\"model\":\"model\",\"stopReason\":\"stop\",\"usage\":{{\"input\":20000,\"output\":1000,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":21000}}}}}}\n",
            serde_json::to_string(workdir.path().to_string_lossy().as_ref()).unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        session_dir.join("pi-2.jsonl"),
        format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"pi-2\",\"timestamp\":\"2026-08-23T12:00:00Z\",\"cwd\":{}}}\n\
             {{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"provider\":\"test\",\"model\":\"model\",\"stopReason\":\"stop\",\"usage\":{{\"input\":80000,\"output\":1000,\"cacheRead\":0,\"cacheWrite\":0,\"totalTokens\":81000}}}}}}\n",
            serde_json::to_string(workdir.path().to_string_lossy().as_ref()).unwrap()
        ),
    )
    .unwrap();
    std::fs::create_dir_all(home.path().join(".pi/agent")).unwrap();
    std::fs::write(
        home.path().join(".pi/agent/models.json"),
        r#"{"providers":{"test":{"models":[{"id":"model","contextWindow":100000}]}}}"#,
    )
    .unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.context_collector = crate::context_collectors::SessionContextCollector::with_roots(
        home.path(),
        home.path(),
        home.path(),
    );
    let mut tracker = SessionTokenTracker::new(Some(home.path().to_path_buf()), None);

    app.sync_session_status_with_tracker(&mut tracker);

    let first_snapshot = app
        .context_states
        .get(&first_amf_session_id)
        .and_then(|state| state.snapshot.as_ref())
        .expect("Pi should participate in the shared five-second sync");
    let second_snapshot = app
        .context_states
        .get(&second_amf_session_id)
        .and_then(|state| state.snapshot.as_ref())
        .expect("the second Pi session should have independent context state");
    assert_eq!(first_snapshot.used_tokens, 21_000);
    assert_eq!(
        first_snapshot.reset.conversation_id.as_deref(),
        Some("pi-1")
    );
    assert_eq!(second_snapshot.used_tokens, 81_000);
    assert_eq!(
        second_snapshot.reset.conversation_id.as_deref(),
        Some("pi-2")
    );
    assert_eq!(
        second_snapshot.band,
        crate::context_tracking::ContextBand::Warning
    );
}

#[test]
fn session_status_sync_refreshes_all_context_harnesses_and_excludes_terminal() {
    let roots = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let write = |path: PathBuf, content: String| {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    };
    let mut store = store_with_repo(workdir.path().to_path_buf(), ProjectStatus::Idle);
    let feature = &mut store.projects[0].features[0];
    let claude = feature.add_session_named(SessionKind::Claude, "Claude".to_string());
    claude.claude_session_id = Some("claude-all".to_string());
    let claude_amf_id = claude.id.clone();
    let codex = feature.add_session_named(SessionKind::Codex, "Codex".to_string());
    codex.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Codex,
        id: "codex-all".to_string(),
    });
    let codex_amf_id = codex.id.clone();
    let opencode = feature.add_session_named(SessionKind::Opencode, "OpenCode".to_string());
    opencode.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Opencode,
        id: "open-all".to_string(),
    });
    let opencode_amf_id = opencode.id.clone();
    let pi = feature.add_session_named(SessionKind::Pi, "Pi".to_string());
    let pi_amf_id = pi.id.clone();
    let terminal = feature.add_session_named(SessionKind::Terminal, "Terminal".to_string());
    let terminal_amf_id = terminal.id.clone();

    let encoded = workdir
        .path()
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();
    write(
        roots
            .path()
            .join(".claude/projects")
            .join(encoded)
            .join("claude-all.jsonl"),
        "{\"type\":\"assistant\",\"timestamp\":\"2026-08-23T12:00:00Z\",\"sessionId\":\"claude-all\",\"requestId\":\"r1\",\"message\":{\"id\":\"m1\",\"usage\":{\"input_tokens\":630000,\"output_tokens\":1}}}\n".to_string(),
    );
    write(
        roots.path().join(".codex/sessions/codex-all.jsonl"),
        format!(
            "{{\"timestamp\":\"2026-08-23T12:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-all\",\"cwd\":{}}}}}\n\
             {{\"timestamp\":\"2026-08-23T12:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1,\"total_tokens\":1}},\"last_token_usage\":{{\"total_tokens\":150000}},\"model_context_window\":200000}}}}}}\n",
            serde_json::to_string(workdir.path().to_string_lossy().as_ref()).unwrap()
        ),
    );
    write(
        roots
            .path()
            .join("opencode/storage/session/project/open-all.json"),
        serde_json::json!({
            "id": "open-all",
            "directory": workdir.path(),
            "time": {"updated": 2_000}
        })
        .to_string(),
    );
    write(
        roots
            .path()
            .join("opencode/storage/message/open-all/m1.json"),
        serde_json::json!({
            "id": "m1",
            "role": "assistant",
            "providerID": "test",
            "modelID": "model",
            "time": {"completed": 2_000},
            "tokens": {"input": 80_000, "output": 0, "cache": {"read": 0, "write": 0}}
        })
        .to_string(),
    );
    write(
        roots.path().join("opencode/models.json"),
        r#"{"test":{"models":{"model":{"limit":{"context":100000}}}}}"#.to_string(),
    );
    write(
        roots.path().join(".pi/agent/sessions/repo/pi-all.jsonl"),
        format!(
            "{{\"type\":\"session\",\"version\":3,\"id\":\"pi-all\",\"cwd\":{}}}\n\
             {{\"type\":\"message\",\"message\":{{\"role\":\"assistant\",\"provider\":\"test\",\"model\":\"model\",\"stopReason\":\"stop\",\"usage\":{{\"totalTokens\":90000}}}}}}\n",
            serde_json::to_string(workdir.path().to_string_lossy().as_ref()).unwrap()
        ),
    );
    write(
        roots.path().join(".pi/agent/models.json"),
        r#"{"providers":{"test":{"models":[{"id":"model","contextWindow":100000}]}}}"#.to_string(),
    );

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.context_collector = crate::context_collectors::SessionContextCollector::with_roots(
        roots.path(),
        roots.path(),
        roots.path(),
    );
    let mut tracker = SessionTokenTracker::new(
        Some(roots.path().to_path_buf()),
        Some(roots.path().to_path_buf()),
    );

    app.sync_session_status_with_tracker(&mut tracker);

    for (session_id, percentage) in [
        (claude_amf_id, 70),
        (codex_amf_id, 75),
        (opencode_amf_id, 80),
        (pi_amf_id, 90),
    ] {
        assert_eq!(
            app.context_states[&session_id]
                .snapshot
                .as_ref()
                .unwrap()
                .percentage
                .get(),
            percentage
        );
        let context_state = app.context_states.get(&session_id).unwrap();
        assert!(
            app.context_hint_states
                .is_eligible(&session_id, Some(context_state))
        );
    }
    assert!(!app.context_states.contains_key(&terminal_amf_id));
}

#[test]
fn sync_session_status_marks_discovered_codex_usage_as_inferred() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let session_dir = home
        .path()
        .join(".codex")
        .join("sessions")
        .join("2026")
        .join("03")
        .join("13");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("rollout.jsonl"),
        format!(
            concat!(
                "{{\"timestamp\":\"2026-03-13T14:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-1\",\"cwd\":\"{}\"}}}}\n",
                "{{\"timestamp\":\"2026-03-13T14:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":7,\"reasoning_output_tokens\":3,\"total_tokens\":110}}}}}}}}\n"
            ),
            workdir.path().display()
        ),
    )
    .unwrap();

    let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 13, 59, 30).unwrap();
    let session = FeatureSession {
        id: "codex-sess".to_string(),
        kind: SessionKind::Codex,
        label: "Codex".to_string(),
        tmux_window: "codex".to_string(),
        claude_session_id: None,
        todo_reference: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Codex,
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at,
        last_accessed: created_at,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at,
        preferred_agent: AgentKind::Codex,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    let mut tracker = SessionTokenTracker::new(
        Some(home.path().to_path_buf()),
        Some(data.path().to_path_buf()),
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status_with_tracker(&mut tracker);

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "codex-1".to_string(),
        }),
    );
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].token_usage_source_match,
        Some(TokenUsageSourceMatch::Inferred),
    );
}

#[test]
fn sync_session_status_does_not_infer_stale_codex_usage_for_new_session() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let codex_dir = home.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let rollout = codex_dir.join("old-rollout.jsonl");
    std::fs::write(
        &rollout,
        format!(
            concat!(
                "{{\"timestamp\":\"2026-03-13T14:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"old-codex\",\"cwd\":\"{}\"}}}}\n",
                "{{\"timestamp\":\"2026-03-13T14:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1000000,\"output_tokens\":1000000,\"total_tokens\":2000000}}}}}}}}\n"
            ),
            workdir.path().display()
        ),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(codex_dir.join("state_5.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY,
            rollout_path TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            cwd TEXT NOT NULL,
            archived INTEGER NOT NULL DEFAULT 0
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads (id, rollout_path, updated_at, cwd, archived)
         VALUES (?1, ?2, ?3, ?4, 0)",
        rusqlite::params![
            "old-codex",
            rollout.to_string_lossy(),
            Utc.with_ymd_and_hms(2026, 3, 13, 14, 0, 0)
                .unwrap()
                .timestamp(),
            workdir.path().to_string_lossy(),
        ],
    )
    .unwrap();

    let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 14, 5, 0).unwrap();
    let session = FeatureSession {
        id: "codex-sess".to_string(),
        kind: SessionKind::Codex,
        label: "Codex".to_string(),
        tmux_window: "codex".to_string(),
        claude_session_id: None,
        todo_reference: None,
        token_usage_source: Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "old-codex".to_string(),
        }),
        token_usage_source_match: Some(TokenUsageSourceMatch::Inferred),
        created_at,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Codex,
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at,
        last_accessed: created_at,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at,
        preferred_agent: AgentKind::Codex,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    let mut tracker = SessionTokenTracker::new(
        Some(home.path().to_path_buf()),
        Some(data.path().to_path_buf()),
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.sync_session_status_with_tracker(&mut tracker);

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(session.token_usage_source, None);
    assert_eq!(session.token_usage_source_match, None);
    assert_eq!(session.status_text, None);
    assert_eq!(session.token_usage, None);
}

#[test]
fn sync_session_status_does_not_duplicate_inferred_sources_in_feature() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let session_dir = home
        .path()
        .join(".codex")
        .join("sessions")
        .join("2026")
        .join("03")
        .join("13");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("rollout.jsonl"),
        format!(
            concat!(
                "{{\"timestamp\":\"2026-03-13T14:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-1\",\"cwd\":\"{}\"}}}}\n",
                "{{\"timestamp\":\"2026-03-13T14:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":7,\"reasoning_output_tokens\":3,\"total_tokens\":110}}}}}}}}\n"
            ),
            workdir.path().display()
        ),
    )
    .unwrap();

    let created_at = Utc::now();
    let first = FeatureSession {
        id: "codex-sess-1".to_string(),
        kind: SessionKind::Codex,
        label: "Codex 1".to_string(),
        tmux_window: "codex".to_string(),
        claude_session_id: None,
        todo_reference: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let second = FeatureSession {
        id: "codex-sess-2".to_string(),
        label: "Codex 2".to_string(),
        tmux_window: "codex-2".to_string(),
        created_at: created_at + chrono::Duration::milliseconds(100),
        ..first.clone()
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![first, second],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Codex,
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at,
        last_accessed: created_at,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at,
        preferred_agent: AgentKind::Codex,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    let mut tracker = SessionTokenTracker::new(
        Some(home.path().to_path_buf()),
        Some(data.path().to_path_buf()),
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status_with_tracker(&mut tracker);

    let sessions = &app.store.projects[0].features[0].sessions;
    assert_eq!(
        sessions[0].token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "codex-1".to_string(),
        }),
    );
    assert_eq!(
        sessions[0].token_usage_source_match,
        Some(TokenUsageSourceMatch::Inferred),
    );
    assert_eq!(sessions[1].token_usage_source, None);
    assert_eq!(sessions[1].token_usage_source_match, None);
}

#[test]
fn sync_session_status_checks_sidebar_inputs_off_thread() {
    let workdir = TempDir::new().unwrap();
    let prompt_path = workdir.path().join(".claude").join("latest-prompt.txt");
    std::fs::create_dir_all(prompt_path.parent().unwrap()).unwrap();
    std::fs::write(&prompt_path, "first prompt").unwrap();

    let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 13, 59, 30).unwrap();
    let session = FeatureSession {
        id: "claude-sess".to_string(),
        kind: SessionKind::Claude,
        label: "Claude".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: Some("claude-session-1".to_string()),
        todo_reference: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Claude,
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at,
        last_accessed: created_at,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at,
        preferred_agent: AgentKind::Claude,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let signature =
        crate::app::SidebarLoadRequest::from_feature(&app.store.projects[0].features[0])
            .signature();
    app.sidebar_load_signatures
        .insert("amf-my-feat".to_string(), signature);

    let mut tracker = SessionTokenTracker::default();
    app.sync_session_status_with_tracker(&mut tracker);

    assert!(
        app.pending_sidebar_loads.contains("amf-my-feat"),
        "sidebar input checks should be queued off the UI thread"
    );
    for _ in 0..20 {
        app.poll_sidebar_load_results();
        if !app.pending_sidebar_loads.contains("amf-my-feat") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !app.pending_sidebar_loads.contains("amf-my-feat"),
        "unchanged sidebar input check should complete"
    );
    assert!(
        app.latest_prompt_for_session("amf-my-feat") == Some("first prompt"),
        "unchanged sidebar input check should leave the existing cache alone"
    );

    std::thread::sleep(std::time::Duration::from_millis(2));
    std::fs::write(&prompt_path, "updated prompt").unwrap();

    app.sync_session_status_with_tracker(&mut tracker);

    assert!(
        app.pending_sidebar_loads.contains("amf-my-feat"),
        "changed prompt inputs should queue a fresh sidebar load"
    );
    for _ in 0..20 {
        app.poll_sidebar_load_results();
        if !app.pending_sidebar_loads.contains("amf-my-feat") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        app.latest_prompt_for_session("amf-my-feat"),
        Some("updated prompt")
    );
}

#[test]
fn note_codex_prompt_submit_marks_repo_root_feature_thinking() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pending_inputs.push(PendingInput {
        session_id: "amf-my-feat".to_string(),
        cwd: workdir.path().display().to_string(),
        message: "Codex finished and is waiting for input".to_string(),
        notification_type: "input-request".to_string(),
        file_path: PathBuf::new(),
        target_file_path: None,
        relative_path: None,
        change_id: None,
        tool: None,
        old_snippet: None,
        new_snippet: None,
        original_file: None,
        proposed_file: None,
        is_new_file: None,
        reason: None,
        response_file: None,
        project_name: Some("my-project".to_string()),
        feature_name: Some("my-feat".to_string()),
        proceed_signal: None,
        request_id: None,
        reply_socket: None,
    });

    app.note_codex_prompt_submit("amf-my-feat", "codex");

    assert!(
        app.ipc_thinking_sessions.contains("amf-my-feat"),
        "repo-root codex feature should be marked thinking"
    );
    assert!(
        app.pending_inputs.is_empty(),
        "prompt submit should clear stale input-request notifications"
    );
}

#[test]
fn note_codex_prompt_submit_marks_codex_session_in_non_codex_feature_thinking() {
    let workdir = TempDir::new().unwrap();
    let mut store = store_with_codex_session(workdir.path(), false);
    store.projects[0].features[0].agent = AgentKind::Claude;
    store.projects[0].features[0].sessions[0].label = "PR triage".to_string();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.note_codex_prompt_submit("amf-my-feat", "codex");

    assert!(app.ipc_thinking_sessions.contains("amf-my-feat"));
    assert!(app.ipc_thinking_feature_sessions.contains("codex-sess"));
}

#[test]
fn apply_codex_live_event_updates_feature_live_state() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    let changed = app.apply_codex_live_event(
        "amf-my-feat",
        &serde_json::json!({
            "type": "plan",
            "payload": { "text": "1. Inspect\n2. Patch" }
        }),
    );

    assert!(changed);
    let live = app
        .codex_live_thread("amf-my-feat")
        .expect("expected live codex state");
    assert_eq!(live.plan_text.as_deref(), Some("1. Inspect\n2. Patch"));
}

#[test]
fn poll_codex_sidebar_metadata_updates_caches() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let cache_key = format!("{}::sess-current", workdir.path().display());
    app.codex_sidebar_metadata_inflight
        .insert(cache_key.clone());
    app.codex_sidebar_metadata_tx
        .send(CodexSidebarMetadataResult {
            cache_key: cache_key.clone(),
            title: Some("Sidebar title".into()),
            prompt: Some("Sidebar prompt".into()),
            model_text: Some("Model: gpt-5.5".into()),
        })
        .unwrap();

    app.poll_codex_sidebar_metadata();

    assert_eq!(
        app.cached_codex_session_title(workdir.path(), "sess-current"),
        Some("Sidebar title")
    );
    assert_eq!(
        app.cached_codex_session_prompt(workdir.path(), "sess-current"),
        Some("Sidebar prompt")
    );
    assert_eq!(
        app.cached_codex_session_model(workdir.path(), "sess-current"),
        Some("Model: gpt-5.5")
    );
    assert!(!app.codex_sidebar_metadata_inflight.contains(&cache_key));
}

#[test]
fn sync_session_status_skips_sidebar_refresh_when_sidebar_hidden() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut view = ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "codex".to_string(),
        "Codex".to_string(),
        SessionKind::Codex,
        VibeMode::default(),
        false,
    );
    view.sidebar_visible = false;
    app.mode = AppMode::Viewing(view);

    app.sync_session_status();

    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn toggling_sidebar_back_on_triggers_sidebar_refresh() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut view = ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "codex".to_string(),
        "Codex".to_string(),
        SessionKind::Codex,
        VibeMode::default(),
        false,
    );
    view.sidebar_visible = false;
    app.mode = AppMode::Viewing(view);

    app.toggle_sidebar_in_view();

    assert!(app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn ipc_input_request_updates_codex_live_work_state() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "input-request",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Need approval before applying the patch.",
        "tool_name": "Bash",
        "relative_path": "src/main.rs"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for input\nRequest: Need approval before applying the patch.\nTool: Bash\nFile: src/main.rs"
        )
    );
    assert_eq!(app.toasts.len(), 1);
    assert!(
        app.toasts[0]
            .message
            .contains("New input request from my-feat")
    );
}

/// Stamp `path` with an explicit modification time. Filesystem timestamp
/// granularity can be coarse enough that two consecutive writes land on the
/// same instant, which would make a recency assertion meaningless.
fn set_mtime(path: &std::path::Path, modified: std::time::SystemTime) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
}

/// A file-backed session wait, as the notification scan builds them.
fn file_session_wait(file_path: &std::path::Path, message: &str) -> PendingInput {
    PendingInput {
        session_id: "amf-my-feat".to_string(),
        cwd: String::new(),
        message: message.to_string(),
        notification_type: "input-request".to_string(),
        file_path: file_path.to_path_buf(),
        target_file_path: None,
        relative_path: None,
        change_id: None,
        tool: None,
        old_snippet: None,
        new_snippet: None,
        original_file: None,
        proposed_file: None,
        is_new_file: None,
        reason: None,
        response_file: None,
        project_name: Some("my-project".to_string()),
        feature_name: Some("my-feat".to_string()),
        proceed_signal: None,
        request_id: None,
        reply_socket: None,
    }
}

#[test]
fn collapsing_session_waits_keeps_the_newest_report_not_the_first_scanned() {
    let dir = TempDir::new().unwrap();
    let older = dir.path().join("feature-local.json");
    let newer = dir.path().join("global.json");
    std::fs::write(&older, "{}").unwrap();
    // Distinct mtimes: filesystem timestamp granularity can be coarse, so
    // stamp the older file explicitly rather than relying on write order.
    let now = std::time::SystemTime::now();
    set_mtime(&older, now - std::time::Duration::from_secs(60));
    std::fs::write(&newer, "{}").unwrap();
    set_mtime(&newer, now);

    // `read_dir` promises no ordering and the feature-local directory is
    // always scanned before the global one, so the older copy routinely
    // arrives first. Keeping whichever came first would strand the stale
    // message and delete the live file.
    let mut inputs = vec![
        file_session_wait(&older, "Stale first report"),
        file_session_wait(&newer, "Current report"),
    ];
    App::collapse_session_waits(&mut inputs);

    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].message, "Current report");
    assert_eq!(inputs[0].file_path, newer);
    assert!(
        !older.exists(),
        "the superseded report's file must be deleted"
    );
    assert!(newer.exists(), "the surviving report keeps its file");
}

#[test]
fn collapsing_session_waits_prefers_a_file_report_over_a_stale_ipc_one() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("stop.json");
    std::fs::write(&file, "{}").unwrap();

    // An IPC entry only survives the preservation loop when no file matched
    // it, and the file fallback is written when the live wait times out — by
    // then the socket the IPC copy names is gone.
    let mut ipc = file_session_wait(std::path::Path::new(""), "Reported over IPC");
    ipc.request_id = Some("req-1".to_string());
    ipc.reply_socket = Some("/tmp/amf-ipc-reply/req-1.sock".to_string());
    let mut inputs = vec![ipc, file_session_wait(&file, "Rewritten as a file")];
    App::collapse_session_waits(&mut inputs);

    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0].message, "Rewritten as a file");
    assert!(
        file.exists(),
        "the winning report's file must not be removed"
    );
}

#[test]
fn a_rewritten_session_wait_file_does_not_toast_again() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let write_wait = |message: &str| {
        std::fs::write(
            notify_dir.join("input-request.json"),
            serde_json::to_string(&serde_json::json!({
                "session_id": "amf-my-feat",
                "cwd": workdir.path().display().to_string(),
                "message": message,
                "type": "input-request",
            }))
            .unwrap(),
        )
        .unwrap();
    };

    write_wait("Need input before continuing.");
    app.scan_notifications_forced();
    assert_eq!(app.toasts.len(), 1);

    // The harness re-reports the same standing wait with a fresher message.
    // The row is replaced silently; a toast per repeat is what buried the
    // dashboard, and full-struct equality no longer recognises the repeat.
    write_wait("Still waiting, now on the second question.");
    app.scan_notifications_forced();

    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(
        app.pending_inputs[0].message,
        "Still waiting, now on the second question."
    );
    assert_eq!(app.toasts.len(), 1, "a re-report must not toast again");
}

#[test]
fn repeated_stop_reports_leave_one_pending_input_per_session() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_repo(workdir.path().to_path_buf(), ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    // Claude's Stop hook fires at every turn boundary, so one waiting session
    // reports the same stop over and over. It is a standing fact about the
    // session, not a queue of requests: the overlay must list it once, showing
    // what the session last said.
    for message in ["First stop", "Second stop", "Third stop"] {
        app.handle_ipc_message_value(serde_json::json!({
            "session_id": "amf-my-feat",
            "cwd": workdir.path().display().to_string(),
            "message": message
        }));
    }

    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].message, "Third stop");
    assert_eq!(
        app.pending_inputs[0].feature_name.as_deref(),
        Some("my-feat")
    );
}

#[test]
fn a_re_reported_input_request_is_not_toasted_again() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_repo(workdir.path().to_path_buf(), ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    let mut request = |message: &str| {
        app.handle_ipc_message_value(serde_json::json!({
            "type": "input-request",
            "session_id": "amf-my-feat",
            "cwd": workdir.path().display().to_string(),
            "message": message
        }));
    };
    request("Waiting on you");
    request("Still waiting on you");

    // The first report is news; the second is the same session saying so
    // again, and a toast per repeat is what buried the dashboard.
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.toasts.len(), 1);
}

#[test]
fn deduping_stops_is_per_session_not_global() {
    let workdir = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let mut store = store_with_repo(workdir.path().to_path_buf(), ProjectStatus::Active);
    let mut second = store.projects[0].features[0].clone();
    second.id = "feat-2".to_string();
    second.name = "other-feat".to_string();
    second.workdir = other.path().to_path_buf();
    second.tmux_session = "amf-other-feat".to_string();
    store.projects[0].features.push(second);

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    for _ in 0..3 {
        app.handle_ipc_message_value(serde_json::json!({
            "session_id": "amf-my-feat",
            "cwd": workdir.path().display().to_string(),
            "message": "waiting"
        }));
        app.handle_ipc_message_value(serde_json::json!({
            "session_id": "amf-other-feat",
            "cwd": other.path().display().to_string(),
            "message": "waiting"
        }));
    }

    // Collapsing by session must not collapse two sessions into one: both
    // features are still waiting and both have to be reachable from `i`.
    let waiting: Vec<&str> = app
        .pending_inputs
        .iter()
        .filter_map(|input| input.feature_name.as_deref())
        .collect();
    assert_eq!(waiting, vec!["my-feat", "other-feat"]);
}

#[test]
fn queued_diff_reviews_are_not_collapsed_into_one() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    for path in ["src/main.rs", "src/app/mod.rs"] {
        app.handle_ipc_message_value(serde_json::json!({
            "type": "change-reason",
            "session_id": "amf-my-feat",
            "cwd": workdir.path().display().to_string(),
            "message": "Review this",
            "relative_path": path,
            "change_id": path
        }));
    }

    // Each review is its own request about its own edit — unlike a stop, two
    // of them are two pieces of work and both must survive.
    assert_eq!(app.pending_inputs.len(), 2);
    assert!(
        app.pending_inputs
            .iter()
            .all(|input| input.notification_type == "change-reason")
    );
}

#[test]
fn ipc_turn_end_archives_superseded_review_notes_for_review_features() {
    let workdir = TempDir::new().unwrap();
    let claude = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("review-notes.md"),
        "## src/main.rs — first\n\nOld context.\n\n---\n\n\
         ## src/main.rs — current\n\nCurrent context.\n\n---\n",
    )
    .unwrap();

    let mut store = store_with_codex_session(workdir.path(), false);
    store.projects[0].features[0].review = true;
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "input-request",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Codex finished and is waiting for input"
    }));

    let live = std::fs::read_to_string(claude.join("review-notes.md")).unwrap();
    assert!(live.contains("Current context."));
    assert!(!live.contains("Old context."));
    let archive = std::fs::read_to_string(claude.join("review-notes-archive.md")).unwrap();
    assert!(archive.contains("Old context."));
    assert!(!archive.contains("Current context."));
}

#[test]
fn ipc_event_binds_exact_codex_usage_source_by_feature_session_id() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "input-request",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "provider_session_id": "codex-provider-123",
        "amf_feature_session_id": "codex-sess",
        "cwd": workdir.path().display().to_string(),
        "message": "Codex finished and is waiting for input"
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "codex-provider-123".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
    );
}

#[test]
fn ipc_event_binds_exact_claude_usage_source_by_feature_session_id() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_single_agent_session(
        workdir.path(),
        AgentKind::Claude,
        SessionKind::Claude,
        "claude-sess",
        "claude",
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": "amf-my-feat",
        "provider_session_id": "claude-provider-123",
        "amf_feature_session_id": "claude-sess",
        "cwd": workdir.path().display().to_string()
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "claude-provider-123".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
    );
}

#[test]
fn ipc_event_binds_exact_opencode_usage_source_by_feature_session_id() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_single_agent_session(
        workdir.path(),
        AgentKind::Opencode,
        SessionKind::Opencode,
        "opencode-sess",
        "opencode",
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "input-request",
        "session_id": "opencode-provider-123",
        "provider_session_id": "opencode-provider-123",
        "amf_feature_session_id": "opencode-sess",
        "cwd": workdir.path().display().to_string(),
        "message": "Agent finished and is waiting for input"
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Opencode,
            id: "opencode-provider-123".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
    );
}

#[test]
fn ipc_exact_usage_source_replaces_inferred_source() {
    let workdir = TempDir::new().unwrap();
    let mut store = store_with_codex_session(workdir.path(), false);
    let session = &mut store.projects[0].features[0].sessions[0];
    session.set_token_usage_source_inferred(TokenUsageSource {
        provider: TokenUsageProvider::Codex,
        id: "old-inferred-codex".to_string(),
    });
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-stop",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "provider_session_id": "exact-codex-session",
        "amf_feature_session_id": "codex-sess",
        "cwd": workdir.path().display().to_string()
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "exact-codex-session".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
    );
}

#[test]
fn ipc_prompt_submit_clears_codex_live_input_and_marks_thinking() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.apply_codex_live_event(
        "amf-my-feat",
        &serde_json::json!({
            "type": "requestUserInput",
            "payload": { "prompt": "Need approval before applying the patch." }
        }),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "prompt-submit",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "prompt": "Continue with the patch."
    }));

    assert!(app.ipc_thinking_sessions.contains("amf-my-feat"));
    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text()),
        None
    );
}

#[test]
fn ipc_prompt_submit_clears_codex_live_review_work() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.apply_codex_live_event(
        "amf-my-feat",
        &serde_json::json!({
            "type": "fileChange",
            "payload": {
                "relative_path": "src/main.rs",
                "status": "proposed"
            }
        }),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "prompt-submit",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "prompt": "Reviewed the diff and continue."
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text()),
        None
    );
}

#[test]
fn ipc_prompt_submit_refreshes_sidebar_plan_cache() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("plan.md"),
        "# Plan\n\n1. Refresh plan cache\n2. Render update\n",
    )
    .unwrap();

    app.handle_ipc_message_value(serde_json::json!({
        "type": "prompt-submit",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "prompt": "Continue with the patch."
    }));

    assert_eq!(
        app.sidebar_plan_for_session("amf-my-feat"),
        Some("Plan\n1. Refresh plan cache\n2. Render update")
    );
}

#[test]
fn ipc_diff_review_updates_codex_live_review_state() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "file_path": "src/main.rs",
        "message": "Review the change before continuing.",
        "tool_name": "Edit"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for diff review\nFile: src/main.rs\nTool: Edit\nRequest: Review the change before continuing.\nHint: use leader V if the review prompt is not appearing."
        )
    );
}

#[test]
fn ipc_diff_review_opens_from_amf_session_when_cwd_does_not_match_feature() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "custom-session");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "custom".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    ));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "session_id": "claude-hook-session",
        "amf_session": "amf-my-feat",
        "cwd": "/tmp/not-the-feature-workdir",
        "message": "Review: src/main.rs",
        "file_path": workdir.path().join("src/main.rs").display().to_string(),
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-ipc",
        "old_snippet": "old",
        "new_snippet": "new",
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string(),
        "request_id": "req-1",
        "reply_socket": "/tmp/amf-ipc-reply/req-1.sock"
    }));

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/main.rs");
            assert_eq!(state.request_id.as_deref(), Some("req-1"));
            assert_eq!(
                state
                    .return_to_view
                    .as_ref()
                    .map(|view| view.feature_name.as_str()),
                Some("my-feat")
            );
        }
        _ => panic!("expected diff review prompt"),
    }
}

#[test]
fn ipc_diff_review_queues_with_toast_from_normal_mode() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "custom-session");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Normal;

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "session_id": "claude-hook-session",
        "amf_session": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/main.rs",
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-ipc",
        "request_id": "req-1"
    }));

    // From the dashboard the review must not steal focus; it is queued
    // as a pending input and announced with a toast.
    assert!(
        matches!(app.mode, AppMode::Normal),
        "expected to stay on the dashboard"
    );
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("New diff review from my-feat"))
            .unwrap_or(false),
        "expected a diff review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn ipc_diff_review_queues_when_viewing_a_different_feature() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "custom-session");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Viewing(ViewState::new(
        "other-project".to_string(),
        "other-feat".to_string(),
        "amf-other-feat".to_string(),
        "custom".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    ));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "session_id": "claude-hook-session",
        "amf_session": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/main.rs",
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-ipc",
        "request_id": "req-1"
    }));

    // Viewing a different feature: stay in that view, queue the review,
    // and announce it with a toast.
    match &app.mode {
        AppMode::Viewing(view) => assert_eq!(view.feature_name, "other-feat"),
        _ => panic!("expected to stay in the other feature's view"),
    }
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("New diff review from my-feat"))
            .unwrap_or(false),
        "expected a diff review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn ipc_tool_activity_temporarily_overrides_older_review_work() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "file_path": "src/main.rs",
        "message": "Review the change before continuing."
    }));
    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-start",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "tool_name": "Bash"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some("State: running tool\nTool: Bash")
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-stop",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "tool_name": "Bash"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for diff review\nFile: src/main.rs\nRequest: Review the change before continuing.\nHint: use leader V if the review prompt is not appearing."
        )
    );
}

#[test]
fn ipc_change_reason_adds_default_request_when_message_is_missing() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "change-reason",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "file_path": "src/main.rs",
        "tool_name": "Edit"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for change reason\nFile: src/main.rs\nTool: Edit\nRequest: Explain why this change is needed."
        )
    );
}
