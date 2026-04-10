//! Auto-detection of PROXY protocol version.
//!
//! Inspects the first bytes of a buffer to determine whether a v1 (text) or
//! v2 (binary) header is present.

use crate::v1::V1_PREFIX;
use crate::v2::V2_SIGNATURE;

/// Detected PROXY protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyVersion {
    V1,
    V2,
}

/// The minimum number of bytes needed to reliably distinguish v1 from v2.
///
/// The v2 signature is 12 bytes; the v1 prefix `PROXY ` is 6 bytes.
/// We need at least 12 bytes to be sure it is *not* v2 before falling back.
pub const DETECT_MIN_BYTES: usize = 12;

/// Attempt to detect whether the buffer contains a v1 or v2 PROXY protocol
/// header.
///
/// Returns `None` if neither signature is recognized **or** if there are
/// fewer than [`DETECT_MIN_BYTES`] bytes available (in which case the caller
/// should wait for more data before retrying).
pub fn detect(buf: &[u8]) -> Option<ProxyVersion> {
    if buf.len() < V1_PREFIX.len() {
        return None;
    }

    // Check v2 first (longer, more specific signature).
    if buf.len() >= V2_SIGNATURE.len() && buf[..V2_SIGNATURE.len()] == V2_SIGNATURE {
        return Some(ProxyVersion::V2);
    }

    // Check v1 prefix.
    if buf[..V1_PREFIX.len()] == *V1_PREFIX {
        return Some(ProxyVersion::V1);
    }

    // If we have enough bytes and neither matched, there is no PROXY header.
    if buf.len() >= DETECT_MIN_BYTES {
        return None;
    }

    // Not enough data to be certain.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_v1() {
        let buf = b"PROXY TCP4 192.168.1.1 10.0.0.1 56324 443\r\n";
        assert_eq!(detect(buf), Some(ProxyVersion::V1));
    }

    #[test]
    fn detect_v2() {
        let mut buf = Vec::from(V2_SIGNATURE.as_slice());
        buf.extend_from_slice(&[0x21, 0x11, 0x00, 0x0C]); // ver2 proxy, inet, stream, 12 bytes
        assert_eq!(detect(&buf), Some(ProxyVersion::V2));
    }

    #[test]
    fn detect_neither() {
        let buf = b"GET / HTTP/1.1\r\n";
        assert_eq!(detect(buf), None);
    }

    #[test]
    fn detect_too_short() {
        let buf = b"PRO";
        assert_eq!(detect(buf), None);
    }

    #[test]
    fn detect_v1_exact_prefix() {
        // Just the prefix plus enough bytes for detection.
        let buf = b"PROXY UNKNOWN\r\n";
        assert_eq!(detect(buf), Some(ProxyVersion::V1));
    }

    #[test]
    fn detect_v2_signature_only() {
        assert_eq!(detect(&V2_SIGNATURE), Some(ProxyVersion::V2));
    }

    #[test]
    fn detect_empty() {
        assert_eq!(detect(b""), None);
    }
}
