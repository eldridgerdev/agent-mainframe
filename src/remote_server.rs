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
//! This module is intentionally minimal for now: a single `/health` route
//! proves the thread starts, binds, and stops cleanly without blocking the
//! UI. Pairing, status relay, and terminal streaming are added as their own
//! epics land on top of this skeleton.

use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, Sender, channel};

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

/// Owns the remote-control server's thread and lets the main loop request
/// shutdown without blocking on the async runtime tearing down.
pub struct RemoteServerHandle {
    pub rx: Receiver<RemoteServerEvent>,
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

            runtime.block_on(run_server(bind_addr, event_tx, shutdown_rx));
        })
        .expect("failed to spawn amf-remote-server thread");

    RemoteServerHandle {
        rx: event_rx,
        shutdown: Some(shutdown_tx),
        join: Some(join),
    }
}

async fn run_server(
    bind_addr: SocketAddr,
    event_tx: Sender<RemoteServerEvent>,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let router = axum::Router::new().route("/health", axum::routing::get(|| async { "ok" }));

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
}
