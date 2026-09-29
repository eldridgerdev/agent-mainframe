//! `App`-side lifecycle management for the Remote Control companion-app
//! server. See `crate::remote_server` for the server thread itself and
//! `docs/backlog/remote-control-companion-app-plan.md` (Epic 1) for the
//! design this implements.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use crate::project::ProjectStatus;
use crate::remote_server::{
    self, AuthorizedDevice, PairingExchangeOutcome, RemoteFeatureStatus, RemoteProjectInfo,
    RemoteServerEvent, RemoteSessionInfo, RemoteStatusSnapshot,
};
use crate::remote_terminal::PaneTarget;

use super::{
    App, AppMode, PairingDialogStatus, PairingDialogView, PairingUrlSource, RemoteDevicesListState,
    RemotePairingState, ViewState,
};

/// Tests bind here: port 0 asks the OS for any free port, so parallel
/// tests never collide. Real runs use `AppConfig::remote_bind`.
#[cfg(test)]
const TEST_BIND_ADDR: &str = "127.0.0.1:0";

/// How long a freshly generated pairing code stays valid. Short enough that
/// a code left on screen isn't a standing risk, long enough to actually
/// scan a QR and complete an exchange.
const PAIRING_CODE_TTL: Duration = Duration::from_secs(300);

/// Failed exchange attempts against one pairing code before it's locked out
/// and a fresh one must be generated (`r` in the dialog). Counted per code,
/// not per device — a new code resets the counter.
const MAX_PAIRING_ATTEMPTS: u32 = 5;

/// How long a `Ctrl+Space Q` pressed before the server was listening stays
/// live. Binding normally takes milliseconds; past this the user has
/// likely moved on (in a session view, to typing into the agent, whose
/// keys would land on a dialog that appeared late), so the request is
/// dropped with a toast rather than opening a dialog nobody is expecting.
pub(super) const PAIRING_REQUEST_WINDOW: Duration = Duration::from_secs(2);

/// How long a loaded authorized-device table is trusted before it is
/// re-read. Pairing and revoking from this instance invalidate it
/// immediately; this bounds how long a revoke made by another AMF instance
/// sharing `amf.db` can go unnoticed.
const AUTHORIZED_DEVICES_TTL: Duration = Duration::from_secs(5);

/// The authorized-device table, cached so the per-tick publish never
/// queries the database. On a failed reload the last good table is kept:
/// the PWA treats a 401 as a revocation and deletes its token, so
/// publishing an empty table over a transient SQLite error would unpair
/// every phone.
#[derive(Default)]
pub struct AuthorizedDevicesCache {
    table: HashMap<String, AuthorizedDevice>,
    /// When `table` was last loaded; `None` means reload on next use.
    loaded_at: Option<Instant>,
    /// `table` has changed (or the server restarted) since it was last
    /// published.
    unpublished: bool,
    /// The previous reload failed — logged once per failure streak rather
    /// than every tick while it keeps retrying.
    load_failing: bool,
}

/// The address the pairing QR sends a phone to, and where it came from:
/// the configured public URL (the user's own tunnel), else the HTTPS
/// address Tailscale serves AMF on, else the bind address.
fn pairing_base(
    public_url: Option<&str>,
    tailscale_url: Option<&str>,
    addr: SocketAddr,
) -> (String, PairingUrlSource) {
    if let Some(url) = public_url.map(str::trim).filter(|url| !url.is_empty()) {
        return (
            url.trim_end_matches('/').to_string(),
            PairingUrlSource::Configured,
        );
    }
    if let Some(url) = tailscale_url {
        return (
            url.trim_end_matches('/').to_string(),
            PairingUrlSource::Tailscale,
        );
    }
    (format!("http://{addr}"), PairingUrlSource::Direct)
}

/// What the QR encodes: the PWA's own URL with the code, so a phone camera
/// opens the pairing page pre-filled.
fn pairing_url(base: &str, code: &str) -> String {
    format!("{base}/?code={code}")
}

/// A bind address no phone can open: this machine's loopback, or the
/// wildcard, which isn't an address at all.
fn phone_cannot_open(source: PairingUrlSource, addr: SocketAddr) -> bool {
    source == PairingUrlSource::Direct && (addr.ip().is_loopback() || addr.ip().is_unspecified())
}

