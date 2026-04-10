//! Mutual TLS (mTLS) client certificate verification.
//!
//! Three modes:
//! - **NotRequired**: no client certificate requested
//! - **Optional**: client certificate requested but connection allowed without
//! - **Required**: valid client certificate mandatory

use std::sync::Arc;

use anyhow::{Context, Result};
use expressgateway_core::types::MutualTlsMode;
use rustls::server::WebPkiClientVerifier;
use rustls::server::danger::ClientCertVerifier;
use rustls::{RootCertStore, pki_types::CertificateDer};

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
        MutualTlsMode::NotRequired => {
            // Return the NoClientAuth verifier (which is what
            // `with_no_client_auth` uses internally).
            Ok(Arc::new(rustls::server::NoClientAuth))
        }
        MutualTlsMode::Optional => {
            let roots = build_root_store(trust_ca_der)?;
            let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
                .allow_unauthenticated()
                .build()
                .map_err(|e| anyhow::anyhow!("failed to build optional client verifier: {e}"))?;
            Ok(verifier)
        }
        MutualTlsMode::Required => {
            let roots = build_root_store(trust_ca_der)?;
            let verifier = WebPkiClientVerifier::builder(Arc::new(roots))
                .build()
                .map_err(|e| anyhow::anyhow!("failed to build required client verifier: {e}"))?;
            Ok(verifier)
        }
    }
}

/// Build a [`RootCertStore`] from a DER-encoded CA certificate.
fn build_root_store(trust_ca_der: Option<&[u8]>) -> Result<RootCertStore> {
    let ca_bytes =
        trust_ca_der.context("trust CA certificate is required for mTLS Optional/Required mode")?;

    let mut roots = RootCertStore::empty();
    let cert = CertificateDer::from(ca_bytes.to_vec());
    roots
        .add(cert)
        .map_err(|e| anyhow::anyhow!("failed to add trust CA to root store: {e}"))?;

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
    }

    #[test]
    fn required_without_ca_fails() {
        let verifier = build_client_cert_verifier(MutualTlsMode::Required, None);
        assert!(verifier.is_err(), "Required without CA should fail");
    }
}
