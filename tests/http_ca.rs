//! `server.http_ca_file`: outbound HTTPS trusts the extra CAs. Its own test binary, since the
//! CAs are process-wide.

use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tls");

/// An HTTPS server with a certificate from the test CA, answering every request with "ok".
async fn serve() -> String {
    let certs: Vec<CertificateDer> =
        CertificateDer::pem_file_iter(format!("{FIXTURES}/server.pem"))
            .unwrap()
            .map(Result::unwrap)
            .collect();
    let key = PrivateKeyDer::from_pem_file(format!("{FIXTURES}/server.key")).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(socket).await else {
                    return;
                };
                let mut buf = [0u8; 2048];
                let _ = tls.read(&mut buf).await;
                let _ = tls
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
                    )
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    format!("https://localhost:{port}/")
}

#[tokio::test]
async fn extra_cas_are_trusted() {
    let url = serve().await;
    // not without the CA
    let before = slonbase::http::builder().build().unwrap();
    assert!(before.get(&url).send().await.is_err());

    // a file without certificates, or a missing one, is an error
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty.pem");
    std::fs::write(&empty, "nothing here\n").unwrap();
    assert!(slonbase::http::trust(Some(&empty)).is_err());
    assert!(slonbase::http::trust(Some(&dir.path().join("missing.pem"))).is_err());

    slonbase::http::trust(Some(std::path::Path::new(&format!("{FIXTURES}/ca.pem")))).unwrap();
    let after = slonbase::http::builder().build().unwrap();
    let response = after.get(&url).send().await.unwrap();
    assert_eq!(response.text().await.unwrap(), "ok");
    // the shared client too (S3, OAuth2 providers)
    let response = slonbase::http::client().get(&url).send().await.unwrap();
    assert_eq!(response.status(), 200);
}
