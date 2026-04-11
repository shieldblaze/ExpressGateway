//! Stateless retry token generation and validation (RFC 9000 Section 8.1.4).
//!
//! Protects against amplification attacks by requiring clients to prove they
//! can receive data at their claimed address. Tokens are encrypted with
//! AES-128-GCM and carry the client address and an expiration timestamp.
//!
//! Token format (after encryption):
//!   `nonce (12 bytes) || ciphertext || tag (16 bytes)`
//!
//! Plaintext format:
//!   `addr_family (1) || ip_bytes (4 or 16) || port (2, BE) || timestamp (8, BE)`

use std::net::SocketAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ring::aead::{self, Aad, BoundKey, Nonce, NonceSequence, NONCE_LEN, AES_128_GCM};
use ring::error::Unspecified;
use thiserror::Error;
use tracing::debug;

/// Errors that can occur during retry token handling.
#[derive(Debug, Error)]
pub enum RetryTokenError {
    /// The token has expired.
    #[error("retry token expired")]
    Expired,
    /// The token is malformed or decryption failed.
    #[error("invalid retry token")]
    Invalid,
    /// The client address in the token does not match the source address.
    #[error("address mismatch in retry token")]
    AddressMismatch,
    /// Token encryption/decryption error.
    #[error("crypto error: {0}")]
    Crypto(String),
}

/// A nonce sequence that uses a single provided nonce exactly once.
///
/// This is appropriate because each token gets a fresh random nonce;
/// we never reuse a `SingleNonce` instance for multiple seal/open calls.
struct SingleNonce(Option<[u8; NONCE_LEN]>);

impl SingleNonce {
    fn new(nonce_bytes: [u8; NONCE_LEN]) -> Self {
        Self(Some(nonce_bytes))
    }
}

impl NonceSequence for SingleNonce {
    fn advance(&mut self) -> Result<Nonce, Unspecified> {
        self.0
            .take()
            .map(Nonce::assume_unique_for_key)
            .ok_or(Unspecified)
    }
}

/// Stateless retry token handler.
///
/// Tokens encode `(client_addr, timestamp)` and are sealed with AES-128-GCM
/// so the server can validate them without per-connection state.
pub struct RetryTokenHandler {
    /// Raw key bytes for AES-128-GCM (16 bytes).
    key_bytes: [u8; 16],
    /// Token validity duration.
    token_lifetime: Duration,
}

impl RetryTokenHandler {
    /// Create a new handler with the given key and token lifetime.
    ///
    /// The key MUST be 16 bytes of cryptographically random material.
    pub fn new(key: [u8; 16], token_lifetime: Duration) -> Self {
        Self {
            key_bytes: key,
            token_lifetime,
        }
    }

    /// Build an AEAD sealing key from the stored key bytes and a nonce.
    fn sealing_key(
        &self,
        nonce_bytes: [u8; NONCE_LEN],
    ) -> Result<aead::SealingKey<SingleNonce>, RetryTokenError> {
        let unbound = aead::UnboundKey::new(&AES_128_GCM, &self.key_bytes)
            .map_err(|e| RetryTokenError::Crypto(format!("failed to create key: {e}")))?;
        Ok(aead::SealingKey::new(unbound, SingleNonce::new(nonce_bytes)))
    }

    /// Build an AEAD opening key from the stored key bytes and a nonce.
    fn opening_key(
        &self,
        nonce_bytes: [u8; NONCE_LEN],
    ) -> Result<aead::OpeningKey<SingleNonce>, RetryTokenError> {
        let unbound = aead::UnboundKey::new(&AES_128_GCM, &self.key_bytes)
            .map_err(|e| RetryTokenError::Crypto(format!("failed to create key: {e}")))?;
        Ok(aead::OpeningKey::new(unbound, SingleNonce::new(nonce_bytes)))
    }

    /// Encode address + timestamp into a plaintext byte vector.
    fn encode_plaintext(client_addr: SocketAddr) -> Vec<u8> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut plaintext = Vec::with_capacity(1 + 16 + 2 + 8);

        match client_addr {
            SocketAddr::V4(v4) => {
                plaintext.push(4u8);
                plaintext.extend_from_slice(&v4.ip().octets());
            }
            SocketAddr::V6(v6) => {
                plaintext.push(6u8);
                plaintext.extend_from_slice(&v6.ip().octets());
            }
        }

