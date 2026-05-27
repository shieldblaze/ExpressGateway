# S13 builder-1 brief — H2→H3 build (LEAD-APPROVED)

**Plan APPROVED** (`s13-h2h3-plan.md`) with the §3 amendment below. You are cleared
to make source changes per this brief. Author≠verifier (R5): you build the cell +
its test suite; the **verifier** independently re-runs, mutates, and re-measures.

## Working environment
- Worktree: `/home/ubuntu/Code/eg-wt-builder1` (branch `feature/h-matrix-s13`, base
  `main` @ `85017edc`). Do ALL work here.
- **Every** cargo invocation: `export CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`
  (R9 shared target). No sudo. Keep ≥25 GB free (`df -h /home/ubuntu`); if tight,
  `rm -rf $CARGO_TARGET_DIR/debug/incremental` (regenerates).
- Commit every increment; push continuously to `origin/feature/h-matrix-s13`.
- **NO `git stash`** (R9 — shared repo). WIP-commit instead.
- R8 is binding: NO full-body buffering anywhere. Bounded in-flight window only.

## §3 CONNECTOR-CONTRACT — RESOLVED (lead). This shapes Hazard (a).
The connector's dropped-`body_tx` semantics are **STATE-DEPENDENT** (read
`lb-quic/src/h3_bridge.rs`):
1. **First peek before ANY event** (`:3309-3313`): `body_rx.recv()==None` → **bodyless
   COMPLETE request (HEADERS+FIN)**. DANGEROUS — a pump dropped here smuggles a
   truncated request as complete.
2. **At `AwaitNext` mid-body** (`j2_req_event_action` `:4128`,
   `Some(Reset)|None => AbortNoFin`): dropped → RESET-without-FIN (safe).

**Therefore the Hazard (a) mitigation is LOAD-BEARING, not optional:**
- The H2-ingress pump MUST be **detached** (`tokio::spawn`, mirror
  `h1_proxy.rs:2034`) so a downstream H2 `RST_STREAM` cancelling the *service*
  future does NOT drop the pump. (Dropping a `JoinHandle` detaches, does not abort.)
- The pump MUST **always emit an explicit terminal event** (`End{..}` or `Reset`) —
  never let `body_tx` drop silently, especially before the first event.
- Map the H2 ingress terminal states (mirror `proxy_h2_to_h2_request`
  `h2_proxy.rs:2024-2061`): `frame()==None && !is_end_stream()` → `Reset`;
  `Some(Err)` → `Reset`; `is_end_stream()`/trailers-frame → `End{trailers}`.

## What to build (lb-l7 only; connector is unchanged, reuse verbatim)

Replace the buffering `proxy_h2_to_h3` (`h2_proxy.rs:2308-2335`,
`collect_h2_request_to_h3_fieldlist` → `request_h3_upstream` → `h3_response_to_h2`)
with a streaming both-legs relay, EXACTLY mirroring `proxy_h1_to_h3`
(`h1_proxy.rs:1988-2259`). Dispatch unchanged (`h2_proxy.rs:1250`).

### (a) `build_h2_to_h3_fieldlist(&parts, sni) -> Result<Vec<(String,String)>, String>`
Head-only refactor of `collect_h2_request_to_h3_fieldlist` (`h2_proxy.rs:2756`):
KEEP the `create_bridge(Http2, Http3)` + `:method`/`:path`/`:scheme`/`:authority`
synthesis (lines 2773-2825), DROP the `body.collect()` + trailer capture (those now
stream — pass `body: Bytes::new()`, `trailers: Vec::new()`). Mirror
`build_h1_to_h3_fieldlist` (`h1_proxy.rs:3052`) but H2 pseudo-synthesis.

### (b) H2-ingress → `ReqBodyEvent` detached pump (~120 lines)
Mirror `proxy_h1_to_h3`'s detached pump (`h1_proxy.rs:2032-2146`) BUT source frames
the H2 way (`proxy_h2_to_h2_request` `h2_proxy.rs:2009-2061`): `body.frame()` loop,
`is_end_stream()` disambiguation, `is_data`/`is_trailers`. Emit `ReqBodyEvent`:
- data → split to ≤`H3_BODY_CHUNK_MAX` `Chunk`s (mirror `send_chunked!`); maintain
  the live `in_flight_bytes` gauge (`record_retained` test-gauge).
- over-cap (`MAX_REQUEST_BODY_BYTES`) → `Reset` (pre-data → connector inline-413;
  mid-body → RESET-without-FIN — the F-CAP-1 boundary).
