//! JA3 TLS fingerprinting.
//!
//! Parses TLS ClientHello messages to extract a JA3 fingerprint consisting of:
//! - TLS version
//! - Cipher suites (excluding GREASE)
//! - Extensions (excluding GREASE)
//! - Supported groups / elliptic curves (excluding GREASE)
//! - EC point formats
//!
//! The fingerprint string is hashed with MD5 to produce the JA3 hash.

use dashmap::DashMap;
use md5::{Digest, Md5};
use tracing::trace;

/// A JA3 fingerprint extracted from a TLS ClientHello.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ja3Fingerprint {
    /// Raw fingerprint string: `version,ciphers,extensions,groups,formats`
    pub raw: String,
    /// MD5 hex digest of the raw fingerprint.
    pub hash: String,
}

/// Extracts JA3 fingerprints from TLS ClientHello messages.
pub struct Ja3Extractor;

impl Ja3Extractor {
    /// Extract a JA3 fingerprint from a TLS ClientHello record.
    ///
    /// Expects the raw TLS record bytes starting from the TLS record header
    /// (content type byte). Returns `None` if the bytes don't represent a
    /// valid ClientHello.
    pub fn extract(client_hello: &[u8]) -> Option<Ja3Fingerprint> {
        let reader = &mut Reader::new(client_hello);

        // TLS Record Layer
        let content_type = reader.read_u8()?;
        if content_type != 0x16 {
            // Not a Handshake record
            return None;
        }

        let _record_version = reader.read_u16()?;
        let _record_length = reader.read_u16()?;

        // Handshake header
        let handshake_type = reader.read_u8()?;
        if handshake_type != 0x01 {
            // Not ClientHello
            return None;
        }

        let _handshake_length = reader.read_u24()?;

        // ClientHello body
        let tls_version = reader.read_u16()?;

        // Random (32 bytes)
        reader.skip(32)?;

        // Session ID
        let session_id_len = reader.read_u8()? as usize;
        reader.skip(session_id_len)?;

        // Cipher Suites
        let cipher_suites_len = reader.read_u16()? as usize;
        let cipher_suites_end = reader.pos + cipher_suites_len;
        let mut cipher_suites = Vec::new();
        while reader.pos < cipher_suites_end {
            let suite = reader.read_u16()?;
            if !Self::is_grease(suite) {
                cipher_suites.push(suite);
            }
        }

        // Compression Methods
        let comp_methods_len = reader.read_u8()? as usize;
        reader.skip(comp_methods_len)?;

        // Extensions
        let mut extensions = Vec::new();
        let mut supported_groups = Vec::new();
        let mut ec_point_formats = Vec::new();

        if reader.remaining() > 2 {
            let extensions_len = reader.read_u16()? as usize;
            let extensions_end = reader.pos + extensions_len;

            while reader.pos < extensions_end {
                let ext_type = reader.read_u16()?;
                let ext_len = reader.read_u16()? as usize;
                let ext_data_start = reader.pos;

                if !Self::is_grease(ext_type) {
                    extensions.push(ext_type);
                }

                // Parse specific extensions
                match ext_type {
                    0x000a => {
                        // supported_groups (elliptic_curves)
                        if ext_len >= 2 {
                            let list_len = reader.read_u16()? as usize;
                            let list_end = reader.pos + list_len;
                            while reader.pos < list_end {
                                let group = reader.read_u16()?;
                                if !Self::is_grease(group) {
                                    supported_groups.push(group);
                                }
                            }
                        }
                    }
                    0x000b => {
                        // ec_point_formats
                        if ext_len >= 1 {
                            let list_len = reader.read_u8()? as usize;
                            let list_end = reader.pos + list_len;
                            while reader.pos < list_end {
                                let format = reader.read_u8()? as u16;
                                ec_point_formats.push(format);
                            }
                        }
                    }
                    _ => {}
                }

                // Advance to end of this extension
                reader.pos = ext_data_start + ext_len;
            }
        }

        // Build JA3 string: version,ciphers,extensions,groups,formats
        let raw = format!(
            "{},{},{},{},{}",
            tls_version,
            join_u16(&cipher_suites),
            join_u16(&extensions),
            join_u16(&supported_groups),
            join_u16(&ec_point_formats),
        );

        let hash = md5_hex(&raw);

        trace!(ja3_raw = %raw, ja3_hash = %hash, "extracted JA3 fingerprint");

        Some(Ja3Fingerprint { raw, hash })
    }

