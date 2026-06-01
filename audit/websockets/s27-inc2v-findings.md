# SESSION 27 INC-2V — independent verification findings

Verifier (not the author of the code or the F-S27-1 fix). Branch
`feature/websockets-s27`. Fix under review: `83746d1c`.

## Verdicts at a glance

| Task | Verdict |
|---|---|
| A — no-regression + determinism (4 suites x3) | PASS |
| B — F-S27-1 CLOSED + regression load-bearing (independent revert) | CONFIRMED CLOSED |
| C — fix correctness; unbounded-hold question | CORRECT; no unbounded-hold finding |
| D — RFC 8441 conformance | PASS (1 PARTIAL: :scheme/:path test gap) |
| E — R8 (i) bounded memory / (ii) backpressure | (i) PASS / **(ii) FAIL → F-S27-2 BLOCKER** |
| F — R13 burst (60 cycles, fd leak) | PASS (zero fd leak, x3) |

## F-S27-1 — premature WS-over-H2 200 — CONFIRMED CLOSED (was HIGH)

The fix (`h2_proxy.rs:1318-1426`) makes `handle_ws_extended_connect` `async`,
dials + completes the upstream RFC 6455 handshake INLINE under
`tokio::time::timeout(self.timeouts.header, ...)` BEFORE any 200:
Refused→502 (1371-1377), Timeout→504 (1378-1384), elapsed→504 (1385-1391),
success→200 + splice-only spawn (1400-1425). Mirrors the accepted H1 sibling.

Independently load-bearing: I reverted ONLY `h2_proxy.rs` to the pre-fix
`e0e5a21b` (`git checkout e0e5a21b -- ...`), re-ran the UNCHANGED regression
suite `tests/ws_h2_upgrade_defer.rs` — all 3 went RED (client saw 200 where
502/502/504 required) — then restored the fix and they went GREEN. Working
tree left clean. Evidence: `s27-fs27-1-proof/inc2v-independent-loadbearing.txt`.

## F-S27-2 — WS relay does NOT backpressure a slow consumer — NEW, BLOCKER (HIGH, DoS)

The shared relay `ws_proxy::proxy_frames` buffers a fast producer's entire
stream in RAM, unbounded, when the consumer is slow/stalled. Proven real-wire:
a non-reading WS-over-H2 client lets a 256 MiB backend flood through to
completion; process RSS climbs to ~282 MiB. Attribution A/B (TINY 64 KiB vs
HUGE 256 MiB client window → identical throughput/memory) proves the
buffering is in the GATEWAY, not the client. Root cause:
`WsConfig::tungstenite_config()` (`ws_proxy.rs:130-136`) leaves tungstenite's
`max_write_buffer_size` at its default `usize::MAX` (unbounded), so the
gateway-side `client_tx.send().await` never parks and the relay never stops
reading the producer.

R12 SIBLING: identical defect on the H1 WS path — both H1
(`h1_proxy.rs:2648-2649`) and H2 (`h2_proxy.rs:1411-1412`) use the same
`server_ws`/`tungstenite_config()`/`proxy_frames`. Fix must be single-sourced
in `WsConfig::tungstenite_config()` (bound `max_write_buffer_size`, small
`write_buffer_size`) and re-verified on BOTH paths. NOT introduced by the
F-S27-1 fix (pre-existing in the shared relay) but in scope for SESSION 27 and
a blocker for any R8-bounded claim under hostile/asymmetric load.

Full proof + numbers + suggested fix: `s27-r8-ws-proof.md`. Reproducer (the
red (ii) + A/B): `s27-fs27-1-proof/ws_h2_r8_backpressure.rs.txt`. Console:
`s27-r8-console.txt`.

## Committed test files (all green; the red (ii) reproducer is NOT committed to tests/)

- `tests/ws_h2_upgrade_defer.rs` (author's; verified load-bearing) — F-S27-1.
- `tests/ws_h2_r8_backpressure.rs` (verifier; property (i) bounded memory only
  — the flooding (ii)/A/B were moved to audit `.rs.txt` because they pollute
  the process-wide VmHWM gauge under parallel run).
- `tests/ws_h2_burst.rs` (verifier) — R13 burst, 60 cycles, zero fd leak.

## Post-fix contract for F-S27-2's regression test (load-bearing target)

Using `ws_h2_r8_backpressure.rs.txt::r8_backpressure_slow_client_stalls_backend`:
with a non-reading client and a 256 MiB backend flood, post-fix the backend
`pushed` MUST plateau far below the flood (a small constant bounded by the
windows + bounded write buffer) and process RSS growth MUST stay bounded
(< tens of MiB). The A/B control must then show the TINY-window run throttled
well below the HUGE-window run (client flow control becomes the throttle).
Confirm catch by reverting the fix → the assertions go red exactly as today.
Verify on H1 and H2.
