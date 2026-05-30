# S19 — B5 + B6 Plan (builder-2 owns both)

Status: APPROVED design intent (lead-authored). builder-2 confirms
implementation specifics in a short pre-impl note, then implements AFTER
B4 has landed (B5 touches `run_dual_pump` in `raw_proxy.rs` — same file as
B4, so serialized per "same-file work serialized"). verifier verifies
independently. No source change before its predecessor is green.

---

## B5 — bounded-state flood test (+ explicit per-stream cap)

### Current state (recon)
- **Connection state**: router enforces `max_connections` (default 100_000;
  `2 * max_connections` dispatch entries) — new Initials **dropped** at cap
  (router.rs:317-329). Entries removed when the actor finishes/idle-times-out
  (router.rs:264). A cap-branch test already exists (router.rs:450). No LRU
  eviction — the policy is reject-new-at-cap (memory bound = cap).
- **Per-stream state**: `streams: HashMap<u64, RawStreamState>` in
  `run_dual_pump` (raw_proxy.rs:396). Currently bounded ONLY implicitly by
  quiche's `initial_max_streams_bidi(16)`. No explicit relay-side cap. The R8
  comment (raw_proxy.rs:481) already promises "B5 caps the table size."

### Design
1. **Explicit per-stream cap** `MAX_RELAY_STREAMS` in `raw_proxy.rs`
   (defense-in-depth, independent of the quiche config so a mis-set
   `max_streams` can't make the table unbounded). In `relay_streams`, when
   inserting a NEW sid via `streams.entry(sid).or_default()`, gate on
   `streams.len() < MAX_RELAY_STREAMS`; over cap → do NOT create a new entry
   (the stream is not relayed; quiche's own MAX_STREAMS already caps the
   client, so over-cap is only reachable if mis-configured — fail safe,
   bounded, logged). Default `MAX_RELAY_STREAMS` industry-safe and matched to
   the quiche stream-grant order of magnitude (R7 pre-auth); documented. The
   per-connection relay memory bound is then exactly
   `MAX_RELAY_STREAMS * 2 * STREAM_RELAY_WINDOW` (completes the R8 comment).
2. **Connection-flood test**: drive the router with > `max_connections`
   distinct Initials; assert the dispatch table never exceeds
   `2 * max_connections`, new Initials are dropped (not OOM/panic), and the
   bound holds. Strengthen/extend the existing cap test, don't weaken it.
3. **Stream-flood test**: a real-wire client opens streams up to / beyond the
   granted limit rapidly; assert the relay table stays bounded
   (≤ `MAX_RELAY_STREAMS`, and in practice ≤ granted `max_streams`), and that
   completed streams are reclaimed (`streams.retain` — eviction-under-load),
   so memory does not grow with total stream count over the connection's life.

### Verification (R8 + R13 a/b/c on the eviction-under-load path)
- (a) inside the ×3 gate. (b) isolation-burst ≥50 iters of the flood path.
- (c) LOAD-BEARING NEGATIVE CONTROL: a variant WITHOUT the cap / WITHOUT
  `streams.retain` reclamation grows the table under flood; the bounded
  version holds. The test must FAIL pre-fix and PASS post-fix.
- The flood test asserts **bounded memory / table size**, not mere "no panic".

---

## B6 — main.rs wiring + quic_modeb_* metrics + the two security proofs

### B6.1 Config (lb-config)
Add an optional Mode-B block to `QuicListenerConfig` (e.g.
`raw_proxy: Option<RawQuicProxyConfig>`), with: backend `addr`, `sni`,
datagram queue caps (`dgram_queue_cap`, default 1024) and `max_relay_streams`
(B5 default). When present, the QUIC listener runs Mode B (terminate +
re-originate); when absent, it runs H3-terminate exactly as today (R3:
H3 path byte-identical — only Mode-B listeners enable datagrams / set
`raw_quic_backend`).

### B6.2 Wiring (lb/src/main.rs + lb-quic listener/router params)
Mirror `spawn_passthrough`/`spawn_quic`: when `raw_proxy` is configured,
build a `QuicUpstreamPool` with a `config_factory` that (verify_peer, loads
trust, mirrors ALPN per existing dial, `enable_dgram(true, cap, cap)`), build
`RawBackend { pool, addr, sni }`, and thread it into `QuicListenerParams` →
`RouterParams::raw_quic_backend = Some(...)`. The client-facing
`build_server_config` (listener.rs) gains an `enable_datagrams` parameter set
true ONLY for a Mode-B listener. Mode B reachable end-to-end through the real
entry point.

### B6.3 Metrics (lb-observability, mirror passthrough_metrics.rs)
New `QuicModeBMetrics` registered off the shared `MetricsRegistry`:
- `quic_modeb_connections` (IntGauge) — active Mode-B proxied connections.
- `quic_modeb_connections_total` (IntCounter) — established two-conn relays.
- `quic_modeb_datagrams_relayed_total` (IntCounter, by direction or summed).
- `quic_modeb_datagrams_dropped_total` (IntCounter) — the B4 drop-newest
  counter surfaced.
- `quic_modeb_streams_active` (IntGauge) — relay table size (B5 bound).
Threaded RouterParams → ActorParams → relay. (B4 leaves the `dropped` counter
plumbed so B6 just exposes it.)

### B6.4 SECURITY PROOFS (by-construction bar — the S15 no-decrypt precedent)
1. **TWO-CONNECTIONS (structural, verifier-inspected)**: the verifier drives a
   real wire path and asserts via `RawProxyOutcome` that `client_scid !=
   upstream_scid` and `client_trace_id != upstream_trace_id` — two distinct
   `quiche::Connection` objects with independent key schedules. The
   architecture (actor-owned 1:1, separate `quiche::accept` server conn +
   `dial_dedicated` client conn) STRUCTURALLY cannot have fewer than two.
   Proof by mechanism, not assertion.
2. **0-RTT-REJECTION (wire test)**: a real client ATTEMPTS 0-RTT/early data;
   the LB does NOT act on it before handshake completion — the connection
   completes via 1-RTT and early data is not forwarded as early. The
   client-facing config sets NO `enable_early_data` (verified absent), so the
   server never grants 0-RTT. `ZeroRttReplayGuard` stays as defence-in-depth.
   This is a SECURITY claim — PROVEN by wire test, never asserted
   (the trusted_cidrs-stub trap).

### Verification
- ×3 gate green incl. the new wiring/metrics. R3: H3-terminate path unchanged
  (a config without `raw_proxy` produces byte-identical behaviour).
- Both security proofs in the report by mechanism, citing the completed test
  logs (R15).
- Scoped llvm-cov ≥80% on session code (verifier re-measured).

## Files touched (B5+B6)
- B5: `crates/lb-quic/src/raw_proxy.rs` (MAX_RELAY_STREAMS, cap in
  `relay_streams`), `crates/lb-quic/tests/` (connection-flood, stream-flood).
- B6: `crates/lb-config/src/lib.rs`, `crates/lb/src/main.rs`,
  `crates/lb-quic/src/{listener.rs,router.rs,conn_actor.rs,raw_proxy.rs}`
  (metrics threading + enable_datagrams param),
  `crates/lb-observability/src/{lib.rs,quic_modeb_metrics.rs}`,
  `crates/lb-quic/tests/` (two-connections + 0-RTT-rejection wire proofs).
