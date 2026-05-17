# Session 3 — Phase 1 design: incremental, bounded, backpressured H3 response egress

Status: PROPOSED — awaiting plan approval. No source changed yet.
Phase 0 baseline must be green before any code lands.

## 0. What ships today (the flaw)

`poll_h3` spawns `h3_to_h1_stream` → `JoinHandle<(u64, Vec<u8>)>`.
`h3_to_h1_stream` calls `read_h1_response_capped` which **reads the
whole upstream response to EOF into one `Vec`**, then
`encode_h3_response` builds one HEADERS + one DATA frame. Only then
does `run_actor` insert a `StreamTx` and `drain_streams_to_conn`
dribble it into quiche. Peak proxy memory for one response = total
response size. This is the exact mirror of the request-side flaw S2
fixed. (Confirmed in
`audit/h3-program/p1c-response-streaming-assessment.md`.)

## 1. Design — a backpressured byte-pipe (mirrors S2, inverted)

The S2 request machine: producer = `poll_h3`, consumer = spawned
task, bounded `mpsc<ReqBodyEvent>`, backpressure when the channel is
full stops `stream_recv`. The response machine is the **inverse**:
producer = spawned task reading the H1 upstream, consumer = the
actor feeding quiche, bounded `mpsc<RespEvent>`, backpressure when
the channel is full stops the upstream socket read.

### 1.1 `RespEvent` — pre-encoded H3 wire bytes, not decoded body

```rust
pub enum RespEvent {
    /// Pre-encoded H3 wire bytes (HEADERS frame, or a DATA frame, or
    /// a trailing HEADERS frame) to stream_send to the client as-is.
    Bytes(Bytes),
    /// All response bytes delivered — set FIN on the client stream.
    End,
    /// Abort: RESET_STREAM to the client, never FIN. Used for
    /// upstream reset / premature EOF / decode error / client cancel.
    Reset,
}
```

**Why pre-encoded bytes, not decoded body chunks:** keeps `StreamTx`
a simple byte queue and the H1 producer solely responsible for H3
framing (HEADERS / DATA / trailing-HEADERS), so the actor-side drain
is uniform and the progressive `StreamTx` variant shares
`drain_streams_to_conn` with the legacy `StreamTx::new(Vec)` byte-for
-byte. (H2/H3 do **not** use this channel — see §1.4; they remain on
the unchanged legacy path. The byte-pipe shape simply means a later
session can repoint them onto it without reshaping it.)

### 1.2 Bounded constants (memory mechanism, body-size independent)

```rust
pub const H3_RESP_CHANNEL_DEPTH: usize = 8;          // mirrors H3_BODY_CHANNEL_DEPTH
pub const H3_RESP_CHUNK_MAX: usize    = 8 * 1024;    // mirrors H3_BODY_CHUNK_MAX
```

Max in-flight response bytes the proxy retains ≈
`H3_RESP_CHANNEL_DEPTH * H3_RESP_CHUNK_MAX` (channel) + ≤ one chunk
in `StreamTx` + quiche's own flow-control-bounded send buffer.
**Independent of total response size.** The 64 MiB
`MAX_RESPONSE_BODY_BYTES` stays as a *separate total-size ceiling*,
explicitly NOT the memory-safety mechanism.

### 1.3 Incremental `stream_h1_response` (new, replaces buffered read)

New `async fn stream_h1_response(stream, tx: &mpsc::Sender<RespEvent>,
cap) -> Result<(), RespAbort>`:

1. Read into a small buffer **only until** the `\r\n\r\n` header
   terminator (bounded by a header cap; reuse a 64 KiB head cap).
   Parse status line + headers.
2. Decide body framing from the parsed headers:
   - **Content-Length: N** — emit `Head`, then read & emit ≤8 KiB
     DATA frames until exactly N body bytes; premature socket EOF
     before N ⇒ `Reset`.
   - **Transfer-Encoding: chunked** — incremental de-chunk state
     machine (net-new; production currently does not parse chunked
     *responses*), emit decoded payload as ≤8 KiB DATA frames;
     decode error or EOF before terminator ⇒ `Reset`.
   - **EOF-delimited** (no CL, no TE) — emit DATA frames until socket
     EOF, then `End`.
3. **Head HEADERS frame content — the no-regression contract:** when
   the upstream supplied `Content-Length`, the emitted H3 HEADERS
   frame carries `:status` + `content-length: N` **byte-identical to
   today's `encode_h3_response` output** (every existing e2e backend
   sends Content-Length, and the existing test client returns only on
   `content-length`-satisfied — so this is load-bearing for
   no-regression). When length is unknown (chunked/EOF) emit
   `:status` only and rely on stream FIN; new tests use a FIN-aware
   client driver.
4. Body emitted *as it arrives from the socket* — the first DATA
   frame is sent before the rest of the body is read. Backpressure:
   `tx.send(...).await` blocks when the bounded channel is full ⇒ the
   upstream `read()` is not called ⇒ TCP backpressure to the
   upstream. End-to-end: slow H3 client → quiche send window full →
   actor stops draining the channel → channel fills → upstream read
   pauses.
5. Error/teardown parity with P1-B: upstream reset, premature EOF
   before Content-Length satisfied, chunked-decode error, and the
   over-`cap` ceiling each ⇒ `RespEvent::Reset` (RESET_STREAM to the
   client) — **never a truncated body presented as a complete
   response** (response-splitting / cache-poisoning guard).

