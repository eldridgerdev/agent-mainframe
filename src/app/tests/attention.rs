use crate::app::attention::{AttentionSource, AttentionState};
use crate::app::*;
use crate::project::{AgentKind, Feature, FeatureSession, Project, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::{Duration, Utc};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Attention layer: why a stopped session is stopped.
// ---------------------------------------------------------------------------

/// A store with `features.len()` features under one project, each with its own
/// tmux session (`amf-feat-N`) and agent, so ordering across mixed harnesses
/// can be exercised.
fn store_with_agents(agents: &[AgentKind]) -> ProjectStore {
    let now = Utc::now();
    let features: Vec<Feature> = agents
        .iter()
        .enumerate()
        .map(|(i, agent)| Feature {
            id: format!("feat-{i}"),
            name: format!("feat-{i}"),
            branch: format!("feat-{i}"),
            workdir: PathBuf::from(format!("/tmp/test-workdir-{i}")),
            is_worktree: false,
            tmux_session: format!("amf-feat-{i}"),
            sessions: vec![],
            collapsed: false,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: agent.clone(),
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
        })
        .collect();

    ProjectStore {
        version: 2,
        projects: vec![Project {
            id: "proj-1".to_string(),
            name: "my-project".to_string(),
            repo: PathBuf::from("/tmp/test-repo"),
            collapsed: false,
            features,
            created_at: now,
            preferred_agent: AgentKind::default(),
            is_git: false,
        }],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    }
}

fn app_with_agents(agents: &[AgentKind]) -> App {
    App::new_for_test(
        store_with_agents(agents),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    )
}

#[test]
fn attention_records_are_narrowed_to_harness_capability() {
    let mut app = app_with_agents(&[AgentKind::Claude, AgentKind::Codex, AgentKind::Pi]);

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-1",
        &AgentKind::Codex,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-2",
        &AgentKind::Pi,
        AttentionState::Question,
        AttentionSource::Hook,
    );

    // Claude can prove it; Codex and Pi cannot and degrade rather than guess.
    assert_eq!(
        app.attention_for("amf-feat-0").map(|r| r.state),
        Some(AttentionState::Question)
    );
    assert_eq!(
        app.attention_for("amf-feat-1").map(|r| r.state),
        Some(AttentionState::Waiting)
    );
    assert_eq!(
        app.attention_for("amf-feat-2").map(|r| r.state),
        Some(AttentionState::Waiting)
    );
}

#[test]
fn attention_question_is_not_downgraded_by_an_inferred_completion() {
    let mut app = app_with_agents(&[AgentKind::Claude]);

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::CompletedAwaitingReview,
        AttentionSource::Inferred,
    );

    // An agent blocked on an answer stays blocked; the sync fallback's
    // agent-agnostic "completed" must not overwrite the harness's better signal.
    assert_eq!(
        app.attention_for("amf-feat-0").map(|r| r.state),
        Some(AttentionState::Question)
    );
}

#[test]
fn attention_question_is_retired_by_a_hook_completion() {
    let mut app = app_with_agents(&[AgentKind::Claude]);

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::CompletedAwaitingReview,
        AttentionSource::Hook,
    );

    // A real `Stop` means the turn the permission prompt interrupted has ended,
    // so the question was answered. Holding onto it would strand the session in
    // the `i` list until it aged out.
    assert_eq!(
        app.attention_for("amf-feat-0").map(|r| r.state),
        Some(AttentionState::CompletedAwaitingReview)
    );
}

#[test]
fn attention_completion_is_upgraded_by_a_later_question() {
    let mut app = app_with_agents(&[AgentKind::Claude]);

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::CompletedAwaitingReview,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );

    assert_eq!(
        app.attention_for("amf-feat-0").map(|r| r.state),
        Some(AttentionState::Question)
    );
}

#[test]
fn attention_re_raise_keeps_the_original_timestamp() {
    let mut app = app_with_agents(&[AgentKind::Claude]);

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    let first = app.attention_for("amf-feat-0").unwrap().since;

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    let second = app.attention_for("amf-feat-0").unwrap().since;

    // Ageing must measure how long the user has ignored the question, not how
    // often the harness repeated itself.
    assert_eq!(first, second);
}

