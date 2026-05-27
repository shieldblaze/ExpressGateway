# Phase 1 — H1→H3 BUILD PLAN (builder-1, S12)

Consumes the M-C extraction (Phase 0a, refactor-eng). Written against the
**LOCKED** connector interface (refactor-eng ↔ builder-1, agreed; pending lead
re-approval of Option A). Supersedes the sketch; this is the build spec.

Base plan: `audit/h-matrix/s12-h1h3-plan.md` (lead). This document is the
H1→H3-specific build derived from it + the negotiated interface.

---

## LOCKED interface (from refactor-eng — do NOT re-derive)

Import path (refactor-eng, confirmed): `use lb_quic::H3RespEvent;` (crate-root
re-export of `lb_quic::h3_bridge::H3RespEvent`, same style as
`lb_quic::H3UpstreamResponse` today). fn: `lb_quic::h3_bridge::
stream_request_to_h3_upstream` (`pub`). Consumed cross-crate from `lb-l7`.

Connector-side guarantees (refactor-eng, confirmed): does ZERO trailer
validation (trusts `End{trailers}` as already-validated — MY pump validates
first); the request-trailer FORWARD arm is NET-NEW connector code (today's H3→H3
`FinNoTrailers` arm has no trailer-ship logic) modeled on
`request_h3_upstream:2521-2543`. H3→H3 passes `forward_req_trailers=false` → bare
FIN → byte-identical. My Phase 1 live request-trailer-forwarding test (BUILT-bar
#6) is the coverage for that net-new forward arm.

```rust
pub enum H3RespEvent {
    Head { status: u16, headers: Vec<(String,String)> }, // ONCE, before Body; pseudo filtered; CL as regular header
    Body(Bytes),                                          // decoded DATA, ≤H3_RESP_CHUNK_MAX, depth H3_RESP_CHANNEL_DEPTH
    Trailers(Vec<(String,String)>),                       // post-DATA HEADERS, pseudo filtered, only when non-empty; ALWAYS emitted by connector
    End,                                                  // clean FIN — never co-emitted with Reset, never on partial
    Reset,                                                // any abort
}

pub async fn stream_request_to_h3_upstream(
    headers: Vec<(String,String)>,   // request field-list (caller builds)
    forward_req_trailers: bool,      // H1→H3 passes TRUE
    addr: SocketAddr, sni: &str, pool: &QuicUpstreamPool,
    body_rx: Receiver<ReqBodyEvent>, // request trailers ride in ReqBodyEvent::End{trailers}
    resp_tx: Sender<H3RespEvent>,
    cap: usize,
) -> Result<(), RespAbort>
```

Contract (relied on by the H1 front):
- `End` only on response_complete (sent_head && between_frames && rx_tail empty
  && upstream FIN). `Reset` on every abort. NEVER `End` on a partial.
- Connector does NO trailer validation — `End{trailers}` is ALREADY-VALIDATED by
  the H1 pump; connector just frames+ships (post-DATA HEADERS, only when
  non-empty, preserving HEADERS→DATA→FIN when empty).

---

## What I build (lb-l7 side only; connector is refactor-eng's)

Replace the buffering `proxy_h1_to_h3` (h1_proxy.rs:1950) — which does
`collect_h1_request_to_h3_fieldlist` (`body.collect().await`) →
`request_h3_upstream` (single DATA frame, whole-body `Vec<u8>` response) →
`h3_response_to_h1` — with a fully streaming both-legs relay.

### Leg 1 — request (H1 M-D-lite ingress → connector body_rx)

Reuse the H1→H1 / H1→H2 M-D-lite ingress pump shape (h1_proxy.rs `proxy_request`
1380-1489) verbatim in intent:
- Build the request field-list via the EXISTING H1→H3 bridge — refactor
  `collect_h1_request_to_h3_fieldlist` (h1_proxy.rs:2709) into a head-only
  variant `build_h1_to_h3_fieldlist(parts, sni, https) -> Vec<(String,String)>`
  (drops the `body.collect()` + trailer capture; those now stream). The bridge
  call (`create_bridge(Http1,Http3).bridge_request`) + `:authority` synthesis is
  preserved unchanged.
- Spawn a pump that feeds `ReqBodyEvent` into the connector's `body_rx`
  (channel depth `H3_BODY_CHANNEL_DEPTH`):
  - `Some(Ok(data))` within cap → `ReqBodyEvent::Chunk(data)` (split ≤
    `H3_BODY_CHUNK_MAX`, or let the connector re-split — TBD, connector already
    re-splits per stream_capacity; PREFER pump-side split to bound the channel
    item size, matching the in-flight gauge).
  - **REQUEST-BODY CAPPING IS MY PUMP'S JOB** (refactor-eng confirmed: the
    connector does NOT cap request bytes — its `H3RespOut` `cap` is the RESPONSE
    cap → `RespAbort::OverCap`, unrelated). My pump tracks `forwarded_total` and
    enforces `MAX_REQUEST_BODY_BYTES`. The 413-vs-Reset boundary is TIMING-CRITICAL
    (mirror H1→H1):
      · **PRE-DATA over-cap** (Reset is the FIRST event, before ANY Chunk
        forwarded) → connector `inline(413)` (h3_bridge.rs:3193) → synthesized
        `Head{413}+Body+End` on resp_rx + fn returns `Ok(())`. I RELAY it like a
        200. This is the only clean-413 path.
      · **MID-BODY over-cap** (Reset AFTER ≥1 Chunk forwarded) → connector
        `AbortNoFin` (RESET-without-FIN, case-7 smuggling guard) → `H3RespEvent::
        Reset` + `Err(RespAbort::UpstreamReset)`. NOT a 413 — a fresh 413 Head
        after the request started streaming (and a response Head may be in flight)
        would be response-splitting. I abort the H1 client, never FIN.
    So my pump must decide pre-data-413 vs mid-body-abort itself (exactly the
    H1→H1 pump): emit `Reset` BEFORE forwarding the first over-cap chunk to get the
    clean 413; once a chunk is out, over-cap is correctly a RESET.
  - `Some(Ok(trailers))` → run `validate_h1_request_trailers`; OK →
    `ReqBodyEvent::End{trailers: validated_vec}`; FORBIDDEN → `ReqBodyEvent::Reset`
    (smuggling guard — a forbidden trailer NEVER becomes a clean End).
  - `None` (clean end) → `ReqBodyEvent::End{trailers: vec![]}` → connector FIN.
  - `Some(Err)` (hyper `IncompleteBody` truncation) → `ReqBodyEvent::Reset` →
    connector RESET-without-FIN (`H3_REQUEST_CANCELLED`). THE F-MD-4 H3
    mirror-image: a truncated inbound NEVER becomes a clean-FIN'd upstream
    request.
