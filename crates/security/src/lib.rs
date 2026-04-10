//! Security: ACL, rate limiting, TLS fingerprinting.
//!
//! This crate provides network-level security primitives for ExpressGateway:
//!
//! - **NACL** (Network Access Control List): binary radix trie for O(prefix_length)
//!   IP matching with allowlist/denylist modes and copy-on-write updates.
//! - **ConnectionRateLimiter**: sliding-window connection rate limiting per IP
//!   with bounded memory via LRU eviction.
//! - **PacketRateLimiter**: token-bucket packet rate limiting per IP and globally
//!   with lazy refill (no background timer).
//! - **JA3**: TLS ClientHello fingerprinting with GREASE filtering and MD5 hashing.

pub mod acl;
pub mod ja3;
pub mod packet_rate_limit;
pub mod rate_limit;

pub use acl::{AclAction, AclMode, Nacl};
pub use ja3::{Ja3BlockList, Ja3Extractor, Ja3Fingerprint};
pub use packet_rate_limit::{
    PacketRateLimitConfig, PacketRateLimiter, RateLimitAction, TokenBucket,
};
pub use rate_limit::ConnectionRateLimiter;
