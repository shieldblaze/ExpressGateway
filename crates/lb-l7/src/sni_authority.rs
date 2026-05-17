//! PROTO-2-15 — SNI ↔ `:authority` / `Host` agreement validator.
//!
//! RFC 6066 §3 defines Server Name Indication: the TLS client signals
//! the intended server hostname during the ClientHello. RFC 9113
//! §8.3.1 (HTTP/2) and RFC 9110 §7.2 (Host header) require the
//! application-layer authority to identify the same origin. If a
//! client opens a TLS session to `attacker.example` (SNI) and then
//! issues `Host: victim.example`, the gateway should refuse the
//! request: the TLS termination point made an authorisation decision
//! (which certificate to present, which set of policies to apply)
//! based on the SNI, but the routing decision would otherwise be
//! made on the application-layer authority. Forwarding such a
//! request is a host-confusion smuggling primitive — comparable to
//! the `:authority` ≠ `Host` pattern in PROTO-2-01, but one layer
//! lower.
//!
//! The canonical response is **421 Misdirected Request** (RFC 9110
//! §15.5.20), which signals to the client that the connection is
//! authoritative for a different origin and the client should re-
//! resolve / re-connect.
//!
//! ## Wiring status
//!
//! The validator lives here, fully unit-tested. Wiring it on the hot
//! path requires capturing the SNI value from the rustls handshake
//! result and threading it from `crates/lb/src/main.rs` (via the
//! TLS-accept future) down into `H1Proxy::serve_connection` /
//! `H2Proxy::serve_connection`. That main.rs handler change is
//! **DEFERRED to Wave-2c** per the Wave-2b-2 scope (PROTO-2-15
//! plan). When Wave-2c lands the SNI propagation, it should call
//! [`check_sni_authority`] inside the proxy `handle` method
//! immediately after [`crate::h2_proxy::check_authority_host_agreement`]
//! and return [`misdirected_response`] on `Err(_)`.

use http::StatusCode;

/// Result of [`check_sni_authority`].
///
/// `Ok(())` means: no SNI was captured (the connection was plain
/// TCP or the SNI extension was absent), or the SNI is consistent
/// with the application-layer authority.
///
/// `Err(SniMismatch { … })` carries enough context for a structured
/// log line; the proxy renders this as 421 Misdirected Request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SniMismatch {
    /// The SNI value captured from the TLS handshake.
    pub sni: String,
    /// The application-layer authority host (from `:authority` or
    /// `Host`; whichever was present and agreed per PROTO-2-01).
    pub authority: String,
}

impl std::fmt::Display for SniMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "SNI `{}` does not match request authority `{}` (RFC 9110 §15.5.20)",
            self.sni, self.authority
        )
    }
}

impl std::error::Error for SniMismatch {}

/// Verify that the TLS SNI value agrees with the HTTP authority.
///
/// `sni` is the value captured from the rustls handshake (None when
/// the connection is plain TCP or the client omitted the SNI
/// extension — the latter is rare but valid per RFC 6066 §3).
///
/// `authority` is the host component of `:authority` (HTTP/2/3) or
/// `Host` (HTTP/1.1). The caller is responsible for resolving the
/// authority/Host agreement upstream (PROTO-2-01); this validator
/// only compares against the SNI.
///
/// Rules:
///   * No SNI → `Ok(())`. There is nothing to compare against.
///   * Empty `authority` → `Ok(())`. The PROTO-2-01 check upstream
///     already rejects a missing/empty Host on the H1 path; for H2
///     `:authority` is required-and-non-empty by §8.3. Either way,
///     this validator is a co-defence, not the primary gate.
///   * Case-insensitive comparison on the host name (RFC 3986
///     §3.2.2).
///   * Ports are ignored. SNI never carries a port (it's a
///     hostname); the authority may carry one. Comparing on host
///     alone matches `Browser` behaviour and the §8.3.1 carve-out.
///   * Trailing dot is normalised (FQDN form vs. relative form).
///
/// # Errors
///
/// Returns [`SniMismatch`] when the SNI host and the authority host
/// disagree case-insensitively. Callers should render this as a
/// 421 Misdirected Request response.
pub fn check_sni_authority(sni: Option<&str>, authority: &str) -> Result<(), SniMismatch> {
    let Some(sni) = sni else {
        return Ok(());
    };
    if authority.is_empty() {
        return Ok(());
    }
    let sni_norm = normalise_host(sni);
    let (auth_host, _port) = split_host_port(authority);
    let auth_norm = normalise_host(auth_host);
    if sni_norm.eq_ignore_ascii_case(&auth_norm) {
        Ok(())
    } else {
        Err(SniMismatch {
            sni: sni.to_owned(),
            authority: authority.to_owned(),
        })
    }
}

