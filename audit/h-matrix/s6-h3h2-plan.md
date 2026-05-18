# S6 — H3 → H2 R8 Streaming Plan (PLAN-ONLY, code-change-free)

- Author: `builder-1` (R9 worktree `.claude/worktrees/builder-1-h3h2`, branch `s6/builder-1-h3h2`)
- Base: `60a13ddc` (= feature/h-matrix-s6, content == verified S5 tip `9c02b08e`)
- Status: PLAN ONLY. ZERO source changes in this commit. Implementation is BLOCKED on lead approval (R5).
- Reference cell (the bar): H3→H1, `crates/lb-quic/src/h3_bridge.rs` (`h3_to_h1_stream_resp` :1915, `write_h1_request` :1672, `stream_h1_response` :1186), `conn_actor::poll_h3` :702.
- Defect this plan closes: H3→H2 currently (a) **deletes every request body** (`Full::<Bytes>::new(Bytes::new())`, `h3_bridge.rs:2271`) and (b) **buffers the whole response** (`body.collect().await`, `h3_bridge.rs:2288`). Both violate R8 and (a) is a silent functional data-loss defect.

---

## 0. TL;DR of the design

Replace `h3_to_h2_roundtrip` (buffered, body-dropping) with a streaming
`h3_to_h2_stream_resp` that has the SAME shape and SAME channel
contract as the proven `h3_to_h1_stream_resp`:

- **Reuse M-A verbatim**: the inbound request-body path
  (`conn_actor::poll_h3` → `body_tx_by_stream` bounded `mpsc<ReqBodyEvent>`,
  depth `H3_BODY_CHANNEL_DEPTH=8`, chunk `H3_BODY_CHUNK_MAX=8 KiB`) is
  already verified by H3→H1 and is protocol-of-backend-agnostic. The
  H3→H2 spawn site stops calling the buffered roundtrip and instead
  registers the SAME `(btx,brx)` request channel + `(resp_tx,resp_rx)`
  response channel + `StreamTx::progressive()` it already builds for the
  H1 branch (`conn_actor.rs:840-882`).
- **New M-B** = a bounded H2 upstream egress: feed the hyper h2 client a
  **streaming `BoxBody`** whose frames are pulled one-at-a-time from
  `brx` (in-flight window = the channel depth, NOT the body), and read
  the H2 response `Incoming` body **frame-by-frame**, re-encoding each
  into an H3 DATA `RespEvent::Bytes` over the SAME bounded `resp_tx`
  the actor already drains under its backpressure gate.
- Net result: H3→H2 reaches the H3→H1 BUILT bar — both directions
  bounded by a fixed in-flight window, end-to-end backpressure both
  ways, and a non-vacuous feature-gated memory + backpressure proof on
  a real wire through a real H2 backend, with a non-empty binary
  request body asserted byte-identical at the backend.

---

## 1. Request path — forward real H3 DATA frames to H2 upstream, bounded-incrementally (FIXES the dropped-body defect)

**Current (broken):** `h3_to_h2_roundtrip` hard-wires
`let body = Full::<Bytes>::new(Bytes::new())...` (`h3_bridge.rs:2271`),
so every inbound H3 request DATA frame is silently discarded; the H2
upstream always sees a zero-length body. POSTs/PUTs are corrupted.

**Plan:** the request body is already produced, bounded, into a
`tokio::sync::mpsc::Receiver<ReqBodyEvent>` by the unchanged
`poll_h3`/`drain_body_stream` machinery (M-A). H3→H2 must consume that
`brx` exactly as `write_h1_request` does — one `ReqBodyEvent` held at a
time — and turn it into the hyper request body:

