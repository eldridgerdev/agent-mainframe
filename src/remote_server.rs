//! On-demand HTTP/WebSocket server for the Remote Control companion app
//! (see `docs/backlog/remote-control-companion-app-plan.md`, Epic 1).
//!
//! AMF's main loop (`main.rs::run_loop`) and `App` state are synchronous.
//! This server needs an async runtime, so it runs on its own dedicated OS
//! thread with its own tokio runtime — started and stopped only by an
//! explicit user toggle (`App::toggle_remote_server`), never automatically.
//! The server thread never touches `App` state directly: it only ever
//! reports lifecycle events back over a plain `std::sync::mpsc` channel,
//! drained non-blockingly every main-loop tick by
//! `App::poll_remote_server_bg`, mirroring the existing `ipc.rs` /
//! `poll_*_bg` cross-thread pattern used throughout `app/`.
//!
//! A `/health` route proves the thread starts, binds, and stops cleanly
//! without blocking the UI. `/status` (Epic 5) exposes a read-only
//! project/feature status snapshot, published by the main loop over a
//! channel rather than read from `App` by the server thread directly — the
//! server thread stores only the latest snapshot it was handed. `/pair/
//! exchange` (Epic 4) is the same shape turned around: the server thread
//! never validates a pairing code or writes a device itself (the pending
//! code and the SQLite write both live on the main loop, the sole DB
//! writer per the plan's DB-concurrency decision) — it only forwards the
//! HTTP request as a `PairingExchangeRequest` and awaits the outcome on a
//! `oneshot` embedded in the request, so `App::poll_remote_server_bg`
//! (`src/app/remote_server.rs`) can answer it inline with the rest of its
//! per-tick work. Terminal streaming is added as its own epic on top of
//! this.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::ws::WebSocketUpgrade;
use axum::extract::{Extension, Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::remote_terminal::{PaneIo, PaneTarget, TerminalContext};

/// Lifecycle events emitted by the server thread and drained by
/// `App::poll_remote_server_bg`.
#[derive(Debug, Clone)]
pub enum RemoteServerEvent {
    /// The server finished binding and is accepting connections.
    Started { addr: SocketAddr },
    /// The server stopped — cleanly (via `stop()`) or because it failed to
    /// bind / hit a fatal error while serving.
    Stopped { error: Option<String> },
}

/// One feature's status, as exposed over `/status`. A deliberately narrow
/// read model of `project::Feature` — the wire shape is the server's own
/// contract with clients, not a mirror of AMF's internal struct, so
/// internal fields can change without moving this API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct RemoteFeatureStatus {
    pub project_name: String,
    pub feature_name: String,
    /// `"active"` / `"idle"` / `"stopped"` (`project::ProjectStatus`'s
    /// `Display` impl).
    pub status: String,
    pub needs_attention: bool,
    /// Why, when `needs_attention` is true: `AttentionState::label()`
    /// ("Question" / "Completed" / "Waiting") or a pending input's kind
    /// ("Diff review", "Fixes ready", …) — see `app/remote_attention.rs`.
    pub attention_reason: Option<String>,
    /// The agent's own message, when the signal carried one.
    #[serde(default)]
    pub attention_detail: Option<String>,
    /// `project::Feature::id` — what `/actions` and deep links address.
    #[serde(default)]
    pub feature_id: String,
    #[serde(default)]
    pub branch: String,
    /// `AgentKind::slug()` of the feature's default harness.
    #[serde(default)]
    pub agent: String,
    #[serde(default)]
    pub sessions: Vec<RemoteSessionInfo>,
    /// The feature's AI summary (`Feature::summary`), when it has one.
    #[serde(default)]
    pub summary: Option<String>,
    #[serde(default)]
    pub nickname: Option<String>,
}

/// One of a feature's sessions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteSessionInfo {
    pub id: String,
    pub label: String,
    /// `SessionKind` in lowercase (`claude`, `terminal`, `todos`, …).
    pub kind: String,
    /// Whether `/sessions/{id}/terminal` can show it right now: a tmux
    /// window of a running feature.
    pub live: bool,
}

/// A project, for the phone's create-feature form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteProjectInfo {
    pub name: String,
    pub preferred_agent: String,
}

/// The full read-only status feed (Phase 1). Rebuilt by `App` and pushed to
/// the server thread on every main-loop tick while the server is running —
/// see `App::poll_remote_server_bg`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RemoteStatusSnapshot {
    pub generated_at: String,
    pub features: Vec<RemoteFeatureStatus>,
    #[serde(default)]
    pub projects: Vec<RemoteProjectInfo>,
    /// Session id → the pane it lives in, for the live sessions in
    /// `features`. Server-side only: which tmux window backs a session is
    /// not the phone's business, only whether it can be opened.
    #[serde(skip)]
    pub pane_targets: HashMap<String, PaneTarget>,
    /// Feature id → its checkout, for `/features/{id}/diff`. Server-side
    /// only, like `pane_targets`.
    #[serde(skip)]
    pub workdirs: HashMap<String, std::path::PathBuf>,
}

type SharedStatus = Arc<Mutex<RemoteStatusSnapshot>>;

/// One device authorized to make authenticated requests, keyed by its
/// token's hash in the table `App` publishes every tick (see
/// `App::refresh_authorized_devices`). Revoked devices are simply absent, so
/// a revoke takes effect on the next tick without the server thread ever
/// touching the database itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizedDevice {
    pub device_id: String,
    pub device_name: String,
}

type SharedAuthTable = Arc<Mutex<HashMap<String, AuthorizedDevice>>>;

/// A `POST /pair/exchange` request, forwarded from the HTTP handler to the
/// main loop. `reply` is a one-shot back-channel — created fresh per
/// request, not a persistent pipe — so the handler can simply `.await` it
/// after sending, with a timeout in case the main loop is unreachable.
pub struct PairingExchangeRequest {
    pub code: String,
    pub device_name: String,
    pub reply: tokio::sync::oneshot::Sender<PairingExchangeOutcome>,
}

