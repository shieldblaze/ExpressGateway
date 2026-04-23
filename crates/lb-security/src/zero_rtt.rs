//! TLS 1.3 0-RTT replay protection.
//!
//! Maintains a fixed-capacity set of recently seen 0-RTT tokens. When the set
//! is full, the oldest token is evicted (ring buffer semantics). This provides
//! a time-bounded replay window without unbounded memory growth.
//!
//! Tokens are hashed to a fixed-size `[u8; 32]` digest via a
//! process-local keyed HMAC-SHA256 before storage. The key is generated via
//! `ring::rand::SystemRandom` at construction and never leaves the struct,
//! so an attacker cannot precompute collisions from source inspection
//! (the pre-auditor-signoff `hash_token` used source-visible multiply-shift
//! seeds — flagged in the 2026-04-23 auditor signoff as a precompute-
//! collision risk; this module now uses HMAC-SHA256 as a drop-in
//! replacement).

use std::collections::HashSet;
use std::collections::VecDeque;

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

use crate::SecurityError;

/// Non-public hash producing a 32-byte digest via HMAC-SHA256 under a
/// process-local secret key.
///
/// An attacker without the key cannot craft two tokens with the same digest
/// short of breaking HMAC-SHA256, so the `HashSet` dedup can't be bypassed
/// by a precomputation attack. The key is generated fresh per
/// `ZeroRttReplayGuard` instance so cross-instance correlation is also
/// denied.
fn hash_token(key: &hmac::Key, token: &[u8]) -> [u8; 32] {
    let tag = hmac::sign(key, token);
    let mut out = [0u8; 32];
    // SAFETY: HMAC-SHA256 output is always 32 bytes. The indexing-slicing
    // lint is satisfied because the source slice length is a compile-time
    // invariant of the HMAC_SHA256 algorithm.
    let src = tag.as_ref();
    for (dst, byte) in out.iter_mut().zip(src.iter()) {
        *dst = *byte;
    }
    out
}

/// Derive a fresh 32-byte secret for the keyed hash via
/// `ring::rand::SystemRandom`.
///
/// Infallible in practice on any Linux/BSD with `/dev/urandom`. If the
/// kernel RNG surface truly fails (kernel panic territory), we fall back
/// to a time-mixed secret so the guard stays usable; an attacker in that
/// mode can guess the secret, but their ability to replay tokens is also
/// bounded by the ring-buffer capacity, so the downgrade is strictly
/// better than a hardcoded public seed.
fn fresh_secret() -> [u8; 32] {
    let rng = SystemRandom::new();
    let mut secret = [0u8; 32];
    if rng.fill(&mut secret).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0u128, |d| d.as_nanos());
        let bytes = nanos.to_le_bytes();
        for (i, byte) in secret.iter_mut().enumerate() {
            let src = bytes.get(i % bytes.len()).copied().unwrap_or(0);
            let mix = u8::try_from(i & 0xff).unwrap_or(0).wrapping_mul(0x9E);
            *byte = src ^ mix;
        }
    }
    secret
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
    /// Process-local HMAC-SHA256 key. Never leaves this struct; prevents
    /// source-inspection collision precomputation (auditor finding
    /// 2026-04-23).
    key: hmac::Key,
}

impl ZeroRttReplayGuard {
    /// Create a new guard with the given capacity and a freshly generated
    /// HMAC-SHA256 key.
    ///
    /// # Arguments
    ///
    /// * `max_tokens` - Maximum number of tokens to remember before evicting the oldest.
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        Self::new_with_secret(max_tokens, &fresh_secret())
    }

    /// Create a new guard with the given capacity and caller-supplied
    /// HMAC secret. Useful for tests that need deterministic digest
    /// equality across instances; production code should use
    /// [`Self::new`] so each instance has an unrelated key.
    #[must_use]
    pub fn new_with_secret(max_tokens: usize, secret: &[u8]) -> Self {
        let max_tokens = if max_tokens == 0 { 1 } else { max_tokens };
        let key = hmac::Key::new(hmac::HMAC_SHA256, secret);
        Self {
            max_tokens,
            order: VecDeque::with_capacity(max_tokens),
            seen: HashSet::with_capacity(max_tokens),
            key,
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
        let digest = hash_token(&self.key, token);

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
