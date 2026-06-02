# SESSION 27 — WebSockets over H2 (RFC 8441) + H3 (RFC 9220)

Branch: `feature/websockets-s27` (from `main` @ `33a0d068`, the S26 promote merge).
Box: 8 cores, 30 GB free, ENA ens5. Shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`.

---

## PHASE 0 — baseline + hygiene + F-S26-1 characterization + state-of-WS survey

### Hygiene
- Base tip confirmed: `main @ 33a0d068` (= `docs(s26)` fixup atop the `5be6c263` `--no-ff`
  promote of the whole S23→S26 quiche::h3 migration). `7d1eec76` (S26 branch tip) is an
  ancestor of main. ✓
- Branch `feature/websockets-s27` created from main, pushed to origin. ✓
- `ps aux`: no S26 build/test strays (only the controlling `claude` process). ✓
- Disk: 30 GB free (> 25 GB bar); `eg-target` 21 GB. Mem 15 Gi, 13 Gi available. ✓

### R1 baseline (on base == main)
- `cargo fmt --all --check`: **PASS** (exit 0, empty diff).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: **PASS** (exit 0, 0 warnings).
- `cargo test --workspace --all-features` ×3: **PASS** — 1442 passed / 0 failed / 17 ignored, ×3,
  exit 0 each (`audit/websockets/s27-baseline/`). Matches the S26 known-good baseline.

### Owner check-in outcome (Phase 0 gate)
- **F-S26-1 → FIX in S27** (owner). Mirror `spawn_tcp` resolve+dispatch in the quic spawn path over
  the existing tested library API. Closes a real operability gap (H3-terminate backends were
  un-configurable in the shipped binary) AND unblocks real-binary e2e for WS-over-H3.
- **F-S27-1 → FIX this session, security-tier** (owner, R6). Sequence: (1) verifier mechanism-proves
  the premature-200 via a real-wire H2 test that FAILS on current main; (2) ws-eng fixes
  `handle_ws_extended_connect` to dial + complete the upstream WS handshake BEFORE returning success
  (200 only on success; 502/504 otherwise) — mirroring the H1 GHSA fix; (3) real-wire regression test
  with a LOAD-BEARING NEGATIVE CONTROL (fails on detached-task version, passes on fix); (4) **R12:
  align all three siblings** — confirm H1 fixed, fix H2, and build WS-over-H3 with
  upstream-before-success ordering FROM THE START; verifier confirms identical ordering across
  H1/H2/H3 so the divergence does not recur a third time. CHANGELOG/security-note entry.
- R6 escape hatch retained: if the verifier's proof shows the fix is unexpectedly large/entangled,
  escalate as genuinely-large rather than forcing it (expected path: tractable sibling-aligned fix).

**Phase 0: COMPLETE — baseline green ×3, F-S26-1 + F-S27-1 decisions taken.**

---

## PHASE 1 — WS relay core + WS-over-H2 (RFC 8441)

The relay core (`WsProxy::proxy_frames`) and the WS-over-H2 implementation already existed on main;
Phase 1 = build the missing real-wire verification to the S27 bar + harden the findings it surfaces.

### Increments (author ≠ verifier on every one)
- **INC-1 (ws-eng) `4f45f5d3`** — real-wire WS-over-H2 happy-path e2e `tests/ws_h2_e2e.rs`: raw `h2`
  client, TLS+ALPN-h2, extended CONNECT (`h2::ext::Protocol`), 200, masked tungstenite Text+Binary
  echo through the gateway → H1 WS backend, clean Close. ×3 deterministic. Lead-reviewed non-vacuous.
- **INC-1V (verifier) `e0e5a21b`** — independently **CONFIRMED F-S27-1** (HIGH/security): on main, a
  pickable H1 backend that refuses the WS handshake (non-101) makes the H2 client receive 200 (false
  success), not 502/504. Code-path proof + load-bearing reproducer (audit/, kept out of the suite).
- **INC-2 (ws-eng) `83746d1c`** — **FIXED F-S27-1**: `handle_ws_extended_connect` now async, dials +
  completes the upstream RFC 6455 handshake INLINE under `timeouts.header` BEFORE any 200
  (Refused→502, Timeout/elapsed→504); 200 only on success → spawn splice. Regression test
  `tests/ws_h2_upgrade_defer.rs` (502/504/no-smuggle); load-bearing proven (revert→RED, restore→GREEN).
  Mirrors the H1 sibling. Lead-reviewed diff correct.
- **INC-2V (verifier) `26e0e223`** — comprehensive independent verification:
  - Task A no-regression ×3 (ws_h2_upgrade_defer 3, ws_h2_e2e 1, ws_proxy_e2e 7, H1 round8 4) — **PASS**.
  - Task B F-S27-1 **CONFIRMED CLOSED**, regression independently load-bearing (reverted to e0e5a21b →
    RED, restored → GREEN).
  - Task C fix-correctness — CORRECT; the spawned `hyper::upgrade::on` await is NOT an unbounded hold
    (hyper resolves the H2 upgrade ~45 ms after the 200 flush regardless of client data; idle/read-frame
    watchdogs reclaim) — **no finding**.
  - Task D **RFC 8441 conformance PASS** (`audit/websockets/s27-rfc8441-conformance.md`); 1 PARTIAL
    (`is_h2_extended_connect` doesn't independently require `:scheme`/`:path` — safe, defaults `:path`
    to `/`); deviations: RFC 7692 compression (none), RFC 9220 H3 (Phase 2).
  - Task E **R8: (i) bounded-memory PASS** (VmHWM flat across 10× message volume); **(ii) backpressure
    FAIL → NEW BLOCKER F-S27-2**.
  - Task F **R13 burst PASS** — 60 upgrade/relay/close cycles, fds 12→12 (zero leak), ×3.

### Findings
- **F-S27-1 (HIGH/security) — FIXED + independently verified.** WS-over-H2 emitted 200 before the
  upstream WS handshake (H2 analog of the H1 GHSA; false-success + smuggle window). R12 sibling
  alignment: H1 was already fixed; H2 now matches; H3 to be built correct from the start.
- **F-S27-2 (HIGH/security, DoS / memory-exhaustion) — H2-ONLY, shipped; clean fix genuinely-large →
  ESCALATED (R6/R7).** A WS-over-H2 client that stops reading lets the gateway buffer a fast/amplifying
  backend's bytes UNBOUNDED in memory (measured ~360 MiB). **Three independent steps reconciled the
  root cause** (the program's "symptom ≠ attribution" discipline):
  - INC-2V (verifier) found the symptom but mis-attributed it to the shared `tungstenite_config`
    (`max_write_buffer_size`) and claimed it was "identical on the shipped H1 path."
  - INC-3 (ws-eng) measure-first **REFUTED** both: the `max_write_buffer_size` bound is a no-op
    (tokio-tungstenite already parks on `WouldBlock`); and WS-over-H1 backpressures correctly.
  - Reconciliation (protocol-expert, `1a308ac3`, `audit/websockets/s27-fs27-2-recon/`) confirmed by
    independent measurement + vendored source: **WS-over-H1 BOUNDED/SAFE** (17/2048 plateau, identical
    with/without the bound — the TCP socket's `WouldBlock` propagates backpressure); **H2 UNBOUNDED**
    because hyper's `UpgradedSendStreamTask::tick` (hyper-1.9.0 `upgrade.rs:98,119`) calls
    `h2::SendStream::send_data` even on `Poll::Pending` capacity → h2's unbounded send buffer
    (`share.rs:48-59`, `prioritize.rs:145-219`); hyper's `max_send_buf_size` only caps reported
    `capacity()`, not `send_data`.
  - Scope: **H2-ONLY** (NOT shared-core, NOT H1). Shipped/reachable (`main.rs:1014` wires
    `with_websocket` on the H2/ALPN path; `h2_proxy.rs:804` advertises extended CONNECT).
  - Fix feasibility: no hyper knob; raw `h2::SendStream` not reachable (`H2Upgraded` is `pub(super)`)
    → only via driving raw h2 = a rearchitecture of H2 serving (LARGE + R3-risky); the relay-level
    drain-gate is refuted (over H2 `send()` returns on buffer, not drain). **TRUE backpressure fix is
    genuinely-large → R6 escalate.**
  - **WS-over-H3 will NOT inherit it** — the planned adapter backpressures by construction (window-gated
    drain + empty-queue refill + partial-write retention); to be re-verified at Stage E.
  - Landed + KEPT (`bd3f991d`, correct & in-scope, NOT the H2 fix): defensive `max_write_buffer_size`
    bound + an anti-hang guard wrapping both forwarding `send().await` in `timeout(read_frame, …)` →
    Close 1008 (reclaims a wedged write on socket transports — load-bearing on H1/raw).
  - **Owner decision: gate WS-over-H2 OFF-by-default** (INC-5 `09ab157d`). New
    `WebsocketConfig.h2_extended_connect` (default false) → `H2Proxy.h2_extended_connect_enabled`;
    when off, `enable_connect_protocol()` is not called (SETTINGS not advertised) and the
    extended-CONNECT fork never fires (off-path extended CONNECT → h2-layer PROTOCOL_ERROR, backend
    dials 0). Negative test `tests/ws_h2_gated_off.rs` (load-bearing: flip on → RED). The proper
    window-aware fix is carried as **CF-S27-2**. Flag is documented with the DoS caveat (opt-in for
    trusted clients only).
- **F-S27-3 (LOW, observability parity) — FIXED** (INC-4 `bc162fb8`). The H2 upstream WS handshake now
  injects the child `traceparent`/`tracestate` (trace context was already threaded via
  `RequestTrace::inject_upstream`), matching H1 (R12). Test asserts same trace-id, different parent-id;
  load-bearing.
- **Lenient `:scheme`/`:path` accept (LOW, RFC 8441 §4) — FIXED** (INC-4 `bc162fb8`). A websocket
  extended CONNECT missing `:scheme`/`:path` now returns **400** (was a silent `:path`=`/` default);
  `tests/ws_h2_conformance.rs` (load-bearing: neutralize → 200).

### INC-5V — FINAL independent WS-over-H2 sign-off (`a00c29b6`, audit-only)
- Gating holds (off→no tunnel/advertise, on→works) — PASS, non-vacuous (`ws_h2_gated_off` ×3).
- F-S27-1 still closed post-churn — PASS (`ws_h2_upgrade_defer` ×3, gate on).
- LOW fixes correct — PASS (`ws_h2_conformance` ×3).
- R8 corrected scope: **WS-over-H1 BOUNDED + backpressured** (plateau 17-18/2048 ×3); WS-over-H2 (i)
  bounded-volume PASS / (ii) true-backpressure carried as CF-S27-2 (gated off). No false H2 claim.
- R3 no-regression — PASS (`h2_proxy_e2e` 5, `ws_proxy_e2e` 7, `round8_ws_upgrade_defer` 4).
- Audit reconciled: `s27-rfc8441-conformance.md` updated; `s27-r8-ws-proof.md` correction banner
  (original "identical on H1"/WsConfig root-cause SUPERSEDED by the `1a308ac3` reconciliation).
- **VERDICT: WS-over-H2 READY to ship (gated/opt-in).** No new findings.

### PHASE 1 — COMPLETE
WS relay core (pre-existing, shared, R12) + WS-over-H2 (RFC 8441) verified to the S27 bar. Findings:
F-S27-1 FIXED, F-S27-3 FIXED, lenient-accept FIXED, F-S27-2 → WS-over-H2 gated off-by-default +
CF-S27-2 carried. Real-wire e2e ✓, R8 (H1 bounded+backpressure) ✓, R13 burst ✓, RFC 8441 conformance ✓.

---

## PHASE 2 — WS-over-H3 (RFC 9220) — PARTIAL (Stages A+B landed; Stage C carried to S28)

F-S26-1 (prerequisite) FIXED + verified first (`ef54a9d3`): the binary's `spawn_quic` now wires the
H3-terminate→backend relay (h1 via `with_backends` / h2 / h3 by `BackendConfig.protocol`); startup
validation rejects raw_proxy+backends and mixed protocol families; real-binary test
`spawn_quic_h3_terminate_forwards_to_h1_backend_through_real_listener` (non-vacuous, ×3); R3 preserved
(Mode B + backendless H3-terminate byte-identical). This closed a real operability gap (H3-terminate
backends were un-configurable in the shipped binary) and unblocked the WS-over-H3 real path.

### Stage A — config + validator (R3-safe) `d1c45d3f`
- RFC-confirmed (RFC 8441 §4 / RFC 9220 §3): extended CONNECT = `:method=CONNECT` + `:protocol` +
  `:scheme` + `:path` + `:authority` (the INVERSE of classic CONNECT, which omits :scheme/:path).
- `validate_request_pseudo_headers` split: classic CONNECT rules unchanged; extended CONNECT requires
  the full set; `:protocol` accepted ONLY under `ws_enabled` (off ⇒ byte-identical to today's #14
  reject — the load-bearing R3 control). `:protocol`-on-non-CONNECT + duplicate-`:protocol` rejected.
- `enable_extended_connect(true)` gated on `ws_enabled` (an H3 SETTINGS entry, not a transport param —
  R3 at transport untouched). `WsConfig`/`ws_enabled` threaded QuicListenerParams→RouterParams→ActorParams.
- New `WebsocketConfig.h3_extended_connect` (opt-in, default off) — a NEWNESS gate for a brand-new
  feature on the freshly-migrated stack, NOT a DoS gate (H3 backpressures by construction; documented).
  `validate_websocket_block` now permits the block on a `quic` listener.
- Tests: 7 new validator + 2 h3_config + 2 lb-config; 8 existing R3 validator tests carried (pass false).

### Stage B — bounded tunnel adapter (standalone) `3bdd77fb` (+ clippy gate-fix `1e87a4dc`)
- New `crates/lb-quic/src/ws_tunnel.rs`: `H3WsTunnel` (AsyncRead+AsyncWrite) ↔ `H3TunnelEndpoints`,
  two bounded mpsc channels (depth 8 × 8 KiB ≈ 64 KiB/dir, matching `H3_BODY_CHANNEL_DEPTH`).
- **R8 proven in isolation:** the write side uses `tokio_util::PollSender::poll_reserve` → the writer
  PARKS when the actor stops draining (no unbounded buffer — the property WS-over-H2 lacked, CF-S27-2).
  Reset → `io::ErrorKind::ConnectionReset` (the F-MD-4-adjacent close-vs-reset distinction); EOF on
  inbound-sender drop; empty-chunk-not-EOF; write-after-actor-gone → BrokenPipe. 9 unit tests ×3.

### Stage C — CARRIED to S28 (owner decision: honest-PARTIAL)
Sharpened estimate ~1 session-equivalent + verify + soak on top; the actor tunnel-mode pump + the
upstream-before-200 ordering are the risk; doesn't fit the S27 tail. Design APPROVED (closure-injection
`ws_relay_launcher` mirroring `config_factory`, resolving the lb-l7↔lb-quic dependency cycle) and
documented in `s27-ws-h3-plan.md` + `s28-handoff.md`. Building blocks for S28 already on main:
validator (A), `ws_tunnel.rs` adapter (B), F-S26-1 binary wiring, `h3_extended_connect` flag.

### F-S26-1 characterization (the gating dependency)

**Verdict: case (a) — "schema + library API both exist; the binary's config→params
translation never calls them." A genuinely SMALL wiring gap, not a missing capability.**

Mechanism (git-proven on main):
- The **library** (`lb-quic`) fully supports an H3-terminate→backend relay. `QuicListenerParams`
  carries `backends` / `h3_backend` / `h2_backend`, and the listener exposes three fluent
  wiring methods, each e2e-tested:
  - `with_backends(Vec<SocketAddr>, TcpPool)` → H3→H1 (Pillar 3b.3c-2)
  - `with_h3_backend(QuicUpstreamPool, addr, sni)` → H3→H3 (Pillar 3b.3c-3)
  - `with_h2_backend(Http2Pool, addr)` → H3→H2 (PROTO-001)
- The **config schema** already carries `[[listeners.backends]]` (`ListenerConfig.backends`,
  shared with the TCP path) and `BackendConfig.protocol` (h1/h2/h3 selector).
- The **binary** gap: `crates/lb/src/main.rs::quic_listener_params_from_config` (line 478) only
  ever calls `with_raw_backend` (Mode B). For the H3-terminate path it **never reads
  `listener_cfg.backends`** and never calls `with_backends`/`with_h3_backend`/`with_h2_backend`.
  Result: a `protocol="quic"` listener with no `raw_proxy` block runs backendless → every H3
  request that reaches established state is answered 502 "no backends available". Only inline-error
  egress is black-box reachable through the production binary.

**Fix size:** read `listener_cfg.backends` + `BackendConfig.protocol` in the quic spawn path and
dispatch to the existing, already-tested library wiring methods (resolve addrs like `spawn_tcp`
does; pick `with_backends`/`with_h2_backend`/`with_h3_backend` by backend protocol). Small,
self-contained, mirrors `spawn_tcp`. **PRE-AUTHORIZED per Phase 0(a).**

### State-of-WebSocket survey (material scope finding)

The codebase **already ships substantial WebSocket support on main** — this session is largely
*verify-to-bar + harden + extend*, not greenfield. Honest inventory:

| Layer | Status on main | Evidence |
|---|---|---|
| **Relay core** (`WsProxy::proxy_frames`) | **EXISTS** — opaque bidirectional tungstenite forwarder; idle-timeout→1001, per-direction read-frame watchdog→1008 (WS-002), client-Ping flood limit→1008 (WS-001), faithful close-code forwarding, half-close, EOF→Close(None). Single-sourced (R12). | `crates/lb-l7/src/ws_proxy.rs` |
| **WS-over-H1** (RFC 6455) | **EXISTS + real-wire tested** — `h1_proxy` `with_websocket` + `handle_ws_upgrade`; GHSA-class "no 101 before upstream handshake" **fixed** (Pingora/Envoy GHSA). | `tests/ws_proxy_e2e.rs` (533 ln, real tungstenite backend), `tests/round8_ws_upgrade_defer.rs` |
| **WS-over-H2** (RFC 8441) | **IMPLEMENTED, under-verified** — `h2_proxy` advertises `enable_connect_protocol()` (SETTINGS_ENABLE_CONNECT_PROTOCOL, RFC 8441 §3); `is_h2_extended_connect` predicate; `handle_ws_extended_connect` returns 200 + relays to an **H1 backend** via the shared core. **No real-wire e2e** (the only "test" is a source-grep code-presence check); **no R8/R13 proof**. | `h2_proxy.rs:804,995,1288`; `crates/lb-l7/tests/h2_connect_protocol_settings.rs` (grep test) |
| **WS-over-H3** (RFC 9220) | **NOT BUILT** — h3 bridge validates CONNECT shape RFC-correctly but has no `:protocol` / extended-CONNECT handling; `h3_config.rs:70` literally tags it "WebSockets-over-H3 is an S26 item". quiche 0.28 **does** support it (`h3::Config::enable_extended_connect`, `Connection::extended_connect_enabled_by_peer`). | `h3_bridge.rs:528`, `h3_config.rs:70`, quiche-0.28 `h3/mod.rs:617,1746` |

### Phase-0 finding candidate (to verify independently in Phase 1)

**F-S27-1 (candidate, HIGH / security): WS-over-H2 emits `200 OK` to the client BEFORE the
upstream WS handshake completes** — the H2 analog of the GHSA-xq2h-p299-vjwv / GHSA-rj35-4m94-77jh
issue that was explicitly **fixed for H1** (`round8_ws_upgrade_defer.rs`). `handle_ws_extended_connect`
(h2_proxy.rs:1288) returns `200` synchronously (line 1371) and dials the backend + drives the
upstream handshake in a **detached task** (line 1314). If the backend is down or refuses the WS, the
client has already been told the tunnel is up. This is also an R12 sibling-divergence (H1 was
deferred; H2 was not). Tractable fix: dial + complete the upstream handshake first; return 200 only
on success, 502/504 otherwise (mirrors the H1 fix's intent). To be mechanism-proven by the verifier
before fixing.

_(The F-S27-1 candidate above was the Phase-0 flag; it was independently confirmed in INC-1V and
fixed in INC-2 — see the Phase 1 section. Phase 3 follows.)_

---

## PHASE 3 — gates + WS soak + promote (honest-PARTIAL)

### R1 ×3 full gate — GREEN (`audit/websockets/s27-phase3-gate/`, HEAD `9be1a3b7`)
- `cargo fmt --all --check`: **PASS**.
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`: **PASS** (exit 0).
- `cargo test --workspace --all-features` ×3: **1484 passed / 0 failed / 18 ignored**, exit 0 each.
  (Baseline 1442 → 1484 = +42 WS tests; +1 ignored. No regression, R3.)