- New `fn h2_request_body_from_rx(req, &mut brx) -> (Request<BoxBody<Bytes,hyper::Error>>, BodyDriver)`:
  - Peek the FIRST `ReqBodyEvent` (same pattern as `write_h1_request:1679`)
    to decide framing: `End`/closed first ⇒ bodyless
    (`Full::new(Bytes::new())` — legitimately empty, content-length 0);
    `Reset` first ⇒ abort 413 before dialing (no upstream connection
    opened — smuggling/over-size guard, mirrors `write_h1_request:1687`);
    `Chunk(b)` first ⇒ a streaming body.
  - The streaming body is a `http_body_util::StreamBody` /
    `channel`-backed `BoxBody` whose `poll_frame` pulls the NEXT
    `ReqBodyEvent` from `brx` and yields exactly one `Frame::data(Bytes)`
    per `Chunk`, completes on `End`, and **errors** (hyper::Error-shaped)
    on `Reset` or premature channel close so hyper RST_STREAMs the
    upstream and the request is never presentable as complete
    (request-smuggling parity with `ReqWriteOutcome::Aborted`,
    `h3_bridge.rs:1764-1797`).
  - We do NOT call `body.collect()`, do NOT pre-size, do NOT cap by
    total body. The only buffer is the single `ReqBodyEvent` currently
    in hand (≤ `H3_BODY_CHUNK_MAX` = 8 KiB) — a FIXED in-flight window
    independent of body size (R8).
- Headers/pseudo-headers: keep the existing `h3_to_h2_roundtrip`
  URI/`:method`/header construction (`h3_bridge.rs:2255-2270`) verbatim
  — only the body argument changes from `Full::new(empty)` to the
  streaming `BoxBody`. Request trailers in `End.trailers` are dropped
  on the H2 leg in this increment (documented; H2 trailer egress is a
  later, separate concern — parity with the H3→H1 P1-C decision at
  `h3_bridge.rs:1735-1762`; NOT a silent data loss because the body is
  fully + correctly framed).

**Owner binding condition 1 (request body must not be dropped):** closed
— a non-empty binary request body now flows DATA-frame-by-DATA-frame to
the H2 backend and is asserted byte-identical there (see §4).

## 2. Upstream H2 egress (M-B) — drive hyper h2 incrementally, no `collect()`

**Current (broken):** `let (parts, body) = resp.into_parts(); let
body_bytes = body.collect().await?.to_bytes();` (`h3_bridge.rs:2287-2294`)
materialises the ENTIRE upstream response in RAM before any byte goes
to the client. Memory ∝ response size.

**Plan — exact functions/consts/mechanism:**

- Keep `Http2Pool::send_request(addr, request) -> Result<Response<Incoming>, Http2PoolError>`
  (`crates/lb-io/src/http2_pool.rs:207`) **unchanged** — it already
  returns `Response<Incoming>` (a streaming body) and only awaits the
  header roundtrip, not the body. M-B is purely on the H3-side
  consumer; no `lb-io` change needed (open question Q1 confirms).
- New `async fn stream_h2_response(resp: Response<Incoming>, tx: &Sender<RespEvent>, cap: usize) -> Result<(), RespAbort>`
  modelled 1:1 on `stream_h1_response` (`h3_bridge.rs:1186`):
  1. From `resp.parts()` build the H3 HEADERS frame
     (`encode_h3_headers_frame(status, content_length_opt)`, reusing
     the existing helper) and `send!(tx, RespEvent::Bytes(headers))`
     **before any body byte** — identical to `stream_h1_response:1304`.
  2. Loop: `while let Some(frame) = body.frame().await` (hyper
     `http_body_util::BodyExt::frame`). For each DATA frame, split its
     `Bytes` into ≤ `H3_RESP_CHUNK_MAX` (8 KiB) slices, encode each as
     an H3 DATA frame, and `send!(tx, RespEvent::Bytes(..))`. Each
     `send!` is `tx.send(..).await` on the bounded
     `mpsc<RespEvent>(H3_RESP_CHANNEL_DEPTH=8)` — **this await is the
     backpressure point**: if the actor's drain (`drain_resp_channels`,
     `conn_actor.rs` ~:404/439) is blocked because the H3 client's QUIC
     stream flow-control window is full (stalled client), the channel
     fills, this `.send().await` parks, `body.frame().await` is not
     called again, hyper stops issuing H2 `WINDOW_UPDATE`s, and the H2
     upstream's send window closes — **stalled H3 client ⇒ paused H2
     upstream read**, the full causal chain.
  3. Trailers frame (if the H2 response carries an HTTP trailers
     `Frame`) re-encoded as a trailing HEADERS `RespEvent::Bytes`
     (parity with the H3→H1 trailer egress C4 work already shipped).
  4. Clean end ⇒ `send!(tx, RespEvent::End)` ⇒ actor FINs.
  5. Any hyper body error / premature end ⇒ best-effort
     `tx.send(RespEvent::Reset)` + `Err(RespAbort::UpstreamReset)` so
     the actor RESET_STREAMs (never FIN) — response-splitting guard,
     identical contract to `stream_h1_response`'s `RespAbort` paths.
