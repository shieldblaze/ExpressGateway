//! HTTP/2 cleartext (h2c) detection and upgrade.
//!
//! Detects the 24-byte HTTP/2 connection preface `PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n`
//! and switches to the H2 codec on match, falling back to HTTP/1.1 otherwise.

/// The 24-byte HTTP/2 connection preface (RFC 9113 Section 3.4).
pub const H2C_PREFACE: &[u8; 24] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Result of attempting to detect the h2c preface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum H2cDetectResult {
    /// The buffer contains the full HTTP/2 connection preface.
    Http2,
    /// The buffer does not start with the HTTP/2 preface -- use HTTP/1.1.
    Http1,
    /// The buffer is too short to determine; need more data.
    Incomplete,
}

/// Detect whether the initial bytes of a connection are the HTTP/2 preface.
///
/// The caller should pass the first bytes read from the connection. If the
/// buffer is shorter than 24 bytes but could still be a prefix of the preface,
/// `Incomplete` is returned. If the buffer definitively starts with the preface,
/// `Http2` is returned. Otherwise `Http1` is returned.
pub fn detect_h2c_preface(buf: &[u8]) -> H2cDetectResult {
    let preface = H2C_PREFACE;
    let check_len = buf.len().min(preface.len());

    if check_len == 0 {
        return H2cDetectResult::Incomplete;
    }

    // Check if what we have so far matches the preface prefix.
    if buf[..check_len] != preface[..check_len] {
        return H2cDetectResult::Http1;
    }

    if buf.len() >= preface.len() {
        H2cDetectResult::Http2
    } else {
        H2cDetectResult::Incomplete
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_preface_detected() {
        assert_eq!(detect_h2c_preface(H2C_PREFACE), H2cDetectResult::Http2);
    }

    #[test]
    fn preface_with_extra_data() {
        let mut buf = H2C_PREFACE.to_vec();
        buf.extend_from_slice(b"\x00\x00\x12\x04"); // SETTINGS frame start
        assert_eq!(detect_h2c_preface(&buf), H2cDetectResult::Http2);
    }

    #[test]
    fn partial_preface_incomplete() {
        let partial = &H2C_PREFACE[..10];
        assert_eq!(detect_h2c_preface(partial), H2cDetectResult::Incomplete);
    }

    #[test]
    fn http1_request_detected() {
        let buf = b"GET / HTTP/1.1\r\nHost: example.com\r\n\r\n";
        assert_eq!(detect_h2c_preface(buf), H2cDetectResult::Http1);
    }

    #[test]
    fn empty_buffer_incomplete() {
        assert_eq!(detect_h2c_preface(b""), H2cDetectResult::Incomplete);
    }

    #[test]
    fn single_byte_p_incomplete() {
        assert_eq!(detect_h2c_preface(b"P"), H2cDetectResult::Incomplete);
    }

    #[test]
    fn single_byte_non_p_is_http1() {
        assert_eq!(detect_h2c_preface(b"G"), H2cDetectResult::Http1);
    }

    #[test]
    fn preface_constant_is_24_bytes() {
        assert_eq!(H2C_PREFACE.len(), 24);
    }

    #[test]
    fn preface_constant_matches_rfc() {
        assert_eq!(H2C_PREFACE, b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n");
    }
}