#[test]
fn attention_clears_when_the_session_produces_output_again() {
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );

    assert!(app.clear_attention("amf-feat-0"));
    assert!(app.attention_for("amf-feat-0").is_none());
    // Clearing an already-clear session reports no change, so callers can skip
    // a redraw.
    assert!(!app.clear_attention("amf-feat-0"));
}

#[test]
fn attention_ages_out_past_the_stale_threshold() {
    let mut app = app_with_agents(&[AgentKind::Claude, AgentKind::Claude]);
    app.config.waiting_stale_minutes = 30;

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-1",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );

    // Just inside the window survives; just outside it does not.
    app.attention.get_mut("amf-feat-0").unwrap().since = Utc::now() - Duration::minutes(29);
    app.attention.get_mut("amf-feat-1").unwrap().since = Utc::now() - Duration::minutes(31);

    assert!(app.age_out_attention());
    assert!(app.attention_for("amf-feat-0").is_some());
    assert!(app.attention_for("amf-feat-1").is_none());
}

#[test]
fn attention_ageing_is_disabled_by_zero() {
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.config.waiting_stale_minutes = 0;

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.attention.get_mut("amf-feat-0").unwrap().since = Utc::now() - Duration::days(7);

    assert!(!app.age_out_attention());
    assert!(app.attention_for("amf-feat-0").is_some());
}

#[test]
fn needs_attention_sorts_questions_first_then_oldest() {
    let mut app = app_with_agents(&[
        AgentKind::Claude,
        AgentKind::Codex,
        AgentKind::Claude,
        AgentKind::Opencode,
    ]);

    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::CompletedAwaitingReview,
        AttentionSource::Hook,
    );
    // Codex narrows to Waiting, which sorts last.
    app.record_attention(
        "amf-feat-1",
        &AgentKind::Codex,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-2",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-3",
        &AgentKind::Opencode,
        AttentionState::Question,
        AttentionSource::Hook,
    );

    // feat-3's question is older than feat-2's, so it leads.
    app.attention.get_mut("amf-feat-3").unwrap().since = Utc::now() - Duration::minutes(10);

    let ordered: Vec<String> = app
        .needs_attention()
        .into_iter()
        .map(|entry| entry.feature_name)
        .collect();

    assert_eq!(ordered, vec!["feat-3", "feat-2", "feat-0", "feat-1"]);
    // No pending inputs here, so every row is explained by the attention layer.
    assert_eq!(app.attention_badge_counts(), ((2, 1, 1), 0));
}

#[test]
fn needs_attention_skips_stopped_features() {
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.store.projects[0].features[0].status = ProjectStatus::Stopped;

    // A record can outlive its session when the user stops a feature without
    // answering it; the list must not offer a session that isn't running.
    assert!(app.needs_attention().is_empty());
    assert_eq!(app.attention_badge_counts(), ((0, 0, 0), 0));
}

/// A generic `input-request` filed against `feature_name`, the pending input
/// `sync.rs` raises when a session goes idle.
fn input_request_for(feature_name: &str) -> PendingInput {
    PendingInput {
        session_id: format!("amf-{feature_name}"),
        cwd: String::new(),
        message: "waiting for input".to_string(),
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
        feature_name: Some(feature_name.to_string()),
        proceed_signal: None,
        request_id: None,
        reply_socket: None,
    }
}

#[test]
fn attention_rows_fold_the_input_request_the_same_feature_raised() {
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.pending_inputs.push(input_request_for("feat-0"));

    // The pending input and the attention record describe the same stop seen
    // through the old signal and the new one, so the overlay lists it once —
    // as a question — and still dispatches through the pending input.
    let rows = app.attention_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state(), Some(AttentionState::Question));
    assert_eq!(rows[0].pending_index(), Some(0));
}

#[test]
fn attention_rows_fold_a_bare_stop_the_same_way_as_an_input_request() {
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::CompletedAwaitingReview,
        AttentionSource::Hook,
    );
    // Claude's Stop hook forwards its own payload, so the pending input it
    // leaves is typed `stop` rather than `input-request`. It describes the
    // same stop the attention record does and must not be listed twice.
    let mut stop = input_request_for("feat-0");
    stop.notification_type = "stop".to_string();
    app.pending_inputs.push(stop);

    let rows = app.attention_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].state(),
        Some(AttentionState::CompletedAwaitingReview)
    );
    assert_eq!(rows[0].pending_index(), Some(0));
}

