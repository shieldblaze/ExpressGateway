# Deferred findings — production-readiness audit

Findings recorded here have been triaged out of the Round-3 fix
cadence with the team-lead's pre-emptive acknowledgement. Each entry
states the rationale; the user's final sign-off lands at FINAL.

---

## sec

### SEC-2-13 — 0-RTT on TCP/TLS listener (info)

**Status**: deferred — **closed-as-not-a-bug**.

`build_server_config` in `crates/lb-security/src/ticket.rs:319-338`
never assigns `max_early_data_size`; rustls 0.23.38 defaults this to
`0`, so 0-RTT is disabled by construction on every TCP/TLS listener.
There is no live attack surface today.

**Defence-in-depth follow-up** (will be folded into the SEC-2-01 fix
PR as a one-line additional test, no separate plan required): add
`#[cfg(test)] fn test_zero_rtt_disabled_invariant()` in `ticket.rs`
that builds the default config and asserts `max_early_data_size ==
0`. If a future change enables 0-RTT, this finding **must be
re-opened as critical** because the TCP path has no replay guard.

### SEC-2-15 — Hyper 1.9.0 smuggling-defence reference matrix (info)

**Status**: deferred — **reference material, not actionable on its
own**.

SEC-2-15 documents what hyper catches at the wire-decoder level vs.
what the gateway must guard against above hyper. The actionable
output of this analysis is **already folded into SEC-2-01's plan**
(the strict TE-codec policy and the `Transfer-Encoding: gzip,
chunked` rejection). No separate fix is needed.

### SEC-2-16 — Atomic-ordering hand-off list for `code` (info)

**Status**: deferred — **handed off to code under CODE-2-04**.

Per lead decision in synthesis §E.3, `code` owns the per-site atomic
ordering audit as a single workspace-wide plan; SEC-2-16 is the
input list. No separate sec plan is authored. Sec will review code's
CODE-2-04 plan when it lands.

---

(Other areas append their own deferred sections below.)

---

## proto (Wave-2b-2)

### PROTO-2-15 wiring side — SNI propagation from TLS-accept site (deferred)

**Status**: validator landed Wave-2b-2; **wiring deferred to Wave-2c**.

`crates/lb-l7/src/sni_authority.rs::check_sni_authority` + the 421
Misdirected Request renderer (`misdirected_response`) ship Wave-2b-2
with full unit-test coverage (`crates/lb-l7/tests/sni_authority_mismatch.rs`
— 7 tests pass). Threading the captured SNI from the
`tokio_rustls::TlsAcceptor::accept` future down into
`H1Proxy::serve_connection` / `H2Proxy::serve_connection`
requires a handler change in `crates/lb/src/main.rs` (the TLS-accept
site is built there). Wave-2b is forbidden from touching `main.rs`;
Wave-2c will:

  1. Capture `acceptor.accept(stream).get_ref().1.server_name()`
     from the rustls handshake result (or the
     `Acceptor::accept`-path equivalent) at the binary's TLS
     handler.
  2. Pass the captured SNI (`Option<String>`) into a new
     `with_sni: Option<String>` builder on each `H{1,2}Proxy`
     instance, or surface it via a request-extension propagated
     through the `serve_connection` IO wrapper.
  3. Inside `H1Proxy::handle` / `H2Proxy::handle`, after the
     PROTO-2-01 `check_authority_host_agreement` block, call
     `lb_l7::sni_authority::check_sni_authority(sni.as_deref(),
     authority_str)` and on `Err(_)` return
     `misdirected_response()` rendered as `Response<BoxBody<…>>`.

### PROTO-2-12 — trailer pass-through across cross-protocol bridges (deferred)

**Status**: baseline pinned Wave-2b-2; **bridge-surface fix deferred
to Wave-2c**.

Investigation showed the proxy hot path's cross-protocol bridges
(every H1↔H2, H1↔H3, H2↔H2, H2↔H3, H3↔H2 path in `h1_proxy.rs`
/ `h2_proxy.rs`) collect bodies via `BodyExt::collect()` then
re-wrap as `http_body_util::Full::new(body_bytes)`. `Full<Bytes>`
is a single-frame body and **cannot carry trailers**. The
`BridgeRequest` / `BridgeResponse` types in `crates/lb-l7/src/lib.rs`
also lack a trailers field, so the bridge trait surface cannot
forward them even if the writeback were fixed.

Wave-2b-2 lands `crates/lb-l7/tests/trailer_passthrough.rs` as a
**behaviour baseline** (6 tests pass today) that pins the current
trailer-dropping shape. Wave-2c must:

  1. Add `pub trailers: Option<http::HeaderMap>` to
     `BridgeRequest` / `BridgeResponse`.
  2. Replace every `Full::new(body_bytes)` writeback in
     `h{1,2}_proxy.rs` translation helpers with a `StreamBody`
     yielding `Frame::data(_)` then `Frame::trailers(_)`.
  3. Plumb trailers through each `Bridge` impl
     (`H1ToH2Bridge::bridge_request`, etc.).
  4. Flip the assertions in `trailer_passthrough.rs` to assert
     trailers ARE preserved.

The H1↔H1 path is already trailer-safe via hyper's `IncomingBody`
round-trip — that path proxies the body as-is and hyper's frame
loop preserves trailers automatically.

### PROTO-2-04 / PROTO-2-05 — wstest + h3spec integration (deferred to Wave 2c CI image)

Both require CI image changes (installing `wstest`, `h3spec`); they
move with the rel-team CI image work in Wave-2c. No code change
attempted in Wave-2b.

### PROTO-2-09 / PROTO-2-11 (H2 half) — `build_listener_mode` strict-protocol-validation, GOAWAY-on-SIGTERM

Both live in `crates/lb/src/main.rs` (forbidden to Wave-2b). Move
with Wave-2c.
