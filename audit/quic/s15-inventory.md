# SESSION 15 — Phase 1 Inventory (QUIC, read-only)

**Author:** quic-eng
**Branch:** feature/quic-proxy-s15 (worktree off main @ 36ee1227)
**Status:** read-only audit; no source modifications

This document inventories everything QUIC-shaped that already exists in
the tree and decides — per file — what S15 Mode A (passthrough) and S16
Mode B (terminate-and-re-originate) can reuse, what they cannot, and
what is net-new. The R12 single-sourcing question — "what is the
smallest surface both modes need?" — is answered at the bottom.

## 1. What exists today

All of the existing QUIC code is **termination-side**: the LB runs
quiche as a QUIC server, completes the TLS 1.3 + transport handshake,
decrypts packets, and bridges H3 onto an upstream HTTP/1, HTTP/2, or
HTTP/3 backend through `lb_io::*`. There is no in-tree passthrough
proxy today; CID-based steering does not exist; the L4 XDP plane
treats QUIC as 5-tuple-keyed UDP.

### 1.1 `crates/lb-quic/src/lib.rs` — 912 LOC

- **What it does today.** Crate root + a self-contained loopback rig.
  Defines `QuicDatagram`, `QuicStream`, `QuicError`. Exports
  `QuicEndpoint`, `roundtrip_datagram`, `roundtrip_stream`,
  `forward_datagram`, `forward_stream`. Re-exports
  `RetryTokenSigner`, `ZeroRttReplayGuard`, `ConnectionParams`. The
  loopback driver runs a quiche server + client end to end against a
  rcgen-issued cert pair, drives the handshake with `drive()`, and
  exercises DATAGRAM and unidirectional-stream APIs.
- **Reusable for passthrough (Mode A): NONE of the wire path.** The
  whole loopback driver is termination + decryption + quiche state
  machine.
- **Reusable type model:** `QuicDatagram` / `QuicStream` are
  transport-independent payload structs; they are still useful as the
  Mode B (S16) in-process datagram/stream shape after termination.
  Mode A never sees them — it forwards raw UDP bytes.
- **Constants:** `H3_ALPN_PROTOS`, `MAX_UDP_DATAGRAM_SIZE = 65_535`,
  `LB_QUIC_TEST_SNI`. Mode A reuses `MAX_UDP_DATAGRAM_SIZE` as the
  recv buffer bound. ALPN/SNI never apply (Mode A does not parse TLS).

### 1.2 `crates/lb-quic/src/listener.rs` — 368 LOC

- **What it does today.** `QuicListener::spawn` is the production
  termination entry: binds `UdpSocket`, loads/generates the 32-byte
  retry secret on disk (mode 0600), constructs `RetryTokenSigner` +
  `ZeroRttReplayGuard`, builds the `quiche::Config` factory, and
  hands everything to `router::spawn`. `QuicListenerParams` flattens
  `lb_config::QuicListenerConfig` (no dep on `lb-config`).
- **Reusable for Mode A:** The **bind + UDP-socket plumbing pattern**
  (Arc-shared `UdpSocket`, `CancellationToken` driven shutdown, the
  `JoinHandle` lifecycle) is the right shape. The cert/key paths,
  retry-secret loader, replay-guard wiring, and `config_factory`
  are termination-only — Mode A does not load TLS material.
- **Not reusable:** `with_backends` / `with_h2_backend` /
  `with_h3_backend` assume termination; the listener decides at
  bind time which backend protocol to call. Mode A's "backend" is
  a `Vec<SocketAddr>` plus a consistent-hash policy; the bridge
  pools (`TcpPool`, `Http2Pool`, `QuicUpstreamPool`) are not used.
- **Pattern to copy, not the code itself:** the
  `load_or_generate_retry_secret` (`O_CREAT|EXCL` + mode 0600 +
  `sync_all`) discipline is exactly right when/if Mode A grows a
  long-lived stateless secret of its own (e.g. for a CID rotation
  hash). Today Mode A does not need one.

### 1.3 `crates/lb-quic/src/router.rs` — 560 LOC ⭐ closest precedent

