# S6 — 9-Cell H-Matrix Inventory + Dependency-Ordered Build Plan

- Auditor: `auditor` (R9 worktree)
- Branch: `feature/h-matrix-s6`
- Content pin: `26ddd43e` ( == verified S5 tip `9c02b08e`; `git diff --stat 9c02b08e 26ddd43e` is empty )
- Scope: READ-ONLY. No source modified.
- Date: 2026-05-18

## 0. TL;DR

| | →H1 backend | →H2 backend | →H3 backend |
|-------------|-------------|-------------|-------------|
| **H1 front** | PARTIAL | PARTIAL | PARTIAL |
| **H2 front** | PARTIAL | PARTIAL | PARTIAL |
| **H3 front** | **BUILT** | PARTIAL | PARTIAL |

Exactly **1 of 9 BUILT** (H3→H1), **8 PARTIAL**, **0 ABSENT**. Every PARTIAL cell exists and proxies functionally end-to-end on a real wire (proven by passing tests), but none meets the R8 bounded-incremental bar: their body path is buffered (`.collect()` / `Limited::new(...).collect()` / `body.collect()`), i.e. memory is bounded by a *total-body cap*, not a *fixed in-flight window*, and there is no end-to-end backpressure or non-vacuous memory proof. No cell is ABSENT — the wiring and real-wire functional tests exist for all 9.

**Recommended Phase 2 next cell: H3→H2** (then H3→H3), because they reuse the already-verified S1–S5 H3 frontend ingress + bounded request machinery (`conn_actor` + `StreamRxBuf` + `request_h3_upstream` shape) that H3→H1 already proved, so the only new work is the *upstream* H2/H3 streaming egress — the smallest delta to a second BUILT cell and it builds the shared "streaming upstream connector" machinery the H1/H2-front cells will later depend on.

## 1. Architecture / real data path map

There are **two distinct request planes** in the codebase; only one is the R8 streaming core.

### 1a. The `lb-l7` `Bridge` trait — header transform ONLY, NOT the streaming core
`crates/lb-l7/src/lib.rs:82-83,122-123` — `BridgeRequest`/`BridgeResponse` both carry `body: bytes::Bytes` (a fully-materialised buffer). All nine `crate::*_to_*` modules (`h1_to_h1.rs`, `h1_to_h2.rs`, `h1_to_h3.rs`, `h2_to_h1.rs`, `h2_to_h2.rs`, `h2_to_h3.rs`, `h3_to_h1.rs`, `h3_to_h2.rs`, `h3_to_h3.rs`) implement only `bridge_request`/`bridge_response`, which clone `req.body`/`resp.body` (`Bytes`) — pure header/pseudo-header rewrite. These are **not** the streaming path. The `crates/lb-l7/tests/bridging_*.rs` and workspace `tests/bridging_*.rs` files are pure unit tests (e.g. `tests/bridging_h3_h1.rs:4-21` builds a `BridgeRequest` in memory and calls `bridge_request`) — **no socket, no listener, NOT real-wire**, so they never count toward BUILT (R5).

### 1b. Front listeners
- H1 / H2: `crates/lb/src/main.rs` binds TCP listeners; terminates into `lb_l7::h1_proxy::H1Proxy` / `lb_l7::h2_proxy::H2Proxy` (`main.rs:62-63,299-308`).
- H3: `crates/lb-quic/src/listener.rs` + `router.rs` (`QuicListener`, `spawn_router`) → per-CID `conn_actor`.

### 1c. Router / bridge / backend connectors
- H3 plane: `crates/lb-quic/src/conn_actor.rs::poll_h3` (line 702) dispatches per-request on backend kind:
  - H1 backend → `h3_to_h1_stream_resp` (`h3_bridge.rs:1915`) — the **R8 streaming path** (request: `write_h1_request` one bounded chunk at a time, `h3_bridge.rs:1672`; response: `stream_h1_response`, `h3_bridge.rs:1186`).
  - H2 backend → `h3_to_h2_roundtrip` (`h3_bridge.rs:2248`) — **buffered** (`body.collect().await`, `h3_bridge.rs:2287`; request body is hard-wired empty `Full::<Bytes>::new(Bytes::new())`, line ~2271).
  - H3 backend → `h3_to_h3_roundtrip` (`h3_bridge.rs:2311`) — **buffered**; accumulates `decoded_body: Vec<u8>` unbounded; doc explicitly: "Request-body forwarding is not supported."