#[test]
fn attention_rows_keep_unexplained_pending_inputs_after_the_explained_ones() {
    let mut app = app_with_agents(&[AgentKind::Claude, AgentKind::Claude]);
    app.record_attention(
        "amf-feat-1",
        &AgentKind::Claude,
        AttentionState::CompletedAwaitingReview,
        AttentionSource::Hook,
    );

    // A diff review is a separate piece of work, not a description of why a
    // session stopped, so it keeps its own row — after everything explained.
    let mut diff_review = input_request_for("feat-0");
    diff_review.notification_type = "diff-review".to_string();
    app.pending_inputs.push(diff_review);

    let rows = app.attention_rows();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].state(),
        Some(AttentionState::CompletedAwaitingReview)
    );
    assert_eq!(rows[0].pending_index(), None);
    assert_eq!(rows[1].state(), None);
    assert_eq!(rows[1].pending_index(), Some(0));
}

#[test]
fn attention_rows_never_fold_one_pending_input_into_two_features() {
    let mut app = app_with_agents(&[AgentKind::Claude, AgentKind::Claude]);
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.record_attention(
        "amf-feat-1",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.pending_inputs.push(input_request_for("feat-0"));

    // Two rows, one pending input: whichever row claims it, the other must not
    // dispatch through the same index and consume someone else's request.
    let rows = app.attention_rows();
    assert_eq!(rows.len(), 2);
    let claimed: Vec<Option<usize>> = rows.iter().map(|row| row.pending_index()).collect();
    assert_eq!(claimed, vec![Some(0), None]);
}

#[test]
fn ageing_out_takes_the_input_request_the_record_explained() {
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.config.waiting_stale_minutes = 30;
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.attention.get_mut("amf-feat-0").unwrap().since = Utc::now() - Duration::minutes(31);
    app.pending_inputs.push(input_request_for("feat-0"));

    assert!(app.age_out_attention());

    // Dropping the record alone would leave the input request behind as a
    // standalone row, and the session would never leave the `i` list.
    assert!(app.attention_for("amf-feat-0").is_none());
    assert!(app.attention_rows().is_empty());
    assert!(app.pending_inputs.is_empty());
}

#[test]
fn ageing_out_leaves_pending_work_it_never_explained() {
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.config.waiting_stale_minutes = 30;
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    app.attention.get_mut("amf-feat-0").unwrap().since = Utc::now() - Duration::minutes(31);

    // A diff review on the same feature is separate work, not a description of
    // why the session stopped, so ageing must not silently discard it.
    let mut diff_review = input_request_for("feat-0");
    diff_review.notification_type = "diff-review".to_string();
    app.pending_inputs.push(diff_review);

    assert!(app.age_out_attention());
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.attention_rows().len(), 1);
}

#[test]
fn badge_counts_keep_pending_work_the_attention_layer_cannot_explain() {
    let mut app = app_with_agents(&[AgentKind::Claude, AgentKind::Claude]);
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );
    // The same feature's generic request is the question seen through the old
    // signal, so it is not counted twice...
    app.pending_inputs.push(input_request_for("feat-0"));
    // ...but another feature's diff review is separate work, and must not
    // vanish from the badge just because someone else asked a question.
    let mut diff_review = input_request_for("feat-1");
    diff_review.notification_type = "diff-review".to_string();
    app.pending_inputs.push(diff_review);

    assert_eq!(app.attention_badge_counts(), ((1, 0, 0), 1));
    assert_eq!(
        crate::ui::header::badge_text((1, 0, 0), 1).as_deref(),
        Some("  [1 question, 1 input request | <leader i>]")
    );
}

