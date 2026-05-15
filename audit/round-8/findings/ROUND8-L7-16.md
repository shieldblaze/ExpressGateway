### ROUND8-L7-16 — H3/QUIC ingress dispatch skips authority value sanitisation (separate unguarded parser; HAProxy `BUG/MAJOR: http: forbid comma in authority` class, H3 leg of ROUND8-L7-09)

Origin: opened by `verify` during the ROUND8-L7-09 re-re-verify (task#76, sha 1a89a4e4, 2026-05-15). L7-09 was VERIFIED-FIXED for its H1/H2 scope (single choke point at the top of `H1Proxy::handle_inner` / `H2Proxy::handle_inner`). The adversarial 4th-path hunt found that the H3/QUIC request path is a SEPARATE dispatch in a different crate that never reaches that choke point and performs no authority sanitisation — this is the H3 leg the L7-09 finding's own Recommendation step 2 explicitly called for ("H3: in the QUIC conn_actor's header-callback path") but the L7-09 fix did not deliver.

Reference: `audit/round-8/research/haproxy.md` lessons 9 + 10 — `BUG/MAJOR: http: forbid comma character in authority value` + `BUG/MEDIUM: h1: Enforce the authority validation` (protocol-neutral validation requires the validation to actually run on *every* parser, including H3).

Severity: medium
Status:   Open (verify, task#76, 2026-05-15) — NON-BLOCKING for prod (future-routing primitive; no host-based routing today, same risk class as L7-09's pre-fix state) but MUST be closed before any vhost / host-based-routing pillar lands.

Divergence:
- **Reference / L7-09**: every protocol parser (H1, H2, **H3**) must reject `:authority` / `Host` values containing comma, whitespace, control characters. Same predicate, applied uniformly.
- **Us (H1/H2)**: closed by ROUND8-L7-09 sha 1a89a4e4 — `crate::authority::validate_request` is the first statement of both `handle_inner` dispatchers.
- **Us (H3/QUIC)**: NOT closed. `lb-quic` is a distinct crate that does **not** depend on `lb-l7` (confirmed `crates/lb-quic/Cargo.toml` has no `lb-l7`/authority dep). `crates/lb-quic/src/conn_actor.rs:361` builds `H3Request::from_headers(headers)` and proceeds directly to upstream selection — `h2_backend` (line 364, `h3_to_h2_roundtrip`), `h3_backend` (line 374, `h3_to_h3_roundtrip`), or `select_backend(backends)` (line 386, `h3_to_h1_roundtrip`) — with no authority validation on any branch. The `:authority` pseudo-header is parsed at `crates/lb-quic/src/h3_bridge.rs:121-131` into `H3Request.authority` and forwarded verbatim into `build_h1_request` (`h3_bridge.rs:139-147`) and the H2/H3 upstream request builders. `grep -rn 'authority::validate|validate_request|forbid comma|BUG/MAJOR|comma'` across `crates/lb-quic/src/` and `crates/lb-h3/src/` returns ZERO matches.

Impact:
- Identical to L7-09 pre-fix: an H3 request with `:authority: victim.example,attacker.example` (or whitespace / control bytes) reaches upstream selection and is forwarded to the backend unsanitised. Not directly exploitable today (no host-based routing; the picker does not key on authority), but it is a smuggling / routing-ACL-desync primitive the moment vhost or host-based routing lands — exactly the "lesson-not-yet-paid-for" framing of L7-09, now confirmed still open for the H3 protocol family.
- Asymmetry risk: H1/H2 now reject these values (400) while H3 silently forwards them. A multi-protocol deployment that trusts the proxy to have sanitised authority uniformly (the explicit L7-09 invariant) is wrong for its H3 listeners.

Reproduction (manual):
- Send an H3 request with `:authority` = `example.test,attacker.example` to a QUIC listener. `H3Request::from_headers` stores it unchanged; `conn_actor.rs:361`→`select_backend`/`h3_to_*_roundtrip` forwards it. No 400. Contrast: the equivalent H1/H2 request returns 400 post-L7-09 (`crates/lb-l7/tests/round8_authority_enforced.rs`).

Recommendation:
1. Give `lb-quic` access to the shared predicate. Either add a `lb-l7` (authority module) dependency, or hoist `lb_l7::authority::{validate, AuthorityError}` into a lower crate (e.g. `lb-core`) so both `lb-l7` and `lb-quic` share ONE implementation (avoids the exact H1-vs-H2-vs-H3 divergence L7-09 / HAProxy `BUG/MEDIUM` warns about — do not re-implement).
2. Call it at the H3 ingress choke point: in `crates/lb-quic/src/conn_actor.rs` immediately after `H3Request::from_headers(headers)` at line 361 and **before** any of the three upstream branches (h2_backend / h3_backend / select_backend). Reject the stream (H3 `:status 400` / appropriate error response) on failure; do not dial upstream.
3. Validate the `:authority` value (and `Host` if an H3 client sends one) with the same non-empty-present-only semantics as `validate_request` (missing authority is PROTO-2-01's gate, not this predicate's).
4. Regression test mirroring `round8_authority_enforced.rs`: drive a real H3 request with comma / whitespace / control-byte `:authority` through the QUIC conn_actor and assert a real accept-counting probe backend records ZERO connections (the same proof shape the L7-09 H1/H2 tests use).
