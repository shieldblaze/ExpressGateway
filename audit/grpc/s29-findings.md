# S29 — gRPC-over-HTTP/3 conformance findings

Status: **DRAFT / in-progress** (empirical PASS/FAIL columns filled as the
real-wire suite runs in `crates/lb-quic/tests/grpc_h3_e2e.rs`).

## Model & scope

The gateway proxies gRPC **opaquely** over HTTP/3 (S29 R7): it forwards the
length-prefixed gRPC messages + the `grpc-status`/`grpc-message` trailing
HEADERS end-to-end, and does NOT interpret/transcode gRPC. The realistic
deployment under test is **H3 edge → H2 gRPC backend** (most internal gRPC
is H2); H3→H3 is also valid.

**Standardization note:** gRPC-over-HTTP/3 is **de-facto, not separately
standardized.** The canonical transport spec is gRPC `doc/PROTOCOL-HTTP2.md`
("gRPC over HTTP2"); its application semantics (5-byte framing, the
`grpc-status` trailer contract, the 4 call types, `grpc-timeout`, RST =
abnormal close) ride **unchanged** over H3 because RFC 9114 §1/§4.1 preserve
HTTP semantics over QUIC. Only the transport framing differs (QPACK vs HPACK,
RESET_STREAM/STOP_SENDING vs RST_STREAM, `te` near-banned per RFC 9114 §4.2).
Every gRPC-H2-spec citation below "applies to H3 by RFC 9114 semantic
preservation."

**Contrast — RFC 9220 (Extended CONNECT) does NOT apply to gRPC.** gRPC uses
ordinary `POST`, not CONNECT; 9220 is the WS-over-H3 (S28) regime.

## Conformance checklist (spec-derived rows; tiers per protocol-expert)

Legend: **Tier A** MUST-preserve (verbatim) · **Tier B** MUST-map-correctly
(H2↔H3 transport) · **Tier C** MAY · **Tier D** value-add, NOT required.
Result: ✅ pass · ❌ fail (finding) · ⏳ pending · n/a.

