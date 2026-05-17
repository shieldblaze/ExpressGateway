# Session 4 — Phase 1 build plan: incremental, bounded, backpressured H3 RESPONSE egress

Status: **DRAFT — awaiting team-lead plan approval. No source changed yet.**
Phase 0 baseline (task #1, lead-owned) must be green before any code lands.

Branch: `feature/h3-quic-s4` @ base `9d2bd9ca` (audit/foundation-pass tip).
This plan executes the APPROVED design in
`audit/h3-program/s3-phase1-plan.md`; the why/what is
`audit/h3-program/p1c-response-streaming-assessment.md`. It re-confirms
every named file/function against the foundation-pass code and states,
per increment, exactly how R8 (no buffering trap) is satisfied.

## 0. Design-vs-code reconfirmation (foundation-pass @9d2bd9ca)

S3 did not touch the response path; the foundation audit did not reshape
it either. Verified line-anchored locations the increments edit:

| Symbol | File:line | Role today |
|---|---|---|
| `run_actor` | `crates/lb-quic/src/conn_actor.rs:115` | actor loop |
| `stream_response: HashMap<u64, StreamTx>` | `conn_actor.rs:118` | per-stream egress cursors |
| `request_tasks: Vec<JoinHandle<(u64, Vec<u8>)>>` | `conn_actor.rs:134` | legacy buffered-Vec jobs |
| `next_wait` + existing 2 ms body tick | `conn_actor.rs:146-158` | wake cadence |
| `task_wait` | `conn_actor.rs:163-186` | first-finished JoinHandle await |
| `stream_response.insert(StreamTx::new(response_bytes))` | `conn_actor.rs:206` | where the buffered Vec becomes a StreamTx |
| `struct StreamTx { bytes, sent, finished }` / `StreamTx::new` | `conn_actor.rs:235-249` | byte cursor |
| `drain_streams_to_conn` | `conn_actor.rs:254-297` | pump StreamTx → quiche |
| `poll_h3` | `conn_actor.rs:381` | readable-stream pump / spawn site |
| **H1 spawn site** (`h3_to_h1_stream`) | `conn_actor.rs:494-512` | the ONLY spawn this plan rewires |
| inline-400 / `h3_to_h2_roundtrip` / `h3_to_h3_roundtrip` spawns | `conn_actor.rs:457 / 466 / 477` | **UNCHANGED** — stay on legacy Vec path |
| `record_retained_for_stream` (req-side gauge to mirror) | `conn_actor.rs:740-765` | pattern for §1.5 |
| `read_h1_response_capped` | `crates/lb-quic/src/h3_bridge.rs:659-704` | reads WHOLE response to EOF (the flaw) |
| `encode_h3_response` | `h3_bridge.rs:733-752` | one HEADERS + one DATA frame |
| `h3_to_h1_stream` | `h3_bridge.rs:944-1112` | incremental request body, **buffered response** at :1102-1111 |
| `find_header_sep` / `parse_status_line` | `h3_bridge.rs:706-716` | reusable head parse helpers |
| `MAX_RESPONSE_BODY_BYTES = 64 MiB` | `h3_bridge.rs:63` | SEPARATE total ceiling (NOT the memory mechanism) |
| `MAX_RETAINED_BODY_BYTES` / `record_retained` | `h3_bridge.rs:483 / 491` | req-side gauge to mirror for §1.5 |
| req-side const mirrors | `h3_bridge.rs:48 (DEPTH), :69 (CHUNK_MAX)` | values to mirror in §1.2 |

Confirmed: the response is fully buffered at `h3_to_h1_stream`
:1102-1111 (`read_h1_response(stream)` → `encode_h3_response(status,
&body)` → returned as one `Vec<u8>` through `request_tasks` →
`StreamTx::new` at `conn_actor.rs:206`). Peak proxy memory per response
== total response size. The design in s3-phase1-plan still applies
unchanged.

## Increment map

- **P1-A (task #3, builder-1)** — §1.1 `RespEvent`, §1.2 bounded
  constants, §1.3 incremental `stream_h1_response` + `encode_h3_response`
  refactor. Pure producer-side; no actor wiring yet (compiles, legacy
  path still live, no behaviour change until P1-B flips the spawn).
- **P1-B (task #4, builder-2)** — §1.4 actor wiring: `resp_rx_by_stream`,
  progressive `StreamTx` variant + drain, backpressure gate, `next_wait`
  cap, **H1-only** spawn-site rewire; error/teardown parity.
- **P1-C (task #5)** — trailing-HEADERS-after-DATA emission + the
  trailers/large-binary no-regression lock (PROTO-2-12).
- **Tests (task #6, builder-1)** — `h3_h1_resp_stream_e2e.rs` R1..R8 +
  §1.5 gauge wiring, real-wire, real decoded outcomes.

Each increment: commit + push to `origin/feature/h3-quic-s4`. Author ≠
verifier — builder never verifies own increment (R5).

## 1.1 `RespEvent` — pre-encoded H3 wire bytes (P1-A, builder-1)

New, in `crates/lb-quic/src/h3_bridge.rs` (alongside `ReqBodyEvent`):

```rust
/// SESSION 4 / P1-A: one unit of the bounded response byte-pipe from
/// the H1-upstream reader task back to the actor. Pre-encoded H3 wire
/// bytes so the actor-side drain is a uniform byte queue (the H1
/// producer owns ALL H3 framing: HEADERS / DATA / trailing-HEADERS).
pub enum RespEvent {
    /// Pre-encoded H3 wire bytes to `stream_send` to the client as-is.
    Bytes(bytes::Bytes),
    /// All response bytes delivered — set FIN on the client stream.
    End,
    /// Abort: RESET_STREAM to the client, never FIN (upstream reset /
    /// premature EOF / decode error / over-cap / client cancel).
    Reset,
}
```

`Bytes` carries `bytes::Bytes` — the streaming hot-path buffer type
mandated by R8/the Bytes optimization. H2/H3/inline-error responses do
NOT use this channel (they remain on the legacy `request_tasks` Vec
path); the byte-pipe shape only means a later session can repoint them
without reshaping it.

## 1.2 Bounded constants — the memory mechanism (P1-A, builder-1)

New in `h3_bridge.rs`, adjacent to `H3_BODY_CHANNEL_DEPTH`
(`:48`) / `H3_BODY_CHUNK_MAX` (`:69`), values mirrored exactly:

```rust
pub const H3_RESP_CHANNEL_DEPTH: usize = 8;        // mirrors H3_BODY_CHANNEL_DEPTH
pub const H3_RESP_CHUNK_MAX: usize    = 8 * 1024;  // mirrors H3_BODY_CHUNK_MAX
```

Max in-flight response bytes the proxy retains ≈
`H3_RESP_CHANNEL_DEPTH * H3_RESP_CHUNK_MAX` (channel) + ≤ one chunk in
the progressive `StreamTx` + quiche's own flow-control-bounded send
buffer. **Independent of total response size.** `MAX_RESPONSE_BODY_BYTES`
(`h3_bridge.rs:63`, 64 MiB) stays a SEPARATE total-size ceiling,
explicitly NOT the memory-safety mechanism — its existing unit test
(`read_h1_response_capped_rejects_over_cap_and_passes_under_cap`,
`h3_bridge.rs:1714`) stays green unchanged.

## 1.3 Incremental `stream_h1_response` + `encode_h3_response` refactor (P1-A, builder-1)

### 1.3.1 `encode_h3_response` refactor — byte-identical extraction

Refactor `h3_bridge.rs:733-752` into two reusable helpers; the existing
public `encode_h3_response` becomes a thin composition so its output and
every caller/test (`encode_h3_response_includes_status_and_body`
`h3_bridge.rs:1614`; `h3_to_h1_roundtrip`; the inline 400/502/413 sites;
H2/H3 round-trips that call it) are **byte-for-byte unchanged**:

```rust
pub fn encode_h3_headers_frame(status: u16, content_length: Option<usize>) -> Result<Vec<u8>, String>;
pub fn encode_h3_data_frame(payload: &[u8]) -> Result<Vec<u8>, String>;

pub fn encode_h3_response(status: u16, body: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = encode_h3_headers_frame(status, Some(body.len()))?;
    out.extend_from_slice(&encode_h3_data_frame(body)?);
    Ok(out)
}
```

`encode_h3_headers_frame(status, Some(len))` MUST emit the exact same
QPACK header list (`:status`, `content-length: N`) and frame bytes as
today (verified by the unchanged `encode_h3_response_*` unit test +
every existing e2e). `content_length: None` ⇒ `:status` only (for
chunked / EOF-delimited, length unknown).

### 1.3.2 `stream_h1_response` — incremental, bounded, backpressured

New `async fn stream_h1_response(stream: &mut TcpStream, tx:
&mpsc::Sender<RespEvent>, cap: usize) -> Result<(), RespAbort>` in
`h3_bridge.rs` (replaces the buffered `read_h1_response` +
`encode_h3_response` tail at `h3_to_h1_stream:1102-1111` once P1-B
rewires; in P1-A it is added but not yet wired):

1. Read into a small buffer **only until** `\r\n\r\n`
   (`find_header_sep`, `h3_bridge.rs:706`), bounded by a 64 KiB head cap.
   Parse status line (`parse_status_line`, `:710`) + headers (reuse the
   existing lowercase-key loop from `read_h1_response_capped:693-698`).
2. **Emit the HEADERS `RespEvent::Bytes` immediately** — before any body
   byte is read. Framing decided from parsed headers:
   - **`Content-Length: N`** — `encode_h3_headers_frame(status,
     Some(N))` (byte-identical to today's HEADERS for every existing
     CL backend — the no-regression contract, see §1.3.3); then read &
     emit ≤`H3_RESP_CHUNK_MAX` `encode_h3_data_frame` chunks until
     exactly N body bytes; socket EOF before N ⇒ `Reset`.
   - **`Transfer-Encoding: chunked`** — `encode_h3_headers_frame(status,
     None)`; net-new incremental de-chunk state machine (production
     does not parse chunked *responses* today), emit decoded payload as
     ≤8 KiB DATA frames; decode error / EOF before terminator ⇒ `Reset`.
   - **EOF-delimited** (no CL, no TE) — `encode_h3_headers_frame(status,
     None)`; emit DATA frames until socket EOF, then `End`.
3. Body emitted **as it arrives from the socket** — the first DATA
   `RespEvent::Bytes` is sent before the rest of the body is read.
   Backpressure: `tx.send(ev).await` blocks when the bounded channel is
   full ⇒ the upstream `stream.read()` is not called ⇒ TCP backpressure
   to the upstream. End-to-end: slow H3 client → quiche send window
   full → actor stops draining the channel (§1.4 gate) → channel fills
   → upstream read pauses.
4. On clean completion emit `RespEvent::End`. Any of: upstream reset,
   premature EOF before `Content-Length` satisfied, chunked-decode
   error, over-`cap` (total > `MAX_RESPONSE_BODY_BYTES`) ⇒
   `RespEvent::Reset` — **never a truncated body presented as a complete
   response** (response-splitting / cache-poisoning guard; parity with
   the request-side P1-B abort at `h3_to_h1_stream:1057-1092`).
   `RespAbort` is the internal error type the actor maps to `Reset`.

### 1.3.3 No-regression contract (load-bearing)

Every existing e2e backend sends `Content-Length`, and the existing
test clients return on `content-length`-satisfied. Therefore for the CL
path the emitted HEADERS frame MUST be byte-identical to today's
`encode_h3_response` HEADERS (`:status` + `content-length: N`), and the
concatenation HEADERS+DATA(+...)+FIN must be wire-equivalent to today's
single HEADERS+DATA+FIN. The chunked / EOF paths (length unknown, rely
on FIN) are exercised only by NEW FIN-aware test clients.

### How P1-A satisfies R8

`encode_h3_headers_frame` is emitted as soon as headers parse; DATA
`RespEvent::Bytes` are emitted as the socket yields them, **before the
full body exists**; `tx.send().await` on the bounded channel is the
backpressure point. Memory is bounded by `H3_RESP_CHANNEL_DEPTH *
H3_RESP_CHUNK_MAX`, **not** by total response size and **not** by the
64 MiB cap. This is bounded-incremental from the first byte — explicitly
NOT "accumulate full response then send" (the rejected buffering trap).
`bytes::Bytes` is used for the hot-path frame buffers.

## 1.4 Actor wiring (P1-B, builder-2 — placeholder, to be integrated)

> builder-2 owns this section per s3-phase1-plan §1.4: `resp_rx_by_stream:
> HashMap<u64, mpsc::Receiver<RespEvent>>` (actor-owned, H1 spawn site
> only); the **additional** progressive `StreamTx` variant
> (`queue: VecDeque<Bytes>`, `ended`, `reset`, `fin_sent`) with the
> existing `StreamTx::new(Vec<u8>)` constructor + its drain kept
> byte-identical so H2/H3 + inline errors are bit-for-bit unaffected;
> `drain_streams_to_conn` handling both variants (H1: drain queue; on
> `ended` && empty → FIN; on `reset` → `stream_shutdown(Write)`);
> the post-`select!` backpressure gate (drain a receiver into its
> `StreamTx` **only when that StreamTx's queue is empty** — mirrors the
> request-side `pending_empty` gate, `conn_actor.rs:401-404`); `next_wait`
> ≤ 2 ms while any response channel is active (same pattern as the
> existing body tick `conn_actor.rs:156-158`); rewire **only** the H1
> spawn at `conn_actor.rs:494-512` to a `stream_h1_response` producer
> task with `resp_tx`. inline-400 / `h3_to_h2_roundtrip` /
> `h3_to_h3_roundtrip` spawns (`conn_actor.rs:457/466/477`) and the
> `request_tasks`/`task_wait`/`StreamTx::new(Vec)` legacy path stay
> EXACTLY as-is. builder-2 to supply final §1.4 text + how it satisfies
> R8 (the gate is the backpressure/memory bound); I integrate before
> requesting approval.

## 1.5 Non-vacuous memory gauge (P1-B, builder-2 — placeholder)

> Mirrors `MAX_RETAINED_BODY_BYTES` / `record_retained` (`h3_bridge.rs:483
> / 491`) and `record_retained_for_stream` (`conn_actor.rs:740-765`):
> add `MAX_RETAINED_RESP_BYTES` atomic + `record_resp_retained`
> (`cfg(any(test, feature="test-gauges"))`), recorded in the actor at
> the largest point (after draining receivers into `StreamTx`, before
> `drain_streams_to_conn`): Σ over streams of progressive-`StreamTx`
> queued bytes + channel occupancy upper bound (`used_slots *
> H3_RESP_CHUNK_MAX`). builder-2 to supply final text.

## 2. Tests (task #6, builder-1) — real listener/router/bridge, binary bodies

New `crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs`, harness modelled on
`h3_h1_stream_body_e2e.rs` (`spawn_backend` :241, `drive_h3*` :311,
`start_listener` :487) but with a FIN-aware response client driver:

- **R1** multi-DATA binary response (≥100 KB; 0xFF/0x00/0x80 markers at
  head/mid/tail) byte-identical at the H3 client.
- **R2** non-vacuous memory bound: 1 MiB response, slow/stalled H3
  client; assert `MAX_RETAINED_RESP_BYTES ≤ 4 * (depth*chunk)` and
  `≪ 1 MiB`; plus liveness + byte-identity after resume (mirrors S2 T5,
  `h3_h1_stream_body_e2e.rs:791`).
- **R3** slow-client backpressure: upstream read provably pauses (gauge
  stays bounded), request still completes correctly.
- **R4** empty response body + zero-length DATA; byte-identical, clean
  FIN.
- **R5** upstream resets mid-response ⇒ client observes RESET_STREAM, no
  truncated body presented as complete.
- **R6** client cancels mid-response ⇒ proxy stops reading upstream,
  per-stream state torn down, no leak.
- **R7** chunked upstream response, byte-identical (new decoder).
- **R8** trailers no-regression: existing `h3_h1_trailers_resp_e2e.rs`
  `pc1` (`:478`) / `pc2` (`:573`) and the `request_h3_upstream`
  (`h3_bridge.rs:1171`) H3-upstream trailer path stay green UNCHANGED
  (H2/H3 wire-identical: they never touch the new channel). P1-C adds
  trailing-HEADERS-after-DATA emission on the H1 path.

Every test asserts the real decoded outcome (status + byte-identical
binary body); never a deadline-as-pass. No working test is ever
weakened / `#[ignore]`d / deleted.

## 3. Honesty statement

Bounded-incremental from the first byte: HEADERS emitted as soon as
headers parse; DATA frames emitted as the socket yields them, before the
full body exists; `tx.send().await` is the backpressure point; memory
bounded by channel depth × chunk, body-size independent. NOT "accumulate
then send". The 64 MiB cap remains a separate ceiling, not the memory
mechanism.

## 4. Team & process

- Plan approval (this doc, team-lead) REQUIRED before ANY source change.
- builder-1: §1.1-1.3 (P1-A, task #3) + tests (task #6, R1-R8).
- builder-2: §1.4 actor/bridge + §1.5 gauge (P1-B, task #4); supplies
  those sections of this plan.
- P1-C (task #5): trailing-HEADERS emission + PROTO-2-12 no-regression.
- verifier: independent from-scratch verification; author ≠ verifier for
  every increment/fix. Never weaken/ignore/delete a working test.
- Commit every increment; push to `origin/feature/h3-quic-s4`
  continuously (instance is ephemeral).
