# Session 4 — Phase 1 build plan: incremental, bounded, backpressured H3 RESPONSE egress

Status: **APPROVED (CONDITIONAL) by team-lead — 2026-05-17.** Phase 0
(task #1) is verifier-confirmed GREEN (R1, 3× deterministic, no
asterisk). R8 satisfied (bounded-incremental from first byte; memory
bound = `H3_RESP_CHANNEL_DEPTH × H3_RESP_CHUNK_MAX`, independent of
total body size and of the 64 MiB cap; end-to-end backpressure stall
chain explicit; `bytes::Bytes` hot path; H2/H3+inline byte-identical).
Source changes are now permitted, subject to the binding conditions
below.

### team-lead approval conditions (BINDING — R6; non-negotiable)

- **C1 (Q2 — confirms the builder-2 ruling already applied @28db0e60).**
  Grep proved NO reusable production H3 cancel/internal-error constant
  exists (only `H3_NO_ERROR=0x0100`, a graceful-drain code). Reusing
  `H3_NO_ERROR` on an abort is a correctness/security defect (signals
  clean completion → truncated-as-complete cache-poisoning, the exact
  §1.3.4 hazard). Ruling stands: new named const
  `H3_INTERNAL_ERROR: u64 = 0x0102` (RFC 9114 §8.1) beside
  `H3_NO_ERROR`, used for the `stream_shutdown(Write, …)` Reset path.
  The abort regression tests MUST assert the H3 client observes
  RESET_STREAM with a **non-`H3_NO_ERROR`** code on every abort path.
- **C2 (pooled-upstream smuggling guard — R6 security, parity with the
  S2 request side).** On EVERY `RespAbort` variant (UpstreamReset /
  PrematureEof / ChunkedDecode / OverCap / BadHead / ClientGone) the
  pooled upstream connection MUST be dropped / marked NON-reusable and
  never returned to the connection pool (a partially-consumed upstream
  response must not poison a future request). Add an explicit
  regression assertion mirroring the S2 request-side guard.
- **C3 (chunked-decoder negative/smuggling tests — R6).** The net-new
  chunked-response decoder MUST have negative tests beyond R7's happy-
  path byte-identity: malformed/non-hex chunk-size, missing CRLF,
  declared-size overflow, junk after the zero-size terminator — each
  ⇒ `RespAbort::ChunkedDecode` ⇒ `Reset`, NEVER a truncated/forwarded
  body presented as complete.
- **C4 (P1-C chunked trailers).** P1-C trailing-HEADERS-after-DATA
  handling MUST include the chunked trailer-section parse (trailer
  fields after the zero-size chunk) and keep PROTO-2-12
  (`h3_h1_trailers_resp_e2e.rs` pc1/pc2 + `request_h3_upstream`) green.

