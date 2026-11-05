use super::pane_runtime::SnapshotSender;
use super::*;

impl App {
    pub fn new(db_path: PathBuf) -> Result<Self> {
        setup::ensure_notify_scripts();
        let db = crate::db::AmfDb::open_or_seed(&db_path, &crate::project::global_db_path())?;
        let (store, store_version) = db.load_store_versioned()?;
        setup::repair_unquoted_claude_hooks_for_store(&store);
        let sidebar_runtime = sidebar_runtime::SidebarRuntime::new();
        // These caches are populated by the background sidebar-load tasks
        // scheduled in startup task 7 (schedule_sidebar_loads_for_all_features).
        // Building them synchronously here required reading every Claude JSONL
        // and PLAN.md file before the first frame could draw, which caused the
        // "Loading AMF..." stall proportional to feature count.
        let latest_prompt_cache = HashMap::new();
        let config = load_config();
        let zai_enabled = config.zai.is_some();
        let zai_monthly = config.zai.as_ref().and_then(|z| z.get_monthly_limit());
        let zai_weekly = config.zai.as_ref().and_then(|z| z.get_weekly_limit());
        let zai_five_hour = config.zai.as_ref().and_then(|z| z.get_five_hour_limit());
        let global_ext = load_global_extension_config();
        let active_extension = store
            .projects
            .first()
            .map(|p| merge_project_extension_config(&global_ext, &p.repo))
            .unwrap_or(global_ext);
        let sidebar_plan_cache = HashMap::new();
        let (codex_sidebar_metadata_tx, codex_sidebar_metadata_rx) = std::sync::mpsc::channel();
        let (view_snapshot_tx, wakeup_rx) = SnapshotSender::new();
        let (harness_check_tx, harness_check_rx) = channel();
        let mut theme = crate::theme::Theme::load(&config.theme);
        theme.set_transparent(config.transparent_background);
        let store_path = db.path.clone();
        let mut app = Self {
            sidebar_runtime,
            store,
            store_path,
            db: Some(db),
            store_version: Some(store_version),
            precall_cleared: None,
            create_project_git_probe: None,
            defer_review_progress_persist: false,
            precall_return: None,
            saved_config: config.clone(),
            config,
            active_extension,
            theme,
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            diagram_worker: crate::mermaid::DiagramWorker::default(),
            paused_plan_interview: None,
            pending_todo_plan_brief: None,
            pending_todo_spawn_prompt: None,
            todo_project_visible: true,
            todo_global_visible: true,
            message: None,
            toasts: Vec::new(),
            should_quit: false,
            pane_content: String::new(),
            pane_lines: Vec::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            viewport_cols: 0,
            viewport_rows: 0,
            viewport_total_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_scroll: 0,
            leader_max_scroll: std::cell::Cell::new(0),
            leader_activated_at: None,
            last_view_activity_at: None,
            view_input_batch: None,
            pending_inputs: Vec::new(),
            pending_automation_prompts: Vec::new(),
            user_stopped_features: std::collections::HashSet::new(),
            compose_direct_targets: std::collections::HashSet::new(),
            compose_drafts: HashMap::new(),
            compose_clipboard_paste: None,
            compose_submit: None,
            next_compose_clipboard_paste_id: 1,
            latest_prompt_cache,
            sidebar_model_cache: HashMap::new(),
            sidebar_plan_cache,
            sidebar_effective_plan_cache: HashMap::new(),
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_session_model_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            active_todos_sidebar_cache: HashMap::new(),
            learning_runs: learning::runtime::LearningRuns::default(),
            usage: UsageManager::new(zai_enabled, zai_monthly, zai_weekly, zai_five_hour),
            token_tracker: SessionTokenTracker::default(),
            context_states: HashMap::new(),
            context_hint_states: context_hints::ContextHintStates::default(),
            context_collector: SessionContextCollector::default(),
            session_status_bg: None,
            active_pr_bg: None,
            gh_graphql_limited_at: None,
            active_prs: HashMap::new(),
            terminal_prs: HashMap::new(),
            confirmed_no_terminal_pr: HashSet::new(),
            pr_review_work: pr_review::runtime::PrReviewWork::default(),
            issue_work: issue_fixer::IssueWork::default(),
            issue_comment_work: issue_fixer::IssueCommentWork::default(),
            latest_prompt_menu_bg: None,
            plan_interview_ai_bg: None,
            plan_interview_mcp_warned: None,
            plan_interview_synthesis_bg: None,
            plan_interview_critique_bg: None,
            plan_interview_directed_feedback_bg: None,
            plan_interview_investigation_bg: None,
            pr_review_return: None,
            pr_review_pending_local: None,
            review_memory_bootstrap_bg: None,
            review_memory_compact_bg: None,
            review_memory_compact_pending: None,
            memory_ai_summary_bg: None,
            ai_review_run: pr_review::runtime::AiReviewRun::default(),
            review_question_work: Default::default(),
            model_analysis_work: Default::default(),
            ai_review_return_to: None,
            pr_workspace: None,
            ai_review_fix_cost_cache: None,
            ai_review_triage_refresh_bg: None,
            ai_review_triage_refresh_pending: None,
            codex_models_cli_bg: None,
            gh_current_user: None,
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            attention: HashMap::new(),
            opencode_thinking_pane_cache: HashMap::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            awaiting_review_fixes: HashMap::new(),
            pending_editor: None,
            ipc_tool_sessions: std::collections::HashSet::new(),
            ipc_thinking_feature_sessions: std::collections::HashSet::new(),
            ipc_tool_feature_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux: Box::new(TmuxManager),
            worktree: std::sync::Arc::new(WorktreeManager),
            debug_log: DebugLog::default(),
            pending_debug_log_entries: VecDeque::new(),
            last_debug_log_flush_at: Instant::now(),
            perf: PerfCollector::new(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            fs_watcher: None,
            evidence_work: screenshots::EvidenceWork::default(),
            tmux_observer: None,
            ipc_chatty_log_count: 0,
            ipc_chatty_log_window_start: Instant::now(),
            last_file_notification_count: 0,
            last_file_notification_fingerprint: None,
            // Checked asynchronously — defaults to false until confirmed.
            vscode_available: false,
            pending_editor_launches: Vec::new(),
            pane_runtime: pane_runtime::PaneRuntime {
                view_snapshot_tx,
                view_snapshot_wakeup_rx: Some(wakeup_rx),
                view_snapshot_stop: None,
                view_snapshot_refresh: None,
                view_snapshot_include_content: None,
                view_snapshot_condvar: None,
                view_snapshot_target: None,
                view_display_frozen_until: None,
            },
            harness_check_tx,
            harness_check_rx,
            remote_server: None,
            remote_server_addr: None,
            pairing_requested: None,
            remote_tailscale: Default::default(),
            remote_reach: Default::default(),
            remote_push: Default::default(),
            remote_devices: Default::default(),
        };

        match crate::fswatch::FsWatcher::start(app.view_wakeup_tx()) {
            Ok(watcher) => app.fs_watcher = Some(watcher),
            Err(e) => {
                crate::debug::log_to_file(
                    crate::debug::LogLevel::Warn,
                    "fswatch",
                    &format!("watcher unavailable, falling back to scan timers: {e}"),
                );
            }
        }
        app.refresh_fs_watch_paths();
        app.load_terminal_prs_from_db();
        // A version-mismatched -C client hangs without delivering any
        // events; leaving the observer unset keeps the faster 5s
        // status-sync polling instead of trusting a dead event stream.
        if TmuxManager::control_clients_compatible() {
            app.tmux_observer = Some(crate::tmux_observer::TmuxObserver::start(
                app.view_wakeup_tx(),
            ));
        } else {
            crate::debug::log_to_file(
                crate::debug::LogLevel::Warn,
                "tmux",
                "tmux observer disabled (client/server version mismatch); using status polling",
            );
        }

        // Seed in-memory caches from DB so first use after restart is fast.
        if let Some(ref db) = app.db {
            let _ = db.evict_stale_token_cache();
            let _ = db.evict_stale_pr_review_cache();
            let _ = db.evict_stale_pr_comment_triage();
            let _ = db.evict_stale_ai_review_cache();
            if let Ok(entries) = db.load_token_cache() {
                app.token_tracker.seed_from_db_cache(entries);
            }
            if let Ok(entries) = db.load_recent_log(app.debug_log.max_entries()) {
                app.debug_log.inject_entries(entries);
            }
        }
        if let Err(e) = app.reconcile_todo_agent_associations() {
            app.log_warn(
                "todos",
                format!("failed startup TODO association reconciliation: {e}"),
            );
        }

        for message in crate::highlight::validate_startup_parsers() {
            match message.level {
                crate::highlight::StartupValidationLevel::Info => {
                    app.log_info("syntax", message.message);
                }
                crate::highlight::StartupValidationLevel::Warn => {
                    app.log_warn("syntax", message.message);
                }
                crate::highlight::StartupValidationLevel::Error => {
                    app.log_error("syntax", message.message);
                }
            }
        }
        crate::highlight::reload_runtime_state();
        app.queue_recoverable_issue_comments();

        Ok(app)
    }

    /// Lightweight constructor for unit/integration tests.
    ///
    /// Accepts a pre-built `ProjectStore` and injected trait objects so
    /// tests can drive App logic without touching the filesystem or
    /// spawning real tmux sessions.
    #[cfg(test)]
    pub fn new_for_test(
        store: ProjectStore,
        tmux: Box<dyn TmuxOps>,
        worktree: Box<dyn WorktreeOps>,
    ) -> Self {
        use crate::extension::ExtensionConfig;
        // `extension_for_repo` (and anything it feeds, e.g.
        // `configured_review_memory_path`) re-reads
        // `~/.config/amf/config.json` from disk regardless of this
        // constructor's in-memory setup, which would otherwise make tests
        // depend on whatever happens to be in that file on the machine
        // running them.
        crate::extension::set_test_global_extension_config(Some(ExtensionConfig::default()));
        let sidebar_runtime = sidebar_runtime::SidebarRuntime::new();
        let latest_prompt_cache = Self::build_latest_prompt_cache(&store);
        let sidebar_plan_cache = Self::build_sidebar_plan_cache(&store);
        let (codex_sidebar_metadata_tx, codex_sidebar_metadata_rx) = std::sync::mpsc::channel();
        let (view_snapshot_tx, wakeup_rx) = SnapshotSender::new();
        let (harness_check_tx, harness_check_rx) = channel();
        Self {
            sidebar_runtime,
            store,
            store_path: PathBuf::new(),
            db: None,
            store_version: None,
            precall_cleared: None,
            create_project_git_probe: None,
            defer_review_progress_persist: false,
            precall_return: None,
            saved_config: AppConfig::default(),
            config: AppConfig {
                // Whether a test warns before starting an agent must not
                // depend on how much memory the machine running it happens to
                // have free. Tests that exercise the memory gate set their own
                // threshold.
                low_memory_warn_mb: 0,
                ..AppConfig::default()
            },
            active_extension: ExtensionConfig::default(),
            theme: crate::theme::Theme::default(),
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            diagram_worker: crate::mermaid::DiagramWorker::default(),
            paused_plan_interview: None,
            pending_todo_plan_brief: None,
            pending_todo_spawn_prompt: None,
            todo_project_visible: true,
            todo_global_visible: true,
            message: None,
            toasts: Vec::new(),
            should_quit: false,
            pane_content: String::new(),
            pane_lines: Vec::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            viewport_cols: 0,
            viewport_rows: 0,
            viewport_total_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_scroll: 0,
            leader_max_scroll: std::cell::Cell::new(0),
            leader_activated_at: None,
            last_view_activity_at: None,
            view_input_batch: None,
            pending_inputs: Vec::new(),
            pending_automation_prompts: Vec::new(),
            user_stopped_features: std::collections::HashSet::new(),
            compose_direct_targets: std::collections::HashSet::new(),
            compose_drafts: HashMap::new(),
            compose_clipboard_paste: None,
            compose_submit: None,
            next_compose_clipboard_paste_id: 1,
            latest_prompt_cache,
            sidebar_model_cache: HashMap::new(),
            sidebar_plan_cache,
            sidebar_effective_plan_cache: HashMap::new(),
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_session_model_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            active_todos_sidebar_cache: HashMap::new(),
            learning_runs: learning::runtime::LearningRuns::default(),
            usage: UsageManager::new(false, None, None, None),
            token_tracker: SessionTokenTracker::default(),
            context_states: HashMap::new(),
            context_hint_states: context_hints::ContextHintStates::default(),
            context_collector: SessionContextCollector::default(),
            session_status_bg: None,
            active_pr_bg: None,
            gh_graphql_limited_at: None,
            active_prs: HashMap::new(),
            terminal_prs: HashMap::new(),
            confirmed_no_terminal_pr: HashSet::new(),
            pr_review_work: pr_review::runtime::PrReviewWork::default(),
            issue_work: issue_fixer::IssueWork::default(),
            issue_comment_work: issue_fixer::IssueCommentWork::default(),
            latest_prompt_menu_bg: None,
            plan_interview_ai_bg: None,
            plan_interview_mcp_warned: None,
            plan_interview_synthesis_bg: None,
            plan_interview_critique_bg: None,
            plan_interview_directed_feedback_bg: None,
            plan_interview_investigation_bg: None,
            pr_review_return: None,
            pr_review_pending_local: None,
            review_memory_bootstrap_bg: None,
            review_memory_compact_bg: None,
            review_memory_compact_pending: None,
            memory_ai_summary_bg: None,
            ai_review_run: pr_review::runtime::AiReviewRun::default(),
            review_question_work: Default::default(),
            model_analysis_work: Default::default(),
            ai_review_return_to: None,
            pr_workspace: None,
            ai_review_fix_cost_cache: None,
            ai_review_triage_refresh_bg: None,
            ai_review_triage_refresh_pending: None,
            codex_models_cli_bg: None,
            gh_current_user: None,
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            attention: HashMap::new(),
            opencode_thinking_pane_cache: HashMap::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            awaiting_review_fixes: HashMap::new(),
            pending_editor: None,
            ipc_tool_sessions: std::collections::HashSet::new(),
            ipc_thinking_feature_sessions: std::collections::HashSet::new(),
            ipc_tool_feature_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux,
            worktree: worktree.into(),
            debug_log: DebugLog::default(),
            pending_debug_log_entries: VecDeque::new(),
            last_debug_log_flush_at: Instant::now(),
            perf: PerfCollector::new(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            fs_watcher: None,
            evidence_work: screenshots::EvidenceWork::default(),
            tmux_observer: None,
            ipc_chatty_log_count: 0,
            ipc_chatty_log_window_start: Instant::now(),
            last_file_notification_count: 0,
            last_file_notification_fingerprint: None,
            vscode_available: false,
            pending_editor_launches: Vec::new(),
            pane_runtime: pane_runtime::PaneRuntime {
                view_snapshot_tx,
                view_snapshot_wakeup_rx: Some(wakeup_rx),
                view_snapshot_stop: None,
                view_snapshot_refresh: None,
                view_snapshot_include_content: None,
                view_snapshot_condvar: None,
                view_snapshot_target: None,
                view_display_frozen_until: None,
            },
            harness_check_tx,
            harness_check_rx,
            remote_server: None,
            remote_server_addr: None,
            pairing_requested: None,
            remote_tailscale: Default::default(),
            remote_reach: Default::default(),
            remote_push: Default::default(),
            remote_devices: Default::default(),
        }
    }

    pub(crate) fn has_active_sidebar(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::Viewing(view) if view.sidebar_session_kind().is_some()
        )
    }

    pub(crate) fn refresh_sidebar_for_current_view(&mut self) {
        let Some((project_name, feature_name, window, session_kind)) = (match &self.mode {
            AppMode::Viewing(view) => view.sidebar_session_kind().map(|session_kind| {
                (
                    view.project_name.clone(),
                    view.feature_name.clone(),
                    view.window.clone(),
                    session_kind,
                )
            }),
            _ => None,
        }) else {
            return;
        };

        let Some((pi, fi)) = self
            .store
            .projects
            .iter()
            .enumerate()
            .find(|(_, project)| project.name == project_name)
            .and_then(|(pi, project)| {
                project
                    .features
                    .iter()
                    .enumerate()
                    .find(|(_, feature)| feature.name == feature_name)
                    .map(|(fi, _)| (pi, fi))
            })
        else {
            return;
        };

        self.refresh_latest_prompt_for_feature(pi, fi);
        self.refresh_sidebar_plan_for_feature(pi, fi);
        self.request_codex_sidebar_metadata_for_view(
            &project_name,
            &feature_name,
            &window,
            &session_kind,
        );
        self.schedule_sidebar_load_for_feature(pi, fi);
    }
}
