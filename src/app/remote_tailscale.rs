//! Tailscale for AMF Remote, from the desk's side: probe the local
//! Tailscale in the background so the pairing QR can use this machine's
//! tailnet address (`App::pairing_base`), start `tailscale serve` when the
//! user presses `t` in the pairing dialog, and describe the setup steps the
//! dialog's `s` view walks through — each marked done from what the probe
//! saw rather than from what the user says.
//!
//! The CLI calls themselves live in `crate::tailscale`; every call here runs
//! on a short-lived thread and reports back over a channel drained by
//! `poll_tailscale_bg`, so a slow or hung `tailscaled` never stalls the UI.

use std::sync::mpsc::{Receiver, Sender, channel};

use super::App;
use crate::tailscale::{self, ServeOutcome, TailscaleCli, TailscaleStatus};

enum TailscaleEvent {
    Probed(TailscaleStatus),
    Served(ServeOutcome),
}

pub struct RemoteTailscaleState {
    /// The latest probe. `None` until the first one lands.
    pub status: Option<TailscaleStatus>,
    pub probing: bool,
    /// A `tailscale serve` started by `t` is still running.
    pub serving: bool,
    /// What the last `t` needs the user to know — an approval link to open,
    /// or why it failed. Cleared by the next attempt or a success.
    pub serve_note: Option<ServeOutcome>,
    tx: Sender<TailscaleEvent>,
    rx: Receiver<TailscaleEvent>,
}

impl Default for RemoteTailscaleState {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            status: None,
            probing: false,
            serving: false,
            serve_note: None,
            tx,
            rx,
        }
    }
}

/// Run `job` off the main loop and send its result back. Unit tests never
/// shell out to a real `tailscale`; they set `status` or feed events
/// directly.
fn spawn_job(tx: Sender<TailscaleEvent>, job: impl FnOnce() -> TailscaleEvent + Send + 'static) {
    #[cfg(not(test))]
    {
        let _ = std::thread::Builder::new()
            .name("amf-tailscale".into())
            .spawn(move || {
                let _ = tx.send(job());
            });
    }
    #[cfg(test)]
    {
        let _ = (tx, job);
    }
}

impl App {
    pub(crate) fn tailscale_cli(&self) -> TailscaleCli {
        TailscaleCli::new(
            self.config.remote_tailscale_cli.as_deref(),
            self.config.remote_tailscale_socket.as_deref(),
        )
    }

    /// The port AMF Remote listens on: the bound one once the server is up,
    /// else the configured one.
    pub(crate) fn remote_port(&self) -> u16 {
        self.remote_server_addr
            .map(|addr| addr.port())
            .unwrap_or_else(|| tailscale::bind_port(&self.config.remote_bind))
    }

    /// Re-check Tailscale in the background. At most one probe runs at a
    /// time; a request while one is in flight is already covered by it.
    pub fn probe_tailscale(&mut self) {
        if self.remote_tailscale.probing {
            return;
        }
        self.remote_tailscale.probing = true;
        let cli = self.tailscale_cli();
        let port = self.remote_port();
        spawn_job(self.remote_tailscale.tx.clone(), move || {
            TailscaleEvent::Probed(cli.probe(port))
        });
    }

    /// `t` in the pairing dialog: serve AMF's port on this machine's
    /// tailnet address. Only offered once a probe has seen Tailscale
    /// running and not already serving it; anything else says what to do
    /// instead.
    pub fn start_tailscale_serve(&mut self) {
        if self.remote_tailscale.serving {
            return;
        }
        match &self.remote_tailscale.status {
            None => {
                self.push_toast_info("Still checking Tailscale — try again in a moment");
                self.probe_tailscale();
                return;
            }
            Some(status) if status.serve_url().is_some() => {
                self.push_toast_info("Tailscale already serves AMF on your tailnet");
                return;
            }
            Some(status) if !status.can_start_serving() => {
                self.push_toast_warning("Tailscale isn't running here — press s for setup steps");
                return;
            }
            Some(_) => {}
        }
        self.remote_tailscale.serving = true;
        self.remote_tailscale.serve_note = None;
        let cli = self.tailscale_cli();
        let port = self.remote_port();
        self.log_info("remote_server", format!("tailscale serve --bg {port}"));
        spawn_job(self.remote_tailscale.tx.clone(), move || {
            TailscaleEvent::Served(cli.serve(port))
        });
    }

