# SESSION 19 — Finish Mode B (B4/B5/B6) → native QUIC proxy complete

Branch: `feature/quic-proxy-s19` (base `main @ 0b768701`, S18 stall-closed
Mode B core relay B1/B2/B3).

Roles: lead (coordination, no feature code), builder-1 (B4), builder-2
(B5+B6), verifier (independent). Author ≠ verifier on every increment (R5).

---

## Phase 0 — baseline + hygiene  ✅ GREEN

### Hygiene
- Base tip confirmed `main @ 0b768701`; branch `feature/quic-proxy-s19`
  created and pushed to origin.
- Process table clean at start — no S18 strays, no pending heartbeat tick
  (`ps`/`ps -u ubuntu` showed only the shell, docker, and a separate
  unrelated Claude session). No process killed (none to kill).
- **Disk (CF-DISK-1 / R9 ≥25 GB)**: started at 13 GB free (under bar);
  `eg-target/debug` was 38 GB, dominated by orphaned duplicate integration-
  test binaries (180–220 MB each) accumulated across 18 sessions. No compile
  running (idle-guard satisfied). Full clean of the artifact dir →
  **50 GB free (26% used)**. Pristine rebuild for the baseline.

### R1 ×3 baseline (full `cargo test --workspace --all-features`, full 8-core)
All FOREGROUND-equivalent (harness background on the real command, completed
log read to completion before any claim — R15). Logs in
`audit/quic/s19-logs/`.

| Run | passed | failed | ignored | exit | log |
|-----|--------|--------|---------|------|-----|
| 1 (full rebuild) | 1364 | 0 | 18 | 0 | baseline-run1.log (wall 13:16, RSS 786 MB) |
| 2 | 1364 | 0 | 18 | 0 | baseline-run2.log |
| 3 | 1364 | 0 | 18 | 0 | baseline-run3.log |

Deterministic 1364/0/18 ×3 — matches the S18 close tally exactly (no
regression in the inherited tree). No flakes, no asterisks.

- `cargo fmt --all --check`: clean (exit 0).
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`:
  clean (exit 0, 0 warnings; log baseline-clippy.log).

### Design prep (parallel with baseline build)
- B4 plan authored + approved: `audit/quic/s19-b4-plan.md`.
- B5+B6 plan authored: `audit/quic/s19-b5-b6-plan.md`.
- Key wiring finding: Mode B is NOT yet reachable from `main.rs`
  (`raw_quic_backend = None` everywhere) — B6 wires it. A QUIC listener is
  wholly H3-terminate OR wholly Mode B (per-`RouterParams`), so enabling
  datagrams only on the Mode-B path keeps the H3 path byte-identical (R3).
- quiche 0.28 datagram semantics confirmed from source (recv queue =
  drop-newest internally; `dgram_send → Err(Done)` on send-queue-full = our
  explicit drop-newest point).

---

## Phase 1 — B4: datagram relay + bounded drop-newest queue  ✅ VERIFIED

Builder: builder-1 (`8ff3df6d`). Verifier: independent (`146701e8`).

### Implementation (raw_proxy.rs)
- `BoundedDgramQueue` (VecDeque<Vec<u8>> + cap + dropped) — explicit
  **drop-newest** at `len >= cap`; `DGRAM_QUEUE_CAP = 1024` (matches quiche
  `enable_dgram` default; R7 pre-auth; documented R8 bound).
- `relay_datagrams`/`pump_dgram_dir`: recv-drain `src` → bounded queue
  (drop-newest if full) → send-drain to `dst`. `dgram_send` policy:
  `Err(Done)` = transient backpressure (retry, never drop); `BufferTooShort`
  = drop-this (un-forwardable); `InvalidState` = drain+disable (mis-wired
  guard). Independent of streams (no FIN/reset/ordering; stream table & pump
  untouched).
- Wired into `run_dual_pump` after `relay_streams`; RELAY_TICK gate extended
  so datagram-only traffic is pumped promptly, no busy-spin when idle.
- `dropped` counter plumbed (accessor) for B6's `quic_modeb_datagrams_dropped`.

### Verification evidence (logs in audit/quic/s19-b4-verify-logs/ + s19-logs/)
- Real-wire pass-through both directions, binary (zero-len/all-zero/non-UTF8/
  near-max), byte-identical; two distinct conns (client_scid != upstream_scid).
- Queue bound under 50k-datagram flood at a stalled sink: connection healthy,
  bounded, no OOM/hang. Drop-newest: flood past cap → `received < sent`
  (drops occurred, bounded), relay not wedged, delivered bytes intact.
- **R13(a)** scoped gate 173/0/0; **R13(b)** 60/60 wire-flood burst + 60/60
  unit burst (0 flake); **R13(c)** negative control proven load-bearing
  (scratch unbounded shape fails len/dropped/order asserts).
- Lead independent sanity: unit 4/0, builder-wire 1/0, verifier-wire 3/0.

## Phase 2 — B5 (bounded-state flood) + B6 (wiring + metrics + 2 security proofs)

### B5 — bounded-state flood + explicit per-stream cap  ✅ VERIFIED
Builder: builder-2 (`55c8e453`). Verifier: independent (`2d98f941`).

- **Per-stream cap**: `MAX_RELAY_STREAMS = 256` (defense-in-depth ceiling,
  independent of quiche's ~32-concurrent grant; per-conn relay memory ≤
  256·2·STREAM_RELAY_WINDOW = 128 MiB hard const). `admit_or_refuse()` in
  `relay_streams`: NEW sid tracked only while table < cap; already-tracked
  sid always processed; over-cap refused (fail-safe, rate-logged).
  `pump_dir`/`streams.retain` unchanged.
- **Bounds proven TAKEN not vacuous (F-CAP-1 bar)**:
  - Per-stream cap refuse branch: grant 320+16, open 320 → table clamps to
    exactly 256 (grant exceeds open count ⇒ clamp = refuse path).
  - Eviction reclamation: 600 sequential streams (≫16 grant, ≫256 cap),
    peak in-flight ≤32, all byte-identical ⇒ bounded by `streams.retain`.
  - Router drop: 40 distinct Initials at max_connections=4 → table caps at
    2·4=8, 36 dropped. Existing cap test not weakened.
- **R13(a)** scoped gate 178/0; **R13(b)** 50/50 wire + 50/50 unit (0 flake);
  **R13(c)** three load-bearing negative controls proven (cap removed→320;
  retain removed→unbounded/hang; router guard disabled→10>8).
- Lead independent sanity: cap/admit/router unit + both wire tests pass.

### B6 — wiring + metrics + 2 security proofs  ✅ VERIFIED
Builder: builder-2 (`73847598` wiring/metrics + cap-knob fix). Verifier:
independent (`acd0b947` security proofs).

- **Config (lb-config)**: `QuicListenerConfig.raw_proxy: Option<RawQuicProxyConfig>`
  {backend_addr, sni, backend_ca_path, dgram_queue_cap=1024,
  max_relay_streams=256}, `#[serde(default)]` ⇒ H3 configs unchanged. Backend
  leg always `verify_peer(true)` (CA path or default roots; no downgrade knob).
