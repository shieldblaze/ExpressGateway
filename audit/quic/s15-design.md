# SESSION 15 — Phase 1 Design (QUIC proxy, both modes)

**Author:** quic-eng
**Branch:** feature/quic-proxy-s15 (worktree off main @ 36ee1227)
**Prerequisite:** `audit/quic/s15-inventory.md`
**Status:** design only — no source modifications until owner check-in.

This document specifies the architecture for:

- **Mode A (S15) — Passthrough.** LB routes encrypted QUIC packets by
  Connection ID without decrypting. LB has no TLS keys for client or
  backend connections. End-to-end TLS preserved. Build target this
  session.
- **Mode B (S16) — Terminate-and-re-originate.** LB terminates client
  QUIC (reusing existing termination), proxies raw streams + datagrams,
  originates fresh QUIC to backend. Sketched here so Mode A's shared
  surfaces are sized correctly.

## 1. Both-mode architecture

```
                    ┌────────────────────────────────────┐
                    │             Client                 │
                    └──────────────┬─────────────────────┘
                                   │ UDP/QUIC
                                   ▼
┌──────────────────────────────────────────────────────────────────┐
│                       LB (one process)                           │
│ ┌─────────────────── UDP datapath (SHARED-2) ──────────────────┐ │
│ │ XDP tier  │  io_uring tier  │  epoll tokio-UDP tier  (recv)  │ │
│ └──────────┬──────────────────────────────────────────────────┘  │
│            ▼                                                     │
│ ┌──── Public-header parser (SHARED-1, quiche-free) ──────────┐   │
│ │ form / version / DCID / SCID / token-len / payload-len     │   │
│ └────────────┬───────────────────────────────┬───────────────┘   │
│              │ Mode A                         │ Mode B           │
│              ▼                                ▼                  │
│   ┌────────────────────────┐    ┌──────────────────────────┐     │
│   │ Passthrough router     │    │ Termination router       │     │
│   │  DashMap<CidPrefix,    │    │  (existing                │     │
│   │    {backend, last_seen}│    │   router.rs +             │     │
│   │  LRU evict / cap       │    │   conn_actor.rs)          │     │
│   │  Retry mint+verify     │    │   Retry / 0-RTT replay    │     │
│   └────────┬───────────────┘    └───────────┬──────────────┘     │
│            │ forward bytes verbatim          │ decrypted streams  │
│            ▼                                  ▼                   │
│  ┌────────────────────┐         ┌──────────────────────────┐     │
│  │ Per-flow backend   │         │ Raw-stream/datagram      │     │
│  │ socket task        │         │ proxy ↔ QuicUpstreamPool │     │
│  └────────┬───────────┘         └────────────┬─────────────┘     │
└────────────┼────────────────────────────────┼────────────────────┘
             │                                │
             ▼                                ▼
   ┌──────────────────┐               ┌────────────────────┐
   │ Backend (QUIC)   │               │ Backend (QUIC)     │
   │ owns TLS keys    │               │ owns TLS keys      │
   └──────────────────┘               └────────────────────┘
```

The **two boxed components are shared** between Mode A and Mode B:

- **SHARED-1** — quiche-free public-header parser
  (`lb_quic::public_header`).
- **SHARED-2** — UDP datapath trait + tiered implementations
  (`lb_quic::udp_dataplane`).

Mode A's whole stack below the parser is new. Mode B reuses the
existing `router.rs` + `conn_actor.rs` + `h3_bridge.rs` tree for
client-side termination, plus `QuicUpstreamPool` for the upstream leg,
with a thin raw-stream/datagram proxy replacing the H3 bridge inside
the actor.

## 2. Connection-ID parsing (the SHARED-1 component)

### 2.1 Long-header layout (RFC 9000 §17.2)

Long headers carry both DCID and SCID inline with their lengths. We
need just enough decode to (a) classify, (b) extract DCID for routing,
(c) for Initial/0-RTT/Retry only, extract Token and SCID.

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|1|1|T T|R R|P P|        Version (32)                           |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| DCID Length(8)| Destination Connection ID (0..160)            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| SCID Length(8)| Source Connection ID      (0..160)            |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

- byte[0] `& 0x80` = Header Form (1 = Long, 0 = Short).
- byte[0] `& 0x40` = Fixed Bit (MUST be 1 for QUIC v1; 0 → drop).
- byte[0] `& 0x30` >> 4 = Type (00 = Initial, 01 = 0-RTT, 10 = Handshake,
  11 = Retry); except when Version == 0 → Version Negotiation packet
  (entire Type-field semantics suspended; payload is a version list).
- byte[1..5] big-endian u32 = Version. Version == 0x00000000 is a
  Version-Negotiation packet; treated specially.
- byte[5] = DCID Length (0..20 per RFC 9000 §17.2).
- byte[6..6+dcid_len] = DCID bytes (the routing key).
- byte[6+dcid_len] = SCID Length (0..20).
- byte[7+dcid_len..7+dcid_len+scid_len] = SCID bytes.

For **Initial** packets only, after SCID:
- varint Token Length (0..2^62-1, but bounded by packet size).
- Token Length bytes of Token.
- varint Length (covers Packet Number + Payload).

For **Retry** packets, after SCID:
- Retry Token (variable, runs to end of packet minus 16-byte tag).
- 16-byte Retry Integrity Tag.

For **0-RTT** and **Handshake**, after SCID:
- varint Length (covers PN + payload).

### 2.2 Short-header layout (RFC 9000 §17.3)

```
 0                   1                   2                   3
 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
|0|1|S|R R|K|P P|         Destination Connection ID (variable)  |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
| Packet Number (8/16/24/32 — *encrypted, unreadable*)          |
+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
```

- byte[0] `& 0x80` = 0 (Short).
- byte[0] `& 0x40` = Fixed Bit (MUST be 1).
- byte[1..1+dcid_len] = DCID bytes.

Short headers do NOT encode DCID length on the wire. The LB MUST
recover it out-of-band — see §2.3.

### 2.3 How the LB knows short-header DCID length

This is the load-bearing decision for Mode A. In the original QUIC
protocol, the endpoint that **issues** a connection ID also chooses
its length. In passthrough:

- Client picks the initial DCID it puts in its first long-header
  Initial. The LB sees this length and records it for the new flow.
- Backend will later issue its own SCID (the server-side CID); its
  length is encoded in the long-header SCID Length byte of the
  backend's response Initial/Handshake. **The LB sees this length too**
  (long header is in cleartext) and records it as the per-flow
  short-header DCID length.
- Once the connection is established, client and server can issue new
  CIDs via `NEW_CONNECTION_ID` frames. **These frames are encrypted
  under 1-RTT keys; the LB cannot read them.** Consequence: any new
  CID issued via NCID looks indistinguishable from a CID the LB has
  never seen.

Operational contract: **the backend must keep its server-issued SCID
length constant for the lifetime of a connection**. Any backend that
rotates to a different-length CID via NCID will desync the LB's
short-header parser. We enforce this as a documented backend contract
(MUST), not via on-wire detection. This is the same contract Cloudflare
quicly-proxy and Google's GFE rely on in passthrough mode.

