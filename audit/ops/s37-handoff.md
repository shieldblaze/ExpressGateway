# S37 handoff — operational layer continued (B config, C hot reload, D latest-deps)

S36 promoted **workstream A (H3 connection recycling)** to `main` standalone and stopped there
(owner pacing call). The leak (CF-GRPC-H3-CHURN-RSS) is FIXED on main. S37 picks up B/C/D fresh.

## What landed in S36 (on main, build on it)
- `max_requests_per_h3_connection` config knob (lb-config `RuntimeConfig`, u32, **default 1000**,
  0=disabled). Threaded lb-config→main.rs→`QuicListenerParams`→`RouterParams`→`ActorParams`.
- In-actor cap→GOAWAY(`goaway_pending`/`goaway_sent` two-flag + per-tick `try_send_pending_goaway`)→
  drain→`graceful_h3_shutdown`→recycle, single-sourced in the shared H3 actor (conn_actor.rs).
- Metric family `QuicH3RecycleMetrics` (`h3_goaway_sent_total`, `h3_connections_recycled_total`).
- Tests: `crates/lb-quic/tests/h3_connection_recycle_e2e.rs` (4) + config (4) + metric (2).
- Proven: sc9 BOUNDED ~22 MB @ cap 1000 (`audit/soak/s36-soak-data/sc9-postfix/`); coverage D-6 PASS.

## Carried workstreams (scope + the head-start S36 already did)
### B — config management (protocol-complete + validated)
`lb-config` (~3200 lines) is ALREADY protocol-broad: ListenerConfig/Grpc/Websocket/H2Security/
Runtime/Xdp/Admin/Security/Observability. B is **audit-for-gaps + strengthen validator + prove
equivalent-config byte-identical**, NOT build-from-scratch. The A cap is already a config knob.
Check coverage of: Mode A/B passthrough, L4/XDP path, all timeouts/limits, TLS, backend protocol.
Validator must fail LOUD with clear errors on invalid input (internet-facing: never silently
mis-route). R3: a config equivalent to today's defaults boots byte-identical (prove via real-binary
boot + the e2e suite under it). Single-sourced schema (R12).

### C — hot reload (the nginx -s reload bar) — correctness-critical, fresh-mind only
Validate new config FIRST (reject+rollback if invalid — NEVER apply a failing config), apply to NEW
connections, drain/preserve in-flight, atomic swap. R13: a live-connection-survives-reload test
under traffic + a negative control (a naive swap WOULD drop it; ours doesn't). Depends on B.

### D — latest-deps upgrade (the held cluster, now unblocked by A)
quiche/tokio-quiche/hyper/h2/tokio/prometheus + rest to latest. Stage like S33: patch group →
breaking → quiche (h3spec diff vs the named-waiver baseline) → hyper/h2 (h2spec 146/147 + **check
hyper#4050**: if a release merged it, NOTE it un-gates WS-H2 — but do NOT un-gate unless #4050 is
actually IN the release). Supersede PR #224/#226. Drop+carry any crate that can't verify.

## Standing context for S37 (hard-won this session)
- Box: 8 cores, 15 GiB RAM / **add an 8 G swapfile** (`/swapfile`; gone on reboot). `CARGO_TARGET_DIR=
  /home/ubuntu/Code/eg-target`, `CARGO_BUILD_JOBS=4`. **Disk is the binding constraint** (67 G; a full
  `--all-features` debug test build ≈ 33 G, llvm-cov needs a SEPARATE ~28 G target). Stage:
  drop release→×3→drop debug→coverage. Build from ONE consistent path (worktree-vs-mainroot path
  duplication doubles eg-target — see [[s36 lesson]]).
- **Monitoring**: every long job (build/×3/soak/coverage) MUST be harness-tracked (run_in_background)
  AND have a stall watchdog (died/low-disk/hang). A teammate going idle after launching a background
  job does NOT re-invoke the lead → a soak completed and the lead sat idle ~6 h. Don't repeat it.
  Watchdog liveness = exact process `comm` names, NOT `pgrep -f <script-name>` (matches your own shell).
- Carry-forwards: CF-S27-2 (hyper#4050 → WS-H2 stays gated until a release includes it),
  F-ESC-1 (multi-kernel XDP, self-hosted), CF-S35-T5-FLAKE (throughput flake), CF-FCAP1-FLAKE,
  CF-SATURATION-1, perf/real-traffic burn-in, **security audit (the serious one — internet-facing +
  all-protocol)**, high-quality docs.
