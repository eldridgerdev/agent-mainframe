//! Egress allowlist: an allowlisting HTTP(S) proxy in its own container, and
//! the network layout that leaves the sandbox no other way out.
//!
//! The sandbox container joins an `--internal` Docker network (no route to the
//! outside, no DNS to the outside). The proxy container joins that network
//! *and* the default bridge, so it is the only thing that can reach the
//! internet. The sandbox gets `HTTP(S)_PROXY` pointing at the proxy, which
//! permits CONNECT only to allowlisted hostnames on port 443.
//!
//! Limits: this filters by the hostname the client asks for. It does not stop
//! data leaving through an allowlisted host (e.g. pushing to github.com), and
//! it cannot help if the allowlist itself is too broad.

use std::fmt::Write as _;

pub const PROXY_PORT: u16 = 8888;

/// Escape a hostname for a POSIX extended regex.
fn regex_escape_host(host: &str) -> String {
    host.replace('.', "\\.")
}

/// One anchored, case-insensitive pattern per allowlist entry. An entry
/// `*.example.com` matches the domain and its subdomains; a bare hostname
/// matches only itself.
pub fn filter_file(allowlist: &[String]) -> String {
    let mut out = String::new();
    for entry in allowlist {
        match entry.strip_prefix("*.") {
            Some(domain) => {
                let _ = writeln!(out, "^(.+\\.)?{}$", regex_escape_host(domain));
            }
            None => {
                let _ = writeln!(out, "^{}$", regex_escape_host(entry));
            }
        }
    }
    out
}

/// tinyproxy configuration. Default-deny, CONNECT to 443 only.
pub fn proxy_conf(filter_path: &str) -> String {
    format!(
        "Port {PROXY_PORT}\n\
         Listen 0.0.0.0\n\
         Timeout 600\n\
         MaxClients 64\n\
         ConnectPort 443\n\
         FilterDefaultDeny Yes\n\
         FilterExtended On\n\
         FilterCaseSensitive Off\n\
         Filter \"{filter_path}\"\n\
         DisableViaHeader Yes\n\
         LogLevel Warning\n\
         PidFile \"/tmp/tinyproxy.pid\"\n"
    )
}

/// Environment that makes common clients use the proxy. `NO_PROXY` stays
/// empty on purpose: the internal network has nothing worth bypassing to.
pub fn proxy_env(proxy_host: &str) -> Vec<(String, String)> {
    let url = format!("http://{proxy_host}:{PROXY_PORT}");
    ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"]
        .iter()
        .map(|k| (k.to_string(), url.clone()))
        .chain([("NO_PROXY".to_string(), String::new())])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_anchors_and_escapes() {
        let f = filter_file(&["github.com".into(), "*.npmjs.org".into()]);
        assert_eq!(f, "^github\\.com$\n^(.+\\.)?npmjs\\.org$\n");
    }

    #[test]
    fn exact_entry_does_not_match_lookalikes() {
        // The anchoring is what stops evilgithub.com / github.com.evil.io.
        let f = filter_file(&["github.com".into()]);
        let pat = f.trim();
        assert!(pat.starts_with('^') && pat.ends_with('$'));
        assert!(!pat.contains(".*"));
    }

    #[test]
    fn conf_is_default_deny_and_443_only() {
        let c = proxy_conf("/etc/tinyproxy/allow.txt");
        assert!(c.contains("FilterDefaultDeny Yes"));
        assert!(c.contains("ConnectPort 443"));
        assert!(!c.contains("ConnectPort 80"));
        assert!(c.contains("Filter \"/etc/tinyproxy/allow.txt\""));
    }

    #[test]
    fn env_points_all_clients_at_the_proxy() {
        let env = proxy_env("amf-proxy");
        assert!(env.contains(&("HTTPS_PROXY".into(), "http://amf-proxy:8888".into())));
        assert!(env.contains(&("https_proxy".into(), "http://amf-proxy:8888".into())));
        assert!(env.contains(&("NO_PROXY".into(), String::new())));
    }
}
