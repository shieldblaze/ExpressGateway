# S18 Phase 1 — CF-S16-RELAY-STALL diag-eng findings (three-bucket diagnosis + decisive intervention)

Author: **diag-eng** (S18 Phase 1). FRESH judgment, one focused pass. Every number
below cites a COMPLETED, fully-read burst log (R15: no result is quoted from an
incomplete/backgrounded job — each burst ran to a `BURST_DONE` line that was read
before any number was written here).

Worked in an isolated worktree off `feature/quic-proxy-s18` @ `566908a1` (the diag
brief's base, which carries all Mode B code). Shared checkout
`/home/ubuntu/Code/ExpressGateway` left **PRISTINE** except this one findings file
(verified: 0 `EG_RELAY_DIAG3`/`EG_RELAY_FIX_READAFTERFIN`/`EG_CLIENT_ONTIMEOUT`/
`vendor-quiche-instr` strings in the shared `crates/` or `Cargo.toml`; `git status`
clean apart from `audit/quic/s18-logs/`). No `git stash`. Dedicated target dir
`/home/ubuntu/Code/eg-s18-diag-target`. Instrumentation (quiche `[patch.crates-io]`
debug accessors + env-gated dumps in `raw_proxy.rs`/the two tests) is worktree-only
and UNCOMMITTED.

---

## VERDICT (one line)

**ROOT: a relay-side DATA-DROP bug in `raw_proxy.rs::pump_dir` — after the upstream
source FIN is read (and quiche COLLECTS that stream in the same `stream_recv`), the
NEXT relay turn's read gate re-issues `stream_recv` on the now-collected stream,
gets `Err(InvalidStreamState)`, and the generic read-error arm executes
`half.pending.clear(); half.done = true` — DISCARDING the still-undrained `u2c`
tail and never forwarding the FIN.** The client is left permanently short → LB
client conn idle-closes at ~20 s → the test hits its budget. **Fix LOCUS =
relay-side (`pump_dir` read gate / read-error arm), production code.** NOT
test-side, NOT quiche, NOT loss recovery.

This **REFUTES** S17's "lost client-leg tail never retransmitted" framing AND the
brief's primary hypothesis (flow-control-blocked buffered tail fixed by client
`on_timeout`): the tail is neither in flight nor buffered nor window-blocked at the
stall — it has been **deleted** by the relay.

---

## BUCKET CLASSIFICATION

Per the brief's three buckets, this is **bucket (a) BUFFERED-NOT-SENT class, but a
NEW sub-split the brief did not enumerate**:

- NOT (a)-flow-control (`stream_capacity == 0`): at every stall `stream_capacity =
  Ok(>0)` — the client leg's flow-control window is OPEN.
- NOT (a)-cwnd (`bytes_in_flight >= cwnd`): `bytes_in_flight = 0` at every stall.
- NOT (a)-relay-not-flushing: the relay is not holding the bytes — it has DROPPED
  them (`send_buf_len = 0`, `send_fin_off = None`).
- NOT (b) IN-FLIGHT-NO-PTO: `bytes_in_flight = 0` (nothing in flight).
- NOT (c) RETRANSMIT-SUPPRESSED: no loss timer armed and nothing to retransmit.

**New sub-split: (a)-RELAY-DROPS-BUFFERED-TAIL** — the relay cleared its own
pending `u2c` tail on a spurious post-FIN re-read of a collected source stream. By
the time the LB idle-closes, `params.conn` genuinely has nothing to send (the bytes
were freed), which is exactly why `bytes_in_flight = 0`, `send_buf_len = 0`, no loss
timer, and no PTO — the relay never had a tail in flight to recover.

---

## TASK A — MEASURED bucket state at ≥3 distinct stalls, BOTH paths

