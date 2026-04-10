//! SNI-based certificate resolution.
//!
//! Implements the rustls [`ResolvesServerCert`] trait with:
//! - Exact hostname matching
//! - Wildcard matching (`*.example.com`)
//! - Default certificate fallback
//! - Thread-safe via [`ArcSwap`]

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use arc_swap::ArcSwap;
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

/// Internal map of SNI hostnames to certified keys.
struct SniCertMap {
    /// Exact hostname -> certificate mapping.
    exact: HashMap<String, Arc<CertifiedKey>>,
    /// Wildcard patterns and their certificates (e.g., `*.example.com`).
    wildcard: Vec<(String, Arc<CertifiedKey>)>,
}

impl SniCertMap {
    fn new() -> Self {
        Self {
            exact: HashMap::new(),
            wildcard: Vec::new(),
        }
    }
}

/// SNI-based certificate resolver that supports exact match, wildcard, and default fallback.
///
/// Thread-safe: the internal cert map and default cert are stored behind [`ArcSwap`],
/// allowing lock-free reads and atomic updates.
pub struct SniResolver {
    certs: ArcSwap<SniCertMap>,
    default_cert: ArcSwap<Option<Arc<CertifiedKey>>>,
}

impl fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let map = self.certs.load();
        f.debug_struct("SniResolver")
            .field("exact_entries", &map.exact.len())
            .field("wildcard_entries", &map.wildcard.len())
            .field("has_default", &self.default_cert.load().is_some())
            .finish()
    }
}

impl SniResolver {
    /// Create a new, empty SNI resolver.
    pub fn new() -> Self {
        Self {
            certs: ArcSwap::from_pointee(SniCertMap::new()),
            default_cert: ArcSwap::from_pointee(None),
        }
    }

    /// Set the default certificate returned when no SNI match is found.
    pub fn set_default(&self, cert: Arc<CertifiedKey>) {
        self.default_cert.store(Arc::new(Some(cert)));
    }

    /// Add a certificate for a specific hostname or wildcard pattern.
    ///
    /// Hostname is normalized to lowercase. If `hostname` starts with `*.`,
    /// it is stored as a wildcard pattern.
    pub fn add(&self, hostname: &str, cert: Arc<CertifiedKey>) {
        let hostname = hostname.to_lowercase();
        let old = self.certs.load();

        let mut new_map = SniCertMap {
            exact: old.exact.clone(),
            wildcard: old.wildcard.clone(),
        };

        if hostname.starts_with("*.") {
            // Store wildcard: remove existing entry for same pattern, then add
            new_map.wildcard.retain(|(p, _)| p != &hostname);
            new_map.wildcard.push((hostname, cert));
        } else {
            new_map.exact.insert(hostname, cert);
        }

        self.certs.store(Arc::new(new_map));
    }

    /// Remove a certificate for a specific hostname or wildcard pattern.
    pub fn remove(&self, hostname: &str) {
        let hostname = hostname.to_lowercase();
        let old = self.certs.load();

        let mut new_map = SniCertMap {
            exact: old.exact.clone(),
            wildcard: old.wildcard.clone(),
        };

        if hostname.starts_with("*.") {
            new_map.wildcard.retain(|(p, _)| p != &hostname);
        } else {
            new_map.exact.remove(&hostname);
        }

        self.certs.store(Arc::new(new_map));
    }

    /// Resolve a hostname to a certificate.
    ///
    /// Resolution order:
    /// 1. Exact match
    /// 2. Wildcard match (first matching `*.domain` pattern)
    /// 3. Default certificate
    fn resolve_hostname(&self, hostname: &str) -> Option<Arc<CertifiedKey>> {
        let hostname = hostname.to_lowercase();
        let map = self.certs.load();

        // 1. Exact match
        if let Some(cert) = map.exact.get(&hostname) {
            return Some(Arc::clone(cert));
        }

        // 2. Wildcard match: for "foo.example.com", check "*.example.com"
        if let Some(dot_pos) = hostname.find('.') {
            let wildcard_pattern = format!("*{}", &hostname[dot_pos..]);
            for (pattern, cert) in &map.wildcard {
                if *pattern == wildcard_pattern {
                    return Some(Arc::clone(cert));
                }
            }
        }

        // 3. Default fallback
        let default = self.default_cert.load();
        default.as_ref().as_ref().map(Arc::clone)
    }
}

