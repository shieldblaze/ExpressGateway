//! gRPC length-prefixed framing.
//!
//! gRPC messages are transmitted over HTTP/2 as length-prefixed frames:
//!
//! ```text
//! [1 byte compressed flag] [4 bytes big-endian length] [N bytes payload]
//! ```
//!
//! See <https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md>.

use bytes::{BufMut, Bytes, BytesMut};

use crate::GrpcError;

/// Header size: 1 byte compressed flag + 4 bytes length.
const GRPC_HEADER_SIZE: usize = 5;

/// A single gRPC message frame.
#[derive(Debug, Clone)]
pub struct GrpcFrame {
    /// Whether the payload is compressed.
    pub compressed: bool,
    /// The raw message bytes (before or after decompression, depending on context).
    pub data: Bytes,
}

/// Default maximum gRPC message size (4 MB), matching the default in
/// grpc-go and grpc-java.
pub const DEFAULT_MAX_MESSAGE_SIZE: u32 = 4 * 1024 * 1024;

/// Decode a single gRPC frame from the front of `buf`.
///
/// `max_message_size` caps the decoded message length. Messages whose
/// 4-byte length field exceeds this limit produce `GrpcError::MessageTooLarge`.
///
/// Returns the decoded frame and the total number of bytes consumed from `buf`
/// (header + payload).
///
/// # Errors
///
/// - [`GrpcError::Incomplete`] if `buf` does not contain a complete frame.
/// - [`GrpcError::InvalidFrame`] if the compressed flag is not 0 or 1.
/// - [`GrpcError::MessageTooLarge`] if the message length exceeds `max_message_size`.
pub fn decode_grpc_frame(
    buf: &[u8],
    max_message_size: u32,
) -> Result<(GrpcFrame, usize), GrpcError> {
    if buf.len() < GRPC_HEADER_SIZE {
        return Err(GrpcError::Incomplete);
    }

    // SAFETY of .get(): we checked len >= GRPC_HEADER_SIZE above.
    let compressed_byte = buf.first().copied().ok_or(GrpcError::Incomplete)?;
    let compressed = match compressed_byte {
        0 => false,
        1 => true,
        other => {
            return Err(GrpcError::InvalidFrame(format!(
                "invalid compressed flag: {other}"
            )));
        }
    };

    // Read big-endian u32 length from bytes 1..5.
    let len_bytes: [u8; 4] = [
        buf.get(1).copied().ok_or(GrpcError::Incomplete)?,
        buf.get(2).copied().ok_or(GrpcError::Incomplete)?,
        buf.get(3).copied().ok_or(GrpcError::Incomplete)?,
        buf.get(4).copied().ok_or(GrpcError::Incomplete)?,
    ];
    let msg_len = u32::from_be_bytes(len_bytes);

    if msg_len > max_message_size {
        return Err(GrpcError::MessageTooLarge {
            size: msg_len,
            limit: max_message_size,
        });
    }

    let msg_len_usize = msg_len as usize;
    let total_len = GRPC_HEADER_SIZE + msg_len_usize;
    if buf.len() < total_len {
        return Err(GrpcError::Incomplete);
    }

    let data = Bytes::copy_from_slice(
        buf.get(GRPC_HEADER_SIZE..total_len)
            .ok_or(GrpcError::Incomplete)?,
    );

    Ok((GrpcFrame { compressed, data }, total_len))
}

/// Encode a gRPC frame into a length-prefixed byte buffer.
///
/// # Errors
///
/// Returns [`GrpcError::InvalidFrame`] if the payload exceeds `u32::MAX` bytes.
pub fn encode_grpc_frame(frame: &GrpcFrame) -> Result<Bytes, GrpcError> {
    let len = u32::try_from(frame.data.len()).map_err(|_| {
        GrpcError::InvalidFrame(format!("payload too large: {} bytes", frame.data.len()))
    })?;

    let mut buf = BytesMut::with_capacity(GRPC_HEADER_SIZE + frame.data.len());
    buf.put_u8(u8::from(frame.compressed));
    buf.put_u32(len);
    buf.extend_from_slice(&frame.data);

    Ok(buf.freeze())
}
