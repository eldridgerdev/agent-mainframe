//! Tailscale as AMF Remote's tunnel.
//!
//! A phone can only install AMF Remote and receive notifications over
//! HTTPS, and `tailscale serve` is the simplest way to get that: it gives
//! this computer an `https://<machine>.<tailnet>.ts.net` address that only
//! the user's own devices can open. This module asks the local `tailscale`
//! CLI whether that is already set up — so the pairing QR can use the
//! tailnet address without the user copying it into `config.json` — and
//! runs `tailscale serve` when the user asks it to.
//!
//! Everything here shells out to the CLI rather than talking to
//! `tailscaled`'s local API: the CLI already knows where the daemon's
//! socket is on every platform, and a user running a userspace daemon can
//! point AMF at theirs with `remote_tailscale_socket`. Parsing is split out
//! into pure functions so it is tested against the CLI's real JSON shape.

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::Deserialize;

/// The program name tried when `remote_tailscale_cli` isn't set.
const DEFAULT_CLI: &str = "tailscale";
/// Where the Mac App Store / standalone app keeps its CLI, which is not on
/// `PATH` unless the user added it.
const MACOS_APP_CLI: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
/// `tailscale status` answers in milliseconds; this only bounds a hung
/// daemon so the pairing dialog or `amf doctor` never waits on it.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// `tailscale serve` waits indefinitely for the user to approve Serve in
/// the admin console when their tailnet hasn't enabled it yet. Past this,
/// the approval link it printed is handed back instead.
const SERVE_TIMEOUT: Duration = Duration::from_secs(10);

/// The tag the access policy grants on, applied to the computer running
/// AMF in Tailscale's Machines page.
pub const ACCESS_TAG: &str = "tag:amf";

/// A Tailscale access policy that lets only the tailnet owner's own
/// devices reach the AMF computer, and only on the HTTPS port
/// `tailscale serve` answers on. Copied by `c` in the pairing dialog's
/// setup view and printed in `docs/remote-control.md`.
pub const ACCESS_POLICY: &str = r#"{
  // AMF Remote: only your own devices may reach the computer running AMF,
  // and only over HTTPS. Tag that computer tag:amf in the Machines page.
  // Remove the default {"src": ["*"], "dst": ["*"], "ip": ["*"]} grant,
  // or it still lets everything reach everything.
  "tagOwners": {
    "tag:amf": ["autogroup:admin"],
  },
  "grants": [
    { "src": ["autogroup:member"], "dst": ["tag:amf"], "ip": ["tcp:443"] },
  ],
}"#;

/// The admin-console pages the setup steps point at.
pub const ADMIN_DNS_URL: &str = "https://login.tailscale.com/admin/dns";
pub const ADMIN_ACL_URL: &str = "https://login.tailscale.com/admin/acls/file";
pub const ADMIN_MACHINES_URL: &str = "https://login.tailscale.com/admin/machines";
pub const DOWNLOAD_URL: &str = "https://tailscale.com/download";

/// What the local Tailscale looks like to AMF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailscaleStatus {
    /// No `tailscale` program could be started.
    NotInstalled,
    /// The CLI ran but couldn't report a status — usually `tailscaled` isn't
    /// running. Carries the CLI's own first line of explanation.
    Unavailable(String),
    /// Installed and running, but not signed in to a tailnet.
    NeedsLogin,
    /// Signed in but switched off (`tailscale down`).
    Stopped,
    Running(TailnetNode),
}

impl TailscaleStatus {
    /// The HTTPS address `tailscale serve` answers on for AMF's port, if it
    /// is serving it.
    pub fn serve_url(&self) -> Option<&str> {
        match self {
            TailscaleStatus::Running(node) => node.serve_url.as_deref(),
            _ => None,
        }
    }

    /// Running with HTTPS certificates on, but not serving AMF yet: the one
    /// state where `tailscale serve` can succeed without the user leaving
    /// AMF first.
    pub fn can_start_serving(&self) -> bool {
        matches!(self, TailscaleStatus::Running(node) if node.serve_url.is_none())
    }
}

