//! PROTO-2-07 — `StrippedRequest<B>` newtype: compile-time guarantee
//! that an `http::Request` has already had its hop-by-hop headers
//! removed (RFC 9110 §7.6.1 plus Connection-listed extras).
//!
//! Per lead-approved L-007: the hot-path bridge code in
//! [`crate::h1_proxy`] / [`crate::h2_proxy`] / [`crate::h2_to_h1`]
//! historically takes raw `http::Request<B>` arguments. A caller can
//! pass an un-stripped request by accident and the proxy will then
//! emit hop-by-hop headers across the H1↔H2/H3 boundary, defeating
//! RFC 9110 §7.6.1. Wrapping the request in a `#[repr(transparent)]`
//! newtype that can ONLY be constructed via [`strip_hop_by_hop`] makes
//! the invariant a type-system property: any function taking
//! `StrippedRequest<B>` is statically guaranteed that the strip ran
//! exactly once.
//!
//! This is an internal-API guarantee — the newtype is `pub` so
//! integration tests can pin the bridge surface, but the constructor
//! is `pub(crate)` so external callers cannot fabricate a
//! `StrippedRequest` without invoking the strip.
//!
//! `#[repr(transparent)]` keeps the layout identical to
//! `http::Request<B>` so the wrapper has zero runtime cost.

use http::Request;

/// A request whose hop-by-hop headers have been stripped per
/// RFC 9110 §7.6.1.
///
/// Construct only via [`strip_hop_by_hop`]. The hop-by-hop strip is
/// thereby tracked at the type level: any function taking
/// `&StrippedRequest<B>` or `StrippedRequest<B>` can rely on the
/// invariant without re-running the strip.
///
/// # Layout
///
/// `#[repr(transparent)]` guarantees the wrapper has the same memory
/// layout as the underlying `http::Request<B>`. Cost is zero.
#[repr(transparent)]
#[derive(Debug)]
pub struct StrippedRequest<B>(Request<B>);

impl<B> StrippedRequest<B> {
    /// Borrow the inner [`Request`] immutably.
    #[must_use]
    pub fn inner(&self) -> &Request<B> {
        &self.0
    }

    /// Mutable access to the inner header map. The hop-by-hop strip
    /// is a one-shot invariant: adding `X-Forwarded-*` / `Via` after
    /// the strip is fine (those are end-to-end). Mutating to
    /// re-introduce a hop-by-hop name is the caller's responsibility
    /// to avoid — the invariant only says "the strip ran", not "the
    /// header set is forever sealed".
    pub fn headers_mut(&mut self) -> &mut http::HeaderMap {
        self.0.headers_mut()
    }

    /// Consume the wrapper and yield the inner [`Request`]. Callers
    /// that need to mutate the request (e.g. append `X-Forwarded-*`,
    /// `Via`) unwrap here. The newtype only encodes the
    /// "hop-by-hop already stripped" invariant; it does not freeze
    /// the request shape.
    #[must_use]
    pub fn into_inner(self) -> Request<B> {
        self.0
    }

    /// Decompose into `(parts, body)` (sugar for
    /// `self.into_inner().into_parts()`).
    #[must_use]
    pub fn into_parts(self) -> (http::request::Parts, B) {
        self.0.into_parts()
    }
}

/// Run the RFC 9110 §7.6.1 hop-by-hop strip on `req`'s header map
/// exactly once and return the result wrapped in [`StrippedRequest`].
///
/// The strip removes the eight canonical hop-by-hop field names plus
/// every name listed inside the `Connection` header value. See
/// [`crate::h1_proxy::strip_hop_by_hop`] for the underlying
/// implementation.
///
/// Constructor is `pub(crate)` so only the in-crate hot path can mint
/// a `StrippedRequest`. External integration tests construct via the
/// `crate::h1_proxy::strip_hop_by_hop_into` shim instead.
pub(crate) fn strip_hop_by_hop<B>(mut req: Request<B>) -> StrippedRequest<B> {
    crate::h1_proxy::strip_hop_by_hop(req.headers_mut());
    StrippedRequest(req)
}

/// Test-only constructor surface so the PROTO-2-07 integration tests
/// can produce a `StrippedRequest` from outside the crate. The
/// production proxy hot path uses the `pub(crate)` factory above.
///
/// # Compile-time invariants
///
/// A raw `http::Request<B>` cannot be passed where
/// `&StrippedRequest<B>` is expected — the type system rejects the
/// mismatch:
///
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
/// And `StrippedRequest::<B>(req)` cannot be constructed by tuple
/// initialisation because the inner field is private:
///
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
        // Compile-time check: the wrapper has the same size as the inner.
        const _: () = {
            assert!(
                std::mem::size_of::<StrippedRequest<()>>() == std::mem::size_of::<Request<()>>(),
            );
        };
    }
}
