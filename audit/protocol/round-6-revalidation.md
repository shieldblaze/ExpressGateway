# Round 6 — `proto` self-verification of own Round-6 delta fixes

Author/verifier: `proto` (self-verified; lead may push to `sec` for
adversarial cross-check)
Branch:    `prod-readiness/round-4` (cherry-picked on top of
           `worktree-agent-a6ba7c29ecdd8dc22`)
Findings:  `audit/protocol/round-6-delta-findings.md` (commit
           `2cc7949`, body identical to upstream `a378be3`)
Toolchain: cargo 1.85.1
Sanity:    `cargo build -p lb-l7 -p lb -p lb-config` — PASS;
           `cargo clippy -p lb-l7 --tests -- -D warnings` — PASS.
           `cargo fmt` — clean on the four touched commits.

The Round-3-mini loop re-entered planning when `proto`'s own
delta-discovery sweep on `prod-readiness/round-4` surfaced four new
issues (2 Medium, 2 Low). Because `proto` authored the fixes, the
self-verification report lives here as a separate file so a future
regression round can re-check; the lead may also push to `sec` for
adversarial cross-check per spec.

| Finding    | Severity | Author-SHA | Verifier-SHA (test run) | Status                                                |
|------------|----------|-----------:|------------------------:|-------------------------------------------------------|
| PROTO-2-16 | Low      |  `9b374cc` | `9b374cc` (doc only)    | Verified-Fixed(self)                                  |
| PROTO-2-17 | Low      |  `e3e7b17` | `e3e7b17`               | Verified-Fixed(self)                                  |
| PROTO-2-18 | Medium   |  `444668d` | `444668d` (tests PASS)  | Verified-Fixed(self-verified-with-lead-cross-check-requested) |
| PROTO-2-19 | Medium   |  `42df990` | `42df990` (tests PASS)  | Verified-Fixed(self-verified-with-lead-cross-check-requested) |

---

## PROTO-2-18 — `check_sni_authority` wired

Author-SHA: `444668d` (`PROTO-2-18 — wire check_sni_authority via with_expected_sni builder`)

### Fix summary
- `H1Proxy` / `H2Proxy` grew a fluent `.with_expected_sni(self,
  sni: Option<String>) -> Self` builder + an `expected_sni:
  Option<String>` field (builder default `None`).
- New entry points `serve_connection_with_cancel_sni(io, peer,
  cancel, sni: Option<String>)` thread a per-connection SNI value
  into the request hot-path. `serve_connection_with_cancel` now
  delegates to the new method with the builder default for back-
  compat.
- Inside `handle`: after `inspect_request` + (H2)
  `check_authority_host_agreement`, call
  `check_sni_authority(expected_sni, authority)` and return 421
  Misdirected Request (RFC 9110 §15.5.20) on mismatch.
- Loopback exception per the sec-r5 caveat —
  `peer.ip().is_loopback()` skips enforcement.
- `crates/lb/src/main.rs` H1s TLS accept site captures
  `tls_stream.get_ref().1.server_name().map(str::to_owned)` and
  threads through both ALPN branches via
  `serve_connection_with_cancel_sni`.

### Proof test

`crates/lb-l7/tests/sni_authority_421.rs`:

  - `test_421_emitted_on_sni_host_mismatch_over_tls` — drives a
    real TLS 1.3 handshake against
    `H1Proxy::serve_connection_with_cancel_sni`, sends
    `SNI=a.test` + `Host: b.test` from a TEST-NET-1 (RFC 5737)
    non-loopback peer. Asserts the response line starts
    `HTTP/1.1 421` with the RFC 9110 §15.5.20 phrase.

  - `test_loopback_allows_mismatch` — same shape but from
    `127.0.0.1`. Asserts the response is NOT `421` (the loopback
    exception fires; the request proceeds and resolves to 502 once
    the closed backend dial fails).

Test run (verifier-SHA `444668d`):

```
test test_421_emitted_on_sni_host_mismatch_over_tls ... ok
test test_loopback_allows_mismatch ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.01s
```

PASS.

### Status

`Verified-Fixed(self-verified-with-lead-cross-check-requested)`.
Lead may push the 421-rejection wiring to `sec` for adversarial
cross-check (bypass-attempt review on the loopback caveat, X-
Forwarded-For trust-chain interaction, peer-IP source on PROXY-
protocol listeners, etc.). The proof test exercises only the
direct accept path; a `sec` review may surface a PROXY-protocol or
mTLS-rewritten-peer edge case worth a follow-up.

---

## PROTO-2-19 — H1 trailer wire emission

Author-SHA: `42df990` (`PROTO-2-19 — H1 trailer wire emission for cross-bridge trailers`)

### Fix summary
- Extracted shared `pub fn build_h1_response_with_trailers(
  translated: BridgeResponse, alt_svc: Option<AltSvcConfig>) ->
  Response<BoxBody<Bytes, hyper::Error>>` used by both
  `upstream_response_to_h1` (H2→H1) and `h3_response_to_h1`
  (H3→H1) so the trailer-aware head shape is identical on both
  bridges.