- Memory: the only retained bytes are the in-hand hyper DATA frame
  (split + dropped incrementally) plus ≤ `H3_RESP_CHANNEL_DEPTH`
  queued `RespEvent::Bytes`. The actor-side gauge already records this
  (`record_resp_retained`, `conn_actor.rs:439`). Total bound =
  `H3_RESP_CHANNEL_DEPTH × (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX)` —
  IDENTICAL to the H3→H1 response bound, body-size independent.

## 3. Response path — H2 response → H3 DATA frames, end-to-end backpressure, no `Vec<u8>` accumulation

Covered structurally by §2: there is **no `decoded_body: Vec<u8>`**
(that anti-pattern is the H3→H3 path, `h3_bridge.rs:2389`; we
deliberately do NOT copy it). The orchestrator:

- New `pub async fn h3_to_h2_stream_resp(req, addr, pool: &Http2Pool, mut body_rx: Receiver<ReqBodyEvent>, resp_tx: Sender<RespEvent>, cap: usize) -> Result<(), RespAbort>`,
  shaped exactly like `h3_to_h1_stream_resp` (`h3_bridge.rs:1915`):
  - Build request + streaming body (§1). On a pre-dial abort (first
    event `Reset`) emit the inline 413 via the same `inline(tx,status,body)`
    helper pattern (`h3_bridge.rs:1926`) and return `Ok(())`.
  - `pool.send_request(addr, request).await`; on `Err` emit inline 502,
    return `Ok(())` (no pooled-TCP poisoning concern — `Http2Pool`
    already evicts the multiplexed entry on Send/Timeout errors,
    `http2_pool.rs:222-227`; documented, no extra C2 bookkeeping needed
    — open question Q2 asks the lead to confirm this is sufficient vs.
    the H1 `set_reusable(false)` discipline).
  - On `Ok(resp)` ⇒ `stream_h2_response(resp, &resp_tx, cap).await`,
    returning its `Result<(), RespAbort>`.
- Actor wiring (`conn_actor::poll_h3`, the `if let Some((h2pool, addr)) =
  h2_backend` branch at :794): replace the `request_tasks.push(spawn(
  Box::pin(h3_to_h2_roundtrip(...))))` block with the **same shape the
  H1 branch already uses** (:839-882): build `(btx,brx)` +
  `(resp_tx,resp_rx)`, `resp_rx_by_stream.insert(sid, resp_rx)`,
  `stream_response.insert(sid, StreamTx::progressive())`,
  `resp_tasks.push(spawn(h3_to_h2_stream_resp(...)))`, and the
  identical `fin ? btx.try_send(End) : register body channel + decode_into_pending`
  tail. This is a near-verbatim copy of the H1 branch with the spawned
  fn swapped — minimal, low-risk delta. H3→H1, H3→H3, inline errors are
  untouched (R3 no-regression).

## 4. Verification design (to the H3→H1 bar — owner binding condition 2)

New test file: `crates/lb-quic/tests/h3_h2_stream_e2e.rs`. Real wire:
front H3 quiche listener → `spawn_router`/`conn_actor` → bridge → a
**real hyper H2 backend** (h2 over plaintext TCP, built with
`hyper::server::conn::http2`, mirroring the existing
`proto_translation_e2e.rs` H2 backend helper — reuse/lift that helper).
All cases gated to compile under `--features test-gauges` (the gauges
`MAX_RETAINED_RESP_BYTES` / `MAX_RETAINED_BODY_BYTES` are
`#[cfg(any(test, feature = "test-gauges"))]`, `h3_bridge.rs:552/588`).
Runner: `cargo test -p lb-quic --features test-gauges --test h3_h2_stream_e2e`.

