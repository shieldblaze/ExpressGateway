//! ROUND8-L7-09 / ROUND8-L7-16 — protocol-neutral authority validator: the
//! hyper/`http`-version-specific request wrapper around `lb_core::authority`.
//!
//! A `,` / whitespace / control byte inside an authority value can desync
//! upstream routing or punch through a Host-based ACL (HAProxy
//! `BUG/MAJOR: http: forbid comma character in authority value`, and
//! `BUG/MEDIUM: h1: Enforce the authority validation during H1 request
//! parsing`, where the H1 parser was missing the check H2/H3 already had).
//!
//! ROUND8-L7-16 hoisted the byte-level predicate into `lb-core` so the H3/QUIC
//! datapath shares the EXACT implementation. Re-deriving it per protocol is
//! precisely the H1-vs-H2-vs-H3 divergence the HAProxy `BUG/MEDIUM` fix warns
//! about, so the predicate is re-exported verbatim here.

// ROUND8-L7-16: single source of truth lives in `lb-core`.
pub use lb_core::authority::{AuthorityError, validate};

/// ROUND8-L7-09 choke point — validate every authority value carried by an
/// inbound request, whatever downstream path it takes (plain, WebSocket
/// upgrade, H2 extended-CONNECT, gRPC).
///
/// Both the H1 and H2 dispatchers call this at the very top of `handle_inner`,
/// BEFORE the fork into the upgrade / CONNECT / gRPC handlers: hoisting it
/// above the fork means a fork added later inherits the check for free. Both
/// the URI authority and the `Host` header are validated when present.
///
/// An absent or empty value is NOT rejected here — PROTO-2-01 owns the
/// missing-authority gate; this only sanitises a present value.
///
/// There is deliberately NO loopback exemption: that carve-out belongs to the
/// SNI-vs-Host AGREEMENT check only. Applying it here would make the upgrade
/// path looser than the plain path — the exact divergence HAProxy's
/// `BUG/MEDIUM` fix was about.
///
/// # Errors
///
/// The first offending value together with its [`AuthorityError`].
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

    // The predicate's own unit tests live in `lb_core::authority`; these pin
    // the `http::Request` wrapper behaviour the QUIC path does not share.

    #[test]
    fn predicate_reexport_is_lb_core() {
        // Must be byte-identical to the shared predicate (same rejects, same
        // loopback policy: none).
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
        // Nothing to sanitise — PROTO-2-01's gate, not this predicate's.
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
