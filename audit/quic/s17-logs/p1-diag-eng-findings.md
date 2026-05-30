# S17 Phase 1 — CF-S16-RELAY-STALL diagnosis (diag-eng)

Author: diag-eng (fresh; no prior-agent judgment carried). One diagnostic pass,
HARD CAP honored. Every number below cites a COMPLETED, fully-read burst log.

Branch under test: `feature/quic-proxy-s17` @ `bbfd79a1` (S17 Phase 0 baseline).
Instrumentation built + run against the shared checkout
(`/home/ubuntu/Code/ExpressGateway`) with `--features test-gauges` (the test file
is `#![cfg(feature = "test-gauges")]`-gated — without the feature the binary has
0 tests; that gate is what produced an empty 6.4 MB binary in early build
attempts). Dedicated target dir `/home/ubuntu/Code/eg-s17-target` was used so the
instrumented build could not collide with the primed `/home/ubuntu/Code/eg-target`
fingerprints. Shared tree restored to PRISTINE after capture (0 `EG_RELAY_DIAG`
strings remain in `raw_proxy.rs` / `s16_b2_multistream.rs`; `git status` clean).

---

## 1. Step A — CODE-PROVABLE: relay arm-E (upstream `on_timeout`) reachability

`run_dual_pump` (raw_proxy.rs) `biased` `select!` arms, in poll order:

```
419  tokio::select! {
420      biased;
421      () = params.cancel.cancelled()        => break;           // arm A
424      pkt = params.inbound.recv()           => client.recv      // arm B
432      r = upstream.socket.recv_from(..)     => upstream.recv    // arm C
449      () = tokio::time::sleep(client_wait)  => params.conn.on_timeout();     // arm D
452      () = tokio::time::sleep(upstream_wait)=> upstream.conn.on_timeout();   // arm E
455  }
```

with, just above (raw_proxy.rs:408-417):

```
408  let mut client_wait   = params.conn.timeout().unwrap_or(IDLE_TICK);
409  let mut upstream_wait  = upstream.conn.timeout().unwrap_or(IDLE_TICK);
414  if !streams.is_empty() {
415      client_wait   = client_wait.min(RELAY_TICK);   // RELAY_TICK = 2ms
416      upstream_wait = upstream_wait.min(RELAY_TICK);
417  }
```

