# S14 — CF-BODY-WALLCLOCK fix plan (PLAN ONLY; authored S13, no compiles)

> **Status: PLAN ONLY.** Authored as the S14 head-start while the H2→H3
> verifier runs the ×3 `--workspace` gate + llvm-cov + h2_proxy.rs mutations in
> the shared target dir. **No `cargo` ran for this document; no source edited.**
> Every site/line below was confirmed by READING the tree at
> `feature/h-matrix-s13` (builder-1 worktree). Build-vs-defer is the owner's call
> AFTER H2→H3 verifies; this plan does not authorize a build.

CF-BODY-WALLCLOCK ([[cf-body-wallclock]]): the H1/H2 request-egress legs bound
the upstream send with a **fixed wall-clock** `timeouts.body`. A large but
**actively-progressing** slow upload that exceeds that wall-clock is aborted as
`ProxyErr::Timeout` (504) even though bytes are still flowing — the exact defect
class F-S7-6 already fixed for the H3 connector (an 8 MiB response truncated at
~4.37 MiB by a 5 s wall-clock; replaced by a no-forward-progress idle deadline).
The fix model is F-S7-6's idle deadline (`H3_RESP_IDLE_TIMEOUT`,
`h3_bridge.rs:166-195`), but it does NOT port trivially because the L7 cells
delegate the byte transfer to an OPAQUE hyper `send_request` future that cannot
be observed per-byte from outside. This plan specifies HOW to derive a progress
signal, WHERE to instrument it, the single-source strategy, the R8/backpressure
argument, and a per-cell load-bearing verification design.

---

## 0. Confirmed topology — where the wall-clock actually lives (READ, not re-derived)

The lead's discovery was verified by reading; the line numbers below are the
CURRENT tree (the lead's brief cited a couple of pre-S13-rewrite line numbers —
corrected here). **Critically, the four cells split into TWO mechanism classes**,
and the wall-clock does NOT sit in the same place for both:

### Class A — H1-EGRESS cells (the wall-clock wraps the L7's own `send_request`)
The body wall-clock is `tokio::time::timeout(self.timeouts.body, send_fut)` in
lb-l7, wrapping a **hyper H1-client** `sender.send_request(req)`:

| Cell | Method | Wall-clock site(s) | Streaming? |
|------|--------|--------------------|------------|
| **H1→H1** | `h1_proxy.rs::proxy_request` (1190) | `h1_proxy.rs:1508` (send) + `:1523` (verdict-rx backstop) | **Branch-B-only** — always streams via the detached pump (`h1_proxy.rs:1220` "Branch-B-only, no lookahead") |
| **H2→H1** | `h2_proxy.rs::proxy_request` (1379) | `h2_proxy.rs:1548` (Branch A buffered send), `h2_proxy.rs:1866` (Branch B streaming send) + `:1881` (verdict-rx backstop) | Branch A (≤window buffered) **+** Branch B (streaming pump) |

For an H1-client `send_request`, the returned future resolves when the **response
head** arrives. The request body streams concurrently out of the pump→channel→
hyper. So `timeout(timeouts.body, send_fut)` bounds *time-to-head*, which for a
backend that withholds its head until it has consumed the whole upload is
effectively a whole-upload wall-clock → **this is where the bug bites**.

### Class B — H2-EGRESS cells (the wall-clock lives in the POOL, not lb-l7)
The body wall-clock is NOT in `drive_h2_upstream_send`; it is
`tokio::time::timeout(self.inner.config.send_timeout, send_fut)` inside
**`lb-io::http2_pool::Http2Pool::send_request`** (`http2_pool.rs:232`,
`send_timeout` default 30 s — `http2_pool.rs:85`,`:391`):

