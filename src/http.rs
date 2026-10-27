//! The outbound HTTP client (OAuth2 providers, S3), with rustls, ring and the Mozilla
//! roots like the database and SMTP connections, so no OpenSSL or system roots are needed.

use std::path::Path;
use std::sync::{Arc, LazyLock, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

/// Extra CA certificates (`server.http_ca_file`), set once at startup.
static EXTRA_ROOTS: OnceLock<Vec<CertificateDer<'static>>> = OnceLock::new();

/// Trusts the certificates of a PEM file for outbound HTTPS, besides the Mozilla roots. Call it
/// before the first request; it applies to the whole process, and only the first call counts.
pub fn trust(ca_file: Option<&Path>) -> Result<()> {
    let Some(path) = ca_file else {
        return Ok(());
    };
    let mut certs = Vec::new();
    for cert in CertificateDer::pem_file_iter(path)
        .with_context(|| format!("failed to read the HTTP CA file {}", path.display()))?
    {
        certs.push(cert.with_context(|| format!("invalid HTTP CA file {}", path.display()))?);
    }
    if certs.is_empty() {
        anyhow::bail!("the HTTP CA file {} has no certificates", path.display());
    }
    let mut check = rustls::RootCertStore::empty();
    for cert in &certs {
        check
            .add(cert.clone())
            .with_context(|| format!("invalid certificate in {}", path.display()))?;
    }
    if EXTRA_ROOTS.set(certs).is_err() {
        tracing::debug!("the HTTP CA file was already loaded");
    }
    Ok(())
}

/// A shared client. It has connect and read timeouts but no total timeout, so large
/// downloads can stream; set a total timeout per request where it fits.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> =
        LazyLock::new(|| builder().build().expect("valid HTTP client configuration"));
    &CLIENT
}

/// The shared client's configuration, for a client of its own (e.g. pinned addresses).
pub fn builder() -> reqwest::ClientBuilder {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    for cert in EXTRA_ROOTS.get().into_iter().flatten() {
        // checked in `trust`
        let _ = roots.add(cert.clone());
    }
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
}
