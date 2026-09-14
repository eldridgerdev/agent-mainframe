//! `App`-side lifecycle management for the Remote Control companion-app
//! server. See `crate::remote_server` for the server thread itself and
//! `docs/backlog/remote-control-companion-app-plan.md` (Epic 1) for the
//! design this implements.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::remote_server::{
    self, AuthorizedDevice, PairingExchangeOutcome, RemoteFeatureStatus, RemoteServerEvent,
    RemoteStatusSnapshot,
};

use super::{
    App, AppMode, PairingDialogStatus, PairingDialogView, RemoteDevicesListState,
    RemotePairingState,
};

/// Default bind address: loopback-only until the pairing/auth epics land,
/// so the skeleton never exposes anything beyond localhost. Port 0 asks the
/// OS for any free port rather than fixing one — nothing outside this
/// machine can reach it yet, no user ever types it, and the eventual
/// pairing QR (Epic 4) will encode whatever host:port the server actually
/// bound to, so there is no reason to also solve "what if the fixed port
/// is taken" right now.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:0";

/// How long a freshly generated pairing code stays valid. Short enough that
/// a code left on screen isn't a standing risk, long enough to actually
/// scan a QR and complete an exchange.
const PAIRING_CODE_TTL: Duration = Duration::from_secs(300);