CONCLUSION (precise): with `biased`, arms are polled A→E in source order every
wake. The two `sleep`s are created at the same instant in the same iteration.
While `!streams.is_empty()`, BOTH waits are clamped to 2 ms, so whenever each
leg's quiche timeout is >= 2 ms the two sleeps are EQUAL-duration and become ready
at the same instant; the biased poll then completes on arm D (client `on_timeout`)
and DROPS arm E's future — `upstream.conn.on_timeout()` never runs on a tie.
Arm E is reachable ONLY in the narrow window where `upstream.conn.timeout()` is
strictly < 2 ms AND strictly < the client timeout (an upstream PTO/loss timer due
sooner than the 2 ms tick and sooner than the client's). So arm E is not strictly
dead by code alone, but it is starved to near-zero whenever any stream is in
flight. This is the production-immunity concern and is settled empirically in §4/§6.

## 2. Instrumentation added (env-gated `EG_RELAY_DIAG`, OFF by default; reverted)

RELAY — `run_dual_pump` (raw_proxy.rs):
- Per-arm fire counters `arm_b/arm_c/arm_d/arm_e` incremented inside arms B/C/D/E;
  `loop_iters`.
- A one-shot no-progress stall dump: client/upstream `path_stats()`
  (recv/sent/lost/retrans/total_pto/cwnd/bytes), `stats().acked_bytes`, per-`sid`
  half-state (c2u/u2c done/fin_sent/src_fin/pend) + `stream_finished`.
- `RELAY-FINAL` line at loop exit with the arm counters + `streams_left`.
- The detector's no-progress KEY was varied across three bursts to remove blind
  spots: v1 = (client_acked, upstream_acked, streams.len) gated on
  `!streams.is_empty()`; v2 = (client_acked, upstream_acked), un-gated, retaining
  each sid's last half-state across reclaim; v3 = UPSTREAM-leg `path_stats().recv`
  only (fires iff the relay stops ingesting backend packets).

BACKEND — `spawn_echo_backend` (s16_b2_multistream.rs):
- Counters `recv_ok` (each `recv_from`→Ok) and `on_timeout_calls` (the recv-timeout
  branch `c.on_timeout()`).
- One-shot `BACKEND-STALL` dump on 3 s of frozen `stats().acked_bytes`:
  `timeout()`, `stats()` sent/acked/lost bytes, `path_stats()`
  sent/lost/retrans/total_pto/stream_retrans_bytes/cwnd/recv, and per-`sid`
  `queued_left` / `peer_fin_seen` (e.1) / `our_fin_sent` (e.2) / `stream_finished`.
- `BACKEND-DEADLINE` line at the 60 s deadline.

## 3. Burst command + logs (R15: foreground, read to completion)

Binary: `/home/ubuntu/Code/eg-s17-target/debug/deps/s16_b2_multistream-d9bd20c52927799e`
(148 MB, `--list` = the one test; confirmed `EG_RELAY_DIAG` strings present).

Per burst (foreground, blocking):
```
for i in $(seq 1 50); do
  EG_RELAY_DIAG=1 timeout 40 "$BIN" --test-threads=1 --nocapture >> "$LOG" 2>&1
done
```

| burst | detector key | log (absolute) | iters | stalls (rc=101) |
|---|---|---|---|---|
| v1 | client+upstream acked, gated on streams non-empty | `/home/ubuntu/Code/eg-s17-evidence/burst-50.log` | 50 | 7 |
| v2 | client+upstream acked, un-gated, retain half-state | `/home/ubuntu/Code/eg-s17-evidence/burst-50-v2.log` | 50 | 4 |
| v3 | UPSTREAM-leg path_stats.recv only | `/home/ubuntu/Code/eg-s17-evidence/burst-50-v3.log` | 50 | 7 |

TOTAL: 150 iterations, 18 stalls (12%). All 18 stalls hit the 25 s `RELAY_BUDGET`
wall (`panicked at s16_b2_multistream.rs:706 ... Elapsed`). Each log read in full.

## 4. Per-stall classification (all 18 stalls)

DISCRIMINATOR (from the brief): backend `peer_fin_seen=false` ⇒ relay surface
(request FIN never arrived); backend `peer_fin_seen=true` + response tail un-acked
⇒ backend/response surface.

Uniform across ALL 18 stalls (representative cite: burst-50.log:45-51, ITER 5):
- Backend, EVERY sid (0/4/8/12/16): `peer_fin_seen=true`, `our_fin_sent=true`,
  `stream_finished=true`, `queued_left=0`. (Counts: `peer_fin_seen=false` = 0/18;
  `queued_left>0` = 0/18.)
- Backend `sent_bytes > acked_bytes` in 18/18 — un-acked response tail
  97–164 KB (burst-50.log: 107062, 97144, 107609, 151851, 125496, 164164, 108937).
- Backend `total_pto >= 1` (1 or 2) and `retrans` 90–134, `stream_retrans_bytes`
  57–120 KB in 18/18 — i.e. the backend PTO DID fire and it DID retransmit its tail
  (burst-50.log:46 `total_pto=1 retrans=109 stream_retrans_bytes=62224`).
- RELAY: `RELAY-STALL` fired 0/18 under ALL THREE detector keyings — including the
  v3 UPSTREAM-recv key (burst-50-v3.log: 0 RELAY-STALL, 7 BACKEND-STALL). The
  relay's upstream-leg `path_stats().recv` never froze for 3 s ⇒ the relay keeps
  ingesting backend packets throughout the stall.
- RELAY: `streams_left=0` in every `RELAY-FINAL` for the stalled iters — the relay
  reclaimed its per-stream table (it believes both halves of every stream `done`).
- RELAY arm counters: `arm_e` (upstream `on_timeout`) fired in exactly 1 of 150
  runs and never more than once (burst-50.log 50×arm_e=0; burst-50-v2.log
  49×arm_e=0 + 1×arm_e=1; burst-50-v3.log 50×arm_e=0). `arm_d` (client
  `on_timeout`) is 0 on healthy runs and ~1 on stalled runs.

| stall (cite) | stuck side | backend peer_fin_seen | backend total_pto / retrans | backend unacked | relay arm_e / arm_d | RELAY-STALL fired | class |
|---|---|---|---|---|---|---|---|
| b1 ITER5 (burst-50.log:45-52) | backend resp tail | true (all sids) | 1 / 109 | 107062 | 0 / 1 | no | RESPONSE-LEG |
| b1 ITER16 (171-177) | backend resp tail | true | 2 / 90 | 97144 | 0 / 1 | no | RESPONSE-LEG |
| b1 ITER24 (266-272) | backend resp tail | true | 1 / 97 | 107609 | 0 / 1 | no | RESPONSE-LEG |
| b1 ITER25 (290-296) | backend resp tail | true | 2 / 134 | 151851 | 0 / 1 | no | RESPONSE-LEG |
| b1 ITER32 (375-381) | backend resp tail | true | 2 / 99 | 125496 | 0 / 1 | no | RESPONSE-LEG |
| b1 ITER41 (481-487) | backend resp tail | true | 2 / 126 | 164164 | 0 / 1 | no | RESPONSE-LEG |
| b1 ITER45 (536-542) | backend resp tail | true | 2 / 100 | 108937 | 0 / 1 | no | RESPONSE-LEG |
| b2 ITER9 (burst-50-v2.log) | backend resp tail | true | 2 / 123 | 113838 | 1 / 0 | no | RESPONSE-LEG |
| b2 ITER11/14/30 | backend resp tail | true | >=1 | ~100KB | 0 / 1 | no | RESPONSE-LEG (×3) |
| b3 ITER3 (burst-50-v3.log) | backend resp tail | true | 2 / 95 | 124762 | 0 / 1 | no | RESPONSE-LEG |
| b3 ITER6/7/12/13/41/49 | backend resp tail | true | 2 | ~100-160KB | 0 | no | RESPONSE-LEG (×6) |

Classification tally: 18/18 = RESPONSE-LEG (backend→relay) stall.
0/18 = relay REQUEST-leg surface. 0/18 = "backend PTO flat" surface as the brief
framed surface #1 (PTO was NOT flat in any stall).

## 5. PROVEN mechanism (class proven; exact failing line is a deeper QUIC question)

The stall is a RESPONSE-leg (backend→relay→client) liveness failure with this
captured, reproducible fingerprint (18/18):

1. The request reaches the backend fully (`peer_fin_seen=true`, all sids,
   burst-50.log:47-51) — NOT a relay request-FIN loss.
2. The backend echoes everything and FINs every stream
   (`our_fin_sent=true`, `stream_finished=true`, `queued_left=0`,
   burst-50.log:47-51).
3. ~100-164 KB of the backend's RESPONSE data is permanently un-acked
   (`sent_bytes>acked_bytes`, burst-50.log:45) and the backend's own PTO fired and
   retransmitted (`total_pto>=1`, `retrans`/`stream_retrans_bytes`>0,
   burst-50.log:46) — so the missing flight is NOT a single un-retransmitted FIN
   packet; the brief's "backend PTO never fires" surface is DISPROVEN here.
4. The relay keeps RECEIVING backend packets right through the stall (v3 detector
   keyed on upstream `path_stats().recv` NEVER fired: burst-50-v3.log 0 RELAY-STALL
   / 7 BACKEND-STALL), yet the backend's `acked_bytes` stays frozen — i.e. the
   relay receives the backend's retransmitted tail but its upstream
   `quiche::Connection` does NOT advance ACKs back to the backend.
5. The relay has reclaimed the stream (`RELAY-FINAL streams_left=0` on every
   stalled iter) — it believes both halves `done` (`u2c.fin_sent`), so it no longer
   tracks the stream and its 2 ms `RELAY_TICK` clamp drops (`streams.is_empty()`),
   reverting to the 100 ms `IDLE_TICK` park; and it never services the upstream
   loss/ACK timer (arm_e ~0/150), so its ACK-to-backend cadence on that now-
   "finished" stream collapses.

Net: the relay's upstream leg considers the response stream complete and stops
ACK-advancing the backend's retransmitted tail, while the backend, still missing
ACKs, retransmits into a relay that never closes the loop. The CLIENT is left short
of the same tail (test panic `client must receive every echoed payload`,
burst-50.log:54) → 25 s budget wall. The starved relay upstream `on_timeout`
(arm_e ~0) is the production-relevant aggravator (the relay never independently
services its upstream loss/ACK timer while busy), but the captured trigger is the
relay declaring the response stream finished and ceasing to ACK its tail — a relay
loss-recovery/ACK-continuity question on the `upstream.conn` leg, not a backend
retransmit-cadence bug and not a relay request-FIN loss.

This is consistent with Phase 0's corroboration that `s16_b2_backpressure`'s
resume path ALSO stalls (90 s Elapsed) — same general response-leg liveness class.

HARD-CAP NOTE: the CLASS is proven and both lead-candidate surfaces are
empirically settled (see §6), but the EXACT failing instruction (why the relay's
`upstream.conn` stops ACK-advancing the retransmitted finished-stream tail) is a
deeper quiche ACK/loss-recovery question that I did NOT isolate to a line in one
pass, and I did NOT spin a fourth hypothesis or attempt a fix. Recommend
R6/verifier independent reproduction before a fix is designed.

## 6. Production-immunity verdict

- Is the RELAY arm-E (upstream `on_timeout`) starvation real? YES.
  - By code (§1): on a 2 ms == 2 ms biased tie, arm D wins and arm E is dropped;
    arm E only reachable when upstream timeout < 2 ms and < client timeout.
  - By capture: `arm_e` fired in 1 of 150 instrumented runs (149× zero) while
    streams were in flight (burst-50.log / -v2 / -v3 RELAY-FINAL lines). The relay
    essentially NEVER services `upstream.conn.on_timeout()`. This is a genuine
    PRODUCTION property of `run_dual_pump`, independent of this test.
- Did arm-E starvation CAUSE this particular stall? NOT directly. ACKs are emitted
  on packet receipt, not on `on_timeout`; the relay kept receiving backend packets
  (v3) yet stopped advancing ACKs after declaring the stream finished. arm-E
  starvation means the relay also never PTO-retransmits/loss-recovers its OWN
  upstream send tail — a real latent production liveness hole that would bite a real
  backend that relied on the relay to retransmit a request-leg tail, but it is not
  the captured trigger of the multistream stall (which is response-leg).
- Fix scope: the production relay is implicated (NOT "test-only"). Two distinct
  production concerns surfaced, both on `run_dual_pump`'s `upstream.conn` handling:
  (a) the captured trigger — the relay ceasing ACK-continuity on a response stream
  it has marked finished while the peer is still retransmitting that stream's tail;
  (b) the proven latent starvation — `upstream.conn.on_timeout()` (arm E) being
  biased-starved so the relay never independently services its upstream loss/PTO
  timer. Neither is fixed by changing only the echo backend. So the Phase-2 fix is
  a PRODUCTION fix on the relay (R12, single-sourced, and the Mode A / matrix
  relay-adjacent `biased`-tie code should be checked for the same arm-E pattern),
  NOT test-only. The earlier S16 "test-rig bug / production relay CORRECT" verdict
  is contradicted by this capture.

---

### Evidence files (all absolute, persist outside the worktree)
- `/home/ubuntu/Code/eg-s17-evidence/diag-eng-findings.md` (this file)
- `/home/ubuntu/Code/eg-s17-evidence/burst-50.log`     (v1, 50 iters, 7 stalls)
- `/home/ubuntu/Code/eg-s17-evidence/burst-50-v2.log`  (v2, 50 iters, 4 stalls)
- `/home/ubuntu/Code/eg-s17-evidence/burst-50-v3.log`  (v3, 50 iters, 7 stalls)
- `/home/ubuntu/Code/eg-s17-evidence/raw_proxy.rs.orig` / `s16_b2_multistream.rs.orig`
  (pristine backups taken before instrumentation)

Shared checkout `/home/ubuntu/Code/ExpressGateway` restored PRISTINE (0
`EG_RELAY_DIAG`; `git status` clean for both files). Throwaway branch
`s17-diag-eng-instr` deleted. Dedicated build dir `/home/ubuntu/Code/eg-s17-target`
may be removed.
