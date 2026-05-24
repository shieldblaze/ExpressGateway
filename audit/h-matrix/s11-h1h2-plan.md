# H1→H2 R8 Plan — S11 head-start (lead-authored, S10 honest-stop deliverable)

- Author: `lead` (S10). Honest-stop deliverable: H1→H2 was NOT built in S10
  (Phase 2 honest-stopped to land the H2→H2 F-MD-4 fix + re-verify within budget).
  This is the S11 build spec, informed by the S10 H2→H2 conversion + its F-MD-4 fix.
- Cell: **H1→H2** — HTTP/1.1 front ⇒ HTTP/2 backend, plane `lb-l7`.
- Status on entry: **PARTIAL (buffering proxy)**, like H2→H2 was — BOTH legs buffer.

## Current-tree reality (lead-read at S10)
- Dispatch: `h1_proxy.rs:1131` — `UpstreamProto::H2 => proxy_h1_to_h2(addr, stripped)`.
- **Request leg buffers (R8 #1):** `proxy_h1_to_h2` (1563) → `translate_h1_request_to_h2`
  (2052) `body.collect().await` → `to_bytes()` → `h2_pool.send_request` (1578).
- **Response leg buffers (R8 #2):** `upstream_response_to_h1` (2176) `body.collect().await`
  (2182) on the H2-backend `Incoming` response before bridging H2→H1.
- H1Proxy already holds `h2_upstream: Option<Arc<Http2Pool>>` (295) + `with_h2_upstream`
  (501) + `has_h2_upstream` (519).

## KEY S10 INSIGHT (do not relearn the hard way)
H1→H2 feeds the **same `Http2Pool` egress** as H2→H2. The S10 H2→H2 F-MD-4 defect
was a **graceful-drop race specific to feeding an H2 upstream via hyper's H2
client through `Http2Pool`**: a downstream client abort cancels the gateway
service future, dropping the in-flight `send_request` future + upstream request
body at a clean frame boundary, so hyper emits a clean END_STREAM upstream BEFORE
observing the injected body-error → truncated request smuggled as complete.
**H1→H2 WILL HAVE THE SAME HAZARD** on its request-egress leg. The S11 build MUST
reuse the S10 fix machinery from the start (do not ship the naive version and
wait for the verifier to catch it again):
1. **DETACH** the upstream `send_request` + verdict resolution into a `tokio::spawn`
   task that owns `send_fut` + the request body, decoupled from the downstream
   (H1) connection future. Caller awaits a `head_rx` oneshot. (h2_proxy.rs
   `proxy_h2_to_h2_request` ~2304–2413 is the reference.)
2. **`Http2Pool::reset_peer(addr)`** (already added in S10, http2_pool.rs ~314) on
   every abort verdict — the multiplexed-pool teardown backstop.
3. **`inject_abort!`**: enqueue the body-abort error then `tx.closed().await`
   bounded by `H2_ABORT_OBSERVE_TIMEOUT` so hyper observes the FIFO Err before any
   channel close.
This makes **CF-DEDUP-1 compelling at S11**: H2→H2 and H1→H2 share the IDENTICAL
egress-to-`Http2Pool` streaming machinery (detached send + reset_peer +
inject_abort + F-CAP-1 caller arm + verdict gate); ONLY the ingress pump differs
(H2 M-D vs H1 M-D-lite). Strongly consider extracting a shared
`stream_request_to_h2_upstream(pump_body, verdict_rx, pool, addr, timeouts)`
helper at S11 and re-verifying BOTH H2→H2 and H1→H2 against it (re-verify cost is
the explicit S11 scope, unlike S10 where mirroring protected the promoted cell).

## Net-new build (two legs, R8)

### Leg 1 — request (H1 M-D-lite ingress → M-B egress, S10-fix-hardened)
- Ingress pump = **M-D-lite** (from H1→H1, `h1_proxy.rs::proxy_request` Branch-B
  pump). **F-MD-4 is the MIRROR-IMAGE of H2** (S9 H1→H1 precedent): the H1 inbound
  body is `Kind::Chan` — `frame()==None` IS the clean END (not ambiguous), and
  `Some(Err(IncompleteBody))` is the truncation signal. `is_end_stream()` is
  deliberately NOT used (the inverse of H2's ambiguous-`None`). So the abort arm
  fires on `Some(Err(_))`, the clean arm on `None`. The `inject_abort!` +
  detached-send hardening still applies on the EGRESS side regardless of the
  ingress framing (the graceful-drop race is an egress/Http2Pool property).
- Egress = `Http2Pool::send_request` with `H2ReqBody` (PumpAbort → Box<dyn Error>),
  via the detached task (above). F-CAP-1 caller arm: classified BodyTooLarge→413 /
  BadRequest→400 over 502; Timeout→504.
- Preamble: H1→H2 request build = bridge `create_bridge(Http1, Http2)` (synthesize
  pseudo-headers from the H1 request-line + Host→:authority), as the current
  `translate_h1_request_to_h2` does — but WITHOUT collecting the body.
- Branch A (≤window) validate-before-dial: zero pool contact on within-window
  malformed.

### Leg 2 — response (H2 backend → H1 front, convert collect → stream)
Replace `upstream_response_to_h1`'s `body.collect().await` (2182) with a streaming
relay: apply the H2→H1 response transform (status + hop-by-hop strip + lowercase,
per `H2ToH1Bridge::bridge_response` / the existing RESPONSE_HOP_BY_HOP set) to the
`parts`, then `body.boxed()` the H2 `Incoming` for streaming-by-construction;
trailers ride the terminal frame. Note: this is the H2→H1 RESPONSE direction
(distinct from H2→H2's response leg). Confirm hyper's H1 encoder frames the boxed
streaming body as chunked (the F-MD-1 lesson: force framing correctly; H1 response
to an H1 client). Memory bounded by hyper's window by construction. (Relates to
CF-RESP-1 — the response-leg `collect()` family.)

## BUILT bar (verifier, independent; author≠verifier)
Real-wire genuine H1 client → real `h1_proxy` listener → router → real H2 backend
(`Http2Pool`), binary bodies byte-identical BOTH directions:
1. Request leg: byte-identity Branch A + B; non-vacuous memory gauge + inverted
   probe (H1 `H1_REQ_MAX_RETAINED_BODY_BYTES` gauge); backpressure both legs.
2. **F-MD-4 H1 mirror-image**: client truncates the H1 request body mid-stream
   (premature close / CL-short / chunked-without-terminator) → H2 backend observes
   `complete=0` (NEVER complete). MUST be tested with the same aggressive timing
   that caught the H2→H2 bug (full-forward + settle + abort) AND the hardened
   in-gate repetition so the parallel gate catches a regression. The egress
   graceful-drop race is the SAME hazard — prove it under `--test-threads=1`
   repetition (≥50×, 0 smuggles) with a load-bearing negative control.
3. **F-CAP-1**: over-cap → 413, forbidden framing → 400, genuine upstream → 502.
4. Response leg: large H2-backend response byte-identical at the H1 client;
   response-leg backpressure (slow H1 client → bounded retained); trailers relayed.
5. R3: H2→H2, H3→H2, H2→H1, H1→H1, proto_translation intact (shared `Http2Pool`).
6. Coverage ≥80% session sub-metric (scoped llvm-cov, R10).
7. fmt/clippy; F-MD-4 + F-CAP-1 regressions INSIDE the ×3 gate (hardened).

## Sequencing after H1→H2
Then **H1→H3 / H2→H3** (need M-C, H3 upstream via QUIC — heaviest; response leg
`h3_response_to_h2` also buffers, CF-RESP-1). H1→H2 done → **7 of 9** BUILT.

## Carry-forwards reinforced by this plan
- **CF-DEDUP-1** (now compelling): extract the shared egress-to-Http2Pool streaming
  machinery; re-verify H2→H2 + H1→H2 together.
- **CF-RESP-1**: response-leg `collect()` in `upstream_response_to_h1` (H1→H2),
  `h3_response_to_h2` (H2→H3), and the H3-front response paths.
- **CF-RESETPEER-1** (if S10 verifier flags it): `Http2Pool::reset_peer` tears down
  the whole multiplexed connection on a single-stream abort — assess multiplex
  collateral / whether it's load-bearing vs the inject_abort per-stream mechanism;
  carries into H1→H2 since it reuses the same egress.
