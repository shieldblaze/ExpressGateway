//! TLS 1.3 0-RTT replay protection.
//!
//! Maintains a fixed-capacity LRU window of recently seen 0-RTT tokens.
//! When the window is full, the **least-recently-used** entry is
//! evicted. Touching an entry that is already present (on a replay)
//! refreshes it to most-recently-used so that a long-lived replay
//! stream keeps the originating token observable instead of pushing
//! it out the back of a FIFO.
//!
//! Tokens are hashed to a fixed-size `[u8; 32]` digest via a
//! process-local keyed HMAC-SHA256 before storage. The key is generated
//! via `ring::rand::SystemRandom` at construction and never leaves the
//! struct, so an attacker cannot precompute collisions from source
//! inspection (the pre-auditor-signoff `hash_token` used source-visible
//! multiply-shift seeds — flagged in the 2026-04-23 auditor signoff as
//! a precompute-collision risk; this module now uses HMAC-SHA256 as a
//! drop-in replacement).
//!
//! Window-size policy (SEC-2-05)
//! -----------------------------
//!
//! The window size is configurable via
//! `[security].zero_rtt_replay_window_size` (Wave-2c
//! `crates/lb-config`). Default is [`DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE`]
//! = `65_536` entries — sized to bound a unique-token spray at
//! ~64 k entries before collapse, while keeping memory under
//! 4 MB (entry size: 32 bytes digest + ~16 B linked-list overhead per
//! entry × 65 536 ≈ 3 MB).

use std::collections::HashMap;

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

use crate::SecurityError;

/// Default capacity of the 0-RTT replay window (SEC-2-05 default for
/// the `[security].zero_rtt_replay_window_size` config knob). 65 536
/// digests × 32 B each (plus a small per-entry overhead) — ~3 MB
/// resident.
pub const DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE: usize = 65_536;

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

/// Doubly-linked node inside the LRU's internal arena.
///
/// Indices into [`ZeroRttReplayGuard::arena`]; `usize::MAX` is the
/// sentinel for "no neighbour". A node's `prev` points toward LRU
/// (front), `next` points toward MRU (back). Eviction pops from
/// `front`; insertion pushes to `back`; replay-touch unlinks the
/// node and pushes it to `back`.
struct Node {
    digest: [u8; 32],
    prev: usize,
    next: usize,
}

const NIL: usize = usize::MAX;

/// Fixed-capacity LRU replay guard for TLS 1.3 0-RTT early data tokens.
///
/// Replaced the prior FIFO ring buffer per SEC-2-05: under sustained
/// unique-token spray a FIFO can push an in-flight replayee out of
/// the window before the legitimate replay attempt arrives. LRU
/// semantics keep frequently-checked tokens at the back so the
/// replay-detection window is *use*-bounded, not *time*-bounded.
///
/// Memory: 32-byte digest + 2 × usize (16 B on 64-bit) per slot +
/// a `HashMap<digest, usize>` index. ~3 MB at the default 65 536
/// capacity.
pub struct ZeroRttReplayGuard {
    max_tokens: usize,
    /// Slab of node records. Indexed by the HashMap. Vacant slots
    /// are tracked via the `free_head` free-list embedded in
    /// `Node::next` (when on the free list, `prev` = NIL,
    /// `next` = next free slot or NIL).
    arena: Vec<Node>,
    /// Head of the free-list inside `arena`.
    free_head: usize,
    /// Index of the least-recently-used node (eviction candidate).
    front: usize,
    /// Index of the most-recently-used node.
    back: usize,
    /// Digest -> arena index. Membership in O(1); promotion on hit
    /// is O(1) via the doubly-linked list.
    index: HashMap<[u8; 32], usize>,
    /// Process-local HMAC-SHA256 key. Never leaves this struct;
    /// prevents source-inspection collision precomputation
    /// (auditor finding 2026-04-23).
    key: hmac::Key,
}

