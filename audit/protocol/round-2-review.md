# Round 2 Findings — `proto`

Owner: `proto` (HTTP/1.1, HTTP/2, HTTP/3, QUIC, TLS, WS, gRPC conformance).
Scope: findings only. No source changes. All file:line references are to
`/home/ubuntu/Code/ExpressGateway` at branch `main`, commit `ac58f61`.

Cross-team context:
- Lead synthesis: `audit/CROSS-REVIEW-SYNTHESIS-r1.md` (T1, T2, T8).
- Inventory: `audit/protocol/round-1-inventory.md` §1–§9.

Severity scale (per audit conventions): `critical | high | medium | low | info`.
Status is `Open` for every finding in this round. Remediation
plans land in Round 3.

---

### PROTO-2-01 — H2 listener silently prefers `:authority` when it disagrees with `Host`
Severity: high
Status:   Proposed-Fix(Wave-2b-2: new `H2Proxy::handle` guard `check_authority_host_agreement` runs after the SmuggleDetector and before hop-by-hop strip; mismatching `:authority` vs `Host` (case-insensitive host compare, default-port latitude per §8.3.1, IPv6-bracket aware) returns `400 Bad Request: :authority disagrees with Host (RFC 9113 §8.3.1)`. Also belt-and-braces guard inside `H2ToH1Bridge::bridge_request` so direct bridge consumers can't bypass. Proof: `crates/lb-l7/tests/h2_authority_host_mismatch.rs` (11 tests including `test_h2_400_on_disagreement`) and two in-module tests in `h2_to_h1.rs`.)
Location: `crates/lb-l7/src/h2_proxy.rs:320-330` (and the `Host`-synthesis path immediately below at lines 337-344).
Description: RFC 9113 §8.3.1 ("Connection-Specific Header Fields" /
":authority Pseudo-Header") states: *"An intermediary that forwards a
request over HTTP/2 MUST construct an `:authority` pseudo-header field
[...]. If the `:authority` pseudo-header field is present, an
intermediary that translates from HTTP/2 to HTTP/1.1 MUST construct a
Host header field [...]. The recipient of an HTTP/2 request MUST
NOT generate a Host header field that differs from the `:authority`."*
The companion §8.3 sentence (which applies to a request constructed
from HTTP/2 received by an *origin or intermediary*) requires that
when both `:authority` and `Host` are present and the two
disagree, the request be treated as malformed. The current code reads:

```rust
let authority = parts
    .uri
    .authority()
    .map(|a| a.as_str().to_owned())
    .or_else(|| { parts.headers.get(hyper::header::HOST) ... });
```

i.e. `:authority` wins unconditionally; `Host` is consulted **only when
:authority is absent**. The two are never compared. A malicious H2
client can therefore set `:authority: victim.example` and
`Host: attacker.example` and the gateway will route on `:authority`
while leaving `Host` untouched in the outbound request when the
backend is H1 (a separate insert at line 339 only fires when Host is
absent). This is a smuggling primitive against any downstream
authorisation that keys on `Host`.
Impact: routing/authz desync; potential host-confusion smuggling
against backends that authorise on `Host`. Cross-cuts the `sec`
smuggling family.
Reproduction: send H2 request with `:authority: a.test`,
`Host: b.test`; observe upstream sees `Host: b.test` (when backend is
H1) and the routing key is `a.test`.
Recommendation: reject mismatching `Host`/`:authority` with 400 Bad
Request before any header normalisation. Implement as a
guard inside `H2Proxy::proxy_request` that runs before
`strip_hop_by_hop`. Add a unit test under `tests/h2_proxy_e2e.rs`.
Cross-ref: lead T8; `sec` smuggling cluster (S-1 / S-3 derivatives);
`code` decides whether to factor a shared `validate_authority` helper.

---

### PROTO-2-02 — `LB_QUIC_ALPN = b"lb-quic"` advertised on every QUIC config; no production H3 listener sets `b"h3"`
Severity: critical
Status:   Proposed-Fix(c941b28 + lb-io follow-through)
Follow-through: `crates/lb-io/src/quic_pool.rs` now installs
`UPSTREAM_H3_ALPN_PROTOS = &[b"h3", b"h3-29"]` on the upstream pool's
dialer-config factory; smoke test `quic_pool::tests::test_pool_dialer_uses_h3`
locks the invariant. The constant is duplicated rather than imported from
`lb_quic::H3_ALPN_PROTOS` because `lb-io` does not depend on `lb-quic`
(adding the edge is a Wave-2c task).
Location: `crates/lb-quic/src/lib.rs:115` (constant), `crates/lb-quic/src/lib.rs:392`
(`cfg.set_application_protos(&[LB_QUIC_ALPN])` inside `build_config`,
the *only* `set_application_protos` call in the tree).
Description: RFC 9114 §3.1 mandates the ALPN identifier `h3` for
HTTP/3. `build_config` is the single QUIC `Config` factory used by
**both** the listener (`Role::Server`) and the upstream
(`Role::Client`) — see the `match role` block at line 405+. There is
no override path that substitutes `b"h3"` for production listeners.
Consequence: no real H3 client will negotiate; the listener can only
talk to the in-tree test rig which sets the same `b"lb-quic"` token.
The whole H3 listener / H3 upstream pool is effectively
non-functional against real-world peers (browsers, curl --http3,
quiche-client with default ALPN). The QPACK / h3_bridge code paths
that the inventory enumerated never execute against external traffic.
Impact: H3 is advertised in docs and the README but does not interop;
any Alt-Svc:`h3=":443"` advertisement at `h1_proxy.rs` will cause
clients to fail-over to HTTPS silently. Privacy/observability gap
plus a complete absence of production H3.
Reproduction: `curl --http3-only -k https://gateway:8443/` against a
QUIC listener; TLS handshake fails with `no_application_protocol`
alert from BoringSSL inside quiche.
Recommendation:
  1. Change `LB_QUIC_ALPN` to `b"h3"` (or remove the constant and
     inline `b"h3"` at the server site) for the real H3 listener.
  2. Keep `b"lb-quic"` only for the loopback transport-only test rig
     and document it as such, behind a `#[cfg(test)]` constant or
     an explicit `protocols: &[&[u8]]` parameter on `build_config`.
  3. Add a `tests/quic_alpn_h3.rs` that asserts the server's ALPN
     advertisement contains exactly `b"h3"` for production
     `Role::Server`.
Cross-ref: lead T8-#2; `rel` advertise-vs-reality gap.

---

### PROTO-2-03 — No 1xx / 100-Continue / 103 Early Hints policy or forwarding test
Status:   Baseline-Pinned-Wave-2b-2 / Fix-Deferred-Wave-2c (investigation: hyper auto-handles `Expect: 100-continue` at the wire level transparently, but `client::conn::http1::send_request().await` resolves on the first non-1xx response so 103 Early Hints frames drop. `crates/lb-l7/tests/informational_responses.rs` (5 tests: test_100_continue_forwarded, test_103_early_hints_forwarded, test_1xx_from_upstream_passes_through_h1, test_h2_informational + the hyper-internal baseline) pin the status-class invariants. Wave-2c installs `OnInformational` callback; see `audit/deferred.md` "PROTO-2-03".)
Severity: medium
Status:   Open
Location: absence — `grep -rn '100[- ]?continue\|EarlyHints\|status::CONTINUE\|StatusCode::CONTINUE\|103' crates/lb-l7 crates/lb` returns zero hits in any proxy module.
Description: RFC 9110 §15.2 ("Informational 1xx") requires a proxy
that receives a 1xx response from upstream to forward it to the
client unless the proxy itself processed the corresponding request
end-to-end. RFC 9113 §8.1 spells out the HTTP/2 representation of
informational responses (HEADERS frame with `:status` in the 1xx
range, never preceded by an END_STREAM). Today the L7 proxies do not
inspect informational responses at all — the hyper server *may* pass
100 transparently for H1 within a single connection, but:
  * The proxy bridge code (`h1_to_h2`, `h1_to_h3`, `h2_to_h1`,
    `h3_to_h1`) materialises responses as `BridgeResponse` which
    has a single `status` field; intermediate 1xx frames cannot be
    represented and are silently dropped.
  * `Expect: 100-continue` semantics for a proxy require holding the
    request body until the upstream sends 100 (or rewriting the
    Expect on hop boundaries). No such gate exists.
  * 103 Early Hints — the entire performance benefit of preloading
    sub-resources — is never forwarded.
Impact: clients that depend on 100-Continue for large uploads
silently hang or send body before authorisation; 103 Early Hints lose
their value across the gateway; HTTP/2 informational frames are
swallowed.
Reproduction: send `PUT /large` with `Expect: 100-continue` from a
client to gateway → upstream returns 100 → client never sees it (or
hyper times out the upload).
Recommendation:
  1. Define a written policy: gateway forwards every 1xx from
     upstream to client verbatim (with hop-by-hop strip).
  2. Wire it into the L7 bridge response path: change
     `BridgeResponse` to carry an `informationals: Vec<...>` or
     stream them as a tokio channel before the final response.
  3. Add `tests/h1_continue_pass_through.rs`,
     `tests/h2_continue_pass_through.rs`, and
     `tests/early_hints_pass_through.rs`.
Cross-ref: `sec` for the Expect-as-DoS surface (request held until
upstream responds — can amplify slow-loris).

---

### PROTO-2-04 — `tests/ws_autobahn.rs` is a `--help` stub; no Autobahn fuzzingclient run, even when `wstest` is installed
Severity: medium
Status:   Open
Location: `tests/ws_autobahn.rs:24-34` (the eprintln-only branches),
plus the test's `which wstest` probe at lines ~15-20.
Description: The test name implies an Autobahn fuzzingclient
conformance run. In reality, even when `wstest` is on PATH, the test
*prints a TODO message and returns OK*. There are no spec cases
executed, no report parsed, no failures asserted. RFC 6455
correctness is therefore not measured anywhere in CI.
Impact: WebSocket conformance (close-handshake codes, fragmentation,
UTF-8 validation, ping/pong, control-frame limits) is unverified.
Regressions on any of the ~520 Autobahn cases ship undetected.
Reproduction: `which wstest && cargo test -p lb-tests ws_autobahn`
prints "wstest detected; full Autobahn run is a Phase F follow-up"
and exits 0.
Recommendation:
  1. Replace the stub with a real harness: spawn the gateway with
     a WebSocket-echo backend, invoke
     `wstest -m fuzzingclient -s autobahn-spec.json`, parse the
     resulting JSON index, and fail the test on any non-OK case
     (with a documented allowlist for known-broken cases).
  2. Add an `autobahn-spec.json` fixture to `tests/fixtures/`.
  3. Pin `wstest` (autobahntestsuite) in the CI image (defer to
     `rel` for image construction; this finding owns the test
     harness gap).
Cross-ref: `rel` CI-image inventory.

---

### PROTO-2-05 — No `h3spec` (or equivalent) integration harness
Severity: medium
Status:   Open
Location: absence — `tests/h3spec.rs` does not exist (`ls tests/`
confirms); `crates/lb-h3` ships only codec round-trip unit tests.
Description: HTTP/3 conformance has no automated assertion against
the gateway's wire behaviour. The `h2spec.rs` test runs `h2spec`
against the H2 listener (when installed); there is no analogue for
H3. The two viable harnesses today are:
  * `h3i` (https://github.com/cloudflare/quiche/tree/master/h3i) —
    Cloudflare-maintained, lives in the same `quiche` workspace
    already in dependencies; scriptable adversarial frames.
  * `quic-tracker` (https://github.com/QUIC-Tracker/quic-tracker) —
    older, narrower coverage but pure Go and self-contained.
Impact: HTTP/3 frame-level conformance (HEADERS oversize, GREASE
frame handling, MAX_PUSH_ID enforcement, settings echo, QPACK
encoder-stream blocking, etc.) and QUIC transport conformance (CIDs,
NEW_TOKEN replay) are unmeasured.
Reproduction: n/a (absence).
Recommendation:
  1. Pick `h3i` (lower-friction since `quiche` is already in deps).
  2. Add `tests/h3spec.rs` that spawns the H3 listener (with
     `b"h3"` ALPN — see PROTO-2-02), invokes `h3i` against it
     with a documented case-set, and asserts zero failures.
  3. Document the chosen case-set in
     `docs/conformance/h3i-cases.md` for future hardening.
Cross-ref: PROTO-2-02 (ALPN); `rel` CI-image inventory.

---

### PROTO-2-06 — `tests/conformance_h{1,2,3}.rs` are codec round-trip unit tests, not server-conformance tests
Severity: low
Status:   Open
Location: `tests/conformance_h1.rs`, `tests/conformance_h2.rs`,
`tests/conformance_h3.rs` (full files; each `<200` lines, all
exercise only the in-tree `lb-h{1,2,3}` codecs).
Description: The filenames promise protocol conformance against the
gateway. They actually exercise the in-tree codec crates
(`lb-h1::parse_*`, `lb-h2::frame::{Frame,Settings,...}`,
`lb-h3::frame::Frame`) by encoding then decoding a value and
asserting equality. The gateway server is never spawned. This
mislabels CI signal: developers reading `cargo test conformance`
green will assume RFC compliance is checked.
Impact: false sense of conformance coverage. Real server-behavioural
tests live in `h2spec.rs` (which silently skips without `h2spec`)
and in the per-protocol e2e tests.
Reproduction: open any of the three files; observe `assert_eq!`
against the in-tree codec only.
Recommendation: pick one:
  * (a) Rename to `tests/codec_roundtrip_h{1,2,3}.rs` and add a top
        comment clarifying the scope.
  * (b) Replace with real server-conformance harnesses: `h2spec`
        for H2, `h3i` for H3 (see PROTO-2-05), and a hand-written
        H1 case list (the existing `tests/h1_proxy_e2e.rs` covers
        most of this; extract the conformance subset).
Preferred: (a) immediately to stop misleading the dashboard;
(b) lands together with PROTO-2-05.
Cross-ref: PROTO-2-05.

---

### PROTO-2-07 — `H2ToH2Bridge` / `H3ToH3Bridge` trait impls do not strip hop-by-hop at the trait level
Severity: low
Status:   Proposed-Fix(Wave-2b-2: option (b)/(c) hybrid — new `crates/lb-l7/src/stripped_request.rs::StrippedRequest<B>` `#[repr(transparent)]` newtype encodes "hop-by-hop already stripped" as a type-system invariant. Proxy fan-out (`H1Proxy::proxy_request`/`proxy_h1_to_h{2,3}`, `H2Proxy::proxy_request`/`proxy_h2_to_h{2,3}`) now consumes `StrippedRequest<IncomingBody>` so the strip is checked at compile time on every internal call site. Constructor is `pub(crate)`; the `#[doc(hidden)] strip_for_test` surface plus `compile_fail` doctests prove the type-system guard. Proof: `crates/lb-l7/tests/stripped_request_newtype.rs` (5 tests). Bridge trait surface itself unchanged — the type-system fence sits one layer up at the proxy hot-path call site, which is where un-stripped requests would otherwise leak.)
Location:
  * `crates/lb-l7/src/h2_to_h2.rs` (entire file — no `HOP_BY_HOP` filter; pseudo-headers and headers pass through verbatim).
  * `crates/lb-l7/src/h3_to_h3.rs` (entire file — same shape).
  * Compare with `crates/lb-l7/src/h1_to_h2.rs:9-18` and
    `crates/lb-l7/src/h2_to_h1.rs:10-19` which both define
    `HOP_BY_HOP_HEADERS` and apply it.
  * Runtime mitigation: `crates/lb-l7/src/h1_proxy.rs::strip_hop_by_hop`
    is invoked at `h2_proxy.rs:332, 512, 902` before the bridge
    runs, which covers the live data path.
Description: RFC 9113 §8.2.2 (and RFC 9114 §4.2 by reference)
forbid the hop-by-hop names `connection`, `keep-alive`,
`proxy-connection`, `transfer-encoding`, `upgrade`, plus any name
listed in `Connection`, from H2/H3 messages entirely. A bridge whose
job is "translate an H2 request to an H2 request" must enforce this
on output even if the input has been laundered upstream — because
the bridge is a trait callers can invoke directly. The runtime
proxy currently calls `strip_hop_by_hop` *before* the bridge
(belt-and-braces), so the live data path is safe today. The blast
radius is "Bridge trait is unsafe to call directly without the
runtime preamble".
Impact: low today; medium if the Bridge trait is ever re-exported
or used by external consumers (e.g. an embedded test harness, an
admin-plane endpoint, or a future filter chain that runs the
bridge in isolation).
Reproduction: write a unit test that constructs an `H2ToH2Bridge`
directly and feeds it a request with `connection: keep-alive,
foo` plus a `foo: secret` header. Observe both pass through.
Recommendation: Round-3 decision required. **Lead pre-approved the
defer-the-choice disposition.** Two options:
  * (a) **Trait-level fix.** Pull `strip_hop_by_hop` (or a shared
        helper in `lb-l7/src/util/hop.rs`) into every `Bridge`
        impl. Belt-and-braces becomes belt-only. Pro: defence in
        depth; safe for external callers. Con: extra allocation /
        iteration on the hot path.
  * (b) **Documented precondition.** Add a `// SAFETY:` doc block
        to the `Bridge` trait stating that callers MUST strip
        hop-by-hop before invocation; mark the in-tree bridges
        `pub(crate)`. Pro: zero perf cost. Con: relies on caller
        discipline.
Author of this finding: `proto`. Co-reviewer: `code` (decides perf
vs. defence-in-depth trade-off in Round 3 plan).
Cross-ref: lead T8-#7; `code` Q-CODE-2 (Round-3 plan).

---

### PROTO-2-08 — `HOP_BY_HOP` in `h1_proxy.rs` lists `trailers` which is not a real header name
Severity: low
Status:   Proposed-Fix(Wave-2b-2: removed `"trailers"`, added `"keep-alive"`, added `"proxy-connection"`; new `tests/hop_by_hop_set.rs` locks the exact RFC 9110 §7.6.1 set; existing internal test renamed to assert end-to-end `Trailer` is preserved)
Location: `crates/lb-l7/src/h1_proxy.rs:54-63`. Specifically line 60:
`HeaderName::from_static("trailers"),`.
Description: There is no `Trailers` (plural) header in any HTTP
RFC. The "trailers" token does exist, but only as a value inside
the `TE:` request header (RFC 9110 §10.1.4 — `TE: trailers`).
The *response* announce-header is `Trailer` (singular). Putting
`trailers` in the `HOP_BY_HOP` removal list is a no-op (no real
client sends a header named `Trailers`), so this is cosmetic, not
a security bug. The *real* hop-by-hop hygiene gap, if any, would
be: `Trailer:` (singular) is not in the list — but `Trailer` is
not hop-by-hop per RFC 9110 §6.6.1; it's an end-to-end announce
header, so it should pass through. The current list is therefore
*correct except for the spurious `trailers` entry*.
Impact: zero runtime; reader-confusion only.
Reproduction: `grep -n trailers crates/lb-l7/src/h1_proxy.rs`.
Recommendation: delete line 60. Optional: add a comment on the
`HOP_BY_HOP` array stating "header names per RFC 9110 §7.6.1"
and listing the canonical eight (`Connection`, `Keep-Alive`,
`Proxy-Authenticate`, `Proxy-Authorization`, `TE`,
`Transfer-Encoding`, `Upgrade`, plus any name listed in the
`Connection` field).
Cross-ref: none.

---

### PROTO-2-09 — `ListenerMode::build_listener_mode` silently falls through to `PlainTcp` for unknown `protocol = …` values
Severity: medium
Status:   Deferred-to-Wave-2c (build_listener_mode is in main.rs:837)
Location: `crates/lb/src/main.rs:837` (`_ => Ok(ListenerMode::PlainTcp),`).
The branch is the final arm of the `match listener_cfg.protocol.as_str()`
construct that starts at the function definition above line 700.
Description: A typo in TOML — `protocol = "h2"` instead of `"h1s"`,
or `protocol = "https"` instead of `"h1s"` — does **not** error.
The listener silently binds as a plain-TCP L4 forwarder and
forwards bytes opaquely. There is no warning log, no config-time
rejection, no metrics-side hint. Operators discover the mistake
only when external clients fail to negotiate TLS / H2 / H3.
Impact: silent misconfiguration → bytes forwarded with no TLS, no
ALPN dispatch, no proxy-protocol awareness. Worse, in a deployment
that *expected* TLS termination at the gateway, the upstream now
receives raw TCP from the internet.
Reproduction: write
```
[[listener]]
address = "0.0.0.0:8443"
protocol = "https"   # typo
```
and run the binary. Observe no error; observe `curl
https://gateway:8443/` produce a TLS-protocol-version error (no
ALPN, no cert).
Recommendation:
  1. Make the unknown branch return
     `anyhow::bail!("unknown listener protocol {p:?}; expected one
     of: plain-tcp, tls, h1, h1s, h3")` so misconfigurations fail
     at startup.
  2. Require `protocol = "plain-tcp"` to be explicit for the L4
     case (no silent default).
  3. Validate the field during config load
     (`crates/lb-controlplane::Config::validate`) before
     `build_listener_mode` is reached, so the failure happens
     before bind.
Cross-ref: `code` (config-validation crate).

---

### PROTO-2-10 — SmuggleDetector unwired in the L7 hot path; hyper 1.x does NOT cover every CL/TE variant the detector targets
Status:   Proposed-Fix(Wave-2b-2: detector hot-path wire-up landed in SEC-2-01 (`e00e85a`) + CODE-2-01 (`dc02517`); Wave-2b-2 lands the wire-up matrix doc `audit/protocol/SMUGGLE-MATRIX.md` mapping every CL/TE variant to hyper-1.9.0, default-mode detector, and H1Strict-mode detector behaviour, plus 13 proof tests in `crates/lb-l7/tests/smuggle_matrix.rs::{test_default_strict_te, test_pipelined_cl_te, test_duplicate_cl_differing, …}` exercising the rows that distinguish the three columns. No PROTO-2-99-A escalation: every variant where hyper passes, the detector either also passes or strictly rejects on top.)
Severity: high
Status:   Open
Location:
  * Detector definition: `crates/lb-security/src/smuggle.rs` —
    `check_cl_te`, `check_te_cl`, `check_h2_downgrade`.
  * Unit tests pass: `tests/security_smuggling_cl_te.rs`,
    `tests/security_smuggling_te_cl.rs`,
    `tests/security_smuggling_h2_downgrade.rs`.
  * **Call-site sweep returns ZERO hits**:
    `grep -rn 'SmuggleDetector\|check_cl_te\|check_te_cl\|check_h2_downgrade' crates/lb-l7 crates/lb/src`
    has no results (re-verified Round 2).
  * `lb-l7` `Cargo.toml` does **not** depend on `lb-security`
    (cross-ref `code` Q-CODE-1-01; T1 in synthesis).
Description: The smuggling-detector family exists, has unit-test
green-lights, and is **never invoked from the request-processing
pipeline**. The gateway therefore relies entirely on hyper 1.x's
built-in defences. Hyper's coverage matrix:

| Attack variant                                          | Hyper 1.x H1 server | Hyper 1.x H1 client | SmuggleDetector |
|---|---|---|---|
| CL + TE: chunked on same H1 request (both headers)      | rejects with 400 (request-decoder errors)  | — | catches |
| TE: chunked, X (multiple TE values)                     | partial — hyper accepts known tokens only  | — | catches non-chunked-as-final-token |
| TE final token != chunked                               | partial                                   | — | catches |
| CL with multiple comma-separated values (`CL: 5,5`)     | rejects                                   | — | catches |
| CL with leading-zero / space-padded value               | rejects                                   | — | catches |
| TE: chunked\r\nTransfer-Encoding: chunked (duplicate)   | rejects                                   | — | catches |
| H2 → H1 downgrade with `Transfer-Encoding` header forwarded onto H1 hop | hyper H2 server rejects TE in H2 headers (RFC 9113 §8.2.2); but `H2ToH1Bridge` could re-inject one via header passthrough if hop-by-hop strip is skipped | depends on bridge | `check_h2_downgrade` catches this |
| Smuggled `\r\n` inside a header value (CRLF injection)  | hyper rejects per httparse                | — | catches |
| Body-after-final-chunk (trailing garbage)               | hyper request-decoder rejects             | — | catches |
| Chunk-extension parsing edge cases                      | hyper accepts most; rejects malformed lengths | — | partial overlap |
| H2 upgrade-via-`Upgrade: h2c` injection                 | hyper does not bootstrap h2c here (not configured); but `Upgrade` not stripped from forwarded request reaches upstream | — | catches via h2_downgrade |

Net: hyper covers ~70% of the obvious cases the detector targets.
The two gaps that matter most:
  (a) **TE-final-token variants** (`Transfer-Encoding: gzip,
      chunked` vs `chunked, gzip`) — RFC 9112 §7.1.1 requires
      `chunked` to be final; hyper accepts both orderings for
      decoded input but does not normalise the outbound header.
  (b) **H2-to-H1 downgrade**: when the listener is H2 and the
      backend is H1, an H2 request that hyper accepted (because
      H2 forbids TE entirely so it's already absent) can still
      have hop-by-hop headers smuggled if `strip_hop_by_hop`
      somehow misses (e.g. via `Trailer:` containing
      `Transfer-Encoding`, or via untrusted intermediary
      regression).

Impact: HTTP request smuggling primitives against backends that
authorise on header parsing differences. High severity in any
deployment that has *more than one* backend behind the same
gateway and where backends may parse TE/CL differently from hyper.
Reproduction: feed a TE-CL desync case (`Transfer-Encoding:
chunked` + `Content-Length: 5`, body is a single chunk followed by
a second smuggled request) to the gateway → observe the gateway
forwards the smuggled portion to the upstream (whose parser
accepts CL where hyper accepted TE).
Recommendation:
  1. Wire `SmuggleDetector::check_cl_te` and
     `check_te_cl` into `H1Proxy::proxy_request` before bridge
     selection. Reject 400 on any positive.
  2. Wire `check_h2_downgrade` into the H2→H1 bridge path
     (`H2Proxy::proxy_h2_to_h1`).
  3. Add `lb-security` as a `lb-l7` dependency (cross-cuts
     `code` Q-CODE-1-01; T1 in synthesis).
  4. Add integration tests at the gateway level (not just unit
     tests of the detector): spawn the gateway, send each
     attack variant via raw TCP, assert 400.
Cross-ref: lead T1; `sec` S-1 / S-3 / S-4; `code` Q-CODE-1-01;
PROTO-2-01 (host disagreement is a smuggling sibling).

---

### PROTO-2-11 — No HTTP/2 `GOAWAY` and no HTTP/3 `CONNECTION_CLOSE` on drain / SIGTERM
Severity: high
Status:   Proposed-Fix(H3 half) — `lb-quic` actor now emits
application-layer `CONNECTION_CLOSE` with `H3_NO_ERROR = 0x0100` on
cancel via `graceful_h3_shutdown`. Proof:
`crates/lb-quic/tests/h3_graceful_close.rs::test_h3_connection_close_emitted_on_cancel`
drives an end-to-end loopback handshake, calls the helper, and asserts
the client observes `peer_error { is_app: true, error_code: 0x0100 }`.
The H/2 `GOAWAY` half (hyper-side wiring in
`crates/lb-l7/src/h{1,2}_proxy.rs` and the SIGTERM handler in
`crates/lb/src/main.rs:1033-1059`) is **Open** — Wave-2c code-owned
work to plumb `lb_core::Shutdown::token()` into the listener crate
and call `hyper::server::conn::http2::Connection::graceful_shutdown`.
Location:
  * SIGTERM handler: `crates/lb/src/main.rs:1033-1059` —
    receives signal, calls `JoinHandle::abort()` on every TCP
    listener task, then `tokio::time::sleep(500ms)`. No path
    invokes hyper's `http2::Connection::graceful_shutdown` (which
    emits GOAWAY with last-stream-id).
  * QUIC drain: `crates/lb-quic/src/listener.rs::shutdown` (the
    method called at `main.rs:1047`) cancels its
    `CancellationToken` and returns a `JoinHandle`. The
    `conn_actor` then drops the `quiche::Connection`, which does
    *not* send a `CONNECTION_CLOSE` frame with
    application-error 0x100 / "graceful". The H3 layer never
    sends a `GOAWAY` either (RFC 9114 §5.2).
  * Grep evidence: `grep -rn 'graceful_shutdown\|GOAWAY\|goaway\|CONNECTION_CLOSE' crates/lb-l7 crates/lb-quic crates/lb/src`
    returns three matches: a doc-comment in `lb-quic/src/lib.rs:606`,
    and two cargo-level metric/setting names in `h2_security.rs`.
    Zero call-sites.
Description: A production HTTP/2 server MUST send a `GOAWAY` frame
with last-stream-id when draining, so clients know which streams
will be processed and which they can safely retry on a fresh
connection (RFC 9113 §6.8). A production HTTP/3 server MUST send
`GOAWAY` (RFC 9114 §5.2) followed by a `CONNECTION_CLOSE` frame
(RFC 9000 §10.2) carrying an application-error indicating
graceful close. Today the gateway emits neither: it `abort()`s
the connection task, which from the client's perspective is
indistinguishable from a TCP RST / connection reset (or a UDP
packet loss for QUIC). Idempotent in-flight retries become
non-idempotent retries.
Impact:
  * Clients that pool H2 connections (typical for browsers and
    gRPC clients) retry streams that the server already
    processed, doubling write-side load on the upstream.
  * H3 clients receive what looks like packet loss, then attempt
    a fresh handshake against a listener that just shut down —
    leading to per-request connect storms.
  * No way to drain in a load-balancer rotation without dropped
    requests.
Reproduction:
  1. Start gateway with active H2 keepalive.
  2. Send SIGTERM. Observe (via Wireshark) the TCP FIN appears
     **without** a preceding GOAWAY frame in the H2 payload.
  3. Same for H3: UDP traffic stops; no `CONNECTION_CLOSE` is
     emitted.
Recommendation:
  1. Plumb a `CancellationToken` per H2/H3 connection so the
     SIGTERM handler can: (a) call hyper's
     `http2::Connection::graceful_shutdown()` on every live H2
     conn, then wait for the drain deadline, (b) send H3 GOAWAY
     (max stream-id encoded) and a quiche
     `Connection::close(true, 0x100, b"shutdown")` on every
     active QUIC connection.
  2. The drain deadline is owned by `rel` (10 s default,
     configurable — see lead synthesis §B-2).
  3. Add unit tests:
     `tests/h2_graceful_goaway.rs`,
     `tests/h3_graceful_close.rs` that assert the frames appear
     on the wire before the socket closes.
Cross-ref: lead T2 (SIGTERM is not a drain); `rel` H7 (drain
deadline + cancellation); `code` Q-CODE-1-04/05 (CancellationToken
plumbing).

---

### PROTO-2-12 — Trailer pass-through across H1↔H2/H3 (non-gRPC) untested and likely broken
Status:   Baseline-Pinned-Wave-2b-2 / Fix-Deferred-Wave-2c (investigation confirms trailers drop on every cross-protocol bridge: `BridgeRequest`/`BridgeResponse` lack a trailers field, proxy writeback uses `http_body_util::Full<Bytes>` which is single-frame. `crates/lb-l7/tests/trailer_passthrough.rs` (6 tests) pins the current trailer-dropping baseline; Wave-2c will flip them green via bridge-surface extension. See `audit/deferred.md` "PROTO-2-12" for the threaded-through plan.)
Severity: medium
Status:   Open
Location:
  * Bridge response shapes: `crates/lb-l7/src/h1_to_h2.rs`,
    `h2_to_h1.rs`, `h1_to_h3.rs`, `h3_to_h1.rs` —
    `BridgeResponse` has a `headers: HeaderMap` and a body
    stream, but **no** `trailers` slot. (Search the type
    definition in `lb-l7/src/lib.rs:48+`.)
  * gRPC trailers (`grpc-status`, `grpc-message`) are
    synthesised in `crates/lb-l7/src/grpc_proxy.rs:404-420` as a
    special-case at the H2 layer; this is the only trailer path
    that demonstrably works.
  * No test exercises a non-gRPC trailer round-trip across the
    bridges. `grep -rn 'Trailer\|trailers()' tests/` returns the
    H1 chunked-trailer codec unit test (`conformance_h1.rs`) and
    the gRPC e2e — nothing across the H1↔H2 / H1↔H3 bridge.
Description: RFC 9110 §6.6 ("Trailer Fields") allows trailers on
both H1 chunked responses and on H2/H3 responses. The H1 codec
parses them (`lb-h1::ChunkedDecoder::trailers`), but the bridge
response struct cannot carry them, so the H1→H2 / H1→H3 / H3→H1
paths drop them silently. The hyper response type *does* support
trailers via `http_body::Frame::Trailers`, so this is a
representation problem inside the LB, not a library limitation.
Impact: any non-gRPC use of trailers (e.g. server timing trailers,
checksum trailers per RFC 9530) is silently dropped across
protocol-mismatched bridges. gRPC-Web → gRPC paths work because of
the dedicated synth code, but generic trailer forwarding is
broken.
Reproduction: spin up an H1 backend that emits chunked response
with `Trailer: X-Checksum\r\n...\r\nX-Checksum: deadbeef\r\n`.
Front it with the gateway in H2 listener mode. Observe the H2
client receives the body but no trailer HEADERS frame.
Recommendation:
  1. Extend `BridgeResponse` to carry
     `trailers: Option<HeaderMap>` and `Frame::Trailers`-aware
     streams.
  2. Update all 6 bridge impls to populate and forward trailers
     (`h1_to_h2`, `h1_to_h3`, `h2_to_h1`, `h2_to_h2`, `h2_to_h3`,
     `h3_to_h1`, `h3_to_h2`, `h3_to_h3`).
  3. Add `tests/trailers_h1_to_h2.rs`,
     `tests/trailers_h2_to_h1.rs`,
     `tests/trailers_h3_to_h1.rs` covering single-trailer and
     multiple-trailer cases.
  4. Strip hop-by-hop names from trailers as well (`Trailer`
     itself, plus `Content-Length`, `Cache-Control`, etc.
     forbidden as trailer names per RFC 9110 §6.6.1).
Cross-ref: PROTO-2-07 (Bridge trait hygiene).

---

### PROTO-2-13 — `SETTINGS_ENABLE_CONNECT_PROTOCOL` IS sent — verified — but no integration test asserts it on the wire
Severity: low
Status:   Open
Location: `crates/lb-l7/src/h2_proxy.rs:246`:
`builder.enable_connect_protocol();` — confirmed present and
called on every H2 server builder. **Status: wired correctly.**
Description: This was a Round-1 candidate finding (PROTO-2-L) on
suspicion that the hyper builder method might not be called. Round
2 verification: the call is present in the production code path
at `h2_proxy.rs:246`, immediately after `builder.timer(...)` and
inside the always-taken `serve_h2_conn` arm. Hyper 1.x's
`http2::Builder::enable_connect_protocol()` sets the
`ENABLE_CONNECT_PROTOCOL` setting in the initial SETTINGS frame
per RFC 8441. The downgrade to "low severity / informational" is
that there is **no integration test that asserts a wire-level
SETTINGS frame contains this setting** — a future refactor could
remove the call without CI noticing (the existing
`tests/ws_proxy_e2e.rs` covers the extended-CONNECT *flow* but
not the *advertisement*).
Impact: currently zero; regression risk medium.
Reproduction: n/a (working today).
Recommendation: add `tests/h2_settings_advertisement.rs` that
opens an H2s connection to the gateway, reads the server's
initial SETTINGS frame from a raw `h2 = "0.4"` client, and
asserts `SettingId::EnableConnectProtocol -> 1` is present.
Cross-ref: PROTO-2-04 (WebSocket conformance breadth).

---

### PROTO-2-14 — TLS 1.2 enabled on every TLS listener; no `tls13_only` config switch
Status:   Proposed-Fix(Wave-2b-2: new `[runtime.tls]` block with `tls13_only: bool` (default false) added to `crates/lb-config/src/lib.rs::RuntimeConfig`; new `lb_security::build_server_config_with_policy` function in `crates/lb-security/src/ticket.rs` configures rustls via `with_protocol_versions(&[&rustls::version::TLS13])` when `tls13_only = true`. Original `build_server_config` retained as a backwards-compat shim that calls the new function with `tls13_only = false`. Proof: `crates/lb-security/tests/tls_versions.rs::test_tls13_only_rejects_tls12` (live TLS 1.2 client handshake against a tls13_only server fails) + `default_config_lists_tls12_and_tls13` + `tls13_only_config_builds_without_tls12`. Wave-2c binary wiring threads the config field into the call.)
Severity: medium
Status:   Open
Location:
  * `crates/lb-security/src/ticket.rs:319-338`
    (`build_server_config`) uses
    `rustls::ServerConfig::builder_with_provider(ring_provider)
    .with_safe_default_protocol_versions()` — which on rustls 0.23
    with feature `tls12` returns `[&TLS13, &TLS12]`.
  * Caller sites: `crates/lb/src/main.rs:214` (`build_tls_stack`),
    `crates/lb/src/main.rs:611` (`build_h1s_tls_stack`). Neither
    passes a version override.
  * `Cargo.toml` for `lb-security`: rustls dependency has the
    `tls12` feature implicit-on. Removing the feature is a
    workspace-wide change.
  * Config knobs: searching `crates/lb-controlplane/src` for
    `tls13|min_version|min_protocol|tls_min` returns zero hits.
Description: TLS 1.2 is enabled on every listener with no
operator-visible way to disable it. RFC 8446 §1.2 and broader
industry guidance recommend TLS 1.3 only for new deployments
fronting the internet (BSI TR-02102, NIST SP 800-52r2 still
permits 1.2 with hardened cipher subsets). The gateway has no
cipher-suite knob either (PROTO-1 inventory §5), so the operator
cannot reduce attack surface from the default.
Impact: TLS 1.2 attack surface includes the well-known
implementation issues (no AEAD enforced; renegotiation is
disabled in rustls but bytes-on-wire fingerprints still expose
1.2). Compliance frameworks (FedRAMP High, PCI 4.0) increasingly
require 1.3-only or 1.3-preferred with explicit downgrade
documentation.
Reproduction: `openssl s_client -tls1_2 -connect gateway:443`
succeeds with default config.
Recommendation:
  1. Add `[tls].min_protocol = "1.3"` config knob (default
     `"1.2"` for compatibility, recommended `"1.3"` documented).
  2. Plumb the knob into `build_server_config` by selecting the
     versions slice:
     ```rust
     let versions = if cfg.min_protocol == TlsVersion::V1_3 {
         &[&rustls::version::TLS13][..]
     } else {
         rustls::DEFAULT_VERSIONS
     };
     ```
     and using `builder.with_protocol_versions(versions)`.
  3. Document the trade-off in the deployment guide; emit a
     `tracing::warn!` at startup when `min_protocol < 1.3`.
  4. Cross-ref `sec` for the compliance angle (their call on
     whether to flip the default).
Cross-ref: `sec` (deployment-profile recommendations); lead
synthesis §C.

---

### PROTO-2-15 — SNI ↔ Host / `:authority` disagreement is not enforced
Severity: medium
Status:   Proposed-Fix-Partial(Wave-2b-2: validator function landed at `crates/lb-l7/src/sni_authority.rs::check_sni_authority` + 421 renderer `misdirected_response()`; unit tests in module (9) + integration `crates/lb-l7/tests/sni_authority_mismatch.rs::test_421_on_mismatch` (7 tests). **TLS-accept-site wiring DEFERRED to Wave-2c** because SNI capture lives in `crates/lb/src/main.rs` which Wave-2b cannot touch — see `audit/deferred.md` "PROTO-2-15 wiring side" for the threaded-through plan.)
Location:
  * No SNI extraction or comparison anywhere in `lb-l7` /
    `lb-quic` proxy paths.
  * TLS terminator (`crates/lb/src/main.rs:214, 611`) uses a
    single-cert rustls `ServerConfig`. The negotiated SNI is
    available via `ServerConnection::server_name()` (rustls 0.23)
    but never read by the proxy layer.
  * H1 Host / H2 `:authority` extraction happens in
    `crates/lb-l7/src/h2_proxy.rs:320` (see PROTO-2-01) and in
    `crates/lb-l7/src/h1_proxy.rs::extract_host` (~line 950);
    neither correlates with SNI.
Description: When the gateway terminates TLS, the SNI presented
by the client is the *intended* destination authority. If the
post-handshake `Host` (H1) or `:authority` (H2/H3) disagrees, the
client is either (a) misconfigured, (b) malicious — using SNI
spoofing to bypass network-level egress controls (the upstream
SDN may route on SNI but authorise on Host), or (c) a legitimate
SNI-agnostic client (which is rare and getting rarer). For a
multi-tenant gateway with virtual-host routing, SNI/Host disagreement
is a host-confusion attack primitive (analogous to PROTO-2-01).
Impact:
  * Low blast radius today because the gateway runs a single
    cert per listener (no SNI-vhost map) and a single
    routing table.
  * High blast radius the instant SNI-based cert resolution or
    SNI-routed multi-tenant deployment lands (the inventory's
    §5.1 mentions a future SNI-based cert resolver).
  * Even today, a non-loopback bind that receives SNI-spoofed
    traffic from a network attacker may misroute requests on the
    Host header alone.
Reproduction: ` openssl s_client -servername a.test -connect
gateway:443 ` then send `GET / HTTP/1.1\r\nHost: b.test\r\n\r\n`.
Observe gateway routes on `b.test` (Host) regardless of SNI
(`a.test`).
Recommendation:
  1. Extract SNI in the TLS-accept path
     (`ServerConnection::server_name()` → propagate via a
     hyper extension or via the request `Extensions` map).
  2. After header normalisation, compare against the H1
     `Host` / H2 `:authority` host portion (port is implicit
     from listener bind; ignore it in the comparison).
  3. **Reject 421 Misdirected Request** when the two
     disagree *and* the listener is bound on a non-loopback
     address. (Loopback exception is for unit tests and
     same-host curls that often omit Host.)
  4. Add `tests/sni_authority_mismatch_rejected.rs`.
  5. Gate behind a config switch
     `[tls].enforce_sni_authority_match = true` so loopback
     and dev deployments are unaffected.
Cross-ref: PROTO-2-01 (the in-protocol sibling of this finding);
`sec` (multi-tenant deployment-mode work).

---

## Summary table

| ID         | Severity | Title (short)                                                | Location                                |
|------------|----------|-------------------------------------------------------------|-----------------------------------------|
| PROTO-2-01 | high     | H2 `:authority` vs `Host` mismatch — silent precedence       | `lb-l7/src/h2_proxy.rs:320-330`         |
| PROTO-2-02 | critical | `LB_QUIC_ALPN = b"lb-quic"`; no production `b"h3"`           | `lb-quic/src/lib.rs:115, 392`           |
| PROTO-2-03 | medium   | No 1xx / 100-Continue / 103 forwarding                       | absence; `lb-l7/src/*`                  |
| PROTO-2-04 | medium   | `ws_autobahn.rs` is a `--help` stub                          | `tests/ws_autobahn.rs:24-34`            |
| PROTO-2-05 | medium   | No `h3spec` (or `h3i`) harness                               | absence                                 |
| PROTO-2-06 | low      | `conformance_h{1,2,3}.rs` are codec round-trip unit tests    | `tests/conformance_h*.rs`               |
| PROTO-2-07 | low      | `H2ToH2Bridge` / `H3ToH3Bridge` trait-level hop-by-hop gap   | `lb-l7/src/h2_to_h2.rs`, `h3_to_h3.rs`  |
| PROTO-2-08 | low      | `HOP_BY_HOP` lists spurious `trailers` name                  | `lb-l7/src/h1_proxy.rs:54-63`           |
| PROTO-2-09 | medium   | `ListenerMode` silent fallthrough to `PlainTcp`              | `lb/src/main.rs:837`                    |
| PROTO-2-10 | high     | SmuggleDetector unwired; hyper-only coverage gaps            | `lb-l7/Cargo.toml`; `lb-security/...`   |
| PROTO-2-11 | high     | No GOAWAY / CONNECTION_CLOSE on SIGTERM                      | `lb/src/main.rs:1033-1059`              |
| PROTO-2-12 | medium   | Trailer pass-through (non-gRPC) untested and broken          | `lb-l7/src/lib.rs` (BridgeResponse)     |
| PROTO-2-13 | low      | `SETTINGS_ENABLE_CONNECT_PROTOCOL` not asserted on the wire  | `lb-l7/src/h2_proxy.rs:246`             |
| PROTO-2-14 | medium   | TLS 1.2 enabled with no `tls13_only` config knob             | `lb-security/src/ticket.rs:319-338`     |
| PROTO-2-15 | medium   | SNI ↔ Host/`:authority` disagreement not enforced            | `lb/src/main.rs:214,611`; `lb-l7/...`   |

## Notes

* Severities are pre-cross-review. Round-2 cross-review may
  adjust based on `sec`/`code`/`rel`/`ebpf` evidence — see
  `audit/protocol/round-2-cross-review.md` once teammates land.
* Every finding cites file:line at the head of the
  Location block. Where the issue is an absence, the
  Location is "absence — `<grep command>` returns no results"
  with the search executed against this commit.
* Round 3 will produce a remediation plan keyed by these IDs.