    /// `o` in the pairing dialog: open the admin-console link a `tailscale
    /// serve` asked the user to approve.
    pub fn open_tailscale_approval_link(&mut self) {
        let Some(ServeOutcome::NeedsApproval(link)) = &self.remote_tailscale.serve_note else {
            return;
        };
        let link = link.clone();
        if let Err(e) = crate::app::util::open_in_browser(&link) {
            self.push_toast_error(format!("Open failed: {e}"));
        }
    }

    /// Drain finished probes and serve attempts. Returns `true` when
    /// anything changed.
    pub(crate) fn poll_tailscale_bg(&mut self) -> bool {
        let events: Vec<TailscaleEvent> = self.remote_tailscale.rx.try_iter().collect();
        let changed = !events.is_empty();
        for event in events {
            match event {
                TailscaleEvent::Probed(status) => self.apply_tailscale_probe(status),
                TailscaleEvent::Served(outcome) => self.apply_tailscale_serve(outcome),
            }
        }
        changed
    }

    fn apply_tailscale_probe(&mut self, status: TailscaleStatus) {
        self.remote_tailscale.probing = false;
        if self.remote_tailscale.status.as_ref() != Some(&status) {
            self.log_debug("remote_server", format!("tailscale: {status:?}"));
        }
        self.remote_tailscale.status = Some(status);
        self.refresh_pairing_url();
    }

    fn apply_tailscale_serve(&mut self, outcome: ServeOutcome) {
        self.remote_tailscale.serving = false;
        match outcome {
            ServeOutcome::Serving => {
                self.remote_tailscale.serve_note = None;
                self.log_info("remote_server", "tailscale serve: serving".to_string());
                self.push_toast_success("Tailscale now serves AMF on your tailnet");
            }
            other => {
                self.log_warn("remote_server", format!("tailscale serve: {other:?}"));
                self.remote_tailscale.serve_note = Some(other);
            }
        }
        // Either way, re-read what Tailscale actually serves now: that is
        // what the QR must point at, not what we asked for.
        self.probe_tailscale();
    }

    /// The setup walkthrough for this machine, from the latest probe.
    pub fn pairing_setup_steps(&self) -> Vec<SetupStep> {
        let cli = self.tailscale_cli();
        setup_steps(&SetupInput {
            status: self.remote_tailscale.status.as_ref(),
            public_url: self
                .config
                .remote_public_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty()),
            cli: &cli,
            port: self.remote_port(),
        })
    }

    #[cfg(test)]
    pub(crate) fn feed_tailscale_probe(&mut self, status: TailscaleStatus) {
        let _ = self
            .remote_tailscale
            .tx
            .send(TailscaleEvent::Probed(status));
    }

    #[cfg(test)]
    pub(crate) fn feed_tailscale_serve(&mut self, outcome: ServeOutcome) {
        let _ = self
            .remote_tailscale
            .tx
            .send(TailscaleEvent::Served(outcome));
    }
}

// ---- setup steps --------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    Done,
    Todo,
    /// Can't tell yet — an earlier step isn't done, or no probe has landed.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupStep {
    pub state: StepState,
    pub title: String,
    pub lines: Vec<String>,
}

impl SetupStep {
    fn new(state: StepState, title: &str, lines: Vec<String>) -> Self {
        Self {
            state,
            title: title.to_string(),
            lines,
        }
    }
}

pub struct SetupInput<'a> {
    pub status: Option<&'a TailscaleStatus>,
    pub public_url: Option<&'a str>,
    pub cli: &'a TailscaleCli,
    pub port: u16,
}

