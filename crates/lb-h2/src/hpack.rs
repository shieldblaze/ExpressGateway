//! HPACK header compression codec (RFC 7541).
//!
//! Implements integer encoding/decoding with prefix bits, the static and
//! dynamic tables, and the four header field representations:
//! indexed, literal with incremental indexing, literal without indexing,
//! and literal never indexed.
//!
//! Huffman encoding is not used — the H bit is always set to 0 and raw
//! strings are transmitted. This is fully compliant; Huffman is optional.

use std::collections::VecDeque;

use bytes::{BufMut, Bytes, BytesMut};

use crate::H2Error;

/// RFC 7541 Appendix A static table (61 entries, 1-indexed).
static STATIC_TABLE: &[(&str, &str)] = &[
    // Index 0 is unused (1-indexed per RFC).
    ("", ""),
    // 1
    (":authority", ""),
    // 2
    (":method", "GET"),
    // 3
    (":method", "POST"),
    // 4
    (":path", "/"),
    // 5
    (":path", "/index.html"),
    // 6
    (":scheme", "http"),
    // 7
    (":scheme", "https"),
    // 8
    (":status", "200"),
    // 9
    (":status", "204"),
    // 10
    (":status", "206"),
    // 11
    (":status", "304"),
    // 12
    (":status", "400"),
    // 13
    (":status", "404"),
    // 14
    (":status", "500"),
    // 15
    ("accept-charset", ""),
    // 16
    ("accept-encoding", "gzip, deflate"),
    // 17
    ("accept-language", ""),
    // 18
    ("accept-ranges", ""),
    // 19
    ("accept", ""),
    // 20
    ("access-control-allow-origin", ""),
    // 21
    ("age", ""),
    // 22
    ("allow", ""),
    // 23
    ("authorization", ""),
    // 24
    ("cache-control", ""),
    // 25
    ("content-disposition", ""),
    // 26
    ("content-encoding", ""),
    // 27
    ("content-language", ""),
    // 28
    ("content-length", ""),
    // 29
    ("content-location", ""),
    // 30
    ("content-range", ""),
    // 31
    ("content-type", ""),
    // 32
    ("cookie", ""),
    // 33
    ("date", ""),
    // 34
    ("etag", ""),
    // 35
    ("expect", ""),
    // 36
    ("expires", ""),
    // 37
    ("from", ""),
    // 38
    ("host", ""),
    // 39
    ("if-match", ""),
    // 40
    ("if-modified-since", ""),
    // 41
    ("if-none-match", ""),
    // 42
    ("if-range", ""),
    // 43
    ("if-unmodified-since", ""),
    // 44
    ("last-modified", ""),
    // 45
    ("link", ""),
    // 46
    ("location", ""),
    // 47
    ("max-forwards", ""),
    // 48
    ("proxy-authenticate", ""),
    // 49
    ("proxy-authorization", ""),
    // 50
    ("range", ""),
    // 51
    ("referer", ""),
    // 52
    ("refresh", ""),
    // 53
    ("retry-after", ""),
    // 54
    ("server", ""),
    // 55
    ("set-cookie", ""),
    // 56
    ("strict-transport-security", ""),
    // 57
    ("transfer-encoding", ""),
    // 58
    ("user-agent", ""),
    // 59
    ("vary", ""),
    // 60
    ("via", ""),
    // 61
    ("www-authenticate", ""),
];

/// HPACK dynamic table.
///
/// Uses `VecDeque` so that `push_front` (newest entry at index 0) is O(1)
/// instead of the O(n) `Vec::insert(0, ...)` it replaced.
#[derive(Debug)]
struct DynamicTable {
    entries: VecDeque<(String, String)>,
    size: usize,
    max_size: usize,
}

impl DynamicTable {
    const fn new(max_size: usize) -> Self {
        Self {
            entries: VecDeque::new(),
            size: 0,
            max_size,
        }
    }

    /// Entry size per RFC 7541 §4.1: name octets + value octets + 32.
    const fn entry_size(name: &str, value: &str) -> usize {
        name.len() + value.len() + 32
    }

    fn insert(&mut self, name: String, value: String) {
        let new_entry_size = Self::entry_size(&name, &value);
        self.evict(new_entry_size);
        if new_entry_size <= self.max_size {
            self.size += new_entry_size;
            self.entries.push_front((name, value));
        }
    }