- Call `stream_request_to_h3_upstream(field_list, /*forward_req_trailers=*/true,
  addr, sni, pool, body_rx, resp_tx, MAX_REQUEST_BODY_BYTES)`.

### Leg 2 — response (connector resp_rx H3RespEvent → H1 client, streamed)

Drive a `Receiver<H3RespEvent>`. **Template = `upstream_response_to_h1`
(h1_proxy.rs:2578), the H2→H1 STREAMING response head-builder — NOT the
buffering `h3_response_to_h1` (h1_proxy.rs:2785) / `build_h1_response_with_trailers`
(h1_proxy.rs:2636).** `upstream_response_to_h1` already solved this exact problem
for H2→H1: it does the response header transform INLINE (drop `:`-pseudo + the
`RESPONSE_HOP_BY_HOP` set, lowercase the rest — same authoritative shape as
`H2ToH1Bridge::bridge_response`), injects `Alt-Svc`, and `body.boxed()` for
streaming-by-construction, deliberately NOT calling the buffering trailer-aware
builder (its doc 2563-2577 IS the CF-RESP-1 rationale, and it explicitly notes
"the H3→H1 leg keeps the buffering `build_h1_response_with_trailers`" — which THIS
plan replaces with the streaming form).
- `Head{status, headers}` → build the H1 response head exactly as
  `upstream_response_to_h1` does: status + the inline pseudo/HOP-BY-HOP-stripped,
  lowercased header set + optional `Alt-Svc`. NO `Trailer:`/chunked-TE
  pre-declaration (trailers unknown at Head-time). The body is a `StreamBody` fed
  from subsequent `Body`/`Trailers`/`End` events → `body.boxed()`. The one delta
  vs `upstream_response_to_h1`: its source is a hyper `Response<IncomingBody>`
  (one boxed body); mine assembles the `StreamBody` from decoded `H3RespEvent`s.
  Factor the shared header transform so both call one helper (avoid a third copy
  of the pseudo/HOP-BY-HOP strip).
