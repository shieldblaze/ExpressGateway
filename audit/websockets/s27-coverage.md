# S27 — Binding session-changed-line coverage (verifier-p3)

**Source:** `audit/websockets/s27-cov.lcov` — full `cargo llvm-cov --workspace
--all-features` run (by the lead, exit 0, full unit + integration + lb-bin suite
instrumented). RE-MEASURED after builder-1's two coverage tests landed
(`4ee6e80d`).
**Method:** READ-ONLY (no cargo). For each session product file, the lcov `DA:`
per-line records were intersected with the session-added new-side lines from
`git diff main...HEAD` (merge-base `33a0d068`). Lines inside each file's
`#[cfg(test)]` module are EXCLUDED (PRODUCT code only). Tool:
`/tmp/s27cov/map.py` + per-file `*.addedlines`. A "session-executable" line =
a session-added line that lcov instrumented (`DA:`); comments/blanks/decls lcov
does not instrument are not counted (they are not coverable). The session
PRODUCT line ranges are identical across both runs (the new tests appended only
inside the existing `#[cfg(test)]` modules — ws_proxy test-mod still @527,
main.rs @3300 — so the measured denominators are unchanged).

## RUN 2 (current, gating) — after the 2 coverage tests @ `4ee6e80d`

| file | sess-exec lines | covered | uncovered | % |
|---|---:|---:|---:|---:|
| crates/lb-config/src/lib.rs        |  50 |  48 |  2 |  **96.0%** |
| crates/lb-l7/src/h2_proxy.rs       | 126 | 110 | 16 |  **87.3%** |
| crates/lb-l7/src/ws_proxy.rs       |  33 |  32 |  1 |  **97.0%** |
| crates/lb-quic/src/ws_tunnel.rs    | 123 | 101 | 22 |  **82.1%** |
| crates/lb-quic/src/h3_bridge.rs    |  24 |  24 |  0 | **100.0%** |
| crates/lb-quic/src/h3_config.rs    |   7 |   7 |  0 | **100.0%** |
| crates/lb-quic/src/conn_actor.rs   |   6 |   6 |  0 | **100.0%** |
| crates/lb-quic/src/listener.rs     |  11 |   6 |  5 |  **54.5%** |
| crates/lb-quic/src/router.rs       |   5 |   5 |  0 | **100.0%** |
| crates/lb/src/main.rs              |  98 |  66 | 32 |  **67.3%** |
| **OVERALL SESSION CODE**           | **483** | **405** | **78** | **83.9%** |

## VERDICT: PASS — 83.9% ≥ 80% (R10 binding bar)

The two targeted tests builder-1 added (`4ee6e80d`) closed both flagged thin
spots: ws_proxy went 39.4% → 97.0% and main.rs 57.1% → 67.3%, lifting overall
77.8% → **83.9% PASS**.