#[test]
fn badge_hit_columns_follow_the_rendered_title_not_a_hand_counted_constant() {
    use ratatui::layout::Rect;

    let badge = crate::ui::header::badge_text((1, 0, 0), 0).expect("badge");
    let cwd = "/home/dev/code/agent-mainframe";
    // Wide enough that the help hint clamps nothing.
    let area = Rect::new(0, 0, 200, 3);

    let columns = crate::ui::header::badge_hit_columns(area, cwd, "0.37.0", (1, 0, 0), 0)
        .expect("badge is clickable");

    // The prefix is the border, then the spans `draw` renders ahead of the
    // badge: " Agent Mainframe ", "v0.37.0 ", "| ", cwd. Spelled out here so a
    // version string of a different length cannot silently shift the hit box
    // out from under the badge.
    let expected_start =
        1 + (" Agent Mainframe ".len() + "v0.37.0 ".len() + "| ".len() + cwd.len());
    assert_eq!(columns.start, expected_start as u16);
    assert_eq!(columns.end, columns.start + badge.len() as u16);
}

#[test]
fn a_narrow_header_shortens_the_cwd_rather_than_the_needs_attention_badge() {
    use ratatui::layout::Rect;

    let badge = crate::ui::header::badge_text((1, 0, 0), 0).expect("badge");
    let cwd = "/home/dev/code/agent-mainframe/.worktrees/some-long-branch";
    let full_width = |width: u16| {
        crate::ui::header::badge_hit_columns(Rect::new(0, 0, width, 3), cwd, "0.37.0", (1, 0, 0), 0)
    };

    let wide = full_width(200).expect("badge is clickable");
    assert_eq!(wide.end - wide.start, badge.len() as u16);

    // Squeeze the row well past where the untruncated title would have run
    // into the help hint. The badge names work waiting on the user and the key
    // that reaches it, so it keeps its columns and the path gives them up.
    let squeezed = full_width(70).expect("badge survives a narrow header");
    assert!(
        squeezed.start < wide.start,
        "expected the badge to move left as the cwd shortened, got {squeezed:?} vs {wide:?}"
    );
    assert_eq!(
        squeezed.end - squeezed.start,
        badge.len() as u16,
        "the badge should be whole, not clipped by the help hint"
    );

    // Below the width that fits a shortened path plus the badge, the badge is
    // partly drawn at most — and never reported clickable beyond what is drawn.
    for width in 10..70u16 {
        if let Some(columns) = full_width(width) {
            let (_, hint) = (0, 7u16);
            let title_end = width.saturating_sub(1).saturating_sub(hint);
            assert!(
                columns.end <= title_end.max(1),
                "width {width}: hit box {columns:?} reaches under the help hint"
            );
        }
    }
}

#[test]
fn a_waiting_session_still_counts_toward_dormancy_and_the_agent_gate() {
    // The attention layer is advisory: it explains why a session stopped, it
    // never excuses the session from the resource guards. Both checks read the
    // store, which cannot see the attention map — this pins that.
    let mut app = app_with_agents(&[AgentKind::Claude]);
    app.record_attention(
        "amf-feat-0",
        &AgentKind::Claude,
        AttentionState::Question,
        AttentionSource::Hook,
    );

    let session_name = "amf-feat-0".to_string();
    app.store.projects[0].features[0]
        .sessions
        .push(FeatureSession {
            id: "sess-1".to_string(),
            kind: SessionKind::Claude,
            label: "claude".to_string(),
            tmux_window: "claude".to_string(),
            command: None,
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
            created_at: Utc::now(),
        });

    // Agent gate: the harness is running, so it is counted.
    let live = crate::resources::limits::LiveHarnesses::from_census(
        &[(session_name.clone(), "claude".to_string(), 0)],
        &[],
    );
    let active = crate::resources::limits::active_harness_sessions(&app.store, &live);
    assert_eq!(active.len(), 1, "a waiting agent still occupies a slot");

    // Dormancy: idle and unattended past both thresholds, question or not.
    let now = Utc::now();
    app.store.projects[0].features[0].last_accessed = now - Duration::hours(5);
    let activity = HashMap::from([(session_name, now - Duration::hours(2))]);
    let dormant = crate::app::dormant::dormant_features(
        &app.store,
        &activity,
        now,
        std::time::Duration::from_secs(60 * 60),
        std::time::Duration::from_secs(4 * 3600),
        &|_| false,
    );
    assert_eq!(dormant.len(), 1, "a waiting feature can still be dormant");
}
