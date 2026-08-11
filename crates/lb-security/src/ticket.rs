//! TLS session-ticket-key rotator (Pingora EC-11).
//!
//! A long-lived ticket key collapses forward secrecy on every session resumed under it: steal the
//! key later and all of that recorded traffic decrypts. Hence scheduled rotation with a short
//! overlap — encrypt with the fresh key, still decrypt with the previous one for one overlap
//! window, then drop it.
//!
//! [`TicketKey`] is an opaque `Arc<dyn ProducesTickets>` rather than the spec's `[u8; 80]` layout
//! because rustls 0.23 does not expose that layout — the handle is the version-stable shape.

use std::fmt;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use arc_swap::ArcSwap;
use parking_lot::Mutex;

use rustls::server::ProducesTickets;
use rustls_pki_types::{CertificateDer, PrivateKeyDer};

/// Errors raised by the session-ticket rotator.
#[derive(Debug, thiserror::Error)]
pub enum TicketError {
    /// Failed to generate fresh ticket-key material from the OS RNG.
    #[error("ticket key generation failed: {0}")]
    KeyGen(String),

    /// `ServerConfig` build failed — usually a cert/key mismatch or a version/provider clash.
    #[error("tls server config build failed: {0}")]
    ServerConfig(String),
}

/// A single ticket-encryption key, opaque over a rustls [`ProducesTickets`] AEAD ticketer.
#[derive(Clone)]
pub struct TicketKey {
    inner: Arc<dyn ProducesTickets>,
}

impl TicketKey {
    /// Fresh ChaCha20-Poly1305 key with a random `key_name`, from the OS RNG.
    ///
    /// # Errors
    ///
    /// [`TicketError::KeyGen`] if the RNG fails.
    pub fn generate() -> Result<Self, TicketError> {
        let inner = rustls::crypto::ring::Ticketer::new()
            .map_err(|e| TicketError::KeyGen(e.to_string()))?;
        Ok(Self { inner })
    }

    /// Encrypt `plain`; `None` means the AEAD refused to seal, which is a hard failure.
    #[must_use]
    pub fn encrypt(&self, plain: &[u8]) -> Option<Vec<u8>> {
        self.inner.encrypt(plain)
    }

    /// Decrypt `cipher`; `None` on a wrong `key_name` or a failed AEAD authenticator.
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

/// A rotating pool of TLS session-ticket keys. There is no timer thread — something must call
/// [`rotate_if_due`](Self::rotate_if_due) periodically or keys never rotate.
pub struct TicketRotator {
    current: Arc<TicketKey>,
    previous: Option<Arc<TicketKey>>,
    rotated_at: Instant,
    rotation_interval: Duration,
    overlap: Duration,
}

impl TicketRotator {
    /// Build a rotator, minting the first current key immediately.
    ///
    /// # Errors
    ///
    /// [`TicketError::KeyGen`] if the RNG fails.
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

    /// Period after which `current` is demoted to `previous`.
    #[must_use]
    pub const fn rotation_interval(&self) -> Duration {
        self.rotation_interval
    }

    /// Grace period during which the demoted key still decrypts.
    #[must_use]
    pub const fn overlap(&self) -> Duration {
        self.overlap
    }

    /// Access the current (encrypting) key.
    #[must_use]
    pub fn current(&self) -> Arc<TicketKey> {
        Arc::clone(&self.current)
    }

    /// The previous (decrypt-only, overlap-bounded) key, if any.
    #[must_use]
    pub fn previous(&self) -> Option<Arc<TicketKey>> {
        self.previous.as_ref().map(Arc::clone)
    }

    /// When the most recent rotation (or construction) occurred.
    #[must_use]
    pub const fn rotated_at(&self) -> Instant {
        self.rotated_at
    }

    /// Drive the rotator to `now`. Both the rotate check and the previous-key expiry run on every
    /// call, so an infrequently polled rotator still erases stale key material on its next poll.
    /// `Ok(true)` only for an actual rotation — expiring the previous key alone does not count.
    ///
    /// # Errors
    ///
    /// [`TicketError::KeyGen`]; the rotator is left unchanged on failure.
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

        // A pair rotated on this call has age 0, so this only fires when no rotation happened.
        if self.previous.is_some() {
            let age_since_rotation = now.saturating_duration_since(self.rotated_at);
            if age_since_rotation >= self.overlap {
                self.previous = None;
            }
        }

        Ok(rotated)
    }