- The gate caught + we fixed: 3 clippy lints in WS-H2 test harnesses (`43a20852`); and a **session R3
  regression** (R14) — INC-5's WS-over-H2 gating was not propagated to `round8_authority_enforced.rs`'s
  `spawn_h2_ws_proxy`, so the security test `test_h2_ext_connect_comma_authority_rejected` got a
  PROTOCOL_ERROR instead of the authority guard's 400; gate-enabled the helper (`9be1a3b7`). These were
  the 4th–5th self-reported-clean gaps the independent full gate surfaced — verify-to-bar earning its keep.
- `fcap1_h2_over_cap_upload_yields_413` flaked 1/3 in the first gate run; **isolation-proven PASS**
  (11.57s) → the documented **CF-FCAP1-FLAKE** (pre-existing H2-timeout saturation race, R2), not a
  session defect. It did NOT recur in the clean re-run (0 failures ×3).

### WS soak (R8/R10 long-lived leak-class, the F-S20-2 concern) — BOTH BOUNDED
New lb-soak scenarios `sc8_ws_h1` (`5041325d`) + `sc8b_ws_h2` (`749a2306`); data in
`audit/soak/s27-soak-data/`. Both 360s/73-sample, against the REAL binary's shared
`WsProxy::proxy_frames` relay (H1 wired at main.rs:940; H2 at main.rs:1013-1020 with
`h2_extended_connect=true`) → real RFC 6455 echo backend, byte-verified multi-frame payloads to 64 KiB,
heavy open→relay→clean-close churn:
- **sc8_ws_h1**: BOUNDED — rss/fds/threads/vmhwm flat, panic=0, ws_churn ok=5,436,016 / ws_sustained
  ok=137,075, **err=0**.
