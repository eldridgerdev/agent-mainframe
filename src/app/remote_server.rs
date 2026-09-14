//! `App`-side lifecycle management for the Remote Control companion-app
//! server. See `crate::remote_server` for the server thread itself and
//! `docs/backlog/remote-control-companion-app-plan.md` (Epic 1) for the
//! design this implements.

use std::net::SocketAddr;

use crate::remote_server::{self, RemoteFeatureStatus, RemoteServerEvent, RemoteStatusSnapshot};

use super::App;

/// Default bind address: loopback-only until the pairing/auth epics land,
/// so the skeleton never exposes anything beyond localhost. Port 0 asks the
/// OS for any free port rather than fixing one — nothing outside this
/// machine can reach it yet, no user ever types it, and the eventual
/// pairing QR (Epic 4) will encode whatever host:port the server actually
/// bound to, so there is no reason to also solve "what if the fixed port
/// is taken" right now.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:0";

impl App {
    /// Flip the on/off toggle. Never called automatically — the
    /// server-lifecycle decision is that this is on-demand only.
    pub fn toggle_remote_server(&mut self) {
        if self.remote_server.is_some() {
            self.stop_remote_server();
        } else {
            self.start_remote_server();
        }
    }

    fn start_remote_server(&mut self) {
        let bind_addr: SocketAddr = DEFAULT_BIND_ADDR
            .parse()
            .expect("DEFAULT_BIND_ADDR must be a valid socket address");
        self.log_info("remote_server", format!("Starting on {bind_addr}"));
        self.remote_server = Some(remote_server::start(bind_addr));
        self.push_toast_info("Remote-control server starting…");
    }

    /// Signals shutdown but leaves `self.remote_server` populated: the
    /// handle is still needed to receive the confirming `Stopped` event on
    /// a later `poll_remote_server_bg` tick, which is what actually clears
    /// it. So `self.remote_server` reads `Some` for the brief window
    /// between requesting a stop and the server thread confirming it.
    fn stop_remote_server(&mut self) {
        if self.remote_server.is_some() {
            self.log_info("remote_server", "Stop requested".to_string());
        }
        if let Some(handle) = &mut self.remote_server {
            handle.stop();
        }
    }

    /// Drain server lifecycle events. Called every main-loop tick, like the
    /// other `poll_*_bg` background jobs. Returns `true` when a redraw is
    /// warranted.
    pub fn poll_remote_server_bg(&mut self) -> bool {
        // Drain into a `Vec` first so the borrow of `self.remote_server`
        // ends before we call back into `self` (log/toast) below.
        let events: Vec<RemoteServerEvent> = match &self.remote_server {
            Some(handle) => handle.rx.try_iter().collect(),
            None => return false,
        };

        let changed = !events.is_empty();
        for event in events {
            match event {
                RemoteServerEvent::Started { addr } => {
                    self.log_info("remote_server", format!("Listening on {addr}"));
                    self.push_toast_info(format!("Remote-control server listening on {addr}"));
                }
                RemoteServerEvent::Stopped { error } => {
                    match error {
                        Some(err) => {
                            self.log_error("remote_server", format!("Server stopped: {err}"));
                            self.push_toast_warning(format!(
                                "Remote-control server stopped: {err}"
                            ));
                        }
                        None => {
                            self.log_info("remote_server", "Server stopped".to_string());
                            self.push_toast_info("Remote-control server stopped");
                        }
                    }
                    // Terminal: the server thread has exited, so there is
                    // nothing left to read from its receiver.
                    self.remote_server = None;
                }
            }
        }

        // Push a fresh snapshot every tick the server is up. Building it is
        // a cheap in-memory scan (no I/O), and the server thread only ever
        // sees the latest one — see `RemoteServerHandle::publish_status`.
        if let Some(handle) = &self.remote_server {
            handle.publish_status(self.build_remote_status_snapshot());
        }

        changed
    }

