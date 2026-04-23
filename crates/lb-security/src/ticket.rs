//! TLS session-ticket-key rotator (Pingora EC-11).
//!
//! A TLS session ticket is an opaque, server-encrypted blob that a resuming
//! client presents to skip a full handshake. The blob is encrypted with a
//! server-held key. If that key is long-lived, forward secrecy on resumed
//! sessions collapses to the lifetime of the key — every resumption since
//! the key was issued becomes decryptable by an attacker who later steals
//! it. The industry answer is scheduled key rotation with a short overlap
//! window: produce new tickets with the fresh key; accept tickets
//! encrypted with the previous key for one overlap period; then drop the
//! old key so recorded traffic cannot be decrypted.
//!
//! This module provides a generic, transport-agnostic rotator. Wiring
//! into an active TLS listener belongs to Pillar 3b alongside
//! `crates/lb/src/main.rs`. Here we expose:
//!
//! * [`TicketKey`] — an opaque handle to a single ticket-encryption key.
//!   Under the hood it is an `Arc<dyn rustls::server::ProducesTickets>`
//!   produced by [`rustls::crypto::ring::Ticketer::new`], which bundles a
//!   ChaCha20-Poly1305 AEAD key plus a 128-bit random `key_name` prefix
//!   used as AEAD-AAD and for constant-time ticket-to-key matching. The
//!   task spec sketched an `[u8; 80]` byte layout; rustls 0.23 does not
//!   expose that layout directly, so the opaque handle is the
//!   version-stable shape that actually ships.
//! * [`TicketRotator`] — holds `current` and optional `previous` keys
//!   with a `rotation_interval` and an `overlap` window.
//! * [`RotatingTicketer`] — an `Arc`-wrapped
//!   [`rustls::server::ProducesTickets`] impl that encrypts with
//!   [`TicketRotator::current`] and decrypts with current-or-previous
//!   (subject to the overlap).

use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use rustls::server::ProducesTickets;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

/// Errors raised by the session-ticket rotator.
#[derive(Debug, thiserror::Error)]
pub enum TicketError {
    /// Failed to generate fresh ticket-key material from the OS RNG.
    #[error("ticket key generation failed: {0}")]
    KeyGen(String),

    /// Building a [`rustls::ServerConfig`] failed — usually because the
    /// certificate chain and private key do not match, or the chosen
    /// protocol versions are incompatible with the `ring` provider.
    #[error("tls server config build failed: {0}")]
    ServerConfig(String),
}

/// A single ticket-encryption key. Opaque handle over a rustls
/// [`ProducesTickets`] AEAD ticketer.
#[derive(Clone)]
pub struct TicketKey {
    inner: Arc<dyn ProducesTickets>,
}

impl TicketKey {
    /// Generate a fresh key using the OS RNG and
    /// [`rustls::crypto::ring::Ticketer::new`] (ChaCha20-Poly1305 + 16-byte
    /// random `key_name`).
    ///
    /// # Errors
    ///
    /// Returns [`TicketError::KeyGen`] if the RNG fails.
    pub fn generate() -> Result<Self, TicketError> {
        let inner = rustls::crypto::ring::Ticketer::new()
            .map_err(|e| TicketError::KeyGen(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Encrypt `plain` with this key, returning the ticket ciphertext.
    /// `None` indicates the AEAD refused to seal (ring is infallible for
    /// sealing under non-malicious inputs; `None` here is a hard failure).
    #[must_use]
    pub fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
        self.inner.encrypt(plain)
    }

    /// Decrypt `cipher` with this key. Returns `None` if the ciphertext
    /// was not produced by this key (wrong `key_name`) or if the AEAD
    /// authenticator rejects it.
    #[must_use]
    pub fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        self.inner.decrypt(cipher)
    }
}

impl fmt::Debug for TicketKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never render key material, not even indirectly. Elide it fully.
        f.debug_struct("TicketKey").finish_non_exhaustive()
    }
}