Authoritative numbers (from the real crate consts, mirroring
`h3_h1_resp_stream_e2e.rs:1092-1097`):
```
DEPTH=8, CHUNK_MAX=8192, FRAME_HDR_MAX=16
C5 channel bound      = DEPTH×(CHUNK_MAX+FRAME_HDR_MAX) = 8×8208 = 65 664 B
RESP ceiling (×4)     = 4 × 65 664                      = 262 656 B
REQ  ceiling (×4)     = 4 × 65 664                      = 262 656 B   (req-side mirror, same const family)
proof body            = 4 MiB = 4 194 304 B  (≥8× margin: 4 194 304/262 656 ≈ 15.97×)
```
Ceiling helper copied from `resp_retained_ceiling(depth,chunk,hdr)`
(`h3_h1_resp_stream_e2e.rs:92`) so `test ceiling == gauge bound`
(`4 × DEPTH×(CHUNK_MAX+FRAME_HDR_MAX)`).

Cases:

1. `h2_e2e_get_response_byte_identical` — sanity: small GET, body +
   status byte-identical, clean FIN. (Liveness floor.)
2. **`h2_e2e_request_body_byte_identical_at_backend`** — sends a
   NON-EMPTY BINARY (non-UTF-8, pseudo-random) request body of ≥1 MiB
   over multiple H3 DATA frames; the real H2 backend captures the full
   received request body and the test asserts it equals the sent bytes
   **byte-for-byte**. This is the direct proof that the dropped-body
   defect is fixed (**binding condition 1**). Mirrors
   `h3_h1_stream_body_e2e.rs::t1_multi_data_frame_binary_body_forwarded_byte_identical`.
3. **`h2_e2e_response_memory_bounded_through_stalled_client`** — 4 MiB
   H2 response, H3 client STALLED mid-stream (reuse
   `drive_h3_response_client_stalled`, `h3_h1_resp_stream_e2e.rs:770`);
   sample `MAX_RETAINED_RESP_BYTES` at the stall and assert
   `≤ 262 656` AND, after client resume, the 4 MiB body arrives
   byte-identical with a clean FIN (non-vacuous: bound ≪ body ≥8×, plus
   liveness/no-corruption — mirrors R2).
4. **`h2_e2e_request_memory_bounded_through_stalled_upstream`** — 4 MiB
   binary request body, the H2 backend STALLS reading its request body;
   sample `MAX_RETAINED_BODY_BYTES` and assert `≤ 262 656` while the
   body is 16× that; then unblock the backend and assert byte-identical
   receipt (mirrors `h3_h1_stream_body_e2e.rs::t5_..._stalled_upstream`,
   :791). This is the request-direction non-vacuous memory proof.
5. **`h2_e2e_backpressure_stalled_client_pauses_h2_upstream_read`** —
   slow/stalled H3 client; assert the H2 backend's body-send progress
   stalls (the upstream observes its H2 send window not opening) while
   retained ≤ ceiling — proves the §2 causal chain end-to-end (mirrors
   `r3_slow_client_backpressures_upstream_read`,
   `h3_h1_resp_stream_e2e.rs:1206`).
6. `h2_e2e_upstream_reset_midbody_resets_client_no_fin` — H2 backend
   resets mid-body ⇒ client sees RESET_STREAM, never a clean FIN
   (response-splitting guard, `RespAbort::UpstreamReset` path).
7. **`h2_e2e_client_reset_midrequest_rsts_h2_upstream_no_truncated_request`**
   (BINDING — lead A2, request-side smuggling-parity, the request
   analog of case 6): the H3 client RESETs MID request body (peer QUIC
   RESET_STREAM before FIN ⇒ `ReqBodyEvent::Reset`). Assert the
   streaming request `BoxBody` errors ⇒ hyper RST_STREAMs that H2
   upstream stream, and the real H2 backend NEVER observes a
   silently-truncated request presented as complete (it sees a body
   stream error / no clean end-of-request, never a short-but-"complete"
   body). H2 multiplexing ⇒ a per-stream RST does not poison the
   connection (lead A2: no extra non-reusable bookkeeping).

All cases run, none `#[ignore]`d. The file MUST NOT contain a stale
"SCAFFOLD ONLY / `#[ignore]`d" comment block (we explicitly do not
repeat the H3→H1 mistake — see §5).