/// A Web Push request from an authenticated device, forwarded to the main
/// loop the same way as pairing: subscriptions are a database write, and
/// sending a test push needs the VAPID key and subscription list the main
/// loop owns. `reply` carries `Err(reason)` for anything the phone should
/// show the user.
pub struct PushRequest {
    pub device_id: String,
    pub kind: PushRequestKind,
    pub reply: tokio::sync::oneshot::Sender<Result<(), String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushRequestKind {
    /// Store (or refresh) this browser's subscription for the device.
    /// Keys are already validated by the handler.
    Subscribe {
        endpoint: String,
        p256dh: String,
        auth: String,
    },
    /// Send a test notification to every subscription the device holds.
    Test,
}

/// Something the phone asked AMF to do (`POST /actions`), forwarded to the
/// main loop, which owns every piece of state these touch.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum RemoteAction {
    StartFeature {
        feature_id: String,
    },
    StopFeature {
        feature_id: String,
    },
    /// `kind`: `terminal`, `agent` (the feature's own harness), or a
    /// harness slug (`claude`, `codex`, `opencode`, `pi`).
    AddSession {
        feature_id: String,
        kind: String,
    },
    CreateFeature {
        project_name: String,
        branch: String,
        /// A harness slug; the project's preferred agent when absent.
        #[serde(default)]
        agent: Option<String>,
        /// `vibeless` / `vibe` / `supervibe`; the default mode when absent.
        #[serde(default)]
        mode: Option<String>,
        /// `None` lets AMF decide, as the automation API does.
        #[serde(default)]
        use_worktree: Option<bool>,
        #[serde(default)]
        review: bool,
    },
    /// The phone opened a session — clears attention the way opening it at
    /// the desk would, for harnesses that can't report resuming.
    SessionOpened {
        session_id: String,
    },
    /// Close one session's window and forget it.
    RemoveSession {
        session_id: String,
    },
    /// Delete a feature: kill its tmux session and remove its worktree.
    DeleteFeature {
        feature_id: String,
    },
    /// The TODO lists a feature can see: its worktree's, its project's,
    /// and the global one.
    ListTodos {
        feature_id: String,
    },
    /// `scope`: `worktree`, `project` or `global`.
    AddTodo {
        feature_id: String,
        scope: String,
        title: String,
    },
    /// `status`: `not_started`, `in_progress` or `completed`.
    SetTodoStatus {
        todo_id: String,
        status: String,
    },
    DeleteTodo {
        todo_id: String,
    },
    /// Start an agent on a TODO in `feature_id` (or return the session
    /// already working on it). Replies `{session_id, prompt}`: the phone
    /// opens the session with the prompt pre-filled, unsent — the same
    /// review-before-send the desk's composer gives.
    StartTodo {
        feature_id: String,
        todo_id: String,
    },
    /// The prompt library as the desk's picker shows it for this feature.
    ListPrompts {
        feature_id: String,
    },
    /// Fill a template's `{{slots}}` exactly as the desk does.
    RenderPrompt {
        body: String,
        #[serde(default)]
        values: HashMap<String, String>,
    },
}

pub struct RemoteCommand {
    pub device_id: String,
    pub action: RemoteAction,
    /// `Ok(result)` — a message to show, or data for the phone to render —
    /// or `Err(reason)`.
    pub reply: tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
}

/// Actions may create a worktree or run hooks on the main loop, so they
/// get longer than pairing to answer.
const ACTION_REPLY_TIMEOUT: Duration = Duration::from_secs(60);

/// The result of validating and (on success) minting a device for a
/// pairing exchange. Every variant here maps to a distinct HTTP status in
/// `outcome_to_response` — none of them leak *why* a code is invalid vs.
/// merely unknown, so a guesser learns nothing beyond "no" and, eventually,
/// "locked out".
#[derive(Debug, Clone)]
pub enum PairingExchangeOutcome {
    Paired {
        device_id: String,
        device_name: String,
        token: String,
    },
    InvalidCode,
    Expired,
    LockedOut,
    /// The device couldn't be persisted (e.g. no database configured) —
    /// distinct from a bad code so a real phone doesn't retry a correct
    /// code expecting a different result.
    InternalError,
}

const PAIRING_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Owns the remote-control server's thread and lets the main loop request
/// shutdown without blocking on the async runtime tearing down.
pub struct RemoteServerHandle {
    pub rx: Receiver<RemoteServerEvent>,
    status_tx: tokio::sync::mpsc::UnboundedSender<RemoteStatusSnapshot>,
    /// Pairing requests, drained non-blockingly by
    /// `App::poll_remote_server_bg` via `try_recv_pairing_request`. The
    /// matching sender lives inside the server thread's axum state — see
    /// `run_server`.
    pairing_rx: tokio::sync::mpsc::UnboundedReceiver<PairingExchangeRequest>,
    /// The current authorized-device table, published every tick — see
    /// `AuthorizedDevice`.
    auth_tx: tokio::sync::mpsc::UnboundedSender<HashMap<String, AuthorizedDevice>>,
    /// One device id per successful authenticated request, for
    /// `App::drain_device_seen_events` to record a last-seen timestamp for.
    device_seen_rx: tokio::sync::mpsc::UnboundedReceiver<(String, String)>,
    /// Web Push requests, drained like `pairing_rx`.
    push_rx: tokio::sync::mpsc::UnboundedReceiver<PushRequest>,
    /// `/actions` requests, drained like `pairing_rx`.
    command_rx: tokio::sync::mpsc::UnboundedReceiver<RemoteCommand>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl RemoteServerHandle {
    /// Signal the server to shut down. Non-blocking — does not join the
    /// server thread, so the caller (the main loop) is never stalled
    /// waiting for the async runtime to tear down. The thread's own
    /// `Stopped` event, drained on a later tick, confirms completion.
    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }

    /// Push a fresh status snapshot for `/status` to serve. Best-effort: if
    /// the server thread has already exited, the send is silently dropped
    /// rather than treated as an error — the next `poll_remote_server_bg`
    /// tick will see the `Stopped` event and clear the handle anyway.
    pub fn publish_status(&self, snapshot: RemoteStatusSnapshot) {
        let _ = self.status_tx.send(snapshot);
    }

    /// Drain one pending pairing exchange request, if any. Non-blocking —
    /// mirrors `handle.rx.try_iter()` for lifecycle events, so
    /// `App::poll_remote_server_bg` can drain every request that arrived
    /// since the last tick without ever waiting on the server thread.
    pub fn try_recv_pairing_request(&mut self) -> Option<PairingExchangeRequest> {
        self.pairing_rx.try_recv().ok()
    }

