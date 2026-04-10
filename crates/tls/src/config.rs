//! TLS configuration builder for `rustls` `ServerConfig` and `ClientConfig`.
//!
//! Builds correctly configured TLS configs from ExpressGateway's config types,
//! using the appropriate cipher suites, protocol versions, ALPN, and mTLS settings.

use std::sync::Arc;

use anyhow::Result;
use expressgateway_core::types::{MutualTlsMode, TlsProfile};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore, ServerConfig};

use crate::alpn;
use crate::ciphers;
use crate::mtls;

/// Builder for TLS configurations.
pub struct TlsConfigBuilder;

impl TlsConfigBuilder {
    /// Build a `rustls` [`ServerConfig`] from certificate chain, private key,
    /// TLS profile, mTLS mode, and optional trust CA for client verification.
    ///
    /// The resulting config has:
    /// - Cipher suites and protocol versions based on the `profile`
    /// - ALPN configured for h2 + http/1.1
    /// - Client certificate verification based on `mtls` mode
    pub fn server_config(
        cert_chain: Vec<CertificateDer<'static>>,
        key: PrivateKeyDer<'static>,
        profile: TlsProfile,
        mtls: MutualTlsMode,
        trust_ca: Option<&[u8]>,
    ) -> Result<ServerConfig> {
        let cipher_suites = ciphers::cipher_suites_for_profile(profile);
        let versions = ciphers::protocol_versions_for_profile(profile);

        // Build a custom CryptoProvider with our selected cipher suites.
        let base_provider = rustls::crypto::ring::default_provider();
        let provider = rustls::crypto::CryptoProvider {
            cipher_suites,
            ..base_provider
        };

        let verifier = mtls::build_client_cert_verifier(mtls, trust_ca)?;

        let mut config = ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&versions)
            .map_err(|e| anyhow::anyhow!("failed to set protocol versions: {e}"))?
            .with_client_cert_verifier(verifier)
            .with_single_cert(cert_chain, key)
            .map_err(|e| anyhow::anyhow!("failed to set server certificate: {e}"))?;

        // Configure ALPN for HTTP/2 and HTTP/1.1.
        alpn::configure_alpn(&mut config);

        Ok(config)
    }

    /// Build a `rustls` [`ClientConfig`] for outbound connections.
    ///
    /// Uses the system/webpki root certificates for server verification.
    /// If `verify_hostname` is false, a dangerous no-verification config is created
    /// (for testing only).
    pub fn client_config(verify_hostname: bool) -> Result<ClientConfig> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());

        if verify_hostname {
            let mut root_store = RootCertStore::empty();
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());

            let config = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| anyhow::anyhow!("failed to set protocol versions: {e}"))?
                .with_root_certificates(root_store)
                .with_no_client_auth();

            Ok(config)
        } else {
            // Dangerous: skip hostname verification (testing only).
            let config = ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| anyhow::anyhow!("failed to set protocol versions: {e}"))?
                .dangerous()
                .with_custom_certificate_verifier(Arc::new(NoVerifier))
                .with_no_client_auth();

            Ok(config)
        }
    }
}

/// A dangerous server certificate verifier that accepts any certificate.
/// Only for testing and development purposes.
#[derive(Debug)]
struct NoVerifier;

impl rustls::client::danger::ServerCertVerifier for NoVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    #[test]
    fn client_config_with_verification() {
        install_crypto_provider();
        let config = TlsConfigBuilder::client_config(true);
        assert!(
            config.is_ok(),
            "client config with verification should build"
        );
    }

    #[test]
    fn client_config_without_verification() {
        install_crypto_provider();
        let config = TlsConfigBuilder::client_config(false);
        assert!(
            config.is_ok(),
            "client config without verification should build"
        );
    }
}
