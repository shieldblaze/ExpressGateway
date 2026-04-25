//! Compression transcoding tests.

use lb_compression::*;

#[test]
fn test_compression_transcode_gzip_to_zstd() {
    let original = b"Transcode test data with enough content to compress well well well well well.";
    // First compress with gzip
    let mut c = Compressor::new(Algorithm::Gzip, None).unwrap();
    let mut gzipped = c.compress(original).unwrap();
    gzipped.extend(c.finish().unwrap());
    // Transcode gzip -> zstd
    let zstd_data = transcode(
        &gzipped,
        Algorithm::Gzip,
        Algorithm::Zstd,
        1_000_000,
        1000.0,
    )
    .unwrap();
    // Decompress zstd and verify matches original
    let mut d = Decompressor::new(Algorithm::Zstd).unwrap();
    let result = d.decompress(&zstd_data, 1_000_000, 1000.0).unwrap();
    assert_eq!(result, original);
}