/// This computer as a node on the tailnet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailnetNode {
    /// `machine.tailnet.ts.net`, without the trailing dot the CLI reports.
    /// Empty when MagicDNS is off.
    pub dns_name: String,
    /// Whether the tailnet can issue this node an HTTPS certificate
    /// (MagicDNS + HTTPS Certificates enabled in the admin console).
    pub https_enabled: bool,
    /// Whether this node carries [`ACCESS_TAG`] — a hint, not proof, that
    /// the access policy has been set up.
    pub tagged_for_amf: bool,
    /// `https://machine.tailnet.ts.net[:port]` when `tailscale serve`
    /// proxies to AMF's port on this machine.
    pub serve_url: Option<String>,
}

/// How a `tailscale serve` attempt went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeOutcome {
    Serving,
    /// The tailnet hasn't enabled Serve (or HTTPS) yet; Tailscale printed
    /// a link for the user to approve it in the admin console.
    NeedsApproval(String),
    Failed(String),
}

/// The `tailscale` CLI AMF talks to: the program plus the daemon socket to
/// use, both from config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailscaleCli {
    program: PathBuf,
    socket: Option<PathBuf>,
    /// Whether `program` came from config. Only the default name falls back
    /// to the macOS app's bundled CLI.
    configured: bool,
}

impl TailscaleCli {
    /// Build from `remote_tailscale_cli` / `remote_tailscale_socket`
    /// (`~/` expanded; blank means unset).
    pub fn new(program: Option<&str>, socket: Option<&str>) -> Self {
        let program = program.map(str::trim).filter(|p| !p.is_empty());
        Self {
            program: program.map_or_else(|| PathBuf::from(DEFAULT_CLI), expand_home),
            socket: socket
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(expand_home),
            configured: program.is_some(),
        }
    }

    /// A shell command line equivalent to running the CLI with `args`, for
    /// instructions the user types themselves.
    pub fn display_command(&self, args: &str) -> String {
        let mut line = self.program.display().to_string();
        if let Some(socket) = &self.socket {
            line.push_str(&format!(" --socket={}", socket.display()));
        }
        line.push(' ');
        line.push_str(args);
        line
    }

    /// Ask Tailscale for this node's state and whether it serves `port`.
    pub fn probe(&self, port: u16) -> TailscaleStatus {
        let ran = match self.run(&["status", "--json"], PROBE_TIMEOUT) {
            Ok(ran) => ran,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return TailscaleStatus::NotInstalled;
            }
            Err(e) => return TailscaleStatus::Unavailable(e.to_string()),
        };
        // `status --json` still prints JSON (with `BackendState`) when it
        // has something to say, e.g. NeedsLogin, even on a non-zero exit —
        // so parse first and only fall back to the error text.
        let Some(parsed) = parse_status(&ran.stdout) else {
            return TailscaleStatus::Unavailable(ran.explanation());
        };
        match parsed {
            ParsedStatus::Running(mut node) => {
                if !node.dns_name.is_empty()
                    && let Ok(serve) = self.run(&["serve", "status", "--json"], PROBE_TIMEOUT)
                    && serve.success
                {
                    node.serve_url = serve_url_for_port(&serve.stdout, port);
                }
                TailscaleStatus::Running(node)
            }
            ParsedStatus::Other(status) => status,
        }
    }

    /// Serve AMF's `port` on this node's tailnet HTTPS address, in the
    /// background (`--bg`), so it survives this process. Tailnet-only:
    /// this never enables Funnel.
    pub fn serve(&self, port: u16) -> ServeOutcome {
        let target = format!("http://127.0.0.1:{port}");
        let ran = match self.run(&["serve", "--bg", &target], SERVE_TIMEOUT) {
            Ok(ran) => ran,
            Err(e) => return ServeOutcome::Failed(e.to_string()),
        };
        if ran.success {
            return ServeOutcome::Serving;
        }
        match approval_link(&format!("{}\n{}", ran.stdout, ran.stderr)) {
            Some(link) => ServeOutcome::NeedsApproval(link),
            None => ServeOutcome::Failed(ran.explanation()),
        }
    }

    fn run(&self, args: &[&str], timeout: Duration) -> std::io::Result<Ran> {
        match run_with_timeout(self.command(&self.program, args), timeout) {
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    && !self.configured
                    && std::path::Path::new(MACOS_APP_CLI).exists() =>
            {
                run_with_timeout(self.command(&PathBuf::from(MACOS_APP_CLI), args), timeout)
            }
            other => other,
        }
    }

    fn command(&self, program: &PathBuf, args: &[&str]) -> Command {
        let mut command = Command::new(program);
        if let Some(socket) = &self.socket {
            command.arg(format!("--socket={}", socket.display()));
        }
        command.args(args);
        command
    }
}

