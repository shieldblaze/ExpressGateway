//! ROUND8-L7-09 / L7-16 — the `http::Request` wrapper around
//! `lb_core::authority`.
//!
//! A `,` / whitespace / control byte in an authority value can desync upstream
//! routing or punch through a Host-based ACL (HAProxy `BUG/MAJOR: http: forbid
//! comma character in authority value`; `BUG/MEDIUM: h1: Enforce the authority
//! validation during H1 request parsing`).
//!
//! The predicate is re-exported from `lb-core` verbatim, never re-derived:
//! per-protocol copies are exactly the divergence that `BUG/MEDIUM` was about.

// ROUND8-L7-16: single source of truth lives in `lb-core`.
pub use lb_core::authority::{AuthorityError, validate};

/// ROUND8-L7-09 choke point — validate every authority value on an inbound
/// request, whatever downstream path it takes.
///
/// Called at the very TOP of both `handle_inner`s, BEFORE the upgrade /
/// CONNECT / gRPC fork, so a fork added later inherits the check for free. An
/// absent or empty value is NOT rejected here — PROTO-2-01 owns that gate.
///
/// There is deliberately NO loopback exemption: that carve-out belongs to the
/// SNI-vs-Host AGREEMENT check only, and applying it here would make the
/// upgrade path looser than the plain path.
///
/// # Errors
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

    // The predicate's own tests live in `lb_core::authority`; these pin the
    // `http::Request` wrapper the QUIC path does not share.

    #[test]
    fn predicate_reexport_is_lb_core() {
        // Must match the shared predicate exactly (loopback policy: none).
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
