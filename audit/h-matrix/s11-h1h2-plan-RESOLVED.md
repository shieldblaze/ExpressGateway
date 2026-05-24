# H1→H2 R8 Plan — S11 RESOLVED (lead-approved, plan-approval artifact per R5)

Base: `feature/h-matrix-s11` @ S10 tip 2f9f13bb. Phase 0 GREEN (R1 ×3 1224/0,
clippy/fmt clean, 27 GB free). This doc resolves `s11-h1h2-plan.md`'s open
questions and is the authoritative build spec. **No source change precedes
this approval.** Author ≠ verifier on every increment (R5).

## Reference reality (lead-read, S11 Phase 0)
- Dispatch `h1_proxy.rs:1238` (`handle`) → `proxy_h1_to_h2` (1563) currently
  buffers BOTH legs: `translate_h1_request_to_h2` (2052, `body.collect()`) and
  `upstream_response_to_h1` (2176, `body.collect()`).
- Reference egress (S10 fix): `proxy_h2_to_h2_request` (h2_proxy.rs:1964).
  Branch B detached-send block = **2300–2413** (verdict gate + `reset_peer` +
  `inject_abort` FIFO-observe + `head_rx` await). This is the F-MD-4
  graceful-drop fix.
- Reference ingress (H1 M-D-lite): `proxy_request` (h1_proxy.rs:1173). H1
  framing: `Kind::Chan`, `frame()==None` = POSITIVE clean end, `Some(Err)` =
  truncation. `is_end_stream()` deliberately NOT used (mirror-image of H2).
- ProxyErr→HTTP map: `proxy_h2_to_h2` (1925–1955): Upstream→502, Timeout→504,
  BodyTooLarge→413, BadRequest→400, Ok→stream.
- Types: `struct PumpAbort` (h2_proxy:108, module-private), `struct H1PumpAbort`
  (h1_proxy:121). Gauges: `record_retained_h1` + `H1_REQ_MAX_RETAINED_BODY_BYTES`
  (h1_proxy:2447/2456). `H2_ABORT_OBSERVE_TIMEOUT` (h2_proxy:130).
- Response transform: `H2ToH1Bridge` + `RESPONSE_HOP_BY_HOP` (h2_to_h1.rs);
  trailer-aware head builder `build_h1_response_with_trailers` (h1_proxy:2244);
  streaming H1→H1 reference `finalize_response` (1778, `body.boxed()`).
- S10 H2→H2 response leg `upstream_h2_response_to_h2` (h2_proxy:2658) already
  streams (`body.boxed()`, "trailers ride terminal frame") — but its downstream
  is H2 (native trailers, no `Trailer:` head declaration needed).

## RESOLVED DECISIONS

### D1 — CF-DEDUP-1: EXTRACT the shared egress helper (APPROVED)
R12 makes a second hand-mirrored copy of the F-MD-4 graceful-drop fix
unacceptable (a divergence = a smuggling hole in a security-critical path).
Extract the detached-send block (h2_proxy:2300–2413) into one `pub(crate)`
helper and route BOTH H2→H2 and H1→H2 through it.

```
pub(crate) async fn drive_h2_upstream_send(
    pool: &Http2Pool,
    backend_addr: SocketAddr,
    upstream_req: Request<H2ReqBody>,      // body already boxed → H2ReqBody
    verdict_rx: oneshot::Receiver<Result<(), ProxyErr>>,
    pump: tokio::task::JoinHandle<()>,
    body_timeout: Duration,
) -> Result<Response<IncomingBody>, ProxyErr>
```
- It is CHANNEL-ERROR AGNOSTIC: it receives the already-built
  `Request<H2ReqBody>` (stream_body already boxed the channel error to
  `Box<dyn Error>`), the `verdict_rx`, and the pump `JoinHandle`. It contains
  the `tokio::spawn` detached task (biased select: verdict vs `send_fut`,
  `reset_peer` on every abort verdict, F-CAP-1 caller arm classifying
  BodyTooLarge/BadRequest over 502, `head_tx`) and the final `head_rx.await`.
  **Logic byte-for-byte equal to the current 2300–2413.**