impl Default for SniResolver {
    fn default() -> Self {
        Self::new()
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        match client_hello.server_name() {
            Some(name) => self.resolve_hostname(name),
            None => {
                // No SNI provided, use default certificate
                let default = self.default_cert.load();
                default.as_ref().as_ref().map(Arc::clone)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::crypto::ring as ring_provider;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use rustls::sign::CertifiedKey;

    /// Create a dummy CertifiedKey for testing.
    /// We use a real cert+key pair so CertifiedKey::from_der succeeds.
    fn make_test_certified_key() -> Arc<CertifiedKey> {
        let cert_pem = include_str!("../tests/data/cert.pem");
        let key_pem = include_str!("../tests/data/key.pem");

        let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_pem.as_bytes())
            .unwrap()
            .unwrap();

        let provider = ring_provider::default_provider();
        let ck = CertifiedKey::from_der(certs, key, &provider).unwrap();
        Arc::new(ck)
    }

    #[test]
    fn exact_match() {
        let resolver = SniResolver::new();
        let ck = make_test_certified_key();
        resolver.add("example.com", Arc::clone(&ck));

        let result = resolver.resolve_hostname("example.com");
        assert!(result.is_some(), "exact match should resolve");
    }

    #[test]
    fn exact_match_case_insensitive() {
        let resolver = SniResolver::new();
        let ck = make_test_certified_key();
        resolver.add("Example.COM", Arc::clone(&ck));

        let result = resolver.resolve_hostname("example.com");
        assert!(
            result.is_some(),
            "case-insensitive exact match should resolve"
        );
    }

    #[test]
    fn wildcard_match() {
        let resolver = SniResolver::new();
        let ck = make_test_certified_key();
        resolver.add("*.example.com", Arc::clone(&ck));

        let result = resolver.resolve_hostname("foo.example.com");
        assert!(result.is_some(), "wildcard should match sub.example.com");

        let result = resolver.resolve_hostname("bar.example.com");
        assert!(result.is_some(), "wildcard should match bar.example.com");
    }

    #[test]
    fn wildcard_does_not_match_bare_domain() {
        let resolver = SniResolver::new();
        let ck = make_test_certified_key();
        resolver.add("*.example.com", Arc::clone(&ck));

        let result = resolver.resolve_hostname("example.com");
        assert!(
            result.is_none(),
            "wildcard *.example.com should not match example.com"
        );
    }

    #[test]
    fn default_fallback() {
        let resolver = SniResolver::new();
        let ck = make_test_certified_key();
        resolver.set_default(Arc::clone(&ck));

        let result = resolver.resolve_hostname("unknown.com");
        assert!(
            result.is_some(),
            "default should be returned for unknown hostname"
        );
    }

    #[test]
    fn exact_takes_priority_over_wildcard() {
        let resolver = SniResolver::new();
        let ck1 = make_test_certified_key();
        let ck2 = make_test_certified_key();

        resolver.add("*.example.com", ck1);
        resolver.add("special.example.com", ck2);

        let result = resolver.resolve_hostname("special.example.com");
        assert!(result.is_some(), "exact match should take priority");
    }

    #[test]
    fn no_match_returns_none_without_default() {
        let resolver = SniResolver::new();
        let result = resolver.resolve_hostname("unknown.com");
        assert!(result.is_none(), "should return None without default");
    }

    #[test]
    fn remove_exact() {
        let resolver = SniResolver::new();
        let ck = make_test_certified_key();
        resolver.add("example.com", Arc::clone(&ck));
        resolver.remove("example.com");

        let result = resolver.resolve_hostname("example.com");
        assert!(result.is_none(), "removed hostname should not resolve");
    }

    #[test]
    fn remove_wildcard() {
        let resolver = SniResolver::new();
        let ck = make_test_certified_key();
        resolver.add("*.example.com", Arc::clone(&ck));
        resolver.remove("*.example.com");

        let result = resolver.resolve_hostname("foo.example.com");
        assert!(result.is_none(), "removed wildcard should not resolve");
    }
}
