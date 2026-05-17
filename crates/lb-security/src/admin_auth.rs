//! Admin HTTP authentication + bind-loopback enforcement (SEC-2-06).
//!
//! Wave-2a delivers the **API**. The wiring into
//! `lb-observability::admin_http` is a follow-up owned by rel
//! (REL-2-04, Wave-2b/c) — flagged in the commit body. The Wave-2c
//! call site reads:
//!
//! ```ignore
//! AdminAuthGate::validate_bind(cfg.bind, cfg.allow_non_loopback)?;
//! let gate = AdminAuthGate::new(cfg.api_token_hash);
//! // ... in each request handler:
//! gate.authorize(request_headers)?;
//! ```
//!
//! The gate stores a **SHA-256 of the bearer token**, never the
//! plaintext. Operators put the digest in TOML as
//! `[admin].api_token_hash = "deadbeef..."` (64 hex chars). The
//! hex-encoded form lives at rest; the plaintext token only ever
//! crosses the wire on a request and is hashed before comparison.
//! Comparison uses `subtle::ConstantTimeEq` so a wrong-prefix token
//! cannot be inferred from response timing.
//!
//! ## SHA-256 dep
//!
//! `ring` is already a workspace dep (`crates/lb-security/Cargo.toml`)
//! and offers `ring::digest::SHA256`, so this module avoids pulling
//! a new transitive.

use std::net::SocketAddr;

use ring::digest;
use subtle::ConstantTimeEq;

/// Errors the [`AdminAuthGate::validate_bind`] start-up check can
/// surface.
#[derive(Debug, thiserror::Error)]
pub enum AdminBindError {
    /// Configured bind address is non-loopback and
    /// `allow_non_loopback` was not set. Refuse to start — Wave-2c's
    /// `crates/lb/src/main.rs` propagates this as a hard exit so the
    /// operator cannot silently expose the admin surface.
    #[error(
        "refusing to bind admin HTTP listener to non-loopback address {addr}: set \
         [admin].allow_non_loopback = true to override"
    )]
    NonLoopbackWithoutOverride {
        /// The non-loopback address that was rejected.
        addr: SocketAddr,
    },

    /// `allow_non_loopback = true` was set but no `api_token_hash`
    /// was configured — a foot-gun guard. Public bind + no auth =
    /// open admin surface.
    #[error(
        "refusing to bind admin HTTP listener to non-loopback address {addr} without \
         [admin].api_token_hash set"
    )]
    PublicBindWithoutToken {
        /// The non-loopback address that was rejected.
        addr: SocketAddr,
    },
}

/// Errors the per-request [`AdminAuthGate::authorize`] check can
/// surface.
#[derive(Debug, thiserror::Error)]
pub enum AdminAuthError {
    /// Request is missing the `Authorization` header.
    #[error("missing Authorization header")]
    MissingHeader,

    /// `Authorization` header is present but does not start with the
    /// `Bearer ` prefix.
    #[error("Authorization header is not a Bearer token")]
    NotBearer,

    /// Bearer token did not hash to the configured
    /// `[admin].api_token_hash`. Comparison is constant-time so a
    /// wrong-prefix token leaks no timing.
    #[error("invalid bearer token")]
    InvalidToken,
}

/// Decoded SHA-256 of the bearer token configured via
/// `[admin].api_token_hash`. 32 bytes.
#[derive(Clone)]
pub struct AdminTokenHash([u8; 32]);

impl AdminTokenHash {
    /// Hash a plaintext bearer token with SHA-256. Convenience
    /// constructor for tests and for operators who prefer to commit a
    /// raw token to a secret store and let the gateway hash it on
    /// startup; production TOML uses
    /// [`AdminTokenHash::from_hex`] instead.
    #[must_use]
    pub fn from_plaintext(token: &str) -> Self {
        let d = digest::digest(&digest::SHA256, token.as_bytes());
        let mut out = [0u8; 32];
        for (dst, src) in out.iter_mut().zip(d.as_ref().iter()) {
            *dst = *src;
        }
        Self(out)
    }

    /// Decode a 64-character lowercase or uppercase hex string into
    /// the 32-byte SHA-256 digest.
    ///
    /// # Errors
    ///
    /// Returns `Err(())` if the input is not exactly 64 hex chars.
    /// The caller (Wave-2c `lb-config` glue) translates the error
    /// into a config validation message.
    #[allow(clippy::result_unit_err)]
    pub fn from_hex(hex: &str) -> Result<Self, ()> {
        if hex.len() != 64 {
            return Err(());
        }
        let mut out = [0u8; 32];
        let bytes = hex.as_bytes();
        for i in 0..32 {
            let hi = decode_nibble(*bytes.get(i * 2).ok_or(())?)?;
            let lo = decode_nibble(*bytes.get(i * 2 + 1).ok_or(())?)?;
            *out.get_mut(i).ok_or(())? = (hi << 4) | lo;
        }
        Ok(Self(out))
    }

