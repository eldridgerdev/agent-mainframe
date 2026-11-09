//! Pure classification of git/ssh stderr and the advice derived from it.
//!
//! Fixes are a closed enum mapped to fixed argv lists; URL text never appears
//! in an argv or shell string.

use super::url::ParsedUrl;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailureClass {
    Auth,
    HostKey,
    NotFoundOrNoAccess,
    Network,
    Timeout,
    EmptyRepo,
    /// Anything unrecognised; the raw stderr is preserved.
    Unknown(String),
}

/// A user-run remedy. Closed set, each with a fixed argv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixId {
    GhAuthLogin,
    SshAdd,
    SshKeygen,
}

impl FixId {
    pub fn argv(self) -> &'static [&'static str] {
        match self {
            Self::GhAuthLogin => &["gh", "auth", "login"],
            Self::SshAdd => &["ssh-add"],
            Self::SshKeygen => &["ssh-keygen", "-t", "ed25519"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::GhAuthLogin => "Log in to GitHub",
            Self::SshAdd => "Add your SSH key to the agent",
            Self::SshKeygen => "Create an SSH key",
        }
    }

    /// Space-joined argv for display / copying. Contains no URL text.
    pub fn command_line(self) -> String {
        self.argv().join(" ")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    pub summary: String,
    pub detail: String,
    pub fixes: Vec<FixId>,
}

pub fn classify_stderr(stderr: &str) -> FailureClass {
    let s = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| s.contains(n));

    if has(&[
        "host key verification failed",
        "remote host identification has changed",
    ]) {
        return FailureClass::HostKey;
    }
    if has(&[
        "permission denied (",
        "authentication failed",
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "invalid username or password",
        "access denied",
    ]) {
        return FailureClass::Auth;
    }
    if has(&[
        "repository not found",
        "does not appear to be a git repository",
        "not found",
        "does not exist",
    ]) {
        return FailureClass::NotFoundOrNoAccess;
    }
    if has(&["timed out", "timeout"]) {
        return FailureClass::Timeout;
    }
    if has(&[
        "could not resolve host",
        "temporary failure in name resolution",
        "name or service not known",
        "network is unreachable",
        "connection refused",
        "no route to host",
        "failed to connect",
    ]) {
        return FailureClass::Network;
    }
    if has(&["empty repository", "remote head refers to nonexistent ref"]) {
        return FailureClass::EmptyRepo;
    }
    FailureClass::Unknown(stderr.trim().to_string())
}

