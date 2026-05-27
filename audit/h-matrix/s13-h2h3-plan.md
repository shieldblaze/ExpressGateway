# S13 — H2→H3 BUILD PLAN (builder-1; authored S12, read-only)

> **S13 is a SHORT session.** Every load-bearing piece of H2→H3 is already
> proven elsewhere — the build is a re-target + a header-transform swap, not a
> green-field cell. Specifically:
> - The **M-C QUIC connector** (`lb_quic::stream_request_to_h3_upstream` +
>   `H3RespOut::Decoded`) is proven TWICE: by **H1→H3** (S12, BUILT 9/9) and by
>   the **H3→H3 `Wire` path** (e2e 20/20). S13 reuses it VERBATIM — zero connector
>   code.
> - The **H2 M-D ingress** (lookahead window → detached pump → terminal-state
>   verdict) is proven by **H2→H1** and **H2→H2** (S10/S11). S13 re-targets its
>   sink, not its logic.
> - The **H2 `ProxyService` `Box<dyn Error>` widening** (R3 COMMIT-A) is already
>   landed and re-confirmed on h1h1/h2h1/h1h2/h2h2.
>
> With those three settled, the matrix closes **9/9** cleanly once H2→H3 lands.
> Two — and only two — points are genuinely net-new; both are NAMED as explicit
> S13 verify targets below (§Hazards) so they are designed-for, not discovered
> mid-session.

Base: `audit/h-matrix/s12-phase1-h1h3-build-plan.md` (the H1→H3 build this mirrors)
and `audit/h-matrix/s12-report.md`. Honest-stop decision: the owner chose the
H3-front header-fix convergence (~3h) over a full H2→H3 build (~10-13h, two
net-new risks) this session; H2→H3 deferred to S13 with this ready plan.

H2→H3 = **H2 front → streaming H3 backend**, on the SAME shared connector
H1→H3 uses (`stream_request_to_h3_upstream` + `H3RespOut::Decoded`).

---

## 1. What I build (lb-l7 side only; the connector is refactor-eng's, unchanged)

Replace the buffering `proxy_h2_to_h3` (`h2_proxy.rs:2308-2335`) — which does
`collect_h2_request_to_h3_fieldlist` (`body.collect().await`, the R8 violation) →
buffering `request_h3_upstream` (whole-body `Vec<u8>`) → `h3_response_to_h2`
(buffering builder) — with a fully streaming both-legs relay, EXACTLY mirroring
the landed `proxy_h1_to_h3` (`h1_proxy.rs:1988-2259`). Dispatch is unchanged
(`h2_proxy.rs:1250`, `UpstreamProto::H3 => proxy_h2_to_h3`).

### Net-new vs reused breakdown

**REUSED ~verbatim (0 or near-0 new lines):**
- The **connector** itself (refactor-eng owns it; already landed + proven on
  H1→H3 and H3→H3 `Wire`). H2→H3 calls `stream_request_to_h3_upstream(headers,
  /*forward_req_trailers=*/true, addr, sni, pool, body_rx, sink)` exactly as
  H1→H3 does.
- The **`ReqBodyEvent` per-arm mapping logic** (Chunk / End{trailers} / Reset)
  from my H1→H3 pump (`h1_proxy.rs:2034-2146`): same over-cap →
  `MAX_REQUEST_BODY_BYTES` pre-data-413-vs-mid-body-Reset boundary, same
  forbidden-trailer → `Reset`, same truncation → `Reset`.
- The **`StreamBody`-from-`H3RespEvent` response relay skeleton**
  (`h1_proxy.rs:2175-2240`): Head→head-builder; Body→`Frame::data`;
  Trailers→`Frame::trailers`; End→clean finish; Reset→inject `Err(box)` (no
  clean terminator).
- The **F-CAP-1 surfacing** (pre-data 413 / pre-dial 502 as synthesized `Head`
  relayed verbatim; mid-body over-cap as `Reset`) — identical, same connector
  `inline()` paths.
- The **`MAX_REQUEST_BODY_BYTES` pump-side cap** (the connector caps the
  RESPONSE via the `Decoded` sink `cap`, not the request).

**NET-NEW lb-l7 (~250-300 lines total; my whole `proxy_h1_to_h3` was 272):**
- **(a) `build_h2_to_h3_fieldlist(&parts, sni) -> Vec<(String,String)>`** (~40
  lines): a head-only refactor of `collect_h2_request_to_h3_fieldlist`
  (`h2_proxy.rs:2756`) — drop the `body.collect()` + trailer capture (those now
  stream); preserve the `create_bridge(Http2, Http3).bridge_request` call +
  `:method`/`:path`/`:scheme`/`:authority` synthesis unchanged. Direct mirror of
  my `build_h1_to_h3_fieldlist`.
