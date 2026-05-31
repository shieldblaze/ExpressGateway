# S20 — Chaos/Soak Suite Design (plan of record)

Status: APPROVED-TO-BUILD under R7 pre-authorization (all choices below are
industry-standard for a long-running stability soak; documented and proceeding
per the brief's pre-auth on load-gen approach, chaos types, observability
metrics).

Base: `feature/chaos-soak-s20` off main `6f9c3523` (S19 promoted, native QUIC
complete).

## 0. Goal restatement

Prove (or disprove) that ExpressGateway upholds its **R8 bounded-state
invariants under sustained hostile load**:

- memory (RSS) bounded — no leak,
- file-descriptor count bounded — no fd leak,
- connection-table / stream-table / flow-table bounded — no growth under churn,
- thread/task count bounded — no task leak.

The soak's PASS for a scenario = these stay flat/bounded across the **completed**
run's full time-series. The soak's FIND = any drifts upward over time. Finding
defects is SUCCESS (R6/GREEN-is-program-level). The verdict is read ONLY from
the completed run (R15).

## 1. Architecture — external-process soak (operator-faithful)

The soak launches the **real `expressgateway` release binary** as a child
process (exactly how an operator runs it: positional TOML arg, SIGTERM drain),
points co-located in-process load generators + chaos injectors at it, and a
sampler scrapes the gateway child's `/proc/<pid>/` (RSS/fd/threads) and its
`/metrics` (state-table gauges) on a fixed cadence, writing a per-scenario CSV
time-series. Load-gen runs in-process inside the driver so the **gateway
child's RSS/fd are cleanly isolated** from the load generator's.

Why external/real-binary (not in-process gateway): RSS/fd isolation, real drain
path, real listener/accept graph, real metrics endpoint — this is the canonical
production-faithful soak shape, and reuses the proven spawn harness in
`tests/reload_zero_drop.rs` (`find_binary`, `ephemeral_port`, `unique_temp_dir`,
SIGTERM delivery).

## 2. Deliverable — `crates/lb-soak` (permanent infra, R11; single-sourced, R12)

New workspace member. Library + binary + the harness's own unit tests (R5 — the
tooling is itself tested, and joins the R1 ×3 gate).

```
crates/lb-soak/
  Cargo.toml
  src/
    lib.rs            -- re-exports; module wiring
    procstat.rs       -- read /proc/<pid>/status (VmRSS,VmHWM,Threads) + count
                         /proc/<pid>/fd entries. Verdict-critical. Unit-tested
                         against a synthetic /proc-style fixture + self-pid.
    metrics.rs        -- minimal Prometheus text-exposition parser:
                         name{labels} value -> map. Verdict-critical. Unit-tested
                         on captured /metrics samples incl. labelled gauges.
    timeseries.rs     -- Sample row + CSV writer + trend analyzer producing a
                         per-metric BOUNDED/DRIFT verdict. Verdict-critical.
                         Unit-tested on synthetic flat / rising / sawtooth series.
    config_gen.rs     -- TOML generators for each datapath (h1, h1s/h2, quic
                         Mode B, passthrough Mode A) + self-signed cert/key 0600
                         + observability metrics_bind. Mirrors reload_zero_drop
                         templates.
    backends.rs       -- origin servers: blocking H1 (200/echo/slow/drop),
                         hyper H2 origin, quiche QUIC/H3 origin (for Mode A/B).
    loadgen.rs        -- sustained drivers: H1 keep-alive + churn, H2 multi-
                         stream, QUIC client (Mode A flow / Mode B stream+dgram).
    chaos.rs          -- injectors: conn-flood, stream-flood, dgram-flood,
                         slowloris, slow-POST, rapid-reset (CVE-2023-44487),
                         partial-write, mid-stream-disconnect, backend-slow,
                         backend-drop, oversize-413-under-teardown.
  src/bin/eg-soak.rs  -- CLI orchestrator (one scenario per invocation).
```

`eg-soak --scenario <name> --duration-secs <n> --sample-secs <s> --out <dir>`:
1. build/locate the gateway binary (caller pre-builds release),
2. spawn the scenario's backend(s),
3. generate the TOML, spawn the gateway child, wait for readiness,
4. spawn the load + chaos tasks (bounded, sized to the scenario),
5. every `sample-secs`: append a CSV row {t, rss_kb, vmhwm_kb, fds, threads,
   <gauges...>} and print a one-line heartbeat snapshot (R9 liveness),
6. on duration elapse: stop load, SIGTERM the gateway, observe clean drain,
   run the trend analyzer, write `verdict.json` + `summary.txt`, write a
   `soak_complete.marker`, reap ALL children (no leaked test processes — R9).

The orchestrator bounds its own log (rotates/caps stdout; CSV is the durable
record). All children tracked + reaped on exit and on panic (Drop guards).

## 3. Observability — the stability signal (R8)

Per sample (default every 15 s):

| Column            | Source                                   | Bound asserted |
|-------------------|------------------------------------------|----------------|
| rss_kb            | /proc/<pid>/status VmRSS                  | flat (no leak) |
| vmhwm_kb          | /proc/<pid>/status VmHWM (peak)           | plateaus       |
| fds               | count /proc/<pid>/fd                      | flat/bounded   |
| threads           | /proc/<pid>/status Threads                | flat (pool)    |
| accept_inflight   | /metrics gauge (per listener)             | ≤ cap, returns |
| quic_modeb_connections     | /metrics gauge                  | returns to ~0  |
| quic_modeb_streams_active  | /metrics gauge                  | ≤ MAX_RELAY_STREAMS, returns |
| quic_modeb_datagrams_dropped_total | /metrics counter        | bounded slope  |
| quic_passthrough_flows     | /metrics gauge                  | ≤ cap, returns |
| quic_passthrough_flows_evicted_total | /metrics counter      | bounded        |
| panic_total       | /metrics counter                          | stays 0        |

