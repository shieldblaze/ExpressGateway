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
        .is_some_and(|v| {
            v.split(',')
                .any(|token| token.trim().eq_ignore_ascii_case("upgrade"))
        })
}

/// Check if `Upgrade` header contains `websocket` (case-insensitive).
fn has_websocket_header(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::UPGRADE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("websocket"))
}

/// Compute the `Sec-WebSocket-Accept` value from the client's
/// `Sec-WebSocket-Key`.
///
/// Per RFC 6455 section 4.2.2:
///   accept = base64(SHA-1(key + "258EAFA5-E914-47DA-95CA-5AB5DC76E45B"))
fn compute_accept_key(key: &str) -> String {
    use base64::Engine;

    let mut input = Vec::with_capacity(key.len() + WS_GUID.len());
    input.extend_from_slice(key.as_bytes());
    input.extend_from_slice(WS_GUID.as_bytes());

    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, &input);
    base64::engine::general_purpose::STANDARD.encode(digest.as_ref())
}

/// Extract the `Sec-WebSocket-Protocol` header value (subprotocol negotiation).
pub fn extract_subprotocols<T>(req: &Request<T>) -> Option<&str> {
    req.headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
}

/// Validate the `Origin` header against an allowed list.
///
/// Returns `true` if the origin is allowed, or if no allowed origins are
/// configured (permissive mode).
pub fn validate_origin<T>(req: &Request<T>, allowed_origins: &[&str]) -> bool {
    if allowed_origins.is_empty() {
        return true;
    }

    match req.headers().get("origin").and_then(|v| v.to_str().ok()) {
        Some(origin) => allowed_origins
            .iter()
            .any(|allowed| origin.eq_ignore_ascii_case(allowed)),
        None => false, // No origin header when origins are required = reject.
    }
}

/// Generate a 101 Switching Protocols response for a valid WebSocket upgrade
/// request.
///
/// Returns `None` if the request is not a valid WebSocket upgrade.
///
/// Optionally includes `Sec-WebSocket-Protocol` if a subprotocol was requested
/// and `selected_subprotocol` is provided.
pub fn upgrade_response<T>(
    req: &Request<T>,
    selected_subprotocol: Option<&str>,
) -> Option<Response<()>> {
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

    if let Some(proto) = selected_subprotocol {
        headers.insert(
            "sec-websocket-protocol",
            HeaderValue::from_str(proto).ok()?,
        );
    }

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
        let resp = upgrade_response(&req, None).expect("should generate upgrade response");
        assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
        assert_eq!(resp.headers().get("upgrade").unwrap(), "websocket");
        assert_eq!(resp.headers().get("connection").unwrap(), "Upgrade");
        // SHA1("dGhlIHNhbXBsZSBub25jZQ==" + GUID) = rLL8gRGGVz2uZqvj0lbBW/mR8E4=
        assert_eq!(
            resp.headers().get("sec-websocket-accept").unwrap(),
            "rLL8gRGGVz2uZqvj0lbBW/mR8E4="
        );
    }

    #[test]
    fn upgrade_response_with_subprotocol() {
        let req = make_upgrade_request();
        let resp = upgrade_response(&req, Some("graphql-ws"))
            .expect("should generate upgrade response");
        assert_eq!(
            resp.headers().get("sec-websocket-protocol").unwrap(),
            "graphql-ws"
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

    #[test]
    fn origin_validation_permissive() {
        let req = make_upgrade_request();
        assert!(validate_origin(&req, &[]));
    }

    #[test]
    fn origin_validation_allowed() {
        let req = Request::builder()
            .uri("/ws")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("Origin", "https://example.com")
            .body(())
            .unwrap();
        assert!(validate_origin(&req, &["https://example.com"]));
    }

    #[test]
    fn origin_validation_rejected() {
        let req = Request::builder()
            .uri("/ws")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
            .header("Origin", "https://evil.com")
            .body(())
            .unwrap();
        assert!(!validate_origin(&req, &["https://example.com"]));
    }

    #[test]
    fn origin_validation_missing_origin_with_requirements() {
        let req = make_upgrade_request();
        assert!(!validate_origin(&req, &["https://example.com"]));
    }

    #[test]
    fn extract_subprotocols_present() {
        let req = Request::builder()
            .uri("/ws")
            .header("Sec-WebSocket-Protocol", "graphql-ws, chat")
            .body(())
            .unwrap();
        assert_eq!(extract_subprotocols(&req), Some("graphql-ws, chat"));
    }

    #[test]
    fn extract_subprotocols_absent() {
        let req = Request::builder().uri("/ws").body(()).unwrap();
        assert_eq!(extract_subprotocols(&req), None);
    }
}
