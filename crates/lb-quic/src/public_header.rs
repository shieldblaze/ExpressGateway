//! SHARED-1: QUIC public-header parser. Quiche-free, no_alloc on the
//! parse path, no panics, no decryption.
//!
//! INVARIANT: this parser reads ONLY the cleartext public-header
//! bytes (form, version, DCID-len/DCID, SCID-len/SCID, token-len/
//! token for Initial, length-varint where present). It NEVER touches
//! encrypted payload, packet-number bytes, or header-protected
//! reserved bits. The Mode A no-decrypt property is LAYERED on top
//! of this guarantee — verifier code-read will check this is upheld.
//!
//! Wire format:
//! * Long header: RFC 9000 §17.2 — `1|1|TT|RR|PP` byte0, then version
//!   (4 bytes BE), DCID-len (1 byte) + DCID, SCID-len (1 byte) + SCID,
//!   then per-type tail (Initial: token-len varint + token + length
//!   varint; 0-RTT/Handshake: length varint; Retry: token + 16-byte
//!   integrity tag — parser surfaces Retry but does not decode its
//!   tail; Version Negotiation: version-list — also not decoded).
//! * Short header: RFC 9000 §17.3 — `0|1|S|RR|K|PP` byte0, then DCID
//!   for a caller-supplied length (NOT encoded on the wire; see
//!   `audit/quic/s15-design.md` §2.3).
//!
//! Note on header protection (RFC 9001 §5.4): HP masks byte0's low 4
//! bits (Long: reserved + PN-len; Short: reserved + key-phase + PN-len)
//! plus the encrypted packet-number bytes. The fields this parser
//! reads (form bit, fixed bit, type bits 4-5 on Long, version, DCID
//! length+bytes, SCID length+bytes, token, length-varint) are NOT
//! header-protected — they are wire-cleartext. So this parser works
//! identically on protected wire packets and on the cleartext
//! pseudo-packets used in RFC 9001's worked examples.

use thiserror::Error;

/// Maximum DCID/SCID length per RFC 9000 §17.2 / §17.3.
pub const MAX_CID_LEN: usize = 20;

/// Long-header type classification.
///
/// `VersionNegotiation` is reported when the version field is
/// `0x00000000` regardless of the type bits, per RFC 9000 §17.2.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongType {
    /// RFC 9000 §17.2.2 — Initial (carries token + length).
    Initial,
    /// RFC 9000 §17.2.3 — 0-RTT (carries length).
    ZeroRtt,
    /// RFC 9000 §17.2.4 — Handshake (carries length).
    Handshake,
    /// RFC 9000 §17.2.5 — Retry (server-origin; tail is opaque to
    /// this parser).
    Retry,
    /// RFC 9000 §17.2.1 — Version Negotiation (`version == 0`).
    VersionNegotiation,
}

/// Parsed public header — borrows from the input slice.
#[derive(Debug)]
pub enum PublicHeader<'a> {
    /// Long-header packet.
    Long {
        /// Long-header type (see [`LongType`]).
        ty: LongType,
        /// Wire `Version` field. `0x00000000` indicates Version
        /// Negotiation (already reflected in `ty`).
        version: u32,
        /// Destination Connection ID bytes (0..=20).
        dcid: &'a [u8],
        /// Source Connection ID bytes (0..=20).
        scid: &'a [u8],
        /// Initial-only token. `None` for non-Initial.
        token: Option<&'a [u8]>,
        /// Length-varint covering PN + payload (Initial, 0-RTT,
        /// Handshake). `None` for Retry and Version Negotiation.
        length: Option<u64>,
    },
    /// Short-header (1-RTT) packet. The DCID length is recovered
    /// out-of-band per `audit/quic/s15-design.md` §2.3.
    Short {
        /// Destination Connection ID bytes.
        dcid: &'a [u8],
    },
}

