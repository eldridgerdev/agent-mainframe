//! Pure parser for git remote URLs accepted by "new project from git URL".
//!
//! Only HTTPS, scp-style SSH (`user@host:path`) and `ssh://` are supported.
//! Anything that could be read as an option, a local path, or a non-network
//! transport is rejected.

/// Which URL form was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    Https,
    /// `[user@]host:path`
    ScpSsh,
    /// `ssh://[user@]host[:port]/path`
    Ssh,
}

/// A validated remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    /// The trimmed input, safe to pass after `--`.
    pub raw: String,
    pub kind: UrlKind,
    pub host: String,
    pub port: Option<u16>,
    pub repo_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlError {
    Empty,
    LeadingDash,
    Whitespace,
    LocalPath,
    UnsupportedScheme(String),
    InvalidHost,
    InvalidPort,
    MissingPath,
    InvalidPath,
    InvalidRepoName,
    EmbeddedPassword,
}

impl std::fmt::Display for UrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "Enter a git URL."),
            Self::LeadingDash => write!(f, "A URL cannot start with '-'."),
            Self::Whitespace => write!(f, "A URL cannot contain spaces or control characters."),
            Self::LocalPath => write!(
                f,
                "Local paths are not supported; use an https:// or ssh URL."
            ),
            Self::UnsupportedScheme(s) => write!(
                f,
                "Unsupported URL scheme '{s}'; use https://, ssh:// or user@host:path."
            ),
            Self::InvalidHost => write!(f, "The URL has a missing or invalid host."),
            Self::InvalidPort => write!(f, "The URL has an invalid port."),
            Self::MissingPath => write!(f, "The URL has no repository path."),
            Self::InvalidPath => write!(f, "The URL path is not a valid repository path."),
            Self::InvalidRepoName => {
                write!(f, "Could not derive a safe repository name from the URL.")
            }
            Self::EmbeddedPassword => write!(f, "Do not put a password in the URL."),
        }
    }
}

impl std::error::Error for UrlError {}

impl ParsedUrl {
    /// Exact match on `github.com` (no alias or suffix matching).
    pub fn is_github(&self) -> bool {
        self.host.eq_ignore_ascii_case("github.com")
    }

    pub fn is_https(&self) -> bool {
        self.kind == UrlKind::Https
    }

    pub fn is_ssh(&self) -> bool {
        !self.is_https()
    }
}

pub fn parse(input: &str) -> Result<ParsedUrl, UrlError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(UrlError::Empty);
    }
    if s.starts_with('-') {
        return Err(UrlError::LeadingDash);
    }
    if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(UrlError::Whitespace);
    }
    if s.contains('\\') {
        return Err(UrlError::LocalPath);
    }
    if let Some((scheme, rest)) = s.split_once("://") {
        let kind = match scheme.to_ascii_lowercase().as_str() {
            "https" => UrlKind::Https,
            "ssh" => UrlKind::Ssh,
            other => return Err(UrlError::UnsupportedScheme(other.to_string())),
        };
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, ""),
        };
        let (host, port) = parse_authority(authority, kind == UrlKind::Https)?;
        let repo_name = repo_name_from_path(path)?;
        return Ok(ParsedUrl {
            raw: s.to_string(),
            kind,
            host,
            port,
            repo_name,
        });
    }
    // No "://": only scp-style is acceptable.
    if s.starts_with('/') || s.starts_with('.') || s.starts_with('~') {
        return Err(UrlError::LocalPath);
    }
    let Some((left, path)) = s.split_once(':') else {
        return Err(UrlError::LocalPath);
    };
    if path.starts_with(':') {
        // `ext::sh ...` and similar remote-helper syntax.
        return Err(UrlError::UnsupportedScheme(left.to_string()));
    }
    if left.contains('/') {
        return Err(UrlError::LocalPath);
    }
    let host = match left.rsplit_once('@') {
        Some((user, host)) => {
            validate_user(user)?;
            host
        }
        None => left,
    };
    let host = validate_host(host)?;
    let repo_name = repo_name_from_path(&format!("/{}", path.trim_start_matches('/')))?;
    Ok(ParsedUrl {
        raw: s.to_string(),
        kind: UrlKind::ScpSsh,
        host,
        port: None,
        repo_name,
    })
}

fn validate_user(user: &str) -> Result<(), UrlError> {
    if user.is_empty()
        || !user
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '+'))
    {
        return Err(UrlError::InvalidHost);
    }
    Ok(())
}

fn parse_authority(
    authority: &str,
    reject_password: bool,
) -> Result<(String, Option<u16>), UrlError> {
    let hostport = match authority.rsplit_once('@') {
        Some((userinfo, hp)) => {
            if userinfo.contains(':') {
                if reject_password {
                    return Err(UrlError::EmbeddedPassword);
                }
                return Err(UrlError::InvalidHost);
            }
            validate_user(userinfo)?;
            hp
        }
        None => authority,
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => {
            let port: u16 = p.parse().map_err(|_| UrlError::InvalidPort)?;
            if port == 0 {
                return Err(UrlError::InvalidPort);
            }
            (h, Some(port))
        }
        None => (hostport, None),
    };
    Ok((validate_host(host)?, port))
}