- H1/H2 plane: `lb_l7::h1_proxy::H1Proxy::handle`/`h2_proxy::H2Proxy` branch on `UpstreamProto` (`upstream.rs:35`):
  - `h1_proxy.rs:1024-1032`: H1→{H1:`proxy_request`@1073, H2:`proxy_h1_to_h2`@1136, H3:`proxy_h1_to_h3`@1165}
  - `h2_proxy.rs:1161-1180`: H2→{H1:`proxy_request`@1288, H2:`proxy_h2_to_h2`@1415, H3:`proxy_h2_to_h3`@1440}

## 2. The H3→H1 "BUILT bar"

H3→H1 is BUILT. Path: H3 listener → `router` → `conn_actor::poll_h3` (`conn_actor.rs:846`) → `h3_to_h1_stream_resp` (`h3_bridge.rs:1915`) → real TCP H1 backend via `lb_io::TcpPool`.

What makes it BUILT (the bar every other cell must clear):

1. **Request body bounded-incremental.** `write_h1_request` (`h3_bridge.rs:1672`) forwards "ONE chunk here — bounded, never the whole body" from the inbound H3 DATA frames via a bounded mpsc; a slow upstream stalls the recv loop → request-side backpressure (`h3_bridge.rs:1721`). Memory is bounded by a fixed in-flight window (`MAX_INFLIGHT_BODY_BYTES`, `h3_bridge.rs:1573`; `MAX_RETAINED_BODY_BYTES`), NOT by body size and NOT by a total-body cap.
2. **Response body bounded-incremental.** `stream_h1_response` (`h3_bridge.rs:1186`) reads only to the head terminator, then streams body chunks over a bounded channel back to the actor; a stalled H3 client → channel full → upstream `read()` not called → TCP backpressure to the H1 upstream (`h3_bridge.rs:1152-1171`).
3. **Non-vacuous memory proof (real-wire).** `crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs`:
   - `r2_response_memory_bounded_through_stalled_client` (line 1099): real quiche H3 client + real TCP backend; 1 MiB response with a stalled client; asserts the live `MAX_RETAINED_RESP_BYTES` gauge `<= RESP_RETAINED_CEILING` (ceiling = `4 × depth×(chunk+hdr)` = 262 656 B, asserted authoritative at line 1122), AND clean FIN + byte-identical after resume (liveness — not vacuous).
   - `r3_slow_client_backpressures_upstream_read` (line 1206): 4 MiB body, slow client; proves the causal backpressure chain (retained ≤ ceiling for a body 16× the ceiling).
   - Request side: `crates/lb-quic/tests/h3_h1_stream_body_e2e.rs::t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream` (line 790) — same shape for the request direction, gated on `--features test-gauges`.
4. **Real-wire tests CURRENTLY PASS (this audit, at `26ddd43e`):**
   - `cargo test -p lb-quic --features test-gauges --test h3_h1_resp_stream_e2e` → **16 passed; 0 failed; 0 ignored** (incl. R1, R2, R3, R4, R5, R6, R7, R8-trailers, C2, C3).
   - `cargo test -p lb-quic --features test-gauges --test h3_h1_stream_body_e2e` → **6 passed; 0 failed; 0 ignored** (incl. T5 memory bound).
   - Note: the in-file comment at `h3_h1_resp_stream_e2e.rs:1049` claiming R1..R8 are "`#[ignore]`d SCAFFOLD ONLY" is **stale** — there are no `#[ignore]` attributes in the file and all 16 tests run and pass. The R2/R3/T5 gauges require the `test-gauges` feature to compile (`crates/lb-quic/Cargo.toml:59`); without it, `--test h3_h1_resp_stream_e2e` fails to compile (E0432 on `MAX_RETAINED_RESP_BYTES`). The Phase-2 R8 gate MUST run with `--features test-gauges` or it silently skips the only non-vacuous memory proof.

The bar: **both** request and response bodies bounded by a fixed in-flight window (not total-body cap), end-to-end backpressure, AND a currently-passing real-wire test with a non-vacuous memory assertion + liveness.

## 3. 9-Cell table

