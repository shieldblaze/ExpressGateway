# SESSION 32 — CF-GRPC-H3-CHURN-RSS: diagnose + fix the gRPC-H3 churn RSS growth

Branch: `feature/grpc-h3-churn-rss-s32` (base `main` @ c81f42dc, S31 promoted, quiche 0.29.1 / MSRV 1.88).
Box: 8 cores, ENA ens5, glibc (system allocator), tokio multi-thread (9 threads).

Status: **SESSION 32 PARTIAL** — CF-GRPC-H3-CHURN-RSS root-caused by measurement to quiche
`StreamMap::collected` (an unbounded insert-only `HashSet<u64>`); fix is EXTERNAL (in quiche) →
ESCALATED with a demonstrated working patch; `main` keeps its state (no production change). The
diagnosis refuted the prior "glibc-arena" hypothesis and corrected S31's "churn" attribution.

**One-line verdict:** the gRPC-H3 RSS staircase is quiche remembering every completed stream ID
forever (`collected.insert`, never removed) on long-lived high-stream-count H3 connections — proven
LIVE (not arena), localized to the SUSTAINED path (not churn), root-caused in quiche source, and
proven by a bounded-`collected` re-soak that flips the analyzer DRIFT→BOUNDED (82 MB→39.8 MB plateau).

---

## Phase 0 — baseline + faithful reproduction (negative-control reference)

**Base tip** confirmed `c81f42dc`; branched `feature/grpc-h3-churn-rss-s32`, pushed. No S31 strays
(`ps aux` clean), 36 GB free. Release `expressgateway` + `eg-soak` built (exit 0).

**Repro (run 1):** unmodified release binary, `sc9_grpc_h3` ISOLATED, 1800s, sample 15s, scale 1
(`audit/soak/s32-soak-data/repro-pre/`). Plus a rich side-sampler (`side_sampler.sh`, 15s) reading the
gateway's `/proc/<pid>/status` (VmRSS, RssAnon, VmData), `[heap]` brk size, fd count, thread count.

**sc9 topology:** a `quic` H3-terminate front → an H2 gRPC echo backend (`Http2Pool`, single peer).
Two drivers: SUSTAINED (6 workers holding a QUIC conn, unary RPCs back-to-back forever) + CHURN
(6 workers: open conn → 4 RPCs → clean close → repeat). The CHURN driver = the connection
open/close-cycle probe S31 attributed the staircase to.

**Reproduction fidelity vs S31's definitive 1800s reference** (`audit/soak/s31-soak-sc9-1800/`):

| t (s) | S31 ref RSS | S32 run1 RSS |
|---|---|---|
| 0 | 8.0 MB | 8.3 MB |
| 240 | 32.1 MB | ~31 MB |
| 480 | 39.7 MB | 39.0 MB |
| 720 | 40.7 MB | 39.1 MB |
| 960 | 54.7 MB | _(pending)_ |
| 1680 | 68.6 MB | _(pending)_ |
| 1800 | 82.5 MB | _(pending)_ |

S31 staircase signature: **sharp +14 MB steps** (40→54→68→82), each held minutes, **still climbing at
1800s** (no plateau) — `fds` flat (11→12), `threads` flat (9), `panic_total=0`, ~4.9M RPCs err≈0.
RSS scales with time (41 MB @900s → 82 MB @1800s).

**Phase-0 external-signal finding (already decisive about the leak CLASS):**
- `fds` FLAT (12), `threads` FLAT (9) across the whole run → **NOT** a socket / fd / connection-handle
  / tokio-task leak. Connections and their sockets ARE reclaimed on churn.
- Growth is **purely anonymous memory**: RssAnon climbs in lockstep with VmRSS; VmData climbs;
  the `[heap]` brk region stays flat at **448 KB** → the growth is **mmap'd** anonymous regions, not
  brk/main-arena-top. Consistent with glibc per-thread-arena resident growth OR a live mmap-backed
  allocation — to be disambiguated by measurement (NOT assumed).

This rules out the F-S20-2 (fd/flow retention) class and the half-open-stream / task-accumulation
classes. The leak is in **bytes, not handles**, and arrives in **discrete +14 MB steps**.