/// Parse-time failures.
#[derive(Debug, Error)]
pub enum HeaderError {
    /// Buffer ran out before a field could be read.
    #[error("packet too short: need {needed} bytes, have {got}")]
    TooShort {
        /// Bytes the parser needed to read the next field.
        needed: usize,
        /// Bytes actually available in the buffer.
        got: usize,
    },
    /// RFC 9000 §17.2 / §17.3 Fixed Bit was clear.
    #[error("fixed bit clear (RFC 9000 §17.2/§17.3)")]
    FixedBitClear,
    /// DCID length byte > [`MAX_CID_LEN`].
    #[error("DCID length {0} exceeds RFC max of 20")]
    DcidTooLong(u8),
    /// SCID length byte > [`MAX_CID_LEN`].
    #[error("SCID length {0} exceeds RFC max of 20")]
    ScidTooLong(u8),
    /// A varint field was malformed.
    #[error("varint: {0}")]
    Varint(#[source] VarintError),
    /// A declared length-varint exceeded the remaining packet bytes.
    #[error("declared length {declared} overruns packet ({remaining} remaining)")]
    Truncated {
        /// Length the wire claimed.
        declared: u64,
        /// Bytes actually remaining in the buffer.
        remaining: usize,
    },
}

/// Failure modes for the inlined RFC 9000 §16 varint reader.
#[derive(Debug, Error)]
pub enum VarintError {
    /// The varint extends past the end of the buffer.
    #[error("varint incomplete: need {needed} bytes, have {got}")]
    Incomplete {
        /// Bytes the varint needed.
        needed: usize,
        /// Bytes available.
        got: usize,
    },
}

// SHARED-V — RFC 9000 §16 varint, used by the QUIC public-header parser.
//
// Returns `(value, bytes_consumed)`. The 2-bit prefix of the first
// byte encodes a length of 1/2/4/8; the remaining 6 bits are the
// high-order bits of the value, big-endian.
fn decode_varint(buf: &[u8]) -> Result<(u64, usize), VarintError> {
    let first = *buf
        .first()
        .ok_or(VarintError::Incomplete { needed: 1, got: 0 })?;
    let len = 1usize << (first >> 6);
    if buf.len() < len {
        return Err(VarintError::Incomplete {
            needed: len,
            got: buf.len(),
        });
    }
    let mut val = u64::from(first & 0x3F);
    for i in 1..len {
        let b = *buf.get(i).ok_or(VarintError::Incomplete {
            needed: len,
            got: buf.len(),
        })?;
        val = (val << 8) | u64::from(b);
    }
    Ok((val, len))
}

/// Parse the public header of `pkt`. For short headers the caller
/// MUST pass the per-flow `short_dcid_len` recovered as
/// `audit/quic/s15-design.md` §2.3 describes.
///
/// # Errors
///
/// Returns [`HeaderError`] on any malformed or truncated wire packet.
/// Never panics on arbitrary input (regression-tested in the proptest
/// no-panic harness).
pub fn parse_public_header(
    pkt: &[u8],
    short_dcid_len: usize,
) -> Result<PublicHeader<'_>, HeaderError> {
    let b0 = *pkt
        .first()
        .ok_or(HeaderError::TooShort { needed: 1, got: 0 })?;
    // RFC 9000 §17.2/§17.3 — Fixed Bit MUST be 1.
    if b0 & 0x40 == 0 {
        return Err(HeaderError::FixedBitClear);
    }
    if b0 & 0x80 == 0 {
        parse_short(pkt, short_dcid_len)
    } else {
        parse_long(pkt)
    }
}