- **sc8b_ws_h2**: BOUNDED — fds 43→43 (flat, no conn leak), rss non-monotone (settling), panic=0,
  ok=99,331/192,632, **err=0** (after soak-eng's R15 attribution of an initial err=40 to clean
  lifecycle-closes, not relay defects — WsEcho counts byte-mismatch as err, clean-close as reconnect).
- Independently confirmed by verifier-p3 (`7f5d9bdb`, read-only, CSV-eyeballed): both COMPLETED,
  BOUNDED, non-vacuous; err=40 attribution SOUND; `fds` discriminant SOUND (analyzer math untouched).
  → the long-lived WS relay is leak-bounded under sustained bidi load + churn over H1 + H2.
- 2 LOW non-blocking residuals tracked: a soak-harness `Some(Err)→Closed` reclassification note + a
  stale `accept_inflight "scraped for visibility"` doc line (ws_gauges omits it). Neither affects the
  leak-class proof.

### R10 scoped coverage (session product code ≥80%, verifier re-measured) — PASS
Independently re-measured by verifier-p3 (`616ba3c3`, `audit/websockets/s27-coverage.md`) from a full
`--workspace --all-features` llvm-cov lcov, mapped per-line onto the session-changed PRODUCT ranges
(test-module lines excluded; whole-file % dilutes): **405/483 = 83.9% — PASS (≥80%)**.
First pass was 77.8% (FAIL); +6.1 pts from 2 targeted tests (`4ee6e80d`): `close_backpressure_1008_on_forward_write_timeout`
(ws_proxy: 39.4%→97.0% — the anti-hang write-timeout guard now exercised) and
`wire_h3_terminate_backends_dispatches_{h2,h3,h1}_arm` (main.rs F-S26-1 dispatch arms now hit).
Per-file: lib.rs 96.0, h2_proxy 87.3, ws_proxy 97.0, ws_tunnel 82.1, h3_bridge/h3_config/conn_actor/router 100,
listener 54.5, main.rs 67.3. The two sub-80% files are immaterial to the 83.9% overall: their uncovered
remainder is binary-boot/e2e-only glue (build_h2_proxy WS branch, the `with_websocket` opt-in setter,
listener-started log, the `run()` spawn_quic call site) + defensive `no-resolved-backend` bails + the
SNI-fallback closure — none load-bearing; the H3→H2/H3 RELAY datapath itself is covered by the
`bridging_h2_h3` / `bridging_h3_h3` integration tests. Documented-acceptable per the
`llvm-cov-session-scope` rule.

