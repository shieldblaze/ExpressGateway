# Round 6 — code delta-discovery sweep on `prod-readiness/round-4`

Scope: diff between `main` and `prod-readiness/round-4` (`d1e1247`) —
looking for **new** Rust-quality regressions introduced by the Round-4
/ 5 / 6 fix landings. Prior-round findings are explicitly out of
scope; only deltas land here.

Sanity check: `cargo check --workspace --all-targets` PASS at
`d1e1247` (57s, no warnings). Cross-referenced against
`audit/ebpf/round-5-verifies-code.md` so Verified-Fixed-Partial items
are not re-litigated.

## Methodology

* Enumerated every `tokio::spawn` / `tracker().spawn` site on `HEAD`
  and diffed against `main`. For each new spawn confirmed (a) it is
  routed through `Shutdown::tracker()`, OR (b) the caller holds the
  `JoinHandle` and `await`s it inside the bounded drain.
* Re-read every `Ordering::*` site touched by Round-4/5/6 and re-paired
  the publish/consume against the CODE-2-04 invariant table.
* Walked the `Drop` order of `async_main` against the lifetimes of
  `Arc<HooksBundle>`, `Watchdog`, `ConnGate`, `ProbeRegistry`,
  `Arc<ArcSwap<TlsConfigBundle>>` to confirm `Shutdown::drain`
  completes before any of those Arcs hit refcount 0.
* Reviewed `ArcSwap<TlsConfigBundle>` aliasing — multiple readers
  (`bundle.load_full()` per accept) vs. single writer
  (`bundle.store(Arc::new(new))` on SIGUSR1).
* `panic = "abort"` — confirmed `[profile.release]` and `[profile.bench]`
  carry `panic = "abort"`; `[profile.dev]` and `[profile.test]`
  inherit rustc's default `unwind` so proptest minimisation + loom
  `catch_unwind` continue to work.
* `Backend.state: Option<Arc<BackendState>>` — searched every consumer
  of the field for correct `None` handling.
* Grepped new `unwrap` / `expect` / `panic!` sites in production code
  paths (excluding `#[cfg(test)]`, doctests, and `mod tests`).
* New runtime deps (`object`, `notify`, `rcgen`, `opentelemetry`,
  `arc-swap`, `rustls-pemfile`, `tracing-subscriber`,
  `proptest`, `loom`): verified each crate's published `rust-version`
  field is ≤ 1.85 (MSRV).

## Result

**Zero new medium-or-higher findings.** The Round-4 fix surface holds
under the eight inspection vectors in the Round-6 brief.

Specifically:

* **New spawn sites (CODE-2-03 follow-on)** — every new long-lived
  spawn introduced this round routes through
  `shutdown.tracker().spawn(...)` and observes the shared
  `CancellationToken` in a `biased select!`. Verified at:
  - `spawn_rotator_ticker` (`crates/lb/src/main.rs:554`) — TLS ticket
    rotator now tracker-attached + cancel-observing (was unparented on
    `main`).
  - `install_hotpath_metrics` pool/DNS sampler (`main.rs:1380`).
  - `spawn_tcp` listener accept-loop (`main.rs:1086`).
  - XDP STATS sampler (`main.rs:1838`).
  - Per-listener Watchdog sweeper (`main.rs:1747`).
  - Per-connection task at the accept-site (`main.rs:2301`,
    `st.tracker.clone().spawn(...)`).
  The QUIC listener's internal `tokio::spawn` (`crates/lb-quic/src/listener.rs:249`)
  is **intentionally** outside the workspace tracker — main.rs holds
  the `JoinHandle` and `await`s it with a bounded `quic_drain_deadline`
  before calling `shutdown.drain(...)` (`main.rs:1931-1942`).
  The H2 WebSocket-upgrade `tokio::spawn` (`h2_proxy.rs:588`,
  CODE-2-09 follow-on) mirrors the existing H1 WS upgrade spawn shape
  that already shipped on `main`; per-request WS tasks were already
  not tracker-attached, so this is a structurally consistent addition
  (see INFO-DELTA-1 below for the recommendation).

* **Atomic ordering** — new sites are paired correctly:
  - `ConnGate::admit` (`conn_gate.rs:179-209`) uses Acquire-load /
    AcqRel `compare_exchange_weak` with Acquire on failure for the
    per-listener counter; rollback on per-IP overflow is `fetch_sub(AcqRel)`.
    The per-IP table is a `DashMap` with its own internal Acquire/Release.
  - `ProbeRegistry` (`probe_registry.rs`) uses Acquire load,
    AcqRel `compare_exchange` for state transitions, Release store on
    the terminal `set_draining`. State byte values are never re-used.
  - `ListenerState::active_connections` `fetch_add(1, AcqRel)` /
    `fetch_sub(1, AcqRel)` — pair correctly with the metrics sampler's
    `fetch` (`Acquire`-equivalent via `i64::from`).
  - `PANIC_TOTAL_FALLBACK` (`main.rs:10809`/`10833`/`10851`) — Release
    on increment, AcqRel on flush, Acquire on the registry-bound
    counter sync. No torn read possible.
  - The `Relaxed` annotation at `ticket.rs:900` (CODE-2-04 follow-on,
    commit `9a8d2ca`) is a stats counter on a single-writer path —
    correct.

