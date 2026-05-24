# H1→H3 R8 Plan — S12 head-start (lead-authored, S11 honest-stop deliverable)

- Author: `lead` (S11). Honest-stop deliverable: H1→H3 was NOT built in S11
  (budget consumed by H1→H2 build + the deep independent verification + an
  R3 drain-test flake fix). This is the S12 build spec.
- Cell: **H1→H3** — HTTP/1.1 front ⇒ HTTP/3 (QUIC) backend, plane `lb-l7` +
  `lb-quic`. Status on entry: **PARTIAL (buffering proxy)** — BOTH legs buffer.
- Sibling: **H2→H3** is the last remaining cell and shares the SAME →H3 egress
  connector. R12 applies — see §Sequencing.

## Current-tree reality (lead-read, S11)
- Dispatch `h1_proxy.rs` → `proxy_h1_to_h3` (1948): `collect_h1_request_to_h3_
  fieldlist` (2710) `body.collect().await` → `request_h3_upstream(headers,
  body: Bytes, trailers, addr, sni, pool)` (h3_bridge.rs:2482) → `h3_response_
  to_h1` (2786).
- **Request leg buffers**: `request_h3_upstream` takes a COLLECTED `body:
  bytes::Bytes` and frames it as a SINGLE `H3Frame::Data` (h3_bridge.rs:2511).
- **Response leg buffers**: `H3UpstreamResponse.body` is the whole upstream body
  (`h3_response_to_h1` does `Bytes::from(resp.body)`, 2791). The connector's own
  doc (h3_bridge.rs:51-56) flags `read_h1_response` reads the whole upstream body
  — a multi-GB ceiling. This is CF-RESP-1 for the H3 upstream.
- `request_h3_upstream` is shared by BOTH `proxy_h1_to_h3` (h1_proxy:1969) and
  `proxy_h2_to_h3` (h2_proxy:2315) — converting it (or adding a streaming
  sibling) serves BOTH remaining cells.
- The BUILT **H3→H3** cell does NOT use `request_h3_upstream`; it streams via the
  `conn_actor` path (bounded per-stream body channels, `h3_to_h3_stream_resp`,
  the `AbortKind` RESET-without-FIN abort semantics, h3_bridge.rs:79-143). That
  is the streaming-egress machinery to factor out and reuse.

## KEY INSIGHTS (do not relearn)
1. **H3 decode_frame R8 trap** (memory `h3-decode-frame-r8-trap`, S7 J1):
   `lb_h3::decode_frame` buffers the WHOLE payload; an H3-upstream streaming
   path must manual-parse the frame header via `decode_varint` and stream the
   DATA payload incrementally — do NOT use `decode_frame` on the hot body path.
2. **F-MD-4 on the H3 upstream is RESET-without-FIN** (h3_bridge.rs:79-91): on a
   mid-body downstream truncation, the proxy (acting as CLIENT toward the QUIC
   upstream) must abort the upstream request stream WITHOUT a clean FIN — the
   QUIC analog of H2's `reset_peer` / H1's `conn_handle.abort()`. A clean FIN on
   a truncated request = smuggled-complete. Gate on the QUIC **stream-end (FIN)**
   signal, NOT on `None` alone.
3. **H1 ingress is the MIRROR-IMAGE** (reuse M-D-lite from H1→H1, h1_proxy
   `proxy_request`): inbound H1 body is `Kind::Chan` — `frame()==None` IS the
   positively-confirmed clean end; `Some(Err(IncompleteBody))` is truncation.
   `is_end_stream()` deliberately NOT used. The abort arm (→ RESET upstream
   without FIN) fires on `Some(Err)`, the clean arm (→ clean FIN) on `None`.