Instrumentation: env-gated `EG_RELAY_DIAG3` (OFF by default). Worktree-only quiche
`[patch.crates-io]` adds read-only `pub` accessors: `SendBuf::send_buf_len`,
`send_fin_off`; `Connection::debug_recovery` (active-path `bytes_in_flight`, `cwnd`,
`loss_detection_timer`), `debug_stream_send_buf_len`, `debug_stream_send_off_back`,
`debug_stream_send_fin_off`, `debug_stream_recv_fin`, `debug_stream_recv_state`. An
at-EXIT dump in `run_dual_pump` (the loop exits at ~20 s client idle-close on a
stall — the approach that captured S17's state) plus a CLIENT-side near-budget dump
in the multistream driver and per-arm `PUMP-*` traces in `pump_dir`.

### Path 1 — `s16_b2_multistream` (5 concurrent streams)

Baseline rate (single-threaded, DIAG3 on): **15/90, 14/90, 10/60** across three
bursts (≈11–17 %; higher than S17's parallel 5.9 % because single-thread
amplifies the race). Cite:
`/home/ubuntu/Code/eg-s18-diag-evidence/multistream-diag3-baseline-90.log`
(15/90), `…/multistream-diag3-upstreamcap-90.log` (14/90),
`…/multistream-diag3-fintrace-60.log` (10/60).

Captured at the stall, for the SHORT sid on `params.conn` (LB→client leg)
— cite `…/multistream-diag3-upstreamcap-90.log` and `…-clientcap-90.log`:

| iter | short sid | short_by | send_buf_len | send_off_back | stream_capacity | bytes_in_flight | cwnd | loss_timer | client got_len |
|---|---|---|---|---|---|---|---|---|---|
| 2  | 16 | 1791  | 0 | 128209 | Ok(22950) | 0 | 22950 | None | 128209 |
| 16 | 4  | 1945  | 0 | 58055  | Ok(29700) | 0 | 29700 | None | (= off_back) |
| 27 | 4  | 6173  | 0 | 53827  | Ok(29700) | 0 | 29700 | None | (= off_back) |
| 31 | 4  | 14542 | 0 | 45458  | Ok(12150) | 0 | 12150 | None | (= off_back) |
| 39 | 16 | 7276  | 0 | 122724 | Ok(25650) | 0 | 25650 | None | (= off_back) |

Invariants across **15/15 + 14/14** stalls:
- `bytes_in_flight = 0`, `loss_timer = None`, `loss_timer_armed = false` — 15/15.
- `send_buf_len = 0` (client-leg send buffer EMPTY) — every stall.
- `stream_capacity = Ok(>0)` (client-leg flow-control OPEN) — every stall.
- The short sid + amount VARY (sid ∈ {0,4,8,16}; short_by 162–54851 B) — the
  lost-flight signature, but see below: it is a DROP not a loss.
- **`relay send_off_back == client got_len` exactly — 7/7** in the client-capture
  burst (`…-clientcap-90.log`): the client received PRECISELY what the LB buffered
  into the stream; ZERO bytes lost on the LB→client wire. The LB simply never had
  the last `short_by` bytes to send.
- `send_fin_off = Some(None)` on the short sid — **10/10** (`…-fintrace-60.log`):
  NO FIN was ever queued on the client leg; `client_stream_finished = false`,
  `got_fin = false`, `client_readable = false` (client drained all it got and waits
  for bytes + a FIN that never come).

### Path 2 — `s16_b2_backpressure` (resume path; single 4 MiB stream)

Baseline rate (DIAG3 on): **4/75**. Cite:
`/home/ubuntu/Code/eg-s18-diag-evidence/backpressure-baseline-75.log`.

| iter | short_by (=dropped pending) | send_buf_len | send_off_back | send_fin_off | stream_capacity | bytes_in_flight | loss_timer | break_reason |
|---|---|---|---|---|---|---|---|---|
| 12 | 11288 | 0 | 4183016 | None | Ok(32848) | 0 | None | upstream_closed |
| 60 | 39312 | 0 | 4154992 | None | Ok(58050) | 0 | None | upstream_closed |
| 71 | 11934 | 0 | 4182370 | None | Ok(43273) | 0 | None | upstream_closed |
| 72 | 22753 | 0 | 4171551 | None | Ok(58050) | 0 | None | upstream_closed |

Identical bucket state to Path 1 (`send_buf_len=0`, `bytes_in_flight=0`,
`loss_timer=None`, `stream_capacity>0`, `send_fin_off=None`,
`send_off_back = 4194304 - short_by`). The break_reason differs only because the
backpressure test's backend closes the upstream at its own ~34 s deadline before the
client idle-closes — the FAILING mechanism is the same.

### The mechanism (per-arm trace, BOTH paths)

For the short sid, dir `u→c`, every stall shows EXACTLY this pair (cite
`…-fintrace-60.log`, `…-droparm-60.log`, `backpressure-baseline-75.log`):

```
PUMP-FIN-SEEN  sid=S dir=u→c n_this_read=N pending_now=P src_recv_state=None
PUMP-READ-ERR  sid=S dir=u→c err=InvalidStreamState(S) dropping_pending=D src_fin_seen=true
```

- `PUMP-FIN-SEEN … src_recv_state=None`: the `stream_recv` that delivered the
  backend echo FIN ALSO collected the upstream stream (so the very next read sees it
  gone).
- `PUMP-READ-ERR … err=InvalidStreamState … dropping_pending=D`: the next relay
  turn's read gate re-reads the collected stream, hits `InvalidStreamState`, and the
  generic read-error arm clears `D` undrained bytes.
- **`dropping_pending (D) == short_by` — 4/4 multistream droparm stalls AND 4/4
  backpressure stalls.** The dropped pending IS the missing tail.
- Drop-arm tally across both droparm bursts: **only `PUMP-READ-ERR` ever fires**
  (multistream 4/4, backpressure 4/4); `PUMP-DRAIN-ERR` / `PUMP-DRAIN-STOPPED`:
  never.

Why the relay still has undrained pending when this happens: the `u2c` drain into
`params.conn.stream_send` took a SHORT write on a prior turn (client-leg flow-control
momentarily tight), leaving a tail in `half.pending` with `src_fin_seen=true`; the
FIN block (`src_fin_seen && pending.is_empty() && !fin_sent`) is correctly skipped
while pending is non-empty; but the read gate (`while half.pending.len() <
STREAM_RELAY_WINDOW`) is NOT gated on `src_fin_seen`, so it re-issues `stream_recv`
on the collected source and trips the drop.

### The quiche `recovery.rs` predicate that explains the recovery state

`src/recovery/congestion/recovery.rs::set_loss_detection_timer` (lines 485–490):

```rust
if self.bytes_in_flight.is_zero() && handshake_status.peer_verified_address {
    self.loss_timer.clear();
    return;
}
```

When the relay drops the tail, `params.conn` has no un-acked client-leg data left
(everything it actually sent was acked) ⇒ `bytes_in_flight == 0` ⇒ quiche CLEARS the
loss timer and arms NO PTO (confirmed: `loss_timer = None`, `loss_timer_armed =
false` at 15/15 stalls; `pto_time_and_space` is never consulted because the timer is
cleared first). This is why S17 saw `total_pto=0` and forcing `on_timeout` thousands
of times never retransmitted anything — there was nothing to retransmit; the bytes
were already freed. The `InvalidStreamState` collection is grounded in
`src/lib.rs::do_stream_recv` (≈5714: `self.streams.get_mut(id).ok_or(
Error::InvalidStreamState(id))?`) + the post-FIN `self.streams.collect(...)` when
`stream.is_complete()` (≈5768).

---

## TASK B — decisive interventions (measured pre/post, same binary, env-gated)

All three measured with the SAME compiled binary; the only delta is an env var, so
the comparison is apples-to-apples. Each burst ran to its `BURST_DONE` line, read in
full.

### B-pre (baseline, no env): multistream **10/90** ; backpressure **4/75**
Cite: `…/multistream-taskB-baseline-90.log`,
`…/backpressure-baseline-75.log`.

### B-1 (brief's PRESCRIBED intervention — REFUTED)
`EG_CLIENT_ONTIMEOUT=1`: the multistream test client driver services
`client_conn.on_timeout()` unconditionally every loop (strongest form — drives the
client's own loss recovery so any lost client→relay MAX_STREAM_DATA/ACK is
retransmitted). **Post: 12/90 ≈ baseline.** Cite:
`…/multistream-taskB-clientontimeout-90.log` (`BURST_DONE … stalls=12`).
**REFUTED** — the root is NOT "the test client never runs loss recovery"; it is not
a test-harness artifact. (Consistent with the measured mechanism: no client window
update can recover bytes the relay has already deleted.)

### B-2 (root-targeted intervention — CONFIRMS the root → ZERO)
`EG_RELAY_FIX_READAFTERFIN=1`: the `pump_dir` read gate gains `&& !half.src_fin_seen`
(stop reading the source once its FIN has been observed — there is nothing more to
read, so the spurious `InvalidStreamState` re-read that clears the tail can never
happen; the still-pending bytes drain on subsequent turns and the FIN is forwarded
normally). **Post: multistream 0/90 ; backpressure 0/75 — stall ELIMINATED on BOTH
paths.** Cite: `…/multistream-taskB-fixreadafterfin-90.log` (`stalls=0`),
`…/backpressure-fixreadafterfin-75.log` (`stalls=0`).

This is the Koch test: removing the suspected cause removes the effect (→0), and the
brief's alternative hypothesis does not (≈baseline). Root CONFIRMED.

> NB: `&& !half.src_fin_seen` is the minimal DIAGNOSTIC gate that proves the
> mechanism; it is NOT being shipped. A production fix should make the read-error
> arm DISTINGUISH "source exhausted/collected after a clean FIN" (drain remaining
> pending + forward the deferred FIN — a clean completion) from a genuine fault
> (the existing fail-safe drop). That is a builder's design call; this pass only
> pins the root and proves the fix locus.

---

## ROOT STATEMENT + recommended fix LOCUS

**ROOT (one line):** `raw_proxy.rs::pump_dir` drops the buffered `u2c` tail (and
never forwards the FIN) when, after the source FIN is read and quiche collects the
stream, the next turn's read gate re-issues `stream_recv` on the collected stream,
hits `Err(InvalidStreamState)`, and the generic read-error arm clears still-pending,
not-yet-delivered bytes.

**Fix LOCUS: relay-side, production code — `crates/lb-quic/src/raw_proxy.rs`
`pump_dir`.** Either (a) gate the read on `!half.src_fin_seen` (do not re-read a
FIN'd/collected source), and/or (b) in the read-side error arm, treat
`InvalidStreamState`/`Done` on an already-`src_fin_seen` half as "source drained —
keep `pending` and let it drain + forward the deferred FIN" rather than the generic
fail-safe drop. The B2 smuggling guard is preserved (a real reset still tears down
without a FIN; this only stops discarding a CLEANLY-FINISHED stream's buffered
tail). NOT a quiche change; NOT a test change (the tests are correct — they simply
expose the relay defect under client-leg flow-control timing).

---

## Evidence logs (all COMPLETED + fully read)

- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-diag3-baseline-90.log` — 15/90; recovery state at all stalls.
- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-diag3-clientcap-90.log` — 7/90; proves `send_off_back == client got_len` 7/7.
- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-diag3-upstreamcap-90.log` — 14/90; upstream collected + relay-half reclaimed at all stalls.
- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-diag3-fintrace-60.log` — 10/60; `send_fin_off=None` 10/10 + PUMP-FIN traces.
- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-diag3-droparm-60.log` — 4/60; drop arm = `PUMP-READ-ERR InvalidStreamState`, `dropping_pending==short_by` 4/4.
- `/home/ubuntu/Code/eg-s18-diag-evidence/backpressure-baseline-75.log` — 4/75; same drop signature on the resume path, `dropping_pending==short_by` 4/4.
- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-taskB-baseline-90.log` — TASK B pre, 10/90.
- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-taskB-clientontimeout-90.log` — TASK B-1 (brief's prescribed), 12/90 — REFUTED.
- `/home/ubuntu/Code/eg-s18-diag-evidence/multistream-taskB-fixreadafterfin-90.log` — TASK B-2 (root fix), 0/90.
- `/home/ubuntu/Code/eg-s18-diag-evidence/backpressure-fixreadafterfin-75.log` — TASK B-2 on resume path, 0/75.

## Hygiene

Shared checkout PRISTINE (verified: `git status` shows only `audit/quic/s18-logs/`;
0 instrumentation-env or `vendor-quiche-instr` strings in shared `crates/` or root
`Cargo.toml`). Instrumentation lives only in the worktree (quiche
`./vendor-quiche-instr` patch + env-gated `raw_proxy.rs`/test dumps), uncommitted,
throwaway. No `git stash`. Target dir `/home/ubuntu/Code/eg-s18-diag-target` and
evidence dir `/home/ubuntu/Code/eg-s18-diag-evidence` may be removed.