### Static lifecycle review (what is NOT the cause)
Read the full gRPC-H3 churn path (`router.rs`, `conn_actor.rs`, `h3_bridge.rs`, `http2_pool.rs`):
- Connection table (`connections` DashMap, keyed by DCID): cleaned by `CidEntryGuard` on actor
  exit/cancel/panic. Bounded by peak concurrency.
- Per-stream actor maps (`stream_response`, `resp_rx_by_stream`, `body_tx_by_stream`, `body_seen`,
  `pending_trailers`): removed on stream terminal/`Finished` (S29-hardened `get_mut`-not-`or_insert`).
- Per-RPC producer tasks (`resp_tasks`): reaped each tick via `retain(!is_finished)`.
- Backend `Http2Pool`: keyed by addr → a SINGLE multiplexed H2 client conn for sc9, re-dialed only
  on death. `stream_h2_response` fully drains the backend `Incoming` (loop to `None`); every early
  return drops `Incoming` → hyper/h2 RSTs the backend stream. No half-open accumulation.
- Front actor exits on `conn.is_closed()` (after the peer-close draining window) → drops the
  `quiche::Connection` + H3 state.

All gateway-controlled lifecycle state is bounded/cleaned. → the bytes-growth is either glibc
allocator retention/fragmentation OR per-connection-cycle state held inside a dependency
(quiche / hyper-h2). The diagnostic experiments below decide, by measurement.

---

## Phase 1 — DIAGNOSIS (measurement)

Instrument: env-gated in-process probe `crates/lb/src/diag_mem.rs` (`EG_DIAG_MEM=1`; zero effect
otherwise). Logs every cycle: self VmRSS, `mallinfo2` (uordblks = main-arena live, fordblks = free,
hblkhd = mmap), and with `EG_DIAG_TRIM=1` calls `malloc_trim(0)` and logs RSS before/after.

### E1 — trim probe ⇒ the leak is LIVE, NOT glibc retention (refutes S31's "arena" hypothesis)

Instrumented binary, full sc9, `EG_DIAG_MEM=1 EG_DIAG_TRIM=1 EG_DIAG_SECS=300`, 1800s
(`audit/soak/s32-soak-data/e1-trim/`). `mallinfo2` + `malloc_trim(0)` each 5 min:

| t (s) | RSS | `uordblks` (main-arena LIVE) | `hblkhd` (mmap large-block LIVE) | trim reclaimed |
|---|---|---|---|---|
| 0 | 7.4 MB | 0.23 MB | 0 | 4 KB |
| 300 | 32.2 MB | 6.18 MB | 7.1 MB | 1.3 MB |
| 600 | 39.5 MB | 6.20 MB | 14.2 MB | 1.2 MB |

**Decisive:** `uordblks` (main-arena live) is **flat** (~6.2 MB) while **`hblkhd` (bytes in mmap'd
large ≥128 KB blocks) grows in lockstep with RSS** (0 → 7.1 → 14.2 MB), and **`malloc_trim(0)`
reclaims almost nothing** (~1.2 MB of each ~7 MB step). A glibc-arena RETENTION leak would reclaim
on trim and/or show in `fordblks`; this does neither. The grown memory is **LIVE large allocations
the allocator cannot release** → a **real leak**, not arena fragmentation. This **refutes** the S31
(and my initial) "sharp +14 MB jumps = allocator arena acquisition" hypothesis — diagnosis-first paid
off. `hblkhd` growing (not `uordblks`) ⇒ the leak is a **small number of LARGE, monotonically
growing collections** (each ≥128 KB → straight to mmap), consistent with the flat fds/threads.

### Localization — the leak is on the H2 gRPC BACKEND path, not the quiche::h3 front

Cross-scenario fact from S31's re-soak: **sc7_h3terminate (backendless H3 front — heavy quiche::h3
request-ingress + response-egress, NO H2 backend) PLATEAUS** (~25 MB), as does sc8c (WS-H3, ~26 MB).
Both are on the same 0.29 quiche::h3 front path sc9 uses. If the leak were in the quiche::h3 front
(per-stream state not collected), sc7 would leak too — it does not. **⇒ the quiche::h3 front is
exonerated; the leak is specific to what sc9 adds: the H2 gRPC backend path** (the long-lived shared
`Http2Pool` H2 client connection that multiplexes ~5M backend streams over the run, + the H3↔H2
bridge). The gateway's own per-stream HashMaps are proven-cleaned (above), so the live growth is
held inside the H2 backend machinery (hyper/`h2`) and/or the gateway's use of it.