    /// Build the read-only status feed served at `/status` (Epic 5): one
    /// entry per feature across every project, with its persisted status
    /// and, when the in-memory attention layer has one, why it needs a
    /// look. A deliberately narrow read model — see
    /// `remote_server::RemoteFeatureStatus`.
    fn build_remote_status_snapshot(&self) -> RemoteStatusSnapshot {
        let features = self
            .store
            .projects
            .iter()
            .flat_map(|project| {
                project.features.iter().map(move |feature| {
                    let attention = self.attention.get(&feature.tmux_session);
                    RemoteFeatureStatus {
                        project_name: project.name.clone(),
                        feature_name: feature.name.clone(),
                        status: feature.status.to_string(),
                        needs_attention: attention.is_some(),
                        attention_reason: attention.map(|record| record.state.label().to_string()),
                    }
                })
            })
            .collect();

        RemoteStatusSnapshot {
            generated_at: chrono::Utc::now().to_rfc3339(),
            features,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ProjectStore;
    use crate::app::attention::{AttentionRecord, AttentionState};
    use crate::project::{AgentKind, Feature, Project, ProjectStatus, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn empty_store() -> ProjectStore {
        ProjectStore {
            version: 2,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: std::collections::HashMap::new(),
        }
    }

    fn one_feature_store() -> ProjectStore {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".to_string(),
            name: "my-feature".to_string(),
            branch: "my-feature".to_string(),
            workdir: PathBuf::from("/tmp/wt"),
            is_worktree: true,
            tmux_session: "amf-my-feature".to_string(),
            sessions: vec![],
            collapsed: false,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: AgentKind::default(),
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
        };
        ProjectStore {
            version: 2,
            projects: vec![Project {
                id: "proj-1".to_string(),
                name: "my-project".to_string(),
                repo: PathBuf::from("/tmp/repo"),
                collapsed: false,
                features: vec![feature],
                created_at: now,
                preferred_agent: AgentKind::default(),
                is_git: true,
            }],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: std::collections::HashMap::new(),
        }
    }

    fn test_app() -> App {
        App::new_for_test(
            empty_store(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        )
    }

    /// Poll until the server has fully stopped (or the deadline passes),
    /// draining both the `Started` and `Stopped` events along the way.
    fn wait_until_stopped(app: &mut App, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while app.remote_server.is_some() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn toggle_starts_then_stops() {
        let mut app = test_app();
        assert!(app.remote_server.is_none());

        app.toggle_remote_server();
        assert!(app.remote_server.is_some());

        app.toggle_remote_server(); // requests stop
        wait_until_stopped(&mut app, Duration::from_secs(2));
        assert!(
            app.remote_server.is_none(),
            "server should report stopped once poll_remote_server_bg drains Stopped"
        );
    }

    #[test]
    fn snapshot_reflects_status_and_attention() {
        let mut app = App::new_for_test(
            one_feature_store(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );

        let snapshot = app.build_remote_status_snapshot();
        assert_eq!(snapshot.features.len(), 1);
        assert_eq!(snapshot.features[0].project_name, "my-project");
        assert_eq!(snapshot.features[0].feature_name, "my-feature");
        assert_eq!(snapshot.features[0].status, "active");
        assert!(!snapshot.features[0].needs_attention);
        assert_eq!(snapshot.features[0].attention_reason, None);

        app.attention.insert(
            "amf-my-feature".to_string(),
            AttentionRecord::new(
                AgentKind::Claude,
                AttentionState::Question,
                chrono::Utc::now(),
            ),
        );

        let snapshot = app.build_remote_status_snapshot();
        assert!(snapshot.features[0].needs_attention);
        assert_eq!(
            snapshot.features[0].attention_reason.as_deref(),
            Some("Question")
        );
    }

    #[test]
    fn poll_publishes_a_snapshot_while_the_server_is_running() {
        let mut app = App::new_for_test(
            one_feature_store(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );

        app.toggle_remote_server();
        // Read the Started event straight off the handle's receiver rather
        // than through poll_remote_server_bg — that method drains the same
        // channel, so calling both here would race it for the event.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut addr = None;
        while Instant::now() < deadline {
            if let Some(handle) = &app.remote_server
                && let Ok(crate::remote_server::RemoteServerEvent::Started { addr: a }) =
                    handle.rx.try_recv()
            {
                addr = Some(a);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let addr = addr.expect("server should have started");

        // The relay task inside the server thread applies snapshots
        // asynchronously, so poll the real HTTP endpoint briefly.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut saw_feature = false;
        while Instant::now() < deadline {
            app.poll_remote_server_bg(); // keeps publishing fresh snapshots
            if let Ok(resp) = ureq::get(format!("http://{addr}/status")).call() {
                let text = resp.into_body().read_to_string().unwrap_or_default();
                if text.contains("my-feature") {
                    saw_feature = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            saw_feature,
            "the server's /status endpoint should eventually reflect App's store"
        );

        app.toggle_remote_server();
        wait_until_stopped(&mut app, Duration::from_secs(2));
    }
}
