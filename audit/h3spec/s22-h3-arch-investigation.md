# S22 — HTTP/3 Architecture Investigation: hand-rolled lb-h3/conn_actor vs `quiche::h3`

**Date:** 2026-05-31 · **Scope:** post-v1 architecture decision · **Source-only, no source files modified.**
**quiche:** `quiche-0.28.0` · **tokio-quiche:** `tokio-quiche-0.18.0` (both in `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`)

> Every claim below is grounded in code with `file:line`. Registry paths are abbreviated:
> `QH = ~/.cargo/.../quiche-0.28.0/src/h3` · `TQ = ~/.cargo/.../tokio-quiche-0.18.0/src`.

---

## Executive summary

The gateway terminates HTTP/3 on a **raw `quiche::Connection`** (`crates/lb-quic/src/router.rs:351` `quiche::accept_with_retry`) and layers its **own** H3/QPACK stack on top: `crates/lb-h3` (framing + a static-table QPACK) and `crates/lb-quic/src/conn_actor.rs::poll_h3` + `h3_bridge.rs` (request-stream state machine + the H3↔backend bridge). `quiche 0.28` **already ships a complete, RFC-9114/9204-enforcing `quiche::h3::Connection`** (`QH/mod.rs`, 7737 lines) that wraps an existing `quiche::Connection` in place (`h3::Connection::with_transport`, `QH/mod.rs:1060`) and self-manages the control + QPACK encoder/decoder streams. The workspace also already depends on **`tokio-quiche 0.18`, which ships `ServerH3Driver`/`H3Driver`** (`TQ/lib.rs:121`) — a proxy-shaped, bounded-channel H3 driver — **but the gateway does not use it** (only `ConnectionParams` is re-exported, see §2).