| Cell | Method | Wall-clock site | Notes |
|------|--------|-----------------|-------|
| **H1→H2** | `h1_proxy.rs::proxy_h1_to_h2_request` (1597) → `drive_h2_upstream_send(.., self.timeouts.body)` (`h1_proxy.rs:1901-1908`) | `http2_pool.rs:232` (`send_timeout`) | shares the driver |
| **H2→H2** | `h2_proxy.rs::proxy_h2_to_h2_request` (1998) → `drive_h2_upstream_send(.., self.timeouts.body)` (`h2_proxy.rs:2317-2324`) | `http2_pool.rs:232` (`send_timeout`) | shares the driver |

`drive_h2_upstream_send`'s own `body_timeout` (the param at `h2_proxy.rs:2913`)
is used ONLY at `h2_proxy.rs:3003` to bound the **F-CAP-1 verdict consultation**
(`tokio::time::timeout(body_timeout, &mut verdict_rx)`), NOT the send. The send
is raced in a `tokio::select!` against the verdict (`h2_proxy.rs:2948-3016`); its
timeout is the pool's `send_timeout`. **So for the H2-egress cells the
progressing-upload truncation is `http2_pool.rs:232`'s header-roundtrip timeout,
and a fix that only touches lb-l7 does NOT reach it.**