| Cell | Status | Evidence (file:line, test, pass/fail) | R8 conformance | Exact gap |
|---|---|---|---|---|
| **H3→H1** | **BUILT** | `h3_bridge.rs:1915` (`h3_to_h1_stream_resp`), `:1186` (`stream_h1_response`), `:1672` (`write_h1_request`); test `tests/h3_h1_resp_stream_e2e.rs` 16/16 PASS + `tests/h3_h1_stream_body_e2e.rs` 6/6 PASS (verified this audit, `--features test-gauges`) | YES — req+resp bounded by in-flight window; e2e backpressure; non-vacuous R2/R3/T5 memory proofs | — (reference cell) |
| **H3→H2** | PARTIAL | `conn_actor.rs:794` → `h3_to_h2_roundtrip` `h3_bridge.rs:2248`; real-wire test `tests/proto_translation_e2e.rs::proxy_h3_listener_h2_backend` (line 640) — PASS (5/5 this audit) | NO | Response **buffered**: `body.collect().await` `h3_bridge.rs:2287`. Request body **dropped entirely** — hard-wired `Full::<Bytes>::new(Bytes::new())` `h3_bridge.rs:~2271` (no request-body forwarding). No memory/backpressure proof; e2e is small-body wiring only ("exercises the *wiring*", `proto_translation_e2e.rs:20`). |
| **H3→H3** | PARTIAL | `conn_actor.rs:804` → `h3_to_h3_roundtrip` `h3_bridge.rs:2311`; nearest real-wire: `tests/h3_upstream_verify.rs`, `tests/quic_native.rs` (H3 upstream pool) | NO | Response **buffered**: unbounded `decoded_body: Vec<u8>` accumulation (`h3_bridge.rs:~2390`). Request body explicitly unsupported (doc: "Request-body forwarding is not supported in 3b.3c-3"). No bounded window, no backpressure, no memory proof, no dedicated real-wire H3→H3 body e2e. |
| **H1→H1** | PARTIAL | `h1_proxy.rs:1073` `proxy_request` (hyper H1 client `send_request`, native `IncomingBody` stream); real-wire test `tests/h1_proxy_e2e.rs` 3/3 PASS this audit | PARTIAL | Body streams via hyper `IncomingBody` (not pre-collected here) BUT no R8 in-flight gauge, no fixed-window bound, no end-to-end backpressure proof, **no non-vacuous memory test** (`h1_proxy_e2e` tests are hop-by-hop/alt-svc/slow-body-timeout, not a stalled-client memory bound). Fails R5/R8 bar (no currently-passing real-wire *streaming* proof). |
| **H1→H2** | PARTIAL | `h1_proxy.rs:1136` `proxy_h1_to_h2` → `translate_h1_request_to_h2` `h1_proxy.rs:1606`; real-wire `tests/proto_translation_e2e.rs::proxy_h1_listener_h2_backend` (368) — PASS | NO | Request **buffered**: `body.collect().await` `h1_proxy.rs:1613`. Response **buffered**: `upstream_response_to_h1` `body.collect().await` `h1_proxy.rs:1727`. Memory bounded by total-body, not in-flight window. No backpressure, no memory proof. |
| **H1→H3** | PARTIAL | `h1_proxy.rs:1165` `proxy_h1_to_h3` → `collect_h1_request_to_h3_fieldlist` + `lb_quic::request_h3_upstream`; real-wire `tests/proto_translation_e2e.rs::proxy_h1_listener_h3_backend` (513) — PASS | NO | Request **buffered**: `collect_h1_request_to_h3_fieldlist` collects body before dial. Response via `h3_response_to_h1` (buffered). No in-flight window / backpressure / memory proof. |
| **H2→H1** | PARTIAL | `h2_proxy.rs:1161` → `proxy_request` `h2_proxy.rs:1288`; real-wire `tests/h2_proxy_e2e.rs` 3/3 PASS this audit | NO | Request **buffered by TOTAL-BODY CAP**: `Limited::new(body, MAX_REQUEST_BODY_BYTES).collect()` `h2_proxy.rs:1314` (explicit ordering fix: "fully RECEIVE + VALIDATE the inbound request body here, BEFORE dialing"). This is exactly the anti-pattern R8 forbids (bound by total cap, not in-flight window). No backpressure, no memory proof. |
| **H2→H2** | PARTIAL | `h2_proxy.rs:1179` → `proxy_h2_to_h2` `h2_proxy.rs:1415` → `translate_h2_request_to_h2` `h2_proxy.rs:1480`; real-wire `tests/proto_translation_e2e.rs::proxy_h2_listener_h2_backend` (442) — PASS | NO | Request **buffered** (`collect()`; sibling comment at `h2_proxy.rs:1311` confirms "H2→H2 / H2→H3 sibling paths also collect() before forwarding"). Response buffered. No in-flight window/backpressure/memory proof. |
| **H2→H3** | PARTIAL | `h2_proxy.rs:1180` → `proxy_h2_to_h3` `h2_proxy.rs:1440` → `lb_quic::request_h3_upstream`; real-wire `tests/proto_translation_e2e.rs::proxy_h2_listener_h3_backend` (573) — PASS | NO | Request **buffered** (collect before forward, per `h2_proxy.rs:1311` sibling note). Response buffered. No in-flight window/backpressure/memory proof. |