fn parse_long(pkt: &[u8]) -> Result<PublicHeader<'_>, HeaderError> {
    // byte0 + 4 version bytes + 1 dcid-len byte minimum.
    let head = pkt.get(..6).ok_or(HeaderError::TooShort {
        needed: 6,
        got: pkt.len(),
    })?;
    let b0 = *head
        .first()
        .ok_or(HeaderError::TooShort { needed: 1, got: 0 })?;
    let v0 = *head.get(1).ok_or(HeaderError::TooShort {
        needed: 2,
        got: head.len(),
    })?;
    let v1 = *head.get(2).ok_or(HeaderError::TooShort {
        needed: 3,
        got: head.len(),
    })?;
    let v2 = *head.get(3).ok_or(HeaderError::TooShort {
        needed: 4,
        got: head.len(),
    })?;
    let v3 = *head.get(4).ok_or(HeaderError::TooShort {
        needed: 5,
        got: head.len(),
    })?;
    let version = u32::from_be_bytes([v0, v1, v2, v3]);

    let dcid_len = *head.get(5).ok_or(HeaderError::TooShort {
        needed: 6,
        got: head.len(),
    })?;
    if usize::from(dcid_len) > MAX_CID_LEN {
        return Err(HeaderError::DcidTooLong(dcid_len));
    }
    let dcid_start = 6usize;
    let dcid_end = dcid_start
        .checked_add(usize::from(dcid_len))
        .ok_or(HeaderError::TooShort {
            needed: usize::MAX,
            got: pkt.len(),
        })?;
    let dcid = pkt.get(dcid_start..dcid_end).ok_or(HeaderError::TooShort {
        needed: dcid_end,
        got: pkt.len(),
    })?;

    let scid_len_idx = dcid_end;
    let scid_len = *pkt.get(scid_len_idx).ok_or(HeaderError::TooShort {
        needed: scid_len_idx.saturating_add(1),
        got: pkt.len(),
    })?;
    if usize::from(scid_len) > MAX_CID_LEN {
        return Err(HeaderError::ScidTooLong(scid_len));
    }
    let scid_start = scid_len_idx.saturating_add(1);
    let scid_end = scid_start
        .checked_add(usize::from(scid_len))
        .ok_or(HeaderError::TooShort {
            needed: usize::MAX,
            got: pkt.len(),
        })?;
    let scid = pkt.get(scid_start..scid_end).ok_or(HeaderError::TooShort {
        needed: scid_end,
        got: pkt.len(),
    })?;

    // Version Negotiation overrides type-bits per RFC 9000 §17.2.1.
    if version == 0 {
        return Ok(PublicHeader::Long {
            ty: LongType::VersionNegotiation,
            version,
            dcid,
            scid,
            token: None,
            length: None,
        });
    }

    let ty = match (b0 >> 4) & 0x03 {
        0b00 => LongType::Initial,
        0b01 => LongType::ZeroRtt,
        0b10 => LongType::Handshake,
        _ => LongType::Retry,
    };

    let tail = pkt.get(scid_end..).ok_or(HeaderError::TooShort {
        needed: scid_end,
        got: pkt.len(),
    })?;
    let (token, length) = match ty {
        LongType::Initial => {
            let (tok_len, tok_len_n) = decode_varint(tail).map_err(HeaderError::Varint)?;
            let tok_len_usize = usize::try_from(tok_len).map_err(|_| HeaderError::Truncated {
                declared: tok_len,
                remaining: tail.len().saturating_sub(tok_len_n),
            })?;
            let tok_start = tok_len_n;
            let tok_end = tok_start
                .checked_add(tok_len_usize)
                .ok_or(HeaderError::Truncated {
                    declared: tok_len,
                    remaining: tail.len().saturating_sub(tok_start),
                })?;
            let tok = tail.get(tok_start..tok_end).ok_or(HeaderError::Truncated {
                declared: tok_len,
                remaining: tail.len().saturating_sub(tok_start),
            })?;
            let after_tok = tail.get(tok_end..).ok_or(HeaderError::TooShort {
                needed: tok_end,
                got: tail.len(),
            })?;
            let (len_val, len_n) = decode_varint(after_tok).map_err(HeaderError::Varint)?;
            let after_len = after_tok.get(len_n..).unwrap_or(&[]);
            let declared_usize = usize::try_from(len_val).unwrap_or(usize::MAX);
            if after_len.len() < declared_usize {
                return Err(HeaderError::Truncated {
                    declared: len_val,
                    remaining: after_len.len(),
                });
            }
            (Some(tok), Some(len_val))
        }
        LongType::ZeroRtt | LongType::Handshake => {
            let (len_val, len_n) = decode_varint(tail).map_err(HeaderError::Varint)?;
            let after_len = tail.get(len_n..).unwrap_or(&[]);
            let declared_usize = usize::try_from(len_val).unwrap_or(usize::MAX);
            if after_len.len() < declared_usize {
                return Err(HeaderError::Truncated {
                    declared: len_val,
                    remaining: after_len.len(),
                });
            }
            (None, Some(len_val))
        }
        // Retry (and the structurally-unreachable VN — VN is short-
        // circuited above the match because the `version == 0` check
        // overrides the type-bits classification, and `ty` is derived
        // only from the 4 type-bit values 0b00/01/10/11; we map them
        // to Initial/0RTT/Handshake/Retry). Folding VN onto the Retry
        // arm keeps the match exhaustive without `unreachable!` (the
        // crate denies `clippy::unreachable`).
        LongType::Retry | LongType::VersionNegotiation => (None, None),
    };

    Ok(PublicHeader::Long {
        ty,
        version,
        dcid,
        scid,
        token,
        length,
    })
}

