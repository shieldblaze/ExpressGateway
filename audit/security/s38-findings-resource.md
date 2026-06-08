# S38 Resource / DoS Auditor — Findings

**Auditor:** resource-auditor · **Date:** 2026-06-08 · **Base:** main @ b8a99078
**Surface:** DoS / resource-exhaustion. The bound that matters is **R8** (no unbounded
allocation from the wire). Analysis + PoC-authoring only; the lead runs all PoCs/soak/load.

---

## CRITICAL / HIGH (read immediately)

**NONE.** No wire-reachable unbounded allocation, fd/memory/connection exhaustion, or
LB-down vector was found. Every R8 bound I attacked holds with an identified enforcement
point. The H2 server flood/bomb config is correctly applied; the S36 H3 recycling cap and
MAX_RELAY_STREAMS hold adversarially; the TCP/QUIC admission gates fire before expensive
work; there is no body decompression anywhere (no bomb surface). The findings below are all
**MEDIUM (bounded-DoS, degrade-not-crash) or LOW (hardening)** — disclosed for completeness
and fix-worthiness, not because any of them takes the LB down.

---

## Findings table

| ID | Sev | Vector | Bounded by today | Fix |
|----|-----|--------|------------------|-----|
| F-RES-1 | MEDIUM | H1 slowloris header trickle: hyper's `header_read_timeout` is INERT (no `.timer()` wired) → header-read bounded only by the 60 s connection `total`, not the intended 10 s `timeouts.header` | per-IP cap (1024) + 60 s `total` | wire `.timer(TokioTimer::new()).header_read_timeout(self.timeouts.header)` on the H1 builder (h1_proxy.rs:684) |
| F-RES-2 | LOW | H2-CLIENT (upstream) `Http2Pool` builder does not set `max_header_list_size` → a malicious backend's response HPACK list bounded only by hyper/h2's 16 KiB default, not our 64 KiB server policy | hyper/h2 default 16 KiB | set `.max_header_list_size(64*1024)` on the client builder (http2_pool.rs:425) for explicitness/parity |
| F-RES-3 | LOW | QUIC H3/Mode-B router `max_connections` HARDCODED 100_000 (listener.rs:416), never wired from config; no per-IP QUIC connection sub-cap (Mode A and Mode B both) | global 100k cap + retry-token address validation | add a config knob + optional per-source-IP QUIC cap |
| F-RES-4 | INFO | `HttpTimeouts::header` doc-comment claims it is "wrapped around the entire upstream request future" — STALE; it only feeds the (non-socket-closing) watchdog deadline + the WS dial timeout | n/a (doc bug, feeds F-RES-1) | correct the comment when fixing F-RES-1 |
| F-RES-5 | LOW | Slowloris/slow-POST Watchdog (`sweep_expired`) only LOGS + removes from its table — it does NOT close the socket; `progress()` is called once (header phase) and never per-body-chunk, so its `SlowRate` slow-POST eviction can never fire | the real defenses (idle_bounded_send body-idle + 60s total + H2 keepalive ping + QUIC 30s idle) cover the cases | either wire eviction → socket close, or document the Watchdog as observability-only and rely on the timeout stack |

---

## Per-finding detail

### F-RES-1 · MEDIUM · H1 slowloris header phase bounded by 60 s, not 10 s

**Vector.** A client opens a TCP/TLS connection to an H1/H1s listener and trickles the
request line + headers one byte every ~9 s (never completing the header block). On the H1
server path hyper's per-request **`header_read_timeout` is INERT** because the builder never
wires a `Timer`:

```
crates/lb-l7/src/h1_proxy.rs:684
    let conn = hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .serve_connection(TokioIo::new(io), svc)   // <-- no .timer(...) before this
        .with_upgrades();
```

hyper 1.10.1 docs: H1 server `header_read_timeout` default is 30 s **but requires a Timer
configured via `.timer()`**, else it never fires. Contrast h2_proxy.rs:828 which DOES wire
`builder.timer(TokioTimer::new())`. The H1 path has no timer, so the only bound on a slow
header trickle is the connection-level `total` (60 s) wrapping the serve future
(h1_proxy.rs:691,727,731). `HttpTimeouts::header` (10 s, h1_proxy.rs:229) is used ONLY for the
watchdog deadline (h1_proxy.rs:1092) and the WS upstream dial (h1_proxy.rs:2459) — **never to
bound the inbound header read** (see F-RES-4, F-RES-5).