- `Body(b)` → `StreamBody` `Frame::data(b)`.
- `Trailers(t)` → relay onto the `StreamBody`'s terminal frame
  (`Frame::trailers`). **The connector MUST propagate the H3 backend's response
  trailers to the H1 front (H3RespEvent::Trailers) — non-negotiable; build the
  propagating path.** Whether hyper-1's H1 encoder actually FLUSHES that terminal
  trailer frame to the H1 client WITHOUT a head `Trailer:` + chunked-TE
  pre-declaration (which I cannot emit — the names are unknown at Head-time) is
  the **CF-RESP-1 / S11 CASE (ii)** open question. Do NOT re-buffer to pre-capture
  trailer names (that is the exact R8 violation this build removes). Do NOT
  silently mark the H1-downstream drop as an "accepted bound" — that decision is
  NOT mine: the verifier's gRPC-shaped real-wire test empirically settles whether
  grpc-status reaches the H1 client, and the lead escalates the
  propagate-vs-document choice to the owner WITH that evidence. See §Trailer
  mandate below.
- `End` → finish the StreamBody cleanly (FIN to H1 client).
- `Reset` → abort the H1 client body (no clean terminator); never FIN. Maps the
  RespAbort contract to the H1 wire.
- **F-CAP-1 status mapping (VERIFIED against landed 369c5e53 — two distinct
  surfacing paths).** Two mechanisms, not one:
  - **Pre-data 413 & pre-dial 502 → synthesized `Head` (relay verbatim).** The
    connector's `H3RespOut::inline()` (h3_bridge.rs:2795) on the `Decoded` arm
    emits `H3RespEvent::Head{status, headers:[]}` + `Body` + `End`, and the fn
    returns `Ok(())`. Two cases reach this: (a) PRE-DATA over-cap (my pump's FIRST
    event was `Reset`, before any Chunk) → `inline(413)`; (b) pre-dial pool-acquire
    / empty-handle / header-encode failure → `inline(502)`. Both arrive on my
    `resp_rx` as a normal synthesized Head I relay VERBATIM — I do NOT manufacture
    these statuses.
  - **MID-BODY over-cap is NOT a 413 → it's `Reset`** (refactor-eng confirmed,
    h3_bridge.rs AbortNoFin path). If my pump emits `Reset` AFTER ≥1 Chunk was
    forwarded, the connector RESET-without-FINs the upstream and sends
    `H3RespEvent::Reset` (+ `Err(RespAbort::UpstreamReset)`) — a fresh 413 Head
    mid-stream would be response-splitting. I abort the H1 client, never FIN. So
    the clean-413 outcome depends on MY pump emitting `Reset` BEFORE the first
    over-cap chunk (see Leg 1). The s7_j2 unit does NOT cover the 413-as-Head
    path — my F-CAP-1 e2e (BUILT-bar #5) is its FIRST coverage.
  - **Response-leg aborts → `Err(RespAbort)` + validate-before-relay guard.**
    `RespAbort` (h3_bridge.rs, RESPONSE-leg enum: UpstreamReset / PrematureEof /
    ChunkedDecode / OverCap[response] / BadHead / ClientGone — all RESPONSE
    faults):
      · BEFORE first `Head` relayed → synthesize a 502 `error_response` (all
        variants are upstream-response faults; pre-Head guard).
      · AFTER a `Head` relayed → the connector already sent `H3RespEvent::Reset`
        on my channel → I truncate the StreamBody (no clean terminator), NEVER a
        new status (response-splitting / cache-poisoning guard). The
        `Err(RespAbort)` return is the connector's bookkeeping; my client-facing
        action was the Reset.
  - Net: the H1 front mostly RELAYS — request-leg 413/502 ride a synthesized Head;
    response-leg faults ride Reset (post-Head) or a synthesized 502 (pre-Head).
    This connector's request leg has NO 400/504 path (malformed request trailers
    became `ReqBodyEvent::Reset` in MY pump BEFORE the connector; the F-S7-6 idle
    deadline is the only timeout and surfaces as a RESPONSE-leg abort → 502/Reset,
    NOT a 504). CONFIRM with refactor-eng there's no request-leg 400/504 path I'm
    missing.

