//! gRPC health check endpoint.
//!
//! Implements the `grpc.health.v1.Health/Check` RPC by returning a
//! length-prefixed protobuf response indicating `SERVING`.
//!
//! The wire format is:
//! - 1 byte: compression flag (0 = not compressed)
//! - 4 bytes: message length (big-endian u32)
//! - N bytes: protobuf `HealthCheckResponse { status: SERVING(1) }`
//!
//! The protobuf encoding of `HealthCheckResponse { status: 1 }` is:
//! field 1 (tag = 0x08), varint 1 (value = 0x01) -> `[0x08, 0x01]`.

use bytes::Bytes;

/// The path for the gRPC health check RPC.
pub const HEALTH_CHECK_PATH: &str = "/grpc.health.v1.Health/Check";

/// The protobuf-encoded `HealthCheckResponse { status: SERVING }`.
const HEALTH_RESPONSE_BODY: &[u8] = &[0x08, 0x01];

/// Build a length-prefixed gRPC message frame for the health check response.
///
/// Returns the 5-byte header followed by the protobuf body.
pub fn health_check_response() -> Bytes {
    let body_len = HEALTH_RESPONSE_BODY.len() as u32;
    let mut buf = Vec::with_capacity(5 + HEALTH_RESPONSE_BODY.len());
    // Compression flag: 0 (no compression)
    buf.push(0);
    // 4-byte big-endian length
    buf.extend_from_slice(&body_len.to_be_bytes());
    // Protobuf body
    buf.extend_from_slice(HEALTH_RESPONSE_BODY);
    Bytes::from(buf)
}

/// Returns `true` if the request path matches the health check RPC.
pub fn is_health_check(path: &str) -> bool {
    path == HEALTH_CHECK_PATH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_encoding() {
        let resp = health_check_response();
        // 5-byte header + 2-byte body = 7 bytes total
        assert_eq!(resp.len(), 7);

        // Compression flag
        assert_eq!(resp[0], 0x00);

        // Length (big-endian u32 = 2)
        assert_eq!(&resp[1..5], &[0x00, 0x00, 0x00, 0x02]);

        // Protobuf body: field 1 = varint 1 (SERVING)
        assert_eq!(&resp[5..], &[0x08, 0x01]);
    }

    #[test]
    fn health_path_detection() {
        assert!(is_health_check("/grpc.health.v1.Health/Check"));
        assert!(!is_health_check("/grpc.health.v1.Health/Watch"));
        assert!(!is_health_check("/other.Service/Method"));
    }

    #[test]
    fn response_has_correct_grpc_frame() {
        let resp = health_check_response();
        let compression = resp[0];
        let length = u32::from_be_bytes([resp[1], resp[2], resp[3], resp[4]]);
        let body = &resp[5..];

        assert_eq!(compression, 0);
        assert_eq!(length as usize, body.len());
    }
}
