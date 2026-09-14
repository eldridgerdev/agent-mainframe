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
//! server thread stores only the latest snapshot it was handed. Pairing and
//! terminal streaming are added as their own epics land on top of this.

use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::extract::State;
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

/// Owns the remote-control server's thread and lets the main loop request
/// shutdown without blocking on the async runtime tearing down.
pub struct RemoteServerHandle {
    pub rx: Receiver<RemoteServerEvent>,
    status_tx: tokio::sync::mpsc::UnboundedSender<RemoteStatusSnapshot>,
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

            runtime.block_on(run_server(bind_addr, event_tx, shutdown_rx, status_rx));
        })
        .expect("failed to spawn amf-remote-server thread");

    RemoteServerHandle {
        rx: event_rx,
        status_tx,
        shutdown: Some(shutdown_tx),
        join: Some(join),
    }
}

async fn run_server(
    bind_addr: SocketAddr,
    event_tx: Sender<RemoteServerEvent>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    mut status_rx: tokio::sync::mpsc::UnboundedReceiver<RemoteStatusSnapshot>,
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

    let router = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/status", axum::routing::get(status_handler))
        .with_state(status);

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

async fn status_handler(State(status): State<SharedStatus>) -> Json<RemoteStatusSnapshot> {
    Json(status.lock().unwrap().clone())
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
