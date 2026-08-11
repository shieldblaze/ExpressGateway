//! SNI ↔ `:authority` / `Host` agreement validator.
//!
//! RFC 6066 §3 SNI signals the intended server hostname in the ClientHello;
//! RFC 9113 §8.3.1 and RFC 9110 §7.2 require the application-layer authority to
//! identify the same origin. A client that opens TLS to `attacker.example` and
//! then sends `Host: victim.example` is exercising a host-confusion smuggling
//! primitive one layer below PROTO-2-01: the TLS termination point chose a
//! certificate and a policy set from the SNI, while routing would otherwise
//! follow the application-layer authority. The canonical refusal is
//! **421 Misdirected Request** (RFC 9110 §15.5.20).
//!
//! The validator IS wired on the hot path — `h1_proxy`, `h2_proxy`, and the
//! binary's TLS-accept site thread the observed SNI in.

use http::StatusCode;

/// Mismatch context from [`check_sni_authority`], carrying enough for a
/// structured log line; the proxy renders it as 421 Misdirected Request.
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
/// `sni` is `None` on plain TCP or when the client omitted the extension (rare
/// but valid per RFC 6066 §3) — nothing to compare, so `Ok(())`. An empty
/// `authority` is also `Ok(())`: PROTO-2-01 is the primary gate and this is a
/// co-defence. Comparison is case-insensitive on the host (RFC 3986 §3.2.2),
/// ignores ports (SNI never carries one), and normalises a trailing dot.
///
/// # Errors
///
/// [`SniMismatch`] when the SNI host and the authority host disagree.
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

/// The canonical 421 Misdirected Request status + body. Returning a
/// `(StatusCode, &'static str)` pair keeps this module free of any hyper /
/// http-body dependency; the proxy site builds the actual response.
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

/// Split `host[:port]`, IPv6-bracket aware. Duplicated from `h2_proxy.rs` to
/// keep this module dependency-free.
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
        // PROTO-2-01 covers empty authority upstream; this is a co-defence.
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
        // SNI never carries a port; an authority may.
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