- trailers → validate (`validate_request_trailers`); OK → `End{trailers}`; forbidden
  → `Reset`.
- clean end (`None && is_end_stream()`) → `End{trailers:[]}`.
- reset (`None && !is_end_stream()`, or `Some(Err)`) → `Reset`.
Pass `forward_req_trailers = true` to the connector.

### (c) `H3RespEvent` → H2 response consumer (~90 lines)
Mirror the H1→H3 response relay (`h1_proxy.rs:2169-2258`) — spawned relay task into a
`StreamBody` of `Result<Frame<Bytes>, _>`; `Head` → head builder; `Body` →
`Frame::data`; `Trailers` → `Frame::trailers`; `End` → drop tx (clean); `Reset` →
inject `Err(box)` (NO clean terminator — the response-splitting guard). TWO deltas:
1. **Head transform = H2→H2 semantics** (NOT H1's): mirror `upstream_h2_response_to_h2`
   (`h2_proxy.rs:2558-2567`) — drop `:`-pseudo, lowercase regular headers, add
   alt-svc, **NO `RESPONSE_HOP_BY_HOP` strip** (unlike `h3_decoded_resp_head_builder`
   `h1_proxy.rs:2816`). Build a NEW helper from the decoded `(status, headers)`.
   **VERIFY:** confirm hyper's H2 server encoder omits/rejects any connection-specific
   header so we never forward `connection:`/`keep-alive` to an H2 client (flag to lead
   if it forwards them — may need a targeted strip; potential R12 note).
2. **Trailers flush NATIVELY** — `Frame::trailers` on the H2 front is emitted by
   hyper's H2 server encoder with NO `Trailer:` pre-declaration (CF-RESP-1 H1
   constraint is GONE). This is the trailer-mandate WIN: gRPC `grpc-status` reaches
   the H2 client. The same `ClientRespBody` BoxBox type is served by the H2 listener
   (`h2_proxy.rs:777,790`), so the `Err(box)` injection → hyper H2 server emits
   `RST_STREAM` downstream (vs H1's missing `0\r\n\r\n`).

### Wiring (mirror `h1_proxy.rs:2014-2167`)
Detached pump (spawn) → `H3RespOut::Decoded{tx, total:0, cap:MAX_RESPONSE_BODY_BYTES}`
→ spawned `connector_handle` calling `stream_request_to_h3_upstream(headers, true,
addr, &sni, &pool, body_rx, sink)` → drain `resp_rx`: first `Head` → 200-path streaming
response; `None|Reset` before Head → 502 (`pump.abort()` + `connector_handle.abort()`).

## Test suite (new `crates/lb-l7/tests/h2h3_md_streaming_verify.rs`, mirror
`h1h3_md_streaming_verify.rs`). Build to the BUILT bar (plan §5 1-9). The two
LOAD-BEARING arms MUST be load-bearing **from the first author pass**:
- **Hazard (a) request F-MD-4** (R13 a/b/c): downstream H2 client RST mid-body →
  backend sees QUIC `complete==0` (no FIN). Burst ≥50 on `current_thread`. Negative
  control: clean upload → `complete==1`.
- **Hazard (b) response F-MD-4** (R13 a/b/c): truncating H3 backend (partial DATA,
  drop no-FIN), **framed so the GUARD is the sole discriminator** — NO content-length
  on the truncation arm (CL-declared arm masks the guard — [[h1h3-fmd4-teardown-not-reset]]
  / #13b lesson). Assert H2 client sees `RST_STREAM`/errored body, NOT clean
  END_STREAM. A CL arm may exist but label it non-load-bearing. Verifier will run the
  guard-deletion mutation; design so deleting `Reset→Err(box)` FAILS the test.
- Real-wire H2-front → router → bridge → real H3 backend, binary bodies byte-identical
  both dirs (~5 MiB). Memory gauge non-vacuous + load-bearing inverted probe.
  Backpressure both legs. F-CAP-1 (pre-data 413 / mid-body RST / pre-dial 502).
  Trailer-mandate gRPC-shaped POSITIVE assertion. Generous test-backend timeouts
  (CF-SATURATION-1) so 8-core gate saturation can't false-flake.

## Report back to lead
When the cell compiles + your suite passes locally (`-p lb-l7 --all-features` for the
new suite, plus a `--workspace --all-features` smoke of nearby cells for R3), commit,
push, and message the lead with: files touched, line counts, which arms are
load-bearing and how you proved them on your machine, and any divergence/issue (esp.
the §(c)1 connection-header question). Do NOT claim BUILT — that's the verifier's
verdict.