- **C5 (gauge soundness — R6/R8; builder-1's catch).** §1.5's
  channel-occupancy term must be `used_slots × (H3_RESP_CHUNK_MAX +
  H3_FRAME_HDR_MAX)`, NOT bare `× H3_RESP_CHUNK_MAX`. `RespEvent::Bytes`
  carries a pre-encoded frame (payload + frame-header varints); §1.2
  bounds it with `+ H3_FRAME_HDR_MAX`. A gauge that under-counts is an
  unsound memory proof (R8 non-vacuous requirement). R2's ceiling stays
  generous (`≤ 4 × DEPTH × (CHUNK_MAX + FRAME_HDR_MAX)`, `≪ 1 MiB`).

C2 binds P1-A signature/teardown (builder-1, §1.3.4) AND P1-B wiring
(builder-2). C3 binds P1-A (the decoder, builder-1) + tests. C1 binds
P1-B + the abort tests. C4 binds P1-C. C5 binds P1-B (builder-2's
§1.5 gauge). P1-A (task #3) may start immediately (it does not depend
on Q2). Conditions are enforced at each owning increment's independent
verification (author ≠ verifier).

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
/// A `RespEvent::Bytes` carries a PRE-ENCODED H3 frame (HEADERS or
/// DATA): a ≤`H3_RESP_CHUNK_MAX` payload PLUS a small frame header
/// (type + length varints). This upper-bounds that header so the §1.5
/// gauge OVER-estimates channel occupancy (never under — same
/// soundness rule as the request gauge, `h3_bridge.rs:757-759`).
pub const H3_FRAME_HDR_MAX: usize     = 16;
```

Max in-flight response bytes the proxy retains ≈
`H3_RESP_CHANNEL_DEPTH * (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX)`
(channel) + ≤ one chunk in the progressive `StreamTx` + quiche's own
flow-control-bounded send buffer. **Independent of total response
size.** `MAX_RESPONSE_BODY_BYTES`
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
4. On clean completion emit `RespEvent::End`. Every abort path ⇒
   `RespEvent::Reset` and **never FIN** — see §1.3.4.

### 1.3.4 `RespAbort` + the abort → `Reset` mapping (Q3 — builder-1 owns, FINAL)

team-lead assigned the `stream_h1_response` signature and the
`RespAbort` → `Reset` mapping to builder-1 (P1-A). Decided and FINAL:

```rust
/// SESSION 4 / P1-A: why the incremental response producer aborted.
/// EVERY variant maps to a single client-facing outcome: emit
/// `RespEvent::Reset` into the channel and return `Err(RespAbort)` —
/// the actor RESET_STREAMs the client and NEVER sets FIN. A partial
/// body is therefore never presentable as a complete response
/// (response-splitting / cache-poisoning guard; parity with the
/// request-side P1-B abort at `h3_to_h1_stream:1057-1092`).
#[derive(Debug)]
pub enum RespAbort {
    /// Upstream socket reset / error mid-response.
    UpstreamReset,
    /// Socket EOF before the declared `Content-Length` was satisfied.
    PrematureEof,
    /// `Transfer-Encoding: chunked` decode error / EOF before the
    /// chunked terminator.
    ChunkedDecode,
    /// Total response exceeded `cap` (`MAX_RESPONSE_BODY_BYTES`).
    OverCap,
    /// HEADERS parse failure / head cap exceeded before `\r\n\r\n`.
    BadHead,
    /// The response channel was closed by the actor (client cancelled
    /// the H3 stream) — stop reading the upstream.
    ClientGone,
}
```

Signature (FINAL):

```rust
async fn stream_h1_response(
    stream: &mut TcpStream,
    tx: &tokio::sync::mpsc::Sender<RespEvent>,
    cap: usize,
) -> Result<(), RespAbort>;
```

Mapping (exhaustive, no other outcome exists):

| Producer condition | `RespAbort` | Wire effect |
|---|---|---|
| upstream socket reset/error mid-response | `UpstreamReset` | `RespEvent::Reset`, never FIN |
| socket EOF before `Content-Length` bytes read | `PrematureEof` | `RespEvent::Reset`, never FIN |
| chunked decode error / EOF before terminator | `ChunkedDecode` | `RespEvent::Reset`, never FIN |
| total bytes > `cap` (= `MAX_RESPONSE_BODY_BYTES`) | `OverCap` | `RespEvent::Reset`, never FIN |
| no `\r\n\r\n` within the 64 KiB head cap / bad status line | `BadHead` | `RespEvent::Reset`, never FIN |
| `tx.send` returns `Err` (actor dropped the receiver = client cancel) | `ClientGone` | producer stops reading upstream; no further events |
| Content-Length satisfied / chunked terminator / EOF-delimited EOF | — (`Ok(())`) | final `RespEvent::End` ⇒ actor sets FIN |

The producer emits `RespEvent::Reset` into the channel *before*
returning `Err(RespAbort)` (best-effort: on `ClientGone` the channel is
already closed, so only the `Err` return applies — the actor already
knows). builder-2's §1.4 actor-side handling sets the progressive
`StreamTx` `reset` flag and `drain_streams_to_conn` issues
`conn.stream_shutdown(sid, Shutdown::Write, H3_INTERNAL_ERROR)` (Q2 —
`0x0102`, see §1.4); the Reset path **never** transitions to the FIN
branch.

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

## 1.4 Actor wiring (P1-B, builder-2)

Owner: builder-2 (P1-B, task #4). This is the approved s3-phase1-plan
§1.4 design, anchored to foundation-pass code (builder-2 independently
re-confirmed every anchor). The H2/H3 + inline-error legacy path is
**not touched**.

**1.4.1 Actor-owned response receivers.** Add to `run_actor`
(`conn_actor.rs:115`) alongside `stream_response` (`:118`):

```rust
let mut resp_rx_by_stream: HashMap<u64, mpsc::Receiver<RespEvent>> = HashMap::new();
```

The matching `mpsc::Sender<RespEvent>` (bounded depth
`H3_RESP_CHANNEL_DEPTH`) is moved into the spawned `stream_h1_response`
producer task; the receiver stays here. **Used by the H1 spawn site
only.**

**1.4.2 Progressive `StreamTx` variant.** `StreamTx`
(`conn_actor.rs:235-249`) gains an **additional** variant for H1
streaming; the existing legacy shape + `StreamTx::new(Vec<u8>)`
constructor + its drain branch in `drain_streams_to_conn`
(`:254-297`) stay **byte-for-byte identical** so H2/H3 + the inline
400/502/413 responses are bit-for-bit unaffected:

```rust
enum StreamTx {
    /// Legacy: one pre-built Vec, byte cursor + FIN-on-empty.
    /// UNCHANGED — serves H2/H3 + inline-error responses.
    Buffered { bytes: Vec<u8>, sent: usize, finished: bool },
    /// SESSION 4 / P1-B: progressive H1 response egress.
    Progressive { queue: VecDeque<bytes::Bytes>, ended: bool, reset: bool, fin_sent: bool },
}
```

`StreamTx::new(Vec<u8>)` continues to construct the `Buffered` variant
(legacy call site `conn_actor.rs:206` unchanged). `drain_streams_to_conn`
matches the variant: `Buffered` runs the existing loop verbatim;
`Progressive` drains `queue` front-to-back via `conn.stream_send(sid,
&chunk, false)` (re-queueing the unsent tail on `Done`/partial), and
then: if `reset` ⇒ `conn.stream_shutdown(sid, Shutdown::Write,
H3_INTERNAL_ERROR)` (Q2, `0x0102` — see below) and drop the stream,
**never FIN**; else if `ended` &&
`queue` empty ⇒ zero-length `stream_send(sid, &[], true)` to set FIN
(same FIN mechanism as the legacy branch).

**1.4.3 Backpressure gate (the memory bound).** After the `select!`
post-event block (near `poll_h3`, `conn_actor.rs:214-227`), for each
H1 stream drain its `resp_rx_by_stream` receiver into its
`Progressive` `StreamTx` **only while that `StreamTx.queue` is empty**
— pull at most enough to refill an empty queue, then stop. This
mirrors the request-side `pending_empty`/`flush_pending` gate
(`conn_actor.rs:401-404`, `:772-810`). `RespEvent::Bytes` → push to
`queue`; `RespEvent::End` → set `ended`; `RespEvent::Reset` (or
receiver closed with no `End`) → set `reset`. Because the gate refuses
to pull while the queue is non-empty, proxy-retained response bytes are
bounded to ≈ one queue's worth + the bounded channel; a stalled client
(quiche send window full ⇒ `Progressive` drain makes no progress ⇒
queue stays non-empty ⇒ gate pulls nothing ⇒ channel fills ⇒
`stream_h1_response`'s `tx.send().await` blocks ⇒ upstream socket read
pauses).

**1.4.4 Wake cadence.** Extend the existing 2 ms `next_wait` cap
(`conn_actor.rs:146-158`, today gated on
`!body_tx_by_stream.is_empty() || !body_pending.is_empty()`) to also
trip while `!resp_rx_by_stream.is_empty()`, so a backpressured response
resumes promptly. Identical pattern to the already-accepted S2
request-body tick; does not defeat backpressure (the gate + bounded
channel still cap in-flight bytes).

**1.4.5 Spawn-site rewire — H1 ONLY.** Replace **only** the H1 spawn at
`conn_actor.rs:494-512` (`h3_to_h1_stream` push): create
`mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH)`, insert the
receiver into `resp_rx_by_stream`, immediately insert a
`StreamTx::Progressive` (empty) into `stream_response`, and spawn the
producer task running `stream_h1_response(stream, &resp_tx,
MAX_RESPONSE_BODY_BYTES)` — the task acquires the pooled upstream,
writes the request head + streams the request body (the existing P1-A
request-side machinery in `h3_to_h1_stream` is reused: the response
half is what changes), then pipes the response via `resp_tx`. The
inline-400 (`:457`), `h3_to_h2_roundtrip` (`:466`), `h3_to_h3_roundtrip`
(`:477`) spawns and the entire `request_tasks` /
`task_wait` (`:163-186`) / `StreamTx::new(Vec)` (`:206`) legacy path
stay **exactly as-is**.

**Q2 — RESET_STREAM error code (RESOLVED — team-lead ruling, R6
correctness; builder-2 owns this paragraph):** the Reset/abort path
issues `conn.stream_shutdown(sid, Shutdown::Write, H3_INTERNAL_ERROR)`
with a new named constant **`H3_INTERNAL_ERROR: u64 = 0x0102`**
(RFC 9114 §8.1), added to the `conn_actor` module beside the existing
`pub const H3_NO_ERROR: u64 = 0x0100` (`conn_actor.rs:56`) and exported
from `lib.rs` alongside it.

Resolution record: builder-2's grep of all of `crates/lb-quic`
(src + tests) found **no reusable production cancel/internal-error
constant** — the only production H3 app-error const is the *graceful*
`H3_NO_ERROR = 0x0100` (`conn_actor.rs:56`, used by `conn.close` at
`:327`, regression-locked by `h3_graceful_close.rs`), and the only
`stream_shutdown(Write, code)` anywhere is a test-only literal `0x10`
in `h3_h1_stream_body_errors_e2e.rs:516` simulating a *client* request
reset (not a server response-abort code). The "reuse existing"
instruction presupposed a reusable code that does not exist; using the
RFC 9114 §8.1 *registered* codepoint is therefore the conformant
choice, not an invented value (team-lead ruling).

- **Reusing `H3_NO_ERROR` (0x0100) is REJECTED** as a correctness/
  security defect: it signals clean completion, so a client/cache
  seeing it on a RESET_STREAM could treat the partial body as a
  complete response — the exact truncated-as-complete / cache-poisoning
  hazard §1.4.6 exists to prevent.
- **`H3_INTERNAL_ERROR` (0x0102), not `H3_REQUEST_CANCELLED`
  (0x010C):** every abort cause on this path — upstream reset,
  premature EOF before `Content-Length`, chunked-decode error,
  over-`cap` ceiling — is a proxy/upstream-side failure to produce a
  faithful complete response, which is precisely `H3_INTERNAL_ERROR`'s
  semantics. `H3_REQUEST_CANCELLED` implies the *requester* cancelled,
  which is the distinct client-cancel path — there the proxy does
  **not** send RESET_STREAM, it stops reading the upstream and tears
  the stream down (§1.4.6). One abort code keeps §1.4.6 uniform.
- **Load-bearing test invariant:** on every abort path the H3 client
  observes RESET_STREAM with a **non-`H3_NO_ERROR`** code (asserted
  `== H3_INTERNAL_ERROR == 0x0102`) AND never a truncated body
  presented as complete. The Phase 1 test plan's R5 (upstream reset)
  and the over-cap / premature-EOF cases MUST explicitly assert the
  non-`H3_NO_ERROR` code (see §2).

### How P1-B satisfies R8

The §1.4.3 gate ("drain the receiver into the `StreamTx` **only when
its `queue` is empty**") is the memory bound: proxy-retained response
bytes ≈ one `VecDeque<bytes::Bytes>` queue (≤ a small number of
≤`H3_RESP_CHUNK_MAX` chunks) + the bounded `mpsc` channel
(`H3_RESP_CHANNEL_DEPTH * H3_RESP_CHUNK_MAX`) + quiche's own
flow-control-bounded send buffer — **independent of total response
size and of the 64 MiB cap**. The end-to-end stall chain: slow H3
client → quiche send window full → `Progressive` drain makes no
progress → `StreamTx.queue` stays non-empty → gate pulls nothing from
the receiver → channel fills → `stream_h1_response`'s `tx.send().await`
blocks → upstream socket `read()` is not called → TCP backpressure to
the upstream. Bytes flow from the first frame (the gate forwards
HEADERS/DATA the instant the queue drains); this is NOT
"accumulate-then-send". The progressive queue is
`VecDeque<bytes::Bytes>` — the `bytes::Bytes` hot-path buffer mandated
by R8.

## 1.5 Non-vacuous memory gauge (P1-B, builder-2)

Owner: builder-2 (P1-B, task #4). Exact mirror of the request-side
gauge: `MAX_RETAINED_BODY_BYTES` / `record_retained`
(`h3_bridge.rs:483 / 491`) and the per-stream summation
`record_retained_for_stream` (`conn_actor.rs:740-765`).

Add to `h3_bridge.rs`, `cfg(any(test, feature="test-gauges"))`:

```rust
pub static MAX_RETAINED_RESP_BYTES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
fn record_resp_retained(n: usize) { /* CAS-max, identical to record_retained */ }
```

Recorded in the actor at the **largest point** — after §1.4.3 drains
the receivers into the `Progressive` `StreamTx`s and **before**
`drain_streams_to_conn` — as the sound UPPER bound:

```
Σ_streams ( Σ progressive-StreamTx.queue chunk lengths )
         + ( used_slots × (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX) )  // channel occupancy upper bound