fn parse_short(pkt: &[u8], short_dcid_len: usize) -> Result<PublicHeader<'_>, HeaderError> {
    if short_dcid_len == 0 || short_dcid_len > MAX_CID_LEN {
        // 0 is a valid CID length on the wire in principle, but Mode A
        // requires a non-empty DCID to route — collapse 0 onto the
        // DcidTooLong error so the router has a single rejection branch.
        // Treat 0 as "caller passed a bogus length"; out-of-range as the
        // explicit cap-violation.
        let len_byte = u8::try_from(short_dcid_len).unwrap_or(u8::MAX);
        return Err(HeaderError::DcidTooLong(len_byte));
    }
    let end = 1usize
        .checked_add(short_dcid_len)
        .ok_or(HeaderError::TooShort {
            needed: usize::MAX,
            got: pkt.len(),
        })?;
    let dcid = pkt.get(1..end).ok_or(HeaderError::TooShort {
        needed: end,
        got: pkt.len(),
    })?;
    Ok(PublicHeader::Short { dcid })
}

#[cfg(test)]
mod tests {
    //! Unit tests for the SHARED-1 public-header parser.
    //!
    //! Fixture provenance: RFC 9001 §A.3 ("Client Initial") provides
    //! the on-wire bytes of a real protected client Initial. The
    //! parser operates on wire bytes; for Long headers, header
    //! protection masks ONLY byte0's low 4 bits and the encrypted PN
    //! bytes, so the wire-cleartext public-header fields the parser
    //! reads are identical between RFC §A.2 (unprotected) and §A.3
    //! (protected). We use the §A.2 unprotected byte sequence for the
    //! fixture because every byte is independently verifiable from
    //! the RFC's worked-example tables without re-running header
    //! protection — addendum-4 requirement: "Add a comment in the
    //! fixture pointing to the exact RFC section so a future reader
    //! can re-verify."
    //!
    //! RFC 9001 has no §A.x Handshake worked example; the Handshake
    //! fixture is hand-built from RFC 9000 §17.2.4.
    use super::*;

    // RFC 9001 §A.2 ("Client Initial" cleartext header):
    //   byte0      = 0xc3 (Long + Fixed + Initial + reserved=0 + PN-len=2)
    //   version    = 0x00000001 (QUIC v1)
    //   dcid_len   = 0x08
    //   dcid       = 0x8394c8f03e515708
    //   scid_len   = 0x00
    //   token_len  = varint 0x00 (1 byte)
    //   length     = varint 0x449e (2 bytes; declared 1182)
    //   PN         = 2 bytes (0x0000_0002 packed) — encrypted payload follows
    //
    // We hand-build the public-header prefix verbatim, then pad the
    // remainder so the declared `length` of 1182 lines up with the
    // remaining buffer. The parser does NOT decode the encrypted
    // payload — it only checks `remaining >= declared`.
    fn rfc9001_a2_initial() -> Vec<u8> {
        let mut pkt = Vec::with_capacity(1200);
        pkt.push(0xc3);
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        pkt.push(0x08);
        pkt.extend_from_slice(&[0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08]);
        pkt.push(0x00);
        pkt.push(0x00); // token length varint = 0
        pkt.extend_from_slice(&[0x44, 0x9e]); // length varint = 1182
        // declared length 1182 — pad so remaining >= 1182.
        pkt.resize(pkt.len() + 1182, 0u8);
        pkt
    }

    #[test]
    fn rfc9001_a2_initial_parses() {
        let pkt = rfc9001_a2_initial();
        let parsed = parse_public_header(&pkt, 0).expect("RFC §A.2 Initial parses");
        match parsed {
            PublicHeader::Long {
                ty,
                version,
                dcid,
                scid,
                token,
                length,
            } => {
                assert_eq!(ty, LongType::Initial);
                assert_eq!(version, 0x0000_0001);
                assert_eq!(dcid, &[0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08][..]);
                let empty: &[u8] = &[];
                assert_eq!(scid, empty);
                assert_eq!(token, Some(empty));
                assert_eq!(length, Some(1182));
            }
            PublicHeader::Short { .. } => panic!("expected Long"),
        }
    }