Backwards-compat fallback if a packet's CID prefix does not match any
tracked DCID-length-known flow: assume the short-header DCID is the
**configured `max_dcid_len_routed`** bytes (default 20 — the RFC max).
Look up in the routing table using that 20-byte prefix; if missed,
drop. This handles late-arriving NCIDs gracefully — the client falls
back to the original DCID on retransmit, which we still recognise.

### 2.4 Public-header parser API

```rust
// crates/lb-quic/src/public_header.rs (new)

/// Maximum DCID length per RFC 9000 §17.2 / §17.3.
pub const MAX_CID_LEN: usize = 20;

pub enum LongType { Initial, ZeroRtt, Handshake, Retry, VersionNegotiation }

pub enum PublicHeader<'a> {
    Long {
        ty: LongType,
        version: u32,        // 0 == Version Negotiation
        dcid: &'a [u8],      // 0..=20 bytes
        scid: &'a [u8],      // 0..=20 bytes
        // Initial only: present iff ty == Initial.
        token: Option<&'a [u8]>,
        // Initial / 0-RTT / Handshake: declared payload length.
        // None for Retry / VersionNegotiation.
        length: Option<u64>,
    },
    Short {
        dcid: &'a [u8],
    },
}

pub enum HeaderError {
    TooShort,
    FixedBitClear,
    DcidTooLong,
    ScidTooLong,
    Varint(VarintError),
    Truncated,
}

/// Parse the public header of `pkt`. For short headers the caller MUST
/// pass the per-flow short_dcid_len recovered as §2.3 describes.
pub fn parse_public_header(pkt: &[u8], short_dcid_len: usize)
    -> Result<PublicHeader<'_>, HeaderError>;
```

Parser is `#[no_panic]`-shaped: all indexing through `get`, all
arithmetic checked, no `unwrap`/`expect`/`panic` (mirrors the
crate-wide `clippy::indexing_slicing` deny in `lb-quic`). Unit tests:
hand-built fixtures from RFC 9001 §A.4 (Initial), §A.5 (Handshake);
property tests via `quiche::Header::from_slice` differential
(quiche-built packets parsed identically by both parsers).

## 3. Mode A passthrough — datapath

### 3.1 Tracking table

```rust
// crates/lb-quic/src/passthrough.rs (new)

struct FlowEntry {
    backend: SocketAddr,         // chosen at first-Initial
    short_dcid_len: usize,       // recorded from backend SCID-length
    last_seen: Instant,          // LRU eviction
    backend_sock: Arc<UdpSocket>,// per-flow backend egress socket
}

// Both keys point at the same FlowEntry Arc — two map entries per flow.
// Same discipline as router.rs uses today.
type Table = DashMap<Vec<u8>, Arc<FlowEntry>>;
// Key forms:
//   client-chosen DCID (the routing key the client uses to reach us)
//   backend-chosen SCID (so backend->client packets can be reverse-routed)
```

Cap: `max_quic_connections` (default 100_000, matching
`router::RouterParams::max_connections`). Cap-entries = `2 *
max_quic_connections` because each flow has two keys. At cap, new
Initials with no valid Retry token are dropped (RETRY is a no-state
ask-to-retry-later; the cap pushes back without OOM).

### 3.2 New-connection routing decision (long-header Initial)

```
1. recv UDP packet from client (peer = src 4-tuple).
2. parse_public_header(pkt, short_dcid_len=0)  // 0 unused for long.
3. classify by form:
     Long Initial   -> §3.2a
     Long 0-RTT     -> §3.2b
     Long Handshake -> §3.2c (rare — only sent after Initial)
     Long Retry     -> §3.2d (server-origin; client-origin invalid here)
     Long VN        -> §3.2e (server-origin; client-origin invalid here)
     Short          -> §3.3
3.2a Initial:
     - look up Table[dcid]
     - if present: this is a retransmit / second Initial; forward.
     - else (new connection):
         * if no token, mint Retry token (RetryTokenSigner.mint(peer, dcid))
           and emit a Retry packet (see §3.6) — do NOT add to table yet.
         * if has token, RetryTokenSigner.verify(token, peer, now):
              - Ok(odcid) ->
                  · CHECK cap; on cap, drop with warn.
                  · backend = consistent_hash(dcid) over live backends.
                  · open per-flow backend UDP socket
                    (bind 0; connect(backend)).
                  · Table[dcid] = Arc::new(FlowEntry { backend, ... })
                  · forward pkt to backend on its socket.
                  · spawn reverse-direction task that recv()s from the
                    backend socket and writes to client peer.
              - Err -> drop silently (poison token / replay / cap).
```

**Consistent hashing.** Hash the entire client-chosen DCID with a
fixed Maglev table over the live backend set. The Maglev table is the
same Unimog-discipline structure the L4 plane uses (publish-atomic,
daisy-chain across reconfig). When the backend set changes, only a
1/N slice of new flows re-route — existing flows stay because Table
lookup is by full DCID, not by hash.

### 3.3 Existing-connection forwarding (short-header)

```
3.3 Short:
     - prefix = pkt[1..1+short_dcid_len_default]   // default 20
     - try Table[prefix] → if hit, forward; update last_seen.
     - else, walk DCID-length candidates (the set of lengths we have
       per-flow records for, deduped + sorted); for each len try
       Table[pkt[1..1+len]].
     - if still miss, drop. Client retransmit will hit when a long
       header refreshes the entry, or the connection is genuinely
       gone.
```

Hot path is single-len lookup (`O(1)` dashmap probe). Multi-len walk
is the unusual case (mid-connection NCID switch); bounded by the
distinct lengths in use, in practice ≤ 2.

### 3.4 Reverse direction (backend → client)

Each `FlowEntry` carries a per-flow `Arc<UdpSocket>` `connect()`-ed to
the chosen backend. A spawned task reads from it and writes the bytes
verbatim to the recorded client peer. The per-flow socket is the
simplest correct shape: kernel `connect()` filters incoming packets to
those from the chosen backend (anti-spoof), and the writer task knows
where to send.

Memory: 1 backend socket per flow → 100_000 sockets at cap is the
hard limit. `fs.file-max` and the LB process `ulimit -n` must accommodate;
LB sets `setrlimit(NOFILE, 200_000)` at startup with an audit log line
if not granted. *Alternative considered & rejected for v1:* shared
backend socket with userspace demux by source 5-tuple — saves FDs
but requires building a separate (peer, backend)-keyed reverse table
and re-deriving the source backend on every packet. Per-flow socket
is the simpler R12 build target; if FD pressure becomes real, the
shared-socket variant is a contained refactor in §8.

### 3.5 CID migration — honest documentation of the limitation

Two kinds of migration to distinguish:

- **Path migration (NAT rebind).** Client's external 4-tuple changes
  (e.g. mobile network handover, NAT reassignment). Client KEEPS the
  same DCID it was using. In Mode A: `Table[dcid]` still hits →
  packet routes to the same backend. **Works correctly.** The
  per-flow backend socket task continues writing to the NEW client
  peer recorded from the most recent inbound packet — `FlowEntry`
  carries a `peer: AtomicSocketAddr` updated on every recv. Backend
  sees the path change in the encrypted layer and validates via
  PATH_CHALLENGE/PATH_RESPONSE — opaque to LB.