---

## BUILT bar (verifier; same shape as S11) — NEW coverage required

Current live H1→H3 coverage is ONLY `proto_translation_e2e.rs`
`proxy_h1_listener_h3_backend` (:513) — a BODYLESS GET asserting response body
`b"hello-h3"`. NONE of the streaming/trailer/byte-identity/abort coverage exists.
Phase 1 MUST add (real H1 client → real h1_proxy listener with h3_upstream wired
→ real H3/QUIC backend):
1. Binary body byte-identical BOTH directions (request body → H3 backend;
   response body → H1 client), non-trivial size (> single-DATA-frame, crosses
   `H3_BODY_CHUNK_MAX`).
2. Non-vacuous memory gauge (H3-upstream request-retained ≤ bounded window,
   body-size independent) + load-bearing inverted probe.
3. Backpressure both legs (slow H1 client backpressures H3 upstream; slow H3
   upstream backpressures H1 client ingest).
4. **F-MD-4 H3 mirror-image** with R13: (a) in-gate + (b) isolation-burst ≥50× +
   (c) load-bearing negative control — gate on FIN, PROVE RESET-without-FIN on
   truncation (CL and chunked premature close → upstream backend sees `complete=0`,
   never a truncated-as-complete request).
5. F-CAP-1 (first coverage of the connector's 413-as-Head path):
   - PRE-DATA over-cap → 413 relayed to the H1 client (synthesized Head; my pump
     emits Reset before the first over-cap chunk).
   - MID-BODY over-cap → H1 client sees a RESET / truncated body (NOT 413, NOT a
     clean FIN) — the response-splitting guard; assert the upstream backend saw
     NO clean-FIN'd complete request.
   - pre-dial upstream-down → 502 (synthesized Head).
   (No request-leg 400/504 path exists on this connector — malformed req-trailers
   → Reset in my pump; idle-timeout → response-leg 502/Reset.)
6. **NEW request-trailer-forwarding test** (RISK-3): H1 request with trailers →
   assert the H3 backend RECEIVES the post-DATA HEADERS trailing field section
   (locks `forward_req_trailers=true` + the `End{trailers}` path; nothing tests
   this today, so the regression would be silent).
7. **gRPC-shaped RESPONSE trailer mandate test** (verifier-owned, EMPIRICAL):
   H3 backend returns a gRPC-shaped response (HEADERS, DATA*, TRAILERS with
   `grpc-status`). Assert on the H1 client wire whether the `grpc-status` trailer
   is RECEIVED. This test EMPIRICALLY settles the CASE-ii question (it does NOT
   assume pass or fail) — its result is the evidence the lead escalates to the
   owner for the propagate-vs-document decision. See §Trailer mandate.
8. Coverage ≥80% session sub-metric (scoped llvm-cov, per
   `llvm-cov-session-scope-method`). fmt/clippy workspace-wide
   (`cross-crate-gate-scope` — the change spans lb-l7 + lb-quic). R3 intact.

**Apply CF-SATURATION-1 proactively**: any new H1→H3 over-cap/large-upload test
sits under the listener `timeouts.body` (wall-clock — the CF-BODY-WALLCLOCK class
the lead escalated to S13). Give such tests a generous listener body timeout +
client wait so 8-core gate saturation cannot false-504 a large push (the fcap1
fe992654 lesson). The H3 EGRESS itself uses the F-S7-6 idle deadline (no
wall-clock truncation), so only the H1 INGRESS side carries the wall-clock class.

---

## Trailer mandate (response-leg, gRPC-shaped) — NOT mine to "accept"

The plan's trailer mandate requires the gRPC-shaped RESPONSE trailers (`grpc-status`
in a post-DATA TRAILERS section) from the H3 backend to reach the downstream
client. My build obligations:
- BUILD the propagating path: connector emits `H3RespEvent::Trailers`; my Leg-2
  relays it onto the `StreamBody` terminal `Frame::trailers`. Non-negotiable.