/// Failed exchange attempts against one pairing code before it's locked out
/// and a fresh one must be generated (`r` in the dialog). Counted per code,
/// not per device — a new code resets the counter.
const MAX_PAIRING_ATTEMPTS: u32 = 5;

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
                    self.remote_server_addr = Some(addr);
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
                    self.remote_server_addr = None;
                    // An in-progress pairing dialog is now pairing against
                    // a server that no longer exists — say so rather than
                    // leaving it silently stuck on "Waiting for phone…".
                    if let AppMode::RemotePairing(state) = &mut self.mode {
                        state.status =
                            PairingDialogStatus::Failed("Remote-control server stopped".into());
                    }
                }
            }
        }

        // Push a fresh snapshot every tick the server is up. Building it is
        // a cheap in-memory scan (no I/O), and the server thread only ever
        // sees the latest one — see `RemoteServerHandle::publish_status`.
        if let Some(handle) = &self.remote_server {
            handle.publish_status(self.build_remote_status_snapshot());
            handle.publish_authorized_devices(self.build_authorized_devices());
        }

        self.drain_pairing_requests() || self.drain_device_seen_events() || changed
    }

    /// Open the pairing dialog with a fresh one-time code, or explain why
    /// not: the server (Epic 1) must already be running — pairing never
    /// starts it, per the on-demand server-lifecycle decision.
    pub fn start_pairing(&mut self) {
        if self.remote_server.is_none() {
            self.push_toast_warning("Start the remote-control server first (Ctrl+Space C)");
            return;
        }
        let Some(addr) = self.remote_server_addr else {
            self.push_toast_warning(
                "Remote-control server is still starting — try again in a moment",
            );
            return;
        };
        self.mode = AppMode::RemotePairing(self.build_pairing_state(addr));
    }

    /// Replace the current code with a fresh one — used both for the `r`
    /// (regenerate) key and to recover from an expired/locked-out code.
    /// Does nothing outside the pairing dialog.
    pub fn regenerate_pairing_code(&mut self) {
        let Some(addr) = self.remote_server_addr else {
            self.cancel_pairing();
            self.push_toast_warning("Remote-control server is no longer running");
            return;
        };
        if matches!(self.mode, AppMode::RemotePairing(_)) {
            self.mode = AppMode::RemotePairing(self.build_pairing_state(addr));
        }
    }

    fn build_pairing_state(&self, addr: SocketAddr) -> RemotePairingState {
        let code = remote_server::generate_pairing_code();
        let qr_payload = format!("amf-pair://{addr}?code={code}");
        let qr_lines = crate::qr::render_qr_lines(&qr_payload).unwrap_or_default();
        RemotePairingState {
            code,
            addr,
            qr_lines,
            expires_at: Instant::now() + PAIRING_CODE_TTL,
            attempts: 0,
            locked: false,
            status: PairingDialogStatus::Waiting,
            view: PairingDialogView::Pairing,
        }
    }

    /// Close the pairing dialog. Since `RemotePairingState` *is* the
    /// pending-pairing state (see its doc comment), dropping it here is
    /// what invalidates the code — a closed dialog leaves nothing an
    /// in-flight `/pair/exchange` request can still match.
    pub fn cancel_pairing(&mut self) {
        if matches!(self.mode, AppMode::RemotePairing(_)) {
            self.mode = AppMode::Normal;
        }
    }

    /// Switch the open pairing dialog to its paired-devices sub-screen
    /// (`v`), loading the current list fresh from the database. Does
    /// nothing outside the dialog, and silently no-ops with no database —
    /// there is nothing to list.
    pub fn open_paired_devices_view(&mut self) {
        if !matches!(self.mode, AppMode::RemotePairing(_)) {
            return;
        }
        let listed = self.db.as_ref().map(|db| db.list_remote_devices());
        let devices = match listed {
            Some(Ok(devices)) => devices,
            Some(Err(e)) => {
                self.log_error("remote_server", format!("failed to list devices: {e}"));
                Vec::new()
            }
            None => Vec::new(),
        };
        if let AppMode::RemotePairing(state) = &mut self.mode {
            state.view = PairingDialogView::Devices(RemoteDevicesListState {
                devices,
                selected: 0,
                confirm_revoke: false,
            });
        }
    }

    /// Back out of the devices sub-screen to the pairing code — `Esc` here
    /// returns to `Pairing` rather than closing the dialog; only `Esc` from
    /// `Pairing` itself does that (`cancel_pairing`).
    pub fn close_paired_devices_view(&mut self) {
        if let AppMode::RemotePairing(state) = &mut self.mode {
            state.view = PairingDialogView::Pairing;
        }
    }

    /// Move the devices-list cursor by `delta`, clamped to the list — and
    /// drop any pending revoke confirmation, same as any other key would
    /// (see `RemoteDevicesListState::confirm_revoke`'s doc comment).
    pub fn move_paired_device_selection(&mut self, delta: i32) {
        let AppMode::RemotePairing(state) = &mut self.mode else {
            return;
        };
        let PairingDialogView::Devices(list) = &mut state.view else {
            return;
        };
        list.confirm_revoke = false;
        if list.devices.is_empty() {
            return;
        }
        let len = list.devices.len() as i32;
        let next = (list.selected as i32 + delta).rem_euclid(len);
        list.selected = next as usize;
    }

    /// Clear a pending revoke confirmation without moving the cursor — any
    /// key other than a second `d` does this.
    pub fn clear_revoke_confirmation(&mut self) {
        let AppMode::RemotePairing(state) = &mut self.mode else {
            return;
        };
        if let PairingDialogView::Devices(list) = &mut state.view {
            list.confirm_revoke = false;
        }
    }

    /// `d` on the devices list: arms a confirmation on the first press,
    /// revokes the selected device on the second — mirrors the prompt
    /// overrides manager's `d`, `d` clear (`app/prompt_overrides.rs`).
    /// Already-revoked rows and an empty list are no-ops either way.
    pub fn request_revoke_selected_device(&mut self) {
        enum Step {
            None,
            Arm {
                device_name: String,
            },
            Revoke {
                device_id: String,
                device_name: String,
            },
        }

        let step = 'step: {
            let AppMode::RemotePairing(state) = &mut self.mode else {
                break 'step Step::None;
            };
            let PairingDialogView::Devices(list) = &mut state.view else {
                break 'step Step::None;
            };
            let Some(device) = list.devices.get(list.selected) else {
                break 'step Step::None;
            };
            if device.revoked {
                break 'step Step::None;
            }
            if !list.confirm_revoke {
                list.confirm_revoke = true;
                break 'step Step::Arm {
                    device_name: device.name.clone(),
                };
            }
            Step::Revoke {
                device_id: device.id.clone(),
                device_name: device.name.clone(),
            }
        };

        let (device_id, device_name) = match step {
            Step::None => return,
            Step::Arm { device_name } => {
                self.push_toast_warning(format!("Revoke \"{device_name}\"? Press d again."));
                return;
            }
            Step::Revoke {
                device_id,
                device_name,
            } => (device_id, device_name),
        };

        let Some(db) = &self.db else { return };
        match db.revoke_remote_device(&device_id) {
            Ok(()) => {
                self.log_info("remote_server", format!("Device revoked: {device_id}"));
                self.push_toast_info(format!("Revoked \"{device_name}\""));
                if let AppMode::RemotePairing(state) = &mut self.mode
                    && let PairingDialogView::Devices(list) = &mut state.view
                    && let Some(device) = list.devices.get_mut(list.selected)
                {
                    device.revoked = true;
                    list.confirm_revoke = false;
                }
            }
            Err(e) => {
                self.log_error("remote_server", format!("failed to revoke device: {e}"));
                self.push_toast_warning("Failed to revoke device — check the debug log (D)");
            }
        }
    }

    /// The device-token authorization table `/status` (and any future
    /// authenticated route) checks incoming `Authorization: Bearer <token>`
    /// headers against, published to the server thread every tick like the
    /// status snapshot. Revoked devices are simply left out, so a revoke
    /// takes effect on the next tick rather than needing its own teardown
    /// path — there's no persistent connection yet (that's Epic 6) for a
    /// revoke to have to tear down.
    fn build_authorized_devices(&self) -> HashMap<String, AuthorizedDevice> {
        let Some(db) = &self.db else {
            return HashMap::new();
        };
        let devices = db.list_remote_devices().unwrap_or_default();
        devices
            .into_iter()
            .filter(|d| !d.revoked)
            .map(|d| {
                (
                    d.token_hash,
                    AuthorizedDevice {
                        device_id: d.id,
                        device_name: d.name,
                    },
                )
            })
            .collect()
    }

    /// Record a last-seen timestamp for every device that made an
    /// authenticated request since the last tick. Best-effort: a failed
    /// write here is logged, not surfaced to the phone, since it already
    /// got its actual response.
    fn drain_device_seen_events(&mut self) -> bool {
        let Some(handle) = &mut self.remote_server else {
            return false;
        };
        let mut seen = Vec::new();
        while let Some(entry) = handle.try_recv_device_seen() {
            seen.push(entry);
        }
        let changed = !seen.is_empty();
        let errors: Vec<String> = match &self.db {
            Some(db) => seen
                .into_iter()
                .filter_map(|(id, name)| {
                    db.touch_remote_device_last_seen(&id)
                        .err()
                        .map(|e| format!("failed to record last-seen for {name} ({id}): {e}"))
                })
                .collect(),
            None => Vec::new(),
        };
        for msg in errors {
            self.log_error("remote_server", msg);
        }
        changed
    }

    /// Drain every pairing exchange request that arrived since the last
    /// tick, answering each one inline. Returns `true` if any arrived (so
    /// the caller folds this into its own redraw signal).
    fn drain_pairing_requests(&mut self) -> bool {
        let Some(handle) = &mut self.remote_server else {
            return false;
        };
        let mut requests = Vec::new();
        while let Some(req) = handle.try_recv_pairing_request() {
            requests.push(req);
        }
        let changed = !requests.is_empty();
        for req in requests {
            let outcome = self.process_pairing_exchange(&req.code, &req.device_name);
            self.apply_pairing_outcome(outcome.clone());
            let _ = req.reply.send(outcome);
        }
        changed
    }

    /// Validate `code` against the active `RemotePairing` dialog state and,
    /// on a match, mint and persist a new device. The only source of truth
    /// for a valid code is `self.mode` itself — see `RemotePairingState`'s
    /// doc comment — so a request with no pairing dialog open, or one that
    /// doesn't match, is indistinguishable from an unknown code to the
    /// caller (`PairingExchangeOutcome::InvalidCode` either way).
    fn process_pairing_exchange(
        &mut self,
        code: &str,
        device_name: &str,
    ) -> PairingExchangeOutcome {
        let AppMode::RemotePairing(state) = &mut self.mode else {
            return PairingExchangeOutcome::InvalidCode;
        };
        if state.locked {
            return PairingExchangeOutcome::LockedOut;
        }
        if Instant::now() > state.expires_at {
            return PairingExchangeOutcome::Expired;
        }
        if state.code != code {
            state.attempts += 1;
            if state.attempts >= MAX_PAIRING_ATTEMPTS {
                state.locked = true;
                return PairingExchangeOutcome::LockedOut;
            }
            return PairingExchangeOutcome::InvalidCode;
        }

        let name = {
            let trimmed = device_name.trim();
            if trimmed.is_empty() {
                "New device".to_string()
            } else {
                trimmed.to_string()
            }
        };
        let Some(db) = &self.db else {
            self.log_error(
                "remote_server",
                "pairing: no database configured, cannot persist device".to_string(),
            );
            return PairingExchangeOutcome::InternalError;
        };
        let token = remote_server::generate_device_token();
        let hash = remote_server::hash_token(&token);
        match db.create_remote_device(&name, &hash) {
            Ok(device) => {
                // One-time code: lock it out after a single successful
                // exchange too, not just after failures, so a replayed
                // request (or an attacker who saw the code) can't mint a
                // second device before the dialog is closed.
                if let AppMode::RemotePairing(state) = &mut self.mode {
                    state.locked = true;
                }
                PairingExchangeOutcome::Paired {
                    device_id: device.id,
                    device_name: name,
                    token,
                }
            }
            Err(e) => {
                self.log_error(
                    "remote_server",
                    format!("pairing: failed to persist device: {e}"),
                );
                PairingExchangeOutcome::InternalError
            }
        }
    }

    /// Reflect an exchange outcome in the dialog (if still open) and in the
    /// log/toast trail — mirrors how `poll_remote_server_bg` reports
    /// lifecycle events above.
    fn apply_pairing_outcome(&mut self, outcome: PairingExchangeOutcome) {
        match &outcome {
            PairingExchangeOutcome::Paired { device_id, .. } => {
                self.log_info("remote_server", format!("Device paired: {device_id}"));
                self.push_toast_info("Device paired");
            }
            PairingExchangeOutcome::InvalidCode => {
                self.log_debug("remote_server", "pairing: invalid code".to_string());
            }
            PairingExchangeOutcome::Expired => {
                self.log_debug("remote_server", "pairing: code expired".to_string());
            }
            PairingExchangeOutcome::LockedOut => {
                self.log_warn(
                    "remote_server",
                    "pairing: too many failed attempts, code locked".to_string(),
                );
            }
            PairingExchangeOutcome::InternalError => {}
        }

        if let AppMode::RemotePairing(state) = &mut self.mode {
            state.status = match outcome {
                PairingExchangeOutcome::Paired { device_name, .. } => {
                    PairingDialogStatus::Paired { device_name }
                }
                PairingExchangeOutcome::InvalidCode => PairingDialogStatus::Failed(format!(
                    "Invalid code ({} attempt{} left)",
                    MAX_PAIRING_ATTEMPTS.saturating_sub(state.attempts),
                    if MAX_PAIRING_ATTEMPTS.saturating_sub(state.attempts) == 1 {
                        ""
                    } else {
                        "s"
                    }
                )),
                PairingExchangeOutcome::Expired => {
                    PairingDialogStatus::Failed("Code expired — press r for a new one".into())
                }
                PairingExchangeOutcome::LockedOut => PairingDialogStatus::Failed(
                    "Too many failed attempts — press r for a new code".into(),
                ),
                PairingExchangeOutcome::InternalError => {
                    PairingDialogStatus::Failed("Pairing failed — check the debug log (D)".into())
                }
            };
        }
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
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut app = App::new_for_test(
            one_feature_store(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
        // /status is authenticated (Epic "device revoke" auth wiring) — a
        // paired device is what lets `build_authorized_devices` publish a
        // non-empty table for this request to pass.
        let token = "test-token";
        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Test Phone", &remote_server::hash_token(token))
            .unwrap();

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
            if let Ok(resp) = ureq::get(format!("http://{addr}/status"))
                .header("Authorization", format!("Bearer {token}"))
                .call()
            {
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

    fn test_app_with_db() -> (tempfile::NamedTempFile, App) {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut app = test_app();
        app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
        (db_file, app)
    }

    /// Opens the dialog the way `start_pairing` would, without needing a
    /// real bound server — `regenerate_pairing_code` also needs
    /// `remote_server_addr` set, so this sets it to match.
    fn open_pairing_dialog(app: &mut App) -> String {
        let addr: SocketAddr = "127.0.0.1:9".parse().unwrap();
        app.remote_server_addr = Some(addr);
        app.mode = AppMode::RemotePairing(app.build_pairing_state(addr));
        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        state.code.clone()
    }

    #[test]
    fn start_pairing_requires_a_running_server() {
        let mut app = test_app();
        app.start_pairing();
        assert!(
            matches!(app.mode, AppMode::Normal),
            "no server running, so pairing shouldn't open"
        );
    }

    #[test]
    fn start_pairing_waits_for_the_server_to_finish_binding() {
        let mut app = test_app();
        app.toggle_remote_server(); // Some(handle), but Started not drained yet
        app.start_pairing();
        assert!(
            matches!(app.mode, AppMode::Normal),
            "server handle exists but addr isn't known yet"
        );
        app.toggle_remote_server();
        wait_until_stopped(&mut app, Duration::from_secs(2));
    }

    #[test]
    fn start_pairing_opens_the_dialog_with_a_six_digit_code_and_a_qr() {
        let mut app = test_app();
        app.toggle_remote_server();
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.remote_server_addr.is_none() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(app.remote_server_addr.is_some());

        app.start_pairing();
        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("expected RemotePairing mode");
        };
        assert_eq!(state.code.len(), 6);
        assert!(state.code.chars().all(|c| c.is_ascii_digit()));
        assert!(!state.qr_lines.is_empty());
        assert!(matches!(state.status, PairingDialogStatus::Waiting));

        app.toggle_remote_server();
        wait_until_stopped(&mut app, Duration::from_secs(2));
    }

    #[test]
    fn cancel_pairing_drops_the_code_so_it_no_longer_validates() {
        let (_db_file, mut app) = test_app_with_db();
        let code = open_pairing_dialog(&mut app);
        app.cancel_pairing();
        assert!(matches!(app.mode, AppMode::Normal));

        let outcome = app.process_pairing_exchange(&code, "phone");
        assert!(matches!(outcome, PairingExchangeOutcome::InvalidCode));
    }

    #[test]
    fn correct_code_pairs_and_persists_a_device() {
        let (_db_file, mut app) = test_app_with_db();
        let code = open_pairing_dialog(&mut app);

        let outcome = app.process_pairing_exchange(&code, "Ryan's iPhone");
        match &outcome {
            PairingExchangeOutcome::Paired {
                device_name, token, ..
            } => {
                assert_eq!(device_name, "Ryan's iPhone");
                assert_eq!(token.len(), 64, "two simple-form UUIDs, hex, no dashes");
            }
            other => panic!("expected Paired, got {other:?}"),
        }

        let devices = app.db.as_ref().unwrap().list_remote_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Ryan's iPhone");

        // `process_pairing_exchange` alone only validates + mints — the
        // dialog's own status is only updated by `apply_pairing_outcome`,
        // same as the real `drain_pairing_requests` flow.
        app.apply_pairing_outcome(outcome);
        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("dialog should still be open, now showing success");
        };
        assert!(matches!(
            state.status,
            PairingDialogStatus::Paired { ref device_name } if device_name == "Ryan's iPhone"
        ));

        // The code is single-use: a replayed exchange must not mint a
        // second device even though the dialog is still showing success.
        let replay = app.process_pairing_exchange(&code, "Attacker");
        assert!(!matches!(replay, PairingExchangeOutcome::Paired { .. }));
        assert_eq!(
            app.db
                .as_ref()
                .unwrap()
                .list_remote_devices()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn blank_device_name_falls_back_to_a_default() {
        let (_db_file, mut app) = test_app_with_db();
        let code = open_pairing_dialog(&mut app);

        let outcome = app.process_pairing_exchange(&code, "   ");
        match outcome {
            PairingExchangeOutcome::Paired { device_name, .. } => {
                assert_eq!(device_name, "New device");
            }
            other => panic!("expected Paired, got {other:?}"),
        }
    }

    #[test]
    fn wrong_code_is_rejected_without_consuming_the_real_one() {
        let (_db_file, mut app) = test_app_with_db();
        let code = open_pairing_dialog(&mut app);

        let outcome = app.process_pairing_exchange("000000", "phone");
        assert!(matches!(outcome, PairingExchangeOutcome::InvalidCode));

        // The real code still works afterwards — one bad guess doesn't
        // burn the pairing session, only counts toward the lockout.
        let outcome = app.process_pairing_exchange(&code, "phone");
        assert!(matches!(outcome, PairingExchangeOutcome::Paired { .. }));
    }

    #[test]
    fn repeated_wrong_codes_lock_out_the_pairing_session() {
        let (_db_file, mut app) = test_app_with_db();
        let code = open_pairing_dialog(&mut app);

        for attempt in 1..=MAX_PAIRING_ATTEMPTS {
            let outcome = app.process_pairing_exchange("000000", "phone");
            if attempt < MAX_PAIRING_ATTEMPTS {
                assert!(
                    matches!(outcome, PairingExchangeOutcome::InvalidCode),
                    "attempt {attempt} should still be a plain rejection"
                );
            } else {
                assert!(
                    matches!(outcome, PairingExchangeOutcome::LockedOut),
                    "the attempt that hits the cap should lock out"
                );
            }
        }

        // Locked out even with the correct code now — only a fresh code
        // (`r` / regenerate_pairing_code) recovers.
        let outcome = app.process_pairing_exchange(&code, "phone");
        assert!(matches!(outcome, PairingExchangeOutcome::LockedOut));
    }

    #[test]
    fn expired_code_is_rejected() {
        let (_db_file, mut app) = test_app_with_db();
        let code = open_pairing_dialog(&mut app);
        let AppMode::RemotePairing(state) = &mut app.mode else {
            unreachable!()
        };
        state.expires_at = Instant::now() - Duration::from_secs(1);

        let outcome = app.process_pairing_exchange(&code, "phone");
        assert!(matches!(outcome, PairingExchangeOutcome::Expired));
    }

    #[test]
    fn regenerate_replaces_the_code_and_invalidates_the_old_one() {
        let (_db_file, mut app) = test_app_with_db();
        let old_code = open_pairing_dialog(&mut app);

        app.regenerate_pairing_code();
        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("expected to still be in the pairing dialog");
        };
        let new_code = state.code.clone();
        assert_ne!(
            old_code, new_code,
            "collision is astronomically unlikely across 10^6 codes"
        );

        assert!(matches!(
            app.process_pairing_exchange(&old_code, "phone"),
            PairingExchangeOutcome::InvalidCode
        ));
        assert!(matches!(
            app.process_pairing_exchange(&new_code, "phone"),
            PairingExchangeOutcome::Paired { .. }
        ));
    }

    #[test]
    fn no_database_configured_fails_the_exchange_without_a_crash() {
        let mut app = test_app(); // no db attached
        let code = open_pairing_dialog(&mut app);

        let outcome = app.process_pairing_exchange(&code, "phone");
        assert!(matches!(outcome, PairingExchangeOutcome::InternalError));
    }

    #[test]
    fn server_stopping_marks_an_open_pairing_dialog_as_failed() {
        let (_db_file, mut app) = test_app_with_db();
        app.toggle_remote_server();
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.remote_server_addr.is_none() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.start_pairing();
        assert!(matches!(app.mode, AppMode::RemotePairing(_)));

        app.toggle_remote_server(); // request stop
        wait_until_stopped(&mut app, Duration::from_secs(2));

        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("dialog should stay open to show the failure");
        };
        assert!(matches!(state.status, PairingDialogStatus::Failed(_)));
    }

    /// End-to-end: a real HTTP client exchanging a real pairing code over
    /// the real server, with `App::poll_remote_server_bg` as the only thing
    /// answering it — the same wiring a real phone would go through.
    #[test]
    fn pair_exchange_round_trips_over_real_http() {
        let (_db_file, mut app) = test_app_with_db();
        app.toggle_remote_server();
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.remote_server_addr.is_none() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(10));
        }
        let addr = app.remote_server_addr.expect("server should have started");
        app.start_pairing();
        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        let code = state.code.clone();

        // A wrong code first, from a client that doesn't know the real one.
        let bad_url = format!("http://{addr}/pair/exchange");
        let wrong = std::thread::spawn({
            let bad_url = bad_url.clone();
            move || {
                let body = serde_json::json!({"code": "000000", "device_name": "x"}).to_string();
                ureq::post(&bad_url)
                    .content_type("application/json")
                    .send(body)
            }
        });
        let mut wrong_result = None;
        let deadline = Instant::now() + Duration::from_secs(2);
        while wrong_result.is_none() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(5));
            if wrong.is_finished() {
                wrong_result = Some(());
            }
        }
        let wrong_response = wrong.join().unwrap();
        assert!(matches!(wrong_response, Err(ureq::Error::StatusCode(401))));

        // Now the real code, from a "phone".
        let good_url = bad_url.clone();
        let good = std::thread::spawn(move || {
            let body = serde_json::json!({"code": code, "device_name": "Test Phone"}).to_string();
            ureq::post(&good_url)
                .content_type("application/json")
                .send(body)
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut response = None;
        while response.is_none() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(5));
            if good.is_finished() {
                response = Some(());
            }
        }
        let mut ok_response = good.join().unwrap().expect("valid code should pair");
        let text = ok_response.body_mut().read_to_string().unwrap();
        let body: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(body["token"].as_str().unwrap().len(), 64);
        assert!(!body["device_id"].as_str().unwrap().is_empty());

        let devices = app.db.as_ref().unwrap().list_remote_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].name, "Test Phone");

        app.toggle_remote_server();
        wait_until_stopped(&mut app, Duration::from_secs(2));
    }

    #[test]
    fn open_paired_devices_view_loads_the_current_list() {
        let (_db_file, mut app) = test_app_with_db();
        open_pairing_dialog(&mut app);
        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone A", "hash-a")
            .unwrap();
        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone B", "hash-b")
            .unwrap();

        app.open_paired_devices_view();
        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("expected the pairing dialog to still be open");
        };
        let PairingDialogView::Devices(list) = &state.view else {
            panic!("expected the devices sub-view");
        };
        assert_eq!(list.devices.len(), 2);
        assert_eq!(list.selected, 0);
    }

    #[test]
    fn close_paired_devices_view_returns_to_pairing() {
        let (_db_file, mut app) = test_app_with_db();
        open_pairing_dialog(&mut app);
        app.open_paired_devices_view();
        app.close_paired_devices_view();

        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("dialog should still be open");
        };
        assert!(matches!(state.view, PairingDialogView::Pairing));
    }

    #[test]
    fn move_paired_device_selection_wraps_and_clears_confirmation() {
        let (_db_file, mut app) = test_app_with_db();
        open_pairing_dialog(&mut app);
        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone A", "hash-a")
            .unwrap();
        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone B", "hash-b")
            .unwrap();
        app.open_paired_devices_view();

        app.move_paired_device_selection(1);
        assert_eq!(selected_index(&app), 1);
        // Wraps around past the end.
        app.move_paired_device_selection(1);
        assert_eq!(selected_index(&app), 0);
        // And past the start going the other way.
        app.move_paired_device_selection(-1);
        assert_eq!(selected_index(&app), 1);
    }

    fn selected_index(app: &App) -> usize {
        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("expected the pairing dialog to be open");
        };
        let PairingDialogView::Devices(list) = &state.view else {
            panic!("expected the devices sub-view");
        };
        list.selected
    }

    #[test]
    fn revoke_requires_a_second_d_and_then_flips_the_flag() {
        let (_db_file, mut app) = test_app_with_db();
        open_pairing_dialog(&mut app);
        let device = app
            .db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone A", "hash-a")
            .unwrap();
        app.open_paired_devices_view();

        // First press only arms the confirmation — nothing revoked yet.
        app.request_revoke_selected_device();
        assert!(
            !app.db
                .as_ref()
                .unwrap()
                .find_remote_device_by_id(&device.id)
                .unwrap()
                .unwrap()
                .revoked
        );
        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        let PairingDialogView::Devices(list) = &state.view else {
            unreachable!()
        };
        assert!(list.confirm_revoke);

        // Second press actually revokes, in the DB and in the dialog's own
        // copy of the row (no reload needed).
        app.request_revoke_selected_device();
        assert!(
            app.db
                .as_ref()
                .unwrap()
                .find_remote_device_by_id(&device.id)
                .unwrap()
                .unwrap()
                .revoked
        );
        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        let PairingDialogView::Devices(list) = &state.view else {
            unreachable!()
        };
        assert!(list.devices[0].revoked);
        assert!(!list.confirm_revoke);
    }

    #[test]
    fn any_other_key_clears_a_pending_revoke_confirmation() {
        let (_db_file, mut app) = test_app_with_db();
        open_pairing_dialog(&mut app);
        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone A", "hash-a")
            .unwrap();
        app.open_paired_devices_view();

        app.request_revoke_selected_device(); // arms it
        app.clear_revoke_confirmation();

        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        let PairingDialogView::Devices(list) = &state.view else {
            unreachable!()
        };
        assert!(!list.confirm_revoke);
    }

    #[test]
    fn revoked_devices_are_excluded_from_the_authorized_table() {
        let (_db_file, app) = test_app_with_db();
        let device = app
            .db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone A", "hash-a")
            .unwrap();

        assert_eq!(app.build_authorized_devices().len(), 1);

        app.db
            .as_ref()
            .unwrap()
            .revoke_remote_device(&device.id)
            .unwrap();
        assert!(app.build_authorized_devices().is_empty());
    }
}