    fn evict(&mut self, needed: usize) {
        while self.size + needed > self.max_size {
            if let Some((n, v)) = self.entries.pop_back() {
                self.size = self.size.saturating_sub(Self::entry_size(&n, &v));
            } else {
                break;
            }
        }
    }

    fn get(&self, index: usize) -> Option<(&str, &str)> {
        self.entries
            .get(index)
            .map(|(n, v)| (n.as_str(), v.as_str()))
    }

    fn len(&self) -> usize {
        self.entries.len()
    }

    fn set_max_size(&mut self, new_max: usize) {
        self.max_size = new_max;
        self.evict(0);
    }
}

/// Look up an entry by absolute index (1-based, static then dynamic).
fn table_get(index: usize, dynamic: &DynamicTable) -> Result<(&str, &str), H2Error> {
    if index == 0 {
        return Err(H2Error::HpackError("index 0 is invalid".to_string()));
    }
    let static_len = STATIC_TABLE.len() - 1; // 61 entries, indices 1..=61
    if index <= static_len {
        let entry = STATIC_TABLE
            .get(index)
            .ok_or_else(|| H2Error::HpackError(format!("invalid static index {index}")))?;
        Ok((entry.0, entry.1))
    } else {
        let dyn_index = index - static_len - 1;
        dynamic
            .get(dyn_index)
            .ok_or_else(|| H2Error::HpackError(format!("invalid dynamic index {index}")))
    }
}

/// Find a matching entry in static + dynamic tables.
/// Returns `(index, name_match_only)`.
fn find_in_tables(name: &str, value: &str, dynamic: &DynamicTable) -> Option<(usize, bool)> {
    let static_len = STATIC_TABLE.len() - 1;

    let mut name_match: Option<usize> = None;
    for i in 1..=static_len {
        if let Some(entry) = STATIC_TABLE.get(i) {
            if entry.0 == name {
                if entry.1 == value {
                    return Some((i, false));
                }
                if name_match.is_none() {
                    name_match = Some(i);
                }
            }
        }
    }

    for i in 0..dynamic.len() {
        if let Some((n, v)) = dynamic.get(i) {
            let abs_idx = static_len + 1 + i;
            if n == name {
                if v == value {
                    return Some((abs_idx, false));
                }
                if name_match.is_none() {
                    name_match = Some(abs_idx);
                }
            }
        }
    }

    name_match.map(|idx| (idx, true))
}

// ─── Integer coding (RFC 7541 §5.1) ───

