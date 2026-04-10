//! ALPN (Application-Layer Protocol Negotiation) configuration.
//!
//! Supported protocols: `h2` and `http/1.1`.
//! Uses static byte slices -- zero allocation on the hot path.

/// ALPN protocol identifier for HTTP/2.
pub const ALPN_H2: &[u8] = b"h2";

/// ALPN protocol identifier for HTTP/1.1.
pub const ALPN_HTTP11: &[u8] = b"http/1.1";

/// Return the default ALPN protocol list (h2 preferred, http/1.1 fallback).
///
/// This allocates two small `Vec<u8>` -- called once during config construction,
/// not on the handshake hot path.
#[inline]
pub fn default_alpn_protocols() -> Vec<Vec<u8>> {
    vec![ALPN_H2.to_vec(), ALPN_HTTP11.to_vec()]
}

/// Configure ALPN protocols on a `ServerConfig`.
#[inline]
pub fn configure_alpn(config: &mut rustls::ServerConfig) {
    config.alpn_protocols = default_alpn_protocols();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_alpn_has_h2_and_http11() {
        let protos = default_alpn_protocols();
        assert_eq!(protos.len(), 2);
        assert_eq!(protos[0], b"h2");
        assert_eq!(protos[1], b"http/1.1");
    }

    #[test]
    fn h2_is_preferred_over_http11() {
        let protos = default_alpn_protocols();
        assert_eq!(protos[0], ALPN_H2);
        assert_eq!(protos[1], ALPN_HTTP11);
    }

    #[test]
    fn alpn_constants_are_valid_ascii() {
        assert!(std::str::from_utf8(ALPN_H2).is_ok());
        assert!(std::str::from_utf8(ALPN_HTTP11).is_ok());
    }
}