    /// Consume the rotator into a cloneable rustls [`ProducesTickets`] handle.
    #[must_use]
    pub fn into_rustls_ticketer(self) -> Arc<dyn ProducesTickets> {
        Arc::new(RotatingTicketer {
            rot: Arc::new(Mutex::new(self)),
        })
    }

    /// Like [`Self::into_rustls_ticketer`], but also returns the shared handle to the same rotator.
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

/// rustls [`ProducesTickets`] backed by a [`TicketRotator`]: encrypt with `current`, decrypt with
/// `current` then `previous`.
pub struct RotatingTicketer {
    rot: Arc<Mutex<TicketRotator>>,
}

impl RotatingTicketer {
    fn lifetime_secs(rot: &TicketRotator) -> u32 {
        let total = rot.rotation_interval.saturating_add(rot.overlap);
        u32::try_from(total.as_secs()).unwrap_or(u32::MAX)
    }

    /// Ticketer view over an existing handle — this is how a cert reload keeps the same rotator.
    #[must_use]
    pub fn ticketer_from(rot: Arc<Mutex<TicketRotator>>) -> Arc<dyn ProducesTickets> {
        Arc::new(Self { rot })
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

/// Terminating [`rustls::ServerConfig`] whose tickets come from the shared [`TicketRotator`].
/// Empty `alpn_protocols` disables ALPN advertisement. Pins the `ring` provider explicitly so the
/// result does not depend on whichever provider is installed as the process default.
///
/// # Errors
///
/// [`TicketError::ServerConfig`] on a cert/key or version/provider disagreement.
pub fn build_server_config(
    rotator: Arc<Mutex<TicketRotator>>,
    cert_chain: Vec<CertificateDer<'static>>,
    key_der: PrivateKeyDer<'static>,
    alpn_protocols: &[&[u8]],
) -> Result<Arc<rustls::ServerConfig>, TicketError> {
    build_server_config_with_policy(rotator, cert_chain, key_der, alpn_protocols, false)
}

/// [`build_server_config`] with an explicit `tls13_only` flag (PROTO-2-14): `true` refuses TLS 1.2
/// ClientHellos, `false` keeps the rustls default set.
///
/// # Errors
///
/// Same shape as [`build_server_config`].
pub fn build_server_config_with_policy(
    rotator: Arc<Mutex<TicketRotator>>,
    cert_chain: Vec<CertificateDer<'static>>,
    key_der: PrivateKeyDer<'static>,
    alpn_protocols: &[&[u8]],
    tls13_only: bool,
) -> Result<Arc<rustls::ServerConfig>, TicketError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = if tls13_only {
        rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .map_err(|e| TicketError::ServerConfig(e.to_string()))?
    } else {
        rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| TicketError::ServerConfig(e.to_string()))?
    }
    .with_no_client_auth();
    let mut cfg = builder
        .with_single_cert(cert_chain, key_der)
        .map_err(|e| TicketError::ServerConfig(e.to_string()))?;
    cfg.ticketer = Arc::new(RotatingTicketer { rot: rotator });
    if !alpn_protocols.is_empty() {
        cfg.alpn_protocols = alpn_protocols.iter().map(|p| p.to_vec()).collect();
    }
    Ok(Arc::new(cfg))
}

// REL-2-03 hot-reloadable TLS cert bundle. Three contracts a reader would otherwise break:
// a FAILED reload must keep the old bundle live, so a botched cert push cannot blackhole the
// listener; the chain-depth cap (≤6) rejects, but near-expiry `not_after` is WARN-ONLY because
// refusing near-expiry certs is exactly wrong during an emergency rotation; and in-flight
// handshakes keep the bundle they snapshotted at accept while new ones pick up the swap.

/// Errors raised when loading or rebuilding a [`TlsConfigBundle`].
#[derive(Debug, thiserror::Error)]
pub enum TlsBundleError {
    /// Disk I/O failure (cert or key file missing, unreadable, etc).
    #[error("tls bundle io error reading {path:?}: {source}")]
    Io {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: std::io::Error,
    },
    /// PEM parsing failure for the cert chain.
    #[error("tls bundle cert PEM parse failed for {0:?}: {1}")]
    CertParse(String, #[source] std::io::Error),
    /// PEM parsing failure for the private key.
    #[error("tls bundle key PEM parse failed for {path:?}: {source}")]
    KeyParse {
        /// Path that failed to parse.
        path: PathBuf,
        /// Underlying parse error.
        #[source]
        source: std::io::Error,
    },
    /// The cert PEM file contained zero `CERTIFICATE` blocks.
    #[error("tls bundle: no certificates found in {0:?}")]
    EmptyChain(PathBuf),
    /// The key PEM file contained no recognised private-key block.
    #[error("tls bundle: no private key found in {0:?}")]
    NoKey(PathBuf),
    /// Cert chain depth exceeds the configured maximum (default 6).
    #[error("tls bundle: chain depth {depth} exceeds max {max}")]
    ChainTooDeep {
        /// Observed depth (number of certs in the bundled chain).
        depth: usize,
        /// Configured maximum.
        max: usize,
    },
    /// Rustls refused the material — usually a cert/key mismatch.
    #[error("tls bundle: rustls config build failed (cert/key mismatch?): {0}")]
    KeyMismatch(String),
}

impl TlsBundleError {
    /// Short, low-cardinality reason string for the
    /// `cert_rotation_failed_total{reason}` metric.
    #[must_use]
    pub const fn reason(&self) -> &'static str {
        match self {
            Self::Io { .. } => "io",
            Self::CertParse(..) => "cert_parse",
            Self::KeyParse { .. } => "key_parse",
            Self::EmptyChain(_) => "empty_chain",
            Self::NoKey(_) => "no_key",
            Self::ChainTooDeep { .. } => "chain_too_deep",
            Self::KeyMismatch(_) => "key_mismatch",
        }
    }
}

/// Cert-chain depth cap. Real chains rarely exceed 4; deeper ones signal a bad bundle glob.
pub const DEFAULT_MAX_CHAIN_DEPTH: usize = 6;

/// A validated TLS cert + key snapshot, read per accept out of a [`SharedTlsBundle`].
pub struct TlsConfigBundle {
    /// Parsed cert chain in leaf-first order (rustls convention).
    pub cert_chain: Vec<CertificateDer<'static>>,
    /// Parsed private key matching the leaf cert.
    pub key: PrivateKeyDer<'static>,
    /// Leaf `notAfter`, best-effort; `UNIX_EPOCH` when unparsed.
    pub not_after: SystemTime,
    /// Server config built from the cert+key; wire into a fresh `TlsAcceptor` per accept.
    pub server_config: Arc<rustls::ServerConfig>,
    /// Monotonic-clock instant the bundle was loaded.
    loaded_at: Instant,
    /// Wall-clock time the bundle was loaded.
    loaded_at_wall: SystemTime,
}

impl fmt::Debug for TlsConfigBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TlsConfigBundle")
            .field("chain_depth", &self.cert_chain.len())
            .field("not_after", &self.not_after)
            .field("loaded_at_wall", &self.loaded_at_wall)
            .finish_non_exhaustive()
    }
}