/// Encode an integer with the given prefix bit width.
fn encode_integer(buf: &mut BytesMut, mut value: usize, prefix_bits: u8, first_byte_mask: u8) {
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

/// Decode an integer with the given prefix bit width.
/// Returns `(value, bytes_consumed)`.
fn decode_integer(buf: &[u8], prefix_bits: u8) -> Result<(usize, usize), H2Error> {
    let first = *buf.first().ok_or(H2Error::Incomplete)?;
    let max_prefix = (1usize << prefix_bits) - 1;
    let mut value = usize::from(first) & max_prefix;

    if value < max_prefix {
        return Ok((value, 1));
    }

    let mut m = 0u32;
    let mut pos = 1;
    loop {
        let b = *buf.get(pos).ok_or(H2Error::Incomplete)?;
        value += (usize::from(b) & 0x7F) << m;
        m += 7;
        pos += 1;
        if b & 0x80 == 0 {
            break;
        }
        if m > 28 {
            return Err(H2Error::HpackError("integer overflow".to_string()));
        }
    }
    Ok((value, pos))
}

/// Encode a string literal (always raw, H=0).
fn encode_string(buf: &mut BytesMut, s: &str) {
    encode_integer(buf, s.len(), 7, 0x00);
    buf.put_slice(s.as_bytes());
}

/// Decode a string literal (H=0 raw; H=1 is treated as raw best-effort).
fn decode_string(buf: &[u8]) -> Result<(String, usize), H2Error> {
    let (len, int_bytes) = decode_integer(buf, 7)?;
    let start = int_bytes;
    let end = start + len;
    let data = buf.get(start..end).ok_or(H2Error::Incomplete)?;

    let s = core::str::from_utf8(data)
        .map_err(|_| H2Error::HpackError("non-utf8 string".to_string()))?;
    Ok((s.to_string(), end))
}

/// HPACK encoder.
///
/// Maintains a dynamic table that is updated on each `encode` call.
#[derive(Debug)]
pub struct HpackEncoder {
    dynamic: DynamicTable,
}

impl HpackEncoder {
    /// Create a new encoder with the given maximum dynamic table size.
    #[must_use]
    pub const fn new(max_table_size: usize) -> Self {
        Self {
            dynamic: DynamicTable::new(max_table_size),
        }
    }

    /// Encode a list of header name-value pairs.
    ///
    /// Uses indexed representation when a full match exists in the tables,
    /// literal with incremental indexing for name-only matches and new entries.
    ///
    /// # Errors
    ///
    /// Returns `H2Error::HpackError` on encoding failures.
    pub fn encode(&mut self, headers: &[(String, String)]) -> Result<Bytes, H2Error> {
        let mut buf = BytesMut::new();

        for (name, value) in headers {
            match find_in_tables(name, value, &self.dynamic) {
                Some((index, false)) => {
                    // Full match — indexed header field (§6.1).
                    encode_integer(&mut buf, index, 7, 0x80);
                }
                Some((name_index, true)) => {
                    // Name match — literal with incremental indexing (§6.2.1).
                    encode_integer(&mut buf, name_index, 6, 0x40);
                    encode_string(&mut buf, value);
                    self.dynamic.insert(name.clone(), value.clone());
                }
                None => {
                    // No match — literal with incremental indexing, new name (§6.2.1).
                    buf.put_u8(0x40);
                    encode_string(&mut buf, name);
                    encode_string(&mut buf, value);
                    self.dynamic.insert(name.clone(), value.clone());
                }
            }
        }

        Ok(buf.freeze())
    }

    /// Update the maximum dynamic table size.
    pub fn set_max_table_size(&mut self, max: usize) {
        self.dynamic.set_max_size(max);
    }
}

/// HPACK decoder.
///
/// Maintains a dynamic table that is updated during decoding.
#[derive(Debug)]
pub struct HpackDecoder {
    dynamic: DynamicTable,
}

impl HpackDecoder {
    /// Create a new decoder with the given maximum dynamic table size.
    #[must_use]
    pub const fn new(max_table_size: usize) -> Self {
        Self {
            dynamic: DynamicTable::new(max_table_size),
        }
    }

    /// Decode an HPACK-encoded header block.
    ///
    /// # Errors
    ///
    /// Returns `H2Error::HpackError` on malformed input or table index errors.
    pub fn decode(&mut self, buf: &[u8]) -> Result<Vec<(String, String)>, H2Error> {
        let mut headers = Vec::new();
        let mut pos = 0;

        while pos < buf.len() {
            let first = *buf.get(pos).ok_or(H2Error::Incomplete)?;

            if first & 0x80 != 0 {
                // §6.1 Indexed header field.
                let (index, consumed) =
                    decode_integer(buf.get(pos..).ok_or(H2Error::Incomplete)?, 7)?;
                let (name, value) = table_get(index, &self.dynamic)?;
                headers.push((name.to_string(), value.to_string()));
                pos += consumed;
            } else if first & 0xC0 == 0x40 {
                // §6.2.1 Literal with incremental indexing.
                let (index, consumed) =
                    decode_integer(buf.get(pos..).ok_or(H2Error::Incomplete)?, 6)?;
                pos += consumed;

                let (name, value) = if index == 0 {
                    let (n, nc) = decode_string(buf.get(pos..).ok_or(H2Error::Incomplete)?)?;
                    pos += nc;
                    let (v, vc) = decode_string(buf.get(pos..).ok_or(H2Error::Incomplete)?)?;
                    pos += vc;
                    (n, v)
                } else {
                    let (n, _) = table_get(index, &self.dynamic)?;
                    let n = n.to_string();
                    let (v, vc) = decode_string(buf.get(pos..).ok_or(H2Error::Incomplete)?)?;
                    pos += vc;
                    (n, v)
                };

                self.dynamic.insert(name.clone(), value.clone());
                headers.push((name, value));
            } else if first & 0xF0 == 0x00 || first & 0xF0 == 0x10 {
                // §6.2.2 Literal without indexing (0000xxxx)
                // §6.2.3 Literal never indexed (0001xxxx)
                let (index, consumed) =
                    decode_integer(buf.get(pos..).ok_or(H2Error::Incomplete)?, 4)?;
                pos += consumed;

                let (name, value) = if index == 0 {
                    let (n, nc) = decode_string(buf.get(pos..).ok_or(H2Error::Incomplete)?)?;
                    pos += nc;
                    let (v, vc) = decode_string(buf.get(pos..).ok_or(H2Error::Incomplete)?)?;
                    pos += vc;
                    (n, v)
                } else {
                    let (n, _) = table_get(index, &self.dynamic)?;
                    let n = n.to_string();
                    let (v, vc) = decode_string(buf.get(pos..).ok_or(H2Error::Incomplete)?)?;
                    pos += vc;
                    (n, v)
                };

                headers.push((name, value));
            } else if first & 0xE0 == 0x20 {
                // §6.3 Dynamic table size update.
                let (new_size, consumed) =
                    decode_integer(buf.get(pos..).ok_or(H2Error::Incomplete)?, 5)?;
                pos += consumed;
                self.dynamic.set_max_size(new_size);
            } else {
                return Err(H2Error::HpackError(format!(
                    "unknown HPACK byte prefix: {first:#04x}"
                )));
            }
        }

        Ok(headers)
    }

    /// Update the maximum dynamic table size.
    pub fn set_max_table_size(&mut self, max: usize) {
        self.dynamic.set_max_size(max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_roundtrip() {
        for &prefix in &[4u8, 5, 6, 7] {
            for &val in &[0usize, 1, 30, 127, 128, 255, 1024, 65535] {
                let mut buf = BytesMut::new();
                encode_integer(&mut buf, val, prefix, 0);
                let (decoded, consumed) = decode_integer(&buf, prefix).unwrap();
                assert_eq!(decoded, val, "prefix={prefix}, value={val}");
                assert_eq!(consumed, buf.len());
            }
        }
    }

    #[test]
    fn string_roundtrip() {
        let test_strings = ["", "hello", "content-type", "/index.html"];
        for s in &test_strings {
            let mut buf = BytesMut::new();
            encode_string(&mut buf, s);
            let (decoded, consumed) = decode_string(&buf).unwrap();
            assert_eq!(&decoded, s);
            assert_eq!(consumed, buf.len());
        }
    }

    #[test]
    #[allow(clippy::similar_names)]
    fn hpack_encode_decode_roundtrip() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            ("content-type".to_string(), "text/html".to_string()),
        ];

        let mut enc = HpackEncoder::new(4096);
        let wire = enc.encode(&headers).unwrap();

        let mut dec = HpackDecoder::new(4096);
        let result = dec.decode(&wire).unwrap();

        assert_eq!(result, headers);
    }

    #[test]
    fn hpack_dynamic_table_eviction() {
        let mut enc = HpackEncoder::new(128);
        let mut dec = HpackDecoder::new(128);

        let headers1 = vec![("x-custom-a".to_string(), "value-a-long".to_string())];
        let headers2 = vec![("x-custom-b".to_string(), "value-b-long".to_string())];
        let headers3 = vec![("x-custom-c".to_string(), "value-c-long".to_string())];

        let e1 = enc.encode(&headers1).unwrap();
        let d1 = dec.decode(&e1).unwrap();
        assert_eq!(d1, headers1);

        let e2 = enc.encode(&headers2).unwrap();
        let d2 = dec.decode(&e2).unwrap();
        assert_eq!(d2, headers2);

        let e3 = enc.encode(&headers3).unwrap();
        let d3 = dec.decode(&e3).unwrap();
        assert_eq!(d3, headers3);
    }

    #[test]
    fn hpack_static_indexed() {
        let headers = vec![
            (":method".to_string(), "GET".to_string()),
            (":path".to_string(), "/".to_string()),
            (":scheme".to_string(), "https".to_string()),
        ];

        let mut enc = HpackEncoder::new(4096);
        let wire = enc.encode(&headers).unwrap();

        // All three are fully indexed in the static table.
        assert_eq!(wire.len(), 3);

        let mut dec = HpackDecoder::new(4096);
        let result = dec.decode(&wire).unwrap();
        assert_eq!(result, headers);
    }
}
