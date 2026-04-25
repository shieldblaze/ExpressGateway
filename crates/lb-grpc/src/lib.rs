//! Native gRPC proxy with framing, trailers, and streaming support.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo,
    clippy::unimplemented,
    clippy::unreachable,
    missing_docs
)]
#![allow(clippy::pedantic, clippy::nursery)]
#![cfg_attr(
    test,
    allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

mod deadline;
mod error;
mod frame;
mod status;
mod streaming;

pub use deadline::GrpcDeadline;
pub use error::GrpcError;
pub use frame::{DEFAULT_MAX_MESSAGE_SIZE, GrpcFrame, decode_grpc_frame, encode_grpc_frame};
pub use status::GrpcStatus;
pub use streaming::StreamingMode;

#[cfg(test)]
mod tests {
    use bytes::Bytes;

    use super::*;

    #[test]
    fn frame_roundtrip_uncompressed() {
        let frame = GrpcFrame {
            compressed: false,
            data: Bytes::from("hello grpc"),
        };
        let encoded = encode_grpc_frame(&frame).unwrap();
        let (decoded, consumed) = decode_grpc_frame(&encoded, DEFAULT_MAX_MESSAGE_SIZE).unwrap();
        assert_eq!(consumed, encoded.len());
        assert_eq!(decoded.data, frame.data);
        assert!(!decoded.compressed);
    }

    #[test]
    fn frame_roundtrip_compressed() {
        let frame = GrpcFrame {
            compressed: true,
            data: Bytes::from("compressed payload"),
        };
        let encoded = encode_grpc_frame(&frame).unwrap();
        let (decoded, _) = decode_grpc_frame(&encoded, DEFAULT_MAX_MESSAGE_SIZE).unwrap();
        assert!(decoded.compressed);
        assert_eq!(decoded.data, frame.data);
    }

    #[test]
    fn frame_empty_data() {
        let frame = GrpcFrame {
            compressed: false,
            data: Bytes::new(),
        };
        let encoded = encode_grpc_frame(&frame).unwrap();
        assert_eq!(encoded.len(), 5); // 1 byte flag + 4 bytes length
        let (decoded, consumed) = decode_grpc_frame(&encoded, DEFAULT_MAX_MESSAGE_SIZE).unwrap();
        assert_eq!(consumed, 5);
        assert!(decoded.data.is_empty());
    }

    #[test]
    fn frame_incomplete_header() {
        let buf = [0u8; 3]; // Less than 5-byte header
        let result = decode_grpc_frame(&buf, DEFAULT_MAX_MESSAGE_SIZE);
        assert!(result.is_err());
    }

    #[test]
    fn frame_incomplete_body() {
        // Header says 100 bytes but only 5 bytes of body present
        let mut buf = vec![0u8]; // compressed = false
        buf.extend_from_slice(&100u32.to_be_bytes()); // length = 100
        buf.extend_from_slice(&[0u8; 5]); // only 5 bytes of body
        let result = decode_grpc_frame(&buf, DEFAULT_MAX_MESSAGE_SIZE);
        assert!(result.is_err());
    }

    #[test]
    fn multi_frame_decode() {
        let frames: Vec<GrpcFrame> = (0..5)
            .map(|i| GrpcFrame {
                compressed: false,
                data: Bytes::from(format!("msg-{i}")),
            })
            .collect();

        let mut buf = Vec::new();
        for f in &frames {
            buf.extend_from_slice(&encode_grpc_frame(f).unwrap());
        }

        let mut offset = 0;
        for (i, _) in frames.iter().enumerate() {
            let (decoded, consumed) =
                decode_grpc_frame(&buf[offset..], DEFAULT_MAX_MESSAGE_SIZE).unwrap();
            assert_eq!(decoded.data, Bytes::from(format!("msg-{i}")));
            offset += consumed;
        }
        assert_eq!(offset, buf.len());
    }

    #[test]
    fn status_from_code_valid() {
        assert_eq!(GrpcStatus::from_code(0).unwrap(), GrpcStatus::Ok);
        assert_eq!(
            GrpcStatus::from_code(4).unwrap(),
            GrpcStatus::DeadlineExceeded
        );
        assert_eq!(
            GrpcStatus::from_code(16).unwrap(),
            GrpcStatus::Unauthenticated
        );
    }

    #[test]
    fn status_from_code_invalid() {
        assert!(GrpcStatus::from_code(17).is_err());
        assert!(GrpcStatus::from_code(99).is_err());
    }

