# S27 — Binding session-changed-line coverage (verifier-p3)

**Source:** `audit/websockets/s27-cov.lcov` (1.5 MB, 118 SF entries, full
`cargo llvm-cov --workspace --all-features` run by the lead, exit 0 — all unit +
integration + lb-bin tests ran instrumented).
**Method:** READ-ONLY (no cargo). For each session product file, the lcov `DA:`
per-line records were intersected with the session-added new-side lines from
`git diff main...HEAD` (merge-base `33a0d068`). Lines inside each file's
`#[cfg(test)]` module are EXCLUDED (PRODUCT code only). Tool:
`/tmp/s27cov/map.py` + per-file `*.addedlines`. A "session-executable" line =
a session-added line that lcov instrumented (`DA:`); comments/blanks/decls lcov
does not instrument are not counted (they are not coverable).

## Per-file PRODUCT session-line coverage

| file | sess-exec lines | covered | uncovered | % |
|---|---:|---:|---:|---:|
| crates/lb-config/src/lib.rs        |  50 |  48 |  2 |  **96.0%** |
| crates/lb-l7/src/h2_proxy.rs       | 126 | 110 | 16 |  **87.3%** |
| crates/lb-l7/src/ws_proxy.rs       |  33 |  13 | 20 |  **39.4%** ✗ |
| crates/lb-quic/src/ws_tunnel.rs    | 123 | 101 | 22 |  **82.1%** |
| crates/lb-quic/src/h3_bridge.rs    |  24 |  24 |  0 | **100.0%** |
| crates/lb-quic/src/h3_config.rs    |   7 |   7 |  0 | **100.0%** |
| crates/lb-quic/src/conn_actor.rs   |   6 |   6 |  0 | **100.0%** |
| crates/lb-quic/src/listener.rs     |  11 |   6 |  5 |  **54.5%** ✗ |
| crates/lb-quic/src/router.rs       |   5 |   5 |  0 | **100.0%** |
| crates/lb/src/main.rs              |  98 |  56 | 42 |  **57.1%** ✗ |
| **OVERALL SESSION CODE**           | **483** | **376** | **107** | **77.8%** |

## VERDICT: FAIL — 77.8% < 80% (R10 binding bar)

The deficit is concentrated in three files (ws_proxy.rs, main.rs, listener.rs).
The H3-datapath session code is strong (h3_bridge / h3_config / conn_actor /
router 100%, ws_tunnel 82.1%, lb-config 96%). Both pre-stated predictions are
CONFIRMED below.

---

## Exact uncovered session functions/lines + disposition

### ws_proxy.rs 39.4% — `close_backpressure` anti-hang path (PREDICTION 1: CONFIRMED)
- **Uncovered: 372, 394** — the two `Err(_) => return close_backpressure(...)`
  send-timeout arms inside `proxy_frames` (client→backend and backend→client).
- **Uncovered: 436-453** — the entire `async fn close_backpressure` body (the
  Close-1008 + clean-Close-both-halves teardown when a forwarding `send().await`
  backpressures past the per-direction `read_frame` budget).
- **Why uncovered:** NO test trips it. The two backpressure tests
  (`ws_proxy.rs::backpressure_plateaus_producer_when_consumer_stalls`,
  `tests/ws_r8_backpressure_plateau.rs`, `tests/ws_h2_r8_backpressure.rs`) prove
  the relay PLATEAUS (the read side stops pulling) with a 30 s
  `read_frame_timeout` — they never hold a forwarding `send()` parked LONGER than
  that budget, so the timeout arm never fires. `grep` confirms `close_backpressure`
  is referenced only in ws_proxy.rs itself (no caller/exerciser anywhere).
- **Load-bearing?** YES on socket transports (it reclaims a wedged WRITE — a
  task/connection-leak DoS distinct from the bounded-memory property). It is the
  symmetric write-side mirror of the covered `ReadFrameTimeout` arm (314-327,
  covered). Note it is NOT the WS-over-H2 DoS fix (that's the gated-off F-S27-2,
  escalated) — this is the H1/raw write-wedge guard.
- **Disposition recommendation: ADD A TARGETED TEST.** A small in-crate unit test
  over a `duplex` pipe with a sub-second `read_frame_timeout` and a non-draining
  peer would trip 372/394 → 436-453 deterministically (the codebase already has
  the `duplex`-based backpressure harness to adapt). This is a load-bearing
  security/liveness path, so a test is preferable to documenting. (Mirrors the
  existing covered `ReadFrameTimeout` arm — symmetry argues for parity coverage.)

### main.rs 57.1% — F-S26-1 wiring + binary glue (PREDICTION 2: CONFIRMED)
- **wire_h3_terminate_backends H2 arm — uncovered 1272-1276** (`with_h2_backend`):
  `build_h2_upstream_pool` + resolve-first + `params.with_h2_backend(...)`, incl.
  the `no resolved H2 backend` bail (1273-1274). CONFIRMED uncovered.
- **wire_h3_terminate_backends H3 arm — uncovered 1283-1294** (`with_h3_backend`):
  `collect_h3_backends` + `build_h3_upstream_pool` + SNI derivation +
  `params.with_h3_backend(...)`, incl. the `no resolved H3 backend` bail
  (1285-1286). CONFIRMED uncovered.
