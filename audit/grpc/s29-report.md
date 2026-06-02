# S29 — gRPC-over-HTTP/3 conformance — REPORT

**Verdict:** SESSION 29 COMPLETE — gRPC-over-H3 verified, 2 findings
(1 fixed, 0 carried, 1 deliberate-deviation); **PROTOCOL SPEC COMPLETE.**

Branch `feature/grpc-h3-s29` (base `eee25093`). gRPC-over-HTTP/3 is the
program's last spec item.

## 1. The model + the proxy path

The gateway proxies gRPC **opaquely** over HTTP/3 (R7): it forwards the
length-prefixed gRPC messages + the `grpc-status`/`grpc-message` trailing
HEADERS end-to-end and does NOT interpret/transcode gRPC. gRPC over H3 is
**de-facto, not separately standardized** — the gRPC HTTP/2 application
semantics ride unchanged over the H3 transport (RFC 9114 §1/§4.1 preserve
HTTP semantics over QUIC). The realistic deployment is **H3 edge → H2 gRPC
backend**; H3→H3 is also valid.

**Key result — gRPC-over-H3 needed essentially zero new production code.**
The existing `quiche::h3` H3-terminate machinery already (a) forwards the
trailing `grpc-status` HEADERS end-to-end, (b) does concurrent bidirectional
streaming, (c) preserves the Trailers-Only shape, and (d) relays the framed
messages opaquely. A gRPC request is an ordinary H3 `POST` with
`content-type: application/grpc` + trailers — the H3 proxy already handles
it. So this session is **conformance characterization** that found a real
bug, not a greenfield build. The only production change is the F-S29-1 fix
(below); everything else is tests + the soak scenario.

## 2. Real-binary e2e — all four call types + trailer preservation

`crates/lb-quic/tests/grpc_h3_e2e.rs` (16 tests): a real quiche H3 client
speaking the gRPC wire format → production `QuicListener` (H3-terminate,
`quiche::h3`) → real hyper H2 gRPC backend. Reuses `lb_grpc`'s frame codec
(new dev-dep on lb-quic; leaf crate, no cycle, prod links zero gRPC
symbols). No `tonic`/`prost`. No `#[ignore]`.

| Test | Asserts |
|---|---|
| `grpc_h3_unary_echo_delivers_status_trailer` | unary: echo + `grpc-status:0` trailer + content-type to backend |
| `grpc_h3_server_stream_delivers_all_messages_and_trailer` | server-stream: 16 msgs + trailer |
| `grpc_h3_client_stream_relays_all_request_messages` | client-stream: N req msgs concatenated + trailer |
| `grpc_h3_bidi_stream_echoes_in_order` | bidi: 8 msgs order-preserved + trailer |
| `grpc_h3_trailers_only_immediate_error_preserved` | A6: single HEADERS+FIN, `:status`+`grpc-status:9`, no DATA |
| `grpc_h3_backend_reset_midresponse_not_laundered_to_clean_status` | B2: mid-response reset NOT laundered to `grpc-status:0` |
| `grpc_h3_without_te_header_still_delivers_trailer` | B3: no `te` required on H3 |
| `grpc_h3_grpc_timeout_forwarded_unclamped` | C1/D1: `grpc-timeout` forwarded `600S` verbatim (H2 clamps) |
| `grpc_h3_health_check_forwarded_not_synthesized` | D2: health forwarded to backend (no synth) |
| `grpc_h3_to_h1_backend_trailer_characterization` | B5/F-S29-2: H1 backend ⇒ no `grpc-status` to client |
| `grpc_h3_large_message_roundtrips_byte_identical` | A3: 512 KiB msg byte-identical + trailer (F-S29-1 regression) |
| `grpc_h3_trailer_survives_all_response_sizes` | F-S29-1: trailer at 1 B…1 MiB |
| `grpc_h3_trailer_survives_any_frame_granularity` | F-S29-1: trailer for giant + small backend frames |
| `grpc_h3_producer_trailer_attribution` | producer (`stream_h2_response`+hyper) delivers Trailers+End |
| `grpc_h3_server_stream_bounded_memory_r8` | R8: ~1 MiB server-stream bounded (gauge ≤ ceiling) |
| `grpc_h3_burst_50_unary_cycles` | R13: 50 unary cycles, each `grpc-status:0` + FIN |