/// The hot-reloadable bundle handle every TLS listener holds.
pub type SharedTlsBundle = Arc<ArcSwap<TlsConfigBundle>>;

impl TlsConfigBundle {
    /// Load, validate and smoke-build a bundle from disk. Never partially constructed.
    ///
    /// # Errors
    ///
    /// [`TlsBundleError`] on any I/O, parse or validation failure.
    pub fn load_from_paths(
        cert: &Path,
        key: &Path,
        alpn: &[&[u8]],
    ) -> Result<Self, TlsBundleError> {
        Self::load_from_paths_with(cert, key, alpn, DEFAULT_MAX_CHAIN_DEPTH, None)
    }

    /// [`Self::load_from_paths`] with an explicit depth cap and a `ticketer` to carry the
    /// session-ticket rotator across a cert swap (REL-2-03).
    ///
    /// # Errors
    ///
    /// Same shape as [`Self::load_from_paths`].
    pub fn load_from_paths_with(
        cert: &Path,
        key: &Path,
        alpn: &[&[u8]],
        max_depth: usize,
        ticketer: Option<Arc<dyn ProducesTickets>>,
    ) -> Result<Self, TlsBundleError> {
        let cert_file = std::fs::File::open(cert).map_err(|e| TlsBundleError::Io {
            path: cert.to_path_buf(),
            source: e,
        })?;
        let mut cert_reader = BufReader::new(cert_file);
        let chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| TlsBundleError::CertParse(cert.display().to_string(), e))?;
        if chain.is_empty() {
            return Err(TlsBundleError::EmptyChain(cert.to_path_buf()));
        }
        if chain.len() > max_depth {
            return Err(TlsBundleError::ChainTooDeep {
                depth: chain.len(),
                max: max_depth,
            });
        }