**Binding conditions satisfied:** (1) case 2 — non-empty binary request
body byte-identical at a real H2 backend; (2) cases 3+4 — non-vacuous
live-gauge memory proofs for BOTH directions under a stalled peer, plus
case 5 backpressure proof; (3) all gated/runnable via
`cargo test -p lb-quic --features test-gauges` and the gauges are the
real feature-compiled statics (a run WITHOUT the flag fails to compile
the memory cases — invalid gate, exactly as called out in
`s6-inventory.md:56`).

## 5. Integrity fix (owner binding condition 3)

`crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs:1048-1057` contains a
stale comment block asserting "R1..R8 real-wire tests — SCAFFOLD ONLY.
`#[ignore]`d ... builder-1 finalizes ... once P1-B is verifier-passed".
This is FALSE at the current tip: there are NO `#[ignore]` attributes
in the file and all 16 tests run and PASS (`s6-inventory.md:54,56`).

Fix (an increment of this plan): replace that comment block with an
accurate one — these R1..R8/C2/C3 tests RUN and PASS, are the H3→H1 R8
verification bar, and the `test-gauges` feature is REQUIRED for R2/R3
to compile (cite the same fact). No test code changed; comment-only.
This does NOT weaken/skip/ignore any test (R5-safe — it removes a false
"ignored" claim, the opposite of masking a passing test).

## 6. R8 self-check — why no path reintroduces full-body buffering

| Buffer | Bound | Body-size independent? |
|---|---|---|
| Inbound H3→bridge req chunk (M-A, unchanged) | 1 × `ReqBodyEvent` ≤ `H3_BODY_CHUNK_MAX` (8 KiB) + channel depth 8 | YES (proven by H3→H1 T5) |
| §1 hyper request `BoxBody` | streams from `brx`, one `Frame::data` per pulled `Chunk`; NO `.collect()`, NO pre-size, NO total cap | YES |
| §2 hyper response read | `body.frame().await` one frame at a time, split to ≤ 8 KiB, dropped after send; NO `Vec<u8>` accumulation, NO `.collect()` | YES |
| §2 response channel | `mpsc<RespEvent>(H3_RESP_CHANNEL_DEPTH=8)`, each ≤ `H3_RESP_CHUNK_MAX+H3_FRAME_HDR_MAX` | YES |
| Actor drain (unchanged) | existing `Progressive` `StreamTx` + gauge `record_resp_retained` | YES (proven by H3→H1 R2/R3) |

Every buffer is bounded by a FIXED in-flight window
(`depth × chunk`), never by total body size and never by a total-body
cap (the `MAX_REQUEST_BODY_BYTES`/`MAX_RESPONSE_BODY_BYTES` 64 MiB
values remain only as DoS abort thresholds, NOT as the memory bound —
same as H3→H1). The forbidden `.collect()` / `Limited::collect()` /
`decoded_body: Vec<u8>` / `Full::new(empty)` patterns are all removed
from the H3→H2 path and none is reintroduced. There is exactly ONE
`.collect()`-free streaming pipe per direction.

## 7. Increment breakdown (ordered, independently-committable, one owning builder each)

Each increment has its own scoped check (`cargo build/test -p lb-quic`,
disk-friendly — full `--workspace` reserved for the Phase 3 gate).

| # | Increment | Owner | Self-check |
|---|---|---|---|
| I0 | **Integrity fix** (§5): rewrite the stale `h3_h1_resp_stream_e2e.rs:1048` comment. Comment-only, zero behaviour change. | builder-1 | `cargo test -p lb-quic --features test-gauges --test h3_h1_resp_stream_e2e` still 16/16 PASS (regression-lock for R3) |
| I1 | **M-B response half**: add `stream_h2_response` (§2) + unit test of HEADERS/DATA/trailers re-encode + over-cap/reset mapping. No actor wiring yet. | builder-1 | `cargo test -p lb-quic` (new unit tests green; nothing else touched) |
| I2 | **M-B request half**: add `h2_request_body_from_rx` streaming `BoxBody` + `h3_to_h2_stream_resp` orchestrator (§1, §3). Old `h3_to_h2_roundtrip` kept temporarily (not yet called by actor). | builder-1 | `cargo build -p lb-quic`; unit tests for the body-driver framing/abort mapping |
| I3 | **Actor wiring**: swap the `poll_h3` H2 branch (:794) from `h3_to_h2_roundtrip` to the H1-shaped streaming spawn; delete now-dead `h3_to_h2_roundtrip`. | builder-1 | `cargo test -p lb-quic --features test-gauges` — H3→H1 suites still 22/22 (R3 no-regression); existing `proto_translation_e2e::proxy_h3_listener_h2_backend` still PASS |
| I4 | **Verification suite**: add `tests/h3_h2_stream_e2e.rs` (§4 cases 1–7, incl. BINDING case 7 per lead A2) incl. the real hyper H2 backend helper. | builder-1 | `cargo test -p lb-quic --features test-gauges --test h3_h2_stream_e2e` — all cases PASS, 0 ignored |
| I5 | **Phase gate**: full suite + clippy + fmt at the agreed gate point. | lead-assigned verifier | Phase-3 gate (`--workspace` reserved here only) |