- `ProxyErr` is currently private to... confirm module: it is defined in
  `h1_proxy.rs:1797`. The helper returns `ProxyErr`; H2→H2 already uses
  `ProxyErr` (it's visible to h2_proxy). Place the helper where both can call
  it; if `ProxyErr`/`PumpAbort` visibility blocks it, widen to `pub(crate)`
  (mechanical, no behaviour change). Builder picks the lowest-visibility home
  (a shared `h2_egress` submodule, or `pub(crate)` in h2_proxy) and documents.
- **H2→H2 re-wire**: Branch B (h2_proxy ~2300) calls the helper instead of the
  inlined block. NET BEHAVIOUR UNCHANGED. This is the explicit S11 re-verify
  scope (R12): the H2→H2 F-MD-4 negative control (R13c) MUST still fail-pre /
  pass-post and the H2→H2 real-wire + isolation-burst MUST still pass.

### D2 — H1→H2 request leg: `proxy_h1_to_h2_request` (mirror of H2→H2)
New method mirroring `proxy_h2_to_h2_request`'s STRUCTURE (lookahead window →
Branch A buffered / Branch B streaming) with these DELTAS:
- **Preamble**: new head-only helper `build_h1_to_h2_upstream_parts(&parts)
  -> Result<(http::request::Parts /*for hyper*/ , ...), String>` extracted from
  `translate_h1_request_to_h2` (2052) MINUS the body collect: run
  `create_bridge(Http1, Http2)` over a body-LESS `BridgeRequest` (method, uri
  path+query, headers, scheme `"http"`, EMPTY body, EMPTY trailers), synthesize
  `:authority` from Host/URI, build the hyper `Request` parts (method + scheme://
  authority/path uri + non-pseudo headers). Do NOT force HTTP/1.1, do NOT strip
  CL/TE the way the H1→H1 egress does (H2 framing is hyper's H2 encoder's job —
  same DELTA as H2→H2's `build_h2_upstream_request_parts`).
- **Ingress framing = H1 M-D-lite (MIRROR-IMAGE of H2), in BOTH phases:**
  - Lookahead phase `None` arm: H1 `None` = clean end → `reached_eof = true;
    break` (NOT gated on `is_end_stream()`). `Some(Err)` = truncation →
    `return Err(BadRequest("inbound H1 request body incomplete"))` (zero pool
    contact — Branch A validate-before-dial property preserved).
  - Branch B pump `None` arm: clean → drop tx → `verdict_tx.send(Ok(()))`
    (NO `inject_abort`). `Some(Err)` arm: `inject_abort!` then
    `verdict_tx.send(Err(BadRequest(..incomplete..)))`. Trailers validated with
    **`validate_h1_request_trailers`** (H1 path), forbidden → `inject_abort!` +
    BadRequest. Over-cap (MAX_REQUEST_BODY_BYTES) → `inject_abort!` +
    BodyTooLarge.
  - The egress channel uses the EXISTING `H1PumpAbort` (h1_proxy:121) as its
    error type (constructible; mapped to `Box<dyn Error>` in the stream_body →
    `H2ReqBody`). NO new `PumpAbort` visibility needed. Define a local
    `inject_abort!` that sends `Err(H1PumpAbort)` then bounded `tx.closed()`
    (reuse `H2_ABORT_OBSERVE_TIMEOUT` — widen to `pub(crate)` if needed).
  - Memory gauge = **`record_retained_h1`** (H1 ingress) at the lookahead-buffer
    and in-flight-window sites, matching `proxy_request`.
- **Branch A**: build buffered `H2ReqBody` from lookahead + validated trailers
  (`build_h2_body_with_trailers` widened to boxed error, as H2→H2 Branch A
  does) → `pool.send_request` directly (validated, so no detached task / no
  F-CAP-1 caller arm needed — identical to H2→H2 Branch A).
- **Branch B**: build channel + stream_body (`H2ReqBody`), spawn the H1-framed
  pump, then call **`drive_h2_upstream_send`** (D1).
- **Dispatch re-wire** `proxy_h1_to_h2` (1563): call `proxy_h1_to_h2_request`,
  map ProxyErr exactly as `proxy_h2_to_h2` does (502/504/413/400), Ok → the
  NEW streaming response leg (D3).

### D3 — H1→H2 response leg: stream, trailer behaviour PROVEN on the wire
Replace `upstream_response_to_h1`'s `body.collect()` with a streaming relay:
- Apply the H2→H1 head transform to PARTS: status, lowercase regular headers,
  drop `:`-pseudo, `RESPONSE_HOP_BY_HOP` strip, Alt-Svc inject — i.e. the
  header half of the existing bridge, WITHOUT collecting the body.
- Body: `body.boxed()` (unknown length → hyper H1 encoder frames chunked; R8
  satisfied — bounded by hyper's H2 receive-window). Reference: `finalize_
  response` (1778) + `upstream_h2_response_to_h2` (2658).
- **Trailers (CF-RESP-1 crux)**: a streamed relay cannot pre-declare `Trailer:`
  names (they arrive only in the terminal frame). The codebase claims hyper's
  H1 encoder needs the head `Trailer:` declaration + chunked TE to FLUSH
  trailers (build_h1_response_with_trailers doc, `proto/h1/encode.rs:163-213`).
  **VERIFIER MUST settle this on the wire** (real H2 backend emitting response
  trailers → real H1 client): (i) if hyper relays trailers on the boxed stream
  WITHOUT the declaration → done; (ii) if NOT and no existing test asserts
  H2→H1 trailer relay THROUGH THE PROXY → streaming wins, document the bounded
  behaviour change (matches nginx default of not forwarding H2 response
  trailers to H1) in the report — NOT a silent drop; (iii) if NOT and a test
  DOES assert proxy trailer relay → R3 conflict → escalate to lead as a
  genuine product decision (stream-without-trailers vs buffer-for-trailers);
  do NOT silently regress. Builder implements (i)/(ii); lead adjudicates (iii).

## Increments (one owning builder each; author ≠ verifier)
- **I1** (builder): D1 extract `drive_h2_upstream_send` + re-wire H2→H2. Commit,
  push. — verifier: H2→H2 re-verify (real-wire + R13 a/b/c negative control).
- **I2** (builder): D2 request leg + dispatch re-wire. Commit, push. — verifier:
  request-leg real-wire (Branch A+B byte-identity, memory gauge + inverted
  probe via `H1_REQ_MAX_RETAINED_BODY_BYTES`, backpressure), F-MD-4 H1
  mirror-image R13 (a in-gate / b isolation-burst ≥50× / c negative control),
  F-CAP-1 (413/400/502).
- **I3** (builder): D3 response leg. Commit, push. — verifier: large response
  byte-identity + response backpressure + trailer wire-behaviour determination.
- **I4** (verifier-owned): R13 layered suite hardening, scoped llvm-cov
  re-measure (≥80% session sub-metric), R3 full regression, Phase 3 ×3 gate.

## BUILT bar (verifier, independent) — unchanged from s11-h1h2-plan.md §76-94.
## Honest-stop: if I1–I4 consume the budget, H1→H3 is plan-only (Phase 2).