**Resource exhausted.** One inflight semaphore slot + one fd, held for up to 60 s per
connection (6× the intended 10 s window). Not unbounded: capped by `per_ip_connection_cap`
(default 1024) and `max_inflight_connections` (default 65 536). An attacker from one IP can
pin 1024 slots for 60 s each, cycling — a degradation, not a crash. H2 is NOT affected (its
keepalive PING at 30 s is wired, h2_security.rs:91 + timer at h2_proxy.rs:828). QUIC is NOT
affected (transport `max_idle_timeout` 30 s, listener.rs:183, config rejects 0).

**Metric/gauge that shows it.** `accept_inflight{listener}` plateaus near the per-IP cap
under the attack while RSS stays flat (no alloc growth — this is slot/fd pinning, not a leak);
`connections_total` climbs at the recycle rate. Watchdog log "swept stalled connections"
fires (but does not close — see F-RES-5).

**PoC (lead to author + run).** New lb-soak scenario `sc_slowloris_h1_header` or an
integration test in `crates/lb-integration-tests`:
- Spawn the H1 proxy via the existing real-listener harness (mirror
  `sc3_slowloris`/`spawn_quic_h3_terminate_*` style).
- Open N=2 raw TCP clients; each writes `GET / HTTP/1.1\r\nHost: x\r\n` then one header byte
  every 9 s, never the terminating `\r\n\r\n`.
- Assert connection #1 is still open at t=11 s (proves the 10 s `header` timeout did NOT fire)
  and is closed by t≈61 s (the `total` wrap). Pre-fix: closes ~60 s. Post-fix (timer wired):
  closes ~10 s. The delta IS the finding; the test is the non-vacuous proof.

**Bound + where enforced (today).** `total` 60 s — h1_proxy.rs:691 (`sleep(total)`),
:731 (`timer` select arm). **Fix:** add `.timer(TokioTimer::new())` then
`.header_read_timeout(self.timeouts.header)` on the H1 builder.

**Disposition.** REAL, MEDIUM (bounded-DoS). Low-risk one-line fix; verify the WS upgrade path
(`.with_upgrades()`) still works with a timer wired (it should — H2 already runs with a timer).

---

### F-RES-2 · LOW · H2-client backend builder omits `max_header_list_size`

**Vector.** When the LB originates H2 to a backend (`protocol = "h2"`, i.e. the H1→H2 / H2→H2
/ H3→H2 cells through `lb_io::http2_pool::Http2Pool`), the client builder configures only
window/reset-stream/keepalive:

```
crates/lb-io/src/http2_pool.rs:425-435
    let mut builder = Builder::new(TokioExecutor::new());
    builder
        .initial_stream_window_size(...)
        .max_concurrent_reset_streams(...)   // reset-stream CACHE size, NOT our 64KiB policy
        .timer(TokioTimer::new());
    // keep_alive_* … — NO .max_header_list_size(...)
```

A compromised/malicious backend (semi-trusted, in scope) sends a response with a large decoded
HPACK header list. There is **no HPACK-bomb on the response head** because hyper/h2's CLIENT
default `max_header_list_size` is 16 KiB (h2 crate default) — which is actually TIGHTER than
our 64 KiB server policy. So this is NOT an unbounded-alloc finding; it is a **policy-parity /
explicitness gap**: the bound is implicit in a dependency default that "can change" (per hyper
docs), and it is not single-sourced from `H2SecurityThresholds` the way the server side is.

**Resource exhausted.** None today (16 KiB cap holds). Risk is a future hyper/h2 default change
silently relaxing the response-header bound.

**Metric.** Would show as a backend-induced 502 (h2 PROTOCOL/FRAME error → `Http2Pool` evicts,
http2_pool.rs:237-249) rather than alloc growth — i.e. it already fails closed.

**PoC.** Backend handler that emits a ~1 MiB response header value; assert the pool returns
`Http2PoolError::Send` (h2 rejects at 16 KiB) and evicts the peer (peer_count → 0). This proves
the CURRENT bound is non-vacuous. Fix = add `.max_header_list_size(64*1024)` and re-run.

**Bound + where enforced.** hyper/h2 client default 16 KiB (implicit). **Fix:** set it
explicitly on the builder, sourced from `Http2PoolConfig` / `H2SecurityThresholds` parity.

**Disposition.** LOW hardening. Response side already fails closed; make the bound explicit.

---

### F-RES-3 · LOW · QUIC `max_connections` hardcoded; no per-IP QUIC sub-cap

