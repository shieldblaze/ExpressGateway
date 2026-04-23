//! TLS 1.3 0-RTT replay protection.
//!
//! Maintains a fixed-capacity set of recently seen 0-RTT tokens. When the set
//! is full, the oldest token is evicted (ring buffer semantics). This provides
//! a time-bounded replay window without unbounded memory growth.
//!
//! Tokens are hashed to a fixed-size `[u8; 32]` digest before storage to avoid
//! retaining (and copying) arbitrarily large raw tokens.

use std::collections::HashSet;
use std::collections::VecDeque;

use crate::SecurityError;

/// Non-cryptographic hash producing a 32-byte digest.
///
/// Uses a multiply-shift scheme seeded with distinct primes per output byte.
/// This is for dedup only -- not for security-sensitive hashing.
fn hash_token(token: &[u8]) -> [u8; 32] {
    // 32 distinct odd seeds so each output byte is an independent hash lane.
    const SEEDS: [u64; 32] = [
        0x9e37_79b9_7f4a_7c15,
        0x6c62_272e_07bb_0142,
        0xbf58_476d_1ce4_e5b9,
        0x94d0_49bb_1331_11eb,
        0xd6e8_feb8_6659_fd93,
        0xcf1b_cdc3_b0e1_7b3f,
        0x6295_1639_2bb6_8c09,
        0x85eb_ca77_c2b2_ae63,
        0xc2b2_ae3d_27d4_eb4f,
        0x2738_1d1c_8c98_b35e,
        0x4cf5_ad43_2745_937f,
        0x8a5c_d789_635d_2dff,
        0x1214_5b01_2836_40eb,
        0x1b36_8ed0_23b3_12c7,
        0x5663_17e6_1357_1649,
        0x3d5e_6b80_ab29_9b31,
        0xa312_2d3e_5b1d_4f7b,
        0xe7a2_b9c3_0f5d_6e19,
        0x1932_8cb1_4a7d_2ff5,
        0x7c4e_1f06_bd8a_4c73,
        0xd841_7ab9_3e26_5ab1,
        0x2b9f_6c34_80e1_7df7,
        0x4e83_d729_1a6c_5b35,
        0x9156_c48d_e7b3_0a89,
        0xf3c1_2e75_a496_8dc3,
        0x0a72_39bf_51d8_6e17,
        0x6d94_50e3_c2a7_bf4b,
        0xb285_6f17_930e_d89d,
        0x54d3_8c21_e6f9_7ab3,
        0x87e6_1b4f_0c3d_5a29,
        0xc914_7d82_b5e0_6f5d,
        0x3a6b_c0d5_e831_94f1,
    ];

    let mut out = [0u8; 32];
    for (slot, seed) in out.iter_mut().zip(SEEDS.iter()) {
        let mut h: u64 = *seed;
        for &b in token {
            h = h.wrapping_mul(0x0100_0000_01b3).wrapping_add(u64::from(b));
        }
        // Mix finalizer (murmur-style).
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        // Intentional truncation: we want only the low byte of each lane.
        #[allow(clippy::cast_possible_truncation)]
        {
            *slot = h as u8;
        }
    }
    out
}

/// Fixed-capacity replay guard for TLS 1.3 0-RTT early data tokens.
///
/// Uses a ring buffer of token digests to bound memory usage. When capacity is
/// reached, the oldest entry is evicted. This means a replayed token is only
/// detected if it arrives while the original is still in the buffer.
pub struct ZeroRttReplayGuard {
    max_tokens: usize,
    /// Ring buffer tracking insertion order for eviction.
    order: VecDeque<[u8; 32]>,
    /// Set for O(1) membership tests.
    seen: HashSet<[u8; 32]>,
}

impl ZeroRttReplayGuard {
    /// Create a new guard with the given capacity.
    ///
    /// # Arguments
    ///
    /// * `max_tokens` - Maximum number of tokens to remember before evicting the oldest.
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        let max_tokens = if max_tokens == 0 { 1 } else { max_tokens };
        Self {
            max_tokens,
            order: VecDeque::with_capacity(max_tokens),
            seen: HashSet::with_capacity(max_tokens),
        }
    }

    /// Check whether a 0-RTT token has been seen before.
    ///
    /// If the token is new, its digest is recorded. If the buffer is at
    /// capacity, the oldest digest is evicted first.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::ZeroRttReplay`] if the token is already in
    /// the buffer (replay detected).
    pub fn check_and_record(&mut self, token: &[u8]) -> Result<(), SecurityError> {
        let digest = hash_token(token);

        if self.seen.contains(&digest) {
            return Err(SecurityError::ZeroRttReplay);
        }

        // Evict oldest if at capacity.
        if self.order.len() >= self.max_tokens {
            if let Some(evicted) = self.order.pop_front() {
                self.seen.remove(&evicted);
            }
        }

        self.seen.insert(digest);
        self.order.push_back(digest);

        Ok(())
    }

    /// Gateway-facing entry point named for its call site in the QUIC
    /// server accept loop (Pillar 3b.3a). Semantically identical to
    /// [`check_and_record`](Self::check_and_record); the separate name
    /// documents the wiring point so a reader of the accept loop can
    /// follow the crate boundary without chasing a generic-sounding
    /// helper.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::ZeroRttReplay`] if the token has been
    /// seen since the buffer was last evicted.
    pub fn check_0rtt_token(&mut self, token: &[u8]) -> Result<(), SecurityError> {
        self.check_and_record(token)
    }
}
