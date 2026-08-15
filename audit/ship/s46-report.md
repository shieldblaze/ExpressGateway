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
  that into a 502 with **no second attempt on a fresh connection**. hyper-util, nginx, Envoy,
  HAProxy and Pingora all distinguish "never reached the server" from "failed at the server" and
  retry only the former; **this pool does not make that distinction at all.**
  *(Correction: an earlier draft of this line said "Envoy/nginx/HAProxy all retry once on a
  reused-connection failure". That **overstates** three of the five for a POST — nginx will not
  retry a sent POST without `non_idempotent`, HAProxy recommends `disable-l7-retry if METH_POST`,
  and Pingora requires an idempotent method. Only hyper-util retries a POST, and only because
  `take_message()` proves nothing was sent. See §1.7.)*

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

---

## Phase 1 — P1: CF-S44

### 1.0 A framing correction that applies to every line number below

Both CF-S44 jobs checked out `0a67d30` = merge of **`6c81222f`** into `eb00081d` — **pre-S45A**. The
S45A de-slop moved 209/212 lines in `grpc_h3_e2e.rs`, so **the CI panic's `:1242` is NOT today's
`:1242`** (locally that is `grpc_h3_health_check_forwarded_not_synthesized`). Every claim here was
checked against BOTH `git show 6c81222f:<file>` and the working tree; the executable code of every
test involved is identical between them — only comments moved.

The binary contains **16 tests at `6c81222f`, 17 now** (the S46 probe). That count is load-bearing:
`running 1 test` + `15 filtered out` = 16 is what demonstrates one test process per test.

### 1.1 REFUTED by evidence (each with its citation)

