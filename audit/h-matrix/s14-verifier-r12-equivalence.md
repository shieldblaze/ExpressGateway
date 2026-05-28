# S14 R12 four-cell equivalence proof — CF-BODY-WALLCLOCK

> **Author:** verifier (`feature/cfbw-s14-verify`, rebased on `e83cf47e`).
> **Scope:** prove that the four CF-BODY-WALLCLOCK-fixed cells (H1→H1,
> H2→H1 Branch B, H1→H2, H2→H2) all reach the SAME single-source
> two-phase idle/head deadline mechanism, with byte-identical call shape
> modulo cell-shape-driven divergence.
> **Memory-hooks:** [[s12-introduced-r12-resolve-before-close]] applies —
> any single-cell divergence introduced by Phase 2 is a session-internal
> R12 finding to resolve in-session, not defer.

---

## 1. Single-source point

The mechanism lives in lb-io:

| Symbol | File | Purpose |
|---|---|---|
| `idle_bounded_send<F, T>` | `crates/lb-io/src/idle_send.rs:93` | Two-phase idle/head loop, generic over the wrapped send future. Used directly by H1-client classes (Class A). |
| `Http2Pool::send_request_idle` | `crates/lb-io/src/http2_pool.rs:296` | Pool wrapper that calls `idle_bounded_send` around the hyper H2 `sender.send_request(req)`. Used by H2-pool classes (Class B). |

Both ultimately reach the **same loop body** in `idle_bounded_send`.
`send_request_idle` is a thin wrapper that adds eviction parity with
the existing `send_request` (ROUND8-L7-10) and collapses the two
`IdleSendError` variants onto `Http2PoolError::Timeout` for Phase 1
enum-stable callers.

---

## 2. Cell call-site inventory (Phase 2)

### 2.1 Class A — direct `idle_bounded_send`

| Cell | File:line | Wrapped future | Arg shape |
|---|---|---|---|
| H1→H1 | `crates/lb-l7/src/h1_proxy.rs:1572` | `sender.send_request(req)` (hyper H1 client) | `(send_fut, Arc::clone(&last_progress), Arc::clone(&upload_complete), epoch, self.timeouts.body, self.timeouts.head)` |
| H2→H1 Branch B | `crates/lb-l7/src/h2_proxy.rs:1909` | `sender.send_request(req)` (hyper H1 client) | `(send_fut, Arc::clone(&last_progress), Arc::clone(&upload_complete), epoch, self.timeouts.body, self.timeouts.head)` |

**Diff:** ZERO arg-shape divergence. The `last_progress` / `upload_complete`
atomics are constructed identically; the `epoch` is each cell's request-start
`tokio::time::Instant`; idle (Phase A) = `self.timeouts.body`; head (Phase B)
= `self.timeouts.head`. The post-error F-CAP-1 verdict-rx backstop arm,
the `pump.abort()` + `conn_handle.abort()` on `Err(idle_err)`, and the
`ProxyErr::Timeout` collapse are also bit-identical (a `diff` of the two
match-blocks differs only on the `tracing::warn!` literal `"h1→h1"` vs
`"h2→h1"`).

### 2.2 Class B — `drive_h2_upstream_send` → `send_request_idle`

| Cell | File:line | Forwarded into shared driver |
|---|---|---|
| H1→H2 | `crates/lb-l7/src/h1_proxy.rs:2008` | `drive_h2_upstream_send(h2_pool, addr, req, verdict_rx, pump, last_progress, upload_complete, epoch, self.timeouts.body, self.timeouts.head, self.timeouts.body)` |
| H2→H2 | `crates/lb-l7/src/h2_proxy.rs:2401` | `drive_h2_upstream_send(h2_pool, addr, req, verdict_rx, pump, last_progress, upload_complete, epoch, self.timeouts.body, self.timeouts.head, self.timeouts.body)` |

**Diff:** ZERO arg-shape divergence; both forward `(idle = body,
head_timeout = head, body_timeout = body)` to the shared driver. The
shared driver internally calls (`crates/lb-l7/src/h2_proxy.rs:3054`):

```rust
pool_for_task.send_request_idle(
    backend_addr,
    upstream_req,
    last_progress,
    upload_complete,
    epoch,
    idle,
    head_timeout,
)
```

— byte-identical for both Class B callers. The pre-existing
`body_timeout` parameter is retained ONLY for the F-CAP-1 verdict-rx
backstop at `:3003` (wedged-pump liveness consultation), NOT consumed by
the send.

---

## 3. Class A vs Class B — equivalence under the mechanism

Both classes drive the **same `idle_bounded_send` loop body** (Class A
directly; Class B via `Http2Pool::send_request_idle` which is itself a
thin wrapper as documented in §1). The single-source guarantee is
therefore: every CF-BODY-WALLCLOCK-fixed cell evaluates exactly one
implementation of the two-phase deadline.

### 3.1 Expected structural divergence (Branch A — Class B only)

H1→H2 and H2→H2 are streaming-vs-buffered hybrid cells. Branch A (request
body fits entirely in the request lookahead window) uses the
pre-Phase-2 buffered send (no pump → no `last_progress` → no idle
deadline applicable) and is therefore unchanged by S14. The CF-BODY-WALLCLOCK
fix only applies to **Branch B** (request > window → streaming pump) which
IS wired through the shared `drive_h2_upstream_send` driver in §2.2.

This is per builder-1's design `audit/h-matrix/s14-builder-1-design.md`
§2.4 and matches the lead's Phase-2 brief. **No defect** — small
within-window uploads do not need an idle watchdog (they complete in one
hop with no pump).

### 3.2 Verdict-vs-send race (F-CAP-1) preservation

In Class A, the `Ok(Err(hyper::Error))` arm consults `verdict_rx` BEFORE
`pump.abort()` and BOUNDED by `self.timeouts.body` (h1_proxy.rs:1596 /
h2_proxy.rs:1933), preserving the F-CAP-1 verdict-vs-send race semantics
verbatim from S9/S10.

In Class B, the detached send task races `verdict_rx` against the send
future with `biased; verdict_rx first` (h2_proxy.rs:3066+), then
`reset_peer(addr)` on abort verdict, then drops the body — preserving the
F-MD-4 detached-pump invariant from S10 unchanged.

Both classes route any `IdleSendError` through their respective
"timeout" classification path (`ProxyErr::Timeout` for Class A;
`Http2PoolError::Timeout` → reset_peer + classified ProxyErr for Class B
via the standard pool-error mapping), so the new timeout class lands on
the same downstream 504 path as the pre-Phase-2 wall-clock timer
(zero-behavior-delta on the error path for the wedge case; the new
behavior is the SUCCESS path that previously 504'd on a slow upload).

---

## 4. Equivalence verdict — **APPROVE**

- **Class A H1→H1 ≡ H2→H1 Branch B**: identical 6-arg shape into
  `idle_bounded_send`; identical post-error verdict-rx backstop; identical
  `pump.abort()` + `conn_handle.abort()` on Phase A/B timeout.
- **Class B H1→H2 ≡ H2→H2**: identical 11-arg shape into the SHARED
  `drive_h2_upstream_send` driver; the driver's internal
  `send_request_idle` call is byte-identical for both callers (it has only
  one call site).
- **Class A loop body ≡ Class B loop body**: both call the SAME
  `idle_bounded_send<F, T>` in lb-io.

No single-cell divergence introduced by Phase 2 wiring. No R12 finding to
escalate.

A future R12 audit only needs to grep for `idle_bounded_send(` and
`send_request_idle(` across the workspace; the four matches above are the
complete set.
