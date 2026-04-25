//! Compression bomb mitigation tests.

use lb_compression::*;

#[test]
fn test_compression_bomb_cap_fires() {
    // Create data that compresses very well (e.g., 1MB of zeros)
    let bomb_data = vec![0u8; 1_000_000];
    let mut c = Compressor::new(Algorithm::Gzip, None).unwrap();
    let mut compressed = c.compress(&bomb_data).unwrap();
    compressed.extend(c.finish().unwrap());
    // Try to decompress with tight limits
    let mut d = Decompressor::new(Algorithm::Gzip).unwrap();
    let result = d.decompress(&compressed, 1000, 10.0); // max 1KB output, max 10x ratio
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(matches!(
        err,
        CompressionError::BombDetected { .. } | CompressionError::OutputTooLarge { .. }
    ));
}
