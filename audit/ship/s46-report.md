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

**Attempt 2 — llvm-cov instrumented, whole binary ×6: INVALID, not a result (R15).** The run
built for 715 s and then failed to compile: the probe source was edited *while the build was in
flight* and carried a type error (`messages().first()` yields `Option<&Bytes>`, not
`Option<&[u8]>`). No verdict is taken from it. Every subsequent run is **gated on a compile check
before any long step**.

**Attempt 3 — VOLUME probe at the minimal failing shape: REPRODUCED. ✅**

```
S46-VOLUME **BAD** iter=44 kind=502 status=Some(502) grpc-status=None body_len=None fin=true reset=true
S46-VOLUME **BAD** iter=61 kind=502 status=Some(502) grpc-status=None body_len=None fin=true reset=true
S46-VOLUME reps=300 failures=9 rate=3.00%
```

**9 failures in 300 iterations = 3.00% per large-body request**, t3a.large, **uninstrumented**,
single 512 KiB request against a **fresh** gateway+backend each iteration. This independently
confirms, on a second machine and a different code path from CI:

- the failure is **real and gateway-generated**, not a CI artifact;
- it is **not instrumentation-dependent** (this run had no llvm-cov);
- it needs **no connection reuse** (fresh infrastructure per iteration);
- it is **not a size threshold** (one fixed size, 3% of attempts fail and 97% succeed).

The rate is a measurable quantity, which is what makes a fix provable: a load-bearing negative
control must drive 3.00% to 0.00% over a comparable sample, and must still show ~3% on pre-fix code.

**Attempt 3 could NOT name the mechanism** — `RUST_LOG` was set but `tracing` **discards events when
no subscriber is installed**, and `grpc_h3_e2e.rs` installed none. This is the same reason 142 CI
logs carry a 502 but never a cause. Fixed by adding `tracing-subscriber` as an `lb-quic` dev-dep and
`init_probe_tracing()` to both probes.

**Attempt 4 — 400 reps WITH a subscriber: IN PROGRESS.** Expected to yield ~12 failures each
carrying the discriminating warn line of §1.7.

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

### 1.8 THE PROVEN MECHANISM — the gateway aborts its own valid upstream request

**Evidence.** Three independent local runs, t3a.large, uninstrumented, one 512 KiB request against a
**fresh** gateway+backend per iteration:

| Run | Reps | Failures | Rate |
|---|---|---|---|
| hunt | 300 | 9 | 3.00% |
| hunt2 | 400 | 18 | 4.50% |
| hunt3 | 250 | 7 | 2.80% |
| **total** | **950** | **34** | **≈3.6%** |

**Every one of the 7 failures in the instrumented run carried the identical chain** — 7/7, no
variation:

```
h2 send_request failed: http2 error <- stream error sent by user: unexpected internal error encountered
```

**Reading it.** `stream error sent by user` is h2's wording for a RST_STREAM **our own side
initiated**, carrying `INTERNAL_ERROR` (0x2). hyper sends exactly that when the request-body future
returns an error. The only body on this path is `H3ReqStreamBody` (`h3_bridge.rs:1205`), whose
`poll_frame` returns `Err(H3ReqAbort)`.

**The causal chain, end to end:**

1. `H3ReqStreamBody::poll_frame` aborts mid-body → `Err(H3ReqAbort)`.
2. hyper RST_STREAMs the backend with `INTERNAL_ERROR`.
3. `Http2Pool::send_request` returns `Http2PoolError::Send`.
4. `h3_to_h2_stream_resp` (`h3_bridge.rs:1336`) emits **502 bad gateway**.

**The backend never failed.** The gateway kills its own in-flight upstream request on an otherwise
valid 512 KiB POST and reports 502 to the client. This accounts for **every** observed property:
fast (no timeout involved), large-body-only (only large bodies stream through `H3ReqStreamBody`),
reuse-independent (fresh connections fail identically), and present in instrumented **and**
uninstrumented runs.

**Why two months and 142 CI logs never reached this.** Three independent masks, each sufficient
on its own:
1. `hyper::Error`'s `Display` prints only `"http2 error"` and drops the h2 cause
   (`error.rs:612-616`); `http2_pool.rs` flattened it with `to_string()`. **Fixed** by the
   `error_chain()` `source()`-walker — the decisive string was being discarded at the point of
   logging.