    /// Push a fresh authorized-device table for `/status` (and any future
    /// authenticated route) to check bearer tokens against. Same
    /// best-effort semantics as `publish_status`.
    pub fn publish_authorized_devices(&self, table: HashMap<String, AuthorizedDevice>) {
        let _ = self.auth_tx.send(table);
    }

    /// Drain one device-seen notification, if any — mirrors
    /// `try_recv_pairing_request`.
    pub fn try_recv_device_seen(&mut self) -> Option<(String, String)> {
        self.device_seen_rx.try_recv().ok()
    }

    /// Drain one Web Push request, if any — mirrors
    /// `try_recv_pairing_request`.
    pub fn try_recv_push_request(&mut self) -> Option<PushRequest> {
        self.push_rx.try_recv().ok()
    }

    /// Drain one `/actions` request, if any.
    pub fn try_recv_command(&mut self) -> Option<RemoteCommand> {
        self.command_rx.try_recv().ok()
    }
}

impl Drop for RemoteServerHandle {
    fn drop(&mut self) {
        self.stop();
        // Best-effort join so tearing down mid-shutdown (e.g. AMF exiting)
        // doesn't leak the OS thread. Graceful shutdown is near-instant with
        // no open connections, so this should never meaningfully block exit.
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Start the remote-control server on a dedicated thread with its own tokio
/// runtime. Returns immediately — binding happens on the server thread, and
/// success or failure is reported via the returned handle's event channel
/// rather than this call's return value, so the main loop is never blocked
/// waiting on the OS to bind a socket.
///
pub fn start(bind_addr: SocketAddr, config: ServerConfig) -> RemoteServerHandle {
    let (event_tx, event_rx) = channel::<RemoteServerEvent>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (status_tx, status_rx) = tokio::sync::mpsc::unbounded_channel::<RemoteStatusSnapshot>();
    let (pairing_tx, pairing_rx) = tokio::sync::mpsc::unbounded_channel::<PairingExchangeRequest>();
    let (auth_tx, auth_rx) =
        tokio::sync::mpsc::unbounded_channel::<HashMap<String, AuthorizedDevice>>();
    let (device_seen_tx, device_seen_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    let (push_tx, push_rx) = tokio::sync::mpsc::unbounded_channel::<PushRequest>();
    let (command_tx, command_rx) = tokio::sync::mpsc::unbounded_channel::<RemoteCommand>();

    let join = std::thread::Builder::new()
        .name("amf-remote-server".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = event_tx.send(RemoteServerEvent::Stopped {
                        error: Some(format!("Failed to start async runtime: {e}")),
                    });
                    return;
                }
            };

            let state = ServerState {
                status: Arc::new(Mutex::new(RemoteStatusSnapshot::default())),
                pairing_tx,
                auth: Arc::new(Mutex::new(HashMap::new())),
                device_seen_tx,
                push_tx,
                command_tx,
                vapid_public_key: config.vapid_public_key.map(Arc::from),
                pane_io: config.pane_io,
            };
            runtime.block_on(run_server(
                bind_addr,
                event_tx,
                shutdown_rx,
                status_rx,
                auth_rx,
                state,
            ));
        })
        .expect("failed to spawn amf-remote-server thread");

    RemoteServerHandle {
        rx: event_rx,
        status_tx,
        pairing_rx,
        auth_tx,
        device_seen_rx,
        push_rx,
        command_rx,
        shutdown: Some(shutdown_tx),
        join: Some(join),
    }
}

/// What a server is started with.
pub struct ServerConfig {
    /// What `GET /push/key` hands the PWA to subscribe with; `None` (no
    /// database to keep a key in) turns Web Push off.
    pub vapid_public_key: Option<String>,
    /// How terminal sockets reach tmux.
    pub pane_io: Arc<dyn PaneIo>,
}

/// State shared across axum handlers. The `mpsc` senders/handles here are
/// all `Clone`, so deriving `Clone` on this struct is enough for axum's
/// `with_state` — no extra `Arc` wrapper needed beyond the ones `status`
/// and `auth` already carry.
#[derive(Clone)]
struct ServerState {
    status: SharedStatus,
    pairing_tx: tokio::sync::mpsc::UnboundedSender<PairingExchangeRequest>,
    /// The authorized-device table `require_device_auth` checks bearer
    /// tokens against — kept current by the relay task, same shape as
    /// `status`.
    auth: SharedAuthTable,
    /// Reports the device id behind a successful auth check, so `App` can
    /// record a last-seen timestamp — the server thread never writes the
    /// database itself.
    device_seen_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
    push_tx: tokio::sync::mpsc::UnboundedSender<PushRequest>,
    command_tx: tokio::sync::mpsc::UnboundedSender<RemoteCommand>,
    vapid_public_key: Option<Arc<str>>,
    pane_io: Arc<dyn PaneIo>,
}

/// What a terminal socket sees of the server: the auth table and the
/// session → pane table from the latest snapshot.
struct SocketContext {
    auth: SharedAuthTable,
    status: SharedStatus,
    device_seen_tx: tokio::sync::mpsc::UnboundedSender<(String, String)>,
}

impl TerminalContext for SocketContext {
    fn authorize(&self, token: &str) -> Option<String> {
        let device = self.auth.lock().unwrap().get(&hash_token(token)).cloned()?;
        let _ = self
            .device_seen_tx
            .send((device.device_id.clone(), device.device_name));
        Some(device.device_id)
    }

    fn still_authorized(&self, token: &str) -> bool {
        self.auth.lock().unwrap().contains_key(&hash_token(token))
    }

    fn resolve(&self, session_id: &str) -> Option<PaneTarget> {
        self.status
            .lock()
            .unwrap()
            .pane_targets
            .get(session_id)
            .cloned()
    }
}

