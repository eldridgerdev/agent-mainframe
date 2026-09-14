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

use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteFeatureStatus {
    pub project_name: String,
    pub feature_name: String,
    /// `"active"` / `"idle"` / `"stopped"` (`project::ProjectStatus`'s
    /// `Display` impl).
    pub status: String,
    pub needs_attention: bool,
    /// `AttentionState::label()` ("Question" / "Completed" / "Waiting")
    /// when `needs_attention` is true.
    pub attention_reason: Option<String>,
}

/// The full read-only status feed (Phase 1). Rebuilt by `App` and pushed to
/// the server thread on every main-loop tick while the server is running —
/// see `App::poll_remote_server_bg`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct RemoteStatusSnapshot {
    pub generated_at: String,
    pub features: Vec<RemoteFeatureStatus>,
}

type SharedStatus = Arc<Mutex<RemoteStatusSnapshot>>;

/// A `POST /pair/exchange` request, forwarded from the HTTP handler to the
/// main loop. `reply` is a one-shot back-channel — created fresh per
/// request, not a persistent pipe — so the handler can simply `.await` it
/// after sending, with a timeout in case the main loop is unreachable.
pub struct PairingExchangeRequest {
    pub code: String,
    pub device_name: String,
    pub reply: tokio::sync::oneshot::Sender<PairingExchangeOutcome>,
}

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
pub fn start(bind_addr: SocketAddr) -> RemoteServerHandle {
    let (event_tx, event_rx) = channel::<RemoteServerEvent>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (status_tx, status_rx) = tokio::sync::mpsc::unbounded_channel::<RemoteStatusSnapshot>();
    let (pairing_tx, pairing_rx) = tokio::sync::mpsc::unbounded_channel::<PairingExchangeRequest>();

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

            runtime.block_on(run_server(
                bind_addr,
                event_tx,
                shutdown_rx,
                status_rx,
                pairing_tx,
            ));
        })
        .expect("failed to spawn amf-remote-server thread");

    RemoteServerHandle {
        rx: event_rx,
        status_tx,
        pairing_rx,
        shutdown: Some(shutdown_tx),
        join: Some(join),
    }
}

/// State shared across axum handlers. `pairing_tx` is `Clone` (an unbounded
/// `mpsc` sender), so deriving `Clone` here is enough for axum's
/// `with_state` — no `Arc` wrapper needed beyond the one `status` already
/// carries.
#[derive(Clone)]
struct ServerState {
    status: SharedStatus,
    pairing_tx: tokio::sync::mpsc::UnboundedSender<PairingExchangeRequest>,
}

async fn run_server(
    bind_addr: SocketAddr,
    event_tx: Sender<RemoteServerEvent>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    mut status_rx: tokio::sync::mpsc::UnboundedReceiver<RemoteStatusSnapshot>,
    pairing_tx: tokio::sync::mpsc::UnboundedSender<PairingExchangeRequest>,
) {
    let status: SharedStatus = Arc::new(Mutex::new(RemoteStatusSnapshot::default()));

    // Relay task: the only writer to `status`, so the lock is never held
    // across an `.await`. `App` pushes a new snapshot on every main-loop
    // tick while the server is running; this just keeps the latest one
    // ready for `/status` to serve without touching `App` itself.
    let status_for_relay = status.clone();
    tokio::spawn(async move {
        while let Some(snapshot) = status_rx.recv().await {
            *status_for_relay.lock().unwrap() = snapshot;
        }
    });

    let state = ServerState { status, pairing_tx };

    let router = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/status", axum::routing::get(status_handler))
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

async fn status_handler(State(state): State<ServerState>) -> Json<RemoteStatusSnapshot> {
    Json(state.status.lock().unwrap().clone())
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
        Ok(Ok(outcome)) => outcome_to_response(outcome),
        // Either the oneshot sender was dropped without a reply (shouldn't
        // happen — `App` always replies) or the main loop hasn't polled
        // this request within the timeout (e.g. AMF is unresponsive).
        // Either way, the honest answer is "try again", not a 4xx that
        // implies the code itself was wrong.
        _ => unavailable_response(),
    }
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
        let mut handle = start(any_local_addr());

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
        let handle = start(any_local_addr());
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
        let mut a = start(any_local_addr());
        let mut b = start(any_local_addr());

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
    fn status_endpoint_serves_the_last_published_snapshot() {
        let mut handle = start(any_local_addr());
        let addr = wait_for_started(&handle);

        // Before anything is published, /status serves the empty default
        // rather than erroring — there's just nothing to report yet.
        let body = ureq::get(format!("http://{addr}/status"))
            .call()
            .expect("GET /status should succeed with no snapshot published")
            .body_mut()
            .read_to_string()
            .expect("body should be readable");
        let empty: RemoteStatusSnapshot =
            serde_json::from_str(&body).expect("response should be valid JSON");
        assert!(empty.features.is_empty());

        let snapshot = RemoteStatusSnapshot {
            generated_at: "2026-09-14T00:00:00Z".to_string(),
            features: vec![RemoteFeatureStatus {
                project_name: "my-project".to_string(),
                feature_name: "my-feature".to_string(),
                status: "active".to_string(),
                needs_attention: true,
                attention_reason: Some("Question".to_string()),
            }],
        };
        handle.publish_status(snapshot.clone());

        // The relay task updates the shared state asynchronously; poll
        // briefly rather than assuming it has landed by the next line.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut fetched = None;
        while std::time::Instant::now() < deadline {
            let text = ureq::get(format!("http://{addr}/status"))
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
}