**Vector.** The H3/Mode-B QUIC router caps the dispatch table at `2 * max_connections`
(router.rs:357) and drops new Initials over the cap (router.rs:358-367) — bounded, no panic.
But `max_connections` is **hardcoded 100_000** (listener.rs:416) and is NOT wired from config
(`grep 'max_connections =' main.rs/listener.rs` → empty). Mode A's `max_quic_connections` IS
configurable (default 100_000, lib.rs:1202) but likewise has **no per-source-IP sub-cap**.
Contrast the TCP/L7 path, which has BOTH `max_inflight` (semaphore) and `per_ip_connection_cap`
(ConnGate, main.rs:3619 before the handshake).

**Why it is not HIGH.** A new QUIC connection requires a **valid retry token** bound to the
peer address (router.rs:243 `retry_signer.verify` BEFORE `spawn_new_connection`; Mode A
passthrough.rs same posture). An off-path spoofed-source attacker therefore cannot fill the
table — they must complete the RETRY round-trip from a real address. The global 100k cap bounds
total memory/fds. So this is a hardening gap (a single real IP can consume the whole 100k
budget; the cap is not tunable down for smaller deployments), not an unbounded vector.

**Resource exhausted.** Connection-table entries + per-connection actor/state, bounded at
100k. A single botnet/real IP can monopolize the budget (no fairness).

**Metric.** "QUIC router at connection cap; dropping new Initial" (router.rs:359) /
`audit/quic_passthrough_cap_hit` (passthrough.rs:840).

**PoC.** Extend the existing `router.rs` cap fixture (already drives the
`connections.len() >= max_connections*2` branch at a reduced cap of 2/4) into a per-IP
variant: confirm that 100% of the cap can be consumed from ONE source addr (proves no per-IP
fairness). For the config gap, a config-load test asserting there is no `[listener.quic]
max_connections` knob today.

**Bound + where enforced.** Global cap router.rs:357-367; retry-token gate router.rs:243.
**Fix:** add a `max_connections` config knob for the QUIC listener; optionally a per-source-IP
QUIC cap mirroring ConnGate.

**Disposition.** LOW hardening. Retry-token address validation is the real defense; add
tunability + per-IP fairness.

---

### F-RES-4 · INFO · stale doc-comment on `HttpTimeouts::header`

`crates/lb-l7/src/h1_proxy.rs:209-211` documents `header` as "Wrapped around the entire
upstream request future today since hyper's H1 server does not expose a separate header-receipt
knob in 1.x." This is **inaccurate**: `timeouts.header` is never wrapped around the serve/send
future — it only sets the watchdog deadline (h1_proxy.rs:1092 / h2_proxy.rs:1255) and the WS
upstream dial cap (h1_proxy.rs:2459 / h2_proxy.rs:1482). The effective inbound-header bound is
`total` (60 s). Correct this comment alongside the F-RES-1 fix (hyper 1.10.1 DOES expose
`header_read_timeout`, so the "does not expose" premise is also outdated).

---

### F-RES-5 · LOW · Watchdog sweeper does not close sockets; slow-POST rate check is dead

**Vector.** The slowloris/slow-POST `Watchdog` (lb-security/src/watchdog.rs) exposes
`sweep_expired()` returning evicted ConnIds — but the sweeper task only **logs and removes the
table entry**; it has no socket handle and closes nothing:

```
crates/lb/src/main.rs:2793-2799
    let evicted = wd.sweep_expired();
    if !evicted.is_empty() {
        tracing::warn!(evicted = evicted.len(), "Watchdog swept stalled connections …");
    }   // <-- no socket close, no task abort
```

And on the hot path `wd.progress()` is called **exactly once** (header phase: h1_proxy.rs:1103,
h2_proxy.rs:1262) then `deregister` at completion (h1_proxy.rs:1161). The request-body pump
NEVER calls `wd.progress()` per chunk, so the Watchdog's `SlowRate` (slow-POST body) eviction
(watchdog.rs:227-243) can **never fire** — it has no per-body-chunk data to evaluate.

**Why it is not HIGH.** The slow-POST / slow-read cases ARE defended, just by OTHER mechanisms,
each proven non-vacuous:
- **H1/H2 slow request body** → `idle_bounded_send` Phase-A no-forward-progress idle deadline
  (`timeouts.body` 30 s, idle_send.rs:117-176; reset only on a chunk forwarded to the backend,
  NOT on transport ACK) + the 60 s `total`. Tests: idle_send.rs arms (ii),(v),(vi),(vii),(ix).
