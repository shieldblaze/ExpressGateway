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
| B1 | B | client cancel ↔ RESET_STREAM/STOP_SENDING (`H3_REQUEST_CANCELLED`); propagate to peer leg | gRPC-H2 *Errors*; RFC 9114 §4.1.1 | ⏳ | reset test (Phase 1b) |
| B2 | B | **mid-call reset ≠ clean finish** (never FIN+`grpc-status:0` for a reset) | gRPC-H2 *Errors*; RFC 9114 §4.1.1 | ⏳ | backend-reset test (Phase 1b) |
| B3 | B | `te: trailers` pass-through; not required on H3 ingress; never emit `te` ≠ `trailers` | RFC 9114 §4.2 | ⏳ | backend `last_te`; no-`te` request (Phase 1b) |
| B4 | B | hop-by-hop / connection-specific fields stripped on H3 leg | RFC 9114 §4.2 | ⏳ | existing H3 cell behavior (Phase 1b) |
| B5 | B | gRPC route MUST NOT go to an **H1 backend** (trailer-incompatible) | gRPC-H2 *Responses*; RFC 9112 §7.1.2 | ⏳ | H3→H1 gRPC trailer-drop test (Phase 1b) |
| C1 | C | forward `grpc-timeout`/`grpc-encoding` unchanged (opaque) | gRPC-H2 *Requests* | ⏳ | grpc-timeout pass-through (Phase 1b) |
| C2 | C | no per-call-type behavior — uniform relay | gRPC-H2 grammar | ✅ | all 4 call types pass through the SAME relay path |
| C3 | C | MAY forward bare non-200 verbatim (client synthesizes) | gRPC-H2 *Responses* | ⏳ | (opaque default; Phase 1b) |
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

## Findings (Phase 1b — the edges)

_(to be filled: B1/B2 reset mapping, B3 te-trailers, B5 H1-backend, C1
grpc-timeout pass-through, A3-under-volume, and the Tier D H2-vs-H3
divergence. Each ❌ characterized with mechanism + spec cite + R6 tier.)_