async fn run_server(
    bind_addr: SocketAddr,
    event_tx: Sender<RemoteServerEvent>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    mut status_rx: tokio::sync::mpsc::UnboundedReceiver<RemoteStatusSnapshot>,
    mut auth_rx: tokio::sync::mpsc::UnboundedReceiver<HashMap<String, AuthorizedDevice>>,
    state: ServerState,
) {
    // Relay task: the only writer to `status`, so the lock is never held
    // across an `.await`. `App` pushes a new snapshot on every main-loop
    // tick while the server is running; this just keeps the latest one
    // ready for `/status` to serve without touching `App` itself.
    let status_for_relay = state.status.clone();
    tokio::spawn(async move {
        while let Some(snapshot) = status_rx.recv().await {
            *status_for_relay.lock().unwrap() = snapshot;
        }
    });

    // Same shape, for the authorized-device table.
    let auth_for_relay = state.auth.clone();
    tokio::spawn(async move {
        while let Some(table) = auth_rx.recv().await {
            *auth_for_relay.lock().unwrap() = table;
        }
    });

    let device_auth = || axum::middleware::from_fn_with_state(state.clone(), require_device_auth);

    let router = web_shell_routes()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route(
            "/status",
            axum::routing::get(status_handler).route_layer(device_auth()),
        )
        .route(
            "/push/key",
            axum::routing::get(push_key_handler).route_layer(device_auth()),
        )
        .route(
            "/push/subscribe",
            axum::routing::post(push_subscribe_handler).route_layer(device_auth()),
        )
        .route(
            "/push/test",
            axum::routing::post(push_test_handler).route_layer(device_auth()),
        )
        .route(
            "/features/{feature_id}/diff",
            axum::routing::get(diff_handler).route_layer(device_auth()),
        )
        .route(
            "/actions",
            axum::routing::post(action_handler).route_layer(device_auth()),
        )
        // Authenticates inside the socket (first message), not by header:
        // see `remote_terminal::ClientMessage::Auth`.
        .route(
            "/sessions/{session_id}/terminal",
            axum::routing::get(terminal_handler),
        )
        .route(
            "/pair/exchange",
            axum::routing::post(pairing_exchange_handler),
        )
        .with_state(state);

    let listener = match tokio::net::TcpListener::bind(bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            let _ = event_tx.send(RemoteServerEvent::Stopped {
                error: Some(format!("Failed to bind {bind_addr}: {e}")),
            });
            return;
        }
    };

    let actual_addr = listener.local_addr().unwrap_or(bind_addr);
    let _ = event_tx.send(RemoteServerEvent::Started { addr: actual_addr });

    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = shutdown_rx.await;
        })
        .await;

    let _ = event_tx.send(RemoteServerEvent::Stopped {
        error: result.err().map(|e| e.to_string()),
    });
}

/// One file of the PWA shell (`src/remote_web/`), embedded at compile time
/// so the server needs nothing on disk and the client always matches the
/// server it came from.
struct WebAsset {
    path: &'static str,
    content_type: &'static str,
    body: &'static [u8],
}

const WEB_ASSETS: &[WebAsset] = &[
    WebAsset {
        path: "/",
        content_type: "text/html; charset=utf-8",
        body: include_bytes!("remote_web/index.html"),
    },
    WebAsset {
        path: "/app.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("remote_web/app.js"),
    },
    WebAsset {
        path: "/app.css",
        content_type: "text/css; charset=utf-8",
        body: include_bytes!("remote_web/app.css"),
    },
    WebAsset {
        path: "/sw.js",
        content_type: "text/javascript; charset=utf-8",
        body: include_bytes!("remote_web/sw.js"),
    },
    WebAsset {
        path: "/manifest.webmanifest",
        content_type: "application/manifest+json",
        body: include_bytes!("remote_web/manifest.webmanifest"),
    },
    WebAsset {
        path: "/icon-192.png",
        content_type: "image/png",
        body: include_bytes!("remote_web/icon-192.png"),
    },
    WebAsset {
        path: "/icon-512.png",
        content_type: "image/png",
        body: include_bytes!("remote_web/icon-512.png"),
    },
];

/// Unauthenticated routes serving the PWA shell. The shell holds no data —
/// everything it shows comes from the authenticated API — so it is safe to
/// hand to anyone who can reach the port, which is what lets a QR scan open
/// it before the phone has a token. `no-cache` (revalidate, not "don't
/// store") keeps an upgraded AMF from serving a stale client.
fn web_shell_routes() -> axum::Router<ServerState> {
    WEB_ASSETS
        .iter()
        .fold(axum::Router::new(), |router, asset| {
            let (content_type, body) = (asset.content_type, asset.body);
            router.route(
                asset.path,
                axum::routing::get(move || async move {
                    (
                        [
                            (axum::http::header::CONTENT_TYPE, content_type),
                            (axum::http::header::CACHE_CONTROL, "no-cache"),
                        ],
                        body,
                    )
                }),
            )
        })
}

async fn status_handler(State(state): State<ServerState>) -> Json<RemoteStatusSnapshot> {
    Json(state.status.lock().unwrap().clone())
}

/// Gate an authenticated route behind `Authorization: Bearer <token>`,
/// checked against the table `App` publishes every tick (see
/// `AuthorizedDevice`). The server thread never touches the database to
/// answer this — a missing/unknown/revoked token is all indistinguishable
/// here, which is the point (see `outcome_to_response`'s equivalent note on
/// pairing codes). On success, reports the device id back to the main loop
/// so it can record a last-seen timestamp — the server thread itself never
/// writes the database.
async fn require_device_auth(
    State(state): State<ServerState>,
    mut request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let Some(token) = token else {
        return unauthorized_response();
    };

    let device = {
        let table = state.auth.lock().unwrap();
        table.get(&hash_token(token)).cloned()
    };
    let Some(device) = device else {
        return unauthorized_response();
    };

    let _ = state
        .device_seen_tx
        .send((device.device_id.clone(), device.device_name.clone()));
    // Handlers that act *as* the device (push subscribe/test) read it back
    // with `Extension<AuthorizedDevice>`.
    request.extensions_mut().insert(device);
    next.run(request).await
}

fn unauthorized_response() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(serde_json::json!({"error": "unauthorized"})),
    )
        .into_response()
}

#[derive(Debug, Deserialize)]
struct PairingExchangeBody {
    code: String,
    #[serde(default)]
    device_name: String,
}