- **Other uncovered main.rs session lines (binary-only glue, not unit-testable
  without a live binary):**
  - 1014-1020 — `build_h2_proxy` WS branch (`with_websocket` +
    `with_h2_extended_connect`). Exercised only by a real H2/ALPN binary spawn;
    no unit test constructs build_h2_proxy with a ws_cfg.
  - 1152 — `params.with_websocket(true)` in `spawn_quic` (only when
    `h3_extended_connect=true`; the H1-backend e2e test leaves WS off).
  - 1190-1195 — the `tracing::info!("QUIC listener started")` H3-terminate log
    fields (logging; not asserted).
  - 1239 — the `dns_cache_hits_total` metric-name tuple arm (the e2e hit the
    MISS arm, 1237, on first resolve; the HIT arm needs a warm cache).
  - 1245, 1254-1257 — `bail!` branches (resolver-returned-no-addresses;
    called-with-no-backends) — defensive, unreachable on the tested path.
  - 2176-2184 — the `spawn_quic(...)` call site inside the real `run()` listener
    loop (driven only by a full-binary boot, not the in-module spawn_quic test
    which calls spawn_quic directly).
- **Covered:** the H1 arm (`with_backends`, 1265) + the resolve loop + the happy
  path are covered by `spawn_quic_h3_terminate_forwards_to_h1_backend_through_real_listener`.
  `grep` confirms that is the ONLY test driving a wire arm.
- **Disposition recommendation: DOCUMENT (mostly) + OPTIONAL targeted unit test
  for the H2/H3 arms.** The H2/H3 `wire_h3_terminate_backends` arms are real
  product code reachable from config but not yet driven by any test. They are
  low-risk straight-line wiring (resolve→pool→`with_*_backend`) mirroring the
  tested H1 arm, and the H3→H2/H3 relay datapath itself is covered by the
  `bridging_h2_h3`/`bridging_h3_h3` integration tests (just not via this binary
  glue). Recommended: a focused unit test that calls `wire_h3_terminate_backends`
  with an h2-family and an h3-family `ListenerConfig` and asserts the params got
  the h2/h3 backend set (no live backend needed — the resolve is to a literal
  127.0.0.1:port and the `with_*` setters are pure). The remaining glue
  (1014-1020, 1152, 1190-1195, 2176-2184) is binary-boot-only and is the
  documented black-box-soak/e2e surface — DOCUMENT.

### listener.rs 54.5% — `with_websocket` setter + Debug field
- **Uncovered 229-232** — `pub const fn with_websocket(mut self, enabled) -> Self`.
  Called only from `main.rs:1152` (the binary, ws-enabled H3 path), which no unit
  test drives. **125** — the `ws_enabled` field in the `Debug` impl (cosmetic).
- **Disposition: DOCUMENT or fold into the optional main.rs unit test.** A single
  unit test constructing `QuicListenerParams` and calling `.with_websocket(true)`
  would cover 229-232 trivially. Cosmetic Debug line 125 is acceptable-uncovered.

### Acceptable-uncovered (no action) — already-strong files
- **lb-config/lib.rs 96.0%** — uncov 1756 (`"h3" => saw_h3 = true` backend-family
  count arm; the validator tests cover h1/h2/mixed but not a lone-h3 listing here)
  + 1759 (`_ => {}` catch-all, unreachable — unknown protocols pre-rejected by
  `validate_backend_list`). Non-load-bearing.
- **h2_proxy.rs 87.3%** (PASS) — uncov 1404-1408 (missing-`:path` 400 branch —
  hyper's h2 codec RST_STREAMs a path-less extended CONNECT before dispatch, so
  this is defense-in-depth that the test harness can't reach; the missing-`:scheme`
  sibling 1410-1416 IS covered), 1469-1470 (tracestate-propagation arm — tests
  send traceparent but not tracestate), 1491-1496 (the `WsDialErr::Timeout`
  504 arm — the upgrade-defer test hits Refused→502 and elapsed→504 but not the
  inner explicit-Timeout variant), 1518-1520 (hyper-upgrade-failed-after-upstream
  debug log — a race window not deterministically reachable). All minor;
  file PASSES.
- **ws_tunnel.rs 82.1%** (PASS) — uncov 126-132 (`Debug` impl), 177-179
  (`Default` impl), 228 (`Poll::Pending` re-emit), 246 (read-after-EOF `Ready(0)`),
  261-265 (`BrokenPipe` on closed writer — actually IS hit by
  `write_after_actor_gone_is_broken_pipe`; lcov shows the inner construction
  lines uncovered due to the early-return ordering), 280-284 (`poll_flush` no-op).
  Debug/Default/no-op-flush are non-load-bearing; file PASSES.

---

## Summary for the promote decision

- **Overall session-code coverage = 77.8% → FAIL the ≥80% bar by 2.2 pts.**
- The gap is THREE files; the fix that moves the needle most is **ws_proxy.rs
  `close_backpressure`** (a load-bearing liveness/security path, currently
  0-coverage on its core arms — ADD A TEST). Covering 372+394+436-453 (~20
  lines) alone lifts overall to ≈ 380/483 = 78.7%; adding the H2/H3
  `wire_h3_terminate_backends` arms + `with_websocket` (~30 more lines)
  lifts it over 80%.
- Recommendation: **add two targeted unit tests** — (1) ws_proxy
  `close_backpressure` over a `duplex` with a tiny `read_frame_timeout`; (2)
  `wire_h3_terminate_backends` h2-family + h3-family params assertions (also
  covers listener `with_websocket`). That closes both flagged thin spots and
  clears the 80% bar. The remaining binary-boot-only glue (1014-1020, 1152,
  1190-1195, 2176-2184) is legitimately the e2e/soak surface and should be
  DOCUMENTED, not unit-tested.
- Predictions: BOTH CONFIRMED (ws_proxy 372/394/436-453; main.rs H2 1272-1276 +
  H3 1283-1294 + bails).