/// The port in a `host:port` bind address, falling back to the default
/// `remote_bind` port when it doesn't parse — the same fallback the server
/// itself uses.
pub fn bind_port(bind: &str) -> u16 {
    bind.parse::<std::net::SocketAddr>()
        .map(|addr| addr.port())
        .unwrap_or(47800)
}

fn expand_home(raw: &str) -> PathBuf {
    match raw.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map_or_else(|| PathBuf::from(raw), |home| home.join(rest)),
        None => PathBuf::from(raw),
    }
}

// ---- parsing ----------------------------------------------------------

#[derive(Debug, PartialEq, Eq)]
enum ParsedStatus {
    Running(TailnetNode),
    Other(TailscaleStatus),
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct StatusJson {
    #[serde(default)]
    backend_state: String,
    #[serde(rename = "Self", default)]
    self_node: Option<SelfJson>,
    #[serde(default)]
    cert_domains: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct SelfJson {
    #[serde(rename = "DNSName", default)]
    dns_name: String,
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
}

/// `tailscale status --json` → state. `None` when it isn't that JSON at
/// all (the daemon couldn't be reached and the CLI printed prose).
fn parse_status(json: &str) -> Option<ParsedStatus> {
    let status: StatusJson = serde_json::from_str(json).ok()?;
    Some(match status.backend_state.as_str() {
        "Running" => {
            let node = status.self_node.unwrap_or(SelfJson {
                dns_name: String::new(),
                tags: None,
            });
            ParsedStatus::Running(TailnetNode {
                dns_name: node.dns_name.trim_end_matches('.').to_string(),
                https_enabled: status.cert_domains.is_some_and(|d| !d.is_empty()),
                tagged_for_amf: node
                    .tags
                    .is_some_and(|tags| tags.iter().any(|t| t == ACCESS_TAG)),
                serve_url: None,
            })
        }
        "NeedsLogin" | "NeedsMachineAuth" => ParsedStatus::Other(TailscaleStatus::NeedsLogin),
        "Stopped" => ParsedStatus::Other(TailscaleStatus::Stopped),
        other => ParsedStatus::Other(TailscaleStatus::Unavailable(format!(
            "Tailscale is {}",
            if other.is_empty() { "not ready" } else { other }
        ))),
    })
}

#[derive(Deserialize, Default)]
struct ServeJson {
    #[serde(rename = "Web", default)]
    web: HashMap<String, WebJson>,
}

#[derive(Deserialize)]
struct WebJson {
    #[serde(rename = "Handlers", default)]
    handlers: HashMap<String, HandlerJson>,
}

#[derive(Deserialize)]
struct HandlerJson {
    #[serde(rename = "Proxy", default)]
    proxy: Option<String>,
}

/// `tailscale serve status --json` → the HTTPS address whose root proxies
/// to `port` on this machine. Prefers the standard HTTPS port, so a user
/// who also serves AMF on a second port gets the tidy URL.
fn serve_url_for_port(json: &str, port: u16) -> Option<String> {
    let serve: ServeJson = serde_json::from_str(json).unwrap_or_default();
    let mut matches: Vec<(String, u16)> = serve
        .web
        .into_iter()
        .filter(|(_, web)| {
            web.handlers
                .get("/")
                .and_then(|handler| handler.proxy.as_deref())
                .is_some_and(|proxy| proxies_to_local_port(proxy, port))
        })
        .filter_map(|(host_port, _)| {
            let (host, https_port) = host_port.rsplit_once(':')?;
            Some((host.to_string(), https_port.parse().ok()?))
        })
        .collect();
    matches.sort_by_key(|(host, https_port)| (*https_port != 443, *https_port, host.clone()));
    matches.into_iter().next().map(|(host, https_port)| {
        if https_port == 443 {
            format!("https://{host}")
        } else {
            format!("https://{host}:{https_port}")
        }
    })
}

/// Whether a serve handler's proxy target is `port` on this machine:
/// `http://127.0.0.1:47800`, `localhost:47800`, `http://[::1]:47800/`, ….
fn proxies_to_local_port(proxy: &str, port: u16) -> bool {
    let rest = proxy
        .strip_prefix("http://")
        .or_else(|| proxy.strip_prefix("https+insecure://"))
        .unwrap_or(proxy);
    let authority = rest.split('/').next().unwrap_or(rest);
    let Some((host, target_port)) = authority.rsplit_once(':') else {
        return false;
    };
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "0.0.0.0")
        && target_port.parse() == Ok(port)
}

/// The admin-console link `tailscale serve` prints when the tailnet must
/// approve Serve or HTTPS first.
fn approval_link(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|word| word.starts_with("https://login.tailscale.com/"))
        .map(|word| word.trim_end_matches(['.', ',', ')']).to_string())
}

