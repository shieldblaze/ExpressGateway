# DEFECT (S5 / task #6 verification) — client cancel of the H3 RESPONSE stream is not propagated to stop the upstream read

Tier: **correctness + resource-exhaustion (security-adjacent)**. Binding
conditions violated: §1.3.4 `RespAbort::ClientGone` contract, binding
condition **C2** (a partially-consumed / abandoned upstream MUST be
dropped, never held/parked), plan §1.4.6 client-cancel teardown.

Found by: `r6_client_cancel_midresponse_stops_upstream_read` and the
`C2/ClientGone` arm of `c2_every_abort_variant_drops_pooled_upstream_and_resets`
in `crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs` (real-wire: real
QuicListener → router → conn_actor → h3_bridge::stream_h1_response →
real H1 backend). NOT a proxy unit test; NOT a timeout-as-failure — the
mechanism is identified in the product source below.

## Proven mechanism

For a **bodyless H3 GET** (HEADERS+FIN on the request stream — exactly
what `drive_h3_response_client` sends, and the common proxy case), the
request stream is fully received and FIN'd. No `body_tx_by_stream` /
`rx_by_stream` entry remains for that stream, so the S2 request-side
peer-reset handlers (`conn_actor.rs:861`, `:944` — which DO correctly
map `StreamReset`/`StreamStopped` → `ReqBodyEvent::Reset` and tear down
per-stream state) are never entered for this stream again.

The client then cancels the **response** stream
(`stream_shutdown(stream_id, Shutdown::Read, code)` → QUIC
STOP_SENDING on the proxy's write side). The actor has **no code path
that observes a client STOP_SENDING / RESET on the response stream**:

- `drain_streams_to_conn` `StreamTx::Progressive` branch
  (`conn_actor.rs:495-549`): the only place the response stream is
  touched. `conn.stream_send(sid, front, false)` would surface the
  client STOP_SENDING as `Err(quiche::Error::StreamStopped(code))`, but
  that is swallowed by the catch-all
  `Err(e) => { tracing::debug!(...); break; }` at
  `conn_actor.rs:519-522`. It does NOT set `reset`, does NOT push `sid`
  to `to_drop`, does NOT remove `resp_rx_by_stream[sid]`, does NOT
  signal the producer.
- Worse: when the `Progressive.queue` is empty at cancel time (the
  common case for a slow client that has drained what it has),
  `conn.stream_send` is **not even called**, so even the swallowed
  error path is unreached. There is no `conn.readable()` /
  STOP_SENDING poll for the response stream anywhere.
- `resp_rx_by_stream` is removed NOWHERE in `conn_actor.rs` on a
  client-cancel (grep: removal sites are all `rx_by_stream` /
  `body_*`, the request side). It is only ever inserted
  (`conn_actor.rs:791`) and drained (`drain_resp_channels`).

Consequence: the producer task's `stream_h1_response` →
`tx.send(RespEvent::…).await` never observes a closed receiver, so it
never returns `Err(RespAbort::ClientGone)` (`h3_bridge.rs:1059`). The
producer keeps calling `stream.read()` on the pooled upstream
**forever**. The pooled connection is held by the never-finishing
producer task and `pooled.set_reusable(false)` /
`h3_to_h1_stream_resp`'s C2 teardown (`h3_bridge.rs:1813`) is never
reached.

## Observed (real wire)

- `r6_client_cancel_midresponse_stops_upstream_read`: endless backend
  (1 TiB declared `Content-Length`); client cancels after 32 KiB. The
  backend's read half NEVER closes within 20 s ⇒
  `read_closed.notified()` times out. The backend keeps being able to
  write (the proxy is still draining the upstream socket). FAIL.
- `c2_every_abort_variant_drops_pooled_upstream_and_resets`
  `C2/ClientGone` arm: same root cause; the test connection eventually
  idle-times-out → `"conn closed early: ... status=Some(200)
  body=32768"` (body == the 32 KiB cancel threshold — the proxy
  delivered 32 KiB then the client cancelled and the proxy never tore
  down).
- `RUST_LOG=lb_quic=debug` shows NO `stream_send (resp)` /
  `stream_shutdown (resp)` / `resp stream aborted` / `ClientGone` /
  `peer reset/stopped` log line for the response stream — confirming
  the path is never reached (not merely mis-handled).

## Impact

1. Resource exhaustion / DoS: a client that opens H3 requests and
   immediately STOP_SENDINGs the response stream leaves the proxy
   reading each upstream indefinitely; pooled upstream connections and
   producer tasks accumulate without bound. (1 TiB-body upstream in the
   repro never stops.)
2. Binding C2 violated: the abandoned upstream is neither dropped nor
   marked non-reusable on client-cancel.
3. §1.3.4 `ClientGone` is specified but unreachable on this path: the
   only `ClientGone` trigger is `tx.send` failing, which requires
   `resp_rx_by_stream[sid]` to be dropped — and nothing drops it on a
   client response-stream cancel.

## Suggested fix area (for the builder — verifier does NOT patch;
author ≠ verifier)

The actor must observe a client STOP_SENDING / RESET on the **response**
stream and, on detecting it, remove `resp_rx_by_stream[sid]` (closing
the channel ⇒ producer's next `tx.send().await` ⇒
`Err(RespAbort::ClientGone)` ⇒ `h3_to_h1_stream_resp` marks the pooled
conn non-reusable and returns, stopping the upstream read) and drop the
`StreamTx`. Candidates: handle `Err(StreamStopped/StreamReset)` in the
`Progressive` send branch (`conn_actor.rs:519`) explicitly (set a
teardown that removes the receiver), AND add a response-stream
STOP_SENDING/RESET poll for streams that have a `resp_rx_by_stream`
entry but an empty queue (so the queue-empty case is also covered) —
mirroring the S2 request-side `StreamReset|StreamStopped` arms at
`conn_actor.rs:861/944`. Re-verify with R6 + C2/ClientGone after the
fix (author ≠ verifier).

## Tests are CORRECT and stay (not weakened / not ignored)

R6 and C2/ClientGone assert the real teardown (backend read-half
closes; pooled upstream not parked). They are the regression lock for
this defect and must stay failing until the product is fixed.
