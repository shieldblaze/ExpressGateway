//! Stateless-retry token mint/verify (RFC 9000 §8.1.3 — address validation).
//!
//! The token binds the peer address and the ODCID under HMAC-SHA256 with a short expiry, so a
//! forged or replayed one buys a spoofer nothing. Wire format, big-endian, MAC over all of it:
//! `version(1) | issued_ms_since_origin(8) | peer_kind(1: 4=v4, 6=v6) | peer_addr(4|16) |
//! port(2) | odcid_len(1) | odcid(0..255) | mac(32)`.

use std::net::{IpAddr, SocketAddr};
use std::time::{Duration, Instant};

use ring::hmac;
use ring::rand::SecureRandom;
use subtle::ConstantTimeEq;

/// Secret-key size in bytes.
pub const RETRY_SECRET_LEN: usize = 32;

/// ODCID cap. Wire CIDs are ≤20 bytes; 255 keeps the length field to one byte.
const RETRY_MAX_ODCID: usize = 255;

/// Wire-format version tag. Bump it (and branch in `verify`) on any layout change so old tokens
/// are cleanly rejected rather than misparsed.
const RETRY_TOKEN_VERSION: u8 = 0x01;

/// HMAC-SHA256 tag size.
const MAC_LEN: usize = 32;

/// Default retry-token lifetime — RFC 9000 §8.1.3 wants "short"; 10 s matches quiche's default.
pub const DEFAULT_RETRY_MAX_AGE: Duration = Duration::from_secs(10);

/// Errors raised by [`RetryTokenSigner::verify`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RetryError {
    /// Shorter than the minimum well-formed token.
    #[error("retry token truncated")]
    Truncated,

    /// Version byte does not match the current format.
    #[error("retry token version mismatch: got {got}, want {want}")]
    VersionMismatch {
        /// Observed version byte.
        got: u8,
        /// Expected version byte.
        want: u8,
    },

    /// Peer-address type byte is neither 4 (IPv4) nor 6 (IPv6).
    #[error("retry token peer-address kind invalid: {0}")]
    InvalidPeerKind(u8),

    /// ODCID-length field claims more bytes than the token carries.
    #[error("retry token odcid length field inconsistent")]
    InvalidOdcidLen,

    /// MAC did not validate — tampered or forged.
    #[error("retry token MAC mismatch")]
    MacMismatch,

    /// Minted for a different peer than the one presenting it.
    #[error("retry token peer-address mismatch")]
    PeerMismatch,

    /// `issued_at` is older than `max_age` relative to `now`.
    #[error("retry token expired")]
    Expired,
}

/// Symmetric mint/verify for stateless-retry tokens.
pub struct RetryTokenSigner {
    key: hmac::Key,
    origin: Instant,
    max_age: Duration,
}

impl std::fmt::Debug for RetryTokenSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetryTokenSigner")
            .field("max_age", &self.max_age)
            // Never render the secret material.
            .finish_non_exhaustive()
    }
}

impl RetryTokenSigner {
    /// Build a signer with a fresh 32-byte secret from [`ring::rand::SystemRandom`].
    ///
    /// # Errors
    ///
    /// The `ring` error as a string if the OS RNG is unavailable.
    pub fn new_random() -> Result<Self, String> {
        let mut secret = [0u8; RETRY_SECRET_LEN];
        ring::rand::SystemRandom::new()
            .fill(&mut secret)
            .map_err(|e| format!("retry signer RNG failed: {e}"))?;
        Ok(Self::new_with_secret(secret))
    }

    /// Build a signer with an explicit secret (test vectors, operator secret rotation).
    #[must_use]
    pub fn new_with_secret(secret: [u8; RETRY_SECRET_LEN]) -> Self {
        Self {
            key: hmac::Key::new(hmac::HMAC_SHA256, &secret),
            origin: Instant::now(),
            max_age: DEFAULT_RETRY_MAX_AGE,
        }
    }

