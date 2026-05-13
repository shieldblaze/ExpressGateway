# Plan for PROTO-2-07 — `StrippedRequest` newtype for Bridge hop-by-hop hygiene

Finding-ref:    PROTO-2-07 (low, Open — lead synthesis §E.1 approved
                option (c), the `StrippedRequest` newtype)
Files touched:
  - `crates/lb-l7/src/util/hop.rs` (new module — newtype +
    constructor + `strip_hop_by_hop` helper relocation)
  - `crates/lb-l7/src/lib.rs` (re-export pattern; private-module
    boundary)
  - `crates/lb-l7/src/h1_proxy.rs` (the existing
    `strip_hop_by_hop` function moves to `util/hop.rs` and becomes
    `pub(crate)`; the call site changes to consume the newtype)
  - `crates/lb-l7/src/h2_proxy.rs` (call sites at lines 332, 512,
    902 per round-2 review evidence)
  - `crates/lb-l7/src/h1_to_h2.rs`
  - `crates/lb-l7/src/h2_to_h1.rs`
  - `crates/lb-l7/src/h1_to_h3.rs`
  - `crates/lb-l7/src/h3_to_h1.rs`
  - `crates/lb-l7/src/h2_to_h2.rs`
  - `crates/lb-l7/src/h3_to_h3.rs`
  - `crates/lb-l7/src/h2_to_h3.rs` (if present; create if not)
  - `crates/lb-l7/src/h3_to_h2.rs` (if present; create if not)
  - `tests/bridge_stripped_request_compile_time.rs` (new — compile-
    fail test asserting bypass is impossible)

Approach:
Authoring credit: `code` proposed the newtype shape (`code`'s
CODE-2-07 cross-review §C); proto endorsed in cross-review §F.1;
lead synthesis §E.1 approved option (c). This plan canonicalises
the shape lead approved.

**The newtype.**

```rust
// crates/lb-l7/src/util/hop.rs

/// Hop-by-hop header names per RFC 9110 §7.6.1.
pub(crate) const HOP_BY_HOP: &[http::HeaderName] = &[
    http::header::CONNECTION,
    http::header::HeaderName::from_static("keep-alive"),
    http::header::PROXY_AUTHENTICATE,
    http::header::PROXY_AUTHORIZATION,
    http::header::TE,
    http::header::TRAILER,           // (cleaned per PROTO-2-08 batch)
    http::header::TRANSFER_ENCODING,
    http::header::UPGRADE,
];

/// A request whose hop-by-hop headers and `Connection`-listed names
/// have been stripped. Constructed only by `StrippedRequest::from_helper`
/// (or `for_test`), which lives in this module — no other code path
/// can produce one.
#[repr(transparent)]
pub struct StrippedRequest<B>(http::Request<B>);

impl<B> StrippedRequest<B> {
    pub(crate) fn from_request(mut req: http::Request<B>) -> Self {
        strip_hop_by_hop(req.headers_mut());
        Self(req)
    }

    pub fn into_parts(self) -> (http::request::Parts, B) {
        self.0.into_parts()
    }
    pub fn headers(&self) -> &http::HeaderMap { self.0.headers() }
    pub fn method(&self) -> &http::Method { self.0.method() }
    pub fn uri(&self) -> &http::Uri { self.0.uri() }
    pub fn version(&self) -> http::Version { self.0.version() }
    pub fn extensions(&self) -> &http::Extensions { self.0.extensions() }

    #[cfg(test)]
    pub fn for_test(req: http::Request<B>) -> Self {
        Self::from_request(req)
    }
}

fn strip_hop_by_hop(headers: &mut http::HeaderMap) {
    // 1) Collect names listed in Connection per RFC 9110 §7.6.1
    let connection_listed: Vec<http::HeaderName> = headers
        .get_all(http::header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|s| s.trim())
        .filter_map(|s| http::HeaderName::try_from(s).ok())
        .collect();

    // 2) Remove the static hop-by-hop set
    for name in HOP_BY_HOP {
        headers.remove(name);
    }

    // 3) Remove the Connection-listed names
    for name in connection_listed {
        headers.remove(&name);
    }
}
```

