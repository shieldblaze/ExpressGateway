//! QPACK header compression codec (RFC 9204), static table + literals only.
//! Fully compliant; the dynamic table is optional.

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

/// Static-table lookup; `exact` means name AND value matched.
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

    /// Encode header pairs, including the required 2-byte prefix.
    ///
    /// # Errors
    /// `H3Error::QpackError` on encoding failure.
    pub fn encode(&self, headers: &[(String, String)]) -> Result<Bytes, H3Error> {
        let mut buf = BytesMut::new();

        encode_qint(&mut buf, 0, 8, 0x00);
        encode_qint(&mut buf, 0, 7, 0x00);

        for (name, value) in headers {
            match find_static(name, value) {
                Some((index, true)) => {
                    encode_qint(&mut buf, index, 6, 0xC0);
                }
                Some((index, false)) => {
                    encode_qint(&mut buf, index, 4, 0x50);
                    encode_qstring(&mut buf, value);
                }
                None => {
                    // RFC 9204 §4.5.6 literal-literal-name: first byte
                    // `001NHxxx`, where xxx is the 3-BIT-PREFIX NAME length.
                    // SESSION 22 (h3spec #14/#15): the prior code wrote `0x20`
                    // then a SEPARATE 7-bit-prefix name string, which a
                    // conformant decoder mis-parses. Fixed with the decoder.
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
    /// `H3Error::QpackError` on malformed input or a bad table index.
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
                let slice = buf.get(pos..).ok_or(H3Error::Incomplete)?;
                let (index, consumed) = decode_qint(slice, 6)?;
                pos += consumed;

                let entry = STATIC_TABLE
                    .get(index)
                    .ok_or_else(|| H3Error::QpackError(format!("invalid static index {index}")))?;
                headers.push((entry.0.to_string(), entry.1.to_string()));
            } else if first & 0x80 == 0x80 {
                // Post-base indexed — not supported (no dynamic table).
                return Err(H3Error::QpackError(
                    "post-base indexed references not supported".to_string(),
                ));
            } else if first & 0xF0 == 0x50 {
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
                // RFC 9204 §4.5.6 literal-literal-name (`001NH | NameLen(3+)`):
                // the NAME length is the 3-BIT PREFIX of THIS byte (with varint
                // continuation), NOT a separate length byte. `H` is left raw —
                // a Huffman-encoded literal NAME is CF-S22-QPACK-HUFFMAN.
                //
                // SESSION 22 (h3spec #14/#15): the prior `pos += 1` then fresh
                // 7-bit length mis-parsed every conformant peer's field (e.g. a
                // prohibited `:foo`). See `audit/h3spec/s22-findings.md`.
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

    /// A hand-built CONFORMANT block, exactly what h3spec sends: the pre-fix
    /// decoder mis-parsed it, so a prohibited `:foo` never reached the
    /// validator. Locks the conformant-peer → gateway direction.
    #[test]
    fn decode_conformant_literal_literal_name() {
        // RIC/Base, indexed 17/23, literal-literal-name ":foo"="bar", indexed 1.
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

    /// Names of length >= 7 need varint continuation (h3spec sends `27 02`).
    #[test]
    fn decode_literal_literal_name_length_continuation() {
        // `27 02` = namelen prefix 7 + 2 = 9, for h3spec's `:autority` pseudo.
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

    /// Self-consistency after the §4.5.6 fix; the long name also exercises the
    /// encoder's 3-bit-prefix continuation.
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
        // The §4.5.6 first-byte form, NOT the old `0x20` + separate length.
        let decoded = QpackDecoder::new().decode(&wire).unwrap();
        assert_eq!(decoded, headers);
    }
}