- **CID-issuance migration (server-rotated CIDs).** Server has issued
  a new CID via `NEW_CONNECTION_ID` (encrypted); client switches to
  it. **In Mode A this is INDISTINGUISHABLE from a new connection.**
  `Table[new-cid]` misses. The packet has a short header (no version,
  no type bits) so the LB cannot even classify it as "probably
  belongs to an existing flow". Outcome:
  - Best case: short-header miss → drop → client retransmit using the
    original (pre-NCID) DCID hits → routes to original backend.
    Client keeps two DCIDs in active use during the migration window.
  - Worst case: client's stack abandons the old DCID and only sends
    the new one — every packet misses → connection dies from the
    client's perspective.

  Mitigation: **document the constraint as a backend MUST.** A
  Mode-A-aware backend SHOULD NOT issue server-side NCIDs that the
  client is allowed to use as new DCIDs without informing the LB.
  In practice this means setting `active_connection_id_limit=2` (the
  minimum allowed) and not retiring the original CID. quiche
  backends control this via `set_active_connection_id_limit`. The
  LB operator declares this in their backend pool config; LB itself
  cannot enforce it (encrypted layer).

  Mode B (S16) does not have this limitation — termination sees NCID
  frames and can track the issued CIDs.

### 3.6 Retry / Version Negotiation / Handshake

- **Retry from backend → client (server-origin Retry).** Backend is
  the QUIC server; if it wants to force its own RETRY, the Retry
  packet arrives at the LB as a backend→LB datagram (reverse direction
  for an in-flight flow). LB forwards verbatim. No special handling.
- **Version Negotiation from backend → client.** Same: forward
  verbatim. The Version field is 0x00000000 and the payload is a
  supported-versions list; opaque to routing.
