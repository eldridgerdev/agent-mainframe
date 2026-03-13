use ureq::config::Config;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

pub fn https_agent() -> ureq::Agent {
    https_config().new_agent()
}

fn https_config() -> Config {
    Config::builder()
        .tls_config(
            TlsConfig::builder()
                .provider(TlsProvider::Rustls)
                .root_certs(RootCerts::PlatformVerifier)
                .build(),
        )
        .build()
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
