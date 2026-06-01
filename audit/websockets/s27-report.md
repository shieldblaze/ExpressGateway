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