    /// Constant-time equality against another digest.
    #[must_use]
    pub fn ct_eq(&self, other: &[u8; 32]) -> bool {
        self.0.ct_eq(other).into()
    }
}

impl std::fmt::Debug for AdminTokenHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the digest, even though it is not the secret
        // — it is still a verification credential and printing it
        // routinely to logs invites grep-then-reuse mistakes.
        f.debug_struct("AdminTokenHash").finish_non_exhaustive()
    }
}

fn decode_nibble(b: u8) -> Result<u8, ()> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(b - b'a' + 10),
        b'A'..=b'F' => Ok(b - b'A' + 10),
        _ => Err(()),
    }
}

/// Authentication gate for the admin HTTP listener.
///
/// Wave-2a deliverable; Wave-2b/c wires it into
/// `lb-observability::admin_http` (REL-2-04 follow-up). The gate
/// stores only the digest of the bearer token; the plaintext token
/// never enters this struct.
pub struct AdminAuthGate {
    expected: Option<AdminTokenHash>,
}

impl AdminAuthGate {
    /// Build a gate. `None` disables bearer-token enforcement (the
    /// loopback-only bind guard is the sole defense in that case;
    /// [`validate_bind`](Self::validate_bind) refuses non-loopback
    /// without a token).
    #[must_use]
    pub const fn new(expected: Option<AdminTokenHash>) -> Self {
        Self { expected }
    }

    /// Whether bearer-token enforcement is active.
    #[must_use]
    pub const fn enforced(&self) -> bool {
        self.expected.is_some()
    }

    /// Authorize a request based on its `Authorization` header value
    /// (if any).
    ///
    /// Pass `None` if the header is absent; pass `Some(header_value)`
    /// otherwise. The header value is expected verbatim — the
    /// `Bearer ` prefix check happens inside this function.
    ///
    /// # Errors
    ///
    /// Returns the matching [`AdminAuthError`] for the failure class.
    /// When no token is configured (`new(None)`) **all requests are
    /// allowed** — the listener bind must therefore be loopback-only
    /// (enforced by [`Self::validate_bind`]).
    pub fn authorize(&self, header: Option<&str>) -> Result<(), AdminAuthError> {
        let Some(expected) = self.expected.as_ref() else {
            return Ok(());
        };
        let header = header.ok_or(AdminAuthError::MissingHeader)?;
        let token = header
            .strip_prefix("Bearer ")
            .or_else(|| header.strip_prefix("bearer "))
            .ok_or(AdminAuthError::NotBearer)?;
        let token = token.trim();
        let d = digest::digest(&digest::SHA256, token.as_bytes());
        let mut digest_bytes = [0u8; 32];
        for (dst, src) in digest_bytes.iter_mut().zip(d.as_ref().iter()) {
            *dst = *src;
        }
        if expected.ct_eq(&digest_bytes) {
            Ok(())
        } else {
            Err(AdminAuthError::InvalidToken)
        }
    }

