# H3/QUIC Path-1 — Dependency-Ordered Build Plan (Sessions 2+)

Derived from `s1-inventory.md`. Branch lineage: `feature/h3-quic-s1` → per-session feature branches.
Principle: nothing depends on an untested foundation. Each session lands code + crate-local tests
that gate `cargo test -p lb-quic` (not only repo-root integration tests).

## Status baseline (entry to S1)

- BUILT (GET/bodyless only): H3→H1, H3→H2, H3→H3, QUIC listener, quiche integration.
- PARTIAL: native QUIC proxy (transport primitives only; no transparent relay/datagram passthrough).
- ABSENT: WebSocket-over-H3 (RFC 9220), gRPC-over-H3.
- Critical cross-cutting gap: **no request-body / DATA-frame forwarding** on ANY H3 upstream path
  (`h3_bridge.rs` bodyless `build_h1_request`; h3→h3 doc "body-less GET"); **no trailer plumbing**
  upstream (`h3_bridge.rs:99` defers to "3b.3b"). This gap blocks gRPC and full WS/streaming.
- Critical test gap: `listener.rs` + `router.rs` accept/RETRY/per-CID dispatch have **0 crate-local
  coverage**; `proptest_header.rs` is feature-gated off by default.

## Dependency graph (build order)

```
S1  QUIC-listener crate-local test coverage  ──┐
S1  H3->H1 bridge foundation hardening        ─┤
                                                ├──> S2  request-body/DATA-frame forwarding
S1 (proptest gate fix)                        ──┘         (unblocks gRPC + streaming)
                                                            │
                       ┌────────────────────────────────────┼───────────────┐
                       v                                     v               v
            S3 H3->H2 + H3->H3 streaming        S4 native QUIC proxy   S5 WS-over-H3
            (full body+trailers)                (transparent relay)    (RFC 9220 ext-CONNECT)
                       │                                                     │
                       └────────────────> S6 gRPC-over-H3 <─────────────────┘
                                                  │
                                                  v
                                          S7 chaos / fault suite
```

## SESSION 1 (this session) — foundation + must-precede-everything

**S1-A. QUIC-listener crate-local test coverage** (gates S2+; closes the 0% finding)
- Add `crates/lb-quic/tests/listener_lifecycle.rs`: spawn REAL `QuicListener::spawn`, assert
  UDP bind + `local_addr`; retry-secret auto-generate at mode `0600` then reload (length check);
  ALPN `h3`/`h3-29` accepted + unknown ALPN rejected; real quiche client handshake to
  `is_established()`; `shutdown()` cancels cleanly and the join completes.
- Add a router accept-path test: two concurrent client connections on one socket → distinct
  per-CID actors (exercise `router.rs` dispatch + `CidEntryGuard`, currently only the cap unit
  test runs). Cover RETRY token round-trip and 0-RTT replay reject per-Initial through the router.
- Fix `proptest_header.rs`: either drop the `#![cfg(feature = "proptest")]` gate for the
  sanity (256-case) budget so it runs in default `cargo test -p lb-quic`, or document the
  feature requirement in CI and ensure CI passes `--features proptest`. Currently it is silent
  dead coverage.
- Exit criterion: `cargo test -p lb-quic` exercises `listener.rs` and `router.rs` accept paths;
  test count rises from 14 with named listener/router lifecycle assertions.

**S1-B. H3→H1 bridge foundation hardening** (prerequisite shape for S2 body work)
- Refactor `h3_bridge.rs::build_h1_request`/`h3_to_h1_roundtrip` to thread an optional request
  body and `content-length`/`transfer-encoding` correctly (implementation in S2; S1 lands the
  seam + a failing/ignored test marker so S2 has a target).
- Add a crate-local H3→H1 e2e (mirror `round8` but assert response status+body verbatim, not
  just probe-hit) so the bridge has a regression gate inside the crate.

## SESSION 2 — request-body / DATA-frame forwarding (HARD PREREQUISITE for gRPC + streaming)

Why first after S1: every remaining capability (full H3→H2/H3, native proxy semantics, WS, gRPC)
needs DATA frames moved across the stream boundary. `s1-inventory.md` documents this is unbuilt
on all three bridges.
- `conn_actor.rs::poll_h3`: stop dispatching on first HEADERS; accumulate DATA frames until FIN,
  thread body bytes into `H3Request`.
