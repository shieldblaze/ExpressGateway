# SESSION 16 — Mode B (terminate-and-re-originate QUIC proxy) — PLAN

> Base: `feature/quic-proxy-s16` @ 30cc22f2 (= main, S15 promoted).
> Mode A (passthrough, S15) routes QUIC by CID without decrypting.
> **Mode B** TERMINATES the client QUIC connection (reusing the existing
> quiche H3-termination stack), proxies **raw** QUIC streams + datagrams,
> and RE-ORIGINATES a fresh QUIC connection upstream. Two distinct QUIC
> connections, not a bridge.

---

## 0. Phase 0 baseline (filled at end of Phase 0)

- Base tip = main @ 30cc22f2 — CONFIRMED.
- Strays: none (only system `cron`); 33 GB free; eg-target 18 GB shared
  (CARGO_TARGET_DIR must be exported per-command — NOT in config.toml).
- compile (`--all-features --no-run`) exit 0; `fmt --check` clean; `clippy
  --workspace --all-features --all-targets -D warnings` clean.
- ×3 gate: RUN1/2/3 each **1349 passed / 0 failed** (deterministic, 8-core).
- fcap1 disposition: CF-SATURATION-1, mechanism captured, RESOLVED (see §6).

---

## 1. Architecture seam (what reuses, what is net-new)

Mode B reuses the **entire** client-facing termination machinery; the only
divergence is at the post-handshake processing step.

**Reused unchanged (R12 single-source):**
- `InboundPacketRouter` (`lb-quic/src/router.rs`) — UDP listener, DCID
  demux into per-connection actor mpsc channels, Retry mint/verify,
  0-RTT replay guard, `quiche::accept_with_retry`, the 2×cap connection
  bound + `CidEntryGuard` cleanup.
- `RetryTokenSigner` (`lb-security/src/retry.rs`), `ZeroRttReplayGuard`
  (`lb-security/src/zero_rtt.rs`).
- `QuicUpstreamPool` (`lb-io/src/quic_pool.rs`) dial machinery
  (`quiche::connect` + handshake drive) for the re-origination leg.
- SHARED-1 `public_header::parse_public_header`, SHARED-2
  `udp_dataplane::{UdpDataplane, TokioUdp}`.
- The actor's low-level connection pump: `drain_conn_send`,
  `conn.recv`, `conn.on_timeout`, the select-loop skeleton.

**Net-new (S16):**
- `lb-quic/src/raw_proxy.rs` — `run_raw_proxy_actor` + `RawStreamRelay`
  + `RawDatagramRelay` + bounded per-stream state table.
- `ActorParams.raw_quic_backend: Option<RawBackend>` and the
  `RouterParams` field that feeds it; `run_actor` early-dispatches to
  `run_raw_proxy_actor` when set (H3 path otherwise UNTOUCHED — R3).
- A dedicated (non-pooled) upstream dial: one upstream `quiche::Connection`
  per client connection (1:1). Reuses the pool's dial/config (extend
  `QuicUpstreamPool` with a `dial_dedicated()` rather than duplicate it —
  R12).
- `enable_dgram` on both client-facing and upstream configs.
- `quic_modeb_*` observability counters (mirrors `quic_passthrough_*`).
- Mode B wiring in `lb/src/main.rs` (per-listener config).

**The splice point** is the top of `run_actor` (conn_actor.rs:139): when
`raw_quic_backend.is_some()`, return `run_raw_proxy_actor(params).await`
before any H3-specific state is built. This keeps the H3 termination path
byte-for-byte unchanged (R3 no-regression) while sharing the pump (R12).

---

## 2. The five design decisions

### 2.1 Connection identity in re-origination state — **CID-based, actor-owned 1:1**

The client connection is identified exactly as today: by its CID in the
router's `DashMap`. The re-origination binding is the **actor's
co-ownership** of two `quiche::Connection` objects — the client-facing one
(handed over by the router after `accept_with_retry`) and the upstream one
(from `dial_dedicated`). **No session token** is introduced: the binding is
in-memory, per-actor, 1:1. This *structurally* yields the two-connections
proof (two distinct quiche objects, distinct SCIDs, distinct TLS keys).

### 2.2 Stream mapping — **identity ID, explicit bounded per-stream state**

QUIC stream IDs are role-quadrant encoded (client/server × bidi/uni). The
LB is *server* to the client and *client* to the backend, so the quadrants
line up under an **identity** map:
- client-conn even (client-initiated) → upstream even (LB-as-client). ✓
- upstream-conn odd (backend-initiated) → client-conn odd (LB-as-server). ✓