impl App {
    /// Flip the on/off toggle. Never called automatically — the
    /// server-lifecycle decision is that this is on-demand only. The only
    /// other start is `start_pairing`, which is itself a keypress asking
    /// for the server (`Ctrl+Space Q`), so it counts as on-demand too.
    pub fn toggle_remote_server(&mut self) {
        if self.remote_server.is_some() {
            self.stop_remote_server();
        } else {
            self.start_remote_server();
        }
    }

    fn start_remote_server(&mut self) {
        #[cfg(test)]
        let configured = TEST_BIND_ADDR.to_string();
        #[cfg(not(test))]
        let configured = self.config.remote_bind.clone();
        let bind_addr: SocketAddr = match configured.parse() {
            Ok(addr) => addr,
            Err(_) => {
                self.push_toast_warning(format!(
                    "remote_bind \"{configured}\" isn't host:port — using {}",
                    super::default_remote_bind()
                ));
                super::default_remote_bind()
                    .parse()
                    .expect("default_remote_bind is a valid address")
            }
        };
        self.log_info("remote_server", format!("Starting on {bind_addr}"));
        // A fresh server starts with an empty table: reload and republish.
        self.invalidate_authorized_devices();
        let vapid_public_key = self.ensure_vapid_key().map(|key| key.public_key_b64());
        self.remote_server = Some(remote_server::start(
            bind_addr,
            remote_server::ServerConfig {
                vapid_public_key,
                pane_io: std::sync::Arc::new(crate::remote_terminal::TmuxPaneIo),
            },
        ));
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
        let tailscale = self.poll_tailscale_bg();
        let events: Vec<RemoteServerEvent> = match &self.remote_server {
            Some(handle) => handle.rx.try_iter().collect(),
            None => return tailscale,
        };

        let changed = !events.is_empty();
        for event in events {
            match event {
                RemoteServerEvent::Started { addr } => {
                    self.log_info("remote_server", format!("Listening on {addr}"));
                    self.push_toast_info(format!("Remote-control server listening on {addr}"));
                    self.remote_server_addr = Some(addr);
                    self.probe_tailscale();
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
                    self.pairing_requested = None;
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

        // Pairing first, so a device minted this tick is already in the
        // authorized table published below — a table built before it would
        // otherwise reach the server thread after the pairing reply and
        // briefly un-authorize the token that reply just handed out.
        let pairing_opened = self.service_pairing_request();
        let paired = self.drain_pairing_requests();
        let seen = self.drain_device_seen_events();
        let pushed = self.drain_push_requests();
        let acted = self.drain_remote_commands();

        // Push a fresh snapshot every tick the server is up. Building it is
        // a cheap in-memory scan (no I/O), and the server thread only ever
        // sees the latest one — see `RemoteServerHandle::publish_status`.
        // The device table is cached and only republished when it changes.
        if self.remote_server.is_some() {
            self.refresh_authorized_devices();
        }
        if let Some(handle) = &self.remote_server {
            handle.publish_status(self.build_remote_status_snapshot());
            if self.remote_devices.unpublished {
                handle.publish_authorized_devices(self.remote_devices.table.clone());
                self.remote_devices.unpublished = false;
            }
        }

        tailscale || pairing_opened || paired || seen || pushed || acted || changed
    }

    /// Open the pairing dialog with a fresh one-time code. Pressing it is
    /// itself an on-demand request, so a stopped server is started first
    /// and the dialog opens once it is listening (`pairing_requested`).
    /// Opened over a session view, the dialog returns to it on close.
    pub fn start_pairing(&mut self) {
        if self.remote_server.is_none() {
            self.pairing_requested = Some(Instant::now());
            self.start_remote_server();
            return;
        }
        let Some(addr) = self.remote_server_addr else {
            self.pairing_requested = Some(Instant::now());
            return;
        };
        match self.take_pairing_host() {
            Some(view) => {
                self.mode = AppMode::RemotePairing(self.build_pairing_state(addr, view));
                self.probe_tailscale();
            }
            None => self.push_toast_info("Close this dialog, then Ctrl+Space Q to pair"),
        }
    }

    /// Honour a `Ctrl+Space Q` pressed before the server was listening.
    /// It waits for the address and for the dashboard or a session view
    /// to be on screen — another dialog reached in the meantime (help, a
    /// picker) is never replaced, but closing it within the window still
    /// gets the pairing dialog. Past `PAIRING_REQUEST_WINDOW` it is dropped
    /// with a toast naming the key, so a slow bind can't pop a dialog up
    /// under someone typing into an agent. Returns `true` when it opened.
    fn service_pairing_request(&mut self) -> bool {
        let Some(requested_at) = self.pairing_requested else {
            return false;
        };
        if requested_at.elapsed() > PAIRING_REQUEST_WINDOW {
            self.pairing_requested = None;
            self.push_toast_info(if self.remote_server_addr.is_some() {
                "Remote-control server ready — Ctrl+Space Q to pair"
            } else {
                "Remote-control server still starting — Ctrl+Space Q to pair once it's listening"
            });
            return true;
        }
        let Some(addr) = self.remote_server_addr else {
            return false;
        };
        let Some(view) = self.take_pairing_host() else {
            return false;
        };
        self.pairing_requested = None;
        self.mode = AppMode::RemotePairing(self.build_pairing_state(addr, view));
        self.probe_tailscale();
        true
    }

    /// The screen the pairing dialog may open over, taken out of
    /// `self.mode`: `Some(None)` for the dashboard, `Some(Some(view))` for
    /// a session view (the dialog returns to it on close). Anything else is
    /// left in place and yields `None`.
    fn take_pairing_host(&mut self) -> Option<Option<ViewState>> {
        match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::Normal => Some(None),
            AppMode::Viewing(view) => Some(Some(view)),
            other => {
                self.mode = other;
                None
            }
        }
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
        if let AppMode::RemotePairing(state) = &mut self.mode {
            let from_view = state.from_view.take();
            self.mode = AppMode::RemotePairing(self.build_pairing_state(addr, from_view));
        }
    }

    fn build_pairing_state(
        &self,
        addr: SocketAddr,
        from_view: Option<ViewState>,
    ) -> RemotePairingState {
        let code = remote_server::generate_pairing_code();
        let (url, url_source) = self.pairing_target(addr);
        let qr_lines = crate::qr::render_qr_lines(&pairing_url(&url, &code)).unwrap_or_default();
        RemotePairingState {
            code,
            url,
            qr_lines,
            expires_at: Instant::now() + PAIRING_CODE_TTL,
            attempts: 0,
            locked: false,
            status: PairingDialogStatus::Waiting,
            view: PairingDialogView::Pairing,
            url_unreachable: phone_cannot_open(url_source, addr),
            url_source,
            from_view,
        }
    }

    /// Open the dialog against `addr` without a running server, for render
    /// and handler tests outside this module.
    #[cfg(test)]
    pub(crate) fn open_pairing_dialog_for_test(&mut self, addr: SocketAddr) {
        self.remote_server_addr = Some(addr);
        self.mode = AppMode::RemotePairing(self.build_pairing_state(addr, None));
    }

    fn pairing_target(&self, addr: SocketAddr) -> (String, PairingUrlSource) {
        pairing_base(
            self.config.remote_public_url.as_deref(),
            self.remote_tailscale
                .status
                .as_ref()
                .and_then(|status| status.serve_url()),
            addr,
        )
    }

    /// Re-point an open dialog's QR when a Tailscale probe changes the
    /// answer — `t` just started serving, or the user ran `tailscale serve`
    /// in another terminal. The code is kept: only where the phone is sent
    /// changes.
    pub(super) fn refresh_pairing_url(&mut self) {
        let Some(addr) = self.remote_server_addr else {
            return;
        };
        let (url, url_source) = self.pairing_target(addr);
        let AppMode::RemotePairing(state) = &mut self.mode else {
            return;
        };
        if state.url == url {
            return;
        }
        state.qr_lines =
            crate::qr::render_qr_lines(&pairing_url(&url, &state.code)).unwrap_or_default();
        state.url_unreachable = phone_cannot_open(url_source, addr);
        state.url_source = url_source;
        state.url = url;
    }

    /// `s` in the pairing dialog: the Tailscale setup walkthrough, with a
    /// fresh probe so each step's tick reflects this moment.
    pub fn open_pairing_setup_view(&mut self) {
        if let AppMode::RemotePairing(state) = &mut self.mode {
            state.view = PairingDialogView::Setup { scroll: 0 };
            self.probe_tailscale();
        }
    }

    pub fn close_pairing_setup_view(&mut self) {
        if let AppMode::RemotePairing(state) = &mut self.mode
            && matches!(state.view, PairingDialogView::Setup { .. })
        {
            state.view = PairingDialogView::Pairing;
        }
    }

    pub fn scroll_pairing_setup(&mut self, delta: i32) {
        if let AppMode::RemotePairing(state) = &mut self.mode
            && let PairingDialogView::Setup { scroll } = &mut state.view
        {
            *scroll = scroll.saturating_add_signed(delta as i16);
        }
    }

    /// `c` in the setup view: copy the AMF access policy for pasting into
    /// Tailscale's access controls.
    pub fn copy_tailscale_access_policy(&mut self) {
        match crate::app::util::copy_to_clipboard(crate::tailscale::ACCESS_POLICY) {
            Ok(()) => self.push_toast_success("Copied the AMF access policy"),
            Err(e) => self.push_toast_error(format!("Clipboard error: {e}")),
        }
    }

    /// Close the pairing dialog. Since `RemotePairingState` *is* the
    /// pending-pairing state (see its doc comment), dropping it here is
    /// what invalidates the code — a closed dialog leaves nothing an
    /// in-flight `/pair/exchange` request can still match.
    pub fn cancel_pairing(&mut self) {
        self.mode = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::RemotePairing(state) => match state.from_view {
                Some(view) => AppMode::Viewing(view),
                None => AppMode::Normal,
            },
            other => other,
        };
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
                self.invalidate_push_subscriptions();
                self.invalidate_authorized_devices();
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

    /// The device-token authorization table `/status` (and every other
    /// authenticated route) checks incoming `Authorization: Bearer <token>`
    /// headers against. Revoked devices are simply left out, so a revoke
    /// takes effect as soon as the table is republished.
    fn load_authorized_devices(&self) -> anyhow::Result<HashMap<String, AuthorizedDevice>> {
        let Some(db) = &self.db else {
            return Ok(HashMap::new());
        };
        Ok(db
            .list_remote_devices()?
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
            .collect())
    }

    /// Reload the cached table if it was invalidated or has outlived
    /// `AUTHORIZED_DEVICES_TTL`. A failed reload keeps the last good table
    /// and retries next tick.
    pub(super) fn refresh_authorized_devices(&mut self) -> &HashMap<String, AuthorizedDevice> {
        let fresh = self
            .remote_devices
            .loaded_at
            .is_some_and(|at| at.elapsed() < AUTHORIZED_DEVICES_TTL);
        if !fresh {
            match self.load_authorized_devices() {
                Ok(table) => {
                    let cache = &mut self.remote_devices;
                    if table != cache.table {
                        cache.table = table;
                        cache.unpublished = true;
                    }
                    cache.loaded_at = Some(Instant::now());
                    cache.load_failing = false;
                }
                Err(e) => {
                    if !self.remote_devices.load_failing {
                        self.log_error(
                            "remote_server",
                            format!("Loading paired devices (keeping the last table): {e}"),
                        );
                    }
                    self.remote_devices.load_failing = true;
                }
            }
        }
        &self.remote_devices.table
    }

    /// Drop the cached device table — call after anything that changes
    /// which devices are authorized (pair, revoke) or when a new server
    /// needs the table published to it.
    pub(super) fn invalidate_authorized_devices(&mut self) {
        self.remote_devices.loaded_at = None;
        self.remote_devices.unpublished = true;
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
                self.invalidate_authorized_devices();
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
        let attention_by_feature = self.remote_attention_by_feature();
        let mut pane_targets = HashMap::new();
        let mut workdirs = HashMap::new();
        let mut features = Vec::new();
        for project in &self.store.projects {
            for feature in &project.features {
                let running = feature.status != ProjectStatus::Stopped;
                workdirs.insert(feature.id.clone(), feature.workdir.clone());
                let sessions = feature
                    .sessions
                    .iter()
                    .map(|session| {
                        let live = running && session.kind.is_tmux_backed();
                        if live {
                            pane_targets.insert(
                                session.id.clone(),
                                PaneTarget {
                                    session: feature.tmux_session.clone(),
                                    window: session.tmux_window.clone(),
                                },
                            );
                        }
                        RemoteSessionInfo {
                            id: session.id.clone(),
                            label: session.label.clone(),
                            kind: serde_json::to_value(&session.kind)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_string))
                                .unwrap_or_default(),
                            live,
                        }
                    })
                    .collect();
                let attention = attention_by_feature.get(&feature.id);
                features.push(RemoteFeatureStatus {
                    project_name: project.name.clone(),
                    feature_name: feature.name.clone(),
                    status: feature.status.to_string(),
                    needs_attention: attention.is_some(),
                    attention_reason: attention.map(|a| a.reason.clone()),
                    attention_detail: attention.and_then(|a| a.detail.clone()),
                    feature_id: feature.id.clone(),
                    branch: feature.branch.clone(),
                    agent: feature.agent.slug().to_string(),
                    sessions,
                    summary: feature.summary.clone(),
                    nickname: feature.nickname.clone(),
                });
            }
        }
        let projects = self
            .store
            .projects
            .iter()
            .map(|project| RemoteProjectInfo {
                name: project.name.clone(),
                preferred_agent: project.preferred_agent.slug().to_string(),
            })
            .collect();

        RemoteStatusSnapshot {
            generated_at: chrono::Utc::now().to_rfc3339(),
            features,
            projects,
            pane_targets,
            workdirs,
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
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
            issue_source: None,
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
    fn pairing_base_prefers_config_then_tailscale_then_the_bind_address() {
        let addr: SocketAddr = "127.0.0.1:47800".parse().unwrap();
        let ts = Some("https://pc.tail1.ts.net");
        assert_eq!(
            pairing_base(Some("https://tunnel.test/"), ts, addr),
            ("https://tunnel.test".into(), PairingUrlSource::Configured)
        );
        assert_eq!(
            pairing_base(Some("  "), ts, addr),
            (
                "https://pc.tail1.ts.net".into(),
                PairingUrlSource::Tailscale
            )
        );
        assert_eq!(
            pairing_base(None, None, addr),
            ("http://127.0.0.1:47800".into(), PairingUrlSource::Direct)
        );
        assert_eq!(
            pairing_url("https://pc.tail1.ts.net", "123456"),
            "https://pc.tail1.ts.net/?code=123456"
        );
    }

    fn serving(url: Option<&str>) -> crate::tailscale::TailscaleStatus {
        crate::tailscale::TailscaleStatus::Running(crate::tailscale::TailnetNode {
            dns_name: "pc.tail1.ts.net".into(),
            https_enabled: true,
            tagged_for_amf: false,
            serve_url: url.map(str::to_string),
        })
    }

    #[test]
    fn a_tailscale_probe_repoints_an_open_dialog_and_keeps_the_code() {
        let mut app = test_app();
        let code = open_pairing_dialog(&mut app);
        {
            let AppMode::RemotePairing(state) = &app.mode else {
                unreachable!()
            };
            assert_eq!(state.url_source, PairingUrlSource::Direct);
            assert!(state.url_unreachable);
        }

        app.feed_tailscale_probe(serving(Some("https://pc.tail1.ts.net")));
        assert!(app.poll_remote_server_bg());

        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("dialog should still be open");
        };
        assert_eq!(state.url, "https://pc.tail1.ts.net");
        assert_eq!(state.url_source, PairingUrlSource::Tailscale);
        assert!(!state.url_unreachable);
        assert_eq!(state.code, code);
        assert!(!app.remote_tailscale.probing);
    }

    #[test]
    fn a_configured_public_url_is_never_replaced_by_tailscale() {
        let mut app = test_app();
        app.config.remote_public_url = Some("https://tunnel.test".into());
        open_pairing_dialog(&mut app);
        app.feed_tailscale_probe(serving(Some("https://pc.tail1.ts.net")));
        app.poll_remote_server_bg();
        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        assert_eq!(state.url, "https://tunnel.test");
        assert_eq!(state.url_source, PairingUrlSource::Configured);
    }

    #[test]
    fn t_only_serves_once_tailscale_is_known_to_be_running() {
        use crate::tailscale::{ServeOutcome, TailscaleStatus};
        let mut app = test_app();
        open_pairing_dialog(&mut app);

        app.start_tailscale_serve(); // no probe yet
        assert!(!app.remote_tailscale.serving);

        app.remote_tailscale.status = Some(TailscaleStatus::NotInstalled);
        app.start_tailscale_serve();
        assert!(!app.remote_tailscale.serving);

        app.remote_tailscale.status = Some(serving(None));
        app.start_tailscale_serve();
        assert!(app.remote_tailscale.serving);

        let link = "https://login.tailscale.com/f/serve?node=abc".to_string();
        app.feed_tailscale_serve(ServeOutcome::NeedsApproval(link.clone()));
        app.poll_remote_server_bg();
        assert!(!app.remote_tailscale.serving);
        assert_eq!(
            app.remote_tailscale.serve_note,
            Some(ServeOutcome::NeedsApproval(link))
        );

        app.start_tailscale_serve();
        assert_eq!(
            app.remote_tailscale.serve_note, None,
            "a retry clears the note"
        );
        app.feed_tailscale_serve(ServeOutcome::Serving);
        app.poll_remote_server_bg();
        assert_eq!(app.remote_tailscale.serve_note, None);
        assert!(
            app.remote_tailscale.probing,
            "success re-probes what is served"
        );
    }

    #[test]
    fn the_setup_view_opens_scrolls_and_returns_to_the_code() {
        let mut app = test_app();
        open_pairing_dialog(&mut app);
        app.open_pairing_setup_view();
        app.scroll_pairing_setup(3);
        app.scroll_pairing_setup(-5);
        {
            let AppMode::RemotePairing(state) = &app.mode else {
                unreachable!()
            };
            assert!(matches!(state.view, PairingDialogView::Setup { scroll: 0 }));
        }
        assert!(
            app.remote_tailscale.probing,
            "opening setup re-checks Tailscale"
        );
        assert!(!app.pairing_setup_steps().is_empty());
        app.close_pairing_setup_view();
        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        assert!(matches!(state.view, PairingDialogView::Pairing));
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
        // paired device is what lets `refresh_authorized_devices` publish a
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

    /// `one_feature_store` (`my-project` / `my-feature`, tmux session
    /// `amf-my-feature`) backed by a temporary database.
    pub(in crate::app) fn test_app_with_feature_and_db() -> (tempfile::NamedTempFile, App) {
        let db_file = tempfile::NamedTempFile::new().unwrap();
        let mut app = App::new_for_test(
            one_feature_store(),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
        (db_file, app)
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
        app.mode = AppMode::RemotePairing(app.build_pairing_state(addr, None));
        let AppMode::RemotePairing(state) = &app.mode else {
            unreachable!()
        };
        state.code.clone()
    }

    #[test]
    fn start_pairing_starts_a_stopped_server_and_opens_once_it_listens() {
        let mut app = test_app();
        app.start_pairing();
        assert!(app.remote_server.is_some(), "Q should start the server");
        assert!(
            matches!(app.mode, AppMode::Normal),
            "the address isn't known until the server reports Started"
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        while !matches!(app.mode, AppMode::RemotePairing(_)) && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(10));
        }
        let AppMode::RemotePairing(state) = &app.mode else {
            panic!("expected the dialog once the server was listening");
        };
        assert!(state.from_view.is_none());
        assert!(app.pairing_requested.is_none());

        app.toggle_remote_server();
        wait_until_stopped(&mut app, Duration::from_secs(2));
    }

    #[test]
    fn a_deferred_pairing_request_waits_for_another_dialog_to_close() {
        let mut app = test_app();
        app.start_pairing();
        app.mode = AppMode::Help(crate::app::HelpState {
            from_view: None,
            scroll_offset: 0,
        });

        let deadline = Instant::now() + Duration::from_secs(2);
        while app.remote_server_addr.is_none() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.poll_remote_server_bg();
        assert!(
            matches!(app.mode, AppMode::Help(_)),
            "another dialog is never replaced"
        );
        assert!(app.pairing_requested.is_some(), "the request is kept");

        app.mode = AppMode::Normal;
        app.poll_remote_server_bg();
        assert!(matches!(app.mode, AppMode::RemotePairing(_)));
        assert!(app.pairing_requested.is_none());

        app.toggle_remote_server();
        wait_until_stopped(&mut app, Duration::from_secs(2));
    }

    #[test]
    fn a_pairing_request_older_than_its_window_is_dropped_not_opened() {
        let mut app = test_app();
        app.start_pairing();
        // Hold the dialog off while the server comes up, then age the
        // request past its window, as a slow bind would.
        app.mode = AppMode::Help(crate::app::HelpState {
            from_view: None,
            scroll_offset: 0,
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while app.remote_server_addr.is_none() && Instant::now() < deadline {
            app.poll_remote_server_bg();
            std::thread::sleep(Duration::from_millis(10));
        }
        app.mode = AppMode::Normal;
        app.pairing_requested =
            Instant::now().checked_sub(PAIRING_REQUEST_WINDOW + Duration::from_secs(1));

        assert!(app.poll_remote_server_bg());
        assert!(
            matches!(app.mode, AppMode::Normal),
            "a stale request must not pop a dialog up under the user"
        );
        assert!(app.pairing_requested.is_none());

        app.toggle_remote_server();
        wait_until_stopped(&mut app, Duration::from_secs(2));
    }

    #[test]
    fn pairing_flags_an_address_no_phone_can_open() {
        let mut app = test_app();
        let loopback: SocketAddr = "127.0.0.1:47800".parse().unwrap();
        let wildcard: SocketAddr = "0.0.0.0:47800".parse().unwrap();
        let lan: SocketAddr = "192.168.0.10:47800".parse().unwrap();

        assert!(app.build_pairing_state(loopback, None).url_unreachable);
        assert!(app.build_pairing_state(wildcard, None).url_unreachable);
        assert!(!app.build_pairing_state(lan, None).url_unreachable);

        app.config.remote_public_url = Some("https://pc.tail1.ts.net".into());
        assert!(!app.build_pairing_state(loopback, None).url_unreachable);
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

        // The fresh token works straight away, with no main-loop tick in
        // between to republish the authorized table — the PWA fetches
        // `/status` the moment pairing returns.
        let status = ureq::get(format!("http://{addr}/status"))
            .header(
                "Authorization",
                format!("Bearer {}", body["token"].as_str().unwrap()),
            )
            .call();
        assert!(status.is_ok(), "fresh token was rejected: {status:?}");

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
        let (_db_file, mut app) = test_app_with_db();
        let device = app
            .db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone A", "hash-a")
            .unwrap();

        assert_eq!(app.refresh_authorized_devices().len(), 1);

        app.db
            .as_ref()
            .unwrap()
            .revoke_remote_device(&device.id)
            .unwrap();
        app.invalidate_authorized_devices();
        assert!(app.refresh_authorized_devices().is_empty());
    }

    #[test]
    fn the_authorized_table_is_cached_until_invalidated() {
        let (_db_file, mut app) = test_app_with_db();
        let db = app.db.as_ref().unwrap();
        db.create_remote_device("Phone A", "hash-a").unwrap();
        assert_eq!(app.refresh_authorized_devices().len(), 1);

        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone B", "hash-b")
            .unwrap();
        assert_eq!(
            app.refresh_authorized_devices().len(),
            1,
            "no reload while fresh"
        );

        app.invalidate_authorized_devices();
        assert_eq!(app.refresh_authorized_devices().len(), 2);
    }

    #[test]
    fn a_failed_reload_keeps_the_last_good_table() {
        let (db_file, mut app) = test_app_with_db();
        app.db
            .as_ref()
            .unwrap()
            .create_remote_device("Phone A", "hash-a")
            .unwrap();
        assert_eq!(app.refresh_authorized_devices().len(), 1);
        app.remote_devices.unpublished = false;

        // Break the table out from under the app's connection so the next
        // reload errors, standing in for a transient SQLITE_BUSY.
        rusqlite::Connection::open(db_file.path())
            .unwrap()
            .execute_batch("DROP TABLE remote_devices")
            .unwrap();
        app.invalidate_authorized_devices();
        app.remote_devices.unpublished = false;

        assert_eq!(app.refresh_authorized_devices().len(), 1);
        assert!(!app.remote_devices.unpublished, "nothing new to publish");
    }
}