* **Drop order with `Shutdown`** — `Shutdown::drain` (`shutdown.rs:100`)
  consumes `self` by value, refuses new spawns (`tracker.close()`),
  signals cancel (`token.cancel()`), then `tracker.wait()` waits for
  every tracked task to exit before returning. By the time
  `async_main` returns, every spawned closure has been dropped, which
  in turn drops the cloned `Arc<HooksBundle>` / `Watchdog` /
  `Arc<ConnGate>` / `Arc<ProbeRegistry>` references. The stack-frame
  owners of these Arcs then drop after `shutdown.drain(...)`
  completes (lines 1950-1985 of `main.rs`). The only Arc that
  outlives drain is `_xdp_loader`, which is explicitly held to the
  very end of `async_main` (`drop(_xdp_loader);` at line 1983) so the
  userspace inserter sees a stable map until all packet-handling tasks
  have exited. No premature Arc-refcount-zero hazard.

* **`ArcSwap<TlsConfigBundle>` correctness** — per-accept readers call
  `bundle.load_full()` (owned `Arc<TlsConfigBundle>` snapshot,
  `ticket.rs:507`) at `main.rs:2327` and `2395`; the SIGUSR1 reload
  path's single writer calls `bundle.store(Arc::new(new))` after the
  new bundle has been fully constructed (`reload_tls_bundle` at
  `ticket.rs:644-655` builds the rustls `ServerConfig` smoke-test
  before storing). `ArcSwap::store` is an atomic pointer publish; a
  snapshot held by an in-flight handshake survives the swap because
  `load_full` clones the inner `Arc` (refcount bump). No torn read,
  no use-after-free, no Drop-reordering race even under a partial
  cert-key write that returns `Err` mid-reload (old bundle stays
  live).

* **`panic = "abort"`** — `[profile.release]` carries `panic = "abort"`
  with the explicit comment that dev/test deliberately keep `unwind`
  for proptest + loom. `[profile.bench]` inherits release. Searched
  every production-code `catch_unwind` callsite: zero hits.
  `catch_unwind` appears only in proptest harnesses
  (`crates/lb-{h1,h2,h3,quic}/tests/proptest_*.rs`) which compile
  under the test profile where `unwind` is live. `CidEntryGuard`
  (`crates/lb-quic/src/cleanup_guard.rs`) is annotated dead-under-abort
  per CODE-2-08, and no new production-code path was added this round
  that relies on its Drop for cleanup invariants — the router actor's
  use of `CidEntryGuard` at `router.rs:382` is the same call site as
  on `main`. Acceptable.

* **`Backend.state: Option<Arc<BackendState>>`** — single consumer of
  the field is `Backend::sync_from_state` (`lb-balancer/src/lib.rs:127`)
  which short-circuits on `None`. Production constructs `Backend` via
  `Backend::new(...)` (`main.rs:1046`) which sets `state: None`; the
  Wave-2 picker wire-in that actually populates `state: Some(...)`
  through `Backend::with_state` is the documented partial deferral
  per `audit/ebpf/round-5-verifies-code.md#L500` (status:
  Verified-Fixed-Partial). No NEW divergence introduced — the `None`
  branch is the production-default path today, and it is correctly
  handled by every consumer.

* **No new production `unwrap` / `expect` / `panic!`** — the only
  matches in the diff outside `mod tests` / `#[cfg(test)]` blocks
  are inside doctests (`stripped_request.rs:108`, `:120`) and a doc
  comment (`main.rs:1414`). The CODE-2-02 `init_panic_hook` is in
  place from the very top of `async_main` so any unexpected panic
  lands a log line + `process::abort` rather than a silent kill.

* **New runtime crates (versions in Cargo.lock)**:
  `arc-swap 1.9.1` (rust_version unspecified — uses 2018 edition;
  fine on 1.85), `dashmap 5.5.3` + `6.1.0` (`rust-version = "1.65"`),
  `loom 0.7.2` (`1.65`, dev-only), `object 0.36.7` + `0.37.3`
  (`1.65`, dev-only via `aya-obj`), `proptest 1.11.0`
  (`rust-version = "1.85"` — exact match for MSRV, dev-only),
  `rcgen 0.13.2` (unspecified — builds on 1.85), `rustls-pemfile 2.2.0`
  (unspecified — builds on 1.85), `tokio-rustls 0.26.4` (`1.71`),
  `tracing-subscriber 0.3.23` (`1.65`). `notify` and `opentelemetry`
  are **not** in the Cargo.lock as runtime deps — both were
  explicitly deferred per `audit/deps-added.md` and re-verified
  absent by `grep -rn 'use notify\|use opentelemetry' crates/` = 0
  hits. MSRV 1.85 pin in `rust-toolchain.toml` holds.