- When `translated.trailers` is non-empty, the helper:
  - Drops any pre-existing `transfer-encoding`, `content-length`,
    or `trailer` from the upstream-translated headers.
  - Injects `Transfer-Encoding: chunked` (RFC 9112 §6.1) — re-
    emission is end-to-end on our hop even though chunked is
    hop-by-hop per RFC 9110 §7.6.1.
  - Injects `Trailer: <comma-list>` (RFC 9110 §6.6) listing the
    trailer names.
- Body emit unchanged: `build_body_with_trailers` produces
  `[Frame::data(...), Frame::trailers(...)]` via `StreamBody`.

### Proof test

`crates/lb-l7/tests/trailer_passthrough.rs` grew two tests that
drive hyper's real H1 server-side encoder over an in-memory
duplex with a `Response` built by
`build_h1_response_with_trailers`:

  - `test_h2_h1_trailers_emitted_on_wire` — backend response
    carries `grpc-status: 0` + `grpc-message: OK` trailers and a
    `Content-Length: 5` header. Asserts the wire bytes:
    - contain `Transfer-Encoding: chunked`
    - contain `Trailer: grpc-status, grpc-message`
    - DO NOT contain `Content-Length: 5` (dropped per §6.5)
    - contain the chunked terminator `0\r\n`
    - contain `grpc-status: 0` and `grpc-message: OK` as trailer
      fields after the terminator
    - Client request carries `TE: trailers` (RFC 9110 §6.6.1).

  - `test_h3_h1_trailers_emitted_on_wire` — same shape via the
    shared helper. Forward-compatible for the H3→H1 leg once
    `lb_quic` surfaces H3 trailers (deferred).

Test run (verifier-SHA `42df990`):

```
test test_h2_h1_trailers_emitted_on_wire ... ok
test test_h3_h1_trailers_emitted_on_wire ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

PASS.

### Status

`Verified-Fixed(self-verified-with-lead-cross-check-requested)`.
The H3 cross-bridge leg is forward-compatible but inactive at
runtime today (the H3 surface gap in `lb_quic::H3UpstreamResponse`
is the same one tracked in `audit/deferred.md` PROTO-2-12 H3
leg). A `sec` cross-check may want to verify the
`TE: trailers`-gated emit interacts safely with hop-by-hop strip
on the request side (we did not modify the request-side strip;
clients arriving without `TE: trailers` continue to have hyper
silently drop the trailer frame as RFC 9110 §6.6.1 mandates).

---

## PROTO-2-16 — H1 cancel comment fixup (Low)

Author-SHA: `9b374cc`. Doc-comment only — corrected the
`H1Proxy::serve_connection_with_cancel` rustdoc to match hyper-1's
actual `http1::graceful_shutdown` contract (`disable_keep_alive()`
+ encoder-level `Connection: close` injection only on a not-yet-
flushed response head). No code change, no test needed.

Status: `Verified-Fixed(self)`.

## PROTO-2-17 — `[security].strict_te` config knob (Low)

Author-SHA: `e3e7b17`. New `[security] strict_te: bool` field in
`LbConfig` selects `SmuggleMode::H1Strict` for the shared
`HooksBundle` at `crates/lb/src/main.rs:1719`. Default `false`
preserves the lenient RFC 9112 baseline. Backward-compatible —
every existing config file continues to parse unchanged. Documented
in `CONFIG.md` with the RFC 9112 §7.1 reference and a worked
example. `cargo test -p lb-config` continues to PASS (31 tests).

Status: `Verified-Fixed(self)`.

---

## Status-field updates in `audit/protocol/round-2-review.md`

- **PROTO-2-15**: flipped `Verified-Fixed-Partial(sec,3586367)` to
  `Verified-Fixed(verifier=proto-self, round-6, author-sha=444668d)`
  with a reference to the proof test. The Round-5 verdict had
  acknowledged that the validator existed; Round-6 completes the
  hot-path wiring so the 421 contract is now enforced.

- **PROTO-2-12**: flipped to
  `Verified-Fixed-Partial(verifier=proto-self, round-6,
  author-sha=7deeaf3+30f9967+42df990)` to reflect that the H1-wire-
  emission gap is closed (the bridge-layer pass-through was already
  verified in Round-5; the wire-emit gap is what PROTO-2-19
  addressed). The H3 surface gap in `lb-quic` remains; that is the
  only reason the status stays `-Partial`.

No other PROTO-2-xx Status fields are affected by the Round-6 delta
fixes.

---

## Workflow notes

- Each commit was fmt + clippy clean before stacking; the worktree
  branch `worktree-agent-a6ba7c29ecdd8dc22` is at
  `42df990` after the four-commit stack.
- `audit/STATE` `# Round 6 status:` note records the mini-loop entry.
- Lead may now push PROTO-2-18 and PROTO-2-19 to `sec` for the
  optional adversarial cross-check (per the spec's
  "lead may also push to sec" carve-out).

Task #43 — completed.
