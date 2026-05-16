# P1-C — Incremental Response Egress: Honest Assessment

Status: **investigation only (≤20 min, bounded)**. No rework done this
increment. This document is the deliverable for P1-C item 3.

## 1. Current response path (what ships today)

```
poll_h3
  └─ tokio::spawn(h3_to_h1_stream(...)) -> JoinHandle<(u64, Vec<u8>)>
        ├─ writes request head + streams request body INCREMENTALLY (P1-A/B: good)
        ├─ read_h1_response(stream)            <-- reads WHOLE response to EOF
        │     └─ accumulates into one `Vec<u8>` `all` (now capped, P1-C)
        └─ encode_h3_response(status, &body)   <-- ONE HEADERS + ONE DATA frame
run_actor select!
  └─ task_wait completes -> stream_response.insert(sid, StreamTx::new(bytes))
drain_streams_to_conn
  └─ StreamTx feeds the single Vec into quiche stream_send incrementally
     (this is incremental on the WIRE, but the whole body is already
      resident in memory before the first byte is sent)
```

**This is FULLY BUFFERED.** Peak memory for one in-flight response is
bounded by the *total response size*, NOT by an in-flight window. It is
the exact mirror of the request-side flaw the program owner rejected
and which P1-A fixed for the request direction. `StreamTx` is
incremental egress *into quiche*, but it cannot start until the entire
`Vec<u8>` exists, so it provides zero memory bound.

P1-C added `MAX_RESPONSE_BODY_BYTES = 64 MiB` (mirrors
`MAX_REQUEST_BODY_BYTES`) as a hard ceiling in `read_h1_response_capped`
so a hostile/mis-configured upstream cannot OOM the proxy. This is a
**safety ceiling, not incremental egress** and not backpressure.

## 2. What genuine incremental response egress requires

To make the response direction memory-bounded the way P1-A made the
request direction:

1. **Replace `JoinHandle<(u64, Vec<u8>)>` with a per-stream channel
   back into the actor.** The spawned task would send
   `RespEvent::{Head{status,headers}, Chunk(Bytes<=N>), End, Reset}`
   over a bounded `mpsc` (depth ≈ `H3_BODY_CHANNEL_DEPTH`). The actor
   owns the receiver; `run_actor`'s `select!` drains it the same way
   `poll_h3`/`flush_pending` drains the request-body pending queue.

2. **Incremental `read_h1_response`.** Today it reads to EOF then
   parses. It must instead:
   - parse the status line + headers as soon as the header terminator
     (`\r\n\r\n`) is in the buffer;
   - then stream the body per framing:
     - **Content-Length**: emit ≤N-byte chunks until CL bytes read;
     - **Transfer-Encoding: chunked**: incremental de-chunk (the test
       harness already has a `dechunk`, but production currently does
       NOT parse chunked responses at all — `read_h1_response`'s
       doc-comment explicitly scopes chunked OUT);
     - **EOF-delimited** (no CL, no TE, `Connection: close`): emit
       chunks until socket EOF.
   This is a new response body state machine roughly mirroring
   `StreamRxBuf::feed_body` on the request side.

3. **Progressive `StreamTx`.** `StreamTx` already sends incrementally;
   it would be fed by appended chunks (HEADERS frame first, then DATA
   frames as `RespEvent::Chunk` arrive, FIN on `End`) instead of one
   pre-built `Vec`. Backpressure: if quiche's stream send window is
   saturated `StreamTx` stalls → the bounded response channel fills →
   the egress task stops reading the upstream socket → genuine
   end-to-end backpressure (the request-side mechanism, inverted).

4. **Error/teardown parity with P1-B.** Mid-response upstream reset,
   premature EOF before Content-Length satisfied, chunked-decode
   error, and the over-cap ceiling must all map to a clean H3 response
   / `RESET_STREAM` *without* presenting a truncated body as complete
   (response-splitting / cache-poisoning analogue of P1-B).

## 3. S1 / prior-green regression surface (risk)

This is a **large, central rework**, not an isolated add:

