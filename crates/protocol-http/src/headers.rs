//! Header manipulation: hop-by-hop stripping and custom injection.
//!
//! Implements RFC 9110 Section 7.6.1 (hop-by-hop header removal) and
//! RFC 9113 Section 8.2.2 (HTTP/2 connection-specific header fields),
//! plus injection of standard proxy headers (Via, X-Forwarded-For, etc.).

use std::net::IpAddr;

use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use uuid::Uuid;

/// Static set of hop-by-hop header names to strip (all lowercase).
const HOP_BY_HOP_NAMES: &[&str] = &[
    "connection",
    "keep-alive",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "proxy-connection",
    "proxy-authenticate",
    "proxy-authorization",
];

/// Strip hop-by-hop headers from a header map per RFC 9110 Section 7.6.1.
///
/// In addition to the static set, this function parses the `Connection` header
/// for dynamically listed hop-by-hop header names and removes those as well.
///
/// The `Upgrade` header is preserved when its value indicates a WebSocket
/// upgrade, to allow WebSocket protocol switching.
///
/// Uses zero-allocation comma scanning for the Connection header value.
pub fn strip_hop_by_hop(headers: &mut HeaderMap) {
    // First, collect dynamic hop-by-hop names from the Connection header.
    let mut dynamic_names: Vec<HeaderName> = Vec::new();
    if let Some(conn_val) = headers.get(header::CONNECTION)
        && let Ok(s) = conn_val.to_str()
    {
        // Zero-allocation comma scanning: iterate bytes for commas.
        for token in comma_split(s) {
            let trimmed = token.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(name) = HeaderName::from_bytes(trimmed.as_bytes()) {
                dynamic_names.push(name);
            }
        }
    }

    // Check if this is a WebSocket upgrade before stripping.
    let is_websocket = headers
        .get(header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"));

    // Remove static hop-by-hop headers.
    for &name in HOP_BY_HOP_NAMES {
        // Preserve Upgrade for WebSocket.
        if name == "upgrade" && is_websocket {
            continue;
        }
        headers.remove(name);
    }

    // Remove dynamic hop-by-hop headers.
    for name in &dynamic_names {
        if *name == header::UPGRADE && is_websocket {
            continue;
        }
        headers.remove(name);
    }
}

/// Inject standard proxy headers into a request header map.
///
/// Adds:
/// - `Via`: protocol version and proxy identifier
/// - `X-Request-ID`: UUID v4 (only if not already present)
/// - `X-Forwarded-For`: client IP address
/// - `X-Forwarded-Proto`: `http` or `https`
pub fn inject_proxy_headers(
    headers: &mut HeaderMap,
    client_ip: IpAddr,
    is_tls: bool,
    http_version: &str,
    proxy_name: &str,
) {
    // Via header: e.g. "1.1 expressgateway"
    let via_value = format!("{http_version} {proxy_name}");
    if let Ok(val) = HeaderValue::from_str(&via_value) {
        headers.append(HeaderName::from_static("via"), val);
    }

    // X-Request-ID: UUID v4 (only if not already present).
    let request_id_name = HeaderName::from_static("x-request-id");
    if !headers.contains_key(&request_id_name) {
        let uuid = Uuid::new_v4().to_string();
        if let Ok(val) = HeaderValue::from_str(&uuid) {
            headers.insert(request_id_name, val);
        }
    }

    // X-Forwarded-For: client IP.
    let xff_name = HeaderName::from_static("x-forwarded-for");
    let ip_str = client_ip.to_string();
    if let Ok(val) = HeaderValue::from_str(&ip_str) {
        headers.append(xff_name, val);
    }

    // X-Forwarded-Proto: http or https.
    let proto = if is_tls { "https" } else { "http" };
    if let Ok(val) = HeaderValue::from_str(proto) {
        headers.insert(HeaderName::from_static("x-forwarded-proto"), val);
    }
}

/// Zero-allocation comma-separated value scanner.
///
/// Returns an iterator over comma-delimited tokens in the given string.
fn comma_split(s: &str) -> CommaSplit<'_> {
    CommaSplit { remaining: s }
}

/// Iterator that splits a string on commas without allocating.
struct CommaSplit<'a> {
    remaining: &'a str,
}