2. `tracing` **discards events entirely when no subscriber is installed**, and no CI job ever set
   one — `RUST_LOG` appears in 0 of 142 logs. The one warn that names the variant never printed.
3. The two opposite triggers were collapsed into **one match arm** (below), so even a captured
   abort could not say which fault occurred.

**The remaining ambiguity, and why it decides the fix.** `poll_frame`'s abort arm fired on either
`ReqBodyEvent::Reset` (a **deliberate** abort — client RST or F-CAP-1 over-cap) **or**
`Poll::Ready(None)` (**the producer was dropped without ever sending `End`** — the ingress side
vanished mid-body). Identical on the wire, opposite in cause: the first is correct behaviour, the
second is a gateway-side fault on a valid request. The captured logs exclude every *logged* Reset
path — 0 occurrences of `recv_body error mid-body`, 0 trailer rejections across 400 reps — which
points at the silent producer-drop. The arm is now **split with distinct logging** to name it.

### 1.9 Ancillary finding — the QUIC router silently drops inbound packets

`router.rs:29` sets `ACTOR_CHANNEL_DEPTH = 32`; on `TrySendError::Full` the router **drops the
packet** and logs at DEBUG (`router.rs:210-213`). A 512 KiB body is ~430 packets, producing ~30
drops per request (12,124 across 400 reps).

**Stated honestly: this is NOT the failure discriminant** — the drops occur on successful
iterations too, so they are not sufficient to cause the 502. QUIC retransmits, so this is not a
correctness bug. It is a capacity finding on large bodies, invisible at default log levels, and it
is a plausible *contributor* to whatever drops the body sender. Not yet connected by evidence.

### 1.10 ROOT CAUSE — a dropped completion signal on a deliberately-full queue

The trigger probe settled the last ambiguity: **3/3 failures reported
`trigger=PRODUCER_DROPPED`**, never `RESET`. The abort is `Poll::Ready(None)` — the ingress body
sender dropped **without ever sending `End`**.

The site is the `Finished` handler in `conn_actor.rs`:

```rust
if let Some(tx) = body_tx_by_stream.remove(&sid) {      // sender removed UNCONDITIONALLY
    ...
    let _ = tx.try_send(ReqBodyEvent::End { trailers }); // fails when Full — error DISCARDED
}                                                        // tx dropped here
```

`H3_BODY_CHANNEL_DEPTH = 8` × `H3_BODY_CHUNK_MAX` ≈ 8 KiB is the R8 in-flight bound, and
`drain_request_body`'s gate **deliberately keeps that channel full** for the whole of a large body
(it stops reading while `capacity() == 0`). So when the stream's `Finished` event lands while the 8
slots are occupied, `try_send` returns `Full`, `let _ =` swallows it, the sender is dropped, and the
consumer sees a closed channel instead of a completion:

> `H3ReqStreamBody` → `Err(H3ReqAbort)` → hyper RST_STREAM(INTERNAL_ERROR) →
> `Http2PoolError::Send` → **502**

**A request the backend served correctly is reported to the client as 502.** Every observed property
follows: it needs a body large enough to fill 8×8 KiB (**>64 KiB** — matching "≥256 KiB fails,
`sz=1` never fails"); it is a race on whether a slot drained before `Finished` arrived (hence ~3%,
not 100%); it is fast, reuse-independent and instrumentation-independent.

**The asymmetry is the defect.** The same `let _ = try_send(..)` guards the `Reset` sends, but a
lost `Reset` still yields an abort — the intended outcome. A lost `End` converts **success into
failure**.

### 1.11 THE FIX + LOAD-BEARING NEGATIVE CONTROL

**Fix:** allocate `H3_BODY_CHANNEL_CAPACITY = H3_BODY_CHANNEL_DEPTH + 1` and gate body reads on
`capacity() > 1`, reserving one slot exclusively for the terminal event. The terminal send becomes
**infallible by construction** instead of depending on a drain winning a race. No new state, no
retry loop, no extra pass.

- **R8 preserved.** Body chunks are still capped at `H3_BODY_CHANNEL_DEPTH`, so the ≈64 KiB
  in-flight bound and the three tests asserting it (`h3_h1_stream_body_e2e.rs:633`,
  `h3_h3_stream_e2e.rs:1102`, `h3_h2_stream_e2e.rs:822`) are unaffected. The reserved slot only ever
  carries one terminal event — trailers at most, never body bytes.