/// Forward a `POST /pair/exchange` request to the main loop and relay its
/// answer back as the HTTP response. The server thread never itself decides
/// whether `code` is valid — see the module doc comment.
async fn pairing_exchange_handler(
    State(state): State<ServerState>,
    Json(body): Json<PairingExchangeBody>,
) -> Response {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let request = PairingExchangeRequest {
        code: body.code,
        device_name: body.device_name,
        reply: reply_tx,
    };

    if state.pairing_tx.send(request).is_err() {
        // The main loop's receiver is gone — practically unreachable while
        // this server thread is itself still running, since both live in
        // the same `RemoteServerHandle`, but handled rather than panicking.
        return unavailable_response();
    }

    match tokio::time::timeout(PAIRING_REPLY_TIMEOUT, reply_rx).await {
        Ok(Ok(outcome)) => {
            // Authorize the new token before answering. Otherwise the
            // client's first `/status` can land before `App` republishes
            // the table on its next tick and get a 401 for a token it was
            // handed a moment ago. That republish (built from the database)
            // will contain this same entry, so this only closes the gap.
            if let PairingExchangeOutcome::Paired {
                device_id,
                device_name,
                token,
            } = &outcome
            {
                state.auth.lock().unwrap().insert(
                    hash_token(token),
                    AuthorizedDevice {
                        device_id: device_id.clone(),
                        device_name: device_name.clone(),
                    },
                );
            }
            outcome_to_response(outcome)
        }
        // Either the oneshot sender was dropped without a reply (shouldn't
        // happen — `App` always replies) or the main loop hasn't polled
        // this request within the timeout (e.g. AMF is unresponsive).
        // Either way, the honest answer is "try again", not a 4xx that
        // implies the code itself was wrong.
        _ => unavailable_response(),
    }
}

async fn terminal_handler(
    State(state): State<ServerState>,
    Path(session_id): Path<String>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let context = Arc::new(SocketContext {
        auth: state.auth.clone(),
        status: state.status.clone(),
        device_seen_tx: state.device_seen_tx.clone(),
    });
    let io = state.pane_io.clone();
    upgrade.on_upgrade(move |socket| {
        crate::remote_terminal::run_terminal_socket(socket, session_id, context, io)
    })
}

/// Per-file and total caps on the patch text sent to a phone: a generated
/// lockfile or a vendored bundle shouldn't cost megabytes to open the list.
const MAX_FILE_PATCH_BYTES: usize = 200 * 1024;
const MAX_TOTAL_PATCH_BYTES: usize = 2 * 1024 * 1024;

/// A feature's changes against its base branch — the same snapshot the
/// desktop diff viewer loads (`diff::load_snapshot`), read-only.
async fn diff_handler(
    State(state): State<ServerState>,
    Path(feature_id): Path<String>,
) -> Response {
    let workdir = state
        .status
        .lock()
        .unwrap()
        .workdirs
        .get(&feature_id)
        .cloned();
    let Some(workdir) = workdir else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "That feature no longer exists."})),
        )
            .into_response();
    };
    let loaded =
        tokio::task::spawn_blocking(move || crate::diff::load_snapshot(&workdir, None, false))
            .await;
    let snapshot = match loaded {
        Ok(Ok(snapshot)) => snapshot,
        Ok(Err(e)) => return push_error_response(&format!("Couldn't load the diff: {e}")),
        Err(_) => return unavailable_response(),
    };
    Json(diff_json(&snapshot)).into_response()
}

fn diff_json(snapshot: &crate::diff::DiffSnapshot) -> serde_json::Value {
    let mut budget = MAX_TOTAL_PATCH_BYTES;
    let files: Vec<serde_json::Value> = snapshot
        .files
        .iter()
        .map(|file| {
            let fits = file.patch.len() <= MAX_FILE_PATCH_BYTES && file.patch.len() <= budget;
            if fits {
                budget -= file.patch.len();
            }
            serde_json::json!({
                "path": file.path,
                "old_path": file.old_path,
                "status": format!("{:?}", file.status).to_lowercase(),
                "additions": file.additions,
                "deletions": file.deletions,
                "is_binary": file.is_binary,
                "patch": if fits { Some(file.patch.as_str()) } else { None },
            })
        })
        .collect();
    serde_json::json!({
        "branch": snapshot.branch,
        "base_ref": snapshot.base_ref,
        "total_additions": snapshot.total_additions,
        "total_deletions": snapshot.total_deletions,
        "files": files,
    })
}

async fn action_handler(
    State(state): State<ServerState>,
    Extension(device): Extension<AuthorizedDevice>,
    Json(action): Json<RemoteAction>,
) -> Response {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let command = RemoteCommand {
        device_id: device.device_id,
        action,
        reply: reply_tx,
    };
    if state.command_tx.send(command).is_err() {
        return unavailable_response();
    }
    match tokio::time::timeout(ACTION_REPLY_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(message))) => Json(serde_json::json!({"message": message})).into_response(),
        Ok(Ok(Err(reason))) => push_error_response(&reason),
        _ => unavailable_response(),
    }
}

async fn push_key_handler(State(state): State<ServerState>) -> Response {
    match &state.vapid_public_key {
        Some(key) => Json(serde_json::json!({"public_key": key.as_ref()})).into_response(),
        None => push_error_response("Push needs AMF's database, which isn't available"),
    }
}

/// The subset of `PushSubscription.toJSON()` a push needs.
#[derive(Debug, Deserialize)]
struct PushSubscribeBody {
    endpoint: String,
    keys: PushSubscribeKeys,
}

#[derive(Debug, Deserialize)]
struct PushSubscribeKeys {
    p256dh: String,
    auth: String,
}

async fn push_subscribe_handler(
    State(state): State<ServerState>,
    Extension(device): Extension<AuthorizedDevice>,
    Json(body): Json<PushSubscribeBody>,
) -> Response {
    // Only https endpoints: this is a URL AMF will POST to later, so it
    // must not be a way to aim AMF's requests at the local network.
    if !body.endpoint.starts_with("https://") {
        return bad_request_response("push endpoint must be https");
    }
    if let Err(e) =
        crate::remote_push::validate_subscription_keys(&body.keys.p256dh, &body.keys.auth)
    {
        return bad_request_response(&e.to_string());
    }
    forward_push_request(
        &state,
        device,
        PushRequestKind::Subscribe {
            endpoint: body.endpoint,
            p256dh: body.keys.p256dh,
            auth: body.keys.auth,
        },
    )
    .await
}

async fn push_test_handler(
    State(state): State<ServerState>,
    Extension(device): Extension<AuthorizedDevice>,
) -> Response {
    forward_push_request(&state, device, PushRequestKind::Test).await
}

