//! Gzip compression roundtrip tests.

use lb_compression::*;

#[test]
fn test_compression_gzip_roundtrip() {
    let original = b"Hello, world! This is a test of gzip compression with repeated data repeated data repeated data.";
    let mut c = Compressor::new(Algorithm::Gzip, None).unwrap();
    let mut compressed = c.compress(original).unwrap();
    compressed.extend(c.finish().unwrap());
    assert!(compressed.len() < original.len()); // actually compressed
    let mut d = Decompressor::new(Algorithm::Gzip).unwrap();
    let decompressed = d.decompress(&compressed, 1_000_000, 1000.0).unwrap();
    assert_eq!(decompressed, original);
}