### Confirmation of the lead's three checkpoints (per-line DA from the new lcov)
- **ws_proxy `close_backpressure` — NOW HIT.** `DA:436=1, 437=1, 448=1, 452=1`
  (the whole fn body) and `DA:372=1` (the client→backend send-timeout arm). The
  new `close_backpressure_1008_on_forward_write_timeout` test trips it. Only
  `DA:394=0` remains — the SECOND (backend→client) send-timeout arm; it is the
  exact symmetric sibling of the now-covered 372 (`close_backpressure` itself is
  fully covered, so the teardown behaviour IS proven; 394 is just the other
  direction's call site).
- **main.rs `wire_h3_terminate_backends` H2 arm — NOW HIT.** `DA:1272=1, 1273=1,
  1276=1` (`build_h2_upstream_pool` + resolve + `with_h2_backend`). Only
  `DA:1274=0` (the `no resolved H2 backend` bail — unreachable with a real
  resolved addr).
- **main.rs `wire_h3_terminate_backends` H3 arm — NOW HIT.** `DA:1283=1, 1284=1,
  1285=1, 1288=1, 1292=1, 1293=1, 1294=1` (`collect_h3_backends` +
  `build_h3_upstream_pool` + `with_h3_backend`). Uncovered: `DA:1286=0` (the
  `no resolved H3 backend` bail) and `1289-1291` (the SNI-fallback
  `unwrap_or_else` closure — the test backend carries an explicit
  `tls_verify_hostname`, so the fallback body is not entered).
- **listener `with_websocket` (229-232) — STILL UNCOVERED** (`DA:229-232=0`). The
  `wire_h3_terminate_backends` tests dispatch the backend arm; they do NOT call
  `with_websocket(true)` (that is the separate H3-extended-CONNECT opt-in on
  main.rs:1152, also still uncovered). This is a one-line `const fn` setter +
  the `ws_enabled` Debug field — see disposition below.

---

## Remaining uncovered session lines (RUN 2) + disposition

### Still <80% per-file, but immaterial to the now-PASS overall

**main.rs 67.3%** — remaining uncovered are the genuinely binary-boot-only glue
+ a few defensive/fallback lines:
- **DOCUMENTED-ACCEPTABLE (binary-boot/e2e-only, per lead):** 1014-1020
  (`build_h2_proxy` WS branch — needs a real H2/ALPN spawn), 1152
  (`params.with_websocket(true)` — the H3-extended-CONNECT opt-in), 1190/1193-1195
  (the "QUIC listener started" H3-terminate log fields), 2176-2184 (the
  `spawn_quic(...)` call site inside the real `run()` loop).
- Minor defensive/fallback: 1239 (`dns_cache_hits_total` HIT-arm tuple — tests
  hit the MISS arm on first resolve), 1245/1254-1257 (resolver-no-addresses /
  called-with-no-backends `bail!`s — unreachable on the tested path), 1274/1286
  (the H2/H3 `no resolved backend` `bail!`s), 1289-1291 (SNI fallback closure —
  test backend has an explicit verify hostname).

**listener.rs 54.5%** — 229-232 (`pub const fn with_websocket`) + 125
(`ws_enabled` Debug field). The setter is only reached from the binary's
H3-extended-CONNECT opt-in (main.rs:1152, itself binary-boot-only).
- *Optional one-liner:* a unit test constructing `QuicListenerParams` and calling
  `.with_websocket(true)` would cover 229-232 trivially if the lead wants
  listener.rs itself over 80% — but it does not change the PASS verdict. The
  Debug field (125) is cosmetic, acceptable-uncovered.

### Acceptable-uncovered (no action) — already-PASS files
- **lb-config/lib.rs 96.0%** — 1756 (`"h3" => saw_h3` family-count arm), 1759
  (`_ => {}` catch-all, unreachable — unknown protocols pre-rejected).
- **h2_proxy.rs 87.3%** — 1404-1408 (missing-`:path` 400 — hyper's h2 codec
  RST_STREAMs a path-less extended CONNECT before dispatch, so it is
  defense-in-depth the harness cannot reach; the missing-`:scheme` sibling
  1410-1416 IS covered), 1469-1470 (tracestate-propagation arm — tests send
  traceparent, not tracestate), 1491-1496 (the explicit `WsDialErr::Timeout` 504
  arm — tests hit Refused→502 and elapsed→504 but not the inner Timeout variant),
  1518-1520 (hyper-upgrade-failed-after-upstream debug log — a race window not
  deterministically reachable).
- **ws_tunnel.rs 82.1%** — 126-132 (`Debug` impl), 177-179 (`Default` impl), 228
  (`Poll::Pending` re-emit), 246 (read-after-EOF `Ready(0)`), 261-265 (BrokenPipe
  construction — exercised by `write_after_actor_gone_is_broken_pipe`; lcov marks
  the inner lines per early-return ordering), 280-284 (`poll_flush` no-op).
- **ws_proxy.rs 97.0%** — 394 (the symmetric second send-timeout call site; the
  teardown fn it calls is fully covered).

---

## Summary for the promote decision

- **RUN 2 (gating) overall session-code coverage = 405/483 = 83.9% → PASS the
  ≥80% R10 bar.** Both previously-failing thin spots (ws_proxy `close_backpressure`,
  main.rs H2/H3 wire arms) are now covered per-line.
- The remaining per-file sub-80% files (main.rs 67.3%, listener.rs 54.5%) are
  dominated by binary-boot-only glue + defensive bails + cosmetic Debug/fallback
  lines — none load-bearing, and the H3→H2/H3 RELAY datapath itself is already
  covered by the `bridging_h2_h3` / `bridging_h3_h3` integration tests. These are
  the DOCUMENT-acceptable set the lead identified.
- No further test is REQUIRED to clear the gate. (Optional: a one-line
  `with_websocket(true)` unit test would lift listener.rs, and tripping the
  backend→client direction would cover ws_proxy 394 — neither changes the PASS.)

---

## RUN 1 (superseded — pre-test-add, recorded for the record) — 77.8% FAIL

Before `4ee6e80d`, the same measurement on the prior lcov was **376/483 = 77.8%
FAIL**: ws_proxy 39.4% (close_backpressure 372/394/436-453 all uncovered),
main.rs 57.1% (H2 arm 1272-1276 + H3 arm 1283-1294 uncovered), listener 54.5%.
That run produced the two predictions (ws_proxy close_backpressure; main.rs
H2/H3 wire arms) which builder-1's tests then closed — confirmed per-line above.
The 77.8% figure is SUPERSEDED by the 83.9% RUN 2 gating number.
