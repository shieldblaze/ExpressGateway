//! Admin HTTP authentication + bind-loopback enforcement (SEC-2-06).
//!
//! The gate holds only a SHA-256 of the bearer token (`[admin].api_token_hash`, 64 hex chars) —
//! the plaintext never enters the struct — and compares with `subtle::ConstantTimeEq` so a
//! wrong-prefix token cannot be recovered from response timing.

use std::net::SocketAddr;

use ring::digest;
use subtle::ConstantTimeEq;

/// Errors from the [`AdminAuthGate::validate_bind`] start-up check.
#[derive(Debug, thiserror::Error)]
pub enum AdminBindError {
    /// Non-loopback bind without `allow_non_loopback`. A hard startup exit, so the admin surface
    /// cannot be exposed silently.
    #[error(
        "refusing to bind admin HTTP listener to non-loopback address {addr}: set \
         [admin].allow_non_loopback = true to override"
    )]
    NonLoopbackWithoutOverride {
        /// The non-loopback address that was rejected.
        addr: SocketAddr,
    },

    /// `allow_non_loopback` without a token hash — public bind plus no auth is an open admin
    /// surface.
    #[error(
        "refusing to bind admin HTTP listener to non-loopback address {addr} without \
         [admin].api_token_hash set"
    )]
    PublicBindWithoutToken {
        /// The non-loopback address that was rejected.
        addr: SocketAddr,
    },
}

/// Errors from the per-request [`AdminAuthGate::authorize`] check.
#[derive(Debug, thiserror::Error)]
pub enum AdminAuthError {
    /// Request is missing the `Authorization` header.
    #[error("missing Authorization header")]
    MissingHeader,

    /// Present but not a `Bearer ` credential.
    #[error("Authorization header is not a Bearer token")]
    NotBearer,

    /// Token did not hash to `[admin].api_token_hash`.
    #[error("invalid bearer token")]
    InvalidToken,
}

/// Decoded 32-byte SHA-256 of the configured bearer token.
#[derive(Clone)]
pub struct AdminTokenHash([u8; 32]);

impl AdminTokenHash {
    /// SHA-256 a plaintext token; production TOML uses [`Self::from_hex`] instead.
    #[must_use]
    pub fn from_plaintext(token: &str) -> Self {
        let d = digest::digest(&digest::SHA256, token.as_bytes());
        let mut out = [0u8; 32];
        for (dst, src) in out.iter_mut().zip(d.as_ref().iter()) {
            *dst = *src;
        }
        Self(out)
    }

    /// Decode a 64-char hex digest.
    ///
    /// # Errors
    ///
    /// `Err(())` unless the input is exactly 64 hex chars; the caller renders the config message.
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
        // Never render the digest: it is still a verification credential, and routine logging
        // invites grep-then-reuse. Enforced by `debug_does_not_print_digest_bytes`.
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

/// Authentication gate for the admin HTTP listener; holds the digest only.
pub struct AdminAuthGate {
    expected: Option<AdminTokenHash>,
}

impl AdminAuthGate {
    /// Build a gate. `None` disables token enforcement, leaving the loopback bind guard as the
    /// only defense — which is why [`Self::validate_bind`] refuses non-loopback without a token.
    #[must_use]
    pub const fn new(expected: Option<AdminTokenHash>) -> Self {
        Self { expected }
    }

    /// Whether bearer-token enforcement is active.
    #[must_use]
    pub const fn enforced(&self) -> bool {
        self.expected.is_some()
    }

    /// Authorize on the verbatim `Authorization` header value, or `None` when absent.
    ///
    /// # Errors
    ///
    /// [`AdminAuthError`]. With no token configured EVERY request is allowed, so the bind must be
    /// loopback-only — see [`Self::validate_bind`].
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

    /// Refuse to start an admin listener that would be exposed without authentication. Call once
    /// before binding. Inputs are flat to keep this crate independent of lb-config.
    ///
    /// # Errors
    ///
    /// [`AdminBindError`].
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
            write!(&mut s, "{b:02x}").unwrap();
        }
        s
    }

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

    #[test]
    fn from_hex_round_trips_with_plaintext_hash() {
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
        let hex: String = to_hex(&h.0);
        assert!(!s.contains(&hex));
    }
}