- **Initial without token from client (Mode A's RETRY).** §3.2a above
  handles. The LB-minted Retry has SCID = `sample_conn_id()` (16
  random bytes), so the client's second Initial DCID equals that
  value. We route on that DCID. This means the **routing DCID after
  a Mode-A Retry is LB-chosen, not client-chosen** — clients-with-
  duplicate-CID hash collisions are eliminated. Good. The downside:
  consistent hashing on the LB-chosen SCID does not give the same
  backend across Mode-A-Retried and non-Retried Initials from the
  same client — acceptable since Retry is rare.
- **Handshake-typed packets from client.** Forward by long-header
  DCID lookup; standard flow.

Implementation note for the Retry packet: `quiche::retry` takes a
quiche state context. Mode A cannot use it. We build the Retry packet
bytes directly per RFC 9000 §17.2.5: long-header skeleton, type bits
= `11`, version = client's version, ODCID + token in the Retry
Pseudo-Packet, then compute the 16-byte AEAD-AES-128-GCM Retry
Integrity Tag (RFC 9001 §5.8) over the pseudo-packet using the
fixed Retry key + nonce specified in RFC 9001 §5.8. ~80 LOC + tests
against quiche's own retry generator (differential).

### 3.7 0-RTT replay protection

In passthrough mode the LB does not terminate, so the existing
`ZeroRttReplayGuard` does not gate the 0-RTT plaintext (encrypted to
the LB). **0-RTT replay protection is the BACKEND's responsibility in
passthrough.** This is a documented trust shift. Any backend exposed
behind a Mode-A LB MUST implement its own 0-RTT replay guard (quiche
0.28 backends call `Config::enable_early_data()` and gate replays
themselves; nginx-quic does so via a configurable token).

What the LB CAN do cheaply: deduplicate Initials with the same
(client SCID + token-prefix) within a short window using the existing
`ZeroRttReplayGuard` shape. This catches naive on-path replays of the
same Initial but does NOT catch genuine 0-RTT data replays (those are
in protected payload). We ship this as a defence-in-depth (§4 below).

## 4. Datapath tiering

The session prompt specifies a three-tier UDP datapath: XDP fast path
→ io_uring fallback → epoll/tokio-UDP fallback. Each tier exercised
or its absence documented.

### 4.1 epoll / tokio-UDP (baseline — required, always present)

Tier 3 is `tokio::net::UdpSocket` — this is what `router.rs` and the
loopback rig use today. Ship this in v1 unconditionally. It is the
correctness baseline and the implementation against which the other
tiers are differentially verified.

### 4.2 io_uring (tier 2 — Linux 5.6+, opportunistic)

`lb_io::Runtime::IoUring` is already auto-detected at startup. The
existing wrapper covers TCP I/O; the io_uring `SQE_RECVMSG_MULTISHOT`
op (kernel 6.0+) is the UDP-equivalent and is the right primitive for
Mode A's recv loop. Wiring io_uring-UDP into Mode A is a contained
addition behind the `Runtime` selector. **Build plan §8 ships tier 3
in v1.0 and tier 2 in v1.1** to keep the v1 verify-bar focused;
io_uring-UDP needs a Linux 6.0+ test box, which the CI fleet does not
guarantee.

### 4.3 XDP native-attach (tier 1 — fastest, conditional)

For Mode A, the XDP fast path would parse the QUIC public header in
the kernel and look up the DCID in a userspace-published map,
returning `XDP_TX` (rewrite + retransmit out the same NIC to the
backend MAC). Feasibility:

- DCID parsing in eBPF: feasible without decryption — eBPF reads
  byte[0] form, byte[5] dcid_len, byte[6..6+dcid_len] DCID. Verifier-
  tractable (bounded loops; dcid_len ≤ 20). Roughly the same shape as
  the existing 5-tuple parser at `ebpf/src/main.rs:707..`.
- BPF map: `BPF_MAP_TYPE_HASH` keyed by `[u8; 20]` DCID, valued by
  backend MAC + IP. Userspace publishes via `bpf_map_update_elem` on
  every Mode A flow add/remove. Atomic per-key; no daisy-chain
  needed because keys are immutable per flow.
- Short-header DCID length recovery: eBPF cannot know per-flow CID
  lengths cheaply — it has to assume one length. Practical fix:
  publish ONE configured `xdp_short_dcid_len` (operator config,
  default 8), require backends to keep the server-side SCID at that
  length, and gate the XDP fast path with that contract. Any flow
  whose backend issued a different-length SCID falls back to
  userspace tier 2/3 transparently (XDP misses → packet bubbles up
  via `XDP_PASS`).
- New eBPF program: estimated ~300 LOC + verifier-log capture. Per
  session exit-(c), **Mode A v1 ships WITHOUT the XDP fast path** —
  io_uring/epoll only. The design surface (eBPF map name, key shape,
  userspace publisher) is documented here as a contract so the XDP
  add later is a non-disruptive enhancement.

### 4.4 Tier-selection logic

```
startup:
  if Runtime::detect() == IoUring  -> recv via io_uring SQEs (v1.1+)
  else                             -> recv via tokio UdpSocket (v1.0)

  if XDP attach available + xdp_dcid_steer feature flag set (v1.2+):
    publish DCID map; eBPF program steers fast path; userspace
    handles only fall-throughs.
  else:
    userspace handles everything.
```

The selector lives in `lb_quic::passthrough::Passthrough::spawn`.
v1.0 only enables tier 3.

## 5. Bounded state (R8)

| State                       | Bound                            | Default     | Eviction       |
| --------------------------- | -------------------------------- | ----------- | -------------- |
| Tracked flows               | `max_quic_connections`           | 100_000     | LRU by `last_seen`; at cap, new Initial dropped or Retry-asked |
| Routing-table entries       | `2 * max_quic_connections`       | 200_000     | tied to flow eviction (both keys removed together via Drop guard) |
| Per-flow datagram backlog   | `per_flow_backlog`               | 32 packets  | drop-newest (§5.1) |
| Backend UDP sockets         | one per flow                     | == flows    | closed on flow eviction |
| Retry-token replay guard    | `retry_replay_capacity`          | 1024 tokens | LRU (existing `ZeroRttReplayGuard` shape) |

LRU runs on a per-recv basis (`last_seen = Instant::now()`) and is
evaluated when a new flow needs an entry and the cap is hit. Eviction
takes the entry with the oldest `last_seen` and drops it — the
per-flow socket task observes channel close and exits, closing the FD.

### 5.1 Per-flow backlog full-policy = drop-newest

Three options weighed:

- **drop-oldest** — preserves the most recent packet. Bad here: QUIC
  retransmits older packets the receiver hasn't yet acked. Dropping
  the oldest means a packet still in-flight from the application's
  ack-pending window vanishes; the QUIC layer will retransmit
  eventually, but we've maximised reordering pain.
- **drop-newest** — preserves in-flight retransmits. Newest packets
  represent the leading edge; their loss is what QUIC's loss-recovery
  primitives are tuned for (Reno, BBR all assume tail loss is the
  signal). **Choose this.**
- **backpressure via socket-drop** — let the UDP recv buffer overflow.
  Worst of both: opaque to the LB (no log line), no per-flow fairness
  (one chatty flow can starve all others).

drop-newest is the same choice tokio-quiche's recv loop makes when its
per-actor channel is full (`mpsc::error::TrySendError::Full → drop +
log`). We match that for consistency.

## 6. Threat model

### 6.1 CID-amplification (small Initial → large Retry)

- **Attack.** Attacker sends a 1200-byte Initial with no token; LB
  replies with a Retry packet. Retry is on the order of 80 bytes
  (header + 16-byte token + 16-byte integrity tag) — actually
  *smaller* than the Initial, so amplification factor < 1. **Not
  exploitable for amplification.**
- **Mitigation.** None needed. Retry is anti-amplification by design
  (RFC 9000 §8.1 inverse: Retry is the protection against off-path
  Initial spoofing).
- **Where it lives.** RFC 9000 §17.2.5; our Retry path §3.6.

### 6.2 CID-flood / tracking-table exhaustion

- **Attack.** Attacker mints valid Retry tokens by completing the
  Retry roundtrip from many spoofed source IPs (but they cannot
  spoof — `RetryTokenSigner` binds token to source 4-tuple, so a
  spoofed-source attacker fails verification). Genuine attacker
  with real source IPs cycles through DCIDs to fill the LB table.
- **Mitigation.** (a) The `max_quic_connections` cap with LRU
  eviction. Genuine attacker reaches the cap; oldest legitimate
  flows are evicted, but the LB does not OOM. (b) Per-source-IP
  flow rate-limit (deferred to v1.1): cap flows per `/24` to a
  fraction of `max_quic_connections`. (c) Audit log on cap-hit
  with peer 4-tuple distribution.
- **Where it lives.** §3.1 + §5; rate-limiter is a deferred ticket.

### 6.3 Routing-table poisoning (spoofed-CID forwarding)

- **Attack.** Off-path attacker sends a short-header packet with a
  guessed/known DCID belonging to victim's flow. LB looks it up →
  forwards to the victim's backend. Two failure modes:
  - The packet's source IP is the attacker, not the victim. **Backend
    decrypts → AEAD fails (attacker doesn't have the keys) →
    connection survives.** Defended at the TLS layer. The threat
    reduces to a backend CPU-burn DoS.
  - The packet's source IP is spoofed to be the victim. Even if it
    decrypts (impossible without keys), the backend's
    `disable_active_migration=true` config rejects path changes
    silently.
- **Mitigation.** (a) AEAD at the backend is the cryptographic
  defence; we rely on it (this is the "encrypted layer" the LB does
  not own). (b) **Source-IP binding per flow:** when a packet arrives
  whose DCID hits `FlowEntry` but whose source IP differs from the
  recorded `peer`, the LB MAY drop it (configurable, default
  enabled). This breaks legitimate NAT-rebind path migration but
  catches spoofed-CID off-path injection at the LB before the
  backend wastes a decrypt. Knob: `strict_source_binding` (default
  false in v1 to not break mobile clients; flip in higher-security
  deployments).
- **Where it lives.** §3.1 `FlowEntry.peer`; new knob in design §8.

### 6.4 Spoofed-CID hijack across backends

- **Attack.** Attacker chooses a DCID known to land on backend B1
  (via knowledge of the consistent-hash function) and sends an
  Initial. LB picks B1; attacker's connection now shares hash space
  with victim's flow. Could it deliberately collide?
- **Mitigation.** Consistent hashing distributes load — a collision
  requires the attacker to land on the same backend, which happens
  1/N of the time for N backends. There is no privilege escalation
  in the collision: backend B1 decrypts both connections under
  separate keys; the attacker cannot read or modify the victim's
  traffic. **Not a real threat.** The mitigation is the AEAD at the
  backend.
- **Where it lives.** §3.2 consistent hash; defence is at the
  backend's TLS layer.

### 6.5 Initial-packet flood

- **Attack.** Attacker fires unbounded Initial packets at the LB to
  consume LB CPU + memory.
- **Mitigation.** (a) **Retry mandatory** for new Initials: every
  no-token Initial gets a Retry response and no state allocation.
  The attacker must complete the roundtrip (proves source-address
  validity) before any LB state is committed. Same defence the
  termination router uses today. (b) Retry-emission rate-limit per
  source IP (deferred v1.1). (c) Cap-with-warn on
  `max_quic_connections`.
- **Where it lives.** §3.6 + §5.

### 6.6 0-RTT replay

- **Mode A.** The encrypted layer is the backend's domain. **The
  backend MUST implement its own 0-RTT anti-replay** (timestamped
  session tickets, server-side dedup window — quiche's default
  posture).
- **LB defence-in-depth.** Reuse `ZeroRttReplayGuard` to dedupe
  (client SCID + token-prefix) tuples within the guard's window.
  This catches naive on-path replays of identical Initials but does
  NOT catch true 0-RTT-data replays (those bytes are in the
  encrypted payload). Documented as belt-and-suspenders.
- **Mode B.** Existing `ZeroRttReplayGuard` applies as today.
- **Where it lives.** §3.7.

### 6.7 Source-address-spoofed Retry-acceptance (token forgery)

- **Attack.** Attacker constructs a token without going through the
  Retry exchange.
- **Mitigation.** `RetryTokenSigner` is HMAC-SHA256 with a 32-byte
  secret rotated by the operator. Forgery requires the secret.
  Cryptographically defended.
- **Where it lives.** `lb_security::retry`; reused unchanged.

### 6.8 Cross-flow DCID prefix collision

- **Attack.** Two distinct flows pick DCIDs sharing a 20-byte prefix
  (cryptographically near-impossible at 2^-160 for random DCIDs but
  possible if clients pick short DCIDs).
- **Mitigation.** Reject Initials whose DCID length is <8 bytes
  (configurable `min_client_dcid_len`, default 8). RFC 9000 §17.2
  allows 0..20 but real clients pick 8+. Drop with audit log.
- **Where it lives.** §3.2 input validation.

## 7. Mode B (S16) sketch — reuse map

Mode B = the LB terminates the client connection (reusing existing
`router.rs` + `conn_actor.rs` + `RetryTokenSigner` + `ZeroRttReplayGuard`
exactly as today), but instead of bridging to H3/H1/H2 it runs a raw
stream + datagram proxy onto a fresh quiche client connection from
`QuicUpstreamPool`.

Net-new for S16:
- `RawStreamProxy` — bidirectional stream copy between the client
  `quiche::Connection` and the upstream `quiche::Connection`,
  preserving STREAM frames, RESET_STREAM, STOP_SENDING, FIN. Roughly
  the shape of `h3_bridge::stream_h1_response` but transport-level
  rather than HTTP-level.
- `RawDatagramProxy` — bidirectional unreliable-datagram copy.
- Stream-credit propagation: upstream flow-control window must shadow
  client flow-control window (and vice versa) to avoid one side
  stalling.

Reused:
- SHARED-1 public-header parser (the client-facing router uses it to
  classify before quiche touches the bytes — same role
  `quiche::Header::from_slice` plays today).
- SHARED-2 UDP datapath (recv loop on the listener side).
- `RetryTokenSigner`, `ZeroRttReplayGuard`.
- `QuicUpstreamPool` for upstream-leg dialing + liveness.
- L4 XDP plane (5-tuple) as the front-door coarse hop.

## 8. Mode A v1 build plan (incremental)

Each increment ends in a single PR with explicit verify gates. All
work is on `feature/quic-proxy-s15`; PRs may stack as feature
branches off it.

### Increment A0 — design + owner check-in (this Phase 1)

- Deliverables: `s15-inventory.md` (DONE), `s15-design.md` (this
  document).
- Verify: owner check-in per session prompt.

### Increment A1 — SHARED-1 public-header parser

- New: `crates/lb-quic/src/public_header.rs` (~300 LOC + tests).
- Surface: `parse_public_header`, `PublicHeader`, `HeaderError`,
  `LongType`, `MAX_CID_LEN`.
- Verify gates:
  - Unit tests against RFC 9001 §A.4/§A.5 fixtures (Initial,
    Handshake).
  - Differential property test (proptest, 1000 cases): every
    quiche-produced long-header packet's `quiche::Header::from_slice`
    output matches ours bit-for-bit on `(type, version, dcid, scid,
    token)`.
  - No new `unsafe`, no `unwrap`/`expect`/`panic`/`indexing_slicing`
    (crate-wide deny).
  - `cargo +nightly llvm-cov -p lb-quic --lib --no-fail-fast` shows
    ≥85% line coverage on `public_header.rs` (session-scope).

### Increment A2 — Passthrough datapath core (tier-3 epoll only)

- New: `crates/lb-quic/src/passthrough.rs` (~500 LOC + tests). Exports
  `PassthroughListener`, `PassthroughParams`.
- Wiring: `crates/lb/src/main.rs` gains a `passthrough_listener:
  Option<PassthroughListener>` analogous to the termination listener.
- Scope: tier-3 `tokio::net::UdpSocket`; consistent-hash backend
  selection; new-Initial Retry mint (skipping `quiche::retry` — use the
  hand-rolled long-header writer); short-header DCID routing with
  the per-flow length recovered from the backend's first server-side
  long header; LRU cap; per-flow backend socket task.
- Verify gates (the verify-bar from session prompt):
  - **(i) Real-QUIC wire test.** `crates/lb/tests/quic_passthrough_e2e.rs`
    spins (real quiche server on `127.0.0.1:N1`) ← (LB on
    `127.0.0.1:N2` configured passthrough → backend N1) ← (real quiche
    client). One full handshake + bidirectional STREAM exchange.
    Asserts: handshake completes; client sees backend's cert (LB has
    none); STREAM bytes round-trip.
  - **(ii) NEVER-DECRYPTED proof.** The LB binary built for this test
    is built with `cargo --no-default-features --features
    quic-passthrough-only` such that no `quiche::Connection`,
    `BoringSSL` cert, or TLS material is linked into the listener
    code path. Mechanism-level proof: `cargo bloat -p lb-quic
    --release --filter quiche` shows zero quiche-Connection symbols
    in the binary segment for the Mode A path. The test rig
    additionally asserts the LB process did not call `read(2)` on
    any cert/key file (audit via `BPF_PROG_TYPE_KPROBE` openat
    trace, or fall back to: no cert file present on the LB box).
  - **(iii) CID-migration proof.** Test sends N packets from the
    client, then has the client's UDP socket rebind to a new
    ephemeral port (kernel-driven NAT rebind simulation) and continue
    using the SAME DCID. Assert: the connection stays alive
    end-to-end; backend metrics show one connection, not two; LB's
    `quic_passthrough_flows` gauge stays at 1.
  - **(iv) Bounded-state proof.** R13 a/b/c on eviction:
    (a) burst-open 200_000 distinct DCIDs (all completing Retry);
    assert LB's resident memory does not exceed `2 *
    max_quic_connections * (FlowEntry-size-bytes + dashmap overhead)
    + 32 MB tolerance`. `lb_quic::passthrough::flows_len` ≤ cap. No
    panic.
    (b) single-shot manual eviction: open cap+1 flows in one test
    thread; oldest flow's `FlowEntry::dropped` observed.
    (c) negative control: same test with `max_quic_connections =
    cap` and only cap-1 opens — no eviction observed.
  - **(v) R3 no-regression.** Full `cargo +stable test --workspace
    --all-features` ×3 with `--test-threads 1` for the H→H matrix
    suite (the §S14 gate at saturation is fragile by precedent —
    keep test-thread = 1 to isolate). Assert 0 new failures.
  - llvm-cov on `passthrough.rs` ≥80% session-scope (lcov DA per
    fn-range, the S7 #7 §7 method).

### Increment A3 — Threat-model defences + observability

- Scope: source-IP binding knob, `min_client_dcid_len` floor, per-IP
  Retry rate-limit (deferred to v1.1 — explicit ticket only here),
  audit-log lines, `quic_passthrough_*` Prometheus gauges/counters
  for `flows`, `flows_evicted_total`, `retry_minted_total`,
  `retry_rejected_total`, `header_parse_errors_total`,
  `backend_socket_errors_total`.
- Verify gates:
  - Unit tests on each defence (table-driven).
  - End-to-end: spoofed-source-IP test (in-process socket-pair
    fixture) hits source-binding defence and is dropped with
    `audit/source_binding_violation` log line.
  - Saturation test: 10_000 cap-hits emit one log line per
    `audit_throttle_window_secs` (default 60s), not 10_000.
  - llvm-cov ≥80% session-scope on the new threat-defence code.

### Increment A4 — Promote to main + S15 report

- Workspace-wide `cargo +stable fmt --all -- --check`, `cargo
  clippy --workspace --all-features --all-targets -- -D warnings`,
  `cargo test --workspace --all-features` ×3 (matching S14
  promote gate).
- `audit/quic/s15-final-report.md` mirrors the S14 final-report
  shape: per-increment evidence, threat-model coverage, build-plan
  carry-forwards.
- Owner sign-off → `git merge --no-ff` onto main.

### Out of v1 (carry-forwards into S15.x or S16)

- io_uring-UDP tier 2 (v1.1).
- XDP-AF_XDP tier 1 (v1.2).
- Per-source-IP Retry rate-limit (v1.1).
- DCID-aware eBPF program (v1.2).
- Mode B (S16) raw stream/datagram proxy.

## 9. Open items needing owner ruling

1. **Strict source-IP binding default — true or false in v1?** Doc
   §6.3. False is the mobile-friendly default; true is the
   high-security default. S14-style ruling needed.
2. **Retry without quiche — ship in v1 or defer?** Doc §3.6. ~80 LOC
   + RFC 9001 §5.8 differential test. My recommendation: SHIP in
   v1 — Retry is the primary Initial-flood defence and skipping it
   creates a v1-only DoS surface.
3. **Min client DCID length floor.** §6.8. Default 8 conflicts with
   no real clients I know of; quiche+quinn+mvfst all pick ≥8.
   Confirm.
4. **`max_quic_connections` default — 100_000 (matches termination
   today)?** §5.
5. **NEVER-DECRYPTED proof mechanism.** §A2 verify gate (ii). I
   propose `cargo bloat` symbol absence + an explicit
   `cfg(not(feature = "quic-passthrough-only"))` guard around any
   `quiche::Connection`/`BoringSSL` use in the Mode A path. Owner
   may prefer a stronger mechanism (capability-syscall trace, seccomp
   filter on `connect(2)` to disallow the BoringSSL handshake).

These are the only Phase 1 decisions blocking Phase 2 start.

(All five resolved in `audit/quic/s15-owner-rulings.md` — see §12.)

## 10. SHARED-2 trait — stable seam contract

Per owner ADDENDUM 1 (s15-owner-rulings.md §9 XDP/io_uring
deferral): the SHARED-2 datapath trait contract is nailed in this
document so v1.1 (io_uring) and v1.2 (XDP) implementations land
without touching passthrough logic. The trait is the **stable seam**
between QUIC routing and the kernel/userspace UDP transport.

### 10.1 Design principles

1. **Passthrough logic is tier-agnostic.** The Mode A router calls
   only methods on `dyn UdpDataplane`; it never imports `tokio`,
   `io_uring`, or `aya`. A new tier ships as a new trait
   implementation in its own module behind a Cargo feature flag —
   zero edits to `passthrough.rs`.
2. **Recv is callback-driven, not stream-driven.** A pull-style
   `recv() -> Future<Packet>` forces the implementer to allocate a
   per-packet buffer and locks in the tokio/Future model. A push-style
   callback `for_each_packet(F)` lets io_uring drive ring completions
   directly and lets XDP deliver out of the kernel via an AF_XDP
   ring without an extra hop.
3. **Send takes a borrowed slice.** No `Vec` ownership transfer; the
   caller (passthrough router) owns the recv buffer and writes the
   reply on the same allocation when possible. io_uring and AF_XDP
   both want this — `send_to(&[u8], dst)` matches the syscall shape
   without intermediate copies.
4. **Attach/detach is explicit and synchronous-ish.** Listener spawn
   and shutdown bind/unbind the tier; the trait reports the bound
   `SocketAddr` so the LB log emits a single concrete address. The
   XDP tier additionally exposes its eBPF `map_fd` for the userspace
   publisher (see §10.4).
5. **Errors are tier-shaped.** A `DataplaneError` enum surfaces
   bind/recv/send failures; the router maps them onto its own
   metrics-and-drop discipline. The trait does NOT panic on transient
   errors — the router decides whether `Err` is fatal.

### 10.2 Rust trait signature

```rust
// crates/lb-quic/src/udp_dataplane.rs (new in Increment A2)

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;

/// Maximum UDP datagram the passthrough router will accept.
/// Mirrors `lb_quic::MAX_UDP_DATAGRAM_SIZE` (65_535).
pub const MAX_UDP_DATAGRAM_SIZE: usize = 65_535;

/// One inbound UDP datagram delivered by the dataplane to the router.
///
/// The dataplane owns the buffer until `Packet` is dropped; the
/// callback may borrow `data` for the duration of its `Future` only.
/// Implementations may pool the underlying allocation across packets.
#[derive(Debug)]
pub struct Packet<'a> {
    /// Datagram payload, length-truncated to the bytes actually read.
    pub data: &'a [u8],
    /// Source peer address (as observed by the kernel / NIC).
    pub from: SocketAddr,
    /// Local bound address the datagram arrived on. Useful for
    /// multi-VIP listeners; v1 only has one bind so this is the
    /// listener's `local_addr` on every packet.
    pub to: SocketAddr,
}

