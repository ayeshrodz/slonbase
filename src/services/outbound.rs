//! HTTP requests to addresses a user typed in (a job's URL, a webhook).
//!
//! Every request goes through the guard of [`crate::http`]: http and https only, public
//! addresses only (checked for every address a name resolves to, and connected to as checked),
//! redirects followed by hand and checked again, the caller's headers sent to the origin they
//! were meant for only, and time and size bounded.

use std::time::Duration;

pub use crate::http::Reply;

use super::Failure;

/// A request to make.
#[derive(Debug, Clone, Copy)]
pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    pub body: Option<&'a str>,
    pub timeout: Duration,
}

/// Whether `url` could be called: resolves its host and refuses what isn't public. Nothing is
/// sent. For checking a URL when it is saved.
pub async fn check(url: &str) -> Result<(), String> {
    crate::http::check_target(url).await
}

/// Makes the request. A refusal (a bad or forbidden address) is permanent; a failure of the
/// call itself (a timeout, a refused connection, a DNS hiccup) may pass.
pub async fn send(request: Request<'_>) -> Result<Reply, Failure> {
    crate::http::guarded(
        request.method,
        request.url,
        request.headers,
        request.body,
        request.timeout,
    )
    .await
    .map_err(|message| Failure {
        retry: !crate::http::refused(&message),
        message,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_refused_address_is_permanent() {
        let failure = send(Request {
            method: "GET",
            url: "http://127.0.0.1/",
            headers: &[],
            body: None,
            timeout: Duration::from_secs(5),
        })
        .await
        .unwrap_err();
        assert!(!failure.retry, "{failure}");
        assert!(check("http://169.254.169.254/").await.is_err());
    }
}