- Do NOT re-buffer to pre-declare `Trailer:` (R8 violation).
- Do NOT mark the H1-downstream outcome as "accepted/bounded" — that judgment is
  the OWNER's, made on the verifier's empirical evidence (BUILT-bar #7), and the
  LEAD escalates it. My report states the EMPIRICAL result only.
HONEST framing for the report: a standard gRPC client speaks H2/H3, so **H2→H3 is
the gRPC-capable →H3 cell where the mandate is fully met downstream**. **H1→H3's**
downstream-trailer reach is the OPEN question BUILT-bar #7 settles (hyper-1 H1
encoder CASE-ii). If #7 shows grpc-status does NOT reach the H1 client, that is a
documented hyper-H1-encoder limitation surfaced WITH evidence — escalated, not
buried, and NOT pre-judged here.

## Risks carried into build
- RISK-1 (response decode): RESOLVED by the decoded `H3RespEvent` (lead ruling
  A2: shared driver + per-front sink; decode/re-encode fused in one `'evloop`).
- RISK-2 (CF-RESP-1 response-trailer reach on the H1 front): **OPEN — settled
  EMPIRICALLY by BUILT-bar #7, escalated to owner. NOT pre-accepted.** (Corrected
  from an earlier draft that wrongly called it an accepted bound — the lead
  forbade silent accept.)
- RISK-3 (request-trailer forwarding): RESOLVED by `forward_req_trailers=true` +
  the new live test (#6).

## Open questions — RESOLVED by the landed extraction (369c5e53, builder-1 read)
1. **Sink shape — RESOLVED: enum, not trait, not raw channel.**
   `H3RespOut::Decoded{ tx: Sender<H3RespEvent>, total: usize, cap: usize }`
   (h3_bridge.rs:2775). I own the `resp_rx` end of a
   `mpsc::channel::<H3RespEvent>(H3_RESP_CHANNEL_DEPTH=8)`, wrap the `tx` in
   `H3RespOut::Decoded{ tx, total: 0, cap: MAX_RESPONSE_BODY_BYTES }`, and pass
   the SINK (cap lives inside it — NOT a fn param). Call:
   `stream_request_to_h3_upstream(headers, /*forward_req_trailers=*/true, addr,
   sni, pool, body_rx, sink).await`.
2. **Inline-abort surfacing — RESOLVED: synthesized `Head`.** `H3RespOut::inline`
   (h3_bridge.rs:2795) on the Decoded arm emits `Head{status, headers:[]}` +
   `Body` + `End` — so request-leg/pre-dial 413/502 arrive as a normal synthesized
   Head I relay. Response-leg faults arrive as `Err(RespAbort)` (+ a `Reset` event
   if post-Head). Both paths handled per §F-CAP-1.
3. **`RespAbort` variants — RESOLVED (read):** UpstreamReset / PrematureEof /
   ChunkedDecode / OverCap / BadHead / ClientGone — ALL response-leg faults → 502
   pre-Head, Reset post-Head. NO request-leg 400/504 variant exists (confirming my
   §F-CAP-1 analysis: request malformed→Reset in my pump; idle-timeout→response
   abort→502). Remaining micro-confirm for refactor-eng: that there is genuinely
   no request-leg 400/504 surfacing I must map (I believe there is not).
- H3→H3-through-connector RESOLVED: A2 routes H3→H3 through the `Wire` sink
  (byte-identical, e2e 20/20). Does not affect H1→H3 consumption.

## Build wiring (locked to real symbols)
```rust
use lb_quic::{H3RespEvent, H3RespOut, stream_request_to_h3_upstream};
// ... build field-list (build_h1_to_h3_fieldlist, no body.collect) ...
let (body_tx, body_rx) = mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
let (resp_tx, mut resp_rx) = mpsc::channel::<H3RespEvent>(H3_RESP_CHANNEL_DEPTH);
let sink = H3RespOut::Decoded { tx: resp_tx, total: 0, cap: MAX_RESPONSE_BODY_BYTES };
// spawn the H1 M-D-lite pump → body_tx (Chunk/End{trailers}/Reset)
// spawn/await: stream_request_to_h3_upstream(headers, true, addr, sni, pool, body_rx, sink)
// drive resp_rx: Head→streaming head-builder; Body→StreamBody; Trailers→terminal frame; End→finalize; Reset→abort
```