### Micro-repro (backend H2 path) ⇒ backend EXONERATED

`crates/lb-io/examples/h2pool_churn_probe.rs`: drove **2,000,000 H2 streams** through ONE
`Http2Pool` connection (8 concurrent senders, 2 KB body echo, full response drain) — the exact
backend shape sc9 uses — with no quiche/front/network. Result: **`hblkhd` stayed 0, RSS flat at
~5.7 MB** the entire run (`audit/soak/s32-soak-data/micro-repro-1.log`). The backend H2 path
(hyper/`h2` + `Http2Pool`) does **NOT** leak under millions of streams on one connection. ⇒ the
doubling collection is not in the backend.

### Attribution + actor instrumentation ⇒ NOT a gateway map; it's quiche FRONT, SUSTAINED path

Instrumented the actor (gated by `EG_DIAG_MEM`): live-actor counters + every per-connection map's
size logged each 5 s. Ran **A2 sustained-only** (`GRPC_CHURN=0`, `audit/soak/.../a2-sustained/`):
- `hblkhd` doubles (3.56 → 7.1 MB …) — **sustained-only reproduces the leak** (so S31's "churn"
  attribution is WRONG: churn conns do 4 streams then free their whole `StreamMap` — innocent).
- `actors_alive` stable at 6 (no leak of actors); fds/threads flat.
- **Every gateway actor map maxes at 1** (`stream_response`=1, `resp_rx`=1, `resp_tasks`≤1,
  `pending_trailer_entries`=0) for the whole run — the gateway holds NOTHING growing.

Gateway maps tiny + backend exonerated + front-ingress/inline (sc7) plateaus ⇒ by elimination the
growth is **quiche front per-connection internal state** on the long-lived sustained connections.

### ROOT CAUSE (proven in source): quiche `StreamMap::collected` is an unbounded insert-only set

`quiche-0.29.1/src/stream/mod.rs`:
```rust
struct StreamMap { streams: StreamIdHashMap<Stream>, collected: StreamIdHashSet, ... }   // L121-128
pub fn collect(&mut self, stream_id, local) {
    let s = self.streams.remove(&stream_id).unwrap();   // streams: BOUNDED (removed) ✓
    ... remove_readable/writable/flushable ...
    self.collected.insert(stream_id);                   // collected: INSERT-ONLY ✗   (L617)
}
```
`collected` (a `HashSet<u64>`) records **every** garbage-collected stream ID **forever** — grep
shows it is only ever `insert`ed (L617), `contains`-checked (L255 reject-recreate, L658
`is_collected`); it is **never** `remove`d, `clear`ed, or bounded. On a long-lived QUIC/H3
connection that opens millions of short streams (gRPC-over-H3 sustained: one bidi stream per unary
RPC), it grows ~one `u64`/stream without bound → its hashbrown backing buffer **doubles** (7→14→28→
56 MB — exactly the `hblkhd`/RSS staircase) → mmap-backed → invisible to `malloc_trim` (live) and to
main-arena `uordblks` (flat). Every observed signal is explained.