/// A rotating pool of TLS session-ticket keys.
///
/// Holds a `current` key used to encrypt new tickets and an optional
/// `previous` key that is still accepted for decryption during the
/// `overlap` window following the last rotation. Once `overlap` has
/// elapsed since that rotation, the previous key is dropped and recorded
/// traffic encrypted with it can no longer be decrypted on this server.
///
/// Rotation is driven externally by calling
/// [`rotate_if_due`](Self::rotate_if_due) on a periodic task. The rotator
/// itself has no timer thread.
pub struct TicketRotator {
    current: Arc<TicketKey>,
    previous: Option<Arc<TicketKey>>,
    rotated_at: Instant,
    rotation_interval: Duration,
    overlap: Duration,
}

impl TicketRotator {
    /// Build a new rotator, generating a fresh current key immediately.
    ///
    /// # Errors
    ///
    /// Returns [`TicketError::KeyGen`] if the RNG fails.
    pub fn new(rotation_interval: Duration, overlap: Duration) -> Result<Self, TicketError> {
        let current = Arc::new(TicketKey::generate()?);
        Ok(Self {
            current,
            previous: None,
            rotated_at: Instant::now(),
            rotation_interval,
            overlap,
        })
    }

    /// Configured rotation interval — the period after which `current` is
    /// demoted to `previous` and a new `current` is generated.
    #[must_use]
    pub const fn rotation_interval(&self) -> Duration {
        self.rotation_interval
    }

    /// Configured overlap — the grace period after rotation during which
    /// the demoted key is still accepted for decryption.
    #[must_use]
    pub const fn overlap(&self) -> Duration {
        self.overlap
    }

    /// Access the current (encrypting) key.
    #[must_use]
    pub fn current(&self) -> Arc<TicketKey> {
        Arc::clone(&self.current)
    }

    /// Access the previous (decrypt-only, overlap-bounded) key, if any.
    #[must_use]
    pub fn previous(&self) -> Option<Arc<TicketKey>> {
        self.previous.as_ref().map(Arc::clone)
    }

    /// When the most recent rotation (or construction) occurred.
    #[must_use]
    pub const fn rotated_at(&self) -> Instant {
        self.rotated_at
    }

    /// Drive the rotator forward given `now`.
    ///
    /// * If `now - rotated_at >= rotation_interval`, demote `current` to
    ///   `previous`, generate a fresh `current`, and bump `rotated_at` to
    ///   `now`.
    /// * If the previous key has outlived its overlap window — measured
    ///   from the most recent `rotated_at` — drop it.
    ///
    /// Both checks run on every call so that a rotator polled
    /// infrequently still erases stale key material on the next poll.
    ///
    /// Returns `Ok(true)` if a rotation happened on this call, `Ok(false)`
    /// otherwise. Dropping an expired previous key alone does not count
    /// as a rotation for the boolean signal.
    ///
    /// # Errors
    ///
    /// Returns [`TicketError::KeyGen`] if the RNG fails while minting the
    /// new current key. On failure the rotator is left unchanged.
    pub fn rotate_if_due(&mut self, now: Instant) -> Result<bool, TicketError> {
        let elapsed = now.saturating_duration_since(self.rotated_at);
        let rotated = if elapsed >= self.rotation_interval {
            let new_current = Arc::new(TicketKey::generate()?);
            let demoted = std::mem::replace(&mut self.current, new_current);
            self.previous = Some(demoted);
            self.rotated_at = now;
            true
        } else {
            false
        };

        // Expire `previous` if it has outlived its overlap window measured
        // from the most recent rotation. A freshly-rotated pair (rotated
        // this call) has age 0, so this is a no-op in that case; it only
        // fires when the rotator was last rotated >= overlap ago without
        // a new rotation happening this call.
        if self.previous.is_some() {
            let age_since_rotation = now.saturating_duration_since(self.rotated_at);
            if age_since_rotation >= self.overlap {
                self.previous = None;
            }
        }

        Ok(rotated)
    }

    /// Wrap this rotator in an `Arc<Mutex<_>>` and expose it as a
    /// rustls [`ProducesTickets`] via [`RotatingTicketer`]. The returned
    /// handle is cheaply cloneable and thread-safe.
    #[must_use]
    pub fn into_rustls_ticketer(self) -> Arc<dyn ProducesTickets> {
        Arc::new(RotatingTicketer {
            rot: Arc::new(Mutex::new(self)),
        })
    }

