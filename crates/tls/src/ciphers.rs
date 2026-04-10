//! Cipher suite selection based on TLS profile.
//!
//! Provides two profiles:
//! - **Modern**: TLS 1.3 only (3 cipher suites)
//! - **Intermediate**: TLS 1.2 + 1.3 (7 cipher suites)

use expressgateway_core::types::TlsProfile;
use rustls::crypto::ring as ring_provider;
use rustls::{CipherSuite, SupportedCipherSuite, SupportedProtocolVersion};

/// TLS 1.3 cipher suite identifiers for the Modern profile.
const MODERN_SUITES: &[CipherSuite] = &[
    CipherSuite::TLS13_AES_256_GCM_SHA384,
    CipherSuite::TLS13_AES_128_GCM_SHA256,
    CipherSuite::TLS13_CHACHA20_POLY1305_SHA256,
];

/// TLS 1.2 ECDHE cipher suite identifiers for the Intermediate profile
/// (in addition to all Modern suites).
const INTERMEDIATE_TLS12_SUITES: &[CipherSuite] = &[
    CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384,
    CipherSuite::TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256,
    CipherSuite::TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384,
    CipherSuite::TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256,
];

/// Return the filtered set of cipher suites for the given [`TlsProfile`].
pub fn cipher_suites_for_profile(profile: TlsProfile) -> Vec<SupportedCipherSuite> {
    let allowed: Vec<CipherSuite> = match profile {
        TlsProfile::Modern => MODERN_SUITES.to_vec(),
        TlsProfile::Intermediate => {
            let mut suites = MODERN_SUITES.to_vec();
            suites.extend_from_slice(INTERMEDIATE_TLS12_SUITES);
            suites
        }
    };

    ring_provider::ALL_CIPHER_SUITES
        .iter()
        .filter(|cs| {
            let suite_id = cs.suite();
            allowed.contains(&suite_id)
        })
        .copied()
        .collect()
}

/// Return the protocol versions enabled for the given [`TlsProfile`].
pub fn protocol_versions_for_profile(
    profile: TlsProfile,
) -> Vec<&'static SupportedProtocolVersion> {
    match profile {
        TlsProfile::Modern => vec![&rustls::version::TLS13],
        TlsProfile::Intermediate => vec![&rustls::version::TLS13, &rustls::version::TLS12],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_profile_has_three_tls13_suites() {
        let suites = cipher_suites_for_profile(TlsProfile::Modern);
        assert_eq!(suites.len(), 3, "Modern profile must have exactly 3 suites");
        for cs in &suites {
            let name = format!("{:?}", cs.suite());
            assert!(
                name.starts_with("TLS13_"),
                "Modern suite {name} should be TLS 1.3"
            );
        }
    }

    #[test]
    fn intermediate_profile_has_seven_suites() {
        let suites = cipher_suites_for_profile(TlsProfile::Intermediate);
        assert_eq!(
            suites.len(),
            7,
            "Intermediate profile must have exactly 7 suites"
        );

        let tls13_count = suites
            .iter()
            .filter(|cs| format!("{:?}", cs.suite()).starts_with("TLS13_"))
            .count();
        let tls12_count = suites
            .iter()
            .filter(|cs| format!("{:?}", cs.suite()).starts_with("TLS_ECDHE"))
            .count();
        assert_eq!(tls13_count, 3, "Should have 3 TLS 1.3 suites");
        assert_eq!(tls12_count, 4, "Should have 4 TLS 1.2 ECDHE suites");
    }

    #[test]
    fn modern_profile_versions_tls13_only() {
        let versions = protocol_versions_for_profile(TlsProfile::Modern);
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0].version, rustls::ProtocolVersion::TLSv1_3);
    }

    #[test]
    fn intermediate_profile_versions_both() {
        let versions = protocol_versions_for_profile(TlsProfile::Intermediate);
        assert_eq!(versions.len(), 2);
        let version_set: Vec<_> = versions.iter().map(|v| v.version).collect();
        assert!(version_set.contains(&rustls::ProtocolVersion::TLSv1_3));
        assert!(version_set.contains(&rustls::ProtocolVersion::TLSv1_2));
    }

    #[test]
    fn modern_suites_are_subset_of_intermediate() {
        let modern = cipher_suites_for_profile(TlsProfile::Modern);
        let intermediate = cipher_suites_for_profile(TlsProfile::Intermediate);

        for ms in &modern {
            assert!(
                intermediate.iter().any(|is| is.suite() == ms.suite()),
                "Modern suite {:?} should be in Intermediate profile",
                ms.suite()
            );
        }
    }
}
