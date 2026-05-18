# S7 — H3 → H3 R8 Streaming Plan (PLAN-ONLY, code-change-free)

- Author: `builder-1` (R9 worktree `.claude/worktrees/builder-1-h3h3-plan`, branch `s6/builder-1-h3h3-plan`)
- Base: `9cb91cee` (feature/h-matrix-s6, H3→H2 verified BUILT + integrated)
- Status: PLAN ONLY. ZERO source changes, ZERO `cargo build`/`test`. Implementation is S7 work, BLOCKED on lead R8 approval of this plan.
- Reference cell (the bar): H3→H1 (`h3_to_h1_stream_resp`, `write_h1_request`, `stream_h1_response`) and the just-verified H3→H2 (`h3_to_h2_stream_resp`, `H3ReqStreamBody`, `stream_h2_response`) — H3→H3 mirrors the H3→H2 shape, swapping the hyper-h2 upstream for a quiche-H3 upstream.
- Defect this plan closes: H3→H3 currently uses `h3_to_h3_roundtrip` (`crates/lb-quic/src/h3_bridge.rs:2642`) which (a) **drops every request body** — doc: *"Request-body forwarding is not supported in 3b.3c-3"*, only a bodyless GET is wired; and (b) **buffers the whole response** in an unbounded `decoded_body: Vec<u8>` (`h3_bridge.rs:2720`, `:2762`). Both violate R8 and (a) is a silent functional data-loss defect (same class as the H3→H2 one we just fixed).

---

## 0. TL;DR of the design

Replace `h3_to_h3_roundtrip` (buffered, body-dropping) with a streaming
`h3_to_h3_stream_resp` that has the SAME shape and SAME channel
contract as the verified `h3_to_h2_stream_resp`:

- **Reuse M-A verbatim** (the verified inbound request-body pump): the
  `conn_actor::poll_h3` H3-backend branch registers the SAME
  `(btx,brx)` `mpsc<ReqBodyEvent>` (depth `H3_BODY_CHANNEL_DEPTH=8`,
  chunk `H3_BODY_CHUNK_MAX=8 KiB`) + `(resp_tx,resp_rx)`
  `mpsc<RespEvent>` (depth `H3_RESP_CHANNEL_DEPTH=8`) + `StreamTx::progressive()`
  + the shared `fin`/body-channel tail it ALREADY builds for the H3→H2
  branch (`conn_actor.rs:809-863`). No M-A rebuild — a near-verbatim
  copy of the H2 branch with the spawned fn swapped.
- **New M-C** = a bounded streaming **H3 upstream connector**: drive the
  pooled quiche upstream connection with the request HEADERS frame then
  incremental request DATA frames pulled one-at-a-time from `brx`
  (in-flight window = the channel depth, NOT body size), AND consume the
  upstream H3 response incrementally — re-encoding each upstream DATA
  frame into a `RespEvent::Bytes` over the SAME bounded `resp_tx` the
  actor already drains under its §1.4.3 backpressure gate. Both
  directions respect QUIC stream flow control.
- Net result: H3→H3 reaches the H3→H1/H3→H2 BUILT bar — both bodies
  bounded by a fixed in-flight window, end-to-end backpressure both
  ways via the QUIC flow-control window, and a feature-gated
  non-vacuous bidirectional memory + backpressure + smuggling-parity
  e2e through a REAL H3 upstream.

The critical structural difference vs H3→H2: there is **no hyper**.
H3→H2 delegated upstream framing/flow-control to the hyper h2 client
(`Http2Pool::send_request` + `Incoming`). H3→H3 must drive the quiche
connection's send/recv event loop **itself** (as the current
`h3_to_h3_roundtrip`/`request_h3_upstream` already do for the buffered
case), so M-C's backpressure is expressed directly against
`quiche::Connection::stream_capacity` / `stream_recv` rather than
through a hyper `Body`. This is more code than H3→H2's M-B but uses
only existing quiche APIs (see §8 — no `lb-io`/codec/quiche change).

---

## 1. Request path — forward real H3 DATA frames to the H3 upstream, bounded-incrementally (FIXES the dropped-body defect)

**Current (broken):** `h3_to_h3_roundtrip` encodes only a HEADERS
frame and `stream_send(..., fin=true)` immediately (`h3_bridge.rs:2694-2712`)
— there is no `body` parameter at all; every inbound request DATA
frame is discarded by the actor because the H3-backend branch takes
the legacy `request_tasks` Vec path (`conn_actor.rs:864-874`) which
never registers a `body_tx_by_stream` channel.