    // Hand-built Handshake per RFC 9000 §17.2.4. There's no RFC 9001
    // §A.x Handshake worked example to copy from; we synthesize a
    // minimal valid packet:
    //   byte0      = 0xe3 (Long=1, Fixed=1, Type=10 Handshake, PN-len=2)
    //   version    = 0x00000001
    //   dcid_len   = 0x04, dcid = 0xdeadbeef
    //   scid_len   = 0x04, scid = 0xcafef00d
    //   length     = varint 0x0010 (16 bytes) — then 16 bytes of pad.
    fn handcrafted_handshake() -> Vec<u8> {
        let mut pkt = Vec::new();
        pkt.push(0xe3);
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        pkt.push(0x04);
        pkt.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        pkt.push(0x04);
        pkt.extend_from_slice(&[0xca, 0xfe, 0xf0, 0x0d]);
        pkt.extend_from_slice(&[0x40, 0x10]); // length varint = 16
        pkt.extend_from_slice(&[0u8; 16]);
        pkt
    }

    #[test]
    fn handshake_parses() {
        let pkt = handcrafted_handshake();
        let parsed = parse_public_header(&pkt, 0).expect("Handshake parses");
        match parsed {
            PublicHeader::Long {
                ty,
                version,
                dcid,
                scid,
                token,
                length,
            } => {
                assert_eq!(ty, LongType::Handshake);
                assert_eq!(version, 0x0000_0001);
                assert_eq!(dcid, &[0xde, 0xad, 0xbe, 0xef][..]);
                assert_eq!(scid, &[0xca, 0xfe, 0xf0, 0x0d][..]);
                assert!(token.is_none());
                assert_eq!(length, Some(16));
            }
            PublicHeader::Short { .. } => panic!("expected Long"),
        }
    }

    #[test]
    fn empty_buffer_is_too_short() {
        match parse_public_header(&[], 0) {
            Err(HeaderError::TooShort { needed: 1, got: 0 }) => {}
            other => panic!("expected TooShort, got {other:?}"),
        }
    }

    #[test]
    fn fixed_bit_clear_long_rejected() {
        // Long-header byte0 with Fixed Bit cleared: 0x80 (form=1, fixed=0).
        let pkt = [0x80u8];
        match parse_public_header(&pkt, 0) {
            Err(HeaderError::FixedBitClear) => {}
            other => panic!("expected FixedBitClear, got {other:?}"),
        }
    }

    #[test]
    fn fixed_bit_clear_short_rejected() {
        // Short-header byte0 with Fixed Bit cleared: 0x00.
        let pkt = [0x00u8; 8];
        match parse_public_header(&pkt, 4) {
            Err(HeaderError::FixedBitClear) => {}
            other => panic!("expected FixedBitClear, got {other:?}"),
        }
    }

    #[test]
    fn dcid_too_long_rejected() {
        // Long header with dcid_len = 21.
        let mut pkt = vec![0xc3];
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        pkt.push(21);
        match parse_public_header(&pkt, 0) {
            Err(HeaderError::DcidTooLong(21)) => {}
            other => panic!("expected DcidTooLong(21), got {other:?}"),
        }
    }

    #[test]
    fn scid_too_long_rejected() {
        // Long header with valid dcid_len=0, scid_len = 21.
        let mut pkt = vec![0xc3];
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        pkt.push(0); // dcid_len
        pkt.push(21); // scid_len
        match parse_public_header(&pkt, 0) {
            Err(HeaderError::ScidTooLong(21)) => {}
            other => panic!("expected ScidTooLong(21), got {other:?}"),
        }
    }

    #[test]
    fn initial_truncated_in_token_len_varint() {
        // Long Initial with dcid_len=0, scid_len=0, then EOF where the
        // token-length varint should be.
        let pkt = [0xc3u8, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00];
        match parse_public_header(&pkt, 0) {
            Err(HeaderError::Varint(VarintError::Incomplete { .. })) => {}
            other => panic!("expected Varint(Incomplete), got {other:?}"),
        }
    }

    #[test]
    fn initial_declared_length_overruns() {
        // Token len = 0, length varint declares 1000 but only 4 bytes
        // remain.
        let mut pkt = vec![0xc3];
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        pkt.push(0); // dcid_len
        pkt.push(0); // scid_len
        pkt.push(0); // token_len = 0
        // length varint = 1000 → 2-byte form 0x43e8.
        pkt.extend_from_slice(&[0x43, 0xe8]);
        pkt.extend_from_slice(&[0u8; 4]);
        match parse_public_header(&pkt, 0) {
            Err(HeaderError::Truncated {
                declared: 1000,
                remaining: 4,
            }) => {}
            other => panic!("expected Truncated, got {other:?}"),
        }
    }