/// Build the canonical 421 Misdirected Request response body.
///
/// The body is the static string `"Misdirected Request: SNI does
/// not match request authority (RFC 9110 §15.5.20)"`. Returning a
/// `(StatusCode, &'static str)` pair keeps this module free of any
/// hyper / http-body dependency; the proxy site builds the actual
/// `Response<BoxBody<…>>` from the pair.
#[must_use]
pub const fn misdirected_response() -> (StatusCode, &'static str) {
    (
        StatusCode::MISDIRECTED_REQUEST,
        "Misdirected Request: SNI does not match request authority (RFC 9110 §15.5.20)",
    )
}

/// Strip a trailing dot from a hostname (FQDN form).
fn normalise_host(s: &str) -> String {
    s.trim_end_matches('.').to_ascii_lowercase()
}

/// Split `host[:port]`, IPv6-bracket aware. Mirror of the helpers in
/// `h2_proxy.rs` / `h2_to_h1.rs`; duplicated to keep this module
/// dependency-free.
fn split_host_port(s: &str) -> (&str, Option<&str>) {
    if let Some(stripped) = s.strip_prefix('[') {
        if let Some(end) = stripped.find(']') {
            let host_with_brackets = &s[..=end + 1];
            let rest = &s[end + 2..];
            let port = rest.strip_prefix(':');
            return (host_with_brackets, port.filter(|p| !p.is_empty()));
        }
        return (s, None);
    }
    match s.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (s, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_sni_passes() {
        assert!(check_sni_authority(None, "example.test").is_ok());
    }

    #[test]
    fn empty_authority_passes() {
        // PROTO-2-01 covers the empty-authority case upstream; this
        // validator is a co-defence, not the primary gate.
        assert!(check_sni_authority(Some("example.test"), "").is_ok());
    }

    #[test]
    fn matching_pair_passes() {
        assert!(check_sni_authority(Some("example.test"), "example.test").is_ok());
    }

    #[test]
    fn case_insensitive_match() {
        assert!(check_sni_authority(Some("EXAMPLE.TEST"), "example.test").is_ok());
        assert!(check_sni_authority(Some("example.test"), "Example.Test").is_ok());
    }

    #[test]
    fn mismatch_rejected() {
        let err = check_sni_authority(Some("attacker.example"), "victim.example").unwrap_err();
        assert_eq!(err.sni, "attacker.example");
        assert_eq!(err.authority, "victim.example");
        assert!(err.to_string().contains("RFC 9110 §15.5.20"));
    }

    #[test]
    fn authority_with_port_compared_on_host_only() {
        // SNI never carries a port; an authority may. Compare on
        // host only.
        assert!(check_sni_authority(Some("example.test"), "example.test:8443").is_ok());
    }

    #[test]
    fn trailing_dot_normalised() {
        assert!(check_sni_authority(Some("example.test."), "example.test").is_ok());
        assert!(check_sni_authority(Some("example.test"), "example.test.").is_ok());
    }

    #[test]
    fn ipv6_authority() {
        assert!(check_sni_authority(Some("[::1]"), "[::1]:443").is_ok());
        // Different host literal must reject.
        assert!(check_sni_authority(Some("[::1]"), "[::2]:443").is_err());
    }

    #[test]
    fn misdirected_response_is_421() {
        let (status, body) = misdirected_response();
        assert_eq!(status, StatusCode::MISDIRECTED_REQUEST);
        assert!(body.contains("Misdirected Request"));
    }
}