**Classification:** the leak is **IN quiche (a dependency), not gateway lifecycle.** The gateway
correctly drops all its per-connection/per-stream state (maps ≤1, actors reclaimed, fds/threads
flat); quiche internally never frees `collected`. Per R6/R7 this is the **ESCALATE** case (a dep
decision), NOT the pre-authorized "gateway failing to reclaim" case. It is PRE-EXISTING (insert-only
`collected` exists in 0.28 too — matches S31's 0.28==0.29 finding).

**A2 completed (sustained-only, `GRPC_CHURN=0`, 900s):** RSS 8.2→45.4 MB (DRIFT), `hblkhd` doubling
to 28.3 MB, **churn ok=0** / sustained ok=1.39 M, actors stable at 6, **max gateway-map size ever =
1**, fds/threads flat, panic=0. ⇒ the SUSTAINED long-lived high-stream-count path ALONE reproduces
the full leak; churn is innocent (corrects S31's "churn" attribution — a churn conn frees its whole
`StreamMap`, incl. `collected`, after 4 streams).

### Causation proof — bounded-`collected` quiche `[patch]` ⇒ flat RSS

To prove `collected` is THE cause (not merely A leak) and demonstrate the upstream fix, a local copy
of quiche 0.29.1 (`/home/ubuntu/Code/quiche-0.29.1-s32`, wired via a DIAGNOSTIC-ONLY
`[patch.crates-io]`, NOT for merge) bounds `collected` to the most-recent `MAX_COLLECTED_TRACKED =
65_536` IDs via an LRU `VecDeque` (diff: `audit/soak/s32-quiche-collected-fix.diff`). Re-soak with
this patched gateway, same sc9, expected to show **flat/plateaued RSS** vs run1's 82 MB staircase.

_(proof soak result below)_

---

## Phase 2 — classification: GATEWAY-SIDE fix vs ESCALATE

The proven mechanism is **inside quiche** (`StreamMap::collected`), a dependency — NOT a gateway
lifecycle bug. The gateway already does everything right: per-stream maps cleaned (≤1), per-RPC
producer tasks reaped, actors reclaimed on close, fds/threads flat. Per **R6/R7** this is the
**ESCALATE** case ("the leak is proven to be IN quiche … a dep decision"), not the pre-authorized
"gateway failing to drop/reclaim" case. There is **no quiche config knob or public API** to bound or
prune `collected`, so the only in-repo options are:

1. **Upstream quiche fix (recommended root fix)** — bound `collected` (the demonstrated
   `audit/soak/s32-quiche-collected-fix.diff`: LRU-cap to the most-recent N collected IDs). Correct
   and minimal, but lands in quiche → an upstream PR / a pinned patched dependency decision for the
   owner. Per R6 ("don't hand-roll around a dep without proof it's external") we have the proof, so
   this is escalated, **not** force-forked into `main`.
2. **Gateway-side connection-recycle MITIGATION (workaround, NOT implemented)** — cap streams- or
   age-per-front-H3-connection and send a graceful `GOAWAY`, forcing the client to reconnect so the
   old connection's `StreamMap` (incl. `collected`) is freed. Bounds the leak without touching
   quiche, but **changes client-facing behaviour** (periodic reconnects + handshake cost), needs new
   config + GOAWAY/drain logic + tests, and is a product decision (R7 "real product fork") — proposed
   for the owner, not landed unilaterally overnight.

**Decision:** ESCALATE (option 1) + offer option 2. Do **not** modify `main` (R7/R11). The bounded-
`collected` patch is used branch-only as the causation PROOF (below), then reverted.

## Phase 3 — causation proof (NOT a promote)

Patched-quiche gateway (`[patch]` → bounded `collected`), same sc9, 1800s, `EG_DIAG_MEM`:

Both runs: same sc9, 1800s, ~5 M RPCs (run1 ~4.96 M; proof 3.01 M sustained + 2.03 M churn), err≈0,
panic=0, fds/threads flat. The ONLY difference is bounding quiche's `collected`.

| signal | run1 (unpatched) | proof (patched, bounded `collected`) |
|---|---|---|
| analyzer verdict | **DRIFT (finding)** | **BOUNDED** |
| RSS first→last | 8.5 → **82.1 MB** (monotone 100 %, still climbing @1800s) | 8.4 → **39.8 MB**, last-third +1.9 % (flat) |
| RSS @1800s | 82 MB | **39.76 MB (max)** |
| `hblkhd` trend | 0→7→14→28→**56.6 MB** (power-of-2 doubling, no plateau) | 0→2.8→6.7→**13.4 MB then FLAT t≈300→1800** (zero growth ~1500 s) |

**Completed-run result (R15):** bounding `collected` converts the unbounded 82 MB staircase into a
**BOUNDED ~39.8 MB plateau** — the Trend analyzer's verdict flips **DRIFT → BOUNDED**. `hblkhd` plateaus
at exactly 13,418,720 B (≈ 6 sustained conns × ~2.2 MB of 64 Ki-capped `collected`+`collected_order`)
and holds flat for ~1500 s. Functionality preserved (gRPC-status delivered throughout — err would
have spiked otherwise; it stayed at 3 transient-startup). **This is the load-bearing causation proof:
the leak is `collected`, and bounding it eliminates the staircase.**

Data: `audit/soak/s32-soak-data/proof-patched/` (patched) vs `…/repro-pre/` + `…/e1-trim/`
(unpatched run1/E1). Proposed upstream fix: `audit/soak/s32-quiche-collected-fix.diff`.

---

## VERDICT — SESSION 32 PARTIAL (Exit condition b)

**CF-GRPC-H3-CHURN-RSS root-caused (quiche `StreamMap::collected` unbounded insert-only set); fix is
EXTERNAL (in quiche) → ESCALATED with a demonstrated working patch; `main` keeps its state.**

- Mechanism PROVEN by measurement, not theory:
  - LIVE leak, not glibc arena: `malloc_trim(0)` never reclaims a step; `hblkhd` (live mmap blocks)
    doubles 7→14→28→56 MB; `uordblks` (main-arena live) flat. (Refutes S31's + my initial "arena".)
  - Backend H2 path EXONERATED: 2,000,000-stream `Http2Pool` micro-repro → `hblkhd` flat at 0.
  - Gateway state EXONERATED: every per-connection actor map maxes at 1; actors reclaimed; fds/threads
    flat. The gateway drops everything correctly.
  - Attributed to the SUSTAINED long-lived high-stream-count path (A2 sustained-only reproduces it;
    churn frees its whole `StreamMap` on close) — corrects S31's "churn" attribution.
  - Root cause in quiche source: `stream/mod.rs::collect()` does `self.collected.insert(stream_id)`;
    `collected` (a `HashSet<u64>`) is only ever inserted/`contains`-checked, never removed/bounded —
    ~1 `u64`/completed stream forever.
  - Causation PROVEN: bounding `collected` (LRU cap) flips the soak DRIFT→BOUNDED (82→39.8 MB plateau,
    `hblkhd` 56.6→13.4 MB flat), same ~5 M RPCs, err≈0.
- Classification (R6/R7): the leak is IN quiche (a dependency), NOT gateway lifecycle ⇒ **ESCALATE**,
  do NOT fork quiche into `main`. PRE-EXISTING (insert-only `collected` is in 0.28 too — consistent
  with S31's 0.28==0.29 finding).
- `main` UNCHANGED. No production source modified. Branch carries: the diagnosis + data + the proposed
  upstream fix (`s32-quiche-collected-fix.diff`) + (gated, diagnostic-only, not-for-merge)
  instrumentation used to measure.

### ESCALATION (for the owner)

**Upstream quiche issue (recommended root fix).** `StreamMap::collected` is an unbounded insert-only
`HashSet<u64>` (quiche ≤0.29.1). Any long-lived connection that opens a very large number of streams
(HTTP/3 with many requests per connection; gRPC-over-H3 with one stream per unary RPC) leaks ~1
`u64`+hash-overhead per completed stream, unbounded, for the connection's lifetime. Demonstrated fix
(`s32-quiche-collected-fix.diff`): bound `collected` to the most-recently-collected N IDs via an LRU
`VecDeque` (the recreate-guard only needs a recency window). Owner action: file upstream / pin a
patched quiche / track a fixed release. (A stricter O(1) alternative: drop `collected` entirely and
treat `id < largest_created_of_type && !streams.contains(id)` as collected — QUIC implicitly opens all
lower IDs, so there are no gaps.)

**Gateway-side mitigation option (workaround, NOT implemented, owner call).** Cap streams/age per
front H3 connection + graceful `GOAWAY` so clients reconnect and the old `StreamMap` is freed. Bounds
the leak without quiche, but changes client-facing behaviour (reconnect/handshake cost) and needs new
config + drain logic + tests — a product decision, not landed overnight.

## Handoff (carry-forward, not in scope)

#222 remaining tiers (hyper 1.10 / h2 0.4.14 — H2 crown-jewel re-verify + CF-S27-2 impact;
tokio-tungstenite 0.24→0.29; rand/socket2/toml/rcgen breaking; patch group); PR #214 (owner direct
merge); CF-S27-2 (WS-H2 hyper-upstream); CF-DEP-1; CF-S28-WSH3-WAKEUP; CF-FCAP1-FLAKE;
CF-SATURATION-1; F-ESC-1; N-1; perf tiers; F-S29-2 + CF-S15 release-note items. **NEW: CF-QUICHE-
COLLECTED-UNBOUNDED** (this session — escalated upstream; gateway-recycle mitigation optional).