/// The pairing dialog's setup walkthrough (`s`), from a phone with nothing
/// installed to a paired, locked-down AMF Remote. Pure, so each state's
/// wording is tested without a real Tailscale.
pub fn setup_steps(input: &SetupInput<'_>) -> Vec<SetupStep> {
    use StepState::{Done, Todo, Unknown};

    let status = input.status;
    let node = match status {
        Some(TailscaleStatus::Running(node)) => Some(node),
        _ => None,
    };
    let serve_cmd = input
        .cli
        .display_command(&format!("serve --bg {}", input.port));
    let mut steps = Vec::new();

    if let Some(url) = input.public_url {
        steps.push(SetupStep::new(
            Done,
            "Phone address set by remote_public_url",
            vec![
                url.to_string(),
                "AMF uses this instead of asking Tailscale. Remove it from".into(),
                "~/.config/amf/config.json to use Tailscale's address automatically.".into(),
            ],
        ));
    }

    let install = vec![
        format!("This computer: {}", tailscale::DOWNLOAD_URL),
        "Your phone: the Tailscale app from its app store.".into(),
        "Already installed? AMF looks for `tailscale` on your PATH; if it".into(),
        "lives elsewhere, set \"remote_tailscale_cli\" to it in config.json.".into(),
    ];
    steps.push(SetupStep::new(
        match status {
            None => Unknown,
            Some(TailscaleStatus::NotInstalled) => Todo,
            Some(_) => Done,
        },
        "Install Tailscale on this computer and your phone",
        match status {
            Some(TailscaleStatus::NotInstalled) | None => install,
            Some(_) => vec![
                format!("Found: {}", input.cli.display_command(""))
                    .trim_end()
                    .to_string(),
            ],
        },
    ));

    let up = input.cli.display_command("up");
    steps.push(match status {
        Some(TailscaleStatus::Running(_)) => SetupStep::new(
            Done,
            "Sign in on both, with the same account",
            vec!["This computer is on your tailnet.".into()],
        ),
        Some(TailscaleStatus::NeedsLogin) => SetupStep::new(
            Todo,
            "Sign in on both, with the same account",
            vec![
                format!("Run: {up}"),
                "and sign in to the same account in the phone's app.".into(),
            ],
        ),
        Some(TailscaleStatus::Stopped) => SetupStep::new(
            Todo,
            "Sign in on both, with the same account",
            vec![format!("Tailscale is switched off. Run: {up}")],
        ),
        Some(TailscaleStatus::Unavailable(why)) => SetupStep::new(
            Todo,
            "Sign in on both, with the same account",
            vec![
                format!("Tailscale didn't answer: {why}"),
                "Start Tailscale (its app, or the tailscaled service), then press r.".into(),
                "Running tailscaled with its own --socket? Set".into(),
                "\"remote_tailscale_socket\" to that path in config.json.".into(),
            ],
        ),
        Some(TailscaleStatus::NotInstalled) | None => SetupStep::new(
            Unknown,
            "Sign in on both, with the same account",
            vec![format!("Run: {up}")],
        ),
    });

    let https_lines = vec![
        "Admin console → DNS: enable MagicDNS and HTTPS Certificates.".into(),
        tailscale::ADMIN_DNS_URL.into(),
    ];
    steps.push(match node {
        Some(node) if node.https_enabled && !node.dns_name.is_empty() => SetupStep::new(
            Done,
            "Turn on HTTPS for your tailnet",
            vec![format!("This computer is {}.", node.dns_name)],
        ),
        Some(_) => SetupStep::new(Todo, "Turn on HTTPS for your tailnet", https_lines),
        None => SetupStep::new(Unknown, "Turn on HTTPS for your tailnet", https_lines),
    });

    let share_lines = vec![
        format!("Press t here, or run: {serve_cmd}"),
        "Only your tailnet can open it. Never use `tailscale funnel`,".into(),
        "which would put AMF on the public internet.".into(),
    ];
    steps.push(match node.and_then(|node| node.serve_url.as_deref()) {
        Some(url) => SetupStep::new(
            Done,
            "Share AMF on your tailnet",
            vec![format!("Your devices open {url}")],
        ),
        None if node.is_some_and(|node| node.https_enabled) => {
            SetupStep::new(Todo, "Share AMF on your tailnet", share_lines)
        }
        None => SetupStep::new(Unknown, "Share AMF on your tailnet", share_lines),
    });

    let lock_lines = vec![
        "1. Access controls: add the AMF policy (press c to copy it).".into(),
        format!("   {}", tailscale::ADMIN_ACL_URL),
        format!(
            "2. Machines: give this computer the {} tag.",
            tailscale::ACCESS_TAG
        ),
        format!("   {}", tailscale::ADMIN_MACHINES_URL),
        "Your devices can then reach only AMF, only over HTTPS.".into(),
    ];
    steps.push(match node {
        Some(node) if node.tagged_for_amf => SetupStep::new(
            Done,
            "Limit access to your own devices (recommended)",
            vec![format!(
                "Tagged {}. Check the policy grants it only tcp:443 (c copies one).",
                tailscale::ACCESS_TAG
            )],
        ),
        Some(_) => SetupStep::new(
            Todo,
            "Limit access to your own devices (recommended)",
            lock_lines,
        ),
        None => SetupStep::new(
            Unknown,
            "Limit access to your own devices (recommended)",
            lock_lines,
        ),
    });

    steps.push(SetupStep::new(
        Todo,
        "Scan the QR code with your phone's camera",
        vec![
            "Esc returns to it. Tap Pair; then in Chrome, ⋮ → Install app,".into(),
            "and Turn on notifications inside AMF Remote.".into(),
        ],
    ));
    steps
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tailscale::TailnetNode;

    fn node(https: bool, serve: Option<&str>, tagged: bool) -> TailscaleStatus {
        TailscaleStatus::Running(TailnetNode {
            dns_name: "pc.tail1.ts.net".into(),
            https_enabled: https,
            tagged_for_amf: tagged,
            serve_url: serve.map(str::to_string),
        })
    }

    fn states(status: Option<&TailscaleStatus>) -> Vec<StepState> {
        let cli = TailscaleCli::new(None, None);
        setup_steps(&SetupInput {
            status,
            public_url: None,
            cli: &cli,
            port: 47800,
        })
        .iter()
        .map(|step| step.state)
        .collect()
    }

    use StepState::{Done, Todo, Unknown};

    #[test]
    fn nothing_installed_starts_at_step_one() {
        assert_eq!(
            states(Some(&TailscaleStatus::NotInstalled)),
            [Todo, Unknown, Unknown, Unknown, Unknown, Todo]
        );
    }

    #[test]
    fn a_fresh_tailnet_needs_https_then_serve() {
        assert_eq!(
            states(Some(&node(false, None, false))),
            [Done, Done, Todo, Unknown, Todo, Todo]
        );
        assert_eq!(
            states(Some(&node(true, None, false))),
            [Done, Done, Done, Todo, Todo, Todo]
        );
    }

    #[test]
    fn a_serving_tagged_node_has_only_pairing_left() {
        let status = node(true, Some("https://pc.tail1.ts.net"), true);
        assert_eq!(states(Some(&status)), [Done, Done, Done, Done, Done, Todo]);
    }

    #[test]
    fn a_stopped_daemon_is_the_sign_in_step() {
        let status = TailscaleStatus::Unavailable("failed to connect".into());
        assert_eq!(states(Some(&status))[..2], [Done, Todo]);
    }

    fn step_text(status: &TailscaleStatus, cli: &TailscaleCli) -> Vec<String> {
        setup_steps(&SetupInput {
            status: Some(status),
            public_url: None,
            cli,
            port: 47800,
        })
        .into_iter()
        .flat_map(|step| step.lines)
        .collect()
    }

    #[test]
    fn the_steps_are_the_same_everywhere_and_name_the_config_keys() {
        let cli = TailscaleCli::new(None, None);
        let missing = step_text(&TailscaleStatus::NotInstalled, &cli);
        assert!(
            missing
                .iter()
                .any(|line| line.contains("remote_tailscale_cli"))
        );
        assert!(!missing.iter().any(|line| line.contains("WSL")));

        let silent = step_text(&TailscaleStatus::Unavailable("no socket".into()), &cli);
        assert!(
            silent
                .iter()
                .any(|line| line.contains("remote_tailscale_socket"))
        );
    }

    #[test]
    fn a_configured_socket_is_in_the_commands_shown() {
        let cli = TailscaleCli::new(None, Some("/tmp/ts.sock"));
        let text = step_text(
            &TailscaleStatus::Running(crate::tailscale::TailnetNode {
                dns_name: "pc.tail1.ts.net".into(),
                https_enabled: true,
                tagged_for_amf: false,
                serve_url: None,
            }),
            &cli,
        );
        assert!(
            text.iter()
                .any(|line| line.contains("tailscale --socket=/tmp/ts.sock serve --bg 47800"))
        );
    }

    #[test]
    fn a_public_url_is_explained_first() {
        let cli = TailscaleCli::new(None, None);
        let steps = setup_steps(&SetupInput {
            status: None,
            public_url: Some("https://example.test"),
            cli: &cli,
            port: 47800,
        });
        assert_eq!(steps[0].title, "Phone address set by remote_public_url");
        assert_eq!(steps.len(), 7);
    }
}