        let key_file = std::fs::File::open(key).map_err(|e| TlsBundleError::Io {
            path: key.to_path_buf(),
            source: e,
        })?;
        let mut key_reader = BufReader::new(key_file);
        let key_der = rustls_pemfile::private_key(&mut key_reader)
            .map_err(|e| TlsBundleError::KeyParse {
                path: key.to_path_buf(),
                source: e,
            })?
            .ok_or_else(|| TlsBundleError::NoKey(key.to_path_buf()))?;

        // Left unparsed on purpose: an x509 crate is a heavy supply-chain edge for one warn-only
        // field, so observability computes the expiry gauge from the exposed cert DER instead.
        let not_after = SystemTime::UNIX_EPOCH;

        // rustls's own cert-vs-key match check — catches the mismatched-upload mistake.
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| TlsBundleError::KeyMismatch(e.to_string()))?
            .with_no_client_auth();
        let mut cfg = builder
            .with_single_cert(chain.clone(), key_der.clone_key())
            .map_err(|e| TlsBundleError::KeyMismatch(e.to_string()))?;
        if let Some(t) = ticketer {
            cfg.ticketer = t;
        }
        if !alpn.is_empty() {
            cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
        }

        Ok(Self {
            cert_chain: chain,
            key: key_der,
            not_after,
            server_config: Arc::new(cfg),
            loaded_at: Instant::now(),
            loaded_at_wall: SystemTime::now(),
        })
    }

    /// When this bundle was loaded (wall-clock).
    #[must_use]
    pub const fn loaded_at_wall(&self) -> SystemTime {
        self.loaded_at_wall
    }

    /// Monotonic instant when this bundle was loaded.
    #[must_use]
    pub const fn loaded_at(&self) -> Instant {
        self.loaded_at
    }

    /// Hand the bundle to a fresh `Arc<ArcSwap<_>>` for sharing.
    #[must_use]
    pub fn into_shared(self) -> SharedTlsBundle {
        Arc::new(ArcSwap::from_pointee(self))
    }
}

