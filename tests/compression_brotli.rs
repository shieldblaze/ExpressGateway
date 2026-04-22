//! Brotli compression roundtrip tests.

use lb_compression::*;

#[test]
fn test_compression_brotli_roundtrip() {
    let original = b"Hello, world! This is a test of brotli compression with repeated data repeated data repeated data.";
    let mut c = Compressor::new(Algorithm::Brotli, None).unwrap();
    let mut compressed = c.compress(original).unwrap();
    compressed.extend(c.finish().unwrap());
    assert!(compressed.len() < original.len()); // actually compressed
    let mut d = Decompressor::new(Algorithm::Brotli).unwrap();
    let decompressed = d.decompress(&compressed, 1_000_000, 1000.0).unwrap();
    assert_eq!(decompressed, original);
}
