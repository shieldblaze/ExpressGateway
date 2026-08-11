//! TLS 1.3 0-RTT replay protection.
//!
//! Two attack-driven choices, both of which look like arbitrary taste from the code alone:
//! * **LRU, not FIFO** (SEC-2-05): a FIFO lets a sustained unique-token spray push the in-flight
//!   replayee out of the window before its replay arrives, so the window must be use-bounded.
//! * **HMAC-SHA256, not multiply-shift** (auditor finding 2026-04-23): the old source-visible
//!   seeds let an attacker precompute digest collisions and walk straight through the dedup.

use std::collections::HashMap;

use ring::hmac;
use ring::rand::{SecureRandom, SystemRandom};

use crate::SecurityError;

/// Default capacity of the 0-RTT replay window (`[security].zero_rtt_replay_window_size`); ~3 MB.
pub const DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE: usize = 65_536;

/// Keyed digest. The per-instance key is what denies collision precomputation and cross-instance
/// correlation — never swap this for an unkeyed hash.
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

/// Fresh 32-byte secret from the OS RNG. The time-mixed fallback is a deliberate degradation for
/// the kernel-RNG-failed case: guessable, but still better than a hardcoded public seed.
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

/// Arena node. `prev` points toward LRU (`front`), `next` toward MRU (`back`); [`NIL`] means no
/// neighbour.
struct Node {
    digest: [u8; 32],
    prev: usize,
    next: usize,
}

const NIL: usize = usize::MAX;

/// Fixed-capacity LRU replay guard for TLS 1.3 0-RTT early-data tokens.
pub struct ZeroRttReplayGuard {
    max_tokens: usize,
    /// Node slab. Vacant slots live on the `free_head` list embedded in `Node::next`, where
    /// `prev` = NIL.
    arena: Vec<Node>,
    /// Head of the free-list inside `arena`.
    free_head: usize,
    /// Least-recently-used node — the eviction candidate.
    front: usize,
    /// Most-recently-used node.
    back: usize,
    /// Digest -> arena index.
    index: HashMap<[u8; 32], usize>,
    /// Process-local HMAC key; never leaves this struct.
    key: hmac::Key,
}

impl ZeroRttReplayGuard {
    /// New guard with a freshly generated HMAC key; `max_tokens` of `0` is coerced to `1`.
    #[must_use]
    pub fn new(max_tokens: usize) -> Self {
        Self::new_with_secret(max_tokens, &fresh_secret())
    }

    /// Guard pre-sized to [`DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE`].
    #[must_use]
    pub fn with_default_window() -> Self {
        Self::new(DEFAULT_ZERO_RTT_REPLAY_WINDOW_SIZE)
    }

    /// Guard with a caller-supplied secret — TESTS ONLY, for cross-instance digest equality.
    /// Production must use [`Self::new`] so each instance keys independently.
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

    /// Record a 0-RTT token, evicting the LRU entry if the window is full. A token already in
    /// the window is a replay and errors with [`SecurityError::ZeroRttReplay`].
    pub fn check_and_record(&mut self, token: &[u8]) -> Result<(), SecurityError> {
        let digest = hash_token(&self.key, token);

        if let Some(&idx) = self.index.get(&digest) {
            // Promote before surfacing: keeps the replayee observable under unique-token spray.
            self.move_to_back(idx);
            return Err(SecurityError::ZeroRttReplay);
        }

        if self.index.len() >= self.max_tokens {
            self.evict_lru();
        }

        let idx = self.alloc_node(digest);
        self.push_back(idx);
        self.index.insert(digest, idx);

        Ok(())
    }

    /// Allocate an arena slot for `digest`, reusing a free-list slot when one exists.
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
            // The None arm is unreachable while the free-list invariant holds; it exists so a
            // violated invariant orphans a slot instead of panicking on a security path.
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

    /// Unlink `idx`. The slot stays valid — the caller must re-link or free it.
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

    /// Evict the LRU node and return its slot to the free list.
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

    /// Alias of [`check_and_record`](Self::check_and_record), named for the QUIC accept loop.
    pub fn check_0rtt_token(&mut self, token: &[u8]) -> Result<(), SecurityError> {
        self.check_and_record(token)
    }
}