- **(b) H2-ingress → `ReqBodyEvent` pump** (~120 lines): take the H2 M-D ingress
  frame loop from `proxy_h2_to_h2_request` (`h2_proxy.rs:1977-2288` — the
  `is_data` / `is_trailers` / `None`-vs-`is_end_stream()` / `Some(Err)` arms +
  `validate_request_trailers` + over-cap), but emit `ReqBodyEvent::Chunk` /
  `End{trailers}` / `Reset` into the connector's `Receiver<ReqBodyEvent>` INSTEAD
  of `Frame<Bytes>` into a hyper `Http2Pool` `H2ReqBody` channel. The per-arm
  mapping is identical to my H1→H3 pump; the one source delta is H2's
  `None`-is-ambiguous (`is_end_stream()` disambiguation) vs H1's `None` = clean
  end.
- **(c) H2-response `H3RespEvent` consumer** (~90 lines): my H1→H3 relay
  skeleton + an H2 response head builder. **NOT** `upstream_h2_response_to_h2`
  (`h2_proxy.rs:2549`) — that consumes a hyper `IncomingBody`; H2→H3's body is
  the decoded `H3RespEvent` stream. Two deltas vs my H1→H3 relay: (i) header
  transform uses **H2→H2 semantics** (drop `:`-pseudo only, NO hop-by-hop strip —
  per `upstream_h2_response_to_h2:2558-2567`), vs my H1 pseudo + `RESPONSE_HOP_BY_HOP`
  strip; (ii) the `Trailers` event maps to an H2 `Frame::trailers` that hyper's
  H2 server encoder flushes NATIVELY (no `Trailer:` pre-declaration — the
  CF-RESP-1 H1 constraint is GONE on the H2 front).
- **(d) Request-leg cancel-survival wiring** (~30-50 lines + a connector-contract
  check): the ONE nontrivial bit — see §Hazard (a).

### Build wiring (locked to real symbols, mirror of H1→H3)
```rust
use lb_quic::{H3RespEvent, H3RespOut, stream_request_to_h3_upstream};
use lb_quic::h3_bridge::{H3_BODY_CHUNK_MAX, ReqBodyEvent, H3_RESP_CHANNEL_DEPTH};
// head-only field list (build_h2_to_h3_fieldlist — NO body.collect)
let (body_tx, body_rx) = mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
let (resp_tx, mut resp_rx) = mpsc::channel::<H3RespEvent>(H3_RESP_CHANNEL_DEPTH);
let sink = H3RespOut::Decoded { tx: resp_tx, total: 0, cap: MAX_RESPONSE_BODY_BYTES };
// spawn the H2 M-D ingress pump → body_tx (Chunk / End{trailers} / Reset)
// spawn/await: stream_request_to_h3_upstream(headers, true, addr, sni, pool, body_rx, sink)
// drive resp_rx: Head→H2 head-builder; Body→StreamBody; Trailers→terminal Frame; End→finalize; Reset→Err(box)
```

---

## 2. THE TWO NAMED HAZARDS — explicit S13 verify targets

These are the only genuinely net-new risk points. They are named here so S13
designs for them from the first author pass rather than discovering them mid-gate.

### Hazard (a) — REQUEST CANCEL-RACE (request-leg F-MD-4)

A downstream **H2 client `RST_STREAM`** cancels the gateway's H2 ingress SERVICE
future. If that cancel drops the pump before it emits `ReqBodyEvent::Reset`, the
connector could see a clean `body_tx` close and (depending on its contract) FIN
the upstream QUIC stream — smuggling a TRUNCATED request to the H3 backend as
COMPLETE.

**Why this is NOT the H2→H2 graceful-drop trap** ([[h2-upstream-graceful-drop-trap]]):
that trap (detached send task + `reset_peer` + `tx.closed().await`) is SPECIFIC
to the `Http2Pool` egress — hyper finalizing a clean `END_STREAM` on cancel.
H2→H3 does NOT egress via `Http2Pool`; it egresses via the QUIC connector, whose
`ReqBodyEvent::Reset → RESET-without-FIN` is the SAME mechanism my H1→H3 pump
uses (no `Http2Pool`, no `reset_peer`). So the *fix shape* differs: the risk is
not hyper-finalizes-END_STREAM, it is the-pump-task-dies-before-emitting-Reset.