We therefore use **identity stream-ID mapping** (no translation table, no
translation bugs) but maintain an **explicit per-stream state entry**
(relay buffers, fin flags, cancellation status) in a bounded table —
needed for R8 per-stream bounding and cancellation tracking. Stream-limit
backpressure (MAX_STREAMS): if the LB cannot yet open the mirror upstream
stream (`stream_send` → `StreamLimit`/`Done`), it pauses reading that
client stream until the backend grants more credit (handled in 2.4/R8).

### 2.3 Cancellation propagation (F-MD-4 analog) — **reset-relayed-as-reset**

The matrix lesson: a client RST/STOP_SENDING mid-body MUST be relayed
upstream as a stream reset, NOT as a clean FIN (else a truncated transfer
is delivered as complete — smuggling/corruption).
- Client RESET_STREAM(code) on stream N → `client_conn.stream_recv(N)`
  yields `Err(StreamReset(code))` → relay calls
  `upstream_conn.stream_shutdown(N, Shutdown::Write, code)` (emits
  RESET_STREAM upstream). **Never** a clean `stream_send(.., fin=true)`.
- Client STOP_SENDING(code) → `upstream_conn.stream_send(N,..)` yields
  `Err(StreamStopped(code))` → relay calls
  `upstream_conn.stream_shutdown(N, Shutdown::Read, code)` (emits
  STOP_SENDING upstream).
- Symmetric backend→client.
- R13 (a) inside ×3 gate, (b) isolation-burst ≥50 iters, (c) load-bearing
  negative control: a relay that maps reset→FIN makes the backend observe a
  clean stream end — the negative-control test asserts the backend saw a
  RESET with the propagated code, so it FAILS pre-fix, PASSES post-fix.

### 2.4 Datagrams (RFC 9221) — **verbatim bidi, per-conn bounded, drop-newest**

- `enable_dgram(true, recv_q, send_q)` on both configs.
- Relay: `client_conn.dgram_recv()` → `upstream_conn.dgram_send()` and
  vice versa, **verbatim** (no reordering, no reframing).
- Bound: per-connection relay queue (default 32, matching §5 Mode-A
  `per_flow_backlog`) + quiche's own `recv_q`/`send_q`. **Full-policy =
  drop-newest** (consistent with design §5.1; for unreliable datagrams the
  application tolerates loss). A `quic_modeb_dgrams_dropped_total` counter
  makes drops observable (no silent loss). Exact quiche 0.28 queue-full
  semantics (drop-front vs `Error::Done` on send) confirmed in B4 build +
  documented.

### 2.5 0-RTT / early data — **OWNER RULING: REJECT in v1, verify by mechanism**

Owner ruling (2026-05-29, this session): **reject 0-RTT in v1**; record full
passthrough as CONSIDERED-AND-REJECTED (not owed v2 debt). Rationale: the
re-originating proxy sends a fresh 1-RTT upstream handshake regardless, so
client 0-RTT can NEVER be 0-RTT to the backend — accepting+buffering (option
B) takes on the hard early-data anti-replay security surface to save only the
client→LB leg's single RTT (LB→backend is 1-RTT either way). Current H3
termination already rejects 0-RTT (no `enable_early_data`), so we inherit
*verified* behavior with zero new surface.

**Binding requirements (Phase 2):**
1. Do NOT call `enable_early_data` on the client-facing config (inherit H3).
   Clients fall back to 1-RTT cleanly.
2. Keep `ZeroRttReplayGuard` active as defence-in-depth (naive Initial-packet
   replay). It's already wired in the router — leave it.
3. **VERIFY the rejection by mechanism** (security property, same bar as the
   S15 no-decrypt proof — an untested "we reject 0-RTT" is the trusted_cidrs
   stub trap): a real-wire test where the client ATTEMPTS 0-RTT early data and
   the LB does NOT accept/act on it as early data — the connection completes
   via 1-RTT and no early data is forwarded upstream before handshake
   completion. → added to the verify bar (§5) and increment B6.
4. Document in s16-report.md + operator docs: 0-RTT rejected in v1; resumed
   clients use 1-RTT (+1 RTT). Full passthrough = considered-and-rejected on
   security-vs-value grounds; revisit ONLY on a concrete latency requirement.
   NOT carried as roadmap debt.

---

## 3. Bounded state (R8) — Mode B contract

