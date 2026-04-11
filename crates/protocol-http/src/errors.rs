//! Error response generation for HTTP error codes.
//!
//! Maps [`expressgateway_core::Error`] variants to well-formed HTTP responses with
//! appropriate status codes: 400, 413, 414, 431, 502, 503, 504.

use bytes::Bytes;
use http::{Response, StatusCode};
use http_body_util::Full;

/// HTTP error with a status code and human-readable message.
#[derive(Debug, Clone)]
pub struct HttpError {
    /// HTTP status code.
    pub status: StatusCode,
    /// Human-readable error message included in the response body.
    pub message: String,
}

impl HttpError {
    /// Create a new HTTP error.
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// 400 Bad Request: malformed HTTP.
    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    /// 413 Content Too Large.
    pub fn content_too_large(size: u64, max: u64) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Request body too large: {size} exceeds limit of {max}"),
        )
    }

    /// 414 URI Too Long.
    pub fn uri_too_long(length: usize, max: usize) -> Self {
        Self::new(
            StatusCode::URI_TOO_LONG,
            format!("URI too long: {length} exceeds limit of {max}"),
        )
    }

    /// 431 Request Header Fields Too Large.
    pub fn header_too_large(size: usize, max: usize) -> Self {
        Self::new(
            StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE,
            format!("Request headers too large: {size} exceeds limit of {max}"),
        )
    }

    /// 502 Bad Gateway.
    pub fn bad_gateway(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, detail)
    }

    /// 503 Service Unavailable.
    pub fn service_unavailable(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, detail)
    }

    /// 504 Gateway Timeout.
    pub fn gateway_timeout() -> Self {
        Self::new(StatusCode::GATEWAY_TIMEOUT, "Gateway timeout")
    }

    /// Build an HTTP response from this error.
    ///
    /// This construction is infallible: status codes are always valid, header
    /// names are static, and the body is a well-formed JSON string.
    ///
    /// JSON-escapes the message to prevent injection of control characters
    /// (newlines, tabs, backslashes, quotes) into the response body.
    pub fn into_response(self) -> Response<Full<Bytes>> {
        let body = format!(
            "{{\"error\":\"{}\",\"message\":\"{}\"}}",
            self.status.as_u16(),
            json_escape(&self.message),
        );
        // SAFETY (logical): builder with valid StatusCode and static header
        // names is infallible. We assert rather than silently swallowing.
        let len = body.len();
        Response::builder()
            .status(self.status)
            .header("content-type", "application/json")
            .header("content-length", len.to_string())
            .body(Full::new(Bytes::from(body)))
            .unwrap_or_else(|_| {
                // Unreachable in practice, but never panic on data path.
                // This builder is also infallible (valid status, no headers
                // besides the body), so the second unwrap is safe.
                Response::new(Full::new(Bytes::from_static(
                    b"{\"error\":\"500\",\"message\":\"internal error\"}",
                )))
            })
    }

    /// Create an [`HttpError`] from an [`expressgateway_core::Error`].
    pub fn from_core_error(err: &expressgateway_core::Error) -> Self {
        // `http_status()` always returns valid codes (4xx/5xx), but guard anyway.
        let status =
            StatusCode::from_u16(err.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        Self::new(status, err.to_string())
    }
}

/// Escape a string for embedding in a JSON string value.
///
/// Handles: `"`, `\`, `/`, `\b`, `\f`, `\n`, `\r`, `\t`, and control
/// characters below 0x20. This avoids pulling in a full JSON serializer
/// for a single string field.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\x08' => out.push_str("\\b"),
            '\x0C' => out.push_str("\\f"),
            c if c < '\x20' => {
                // Unicode escape for other control characters.
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.status.as_u16(), self.message)
    }
}

impl std::error::Error for HttpError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_request_response() {
        let err = HttpError::bad_request("malformed request line");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            resp.headers().get("content-type").unwrap(),
            "application/json"
        );
    }

    #[test]
    fn content_too_large_response() {
        let err = HttpError::content_too_large(2_000_000, 1_000_000);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn uri_too_long_response() {
        let err = HttpError::uri_too_long(10_000, 8_192);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::URI_TOO_LONG);
    }

    #[test]
    fn header_too_large_response() {
        let err = HttpError::header_too_large(64_000, 32_000);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::REQUEST_HEADER_FIELDS_TOO_LARGE);
    }

    #[test]
    fn bad_gateway_response() {
        let err = HttpError::bad_gateway("connection refused");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn service_unavailable_response() {
        let err = HttpError::service_unavailable("no healthy backends");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn gateway_timeout_response() {
        let err = HttpError::gateway_timeout();
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn from_core_error_body_too_large() {
        let core_err = expressgateway_core::Error::BodyTooLarge {
            size: 5000,
            max: 1000,
        };
        let http_err = HttpError::from_core_error(&core_err);
        assert_eq!(http_err.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn from_core_error_no_healthy_backend() {
        let core_err = expressgateway_core::Error::NoHealthyBackend;
        let http_err = HttpError::from_core_error(&core_err);
        assert_eq!(http_err.status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn from_core_error_backend_timeout() {
        let core_err = expressgateway_core::Error::BackendTimeout {
            addr: "127.0.0.1:8080".parse().unwrap(),
        };
        let http_err = HttpError::from_core_error(&core_err);
        assert_eq!(http_err.status, StatusCode::GATEWAY_TIMEOUT);
    }

    #[test]
    fn from_core_error_backend_connection_failed() {
        let core_err = expressgateway_core::Error::BackendConnectionFailed {
            addr: "127.0.0.1:8080".parse().unwrap(),
            reason: "connection refused".into(),
        };
        let http_err = HttpError::from_core_error(&core_err);
        assert_eq!(http_err.status, StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn json_escape_special_characters() {
        assert_eq!(json_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(json_escape("a\\b"), "a\\\\b");
        assert_eq!(json_escape("a\nb"), "a\\nb");
        assert_eq!(json_escape("a\rb"), "a\\rb");
        assert_eq!(json_escape("a\tb"), "a\\tb");
    }

    #[test]
    fn json_escape_control_characters() {
        assert_eq!(json_escape("\x00"), "\\u0000");
        assert_eq!(json_escape("\x1f"), "\\u001f");
    }

    #[test]
    fn error_response_no_panic_on_special_chars() {
        // Ensure the error response builder handles all characters without panic.
        let err = HttpError::bad_request("quote: \" newline: \n backslash: \\");
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