## CF-DEDUP-2 (NEW, compelling): extract a shared streaming H3-upstream connector
Both H1→H3 and H2→H3 need: pump a generic inbound body stream → incremental H3
`Data` frames on a bounded QUIC send window → gate the response head on a
validated terminal state → RESET-without-FIN on abort. Factor this as
`stream_request_to_h3_upstream(headers, body_pump, trailers, addr, sni, pool,
timeouts) -> H3UpstreamResponseStream` reusing the H3→H3 conn_actor framing +
`AbortKind` machinery. ONLY the ingress pump differs (H1 M-D-lite vs H2 M-D
framing) — exactly the CF-DEDUP-1 shape from S11's `drive_h2_upstream_send`.
Build the shared connector once; wire H1→H3 (S12) and H2→H3 (same or next
session) onto it. This is heavier than M-B (S7 precedent: M-C QUIC connector is
the heaviest), so budget accordingly.

## Net-new build (two legs, R8)
### Leg 1 — request (H1 M-D-lite ingress → streaming H3 egress)
- Ingress pump = M-D-lite (H1→H1): `None`=clean→clean FIN; `Some(Err)`=truncation
  → abort upstream stream WITHOUT FIN (RESET); trailers validated via
  `validate_h1_request_trailers`; over-cap (MAX_REQUEST_BODY_BYTES) → 413 + abort.
- Egress = the new `stream_request_to_h3_upstream`: each inbound chunk → an H3
  `Data` frame (manual-framed, NOT via `decode_frame`); bounded in-flight window
  (QUIC stream flow control + a fixed channel depth); memory gauge analogous to
  `H1_REQ_MAX_RETAINED_BODY_BYTES` (add an H3-upstream gauge or reuse).
- F-CAP-1 caller arm: classified 413/400 over 502; Timeout→504. Validate-before-
  dial (Branch A) for within-window malformed = zero QUIC contact.

### Leg 2 — response (H3 upstream → H1 front, convert whole-body → stream)
- Replace the whole-body `H3UpstreamResponse.body` with a streaming response: the
  connector yields a bounded per-stream RESPONSE body channel (the H3→H3 path
  already has `h3_to_h1_stream` + the RESPONSE channel depth const, h3_bridge.rs
  ~143) → `body.boxed()` to the H1 client. Apply the H3→H1 head transform
  (status + `RESPONSE_HOP_BY_HOP` + trailer-aware shape). Note: H1 trailer relay
  on a STREAMED response hit CASE (ii) in S11 (hyper drops trailers absent a head
  `Trailer:` declaration; bounded behaviour, matches nginx). The same applies
  here — do NOT reintroduce a collect to capture trailer names. (CF-RESP-1.)

## BUILT bar (verifier, independent; author≠verifier) — same shape as S11:
real-wire H1 client → real h1_proxy listener (h3_upstream wired) → real H3/QUIC
backend; binary bodies byte-identical both directions; non-vacuous memory gauge +
load-bearing inverted probe; backpressure both legs; F-MD-4 H3 mirror-image with
R13 (a) in-gate + (b) isolation-burst ≥50× + (c) load-bearing negative control
(gate on FIN, prove RESET-without-FIN on truncation); F-CAP-1 413/400/502;
coverage ≥80% session sub-metric (scoped llvm-cov); fmt/clippy; R3 intact.

## Sequencing
- S12: extract the shared streaming H3-upstream connector (CF-DEDUP-2) + build
  H1→H3 + re-verify. If budget allows, also wire **H2→H3** onto the same
  connector and verify → **9 of 9 BUILT** (matrix complete). If not, H2→H3 is the
  S13 head-start (its plan is then trivial — same connector, H2 M-D ingress).
- R12: when the shared connector lands, the H3→H3 cell's egress should be
  reviewed for convergence onto it (or explicitly justified as divergent).

## Carry-forwards reinforced
- CF-RESP-1: H3 upstream response whole-body read (`read_h1_response` multi-GB
  ceiling, h3_bridge.rs:51-56) — the response-leg streaming above closes it for
  H1→H3; H2→H3 (`h3_response_to_h2`) shares the issue.
- CF-DEDUP-2 (new): shared streaming H3-upstream connector across the 3 →H3 cells.
- N-1 jumbo-MTU xdp.frags deployment doc (unrelated, owner).