| # | Claim refuted | Evidence |
|---|---|---|
| R-1 | **A monotonic response-size threshold at 256 KiB** | In the SAME job `91548350113`: `FAIL [0.232s] … grpc_h3_trailer_survives_all_response_sizes` (log:1939) and `PASS [0.722s] … grpc_h3_trailer_survives_any_frame_granularity` (log:1964). The latter drives 512 KiB **and** 1 MiB under **two** backend modes = four large bodies, all clean, one second later. Non-vacuous: a 502 emits `RespEvent::Head{status, headers: Vec::new()}` (`h3_bridge.rs:1336`) carrying **no** `grpc-status`, and granularity asserts `field("grpc-status") == Some("0")`, so a 502 there **would** have failed it. |
| R-2 | **Timeout / gate-saturation** | The failing test's TOTAL wall time was **0.232 s**, covering cert-gen, backend spawn, listener spawn, the `sz=1` iteration AND the failing `sz=262144` iteration. Both candidate timeouts far exceed it: `DEFAULT_H2_SEND_TIMEOUT = 30 s` (`http2_pool.rs:39`, used at `:67`; the test uses `Http2PoolConfig::default()` at `grpc_h3_e2e.rs:219`, **no override anywhere in the file**), and — the constant §4b's saturation story actually turns on — `DEFAULT_CONNECT_TIMEOUT_MS = 5_000` (`lb-io/src/pool.rs:29`). The H3→H2 leg uses the **fixed** `send_request` (`h3_bridge.rs:1332`), NOT the shorter `idle_bounded_send`, which would have collapsed onto the same `Timeout` variant. No QUIC-side deadline is shorter (client idle 30 s, driver budget 45 s). |
| R-3 | **Hang candidate (a): blocked on the suite serial guard** | In hang job `91517507600`, **15 of the binary's 16 tests COMPLETED and PASSED** — every sibling returned and dropped `_suite_serial`, so nothing held it. This argument does not depend on nextest's execution model. Corroboration: the sibling **uninstrumented** jobs `91517507558` / `91548350046` ran `cargo test --workspace --all-features` — one process, guard **genuinely** contended — and passed both tests. Further corroboration: nextest is process-per-test ([nexte.st/docs/design/how-it-works](https://nexte.st/docs/design/how-it-works/)), so the process-global `static SUITE_SERIAL` (`grpc_h3_e2e.rs:44`) is uncontended under nextest anyway. |
| R-4 | **"We forgot `sender.ready()`"** | For hyper 1.x **http2**, `SendRequest::poll_ready` is *just* an `is_closed()` check (`hyper/src/client/conn/http2.rs:97-103`). Awaiting `ready()` adds nothing beyond what `PeerEntry::is_alive()` already does. This is NOT the defect and should not be chased. |
| R-5 | **The request-body path** | `h2_request_body_from_rx` 502s only via `builder.body(body).map_err(|_| 502u16)` (`h3_bridge.rs:1290`) — an invalid method/URI/header, size-independent. |

### 1.2 NOT refuted — stated honestly

**Hang candidate (b) is IMPLAUSIBLE, not refuted.** An earlier draft of this report claimed the
driver loop's "only awaits are bounded". **That claim was wrong and is withdrawn.**
`drive_grpc_h3_core` (`grpc_h3_e2e.rs:351-501`) contains three awaits: the bounded read at `:478`
(`tokio::time::timeout(to.min(25 ms), sock.recv_from(..))`), an **unbounded** `sock.send_to(..).await`
at `:380` sitting inside `while let Ok((n, info)) = conn.send(..)` (`:379`) — an inner loop that
**never re-checks `deadline`** — and an unbounded `UdpSocket::bind(..).await` at `:359`, pre-loop and
outside the budget entirely. Setup outside the budget is likewise unbounded (`QuicListener::spawn`
awaits `UdpSocket::bind`; `spawn_h2_grpc_backend` awaits `TcpListener::bind`). A loopback UDP
`send_to` wedging for 5 h would be extraordinary, and `conn.send` terminates on `Err(Done)` — hence
*implausible*. But the honest statement is that the deadline is **not** enforced on every await path,
which is exactly what candidate (b) asserts.

### 1.3 The GATEWAY-vs-HARNESS ruling

**GATEWAY-GENERATED.** The client received a well-formed H3 response carrying `:status 502`. That
status originates only in the gateway's own `inline(&resp_tx, 502, b"bad gateway")` paths; the test
client cannot synthesize one. This does **not** by itself make it a gateway *defect* — the gateway
may be faithfully reporting a real upstream failure — but the 502 is emitted by gateway code.

### 1.4 What the CI evidence CANNOT settle

**The failure block carries NO gateway log output.** `grep -c RUST_LOG` over the whole job = **0**;
the coverage step's env block lists only `CARGO_TERM_COLOR`, `RUSTFLAGS`, `RUST_MSRV`, `CARGO_HOME`,
`CARGO_INCREMENTAL`, `CACHE_ON_FAILURE`; and `grpc_h3_e2e.rs` installs no `tracing_subscriber`. So
`h3_bridge.rs:1335` `tracing::warn!(error = %e, %addr, "H3→H2 stream send_request failed")` had no
subscriber and emitted nothing. The discriminating `Http2PoolError` variant
(`Dial`/`Handshake`/`Send`/`Timeout`) exists **only** in that suppressed line.
**Naming the mechanism therefore requires a local repro with `RUST_LOG` — it cannot be read out of CI.**

### 1.5 The residual hypothesis (HYPOTHESIS, not finding — R2)

With `Timeout` excluded by arithmetic, a fast 502 must be `Dial`, `Handshake`, or `Send`. The
structural observation that fits the evidence:

- `PeerEntry::is_alive()` (`http2_pool.rs:78`) is a **TOCTOU** check on a *reused* sender —
  `!is_closed() && !driver.is_finished()` can be true at check time and false at send time.
- On failure `send_request` evicts and returns `Err`; `h3_bridge.rs:1336` turns that into a **502
  with no retry on a fresh connection**.
- Position fits: in the failing test, one gateway serves all four sizes, so `sz=262144` is the
  **second** request — the first to reuse the pooled H2 leg. In the PASSING granularity test
  `start_h3_listener_h2` is called **inside** the loop (`grpc_h3_e2e.rs:1179`), so its 512 KiB is a
  **first** request on a fresh gateway.

Counter-evidence against a *pure* position story: `grpc_h3_burst_50_unary_cycles` passed (2.170 s),
driving 50 requests through one gateway — but note these are 50 **fresh QUIC/H3 connections** reusing
only the **H2 upstream leg**, with ~8-byte payloads. It refutes pure-position for tiny bodies and
leaves a **size × position interaction** as the live hypothesis. That is precisely what the S46 probe
(§1.6) is built to separate.

### 1.6 Reproduction attempts (t3a.large — FUNCTIONAL verdicts only)

**Attempt 1 — quiet, serialized, uninstrumented: NEGATIVE (no repro).**
`s46_cfs44_size_vs_position_probe`, 3 reps, ~10 s each: **48/48 cells clean**, every arm
(`fresh-256k`, `reuse-256k`, `orig-order`, `rev-order`), every size 1 B → 1 MiB, every position.
Zero 502s, zero gateway warns. Consistent with CI, where the uninstrumented `Test` job passed both
times and only the instrumented Coverage job failed.

**Attempt 2 — llvm-cov instrumented, whole binary: IN PROGRESS.**

### 1.7 LIBRARY-USAGE VALIDATION (R7) — verdict: **NEITHER-BUT-A-DESIGN-GAP**

Performed against **vendored primary source at the locked versions** (hyper 1.10.1, h2 0.4.14,
hyper-util 0.1.20 under `~/.cargo/registry/src/`), not documentation.

**hyper is not at fault, and our liveness check is not defective.** `PeerEntry::is_alive()` is
*provably equivalent* to the reference implementation's for h2: hyper-util's poolable liveness is
`is_open() = !is_poisoned() && is_ready()` (`client/legacy/client.rs:854-856`), and for h2
`is_ready()` bottoms out at `!giver.is_canceled()` (`dispatch.rs:141-147`) while `is_closed()` **is**
`giver.is_canceled()` — so `is_ready() == !is_closed()` exactly. Ours is that same test **plus** a
strictly stronger `!driver.is_finished()`. hyper documents the residual TOCTOU as unavoidable
(`http2.rs:112-118`: *"even after checking this is ready, sending a request may still fail because
the connection was closed in the meantime"*). No liveness check can close it; every reference closes
it downstream instead.

**"We forgot `ready()`" is refuted a second, stronger way:** hyper-util does not call it for h2 at
all — `PoolTx::Http2(_) => Poll::Ready(Ok(()))` (`client.rs:779-789`), and `poll_ready`'s `_cx` is
unused so it can never even pend. **Nobody should chase this.**

**The actual gap: we call the wrong API and throw away the decisive information.**
`SendRequest::send_request` binds the never-sent request to `_req` and **drops** it
(`http2.rs:150-171`); `try_send_request` hands it **back** via `take_message()`
(`http2.rs:181-204`) — and its return *is* proof the bytes never reached the server
(`dispatch.rs:19-24`). `http2_pool.rs:157-169` then flattens every outcome into
`Http2PoolError::Send(e.to_string())`, conflating "never sent" with "failed at the server" — the
single distinction all five references are built on (hyper-util `client.rs:248-271`, nginx
`proxy_next_upstream`/`non_idempotent`, Envoy `reset-before-request`, HAProxy `disable-l7-retry`,
Pingora `fail_to_connect` "guarantees that nothing was sent upstream").

**The streaming body does NOT block a fix** — this reverses an earlier tentative reading in this
report. A polled `H3ReqStreamBody` is indeed unreplayable (mpsc cannot be rewound), but in the
never-sent case hyper refuses the request *before the body is ever polled*: `body_rx` is undrained
and `first` still holds `b0`, so the request is byte-for-byte replayable **precisely when** replay
is safe. The two conditions coincide, which is why hyper-util needs neither a replay buffer nor an
idempotency check.

#### The FALSIFIABLE PREDICTION that settles attribution in one run

`hyper::Error`'s `Display` prints only `description()` (`error.rs:612-616`) and
`http2_pool.rs:163` uses `e.to_string()`, so the `.with("connection was not ready")` cause is
dropped. If the stale-pooled TOCTOU is the mechanism, the warn line will read **exactly**:

```
h2 send_request failed: operation was canceled
```

`Kind::Canceled` (`error.rs:540`) is a sound never-sent discriminator on the h2 path — its three
h2-reachable construction sites (`http2.rs:167`, `http2.rs:196`, `dispatch.rs:223`) are all
never-sent.

| Observed warn line | Meaning | Implicates the retry gap? |
|---|---|---|
| `h2 send_request failed: operation was canceled` | never sent — stale-pooled TOCTOU | **YES** |
| `h2 send_request failed: <anything else>` | sent; upstream genuinely failed | **NO — the 502 is faithful** |
| `upstream dial failed:` / `h2 handshake failed:` | fresh dial; nothing was reused | **NO** |
| `h2 send_request timed out` | 30 s elapsed | already refuted at 0.232 s |

**What is explicitly NOT claimed:** that this gap explains CF-S44. It is unproven until a repro
names the variant, and if the variant is `Dial`/`Handshake` then nothing was reused and this
finding is **irrelevant** to that failure. A plausible gap must not become an attribution
(`feedback-symptom-not-attribution`). The design gap is worth fixing on its own merits either way.

*(Phases 2–4 appended as each completes.)*