/// Errors surfaced by a [`UdpDataplane`] implementation.
///
/// The router maps these to its own drop/metric/log discipline; the
/// dataplane does not decide policy.
#[derive(Debug, thiserror::Error)]
pub enum DataplaneError {
    /// Bind failed; the listener cannot start.
    #[error("dataplane bind failed: {0}")]
    Bind(#[source] std::io::Error),
    /// Recv hit a transient OS error (would-block, ENOBUFS). Router
    /// MAY continue.
    #[error("dataplane recv: {0}")]
    Recv(#[source] std::io::Error),
    /// Send hit a transient OS error. Router MAY drop the packet.
    #[error("dataplane send: {0}")]
    Send(#[source] std::io::Error),
    /// Tier-specific fatal error (eBPF verifier rejected the program,
    /// io_uring kernel doesn't support multishot recvmsg, etc.).
    /// Router MUST fall back to the next tier on the ladder.
    #[error("dataplane unavailable on this kernel/NIC: {0}")]
    Unavailable(String),
}

/// The seam. Three implementations exist (v1.0..v1.2 — §10.5).
pub trait UdpDataplane: Send + Sync + 'static {
    /// Local socket address the dataplane is bound to. Stable for
    /// the lifetime of the impl; the router logs it once at spawn.
    fn local_addr(&self) -> SocketAddr;

    /// Run the recv loop until `cancel` fires, dispatching each
    /// inbound packet through `on_packet`. The callback is invoked
    /// on the runtime's task; the impl MUST NOT hold the buffer
    /// past the returned future's `Poll::Ready`.
    ///
    /// Implementations are free to recv-batch (recvmmsg, io_uring
    /// multishot, AF_XDP ring) — the only contract is that `on_packet`
    /// observes EACH datagram EXACTLY ONCE in arrival order WITHIN
    /// a single 4-tuple, and that the future returned by `on_packet`
    /// is awaited before the buffer is recycled.
    ///
    /// Returns `Ok(())` on clean cancellation, `Err(Recv|Unavailable)`
    /// on fatal failure.
    fn recv_loop<'a>(
        &'a self,
        cancel: CancellationToken,
        on_packet: PacketHandler<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), DataplaneError>> + Send + 'a>>;

    /// Send `buf` to `dst`. Returns the number of bytes accepted by
    /// the kernel (== buf.len() on a healthy UDP socket; impls MAY
    /// short-write on backpressure and the router will retry).
    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        dst: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Result<usize, DataplaneError>> + Send + 'a>>;

    /// Tier identifier for metrics + logs ("tokio-udp" / "io-uring" /
    /// "xdp-af-xdp"). Stable per impl.
    fn tier_name(&self) -> &'static str;

    /// XDP fast-path hook (v1.2 only). Returns the eBPF DCID-routing
    /// map's file descriptor so the userspace publisher can call
    /// `bpf_map_update_elem` on flow add/remove. Other tiers return
    /// `None`. The router treats `None` as "no fast-path map; route
    /// every packet in userspace" (v1.0/v1.1 behavior).
    fn dcid_map_fd(&self) -> Option<i32> {
        None
    }
}

/// Callback shape for [`UdpDataplane::recv_loop`].
///
/// `Arc` so impls can clone it into per-task closures (io_uring
/// completion handlers, AF_XDP frame processors).
pub type PacketHandler<'a> = Arc<
    dyn for<'p> Fn(Packet<'p>) -> Pin<Box<dyn Future<Output = ()> + Send + 'p>>
    + Send + Sync + 'a,