| State | Bound | Default | Policy |
|---|---|---|---|
| Tracked connections | `max_quic_connections` (reuse) | 100_000 | router 2×cap drop-new (existing) |
| Upstream conns | 1 per client conn | == conns | dropped on actor exit |
| Per-conn stream table | `max_streams_per_conn` | 256 | new stream past cap → STOP/refuse; audit |
| Per-stream relay buffer | bounded window (NOT body-size) | 64 KiB-class | backpressure (no unbounded buffer) |
| Per-conn datagram queue | `dgram_queue_len` | 32 | drop-newest + counter |

**Backpressure (R8):** the relay uses bounded per-stream copy buffers and
respects quiche flow control on BOTH connections. A slow backend → the
upstream stream's send-window fills → relay stops reading the client
stream → client's flow-control window closes (client pauses). Symmetric for
a slow client. The bounded window is the memory-safety mechanism; a
total-cap (if any) is a separate limit, not the bound (R8).

---

## 4. Increment breakdown (each: plan-approved, author≠verifier, pushed)

- **B0** — this plan + 0-RTT owner ruling. (Phase 1)
- **B1** — actor seam + dedicated upstream dial. Add `raw_quic_backend` to
  Router/ActorParams; `run_raw_proxy_actor` skeleton (handshake both legs,
  established, idle close); `QuicUpstreamPool::dial_dedicated`. Verify:
  H3 path unchanged (re-run H3 suite); a minimal wire test that both legs
  handshake and the actor holds two distinct connections.
- **B2** — `RawStreamRelay`: bidi raw STREAM copy + FIN, identity IDs,
  bounded per-stream buffers, end-to-end backpressure. Verify: multi-stream
  bidi wire test, binary payloads byte-identical; backpressure proof.
- **B3** — cancellation propagation (RESET_STREAM/STOP_SENDING). Verify:
  R13 (a)+(b)+(c) F-MD-4 analog (reset-not-FIN negative control).
- **B4** — `RawDatagramRelay` + bounded queue + drop-newest policy. Verify:
  datagram wire test (verbatim), queue-bound + drop-newest proof, counter.
- **B5** — bounded state hardening: per-stream table cap, connection-flood
  + stream-flood proofs (memory stays bounded). Verify: R13-style flood.
- **B6** — `lb/src/main.rs` wiring (per-listener Mode B config) +
  `quic_modeb_*` metrics + the full real-QUIC wire test + two-connections
  proof (verifier inspects LB connection state, not by assertion).
- **B7** — gates: ×3 --all-features, scoped llvm-cov ≥80% session-code,
  clippy/fmt, Mode A regression (R12), H3 + matrix + XDP intact (R3).

(Increments may merge/split as the build reveals the true seam size. Each
lands on `feature/quic-proxy-s16` with its own verify evidence.)

## 5. Verify bar (R5)

- **Real wire test**: real quiche client ⇄ LB (Mode B) ⇄ real quiche
  backend; multiple bidi streams + datagrams; binary byte-identical.
- **Two-connections proof**: by mechanism — distinct SCIDs, distinct keys,
  two `quiche::Connection` objects in LB state; NOT a CID bridge.
- **0-RTT-rejection proof** (owner-mandated): real-wire test, client attempts
  early data → LB does not act on it before handshake; completes via 1-RTT.
- **F-MD-4 analog**: R13 (a)+(b)+(c), load-bearing negative control.
- **Bounded state**: per-conn + per-stream + datagram-queue, flood-tested.
- **R12**: any SHARED-1/-2 (or pool) change re-verifies Mode A.
- **R3**: H3 termination, 9 matrix cells, XDP, Mode A all intact.

## 6. fcap1 disposition (Phase 0 leftover) — filled after burst

`fcap1_h2_over_cap_upload_yields_413` (tests/h2h1_md_streaming_verify.rs;
H2→H1; cap = 64 MiB, pushes 66 MiB). S15 RUN1 failure: `status=502
written=1507328` (~1.5 MiB ≪ 64 MiB cap) → drain-close before cap, gateway
correctly 502'd; CF-SATURATION-1 class. Re-verified: ×3 gate fcap1 ok 3/3 +
dedicated 12-iter burst under 7×CPU saturation 12/12 PASS, all status=413 with
written ∈ {67174400, 67239936} (every run crossed the 64 MiB cap; cap arm
TAKEN). 15/15 saturated observations correct. RESOLVED, not asterisked.
