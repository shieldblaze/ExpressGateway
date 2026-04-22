//! Zstandard compression roundtrip tests.

use lb_compression::*;

#[test]
fn test_compression_zstd_roundtrip() {
    let original = b"Hello, world! This is a test of zstd compression with repeated data repeated data repeated data.";
    let mut c = Compressor::new(Algorithm::Zstd, None).unwrap();
    let mut compressed = c.compress(original).unwrap();
    compressed.extend(c.finish().unwrap());
    assert!(compressed.len() < original.len()); // actually compressed
    let mut d = Decompressor::new(Algorithm::Zstd).unwrap();
    let decompressed = d.decompress(&compressed, 1_000_000, 1000.0).unwrap();
    assert_eq!(decompressed, original);
}