>;

/// Factory: select the highest-capability tier the host supports.
///
/// `policy` lets operators force a specific tier (e.g. for testing
/// or when an XDP attach has been observed to silently drop on this
/// driver — see `lb_l4_xdp::nic_compat`).
pub enum TierPolicy {
    /// Walk the ladder XDP → io_uring → tokio-UDP, picking the first
    /// that initializes successfully. v1.0: only `TokioUdp` exists;
    /// `Auto` always selects it.
    Auto,
    /// Force tokio-UDP. Always available.
    TokioUdp,
    /// Force io_uring (v1.1+). Fails on kernels < 6.0 or hosts where
    /// `lb_io::Runtime::detect() != IoUring`.
    IoUring,
    /// Force XDP (v1.2+). Fails on kernels < 5.5, on non-native NIC,
    /// or against the silent-drop blocklist (`lb_l4_xdp::nic_compat`).
    Xdp { iface: String },
}

pub fn select_dataplane(
    bind: SocketAddr,
    policy: TierPolicy,
) -> Result<Arc<dyn UdpDataplane>, DataplaneError>;
```

### 10.3 What each tier MUST implement

Every tier MUST satisfy the **correctness contract**:

- `recv_loop` delivers EACH datagram EXACTLY ONCE in arrival order
  within a single 4-tuple (per-flow ordering; cross-flow reordering
  is allowed and matches kernel UDP).
- `send_to(buf, dst)` either fully sends `buf` or returns `Err`.
  Partial sends are not allowed (UDP is datagram-atomic).
- `local_addr` is stable for the lifetime of the impl.
- Cancellation via the `CancellationToken` returns from `recv_loop`
  within one packet of cancel fire (one in-flight `on_packet` future
  may complete first; no new packets are observed after that).
- The impl MUST NOT panic on transient OS errors. Fatal errors
  surface as `DataplaneError::Unavailable` so the router can fall
  back.
- The impl MUST NOT decrypt, inspect, or modify packet payloads —
  the dataplane is bytes-in / bytes-out. (Reinforces the
  no-decrypt property; the router is the only place that even
  parses the public header.)

Tier-specific additions:

- **TokioUdp:** uses `tokio::net::UdpSocket::recv_from` + `send_to`.
  Single-task recv loop. Per-packet `Vec<u8>` allocation acceptable
  at v1.0 scale; a future PR may pool with `BytesMut`. No special
  kernel requirements.
- **IoUring (v1.1):** uses `IORING_OP_RECVMSG_MULTISHOT` (kernel 6.0+).
  Recv buffer pool sized to `2 * max_quic_connections` (one packet
  in-flight per flow). Falls back via `Unavailable` if the kernel
  rejects multishot. Must NOT silently fall back internally — that's
  the router/factory's decision.
- **Xdp (v1.2):** AF_XDP socket per RX queue, eBPF DCID-routing
  program in `lb-l4-xdp/ebpf/`. `dcid_map_fd` exposes the BPF map
  pinned at `<bpffs>/quic_dcid_routes` so the userspace publisher
  (running in passthrough.rs) calls `bpf_map_update_elem` on every
  flow add/remove. Userspace tier still handles all flows not
  resolved by the fast path (Initial, Retry, short-header miss).
  Must NOT depend on `lb_l4_xdp::ConntrackTable` — the XDP QUIC
  steerer is a SEPARATE eBPF program from the existing 5-tuple
  conntracker, owned by lb-quic.

### 10.4 Lifecycle

```
┌────── listener spawn ──────────────────────────────────────┐
│ 1. select_dataplane(bind, TierPolicy::Auto)                │
│    → returns Arc<dyn UdpDataplane>                         │
│ 2. dataplane.local_addr() logged to operator               │
│ 3. router.spawn(dataplane, ...) clones the Arc             │
│ 4. router calls dataplane.recv_loop(cancel, on_packet)     │
│    via tokio::spawn — runs until cancel                    │
│ 5. on cancel: recv_loop returns Ok(());                    │
│    Arc drop runs impl-specific cleanup (close UdpSocket,   │
│    detach XDP program via XdpLoader::detach_verifying,     │
│    close io_uring fd).                                     │
└────────────────────────────────────────────────────────────┘
```

XDP detach uses the existing `lb_l4_xdp::loader::XdpLoader`
discipline (netlink prog-id verify, `DetachLeftProgramAttached`
hard error). The QUIC-DCID eBPF program is loaded/attached/detached
through the same loader code path — single-sourced (R12) with the
5-tuple conntrack program.

### 10.5 Implementation matrix v1.0 → v1.2

| Tier               | Module                                | Cargo feature        | Ship | Verify-gate test                       |
| ------------------ | ------------------------------------- | -------------------- | ---- | -------------------------------------- |
| TokioUdp           | `lb-quic/src/udp_tokio.rs`            | (default)            | v1.0 | A2 verify gate (i): real QUIC wire E2E |
| IoUring            | `lb-quic/src/udp_iouring.rs`          | `quic-passthrough-iouring` | v1.1 | Differential vs TokioUdp on same harness; same A2 (i) test parametrised over tier policy |
| Xdp (DCID steerer) | `lb-quic/src/udp_xdp.rs` + `lb-l4-xdp/ebpf/src/quic_dcid_steer.rs` | `quic-passthrough-xdp` | v1.2 | E2E E2E with XDP attach via `XdpLoader::attach_with_fallback`; fall-through check (short-header miss → userspace tier 3 picks up); R13 a/b/c on map eviction |

v1.0 ships only the default feature; `select_dataplane` is a `match`
with exactly one arm. v1.1 and v1.2 add arms; passthrough.rs is
unchanged.

### 10.6 What stays out of the trait

- **Connection-ID parsing.** SHARED-1 lives in
  `lb-quic/src/public_header.rs` and is called by the ROUTER from
  inside `on_packet`, not by the dataplane. Dataplanes are
  protocol-blind.
- **Flow table state.** `Table: DashMap<Vec<u8>, Arc<FlowEntry>>`
  lives in the router. The XDP fast-path map is a SEPARATE eBPF
  map keyed on a DCID prefix — its key/value shape is owned by
  the XDP tier impl and published by the router via the `dcid_map_fd`
  hook.
- **Retry mint/verify.** Lives in the router (calls
  `lb_security::RetryTokenSigner`). Dataplane never sees tokens.

The trait is intentionally minimal: bind, recv-loop, send-to, name,
optional map FD. Adding methods later is a breaking change, so the
v1 surface is exactly what every tier needs and nothing else.

## 11. Operator-facing notes (for DEPLOYMENT.md addendum)

This section captures the operator-facing characteristics of Mode A
v1 so Increment A4 (promote) propagates them to `DEPLOYMENT.md` and
`RUNBOOK.md` verbatim. Audit reviewers reading this design can verify
the operator surface matches the build target.

### 11.1 Datapath tier in v1

> **Mode A passthrough v1.0 ships the tier-3 (`tokio-UDP`) datapath
> only. The XDP and `io_uring` fast paths are later performance tiers
> — v1.1 will add `io_uring` (Linux 6.0+); v1.2 will add an XDP DCID
> steerer (Linux 5.5+, native-attach-capable NIC). Mode A v1.0 works
> on any kernel that supports `tokio::net::UdpSocket` and is fully
> correct passthrough — it routes by Connection ID, preserves
> end-to-end TLS, and bounds state. XDP is a throughput tier, not a
> correctness gap. No operator action is required to use the v1.0
> tier; future tiers are opt-in behind Cargo features
> (`quic-passthrough-iouring`, `quic-passthrough-xdp`) and a
> `TierPolicy::Auto` or `::Xdp{iface}` knob in the listener config.**

### 11.2 `strict_source_binding` — per-pool, default false

> **`strict_source_binding` is a per-pool configuration knob on Mode A
> listeners; the default is `false`. With the default, the LB
> tolerates source-IP changes within a tracked flow (NAT rebind,
> mobile network handover) — packets continue to route by DCID to
> the original backend. When set to `true` for a pool, the LB drops
> short-header packets whose source 4-tuple differs from the flow's
> recorded peer, breaking NAT-rebind path migration in exchange for
> faster off-path-spoof rejection at the LB (before the backend's
> AEAD layer). Recommended posture: leave at default for mobile-
> facing pools; flip to `true` for internal pools where clients have
> stable addresses and adversary off-path injection is a concern.**

### 11.3 Backend operational contracts

> **Mode A passthrough imposes two contracts on backends:**
>
> **B1. Connection-ID length stability.** A backend MUST keep its
> server-issued SCID length constant for a connection's lifetime. The
> LB records the backend's SCID length at handshake and uses it to
> parse subsequent short-header DCIDs. Backends rotating to a
> different-length CID via `NEW_CONNECTION_ID` will desync the LB
> parser and connections may drop. quiche 0.28 backends: set a fixed
> length in `Config::set_max_idle_timeout` adjacent code paths and
> verify with `quiche::ConnectionId::len()` audit.
>
> **B2. CID-rotation limit.** A backend SHOULD set
> `active_connection_id_limit = 2` (RFC 9000 §17.2 minimum) and SHOULD
> NOT retire the original client-known CID for the connection lifetime.
> Server-issued CID rotation via `NEW_CONNECTION_ID` is encrypted; the
> LB cannot track it and the client may end up with a CID the LB has
> never seen, manifesting as connection drop. Backends with strict
> rotation requirements should run behind Mode B (terminate-and-
> re-originate, S16), not Mode A.**

### 11.4 0-RTT replay protection

> **In Mode A passthrough, 0-RTT replay protection is the BACKEND's
> responsibility. The LB does not terminate TLS and cannot decrypt
> 0-RTT early data. Backends that accept 0-RTT MUST implement their
> own anti-replay window (quiche: server-side dedup on session-ticket
> nonces). The LB's `ZeroRttReplayGuard` only catches naive on-path
> Initial duplicates — defence-in-depth, not the primary control.**

### 11.5 Bounded state — capacity planning

> **Default `max_quic_connections = 100_000`. Each tracked flow
> occupies two routing-table entries and one per-flow backend UDP
> socket. Sized memory:
> ~ `2 * 100_000 * (FlowEntry-size + dashmap-overhead) + 100_000
> sockets`. The LB process MUST be granted `ulimit -n 200_000` (the
> binary requests `setrlimit(NOFILE, 200_000)` at startup and logs
> a warning if denied). When the cap is reached, new Initial packets
> with no Retry token receive a Retry; new Initials with valid Retry
> tokens are dropped with a single `audit/quic_passthrough_cap_hit`
> log line per `audit_throttle_window_secs`. No OOM, no panic — the
> bound is enforced.**

### 11.6 Promotion note for Increment A4

The §11.1–§11.5 blocks copy verbatim into a new
`DEPLOYMENT.md#quic-passthrough-mode-a-v10` section during
Increment A4 (promote). The S15 final report records the
DEPLOYMENT.md diff as part of the promote-gate evidence.