I0 lands first (cheap, unblocks the integrity claim and re-locks R3).
I1→I2→I3 are sequential (I3 depends on both halves). I4 depends on I3.
Each is a single small commit on `s6/builder-1-h3h2` after lead approval.

## 8. Scope / size estimate & open questions

**Size: S–M** (matches `s6-inventory.md:91`). New code is one streaming
response fn + one streaming request-body adapter + one orchestrator
(all close clones of proven H3→H1 fns) + a ~6-case e2e file + a
comment fix. No new shared infra, no `lb-io` change, no protocol codec
change. Estimated ≈ 350–550 LoC src + ≈ 500–700 LoC test.
Risk concentrated in I4 (real H2 backend harness wiring) and the
backpressure timing of case 5 (mitigated by reusing the proven
`drive_h3_response_client_stalled` harness).

**Open questions — RESOLVED (lead, BINDING):**

- **A1 (Q1):** APPROVED — entire M-B delta stays in `lb-quic`, no
  `lb-io`/codec change. If implementation reveals one is required,
  STOP and re-request approval (scope change).
- **A2 (Q2):** Pool eviction on Send/Timeout is sufficient for
  connection-level faults; the erroring request `BoxBody` ⇒ hyper
  per-stream RST_STREAM is the correct + sufficient H2-equivalent
  partial-request guard (H2 multiplexing ⇒ per-stream RST does not
  poison the connection). NO extra non-reusable bookkeeping. Adds
  BINDING verification case 7 (above) + into increment I4.
- **A3 (Q3):** APPROVED — dropping request trailers on the H2 leg is
  in-scope-out for S6 (parity with H3→H1 P1-C). MUST stay documented
  in the code comment AND listed in the S6 report as scoped-out.
- **A4 (Q4):** No pre-approval micro-commit. I0 is the first
  post-approval increment, as sequenced.

---

**Original open questions (for the record):**

- **Q1.** Confirm M-B requires NO `lb-io` change:
  `Http2Pool::send_request` already returns a streaming
  `Response<Incoming>` and only awaits the header roundtrip, and a
  streaming `BoxBody<Bytes,hyper::Error>` request is already the
  pool's body type (`http2_pool.rs:102,210`). Plan assumes the entire
  M-B delta is in `lb-quic`. Approve?
- **Q2.** C2/pooled-connection discipline: the H1 path marks the
  pooled TCP non-reusable on every abort (`h3_bridge.rs:1980`). The H2
  pool instead **evicts the multiplexed peer entry on any
  Send/Timeout error** (`http2_pool.rs:222`). Plan treats that pool
  eviction as the H2-equivalent guard and adds NO extra non-reusable
  bookkeeping. Confirm acceptable, OR specify a desired explicit
  per-stream poison signal for partial-request aborts (e.g. force
  eviction when the streaming request body errors mid-flight so a
  half-sent H2 request can't share the connection).
- **Q3.** Request-trailer handling on the H2 leg: plan DROPS request
  trailers (parity with the H3→H1 P1-C decision, documented, body
  still correctly framed). Confirm dropping (vs. forwarding H2 request
  trailers) is in-scope-out for S6.
- **Q4.** Should I0 (the integrity comment fix) be split into its own
  pre-approval micro-commit since it is comment-only and independently
  R5-safe, or stay sequenced inside this plan's increment series after
  approval? (Plan currently sequences it as I1-of-implementation, post
  lead approval, to honour "no code/test change before approval".)