**Plan.** The request body is already produced, bounded, into a
`tokio::sync::mpsc::Receiver<ReqBodyEvent>` by the unchanged M-A
machinery once the H3-backend branch registers the channel (exactly as
the H3→H2 branch does). M-C consumes that `brx` the SAME way
`H3ReqStreamBody` / `h2_request_body_from_rx` do for H2:

- New `h3_to_h3_stream_resp(req, addr, sni, pool, body_rx, resp_tx, cap)`
  orchestrator, shaped 1:1 on `h3_to_h2_stream_resp`
  (`h3_bridge.rs:2324`):
  - Peek the FIRST `ReqBodyEvent` (bounded — one event) to choose the
    request shape, exactly as `h2_request_body_from_rx` /
    `write_h1_request` do:
    - `End` / channel-closed first ⇒ **bodyless** request: send the
      HEADERS frame with FIN (byte-identical to today's bodyless GET —
      no regression for the currently-wired case).
    - `Reset` first ⇒ pre-dial abort: emit the inline `413` via the
      same `inline(tx,status,body)` helper pattern
      (`h3_bridge.rs:2335`) and dial NOTHING (oversized / cancel
      before data — smuggling-guard parity).
    - `Chunk(b0)` first ⇒ a **streaming** request body.
  - Acquire the pooled upstream (`pool.acquire(addr, sni)` — unchanged
    `QuicUpstreamPool` API, `quic_pool.rs:264`). On acquire failure ⇒
    inline `502`, `Ok(())` (same as the H2 path).
- The request-DATA pump (inside the M-C event loop, §2): each
  `ReqBodyEvent::Chunk(b)` pulled from `brx` is `encode_frame(H3Frame::Data{b})`
  and written to upstream stream 0 with `stream_send(sid, &frame, fin=false)`.
  Exactly **one event is held at a time** (peek-then-loop, identical to
  `write_h1_request`/`H3ReqStreamBody`); the only retained request
  bytes are the in-hand chunk (≤ `H3_BODY_CHUNK_MAX` = 8 KiB) plus the
  `H3_BODY_CHANNEL_DEPTH`-bounded channel — a FIXED in-flight window,
  body-size independent, NOT a total-body cap.
- `ReqBodyEvent::End` ⇒ send the (already-peeked or empty) final DATA
  with `fin=true` to cleanly terminate the upstream request stream.
  Request trailers in `End.trailers` are **DROPPED** on the H3→H3 leg
  in this increment — documented; parity with the H3→H1 P1-C and the
  H3→H2 (lead A3) scoped-out decision. (Note: unlike H1/H2,
  forwarding H3 request trailers here would be a *small* future
  follow-on since `request_h3_upstream` already shows the
  trailing-HEADERS encode; explicitly listed as a scoped-out S7 item,
  not silent loss — the body is fully + correctly framed by the FIN.)
- `ReqBodyEvent::Reset` (mid-body client RESET / producer dropped
  before `End`) ⇒ M-C **does NOT** send FIN; it
  `stream_shutdown(sid, Shutdown::Write, H3_REQUEST_CANCELLED)` (RFC
  9114 error code) and marks the pooled conn non-reusable, so the
  upstream never sees a completable (truncated-as-complete) request —
  request-smuggling parity with `H3ReqStreamBody`'s abort and
  `write_h1_request`'s pre-terminator return. This is the binding
  case-7 mechanism for H3→H3.

This FIXES owner binding condition 1 (request body must not be
dropped) for H3→H3.

## 2. Upstream H3 connector (M-C) — drive the quiche upstream incrementally, no `decoded_body`, with QUIC flow control BOTH ways

**Current (broken):** the `h3_to_h3_roundtrip` event loop
(`h3_bridge.rs:2724-2797`) reads upstream stream bytes into `rx_tail`,
decodes frames, and **accumulates every DATA payload into
`decoded_body: Vec<u8>`** until `content-length`/FIN, then
`encode_h3_response(status, &decoded_body)` — memory ∝ response size,
no backpressure, request body absent.

**Plan — exact mechanism, functions, consts, channels.**

