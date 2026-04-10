//! Mutual TLS (mTLS) client certificate verification.
//!
//! Three modes:
//! - **NotRequired**: no client certificate requested
//! - **Optional**: client certificate requested but connection allowed without
//! - **Required**: valid client certificate mandatory

use std::sync::Arc;

use expressgateway_core::types::MutualTlsMode;
use rustls::pki_types::CertificateDer;
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::ClientCertVerifier;
use rustls::RootCertStore;

use crate::error::{Result, TlsError};

/// Build a [`ClientCertVerifier`] for the given mTLS mode.
///
/// For `Optional` and `Required` modes, a trust CA certificate must be provided
/// to validate client certificates against.
///
/// For `NotRequired` mode, `trust_ca_der` is ignored and no client cert is requested.
pub fn build_client_cert_verifier(
    mode: MutualTlsMode,
    trust_ca_der: Option<&[u8]>,
) -> Result<Arc<dyn ClientCertVerifier>> {
    match mode {
        MutualTlsMode::NotRequired => Ok(Arc::new(rustls::server::NoClientAuth)),
        MutualTlsMode::Optional => {
            let roots = build_root_store(trust_ca_der, "Optional")?;
            let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
                .allow_unauthenticated()
                .build()
                .map_err(|e| TlsError::ClientVerifier {
                    reason: e.to_string(),
                })?;
            Ok(verifier)
        }
        MutualTlsMode::Required => {
            let roots = build_root_store(trust_ca_der, "Required")?;
            let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
                .build()
                .map_err(|e| TlsError::ClientVerifier {
                    reason: e.to_string(),
                })?;
            Ok(verifier)
        }
    }
}

/// Build a [`RootCertStore`] from a DER-encoded CA certificate.
fn build_root_store(trust_ca_der: Option<&[u8]>, mode: &'static str) -> Result<RootCertStore> {
    let ca_bytes = trust_ca_der.ok_or(TlsError::MissingTrustCa { mode })?;

    let mut roots = RootCertStore::empty();
    let cert = CertificateDer::from(ca_bytes.to_vec());
    roots.add(cert).map_err(|e| TlsError::TrustCaAdd {
        reason: e.to_string(),
    })?;

    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_required_needs_no_ca() {
        let verifier = build_client_cert_verifier(MutualTlsMode::NotRequired, None);
        assert!(verifier.is_ok(), "NotRequired should succeed without CA");
    }

    #[test]
    fn optional_without_ca_fails() {
        let verifier = build_client_cert_verifier(MutualTlsMode::Optional, None);
        assert!(verifier.is_err(), "Optional without CA should fail");
        assert!(matches!(
            verifier.unwrap_err(),
            TlsError::MissingTrustCa { mode: "Optional" }
        ));
    }

    #[test]
    fn required_without_ca_fails() {
        let verifier = build_client_cert_verifier(MutualTlsMode::Required, None);
        assert!(verifier.is_err(), "Required without CA should fail");
        assert!(matches!(
            verifier.unwrap_err(),
            TlsError::MissingTrustCa { mode: "Required" }
        ));
    }
}
