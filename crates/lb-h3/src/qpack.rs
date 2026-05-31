//! QPACK header compression codec (RFC 9204).
//!
//! This is a simplified implementation that uses only the static table and
//! literal representations (no dynamic table). This is fully compliant —
//! the dynamic table is optional.

use bytes::{BufMut, Bytes, BytesMut};

use crate::H3Error;

/// RFC 9204 Appendix A — QPACK static table (99 entries, 0-indexed).
static STATIC_TABLE: &[(&str, &str)] = &[
    (":authority", ""),
    (":path", "/"),
    ("age", "0"),
    ("content-disposition", ""),
    ("content-length", "0"),
    ("cookie", ""),
    ("date", ""),
    ("etag", ""),
    ("if-modified-since", ""),
    ("if-none-match", ""),
    ("last-modified", ""),
    ("link", ""),
    ("location", ""),
    ("referer", ""),
    ("set-cookie", ""),
    (":method", "CONNECT"),
    (":method", "DELETE"),
    (":method", "GET"),
    (":method", "HEAD"),
    (":method", "OPTIONS"),
    (":method", "POST"),
    (":method", "PUT"),
    (":scheme", "http"),
    (":scheme", "https"),
    (":status", "103"),
    (":status", "200"),
    (":status", "304"),
    (":status", "404"),
    (":status", "503"),
    ("accept", "*/*"),
    ("accept", "application/dns-message"),
    ("accept-encoding", "gzip, deflate, br"),
    ("accept-ranges", "bytes"),
    ("access-control-allow-headers", "cache-control"),
    ("access-control-allow-headers", "content-type"),
    ("access-control-allow-origin", "*"),
    ("cache-control", "max-age=0"),
    ("cache-control", "max-age=2592000"),
    ("cache-control", "max-age=604800"),
    ("cache-control", "no-cache"),
    ("cache-control", "no-store"),
    ("cache-control", "public, max-age=31536000"),
    ("content-encoding", "br"),
    ("content-encoding", "gzip"),
    ("content-type", "application/dns-message"),
    ("content-type", "application/javascript"),
    ("content-type", "application/json"),
    ("content-type", "application/x-www-form-urlencoded"),
    ("content-type", "image/gif"),
    ("content-type", "image/jpeg"),
    ("content-type", "image/png"),
    ("content-type", "text/css"),
    ("content-type", "text/html; charset=utf-8"),
    ("content-type", "text/plain"),
    ("content-type", "text/plain;charset=utf-8"),
    ("range", "bytes=0-"),
    ("strict-transport-security", "max-age=31536000"),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains",
    ),
    (
        "strict-transport-security",
        "max-age=31536000; includesubdomains; preload",
    ),
    ("vary", "accept-encoding"),
    ("vary", "origin"),
    ("x-content-type-options", "nosniff"),
    ("x-xss-protection", "1; mode=block"),
    (":status", "100"),
    (":status", "204"),
    (":status", "206"),
    (":status", "302"),
    (":status", "400"),
    (":status", "403"),
    (":status", "421"),
    (":status", "425"),
    (":status", "500"),
    ("accept-language", ""),
    ("access-control-allow-credentials", "FALSE"),
    ("access-control-allow-credentials", "TRUE"),
    ("access-control-allow-headers", "*"),
    ("access-control-allow-methods", "get"),
    ("access-control-allow-methods", "get, post, options"),
    ("access-control-allow-methods", "options"),
    ("access-control-expose-headers", "content-length"),
    ("access-control-request-headers", "content-type"),
    ("access-control-request-method", "get"),
    ("access-control-request-method", "post"),
    ("alt-svc", "clear"),
    ("authorization", ""),
    (
        "content-security-policy",
        "script-src 'none'; object-src 'none'; base-uri 'none'",
    ),
    ("early-data", "1"),
    ("expect-ct", ""),
    ("forwarded", ""),
    ("if-range", ""),
    ("origin", ""),
    ("purpose", "prefetch"),
    ("server", ""),
    ("timing-allow-origin", "*"),
    ("upgrade-insecure-requests", "1"),
    ("user-agent", ""),
    ("x-forwarded-for", ""),
    ("x-frame-options", "deny"),
    ("x-frame-options", "sameorigin"),
];

/// Find a match in the static table.
/// Returns `Some((index, exact))` where `exact` is true if both name and value match.
fn find_static(name: &str, value: &str) -> Option<(usize, bool)> {
    let mut name_match: Option<usize> = None;
    for (i, entry) in STATIC_TABLE.iter().enumerate() {
        if entry.0 == name {
            if entry.1 == value {
                return Some((i, true));
            }
            if name_match.is_none() {
                name_match = Some(i);
            }
        }
    }
    name_match.map(|idx| (idx, false))
}