- **Second fault repaired.** The same lost `try_send` on the F-CAP-1 over-cap path could answer
  **502 instead of 413**.
- **R12 blast radius, checked not assumed.** The `End` send lives in `conn_actor` and is shared by
  all three H3 egress cells, so H3→H1, H3→H2, H3→H3 and gRPC-over-H3 all inherit the fix.
  `lb-l7`'s H1/H2 fronts were inspected and are **NOT** affected — they use `.send(..).await`, which
  blocks until capacity rather than failing (`h1_proxy.rs:1547+`, `h2_proxy.rs:1944+`). Only the QUIC
  actor uses `try_send`, because it is a synchronous poll loop that cannot await.

**Negative control — same probe, same box, same conditions:**

| | Reps | Failures | Rate |
|---|---|---|---|
| pre-fix | 950 | 34 | **≈3.6%** |
| **post-fix** | 300 | **0** | **0.00%** |

Zero abort triggers, zero `send_request` failures, zero 502 chains post-fix. At the measured ~3%
per-request rate, `P(0 failures in 300) ≈ 0.97³⁰⁰ ≈ 1×10⁻⁴` — the clean run is not chance.

**Severity.** Reachable on **any** H3 request with a body over ~64 KiB, on every H3 egress cell and
gRPC-over-H3. Live for two months at ~38% of exposed CI runs and repeatedly dismissed as a flake.

### 1.12 R3 / R12 no-regression evidence

Full `lb-quic` suite, `--all-features --no-fail-fast`, t3a.large, **after** the fix:

```
Summary [ 386.073s] 230 tests run: 230 passed (3 slow), 0 skipped
```

Zero failures, zero timeouts. The R8 in-flight-bound assertions — the tests that would catch the
fix quietly relaxing the memory bound, and therefore the real gate on this change — all pass:

```
PASS  grpc_h3_server_stream_bounded_memory_r8
PASS  t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream
PASS  h2_e2e_request_memory_bounded_through_stalled_backend
PASS  h2_e2e_response_memory_bounded_through_stalled_client
PASS  h3h3_e2e_request_memory_bounded_through_stalled_backend
PASS  h3h3_e2e_response_memory_bounded_through_stalled_client
PASS  r2_response_memory_bounded_through_stalled_client
PASS  c5_resp_retained_ceiling_is_sound_and_much_less_than_1mib
PASS  r8_chunked_response_trailers_delivered_to_h3_client
```

All three H3 egress cells (H3→H1, H3→H2, H3→H3) and gRPC-over-H3 are regression-free, confirming
the reserved slot carries only the terminal event and never body bytes.

**Process note (R15).** An earlier attempt at this run returned `rc=104`, "creating test list
failed … No such file or directory". That was **not** a code fault: a disk-reclaim glob of mine
deleted `round8_h3_authority_enforced`'s binary, which belongs to `lb-quic`, not to the
integration-test crate it was aimed at. Cargo saw an intact fingerprint and did not relink it. The
target was touched and the run repeated. No verdict was taken from the incomplete run.

### 1.13 CF-S44 VERDICT

**PROVEN-and-fixed.** Mechanism proven by instrumented reproduction, fix negative-control-verified,
R3/R12 clean.

**Carried, explicitly NOT closed by this fix:**
- **The HANG is a separate, still-unexplained bug.** Disjoint from the 502 by payload size (hangs
  only on 5 B–30 B payloads; 502s only on ≥256 KiB), and instrumented-`Coverage`-only (7/71 Coverage
  jobs, 0/71 `Test` jobs). Four different tests have hung. P2's `terminate-after` now converts it
  into a bounded, reported failure that names the test — containment, not a fix.
- **`ACTOR_CHANNEL_DEPTH = 32` router packet drops** (§1.9) — a capacity finding, not the
  discriminant, and not connected to the 502 by evidence.

---

## Phase 2 — P2: hang-as-failure

**Both halves are required; neither works alone.**