```

`used_slots = tx.max_capacity() - tx.capacity()` on the
`Sender<RespEvent>` (same technique as `record_retained_for_stream`'s
`chan_used`). The gauge MUST over- not under-estimate: each queued
`RespEvent::Bytes` carries a PRE-ENCODED H3 frame — a
≤`H3_RESP_CHUNK_MAX` payload PLUS the frame-header varints — so the
per-slot bound is `H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX`, NOT bare
`H3_RESP_CHUNK_MAX` (C5; builder-1's catch — a bare-`H3_RESP_CHUNK_MAX`
gauge would UNDER-count by the frame header and be an unsound memory
proof, violating R8's non-vacuous requirement; this matches the
request-gauge soundness rule the §1.2 comment cites,
`h3_bridge.rs:757-759`, and §1.2's own
`H3_RESP_CHANNEL_DEPTH × (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX)`
bound). **R2** (1 MiB response, stalled H3 client) asserts
`MAX_RETAINED_RESP_BYTES ≤ 4 × (H3_RESP_CHANNEL_DEPTH ×
(H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX))` and `≪ 1 MiB` — still
non-vacuous (the `+ H3_FRAME_HDR_MAX` per slot is ≈128 B vs the 64 KiB
channel term; the `≪ 1 MiB` / body-size-independence proof is
unaffected).

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
