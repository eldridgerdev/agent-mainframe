use std::time::Duration;
use ureq::config::Config;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

pub fn https_agent() -> ureq::Agent {
    https_config().new_agent()
}

/// For a probe that must answer quickly or not at all: `timeout` bounds the
/// whole call, and an HTTP error status is returned as a response rather than
/// an `Err`, so the caller can tell "nothing answered" from "something else
/// answered".
pub fn probe_agent(timeout: Duration) -> ureq::Agent {
    tls_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

fn https_config() -> Config {
    tls_builder()
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .build()
}

fn tls_builder() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    Config::builder().tls_config(
        TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .root_certs(RootCerts::PlatformVerifier)
            .build(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_agent_uses_rustls_with_platform_roots() {
        let config = https_config();
        let tls = config.tls_config();

        assert_eq!(tls.provider(), TlsProvider::Rustls);
        assert!(matches!(tls.root_certs(), RootCerts::PlatformVerifier));
    }
}
