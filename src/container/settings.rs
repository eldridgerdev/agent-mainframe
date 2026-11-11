//! User-facing container settings (`projects.container` in the app config).

use serde::{Deserialize, Serialize};

/// Image AMF builds itself when the configured image is this one.
pub const DEFAULT_IMAGE: &str = "amf-sandbox:latest";

/// Hosts the sandbox may reach through the egress proxy by default: the agent
/// APIs plus the usual package and source hosts. Everything else is denied.
pub const DEFAULT_EGRESS_ALLOWLIST: &[&str] = &[
    "api.anthropic.com",
    "statsig.anthropic.com",
    "sentry.io",
    "api.openai.com",
    "chatgpt.com",
    "github.com",
    "codeload.github.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "registry.npmjs.org",
    "pypi.org",
    "files.pythonhosted.org",
    "crates.io",
    "static.crates.io",
    "index.crates.io",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ContainerSettings {
    /// Docker OCI runtime. `runc` is plain Docker (shared host kernel);
    /// `runsc` (gVisor) or `kata` put a user-space kernel or a hypervisor
    /// between the repo's code and the host. The runtime must already be
    /// registered with the Docker daemon.
    pub runtime: String,
    pub image: String,
    /// Route the sandbox's network through an allowlisting proxy and give it
    /// no other way out. Turning this off gives the sandbox full network
    /// access.
    pub egress_allowlist_enabled: bool,
    /// Hostnames, or `*.example.com` for a domain and its subdomains.
    pub egress_allowlist: Vec<String>,
    /// `docker run --memory`, e.g. `4g`.
    pub memory: String,
    pub pids_limit: u32,
}

impl Default for ContainerSettings {
    fn default() -> Self {
        Self {
            runtime: "runc".into(),
            image: DEFAULT_IMAGE.into(),
            egress_allowlist_enabled: true,
            egress_allowlist: DEFAULT_EGRESS_ALLOWLIST
                .iter()
                .map(|s| s.to_string())
                .collect(),
            memory: "4g".into(),
            pids_limit: 512,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    Runtime(String),
    Image(String),
    AllowlistEntry(String),
    Memory(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Runtime(r) => {
                write!(
                    f,
                    "container runtime '{r}' is not a valid Docker runtime name"
                )
            }
            Self::Image(i) => write!(f, "container image '{i}' is not a valid image reference"),
            Self::AllowlistEntry(e) => write!(
                f,
                "egress allowlist entry '{e}' is not a hostname (use host.example.com or *.example.com)"
            ),
            Self::Memory(m) => {
                write!(
                    f,
                    "container memory limit '{m}' is not valid (use e.g. 4g or 512m)"
                )
            }
        }
    }
}

impl std::error::Error for SettingsError {}

fn simple_token(s: &str, extra: &[char]) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
}

/// A hostname or `*.hostname`. Rejects anything that could change meaning in a
/// proxy filter pattern.
pub fn valid_allowlist_entry(entry: &str) -> bool {
    let host = entry.strip_prefix("*.").unwrap_or(entry);
    !host.is_empty()
        && host.len() <= 253
        && host.contains('.')
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

impl ContainerSettings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if !simple_token(&self.runtime, &['-', '_', '.']) {
            return Err(SettingsError::Runtime(self.runtime.clone()));
        }
        if !simple_token(&self.image, &['-', '_', '.', ':', '/', '@']) {
            return Err(SettingsError::Image(self.image.clone()));
        }
        let mem = self
            .memory
            .trim_end_matches(['k', 'm', 'g', 'K', 'M', 'G', 'b', 'B']);
        if mem.is_empty() || !mem.chars().all(|c| c.is_ascii_digit()) {
            return Err(SettingsError::Memory(self.memory.clone()));
        }
        for entry in &self.egress_allowlist {
            if !valid_allowlist_entry(entry) {
                return Err(SettingsError::AllowlistEntry(entry.clone()));
            }
        }
        Ok(())
    }

    pub fn uses_default_image(&self) -> bool {
        self.image == DEFAULT_IMAGE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_validate() {
        ContainerSettings::default().validate().unwrap();
    }

    #[test]
    fn rejects_flag_like_runtime_and_image() {
        let mut s = ContainerSettings {
            runtime: "--privileged".into(),
            ..Default::default()
        };
        assert!(matches!(s.validate(), Err(SettingsError::Runtime(_))));
        s.runtime = "runsc".into();
        s.image = "-v".into();
        assert!(matches!(s.validate(), Err(SettingsError::Image(_))));
    }

    #[test]
    fn allowlist_entries_must_be_hostnames() {
        for ok in ["github.com", "*.github.com", "a-b.example.co.uk"] {
            assert!(valid_allowlist_entry(ok), "{ok}");
        }
        for bad in [
            "*",
            "localhost",
            "*.",
            "a..b",
            "-a.com",
            "a.com/path",
            "a.com:443",
            "(a|b).com",
            "a.*.com",
            "a b.com",
            "",
        ] {
            assert!(!valid_allowlist_entry(bad), "{bad}");
        }
    }

    #[test]
    fn memory_limit_shape() {
        let mut s = ContainerSettings::default();
        for ok in ["4g", "512m", "1024"] {
            s.memory = ok.into();
            assert!(s.validate().is_ok(), "{ok}");
        }
        for bad in ["", "g", "four", "4 g", "-1g"] {
            s.memory = bad.into();
            assert!(s.validate().is_err(), "{bad}");
        }
    }

    #[test]
    fn partial_config_fills_defaults() {
        let s: ContainerSettings = serde_json::from_str(r#"{"runtime":"runsc"}"#).unwrap();
        assert_eq!(s.runtime, "runsc");
        assert!(s.egress_allowlist_enabled);
        assert!(!s.egress_allowlist.is_empty());
    }
}