fn validate_host(host: &str) -> Result<String, UrlError> {
    let ok = !host.is_empty()
        && !host.starts_with('-')
        && !host.starts_with('.')
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(host.to_string())
    } else {
        Err(UrlError::InvalidHost)
    }
}

/// `path` starts with `/` (or is empty).
fn repo_name_from_path(path: &str) -> Result<String, UrlError> {
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(UrlError::MissingPath);
    }
    if trimmed.contains(['?', '#']) {
        return Err(UrlError::InvalidPath);
    }
    let segments: Vec<&str> = trimmed.trim_start_matches('/').split('/').collect();
    if segments
        .iter()
        .any(|s| s.is_empty() || *s == "." || *s == "..")
    {
        return Err(UrlError::InvalidPath);
    }
    derive_repo_name(segments.last().copied().unwrap_or(""))
}

/// Strip a trailing `.git` and reject names unsafe to use as a directory.
pub fn derive_repo_name(last_segment: &str) -> Result<String, UrlError> {
    let name = last_segment.trim_end_matches('/');
    let name = name.strip_suffix(".git").unwrap_or(name);
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(|c| c.is_control())
    {
        return Err(UrlError::InvalidRepoName);
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts() {
        // (input, kind, host, port, repo)
        let cases = [
            (
                "https://github.com/o/r",
                UrlKind::Https,
                "github.com",
                None,
                "r",
            ),
            (
                "https://github.com/o/r.git",
                UrlKind::Https,
                "github.com",
                None,
                "r",
            ),
            (
                "https://github.com/o/r/",
                UrlKind::Https,
                "github.com",
                None,
                "r",
            ),
            (
                "https://github.com/o/r.git/",
                UrlKind::Https,
                "github.com",
                None,
                "r",
            ),
            (
                "https://host:8443/group/sub/r.git",
                UrlKind::Https,
                "host",
                Some(8443),
                "r",
            ),
            ("git@host:o/r.git", UrlKind::ScpSsh, "host", None, "r"),
            ("host:o/r", UrlKind::ScpSsh, "host", None, "r"),
            (
                "ssh://git@host:2222/o/r.git",
                UrlKind::Ssh,
                "host",
                Some(2222),
                "r",
            ),
            ("ssh://host/~user/r", UrlKind::Ssh, "host", None, "r"),
            (
                "  https://github.com/o/r  ",
                UrlKind::Https,
                "github.com",
                None,
                "r",
            ),
        ];
        for (input, kind, host, port, repo) in cases {
            let p = parse(input).unwrap_or_else(|e| panic!("{input:?} rejected: {e:?}"));
            assert_eq!(p.kind, kind, "{input}");
            assert_eq!(p.host, host, "{input}");
            assert_eq!(p.port, port, "{input}");
            assert_eq!(p.repo_name, repo, "{input}");
            assert_eq!(p.raw, input.trim());
        }
    }

    #[test]
    fn rejects() {
        let cases = [
            "-oProxyCommand=x",
            "--upload-pack=x",
            "file:///x",
            "/abs/path",
            "./rel",
            "../rel",
            "ext::sh -c id",
            "git://host/o/r",
            "http://host/o/r",
            "https://host/",
            "https://host",
            "https://host/..",
            "https://host/o/../r",
            "git@host:",
            "git@host:/",
            "",
            "   ",
            "https://host/o/ r",
            "https://user:pw@host/o/r",
            "https://host:0/o/r",
            "https://host:99999/o/r",
            "https://-bad/o/r",
            "https://host/.git",
            "relative/path",
            "C:\\x\\y",
            "ssh://host",
        ];
        for input in cases {
            assert!(parse(input).is_err(), "{input:?} should be rejected");
        }
    }

    #[test]
    fn github_exact_match() {
        assert!(parse("https://github.com/o/r").unwrap().is_github());
        assert!(parse("git@github.com:o/r.git").unwrap().is_github());
        assert!(!parse("https://github.com.evil.io/o/r").unwrap().is_github());
        assert!(!parse("https://evilgithub.com/o/r").unwrap().is_github());
        assert!(!parse("https://gitlab.com/o/r").unwrap().is_github());
    }

    #[test]
    fn repo_name_derivation() {
        assert_eq!(derive_repo_name("r.git").unwrap(), "r");
        assert_eq!(derive_repo_name("r").unwrap(), "r");
        for bad in ["", ".", "..", ".git", "a/b", "a\\b"] {
            assert!(derive_repo_name(bad).is_err(), "{bad:?}");
        }
    }
}