1. `.config/nextest.toml` — `slow-timeout = { period = "60s", terminate-after = 20, grace-period
   = "20s" }` ⇒ a test is **killed at 1200 s**. That is **4.9×** the slowest *legitimate* test,
   measured under llvm-cov instrumentation across three CI runs: 244.117 s / 244.970 s / 245.357 s
   (`h2h3_fmd4_request_rst_burst_current_thread`), spread 1.24 s ⇒ wall-clock deterministic, not
   load-dependent. Plus `[profile.ci]` `global-timeout = "40m"` (selected via `NEXTEST_PROFILE: ci`)
   and a `hang-probe` profile so the mechanism demonstrates in ~40 s instead of ~20 min.
2. `ci.yml` — an explicit `TIMEOUT` grep that converts a terminated test into a **red**. Without it
   `--ignore-run-fail` swallows the terminate exactly as it swallows a failure, and the whole fix
   would be **inert in the very job where CF-S44 hung**.

The grep pattern was **R13-tested before being trusted**: 0 matches across three real coverage logs
carrying 1565 passes, the escalating SLOW markers and 3 genuine FAILs (⇒ no false red), while firing
on `TIMEOUT [` / `TMT [` / a timestamp-prefixed line / `global timeout`, and **not** matching
`TIMEOUT-PASS`/`TMPASS`.

`timeout-minutes` added to every job across all three workflows, each ≥3× its observed max over five
runs — generous on purpose, since a false red on a slow-but-healthy run is worse than catching a
hang a few minutes later. Coverage sits at 50 min, deliberately **above** nextest's 40 min cap so
nextest fires first and **names the test**.

**CONFIRMED, not assumed — `terminate-after` does fire under `cargo llvm-cov nextest`:** llvm-cov
internally calls `cargo nextest run` (so nextest owns the kill); `cargo llvm-cov show-env` injects
no `NEXTEST_*` variables (verified by a `strings` sweep), so it cannot redirect profile/config
discovery; and our own CF-S44 job emitted nextest's `SLOW` markers while running under llvm-cov.