pub fn advise(class: &FailureClass, url: &ParsedUrl) -> Advice {
    let host = &url.host;
    let gh = url.is_github() && url.is_https();
    let (summary, detail, fixes): (String, String, Vec<FixId>) = match class {
        FailureClass::Auth if url.is_https() => (
            format!("Authentication failed for {host}."),
            if gh {
                "Log in with the GitHub CLI, then re-run the check.".to_string()
            } else {
                "Configure a git credential helper or access token for this host, or use an SSH URL. AMF does not store credentials.".to_string()
            },
            if gh { vec![FixId::GhAuthLogin] } else { vec![] },
        ),
        FailureClass::Auth => (
            format!("SSH authentication to {host} was rejected."),
            "Make sure an SSH key is loaded in your agent and registered with the host.".to_string(),
            vec![FixId::SshAdd, FixId::SshKeygen],
        ),
        FailureClass::HostKey => (
            format!("The host key for {host} is not trusted."),
            "Connect once yourself (for example `ssh` to the host) and verify the fingerprint against the provider's published keys, then re-run the check. AMF never accepts host keys automatically.".to_string(),
            vec![],
        ),
        FailureClass::NotFoundOrNoAccess => (
            "Repository not found, or you do not have access.".to_string(),
            if gh {
                "Check the URL. For a private repository, log in with the GitHub CLI.".to_string()
            } else {
                "Check the URL and that your account can read this repository.".to_string()
            },
            if gh { vec![FixId::GhAuthLogin] } else { vec![] },
        ),
        FailureClass::Network => (
            format!("Could not reach {host}."),
            "Check your network connection, DNS, VPN or proxy, then retry.".to_string(),
            vec![],
        ),
        FailureClass::Timeout => (
            format!("Timed out connecting to {host}."),
            "The host did not respond in time. Check your network or firewall, then retry.".to_string(),
            vec![],
        ),
        FailureClass::EmptyRepo => (
            "The remote repository is empty.".to_string(),
            "It has no commits or default branch to clone. Push a first commit and retry.".to_string(),
            vec![],
        ),
        FailureClass::Unknown(raw) => (
            "git could not reach the repository.".to_string(),
            if raw.is_empty() {
                "git reported no further details.".to_string()
            } else {
                raw.clone()
            },
            vec![],
        ),
    };
    Advice {
        summary,
        detail,
        fixes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_clone::url::parse;

    #[test]
    fn classifies_fixtures() {
        let cases = [
            (
                "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights\nand the repository exists.",
                FailureClass::Auth,
            ),
            (
                "Host key verification failed.\nfatal: Could not read from remote repository.",
                FailureClass::HostKey,
            ),
            (
                "remote: Repository not found.\nfatal: repository 'https://github.com/o/r/' not found",
                FailureClass::NotFoundOrNoAccess,
            ),
            (
                "remote: Invalid username or password.\nfatal: Authentication failed for 'https://github.com/o/r/'",
                FailureClass::Auth,
            ),
            (
                "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
                FailureClass::Auth,
            ),
            (
                "fatal: unable to access 'https://nope.invalid/o/r/': Could not resolve host: nope.invalid",
                FailureClass::Network,
            ),
            (
                "ssh: connect to host h port 22: Connection timed out\nfatal: Could not read from remote repository.",
                FailureClass::Timeout,
            ),
            (
                "warning: You appear to have cloned an empty repository.",
                FailureClass::EmptyRepo,
            ),
        ];
        for (stderr, want) in cases {
            assert_eq!(classify_stderr(stderr), want, "{stderr}");
        }
        assert_eq!(
            classify_stderr("  something odd happened \n"),
            FailureClass::Unknown("something odd happened".into())
        );
    }

    #[test]
    fn gh_login_only_for_github_https() {
        let gh = parse("https://github.com/o/r").unwrap();
        let gh_ssh = parse("git@github.com:o/r.git").unwrap();
        let other = parse("https://gitlab.example.com/o/r").unwrap();
        let evil = parse("https://github.com.evil.io/o/r").unwrap();
        assert_eq!(
            advise(&FailureClass::Auth, &gh).fixes,
            vec![FixId::GhAuthLogin]
        );
        assert!(advise(&FailureClass::Auth, &other).fixes.is_empty());
        assert!(advise(&FailureClass::Auth, &evil).fixes.is_empty());
        assert!(
            !advise(&FailureClass::Auth, &gh_ssh)
                .fixes
                .contains(&FixId::GhAuthLogin)
        );
        assert!(
            !advise(&FailureClass::NotFoundOrNoAccess, &gh_ssh)
                .fixes
                .contains(&FixId::GhAuthLogin)
        );
    }

    #[test]
    fn ssh_auth_offers_ssh_fixes() {
        for u in ["git@github.com:o/r.git", "ssh://git@host:2222/o/r.git"] {
            let a = advise(&FailureClass::Auth, &parse(u).unwrap());
            assert_eq!(a.fixes, vec![FixId::SshAdd, FixId::SshKeygen]);
        }
    }

    #[test]
    fn generic_hosts_and_unknown_keep_text() {
        let u = parse("https://example.org/o/r").unwrap();
        for c in [
            FailureClass::HostKey,
            FailureClass::Network,
            FailureClass::Timeout,
            FailureClass::EmptyRepo,
            FailureClass::NotFoundOrNoAccess,
        ] {
            let a = advise(&c, &u);
            assert!(a.fixes.is_empty());
            assert!(!a.summary.is_empty());
        }
        let a = advise(&FailureClass::Unknown("raw boom".into()), &u);
        assert!(a.detail.contains("raw boom"));
    }

    #[test]
    fn fix_argv_has_no_url_text() {
        for f in [FixId::GhAuthLogin, FixId::SshAdd, FixId::SshKeygen] {
            assert!(
                f.argv()
                    .iter()
                    .all(|a| !a.contains("://") && !a.contains('@'))
            );
        }
        assert_eq!(FixId::GhAuthLogin.command_line(), "gh auth login");
    }
}
