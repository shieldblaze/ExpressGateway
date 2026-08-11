//! PROTO-2-07 — `StrippedRequest<B>`: a compile-time guarantee that hop-by-hop
//! headers were removed (RFC 9110 §7.6.1 plus `Connection`-listed extras).
//! Constructible ONLY via [`strip_hop_by_hop`], so an un-stripped request
//! cannot reach the H1↔H2/H3 boundary by accident.

use http::Request;

/// A request whose hop-by-hop headers have been stripped (RFC 9110 §7.6.1).
/// Any function taking one may rely on that without re-running the strip.
#[repr(transparent)]
#[derive(Debug)]
pub struct StrippedRequest<B>(Request<B>);

impl<B> StrippedRequest<B> {
    /// Borrow the inner [`Request`] immutably.
    #[must_use]
    pub fn inner(&self) -> &Request<B> {
        &self.0
    }

    /// Mutable access to the inner header map. The invariant is "the strip
    /// ran", NOT "the header set is sealed" — re-introducing a hop-by-hop name
    /// here is the caller's responsibility to avoid.
    pub fn headers_mut(&mut self) -> &mut http::HeaderMap {
        self.0.headers_mut()
    }

    /// Consume the wrapper and yield the inner [`Request`].
    #[must_use]
    pub fn into_inner(self) -> Request<B> {
        self.0
    }

    /// Decompose into `(parts, body)`.
    #[must_use]
    pub fn into_parts(self) -> (http::request::Parts, B) {
        self.0.into_parts()
    }
}

/// Run the RFC 9110 §7.6.1 strip once and wrap the result. `pub(crate)` so only
/// the in-crate hot path can mint one; tests go through [`strip_for_test`].
pub(crate) fn strip_hop_by_hop<B>(mut req: Request<B>) -> StrippedRequest<B> {
    crate::h1_proxy::strip_hop_by_hop(req.headers_mut());
    StrippedRequest(req)
}

/// Test-only constructor so the PROTO-2-07 integration tests can produce a
/// `StrippedRequest` from outside the crate.
/// # Compile-time invariants
///
/// A raw `http::Request<B>` cannot stand in for `&StrippedRequest<B>`:
/// ```compile_fail
/// use http::Request;
/// use lb_l7::stripped_request::StrippedRequest;
///
/// fn takes_stripped<B>(_r: &StrippedRequest<B>) {}
/// let raw: Request<()> = Request::builder().uri("/").body(()).unwrap();
/// // ERROR: expected `&StrippedRequest<()>`, found `&Request<()>`.
/// takes_stripped(&raw);
/// ```
///
/// And the tuple struct cannot be initialised directly (private field):
/// ```compile_fail
/// use http::Request;
/// use lb_l7::stripped_request::StrippedRequest;
///
/// let raw: Request<()> = Request::builder().uri("/").body(()).unwrap();
/// // ERROR: cannot initialise tuple struct with private field.
/// let _s: StrippedRequest<()> = StrippedRequest(raw);
/// ```
#[doc(hidden)]
#[must_use]
pub fn strip_for_test<B>(req: Request<B>) -> StrippedRequest<B> {
    strip_hop_by_hop(req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderValue;
    use http::header::CONNECTION;

    #[test]
    fn strip_removes_hop_by_hop() {
        let req = Request::builder()
            .uri("/")
            .header(CONNECTION, "keep-alive")
            .header("keep-alive", "timeout=5")
            .header("x-keep", "v")
            .body(())
            .unwrap();
        let s = strip_hop_by_hop(req);
        assert!(s.inner().headers().get(CONNECTION).is_none());
        assert!(s.inner().headers().get("keep-alive").is_none());
        assert_eq!(
            s.inner().headers().get("x-keep"),
            Some(&HeaderValue::from_static("v"))
        );
    }

    #[test]
    fn into_inner_yields_request() {
        let req = Request::builder().uri("/x").body(()).unwrap();
        let s = strip_hop_by_hop(req);
        let r = s.into_inner();
        assert_eq!(r.uri().path(), "/x");
    }

    #[test]
    fn repr_transparent_zero_cost() {
        const _: () = {
            assert!(
                std::mem::size_of::<StrippedRequest<()>>() == std::mem::size_of::<Request<()>>(),
            );
        };
    }
}