- **What it does today.** `InboundPacketRouter` (`router::spawn`)
  owns one `UdpSocket`, recv-loops in `router_main`, and for each
  packet parses the QUIC public header with
  `quiche::Header::from_slice(pkt, MAX_CONN_ID_LEN)` → looks up
  the DCID in a `DashMap<Vec<u8>, mpsc::Sender<InboundPacket>>`.
  If found, forwards to the per-connection actor channel
  (bounded, depth 32, drop-on-full). If not found and the packet
  is an Initial:
  - Initial without token → `send_retry` (mints token via
    `RetryTokenSigner::mint(peer, &header.dcid)` and calls
    `quiche::retry(...)` to build the on-wire RETRY packet).
  - Initial with token + verified → `0-RTT` replay check via
    `build_replay_key(scid + first 32 bytes of token) ->
    ZeroRttReplayGuard::check_0rtt_token`, then
    `spawn_new_connection` → `quiche::accept_with_retry` →
    spawn actor task, register *two* DashMap entries (the
    server-side SCID and the client's first-Initial DCID) inside
    a `CidEntryGuard` (auto-cleanup on drop/panic).
  - Initial with invalid token → drop silently.
- **Reusable for Mode A — IDEA, not code:**
  - The **shared-UDP-socket recv loop** with `tokio::select! {
    cancel, recv_from }` is the right shape.
  - The **DashMap dispatch** keyed by a DCID prefix is the right
    shape; Mode A's table maps `CID-prefix -> backend SocketAddr`
    (and an `Instant` last-seen for LRU eviction) instead of mapping
    to an mpsc actor channel.
  - The **bounded-capacity DoS gate** (`max_connections = 100_000`,
    `cap_entries = 2 * max_connections`, return-drop-no-OOM) is
    exactly the discipline Mode A needs; auditor signoff
    2026-04-23 references PROMPT.md §6 conntrack scale and the
    rationale carries over.
  - The **RETRY-on-tokenless-Initial** path is reusable
    *conceptually* for Mode A: passthrough still wants Retry
    protection against source-address-spoofed Initial floods,
    minted with `RetryTokenSigner::mint(peer, &dcid)` exactly as
    today. The difference: Mode A's Retry SCID becomes the
    DCID-prefix the LB will route on; the LB does not call
    `quiche::accept_with_retry` afterward — it forwards the
    second Initial (with the verified token) verbatim to the
    chosen backend.
- **Not reusable:**
  - The `quiche::Header::from_slice` call pulls quiche into the
    Mode A hot path; Mode A wants a quiche-free, allocation-free
    long/short-header parser that returns `(form, version,
    dcid_slice)` from `&[u8]` without copying. See §3.
  - `spawn_new_connection`, `accept_with_retry`, the actor mpsc,
    `CidEntryGuard` — all coupled to per-connection actor /
    termination. Mode A has no per-connection actor.
- **Code review notes for the new module:** copy the doc-comment
  discipline (audit-signoff cross-refs inline), the `biased`
  `select!`, the explicit cap-with-warning, and the
  `CidEntryGuard` Drop-runs-on-panic pattern (CODE-2-08).

### 1.4 `crates/lb-quic/src/conn_actor.rs` — 1292 LOC

One tokio task per accepted connection: owns a `quiche::Connection`,
drives recv-loop + timer + cancel, pumps H3 via `h3_bridge`. **Mode A:
nothing reusable** — passthrough has no per-connection state machine.
**Mode B (S16): reuse wholesale** — Mode B *is* termination; only the
H3 bridge is replaced by raw stream/datagram proxy logic onto an
outbound quiche client.

### 1.5 `crates/lb-quic/src/h3_bridge.rs` — 5297 LOC

H3 termination + the H1/H2/H3 egress legs for the H→H matrix (S6–S14).
**Mode A: nothing.** **Mode B: marginal** — proxies raw QUIC streams,
not H3 — H3-aware logic is bypassed. QPACK + frame helpers live in
`lb-h3`, not here.

### 1.6 `crates/lb-io/src/quic_pool.rs` — 782 LOC

- **What it does today.** `QuicUpstreamPool` — per-peer
  `DashMap<SocketAddr, Mutex<VecDeque<UpstreamQuicConn>>>` of
  reusable client quiche connections to H3 backends, FIFO LRU,
  bounds on drop, max-age + idle-timeout discard on acquire,
  PING-ACK liveness probe before reuse (`send_ack_eliciting` +
  bounded ACK wait, default 100 ms). Used by the H3-bridge's
  H3→H3 egress and by tests.
- **Reusable for Mode A: NOTHING.** Passthrough has no outbound
  quiche connection — the LB forwards raw UDP datagrams to the
  backend's UDP socket without any TLS state of its own.
- **Reusable for Mode B (S16):** strongly. Mode B will terminate the
  client connection and originate a fresh client connection to the
  backend — `QuicUpstreamPool` is exactly the shape that pool needs
  (per-peer LRU, liveness probe, cert verification via caller-supplied
  config). The S16 plan can reuse this verbatim or extend its
  config-supplied `quiche::Config` to carry the Mode-B-specific
  ALPN / cert posture.

### 1.7 `crates/lb-l4-xdp/` — XDP datapath (~4 k LOC)

- `lib.rs` (656): `XdpError`, `FlowKey` (5-tuple), `ConntrackTable`
  (FIFO eviction), `MaglevTable`, `HotSwapManager`.
- `loader.rs` (1852): aya-based loader; `attach_with_fallback` on a
  `Drv → Skb → Generic` mode ladder (EBPF-2-04). Pin names
  `CONNTRACK_PIN_NAME`, `CONNTRACK_V6_PIN_NAME`, `L7_PORTS_PIN_NAME`.
- `nic_compat.rs` (594): `(driver, firmware)` silent-drop blocklist
  (aya #1193 / Cilium lesson 8). Target box `ena/7.0` is NOT blocked.
- `netlink_xdp.rs` (391): raw `RTM_GETLINK` prog-id query.
- `bpffs.rs`, `stats_export.rs`, `sim.rs`: CI-safe sim + pin/mount-type
  checks + stats accessors.
- `ebpf/src/main.rs`: in-kernel program — parses UDP (IPPROTO=17), uses
  the 5-tuple as the conntrack key. `BACKENDS_V4` map exists but
  consistent-hash backend selection is Pillar 4b-3-deferred. **No
  QUIC-aware logic today.**

**Reusable for Mode A — idea-level only.** Userspace loader + ENA gate
+ Maglev/Unimog daisy-chain discipline carry over. The eBPF source
itself does not understand QUIC: Mode A's XDP fast path would need
either (a) a new eBPF program that reads the long/short QUIC header and
hashes a DCID prefix, or (b) the v1 ship-without-XDP exit per session
prompt §exit-(c). If v1 reuses the existing 5-tuple conntrack as a
coarse first-hop (packets from the same source land on the same LB
shard, then LB routes by DCID), the XDP plane needs zero changes.

**Not reusable:** the 5-tuple `FlowKey` is wrong for Mode A's
CID-migration claim — clients that rebind ports still must reach the
same backend if their DCID is unchanged. XDP-side CID steering is new
eBPF code.

### 1.8 `lb-h3` (`varint.rs`, `frame.rs`, `qpack.rs`, `security.rs`)

- `varint::{encode_varint, decode_varint, MAX_VARINT}` is the **only**
  shared piece Mode A needs from this crate: a QUIC variable-length
  integer decoder. The QUIC long-header `Token Length` and
  `Length` fields are variable-length integers (RFC 9000 §17.2),
  so any quiche-free long-header parser used by Mode A needs this.

## 2. What does NOT exist today

1. A **quiche-free QUIC public-header parser** that returns
   `(form, version, dcid)` from `&[u8]` without copying and without
   pulling in the quiche state machine. (`quiche::Header::from_slice`
   exists but it allocates and is in a termination-shaped crate.)
2. A **CID-keyed dispatch table** (LB-internal): `DashMap<CidPrefix,
   { backend: SocketAddr, last_seen: Instant }>` with LRU eviction
   under a capacity bound and a published `quic_passthrough_connections`
   gauge.
3. A **passthrough datapath**: pop a UDP datagram from a shared
   `UdpSocket`, parse the public header, route by DCID, `send_to`
   the chosen backend; the reverse direction reads from a per-flow
   backend socket and writes back to the client.
4. **DCID-aware XDP steering** (if S15 v1 takes the XDP fast path):
   a new eBPF program that decodes the long/short-header DCID and
   looks it up in a userspace-published map. Mode A v1 may ship
   without it per session prompt §exit-(c).
5. A **passthrough Retry path** that mints a token, builds a Retry
   packet *without* `quiche::retry` (because that function only
   accepts a quiche connection context). Mode A could either reuse
   `RetryTokenSigner` and hand-roll the Retry frame bytes per RFC
   9000 §17.2.5, or skip Retry in v1 and accept the Initial-flood
   amplification trade-off explicitly.
6. **CID-issuance tracking** for short-header DCID-length recovery:
   the LB does not see encrypted `NEW_CONNECTION_ID` frames in Mode A.
   This is the key architectural limitation; design doc §3 must call
   it out.

## 3. The smallest shared surface (R12 single-sourcing)

Both Mode A (S15) and Mode B (S16) need exactly two new shared
components, plus reuse of one existing piece:

**SHARED-1 — QUIC public-header parser (new, lb-quic).**
A free function (or thin struct) that, given `&[u8]`, returns:
- header form bit (long vs short)
- for long: type bits (Initial / 0-RTT / Handshake / Retry / VN),
  version (`u32`, big-endian), DCID length, DCID bytes, SCID length,
  SCID bytes, optional Token Length + Token, optional payload Length
- for short: spin bit, key-phase bit (encrypted — *not* readable here),
  DCID bytes for a CALLER-SUPPLIED dcid length

Public surface:
```rust
pub fn parse_public_header(pkt: &[u8]) -> Result<PublicHeader<'_>, HeaderError>;
pub fn parse_short_header(pkt: &[u8], dcid_len: usize) -> Result<ShortHeader<'_>, HeaderError>;
pub enum PublicHeader<'a> { Long(LongHeader<'a>), Short(ShortHeader<'a>) }
```

Mode A uses this on every inbound packet. Mode B uses it on the
fast-pass-through case (a packet whose CID is already terminated by
this LB shard — the public-header peek decides actor dispatch before
quiche touches the bytes; same role `quiche::Header::from_slice`
plays today, just without the quiche dep).

**SHARED-2 — UDP datapath plumbing (new, lb-quic).**
A thin `UdpDataplane` trait that abstracts:
```rust
async fn recv(&self) -> io::Result<(BytesMut, SocketAddr)>;
async fn send(&self, buf: &[u8], to: SocketAddr) -> io::Result<usize>;
```
with three implementations: epoll (`tokio::net::UdpSocket`, today),
io_uring (when the `lb_io::Runtime` reports `IoUring`), and an
XDP-AF_XDP socket (deferred). Mode A drives this on both directions
(client-side socket + per-backend sockets). Mode B's terminator path
also uses it (the inbound listener side) and the upstream-leg uses
`QuicUpstreamPool`'s already-existing dialer.

**REUSE-1 — `RetryTokenSigner`.**
`lb_security::RetryTokenSigner` is a standalone, well-tested
stateless-Retry primitive (`mint(peer, dcid)` / `verify(token, peer,
now)`). Mode A and Mode B can both call it; Mode A skips
`quiche::retry` and instead hand-rolls the Retry frame bytes via the
new long-header writer in SHARED-1, since `quiche::retry` requires a
quiche connection context.

Out of scope for shared: `ZeroRttReplayGuard` (Mode A does not see
0-RTT plaintext, so it cannot meaningfully gate; the BACKEND owns
this in passthrough — design doc §3 §0-RTT).

## 4. Mode-by-mode reuse summary

| Component                              | S15 Mode A | S16 Mode B | Notes |
| -------------------------------------- | ---------- | ---------- | ----- |
| `lb-quic/lib.rs` loopback rig          | no         | no         | test-only |
| `QuicDatagram`/`QuicStream`            | no         | reuse      | termination payload types |
| `QuicListener::spawn` shape            | copy       | reuse      | bind/cancel pattern only |
| `RetryTokenSigner`                     | reuse      | reuse      | both modes mint+verify |
| `ZeroRttReplayGuard`                   | no         | reuse      | A cannot read 0-RTT |
| `router::*` (incl. DashMap dispatch)   | copy idea  | reuse      | A keys on DCID→backend, not DCID→actor |
| `quiche::Header::from_slice`           | no         | replace    | swap for SHARED-1 quiche-free parser |
| `conn_actor::*`                        | no         | reuse      | termination only |
| `h3_bridge::*`                         | no         | no         | H3 termination — neither mode terminates H3 v1 |
| `lb_io::quic_pool::QuicUpstreamPool`   | no         | reuse      | B originates client connections |
| `lb_io::pool::TcpPool`/`Http2Pool`     | no         | no         | upstream H1/H2 not in scope |
| `lb-h3 varint`                         | reuse      | reuse      | header parser uses it |
| `lb-l4-xdp/loader`                     | reuse      | reuse      | unchanged 5-tuple plane works for both |
| `lb-l4-xdp/ebpf` (5-tuple conntrack)   | reuse      | reuse      | as coarse first-hop; CID steering deferred |
| `lb-l4-xdp` CID-aware eBPF program     | NEW v1?    | n/a        | session exit-(c) — v1 may ship without it |
| Passthrough CID-table + datapath       | NEW        | n/a        | the only material new code for Mode A |

## 5. Footprint estimate for S15 Phase 2 (build)

If the design doc §8 plan stays close to this inventory, S15 Mode A
adds:

- `lb-quic/src/public_header.rs` (SHARED-1): ~200–300 LOC + unit
  tests against RFC 9000 §17.2 examples + property tests.
- `lb-quic/src/passthrough.rs` (CID table + datapath): ~400–600 LOC.
  CID-keyed `DashMap` with `Instant` last-seen, LRU eviction under
  `max_quic_connections` cap, per-flow backend-socket task, bounded
  datagram backlog with explicit drop-oldest policy (justified in
  design §5).
- Optional Retry-without-quiche: ~100 LOC if shipped in v1.
- Verify-bar tests (~5 files): real-quiche client → LB → real-quiche
  backend; CID-migration same-DCID-different-path; bounded-state
  R13 a/b/c; never-decrypted assertion (mechanism-level: the LB
  binary linkage and the test rig prove no TLS keys are loaded).
- No XDP eBPF changes if v1 takes the io_uring/epoll path (session
  exit-(c) honest declaration).

The H3 termination path stays untouched; all of S6–S14's verify gates
on the H→H matrix continue to bind.

## 6. Open questions for the design doc (§3 inputs)

1. **Short-header DCID length.** In passthrough the LB never sees the
   backend issue its SCID/NCID (encrypted). The LB must record at
   first contact the DCID length the CLIENT chose (visible in the
   Initial), and for subsequent short-header packets it must assume
   that length. If the backend issues a different-length CID via
   `NEW_CONNECTION_ID`, the client may later switch to it — the LB
   parses the wrong number of bytes and routes wrong. Mitigation
   options for design §3: (a) require backends to keep client-issued
   DCID length for the connection lifetime (operational contract);
   (b) negotiate via a control-plane health check; (c) detect via a
   short-header miss + drop. Pick one, justify it.
2. **Retry in v1?** Adds ~100 LOC + a hand-rolled long-header writer.
   Skipping it accepts an Initial-flood amplification window. Lead
   should rule.
3. **XDP fast path in v1?** Per exit-(c), v1 may ship io_uring/epoll
   only. The CID-aware eBPF program is the largest single deferral
   if so.
4. **Datagram backlog policy.** drop-oldest (smoothest under burst,
   but loses ordering signal), drop-newest (preserves the head,
   penalises burst tail), or rely on socket-level drop (UDP recv
   buffer overflow). Design §5 picks one with justification.

## 7. References — base tree

- main @ 36ee1227 (S14 promote — CFBW closed on 4 H1/H2 cells, R11).
- H-matrix 9/9 BUILT.
- `lb_security::RetryTokenSigner` last touched ~Pillar 3b.3a.
- `lb_security::ZeroRttReplayGuard` last touched ~Pillar 3b.3a.
- L4 XDP target NIC: `ens5` (`ena/7.0`, off the silent-drop blocklist).