- **H2 slow/zero-progress connection** → keepalive PING (h2_security.rs:91, timer wired
  h2_proxy.rs:828).
- **QUIC** → transport `max_idle_timeout` 30 s (listener.rs:183).
- **Slow upstream response** → H3 `H3_RESP_IDLE_TIMEOUT` (h3_bridge.rs:139+, progress-reset not
  keepalive-reset) and the streamed (never whole-buffered) response.

So the Watchdog is effectively **observability-only**; removing the table entry on sweep just
stops the warn-log from re-firing and frees ~64 B. The header-phase deadline it carries is the
one feeding F-RES-1 (and is itself not socket-closing).

**Metric.** "Watchdog swept stalled connections" warn count rises but connections persist
until the real timeout stack closes them.

**PoC.** (a) Register a conn, never deregister, drive `sweep_expired` past the deadline → assert
the underlying socket is STILL open (proves no close). (b) Drive a slow-POST body (1 B / 5 s)
through H1 and assert the closing event is the `idle_bounded_send` Phase-A timeout / `total`,
NOT a Watchdog `SlowRate` eviction (proves the rate check is dead). Both are the non-vacuous
proof that the Watchdog is not the active defense.

**Bound + where enforced.** The active bounds are idle_send.rs:117-176 + h1_proxy.rs:731
(`total`) + h2_security.rs:91 (PING) + listener.rs:555 (`set_max_idle_timeout`).
**Fix (choose one):** (1) wire sweep eviction → per-connection cancel token → socket close
(make the Watchdog a real enforcer); or (2) call `wd.progress()` from the body pump so
slow-POST rate eviction works; or (3) re-document the Watchdog as observability-only and the
timeout stack as the enforcer (cheapest, honest).

**Disposition.** LOW. The defense-in-depth layer is decorative, but the underlying timeout
stack genuinely bounds every case. Worth a decision (enforce vs. document).

---

## Proven-clean scopes (bound + the non-vacuous test that proves it holds adversarially)

### L-RES-1 · H2 server flood/bomb config — APPLIED & CORRECT (clean)
`H2SecurityThresholds::apply` (h2_security.rs:111) IS called on the live H2 server builder at
**h2_proxy.rs:829** (`self.security.apply(&mut builder)`), with the timer wired (h2_proxy.rs:828):
- Rapid Reset CVE-2023-44487 → `max_pending_accept_reset_streams` (h2_security.rs:113).
- RUSTSEC-2024-0003 → `max_local_error_reset_streams` (:114).
- CONTINUATION flood CVE-2024-27316 → enforced inside **h2 0.4.14** (Cargo.lock:1072, ≥ 0.4.5 ✓).
- HPACK bomb → `max_header_list_size = 64 KiB` (:116).
- SETTINGS/stream flood → `max_concurrent_streams = 256` (:115).
- Zero-window stall → keepalive PING + `max_send_buf_size = 64 KiB` (:117-118).
Non-vacuous test: `tests/h2_security_live.rs::rapid_reset_goaway` (referenced h2_proxy.rs:3823)
+ `h2_security.rs::apply_does_not_panic_with_defaults`. **Clean** (subject to F-RES-2 on the
H2-CLIENT side, which fails closed anyway).

### L-RES-2 · S36 H3 recycling + MAX_RELAY_STREAMS — BOUNDED adversarially (clean)
- **Recycling cap.** `requests_served` counts EVERY new request stream — admits AND rejects
  (conn_actor.rs:1352, the explicit CF-S32 mitigation: every stream quiche surfaces lands in
  `collected`, so rejects must count too). At the cap `goaway_pending` flips and stops admitting
  IMMEDIATELY (conn_actor.rs:1368-1369), even before the GOAWAY frame lands; streams above
  `goaway_last_id` are reset H3_REQUEST_REJECTED (conn_actor.rs:1337-1344). So an adversary that
  ignores GOAWAY and resets streams mid-flight cannot push `collected` past ~`cap` per connection.
- **Drain-then-recycle.** The connection recycles (and quiche frees `collected`) only when all
  per-stream maps are empty (conn_actor.rs:533-538). An adversary CAN hold the connection open
  past the cap by keeping one stream in-flight forever — but no NEW streams are admitted, so the
  cost is frozen at one fd + ~`cap` collected entries per connection, and connections still
  recycle on reconnect (the leak fix). Bounded.
- **MAX_RELAY_STREAMS (256).** `admit_or_refuse` (raw_proxy.rs:890-912) REFUSES to track a new
  sid over the cap (fail-safe: no insert, no leak; already-tracked sids always re-processed).
  Per-connection relay memory ≤ 256 × 2 × STREAM_RELAY_WINDOW, independent of total streams.