/// Atomically swap in a freshly-loaded bundle. A FAILED reload leaves the old bundle live.
/// `ticketer` carries the rotator across the swap; `None` forces full handshakes afterwards.
///
/// # Errors
///
/// Same shape as [`TlsConfigBundle::load_from_paths`].
pub fn reload_tls_bundle(
    bundle: &SharedTlsBundle,
    cert: &Path,
    key: &Path,
    alpn: &[&[u8]],
    ticketer: Option<Arc<dyn ProducesTickets>>,
) -> Result<(), TlsBundleError> {
    let new =
        TlsConfigBundle::load_from_paths_with(cert, key, alpn, DEFAULT_MAX_CHAIN_DEPTH, ticketer)?;
    bundle.store(Arc::new(new));
    Ok(())
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
        let generated = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let cert_der: Vec<u8> = generated.cert.der().to_vec();
        let key_der: Vec<u8> = generated.signing_key.serialize_der();
        let cert_chain = vec![CertificateDer::from(cert_der)];
        let key = PrivateKeyDer::Pkcs8(rustls_pki_types::PrivatePkcs8KeyDer::from(key_der));

        let rot =
            TicketRotator::new(Duration::from_secs(86_400), Duration::from_secs(3_600)).unwrap();
        let current_handle = rot.current();
        let rot_arc = Arc::new(Mutex::new(rot));
        let server_cfg = build_server_config(Arc::clone(&rot_arc), cert_chain, key, &[]).unwrap();

        let plain = sample_plain();
        let ct = server_cfg
            .ticketer
            .encrypt(&plain)
            .expect("ticketer produced ciphertext");
        let recovered = current_handle
            .decrypt(&ct)
            .expect("current rotator key decrypts its own tickets");
        assert_eq!(recovered, plain);

        // Must be true or rustls never advertises session tickets at all.
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

    fn write_self_signed(dir: &Path, cn: &str) -> (PathBuf, PathBuf) {
        let generated = rcgen::generate_simple_self_signed(vec![cn.to_string()]).unwrap();
        let cert_pem = generated.cert.pem();
        let key_pem = generated.signing_key.serialize_pem();
        let cert_path = dir.join(format!("{cn}.crt"));
        let key_path = dir.join(format!("{cn}.key"));
        std::fs::write(&cert_path, cert_pem).unwrap();
        std::fs::write(&key_path, key_pem).unwrap();
        (cert_path, key_path)
    }

    #[test]
    fn tls_bundle_load_from_paths_round_trip() {
        let dir = tempdir();
        let (cert, key) = write_self_signed(&dir, "a.example");
        let bundle = TlsConfigBundle::load_from_paths(&cert, &key, &[]).unwrap();
        assert_eq!(bundle.cert_chain.len(), 1);
        assert!(Arc::strong_count(&bundle.server_config) >= 1);
    }

    #[test]
    fn tls_bundle_load_from_paths_alpn_advertised() {
        let dir = tempdir();
        let (cert, key) = write_self_signed(&dir, "alpn.example");
        let bundle = TlsConfigBundle::load_from_paths(&cert, &key, &[b"h2", b"http/1.1"]).unwrap();
        assert_eq!(
            bundle.server_config.alpn_protocols,
            vec![b"h2".to_vec(), b"http/1.1".to_vec()]
        );
    }

    #[test]
    fn tls_bundle_load_from_paths_missing_cert_io_error() {
        let dir = tempdir();
        let cert = dir.join("absent.crt");
        let key = dir.join("absent.key");
        let err = TlsConfigBundle::load_from_paths(&cert, &key, &[]).unwrap_err();
        assert_eq!(err.reason(), "io");
    }

    #[test]
    fn tls_bundle_load_from_paths_empty_cert_rejected() {
        let dir = tempdir();
        let cert = dir.join("empty.crt");
        let key = dir.join("dummy.key");
        std::fs::write(&cert, b"").unwrap();
        let generated = rcgen::generate_simple_self_signed(vec!["dummy".to_string()]).unwrap();
        std::fs::write(&key, generated.signing_key.serialize_pem()).unwrap();
        let err = TlsConfigBundle::load_from_paths(&cert, &key, &[]).unwrap_err();
        assert_eq!(err.reason(), "empty_chain");
    }

    #[test]
    fn tls_bundle_mismatched_key_rejected() {
        let dir = tempdir();
        let (cert_a, _key_a) = write_self_signed(&dir, "site-a");
        let (_cert_b, key_b) = write_self_signed(&dir, "site-b");
        let err = TlsConfigBundle::load_from_paths(&cert_a, &key_b, &[]).unwrap_err();
        assert_eq!(err.reason(), "key_mismatch");
    }

    #[test]
    fn reload_tls_bundle_succeeds_swaps_under_readers() {
        let dir = tempdir();
        let (cert_a, key_a) = write_self_signed(&dir, "v1.example");
        let bundle = TlsConfigBundle::load_from_paths(&cert_a, &key_a, &[b"h2"])
            .unwrap()
            .into_shared();
        let snap_before = bundle.load_full();

        let (cert_b, key_b) = write_self_signed(&dir, "v2.example");
        reload_tls_bundle(&bundle, &cert_b, &key_b, &[b"h2"], None).unwrap();
        let snap_after = bundle.load_full();
        assert!(
            !Arc::ptr_eq(&snap_before, &snap_after),
            "reload must swap the inner Arc"
        );
        assert_eq!(
            snap_before.server_config.alpn_protocols,
            vec![b"h2".to_vec()]
        );
    }

    #[test]
    fn reload_tls_bundle_invalid_keeps_old_live() {
        let dir = tempdir();
        let (cert_a, key_a) = write_self_signed(&dir, "live.example");
        let bundle = TlsConfigBundle::load_from_paths(&cert_a, &key_a, &[])
            .unwrap()
            .into_shared();
        let snap_before = bundle.load_full();

        let bogus_cert = dir.join("nope.crt");
        let bogus_key = dir.join("nope.key");
        let err = reload_tls_bundle(&bundle, &bogus_cert, &bogus_key, &[], None).unwrap_err();
        assert_eq!(err.reason(), "io");
        let snap_after = bundle.load_full();
        assert!(
            Arc::ptr_eq(&snap_before, &snap_after),
            "failed reload must keep the same bundle"
        );
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        // Stats-class per docs/decisions/atomics.md: uniqueness is the only property.
        let id = N.fetch_add(1, Ordering::Relaxed); // CLIPPY-OK: stats-class, tempdir id generation
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("eg-tls-bundle-{pid}-{id}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