async fn forward_push_request(
    state: &ServerState,
    device: AuthorizedDevice,
    kind: PushRequestKind,
) -> Response {
    let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
    let request = PushRequest {
        device_id: device.device_id,
        kind,
        reply: reply_tx,
    };
    if state.push_tx.send(request).is_err() {
        return unavailable_response();
    }
    match tokio::time::timeout(PAIRING_REPLY_TIMEOUT, reply_rx).await {
        Ok(Ok(Ok(()))) => StatusCode::NO_CONTENT.into_response(),
        Ok(Ok(Err(reason))) => push_error_response(&reason),
        _ => unavailable_response(),
    }
}

/// A 409 carrying a reason the phone shows as-is.
fn push_error_response(reason: &str) -> Response {
    (
        StatusCode::CONFLICT,
        Json(serde_json::json!({"error": reason})),
    )
        .into_response()
}

fn bad_request_response(reason: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({"error": reason})),
    )
        .into_response()
}

fn unavailable_response() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "unavailable"})),
    )
        .into_response()
}

fn outcome_to_response(outcome: PairingExchangeOutcome) -> Response {
    match outcome {
        PairingExchangeOutcome::Paired {
            device_id, token, ..
        } => (
            StatusCode::OK,
            Json(serde_json::json!({"device_id": device_id, "token": token})),
        )
            .into_response(),
        PairingExchangeOutcome::InvalidCode => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid_code"})),
        )
            .into_response(),
        PairingExchangeOutcome::Expired => (
            StatusCode::GONE,
            Json(serde_json::json!({"error": "expired"})),
        )
            .into_response(),
        PairingExchangeOutcome::LockedOut => (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "locked_out"})),
        )
            .into_response(),
        PairingExchangeOutcome::InternalError => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "internal_error"})),
        )
            .into_response(),
    }
}