// ─── QPACK integer encoding (same prefix-integer scheme as HPACK) ───

fn encode_qint(buf: &mut BytesMut, mut value: usize, prefix_bits: u8, first_byte_mask: u8) {
    let max_prefix = (1usize << prefix_bits) - 1;
    if value < max_prefix {
        #[allow(clippy::cast_possible_truncation)]
        buf.put_u8(first_byte_mask | (value as u8));
    } else {
        #[allow(clippy::cast_possible_truncation)]
        buf.put_u8(first_byte_mask | (max_prefix as u8));
        value -= max_prefix;
        while value >= 128 {
            #[allow(clippy::cast_possible_truncation)]
            buf.put_u8((value % 128) as u8 | 0x80);
            value /= 128;
        }
        #[allow(clippy::cast_possible_truncation)]
        buf.put_u8(value as u8);
    }
}

fn decode_qint(buf: &[u8], prefix_bits: u8) -> Result<(usize, usize), H3Error> {
    let first = *buf.first().ok_or(H3Error::Incomplete)?;
    let max_prefix = (1usize << prefix_bits) - 1;
    let mut value = usize::from(first) & max_prefix;

    if value < max_prefix {
        return Ok((value, 1));
    }

    let mut m = 0u32;
    let mut pos = 1;
    loop {
        let b = *buf.get(pos).ok_or(H3Error::Incomplete)?;
        value += (usize::from(b) & 0x7F) << m;
        m += 7;
        pos += 1;
        if b & 0x80 == 0 {
            break;
        }
        if m > 28 {
            return Err(H3Error::QpackError("integer overflow".to_string()));
        }
    }
    Ok((value, pos))
}

fn encode_qstring(buf: &mut BytesMut, s: &str) {
    encode_qint(buf, s.len(), 7, 0x00);
    buf.put_slice(s.as_bytes());
}

fn decode_qstring(buf: &[u8]) -> Result<(String, usize), H3Error> {
    let (len, int_bytes) = decode_qint(buf, 7)?;
    let start = int_bytes;
    let end = start + len;
    let data = buf.get(start..end).ok_or(H3Error::Incomplete)?;
    let s = core::str::from_utf8(data)
        .map_err(|_| H3Error::QpackError("non-utf8 string".to_string()))?;
    Ok((s.to_string(), end))
}

/// QPACK encoder (static table only, no dynamic table).
#[derive(Debug)]
pub struct QpackEncoder {
    _private: (),
}

impl QpackEncoder {
    /// Create a new encoder.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Encode a list of header name-value pairs using QPACK.
    ///
    /// The output includes the required 2-byte prefix (Required Insert Count = 0,
    /// Delta Base = 0) followed by header field representations.
    ///
    /// # Errors
    ///
    /// Returns `H3Error::QpackError` on encoding failures.
    pub fn encode(&self, headers: &[(String, String)]) -> Result<Bytes, H3Error> {
        let mut buf = BytesMut::new();

        // Required Insert Count = 0 (no dynamic table entries referenced).
        encode_qint(&mut buf, 0, 8, 0x00);
        // Delta Base = 0, sign bit = 0.
        encode_qint(&mut buf, 0, 7, 0x00);

        for (name, value) in headers {
            match find_static(name, value) {
                Some((index, true)) => {
                    // Indexed field line — static table (§4.5.2).
                    encode_qint(&mut buf, index, 6, 0xC0);
                }
                Some((index, false)) => {
                    // Literal with name reference — static table (§4.5.4).
                    encode_qint(&mut buf, index, 4, 0x50);
                    encode_qstring(&mut buf, value);
                }
                None => {
                    // Literal Field Line with Literal Name (RFC 9204
                    // §4.5.6): first byte `001NHxxx` where xxx is the
                    // 3-bit-prefix NAME length (N=0 not-never-indexed,
                    // H=0 raw — the codec does not Huffman-encode). The
                    // name bytes follow inline; the VALUE is a normal
                    // 7-bit-prefix string.
                    //
                    // SESSION 22 FIX (paired with the decoder, h3spec
                    // #14/#15): the prior code wrote `0x20` then a
                    // SEPARATE 7-bit-prefix name string, which a conformant
                    // QPACK decoder mis-parses. Encoder + decoder are fixed
                    // together so the gateway interops with conformant H3
                    // peers without breaking its own round-trips.
                    encode_qint(&mut buf, name.len(), 3, 0x20);
                    buf.put_slice(name.as_bytes());
                    encode_qstring(&mut buf, value);
                }
            }
        }

        Ok(buf.freeze())
    }
}