### The verdict-rx backstops are NOT the bug (leave them fixed, or rename)
`h1_proxy.rs:1523`, `h2_proxy.rs:1881`, `h2_proxy.rs:3003` all bound a
**post-error verdict consultation** (a wedged-pump liveness backstop), reached
only AFTER `send_request` already errored. They never truncate a progressing
upload. They should KEEP a fixed (small) bound — converting them to idle would
be wrong (there's no progress to measure on a dead error path). The plan touches
them only to STOP reusing the now-repurposed `timeouts.body` name if that causes
confusion (optional; see §3 note).

---

## 1. The progress signal — how to observe an opaque hyper send

F-S7-6 can reset `idle_deadline` because the H3 connector is a hand-rolled event
loop that sees every `stream_send`/`stream_recv`. The L7 cells can't see inside
hyper's `send_request`. BUT every Class-A/Class-B cell already owns the thing
that DOES see forward progress: **the detached ingress pump** that reads
`body.frame()` and pushes each chunk into the bounded channel hyper drains
(`h1_proxy.rs:1442/1473` H1→H1; `h2_proxy.rs:1852`-region + the `send_chunked!`
macro H2→H1 Branch B; the H1→H2 / H2→H2 pumps feeding the `H2ReqBody` channel).

**A successful `tx.send(Ok(frame)).await` is a forward-progress event**: it only
resolves once hyper has pulled the *previous* chunk out of the bounded channel
(the channel depth is the in-flight window). That is precisely "the upstream
accepted more bytes" — the same semantic as F-S7-6's `stream_send n>0`.

**Mechanism (validated against the existing pumps):**
- Introduce a shared `last_progress: Arc<AtomicU64>` (millis since an epoch
  captured at request start, via `tokio::time::Instant` deltas — NOT
  `SystemTime`, to stay monotonic and test-clock-friendly under
  `tokio::time::pause`).
- The pump bumps `last_progress` to "now" on:
  - each chunk successfully handed to the channel (the `send_chunked!`/`tx.send`
    success path — the exact sites that already bump `in_flight_bytes` for the
    R8 gauge, so the instrumentation is co-located and provably on the
    forward-progress path), AND
  - the terminal `End{}`/clean-EOF send (so a fast small upload that finishes
    well inside the idle window is never mis-aborted while waiting for a slow
    backend head — see §1.1).
- Replace the fixed `timeout(timeouts.body, send_fut)` with a **select-loop**:
  arm a timer for `last_progress + idle_timeout`; on fire, re-check
  `last_progress`; if it advanced since the timer was armed, RE-ARM (progress
  happened); only if `now - last_progress >= idle_timeout` with the send still
  pending → abort with `ProxyErr::Timeout`. Equivalent to F-S7-6's
  `send_progress!` reset, expressed as an outer idle-watchdog because we can't
  reset from inside hyper.

### 1.1 The response-head-wait sub-case (the genuinely hard part — FLAG)
There are TWO progress directions and the pump only sees the REQUEST one:
1. **Request upload in flight** — covered by the pump signal above.
2. **Upload finished (clean End sent), now waiting for the backend's response
   HEAD** — the pump is done; there is NO further pump progress to observe, yet
   the upload is legitimately complete and the backend may be slow to respond.

If the idle watchdog kept ticking off `last_progress` after the upload finished,
a slow-responding backend would be idle-aborted — turning a wall-clock-on-upload
bug into a wall-clock-on-head bug. So the plan MUST special-case "upload
complete": once the pump signals its terminal `End{}`/clean verdict, the
head-wait should fall back to a SEPARATE fixed bound (a head-roundtrip timeout,
the same role the pool's `send_timeout` plays for Class B). i.e. the idle
deadline governs the UPLOAD phase; a fixed `head_timeout` governs the
post-upload HEAD-wait phase. This is a deliberate two-phase design, not a single
idle timer. **This is the §"flag" the lead asked for: the opaque hyper send
makes the head-wait leg un-idle-able; we bound it with a fixed head timeout, not
an idle one.** (F-S7-6 didn't face this because the connector multiplexes both
directions on one loop.)

### 1.2 The RESPONSE-read leg — out of scope, already idle-bounded for H3, and
NOT wall-clock for H1/H2
The body wall-clock under discussion is the REQUEST-egress send. The
response-BODY read leg is NOT wrapped in `timeouts.body` for any of the 4 cells
(the H1/H2 response bodies stream via `upstream_response_to_h1` /
`upstream_h2_response_to_h2` / the boxed `Incoming`, governed by hyper's own
read + the connection idle, not a `timeouts.body` cap). So there is no
symmetric response-leg wall-clock to fix in these 4 cells. (The →H3 response
leg already uses the F-S7-6 idle deadline.) The plan should STATE this
explicitly so the verifier doesn't hunt for a response-leg site that isn't
there.

---

## 2. Per-cell exact sites + instrumentation points

### H1→H1 (`h1_proxy.rs::proxy_request`)
- Pump: the `tokio::spawn` block ending `h1_proxy.rs:1502`; progress bump at
  each `tx.send(Ok(frame)).await` success (data + forwarded-trailers frame).
- Replace the `timeout(self.timeouts.body, send_fut)` at **`h1_proxy.rs:1508`**
  with the two-phase idle-watchdog/head-timeout select-loop (§1).
- Leave `:1523` verdict-rx backstop fixed.

### H2→H1 (`h2_proxy.rs::proxy_request`)
- Branch A (`h2_proxy.rs:1548`): body is ≤ the 64 KiB lookahead window
  (`concat_chunks`), sent in ONE buffered `send_request`. A within-window body
  CANNOT be a "slow large progressing upload" → the wall-clock here is harmless.
  **Decision: convert it to the fixed `head_timeout` (rename/semantic only) for
  consistency, but it is NOT a load-bearing fix site.** Document so the verifier
  doesn't expect a Branch-A negative control to flip.
- Branch B (`h2_proxy.rs:1866`): the streaming pump (`tokio::spawn` ending
  `:1860`, `send_chunked!` macro). Progress bump at each channel push; replace
  the `timeout(self.timeouts.body, send_fut)` at **`:1866`** with the two-phase
  loop. Leave `:1881` verdict backstop fixed. **This is the H2→H1 load-bearing
  site.**

### H1→H2 + H2→H2 (Class B — the POOL owns the wall-clock)
The lb-l7 sites (`drive_h2_upstream_send`) do NOT wall-clock the send. The
truncation is **`lb-io::http2_pool::send_request`** `:232`. Two options:

- **Option B1 (preferred, single-source + correct layer):** add an idle-aware
  send path to `Http2Pool`. The H2→H2 / H1→H2 pumps already push into the
  `H2ReqBody` channel; thread a `last_progress: Arc<AtomicU64>` from the pump
  (constructed in `proxy_h2_to_h2_request` / `proxy_h1_to_h2_request`) THROUGH
  `drive_h2_upstream_send` INTO a new `Http2Pool::send_request_idle(addr, req,
  last_progress, idle, head_timeout)` that replaces the fixed
  `timeout(send_timeout, send_fut)` at `http2_pool.rs:232` with the same
  two-phase select-loop. The existing `send_request` stays (used by Branch-A
  buffered callers / other paths) — or is re-expressed in terms of the new one
  with `idle == head_timeout == send_timeout` so there is ONE implementation.
- **Option B2 (lb-l7-only, rejected):** wrap the driver's `send_fut` in an
  lb-l7 idle loop. REJECTED: it double-bounds (pool's 30 s send_timeout still
  fires underneath) and splits the timeout policy across two crates (R14
  divergence risk + cross-crate gate scope, [[cross-crate-gate-scope]]).

**This is the cross-crate flag:** the H2-egress fix CANNOT be lb-l7-only; it
must land in lb-io `http2_pool.rs`. The fmt/clippy/gate scope is therefore
workspace-wide ([[cross-crate-gate-scope]]). Confirm with the pool owner that
`send_timeout` semantics may change from "header roundtrip wall-clock" to
"upload-idle + head-roundtrip" (a behavior change to a public-ish pool knob).

---

## 3. Single-source strategy (R12/R14 — sibling divergence is a close-blocker)

Three distinct send shapes ⇒ at most THREE implementations, ideally driven by
ONE shared idle-watchdog helper:

- **`idle_bounded_send` helper** (new, shared): given a pinned send future, a
  `last_progress: Arc<AtomicU64>`, an `idle: Duration`, and a `head_timeout:
  Duration`, runs the two-phase select-loop (§1) and returns
  `Result<T, TimedOut>`. Generic over the future's output so BOTH the H1-client
  `send_request` future (Class A) and the pool send future (Class B, Option B1)
  use the SAME loop body. Place it where both crates can reach it — candidate:
  a small `lb-io` util (lb-l7 depends on lb-io) OR an `lb-core` timing util.
  Decide by where `Arc<AtomicU64>`+`Instant` math is cleanest; prefer lb-io so
  the pool path uses it natively and lb-l7 imports it.
- **Class A** (H1→H1, H2→H1 Branch B): both call `idle_bounded_send` around the
  H1-client `send_fut`, with the pump bumping the shared `last_progress`. ONE
  call shape, two call sites.
- **Class B** (H1→H2, H2→H2): both already funnel through
  `drive_h2_upstream_send`; route the new `last_progress` through it into
  `Http2Pool::send_request_idle` which calls `idle_bounded_send`. ONE pool
  method, ONE driver, two call sites.

Net: ONE watchdog implementation, reused by both classes. The R14 audit reduces
to "all four cells import the same helper and pass a pump-fed `last_progress`".

**Verdict-rx backstops** (`h1_proxy.rs:1523`, `h2_proxy.rs:1881`, `:3003`): keep
fixed; they are not the bug. If `self.timeouts.body` is repurposed as the idle
budget, give the backstop its own small fixed const (e.g. a
`VERDICT_BACKSTOP = 5 s` mirroring `H2_ABORT_OBSERVE_TIMEOUT`) so the rename
doesn't silently change the backstop. (Minor; flag in the build.)

---

## 4. R8 / backpressure preservation argument

The fix must NOT weaken the R8 bounded-window backpressure the cells already
prove. Argument:
- `last_progress` is an `AtomicU64` bumped on the EXISTING channel-push success
  path — it reads the same event the R8 in-flight gauge reads, adds no buffering,
  holds no bytes. Zero new retained memory.
- The idle watchdog is an OUTER timer; it does not poll or drain the body
  channel, so it cannot change the pump↔hyper↔backend flow-control chain. When
  the backend stalls, the pump parks on a full channel exactly as today — and
  because a parked pump means NO channel push, `last_progress` correctly stops
  advancing, so the idle deadline DOES eventually fire on a genuine wedge (the
  desired liveness: a dead-but-connected upstream is still aborted within
  `idle`, never an infinite hang — same property F-S7-6 guarantees,
  `h3_bridge.rs:181-187`).
- Backpressure-park is NOT progress: we bump only on `tx.send` SUCCESS (a chunk
  actually left into the window), never on park/poll-pending — mirroring
  F-S7-6's R-S76-5 "never reset on backpressure parks". A slow-but-progressing
  upload bumps on every accepted chunk → never idle-aborts. A wedged upload stops
  bumping → idle-aborts at `idle`. Exactly the F-S7-6 contract.

---

## 5. Verification design — per-cell load-bearing negative control

Mirror the F-S7-6 proof shape and the streaming-cell BUILT-bar discipline. For
EACH of the 4 cells (H1→H1, H2→H1, H1→H2, H2→H2):

### (a) Slow-but-progressing large upload SUCCEEDS post-fix / 504 pre-fix (THE load-bearing arm)
- Backend: reads the request body to completion then 200-echoes. Gateway
  `timeouts.body` (Class A) / pool `send_timeout` (Class B) set SHORT (e.g. 2 s)
  so the OLD wall-clock would fire.
- Client: streams a body LARGER than `rate × 2 s` but paced so each chunk lands
  within `idle` (e.g. a chunk every 500 ms for 6 s total = 12 chunks, total
  wall-clock 6 s ≫ the 2 s bound, but max inter-chunk gap 500 ms ≪ `idle`).
- POST-FIX assert: status 200, body byte-identical, backend saw COMPLETE. This
  upload provably exceeds the old wall-clock yet succeeds → proves the idle
  semantics.
- LOAD-BEARING proof (the verifier runs it): with the fix reverted (restore the
  fixed `timeout(2 s, send_fut)`), this SAME test must 504 — i.e. the test FAILS
  pre-fix. Design the pacing so `total_wallclock > body_timeout` is unambiguous
  (6 s vs 2 s). This is the per-cell negative control the lead requires.

### (b) Genuine fast stall STILL 504s promptly (the liveness arm — must not regress)
- Backend: accepts the head/dial, reads the FIRST chunk, then WEDGES (never
  reads more, never responds) — a dead-but-connected upstream.
- Client: sends one chunk, then stalls (no more data, no FIN).
- Assert: the gateway aborts with `ProxyErr::Timeout` (504) within ~`idle`
  (+ slack), NOT after an unbounded hang and NOT a clean 200. Proves the idle
  watchdog still fires on a real wedge (the property the wall-clock gave us,
  preserved). Use a SHORT `idle` in the test so the wedge arm is fast.
- Non-vacuity: assert it did NOT 504 in <100 ms either (it waited ≈`idle`),
  so the test isn't trivially passing on an unrelated early error.

### (c) Within-window / fast clean upload unaffected (regression guard)
- A small (≤ window for H2→H1 Branch A) clean upload → 200, no false 504. Proves
  the common path is untouched and Branch A (which we only rename) still works.

### (d) Burst / saturation hygiene (CF-SATURATION-1, [[gate-saturation-test-fragility]])
- The slow-progressing arm uses real wall-clock sleeps; under 8-core gate
  saturation a too-tight `idle` could false-fire. Choose `idle` and the client
  pacing with a wide margin (pacing gap ≤ `idle/3`), and give the SUCCESS arm a
  generous client/overall deadline so saturation cannot false-504 it. Run the
  load-bearing arm on `current_thread` + a small repetition (≥10) per
  [[parallel-gate-masks-smuggle]] discipline, since this is a timing-race fix.

### (e) Test clock
- Prefer `tokio::time` virtual clock (`start_paused`) for the deterministic
  arms where the backend mock can be driven on the paused clock; fall back to
  real sleeps with generous margins where a real hyper/h2/pool send must run on
  the real timer (the pool's internal `timeout` uses the real Tokio timer unless
  the test runtime is paused — verify which during build). Document per-arm which
  clock is used so the verifier can reproduce.

### Coverage / gates
- fmt + clippy **workspace-wide** (spans lb-l7 + lb-io — [[cross-crate-gate-scope]]).
- Scoped llvm-cov over the new `idle_bounded_send` + each touched send site
  ([[llvm-cov-session-scope-method]], `--workspace` for the dep-crate lines per
  [[llvm-cov-workspace-for-depcrate-lines]]).
- R3: the existing F-CAP-1 (413/400/502) + F-MD-4 smuggle arms of all 4 cells
  must stay green (the verdict-backstop and abort paths are untouched).

---

## 6. Honest hour estimate

- **Build** — shared `idle_bounded_send` helper + two-phase head-timeout design,
  thread `last_progress` through 2 pumps + the pool, `Http2Pool::send_request_idle`
  (Option B1, the cross-crate piece), 4 call sites rewired: **4-6 h** (the pool
  change + the head-wait two-phase split are the time sinks; the helper itself is
  small).
- **Test suite** — per-cell (a)+(b)+(c) arms × 4 cells, with paced-upload + wedge
  mock backends for H1-client AND H2-pool egress, the load-bearing revert proof
  framed per cell, saturation-safe pacing: **5-7 h** (the timing arms are
  fiddly; the wedge/liveness arm must be fast-yet-non-vacuous).
- **Independent verify** (author≠verifier) — ×3 `--workspace` gate + the per-cell
  revert mutation (each (a) arm must 504 pre-fix) + the pool-semantics
  cross-check + coverage: **3-4 h**.
- **TOTAL ≈ 12-17 h.** This is a genuine cross-cutting, cross-crate change (4
  cells, 2 crates, a public-ish pool-knob semantic change, a non-trivial
  two-phase timer), NOT a small fix — consistent with the owner's honest-stop
  framing (matrix-complete 9/9 + this ready plan beats a rushed S13 attempt).

---

## 7. Risks / open questions for the owner

- **R-CFBW-1 (cross-crate, the big one):** the H2-egress fix changes
  `lb-io::http2_pool` `send_timeout` semantics from "header-roundtrip wall-clock"
  to "upload-idle + head-roundtrip". Needs pool-owner sign-off; it touches a
  shared infra crate other paths may rely on. If the owner wants lb-io frozen,
  the H2-egress cells can't be fixed this way → either Option B2 (rejected,
  double-bound) or defer the 2 H2-egress cells and fix only Class A — but that
  would be a deliberate R14 sibling divergence the owner must bless.
- **R-CFBW-2 (head-wait phase):** the two-phase idle-then-fixed-head-timeout is
  the part with no F-S7-6 precedent (§1.1). The fixed `head_timeout` value (and
  whether it should equal the old `timeouts.body`) is a product decision.
- **R-CFBW-3:** H2→H1 Branch A (`h2_proxy.rs:1548`) and the verdict backstops
  are NOT load-bearing fix sites; touching them is consistency-only. Confirm the
  owner wants the rename rather than leaving them as-is (less churn, but a
  visible sibling inconsistency an R14 reviewer would flag either way).
- **R-CFBW-4 (test clock):** the pool's internal `tokio::time::timeout` and a
  real hyper send may not honor a paused test clock cleanly; some arms may need
  real sleeps with generous margins (saturation risk). Resolve empirically at
  build (NOT now — no compiles permitted this task).
- Carry-forward note: F-S7-6's doc (`h3_bridge.rs:191-194`) already flags
  `request_h3_upstream`'s OWN fixed 30 s wall-clock (CF-S7-RHU) as the same bug
  class on the →H3 request leg. That is a FIFTH site, out of this 4-cell scope,
  but worth folding into the same S14 sweep if the owner authorizes the build —
  flag it so the matrix-wide "no wall-clock body truncation anywhere" property is
  closed in one session rather than leaving a known sibling.
