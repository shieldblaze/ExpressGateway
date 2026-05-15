//! ROUND8-L7-09 / ROUND8-L7-16 — protocol-neutral authority validator
//! (H1/H2 `http::Request` wrapper).
//!
//! References:
//! - HAProxy `BUG/MAJOR: http: forbid comma character in authority
//!   value`.
//! - HAProxy `BUG/MEDIUM: h1: Enforce the authority validation during
//!   H1 request parsing` (H1 parser missed the check that H2/H3 had).
//! - RFC 9110 §4 authority component definition.
//! - RFC 3986 §3.2 host = IP-literal / IPv4 / reg-name.
//!
//! ROUND8-L7-16: the byte-level predicate ([`validate`] /
//! [`AuthorityError`]) was hoisted into `lb-core` (a leaf crate both
//! `lb-l7` and `lb-quic` depend on — no cycle, `lb-core` has zero
//! `lb-*` deps) so the H3/QUIC datapath shares the EXACT same
//! implementation rather than re-deriving it. Re-implementing it
//! per-protocol is precisely the H1-vs-H2-vs-H3 divergence the
//! HAProxy `BUG/MEDIUM` fix warns about. This module is now only the
//! hyper/`http`-version-specific request wrapper; the predicate is
//! re-exported verbatim so every existing
//! `crate::authority::{validate, AuthorityError}` callsite stays
//! byte-identical with the H3 path.
//!
//! Today the gateway validates SNI / Host agreement
//! (`sni_authority::check_sni_authority`) but does not sanitise the
//! authority value itself. A `,` / whitespace / control byte inside
//! the value can desync upstream routing decisions or punch through
//! a Host-based ACL. This validator runs on every parser path before
//! the agreement comparison.

// ROUND8-L7-16: single source of truth lives in `lb-core`.
pub use lb_core::authority::{AuthorityError, validate};

/// ROUND8-L7-09 choke point — validate every authority value carried
/// by an inbound request, regardless of which downstream path the
/// request will take (plain, WebSocket-upgrade, H2 extended-CONNECT,
/// or gRPC).
///
/// This is the SINGLE place both the H1 and H2 dispatchers call, at
/// the very top of `handle_inner`, BEFORE the fork into the
/// upgrade / CONNECT / gRPC handlers. Hoisting the check above the
/// fork (rather than duplicating it into each handler) is the
/// less-rot-prone shape: a new fork added later inherits the check
/// for free.
///
/// Both candidates are validated when present:
/// - the URI authority (H2 `:authority` pseudo-header, surfaced by
///   hyper as `uri.authority()`; also any absolute-form H1 target);
/// - the `Host` header (H1 carries the authority here per RFC 9112
///   §3.2; an H2 client may additionally send `Host`).
///
/// An absent or empty value is NOT rejected here — PROTO-2-01 covers
/// the missing-authority gate; this predicate only sanitises a
/// present value (HAProxy `BUG/MAJOR` comma class).
///
/// There is deliberately NO loopback exemption: the loopback carve-out
/// applies only to the SNI-vs-Host *agreement* check, not to authority
/// value sanitisation. Applying the carve-out here would make the
/// upgrade path looser than the plain path — exactly the H1-vs-H2
/// divergence the HAProxy `BUG/MEDIUM` fix was about.
///
/// # Errors
///
/// Returns the first offending value together with its
/// [`AuthorityError`] when any present candidate fails [`validate`].
pub fn validate_request<B>(req: &http::Request<B>) -> Result<(), (String, AuthorityError)> {
    for candidate in [
        req.uri().authority().map(http::uri::Authority::as_str),
        req.headers()
            .get(http::header::HOST)
            .and_then(|v| v.to_str().ok()),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    {
        if let Err(err) = validate(candidate) {
            return Err((candidate.to_owned(), err));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    // ROUND8-L7-16: the byte-level predicate's own unit tests live in
    // `lb_core::authority`. These pin the H1/H2 `http::Request`
    // wrapper behaviour the QUIC path does NOT share (URI authority +
    // Host header extraction, empty/absent skip semantics).

    #[test]
    fn predicate_reexport_is_lb_core() {
        // The re-exported predicate must be byte-identical to the
        // shared one (same rejects, same loopback policy: none).
        assert_eq!(validate("a,b"), Err(AuthorityError::Comma));
        assert_eq!(validate("a b"), Err(AuthorityError::Whitespace));
        assert!(validate("example.com:8080").is_ok());
    }

    #[test]
    fn request_uri_authority_validated() {
        let req = http::Request::builder()
            .uri("http://victim.example,attacker.example/p")
            .body(())
            .unwrap();
        assert_eq!(
            validate_request(&req),
            Err((
                "victim.example,attacker.example".to_owned(),
                AuthorityError::Comma
            ))
        );
    }

    #[test]
    fn request_host_header_validated() {
        let req = http::Request::builder()
            .uri("/p")
            .header(http::header::HOST, "victim.example attacker")
            .body(())
            .unwrap();
        assert_eq!(
            validate_request(&req),
            Err((
                "victim.example attacker".to_owned(),
                AuthorityError::Whitespace
            ))
        );
    }

    #[test]
    fn request_absent_and_empty_authority_skipped() {
        // No authority, no Host → nothing to sanitise (PROTO-2-01's
        // gate, not this predicate's).
        let req = http::Request::builder().uri("/p").body(()).unwrap();
        assert_eq!(validate_request(&req), Ok(()));
    }

    #[test]
    fn request_valid_authority_passes() {
        let req = http::Request::builder()
            .uri("http://example.test:8080/p")
            .body(())
            .unwrap();
        assert_eq!(validate_request(&req), Ok(()));
    }
}
