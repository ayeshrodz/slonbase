//! The outbound HTTP client (OAuth2 providers, S3), with rustls, ring and the Mozilla
//! roots like the database and SMTP connections, so no OpenSSL or system roots are needed.

use std::sync::{Arc, LazyLock};
use std::time::Duration;

/// A shared client. It has connect and read timeouts but no total timeout, so large
/// downloads can stream; set a total timeout per request where it fits.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
        reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(Duration::from_secs(60))
            .user_agent(concat!("slonbase/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("valid HTTP client configuration")
    });
    &CLIENT
}
