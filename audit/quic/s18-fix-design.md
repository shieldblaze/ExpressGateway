# S18 Phase 2 — pump_dir fix design (lead-approved plan)

## Root (Phase 1, PROVEN + independently-confirmation-pending)
`raw_proxy.rs::pump_dir` read gate (`while half.pending.len() < STREAM_RELAY_WINDOW`, :751)
is NOT gated on `half.src_fin_seen`. When the source FIN is read, quiche delivers the FIN
AND collects (reclaims) the stream in the same `stream_recv` (`stream.is_complete()`). If the
drain into `dst.stream_send` short-wrote on that turn (client-leg flow-control momentarily
tight), `half.pending` still holds an undelivered tail, so the FIN-forward block (:876,
`src_fin_seen && pending.is_empty() && !fin_sent`) is correctly skipped. The NEXT turn's read
gate re-enters and calls `src.stream_recv` on the now-collected stream → `Err(InvalidStreamState)`
→ the generic read-error arm (:796) runs `half.pending.clear(); half.done = true` → the tail is
DROPPED and the FIN never forwarded. Client is permanently short → 20s idle-close → budget wall.
Measured: `dropping_pending == short_by` 4/4 both paths; only `PUMP-READ-ERR InvalidStreamState`
fires; `bytes_in_flight=0`, `loss_timer=None` (explains S17's `total_pto=0`).

## THE FIX (proven minimal — diag-eng B2: 0/90 multistream + 0/75 backpressure)
Gate the read loop on `!half.src_fin_seen`:
```rust
// Read gate: pull from src only while pending is below the window AND the
// source FIN has not yet been observed. Once the source FIN is read, quiche
// has COLLECTED the stream (stream.is_complete()); re-issuing stream_recv on a
// collected stream returns Err(InvalidStreamState), which the generic read-error
// arm below would treat as a fault and DROP the still-pending tail + the FIN
// (CF-S16-RELAY-STALL). There is nothing more to read after the FIN, so skip the
// read entirely and let the pending tail drain + the deferred FIN forward on
// subsequent turns.
while !half.src_fin_seen && half.pending.len() < STREAM_RELAY_WINDOW {
    ...unchanged...
}
```
This is a ONE-condition change. Everything below the read loop (drain, FIN-forward, B3
reset/stop propagation) is UNTOUCHED. The drain + FIN-forward blocks already run every turn
after the read loop, so the pending tail drains over subsequent 2ms-tick turns and the FIN
forwards via the existing `:876` block once pending empties.

### Why this is correct + complete
- Once `src_fin_seen`, the source stream is exhausted — there is genuinely nothing more to read.
- The bounded-window backpressure (R8) is preserved: the read gate still caps pending at
  `STREAM_RELAY_WINDOW` while reading; after FIN we only DRAIN (which can't grow pending).
- The B3 smuggling guard is preserved: a peer RESET_STREAM surfaces as `Err(StreamReset)`
  (its own arm, :776) BEFORE src_fin_seen would be set on a truncated transfer; a genuine
  fault still drops-without-FIN. This change only stops discarding a CLEANLY-FINISHED
  stream's buffered tail.
- SINGLE-SOURCED (R12): `pump_dir` is the one helper for BOTH relay directions (u→c and
  c→u), so this fixes the symmetric request-leg case too. No sibling divergence.

### Scope decision: ship (a) ONLY
diag-eng noted an optional belt-and-suspenders (b): make the generic read-error arm treat
`InvalidStreamState` on an already-`src_fin_seen` half as benign. With (a) in place the read
is never issued post-FIN, so (b) is unreachable for this bug. We ship the MINIMAL proven
change (a) and do NOT add untested branch logic. (b) noted as a non-blocking consideration.

## REGRESSION TEST (R13(c) load-bearing negative control, made deterministic)
Add a `#[cfg(test)]` test in `raw_proxy.rs`'s test module that drives `pump_dir` directly and
DETERMINISTICALLY reproduces the post-FIN short-write → re-read drop:
1. Build a src↔relay↔dst trio (reuse `established_pair`/`handshake_pair`/`pump_once`). Configure
   the `dst` peer (client) with a SMALL `initial_max_stream_data_bidi_remote` so the relay's
   `dst.stream_send` short-writes deterministically, leaving a pending tail.
2. Put a multi-KiB payload + FIN on the src stream; deliver it so a single `pump_dir` call reads
   the data + FIN (`src_fin_seen=true`) but the drain short-writes (pending non-empty).
3. Call `pump_dir` AGAIN (the buggy re-read turn).
4. Assert the relay did NOT drop: `half.pending` was preserved/drained and, after enough
   turns + opening the dst window, the dst stream receives the FULL byte-identical payload + a
   clean FIN (`half.fin_sent == true`, `half.done` via FIN not via drop).
**Prove it is load-bearing**: temporarily revert the one-line fix, run the new test, CONFIRM it
FAILS (drop observed), restore the fix, CONFIRM it PASSES. Cite both completed runs.

## Verification ownership
- builder-1: implement (a) + the regression test; compile; demonstrate the negative control
  (fails pre-fix, passes post-fix); commit+push. Self-check only.
- verifier (separate worktree, quiet box): independently reproduce the InvalidStreamState drop
  on the UNFIXED tree (confirm root), then verify the SHIPPED fix to R13 — ×3 gate clean,
  ≥75-iter dual-path isolation burst (s16_b2_multistream + s16_b2_backpressure resume) ZERO
  stalls, load-bearing negative control. Independent runs, cite completed logs (R15).
- lead: diff-review, adjudicate against completed logs.
