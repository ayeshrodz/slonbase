//! Client IP addresses: the connection's address, and the address a trusted proxy reports
//! in a header (the settings' `trustedProxy`), and IP ranges (`10.0.0.0/8`).

use std::net::{IpAddr, SocketAddr};

use axum::extract::ConnectInfo;
use axum::http::{Extensions, HeaderMap};

use crate::settings::TrustedProxy;

/// The address of the connection (unknown in tests that don't go through a socket).
pub fn remote_ip(extensions: &Extensions) -> Option<IpAddr> {
    extensions
        .get::<ConnectInfo<SocketAddr>>()
        .map(|info| info.0.ip())
}

fn parse_ip(raw: &str) -> Option<IpAddr> {
    let raw = raw.trim().trim_matches('"');
    if let Ok(ip) = raw.parse() {
        return Some(ip);
    }
    // `1.2.3.4:5678`, `[::1]:5678`
    raw.parse::<SocketAddr>().ok().map(|a| a.ip())
}

/// The client's address: from the first trusted proxy header that has one, else the
/// connection's. Lists (`X-Forwarded-For: client, proxy1`) use the rightmost address,
/// or the leftmost with `useLeftmostIP` (only safe when the proxy overwrites the header).
pub fn user_ip(
    headers: &HeaderMap,
    remote: Option<IpAddr>,
    proxy: &TrustedProxy,
) -> Option<IpAddr> {
    for name in &proxy.headers {
        let Some(value) = headers.get(name.trim()).and_then(|v| v.to_str().ok()) else {
            continue;
        };
        let mut parts = value.split(',').map(str::trim).filter(|p| !p.is_empty());
        let picked = if proxy.use_leftmost_ip {
            parts.next()
        } else {
            parts.next_back()
        };
        if let Some(ip) = picked.and_then(parse_ip) {
            return Some(ip);
        }
    }
    remote
}

/// The client of a request: its connection address, headers and user agent.
#[derive(Debug, Clone, Default)]
pub struct Client {
    pub remote: Option<IpAddr>,
    pub headers: HeaderMap,
}

impl Client {
    /// The client's address (see [`user_ip`]).
    pub fn ip(&self, proxy: &TrustedProxy) -> Option<IpAddr> {
        user_ip(&self.headers, self.remote, proxy)
    }

    pub fn user_agent(&self) -> &str {
        self.headers
            .get(axum::http::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for Client {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _: &S,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self {
            remote: remote_ip(&parts.extensions),
            headers: parts.headers.clone(),
        })
    }
}

/// An IP address or a CIDR range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    addr: IpAddr,
    bits: u8,
}

impl Range {
    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        let (ip, bits) = match raw.split_once('/') {
            Some((ip, bits)) => (ip, Some(bits)),
            None => (raw, None),
        };
        let addr: IpAddr = ip.parse().ok()?;
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let bits = match bits {
            Some(b) => b.parse::<u8>().ok().filter(|b| *b <= max)?,
            None => max,
        };
        Some(Self { addr, bits })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        let ip = match (self.addr, ip) {
            // an IPv4-mapped IPv6 address matches IPv4 ranges
            (IpAddr::V4(_), IpAddr::V6(v6)) => match v6.to_ipv4_mapped() {
                Some(v4) => IpAddr::V4(v4),
                None => return false,
            },
            (_, ip) => ip,
        };
        match (self.addr, ip) {
            (IpAddr::V4(a), IpAddr::V4(b)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.bits)).unwrap_or(0);
                u32::from(a) & mask == u32::from(b) & mask
            }
            (IpAddr::V6(a), IpAddr::V6(b)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.bits))
                    .unwrap_or(0);
                u128::from(a) & mask == u128::from(b) & mask
            }
            _ => false,
        }
    }
}

/// Whether `ip` is in any of the ranges (invalid entries are ignored).
pub fn in_any(ranges: &[String], ip: IpAddr) -> bool {
    ranges
        .iter()
        .filter_map(|r| Range::parse(r))
        .any(|r| r.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn ranges() {
        let r = Range::parse("10.0.0.0/8").unwrap();
        assert!(r.contains("10.1.2.3".parse().unwrap()));
        assert!(!r.contains("11.0.0.1".parse().unwrap()));
        assert!(r.contains("::ffff:10.0.0.1".parse().unwrap()));
        let one = Range::parse("192.168.1.5").unwrap();
        assert!(one.contains("192.168.1.5".parse().unwrap()));
        assert!(!one.contains("192.168.1.6".parse().unwrap()));
        let v6 = Range::parse("fd00::/8").unwrap();
        assert!(v6.contains("fd12::1".parse().unwrap()));
        assert!(!v6.contains("fe80::1".parse().unwrap()));
        assert!(
            Range::parse("0.0.0.0/0")
                .unwrap()
                .contains("8.8.8.8".parse().unwrap())
        );
        for bad in ["", "10.0.0.0/33", "nope", "::/129"] {
            assert!(Range::parse(bad).is_none(), "{bad}");
        }
        assert!(in_any(
            &["bad".into(), "127.0.0.0/8".into()],
            "127.0.0.1".parse().unwrap()
        ));
    }

    #[test]
    fn proxy_headers() {
        let remote: Option<IpAddr> = Some("10.0.0.1".parse().unwrap());
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("203.0.113.9, 198.51.100.7"),
        );
        let none = TrustedProxy::default();
        assert_eq!(
            user_ip(&headers, remote, &none),
            remote,
            "untrusted headers are ignored"
        );
        let mut proxy = TrustedProxy {
            headers: vec!["X-Forwarded-For".into()],
            use_leftmost_ip: false,
        };
        assert_eq!(
            user_ip(&headers, remote, &proxy),
            Some("198.51.100.7".parse().unwrap())
        );
        proxy.use_leftmost_ip = true;
        assert_eq!(
            user_ip(&headers, remote, &proxy),
            Some("203.0.113.9".parse().unwrap())
        );
        headers.insert("x-forwarded-for", HeaderValue::from_static("garbage"));
        assert_eq!(user_ip(&headers, remote, &proxy), remote);
        headers.insert("x-real-ip", HeaderValue::from_static("[2001:db8::1]:443"));
        proxy.headers = vec!["X-Real-IP".into()];
        assert_eq!(
            user_ip(&headers, remote, &proxy),
            Some("2001:db8::1".parse().unwrap())
        );
    }
}