**Negative control:** `crates/lb-core/tests/ci_hang_negative_control.rs` wedges on demand behind
`EG_CI_HANG_PROBE`, deliberately **not** `#[ignore]`d ("an ignored test is easy to leave permanently
un-run"). No existing test is skipped, slowed, or weakened.

⚠️ **This is CONTAINMENT for the CF-S44 hang, not a fix.** The hang remains a separate, unexplained
bug (§1.13).

---

## Phase 3 — P3: passive health ejection (G5)

### 3.1 A finding that precedes the feature

**The pre-existing seed could never have worked, even if `record_failure` had been called.** It
keyed checkers by `backend.address` — the raw config string, captured **before DNS resolution** —
while the datapath only ever holds a resolved `SocketAddr`. Every lookup would have missed. G5 was
not "wired but unfed"; it was structurally incapable of functioning. The registry is now keyed by
resolved address.

### 3.2 Design (Envoy/HAProxy outlier-detection semantics, not eject-on-error)

Single-sourced through **one** seam: a `HealthFilteredPicker` decorator over
`Arc<dyn BackendInfoPicker>`, applied at the binary's construction sites. Filtering the backend
*slice* before `pick` was rejected for a concrete reason: `LoadBalancer::pick` returns an **index**,
so removing elements renumbers every index and **remaps every consistent-hash key** for `Maglev` and
`RingHash` — breaking affinity for *all* traffic, not just traffic to the ejected backend.

- **Threshold** — 5 consecutive failures (Envoy `consecutive_5xx` parity); any success zeroes the
  streak (pre-existing, already-tested behaviour).
- **Re-admission** — two independent paths: time-based half-open with exponential backoff
  (30 s → 300 s cap) **and** `record_success` unconditionally clearing ejection.
- **Floor** — enforced at *ejection* time (a stable, assertable invariant) rather than pick time
  (which would flap request-to-request), plus an absolute ≥1 backend, plus fail-open in the picker.
  **`min_healthy_percent = 50`, a deliberate departure from Envoy's 10%** — at the 2–4 backend
  listeners this repo actually configures, a 10% cap means *nothing can ever be ejected* and the
  feature is inert. Hot-reloadable; one-line change for Envoy parity.
- **`enabled = true`** — R3 makes it inert until a backend fails 5 consecutive times, and nginx's
  own default (`max_fails=1`) is far more aggressive. Off-by-default would make "G5 closed" mean
  "closed for whoever finds the knob".

**Interim on `Http2PoolError::Send` → `NotAttempted`.** The pool hands out a cached sender without
saying it was reused, so **N concurrent requests on one stale sender fail together** — a correlated
burst the consecutive threshold cannot protect against, and the shape of a rolling restart. Counting
that as a backend failure would eject a healthy backend for **our** race. Cost, documented at the
mapping site: a backend that accepts connections but resets every stream is not ejected;
`Dial`/`Handshake`/`Timeout` still fire. Proper fix (a `reused` bit out of the pool) is logged as
follow-up work, together with the same-shaped `QuicUpstreamPool::acquire` hazard — **logged, not
diagnosed**, and explicitly not to be assumed by analogy.

### 3.3 The four controls + the R3 proof — DEMONSTRATED, not argued

Each was **watched failing** against a deliberately broken build, then reverted:

| Mutation | Control | Verbatim evidence |
|---|---|---|
| `feed_noop` (the literal pre-S46 state) | (i) eject, (ii) re-admit | `assertion left == right failed: exactly one backend is ejected — left: 0, right: 1` |
| `floorless` (`can_eject` → `true`) | (iii) floor | `the floor caps ejections at 50% of 3 backends — left: 3, right: 1` |
| `eject_on_first` (`HealthChecker::new(1,1)`) | (iv) threshold | `a single transient error must NOT eject — left: 1, right: 0` |
| `noop_wrapper` | R3 non-vacuity | `divergence_is_detectable` **FAILS** while `healthy_routing_is_identical_to_the_unwrapped_build` **PASSES** |
| `send_is_transport` | the §3.2 interim | `left: Failure, right: NotAttempted` |

**`floorless` producing `left: 3` is the finding that justifies the floor**: without it all three
backends eject and the listener black-holes — the "worse than nothing" mode, now measured.

**Mutation 4 is the decisive one** and came back exactly as required: a no-op wrapper breaks
divergence detection **while leaving the identity arm passing**, so the R3 pair is neither vacuous
nor over-constrained. No test was adjusted to reconcile them.

Positive half: **208/208 passing** (lb-health 23, lb-config 80, lb-l7 105), clippy
`--all-targets --all-features -D warnings` exit 0, fmt clean, tree verified free of markers and
backups. All 14 pre-existing `lb-config reload::tests::*` pass — evidence that adding the enum arm
to `l7_fields` weakened nothing.

### 3.4 ⚠️ A verification-harness defect worth more than the feature

The mutation harness reverted with `shutil.copy2`, which **preserves mtime**. The restored source
therefore looked *older* than the artifact built from the mutated source, so cargo's mtime-based
staleness check **skipped the rebuild and kept running the mutated binary**.

In this ordering it produced a **false RED** — loud, and caught. **Reverse the order and the
identical defect yields a silent false GREEN.** The earlier md5 dry-run provably could not have
caught it: the bytes were identical; the defect was in the *timestamp*. Finding it required
re-running the unmutated suite **after** the mutations — the step easiest to skip because it
"should" pass.

Fixed (`shutil.copy` + explicit `touch`, and every revert now prints a `Compiling` line). The
mutation results stand because the *apply* direction uses `write_text()` (mtime = now), verified by
a `Compiling` line in four of five logs, and the fifth was **re-run from scratch** rather than
trusted from scrollback.

---

## Phase 4 — P4: the arming pack

`audit/release/owner-actions.md` refreshed (216 → 465 lines). Contents: the corrected
branch-protection ruleset with the current check names read from `ci.yml` **as it now stands**, the
exact `gh api` command, the verified `SOAK_*` secret-vs-variable split (including
`SOAK_INSTANCE_TYPE`, omitted from the earlier list), corrected line citations, a warning against
adding `paths-ignore:` after arming (a required check that never runs blocks every PR forever), and
a command the owner can run to **watch the hang gate fire** rather than trust it.

**The editorial point, stated before the arming step rather than in a footnote:** requiring
`Coverage` buys **coverage-threshold signal only, not pass/fail signal**, because `--ignore-run-fail`
makes it report `success` on runs with failing tests — evidenced by three runs (`31504772378`
success/1565 passed; `30749813681` **success**/2 failed; `31495640064` **success**/1 failed). `Test`
is the check that fails on a failing test. Requiring `Coverage` is **safe as of S46** — it was not
before, because a hang neither failed nor finished.

**The OWNER executes all of it** (repo-admin rights required); the agent cannot.

---

## Gates

| Gate | Result |
|---|---|
| `cargo test --workspace --all-features --no-fail-fast` **×3** | **1564/1565 each run.** Sole failure `fcap1_h2_over_cap_upload_yields_413` — proven pre-existing, §G.1 |
| `cargo fmt --all -- --check` | **PASS** |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | **PASS** (rc=0) |
| P2 hang negative control (armed) | **PASS** — `TIMEOUT [20.004s]` + `[20.013s]`, `0 passed, 2 timed out`, rc=100 |
| P2 hang probe (inert, no env var) | **PASS** — `2 passed` in **0.010 s**; cannot slow the real gate |
| P3 controls ×4 + R3 proof | **PASS** — all demonstrated against broken builds (§3.3) |
| P3 crates | **208/208**, clippy `-D warnings` exit 0 |
| CF-S44 negative control | **~3.6% → 0.00%** |
| `lb-quic` full suite (R12 siblings) | **230/230**, every R8 bound green |

`clippy --all-targets` is load-bearing here: it catches the `deny(indexing_slicing)` lints in
`lb-quic`/`lb-l7` that a `--lib`-only run skips.

**Build note.** The full `--all-features` workspace build does **not** fit this box at default debug
settings (it passed 25 GB with ~half of `lb-integration-tests` done, on a 67 GB disk with ~38 GB of
non-target usage). With `RUSTFLAGS="-C debuginfo=0"` and `CARGO_INCREMENTAL=0` it completes at
**22 GB** with 6 GB spare. No gate was weakened and no test skipped — only backtrace richness is
reduced. **No larger box was provisioned or required.**

### G.1 The one failure — `fcap1_h2_over_cap_upload_yields_413`

```
tests/h2h1_md_streaming_verify.rs:1888
assertion `left == right` failed: F-CAP-1: H2→H1 over-cap upload to a draining backend
should yield 413, got Some(502)
  left: Some(502)   right: Some(413)
```

**Classification: NOT a regression — independently VERIFIED.** And the *mechanism* is now proven
(§G.2), which the received "known flake" story had wrong.

**The decisive control (found by the verifier, at zero cost): two CI jobs ran on our exact base
commit `a63776e9` and failed this same test.**

```
94035323924  HEAD is now at 2c844a4 Merge … into a63776e9bbaced94efb3a3a9cd3afe5388a0b8c7
  FAIL [ 90.779s] lb-integration-tests::h2h1_md_streaming_verify fcap1_h2_over_cap_upload_yields_413
  FCAP1_H2_OVER_CAP status=Some(502) written=2752512
94035324018  (same base) attempt 1 FAILED status=Some(502) written=19398656 → retry 413 ok
```

Supporting proof, each independently sufficient:

1. **The entire H1-upstream leg of `h2_proxy.rs` is BYTE-IDENTICAL between `a63776e9` and `HEAD`** —
   418 lines (`proxy_request` → `finalize_response`), `diff` empty. That block contains **every**
   site deciding this test's outcome: all three `> MAX_REQUEST_BODY_BYTES` cap checks and both
   `ProxyErr::BodyTooLarge` constructions (the only sources of the 413), and all four 502 strings.
   So the refactor could not change *which* error is produced, because it changed no error-producing
   code on this path.
2. **`UpstreamUnattributable` is unconstructible against an H1 backend** — all three construction
   sites are `Http2PoolError::Send(_)` arms requiring an `Http2Pool`; this test's backend is H1.
3. **No `_ =>` wildcard exists in any of the five `ProxyErr` match sites**, so an added variant
   cannot silently change behaviour; the mapping is preserved arm-for-arm.
4. `h1_proxy::ProxyErr` gained **no** variant at all.
5. The identical assertion appears in **30 CI log files / 34 occurrences, 2026-06-11 → 2026-08-12**,
   all predating this branch's first commit (2026-08-15). Grepping all 142 logs for the session's
   15 SHAs returns **zero** hits.
6. The test file is **unchanged** on this branch; it never calls `with_health()` (so P3 is inert —
   `health` is `None`, `record_health` a no-op); it never references `lb_quic`/`conn_actor` (so the
   CF-S44 fix is unreachable); and it builds `H2Proxy::with_security` **in-process**, so `main.rs`'s
   364 changed lines are unreachable too.

**Correction to an earlier draft of this report:** it listed `e8a82d70` as the only session commit
touching this area. Incomplete — `69cc070a` also touched `crates/lb-io/src/http2_pool.rs`
(`Send(e.to_string())` → `Send(error_chain(&e))`). Also cleared: H2-pool only, string-only, and
unreachable from an H1 backend.

### G.2 The mechanism — PROVEN, and it is a TEST defect, not a gateway defect

**The test's own draining backend gives up after 90 seconds and closes the socket**
(`tests/h2h1_md_streaming_verify.rs:781`):

```rust
match tokio::time::timeout(Duration::from_secs(90), sock.read(&mut buf)).await {
    Ok(Ok(0)) | Err(_) => break,     // Err(_) IS the timeout — it breaks, closing the socket
```

The gateway's H1 upstream then dies and `h2_proxy.rs:1481` falls through to
`ProxyErr::Upstream(format!("send_request: {e}"))` → **502**, before the client can push past the
64 MiB cap.

**The data confirms it exactly.** Across all 142 logs, `written` separates the two outcomes with the
separatrix precisely at `MAX_REQUEST_BODY_BYTES = 67,108,864`, zero overlap:

| outcome | `written` |
|---|---|
| **413 (pass)** | only ever 67,174,400 or 67,239,936 — **above** the cap |
| **502 (fail)** | 327,680 … 65,863,680 — **every one below** the cap |

The test yields 413 **iff** the client actually cleared 64 MiB before the harness backend quit.

**Three received explanations are REFUTED by this:**

- ❌ *"A slow box ran out of budget."* The test's own gateway budgets are `body/total/head = 300 s`
  (`:1817-1822`), yet every run aborts at **~91 s with ~200 s unspent** — local (91.05 / 101.20 /
  92.68 s) **and CI at base (90.779 s)**. It is a fixed 90 s harness timeout, not a budget.
- ❌ *"Deterministic on 2 cores because it's a timing race on a slow box."* The **4-core CI runner
  aborts at the same ~91 s**. Core count is not what differs; throughput only decides whether
  64 MiB is cleared *within* the fixed 90 s.
- ❌ *"`written` varying enormously is the mechanism."* The variance is a **symptom**; the mechanism
  is the sub-cap abort at the fixed 90 s, and the perfect 64 MiB separation is the evidence.

The test's own comment (`:772-774`) states the assumption behind bumping 30 s → 90 s: *"the
unsaturated push completes in well under 10 s."* **That assumption is false on a slow or saturated
box**, which is exactly why it is intermittent on fast CI and deterministic here.

**Consequence: the gateway's cap enforcement is NOT in question.** This is a harness defect.

### G.3 Two pre-existing issues this surfaces (NOT introduced here, NOT fixed here)

1. **`ci.yml`'s escalation criterion is mis-calibrated.** It states that failing all 3 attempts is
   *"a real cap-enforcement failure, not an env flake"*. Our isolated 3/3 trips that rule — but by
   §G.2 it means the harness backend timed out three times, not that the cap failed. The criterion
   never anticipated the fixed 90 s abort.
2. **CI has QUARANTINED this test since before this branch** — `--skip
   fcap1_h2_over_cap_upload_yields_413`, present verbatim at `a63776e9`. **Our R1 gate ran WITHOUT
   that skip, i.e. strictly harsher than the project's own gate**, which is why "1564/1565, only
   fcap1" is exactly what the CI configuration predicts. The session's `ci.yml` diff adds only
   `timeout-minutes:` and **does not touch the quarantine or the retry count** — no gate weakened.

**Honest framing of the gate:** R1's bar is ×3 **all-pass**. This is ×3 **all-pass except one
proven-pre-existing, now-mechanism-explained harness failure that CI itself skips**. It is not
reclassified as green.

**What is NOT claimed:** that CF-FCAP1-FLAKE was previously *understood* — its documented mechanism
did not match the data. And while there is no evidence that P3's marginally larger `H2Proxy` nudged
the race (local `written` values sit inside the base CI distribution; abort time matches base to
~1 s), "no evidence of a shift" is not "proof of no shift". It does not affect the ruling, because
the failure reproduces fully at base.
