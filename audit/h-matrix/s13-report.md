# SESSION 13 REPORT — H2→H3 (closes the H-to-H matrix) + CF-BODY-WALLCLOCK

Branch: `feature/h-matrix-s13` (base `main` @ `85017edc`)
Role layout: lead (this report) / builder-1 (H2→H3) / builder-2 (CF-BODY-WALLCLOCK) / verifier.

Status: **IN PROGRESS** — Phase 0 + §3 connector-contract resolution complete.

---

## Phase 0 — baseline + environment hygiene

- **Base tip confirmed:** `git log` HEAD = `85017edc` ("Promote S12: H1→H3 BUILT (8 of 9) …"). ✓
- **Working tree:** clean on `main`. ✓
- **Strays:** `ps aux` showed no cargo/rustc/lb_/target strays from S12. ✓
- **Disk (CF-DISK-1 / R9):** initial free = **20 GB — BELOW the ≥25 GB floor.**
  Root cause: warm `eg-target` = 32 GB (`debug/deps` 26 GB, `debug/incremental` 5.1 GB).
  Surgical reclaim: removed `debug/incremental` (pure compile-speed cache, zero
  correctness value, regenerates) → **25 GB free** (R9 floor met). `deps` warm cache
  preserved (no cold rebuild). Coverage is SCOPED (R10) so its instrumented profile
  stays small. Will re-check disk periodically through the session.
- **CARGO_TARGET_DIR:** unset in env; repo `.cargo/config.toml` does not set it.
  All cargo invocations explicitly export `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` (R9).

### R1 baseline (×3, `cargo test --workspace --all-features`)
- Run 1: <in progress — 76 test binaries `test result: ok` so far, all green, no failures>
- Run 2: pending
- Run 3: pending
- clippy `--all-targets --all-features -D warnings`: pending
- fmt `--check`: pending

---

## Phase 1 — H2→H3

### §3 CONNECTOR-CONTRACT PREREQUISITE — RESOLVED (lead, first S13 action)

The plan (§3) posed a binary question: does the connector treat `body_tx` dropped
without a final `ReqBodyEvent::End` as a `Reset` (RESET-without-FIN) or as a FIN?
Reading `crates/lb-quic/src/h3_bridge.rs` shows the answer is **STATE-DEPENDENT**:

1. **First peek, before ANY event** (`h3_bridge.rs:3309-3313`): `body_rx.recv() == None`
   → treated as a **bodyless COMPLETE request → HEADERS+FIN** (content-length-0
   semantics). This is the DANGEROUS arm: a pump dropped here would smuggle a
   truncated request to the H3 backend as a complete bodyless one.
2. **At `AwaitNext`, mid-body** (`j2_req_event_action` `h3_bridge.rs:4115-4130`,
   `Some(ReqBodyEvent::Reset) | None => J2ReqAction::AbortNoFin`): dropped →
   `stream_shutdown(Write, H3_REQUEST_CANCELLED)`, **RESET-without-FIN** (safe).
   The `None`-models-dropped-producer case is documented at 4103-4108.

**Consequence for Hazard (a):** the detached-pump mitigation is **LOAD-BEARING, not
"defended by construction."** H1→H3's pump always emits an explicit terminal event
(`End`/`Reset`), so it never reaches the first-peek-None bodyless arm. H2→H3 under a
downstream H2 `RST_STREAM` (service-future cancel) is the FIRST path that could drop
the pump before any event — so the H2→H3 pump MUST (i) be detached (spawned, so a
service-future cancel does not drop it — mirror `h1_proxy.rs:2034` + the detached
`connector_handle` at `:2159`), and (ii) always emit an explicit terminal `End`/`Reset`
(never let `body_tx` drop silently at the first-peek boundary).

### Reference implementation read (lead, for plan approval)
- H1→H3 cell to mirror: `crates/lb-l7/src/h1_proxy.rs:1988-2259` (`proxy_h1_to_h3`):
  detached pump (`:2034`) → spawned connector (`:2159`) → `resp_rx` drain →
  `StreamBody` with `Err(H1PumpAbort)` injection on `H3RespEvent::Reset` (`:2209-2213`).
- H2 ingress to reuse: `crates/lb-l7/src/h2_proxy.rs:1977-…` (`proxy_h2_to_h2_request`):
  lookahead window (`:2009-2062`); F-MD-4 `None`+`is_end_stream()` disambiguation
  (`:2024-2038`) — a within-window reset is **zero-dial safe** (returns BadRequest
  before any pool contact). Branch A (within-window, buffered) vs Branch B (streaming).
- Buffering cell to REPLACE: `proxy_h2_to_h3` (`h2_proxy.rs:2308-2335`, dispatch at
  `h2_proxy.rs:1250` `UpstreamProto::H3 => proxy_h2_to_h3`).
- Trailer-mandate WIN: H2 front carries trailers natively (hyper H2 server flushes
  `Frame::trailers` — no `Trailer:` pre-declaration, CF-RESP-1 constraint GONE). H2→H3
  is THE gRPC-capable →H3 cell; BUILT bar ASSERTS `grpc-status` reaches the H2 client.

(continued as the session progresses)
