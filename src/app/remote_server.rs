//! `App`-side lifecycle management for the Remote Control companion-app
//! server. See `crate::remote_server` for the server thread itself and
//! `docs/backlog/remote-control-companion-app-plan.md` (Epic 1) for the
//! design this implements.

use std::net::SocketAddr;

use crate::remote_server::{self, RemoteServerEvent};

use super::App;

/// Default bind address: loopback-only until the pairing/auth epics land,
/// so the skeleton never exposes anything beyond localhost.
const DEFAULT_BIND_ADDR: &str = "127.0.0.1:7864";

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

        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::ProjectStore;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
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
}