**Bridge trait signature update.**

```rust
// crates/lb-l7/src/lib.rs
pub trait Bridge {
    type RespBody;
    async fn proxy<B>(
        &self,
        req: StrippedRequest<B>,
        upstream: &Upstream,
    ) -> Result<BridgeResponse<Self::RespBody>, BridgeError>
    where
        B: http_body::Body + Send;
}
```

The signature requires `StrippedRequest`. The only public way to
construct a `StrippedRequest` is `from_request`, which is
`pub(crate)`. External code cannot construct one. The strip
happens exactly once, at the boundary inside `H{1,2,3}Proxy::
proxy_request` where the helper builds the `StrippedRequest` from
the hyper-parsed `Request`.

**Migration plan (Round 4, mechanical):**
  1. Move `strip_hop_by_hop` out of `h1_proxy.rs` into
     `util/hop.rs`; delete the old `HOP_BY_HOP` constant in
     `h1_proxy.rs` (PROTO-2-08 batch removes the spurious
     `trailers` entry in the same edit).
  2. Update `H1Proxy::proxy_request` and `H2Proxy::proxy_request`
     to wrap the parsed `Request` in `StrippedRequest::from_request`
     once, immediately after `parse_request`. The existing in-
     proxy `strip_hop_by_hop` calls (lines 332, 512, 902 of
     `h2_proxy.rs`) are deleted — the strip now happens at the
     newtype constructor.
  3. Update every `Bridge` impl to consume `StrippedRequest`. The
     `into_parts` accessor recovers the inner `Parts + B` for
     downstream serialisation.
  4. Add `tests/bridge_stripped_request_compile_time.rs` with a
     `trybuild` compile-fail case demonstrating that an external
     caller cannot construct `StrippedRequest`.

**`StrippedTrailers` companion (PROTO-2-12 cross-cut):** the same
pattern applies to trailers; the `StrippedTrailers` newtype lives
in the same module, constructed via `from_trailers` that runs the
trailer-side strip (RFC 9110 §6.6.1 forbids names; PROTO-2-12 plan
defines the list).

Proof:
  - Test: `tests/bridge_stripped_request_compile_time.rs::external_construction_fails`
    Invariant: `trybuild` UI test attempts to construct
    `lb_l7::util::hop::StrippedRequest::from_request(req)` from
    outside the crate; compilation fails with "method not in scope"
    or "function is private".
  - Test: `tests/bridge_stripped_request_compile_time.rs::bridge_signature_requires_stripped`
    Invariant: `trybuild` UI test attempts to pass a raw
    `http::Request` to a bridge; compilation fails with "expected
    StrippedRequest, found Request".
  - Test (runtime): `tests/bridge_hop_by_hop_stripped.rs::connection_keep_alive_stripped`
    Invariant: H2→H2 bridge with input headers `connection:
    keep-alive, foo`, `foo: secret`; assert outbound request has
    neither `connection` nor `foo`.
  - Test: `tests/bridge_hop_by_hop_stripped.rs::transfer_encoding_stripped`
    Invariant: input `transfer-encoding: chunked`; outbound has none.
  - Test: `tests/bridge_hop_by_hop_stripped.rs::keep_alive_stripped`
    Invariant: input `keep-alive: timeout=5`; outbound has none.

Risk / blast radius:
  - **Type-system change touches every bridge.** Mechanical
    migration; no runtime behaviour change on the production path
    (the existing `strip_hop_by_hop` calls did the same strip; the
    newtype just moves the responsibility one layer earlier and
    type-enforces it).
  - Existing `tests/h2_proxy_e2e.rs` etc. construct requests
    directly to feed the bridge in unit tests; they migrate to
    `StrippedRequest::for_test`.
  - Zero perf cost (the strip happens once per request as it did
    before; the newtype is `#[repr(transparent)]`).

Cross-ref:    closes PROTO-2-07; companion to PROTO-2-12
              (`StrippedTrailers`); composes with PROTO-2-08
              (the spurious `trailers` entry is removed when
              `HOP_BY_HOP` moves into the new module).
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
