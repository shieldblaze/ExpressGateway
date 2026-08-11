//! PROTO-2-07 — `StrippedRequest<B>` newtype: a compile-time guarantee that an
//! `http::Request` has already had its hop-by-hop headers removed
//! (RFC 9110 §7.6.1 plus `Connection`-listed extras).
//!
//! The hot-path bridge code historically took raw `http::Request<B>`, so a
//! caller could pass an un-stripped request by accident and the proxy would
//! then emit hop-by-hop headers across the H1↔H2/H3 boundary. A
//! `#[repr(transparent)]` newtype constructible ONLY via [`strip_hop_by_hop`]
//! makes the invariant a type-system property at zero runtime cost. The type is
//! `pub` so integration tests can pin the bridge surface, but the constructor
//! is `pub(crate)` so external callers cannot fabricate one.

use http::Request;

/// A request whose hop-by-hop headers have been stripped per RFC 9110 §7.6.1.
/// Construct only via [`strip_hop_by_hop`]; any function taking one can rely on
/// the invariant without re-running the strip. `#[repr(transparent)]`, so the
/// wrapper costs nothing at runtime.
#[repr(transparent)]
#[derive(Debug)]
pub struct StrippedRequest<B>(Request<B>);

impl<B> StrippedRequest<B> {
    /// Borrow the inner [`Request`] immutably.
    #[must_use]
    pub fn inner(&self) -> &Request<B> {
        &self.0
    }

    /// Mutable access to the inner header map. The strip is a ONE-SHOT
    /// invariant: adding `X-Forwarded-*` / `Via` afterwards is fine (they are
    /// end-to-end), but re-introducing a hop-by-hop name is the caller's
    /// responsibility to avoid — the invariant says "the strip ran", not "the
    /// header set is sealed".
    pub fn headers_mut(&mut self) -> &mut http::HeaderMap {
        self.0.headers_mut()
    }

    /// Consume the wrapper and yield the inner [`Request`]. The newtype encodes
    /// only "hop-by-hop already stripped"; it does not freeze the shape.
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

/// Run the RFC 9110 §7.6.1 hop-by-hop strip exactly once and wrap the result:
/// the eight canonical field names plus every name listed inside `Connection`.
/// `pub(crate)` so only the in-crate hot path can mint a [`StrippedRequest`];
/// integration tests go through [`strip_for_test`].
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
        // Compile-time check: the wrapper has the same size as the inner.
        const _: () = {
            assert!(
                std::mem::size_of::<StrippedRequest<()>>() == std::mem::size_of::<Request<()>>(),
            );
        };
    }
}