M-C is one `async fn` that owns the pooled `quiche::Connection` and
runs a single send/recv/timeout event loop (same skeleton as the
current `h3_to_h3_roundtrip` loop and `request_h3_upstream`, which is
the proven way to drive a pooled upstream quiche conn), but with the
buffering replaced by bounded incremental pumps in BOTH directions:

### 2a. Upstream send half (request egress, flow-control-respecting)
- Before writing each request DATA frame, consult
  `quiche::Connection::stream_capacity(sid)` (quiche 0.28,
  `lib.rs:6261`). If capacity < the next frame, do **not** pull the
  next `ReqBodyEvent` from `brx` this iteration — the chunk stays in
  the channel. Because M-A only refills `brx` as it drains, a stalled
  upstream send window ⇒ `brx` fills ⇒ `poll_h3`/`drain_body_stream`
  stops calling `stream_recv` on the *downstream client* stream ⇒
  quiche stops extending the downstream client's flow-control window ⇒
  **the H3 client is paused** (the existing M-A request-side
  backpressure, unchanged — verified by H3→H1 T5 / H3→H2 case 4).
- Partial `stream_send` returns are handled exactly as the current
  loop does (advance by `n`, retry next iteration) — but the
  *unsent-chunk* state is at most ONE in-hand `ReqBodyEvent`, never an
  accumulation.

### 2b. Upstream recv half (response ingress, bounded, no `Vec<u8>`)
- Keep a SMALL bounded `rx_tail` working buffer ONLY large enough to
  hold one undecoded frame header + one in-progress frame up to
  `H3_RESP_CHUNK_MAX` (mirrors `stream_h1_response`'s bounded read
  buffer). Decode frames incrementally:
  - First HEADERS frame ⇒ build the downstream response HEADERS
    `RespEvent::Bytes` via `encode_h3_headers_frame(status, cl_opt)`
    (reuse the existing helper — byte-identical to the H3→H1/H2 head),
    `resp_tx.send(...).await`.
  - Each DATA frame payload ⇒ split into `≤ H3_RESP_CHUNK_MAX` slices,
    `encode_h3_data_frame(slice)` each, `resp_tx.send(RespEvent::Bytes(..)).await`.
    **No `decoded_body` accumulation** — the payload is forwarded and
    dropped incrementally.
  - A post-DATA trailing HEADERS frame ⇒ one trailing-HEADERS
    `RespEvent::Bytes` (parity with `stream_h1_response`'s C4 / the
    H3→H2 trailer handling), BEFORE `End`.
  - Clean stream FIN ⇒ `resp_tx.send(RespEvent::End).await` ⇒ the
    actor FINs the downstream client.
  - Upstream RESET_STREAM / decode error / premature FIN ⇒ best-effort
    `resp_tx.send(RespEvent::Reset)` + return `Err(RespAbort::UpstreamReset)`
    so the actor RESET_STREAMs the client and NEVER FINs (a partial
    body is never presentable as complete — response-splitting guard,
    identical contract to `stream_h1_response`/`stream_h2_response`).