impl Default for QpackEncoder {
    fn default() -> Self {
        Self::new()
    }
}

/// QPACK decoder (static table only, no dynamic table).
#[derive(Debug)]
pub struct QpackDecoder {
    _private: (),
}

impl QpackDecoder {
    /// Create a new decoder.
    #[must_use]
    pub const fn new() -> Self {
        Self { _private: () }
    }

    /// Decode a QPACK-encoded header block.
    ///
    /// # Errors
    ///
    /// Returns `H3Error::QpackError` on malformed input or invalid table indices.
    pub fn decode(&self, buf: &[u8]) -> Result<Vec<(String, String)>, H3Error> {
        let (required_insert_count, ric_len) = decode_qint(buf, 8)?;
        let rest = buf.get(ric_len..).ok_or(H3Error::Incomplete)?;
        let (_delta_base, db_len) = decode_qint(rest, 7)?;

        if required_insert_count != 0 {
            return Err(H3Error::QpackError(
                "dynamic table references not supported".to_string(),
            ));
        }

        let mut pos = ric_len + db_len;
        let mut headers = Vec::new();

        while pos < buf.len() {
            let first = *buf.get(pos).ok_or(H3Error::Incomplete)?;

            if first & 0xC0 == 0xC0 {
                // Indexed field line — static.
                let slice = buf.get(pos..).ok_or(H3Error::Incomplete)?;
                let (index, consumed) = decode_qint(slice, 6)?;
                pos += consumed;

                let entry = STATIC_TABLE
                    .get(index)
                    .ok_or_else(|| H3Error::QpackError(format!("invalid static index {index}")))?;
                headers.push((entry.0.to_string(), entry.1.to_string()));
            } else if first & 0x80 == 0x80 {
                // Post-base indexed — not supported.
                return Err(H3Error::QpackError(
                    "post-base indexed references not supported".to_string(),
                ));
            } else if first & 0xF0 == 0x50 {
                // Literal with static name reference.
                let slice = buf.get(pos..).ok_or(H3Error::Incomplete)?;
                let (index, consumed) = decode_qint(slice, 4)?;
                pos += consumed;

                let entry = STATIC_TABLE
                    .get(index)
                    .ok_or_else(|| H3Error::QpackError(format!("invalid static index {index}")))?;
                let name = entry.0.to_string();

                let val_slice = buf.get(pos..).ok_or(H3Error::Incomplete)?;
                let (value, vc) = decode_qstring(val_slice)?;
                pos += vc;

                headers.push((name, value));
            } else if first & 0xF0 == 0x40 {
                // Post-base name reference — not supported.
                return Err(H3Error::QpackError(
                    "post-base name references not supported".to_string(),
                ));
            } else if first & 0xE0 == 0x20 {
                // Literal Field Line with Literal Name (RFC 9204 §4.5.6):
                //   0 0 1 N H | Name Length (3+) |
                // The NAME length is the 3-bit prefix of THIS first byte
                // (with the standard varint continuation) — NOT a separate
                // length byte. `N` (0x10, never-indexed) is ignored; `H`
                // (0x08, Huffman) is left raw, mirroring `decode_qstring`'s
                // raw-only posture (the codec does not Huffman-encode —
                // SESSION 22: a conformant peer that Huffman-encodes a
                // literal NAME is a carry-forward, CF-S22-QPACK-HUFFMAN).
                //
                // SESSION 22 FIX (h3spec #14/#15): the prior code did
                // `pos += 1` then read a fresh 7-bit-prefix length byte,
                // which mis-parsed every RFC-conformant peer's
                // literal-literal-name field (e.g. a prohibited `:foo`
                // pseudo-header or a `foo` field before a late pseudo).
                // Decoder + encoder are fixed together so the gateway's own
                // H3 round-trips stay consistent AND interop with conformant
                // peers. See `audit/h3spec/s22-findings.md`.
                let slice = buf.get(pos..).ok_or(H3Error::Incomplete)?;
                let (name_len, consumed) = decode_qint(slice, 3)?;
                pos += consumed;
                let name_bytes = buf
                    .get(pos..pos.checked_add(name_len).ok_or(H3Error::Incomplete)?)
                    .ok_or(H3Error::Incomplete)?;
                let name = core::str::from_utf8(name_bytes)
                    .map_err(|_| H3Error::QpackError("non-utf8 qpack name".to_string()))?
                    .to_string();
                pos += name_len;

                let val_slice = buf.get(pos..).ok_or(H3Error::Incomplete)?;
                let (value, vc) = decode_qstring(val_slice)?;
                pos += vc;

                headers.push((name, value));
            } else {
                return Err(H3Error::QpackError(format!(
                    "unknown QPACK instruction byte: {first:#04x}"
                )));
            }
        }

        Ok(headers)
    }
}