impl ZeroRttReplayGuard {
    /// Create a new guard with the given capacity and a freshly generated
    /// HMAC-SHA256 key.
    ///
    /// # Arguments
    ///
    /// * `max_tokens` - Maximum number of tokens to remember before
    ///   evicting the LRU. A value of `0` is coerced to `1`.
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        Self::new_with_secret(max_tokens, &fresh_secret())
    }

    /// Create a guard pre-sized to
    /// [`DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE`].
    #[must_use]
    pub fn with_default_window() -> Self {
        Self::new(DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE)
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
            arena: Vec::with_capacity(max_tokens),
            free_head: NIL,
            front: NIL,
            back: NIL,
            index: HashMap::with_capacity(max_tokens),
            key,
        }
    }

    /// Number of digests currently in the window (snapshot).
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// `true` when no digests are in the window.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Configured maximum window size.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.max_tokens
    }

    /// Check whether a 0-RTT token has been seen before.
    ///
    /// On a hit, the matching entry is promoted to most-recently-used
    /// so frequently-checked tokens are not evicted under spray.
    ///
    /// On a miss, the token is recorded and (if the window is at
    /// capacity) the least-recently-used entry is evicted first.
    ///
    /// # Errors
    ///
    /// Returns [`SecurityError::ZeroRttReplay`] if the token is
    /// already in the window (replay detected).
    pub fn check_and_record(&mut self, token: &[u8]) -> Result<(), SecurityError> {
        let digest = hash_token(&self.key, token);

        if let Some(&idx) = self.index.get(&digest) {
            // Replay — promote to MRU so the replayee stays observable
            // even under sustained unique-token spray, then surface.
            self.move_to_back(idx);
            return Err(SecurityError::ZeroRttReplay);
        }

        // Evict LRU if at capacity.
        if self.index.len() >= self.max_tokens {
            self.evict_lru();
        }

        // Insert at MRU.
        let idx = self.alloc_node(digest);
        self.push_back(idx);
        self.index.insert(digest, idx);

        Ok(())
    }

    /// Allocate an arena slot for `digest`. Reuses a free-list slot
    /// when available; otherwise extends the `Vec`.
    fn alloc_node(&mut self, digest: [u8; 32]) -> usize {
        if self.free_head == NIL {
            let idx = self.arena.len();
            self.arena.push(Node {
                digest,
                prev: NIL,
                next: NIL,
            });
            idx
        } else {
            let idx = self.free_head;
            // free_head was populated by `free_node` which guarantees
            // the slot exists and `next` points at the next free slot
            // (or NIL). If the invariant is violated (cannot happen
            // under normal use), fall back to extending the arena so
            // we still produce a valid index — the released slot is
            // simply orphaned rather than reused.
            match self.arena.get_mut(idx) {
                Some(node) => {
                    self.free_head = node.next;
                    node.digest = digest;
                    node.prev = NIL;
                    node.next = NIL;
                    idx
                }
                None => {
                    self.free_head = NIL;
                    let fresh = self.arena.len();
                    self.arena.push(Node {
                        digest,
                        prev: NIL,
                        next: NIL,
                    });
                    fresh
                }
            }
        }
    }

    /// Return a slot to the free list. Does not touch `index`.
    fn free_node(&mut self, idx: usize) {
        if let Some(node) = self.arena.get_mut(idx) {
            node.prev = NIL;
            node.next = self.free_head;
            self.free_head = idx;
        }
    }

    /// Push `idx` onto the MRU end of the LRU list.
    fn push_back(&mut self, idx: usize) {
        let prev_back = self.back;
        if let Some(node) = self.arena.get_mut(idx) {
            node.prev = prev_back;
            node.next = NIL;
        }
        if prev_back == NIL {
            self.front = idx;
        } else if let Some(prev) = self.arena.get_mut(prev_back) {
            prev.next = idx;
        }
        self.back = idx;
    }

    /// Unlink `idx` from the LRU list. The slot stays valid; the
    /// caller is responsible for either re-linking (promotion) or
    /// freeing it.
    fn unlink(&mut self, idx: usize) {
        let (prev, next) = self.arena.get(idx).map_or((NIL, NIL), |n| (n.prev, n.next));
        if prev == NIL {
            self.front = next;
        } else if let Some(p) = self.arena.get_mut(prev) {
            p.next = next;
        }
        if next == NIL {
            self.back = prev;
        } else if let Some(n) = self.arena.get_mut(next) {
            n.prev = prev;
        }
    }

    fn move_to_back(&mut self, idx: usize) {
        if self.back == idx {
            return;
        }
        self.unlink(idx);
        self.push_back(idx);
    }

    /// Evict the front node (LRU). Removes from the `index` map and
    /// returns the slot to the free list.
    fn evict_lru(&mut self) {
        let idx = self.front;
        if idx == NIL {
            return;
        }
        let digest = self.arena.get(idx).map(|n| n.digest);
        self.unlink(idx);
        if let Some(d) = digest {
            self.index.remove(&d);
        }
        self.free_node(idx);
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