16/16 green (sequential 5.45 s). fmt + clippy `--all-targets --all-features
-D warnings` clean.

## 3. F-S29-1 (FIXED) — large H3 response dropped the trailing `grpc-status`

The headline finding — a **general H3-egress bug, gRPC-fatal**, that the
conformance pass exposed.

- **Symptom:** a response whose body exceeds ~448 KiB (threshold scales with
  upstream frame size) delivered the full body but **dropped the trailing
  `grpc-status`/`grpc-message` HEADERS + FIN** — the stream hung (no trailer,
  no FIN, no reset). gRPC clients never see a status ⇒ the RPC is broken.
- **Attribution (proven, R2):** the producer (`stream_h2_response` + hyper
  H2 client) delivers `Trailers`+`End` correctly for all sizes with no
  backpressure (`grpc_h3_producer_trailer_attribution` 4/4) — the loss is in
  the actor egress under backpressure. An instrumented actor trace on the
  512 KiB case showed: `pull Trailers → emit Trailers OK` (the `StreamTx` is
  removed by `retain`) → a fresh `StreamTx` re-created by
  `drain_resp_channels`' `entry().or_insert_with()` → it replays the leftover
  `End`/`Disconnected` and fires a spurious `RESET` (`stream_shutdown`) that
  **discarded the still-buffered trailer+FIN**. Small responses raced clear
  before the reset; large ones lost the trailer. Also caused a post-response
  2 ms busy-loop.
- **Fix** (`conn_actor.rs::drain_resp_channels`, 20/7 lines): `get_mut`
  instead of `entry().or_insert_with()` — a missing `StreamTx` means the
  stream already terminated correctly, so drop the stale receiver and skip.
  No respawn, no spurious reset, no busy-loop. Single-sourced in the shared
  H3 response egress ⇒ fixes H3→H1/H2/H3 alike (R12).
- **Verification:** the size sweep delivers `grpc-status:0` + clean FIN at
  every size 1 B…1 MiB and for giant + small backend frames; the full
  `lb-quic --all-features` suite re-verified **33 suites, 0 failed** (every
  H3 cell + Mode-B + WS path intact, R3/R12).

Tier: CORRECTNESS / trailer-preservation (R4 — never asterisked). It is
NOT gRPC-specific — gRPC merely exposed it because gRPC *always* ends with a
trailer; any large H3-terminate response with trailers was affected.

**Independent fix audit (author ≠ verifier).** A separate review of the fix
+ tests confirmed: (a) the fix is correct and complete — `get_mut → None`
is reachable ONLY post-wire-terminal (the two-map insert sites are
synchronous/atomic and the only `stream_response` removers fire after the
terminal frame is on the wire), so it can never drop a receiver for a live
stream / truncate a response; the fast-tick gate disengages correctly
(resolving the busy-loop); the post-FIN `ClientGone` is harmless. (b) The
regression tests are genuinely load-bearing — the H2 backend emits
`grpc-status` ONLY in the trailer (never the head) and the client routes a
field to `trailer_pairs` only when a SECOND HEADERS frame actually decoded,
so `field("grpc-status") == Some("0")` is unreachable unless the trailer
arrived; under the bug, the reset → no 2nd HEADERS → assertion fails. None
vacuous.

## 4. F-S29-2 (deliberate deviation, LOW) — gRPC over an H1 backend

