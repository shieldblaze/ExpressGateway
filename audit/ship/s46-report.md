# Session 46 — Ship-blockers: CF-S44 diagnosis, hang-as-failure, health ejection, gate arming

**Branch:** `feature/ship-blockers-s46` · **Base:** `main @ a63776e9` (confirmed == `origin/main`)
**Box:** t3a.large — 2 vCPU / 7.7 GiB RAM / 8 GiB swap (enabled this session) / 20 GiB free disk.
**⚠️ Every measurement in this report was taken on t3a.large.** Any perf-shaped observation is
**NOT-COMPARABLE** to the S39 baseline (c6a.2xlarge: WS 42.5k / H2 32k / H3 18.5k / H1 14–18k rps).
Only FUNCTIONAL pass/fail verdicts are reported.

---

## Phase 0 — baseline and evidence

### 0.1 Environment (completed, verified)

| Item | State | Evidence |
|---|---|---|
| Base tip | `a63776e9` == `origin/main` | `git ls-remote origin main` → `a63776e9bbaced94efb3a3a9cd3afe5388a0b8c7` |
| Working tree | clean at branch creation | `git status --short` → empty |
| Branch | `feature/ship-blockers-s46` pushed | `* [new branch] feature/ship-blockers-s46` |
| Strays | none — no cargo/rustc/gateway/test processes at start | `ps aux` sweep |
| Swap | **was OFF**; 8 GiB `/swapfile` enabled this session | `swapon --show` → `/swapfile file 8G` |
| Disk | 13 GiB free → **20 GiB free** after reclaim | `df -h /` |
| `CARGO_BUILD_JOBS` | 2 | exported for every build/test invocation |
| Toolchain | rustc 1.88.0 (pinned by `rust-toolchain.toml`) | `rustc --version` |
| Test tooling | cargo-nextest 0.9.135, cargo-llvm-cov 0.8.7 | `--version` |

Disk reclaim was **non-destructive**: removed the unreferenced `1.95.0` rustup toolchain (CI uses
only `RUST_MSRV`=1.88 and `nightly`), pruned 0-image Docker volumes, `apt-get clean`, vacuumed
journals. The stale session clones under `/home/ubuntu` (`ws-smoke`, `ws-clean`, `cp-lead-run`,
`cp-ops-clone`) were **left untouched** — they carry uncommitted work on their own branches.

### 0.2 CF-S44 — KNOWN FACTS (from `audit/ci/s44-grpc-h3-te-trailer-hang.md`, S44 evidence)

All from the Coverage job, `--all-features` under **llvm-cov instrumentation**, branch
`s44-coverage-metric`, SHA `6c81222f`:

1. **The hang.** `lb-quic::grpc_h3_e2e grpc_h3_without_te_header_still_delivers_trailer` ran
   ≥17,940 s against its own `OVERALL = 20 s` budget (~900×) in run `30755744744` / job
   `91517507600`. 1564 of 1565 tests completed; this one never returned. Job cancelled manually at
   5 h 02 m. Orphans killed: `cargo-llvm-cov`, `cargo-nextest`, `grpc_h3_e2e-ee1b7dc6f6c6ee7e`.
2. **The 502.** A re-run on the *identical* SHA passed that test in 0.100 s but failed a *different*
   test in the *same* binary: `grpc_h3_trailer_survives_all_response_sizes`,
   `assertion left == right failed: sz=262144 / left: Some(502) / right: Some(200)`.
3. **Instrumentation-only so far.** The uninstrumented `Test` job passed both times. Runs
   `30749813681` and `30751410142` completed all 1565 tests in ~343 s / ~372 s.
4. **Not caused by the S44 coverage change** — that change is a LCOV text parser in a *later*
   workflow step which was still `pending` at cancellation.
5. **The job has no `timeout-minutes:`**, and runs with `--ignore-run-fail`, so a hang neither
   fails nor completes — it silently burns to GitHub's 6 h ceiling. (This is P2.)
6. CF-S44 is **not** on the known-flake list (CF-FCAP1-FLAKE, CF-SATURATION-1, CF-S35-T5-FLAKE,
   CF-S37-D6-H2PROXY-FLAKY, CF-S38-RELOAD-BOOT-FLAKE) and **must not be added without a proven
   mechanism** (R2).

### 0.3 Code facts established this session (source reads at tip `a63776e9`)

The failing test drives **H3-front → H2-backend** (`start_h3_listener_h2`), so the relevant code
path is `h3_to_h2_stream_resp` (`crates/lb-quic/src/h3_bridge.rs:1296`). Its 502 sites:

| Site | Trigger |
|---|---|
| `h3_bridge.rs:1327` | `h2_request_body_from_rx` returns `Err(_)` (non-413) |
| `h3_bridge.rs:1336` | **`pool.send_request(addr, request)` returns `Err`** — logs `tracing::warn!(error = %e, %addr, "H3→H2 stream send_request failed")` |
| `h3_bridge.rs:1407/1428` | `on_head` default `status = 502` when `:status` is absent/unparseable |

**The existing `warn` log is already a perfect discriminator** — `Http2PoolError`
(`crates/lb-io/src/http2_pool.rs:91`) has four `Display`-distinct variants:
`upstream dial failed: {0}` · `h2 handshake failed: {0}` · `h2 send_request failed: {0}` ·
`h2 send_request timed out`. **No instrumentation patch is required to name the mechanism** — a
repro with `RUST_LOG` capture reads it straight out. This is the Phase-1c instrument.

Two further structural facts, both load-bearing for the hypotheses below:

- **`send_timeout` is 30 s and is the PRODUCTION default.** The test builds its pool with
  `Http2PoolConfig::default()` (`grpc_h3_e2e.rs:219`); `http2_pool.rs:330` asserts that default is
  `Duration::from_secs(30)`. So a 502 from the `Timeout` variant requires 256 KiB over *loopback*
  to exceed **30 seconds** — a very large budget, which materially weakens the naive
  "instrumented build is just slow" story and is why it is being measured rather than assumed.
- **There is NO retry on a reused pooled connection.** `acquire_sender`
  (`http2_pool.rs:221`) returns a cached sender whenever `entry.is_alive()`; if the subsequent
  `sender.send_request()` fails, the pool evicts the peer and returns `Err` — the caller turns
  that into a 502 with **no second attempt on a fresh connection**. Envoy/nginx/HAProxy all retry
  once on a reused-connection failure; this pool does not.

### 0.4 The sweep-order observation (a candidate confound worth testing explicitly)

`grpc_h3_trailer_survives_all_response_sizes` (`grpc_h3_e2e.rs:1138`) sweeps
`[1, 256*1024, 512*1024, 1024*1024]` against **one** gateway and **one** backend created before the
loop. Therefore `sz=262144` is not only "the 256 KiB case" — it is also **the first request that
reuses a pooled H2 connection** (`sz=1` performs the fresh dial). "256 KiB" and "second iteration"
are perfectly confounded in the failing run, and the reported failure is consistent with either.

This yields a decisive, cheap experiment (Phase 1a): **reorder the sweep**. If the failure follows
the *position* in the loop it is connection-reuse; if it follows the *size* it is size-dependent.

### 0.5 UNKNOWNS — what is NOT yet established

| # | Unknown | How it will be settled |
|---|---|---|
| U1 | Is the 502 **size**-dependent or **loop-position** (connection-reuse) dependent? | Reordered-sweep experiment (1a) |
| U2 | Did `grpc_h3_trailer_survives_any_frame_granularity` (512 KiB + 1 MiB) **pass in the same run** that failed at 256 KiB? If yes, a monotonic size threshold at 256 KiB is refuted. | CI log forensics (in flight) |
| U3 | **Which** `Http2PoolError` variant produced the 502? | `RUST_LOG` capture at a live repro (1c) |
| U4 | **GATEWAY or HARNESS?** Does the same request succeed straight to the backend, bypassing the gateway? | Hold-one-variable isolation (1b) |
| U5 | Is the hang mechanism (a) the untimed `SUITE_SERIAL.lock().await` (`grpc_h3_e2e.rs:49`) or (b) an unbounded inner await outside the deadline? | Bounded-guard probe + task dump (1c) |
| U6 | Does the failure reproduce **only** under llvm-cov instrumentation, or also uninstrumented under load? | ×3 baseline + instrumented repro |
| U7 | Is the trailer involved at all, or is the 502 pre-response (upstream-side) and the trailer assertion merely downstream of it? | U3 answers this — a `send_request` failure is pre-response |

**Explicitly NOT concluded:** that this is a flake, that it is saturation, that it is F-S29-1
recurrence, or that it is a product bug. The S44 note's own saturation hypothesis is recorded there
as "a hypothesis, not a finding" and is treated that way here.

### 0.6 Baseline gate status

`cargo test --workspace --all-features --no-run` launched under `CARGO_BUILD_JOBS=2` with a 3 h
hard timeout. R1 ×3 + clippy + fmt follow. Results recorded in §Gates when the runs COMPLETE
(R15: no verdict from an incomplete job).

*(Sections for Phases 1–4 are appended as each completes.)*