- **The recv-side flow-control / backpressure point (the load-bearing
  mechanism).** `resp_tx` is the bounded `mpsc<RespEvent>(H3_RESP_CHANNEL_DEPTH=8)`.
  The `resp_tx.send(...).await` is the gate: if the actor's
  `drain_resp_channels` §1.4.3 gate is not pulling (because the
  downstream H3 client's QUIC send window is full — a stalled client),
  the channel fills and `send().await` parks. While parked, M-C does
  **not** call `stream_recv` on the *upstream* connection this
  iteration, so quiche does not consume the upstream stream's received
  data, does not advance the upstream stream's `MAX_STREAM_DATA`, and
  the upstream H3 server's send window closes — **stalled downstream
  client ⇒ paused upstream H3 read, via the QUIC flow-control window**.
  This is the exact analogue of H3→H2's "stalled client ⇒ hyper stops
  WINDOW_UPDATE ⇒ upstream send window closes", expressed natively in
  quiche instead of hyper.

### 2c. End-to-end backpressure causal chain (explicit, both directions)
- **Response direction (downstream client stalls):**
  downstream H3 client stops reading
  → quiche (front listener) stops extending the client stream's flow
  window
  → `drain_resp_channels` §1.4.3 gate stops pulling (`queue` non-empty)
  → `resp_tx` (depth 8) fills
  → M-C's `resp_tx.send().await` parks
  → M-C stops calling `stream_recv` on the upstream quiche conn
  → upstream stream's received data is not consumed; quiche does not
    send `MAX_STREAM_DATA`/`MAX_DATA` to the upstream
  → **upstream H3 server's send window closes — upstream read paused.**
  Max retained ≈ `H3_RESP_CHANNEL_DEPTH × (H3_RESP_CHUNK_MAX +
  H3_FRAME_HDR_MAX)` + one in-hand frame — body-size independent.
- **Request direction (upstream backend stalls reading):**
  upstream H3 server stops reading its request stream
  → quiche (upstream conn) stops extending M-C's send window
  → `stream_capacity(sid)` stays small; M-C stops pulling `brx`
  → `brx` (depth 8) fills
  → M-A `poll_h3`/`drain_body_stream` stops calling `stream_recv` on
    the downstream client stream
  → front quiche stops extending the *client's* request stream window
  → **downstream H3 client's request upload is paused.**
  Max retained ≈ `H3_BODY_CHANNEL_DEPTH × (H3_BODY_CHUNK_MAX +
  MAX_FRAME_HEADER_BYTES)` + one in-hand chunk — body-size independent.

### 2d. Gauges — reuse the EXISTING feature-gated statics verbatim
No new gauge. The response-direction proof reads
`lb_quic::h3_bridge::MAX_RETAINED_RESP_BYTES`, fed by the unchanged
`drain_resp_channels` §1.5 block (`conn_actor.rs:426-440`) — because
M-C uses the SAME `resp_tx`/`StreamTx::progressive()` path, that gauge
already measures M-C's retained bytes with zero new wiring. The
request-direction proof reads `MAX_RETAINED_BODY_BYTES`, fed by the
unchanged M-A request-pump gauge (`record_retained`,
`h3_bridge.rs:558`) — also zero new wiring (same reason H3→H2 case 4
worked without touching the gauge). This is a deliberate design choice
so the R8 memory proof is the *same instrument* the H3→H1/H3→H2 cells
were graded on.

## 3. Response path — H3 upstream response → downstream H3, no `Vec<u8>` accumulation

Covered structurally by §2b: there is **no `decoded_body: Vec<u8>`**;
the upstream response is re-emitted frame-by-frame onto the bounded
`resp_tx`. The orchestrator `h3_to_h3_stream_resp` returns
`Result<(), RespAbort>` exactly like `h3_to_h2_stream_resp` /
`h3_to_h1_stream_resp`; the actor logs the abort and RESET_STREAMs.
`h3_to_h3_roundtrip` is **deleted** once the actor is rewired (it has
no other caller — verified: only `conn_actor.rs:870` and the
`lib.rs:124` re-export reference it; `request_h3_upstream` is a
SEPARATE function used by H1→H3/H2→H3 and is **left byte-for-byte
untouched** — see §8 / R3 risk).

## 4. Verification design (to the H3→H1 bar — owner binding condition 2)

New real-wire suite: `crates/lb-quic/tests/h3_h3_stream_e2e.rs`. Real
wire: front quiche H3 client → production `QuicListener`
(`.with_h3_backend(pool, addr, sni)`) → `router`/`conn_actor` (H3
branch) → `h3_to_h3_stream_resp` → a **REAL H3 upstream** (a quiche
server speaking the lb-h3 codec — reuse/extend the existing
`spawn_h3_*` mock-upstream pattern from `tests/proto_translation_e2e.rs`
/ the `h3_upstream_verify.rs` / `quic_native.rs` harness; the
upstream must be a genuine QUIC endpoint, not an in-process shortcut).
All gauge cases compile only under `--features test-gauges` (the
`MAX_RETAINED_*` statics are `#[cfg(any(test, feature="test-gauges"))]`).
Runner: `cargo test -p lb-quic --features test-gauges --test h3_h3_stream_e2e`.

Authoritative numbers (mirror the H3→H1/H3→H2 C5 formula exactly):
```
DEPTH=8, CHUNK_MAX=8192, FRAME_HDR_MAX=16   (req: H3_BODY_CHUNK_MAX/MAX_FRAME_HEADER_BYTES)
C5 channel bound = DEPTH×(CHUNK_MAX+FRAME_HDR_MAX) = 8×8208 = 65 664 B
RESP/REQ ceiling = 4 × 65 664                       = 262 656 B
proof body       = 4 MiB = 4 194 304 B  (≥8× margin: ≈15.97×; backpressure case uses 8 MiB ≈32×)
```
Ceiling helper copied from `h3_h2_stream_e2e::retained_ceiling`
(`= 4 × depth×(chunk+hdr)`) so `test ceiling == gauge bound`.

Cases (names final; mirror `h3_h2_stream_e2e.rs`'s 7):
1. `h3h3_e2e_get_response_byte_identical` — liveness floor: GET, 200 +
   body byte-identical, clean FIN (no-regression for today's only
   wired case).
2. **`h3h3_e2e_request_body_byte_identical_at_backend`** (BINDING cond
   1) — ≥1 MiB NON-UTF-8 binary request body over many DATA frames;
   the real H3 backend captures the full received body and the test
   asserts it equals the sent bytes BYTE-FOR-BYTE *and* that the
   backend saw a cleanly-ended request. Proves the dropped-request-
   body defect is fixed.
3. **`h3h3_e2e_response_memory_bounded_through_stalled_client`**
   (BINDING cond 2, response dir) — 4 MiB H3 response, stalled H3
   client; sample `MAX_RETAINED_RESP_BYTES` ≤ 262 656 (≥8× < body)
   AND, after resume, 4 MiB byte-identical + clean FIN (non-vacuous +
   liveness). Mirrors H3→H2 case 3.
4. **`h3h3_e2e_request_memory_bounded_through_stalled_backend`**
   (BINDING cond 2, request dir) — 4 MiB binary request body, the H3
   backend stalls reading; sample `MAX_RETAINED_BODY_BYTES` ≤ 262 656
   while body is 16× that; unblock ⇒ byte-identical. Mirrors H3→H2
   case 4.
5. **`h3h3_e2e_backpressure_stalled_client_pauses_upstream_read`** —
   8 MiB response, stalled client; retained ≤ ceiling (~32× body) AND
   completes byte-identical after resume (the §2c response-direction
   causal chain held with no drop/corruption). Mirrors H3→H2 case 5.
6. `h3h3_e2e_upstream_reset_midbody_resets_client_no_fin` — the H3
   backend RESET_STREAMs mid-body ⇒ the client never gets a clean
   complete 200 (response-splitting guard, `RespAbort::UpstreamReset`).
7. **`h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request`**
   (BINDING — request-side smuggling-parity, the analog of H3→H2 case
   7 / lead A2) — the H3 client RESETs MID request body; M-C
   `stream_shutdown(Write)`s the upstream request stream WITHOUT FIN
   and drops the pooled conn non-reusable; the real H3 backend NEVER
   observes a silently-truncated request presented as complete (it
   sees a reset/incomplete request stream, never a short-but-clean
   FIN'd body).

All cases run, none `#[ignore]`d. The file MUST NOT carry any
stale/`#[ignore]`-claiming comment (the H3→H1 integrity lesson).

**Binding conditions satisfied:** (1) case 2 — non-empty binary
request body byte-identical at a real H3 backend; (2) cases 3+4 —
non-vacuous live-gauge memory proofs both directions under a stalled
peer, plus case 5 backpressure proof; (3) all gated/runnable via
`cargo test -p lb-quic --features test-gauges` and the gauges are the
real feature-compiled statics (a run WITHOUT the flag fails to compile
the memory cases — invalid gate by design).

## 5. Integrity / no-regression carry-forwards

- `request_h3_upstream` (the H1→H3 / H2→H3 connector) is **NOT
  touched** — H3→H3 gets a NEW `h3_to_h3_stream_resp`; the buffered
  `request_h3_upstream` and its callers (`h1_proxy.rs:1183`,
  `h2_proxy.rs`) stay byte-for-byte (R3). (Bounded streaming for the
  H1→H3/H2→H3 cells is a *later* H-matrix cell that will reuse M-C —
  out of scope for S7.)
- No stale `#[ignore]`/SCAFFOLD comment is introduced (H3→H1 lesson).
- H3→H1 and H3→H2 are preserved (R3): the actor change is confined to
  swapping ONLY the `if let Some((qpool, addr, sni)) = h3_backend`
  branch (`conn_actor.rs:864-874`) from the legacy `request_tasks` Vec
  path to the H3→H2-shaped `resp_tasks` streaming path; the H1 and H2
  branches and the inline-error path are untouched. Self-checks
  re-run `h3_h1_*` + `h3_h2_stream_e2e` to prove no regression.

## 6. R8 self-check — why no path reintroduces full-body buffering

| Buffer | Bound | Body-size independent? |
|---|---|---|
| Inbound H3→bridge req chunk (M-A, unchanged) | 1 × `ReqBodyEvent` ≤ `H3_BODY_CHUNK_MAX` (8 KiB) + channel depth 8 | YES (proven by H3→H1 T5 / H3→H2 case 4) |
| §1/§2a upstream request egress | one in-hand `ReqBodyEvent` written per `stream_send`; gated by `stream_capacity`; NO accumulation, NO total cap | YES |
| §2b upstream response `rx_tail` working buffer | ≤ one frame header + one ≤`H3_RESP_CHUNK_MAX` in-progress frame (bounded read, like `stream_h1_response`) | YES |
| §2b response channel | `mpsc<RespEvent>(H3_RESP_CHANNEL_DEPTH=8)`, each ≤ `H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX` | YES |
| Actor drain (unchanged) | existing `Progressive` `StreamTx` + `record_resp_retained` gauge | YES (proven by H3→H1 R2/R3, H3→H2 case 3/5) |

Every buffer is bounded by a FIXED in-flight window (`depth × chunk`),
never by total body size and never by a total-body cap. The forbidden
patterns are all removed from the H3→H3 path and none is
reintroduced: the unbounded `decoded_body: Vec<u8>` is **deleted**;
there is no `.collect()`; the 64 MiB `MAX_REQUEST_BODY_BYTES` /
`MAX_RESPONSE_BODY_BYTES` remain ONLY as DoS abort thresholds passed as
`cap` (identical role to H3→H1/H3→H2), NOT as the memory bound. There
is exactly ONE bounded streaming pipe per direction.

## 7. Increment breakdown (ordered, independently-committable; S7)

Each increment is a single small commit on the S7 branch with its own
scoped check (`cargo test -p lb-quic [--features test-gauges]` —
disk-friendly; full `--workspace` reserved for the S7 Phase-3 gate).

| # | Increment | Self-check |
|---|---|---|
| J1 | **M-C response/recv half + orchestrator skeleton**: add `h3_to_h3_stream_resp` driving the pooled quiche conn, bodyless-request path only (HEADERS+FIN), incremental response re-encode onto `resp_tx` (no `decoded_body`). Old `h3_to_h3_roundtrip` kept, not yet rewired. | `cargo test -p lb-quic --lib` green + a unit test of the response frame re-encode/abort mapping (pure, like H3→H2 I1's). |
| J2 | **M-C request/send half**: peek-first-event framing + incremental request-DATA pump with `stream_capacity` gating + mid-body `Reset` ⇒ `stream_shutdown(Write)`+non-reusable. Still not rewired. | `cargo test -p lb-quic --lib` green + unit test of the framing/abort decision (analogue of H3→H2 I2's `s6_i2_*`). |
| J3 | **Actor rewire + delete dead code**: swap the `h3_backend` branch (`conn_actor.rs:864-874`) to the H3→H2-shaped `resp_tasks` streaming spawn (btx/brx + resp_tx/resp_rx + Progressive + shared fin/body tail); delete `h3_to_h3_roundtrip`; fix its `lib.rs`/doc references. | `cargo test -p lb-quic --features test-gauges` — `h3_h1_*` + `h3_h2_stream_e2e` ALL still green (R3); existing H3→H3 wiring test (`proto_translation_e2e` / `quic_native` / `h3_upstream_verify`) still green. |
| J4 | **Verification suite**: add `tests/h3_h3_stream_e2e.rs` (§4 cases 1–7, incl. BINDING 2 + 7) with a real quiche H3 upstream. | `cargo test -p lb-quic --features test-gauges --test h3_h3_stream_e2e` — 7/7, 0 ignored, deterministic (≥3 runs). |
| J5 | **S7 phase gate** (verifier-assigned, not builder-1): full `--workspace` + clippy + fmt at the agreed gate point. | S7 Phase-3 gate (the only `--workspace` build). |

J1→J2→J3 sequential (J3 needs both halves). J4 depends on J3. Author
≠ verifier: builder-1 implements J1–J4 and does NOT self-verify the
cell.

## 8. Scope / size estimate, NEW machinery / scope items, open questions

**Size: M** (matches `s6-inventory.md` "H3→H3 … Size: M"). Larger than
H3→H2's S–M because there is no hyper to delegate upstream framing /
flow control to — M-C must drive the quiche send/recv/timeout event
loop itself. But the loop skeleton already exists twice
(`h3_to_h3_roundtrip`, `request_h3_upstream`); the net new work is
swapping the two buffering points for the two bounded pumps + the
`stream_capacity` gate. Estimated ≈ 450–650 LoC src + ≈ 700–900 LoC
test (the e2e needs a real quiche H3 upstream harness — the heaviest
part; reuse the existing mock-H3-upstream pattern, do not hand-roll a
new QUIC stack).

**NEW shared machinery / known-up-front scope items (flagged, NOT
implemented this session):**

- **S-1 (likely NONE — analog of the I0.5 hyper::Error blocker):** the
  H3→H2 cell needed an `lb-io` widening because `hyper::Error` had no
  public constructor. The H3 upstream pool (`QuicUpstreamPool` /
  `UpstreamQuicConn`) exposes the raw `&mut quiche::Connection`
  (`connection_mut`), its `Arc<UdpSocket>`, `local`/`peer`, and
  `set_reusable` — M-C drives quiche directly with NO intermediate
  typed-error body, so there is **no analogous error-type blocker**.
  Request-abort is expressed as `stream_shutdown(Write,...)` +
  `set_reusable(false)` on the existing API — no `lb-io`/quiche/codec
  change anticipated. **Open question Q1** asks the lead to confirm
  this analysis so S7 starts informed (if implementation reveals a
  gap, the A1 honest-stop/escalation contract applies — STOP and
  re-request approval, do not self-decide).
- **S-2 (pool single-stream limitation — design constraint, document
  do-not-fix):** `h3_to_h3_roundtrip` today sends FIN on upstream
  stream 0 and unconditionally `set_reusable(false)` because the pool
  "carries no stream-ID allocation state across checkouts"
  (`h3_bridge.rs:2799-2804`). M-C MUST preserve that disposition
  (one request per pooled upstream conn, non-reusable on completion)
  — it is correct and sufficient for R8 (correctness/memory are
  unaffected; only connection-reuse efficiency, explicitly out of
  scope). Multi-stream pooled H3 upstreams are a separate future
  optimisation, NOT an S7 deliverable. Listed so S7 does not
  accidentally widen scope.
- **S-3 (no new gauge):** explicitly NOT new machinery — M-C reuses
  `MAX_RETAINED_RESP_BYTES`/`MAX_RETAINED_BODY_BYTES` verbatim via the
  unchanged actor drain + M-A pump (§2d). Recorded here so the lead
  can confirm reusing the H3→H1/H3→H2 instrument (vs a bespoke gauge)
  is the intended R8 measurement.

**Open questions for the lead:**

- **Q1.** Confirm M-C needs NO `lb-io`/`quiche`/codec change (S-1
  analysis): the existing `QuicUpstreamPool::acquire` +
  `UpstreamQuicConn::{connection_mut,socket,local,peer}` +
  `set_reusable` surface is sufficient to drive bounded bidirectional
  streaming with `stream_capacity`-gated send and bounded recv. Approve
  this as the S7 scope boundary (with the standing A1 escalation
  contract if implementation proves otherwise)?
- **Q2.** Confirm the S-2 disposition: M-C keeps "one request per
  pooled upstream conn, `set_reusable(false)` on completion" (parity
  with today's `h3_to_h3_roundtrip`). Multi-stream upstream pooling is
  explicitly OUT of S7 scope. Acceptable?
- **Q3.** Request trailers on the H3→H3 leg: plan DROPS them (parity
  with H3→H1 P1-C / H3→H2 lead A3), documented, body fully framed by
  FIN. Confirm in-scope-out for S7 (vs forwarding the trailing-HEADERS
  frame — which `request_h3_upstream` already shows how to encode, so
  it could be a small follow-on if the lead wants it IN).
- **Q4.** The e2e (J4) needs a REAL quiche H3 upstream that speaks the
  lb-h3 codec and can (a) capture a multi-MiB request body, (b) stall
  reading, (c) RESET mid-body, (d) stall its own response. Confirm
  reusing/extending the existing mock-H3-upstream harness
  (`proto_translation_e2e`/`quic_native`/`h3_upstream_verify`) is
  acceptable rather than a brand-new upstream implementation (keeps
  J4 from ballooning; the bar is "real wire", which a real quiche
  endpoint satisfies).