gRPC mandates HTTP/2+; an H1 origin cannot reliably carry the always-present
trailing-`grpc-status` HEADERS. `grpc_h3_to_h1_*` confirms an
H3-front→H1-backend gRPC call returns `200` but no `grpc-status` reaches the
client. **Resolution (industry-standard, matches the H2 path's "gRPC
REQUIRES HTTP/2"):** document that gRPC requires an H2/H3 backend; no code
change (the opaque H3 front is protocol-blind by design — a content-type
guard would add gRPC-awareness the model explicitly avoids, R7). v1
release-note limitation.

## 5. Tier-D divergence (owner-ratified) + the gated-vs-default decision

The H2 path has a full gRPC-**aware** proxy (`grpc_proxy.rs`): `grpc-timeout`
clamp + `DEADLINE_EXCEEDED`, `/Health/Check` synthesis, HTTP→grpc-status
synthesis, `te: trailers` enforcement. H3 has none — gRPC flows opaquely.
Per the conformance analysis these (D1/D2/D3) are **value-add, NOT
conformance requirements** of a transport proxy, so the opaque H3 path is
conformant and the H2-vs-H3 gap is a deliberate, ratifiable feature
distinction (H2 = gRPC-aware proxy; H3 = opaque transport proxy). Verified:
`grpc-timeout` forwarded unclamped (`600S`), health forwarded to backend.

**Gated-vs-default → NO gate.** Unlike WS-over-H3 (a brand-new validated
path + an advertised `SETTINGS` capability, which warranted a newness gate),
gRPC over H3 adds **no new request-handling path** — the only change is the
F-S29-1 bug fix to the existing egress. So `grpc_enabled` would gate
nothing; gRPC-over-H3 is on by virtue of normal H3 proxying. No
interpret/transcode fork arose.

## 6. R8 (bounded streaming) + R13 (burst) + reset/trailer negative control

- **R8:** `grpc_h3_server_stream_bounded_memory_r8` — a ~1 MiB / 512-message
  gRPC server-stream keeps the H3 response egress bounded:
  `MAX_RETAINED_RESP_BYTES` peaked at **73 856 B**, well under the
  `4×depth×(chunk+hdr)` = **262 656 B** ceiling, with total (1 054 098 B) >>
  ceiling so the bound is genuinely exercised (non-vacuous) and the gauge
  > 0. Message-volume-independent.
- **R13:** `grpc_h3_burst_50_unary_cycles` — 50 sequential unary cycles
  (fresh H3 conn each), every one delivering `grpc-status:0` + FIN. The
  load-bearing **negative control** is the B2 reset test (a mid-response
  reset must NOT be laundered into a clean `grpc-status:0`).

## 7. gRPC-over-H3 soak (`sc9_grpc_h3`)

A backend-proxied gRPC-over-H3 soak (`lb-soak`): quiche::h3 client → H3
gateway → H2 gRPC echo backend, sustained + churn, verifying `grpc-status:0`
+ echo per RPC in-client (a trailer-drop regression under load ⇒ `err()`).
Unlike sc7's BACKENDLESS inline-400 H3 front, this drives the REAL
backend-proxied response-trailer path the F-S29-1 fix corrected.

**Definitive 900 s run (181 samples):** `grpc_h3_sustained ok=682834 err=0`
+ `grpc_h3_churn ok=499572 err=0` = **1 182 406 RPCs, ZERO errors**,
`grpc-status:0` + the echoed message verified in-client throughout. **NO
LEAK:** `fds` BOUNDED (flat 12 — the binding conn/stream-leak discriminant
per this program's methodology), `threads` BOUNDED (9), `panic_total`
BOUNDED (0).

**RSS — bounded (plateau); the analyzer's `DRIFT` is the warmup ramp.** RSS
rose from the 19.6 MB boot to a ~46 MB steady state by t≈500 s, then stayed
flat: t=550→900 s is 46.0–46.4 MB across **350 s + ~500 K more RPCs adding
+0.4 MB** (noise). So there is no per-RPC leak — a leak would keep RSS
climbing with RPC count; instead it plateaus. The analyzer flags `DRIFT`
because the slow 19→46 MB *warmup ramp* (pools/arenas/peak-concurrency
HashMap high-water filling) sits in the first-vs-last-third comparison
window of a low-baseline gauge — the documented "vmhwm is a peak gauge not a
leak" / low-baseline false-positive class. `fds` is the conn-leak signal and
it is BOUNDED. (The shared analyzer is deliberately NOT weakened — the
`DRIFT` is characterized, not suppressed.) 46 MB steady-state for an L7
gateway under 1.18 M RPCs is healthy. A supplementary 600 s run agreed (one
transient 502 under churn out of 333 K — the gateway correctly signalling a
backend hiccup, not a trailer drop).

Verdict: the F-S29-1 fix holds under sustained load + churn — no fd / stream
/ thread leak, no busy-loop (RSS flat in steady state), no crash, trailers
delivered for every one of 1.18 M RPCs.

## 8. Phase 3 gates

- **Binding ×3 `--workspace --all-features`: CLEAN GREEN ×3** (all 3 runs
  exit 0, 0 failed; `grpc_h3_e2e` 16/16 in every run; disk stable 4.9 GB).
  An independent verifier ran a first ×3 (runs 1&2 fully green 1512/0/18;
  run 3 had 2 grpc_h3_e2e 502 saturation flakes — proven by solo-pass +
  runs-1&2-green + the 502 signature). Those 2 new tests were
  saturation-fragile because this binary ran 16 in-process gateways
  concurrently; fixed by serializing the binary (`SUITE_SERIAL`, one gateway
  at a time — matching the non-flaking single-gateway H3-cell binaries), no
  assertion weakened. The fresh post-serialization ×3 is clean.
- clippy `--all-targets --all-features -D warnings` + fmt --check: clean.
- **Scoped llvm-cov** (the changed egress): the changed function
  `conn_actor::drain_resp_channels` (the F-S29-1 fix) = **85.5% (47/55
  executable lines) ≥ 80%**, and the fix's load-bearing lines (the `get_mut`
  + the stale-receiver `remove`+`continue`) are covered. Measured via
  `cargo llvm-cov -p lb-quic --all-features --test grpc_h3_e2e` (the gRPC
  suite drives the egress on every response, incl. the B2 reset + B5 paths);
  the 8 uncovered lines are pre-existing guard/gauge branches the gRPC-only
  suite does not exercise (whole-file conn_actor 51.4% is diluted by the
  WS/request/graceful-close paths — per the session-scope method the
  changed-function metric governs). lcov: `audit/grpc/s29-cov.lcov`.
- The pre-existing `h2h3_backpressure` saturation flake did NOT fire in
  either ×3 (CF-SATURATION-1 class; isolation-proven 8/8 un-saturated).

## 9. PROGRAM STATE — PROTOCOL SPEC COMPLETE

After S29 the protocol spec is complete:
- the 9-cell H-matrix (H1/H2/H3 × H1/H2/H3),
- both QUIC modes (A passthrough, B native proxy),
- the full WebSocket matrix (WS over H1/H2/H3, RFC 6455/8441/9220),
- **gRPC-over-H3** (this session),
- on `quiche::h3` (the S23→S26 migration),
- stress-tested (the lb-soak suite) + conformance-hardened (h2spec, h3spec,
  the WS RFC passes, and now the gRPC-over-H3 pass).

### Remaining — HARDENING workstreams, NOT spec gaps
- **CF-S27-2** — WS-over-H2 window-aware backpressure (genuinely large;
  until it lands WS-H2 stays gated).
- **CF-QUICHE-UPGRADE** — h3spec #1-10 + #23/#25 + §7.1 (quiche-0.28 limits).
- **CF-S28-WSH3-WAKEUP** — the 2 ms busy-poll on a live WS-H3 tunnel
  (bounded, not a leak). _Note: the F-S29-1 fix removed the analogous
  post-RESPONSE busy-loop on the H3 egress; the WS-tunnel poll is separate
  (`pump_ws_tunnels`)._
- **CF-FCAP1-FLAKE / CF-SATURATION-1** — the pre-existing H2-timeout
  saturation flake (isolation-proven).
- **CF-DEP-1** — Dependabot (owner).
- **F-ESC-1** (multi-kernel CI lane), **N-1** (jumbo-MTU), Mode-A deferred
  perf tiers + **CF-S15-PASSTHROUGH-RETRY-ODCID** (v1 release-note item),
  **F-S29-2** (gRPC requires an H2/H3 backend — v1 release-note).

None of the above is a spec gap.
