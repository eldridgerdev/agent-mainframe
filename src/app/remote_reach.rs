//! Does the pairing address actually lead to AMF? While the pairing dialog
//! is open, this asks the address the QR encodes for AMF's `/health` from
//! this computer, so a QR that can only fail — a tunnel that is down,
//! Tailscale logged out, a typo in `remote_public_url` — says so on the
//! desk instead of leaving the phone on a blank screen.
//!
//! The answer is advisory in one direction only. Checked from here rather
//! than from the phone, a pass doesn't promise the phone gets through (it
//! may not be on the tailnet), but a failure means nobody will: the address
//! names this machine and even this machine can't reach AMF through it.
//!
//! Like `remote_tailscale`, every check runs on a short-lived thread and
//! reports back over a channel drained by `poll_pairing_reach_bg`.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use super::{App, AppMode, PairingReach, PairingUrlSource};
use crate::tailscale::{TailscaleCli, TailscaleStatus};

/// Bounds one check end to end. An address that resolves to a peer that is
/// offline doesn't refuse — it just never answers — so this is what turns
/// that into a verdict.
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
/// How often a failing address is asked again while the dialog stays open,
/// so fixing it in another terminal (`tailscale up`) clears the warning
/// without reopening the dialog.
const RECHECK_INTERVAL: Duration = Duration::from_secs(10);
/// A reason longer than this is cut, so one odd TLS error can't push the
/// QR off the dialog.
const MAX_REASON_CHARS: usize = 60;

pub struct RemoteReachState {
    /// A check is running. At most one at a time; a result for an address
    /// the dialog has since moved off is dropped, and the next poll asks
    /// again for the new one.
    in_flight: bool,
    last_started: Option<Instant>,
    tx: Sender<(String, PairingReach)>,
    rx: Receiver<(String, PairingReach)>,
}

impl Default for RemoteReachState {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            in_flight: false,
            last_started: None,
            tx,
            rx,
        }
    }
}

/// Ask `base` for AMF's `/health`. Only AMF's own `ok` counts: a tunnel that
/// is up in front of an AMF that isn't (`tailscale serve` answers 502) or
/// that points at some other port is reported as such.
pub fn check_health(base: &str, timeout: Duration) -> PairingReach {
    let url = format!("{}/health", base.trim_end_matches('/'));
    match crate::http_client::probe_agent(timeout).get(&url).call() {
        Ok(mut response) => {
            let status = response.status();
            if !status.is_success() {
                return PairingReach::NotAmf(format!("HTTP {}", status.as_u16()));
            }
            match response.body_mut().with_config().limit(64).read_to_string() {
                Ok(body) if body.trim() == "ok" => PairingReach::Reachable,
                _ => PairingReach::NotAmf("unexpected reply".into()),
            }
        }
        Err(err) => PairingReach::Unreachable(describe_error(&err)),
    }
}

fn describe_error(err: &ureq::Error) -> String {
    let reason = match err {
        ureq::Error::Timeout(_) => "timed out".to_string(),
        ureq::Error::HostNotFound => "name doesn't resolve".to_string(),
        ureq::Error::Io(io) if io.kind() == std::io::ErrorKind::ConnectionRefused => {
            "connection refused".to_string()
        }
        other => other.to_string(),
    };
    if reason.chars().count() > MAX_REASON_CHARS {
        let cut: String = reason.chars().take(MAX_REASON_CHARS - 1).collect();
        format!("{cut}…")
    } else {
        reason
    }
}

/// The host part of `scheme://host[:port][/path]`.
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let end = rest.find(['/', ':', '?', '#']).unwrap_or(rest.len());
    &rest[..end]
}

pub struct ReachAdviceInput<'a> {
    pub url: &'a str,
    pub source: PairingUrlSource,
    pub status: Option<&'a TailscaleStatus>,
    pub cli: &'a TailscaleCli,
}