## 12. Phase 1 — owner rulings (RESOLVED)

All five §9 open items resolved on 2026-05-28 — see
`audit/quic/s15-owner-rulings.md` (commit 05ecea73).

| §9 item                              | Ruling                                                                                                   | Phase 2 binding                                                                                                                                                                                          |
| ------------------------------------ | -------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 1 — Both-mode arch                   | CONFIRM. SHARED-1 + SHARED-2 correct; R12 single-sourcing correct.                                       | Proceed to Phase 2 as designed. SHARED-2 contract pinned in §10 above (owner addendum 1).                                                                                                                |
| 1' — `strict_source_binding` default | PER-POOL knob, DEFAULT **false**. Mobile availability > unbounded AEAD CPU savings.                      | Increment A2: knob lives on per-pool config, default false. A3 verify gate (iii): NAT-rebind test runs with default false; ADDITIONAL test runs `strict_source_binding=true` on one pool to prove the knob fires (untested config option is worse than no option). |
| 2 — Retry without quiche in v1?      | **SHIP IN v1.** Retry is the primary Initial-flood defence.                                              | Increment A2: ~80 LOC hand-rolled long-header writer + RFC 9001 §5.8 differential test against `quiche::retry`.                                                                                          |
| 3 — `min_client_dcid_len` floor      | DEFAULT **8**.                                                                                           | Increment A2: knob lives on the listener config, default 8. Initials with shorter DCID dropped with audit log.                                                                                           |
| 4 — `max_quic_connections` default   | DEFAULT **100_000**.                                                                                     | Increment A2: knob default 100_000; cap-entries = 2 × cap. §11.5 above documents capacity-planning surface.                                                                                              |
| 5 — NEVER-DECRYPTED proof            | Construction over observation: linkage (cargo bloat) + state (type-level FlowEntry assertion) + kprobe cross-check. DROP seccomp. | Increment A2 verify gate (ii): three-part proof exactly as ruled. The `cfg(not(feature = "quic-passthrough-only"))` guards land in A2; `FlowEntry` MUST hold no keying material (type-level assertion + reviewer code-read in A2 verify report). |
| XDP / io_uring deferral              | ENDORSE v1=tier-3-only; tier-2 → v1.1; tier-1 → v1.2; SHARED-2 contract documented this session.         | §10 above (this commit). §11.1 above for operator-facing note. The tier ladder is not in Phase 2 v1.0 scope beyond the trait.                                                                            |

Phase 2 build cleared. Increment A1 (SHARED-1 public-header parser)
is unblocked and is the next task on the queue.