    /// Observe this rotator through a rustls [`ProducesTickets`] view
    /// without losing access to the rotator state. Returns the shared
    /// `Arc<Mutex<_>>` handle and the trait object; both point at the
    /// same rotator.
    #[must_use]
    pub fn as_rustls_ticketer(self) -> (Arc<Mutex<Self>>, Arc<dyn ProducesTickets>) {
        let rot = Arc::new(Mutex::new(self));
        let producer: Arc<dyn ProducesTickets> = Arc::new(RotatingTicketer {
            rot: Arc::clone(&rot),
        });
        (rot, producer)
    }
}

impl fmt::Debug for TicketRotator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TicketRotator")
            .field("rotation_interval", &self.rotation_interval)
            .field("overlap", &self.overlap)
            .field("has_previous", &self.previous.is_some())
            .finish_non_exhaustive()
    }
}

/// rustls [`ProducesTickets`] impl backed by a [`TicketRotator`].
///
/// * `encrypt` always uses `current`.
/// * `decrypt` tries `current` first, then `previous` (if present).
/// * `lifetime` reports `rotation_interval + overlap` in seconds,
///   saturated at `u32::MAX`.
pub struct RotatingTicketer {
    rot: Arc<Mutex<TicketRotator>>,
}

impl RotatingTicketer {
    fn lifetime_secs(rot: &TicketRotator) -> u32 {
        let total = rot.rotation_interval.saturating_add(rot.overlap);
        u32::try_from(total.as_secs()).unwrap_or(u32::MAX)
    }
}

impl fmt::Debug for RotatingTicketer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RotatingTicketer").finish_non_exhaustive()
    }
}

impl ProducesTickets for RotatingTicketer {
    fn enabled(&self) -> bool {
        true
    }

    fn lifetime(&self) -> u32 {
        let rot = self.rot.lock();
        Self::lifetime_secs(&rot)
    }

    fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
        let rot = self.rot.lock();
        rot.current.encrypt(plain)
    }

    fn decrypt(&self, cipher: &[u8]) -> Option<Vec<u8>> {
        let rot = self.rot.lock();
        if let Some(plain) = rot.current.decrypt(cipher) {
            return Some(plain);
        }
        rot.previous.as_ref().and_then(|prev| prev.decrypt(cipher))
    }
}

