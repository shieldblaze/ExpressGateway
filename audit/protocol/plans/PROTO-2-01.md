# Plan for PROTO-2-01 — H2 reject when `:authority` ≠ `Host`

Finding-ref:    PROTO-2-01 (high, Open)
Files touched:
  - `crates/lb-l7/src/h2_proxy.rs` (request-validation hook)
  - `tests/h2_authority_host_mismatch.rs` (new)
  - `tests/h2_proxy_e2e.rs` (existing; add positive-path regression)

Approach:
RFC 9113 §8.3.1 forbids forwarding an HTTP/2 request where the `Host`
header disagrees with the `:authority` pseudo-header. Today
`h2_proxy.rs:320-330` reads `parts.uri.authority()` and falls back to
`Host` only when `:authority` is absent; the two are never compared.

The fix lives entirely inside `H2Proxy::proxy_request` (in
`crates/lb-l7/src/h2_proxy.rs`) at the **request-validation hook
immediately after hyper hands back `parts`** and *before*
`strip_hop_by_hop` runs. Concrete shape:

```rust
// crates/lb-l7/src/h2_proxy.rs, inside proxy_request, after let
// (parts, body) = req.into_parts();
fn validate_h2_authority(parts: &http::request::Parts)
    -> Result<(), http::StatusCode>
{
    let pseudo = parts.uri.authority().map(|a| a.as_str());
    let host_hdr = parts
        .headers
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok());
    match (pseudo, host_hdr) {
        (Some(p), Some(h)) if !authority_eq_host(p, h) =>
            Err(http::StatusCode::BAD_REQUEST),
        _ => Ok(()),
    }
}
```

`authority_eq_host` performs case-insensitive comparison of the host
portion; the port portion is compared only when both sides specify one
(per RFC 9110 §7.4 default-port rules). The helper lives in a new
`crates/lb-l7/src/util/authority.rs` module so PROTO-2-15 (SNI / Host
agreement) can re-use it.

On `Err`, `proxy_request` returns a `Response::builder().status(400)
.body(...)` *before* selecting the H2-to-H1 or H2-to-H2 bridge — i.e.
the malformed request never reaches `strip_hop_by_hop` and never
reaches the upstream connector.

Cross-team file-ownership: this edit lands inside proto's
`h2_proxy.rs` lane (synthesis §D). The new `util/authority.rs` is also
proto-owned. No other team needs to touch these files in Round 4.

Proof:
  - Test: `tests/h2_authority_host_mismatch.rs::rejects_authority_host_disagreement`
    Invariant: a hyper `h2 = "0.4"` client opens an H2 stream with
    `:authority = "a.test"` and explicit `host: b.test` header; the
    gateway responds `400` and the upstream connector is never invoked
    (assert via a counter on the test upstream).
  - Test: `tests/h2_authority_host_mismatch.rs::accepts_matching_authority_host`
    Invariant: `:authority = "a.test"` + `host: a.test` passes through
    with status `200`.
  - Test: `tests/h2_authority_host_mismatch.rs::accepts_authority_only`
    Invariant: `:authority` set, `Host` absent → pass through (hyper's
    common case).
  - Test: `tests/h2_authority_host_mismatch.rs::port_default_match`
    Invariant: `:authority = "a.test:443"` + `host: a.test` matches
    when listener is HTTPS (RFC 9110 §7.4 default-port).

Risk / blast radius:
  - Low. The check fires only on a path that today silently mis-routes;
    the new behaviour is RFC-mandated and a 400 response is the
    spec-prescribed signal.
  - Mild interop risk: legacy clients that send conflicting
    `:authority`/`Host` (the gRPC-go pre-1.42 bug is the only known
    case in the wild) will newly fail. Mitigation: emit a structured
    log at `warn` level with both values so operators can spot
    misbehaving clients; document the new behaviour in the release notes.
  - No perf cost: one `HeaderMap::get` + one `&str` compare per
    request, in a code path that already does similar work.

Cross-ref:    closes PROTO-2-01; provides `authority_eq_host` helper
              consumed by PROTO-2-15; complementary to SEC-2-01
              smuggling family.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