**Mitigation to build:** own the connector-drive + pump in a structure that
SURVIVES the downstream H2 cancel (detach the pump so a service-future cancel
only drops the caller's `resp_rx` receive, not the pump that owns the inbound
body — analogous to the H1→H3 detached pump + `connector_handle`). The pump
emits `ReqBodyEvent::Reset` on a positively-confirmed non-`END_STREAM` end.

**S13 verify (R13 a/b/c) — REQUIRED:**
- (a) in-gate: downstream H2 client RSTs mid-body → H3 backend sees
  `complete == 0` (RESET-without-FIN), NEVER a truncated-as-complete request.
- (b) isolation-burst ≥50× on `current_thread` (the H2→H2 smuggle was
  intermittent ~25-50% single-threaded — see [[parallel-gate-masks-smuggle]];
  budget the same hardening: `current_thread` + repetition loop + load-bearing
  negative control).
- (c) load-bearing negative control: a CLEAN H2 upload → `complete == 1`; the
  RST upload → `complete == 0` — gate on the QUIC FIN the backend observes.

### Hazard (b) — RESPONSE-LEG LOAD-BEARING ARM (response-leg F-MD-4)

The response-leg guard mechanism is CONFIRMED reused: `ClientRespBody =
BoxBody<Bytes, Box<dyn Error+Send+Sync>>` (`h1_proxy.rs:99`) is the SAME type the
H2 listener serves via hyper `http2::Builder::serve_connection`
(`h2_proxy.rs:777,790`). On `H3RespEvent::Reset` the relay injects `Err(box)`
into the `StreamBody` → hyper's H2 SERVER encoder emits **`RST_STREAM`**
downstream (vs the H1 front's missing `0\r\n\r\n`). Same injection I built for
H1→H3; different wire manifestation. No `reset_peer` on the response leg — that
is an `Http2Pool`-request-leg concern only.

**Apply the #13b CL-masking lesson** ([[h1h3-fmd4-teardown-not-reset]], the S12
#13b finding): a content-length-declared truncation arm is NON-load-bearing —
hyper's H2 CLIENT detects the CL-underrun ON ITS OWN and errors even with the
guard deleted, MASKING the guard. So:

**S13 verify (R13 a/b/c) — REQUIRED, guard-as-sole-discriminator from the first
author pass:**
- The response-truncation test's discriminator MUST be the GUARD: a truncating
  H3 backend that sends partial DATA then drops the stream no-FIN, framed so the
  ONLY signal is the guard. Assert the **H2 client sees `RST_STREAM` / an
  errored body**, NOT a clean `END_STREAM`. Predicate: any clean-terminated body
  on an aborted stream == a smuggled-complete leak == test FAILS.
- A CL-declared arm MAY be kept as additional CL-path coverage, but it is NOT the
  load-bearing arm and must be labeled non-load-bearing (exactly as #13b did).
- (b) ≥50-iter `current_thread` burst on the load-bearing arm; (c) clean
  response → `Ok`/`END_STREAM` non-vacuity control.
- **S13 verifier MUST run the guard-deletion mutation** against the load-bearing
  arm (delete the `Reset → Err(box)` injection): it must FAIL (leak). This is the
  in-suite load-bearing re-confirmation the #13b CL arm lacked.

---

## 3. CONNECTOR-CONTRACT PREREQUISITE (confirm at S13 START)

The single design question to nail before building Hazard (a): **does the
connector treat `body_tx` dropped WITHOUT a final `ReqBodyEvent::End` as a
`Reset` (RESET-without-FIN), or does it FIN the upstream?**
- If **dropped == Reset**: the cancel-race is largely defended by construction
  (a dying pump → dropped `body_tx` → connector RESETs) and the mitigation is
  lighter.
- If **dropped == FIN** (or unspecified): the pump MUST emit an explicit
  `ReqBodyEvent::Reset` before it can die, and the detached-pump survival wiring
  is load-bearing.

Confirm with refactor-eng (connector owner) as the first S13 action; it picks the
exact shape of Hazard-(a) mitigation. (My H1→H3 pump always emits an explicit
terminal event — `End` or `Reset` — so H1→H3 never relied on the drop semantics;
H2→H3 under downstream-cancel is the first path that might.)

---

## 4. TRAILER-MANDATE WIN — fully met downstream

The H2 front carries trailers **NATIVELY**. The connector emits
`H3RespEvent::Trailers`; the relay maps it to an H2 `Frame::trailers`; hyper's H2
server encoder flushes it to the downstream H2 client WITHOUT any `Trailer:`
pre-declaration. So:
- The trailer mandate is **FULLY MET downstream** — **NO CASE-ii**, **NO owner
  escalation**, unlike H1→H3 (whose hyper-1 H1 encoder drops streamed trailers,
  [[streamed-response-drops-trailers]] / CF-RESP-1).
- **H2→H3 is THE gRPC-capable →H3 cell** (a real gRPC client speaks H2/H3). The
  BUILT bar **ASSERTS** `grpc-status` REACHES the H2 client (a positive
  assertion, NOT the empirical-only record H1→H3's BUILT-bar #7 used).

---

## 5. BUILT bar (verifier; same shape as S11/H1→H3) + estimate

Current live H2→H3 coverage is only `proto_translation_e2e.rs::proxy_h2_listener_h3_backend`
(:573) + `bridging_h2_h3.rs::test_bridge_h2_to_h3` — neither exercises
streaming/trailer/byte-identity/abort. S13 adds a new suite (mirror
`tests/h1h3_md_streaming_verify.rs`, ~1300-1500 lines; my H1→H3 suite was 1444):

1. **Binary body byte-identical BOTH directions** (request → H3 backend; response
   → H2 client), non-trivial size (~5 MiB, crosses `H3_BODY_CHUNK_MAX` many×).
2. **Request-trailer FORWARDING** — H2 request trailers → H3 backend receives the
   post-DATA HEADERS section (`forward_req_trailers=true`).
3. **F-MD-4 REQUEST R13 a/b/c** — including **Hazard (a)** the downstream-RST
   cancel-race arm (H2-specific). Gate on the backend's QUIC FIN.
4. **F-MD-4 RESPONSE R13 a/b/c** — **Hazard (b)** the guard-as-sole-discriminator
   load-bearing arm: assert the H2 client sees `RST_STREAM`/errored body, NOT a
   clean `END_STREAM`. Guard-deletion mutation must FAIL.
5. **F-CAP-1** — pre-data over-cap → 413 (synthesized Head); mid-body over-cap →
   `RST_STREAM` (NOT 413, NOT clean END_STREAM); pre-dial down → 502. (No
   request-leg 400/504 path — malformed req-trailers → `Reset` in the pump;
   idle-timeout → response-leg 502/Reset via F-S7-6.)
6. **Trailer mandate gRPC-shaped (POSITIVE assertion)** — H3 backend returns
   HEADERS, DATA*, TRAILERS(`grpc-status`); ASSERT `grpc-status` REACHES the H2
   client (the win — fully met, not empirical).
7. **Memory gauge** non-vacuous (in-flight ≤ bounded window, body-size
   independent) + load-bearing inverted probe.
8. **Backpressure both legs** (slow H2 client backpressures H3 upstream; slow H3
   upstream backpressures H2 ingest).
9. **Coverage ≥80% session sub-metric** (scoped llvm-cov per
   [[llvm-cov-session-scope-method]]; `--workspace` for the lb-quic/lb-l7
   dep-crate lines per [[llvm-cov-workspace-for-depcrate-lines]]). fmt/clippy
   **workspace-wide** ([[cross-crate-gate-scope]] — spans lb-l7 + lb-quic). R3
   intact. R13 burst ×3 (apply CF-SATURATION-1: generous test-backend timeouts so
   8-core gate saturation cannot false-flake).

### Honest hour estimate
- **Build** (lb-l7 rewire + `build_h2_to_h3_fieldlist` refactor + cancel-survival
  wiring): **3-4h**.
- **Test suite author + green** (the H2 mock-backend RST/reset harness mechanics
  + the two load-bearing arms are where time goes — Hazard (b) needs the guard
  load-bearing on the FIRST pass per the #13b lesson, and Hazard (a) is genuinely
  new): **4-5h**.
- **Independent verify** (author≠verifier; full `--workspace --all-features` gate
  ×3 + mutation proofs on BOTH F-MD-4 arms): **3-4h**.
- **TOTAL ≈ 10-13h build+verify.** Lower-risk than H1→H3 was (connector proven
  twice, response-leg guard mechanism proven, trailer mandate trivially met), but
  NOT small — the two named hazards are real. With this plan in hand and the
  connector-contract check done first, S13 should close the matrix 9/9 cleanly.

## Risks carried into S13
- HAZARD (a) request cancel-race — §2(a), R13-verified.
- HAZARD (b) response-leg load-bearing arm — §2(b), guard-as-sole-discriminator +
  guard-deletion mutation.
- Connector-contract (drop==Reset?) — §3, confirm at S13 start.
- CF-BODY-WALLCLOCK ([[cf-body-wallclock]]): the H2 INGRESS side carries the
  wall-clock `timeouts.body` class (owner-deferred to S13); the H3 EGRESS uses the
  F-S7-6 idle deadline (no wall-clock truncation). Give over-cap/large-upload
  tests generous listener body timeout + client wait (CF-SATURATION-1).