- `run_actor`'s `select!` loop, `request_tasks: Vec<JoinHandle<(u64,
  Vec<u8>)>>`, the `task_wait` future, and `stream_response: HashMap<u64,
  StreamTx>` all change shape. Every actor test depends on this:
  S1 e2e `h3_h1_bridge_e2e`, Phase 0 `h3_h1_binary_body_e2e`, P1-A
  `h3_h1_stream_body_e2e` (T1–T5), P1-B `h3_h1_stream_body_errors_e2e`,
  `listener_lifecycle`, `quic_router_leak`, `round8_h3_authority_*`,
  `h3_graceful_close`.
- The H3→H2 and H3→H3 upstream branches (`h3_to_h2_roundtrip`,
  `h3_to_h3_roundtrip`) also return `Vec<u8>` through the same
  `request_tasks` machinery and the PROTO-2-12 response-trailer path
  (`request_h3_upstream` / `H3UpstreamResponse`) — they would need the
  same channelization or a compatibility shim, widening blast radius
  to the lb-l7 / repo-root bridging + trailer suites (which are NOT
  buildable under the `-p lb-quic` disk constraint, so a regression
  there would not even be caught locally this session).
- Production currently does not parse chunked **responses** at all;
  adding a chunked-response decoder is net-new parsing surface with
  its own smuggling/edge-case test burden.
- The graceful-shutdown path (`graceful_h3_shutdown`) interacts with
  outstanding `StreamTx` state; partial progressive sends during
  drain need re-reasoning (PROTO-2-11).

## 4. Does it fit the remaining Session 2 budget? — Honest call

**No.** Recommendation: **defer to Session 3 as the headline item.**

Reasoning:
- It is the structural twin of the P1-A request rework, which itself
  consumed a full increment with its own multi-test e2e proof.
- It touches the actor's core `select!`/task model that *every* green
  actor test depends on, plus the H2/H3 upstream branches and the
  not-locally-buildable lb-l7/repo-root trailer + bridging suites —
  high regression blast radius, low local verifiability this session.
- It needs a brand-new incremental response body state machine
  (CL / chunked / EOF-delimited) **and** P1-B-parity error/teardown
  paths, each requiring real-decoded-outcome e2e tests.
- Attempting it inside the remaining budget would force either a risky
  half-rewrite (explicitly prohibited) or buffering-disguised-as-
  streaming (dishonest, prohibited).

The bounded, honest, in-budget P1-C deliverable is therefore:
1. trailers no-regression + documented intentional-drop (done);
2. large-binary response correctness lock-in (done, `pc2`);
3. the `MAX_RESPONSE_BODY_BYTES` OOM ceiling + its unit test (done —
   low-risk: sole production caller passes the const unchanged, every
   existing response body ≤256 KiB ≪ 64 MiB, so zero behavioural
   change for conformant responses; see §5).

## 5. Safety-cap decision — ADDED (not deferred)

`MAX_RESPONSE_BODY_BYTES = 64 * 1024 * 1024` was **added**.

- Placement: `read_h1_response` delegates to a new
  `read_h1_response_capped(stream, cap)`; `read_h1_response` is the
  **sole production caller** and passes the named const, so the
  production wire/parse behaviour is byte-for-byte unchanged.
- Why it does NOT risk any existing test: every existing test's
  response body is ≤ 256 KiB (`pc2` is the largest at 256 KiB + 777 B),
  which is ≪ the 64 MiB ceiling, so the cap branch is never taken for
  any conformant response in the suite. The refactor is a pure
  extraction (no logic change on the in-budget path).
- Test: `read_h1_response_capped_rejects_over_cap_and_passes_under_cap`
  drives the REAL function against a real localhost TCP backend with a
  tiny cap and asserts BOTH the over-cap `Err` (real decoded outcome,
  Err message names the cap) AND the under-cap parse (status + binary
  0xFF/0x00/0x80 body) — no deadline-as-pass.
- `MAX_RESPONSE_BODY_BYTES` carries `// TODO(s3): config + incremental
  egress`.

## 6. Session 3 headline recommendation

**Incremental, memory-bounded, backpressured H3 response egress** —
channel-back-into-actor + incremental `read_h1_response`
(CL/chunked/EOF) + progressive `StreamTx`, with P1-B-parity
mid-response error/teardown, replacing the buffer-and-cap shipped here.