    #[test]
    fn status_to_http() {
        assert_eq!(GrpcStatus::Ok.to_http_status(), 200);
        assert_eq!(GrpcStatus::NotFound.to_http_status(), 404);
        assert_eq!(GrpcStatus::Unauthenticated.to_http_status(), 401);
        assert_eq!(GrpcStatus::PermissionDenied.to_http_status(), 403);
        assert_eq!(GrpcStatus::Unavailable.to_http_status(), 503);
    }

    #[test]
    fn status_from_http() {
        assert_eq!(GrpcStatus::from_http_status(200), GrpcStatus::Ok);
        assert_eq!(
            GrpcStatus::from_http_status(401),
            GrpcStatus::Unauthenticated
        );
        assert_eq!(GrpcStatus::from_http_status(503), GrpcStatus::Unavailable);
    }

    #[test]
    fn deadline_parse_seconds() {
        assert_eq!(GrpcDeadline::parse_timeout("5S").unwrap(), 5000);
    }

    #[test]
    fn deadline_parse_millis() {
        assert_eq!(GrpcDeadline::parse_timeout("100m").unwrap(), 100);
    }

    #[test]
    fn deadline_parse_hours() {
        assert_eq!(GrpcDeadline::parse_timeout("1H").unwrap(), 3_600_000);
    }

    #[test]
    fn deadline_parse_minutes() {
        assert_eq!(GrpcDeadline::parse_timeout("2M").unwrap(), 120_000);
    }

    #[test]
    fn deadline_parse_micros() {
        assert_eq!(GrpcDeadline::parse_timeout("1000000u").unwrap(), 1000);
    }

    #[test]
    fn deadline_parse_nanos() {
        assert_eq!(GrpcDeadline::parse_timeout("1000000000n").unwrap(), 1000);
    }

    #[test]
    fn deadline_remaining() {
        assert_eq!(GrpcDeadline::remaining(5000, 3000), Some(2000));
        assert_eq!(GrpcDeadline::remaining(5000, 5000), None);
        assert_eq!(GrpcDeadline::remaining(5000, 6000), None);
    }

    #[test]
    fn deadline_format_roundtrip() {
        let formatted = GrpcDeadline::format_timeout(5000);
        let parsed = GrpcDeadline::parse_timeout(&formatted).unwrap();
        assert_eq!(parsed, 5000);
    }

    #[test]
    fn deadline_invalid_format() {
        assert!(GrpcDeadline::parse_timeout("").is_err());
        assert!(GrpcDeadline::parse_timeout("S").is_err());
        assert!(GrpcDeadline::parse_timeout("abc").is_err());
        assert!(GrpcDeadline::parse_timeout("5x").is_err());
    }

    #[test]
    fn deadline_submillisecond_rounds_up() {
        // 500 microseconds should round up to 1ms, not truncate to 0ms.
        assert_eq!(GrpcDeadline::parse_timeout("500u").unwrap(), 1);
        // 1 microsecond should round up to 1ms.
        assert_eq!(GrpcDeadline::parse_timeout("1u").unwrap(), 1);
        // 999 nanoseconds should round up to 1ms.
        assert_eq!(GrpcDeadline::parse_timeout("999n").unwrap(), 1);
        // 1 nanosecond should round up to 1ms.
        assert_eq!(GrpcDeadline::parse_timeout("1n").unwrap(), 1);
        // 0 microseconds stays 0 (no non-zero input to round).
        assert_eq!(GrpcDeadline::parse_timeout("0u").unwrap(), 0);
        // 0 nanoseconds stays 0.
        assert_eq!(GrpcDeadline::parse_timeout("0n").unwrap(), 0);
    }

    #[test]
    fn grpc_message_too_large() {
        // Build a frame with a 10 MB length field and a small max.
        let mut buf = vec![0u8]; // compressed = false
        let big_len: u32 = 10 * 1024 * 1024;
        buf.extend_from_slice(&big_len.to_be_bytes());
        // We don't need to actually provide the payload bytes for the size
        // check to fire.
        buf.extend_from_slice(&[0u8; 64]);
        let result = decode_grpc_frame(&buf, 4 * 1024 * 1024);
        assert!(matches!(
            result,
            Err(GrpcError::MessageTooLarge { size, limit })
            if size == big_len && limit == 4 * 1024 * 1024
        ));
    }
}