    /// Check if a TLS value is a GREASE (Generate Random Extensions And Sustain
    /// Extensibility) value per RFC 8701.
    ///
    /// GREASE values match the pattern `0x?A?A` where `?` is the same nibble:
    /// 0x0A0A, 0x1A1A, 0x2A2A, ..., 0xFAFA
    pub fn is_grease(value: u16) -> bool {
        let hi = (value >> 8) as u8;
        let lo = (value & 0xFF) as u8;
        // Both bytes must be equal and match the pattern 0x?A
        hi == lo && (hi & 0x0F) == 0x0A
    }
}

/// Thread-safe JA3 block list.
///
/// Stores MD5 hashes of known-bad JA3 fingerprints and provides O(1)
/// lookup to check if a fingerprint is blocked.
pub struct Ja3BlockList {
    blocked: DashMap<String, ()>,
}

impl Ja3BlockList {
    /// Create a new empty block list.
    pub fn new() -> Self {
        Self {
            blocked: DashMap::new(),
        }
    }

    /// Add a JA3 hash to the block list.
    pub fn add(&self, hash: impl Into<String>) {
        self.blocked.insert(hash.into(), ());
    }

    /// Remove a JA3 hash from the block list.
    pub fn remove(&self, hash: &str) -> bool {
        self.blocked.remove(hash).is_some()
    }

    /// Check if a fingerprint is blocked.
    pub fn is_blocked(&self, fingerprint: &Ja3Fingerprint) -> bool {
        self.blocked.contains_key(&fingerprint.hash)
    }

    /// Check if a hash is blocked.
    pub fn is_hash_blocked(&self, hash: &str) -> bool {
        self.blocked.contains_key(hash)
    }

    /// Number of entries in the block list.
    pub fn len(&self) -> usize {
        self.blocked.len()
    }

    /// Whether the block list is empty.
    pub fn is_empty(&self) -> bool {
        self.blocked.is_empty()
    }
}

impl Default for Ja3BlockList {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn join_u16(values: &[u16]) -> String {
    values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("-")
}

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let result = hasher.finalize();
    // Format as lowercase hex
    result.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Simple byte reader for parsing TLS records.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_u8(&mut self) -> Option<u8> {
        if self.pos >= self.data.len() {
            return None;
        }
        let val = self.data[self.pos];
        self.pos += 1;
        Some(val)
    }

    fn read_u16(&mut self) -> Option<u16> {
        if self.pos + 2 > self.data.len() {
            return None;
        }
        let val = u16::from_be_bytes([self.data[self.pos], self.data[self.pos + 1]]);
        self.pos += 2;
        Some(val)
    }

    fn read_u24(&mut self) -> Option<u32> {
        if self.pos + 3 > self.data.len() {
            return None;
        }
        let val = (self.data[self.pos] as u32) << 16
            | (self.data[self.pos + 1] as u32) << 8
            | (self.data[self.pos + 2] as u32);
        self.pos += 3;
        Some(val)
    }