// ---- running the CLI ---------------------------------------------------

struct Ran {
    success: bool,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

impl Ran {
    /// The CLI's own first line of explanation for a failure.
    fn explanation(&self) -> String {
        if self.timed_out {
            return "tailscale didn't answer in time".to_string();
        }
        self.stderr
            .lines()
            .chain(self.stdout.lines())
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or("tailscale failed without saying why")
            .to_string()
    }
}

/// Run `command`, killing it past `timeout`. Output is collected on reader
/// threads so a chatty process can't block on a full pipe while we wait.
fn run_with_timeout(mut command: Command, timeout: Duration) -> std::io::Result<Ran> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    let read_all = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_string(&mut text);
            }
            text
        })
    };
    let stdout = read_all(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = read_all(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );

    let deadline = Instant::now() + timeout;
    let (success, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status.success(), false);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break (false, true);
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    Ok(Ran {
        success,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
        timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `tailscale status --json` (1.102).
    const RUNNING: &str = r#"{
        "Version": "1.102.4",
        "BackendState": "Running",
        "Self": { "DNSName": "amf-dev.tail88768d.ts.net.", "HostName": "amf-dev", "Tags": null },
        "MagicDNSSuffix": "tail88768d.ts.net",
        "CertDomains": ["amf-dev.tail88768d.ts.net"]
    }"#;

    /// A real `tailscale serve status --json` after `tailscale serve --bg 47800`.
    const SERVING: &str = r#"{
        "TCP": { "443": { "HTTPS": true } },
        "Web": {
            "amf-dev.tail88768d.ts.net:443": {
                "Handlers": { "/": { "Proxy": "http://127.0.0.1:47800" } }
            }
        }
    }"#;

    fn running(json: &str) -> TailnetNode {
        match parse_status(json) {
            Some(ParsedStatus::Running(node)) => node,
            other => panic!("expected Running, got {other:?}"),
        }
    }

    #[test]
    fn reads_a_running_node() {
        let node = running(RUNNING);
        assert_eq!(node.dns_name, "amf-dev.tail88768d.ts.net");
        assert!(node.https_enabled);
        assert!(!node.tagged_for_amf);
        assert_eq!(node.serve_url, None);
    }

    #[test]
    fn https_is_off_without_cert_domains_and_tags_are_read() {
        let node = running(
            r#"{"BackendState":"Running","CertDomains":null,
                "Self":{"DNSName":"pc.t.ts.net.","Tags":["tag:amf"]}}"#,
        );
        assert!(!node.https_enabled);
        assert!(node.tagged_for_amf);
    }

    #[test]
    fn maps_the_other_backend_states() {
        let other = |state: &str| parse_status(&format!(r#"{{"BackendState":"{state}"}}"#));
        assert_eq!(
            other("NeedsLogin"),
            Some(ParsedStatus::Other(TailscaleStatus::NeedsLogin))
        );
        assert_eq!(
            other("Stopped"),
            Some(ParsedStatus::Other(TailscaleStatus::Stopped))
        );
        assert_eq!(
            other("Starting"),
            Some(ParsedStatus::Other(TailscaleStatus::Unavailable(
                "Tailscale is Starting".into()
            )))
        );
        assert_eq!(parse_status("failed to connect to local tailscaled"), None);
    }

    #[test]
    fn finds_the_serve_url_for_amfs_port() {
        assert_eq!(
            serve_url_for_port(SERVING, 47800).as_deref(),
            Some("https://amf-dev.tail88768d.ts.net")
        );
        assert_eq!(serve_url_for_port(SERVING, 3000), None);
        assert_eq!(serve_url_for_port("{}", 47800), None);
        assert_eq!(serve_url_for_port("not json", 47800), None);
    }

    #[test]
    fn a_non_standard_https_port_keeps_its_port_and_443_wins() {
        let json = r#"{"Web":{
            "pc.t.ts.net:8443":{"Handlers":{"/":{"Proxy":"localhost:47800"}}},
            "pc.t.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:47800/"}}}}}"#;
        assert_eq!(
            serve_url_for_port(json, 47800).as_deref(),
            Some("https://pc.t.ts.net")
        );
        let only_8443 = r#"{"Web":{
            "pc.t.ts.net:8443":{"Handlers":{"/":{"Proxy":"localhost:47800"}}}}}"#;
        assert_eq!(
            serve_url_for_port(only_8443, 47800).as_deref(),
            Some("https://pc.t.ts.net:8443")
        );
    }

    #[test]
    fn only_a_local_proxy_on_the_same_port_counts() {
        assert!(proxies_to_local_port("http://127.0.0.1:47800", 47800));
        assert!(proxies_to_local_port("localhost:47800", 47800));
        assert!(proxies_to_local_port("http://[::1]:47800/amf", 47800));
        assert!(!proxies_to_local_port("http://127.0.0.1:47801", 47800));
        assert!(!proxies_to_local_port("http://10.0.0.5:47800", 47800));
        assert!(!proxies_to_local_port("47800", 47800));
    }

    #[test]
    fn extracts_the_approval_link() {
        let output = "Serve is not enabled on your tailnet.\nTo enable, visit:\n\n         \
                      https://login.tailscale.com/f/serve?node=nABC123.\n";
        assert_eq!(
            approval_link(output).as_deref(),
            Some("https://login.tailscale.com/f/serve?node=nABC123")
        );
        assert_eq!(approval_link("error: nope"), None);
    }

    #[test]
    fn display_command_includes_a_configured_socket() {
        let plain = TailscaleCli::new(None, None);
        assert_eq!(
            plain.display_command("serve --bg 47800"),
            "tailscale serve --bg 47800"
        );
        let custom = TailscaleCli::new(Some("/opt/ts/tailscale"), Some("/run/ts.sock"));
        assert_eq!(
            custom.display_command("status"),
            "/opt/ts/tailscale --socket=/run/ts.sock status"
        );
    }

    #[test]
    fn a_missing_program_is_not_installed() {
        let cli = TailscaleCli::new(Some("/nonexistent/amf-test/tailscale"), None);
        assert_eq!(cli.probe(47800), TailscaleStatus::NotInstalled);
    }

    #[test]
    fn a_hung_command_is_killed_at_the_timeout() {
        let mut command = Command::new("sleep");
        command.arg("5");
        let started = Instant::now();
        let ran = run_with_timeout(command, Duration::from_millis(100)).unwrap();
        assert!(ran.timed_out);
        assert!(!ran.success);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn bind_port_falls_back_to_the_default() {
        assert_eq!(bind_port("127.0.0.1:47801"), 47801);
        assert_eq!(bind_port("nonsense"), 47800);
    }
}