/// What to do about an address that failed its check. A `*.ts.net`
/// address only works while this computer's Tailscale is signed in and
/// serving AMF on that same name, and the probe already knows which of
/// those is missing. Pure, so every state's wording is tested.
pub fn reach_advice(input: &ReachAdviceInput<'_>) -> Option<String> {
    let host = url_host(input.url);
    if host.ends_with(".ts.net") {
        let up = input.cli.display_command("up");
        return match input.status? {
            TailscaleStatus::NotInstalled => {
                Some("AMF can't find Tailscale here — press s for setup.".into())
            }
            TailscaleStatus::Unavailable(_) => Some(format!(
                "Tailscale isn't running here: start it, then `{up}`."
            )),
            TailscaleStatus::NeedsLogin => {
                Some(format!("Tailscale is logged out here: run `{up}`."))
            }
            TailscaleStatus::Stopped => {
                Some(format!("Tailscale is switched off here: run `{up}`."))
            }
            TailscaleStatus::Running(node) => match node.serve_url.as_deref() {
                None => Some("Tailscale isn't serving AMF: press t.".into()),
                Some(served) if url_host(served) != host => Some(format!(
                    "Tailscale serves AMF at {served} — update remote_public_url."
                )),
                Some(_) => None,
            },
        };
    }
    match input.source {
        PairingUrlSource::Configured => {
            Some("Check the tunnel behind remote_public_url in config.json.".into())
        }
        PairingUrlSource::Direct => Some("Check remote_bind and this computer's firewall.".into()),
        PairingUrlSource::Tailscale => None,
    }
}

/// Run one check off the main loop. Unit tests never touch the network
/// from here; they feed results with `feed_pairing_reach`.
fn spawn_check(tx: Sender<(String, PairingReach)>, url: String) {
    #[cfg(not(test))]
    {
        let _ = std::thread::Builder::new()
            .name("amf-pairing-reach".into())
            .spawn(move || {
                let reach = check_health(&url, CHECK_TIMEOUT);
                let _ = tx.send((url, reach));
            });
    }
    #[cfg(test)]
    {
        let _ = (tx, url);
    }
}

impl App {
    /// Ask the open pairing dialog's address for `/health` in the
    /// background. Does nothing outside the dialog, or while a check is
    /// already running (its result, or the poll after it, covers this).
    pub(crate) fn check_pairing_reach(&mut self) {
        let AppMode::RemotePairing(state) = &mut self.mode else {
            return;
        };
        if state.url_unreachable {
            state.reach = PairingReach::NotChecked;
            return;
        }
        if self.remote_reach.in_flight {
            return;
        }
        let url = state.url.clone();
        self.remote_reach.in_flight = true;
        self.remote_reach.last_started = Some(Instant::now());
        spawn_check(self.remote_reach.tx.clone(), url);
    }

    /// Ask again right away if the last answer was a failure — Tailscale
    /// just changed state, so the advice (and maybe the verdict) may be
    /// stale.
    pub(crate) fn recheck_failed_pairing_reach(&mut self) {
        if let AppMode::RemotePairing(state) = &self.mode
            && state.reach.failed()
        {
            self.check_pairing_reach();
        }
    }

    /// Drain finished checks into the dialog and start the next one when
    /// it's due. Returns `true` when the dialog changed.
    pub(crate) fn poll_pairing_reach_bg(&mut self) -> bool {
        let results: Vec<(String, PairingReach)> = self.remote_reach.rx.try_iter().collect();
        let mut changed = false;
        for (url, reach) in results {
            self.remote_reach.in_flight = false;
            let AppMode::RemotePairing(state) = &mut self.mode else {
                continue;
            };
            if state.url != url || state.reach == reach {
                continue;
            }
            state.reach = reach.clone();
            changed = true;
            self.log_debug("remote_server", format!("pairing address {url}: {reach:?}"));
        }

        let AppMode::RemotePairing(state) = &self.mode else {
            return changed;
        };
        let failed = state.reach.failed();
        let due = match &state.reach {
            // A dialog whose address changed under a running check: that
            // check's answer was dropped, so ask about the new one.
            PairingReach::Checking => true,
            _ if failed => self
                .remote_reach
                .last_started
                .is_none_or(|started| started.elapsed() >= RECHECK_INTERVAL),
            _ => false,
        };
        if due && !self.remote_reach.in_flight {
            self.check_pairing_reach();
            if failed {
                // So the advice follows a `tailscale up` run elsewhere.
                self.probe_tailscale();
            }
        }
        changed
    }

    /// What the open dialog should suggest for its failed address.
    pub fn pairing_reach_advice(&self) -> Option<String> {
        let AppMode::RemotePairing(state) = &self.mode else {
            return None;
        };
        let cli = self.tailscale_cli();
        reach_advice(&ReachAdviceInput {
            url: &state.url,
            source: state.url_source,
            status: self.remote_tailscale.status.as_ref(),
            cli: &cli,
        })
    }

