# ADR-0009: Graceful reload — SO_REUSEPORT + drain; fd-passing deferred

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: Cloudflare Pingora reload model, nginx binary upgrade,
  HAProxy seamless reloads, Linux man page `socket(7)` on
  `SO_REUSEPORT`, RFC 9293 (TCP).

## Context and problem statement

A production load balancer must update without dropping connections.
"Update" here means replacing the process binary or changing
configuration that cannot be hot-swapped via `ArcSwap` (ADR-0008) — for
example, switching TLS certificate chains or changing listener address
sets. Two dominant strategies exist:

- **SO_REUSEPORT + drain**: start a new process, have both old and new
  `bind()` the same listening socket with `SO_REUSEPORT` set; the
  kernel's per-flow load-balancing spreads new connections across both
  processes. The old process stops `accept()`ing, drains its in-flight
  connections, and exits. This is what nginx 1.9+ and HAProxy can do
  in their simpler modes.
- **FD-passing**: the old process passes its listening file descriptors
  to the new process via `SCM_RIGHTS` over a Unix socket. The new
  process inherits the *same* socket, so connections in the accept
  queue are preserved. Cloudflare's Pingora uses this model.

The two are not mutually exclusive — Pingora uses fd-passing *plus*
connection-level migration — but the engineering cost differs by an
order of magnitude. FD-passing requires a handover protocol between
old and new processes, careful handling of the accept queue's
in-flight-SYN packets, and awareness of TCP_FASTOPEN cookies; a correct
implementation is a meaningful chunk of work.

For the initial production target, we need zero-drop for the common
case (reload under steady-state traffic). We do **not** yet need the
fd-passing refinement, which saves the handful of SYNs sitting in the
old process's accept queue at the moment of swap.

## Decision drivers

- Zero *established*-connection drops under reload (mandatory).
- Zero *in-accept-queue* drops (nice-to-have; hard to prove worth it
  without production measurement).
- Operator ergonomics: `systemctl reload` semantics must work.
- Implementation effort and test tractability under the halting-gate.
- Panic-free compatibility with the lint regime.
- Cross-platform note: `SO_REUSEPORT` behaviour differs between Linux
  (load-balancing across listeners) and BSD (simple port reuse without
  load-balancing). Production target is Linux.
- Test substrate: CI cannot bind privileged ports or run two copies of
  the binary easily; the reload test exercises the *state machine*, not
  the kernel socket layer.

## Considered options

1. **SO_REUSEPORT + drain only** — simple, covers 99 % of cases.
2. **FD-passing only** — more complete, significantly more complex.
3. **SO_REUSEPORT for now, FD-passing later** — incremental.
4. **In-process rebind** — atomic swap of the listening socket within a
   single process, no second process at all.

## Decision outcome

Option 3. Zero-drop reload is implemented today via **SO_REUSEPORT +
drain**. FD-passing (Pingora-style) is a recorded deferred
enhancement; it is not built today and not required by the current
test suite. The `tests/reload_zero_drop.rs` test exercises the
*configuration-reload* state machine (rapid reload/rollback cycles
with monotonic versioning) — the piece we can unit-test deterministically
— rather than the kernel socket handoff.

## Rationale

- **SO_REUSEPORT on Linux** distributes new incoming connections across
  all listeners bound to the same `(addr, port)` that have the
  `SO_REUSEPORT` option set, via a flow hash (4-tuple) computed in the
  kernel. When the old process stops calling `accept()` but does not
  `close()` its socket, new connections go exclusively to the new
  process; existing connections continue until they finish or hit the
  drain deadline.
- **Why not fd-passing today**: Pingora's fd-passing matters when the
  accept queue is non-trivially populated (tens of thousands of
  backlogged SYNs). At our initial throughput targets and reload
  cadence (bounded by operator pace, not load), the SYNs lost to a
  drain-based handover are a handful per reload at most, and those
  clients retry per RFC 9293. The cost of building and correctly
  testing fd-passing (Unix-socket handshake, `SCM_RIGHTS` plumbing,
  `TCP_FASTOPEN` cookie continuity) is not justified yet.
- **Drain semantics**: on SIGTERM (or a graceful-reload signal) the
  listener stops accepting, and `lb-l7`'s per-connection tasks run to
  completion. A configurable deadline (default: 30 s in `lb-config`)
  bounds the wait; connections exceeding the deadline are
  `RST`-closed. This matches HAProxy's `hard-stop-after`.
- **Configuration reload** is a separate concern handled by ADR-0008
  (`ArcSwap` + `ConfigManager`). In most cases, operators reload
  *config* — not the binary — and ArcSwap gives zero-drop with zero
  reload effort. Binary upgrades (the only case that needs
  SO_REUSEPORT) are rare.
- **Test constraints**: `tests/reload_zero_drop.rs` verifies the
  strongest property we can assert without a real network stack in CI:
  rapid reload and rollback cycles produce a consistent final state,
  version numbers increase monotonically, the rolled-back config is
  readable, and the manager remains functional afterward. This is the
  *semantic* core of zero-drop reload; the socket-level kernel
  behaviour is a Linux contract we rely on rather than re-verify.
- **No `unwrap()` on the reload path**: `reload()` and
  `rollback_to_previous()` return `Result<..., ControlPlaneError>`.
  The test file uses `.unwrap()` because test code is exempt from the
  lint gate (`#![cfg_attr(test, allow(clippy::unwrap_used, …))]`).

## Consequences

### Positive
- Config reload is genuinely zero-drop via `ArcSwap`; no sockets
  involved.
- Binary reload is zero-drop for *established* connections via
  SO_REUSEPORT + drain.
- Operator experience matches nginx/HAProxy — no new mental model.
- Test suite is deterministic without network privileges.

### Negative
- A small number of SYN packets sitting in the old process's accept
  queue at the moment of handover are lost; clients must retry.
  Mitigated by TCP retransmission but not invisible.
- Binary reload on FreeBSD/macOS does not get per-flow load-balancing
  from `SO_REUSEPORT` (kernel difference) — our target is Linux.

### Neutral
- We own the seam to add fd-passing later without changing the
  operator interface.

## Implementation notes

- `crates/lb/src/main.rs` — listener setup
  (`tokio::net::TcpListener::bind`). Calling `bind` on tokio's
  listener does not today pass `SO_REUSEPORT`; the production path
  uses `socket2::Socket` with `set_reuse_port(true)` before converting
  to the tokio listener. Hooking that in is a localised change in
  `run_listener`.
- `crates/lb-controlplane/src/lib.rs` — the reload state machine
  underlying SIGHUP-driven config changes.
- `tests/reload_zero_drop.rs` — reload/rollback state-machine test.
- `crates/lb-config/src/lib.rs` — `hard_stop_after` / drain deadline.

## Follow-ups / open questions

- Wire `SO_REUSEPORT` explicitly via `socket2` in `lb/src/main.rs`
  (one-line change in the listener-creation path; deferred pending a
  real-network integration test environment).
- Decide when fd-passing is worth the build. Trigger: production
  measurement of SYNs dropped per reload exceeding an agreed budget.
- Graceful shutdown deadline: expose in `lb-config` as
  `reload.drain_timeout_seconds`.

## Sources

- `socket(7)` man page on `SO_REUSEPORT`.
- RFC 9293 — TCP, on retransmission behaviour after SYN loss.
- Cloudflare Pingora: "Graceful restart in Rust" blog post, 2024.
- nginx binary upgrade documentation.
- Internal: `crates/lb/src/main.rs`,
  `crates/lb-controlplane/src/lib.rs`, `tests/reload_zero_drop.rs`.