## Informational observations (sub-low; recorded for completeness, not regressed)

These do not loop back to Round 3 but are listed so a future hardening
pass can pick them up.

### INFO-DELTA-1 — per-request WS upgrade spawns are not tracker-attached
File: `crates/lb-l7/src/h2_proxy.rs:588` (and pre-existing twin at
`h1_proxy.rs:819` / `ws_proxy.rs:618`).
Severity: info
Introduced-by: `fc42d60` (CODE-2-09 follow-on); the H1 twin pre-dates
the round.
The H2 extended-CONNECT WebSocket upgrade path spawns a detached task
on the runtime tokio handle, not the workspace `TaskTracker`. Per the
H2 proxy's own design, the upgrade task continues independently of the
H2 connection future once `hyper::upgrade::on()` succeeds. On SIGTERM
the parent H2 connection task IS tracker-attached (`main.rs:2301`)
and the cancel token reaches it; the WS upgrade task, however, runs to
completion or to its own backend-side error without the drain budget.
Real-world impact is bounded by the WS-config idle / read deadlines
(`ws_proxy.rs::config()`) — a WS session that hits the idle deadline
during drain exits cleanly. Recommendation: thread `Shutdown::tracker`
through `bridge_request_to_h2_connect` so the spawn can be routed
through `tracker.spawn` and `tracker.wait()` waits for WS drain too.

### INFO-DELTA-2 — admin HTTP accept loop uses bare `tokio::spawn`
File: `crates/lb-observability/src/admin_http.rs:244` + `262` + `348`.
Severity: info
Introduced-by: pre-existing on `main` (`e6c119b`); re-verified
unchanged across `9484544` (SEC-2-06) and `7108d9e` (REL-2-04).
The admin-listener accept loop and per-connection serve task use bare
`tokio::spawn`, not the workspace tracker. The accept loop observes a
`CancellationToken` so it exits promptly on SIGTERM; the
per-connection serve tasks (`/metrics`, `/livez`, etc.) are not
joined by drain. Real-world impact: a scraper that is mid-stream when
SIGTERM fires will get its socket dropped on runtime shutdown rather
than a clean response. Acceptable for an admin port. Not regressed
this round; recorded so a future hardening pass can pick it up
alongside INFO-DELTA-1.

### INFO-DELTA-3 — `Watchdog::sweep_expired` runs unconditionally even with no registrants
File: `crates/lb/src/main.rs:1747-1768`.
Severity: info
Introduced-by: `49a6f94` (SEC-2-03 follow-on).
The sweeper ticks at `sweep_interval_ms` (default 1000 ms) regardless
of whether any connection is registered. Under low load this is a
no-op DashMap iteration; under steady-state this is also fine
(O(registered)). No correctness issue. Recommendation: gate the tick
on `watchdog.len() > 0` if the always-on tick shows up in flame
graphs.

### INFO-DELTA-4 — `_xdp_loader` Arc holds eBPF FDs past `Shutdown::drain`
File: `crates/lb/src/main.rs:1983` (`drop(_xdp_loader);`).
Severity: info
Introduced-by: pre-existing (`fc050b0`); re-verified for ordering
correctness this round.
The XDP loader's Arc is held until after `shutdown.drain(...)`
returns. This is intentional (and correctly commented) so the
userspace map inserter sees stable FDs until the last per-connection
task has exited. The interaction with `panic = "abort"` is benign:
on abort the kernel reaps the eBPF FDs anyway, so a panicked
process does not leak XDP attach state. Recorded for completeness
because the ordering is a delta from the original `main`-branch
non-XDP path.

---

## Summary

| Vector                                              | Verdict        |
|-----------------------------------------------------|----------------|
| New `tokio::spawn` / `tracker().spawn` sites        | Clean          |
| New atomic-ordering sites                           | Clean          |
| `Drop` order with `lb_core::Shutdown`               | Clean          |
| `ArcSwap<TlsConfigBundle>` reader/writer aliasing   | Clean          |
| `panic = "abort"` vs. `catch_unwind` in production  | Clean          |
| `Backend.state: Option<Arc<BackendState>>` consumers| Clean          |
| New `unwrap` / `expect` / `panic!` in hot path      | Clean          |
| New-dep MSRV (1.85) compatibility                   | Clean          |

Total: zero new medium-or-higher findings; four sub-low informational
observations recorded for future hardening.

Verifier-SHA: `fc4f8e4`.
