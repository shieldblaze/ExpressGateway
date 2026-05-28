# S14 Phase 2 (builder-1) — cell-wiring design (PLAN ONLY)

> **Status: PLAN ONLY — awaiting plan-approval before any `.rs` edit.**
> Branch: `feature/cfbw-s14-builder` @ `e33343cd` (Phase 1 integrated).
> Authoring agent: builder-1 (h-matrix-s14).
> Base reading: `audit/h-matrix/s14-cf-body-wallclock-plan.md` §2-5 (per-cell
> sites + single-source strategy + R8 preservation + load-bearing arms);
> `audit/h-matrix/s14-pool-eng-design.md` (Phase 1 helper / pool method API).
>
> Scope: wire ALL 4 H1/H2 cells onto Phase 1's
> `lb_io::idle_send::idle_bounded_send` (Class A) +
> `lb_io::http2_pool::Http2Pool::send_request_idle` (Class B), and add the
> new `head_timeout` knob (R-CFBW-2). Per-cell **R13** tests are explicitly
> Phase 3 (verifier); Phase 2 must keep R3 (existing tests) green.

---

## 0. Resolved owner rulings carried forward (do not re-escalate)

| Tag | Ruling |
|---|---|
| **R-CFBW-1** | Single-source via the Phase-1 lb-io helper + pool method. (Done in Phase 1; Phase 2 wires it.) |
| **R-CFBW-2** | Add a SEPARATE configurable `head_timeout`, default **60 s**. New knob on `HttpTimeoutsConfig` / `HttpTimeouts`. |
| **R-CFBW-3** (Branch-A consistency) | h2_proxy.rs:1548 (H2→H1 Branch A): rename `self.timeouts.body` → `self.timeouts.head` (consistency only, NOT a load-bearing idle site). h1_proxy.rs:1710 / h2_proxy.rs:2101 (the Branch-A buffered `h2_pool.send_request(...)` calls for H1→H2 / H2→H2) are NOT load-bearing either — keep `send_request` (no new params, pool's own `send_timeout` continues to bound them). |
| **Backstops** | h1_proxy.rs:1523, h2_proxy.rs:1881, h2_proxy.rs:3003 (verdict-rx backstops) stay AS-IS. They use `self.timeouts.body` / `body_timeout` and are post-error wedged-pump liveness consultations — NOT the bug. Leave them passing `self.timeouts.body` (still meaningful as "a small bound on a dead error path", unchanged shape). |
| **CF-S7-RHU (5th site)** | OUT of S14 scope per plan §7 carry-forward. |

---

## 1. Surface additions (lb-config / lb-l7 / lb-binary)

### 1.1 `crates/lb-config/src/lib.rs`

**Add** field + default + validation:

```rust
// in HttpTimeoutsConfig (around :803-:817):
    /// **S14 / CF-BODY-WALLCLOCK (R-CFBW-2)** — Phase-B fixed cap on the
    /// post-upload head wait (separate from the Phase-A idle deadline
    /// derived from `body_timeout_ms`). Defaults to 60 seconds.
    #[serde(default = "default_head_timeout_ms")]
    pub head_timeout_ms: u64,

// Default impl :819-:827:
    head_timeout_ms: default_head_timeout_ms(),

// const fn block, after default_total_timeout_ms (:837-:839):
const fn default_head_timeout_ms() -> u64 {
    60_000
}

// validate_http_timeouts :1301-:1320 — add:
        if http.head_timeout_ms == 0 {
            return Err(ConfigError::Validation(format!(
                "listener {i} http.head_timeout_ms must be > 0"
            )));
        }
```

**Update test fixtures** at :1865 (parse_config defaults) and :1986
(`validate_zero_http_timeout_rejected`) to include the new field — both
existing structs use `HttpTimeoutsConfig { ... }` literal construction
and will fail to compile without it. Pattern: `head_timeout_ms: 60_000`.

### 1.2 `crates/lb-l7/src/h1_proxy.rs`

**Extend** `HttpTimeouts` (:208-:228):

```rust
pub struct HttpTimeouts {
    pub header: Duration,
    pub body: Duration,
    pub total: Duration,
    /// **S14 / CF-BODY-WALLCLOCK (R-CFBW-2)** — Phase B fixed cap on the
    /// post-upload head wait, separate from the Phase-A `body` idle
    /// deadline. Defaults to 60 s.
    pub head: Duration,
}

impl Default for HttpTimeouts {
    fn default() -> Self {
        Self {
            header: Duration::from_secs(10),
            body:   Duration::from_secs(30),
            total:  Duration::from_secs(60),
            head:   Duration::from_secs(60),
        }
    }
}
```

### 1.3 `crates/lb/src/main.rs` — both `build_h1_proxy` (:823-:827) and `build_h2_proxy` (:890-:894)

```rust
let timeouts = http_cfg.map_or_else(HttpTimeouts::default, |h| HttpTimeouts {
    header: Duration::from_millis(h.header_timeout_ms),
    body:   Duration::from_millis(h.body_timeout_ms),
    total:  Duration::from_millis(h.total_timeout_ms),
    head:   Duration::from_millis(h.head_timeout_ms),   // ← NEW
});
```

(Workspace-wide grep of `HttpTimeouts { header:`, `HttpTimeouts {\n`
also turns up two test sites in lb-l7: `h2_proxy.rs:3431`,
`h1_proxy.rs:3340` — they call `HttpTimeouts::default()`, no field
literal, so they pick up the new default for free.)

---

## 2. Per-cell wiring — exact diffs

All four cells follow the SAME pump-bump shape. The shared pre-pump
allocation is:

```rust
// (placed immediately above the `tokio::spawn(async move { ... })` pump)
let last_progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
let upload_complete = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
let epoch = tokio::time::Instant::now();
let last_progress_pump = std::sync::Arc::clone(&last_progress);
let upload_complete_pump = std::sync::Arc::clone(&upload_complete);
let epoch_pump = epoch;
```

The pump body adds two helpers, scoped inside its `async move` block:

```rust
let bump = || {
    let dt = tokio::time::Instant::now()
        .saturating_duration_since(epoch_pump);
    let ms = u64::try_from(dt.as_millis()).unwrap_or(u64::MAX);
    last_progress_pump.store(ms, std::sync::atomic::Ordering::Relaxed);
};
let set_complete = || {
    upload_complete_pump.store(true, std::sync::atomic::Ordering::Release);
};
```

**(Captured via `move` — both Arcs are clones that the pump owns;
`epoch_pump` is a `Copy` of the outer `epoch`.)**

`bump` is invoked on EVERY `tx.send(Ok(frame))`-success path (the
SAME sites that today touch `in_flight_bytes.fetch_add` / `record_retained`
— guaranteed forward-progress). `set_complete` is invoked at EVERY
verdict-Ok terminal frame point (clean EOF, trailer-Ok), so a small fast
upload that finishes inside the idle window cannot be idle-aborted while
the backend takes a moment to return the head.

NOTE: `tx.send(Err(...))` (inject_abort) and verdict-Err sites do NOT
bump and do NOT set complete. The pump's death by panic/abort leaves
`upload_complete = false` and `last_progress` frozen → the helper times
out via Phase A (`IdleTimeout`) — the desired wedge-liveness property,
unchanged from the old fixed wall-clock semantics.

### 2.1 — Cell H1→H1 (Class A): `h1_proxy.rs::proxy_request`

| Line | Change |
|---|---|
| :1298 (just above `let pump = tokio::spawn`) | Insert the shared allocation block §2 (`last_progress` / `upload_complete` / `epoch` + clones). |
| Inside the pump (after :1298, before macros) | Insert `bump` and `set_complete` closures. |
| `send_chunked!` body (:1335) — replace `if tx.send(Ok(Frame::data(chunk))).await.is_err() { ... break; }` arms so that the SUCCESS path calls `bump();` after the `tx.send`. | |
| :1442 (trailer-Ok branch, after `let _ = tx.send(Ok(frame)).await;` and before `let _ = verdict_tx.send(Ok(()));`) | `bump(); set_complete();` |
| :1424 (clean-EOF branch, before `let _ = verdict_tx.send(Ok(()));`) | `set_complete();` (no `bump` — no frame was sent on this arm; setting complete suffices, the previous data-frame bump still anchors the watchdog) |
| **:1508** (the SEND site): replace<br>`let resp = match tokio::time::timeout(self.timeouts.body, send_fut).await {`<br>with the helper. | See diff snippet below. |
| :1510 / :1538 error arms | Map `IdleSendError::*` → `ProxyErr::Timeout` (collapse to existing variant; log discriminant via tracing). |
| :1523 backstop | UNCHANGED — `tokio::time::timeout(self.timeouts.body, verdict_rx)` stays. |

**Send-site replacement (h1_proxy.rs:1507-1542)**:

```rust
// BEFORE
let send_fut = sender.send_request(req);
let resp = match tokio::time::timeout(self.timeouts.body, send_fut).await {
    Ok(Ok(r)) => r,
    Ok(Err(e)) => { /* F-CAP-1 verdict-rx backstop … 502 fallthrough */ }
    Err(_) => { /* pump.abort(); conn_handle.abort(); 504 */ }
};

// AFTER
let send_fut = sender.send_request(req);
let resp = match lb_io::idle_send::idle_bounded_send(
    send_fut,
    std::sync::Arc::clone(&last_progress),
    std::sync::Arc::clone(&upload_complete),
    epoch,
    self.timeouts.body,
    self.timeouts.head,
)
.await
{
    Ok(Ok(r)) => r,
    Ok(Err(e)) => { /* F-CAP-1 verdict-rx backstop … 502 fallthrough — UNCHANGED */ }
    Err(idle_err) => {
        // Phase-1 collapse onto ProxyErr::Timeout; phase logged.
        tracing::warn!(
            error = %idle_err,
            "h1→h1 idle/head deadline fired",
        );
        pump.abort();
        conn_handle.abort();
        return Err(ProxyErr::Timeout);
    }
};
```

The `Ok(Err(e))` (F-CAP-1) inner arm is **byte-identical** to the
existing code (the inner hyper error type — `hyper::Error` — is
unchanged because `idle_bounded_send<F, T>` returns `Result<T,
IdleSendError>` where `T = Result<Response<…>, hyper::Error>`, the same
shape `tokio::time::timeout` returned for a `Result`-output future).
F-CAP-1 verdict-rx backstop preserved as-is per §0.

### 2.2 — Cell H2→H1 Branch B (Class A): `h2_proxy.rs::proxy_request`

Exactly the SAME shape as H1→H1, applied to the second pump in this file:

| Line | Change |
|---|---|
| :1630 (above `let pump = tokio::spawn`) | Shared allocation block. |
| Pump body (:1631…) | `bump` / `set_complete` closures. |
| `send_chunked!` (:1679) success path | `bump();` |
| Trailer-Ok branch (:1803) before verdict | `bump(); set_complete();` |
| Clean-`is_end_stream()` arm (:1781) before verdict | `set_complete();` |
| **:1866** SEND site | Replace `tokio::time::timeout(self.timeouts.body, send_fut)` → `idle_bounded_send(send_fut, ..., self.timeouts.body, self.timeouts.head)`. Match arms exactly mirror §2.1. |
| :1881 backstop | UNCHANGED (`tokio::time::timeout(self.timeouts.body, verdict_rx)`). |

### 2.3 — Cell H2→H1 Branch A (consistency rename only): `h2_proxy.rs:1548`

```rust
// BEFORE
let resp = match tokio::time::timeout(self.timeouts.body, send_fut).await {

// AFTER
let resp = match tokio::time::timeout(self.timeouts.head, send_fut).await {
```

Body unchanged. Plan §2 / R-CFBW-3: within-window buffered body cannot
be a slow-progressing upload; renaming to `head` is a consistency-only
move. NO idle watchdog, NO `last_progress`. (Verifier expects this arm
NOT to flip on revert; document in §5.)

### 2.4 — Cells H1→H2 and H2→H2 (Class B): shared driver `drive_h2_upstream_send`

Threading the four new params through the shared driver. The driver
takes them as `Arc`s + a `Duration` triple (`body_timeout` stays — used
ONLY by the F-CAP-1 verdict-rx backstop at :3003; do NOT remove).

**Signature change** (`h2_proxy.rs:2907-2914`):

```rust
// BEFORE
pub(crate) async fn drive_h2_upstream_send(
    pool: &Http2Pool,
    backend_addr: SocketAddr,
    upstream_req: Request<lb_io::http2_pool::H2ReqBody>,
    mut verdict_rx: tokio::sync::oneshot::Receiver<Result<(), ProxyErr>>,
    pump: tokio::task::JoinHandle<()>,
    body_timeout: Duration,
) -> Result<Response<IncomingBody>, ProxyErr>

// AFTER
#[allow(clippy::too_many_arguments)]
pub(crate) async fn drive_h2_upstream_send(
    pool: &Http2Pool,
    backend_addr: SocketAddr,
    upstream_req: Request<lb_io::http2_pool::H2ReqBody>,
    mut verdict_rx: tokio::sync::oneshot::Receiver<Result<(), ProxyErr>>,
    pump: tokio::task::JoinHandle<()>,
    last_progress: std::sync::Arc<std::sync::atomic::AtomicU64>,
    upload_complete: std::sync::Arc<std::sync::atomic::AtomicBool>,
    epoch: tokio::time::Instant,
    idle: Duration,
    head_timeout: Duration,
    body_timeout: Duration,
) -> Result<Response<IncomingBody>, ProxyErr>
```

**Inside the detached `tokio::spawn`** at :2944 — replace
`pool_for_task.send_request(backend_addr, upstream_req)` with
`pool_for_task.send_request_idle(backend_addr, upstream_req,
last_progress, upload_complete, epoch, idle, head_timeout)`. The
biased `tokio::select!` against `verdict_rx` (:2948) is unchanged —
`send_request_idle` returns the SAME `Result<Response<Incoming>,
Http2PoolError>` shape as `send_request`. The F-CAP-1 verdict-rx
backstop at :3003 (`tokio::time::timeout(body_timeout, &mut verdict_rx)`)
is unchanged — that's the post-error wedged-pump consultation, not the
send, and it must keep its fixed bound per §0.

**Call-site changes**:

- **H1→H2** (`h1_proxy.rs:1597-1910`, the `proxy_h1_to_h2_request` fn):
  - :1749 (above `let pump = tokio::spawn`) → shared allocation block.
  - Pump body (`send_chunked!` :1774 success, trailer-Ok :1857, clean-EOF
    :1847) → bump / set_complete sites mirroring §2.1.
  - :1901-1908 → replace the `drive_h2_upstream_send(...)` call with the
    new 10-arg signature:
    ```rust
    drive_h2_upstream_send(
        h2_pool, backend_addr, upstream_req, verdict_rx, pump,
        std::sync::Arc::clone(&last_progress),
        std::sync::Arc::clone(&upload_complete),
        epoch,
        self.timeouts.body,
        self.timeouts.head,
        self.timeouts.body,
    )
    .await
    ```
- **H2→H2** (`h2_proxy.rs:1998-2326`, the `proxy_h2_to_h2_request` fn):
  - :2142 (above `let pump = tokio::spawn`) → shared allocation block.
  - Pump body (`send_chunked!` :2167 success, trailer-Ok :2276,
    clean-`is_end_stream()` :2261) → bump / set_complete sites.
  - :2317-2324 → replace `drive_h2_upstream_send(...)` call with the
    same new 10-arg shape as H1→H2.

(`pool_for_task.send_request_idle` requires `clone` of the two Arcs at
the top of the detached task. Final detail: the driver's signature takes
the Arcs by value; clones for the detached `tokio::spawn` are inside the
driver — one place, not duplicated at each call site.)

### 2.5 — Branch-A buffered sends for H1→H2 (:1710) and H2→H2 (:2101)

These call `h2_pool.send_request(backend_addr, upstream_req).await` for a
within-window Branch-A buffered body. Per R-CFBW-3 (§0) the within-window
body is not a slow-progressing upload; we keep `send_request` (pool's
own `send_timeout` bounds it). NO change. Documented here so the verifier
diff doesn't expect a wiring; their per-cell smoke arms will still hit
this path on small-fast uploads.

---

## 3. R12 single-source equivalence table

The audit-ready claim: ALL FOUR streaming-pump cells call the SAME
helper-or-pool-method with the SAME 6-arg call shape (5 of which are
single-sourced from a `self.timeouts.{body,head}` + the per-request
`last_progress` / `upload_complete` / `epoch`).

| Cell | Helper invoked | Call shape |
|---|---|---|
| H1→H1 | `lb_io::idle_send::idle_bounded_send` | `(send_fut, Arc::clone(&last_progress), Arc::clone(&upload_complete), epoch, self.timeouts.body, self.timeouts.head)` |
| H2→H1 Branch B | `lb_io::idle_send::idle_bounded_send` | (IDEM) |
| H1→H2 | `Http2Pool::send_request_idle` (Phase 1) | `(addr, req, Arc::clone(&last_progress), Arc::clone(&upload_complete), epoch, self.timeouts.body, self.timeouts.head)` — `Arc::clone`s applied inside the driver before the detached spawn |
| H2→H2 | `Http2Pool::send_request_idle` (Phase 1) | (IDEM) |

The two Class A cells share the lb-l7 helper invocation byte-for-byte.
The two Class B cells funnel through the SAME driver
`drive_h2_upstream_send`, so they share the pool-method invocation
byte-for-byte. ONE watchdog implementation (Phase 1) drives all four —
the R14 audit reduces to "all four cells pass the same Arc-handle pair
+ the same `{body, head}` Durations".

---

## 4. R8 / R3 preservation argument

- **R8 (bounded memory)**: `last_progress` is a single `AtomicU64` and
  `upload_complete` is a single `AtomicBool` per request — no body bytes
  held. The bump is co-located with the EXISTING `in_flight_bytes`
  fetch_add / `record_retained` sites; same instruction window, same
  forward-progress event. Plan §4.
- **R3 (existing test stays green)**:
  - F-CAP-1 (413/400/502 verdict-vs-send race): the **inner**
    `Ok(Err(hyper::Error))` arm of every send-site is preserved verbatim
    (the helper passes the hyper error through unchanged when the future
    resolves). The verdict-rx backstops are not touched.
  - F-MD-4 (smuggle): `inject_abort!` and the verdict-Err arms do NOT
    bump and do NOT set complete; the smuggle abort path is purely
    additive-instrumentation-clean.
  - Trailer passthrough: trailer-Ok branch is the ONLY place we call
    BOTH `bump()` and `set_complete()` adjacent (after the
    `tx.send(Ok(frame))` succeeds for the trailers frame). Order: bump
    (the trailer chunk was just accepted) THEN set_complete (terminal).
  - `_md_streaming_verify` / `_resp_streaming_verify`: timing-tight gauge
    arms — the new atomics are on the same hot path but do not change
    the gauge value (separate counter).
- **F-CAP-1 invariant**: the over-cap arm relies on the pump's
  `inject_abort!`-then-verdict shape. Helper changes nothing here: when
  the pump aborts mid-upload, `upload_complete` stays `false` AND
  `last_progress` stops advancing, so the helper's Phase-A
  `IdleTimeout` may fire — but the hyper send future ALREADY errors
  (because `inject_abort!` injected a body Err) so the inner
  `Ok(Err(e))` arm wins, consults verdict-rx, and classifies BodyTooLarge
  /BadRequest exactly as today. Verified by inspection: the new helper
  cannot race ahead of the existing inner-error arm (the future errors
  the moment hyper's body sees `Err`, well before any
  `self.timeouts.body` idle expiration).

---

## 5. Per-cell commits + push cadence

One commit per cell + 1 commit each for the lb-config + lb-l7 surface +
lb-binary wiring + (optional) docs. Suggested order so each compiles
in isolation:

1. `feat(s14): lb-config + lb-l7 + lb head_timeout knob (R-CFBW-2)` —
   surface change only; defaults pick up correctly; no behavior change
   (no cells yet wired; lb-l7's `HttpTimeouts::head` is a new dead
   field). Workspace builds, tests green.
2. `feat(s14): wire H2→H1 Branch A to self.timeouts.head (rename only)` —
   single-line change; isolated.
3. `feat(s14 H1→H1): idle_bounded_send wiring + pump progress bumps` —
   Class A cell #1.
4. `feat(s14 H2→H1 Branch B): idle_bounded_send wiring + pump bumps` —
   Class A cell #2.
5. `feat(s14): drive_h2_upstream_send threads last_progress/head — Class B driver` —
   driver-signature change, broken call sites — temporarily replace the
   two callers in the SAME commit (mechanical: the new params slot in,
   the new pump-bump sites are added — call this commit a single atomic
   driver+callers change so the workspace never breaks).

Push after each commit; verifier diffs per cell.

`cargo check --workspace --all-features` runs after every commit; the
full gate (`cargo test -p lb-l7 --all-features`, `cargo clippy
--workspace -- -D warnings`, `cargo fmt --all --check`) runs once at the
tip.

---

## 6. Out of scope (per S14 plan §0 + plan §7)

- R13(a)(b)(c) per-cell tests (verifier Phase 3).
- Coverage (verifier).
- CF-S7-RHU 5th site on `request_h3_upstream` H3-leg wall-clock.
- Reworking the verdict-rx backstops (`h1_proxy.rs:1523`,
  `h2_proxy.rs:1881`, `:3003`). They use `self.timeouts.body` /
  `body_timeout` — that's the post-error consultation, not the bug.
- Renaming `body_timeout` in `drive_h2_upstream_send` — still semantically
  "bound on the post-error verdict-rx wait", unchanged role.

---

## 7. Open questions for the lead before plan-approval

1. **Branch-A buffered `h2_pool.send_request` (h1_proxy.rs:1710 / h2_proxy.rs:2101)** —
   the lead's brief did NOT ask to migrate Branch A to `send_request_idle`.
   Plan §2 / R-CFBW-3 says Branch A is NOT load-bearing for the idle fix,
   so I keep `send_request`. Confirm? If you'd rather Branch A
   *also* migrate (for R12 single-source byte-equivalence on EVERY pool
   call), I'll change to `send_request_idle` with `last_progress` /
   `upload_complete` constructed and `upload_complete.store(true)`
   immediately after the build (since the body is already in-memory).
   Current plan: NO migration, document under §2.5.
2. **F-CAP-1 with the idle watchdog** — confirm the §4 invariant
   argument is sufficient: I do NOT add a defensive "if
   `upload_complete == false` and the inner future errored, don't trust
   `IdleTimeout`" early-return, because the existing inner-error arm
   wins first anyway. If you'd like an explicit
   `inject_abort_in_progress` flag to suppress an idle-fire during the
   abort handshake window, that's a Phase-2.5 commit on top.
3. **Backstop `body_timeout` rename** — plan §3 suggests optionally
   introducing a `VERDICT_BACKSTOP` const (5 s). I've NOT included it
   (no behavior change, minor cleanup) — confirm scope-out for S14 or
   ask me to add a 4th commit.

I'll wait for explicit plan-approval (and answers to 1-3) before
touching any `.rs` file.