- **Wiring (main.rs)**: `build_raw_quic_backend` → `QuicUpstreamPool`
  (verify_peer + trust + ALPN-mirror + enable_dgram) → `RawBackend`; `spawn_quic`
  registers `QuicModeBMetrics` + sets `RouterParams::raw_quic_backend` when
  `raw_proxy` present. Mode B reachable end-to-end through the real entry point.
- **R3**: `build_server_config(enable_datagrams = raw_backend.is_some())` ⇒
  H3-terminate listeners pass false ⇒ advertised transport params byte-identical.
- **Metrics (lb-observability)**: `QuicModeBMetrics` {connections gauge,
  connections_total, datagrams_dropped_total, streams_active}, threaded at
  run_dual_pump/actor-lifetime (RAII guard) — B4/B5 helper signatures untouched.
- **R14/R12 cap-knob fix**: both config caps fully wired to the real bounds,
  single-sourced (config → RawBackend → run_dual_pump → BoundedDgramQueue +
  relay_streams/admit_or_refuse); consts kept as documented defaults; B5 test
  call-sites pass the const (byte-identical behaviour).

#### Security proofs (by-construction bar)
- **TWO-CONNECTIONS** (s19_b6_two_connections): `client_scid != upstream_scid`,
  distinct trace_ids, ALPN mirrored, + load-bearing independence witness
  (backend's observed inbound SCID == upstream_scid, NOT derived from client ⇒
  not a bridge). Structural 1:1: `quiche::accept_with_retry` (router.rs:351) +
  `dial_dedicated` (quic_pool.rs:412) = two distinct `quiche::Connection` in one
  actor — cannot be fewer than two.
- **0-RTT-REJECTION** (s19_b6_zero_rtt_rejection): by construction —
  `enable_early_data` absent from all non-test config ⇒ server cannot issue
  early-data tickets / accept 0-RTT. Wire — a WILLING client resumes
  (`is_resumed=true`) + attempts early data; observed `is_in_early_data=false`,
  no pre-handshake server data, completes via full 1-RTT, early bytes delivered
  only after. `ZeroRttReplayGuard` = defence-in-depth (untouched).
- **Metrics non-vacuous** (s19_b6_metrics_nonvacuous): connections_total≥1,
  gauge returns to 0 (RAII), datagrams_dropped=128>0 under flood, streams_active
  moves off 0 — guards the stub-metric trap.
- R3 re-confirmed (h3_h3 22/22, listener_lifecycle 6/6); B5 re-confirmed
  (cap unit 10/10) under the const→param cap wiring. Lead sanity: all pass.

## Phase 3 — gates + promote
<pending>

## Carry-forwards
CF-DEP-1, CF-IGN-1 (16 inherited #[ignore]), CF-FCAP-MARGIN, F-ESC-1,
N-1 (jumbo-MTU), Mode A deferred perf tiers (io_uring v1.1, XDP v1.2),
coverage/disk items.

## S20 handoff
<pending — chaos/soak suite, with the stall-fix client-leg send/FIN path
flagged as a priority soak target>