### Test-run ground truth (this audit, at `26ddd43e`)
- `h3_h1_resp_stream_e2e` + `h3_h1_stream_body_e2e` (`--features test-gauges`): **22/22 PASS, 0 ignored**.
- `h3_h1_stream_body_e2e` (no feature): 5/5 PASS but T5 (memory bound) silently absent — feature-gated.
- `proto_translation_e2e`: **5/5 PASS** (H1→H2, H2→H2, H1→H3, H2→H3, H3→H2 — wiring/small-body only).
- `h1_proxy_e2e`: 3/3 PASS. `h2_proxy_e2e`: 3/3 PASS (functional H1→H1 / H2→H1, no memory/backpressure assertion).

## 4. Dependency-ordered build plan

### Shared machinery dependency graph
- **M-A — H3 ingress + bounded request pump** (S1–S5): DONE, verified by H3→H1. Reusable verbatim by H3→H2, H3→H3.
- **M-B — bounded streaming UPSTREAM connector for H2** (hyper h2 client driven incrementally with an in-flight window, replacing `body.collect()` in `Http2Pool` send paths): NOT built. Needed by H3→H2, H1→H2, H2→H2.
- **M-C — bounded streaming UPSTREAM connector for H3** (drive `request_h3_upstream` / `QuicUpstreamPool` with incremental DATA frames + flow control, replacing the unbounded `decoded_body`): NOT built. Needed by H3→H3, H1→H3, H2→H3.
- **M-D — bounded H1 INGRESS pump for the `lb-l7` plane** (stream `IncomingBody` with an in-flight window + R8 gauge instead of `Limited::collect()`): NOT built. Needed by H1→*, H2→* once their upstream connector is bounded. The H2 ingress also has a validate-before-forward ordering constraint (`h2_proxy.rs:1297-1313`) that any streaming rework must preserve (h2spec RST/GOAWAY race).
- **M-E — R8 response egress for the `lb-l7` plane** (stream upstream response instead of `upstream_response_to_h1`'s `collect()`): NOT built. Needed by every H1-front / H2-front cell.

### Recommended order & rationale

1. **H3→H2** (next, Phase 2). Size: **S–M**. Reuses M-A wholesale (proven). Net new = M-B (bounded H2 upstream egress) + H3 response re-encode streaming. Smallest delta to a 2nd BUILT cell; unlocks M-B for three cells. *Also fix the dropped request body* (`Full::new(Bytes::new())`) — currently H3→H2 forwards no request body at all, the most severe functional gap of any PARTIAL cell.
2. **H3→H3** (Phase 2, after/with H3→H2). Size: **M**. Reuses M-A; net new = M-C (bounded H3 upstream connector + flow control) + remove unbounded `decoded_body`. Unlocks M-C for H1→H3/H2→H3. Sequenced second because M-C (QUIC stream flow-control plumbing) is heavier than M-B.
3. **H2→H1** (Phase 3). Size: **M**. Reuses the proven `stream_h1_response`/`write_h1_request` egress (M from H3→H1). Net new = M-D for H2 ingress *preserving the validate-before-forward order* (`h2_proxy.rs:1297`). Highest-value lb-l7-plane cell because the H1-backend egress is already verified.
4. **H1→H1** (Phase 3). Size: **S–M**. Body already streams via hyper; net new = add R8 in-flight gauge + a non-vacuous stalled-client memory test (M-D-lite + M-E). Cheap to promote to BUILT once the gauge/test pattern from H3→H1 is templated.
5. **H1→H2 / H2→H2** (Phase 4). Size: **M each**. Depend on M-B (from step 1) + M-D/M-E.
6. **H1→H3 / H2→H3** (Phase 4, last). Size: **M each**. Depend on M-C (from step 2) + M-D/M-E. Last because they stack the two heaviest unbuilt connectors.

### Phase 2 recommendation
Build **H3→H2 first, then H3→H3**. Rationale: both reuse the S1–S5-verified H3 frontend (M-A) that H3→H1 already proved at the highest bar, so Phase 2 only has to build and prove the *upstream* bounded connectors (M-B then M-C) — the exact shared machinery every remaining lb-l7-plane cell later needs. Each is a small, well-scoped delta from an already-BUILT sibling, and H3→H2 additionally closes a real functional defect (request body silently dropped). Carry forward: any Phase 2 R8 gate MUST be run with `cargo test -p lb-quic --features test-gauges` and must delete/replace the stale `#[ignore]`-claiming comment block at `h3_h1_resp_stream_e2e.rs:1049`.
