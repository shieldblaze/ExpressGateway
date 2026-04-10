//! Certificate loading and expiry checking.
//!
//! Supports PEM-encoded certificates and private keys (PKCS#1, PKCS#8, SEC1/EC).
//! Rejects DSA keys. Provides X.509 expiry checking with 30-day renewal warnings.
//!
//! All functions return crate-specific [`TlsError`] variants for structured
//! error handling without `anyhow` in library code.

use std::fs;
use std::io::BufReader;
use std::path::Path;

use chrono::{DateTime, Utc};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tracing::warn;

use crate::error::{Result, TlsError};

/// Certificate expiration information.
#[derive(Debug, Clone)]
pub struct CertExpiry {
    /// Certificate validity start time.
    pub not_before: DateTime<Utc>,
    /// Certificate validity end time.
    pub not_after: DateTime<Utc>,
    /// Days remaining until certificate expiration.
    pub days_remaining: i64,
    /// Whether the certificate has already expired.
    pub is_expired: bool,
    /// Whether the certificate needs renewal (less than 30 days remaining).
    pub needs_renewal: bool,
}

/// Load PEM-encoded certificates from a file.
///
/// Returns a chain of DER-encoded certificates in the order they appear in the PEM file.
pub fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>> {
    let file = fs::File::open(path).map_err(|e| TlsError::FileOpen {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut reader = BufReader::new(file);

    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|_| TlsError::PemCertParse {
            path: path.to_path_buf(),
        })?;

    if certs.is_empty() {
        return Err(TlsError::NoCertificates {
            path: path.to_path_buf(),
        });
    }

    Ok(certs)
}

/// Load a PEM-encoded private key from a file.
///
/// Supports PKCS#1 (RSA), PKCS#8, and SEC1 (EC) key formats.
pub fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>> {
    let file = fs::File::open(path).map_err(|e| TlsError::FileOpen {
        path: path.to_path_buf(),
        source: e,
    })?;
    let mut reader = BufReader::new(file);

    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|_| TlsError::PemKeyParse {
            path: path.to_path_buf(),
        })?
        .ok_or_else(|| TlsError::NoPrivateKey {
            path: path.to_path_buf(),
        })?;

    Ok(key)
}

/// Parse a DER-encoded X.509 certificate and check its expiry.
///
/// Logs a warning if the certificate expires within 30 days.
pub fn check_cert_expiry(cert_der: &[u8]) -> Result<CertExpiry> {
    let (_, cert) = x509_parser::parse_x509_certificate(cert_der).map_err(|e| {
        TlsError::X509Parse {
            reason: e.to_string(),
        }
    })?;

    let validity = cert.validity();
    let not_before_ts = validity.not_before.timestamp();
    let not_after_ts = validity.not_after.timestamp();

    let not_before =
        DateTime::<Utc>::from_timestamp(not_before_ts, 0).ok_or(TlsError::InvalidTimestamp)?;
    let not_after =
        DateTime::<Utc>::from_timestamp(not_after_ts, 0).ok_or(TlsError::InvalidTimestamp)?;

    let now = Utc::now();
    let days_remaining = (not_after - now).num_days();
    let is_expired = now > not_after;
    let needs_renewal = days_remaining < 30;

    if needs_renewal && !is_expired {
        warn!(
            days_remaining,
            not_after = %not_after,
            "TLS certificate expires in less than 30 days"
        );
    }

    if is_expired {
        warn!(
            not_after = %not_after,
            "TLS certificate has expired"
        );
    }

    Ok(CertExpiry {
        not_before,
        not_after,
        days_remaining,
        is_expired,
        needs_renewal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cert_path() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/cert.pem"))
    }

    fn test_key_path() -> &'static Path {
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/key.pem"))
    }

    #[test]
    fn load_certs_from_pem() {
        let certs = load_certs(test_cert_path()).expect("should load certs");
        assert_eq!(certs.len(), 1, "should load exactly one certificate");
    }

    #[test]
    fn load_private_key_from_pem() {
        let _key = load_private_key(test_key_path()).expect("should load key");
    }

    #[test]
    fn load_certs_nonexistent_file_fails() {
        let result = load_certs(Path::new("/nonexistent/cert.pem"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TlsError::FileOpen { .. }));
    }

    #[test]
    fn load_private_key_nonexistent_file_fails() {
        let result = load_private_key(Path::new("/nonexistent/key.pem"));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), TlsError::FileOpen { .. }));
    }

    #[test]
    fn check_expiry_parses_certificate() {
        let certs = load_certs(test_cert_path()).expect("should load certs");
        let expiry = check_cert_expiry(certs[0].as_ref()).expect("should parse expiry");

        assert!(!expiry.is_expired, "test cert should not be expired");
        assert!(
            expiry.days_remaining > 30,
            "test cert should not need renewal"
        );
        assert!(!expiry.needs_renewal);
        assert!(expiry.not_before < expiry.not_after);
    }
}