    fn skip(&mut self, n: usize) -> Option<()> {
        if self.pos + n > self.data.len() {
            return None;
        }
        self.pos += n;
        Some(())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_grease_detection() {
        // All valid GREASE values
        assert!(Ja3Extractor::is_grease(0x0A0A));
        assert!(Ja3Extractor::is_grease(0x1A1A));
        assert!(Ja3Extractor::is_grease(0x2A2A));
        assert!(Ja3Extractor::is_grease(0x3A3A));
        assert!(Ja3Extractor::is_grease(0x4A4A));
        assert!(Ja3Extractor::is_grease(0x5A5A));
        assert!(Ja3Extractor::is_grease(0x6A6A));
        assert!(Ja3Extractor::is_grease(0x7A7A));
        assert!(Ja3Extractor::is_grease(0x8A8A));
        assert!(Ja3Extractor::is_grease(0x9A9A));
        assert!(Ja3Extractor::is_grease(0xAAAA));
        assert!(Ja3Extractor::is_grease(0xBABA));
        assert!(Ja3Extractor::is_grease(0xCACA));
        assert!(Ja3Extractor::is_grease(0xDADA));
        assert!(Ja3Extractor::is_grease(0xEAEA));
        assert!(Ja3Extractor::is_grease(0xFAFA));

        // Non-GREASE values
        assert!(!Ja3Extractor::is_grease(0x0001)); // TLS_RSA_WITH_NULL_MD5
        assert!(!Ja3Extractor::is_grease(0x00FF));
        assert!(!Ja3Extractor::is_grease(0xC02B)); // TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256
        assert!(!Ja3Extractor::is_grease(0x1301)); // TLS_AES_128_GCM_SHA256
        assert!(!Ja3Extractor::is_grease(0x0A0B)); // Close but not GREASE
        assert!(!Ja3Extractor::is_grease(0x0B0A)); // Reversed nibbles
        assert!(!Ja3Extractor::is_grease(0x0A1A)); // Different high nibbles
    }

    #[test]
    fn test_md5_hex() {
        // Known MD5 test vector
        assert_eq!(md5_hex(""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex("abc"), "900150983cd24fb0d6963f7d28e17f72");
    }

    #[test]
    fn test_ja3_hash_computation() {
        // Build a known JA3 string and verify the hash
        let raw = "769,47-53-5-10-49161-49162-49171-49172-50-56-19-4,0-10-11,23-24-25,0";
        let hash = md5_hex(raw);

        let fp = Ja3Fingerprint {
            raw: raw.to_string(),
            hash: hash.clone(),
        };

        // The hash should be deterministic
        assert_eq!(fp.hash, md5_hex(raw));
        assert_eq!(fp.hash.len(), 32); // MD5 hex is always 32 chars
    }

    #[test]
    fn test_join_u16() {
        assert_eq!(join_u16(&[]), "");
        assert_eq!(join_u16(&[1]), "1");
        assert_eq!(join_u16(&[1, 2, 3]), "1-2-3");
        assert_eq!(join_u16(&[769, 47, 53]), "769-47-53");
    }

    #[test]
    fn test_block_list_basic() {
        let bl = Ja3BlockList::new();
        assert!(bl.is_empty());

        bl.add("abc123");
        assert_eq!(bl.len(), 1);
        assert!(bl.is_hash_blocked("abc123"));
        assert!(!bl.is_hash_blocked("def456"));

        assert!(bl.remove("abc123"));
        assert!(bl.is_empty());
        assert!(!bl.remove("abc123"));
    }

    #[test]
    fn test_block_list_fingerprint_check() {
        let bl = Ja3BlockList::new();
        let fp = Ja3Fingerprint {
            raw: "test".to_string(),
            hash: "098f6bcd4621d373cade4e832627b4f6".to_string(),
        };

        assert!(!bl.is_blocked(&fp));
        bl.add("098f6bcd4621d373cade4e832627b4f6");
        assert!(bl.is_blocked(&fp));
    }

    /// Build a minimal TLS 1.2 ClientHello for testing.
    fn build_test_client_hello(
        tls_version: u16,
        cipher_suites: &[u16],
        extensions: &[(u16, Vec<u8>)],
    ) -> Vec<u8> {
        let mut hello_body = Vec::new();

        // Client version
        hello_body.push((tls_version >> 8) as u8);
        hello_body.push((tls_version & 0xFF) as u8);

        // Random (32 bytes of zeros)
        hello_body.extend_from_slice(&[0u8; 32]);

        // Session ID (empty)
        hello_body.push(0);

        // Cipher suites
        let cs_len = (cipher_suites.len() * 2) as u16;
        hello_body.push((cs_len >> 8) as u8);
        hello_body.push((cs_len & 0xFF) as u8);
        for &cs in cipher_suites {
            hello_body.push((cs >> 8) as u8);
            hello_body.push((cs & 0xFF) as u8);
        }

        // Compression methods (1 byte: null)
        hello_body.push(1);
        hello_body.push(0);

        // Extensions
        if !extensions.is_empty() {
            let mut ext_bytes = Vec::new();
            for &(ext_type, ref ext_data) in extensions {
                ext_bytes.push((ext_type >> 8) as u8);
                ext_bytes.push((ext_type & 0xFF) as u8);
                let ext_data_len = ext_data.len() as u16;
                ext_bytes.push((ext_data_len >> 8) as u8);
                ext_bytes.push((ext_data_len & 0xFF) as u8);
                ext_bytes.extend_from_slice(ext_data);
            }
            let ext_total_len = ext_bytes.len() as u16;
            hello_body.push((ext_total_len >> 8) as u8);
            hello_body.push((ext_total_len & 0xFF) as u8);
            hello_body.extend_from_slice(&ext_bytes);
        }

        // Build full TLS record
        let mut record = Vec::new();
        // Content type: Handshake (0x16)
        record.push(0x16);
        // Record version (TLS 1.0 for the record layer is common)
        record.push(0x03);
        record.push(0x01);

        // Handshake message
        let mut handshake = Vec::new();
        // Handshake type: ClientHello (0x01)
        handshake.push(0x01);
        // Handshake length (3 bytes)
        let hello_len = hello_body.len();
        handshake.push(((hello_len >> 16) & 0xFF) as u8);
        handshake.push(((hello_len >> 8) & 0xFF) as u8);
        handshake.push((hello_len & 0xFF) as u8);
        handshake.extend_from_slice(&hello_body);

        // Record length
        let record_len = handshake.len() as u16;
        record.push((record_len >> 8) as u8);
        record.push((record_len & 0xFF) as u8);
        record.extend_from_slice(&handshake);

        record
    }

    #[test]
    fn test_extract_simple_client_hello() {
        let cipher_suites = vec![0xC02B, 0xC02F, 0x009E, 0x009C];
        let extensions: Vec<(u16, Vec<u8>)> = vec![
            (0x0000, vec![0x00, 0x05, 0x00, 0x03, 0x01, 0x00, 0x00]), // server_name
            (0x0017, vec![]),                                         // extended_master_secret
        ];

        let hello = build_test_client_hello(0x0303, &cipher_suites, &extensions);
        let fp = Ja3Extractor::extract(&hello).expect("should parse ClientHello");

        // TLS version 0x0303 = 771
        assert!(fp.raw.starts_with("771,"));

        // Cipher suites: 49195-49199-158-156
        let parts: Vec<&str> = fp.raw.split(',').collect();
        assert_eq!(parts[0], "771");
        assert_eq!(parts[1], "49195-49199-158-156");
        // Extensions: 0-23
        assert_eq!(parts[2], "0-23");

        // Hash should be 32 hex chars
        assert_eq!(fp.hash.len(), 32);
    }

    #[test]
    fn test_extract_with_grease_filtering() {
        // Include GREASE values that should be filtered out
        let cipher_suites = vec![0x0A0A, 0xC02B, 0x1A1A, 0xC02F];
        let extensions: Vec<(u16, Vec<u8>)> = vec![
            (0x2A2A, vec![]),                                         // GREASE extension
            (0x0000, vec![0x00, 0x05, 0x00, 0x03, 0x01, 0x00, 0x00]), // server_name
        ];

        let hello = build_test_client_hello(0x0303, &cipher_suites, &extensions);
        let fp = Ja3Extractor::extract(&hello).expect("should parse ClientHello");

        let parts: Vec<&str> = fp.raw.split(',').collect();
        // GREASE cipher suites (0x0A0A, 0x1A1A) should be filtered
        assert_eq!(parts[1], "49195-49199");
        // GREASE extension (0x2A2A) should be filtered
        assert_eq!(parts[2], "0");
    }

    #[test]
    fn test_extract_with_supported_groups_and_ec_formats() {
        // supported_groups extension (0x000a)
        let mut groups_data = Vec::new();
        let groups: Vec<u16> = vec![0x0A0A, 0x0017, 0x0018, 0x0019]; // GREASE + real groups
        let groups_len = (groups.len() * 2) as u16;
        groups_data.push((groups_len >> 8) as u8);
        groups_data.push((groups_len & 0xFF) as u8);
        for &g in &groups {
            groups_data.push((g >> 8) as u8);
            groups_data.push((g & 0xFF) as u8);
        }

        // ec_point_formats extension (0x000b)
        let mut formats_data = Vec::new();
        let formats: Vec<u8> = vec![0x00, 0x01, 0x02]; // uncompressed, ansiX962_compressed_prime, ansiX962_compressed_char2
        formats_data.push(formats.len() as u8);
        formats_data.extend_from_slice(&formats);

        let cipher_suites = vec![0xC02B];
        let extensions = vec![(0x000a, groups_data), (0x000b, formats_data)];

        let hello = build_test_client_hello(0x0303, &cipher_suites, &extensions);
        let fp = Ja3Extractor::extract(&hello).expect("should parse ClientHello");

        let parts: Vec<&str> = fp.raw.split(',').collect();
        // Supported groups: 23-24-25 (GREASE 0x0A0A filtered)
        assert_eq!(parts[3], "23-24-25");
        // EC point formats: 0-1-2
        assert_eq!(parts[4], "0-1-2");
    }

    #[test]
    fn test_extract_non_handshake() {
        // Not a handshake record (content type 0x17 = Application Data)
        let data = vec![0x17, 0x03, 0x03, 0x00, 0x05, 0x01, 0x02, 0x03, 0x04, 0x05];
        assert!(Ja3Extractor::extract(&data).is_none());
    }

    #[test]
    fn test_extract_non_client_hello() {
        // Handshake but not ClientHello (type 0x02 = ServerHello)
        let data = vec![
            0x16, 0x03, 0x03, 0x00, 0x05, // TLS record header
            0x02, 0x00, 0x00, 0x01, 0x00, // ServerHello handshake
        ];
        assert!(Ja3Extractor::extract(&data).is_none());
    }

    #[test]
    fn test_extract_truncated() {
        let data = vec![0x16, 0x03];
        assert!(Ja3Extractor::extract(&data).is_none());
    }

    #[test]
    fn test_ja3_hash_is_deterministic() {
        let cipher_suites = vec![0xC02B, 0xC02F];
        let extensions: Vec<(u16, Vec<u8>)> = vec![];

        let hello = build_test_client_hello(0x0303, &cipher_suites, &extensions);

        let fp1 = Ja3Extractor::extract(&hello).unwrap();
        let fp2 = Ja3Extractor::extract(&hello).unwrap();

        assert_eq!(fp1.raw, fp2.raw);
        assert_eq!(fp1.hash, fp2.hash);
    }

    #[test]
    fn test_block_list_concurrent() {
        use std::sync::Arc;
        use std::thread;

        let bl = Arc::new(Ja3BlockList::new());
        let mut handles = Vec::new();

        for i in 0..10 {
            let bl = Arc::clone(&bl);
            handles.push(thread::spawn(move || {
                bl.add(format!("hash_{}", i));
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(bl.len(), 10);
        for i in 0..10 {
            assert!(bl.is_hash_blocked(&format!("hash_{}", i)));
        }
    }
}