/// Build a rustls [`rustls::ServerConfig`] that terminates TLS with the
/// provided certificate chain and private key, and mints session tickets
/// via the shared [`TicketRotator`].
///
/// The returned config is cheap to clone (internally an [`Arc`]).
/// Callers wiring it into a listener should share the returned
/// `Arc<ServerConfig>` across all connections on that listener so the
/// rotator's hot-path `Mutex` is observed by every session.
///
/// Uses the `ring` [`CryptoProvider`](rustls::crypto::CryptoProvider)
/// explicitly so the call is independent of whichever provider is
/// installed as the process default.
///
/// # Errors
///
/// Returns [`TicketError::ServerConfig`] if the cert chain / key pair
/// does not agree with the `ring` provider's supported signatures, or
/// if the provider rejects the default protocol versions.
pub fn build_server_config(
    rotator: Arc<Mutex<TicketRotator>>,
    cert_chain: Vec<CertificateDer<'static>>,
    key_der: PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ServerConfig>, TicketError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| TicketError::ServerConfig(e.to_string()))?
        .with_no_client_auth();
    let mut cfg = builder
        .with_single_cert(cert_chain, key_der)
        .map_err(|e| TicketError::ServerConfig(e.to_string()))?;
    cfg.ticketer = Arc::new(RotatingTicketer { rot: rotator });
    Ok(Arc::new(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_plain() -> Vec<u8> {
        b"session-state-blob".to_vec()
    }

    #[test]
    fn rotate_if_due_noop_before_interval() {
        let mut rot =
            TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
        let start = rot.rotated_at();
        let before = start + Duration::from_secs(3_600);
        let did_rotate = rot.rotate_if_due(before).unwrap();
        assert!(!did_rotate);
        assert!(rot.previous().is_none());
        assert_eq!(rot.rotated_at(), start);
    }

    #[test]
    fn rotate_if_due_swaps_keys_at_interval() {
        let interval = Duration::from_secs(60);
        let mut rot = TicketRotator::new(interval, Duration::from_secs(30)).unwrap();
        let original_current = rot.current();
        let now = rot.rotated_at() + interval;
        let did_rotate = rot.rotate_if_due(now).unwrap();
        assert!(did_rotate);
        let new_current = rot.current();
        let new_previous = rot.previous().expect("previous set after rotation");
        assert!(Arc::ptr_eq(&original_current, &new_previous));
        assert!(!Arc::ptr_eq(&original_current, &new_current));
        assert_eq!(rot.rotated_at(), now);
    }

    #[test]
    fn overlap_preserves_previous_for_decrypt() {
        let interval = Duration::from_secs(60);
        let overlap = Duration::from_secs(30);
        let mut rot = TicketRotator::new(interval, overlap).unwrap();
        let ticket = rot.current().encrypt(&sample_plain()).unwrap();
        let t_rot = rot.rotated_at() + interval;
        rot.rotate_if_due(t_rot).unwrap();
        let (_rot_handle, ticketer) = rot.as_rustls_ticketer();
        let recovered = ticketer
            .decrypt(&ticket)
            .expect("previous decrypt in overlap");
        assert_eq!(recovered, sample_plain());
    }

    #[test]
    fn new_tickets_use_current_key() {
        let rot =
            TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
        let plain = sample_plain();
        let (rot_handle, ticketer) = rot.as_rustls_ticketer();
        let ct = ticketer.encrypt(&plain).unwrap();
        let current = rot_handle.lock().current();
        let recovered = current.decrypt(&ct).expect("current decrypts own tickets");
        assert_eq!(recovered, plain);
        {
            let mut guard = rot_handle.lock();
            let now = guard.rotated_at() + Duration::from_secs(86_400);
            guard.rotate_if_due(now).unwrap();
        }
        let ct_after = ticketer.encrypt(&plain).unwrap();
        let prev = rot_handle
            .lock()
            .previous()
            .expect("previous exists after rotation");
        assert!(
            prev.decrypt(&ct_after).is_none(),
            "previous key must not decrypt tickets minted after rotation"
        );
        let recovered_prev = prev
            .decrypt(&ct)
            .expect("previous decrypts pre-rotation tickets");
        assert_eq!(recovered_prev, plain);
    }

    #[test]
    fn build_server_config_round_trip_encrypts_and_decrypts_with_current_key() {
        // Generate an in-memory self-signed cert+key (rcgen is already a
        // dev-dep for this crate) and feed both into `build_server_config`.
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der: Vec<u8> = generated.cert.der().to_vec();
        let key_der: Vec<u8> = generated.key_pair.serialize_der();
        let cert_chain = vec![CertificateDer::from(cert_der)];
        let key = PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(key_der));

        let rot =
            TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
        let current_handle = rot.current();
        let rot_arc = Arc::new(Mutex::new(rot));
        let server_cfg = build_server_config(Arc::clone(&rot_arc), cert_chain, key).unwrap();

        // The config's ticketer encrypts with the rotator's current key:
        // we encrypt through the ServerConfig's ticketer and decrypt
        // directly with the rotator's current TicketKey.
        let plain = sample_plain();
        let ct = server_cfg
            .ticketer
            .encrypt(&plain)
            .expect("ticketer produced ciphertext");
        let recovered = current_handle
            .decrypt(&ct)
            .expect("current rotator key decrypts its own tickets");
        assert_eq!(recovered, plain);

        // ticketer.enabled() must be true so rustls actually advertises
        // session tickets to clients.
        assert!(server_cfg.ticketer.enabled());
    }

    #[test]
    fn previous_expires_after_overlap() {
        let interval = Duration::from_secs(60);
        let overlap = Duration::from_secs(30);
        let mut rot = TicketRotator::new(interval, overlap).unwrap();
        let ticket = rot.current().encrypt(&sample_plain()).unwrap();
        let t_rot = rot.rotated_at() + interval;
        rot.rotate_if_due(t_rot).unwrap();
        let t_expire = t_rot + overlap;
        rot.rotate_if_due(t_expire).unwrap();
        assert!(
            rot.previous().is_none(),
            "previous key must be dropped once overlap elapses"
        );
        let (_rot_handle, ticketer) = rot.as_rustls_ticketer();
        assert!(
            ticketer.decrypt(&ticket).is_none(),
            "ticket encrypted with expired previous key must not decrypt"
        );
    }
}
