//! WebSocket upgrade detection and response generation (RFC 6455).
//!
//! For HTTP/1.1, a WebSocket upgrade requires:
//! - `Connection: Upgrade`
//! - `Upgrade: websocket`
//! - `Sec-WebSocket-Key` header
//! - `Sec-WebSocket-Version: 13`

use http::{HeaderMap, HeaderValue, Request, Response, StatusCode};

/// The WebSocket GUID used to compute `Sec-WebSocket-Accept` (RFC 6455 sec 4.2.2).
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-5AB5DC76E45B";

/// Returns `true` if the HTTP/1.1 request headers indicate a WebSocket upgrade.
pub fn is_websocket_upgrade<T>(req: &Request<T>) -> bool {
    let headers = req.headers();
    has_upgrade_header(headers)
        && has_websocket_header(headers)
        && headers.contains_key("sec-websocket-key")
}

/// Check if `Connection` header contains `upgrade` (case-insensitive).
fn has_upgrade_header(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::CONNECTION)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
        .unwrap_or(false)
}

/// Check if `Upgrade` header contains `websocket` (case-insensitive).
fn has_websocket_header(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().eq_ignore_ascii_case("websocket"))
        .unwrap_or(false)
}

/// Compute the `Sec-WebSocket-Accept` value from the client's
/// `Sec-WebSocket-Key`.
fn compute_accept_key(key: &str) -> String {
    use std::io::Write;

    let mut hasher = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    let mut buf = Vec::with_capacity(key.len() + WS_GUID.len());
    write!(&mut buf, "{}{}", key, WS_GUID).unwrap();
    hasher.update(&buf);
    let digest = hasher.finish();

    base64_encode(digest.as_ref())
}

/// Simple base64 encoding (standard alphabet, with padding).
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;

        result.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        result.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[(triple & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Generate a 101 Switching Protocols response for a valid WebSocket upgrade
/// request.
///
/// Returns `None` if the request is not a valid WebSocket upgrade.
pub fn upgrade_response<T>(req: &Request<T>) -> Option<Response<()>> {
    if !is_websocket_upgrade(req) {
        return None;
    }

    let key = req.headers().get("sec-websocket-key")?.to_str().ok()?;

    let accept = compute_accept_key(key);

    let mut resp = Response::builder()
        .status(StatusCode::SWITCHING_PROTOCOLS)
        .body(())
        .ok()?;

    let headers = resp.headers_mut();
    headers.insert(http::header::UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(
        http::header::CONNECTION,
        HeaderValue::from_static("Upgrade"),
    );
    headers.insert("sec-websocket-accept", HeaderValue::from_str(&accept).ok()?);

    Some(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_upgrade_request() -> Request<()> {
        Request::builder()
            .uri("/ws")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("Sec-WebSocket-Version", "13")
            .body(())
            .unwrap()
    }

    #[test]
    fn detects_valid_upgrade() {
        let req = make_upgrade_request();
        assert!(is_websocket_upgrade(&req));
    }

    #[test]
    fn rejects_missing_connection() {
        let req = Request::builder()
            .uri("/ws")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn rejects_missing_upgrade() {
        let req = Request::builder()
            .uri("/ws")
            .header("Connection", "Upgrade")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn rejects_missing_key() {
        let req = Request::builder()
            .uri("/ws")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .body(())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn rejects_plain_http() {
        let req = Request::builder()
            .uri("/api")
            .header("Content-Type", "application/json")
            .body(())
            .unwrap();
        assert!(!is_websocket_upgrade(&req));
    }

    #[test]
    fn upgrade_response_correct() {
        let req = make_upgrade_request();
        let resp = upgrade_response(&req).expect("should generate upgrade response");
        assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(resp.headers().get("upgrade").unwrap(), "websocket");
        assert_eq!(resp.headers().get("connection").unwrap(), "Upgrade");
        // SHA1(dGhlIHNhbXBsZSBub25jZQ== + GUID) base64-encoded
        assert_eq!(
            resp.headers().get("sec-websocket-accept").unwrap(),
            "rLL8gRGGVz2uZqvj0lbBW/mR8E4="
        );
    }

    #[test]
    fn case_insensitive_upgrade_header() {
        let req = Request::builder()
            .uri("/ws")
            .header("Connection", "upgrade")
            .header("Upgrade", "WebSocket")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .body(())
            .unwrap();
        assert!(is_websocket_upgrade(&req));
    }
}
