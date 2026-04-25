//! QUIC variable-length integer encoding/decoding (RFC 9000 §16).
//!
//! The encoding uses 1, 2, 4, or 8 bytes depending on the value:
//!
//! | 2-bit prefix | Length | Usable bits | Maximum value        |
//! |--------------|--------|-------------|----------------------|
//! | 00           | 1      | 6           | 63                   |
//! | 01           | 2      | 14          | 16383                |
//! | 10           | 4      | 30          | 1073741823           |
//! | 11           | 8      | 62          | 4611686018427387903  |

use bytes::{BufMut, BytesMut};

use crate::H3Error;

/// Maximum value that can be encoded as a QUIC variable-length integer.
pub const MAX_VARINT: u64 = (1 << 62) - 1;

/// Decode a QUIC variable-length integer from `buf`.
///
/// Returns `(value, bytes_consumed)`.
///
/// # Errors
///
/// Returns `H3Error::Incomplete` if the buffer is too short.
/// Returns `H3Error::InvalidVarint` on malformed encoding.
pub fn decode_varint(buf: &[u8]) -> Result<(u64, usize), H3Error> {
    let first = *buf.first().ok_or(H3Error::Incomplete)?;
    let prefix = first >> 6;

    match prefix {
        0b00 => {
            let val = u64::from(first & 0x3F);
            Ok((val, 1))
        }
        0b01 => {
            let b1 = *buf.get(1).ok_or(H3Error::Incomplete)?;
            let val = (u64::from(first & 0x3F) << 8) | u64::from(b1);
            Ok((val, 2))
        }
        0b10 => {
            let b1 = *buf.get(1).ok_or(H3Error::Incomplete)?;
            let b2 = *buf.get(2).ok_or(H3Error::Incomplete)?;
            let b3 = *buf.get(3).ok_or(H3Error::Incomplete)?;
            let val = (u64::from(first & 0x3F) << 24)
                | (u64::from(b1) << 16)
                | (u64::from(b2) << 8)
                | u64::from(b3);
            Ok((val, 4))
        }
        0b11 => {
            let mut val = u64::from(first & 0x3F);
            for i in 1..8 {
                let b = *buf.get(i).ok_or(H3Error::Incomplete)?;
                val = (val << 8) | u64::from(b);
            }
            Ok((val, 8))
        }
        _ => Err(H3Error::InvalidVarint),
    }
}

/// Encode a QUIC variable-length integer into `buf`.
///
/// # Errors
///
/// Returns `H3Error::InvalidVarint` if `value > MAX_VARINT`.
pub fn encode_varint(buf: &mut BytesMut, value: u64) -> Result<usize, H3Error> {
    if value > MAX_VARINT {
        return Err(H3Error::InvalidVarint);
    }

    if value < 64 {
        #[allow(clippy::cast_possible_truncation)]
        buf.put_u8(value as u8);
        Ok(1)
    } else if value < 16384 {
        #[allow(clippy::cast_possible_truncation)]
        let v = (value as u16) | 0x4000;
        buf.put_u16(v);
        Ok(2)
    } else if value < 1_073_741_824 {
        #[allow(clippy::cast_possible_truncation)]
        let v = (value as u32) | 0x8000_0000;
        buf.put_u32(v);
        Ok(4)
    } else {
        let v = value | 0xC000_0000_0000_0000;
        buf.put_u64(v);
        Ok(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_small() {
        for v in 0..64u64 {
            let mut buf = BytesMut::new();
            let written = encode_varint(&mut buf, v).unwrap();
            assert_eq!(written, 1);
            let (decoded, consumed) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, v);
            assert_eq!(consumed, 1);
        }
    }

    #[test]
    fn roundtrip_two_byte() {
        for &v in &[64u64, 100, 1000, 16383] {
            let mut buf = BytesMut::new();
            let written = encode_varint(&mut buf, v).unwrap();
            assert_eq!(written, 2);
            let (decoded, consumed) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, v);
            assert_eq!(consumed, 2);
        }
    }

    #[test]
    fn roundtrip_four_byte() {
        for &v in &[16384u64, 100_000, 1_073_741_823] {
            let mut buf = BytesMut::new();
            let written = encode_varint(&mut buf, v).unwrap();
            assert_eq!(written, 4);
            let (decoded, consumed) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, v);
            assert_eq!(consumed, 4);
        }
    }

    #[test]
    fn roundtrip_eight_byte() {
        for &v in &[1_073_741_824u64, u64::MAX >> 2] {
            let mut buf = BytesMut::new();
            let written = encode_varint(&mut buf, v).unwrap();
            assert_eq!(written, 8);
            let (decoded, consumed) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, v);
            assert_eq!(consumed, 8);
        }
    }

    #[test]
    fn overflow() {
        let mut buf = BytesMut::new();
        assert!(encode_varint(&mut buf, MAX_VARINT + 1).is_err());
    }
}