    #[test]
    fn short_header_positive() {
        // Short header: byte0 = 0x40 (form=0, fixed=1), then 8-byte DCID.
        let mut pkt = vec![0x40u8];
        pkt.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let parsed = parse_public_header(&pkt, 8).expect("short parses");
        match parsed {
            PublicHeader::Short { dcid } => {
                assert_eq!(dcid, &[1, 2, 3, 4, 5, 6, 7, 8][..]);
            }
            PublicHeader::Long { .. } => panic!("expected Short"),
        }
    }

    #[test]
    fn short_header_zero_len_rejected() {
        let pkt = [0x40u8];
        match parse_public_header(&pkt, 0) {
            Err(HeaderError::DcidTooLong(0)) => {}
            other => panic!("expected DcidTooLong(0), got {other:?}"),
        }
    }

    #[test]
    fn short_header_too_short_for_dcid() {
        // Declare 8-byte DCID but only have 4 bytes in the buffer
        // (byte0 + 3 DCID bytes).
        let pkt = [0x40u8, 1, 2, 3];
        match parse_public_header(&pkt, 8) {
            Err(HeaderError::TooShort { needed: 9, got: 4 }) => {}
            other => panic!("expected TooShort, got {other:?}"),
        }
    }

    #[test]
    fn version_negotiation_overrides_type_bits() {
        // Long header with version = 0x00000000; type bits irrelevant.
        let mut pkt = vec![0xc3];
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
        pkt.push(4);
        pkt.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        pkt.push(0); // scid_len
        let parsed = parse_public_header(&pkt, 0).expect("VN parses");
        match parsed {
            PublicHeader::Long {
                ty,
                version,
                length,
                token,
                ..
            } => {
                assert_eq!(ty, LongType::VersionNegotiation);
                assert_eq!(version, 0);
                assert!(length.is_none());
                assert!(token.is_none());
            }
            PublicHeader::Short { .. } => panic!("expected Long"),
        }
    }

    #[test]
    fn retry_classified_no_length() {
        // Long header type bits = 0b11 (Retry), non-VN version.
        let mut pkt = vec![0xf0];
        pkt.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
        pkt.push(0); // dcid_len
        pkt.push(0); // scid_len
        // Retry tail is opaque — parser does not consume.
        pkt.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef]);
        let parsed = parse_public_header(&pkt, 0).expect("Retry parses");
        match parsed {
            PublicHeader::Long {
                ty, length, token, ..
            } => {
                assert_eq!(ty, LongType::Retry);
                assert!(length.is_none());
                assert!(token.is_none());
            }
            PublicHeader::Short { .. } => panic!("expected Long"),
        }
    }

    // SHARED-V — RFC 9000 §16 varint sanity. Locks the encode boundary
    // between 1/2/4/8-byte forms.
    #[test]
    fn varint_one_byte() {
        let (v, n) = decode_varint(&[0x00]).unwrap();
        assert_eq!((v, n), (0, 1));
        let (v, n) = decode_varint(&[0x3f]).unwrap();
        assert_eq!((v, n), (63, 1));
    }

    #[test]
    fn varint_two_byte() {
        let (v, n) = decode_varint(&[0x40, 0x40]).unwrap();
        assert_eq!((v, n), (64, 2));
        let (v, n) = decode_varint(&[0x7f, 0xff]).unwrap();
        assert_eq!((v, n), (16_383, 2));
    }

    #[test]
    fn varint_four_byte() {
        let (v, n) = decode_varint(&[0x80, 0x00, 0x40, 0x00]).unwrap();
        assert_eq!((v, n), (16_384, 4));
    }

    #[test]
    fn varint_eight_byte() {
        let (v, n) = decode_varint(&[0xc0, 0, 0, 0, 0x40, 0, 0, 0]).unwrap();
        assert_eq!((v, n), (1 << 30, 8));
    }

    #[test]
    fn varint_incomplete() {
        match decode_varint(&[0x40]) {
            Err(VarintError::Incomplete { needed: 2, got: 1 }) => {}
            other => panic!("expected Incomplete, got {other:?}"),
        }
    }
}