    #[cfg(test)]
    pub(crate) fn feed_pairing_reach(&mut self, url: &str, reach: PairingReach) {
        let _ = self.remote_reach.tx.send((url.to_string(), reach));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tailscale::TailnetNode;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    /// A one-shot HTTP server answering every request with `response`.
    fn serve_once(response: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn only_amfs_own_ok_counts_as_reachable() {
        let amf = serve_once("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
        assert_eq!(check_health(&amf, CHECK_TIMEOUT), PairingReach::Reachable);

        let proxy_error = serve_once(
            "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(
            check_health(&proxy_error, CHECK_TIMEOUT),
            PairingReach::NotAmf("HTTP 502".into())
        );

        let other =
            serve_once("HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        assert_eq!(
            check_health(&other, CHECK_TIMEOUT),
            PairingReach::NotAmf("unexpected reply".into())
        );
    }

    #[test]
    fn a_closed_port_is_unreachable() {
        let addr = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        // The listener is dropped here, so nothing is on the port.
        assert!(matches!(
            check_health(&format!("http://{addr}"), CHECK_TIMEOUT),
            PairingReach::Unreachable(_)
        ));
    }

    #[test]
    fn a_silent_peer_times_out_instead_of_hanging() {
        // Accepts and never answers — what an offline tailnet peer looks
        // like from the caller's side.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = std::thread::spawn(move || listener.accept().map(|(s, _)| s));
        let started = Instant::now();
        let reach = check_health(&format!("http://{addr}"), Duration::from_millis(300));
        assert_eq!(reach, PairingReach::Unreachable("timed out".into()));
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(hold);
    }

    #[test]
    fn url_host_strips_scheme_port_and_path() {
        assert_eq!(url_host("https://pc.tail1.ts.net"), "pc.tail1.ts.net");
        assert_eq!(
            url_host("https://pc.tail1.ts.net:8443/x"),
            "pc.tail1.ts.net"
        );
        assert_eq!(url_host("http://127.0.0.1:47800"), "127.0.0.1");
    }

    fn advice(
        url: &str,
        source: PairingUrlSource,
        status: Option<TailscaleStatus>,
    ) -> Option<String> {
        reach_advice(&ReachAdviceInput {
            url,
            source,
            status: status.as_ref(),
            cli: &TailscaleCli::new(None, None),
        })
    }

    fn running(serve_url: Option<&str>) -> TailscaleStatus {
        TailscaleStatus::Running(TailnetNode {
            dns_name: "pc.tail1.ts.net".into(),
            https_enabled: true,
            tagged_for_amf: true,
            serve_url: serve_url.map(str::to_string),
            peers: 1,
        })
    }

    #[test]
    fn a_ts_net_address_is_advised_from_the_tailscale_probe() {
        let url = "https://pc.tail1.ts.net";
        let configured = PairingUrlSource::Configured;
        assert_eq!(advice(url, configured, None), None);
        assert!(
            advice(url, configured, Some(TailscaleStatus::NeedsLogin))
                .unwrap()
                .contains("logged out here: run `tailscale up`")
        );
        assert!(
            advice(url, configured, Some(TailscaleStatus::Stopped))
                .unwrap()
                .contains("switched off")
        );
        assert!(
            advice(url, configured, Some(TailscaleStatus::NotInstalled))
                .unwrap()
                .contains("press s")
        );
        assert_eq!(
            advice(url, configured, Some(running(None))).as_deref(),
            Some("Tailscale isn't serving AMF: press t.")
        );
        assert!(
            advice(
                url,
                configured,
                Some(running(Some("https://other.tail1.ts.net")))
            )
            .unwrap()
            .contains("serves AMF at https://other.tail1.ts.net")
        );
        // Serving this very name: the advice has nothing to add.
        assert_eq!(advice(url, configured, Some(running(Some(url)))), None);
    }

    #[test]
    fn other_addresses_point_at_what_the_user_controls() {
        assert!(
            advice(
                "https://amf.example.com",
                PairingUrlSource::Configured,
                Some(TailscaleStatus::NeedsLogin)
            )
            .unwrap()
            .contains("remote_public_url")
        );
        assert!(
            advice("http://192.168.1.5:47800", PairingUrlSource::Direct, None)
                .unwrap()
                .contains("remote_bind")
        );
    }
}
