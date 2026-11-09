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

/// What a guarded request got back.
#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    /// The start of the body, as text (at most [`EXCERPT`] bytes).
    pub excerpt: String,
    /// Where the request ended up, after redirects.
    pub url: String,
}

/// How much of a response body is kept.
pub const EXCERPT: usize = 2048;
const MAX_REDIRECTS: usize = 3;
const READ_LIMIT: usize = 64 * 1024;

/// A request to a URL a user typed in (a dashboard cron job), so it is *guarded*: only http and
/// https, every address the name resolves to must be public (no loopback, private, link-local or
/// metadata address), the connection goes to the addresses just checked, redirects are followed
/// by hand and checked the same way (a redirect to another host drops the headers), and the
/// whole thing is bounded in time and in the size read.
pub async fn guarded(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
    timeout: Duration,
) -> Result<Reply, String> {
    request(method, url, headers, body, timeout, false).await
}

/// Resolves a URL's host the way [`guarded`] would and refuses it when it isn't public; nothing
/// is sent. For checking a job as it is being made.
pub async fn check_target(url: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(url).map_err(|_| "Not a valid URL.".to_string())?;
    target(&url, false).await.map(|_| ())
}

/// The reasons a request is refused before anything is sent; retrying can't change them.
const REFUSALS: &[&str] = &[
    "Only http and https",
    "Leave the user name",
    "The URL has no host",
    "The URL has no port",
    "is not a public address",
    "Not a valid URL",
    "Unknown method",
];

/// Whether an error from [`guarded`] is a refusal (a bad or forbidden address) rather than a
/// failure of the call itself (a timeout, a refused connection, a DNS hiccup).
pub fn refused(message: &str) -> bool {
    REFUSALS.iter().any(|r| message.contains(r))
}

async fn target(
    url: &reqwest::Url,
    allow_private: bool,
) -> Result<Vec<std::net::SocketAddr>, String> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err("Only http and https URLs are allowed.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Leave the user name and password out of the URL; use a header.".into());
    }
    let host = url.host_str().ok_or("The URL has no host.")?.to_string();
    let port = url.port_or_known_default().ok_or("The URL has no port.")?;
    let addrs: Vec<std::net::SocketAddr> =
        tokio::net::lookup_host((host.trim_matches(['[', ']']), port))
            .await
            .map_err(|_| format!("Can't find the host {host}."))?
            .collect();
    if addrs.is_empty() {
        return Err(format!("Can't find the host {host}."));
    }
    if !allow_private && !addrs.iter().all(|a| crate::ip::is_public(a.ip())) {
        return Err(format!(
            "{host} is not a public address; calls to this server or to private networks are not allowed."
        ));
    }
    Ok(addrs)
}

async fn request(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
    timeout: Duration,
    allow_private: bool,
) -> Result<Reply, String> {
    let mut method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| format!("Unknown method {method}."))?;
    let mut url = reqwest::Url::parse(url).map_err(|_| "Not a valid URL.".to_string())?;
    let mut body = body.map(str::to_string);
    let headers = headers.to_vec();
    let first_host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let deadline = tokio::time::Instant::now() + timeout;
    for hop in 0..=MAX_REDIRECTS {
        // (only the first hop may be private, and only for the tests)
        let addrs = target(&url, allow_private && hop == 0).await?;
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        let client = builder()
            .resolve_to_addrs(&host, &addrs)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| e.to_string())?;
        let mut req = client
            .request(method.clone(), url.clone())
            .timeout(deadline.saturating_duration_since(tokio::time::Instant::now()));
        if host == first_host {
            for (k, v) in &headers {
                req = req.header(k, v);
            }
        }
        if let Some(b) = &body {
            req = req.body(b.clone());
        }
        let response = req
            .send()
            .await
            .map_err(|e| format!("The request failed: {}.", short(e)))?;
        if response.status().is_redirection() {
            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|l| l.to_str().ok())
            else {
                return Err("A redirect without a place to go.".into());
            };
            url = url
                .join(location)
                .map_err(|_| "A redirect to an invalid URL.".to_string())?;
            let status = response.status().as_u16();
            if status == 303 || ((status == 301 || status == 302) && method != reqwest::Method::GET)
            {
                method = reqwest::Method::GET;
                body = None;
            }
            continue;
        }
        let status = response.status().as_u16();
        let mut response = response;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(short)? {
            if bytes.len() < READ_LIMIT {
                bytes.extend_from_slice(&chunk);
            } else {
                break;
            }
        }
        bytes.truncate(EXCERPT);
        return Ok(Reply {
            status,
            excerpt: String::from_utf8_lossy(&bytes).into_owned(),
            url: url.to_string(),
        });
    }
    Err("Too many redirects.".into())
}

/// The cause of a reqwest error without the URL (which can hold a secret in its query).
fn short(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        text = s.to_string();
        source = s.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A one-shot server answering with `reply`; returns its port and what it was sent.
    async fn serve(reply: &'static str) -> (u16, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = vec![0u8; 8192];
            let n = sock.read(&mut buf).await.unwrap();
            sock.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        (port, task)
    }

    #[tokio::test]
    async fn refusals_are_told_from_failures_of_the_call() {
        for url in [
            "http://127.0.0.1/",
            "ftp://example.com/",
            "http://u:p@example.com/",
            "nope",
        ] {
            let err = guarded("GET", url, &[], None, Duration::from_secs(5))
                .await
                .unwrap_err();
            assert!(refused(&err), "{url}: {err}");
        }
        assert!(!refused("The request failed: connection refused."));
        assert!(!refused("Looking up example.com took too long."));
    }

    #[tokio::test]
    async fn the_guard_refuses_private_and_odd_addresses() {
        for url in [
            "http://127.0.0.1/",
            "http://localhost:8090/",
            "http://10.0.0.5/x",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::1]/",
            "http://0.0.0.0/",
            "ftp://example.com/",
            "file:///etc/passwd",
            "http://user:pass@example.com/",
        ] {
            let got = guarded("GET", url, &[], None, Duration::from_secs(5)).await;
            assert!(got.is_err(), "{url} must be refused, got {got:?}");
        }
        assert!(check_target("http://127.0.0.1/").await.is_err());
    }

    #[tokio::test]
    async fn a_request_sends_the_method_headers_and_body_and_reads_the_answer() {
        let (port, seen) = serve("HTTP/1.1 202 Accepted\r\ncontent-length: 5\r\n\r\nhello").await;
        let got = request(
            "POST",
            &format!("http://127.0.0.1:{port}/hook?a=1"),
            &[("x-token".into(), "s3cret".into())],
            Some("{\"n\":1}"),
            Duration::from_secs(5),
            true,
        )
        .await
        .unwrap();
        assert_eq!((got.status, got.excerpt.as_str()), (202, "hello"));
        let sent = seen.await.unwrap().to_ascii_lowercase();
        assert!(sent.starts_with("post /hook?a=1 "), "{sent}");
        assert!(sent.contains("x-token: s3cret"));
        assert!(sent.ends_with("{\"n\":1}"));
    }

    #[tokio::test]
    async fn a_redirect_to_a_private_address_is_refused() {
        // the first hop is allowed here only so the test can reach it; the second is guarded
        let (port, _seen) = serve(
            "HTTP/1.1 302 Found\r\nlocation: http://169.254.169.254/\r\ncontent-length: 0\r\n\r\n",
        )
        .await;
        let got = request(
            "GET",
            &format!("http://127.0.0.1:{port}/"),
            &[],
            None,
            Duration::from_secs(5),
            true,
        )
        .await;
        assert!(got.unwrap_err().contains("not a public address"));
    }
}