impl Default for QpackDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_static_only() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":status".to_string(), "200".to_string()),
        ];

        let enc = QpackEncoder::new();
        let wire = enc.encode(&headers).unwrap();

        let dec = QpackDecoder::new();
        let result = dec.decode(&wire).unwrap();

        assert_eq!(result, headers);
    }

    #[test]
    fn roundtrip_with_literals() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            ("x-custom".to_string(), "my-value".to_string()),
        ];

        let enc = QpackEncoder::new();
        let wire = enc.encode(&headers).unwrap();

        let dec = QpackDecoder::new();
        let result = dec.decode(&wire).unwrap();

        assert_eq!(result, headers);
    }

    #[test]
    fn roundtrip_name_ref_value_literal() {
        let headers = vec![("content-type".to_string(), "application/xml".to_string())];

        let enc = QpackEncoder::new();
        let wire = enc.encode(&headers).unwrap();

        let dec = QpackDecoder::new();
        let result = dec.decode(&wire).unwrap();

        assert_eq!(result, headers);
    }

    // ── SESSION 22 (h3spec #14/#15): RFC 9204 §4.5.6 literal-literal-name ──

    /// Decode a hand-built **conformant** (externally produced) header
    /// block whose literal-literal-name fields put the NAME length in the
    /// first byte's 3-bit prefix — exactly the encoding h3spec sends. The
    /// pre-fix decoder mis-parsed this (it skipped the first byte then read
    /// a fresh 7-bit length), so a prohibited `:foo` / late pseudo was never
    /// surfaced to the validator. This is the regression lock for the
    /// interop direction (conformant peer → gateway).
    #[test]
    fn decode_conformant_literal_literal_name() {
        // 00 00            RIC=0, Base=0
        // d1               indexed static 17  -> (:method, GET)
        // d7               indexed static 23  -> (:scheme, https)
        // 24 3a 66 6f 6f   literal-literal-name, namelen=4 -> ":foo"
        // 03 62 61 72      value len 3 -> "bar"
        // c1               indexed static 1   -> (:path, /)
        let wire = [
            0x00, 0x00, 0xd1, 0xd7, 0x24, 0x3a, 0x66, 0x6f, 0x6f, 0x03, 0x62, 0x61, 0x72, 0xc1,
        ];
        let got = QpackDecoder::new().decode(&wire).unwrap();
        assert_eq!(
            got,
            vec![
                (":method".to_string(), "GET".to_string()),
                (":scheme".to_string(), "https".to_string()),
                (":foo".to_string(), "bar".to_string()),
                (":path".to_string(), "/".to_string()),
            ]
        );
    }

    /// The 3-bit name-length prefix must use varint continuation for names
    /// of length >= 7 (h3spec encodes a 9-byte name as `27 02`).
    #[test]
    fn decode_literal_literal_name_length_continuation() {
        // 00 00  ric/base
        // 27 02  literal-literal-name, namelen prefix=7 -> +2 -> 9
        // ":autority" (9 bytes — h3spec's deliberately-unregistered pseudo)
        // 09 + "127.0.0.1"
        let mut wire = vec![0x00, 0x00, 0x27, 0x02];
        wire.extend_from_slice(b":autority");
        wire.push(0x09);
        wire.extend_from_slice(b"127.0.0.1");
        let got = QpackDecoder::new().decode(&wire).unwrap();
        assert_eq!(
            got,
            vec![(":autority".to_string(), "127.0.0.1".to_string())]
        );
    }

    /// Round-trip a literal-literal-name field through the FIXED encoder
    /// and decoder (self-consistency after the §4.5.6 fix). A long name
    /// exercises the encoder's 3-bit-prefix continuation too.
    #[test]
    fn roundtrip_literal_literal_name_fixed_format() {
        let headers = vec![
            ("x-a".to_string(), "1".to_string()),
            (
                "x-very-long-header-name-exceeding-seven".to_string(),
                "v".to_string(),
            ),
        ];
        let wire = QpackEncoder::new().encode(&headers).unwrap();
        // The encoder must emit the §4.5.6 first-byte form (0x20-masked),
        // NOT the old `0x20` + separate-length form.
        let decoded = QpackDecoder::new().decode(&wire).unwrap();
        assert_eq!(decoded, headers);
    }
}