impl<'a> Iterator for CommaSplit<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining.is_empty() {
            return None;
        }
        match self.remaining.find(',') {
            Some(idx) => {
                let token = &self.remaining[..idx];
                self.remaining = &self.remaining[idx + 1..];
                Some(token)
            }
            None => {
                let token = self.remaining;
                self.remaining = "";
                Some(token)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_static_hop_by_hop_headers() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
        headers.insert(
            HeaderName::from_static("keep-alive"),
            HeaderValue::from_static("timeout=5"),
        );
        headers.insert(header::TE, HeaderValue::from_static("trailers"));
        headers.insert(
            header::TRANSFER_ENCODING,
            HeaderValue::from_static("chunked"),
        );
        headers.insert(
            HeaderName::from_static("proxy-connection"),
            HeaderValue::from_static("keep-alive"),
        );
        headers.insert(header::HOST, HeaderValue::from_static("example.com"));

        strip_hop_by_hop(&mut headers);

        assert!(!headers.contains_key(header::CONNECTION));
        assert!(!headers.contains_key("keep-alive"));
        assert!(!headers.contains_key(header::TE));
        assert!(!headers.contains_key(header::TRANSFER_ENCODING));
        assert!(!headers.contains_key("proxy-connection"));
        // Host should be preserved.
        assert!(headers.contains_key(header::HOST));
    }

    #[test]
    fn strips_dynamic_hop_by_hop_from_connection_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::CONNECTION,
            HeaderValue::from_static("X-Custom, X-Other"),
        );
        headers.insert(
            HeaderName::from_static("x-custom"),
            HeaderValue::from_static("val1"),
        );
        headers.insert(
            HeaderName::from_static("x-other"),
            HeaderValue::from_static("val2"),
        );
        headers.insert(
            HeaderName::from_static("x-keep"),
            HeaderValue::from_static("val3"),
        );

        strip_hop_by_hop(&mut headers);

        assert!(!headers.contains_key("x-custom"));
        assert!(!headers.contains_key("x-other"));
        assert!(headers.contains_key("x-keep"));
    }

    #[test]
    fn preserves_upgrade_for_websocket() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("websocket"));

        strip_hop_by_hop(&mut headers);

        // Upgrade should be preserved for WebSocket.
        assert!(headers.contains_key(header::UPGRADE));
        // Connection itself is still stripped.
        assert!(!headers.contains_key(header::CONNECTION));
    }

    #[test]
    fn strips_upgrade_for_non_websocket() {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONNECTION, HeaderValue::from_static("Upgrade"));
        headers.insert(header::UPGRADE, HeaderValue::from_static("h2c"));

        strip_hop_by_hop(&mut headers);

        assert!(!headers.contains_key(header::UPGRADE));
        assert!(!headers.contains_key(header::CONNECTION));
    }

    #[test]
    fn inject_proxy_headers_adds_all() {
        let mut headers = HeaderMap::new();
        let client_ip: IpAddr = "10.0.0.1".parse().unwrap();

        inject_proxy_headers(&mut headers, client_ip, true, "1.1", "expressgateway");

        assert!(headers.contains_key("via"));
        assert_eq!(
            headers.get("via").unwrap().to_str().unwrap(),
            "1.1 expressgateway"
        );

        assert!(headers.contains_key("x-request-id"));
        // Validate it is a UUID.
        let rid = headers.get("x-request-id").unwrap().to_str().unwrap();
        assert!(Uuid::parse_str(rid).is_ok());

        assert!(headers.contains_key("x-forwarded-for"));
        assert_eq!(
            headers.get("x-forwarded-for").unwrap().to_str().unwrap(),
            "10.0.0.1"
        );

        assert!(headers.contains_key("x-forwarded-proto"));
        assert_eq!(
            headers.get("x-forwarded-proto").unwrap().to_str().unwrap(),
            "https"
        );
    }

    #[test]
    fn inject_proxy_headers_http_proto() {
        let mut headers = HeaderMap::new();
        let client_ip: IpAddr = "192.168.1.1".parse().unwrap();

        inject_proxy_headers(&mut headers, client_ip, false, "2", "eg");

        assert_eq!(
            headers.get("x-forwarded-proto").unwrap().to_str().unwrap(),
            "http"
        );
        assert_eq!(headers.get("via").unwrap().to_str().unwrap(), "2 eg");
    }

    #[test]
    fn inject_does_not_overwrite_existing_request_id() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-request-id"),
            HeaderValue::from_static("existing-id"),
        );

        let client_ip: IpAddr = "10.0.0.1".parse().unwrap();
        inject_proxy_headers(&mut headers, client_ip, false, "1.1", "proxy");

        assert_eq!(
            headers.get("x-request-id").unwrap().to_str().unwrap(),
            "existing-id"
        );
    }

    #[test]
    fn comma_split_basic() {
        let tokens: Vec<&str> = comma_split("a, b, c").collect();
        assert_eq!(tokens, vec!["a", " b", " c"]);
    }

    #[test]
    fn comma_split_single() {
        let tokens: Vec<&str> = comma_split("keep-alive").collect();
        assert_eq!(tokens, vec!["keep-alive"]);
    }

    #[test]
    fn comma_split_empty() {
        let tokens: Vec<&str> = comma_split("").collect();
        assert!(tokens.is_empty());
    }
}