`encode_h3_response` is refactored into reusable
`encode_h3_headers_frame(status, content_length: Option<usize>)` +
`encode_h3_data_frame(&[u8])`; the existing `encode_h3_response`
becomes `headers(Some(len)) + data(body)` so its output and all its
callers/tests are byte-identical.

### 1.4 Actor changes (`conn_actor.rs`)

**H2/H3 LEFT UNCHANGED (program-owner-mandated clarification).** No
net-new buffering layer / shim is added for H2/H3. The legacy path —
`request_tasks: Vec<JoinHandle<(u64, Vec<u8>)>>`, the `task_wait`
select arm, `StreamTx::new(Vec<u8>)`, `drain_streams_to_conn` — is
**kept exactly as-is** and continues to serve H2/H3 *and* the inline
error responses (400 authority reject etc.). Not a line of H2/H3
code changes. H2/H3 already buffer a full `Vec<u8>` today (their own
pre-existing behaviour); S3 adds nothing to it. A later session
converts H2/H3 by repointing their spawn onto the channel machinery
S3 builds for H1 and deleting the pre-existing legacy `Vec` path —
removing a path that existed *before* S3, not a layer S3 added.

- Add `resp_rx_by_stream: HashMap<u64, mpsc::Receiver<RespEvent>>`
  (actor owns receivers) used **only by the H1 spawn site**, drained
  alongside the retained legacy `request_tasks`/`task_wait` arm.
- `StreamTx` gains an **additional** progressive variant used solely
  by H1 (`queue: VecDeque<Bytes>`, `ended`, `reset`, `fin_sent`); the
  existing `StreamTx::new(Vec<u8>)` constructor + its drain stay
  byte-identical so H2/H3 + inline errors are bit-for-bit unaffected.
  `drain_streams_to_conn` handles both variants; H1 streaming: drain
  queue; on `ended` && queue empty send FIN; on `reset` call
  `conn.stream_shutdown(sid, Shutdown::Write, code)`.
- After `select!`, drain each receiver into its `StreamTx` **only
  when that StreamTx's queue is empty** (the backpressure gate,
  mirroring the request-side `pending_empty` gate). This bounds
  proxy-retained bytes to ≈ channel depth and creates the stall that
  propagates to the upstream.
- `select!` wakes promptly: cap `next_wait` to 2 ms while any
  response channel is active (identical pattern to the existing
  request-body 2 ms tick — consistent with S2, already accepted).
- Spawn sites: **only** H1 changes → `stream_h1_response` producer
  task with `resp_tx`. H2/H3 + inline error spawns: untouched, still
  `request_tasks.push(...)` → `task_wait` → `StreamTx::new(Vec)`.

### 1.5 Non-vacuous memory proof gauge

Add `MAX_RETAINED_RESP_BYTES` atomic + `record_resp_retained`
(`cfg(any(test, feature="test-gauges"))`), recorded in the actor at
the largest point (after draining receivers into `StreamTx`, before
`drain_streams_to_conn`): Σ over streams of `StreamTx` queued bytes +
channel occupancy upper bound (`used_slots * H3_RESP_CHUNK_MAX`).
Mirrors `MAX_RETAINED_BODY_BYTES` exactly.

## 2. Tests (real listener/router/bridge path, binary bodies)

New `crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs`:

- **R1** multi-DATA binary response (≥100 KB, 0xFF/0x00/0x80 markers
  at head/mid/tail) byte-identical at the H3 client.
- **R2** non-vacuous memory bound: 1 MiB response, slow/stalled H3
  client; assert `MAX_RETAINED_RESP_BYTES ≤ 4 * (depth*chunk)` and
  `≪ 1 MiB` (mirrors S2 T5; proves RSS does not grow with response
  size), plus liveness + byte-identity after the client resumes.
- **R3** slow-client backpressure: client reads slowly; assert the
  upstream read pauses (gauge stays bounded, no unbounded buffering)
  and the request still completes correctly.
- **R4** empty response body + zero-length DATA; byte-identical,
  clean FIN.
- **R5** upstream resets mid-response ⇒ client observes RESET_STREAM,
  no truncated body presented as complete.
- **R6** client cancels mid-response ⇒ proxy stops reading upstream,
  per-stream state torn down, no leak.
- **R7** chunked upstream response, byte-identical (new decoder).
- **R8** trailers after response DATA — PROTO-2-12 no-regression:
  the existing `h3_h1_trailers_resp_e2e.rs` PC1/PC2 and the
  H3-upstream `request_h3_upstream` trailer path stay green
  unchanged (H2/H3 shim is byte-on-wire identical).

Every test asserts the real decoded outcome (status + byte-identical
binary body), never a deadline-as-pass.

## 3. Honesty statement

This is bounded-incremental from the first byte: `Head` is emitted as
soon as headers parse; DATA frames are emitted as the socket yields
them, before the full body exists; `tx.send().await` is the
backpressure point. It is **not** "accumulate then send". The 64 MiB
cap remains a separate ceiling, not the memory mechanism.

## 4. Team & process

- Plan approval (this doc) before any source change.
- builder-A: §1.3 + §1.4 bridge/actor rework + R1–R4,R7.
- builder-B: §1.5 gauge + R5,R6,R8 + P1-B-parity teardown review.
- verifier: independent from-scratch verification; author ≠ verifier
  for every fix. Never weaken/ignore/delete a working test.
- Commit every increment; push to origin/feature/h3-quic-s3.