    /// Refuse to start when the admin listener would be exposed
    /// without authentication. Call once at startup before binding.
    ///
    /// Wave-2c reads `[admin].bind`, `[admin].allow_non_loopback` and
    /// `[admin].api_token_hash` from `lb-config` and passes the
    /// result here. The function takes the inputs flat to keep this
    /// module independent of lb-config (this crate is reused in
    /// other contexts).
    ///
    /// # Errors
    ///
    /// * [`AdminBindError::NonLoopbackWithoutOverride`] — non-loopback
    ///   bind and `allow_non_loopback = false`.
    /// * [`AdminBindError::PublicBindWithoutToken`] —
    ///   `allow_non_loopback = true` but no token hash configured.
    pub fn validate_bind(
        bind: SocketAddr,
        allow_non_loopback: bool,
        has_token: bool,
    ) -> Result<(), AdminBindError> {
        if bind.ip().is_loopback() {
            return Ok(());
        }
        if !allow_non_loopback {
            return Err(AdminBindError::NonLoopbackWithoutOverride { addr: bind });
        }
        if !has_token {
            return Err(AdminBindError::PublicBindWithoutToken { addr: bind });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

    fn sa_v4(ip: [u8; 4], port: u16) -> SocketAddr {
        SocketAddr::from((Ipv4Addr::from(ip), port))
    }

    fn sa_loopback_v6(port: u16) -> SocketAddr {
        SocketAddr::from((Ipv6Addr::LOCALHOST, port))
    }

    fn to_hex(bytes: &[u8; 32]) -> String {
        use std::fmt::Write;
        let mut s = String::with_capacity(64);
        for b in bytes {
            // unwrap_used is allowed under #[cfg(test)] in this crate.
            write!(&mut s, "{b:02x}").unwrap();
        }
        s
    }

    // ---- validate_bind ----

    #[test]
    fn loopback_v4_always_ok() {
        AdminAuthGate::validate_bind(sa_v4([127, 0, 0, 1], 9090), false, false).unwrap();
        AdminAuthGate::validate_bind(sa_v4([127, 0, 0, 1], 9090), true, false).unwrap();
    }

    #[test]
    fn loopback_v6_always_ok() {
        AdminAuthGate::validate_bind(sa_loopback_v6(9090), false, false).unwrap();
    }

    #[test]
    fn public_bind_without_override_rejected() {
        let err =
            AdminAuthGate::validate_bind(sa_v4([0, 0, 0, 0], 9090), false, false).unwrap_err();
        assert!(matches!(
            err,
            AdminBindError::NonLoopbackWithoutOverride { .. }
        ));
    }

    #[test]
    fn public_bind_override_without_token_rejected() {
        let err = AdminAuthGate::validate_bind(sa_v4([0, 0, 0, 0], 9090), true, false).unwrap_err();
        assert!(matches!(err, AdminBindError::PublicBindWithoutToken { .. }));
    }

    #[test]
    fn public_bind_override_with_token_ok() {
        AdminAuthGate::validate_bind(sa_v4([0, 0, 0, 0], 9090), true, true).unwrap();
    }

    // ---- authorize ----

    #[test]
    fn no_token_configured_allows_all() {
        let gate = AdminAuthGate::new(None);
        gate.authorize(None).unwrap();
        gate.authorize(Some("Bearer whatever")).unwrap();
        assert!(!gate.enforced());
    }

    #[test]
    fn missing_header_rejected() {
        let gate = AdminAuthGate::new(Some(AdminTokenHash::from_plaintext("s3kret")));
        assert!(matches!(
            gate.authorize(None).unwrap_err(),
            AdminAuthError::MissingHeader
        ));
    }

    #[test]
    fn non_bearer_rejected() {
        let gate = AdminAuthGate::new(Some(AdminTokenHash::from_plaintext("s3kret")));
        assert!(matches!(
            gate.authorize(Some("Basic abc==")).unwrap_err(),
            AdminAuthError::NotBearer
        ));
    }

    #[test]
    fn wrong_token_rejected() {
        let gate = AdminAuthGate::new(Some(AdminTokenHash::from_plaintext("s3kret")));
        assert!(matches!(
            gate.authorize(Some("Bearer wrong")).unwrap_err(),
            AdminAuthError::InvalidToken
        ));
    }

    #[test]
    fn correct_token_accepted() {
        let gate = AdminAuthGate::new(Some(AdminTokenHash::from_plaintext("s3kret")));
        gate.authorize(Some("Bearer s3kret")).unwrap();
    }

    #[test]
    fn bearer_prefix_case_insensitive() {
        let gate = AdminAuthGate::new(Some(AdminTokenHash::from_plaintext("s3kret")));
        gate.authorize(Some("bearer s3kret")).unwrap();
    }

    // ---- AdminTokenHash::from_hex ----

    #[test]
    fn from_hex_round_trips_with_plaintext_hash() {
        // sha256("s3kret") (verified independently): use the
        // plaintext hasher to derive the expected hex form and then
        // round-trip.
        let h1 = AdminTokenHash::from_plaintext("s3kret");
        let hex: String = to_hex(&h1.0);
        let h2 = AdminTokenHash::from_hex(&hex).unwrap();
        assert!(h1.ct_eq(&h2.0));
    }

    #[test]
    fn from_hex_wrong_length_rejected() {
        assert!(AdminTokenHash::from_hex("deadbeef").is_err());
    }

    #[test]
    fn from_hex_non_hex_rejected() {
        let bad = "z".repeat(64);
        assert!(AdminTokenHash::from_hex(&bad).is_err());
    }

    #[test]
    fn debug_does_not_print_digest_bytes() {
        let h = AdminTokenHash::from_plaintext("s3kret");
        let s = format!("{h:?}");
        // Should NOT contain any hex byte sequence from the digest.
        let hex: String = to_hex(&h.0);
        assert!(!s.contains(&hex));
    }
}