    /// Override the default [`DEFAULT_RETRY_MAX_AGE`] lifetime.
    #[must_use]
    pub const fn with_max_age(mut self, max_age: Duration) -> Self {
        self.max_age = max_age;
        self
    }

    /// Configured retry-token max age.
    #[must_use]
    pub const fn max_age(&self) -> Duration {
        self.max_age
    }

    /// Mint a retry token binding `peer` and `odcid`. Never panics: an `odcid` longer than
    /// [`RETRY_MAX_ODCID`] is SILENTLY TRUNCATED (the wire length field is a `u8`), so a caller
    /// holding untrusted ODCID bytes must reject over-length input itself rather than rely on
    /// `verify` round-tripping what it passed in.
    #[must_use]
    pub fn mint(&self, peer: SocketAddr, odcid: &[u8]) -> Vec<u8> {
        self.mint_at(peer, odcid, Instant::now())
    }

    /// [`Self::mint`] with an explicit `now`, so tests can age a token without sleeping.
    #[must_use]
    pub fn mint_at(&self, peer: SocketAddr, odcid: &[u8], now: Instant) -> Vec<u8> {
        let truncated_odcid = odcid.get(..odcid.len().min(RETRY_MAX_ODCID)).unwrap_or(&[]);
        let mut token = Vec::with_capacity(64 + truncated_odcid.len());
        token.push(RETRY_TOKEN_VERSION);
        let issued_ms = u64::try_from(now.saturating_duration_since(self.origin).as_millis())
            .unwrap_or(u64::MAX);
        token.extend_from_slice(&issued_ms.to_be_bytes());
        encode_peer(peer, &mut token);
        // ODCID length cast is safe: we just clamped to 255.
        #[allow(clippy::cast_possible_truncation)]
        {
            token.push(truncated_odcid.len() as u8);
        }
        token.extend_from_slice(truncated_odcid);
        let tag = hmac::sign(&self.key, &token);
        token.extend_from_slice(tag.as_ref());
        token
    }

    /// Verify the MAC, peer binding and expiry; return the ODCID the token was minted for.
    ///
    /// # Errors
    ///
    /// See [`RetryError`].
    pub fn verify(
        &self,
        token: &[u8],
        peer: SocketAddr,
        now: Instant,
    ) -> Result<Vec<u8>, RetryError> {
        // Smallest well-formed token: v4 peer, empty odcid.
        if token.len() < 1 + 8 + 1 + 4 + 2 + 1 + MAC_LEN {
            return Err(RetryError::Truncated);
        }

        if token.first().copied() != Some(RETRY_TOKEN_VERSION) {
            return Err(RetryError::VersionMismatch {
                got: token.first().copied().unwrap_or(0),
                want: RETRY_TOKEN_VERSION,
            });
        }

        let mac_start = token.len() - MAC_LEN;
        let body = token.get(..mac_start).ok_or(RetryError::Truncated)?;
        let mac = token.get(mac_start..).ok_or(RetryError::Truncated)?;

        // Constant-time compare — `ct_eq` over `==` is the whole point; do not "simplify" it.
        let expected = hmac::sign(&self.key, body);
        if expected.as_ref().ct_eq(mac).unwrap_u8() != 1 {
            return Err(RetryError::MacMismatch);
        }

        // Only now is the body authentic enough to parse.
        let mut cursor = 1usize; // skip version
        let issued_ms_bytes = body.get(cursor..cursor + 8).ok_or(RetryError::Truncated)?;
        cursor += 8;
        let mut issued_buf = [0u8; 8];
        issued_buf.copy_from_slice(issued_ms_bytes);
        let issued_ms = u64::from_be_bytes(issued_buf);

        let (token_peer, after_peer) = decode_peer(body, cursor)?;
        cursor = after_peer;

        let odcid_len = *body.get(cursor).ok_or(RetryError::Truncated)? as usize;
        cursor += 1;
        let odcid = body
            .get(cursor..cursor + odcid_len)
            .ok_or(RetryError::InvalidOdcidLen)?;

        if token_peer != peer {
            return Err(RetryError::PeerMismatch);
        }

        let issued_at = self.origin + Duration::from_millis(issued_ms);
        let age = now.saturating_duration_since(issued_at);
        if age > self.max_age {
            return Err(RetryError::Expired);
        }

        Ok(odcid.to_vec())
    }
}