- `h3_bridge.rs`: `h3_to_h1_roundtrip` (real CL/chunked body), `h3_to_h2_roundtrip` (stream body
  into hyper H2 request), `h3_to_h3_roundtrip` (forward DATA frames + FIN downstream→upstream).
- Trailers: plumb request/response trailers (the `h3_bridge.rs:99` "3b.3b" TODO).
- Tests: POST-with-body e2e for each of H3→H1/H2/H3 asserting body integrity both directions;
  large-body / multi-DATA-frame / partial-flush cases.
- Exit: bodyful round-trip proven for all 3 bridges in crate-local + repo-root tests.

## SESSION 3 — H3→H2 and H3→H3 streaming completeness

Depends on S2 (body plumbing). Upgrades #2/#3 from "BUILT (GET only)" to full HTTP semantics.
- Bidirectional streaming (server-push-style response streaming, backpressure, flow-control
  window propagation between downstream and upstream QUIC/H2 conns).
- Connection reuse / pooling correctness under concurrent streams (`QuicUpstreamPool`,
  `Http2Pool`) — multiplexed streams on one upstream conn.
- Tests: concurrent multi-stream e2e; slow-backend backpressure; FIN/RST propagation;
  trailer-only responses.

## SESSION 4 — native QUIC proxy (transparent relay)

Depends on S1 (listener/router coverage) + S2 (stream/datagram threading patterns). Independent
of S3/S5/S6 and may run in parallel with S3.
- Define scope: transparent QUIC stream + QUIC DATAGRAM relay between a downstream client conn
  and an upstream QUIC conn WITHOUT H3 frame parsing (distinct from `h3_to_h3_roundtrip`).
- Plumb QUIC DATAGRAM through `QuicListener`→router→actor (today only the `QuicEndpoint` test
  helper touches the datagram API; not on the production path).
- Decide connection-migration / path-validation policy (server currently
  `set_disable_active_migration(true)` — transparent relay may need this revisited).
- Tests: opaque stream relay e2e; datagram relay e2e through the real listener; migration
  behaviour assertion.

## SESSION 5 — WebSocket-over-H3 (RFC 9220 / extended CONNECT)

Depends on S2 (bidirectional stream relay) + S1 (listener coverage). ABSENT today.
- `lb-h3`/`conn_actor.rs`: advertise `SETTINGS_ENABLE_CONNECT_PROTOCOL`; parse extended CONNECT
  (`:method=CONNECT` + `:protocol=websocket`) per RFC 9220 §3; map the H3 bidi stream to a
  WebSocket tunnel.
- Reuse `crates/lb-l7/src/ws_proxy.rs` framing where possible; bridge H3-CONNECT stream to the
  existing WS state machine (currently H2/TCP only; module doc flags RFC 9220 as post-v1).
- Tests: WS echo over H3 e2e; close-code propagation; idle/ping-flood limits over H3 (mirror
  `tests/ws_proxy_e2e.rs` against a `QuicListener`).

## SESSION 6 — gRPC-over-H3

Depends on S2 (body+trailers) AND S3 (bidi streaming) AND S5 patterns (long-lived bidi streams).
ABSENT today; hardest because gRPC needs bidi streaming + HTTP trailers (`grpc-status`).
- gRPC framing pass-through over H3 streams; trailer translation (`grpc-status`, `grpc-message`)
  to/from upstream H2/H3 backends; deadline (`grpc-timeout`) propagation.
- Tests: gRPC unary, server-streaming, client-streaming, bidi-streaming, status translation,
  deadline — over H3 (mirror the existing H2 `tests/grpc_*.rs` against `QuicListener`).

## SESSION 7 — chaos / fault suite

Depends on all functional paths (S2–S6) existing so faults are injected on real datapaths.
- Packet loss/reorder/dup on the QUIC path; 0-RTT replay flood through the real listener;
  Initial flood vs `max_connections` (100_000) cap; upstream conn death mid-stream; RETRY
  storm; cert rotation under load; graceful-shutdown under in-flight streams; idle-timeout
  and migration-disabled edge behaviour.
- Wire into the prod-readiness CI gates (consolidating with the existing CI workflow).

## Parallelisation notes

- S1 must complete before S2 (test gate) — strictly serial.
- S2 must complete before S3, S5, S6 (body/trailer prerequisite) — strictly serial at S2.
- S4 (native proxy) depends only on S1+S2 → can run in parallel with S3.
- S5 depends on S2; can run in parallel with S3/S4.
- S6 depends on S2+S3+S5 → latest functional session.
- S7 last (needs S2–S6 datapaths).