Note on "task count": there is no in-product tokio-task gauge; the runtime
worker pool is fixed-size, so **Threads** (/proc) is the runtime-thread signal,
and the **state-table gauges** (accept_inflight, modeb/passthrough connections &
streams) are the per-connection/per-stream task proxies. This is the standard
external-soak approach; we measure what the process actually holds. (Adding a
product-side task gauge is out of scope for S20 — minimal product change, R3.)

### Trend verdict logic (timeseries.rs)

For each gauge/RSS series: drop a warmup prefix (first 10% of samples), then
compare the median of the first steady-state third vs the last third.
- **BOUNDED** if last-third median ≤ first-third median × (1 + band) where band
  = 0.10 for RSS, OR the series returns to within band of baseline after the
  load/churn phase (for tables that should drain to ~0).
- **DRIFT/LEAK** if last-third median exceeds first-third by > band AND the
  per-window slope is consistently positive (monotone-ish across ≥3 windows),
  i.e. growth that does not plateau. Reported with the slope + Δ + the raw
  series so the verdict is auditable (R15 evidence). Counters (dropped/evicted)
  are assessed by slope flattening, not absolute value.

A clean scenario is a CLAIM that needs evidence (R13): its `verdict.json`
carries the full series + the BOUNDED computation, not "looked fine".

## 4. Scenarios (first soak)

Each scenario = its own gateway child + config + load/chaos + sampler + CSV.
Run concurrently on distinct ports (8 cores; co-located OK — measuring
stability, not throughput).

| ID | Datapath / cells | Load | Chaos | Priority target |
|----|------------------|------|-------|-----------------|
| sc1 | H1 front → H1 backend | sustained keep-alive + churn, mixed body sizes | conn-flood | 9-cells/Mode-mixed (#4) |
| sc2 | H1s/h2 front → H2 backend | H2 multi-stream sustained | rapid-reset (CVE-2023-44487) + stream-flood | #4 + standard chaos |
| sc3 | H1 front → H1 backend | low steady | slowloris + slow-POST (held conns) | standard chaos (fd/timeout reaping) |
| sc4 | QUIC Mode B (terminate) → QUIC backend | sustained bidi streams under FC backpressure + churn | datagram-flood | #1 (S18 stall-fix FIN path) + #2 (Mode B bounds) |
| sc5 | QUIC Mode A (passthrough) → QUIC backend | many flows + flow churn | flow-flood / source variation | #4 (Mode A) |
| sc6 | H1s front → H1 backend | oversize requests → 413 | mid-response client disconnect / TLS teardown race | #3 (CF-S19-TLS-TEARDOWN-413) |

Coverage honesty (R2/R4 — no asterisks): the first soak drives the **TCP-front
cells (H1/H2 × H1/H2)**, **Mode A**, **Mode B**, and the named priority chaos.
The **H3-front cells (H3→H1/H2/H3)** are NOT separately load-driven in this
first soak — Mode B already exercises QUIC-terminate state, and the functional
H3 paths are covered by the S7–S13 md_streaming_verify suites; a dedicated
H3-front load driver is scoped to S21 and stated as such in the report (not
claimed as covered). **XDP/L4** soak needs root + multi-kernel (F-ESC-1) and is
out of scope for this userspace-datapath soak; stated, not asterisked.

## 5. Duration & cadence

- Apparatus smoke: ~3–5 min on sc1 to validate end-to-end before committing the
  long run (catches a broken apparatus early).
- Primary soak: **≥120 min** (hours-class; post-warmup leaks show after warmup,
  R15). Sampler every 15 s ⇒ ~480 samples/scenario — ample for a trend. Extend
  if budget allows; if a target needs longer to be conclusive, say so honestly
  (no over-claim).
- The run is detached/background with periodic CSV snapshots as its heartbeat
  (R9 — a quiet soak is expected, not a hung agent). Verdict read ONLY at
  completion (R15).

## 6. Process / roles

- **lead** (this agent): designs + builds the apparatus (test infra, not product
  feature code), runs the primary soak, reads it to completion, owns
  report+verdict, enforces R15.
- **verifier** (separate agent): independently (a) reviews apparatus measurement
  correctness, (b) reproduces every finding with its OWN run before it's
  believed (R13), (c) confirms clean scenarios via time-series.
- **builder-1** (separate agent): fixes only cheap+tractable findings (product
  code), each with a regression test, each verifier-confirmed.

Author ≠ verifier is preserved for every finding. Gates run foreground and are
read to completion before any number is reported (R15 / never-report-unread).

## 7. Hygiene (R9)

CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target (exported per command),
CARGO_INCREMENTAL=0, ≥25 GB free with periodic checks, soak logs bounded/rotated,
non-root, no sudo, no git stash, all spawned children reaped at session end.

## 8. Outputs

- `audit/soak/s20-report.md` — design, run params+duration, per-scenario
  time-series verdicts, every finding (mechanism+tier+reproduced-by+fixed/carried),
  every clean scenario with bounded evidence, R1 ×3 baseline, prioritized S21
  fix list.
- `audit/soak/s20-soak-data/` — raw CSV time-series + verdict.json per scenario
  (the R15 evidence behind every verdict; version-controlled).