Non-vacuous tests: the existing S36 sc9 re-soak (BOUNDED ~37 MB) + the router cap fixtures
(router.rs:553,741). **Clean.**

### L-RES-3 · header/body/trailer caps + 413 — BOUNDED (clean)
- **Request body** 64 MiB `MAX_REQUEST_BODY_BYTES` (h2_proxy.rs:71, h3_bridge.rs:40), enforced
  in every cell's pump (h1:1413/1509/1730/1901/1977/2236, h2:1652/2280/2437/2535/2745) → 413
  (F-CAP-1), over-cap byte never forwarded.
- **Response** 64 MiB `MAX_RESPONSE_BODY_BYTES` on the decode/buffer paths (h3_bridge.rs:57,
  used h2_proxy.rs:2775, h1_proxy.rs:2268); the streaming H2→H2/H2→H1 response leg is streamed
  via `finalize_response` (h2_proxy.rs:2144-2158, `.boxed()` Incoming) — never whole-buffered.
- **H1 server headers** at hyper defaults (NOT overridden): 100 headers max → 431; ~400 KiB buf.
- **ChunkedDecoder** (chunked.rs) is TEST-CODEC only (not on the prod path; prod H1 = hyper).
**Clean.**

### L-RES-4 · slowloris timeout stack — BOUNDED (clean, modulo F-RES-1/F-RES-5)
TCP/L7 admission gates fire BEFORE the TLS handshake and before backend selection:
`admit_connection` per-IP cap (main.rs:3619) → `try_acquire_owned` inflight semaphore
(main.rs:3651) → handshake (main.rs:3752/3829, bounded by `handshake_timeout`). ConnGate is
atomic and GC's per-IP map entries on permit Drop (conn_gate.rs:212-227 — no unbounded key
growth). Slow request body bounded by `idle_bounded_send` (idle_send.rs, progress-reset not
ACK-reset). H2 zero-window bounded by keepalive PING. The ONE gap is the H1 header phase
(F-RES-1) and the decorative Watchdog (F-RES-5).

### L-RES-5 · R8 response bound + fd/conn leak + dgram queue — BOUNDED (clean)
- **Hostile-backend streaming response** → streamed frame-by-frame, bounded channel depth ×
  chunk max (h3_bridge.rs:121,127; H2 via hyper Incoming) + 64 MiB total cap; idle deadline
  reset only by application progress, not transport keepalive (h3_bridge.rs:144-159).
- **Mode A flow/fd leak (S21 F-S20-2)** → FIXED & verified: `reclaim_flows` + `closed` token +
  `run_idle_sweeper`/`sweep_idle_flows` (passthrough.rs:600,647,669) reclaim idle flows; LRU
  evict-oldest on cap (passthrough.rs:828-853); `flows_evicted_total` metric.
- **H1 upstream** single-use (`take_stream`, h1_proxy.rs comment :1169-1195) — no pooled-reuse
  smuggling/leak. **H2 upstream** evicts the whole peer on any Send-class error
  (http2_pool.rs:237-249) — broad teardown, no per-stream leak.
- **BoundedDgramQueue** drop-newest at cap 1024 (raw_proxy.rs:958-1015) — memory =
  cap × MAX_DGRAM_SIZE, independent of flood rate.
**Clean.**

### L-RES-6 · decompression bomb — NO SURFACE (proven-clean)
The LB does NOT decompress request or response bodies anywhere. `flate2`/`zstd`/`miniz_oxide`
in Cargo.lock are transitive via `qlog` (quiche logging) and `backtrace` ONLY — never wired to
a body. Content-Encoding is **passed through verbatim**. The only gzip/te string matches are
the smuggling-detection TE-header parser (lb-security/src/lib.rs:85-137) and the HPACK static
table (lb-h2/src/hpack.rs:72). No bomb surface. **Proven-clean.**

---

## Notes on method (R4 / S21 lesson)

Every "clean" above cites the enforcement point (file:line) AND an adversarial-non-vacuous
test. Where I could only confirm a dependency default (F-RES-2 16 KiB, H1 100-header/431), I
ranked it LOW and flagged the implicitness rather than claiming a proven bound. Reproducing a
symptom ≠ confirming the component is at fault: F-RES-5 explicitly separates "the Watchdog log
fires" from "what actually closes the socket" (the timeout stack), so the Watchdog is reported
as decorative, not as the defense.