        plaintext.extend_from_slice(&client_addr.port().to_be_bytes());
        plaintext.extend_from_slice(&now.to_be_bytes());
        plaintext
    }

    /// Generate a retry token for the given client address.
    ///
    /// Returns `nonce (12) || ciphertext || tag (16)`.
    pub fn generate_token(&self, client_addr: SocketAddr) -> Result<Vec<u8>, RetryTokenError> {
        // Generate a random nonce per RFC 9000 Section 8.1.4.
        let mut nonce_bytes = [0u8; NONCE_LEN];
        use rand::RngCore;
        rand::thread_rng().fill_bytes(&mut nonce_bytes);

        let mut plaintext = Self::encode_plaintext(client_addr);

        let mut sealing_key = self.sealing_key(nonce_bytes)?;

        // Seal in place, appending the authentication tag.
        sealing_key
            .seal_in_place_append_tag(Aad::empty(), &mut plaintext)
            .map_err(|e| RetryTokenError::Crypto(format!("seal failed: {e}")))?;

        // Prepend the nonce so the validator can extract it.
        let mut token = Vec::with_capacity(NONCE_LEN + plaintext.len());
        token.extend_from_slice(&nonce_bytes);
        token.extend_from_slice(&plaintext);

        debug!(
            addr = %client_addr,
            token_len = token.len(),
            "generated retry token"
        );

        Ok(token)
    }

    /// Validate a retry token against the claimed client address.
    pub fn validate_token(
        &self,
        token: &[u8],
        client_addr: SocketAddr,
    ) -> Result<(), RetryTokenError> {
        // Minimum: nonce (12) + addr_family (1) + ipv4 (4) + port (2) + ts (8) + tag (16) = 43
        if token.len() < NONCE_LEN + 1 + 4 + 2 + 8 + AES_128_GCM.tag_len() {
            return Err(RetryTokenError::Invalid);
        }

        let (nonce_bytes_slice, ciphertext) = token.split_at(NONCE_LEN);
        let nonce_bytes: [u8; NONCE_LEN] = nonce_bytes_slice
            .try_into()
            .map_err(|_| RetryTokenError::Invalid)?;

        let mut buf = ciphertext.to_vec();

        let mut opening_key = self.opening_key(nonce_bytes)?;

        let plaintext = opening_key
            .open_in_place(Aad::empty(), &mut buf)
            .map_err(|_| RetryTokenError::Invalid)?;

        // Parse address family.
        if plaintext.is_empty() {
            return Err(RetryTokenError::Invalid);
        }
        let addr_family = plaintext[0];
        let (expected_addr, rest) = match addr_family {
            4 => {
                if plaintext.len() < 1 + 4 + 2 + 8 {
                    return Err(RetryTokenError::Invalid);
                }
                let ip_bytes: [u8; 4] = plaintext[1..5]
                    .try_into()
                    .map_err(|_| RetryTokenError::Invalid)?;
                let ip = std::net::Ipv4Addr::from(ip_bytes);
                let port_bytes: [u8; 2] = plaintext[5..7]
                    .try_into()
                    .map_err(|_| RetryTokenError::Invalid)?;
                let port = u16::from_be_bytes(port_bytes);
                (SocketAddr::from((ip, port)), &plaintext[7..])
            }
            6 => {
                if plaintext.len() < 1 + 16 + 2 + 8 {
                    return Err(RetryTokenError::Invalid);
                }
                let ip_bytes: [u8; 16] = plaintext[1..17]
                    .try_into()
                    .map_err(|_| RetryTokenError::Invalid)?;
                let ip = std::net::Ipv6Addr::from(ip_bytes);
                let port_bytes: [u8; 2] = plaintext[17..19]
                    .try_into()
                    .map_err(|_| RetryTokenError::Invalid)?;
                let port = u16::from_be_bytes(port_bytes);
                (SocketAddr::from((ip, port)), &plaintext[19..])
            }
            _ => return Err(RetryTokenError::Invalid),
        };

        // Validate address matches.
        if expected_addr != client_addr {
            return Err(RetryTokenError::AddressMismatch);
        }

        // Parse and check timestamp.
        if rest.len() < 8 {
            return Err(RetryTokenError::Invalid);
        }
        let ts_bytes: [u8; 8] = rest[..8].try_into().map_err(|_| RetryTokenError::Invalid)?;
        let token_ts = u64::from_be_bytes(ts_bytes);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if now.saturating_sub(token_ts) > self.token_lifetime.as_secs() {
            return Err(RetryTokenError::Expired);
        }

        debug!(
            addr = %client_addr,
            "retry token validated successfully"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn test_key() -> [u8; 16] {
        [
            0xDE, 0xAD, 0xBE, 0xEF, 0xCA, 0xFE, 0xBA, 0xBE, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06,
            0x07, 0x08,
        ]
    }

    #[test]
    fn generate_and_validate_ipv4_token() {
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr: SocketAddr = (Ipv4Addr::new(192, 168, 1, 1), 12345).into();

        let token = handler.generate_token(addr).expect("generate should succeed");
        assert!(!token.is_empty());

        handler
            .validate_token(&token, addr)
            .expect("should be valid");
    }

    #[test]
    fn generate_and_validate_ipv6_token() {
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr: SocketAddr = (Ipv6Addr::LOCALHOST, 443).into();

        let token = handler.generate_token(addr).expect("generate should succeed");
        handler
            .validate_token(&token, addr)
            .expect("should be valid");
    }

    #[test]
    fn reject_wrong_address() {
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr1: SocketAddr = (Ipv4Addr::new(10, 0, 0, 1), 1000).into();
        let addr2: SocketAddr = (Ipv4Addr::new(10, 0, 0, 2), 1000).into();

        let token = handler.generate_token(addr1).expect("generate should succeed");
        // Decryption succeeds but address mismatch is caught.
        let result = handler.validate_token(&token, addr2);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            RetryTokenError::AddressMismatch
        ));
    }

    #[test]
    fn reject_tampered_token() {
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr: SocketAddr = (Ipv4Addr::new(10, 0, 0, 1), 1000).into();

        let mut token = handler.generate_token(addr).expect("generate should succeed");
        // Flip a byte in the ciphertext portion (after the 12-byte nonce).
        if token.len() > NONCE_LEN + 1 {
            token[NONCE_LEN + 1] ^= 0xFF;
        }
        // AEAD decryption must fail.
        let result = handler.validate_token(&token, addr);
        assert!(matches!(result.unwrap_err(), RetryTokenError::Invalid));
    }

    #[test]
    fn reject_empty_token() {
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr: SocketAddr = (Ipv4Addr::LOCALHOST, 8080).into();

        let result = handler.validate_token(&[], addr);
        assert!(matches!(result.unwrap_err(), RetryTokenError::Invalid));
    }

    #[test]
    fn reject_malformed_token() {
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr: SocketAddr = (Ipv4Addr::LOCALHOST, 8080).into();

        // Token too short.
        let result = handler.validate_token(&[0x01, 0x02], addr);
        assert!(result.is_err());
    }

    #[test]
    fn reject_wrong_port() {
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr1: SocketAddr = (Ipv4Addr::LOCALHOST, 1000).into();
        let addr2: SocketAddr = (Ipv4Addr::LOCALHOST, 2000).into();

        let token = handler.generate_token(addr1).expect("generate should succeed");
        let result = handler.validate_token(&token, addr2);
        assert!(result.is_err());
    }

    #[test]
    fn different_keys_cannot_validate() {
        let handler1 = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let handler2 = RetryTokenHandler::new([0xFF; 16], Duration::from_secs(60));
        let addr: SocketAddr = (Ipv4Addr::LOCALHOST, 443).into();

        let token = handler1.generate_token(addr).expect("generate should succeed");
        let result = handler2.validate_token(&token, addr);
        assert!(matches!(result.unwrap_err(), RetryTokenError::Invalid));
    }

    #[test]
    fn tokens_are_unique_per_call() {
        // Each call generates a fresh random nonce, so tokens must differ.
        let handler = RetryTokenHandler::new(test_key(), Duration::from_secs(60));
        let addr: SocketAddr = (Ipv4Addr::LOCALHOST, 443).into();

        let t1 = handler.generate_token(addr).expect("generate should succeed");
        let t2 = handler.generate_token(addr).expect("generate should succeed");
        assert_ne!(t1, t2, "tokens should differ due to random nonce");

        // But both should validate.
        handler.validate_token(&t1, addr).expect("t1 valid");
        handler.validate_token(&t2, addr).expect("t2 valid");
    }
}