The 15 H3/QPACK findings (#11–25) are almost entirely **gateway re-implementation gaps in code that `quiche::h3` ships and gets right** — including the §4.5.6 QPACK literal-literal-name bug, which `quiche::h3` encodes/decodes correctly (`QH/qpack/encoder.rs:77`, `QH/qpack/decoder.rs:134-135`). The genuinely proxy-specific surface is the **H3↔backend bridge** (the bounded-incremental relay with end-to-end backpressure) — and even that has a near-exact analogue in tokio-quiche's `H3Driver` (`TQ/http3/driver/mod.rs:187-197`, bounded `mpsc` body streams).

**Recommendation (C):** post-v1, **migrate the H3-terminate front to `quiche::h3::Connection`** (the lower-risk option), or to **tokio-quiche `ServerH3Driver`** (more ambitious). Either deletes lb-h3 + the `poll_h3` frame/QPACK/control-stream machinery and closes findings #11, #16–25 by construction. #12–15 (pseudo-header validation) stay the gateway's job either way (both libraries hand the app a raw header list and say "the application should validate"), but the **interop corruption that masked #14/#15 disappears** because the QPACK decode is correct. #1–10 are quiche-transport and unaffected by any H3 migration.

---

## 1. `quiche::h3` (0.28) module surface — what it provides and enforces

### 1.1 Construction & event loop
- **`h3::Config`** with `set_max_field_section_size` / `set_qpack_max_table_capacity` / `set_qpack_blocked_streams` / `enable_extended_connect` / `set_additional_settings` (`QH/mod.rs:596,603,610,617,644`).
- **`h3::Connection::with_transport(conn: &mut quiche::Connection, config)`** (`QH/mod.rs:1060`) — wraps an **existing** `quiche::Connection` (exactly what the gateway has from `quiche::accept_with_retry`). On construction it **sends SETTINGS and opens the local control + QPACK encoder/decoder streams itself**: `send_settings` (`:1072`), `open_qpack_encoder_stream` / `open_qpack_decoder_stream` (`:1083-1084`).
- **`poll<F>(conn) -> Result<(u64, Event)>`** (`QH/mod.rs:2030`) — the single driver call. `Event` (`QH/mod.rs:755-804`): `Headers { list, more_frames }`, `Data`, `Finished`, `Reset(u64)`, `PriorityUpdate`, `GoAway`.
- **Send/recv:** `send_response` (`:1188`), `send_response_with_priority` (`:1244`), `send_additional_headers` (`:1300`, trailers), `send_body` (`:1511`), `send_body_zc` (`:1552`), `recv_body` (`:1761`), `recv_body_buf` (`:1804`), `send_goaway` (`:2133`), `take_last_priority_update` (`:1983`), `peer_settings_raw` (`:2193`).

### 1.2 RFC error conditions enforced INTERNALLY by `poll()` (it calls `conn.close(true, code, …)` itself)
`process_readable_stream` (`QH/mod.rs:2520`) reads the **client unidirectional control / QPACK / push streams** itself and enforces:

| RFC condition | Wire code | Citation |
|---|---|---|
| Second control stream | StreamCreationError 0x103 | `QH/mod.rs:2571-2578` |
| Server received a push stream | StreamCreationError | `:2596-2603` |
| Second QPACK encoder / decoder stream | StreamCreationError | `:2609-2618`, `:2630-2639` |
| **Critical stream (control/QPACK) closed** → **#24** | ClosedCriticalStream 0x104 | `close_conn_if_critical_stream_finished` `QH/mod.rs:933-944`, called at `:2587,2620,2641` |
| Bad stream-type set | (varies) `e.to_wire()` | `:2544-2546` |

The **per-stream frame-type FSM** `Stream::set_frame_type` (`QH/stream.rs:268`) enforces (and `poll` closes the connection on the returned error at `QH/mod.rs:2682-2706`):

| RFC condition (h3spec #) | Result | Citation |
|---|---|---|
| First control frame ≠ SETTINGS → **#16** | `MissingSettings` (0x10a) | `QH/stream.rs:284` |
| Second SETTINGS → **#19** | `FrameUnexpected` (0x105) | `:287-288` |
| DATA / HEADERS on control stream → **#17/#18** | `FrameUnexpected` | `:292-296` |
| **DATA before HEADERS on a request stream → #11** | `FrameUnexpected` | `:319-320` |
| **CANCEL_PUSH on a request stream → #21** | `FrameUnexpected` | `:340-341` |
| SETTINGS/GOAWAY/MAX_PUSH/PRIORITY_UPDATE on a request stream | `FrameUnexpected` | `:343-356` |

`process_frame` (`QH/mod.rs:2867`) enforces the rest:
- **H2 SETTINGS ids (0x0/0x2/0x3/0x4/0x5) → #20** `H3_SETTINGS_ERROR` (0x109): `QH/frame.rs:622-623` (SETTINGS parse) and the H3-datagram TP mismatch at `QH/mod.rs:2918-2924`.
- **QPACK field-section decode failure → #22** `QPACK_DECOMPRESSION_FAILED` (0x200): the HEADERS arm maps a `qpack::Error` to `Error::QpackDecompressionFailed` and `conn.close(true, …)` at `QH/mod.rs:2957-2967`.
- GOAWAY / MAX_PUSH_ID id rules → `FrameUnexpected`/`IdError` (`:3013-3060`).

All of these calls use **`conn.close(true, code, …)`** — `app=true` = an **application** CONNECTION_CLOSE (frame 0x1d) — which is exactly the lesson the gateway learned the hard way this session (`s22-findings.md:145`: an `app=false` close put the H3 code in the transport error space and h3spec rejected it). `quiche::h3` already does it right.

### 1.3 What `quiche::h3` does NOT do — pseudo-header validation (#12–15)
`Event::Headers.list` is the **raw** decoded header list; the doc states: *"The application should validate pseudo-headers and headers."* (`QH/mod.rs:758-759`). `process_frame`'s HEADERS arm decodes QPACK and returns the list **without** §4.3/§4.3.1 pseudo-header checks (`QH/mod.rs:2929-3006`). So **#12–15 are NOT auto-fixed by migrating to `quiche::h3`** — the gateway would keep `validate_request_pseudo_headers` (`h3_bridge.rs:890`). (`Error::MessageError` 0x10e exists at `QH/mod.rs:416,471` but is the app's to raise.)

### 1.4 QPACK §4.5.6 — `quiche` gets it RIGHT (the migration-evidence headline)
RFC 9204 §4.5.6 "Literal Field Line with Literal Name": the first byte is `001NH` + a **3-bit-prefix** name length on that same byte; H (0x08) is the Huffman bit.
- **quiche encoder** (`QH/qpack/encoder.rs:77`): `encode_str::<true>(h.name(), LITERAL, 3, &mut b)` — `LITERAL = 0b0010_0000` (`QH/qpack/mod.rs:34`), **prefix = 3**, name length in the first byte (with the Huffman bit set by `encode_str` at `:157`). Correct.
- **quiche decoder** (`QH/qpack/decoder.rs:134-135`): `name_huff = b.as_ref()[0] & 0x08 == 0x08; name_len = decode_int(&mut b, 3)?` — reads the Huffman bit and the **3-bit prefix from the same first byte**. Correct.
- **gateway's pre-S22 bug:** the lb-h3 codec wrote `0x20` then a **separate 7-bit-prefix length byte** for the name, and the decoder did `pos += 1` then read a fresh 7-bit length — non-conformant in **both** directions (self-consistent, so internal round-trips + h2spec passed, but every conformant peer mis-decoded). Fixed in S22 to `encode_qint(&mut buf, name.len(), 3, 0x20)` / `decode_qint(slice, 3)` (`crates/lb-h3/src/qpack.rs:256` and `:344-372`) — i.e. the gateway *manually reproduced* what quiche already had. **A `quiche::h3` build would never have had this bug.**

`quiche`'s QPACK also supports **Huffman** name/value coding (encoder `QH/qpack/encoder.rs:155-158`, decoder `:134,246`); the gateway's codec is **raw-only** (`crates/lb-h3/src/qpack.rs:187-189` always emits a raw string; `decode_qstring` `:192` reads raw) — this is the open carry-forward **CF-S22-QPACK-HUFFMAN** (`s22-findings.md:157`), which migration also closes.

`quiche` enforces **QPACK encoder/decoder unidirectional stream** opening rules (single-stream, critical-stream-close) in `process_readable_stream` (§1.2). It uses a **static-table-only** QPACK (no dynamic table — same simplification as the gateway), so **#23 (encoder-stream dyn-table capacity > limit) and #25 (decoder-stream Insert Count Increment = 0)** are *instruction-stream* checks neither implementation performs on the wire today; `quiche` does at least *open and track* those streams (so it would not silently ignore them the way the gateway does — but it does not fully validate their contents in 0.28).

---

## 2. tokio-quiche 0.18 `H3Driver` — shipped, proxy-shaped, but UNUSED by the gateway

**It exists and is proxy-shaped.** `TQ/lib.rs:119,121` re-export `ClientH3Driver` / `ServerH3Driver`; the driver lives in `TQ/http3/driver/{mod,server,client,streams,connection}.rs`. It implements `ApplicationOverQuic` and drives a `quiche::h3::Connection` for you. Its inbound/outbound model is **exactly the gateway's hand-rolled shape**:
- `H3Event::IncomingHeaders(IncomingH3Headers)` (`TQ/http3/driver/mod.rs:231,221`) hands the app, per request: `stream_id`, `headers: Vec<h3::Header>` (raw), a `send: OutboundFrameSender` for the response body, and a `recv: InboundFrameStream` for the request body (`TQ/http3/driver/mod.rs:187-202`).
- `InboundFrameStream = mpsc::Receiver<InboundFrame>` (`TQ/http3/driver/mod.rs:131`); `OutboundFrameSender = PollSender<OutboundFrame>` (`:119`). Streams are created with **bounded** `mpsc::channel(capacity)` (`TQ/http3/driver/streams.rs:74,184`) and expose a backpressure future that *"resolves when `send` has capacity again"* (`TQ/http3/driver/streams.rs:95`). This is the **same bounded-channel + capacity-gate end-to-end-backpressure design** the gateway built by hand in `conn_actor`/`h3_bridge` (the `H3_BODY_CHANNEL_DEPTH`/`H3_RESP_CHANNEL_DEPTH` mpsc channels, `conn_actor.rs:53,938-940`).
- `H3Event::{ResetStream, ConnectionShutdown}` (`TQ/http3/driver/mod.rs:245,250`) cover the abort/teardown control the proxy needs.

**It is NOT used.** The only tokio-quiche reference in gateway code is `pub use tokio_quiche::ConnectionParams;` (`crates/lb-quic/src/lib.rs:135`), and `ConnectionParams` has **no real-value usage anywhere** in `crates/` (grep finds only the re-export line; the `.listen()` at `crates/lb/src/main.rs:2620` is the **TCP** listener via `lb-io`, not QUIC). The QUIC/H3 front is driven entirely by **raw quiche** (`router.rs:351` `quiche::accept_with_retry`; `conn_actor.rs::poll_h3`). So **tokio-quiche 0.18 is effectively a transitive/config-only dependency** for the H3-front path — the workspace pays the dep cost but uses none of its H3 driver. (`Cargo.toml:96`, `crates/lb-quic/Cargo.toml:45,90`.)

---

## 3. What lb-h3 / conn_actor reimplements that `quiche::h3` already provides

| Concern | Gateway (hand-rolled) | quiche::h3 equivalent | Verdict |
|---|---|---|---|
| **Frame codec** | `lb-h3/src/frame.rs` `decode_frame`/`encode_frame` (`:84`, `:159`), `H3Frame` enum (`:19`) | `QH/frame.rs` `Frame::{to_bytes,parse_*}`, `QH/stream.rs` FSM | **Duplicate.** lb-h3's `decode_frame` requires the **whole payload buffered** (`frame.rs:101` returns `Incomplete` until `buf[header..total]` exists) — the reason the body relay must *manually* parse frame headers (`h3_bridge.rs:670` `try_parse_frame_header`). quiche::h3 streams DATA natively via `recv_body` (§4). |
| **QPACK** | `lb-h3/src/qpack.rs` static-table `QpackEncoder`/`QpackDecoder`, raw-only, **had the §4.5.6 bug** | `QH/qpack/{encoder,decoder,static_table}.rs`, **§4.5.6 correct + Huffman** | **Duplicate + was buggy.** §1.4. |
| **Request-stream state machine** | `conn_actor.rs::poll_h3` (`:789`) + `StreamRxBuf::feed`/`feed_body` (`h3_bridge.rs:419`,`:494`) + `FeedError` (`:373`); frame-sequencing guards (`:445-464`) | `QH/mod.rs::poll` (`:2030`) + `process_readable_stream` (`:2520`) + `Stream::set_frame_type` (`stream.rs:268`) | **Duplicate.** The gateway re-implements §4.1/§7.2 sequencing (this session, for #11/#21). |
| **Control + QPACK uni-streams** | **Not read** — `poll_h3` drains+discards every client uni stream (`conn_actor.rs:851-855`, `if sid % 4 != 0`) | `process_readable_stream` parses + state-machines them (`QH/mod.rs:2569-2647`) | **Missing in gateway** (root cause of #16–20/#24). |
| **App error-code close** | `reset_h3_stream` (`conn_actor.rs:751`) + per-finding `conn.close(true, CODE, …)` (`:1173,1193`), with hand-defined constants (`:61,91,100,108`) | `poll`/`process_*` call `conn.close(true, e.to_wire(), …)` internally | **Duplicate** (gateway re-derived `app=true` + the codes; §1.2). |

**Genuinely NOT in `quiche::h3` (proxy-specific — see §A):** the H3↔backend bridge (`h3_bridge.rs` `h3_to_h2_stream_resp` `:2733`, `h3_to_h3_stream_resp` `:3312`, `stream_h1_response` `:1560`), the bounded-incremental request-body/response relay with the §1.4.3 backpressure gate (`conn_actor.rs:402`), the per-stream bounded channels + `body_pending` flush (`conn_actor.rs:53,202-254,819-822`), the retained-bytes memory gauge (`h3_bridge.rs:699-742`), and the conn_actor↔backend wiring (H3→H1/H2/H3 cell selection). These have a tokio-quiche-`H3Driver` analogue (§2) but **no `quiche::h3`-only analogue** — quiche::h3 gives you `recv_body`/`send_body` primitives, not a relay.

---

## 4. WHY hand-rolled? (confirmed from code, audit docs, git log)

- **(a) Predates tokio-quiche `H3Driver` adoption — CONFIRMED as the operative reason.** The first H3 bridge landed at `cf045248` "Pillar 3b.3c-2: InboundPacketRouter + actor + H3 bridge" (git log, earliest H3 commit), wiring `quiche::accept` directly. The dep comment (`Cargo.toml:90-95`, `docs/decisions/quinn-to-quiche-migration.md`) describes a **quinn→quiche transport** migration; tokio-quiche was pulled in for `ConnectionParams`/listener config, and the H3 layer was hand-rolled on raw quiche rather than adopting `ServerH3Driver`. There is **no code comment or audit decision anywhere justifying the hand-roll over `quiche::h3`/`H3Driver`** — it was simply the path taken at 3b.3c and then built out cell-by-cell across S1–S13 (the H-matrix). This is the classic **missed-reuse (c)** outcome: each H-matrix session deepened the hand-rolled stack (e.g. S2 `f2af73c4` "incremental H3->H1 request-body streaming") instead of switching to the library.
- **(b) Proxy-shape needs — REAL, but does NOT require hand-rolling the H3 protocol.** The R8 bounded-incremental relay + end-to-end backpressure + the H3→H1/H2/H3 bridge are genuine and proxy-specific (§A). But they sit **above** the H3 frame/QPACK/control-stream layer and consume `recv_body`/`send_body` (quiche::h3) or `Inbound/OutboundFrameStream` (H3Driver) — neither library forces a buffering shape (see (4)/§4-streaming below). The proxy shape justifies a custom **bridge**, not a custom **frame codec / QPACK / control-stream FSM**.
- **(4) Does `quiche::h3` support CHUNKED body streaming + backpressure? — YES (refutes the stated reason for manual frame parsing).** The memory note "lb_h3::decode_frame buffers whole payload" is a limitation of the **gateway's own** `lb-h3::decode_frame` (`frame.rs:101`), **not** of `quiche::h3`:
  - **recv:** `recv_body(conn, sid, out: &mut [u8]) -> Result<usize>` fills a **caller-sized** buffer and returns bytes read; `recv_body_buf` loops `while out.has_remaining_mut()` and is *"inherently limited by how much data is in quiche's receive buffer"* (`QH/mod.rs:1761,1804-1864`). The application picks the chunk size and the read cadence → **bounded-incremental by construction**, and not reading = the QUIC flow-control window is not extended = the peer is paused (the same backpressure mechanism the gateway notes at `conn_actor.rs:826-833`).
  - **send:** `send_body(conn, sid, body, fin) -> Result<usize>` returns the bytes actually written and **`Done`/partial on a blocked stream**, with the doc instructing retry-when-writable (`QH/mod.rs:1497-1532`). That is end-to-end backpressure on the egress.
  - So `quiche::h3` **can** express the proxy streaming shape; the manual frame-header parsing in `h3_bridge.rs:670` exists only to work around lb-h3's own whole-payload `decode_frame`, which would be deleted by migration.

---

## 5. Would migrating fix the S22 findings? (per-finding)

Legend: **Y** = handled natively by `quiche::h3` (and `H3Driver`, which wraps it); **App** = library hands you a raw header list, the gateway still validates (but the QPACK *decode* is correct, so no interop corruption); **N/A** = not an H3-layer finding.

| # | Condition | quiche::h3 native? | Evidence |
|---|---|---|---|
| 1–8 | Transport-param CONNECTION_CLOSE suppression | **N/A** (quiche transport; CF-QUICHE-UPGRADE) | `s22-findings.md:67-73` |
| 9–10 | Header reserved bits | **N/A** (quiche transport; header-protected) | `s22-findings.md:75-76` |
| 11 | DATA before HEADERS → FRAME_UNEXPECTED | **Y** | `QH/stream.rs:319-320` + close at `QH/mod.rs:2682-2706` |
| 12 | Duplicate pseudo-header | **App** | `QH/mod.rs:758-759`; gateway keeps `validate_request_pseudo_headers` (`h3_bridge.rs:907-928`) |
| 13 | Missing mandatory pseudo-header | **App** | same |
| 14 | Prohibited/unknown pseudo-header | **App** (but the §4.5.6 QPACK *decode* of the literal name is **Y/correct**) | `QH/qpack/decoder.rs:134-135`; gateway check `h3_bridge.rs:933-935` |
| 15 | Pseudo-header after regular field | **App** | gateway check `h3_bridge.rs:902-904` |
| 16 | First control frame ≠ SETTINGS → MISSING_SETTINGS | **Y** | `QH/stream.rs:284` |
| 17 | DATA on control stream → FRAME_UNEXPECTED | **Y** | `QH/stream.rs:292-293` |
| 18 | HEADERS on control stream → FRAME_UNEXPECTED | **Y** | `QH/stream.rs:295-296` |
| 19 | Second SETTINGS → FRAME_UNEXPECTED | **Y** | `QH/stream.rs:287-288` |
| 20 | H2 SETTINGS ids → SETTINGS_ERROR | **Y** | `QH/frame.rs:622-623` |
| 21 | CANCEL_PUSH on request stream → FRAME_UNEXPECTED | **Y** | `QH/stream.rs:340-341` |
| 22 | Invalid static index → QPACK_DECOMPRESSION_FAILED | **Y** | `QH/mod.rs:2957-2967` |
| 23 | Encoder-stream dyn-table capacity > limit | **Partial** — quiche *opens/tracks* the encoder stream (`QH/mod.rs:2607-2626`) but 0.28 does not fully validate its instruction contents (static-only) | — |
| 24 | Critical stream closed → CLOSED_CRITICAL_STREAM | **Y** | `QH/mod.rs:933-944,2587,2620,2641` |
| 25 | Decoder-stream Insert Count Increment = 0 | **Partial** — decoder stream opened/tracked (`QH/mod.rs:2628-2647`), instruction not fully validated in 0.28 | — |

**Tally for an H3 migration (#11–25):** **Y = 11** (#11, #16–22, #24 — i.e. all the carried-to-S23 control-stream/frame-sequencing/QPACK-error findings, by construction); **App = 4** (#12–15, still validated by the gateway, but with correct QPACK so #14/#15's interop corruption vanishes); **Partial = 2** (#23/#25, improved — streams tracked not ignored — but not fully enforced in quiche 0.28). The **§4.5.6 QPACK bug (the most valuable thing this pass found, `s22-findings.md:134`) is code `quiche::h3` gets right** — direct migration evidence.

---

## A. JUSTIFIED hand-rolled surface (genuinely proxy-specific — KEEP)

These have **no `quiche::h3`-only equivalent** (quiche gives primitives, not a relay); they ARE the gateway's value-add:
- The **H3↔backend bridge**: `h3_to_h2_stream_resp` (`h3_bridge.rs:2733`), `h3_to_h3_stream_resp` (`:3312`), `stream_h1_response` (`:1560`) — cross-protocol request/response translation, trailer re-encoding (`:1981`), authority sanitisation parity with H1/H2 (`conn_actor.rs:903-917`).
- The **bounded-incremental relay + end-to-end backpressure**: the §1.4.3 response gate (`conn_actor.rs:402-456`), per-stream bounded `mpsc` body/response channels (`conn_actor.rs:53,938-940`), `body_pending` flush (`:819-822`), the retained-bytes memory proof gauge (`h3_bridge.rs:699-742`, the R8 bound).
- The **conn_actor↔backend wiring**: H3→H1/H2/H3 cell selection, the `QuicUpstreamPool`/`Http2Pool`/`TcpPool` dial paths, and the actor's lifecycle/cleanup.

*(Caveat: tokio-quiche's `H3Driver` provides a near-identical bounded-`mpsc` body-stream model (§2), so even this surface is "proxy-specific" only relative to bare `quiche::h3`; it is not novel relative to `H3Driver`.)*

## B. SHOULD-HAVE-BEEN-LIBRARY surface (DELETE on migration)

Everything in §3's "Duplicate"/"Missing" rows — i.e. **all of `crates/lb-h3`** (frame codec 329 LOC + QPACK 515 LOC + varint/security) and the **protocol-layer half of `poll_h3`/`h3_bridge`**: `StreamRxBuf::feed`/`feed_body` + `FeedError` + frame-sequencing guards + `try_parse_frame_header` + `reset_h3_stream` + the H3 error-code constants + the uni-stream drain. This is framing + QPACK + control-stream state machine + RFC error enforcement that `quiche::h3` ships (§1) and the gateway re-derived — including the bug.

## C. Migration recommendation (post-v1)

**Recommended: migrate the H3-terminate front to `quiche::h3::Connection` (Option C1, lower risk).** The gateway already holds a raw `quiche::Connection`; `h3::Connection::with_transport(conn, &h3_config)` (`QH/mod.rs:1060`) drops the H3 layer on in place and auto-opens the control/QPACK streams. Replace `poll_h3`'s frame-decode loop with `h3_conn.poll(conn)` → match `Event::{Headers,Data,Finished,Reset,GoAway}`; keep the bridge (§A) consuming `recv_body`/`send_body`; keep `validate_request_pseudo_headers` for #12–15.

- **Buys:** closes #11, #16–22, #24 by construction (11 findings), improves #23/#25, and eliminates the §4.5.6 interop corruption + CF-S22-QPACK-HUFFMAN (gains Huffman). Deletes ~1.2k LOC of duplicated, security-sensitive protocol code (all of lb-h3 + the protocol half of `poll_h3`/`h3_bridge`).
- **Risk/effort:** Medium-high. The bridge (§A) is the irreducible work — it must be re-pointed from `StreamRxBuf` to `recv_body`/`send_body` while preserving the proven R8 bound + backpressure (re-run the H-matrix memory gauges + the soak). `h3_bridge.rs` is 5719 LOC and deeply entangled with `StreamRxBuf`; this is a multi-session rewrite of the H3-front, requiring the full ×3 gate + re-soak + a fresh h3spec pass. Mode A/B passthrough (raw QUIC relay) is untouched (it never parses H3).
- **Alternative C2 (more ambitious): adopt tokio-quiche `ServerH3Driver`** (`TQ/lib.rs:121`) — already a workspace dep (§2). Its `IncomingH3Headers` bounded-`mpsc` body model (§2) maps almost 1:1 onto the gateway's existing channel design, so the bridge changes are smaller, but it also subsumes the listener/router/conn-actor loop (a bigger blast radius on the accept path). Higher reward (deletes more, including parts of `conn_actor`/`router`), higher risk.
- **NOT recommended:** keep-hand-rolled. The only standing argument was the proxy streaming shape, and §4-(4) refutes it: `quiche::h3` `recv_body`/`send_body` express chunked streaming + backpressure. Continuing to hand-roll means continuing to carry #16–25 *and* re-deriving RFC enforcement by hand (the §4.5.6 bug is the cautionary tale).

**Pre-condition / sequencing:** this is a post-v1 item and is **orthogonal to CF-QUICHE-UPGRADE** (the #1–10 transport bump, `s22-findings.md:106`) — but a quiche bump would land first anyway, and a newer quiche may also tighten #23/#25 on the QPACK instruction streams. Sequence: (1) ship v1 with the hand-rolled stack + the S22 fixes already landed (#11–15, #22); (2) CF-QUICHE-UPGRADE evaluation; (3) the H3-front migration here, gated on a green h3spec re-run proving #16–24 close.