### Phase-3 disk hygiene
Surgical trims of the stale `llvm-cov-target` (20G) + `release/` + `debug/incremental` kept the box
above the bar throughout (CF-DISK-1; never `rm` during an active compile). Coverage regenerates
`llvm-cov-target`.

---

## VERDICT — SESSION 27 PARTIAL

**SESSION 27 PARTIAL — WebSocket proxying delivered over H1 (RFC 6455) + H2 (RFC 8441, gated
off-by-default); WS-over-H3 (RFC 9220) groundwork landed (Stages A+B), Stage C carried to S28.**

Findings: **5 total — 4 fixed, 1 carried.**
- F-S27-1 (HIGH/security, premature-200 / GHSA-class smuggle on WS-over-H2) — **FIXED** + verified.
- F-S27-2 (HIGH/security, H2-only unbounded-buffer DoS) — WS-over-H2 **gated off-by-default**; proper
  window-aware fix **carried as CF-S27-2** (S28). H1 safe; H3 safe by construction.
- F-S27-3 (LOW, H2 upstream trace-parity) — **FIXED**.
- Lenient `:scheme`/`:path` extended-CONNECT accept (LOW, RFC 8441 §4) — **FIXED** (now 400).
- F-S26-1 (operability — H3-terminate backends un-configurable in the binary) — **FIXED** (prerequisite).

Delivered + verified: WS-over-H1 (shipped, R8-bounded+backpressured) · WS-over-H2 (gated, real-wire
e2e + RFC 8441 conformance + R8(H1) + R13 + F-S27-1/3 fixed) · F-S26-1 binary wiring · WS-over-H3
Stages A+B (RFC-9220 config+validator R3-safe + bounded tunnel adapter, R8-in-isolation + reset-vs-EOF)
· WS soak H1+H2 BOUNDED · R1 ×3 GREEN.

Carried to S28: **Stage C** (WS-over-H3 relay wiring — approved closure-injection design + real-wire
e2e + R8/R13/reset-vs-EOF + RFC 9220 conformance + H3 soak); **CF-S27-2** (WS-over-H2 window-aware
backpressure); **gRPC-over-H3** (the program's last spec item). See `s28-handoff.md`.