/// Mint a new bearer token for a freshly paired device: 256 bits of
/// randomness as lowercase hex, built from two v4 UUIDs (already a
/// dependency, `getrandom`-backed) rather than adding a `rand` crate
/// solely for this.
pub fn generate_device_token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// Hash a token for storage — `remote_devices` (`src/db/remote_devices.rs`)
/// only ever holds this, never the plaintext.
pub fn hash_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(token.as_bytes());
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Generate a one-time pairing code: 6 decimal digits, easy to read off the
/// screen and type by hand if the QR can't be scanned. Short-lived and
/// rate-limited (`App::process_pairing_exchange`), so the deliberately
/// small keyspace is an acceptable trade for readability.
pub fn generate_pairing_code() -> String {
    let n = (uuid::Uuid::new_v4().as_u128() % 1_000_000) as u32;
    format!("{n:06}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn any_local_addr() -> SocketAddr {
        "127.0.0.1:0".parse().unwrap()
    }

    #[test]
    fn starts_and_stops_cleanly_without_a_network_client() {
        let mut handle = start(any_local_addr(), test_config(None));

        let started = handle
            .rx
            .recv_timeout(Duration::from_secs(2))
            .expect("server did not report Started");
        match started {
            RemoteServerEvent::Started { addr } => {
                assert_ne!(addr.port(), 0, "OS should have assigned a real port");
            }
            RemoteServerEvent::Stopped { error } => {
                panic!("server stopped before starting: {error:?}");
            }
        }

        handle.stop();

        let stopped = handle
            .rx
            .recv_timeout(Duration::from_secs(2))
            .expect("server did not report Stopped after stop()");
        match stopped {
            RemoteServerEvent::Stopped { error } => {
                assert!(error.is_none(), "graceful stop should not report an error");
            }
            RemoteServerEvent::Started { .. } => {
                panic!("unexpected second Started event");
            }
        }
    }

    #[test]
    fn drop_without_explicit_stop_shuts_down_and_joins() {
        let handle = start(any_local_addr(), test_config(None));
        // Wait for it to actually be listening before dropping, so the
        // drop path exercises real shutdown rather than a not-yet-bound
        // runtime.
        handle
            .rx
            .recv_timeout(Duration::from_secs(2))
            .expect("server did not report Started");
        drop(handle); // Drop::drop signals shutdown and joins the thread.
        // Reaching this line without hanging is the assertion: Drop must
        // not block indefinitely waiting on the async runtime.
    }

    #[test]
    fn two_servers_can_run_on_different_ports_concurrently() {
        let mut a = start(any_local_addr(), test_config(None));
        let mut b = start(any_local_addr(), test_config(None));

        let a_addr = match a.rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            RemoteServerEvent::Started { addr } => addr,
            other => panic!("unexpected event: {other:?}"),
        };
        let b_addr = match b.rx.recv_timeout(Duration::from_secs(2)).unwrap() {
            RemoteServerEvent::Started { addr } => addr,
            other => panic!("unexpected event: {other:?}"),
        };
        assert_ne!(a_addr.port(), b_addr.port());

        a.stop();
        b.stop();
        let _ = a.rx.recv_timeout(Duration::from_secs(2));
        let _ = b.rx.recv_timeout(Duration::from_secs(2));
    }

    fn wait_for_started(handle: &RemoteServerHandle) -> SocketAddr {
        match handle
            .rx
            .recv_timeout(Duration::from_secs(2))
            .expect("server did not report Started")
        {
            RemoteServerEvent::Started { addr } => addr,
            RemoteServerEvent::Stopped { error } => {
                panic!("server stopped before starting: {error:?}")
            }
        }
    }

    #[test]
    fn push_routes_require_auth_and_forward_as_the_device() {
        let mut handle = start(
            any_local_addr(),
            test_config(Some("vapid-public".to_string())),
        );
        let addr = wait_for_started(&handle);
        assert!(matches!(
            ureq::get(format!("http://{addr}/push/key")).call(),
            Err(ureq::Error::StatusCode(401))
        ));
        let token = publish_one_authorized_device(&handle, addr);
        let bearer = format!("Bearer {token}");

        let mut key = ureq::get(format!("http://{addr}/push/key"))
            .header("Authorization", &bearer)
            .call()
            .unwrap();
        let key: serde_json::Value =
            serde_json::from_str(&key.body_mut().read_to_string().unwrap()).unwrap();
        assert_eq!(key["public_key"], "vapid-public");

        // Refused before reaching the main loop: not https, bad keys.
        let refused = ureq::post(format!("http://{addr}/push/subscribe"))
            .header("Authorization", &bearer)
            .content_type("application/json")
            .send(
                serde_json::json!({
                    "endpoint": "http://192.168.1.1/",
                    "keys": {"p256dh": "x", "auth": "y"},
                })
                .to_string(),
            );
        assert!(matches!(refused, Err(ureq::Error::StatusCode(400))));

        // A test push is forwarded tagged with the authenticated device, and
        // the main loop's refusal comes back as a 409 with its reason.
        let url = format!("http://{addr}/push/test");
        let client = std::thread::spawn(move || {
            ureq::post(url)
                .header("Authorization", bearer)
                .config()
                .http_status_as_error(false)
                .build()
                .send_empty()
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let request = loop {
            if let Some(request) = handle.try_recv_push_request() {
                break request;
            }
            assert!(std::time::Instant::now() < deadline, "no push request");
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(request.device_id, "dev-1");
        assert_eq!(request.kind, PushRequestKind::Test);
        request
            .reply
            .send(Err("not subscribed".to_string()))
            .unwrap();
        let mut response = client.join().unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert!(
            response
                .body_mut()
                .read_to_string()
                .unwrap()
                .contains("not subscribed")
        );
        handle.stop();
    }

    struct NoPanes;

    impl PaneIo for NoPanes {
        fn capture(&self, _: &PaneTarget) -> anyhow::Result<crate::remote_terminal::PaneFrame> {
            anyhow::bail!("no panes in tests")
        }
        fn send(
            &self,
            _: &PaneTarget,
            _: &crate::remote_terminal::PaneInput,
        ) -> anyhow::Result<()> {
            anyhow::bail!("no panes in tests")
        }
        fn history(&self, _: &PaneTarget, _: u32) -> anyhow::Result<String> {
            anyhow::bail!("no panes in tests")
        }
    }

    /// A pane whose screen is whatever literal text has been typed into it.
    #[derive(Default)]
    struct FakePanes {
        screen: Mutex<String>,
        received: Mutex<Vec<crate::remote_terminal::PaneInput>>,
    }

    impl PaneIo for FakePanes {
        fn capture(&self, _: &PaneTarget) -> anyhow::Result<crate::remote_terminal::PaneFrame> {
            Ok(crate::remote_terminal::PaneFrame {
                cols: 80,
                rows: 24,
                cursor_x: 0,
                cursor_y: 0,
                cursor_visible: true,
                ansi: self.screen.lock().unwrap().clone(),
            })
        }
        fn send(
            &self,
            _: &PaneTarget,
            input: &crate::remote_terminal::PaneInput,
        ) -> anyhow::Result<()> {
            if let crate::remote_terminal::PaneInput::Literal(text) = input {
                self.screen.lock().unwrap().push_str(text);
            }
            self.received.lock().unwrap().push(input.clone());
            Ok(())
        }
        fn history(&self, _: &PaneTarget, lines: u32) -> anyhow::Result<String> {
            Ok(format!("history({lines})\n{}", self.screen.lock().unwrap()))
        }
    }

    type TestSocket =
        tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

    fn open_terminal(addr: SocketAddr, session_id: &str, token: &str) -> TestSocket {
        let (mut socket, _) =
            tungstenite::connect(format!("ws://{addr}/sessions/{session_id}/terminal")).unwrap();
        socket
            .send(tungstenite::Message::text(
                serde_json::json!({"type": "auth", "token": token}).to_string(),
            ))
            .unwrap();
        socket
    }

    /// Read messages until one satisfies `want`, failing after a few seconds.
    fn read_until(
        socket: &mut TestSocket,
        want: impl Fn(&serde_json::Value) -> bool,
    ) -> serde_json::Value {
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
        }
        loop {
            let message = socket
                .read()
                .expect("socket closed before the expected message");
            if let tungstenite::Message::Text(text) = message {
                let value: serde_json::Value = serde_json::from_str(&text).unwrap();
                if want(&value) {
                    return value;
                }
            }
        }
    }

    #[test]
    fn terminal_socket_ends_when_its_session_goes_away() {
        let handle = start(
            any_local_addr(),
            ServerConfig {
                vapid_public_key: None,
                pane_io: Arc::new(FakePanes::default()),
            },
        );
        let addr = wait_for_started(&handle);
        let token = publish_one_authorized_device(&handle, addr);
        let mut targets = HashMap::new();
        targets.insert(
            "session-1".to_string(),
            PaneTarget {
                session: "amf-x".into(),
                window: "claude".into(),
            },
        );
        handle.publish_status(RemoteStatusSnapshot {
            pane_targets: targets,
            ..Default::default()
        });
        let mut socket = open_terminal(addr, "session-1", &token);
        read_until(&mut socket, |m| m["type"] == "frame");

        handle.publish_status(RemoteStatusSnapshot::default());

        let gone = read_until(&mut socket, |m| m["type"] == "gone");
        assert!(gone["message"].as_str().unwrap().contains("isn't running"));
    }

    #[test]
    fn terminal_socket_streams_frames_and_forwards_input() {
        let panes = Arc::new(FakePanes::default());
        *panes.screen.lock().unwrap() = "$ ".to_string();
        let handle = start(
            any_local_addr(),
            ServerConfig {
                vapid_public_key: None,
                pane_io: panes.clone(),
            },
        );
        let addr = wait_for_started(&handle);
        let token = publish_one_authorized_device(&handle, addr);
        let mut targets = HashMap::new();
        targets.insert(
            "session-1".to_string(),
            PaneTarget {
                session: "amf-x".into(),
                window: "claude".into(),
            },
        );
        handle.publish_status(RemoteStatusSnapshot {
            pane_targets: targets,
            ..Default::default()
        });

        // A bad token learns nothing about the session.
        let mut rejected = open_terminal(addr, "session-1", "wrong");
        let error = read_until(&mut rejected, |m| m["type"] == "error");
        assert_eq!(error["message"], "unauthorized");

        let mut socket = open_terminal(addr, "session-1", &token);
        let frame = read_until(&mut socket, |m| m["type"] == "frame");
        assert_eq!(frame["ansi"], "$ ");
        assert_eq!(frame["cols"], 80);

        // Raw terminal bytes arrive as tmux keys, and the echo streams back.
        socket
            .send(tungstenite::Message::text(
                serde_json::json!({"type": "input", "data": "ls\r"}).to_string(),
            ))
            .unwrap();
        read_until(&mut socket, |m| m["type"] == "frame" && m["ansi"] == "$ ls");
        assert_eq!(
            *panes.received.lock().unwrap(),
            vec![
                crate::remote_terminal::PaneInput::Literal("ls".into()),
                crate::remote_terminal::PaneInput::Key("Enter".into()),
            ]
        );

        // Scrollback on request, capped.
        socket
            .send(tungstenite::Message::text(
                serde_json::json!({"type": "history", "lines": 999_999}).to_string(),
            ))
            .unwrap();
        let history = read_until(&mut socket, |m| m["type"] == "history");
        assert_eq!(history["ansi"], "history(5000)\n$ ls");

        // A key name that isn't tmux vocabulary never reaches tmux.
        socket
            .send(tungstenite::Message::text(
                serde_json::json!({"type": "key", "name": "Enter; kill-server"}).to_string(),
            ))
            .unwrap();
        let error = read_until(&mut socket, |m| m["type"] == "error");
        assert_eq!(error["message"], "unknown key");
        assert_eq!(panes.received.lock().unwrap().len(), 2);

        // Revoking the device closes a socket that is already open.
        let mut revoked = open_terminal(addr, "session-1", &token);
        read_until(&mut revoked, |m| m["type"] == "frame");
        handle.publish_authorized_devices(HashMap::new());
        let error = read_until(&mut revoked, |m| m["type"] == "error");
        assert_eq!(error["message"], "unauthorized");
        read_until(&mut socket, |m| {
            m["type"] == "error" && m["message"] == "unauthorized"
        });
    }

    fn test_config(vapid_public_key: Option<String>) -> ServerConfig {
        ServerConfig {
            vapid_public_key,
            pane_io: Arc::new(NoPanes),
        }
    }

    /// Publish a single authorized device and wait (briefly — the relay
    /// task applies it asynchronously) until a request bearing its token
    /// actually gets past `require_device_auth`, so callers don't race the
    /// relay. Returns the plaintext token to send as `Authorization: Bearer
    /// <token>`.
    fn publish_one_authorized_device(handle: &RemoteServerHandle, addr: SocketAddr) -> String {
        let token = "test-device-token".to_string();
        let mut table = HashMap::new();
        table.insert(
            hash_token(&token),
            AuthorizedDevice {
                device_id: "dev-1".to_string(),
                device_name: "Test Phone".to_string(),
            },
        );
        handle.publish_authorized_devices(table);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if let Ok(resp) = ureq::get(format!("http://{addr}/status"))
                .header("Authorization", format!("Bearer {token}"))
                .call()
                && resp.status() == StatusCode::OK
            {
                return token;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("authorized device never took effect on /status");
    }

    #[test]
    fn serves_the_pwa_shell_without_a_token() {
        let handle = start(any_local_addr(), test_config(None));
        let addr = wait_for_started(&handle);

        for asset in WEB_ASSETS {
            let resp = ureq::get(format!("http://{addr}{}", asset.path))
                .call()
                .unwrap_or_else(|e| panic!("{} failed: {e}", asset.path));
            assert_eq!(
                resp.headers()["content-type"].to_str().unwrap(),
                asset.content_type,
                "{}",
                asset.path
            );
            assert!(!asset.body.is_empty(), "{}", asset.path);
        }

        // The QR opens `/?code=…`; the query must still reach the shell.
        let resp = ureq::get(format!("http://{addr}/?code=123456"))
            .call()
            .unwrap();
        assert!(
            resp.into_body()
                .read_to_string()
                .unwrap()
                .contains("/app.js")
        );
    }

    #[test]
    fn status_requires_a_bearer_token() {
        let handle = start(any_local_addr(), test_config(None));
        let addr = wait_for_started(&handle);

        let resp = ureq::get(format!("http://{addr}/status")).call();
        assert!(matches!(resp, Err(ureq::Error::StatusCode(401))));
    }

    #[test]
    fn status_rejects_an_unknown_or_revoked_token() {
        let handle = start(any_local_addr(), test_config(None));
        let addr = wait_for_started(&handle);
        // An authorized table with a *different* device than the one about
        // to be tried — same effect as an unknown or revoked token, since
        // `require_device_auth` never distinguishes the two (see its doc
        // comment).
        let mut table = HashMap::new();
        table.insert(
            hash_token("someone-elses-token"),
            AuthorizedDevice {
                device_id: "dev-1".to_string(),
                device_name: "Other Phone".to_string(),
            },
        );
        handle.publish_authorized_devices(table);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut rejected = None;
        while std::time::Instant::now() < deadline {
            let resp = ureq::get(format!("http://{addr}/status"))
                .header("Authorization", "Bearer not-a-real-token")
                .call();
            if let Err(ureq::Error::StatusCode(401)) = resp {
                rejected = Some(());
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(rejected.is_some(), "an unknown token should be rejected");
    }

    #[test]
    fn status_endpoint_serves_the_last_published_snapshot() {
        let mut handle = start(any_local_addr(), test_config(None));
        let addr = wait_for_started(&handle);
        let token = publish_one_authorized_device(&handle, addr);

        let snapshot = RemoteStatusSnapshot {
            generated_at: "2026-09-14T00:00:00Z".to_string(),
            features: vec![RemoteFeatureStatus {
                project_name: "my-project".to_string(),
                feature_name: "my-feature".to_string(),
                status: "active".to_string(),
                needs_attention: true,
                attention_reason: Some("Question".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        };
        handle.publish_status(snapshot.clone());

        // The relay task updates the shared state asynchronously; poll
        // briefly rather than assuming it has landed by the next line.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut fetched = None;
        while std::time::Instant::now() < deadline {
            let text = ureq::get(format!("http://{addr}/status"))
                .header("Authorization", format!("Bearer {token}"))
                .call()
                .unwrap()
                .body_mut()
                .read_to_string()
                .unwrap();
            let body: RemoteStatusSnapshot = serde_json::from_str(&text).unwrap();
            if !body.features.is_empty() {
                fetched = Some(body);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        assert_eq!(fetched, Some(snapshot));

        handle.stop();
        let _ = handle.rx.recv_timeout(Duration::from_secs(2));
    }

    #[test]
    fn a_successful_auth_reports_the_device_as_seen() {
        let mut handle = start(any_local_addr(), test_config(None));
        let addr = wait_for_started(&handle);
        publish_one_authorized_device(&handle, addr);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut seen = None;
        while std::time::Instant::now() < deadline {
            if let Some(entry) = handle.try_recv_device_seen() {
                seen = Some(entry);
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(seen, Some(("dev-1".to_string(), "Test Phone".to_string())));
    }
}