fn encode_peer(peer: SocketAddr, out: &mut Vec<u8>) {
    match peer.ip() {
        IpAddr::V4(v4) => {
            out.push(4);
            out.extend_from_slice(&v4.octets());
        }
        IpAddr::V6(v6) => {
            out.push(6);
            out.extend_from_slice(&v6.octets());
        }
    }
    out.extend_from_slice(&peer.port().to_be_bytes());
}

fn decode_peer(body: &[u8], start: usize) -> Result<(SocketAddr, usize), RetryError> {
    let kind = *body.get(start).ok_or(RetryError::Truncated)?;
    let (ip_len, cursor) = match kind {
        4 => (4usize, start + 1),
        6 => (16usize, start + 1),
        other => return Err(RetryError::InvalidPeerKind(other)),
    };
    let ip_bytes = body
        .get(cursor..cursor + ip_len)
        .ok_or(RetryError::Truncated)?;
    let port_bytes = body
        .get(cursor + ip_len..cursor + ip_len + 2)
        .ok_or(RetryError::Truncated)?;
    let mut port_buf = [0u8; 2];
    port_buf.copy_from_slice(port_bytes);
    let port = u16::from_be_bytes(port_buf);
    let ip = if kind == 4 {
        let mut b = [0u8; 4];
        b.copy_from_slice(ip_bytes);
        IpAddr::V4(std::net::Ipv4Addr::from(b))
    } else {
        let mut b = [0u8; 16];
        b.copy_from_slice(ip_bytes);
        IpAddr::V6(std::net::Ipv6Addr::from(b))
    };
    Ok((SocketAddr::new(ip, port), cursor + ip_len + 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn test_signer() -> RetryTokenSigner {
        RetryTokenSigner::new_with_secret([0x5au8; RETRY_SECRET_LEN])
    }

    fn loopback_peer() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 4242)
    }

    #[test]
    fn mint_then_verify_roundtrip() {
        let signer = test_signer();
        let peer = loopback_peer();
        let odcid = b"abcd-efgh-12345678";
        let token = signer.mint(peer, odcid);
        let now = Instant::now();
        let recovered = signer.verify(&token, peer, now).unwrap();
        assert_eq!(recovered, odcid);
    }

    #[test]
    fn verify_rejects_tampered_token() {
        let signer = test_signer();
        let peer = loopback_peer();
        let mut token = signer.mint(peer, b"odcid");
        // Flip a byte inside the authenticated region, not in the MAC.
        let mid = token.len() / 2;
        token[mid] ^= 0xff;
        let err = signer.verify(&token, peer, Instant::now()).unwrap_err();
        assert_eq!(err, RetryError::MacMismatch);
    }

    #[test]
    fn verify_rejects_expired_token() {
        let signer = RetryTokenSigner::new_with_secret([0xa5u8; RETRY_SECRET_LEN])
            .with_max_age(Duration::from_millis(10));
        let peer = loopback_peer();
        let t0 = Instant::now();
        let token = signer.mint_at(peer, b"odcid", t0);
        let t1 = t0 + Duration::from_secs(1);
        let err = signer.verify(&token, peer, t1).unwrap_err();
        assert_eq!(err, RetryError::Expired);
    }

    #[test]
    fn verify_rejects_wrong_peer() {
        let signer = test_signer();
        let minted_for = loopback_peer();
        let token = signer.mint(minted_for, b"odcid");
        let other = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 4242);
        let err = signer.verify(&token, other, Instant::now()).unwrap_err();
        assert_eq!(err, RetryError::PeerMismatch);
    }
}