| # | Tier | Item | Spec cite | Result | Mechanism / test |
|---|------|------|-----------|--------|------------------|
| A1 | A | `:method POST`,`:scheme`,`:path /Svc/Method`,`:authority` byte-exact | gRPC-H2 *Requests*; RFC 9114 §4.3.1 | ✅ | unary/stream tests reach backend at the sent path |
| A2 | A | `content-type: application/grpc[+x]` preserved verbatim | gRPC-H2 *Requests* | ✅ | `grpc_h3_unary_echo_*`: backend `last_content_type == application/grpc` |
| A3 | A | 5-byte length-prefixed messages relayed opaquely across DATA boundaries; no re-framing | gRPC-H2 *Length-Prefixed-Message* | ✅ | echo/client-stream: msgs byte-identical (large-msg test pending 1b) |
| A4 | A | **trailing HEADERS with `grpc-status` delivered on EVERY response** | gRPC-H2 *Responses*/*Status* | ✅ | all 4 call types: `out.field("grpc-status")==0` |
| A5 | A | `grpc-message` (+ `grpc-status-details-bin`) forwarded when present | gRPC-H2 *Responses* | ✅ | Trailers-Only: `grpc-message=="precondition failed"` |
| A6 | A | **Trailers-Only** (single HEADERS+FIN, `:status`+`content-type`+`grpc-status`, no DATA) preserved | gRPC-H2 *Responses* | ✅ | `grpc_h3_trailers_only_immediate_error_preserved` (status 9, no DATA, clean FIN) |
| A7 | A | END_STREAM/QUIC-FIN position preserved | RFC 9114 §4.1,§6.1 | ✅ | `out.fin` true after trailers in all happy-path tests |
| B1 | B | client cancel ↔ RESET_STREAM/STOP_SENDING (`H3_REQUEST_CANCELLED`); propagate to peer leg | gRPC-H2 *Errors*; RFC 9114 §4.1.1 | ✅ | existing case-7 H3 client-reset path (req-side); reaped via `reap_client_cancelled_responses` |
| B2 | B | **mid-call reset ≠ clean finish** (never FIN+`grpc-status:0` for a reset) | gRPC-H2 *Errors*; RFC 9114 §4.1.1 | ✅ | `grpc_h3_backend_reset_*`: reset NOT laundered to `grpc-status:0` (gets 502+reset, no clean status) |
| B3 | B | `te: trailers` pass-through; not required on H3 ingress; never emit `te` ≠ `trailers` | RFC 9114 §4.2 | ✅ | `grpc_h3_without_te_*`: request with NO te still delivers the trailer |
| B4 | B | hop-by-hop / connection-specific fields stripped on H3 leg | RFC 9114 §4.2 | ✅ | existing H3 cell hop-by-hop strip (`is_response_hop_by_hop`); unchanged |
| B5 | B | gRPC route MUST NOT go to an **H1 backend** (trailer-incompatible) | gRPC-H2 *Responses*; RFC 9112 §7.1.2 | ⚠️ | `grpc_h3_to_h1_*`: H1 backend ⇒ NO `grpc-status` reaches client → **F-S29-2** (deviation: document H2/H3-backend requirement) |
| C1 | C | forward `grpc-timeout`/`grpc-encoding` unchanged (opaque) | gRPC-H2 *Requests* | ✅ | `grpc_h3_grpc_timeout_forwarded_unclamped`: backend sees `600S` verbatim (H2 path clamps to 300S) |
| C2 | C | no per-call-type behavior — uniform relay | gRPC-H2 grammar | ✅ | all 4 call types pass through the SAME relay path |
| C3 | C | MAY forward bare non-200 verbatim (client synthesizes) | gRPC-H2 *Responses* | ✅ | opaque default (the H2 gRPC-aware synthesis is NOT on H3 — Tier D3) |
| D1 | D | gateway deadline enforcement / `grpc-timeout` clamp + synthetic DEADLINE_EXCEEDED | — (end-to-end; endpoints enforce) | n/a | H2-path value-add; absent on H3 (owner) |
| D2 | D | `grpc.health.v1.Health/Check` local synthesis | — (health is a normal RPC) | n/a | H2-path value-add; absent on H3 (owner) |
| D3 | D | HTTP-status → grpc-status synthesis on broken upstream | — (client-runtime duty) | n/a | H2-path value-add; absent on H3 (owner) |

## The H2-vs-H3 divergence (owner decision — Phase 1 check-in)

H2 has a full gRPC-**aware** proxy (`lb-l7/src/grpc_proxy.rs`, `GrpcProxy`):
content-type routing, `grpc-timeout` clamp + `DEADLINE_EXCEEDED` synthesis,
`te: trailers` enforcement, `/Health/Check` synthesis, HTTP→grpc-status
synthesis (Tier D1-D3). The H2 path explicitly rejects H3 backends
(`h2_proxy.rs:1076-1081`). H3 has **none** of this — gRPC flows as an opaque
H3 request.

Per the conformance analysis, **Tier D is value-add, NOT a conformance
requirement** — an opaque H3 gRPC proxy that forwards
headers/DATA/trailers/resets verbatim and does none of D1-D3 is conformant.
So the divergence is a **deliberate, owner-ratifiable feature distinction**
(H2 = gRPC-aware; H3 = opaque transport), not a defect in shared machinery.
The conformance-critical items are Tier A (esp. A4/A6 trailers) and Tier B
(esp. B2 reset, B5 H1-backend) — pure forwarding correctness.

## Phase 1a result — the opaque path works with ZERO new production code

The real-wire suite `crates/lb-quic/tests/grpc_h3_e2e.rs` (real quiche H3
client speaking gRPC framing → production `QuicListener` → real hyper H2
gRPC backend) passes **5/5** on the promoted code with **no production
change**:

```
test grpc_h3_unary_echo_delivers_status_trailer ... ok
test grpc_h3_server_stream_delivers_all_messages_and_trailer ... ok
test grpc_h3_client_stream_relays_all_request_messages ... ok
test grpc_h3_bidi_stream_echoes_in_order ... ok
test grpc_h3_trailers_only_immediate_error_preserved ... ok
```

The existing `quiche::h3` H3-terminate machinery already (a) forwards the
trailing `grpc-status`/`grpc-message` HEADERS end-to-end (Tier A4/A5), (b)
preserves the Trailers-Only single-field-section shape (A6), (c) relays the
length-prefixed messages opaquely for all four call types (A3/C2), and (d)
preserves the content-type + FIN (A2/A7). So **gRPC-over-H3 in the opaque
model is characterization, not greenfield** — the only repo change so far is
a NEW test file + an `lb-grpc` dev-dep on `lb-quic`.

This is the expected happy-path pass; the conformance FINDINGS are at the
edges (Tier B + the Tier D divergence), characterized in Phase 1b below.

## Findings

### F-S29-1 — large-response trailing-`grpc-status` HEADERS dropped (FIXED) — HIGH / trailer-preservation

**Tier:** CORRECTNESS / trailer-preservation (R4 — never asterisked). **Status: FIXED this session** (single-sourced, shared H3-egress).

**Symptom:** an H3-terminate response whose total body exceeds a
size threshold (~448 KiB with a single large upstream body frame; higher
with smaller frames — the threshold scales with frame size) delivers the
full body but **drops the trailing `grpc-status`/`grpc-message` HEADERS +
FIN**: the stream hangs with no trailer, no FIN, no reset. gRPC-fatal (the
client never sees a status). General to the H3 egress — gRPC merely
exposes it because gRPC *always* ends with a trailer.

**Attribution (proven, not assumed — R2):**
- The producer (`stream_h2_response`) + hyper H2 client deliver `Trailers`
  + `End` correctly for all sizes with no backpressure
  (`grpc_h3_producer_trailer_attribution`: 4/4 `got_trailers=true`). So the
  trailer is received at the gateway — the loss is downstream.
- Instrumented actor trace on the failing 512 KiB case showed the exact
  sequence: `pull Trailers → emit Trailers OK` (fin_sent, `retain` removes
  the `StreamTx`) → a fresh `StreamTx` is **re-created** by
  `drain_resp_channels`' `entry().or_insert_with()` → it replays the
  leftover `End`/`Disconnected` → fires a spurious `ended`-FIN +
  **RESET-branch** (`stream_shutdown(Write, H3_INTERNAL_ERROR)`), looping
  every 2 ms. For a *small* response the trailer+FIN were already delivered
  before the reset (benign); for a *large* one they are still buffered in
  quiche's send queue and the reset **discards** them.

**Root cause:** after `drain_streams_to_conn`'s `retain` removes a terminal
`StreamTx` from `stream_response`, the matching `resp_rx_by_stream` receiver
is **not** cleaned up; `drain_resp_channels` then re-creates a fresh
`StreamTx` via `entry().or_insert_with()` and the stale buffered events
drive a spurious RESET that discards a large response's still-buffered
trailer+FIN. (Also caused a post-response 2 ms busy-loop.)

**Fix** (`conn_actor.rs::drain_resp_channels`): `get_mut` instead of
`entry().or_insert_with()`; a missing `StreamTx` means the stream already
terminated correctly, so drop the stale receiver and skip — no respawn, no
spurious reset, no busy-loop. Single-sourced in the shared H3 response
egress ⇒ fixes H3→H1/H2/H3 alike (R12).

**Regression tests:** `grpc_h3_large_message_roundtrips_byte_identical`
(512 KiB), `grpc_h3_trailer_survives_all_response_sizes` (1 B…1 MiB),
`grpc_h3_trailer_survives_any_frame_granularity` (giant + small frames),
`grpc_h3_producer_trailer_attribution`. All green; the size sweep delivers
`grpc-status:0` + clean FIN at every size up to 1 MiB.

### F-S29-2 — gRPC over an HTTP/1.1 backend drops `grpc-status` (deliberate deviation) — LOW

**Tier:** DELIBERATE DEVIATION (B5; spec ref gRPC-H2 *Responses* + RFC 9112
§7.1.2). gRPC mandates HTTP/2+; an H1 origin cannot reliably carry the
always-present trailing-`grpc-status` HEADERS. `grpc_h3_to_h1_*` confirms:
an H3-front → H1-backend gRPC call returns `200` but **no `grpc-status`
trailer** reaches the client. The opaque H3 front is protocol-blind (no
content-type guard), so it does not reject the route. **Resolution
(industry-standard, matches the H2 path's "gRPC REQUIRES HTTP/2"):**
document that gRPC requires an H2/H3 backend; operators must not configure
an H1 backend for a gRPC route. No code change (the opaque model is
correct; a content-type guard on the H3 front would add gRPC-awareness the
model explicitly avoids — R7). Carry as a v1 release-note limitation.

### Tier-D divergence (H2 gRPC-aware proxy vs opaque H3) — owner-ratifiable, NOT a defect

Characterized + verified as deliberate (conformant per the analysis — Tier D
is value-add, not required):
- **D1** `grpc-timeout` forwarded **unclamped** on H3 (`...unclamped` test:
  backend sees `600S`; the H2 path clamps to 300 s + synthesizes
  `DEADLINE_EXCEEDED`).
- **D2** `/grpc.health.v1.Health/Check` **forwarded** to the backend on H3
  (`...health_check_forwarded` test: backend hit ≥1; the H2 path synthesizes
  SERVING locally without dialing).
- **D3** No HTTP-status→grpc-status synthesis on H3 (opaque forward; the
  client runtime synthesizes, per spec).

These make H3 a pure opaque transport proxy and H2 a gRPC-aware proxy — a
deliberate, owner-ratifiable feature distinction, not a shared-machinery
divergence. **Owner decision items (Phase 1 check-in):** ratify the opaque
model + the gated-vs-default call (below).

### Gated-vs-default decision — NO gate needed

Unlike WS-over-H3 (S28, which added a brand-new validated path + a
`SETTINGS`-advertised capability and so warranted a newness gate), gRPC over
H3 adds **no new request-handling path**: a gRPC request is an ordinary H3
`POST` with `content-type: application/grpc` + trailers, already handled by
the H3 proxy. The only change is the F-S29-1 fix to the **existing** shared
egress (which is strictly a bug fix benefiting all H3 responses). So there is
nothing new to gate — gRPC-over-H3 is on by virtue of normal H3 proxying, and
`grpc_enabled` would gate nothing. (Owner may still choose to advertise gRPC
support as a config flag for clarity, but it is not a safety/newness gate.)
