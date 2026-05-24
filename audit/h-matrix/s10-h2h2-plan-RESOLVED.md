# H2→H2 R8 Plan — SESSION 10 Phase 1 (lead-resolved, build spec)

- Author: `lead` (S10). Resolves `s9-h2h2-plan.md` against the current tree
  (`feature/h-matrix-s10`, base `main` @ `cce5a8ed`). Lead-approved build spec.
- Cell: **H2→H2** — HTTP/2 front ⇒ HTTP/2 backend, plane `lb-l7`.
- Status of cell on entry: **PARTIAL (buffering proxy)** — `proxy_h2_to_h2`
  exists and passes a real-wire e2e (`proto_translation_e2e.rs:442`) but
  **buffers BOTH legs** (R8 violations, see below). S10 converts it to the R8 bar.

## Current-tree reality (read by lead before approval)
- Dispatch: `h2_proxy.rs:1224` — `UpstreamProto::H2 => proxy_h2_to_h2(addr, stripped)`.
- **Request leg buffers (R8 VIOLATION #1):** `proxy_h2_to_h2` (1896) →
  `translate_h2_request_to_h2` (1961) does `body.collect().await` →
  `to_bytes()`. Whole inbound request body materialised before dial.
- **Response leg buffers (R8 VIOLATION #2):** `upstream_h2_response_to_h2`
  (2159) does `body.collect().await` on the upstream `Incoming` response —
  whole backend response body materialised before relay. (Contrast H2→H1:
  `finalize_response` (1881) boxes the `IncomingBody` → streaming-by-construction.)
- Reference pump to mirror: M-D in `proxy_request` (1333–1879) — lookahead
  validate-before-dial, Branch A (≤window buffered) / Branch B (streaming),
  in-flight gauge (`in_flight_bytes`), `PumpAbort`, `verdict_tx/rx`, F-MD-4
  (`is_end_stream` gating, `Err(PumpAbort)`-before-verdict), F-CAP-1 caller arm
  (await verdict, prefer 413/400 over 502). Leaf helpers already free/shared:
  `validate_request_trailers` (2094), `concat_chunks` (2116),
  `build_h2_body_with_trailers` (2127), `record_retained`, `PumpAbort` (108),
  consts `MAX_REQUEST_BODY_BYTES`/`H2_REQ_CHANNEL_DEPTH`/`H2_REQ_CHUNK_MAX`.
- M-B egress: `lb_io::http2_pool::Http2Pool::send_request(addr, Request<H2ReqBody>)`
  (http2_pool.rs:225) where `H2ReqBody = BoxBody<Bytes, Box<dyn Error+Send+Sync>>`
  (75). Pool internally dials/handshakes/multiplexes and **evicts on Send error**
  (ROUND8-L7-10). `Http2PoolError::{Dial,Handshake,Send,Timeout}`.
- H2Proxy already holds `h2_upstream: Option<Arc<Http2Pool>>` (137) with
  `with_h2_upstream` setter (590); guard at 1901.

## RESOLVED open questions (lead decisions — do NOT re-decide)

### Q-HH-1 (dedup vs mirror) → **MIRROR. Defer extraction to CF-DEDUP-1.**
Build a new streaming request path for H2→H2 by MIRRORING the M-D
lookahead+Branch-A+Branch-B+pump orchestration into the H2→H2 path. **Do NOT
edit `proxy_request` (the BUILT/promoted H2→H1 cell)** — extracting a shared
generic pump would re-open a promoted cell for re-verification (S9 Q-H1
precedent: mirror to protect BUILT cells). REUSE the leaf helpers and consts
(they are already free functions/consts; reusing them is not editing the BUILT
pump): `PumpAbort`, `validate_request_trailers`, `concat_chunks`,
`build_h2_body_with_trailers` (for Branch A), the gauge consts, `record_retained`.
The ONLY duplicated code is the orchestration. **R12 divergence guard:** the
verifier MUST run the identical F-MD-4 / F-CAP-1 / memory / backpressure proof
battery against H2→H2 that exists for H2→H1; any behavioural divergence on a
shared scenario is a blocker. CF-DEDUP-1 (unify once both cells have independent
regression locks) remains a dedicated future-session task.

### Q-HH-2 (trailers end-to-end) → forward + reject-pseudo, both legs.
- Request leg: the mirrored pump calls `validate_request_trailers` on the
  inbound trailers frame BEFORE forwarding (same as M-D); a pseudo-header in
  trailers → `Err` → `PumpAbort` injected → upstream RST (never complete).
  Legit trailers ride the channel as the terminal `Frame::trailers`.
- Response leg: upstream response trailers flow through the boxed `Incoming`
  body naturally (hyper relays the terminal trailers frame) — no collect needed.
- Proof: a real-wire trailers test (legit fwd + forbidden-pseudo reject).

### Q-HH-3 (M-B multiplexing / GOAWAY / send_timeout) → no regression; bound check.
- Eviction-on-Send-error is M-B's existing ROUND8-L7-10 behaviour; the pump
  injecting `PumpAbort` → Send error → eviction is the SAME path H3→H2 exercises.
  Verifier confirms H3→H2 real-wire tests still pass (R3).
- **send_timeout bound:** `Http2Pool` wraps the header roundtrip in its
  `send_timeout` (default 30 s). For a backpressured streaming upload where the
  backend defers its response head until body-complete, the roundtrip can
  approach body-transfer time. BUILD CHECK: the proof harness must configure the
  pool `send_timeout` ≥ the test's expected transfer time (the 48 MiB
  backpressure probe), so a deliberate stall does not spuriously time out. The
  product default (30 s) is acceptable; this is a TEST-CONFIG note, not a fork.

## Net-new build (two legs, R8)

### Leg 1 — request (M-D mirror → M-B egress)
Replace the buffering `translate_h2_request_to_h2` body-collect with the M-D
pump. Differences vs the H2→H1 pump (the ONLY deltas):
1. **Preamble:** H2→H2 keeps the request HTTP/2-shaped for the H2 upstream.
   Run the `create_bridge(Http2, Http2)` header normalization (lowercase regular
   headers, keep pseudo-headers) on `parts.headers` BEFORE the pump. Do NOT force
   HTTP/1.1 or strip CL/TE the way the H1-egress pump does (that was an
   H1-framing fix; H2 upstream framing is hyper's H2 encoder's job). Build the
   upstream `Request` parts with method/uri/authority/scheme + normalized headers.
2. **Body type:** the Branch-B `StreamBody` error type maps `PumpAbort` →
   `Box<dyn Error+Send+Sync>` so the body is `H2ReqBody`. (`PumpAbort: Error`
   already.) Branch A uses `build_h2_body_with_trailers(...).map_err(Into::into)
   .boxed()` → `H2ReqBody`.
3. **Egress call:** `h2_pool.send_request(backend_addr, Request<H2ReqBody>)`
   instead of dialing an H1 sender. NO `conn_handle` to manage (the pool owns the
   connection/driver). The pump still runs as a spawned task concurrently with
   `send_request` (hyper must pull the channel for the pump to progress under
   backpressure).
4. **F-CAP-1 caller arm:** on `Err(Http2PoolError::Send(_))` (and any non-Timeout
   send error), await `verdict_rx` BOUNDED by `self.timeouts.body` and prefer a
   classified `BodyTooLarge`→413 / `BadRequest`→400 over the generic 502.
   `Http2PoolError::Timeout` → 504. Genuine Dial/Handshake/Send (verdict Ok or
   absent) → 502. This is the mirror of `h2_proxy.rs:1822–1849`.
5. **Validate-before-RESPONSE-relay gate:** identical to M-D (1857–1878) — relay
   the response head only after the pump's verdict is `Ok(())`; on `Err` abort
   (do not relay the backend response body); pump-vanished → BadRequest.
6. Branch A (≤window) keeps validate-before-dial: zero pool contact for a
   malformed within-window request (the F-COR-1 ordering invariant).

### Leg 2 — response (convert collect → streaming relay)
Replace `upstream_h2_response_to_h2`'s `body.collect().await` with a
streaming relay (mirror `finalize_response` + H2→H2 header normalization):
- Take `(parts, body)` from the upstream `Response<Incoming>`.
- Normalize response headers (lowercase, drop `:`-prefixed; H2→H2 has no
  hop-by-hop strip beyond what the bridge did — match `H2ToH2Bridge::bridge_response`
  semantics: lowercase regular headers).
- Add alt-svc if configured.
- `body.boxed()` (the `Incoming` body) — streaming-by-construction; trailers
  flow through the terminal frame. Backpressure: downstream H2 flow control →
  hyper stops pulling the upstream `Incoming` → upstream H2 flow control.
- This removes the response-leg `collect()` (R8 violation #2). No owned
  intermediate body buffer; memory bounded by hyper's window, by construction.

## BUILT bar for H2→H2 (verifier, independent; author≠verifier)
Real-wire genuine H2 client → real `h2_proxy` listener → router → real H2
backend (`Http2Pool`), binary (non-UTF8) bodies byte-identical BOTH directions:
1. **Request leg** (full M-D battery, mirror of H2→H1):
   - Byte-identity Branch A (≤64 KiB window) and Branch B (>window): 1 KiB,
     5 MiB, 8 MiB → 200, bodies verbatim at the backend.
   - Non-vacuous memory: in-situ retained ≈ 1 window ≤ 256 KiB ≪ body; inverted
     probe load-bearing (trips when the bound is lowered below the window).
   - Backpressure: large body (≈48 MiB), backend pause → retained ≪ body, resume
     → full + 200.
   - **F-MD-4**: real-wire client RST_STREAM mid-body → backend observes
     `complete=0` (truncated request NEVER presented complete), `dials`/attempt
     non-vacuous.
   - **F-CAP-1**: streaming over-cap (>64 MiB) → **413** (NOT 502); forbidden
     pseudo-header trailer → **400** (NOT 502); genuine upstream failure → 502.
2. **Response leg** (the converted egress):
   - Large response body (≈8–48 MiB) byte-identical at the client.
   - Response-leg backpressure: downstream client stalls reading → bounded
     retained memory (no whole-response buffer; `collect()` gone).
   - Trailers relayed (Q-HH-2).
3. **R3 no-regression:** H3→H2 (M-B's other consumer), H2→H1, H1→H1 real-wire
   suites intact; full `--workspace --all-features` green.
4. **Coverage:** ≥80% on the H2→H2 session sub-metric (verifier re-measured with
   the SCOPED llvm-cov per R10/CF-DISK-1), BINDING sub-metric not whole-file.
5. fmt/clippy clean; F-MD-4 + F-CAP-1 regression tests INSIDE the ×3 gate (R10).

## Increment plan (each: plan-approved → build → independent verify → commit → push)
- **I1**: Leg-1 request pump (mirror M-D → M-B) replacing
  `translate_h2_request_to_h2` collect; build + compile; builder smoke.
- **I2**: Leg-2 response streaming relay replacing `upstream_h2_response_to_h2`
  collect.
- **I3**: F-CAP-1 caller arm + verdict gate on the H2→H2 path.
- **V**: verifier — real-wire proof suite (both legs) + F-MD-4 + F-CAP-1 +
  coverage re-measure + R3 sweep. Author≠verifier strictly.
(I1–I3 are one builder's serialized same-file work on `h2_proxy.rs`; may be
combined into one build increment if cohesive, but each must compile + the
verifier owns the proofs.)

## NEW carry-forward surfaced at plan time
- **CF-RESP-1 (NEW):** the response-leg `collect()` buffering also exists in the
  H2→H3 response path (`h3_response_to_h2`, 2315) and likely the H3-front
  response paths — track for the remaining unbuilt cells (H2→H3, H1→H3). Not in
  S10 scope beyond H2→H2's own leg. (Relates to the H3 program "response-egress
  headline NOT started" note.)
