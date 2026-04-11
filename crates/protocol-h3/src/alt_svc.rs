//! Alt-Svc header injection for HTTP/3 advertisement (RFC 7838).
//!
//! Generates `Alt-Svc` headers to advertise HTTP/3 availability in HTTP/1.1
//! and HTTP/2 responses, enabling clients to upgrade to QUIC transport.

use std::fmt;

/// Represents an `Alt-Svc` header value for HTTP/3 advertisement.
///
/// Format: `h3=":PORT"; ma=MAX_AGE`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AltSvcHeader {
    /// The port number to advertise for HTTP/3.
    pub port: u16,
    /// Maximum age in seconds that the Alt-Svc information is valid.
    pub max_age: u32,
}

impl AltSvcHeader {
    /// Create a new Alt-Svc header advertising HTTP/3 on the given port.
    pub fn new(port: u16, max_age: u32) -> Self {
        Self { port, max_age }
    }

    /// Format the header value string.
    ///
    /// Returns a string like `h3=":443"; ma=86400`.
    pub fn header_value(&self) -> String {
        format!("h3=\":{}\"; ma={}", self.port, self.max_age)
    }

    /// The HTTP header name.
    pub fn header_name(&self) -> &'static str {
        "alt-svc"
    }

    /// Convenience: inject Alt-Svc into an HTTP response builder.
    pub fn inject_into_response(
        &self,
        response: http::response::Builder,
    ) -> http::response::Builder {
        response.header(self.header_name(), self.header_value())
    }

    /// Create a "clear" Alt-Svc header that tells clients to discard
    /// previously cached alternatives.
    pub fn clear() -> String {
        "clear".to_string()
    }

    /// Generate Alt-Svc header value for multiple H3 endpoints.
    pub fn multi(entries: &[AltSvcHeader]) -> String {
        entries
            .iter()
            .map(|e| e.header_value())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl fmt::Display for AltSvcHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.header_value())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_header_generation() {
        let alt_svc = AltSvcHeader::new(443, 86400);
        assert_eq!(alt_svc.header_value(), "h3=\":443\"; ma=86400");
    }

    #[test]
    fn custom_port_and_max_age() {
        let alt_svc = AltSvcHeader::new(8443, 3600);
        assert_eq!(alt_svc.header_value(), "h3=\":8443\"; ma=3600");
    }

    #[test]
    fn display_impl() {
        let alt_svc = AltSvcHeader::new(443, 86400);
        assert_eq!(format!("{alt_svc}"), "h3=\":443\"; ma=86400");
    }

    #[test]
    fn header_name_is_alt_svc() {
        let alt_svc = AltSvcHeader::new(443, 86400);
        assert_eq!(alt_svc.header_name(), "alt-svc");
    }

    #[test]
    fn clear_header() {
        assert_eq!(AltSvcHeader::clear(), "clear");
    }

    #[test]
    fn multi_endpoint_header() {
        let entries = vec![AltSvcHeader::new(443, 86400), AltSvcHeader::new(8443, 3600)];
        let value = AltSvcHeader::multi(&entries);
        assert_eq!(value, "h3=\":443\"; ma=86400, h3=\":8443\"; ma=3600");
    }

    #[test]
    fn inject_into_http_response() {
        let alt_svc = AltSvcHeader::new(443, 86400);
        let builder = http::Response::builder().status(200);
        let builder = alt_svc.inject_into_response(builder);
        let response = builder.body(()).unwrap();

        let header = response.headers().get("alt-svc").unwrap();
        assert_eq!(header.to_str().unwrap(), "h3=\":443\"; ma=86400");
    }

    #[test]
    fn equality() {
        let a = AltSvcHeader::new(443, 86400);
        let b = AltSvcHeader::new(443, 86400);
        let c = AltSvcHeader::new(8443, 86400);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn zero_port() {
        let alt_svc = AltSvcHeader::new(0, 0);
        assert_eq!(alt_svc.header_value(), "h3=\":0\"; ma=0");
    }
}
