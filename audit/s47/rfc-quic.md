# S47 — QUIC transport RFC conformance review (RFC 9000 / 9001 / 9002)

**Reviewer:** rfc-quic · **Base:** `review/s47-rfc-security` from main @ `01915a77`
**Method:** line-by-line adversarial read of the QUIC datapath. NO builds/tests run
(2 vCPU box; verification is the lead's CI job). RFC text quoted from
`rfc-editor.org` canonical `.txt` (RFC 9000, 9001, 9287, 9369) — fetched, not recalled.

**Scope covered:** `public_header.rs` (613), `passthrough.rs` (1695), `router.rs` (671),
`listener.rs` (506), `udp_dataplane.rs` (313), `conn_actor.rs` (1571), `raw_proxy.rs` (2076),
`cleanup_guard.rs` (60), `lib.rs`, plus `lb-security/src/retry.rs` and
`lb-io/src/quic_pool.rs` (reached from the datapath).

---

## Verdict on the S38 conclusion (re-verified)

**S38's "the only wire parser we own is `lb_quic::public_header`" is CONFIRMED.**
Every other datagram-level parse on the QUIC path is `quiche::Header::from_slice`
(`router.rs:142`) or inside `quiche::Connection::recv`. No second hand-rolled parser
exists on the data path. Grep of all `crates/lb-quic/src/*.rs` with `#[cfg(test)]`
tails stripped finds **zero** `unwrap()/expect()/panic!/unreachable!` and exactly one
raw slice — `passthrough.rs:844 scid[..8].copy_from_slice(...)`, a constant range on a
`[u8; 16]` array, so it is compile-time bounds-checked.

**`public_header.rs` is panic-free. No CRITICAL finding.** Every read is `.get()`,
every offset add is `checked_add`/`saturating_add`, `decode_varint`'s
`1usize << (first >> 6)` can only yield 1/2/4/8, and the `usize::try_from(len_val)
.unwrap_or(usize::MAX)` at :265 is the *safe* direction (a huge declared length forces
`Truncated`). The fuzz target `fuzz/fuzz_targets/quic_public_header.rs` exists and
varies `short_dcid_len` over `0..=24`, which is the right shape. The findings below are
**logic and protocol** defects, not memory-safety defects.

---

## Findings, ranked

| ID | Sev | Site | Claim |
|---|---|---|---|
| QUIC-01 | **HIGH** | `router.rs:154-160`, `passthrough.rs:570-586` | No RFC 9000 §14.1 1200-byte gate: an 8-byte spoofed Initial elicits a 92-byte Retry = **11.5× UDP reflector** on the default H3 listener |
| QUIC-02 | **HIGH** | `passthrough.rs:555-562`, `:769`, `:795` | Mode A rebinds a flow's return path to any unauthenticated packet's source, with no path validation (RFC 9000 §9.3) → sustained-bandwidth reflector; `strict_source_binding=true` is bypassable via a long-header Initial |
| QUIC-03 | MEDIUM | `passthrough.rs:591-598` | Retry token's ODCID binding is verified then discarded → a client re-controls Maglev backend selection and can pin all flows to one backend (RFC 9000 §8.1.4) |
| QUIC-04 | MEDIUM | `public_header.rs:230-235`, `passthrough.rs:267` | Long-header type bits and the Retry Integrity Tag key are version-specific; both are hardcoded to v1 → QUIC v2 (RFC 9369) is 100% broken through Mode A and the LB emits invalid Retries for any non-v1 version |
| QUIC-05 | MEDIUM | `public_header.rs:142-144` | RFC 9287 QUIC-bit greasing (negotiated invisibly to the LB) makes Mode A drop every 1-RTT packet → connection death against nginx/msquic/Google-QUICHE backends |
| QUIC-06 | LOW | `router.rs:154-160` | No Version Negotiation packet is ever sent (RFC 9000 §5.2.2 SHOULD); an unsupported-version client gets silence |
| QUIC-07 | LOW | `passthrough.rs:732-739` | Reverse-path SCID learning inserts routing keys with no cap check → a malicious backend grows the flow table past `2 × max_quic_connections` |
| QUIC-08 | LOW | `passthrough.rs:836-847`, `router.rs:344-364` | RNG-failure fallback emits a **predictable** connection ID while the comment claims "fail closed"; 0 bits of entropy vs RFC 9000 §8.1's 64-bit criterion |
| QUIC-09 | INFO | `public_header.rs:183-205` | v1's 20-byte CID cap is applied to all versions incl. VN (RFC 9000 §17.2 "SHOULD be able to read longer connection IDs from other QUIC versions") |
| QUIC-10 | INFO | `public_header.rs:142-144` | Fixed-bit rejection precedes the Version-Negotiation classification; §17.2 exempts VN packets. Inert today (VN is server-origin and the reverse path ignores parse errors) — latent trap |
| QUIC-11 | INFO | `raw_proxy.rs:691`, `:826` | 64 KiB + 16 KiB scratch `Vec`s allocated per relay pass (every 2 ms while active) — allocator churn, no correctness impact |

Clean scopes with negative evidence in §Clean below: stateless reset, 0-RTT replay,
connection-entry leaks, Mode B relay bounds, `udp_dataplane`.

---

## QUIC-01 — HIGH — Retry reflection: no RFC 9000 §14.1 minimum-datagram gate

### Code

`crates/lb-quic/src/router.rs:142-161` (H3-terminate listener — the **default** `protocol = "quic"` path):

```rust
let header = match Header::from_slice(pkt, quiche::MAX_CONN_ID_LEN) {
    Ok(h) => h,
    Err(e) => return Err(format!("header parse: {e}")),
};
let dcid_key: Vec<u8> = header.dcid.to_vec();
if let Some(sender) = connections.get(&dcid_key) { ... return Ok(()); }
if header.ty != Type::Initial {
    return Ok(());
}
let token_nonempty = header.token.as_ref().is_some_and(|t| !t.is_empty());
if !token_nonempty {
    return send_retry(&header, peer, local, params).await;
}
```

`crates/lb-quic/src/passthrough.rs:570-586` (Mode A):

```rust
let tok = token.unwrap_or(&[]);
if tok.is_empty() && ctx.params.mint_retry {
    // Mint Retry: stateless, so no flow is allocated.
    let new_scid = sample_lb_scid();
    let retry_token = ctx.retry_signer.mint(from, dcid);
    ...
    if let Err(e) = ctx.listener_sock.send_to(&out, from).await {
```

Neither path — nor `listener.rs`, nor `udp_dataplane.rs` — ever compares the datagram
length against 1200. `rg '1200' crates/lb-quic/src/` returns only
`max_recv_udp_payload_size` config plumbing and a test fixture.

### Spec

RFC 9000 §14.1: *"A server **MUST** discard an Initial packet that is carried in a UDP
datagram with a payload that is smaller than the smallest allowed maximum datagram size
of 1200 bytes."*

RFC 9000 §8.1: *"Prior to validating the client address, servers **MUST NOT** send more
than three times as many bytes as the number of bytes they have received. This limits
the magnitude of any amplification attack that can be mounted using spoofed source
addresses."*

§14.1 exists precisely to make §8.1 satisfiable: the 1200-byte floor is what guarantees
the ~100-byte Retry is a *de*-amplification.

### Exploit — exact bytes

**(a) H3-terminate listener (default deployment), 8-byte datagram:**

```
c0                 byte0: long form, fixed bit, type 0b00 = Initial
00 00 00 01        version = 1
00                 DCID Len = 0
00                 SCID Len = 0
00                 Token Length varint = 0
                   → 8 bytes of UDP payload
```

`quiche::Header::from_bytes` (verified in
`~/.cargo/registry/.../quiche-0.29.1/src/packet.rs:355-412`) accepts this: it reads
first/version/dcid_len/dcid/scid_len/scid and, for `Type::Initial`, one
`get_bytes_with_varint_length()`. It does **not** check the fixed bit, a minimum DCID
length, or the datagram size. `header.ty == Initial`, `token = Some(vec![])` ⇒
`token_nonempty == false` ⇒ `send_retry`.

`send_retry` (`router.rs:222-245`) mints `RetryTokenSigner::mint(peer, &header.dcid)`.
For a v4 peer with an empty ODCID the token is
`1 + 8 + 1 + 4 + 2 + 1 + 0 + 32 = 49` bytes (layout at `lb-security/src/retry.rs:3-5`).
`quiche::retry` then writes
`byte0(1) + version(4) + DCIDLen(1) + DCID(0) + SCIDLen(1) + SCID(20, from
sample_conn_id() → [u8; quiche::MAX_CONN_ID_LEN]) + token(49) + tag(16)` = **92 bytes**.

**Amplification = 92 / 8 = 11.5×.** (IPv6 peer: token 61 → 104 bytes → **13×**.)

**(b) Mode A passthrough, 17-byte datagram** (`min_client_dcid_len = 8` forces the DCID):

```
c0 00 00 00 01 08 <8 dcid bytes> 00 00 00
                   → 17 bytes: byte0, version, dcid_len=8, dcid, scid_len=0,
                     token_len varint=0, length varint=0
```

`build_retry_packet` emits `1+4+1+0+1+16+token(57)+16` = **96 bytes** →
**5.6×** (IPv6: 108 → 6.4×).

Both exceed the §8.1 3× ceiling. Source spoofing is unconstrained — the LB replies to
whatever `recv_from` reports, so any victim IP can be targeted. There is no per-source
Retry rate limit on either path (the S15 design lists one as "deferred v1.1",
`audit/quic/s15-design.md` §6.5(b)).

**Secondary impact — self-DoS.** `router_main` (`router.rs:99-130`) awaits
`dispatch_packet` inline in the single recv task, and `send_retry` awaits
`socket.send_to`. Each spoofed 8-byte Initial therefore costs one HMAC-SHA256 + one
`sendto` **on the same task that dispatches every existing connection's packets**.
A flood does not merely reflect; it starves every live QUIC connection on that listener.
Mode A has the identical shape (`udp_dataplane.rs:186` awaits `on_packet(pkt)` inline,
and `handle_initial` additionally awaits `UdpSocket::bind` + `connect` per new flow).

### Prior art check — NOT already-known, and the documented rationale is refuted

`audit/quic/s15-design.md:516-524` §6.1 "CID-amplification (small Initial → large Retry)"
ruled this **"Not exploitable for amplification"**. Both premises of that ruling are
false against the shipped code:

> "Attacker sends a **1200-byte** Initial with no token" — the code never requires 1200.
> "Retry is on the order of 80 bytes (header + **16-byte token** + 16-byte integrity tag)"
> — the token is 49–69 bytes, not 16, so the Retry is 88–108 bytes.

S38 (`s38-findings-resource.md:139-142`) reasoned only about the *connection table*
("an off-path spoofed-source attacker cannot fill the table"), which remains true and is
orthogonal. `docs/known-limitations.md` "Mode A passthrough relies on the QUIC Retry
round-trip" documents the per-IP cap gap, not reflection. No document anywhere mentions
§14.1 or a datagram-size floor.

### Would a test catch it? No.

`crates/lb-quic/tests/router_accept_path.rs` has 3 tests, all driving **real quiche
clients**, which pad every Initial to 1200 per §14.1 — the under-size case is
unreachable from them. `passthrough.rs`'s in-crate tests call `handle_initial` directly
with a synthetic `vec![0u8; 8]` payload and never assert on datagram size. No test in
`tests/`, `crates/lb-quic/tests/`, or `fuzz/` sends a short raw Initial and measures the
response size.

### Fix shape (not applied)

One length gate before the `Type::Initial` branch in both routers:
`if pkt.len() < 1200 { return Ok(()); }` — and, for the H3 path, add the §7.2 8-byte
client-DCID floor Mode A already has, which also removes the `connections`
empty-`Vec<u8>` dispatch key an attacker can otherwise claim
(`router.rs:301-302` inserts `header.dcid.to_vec()` verbatim).

---

## QUIC-02 — HIGH — Mode A rebinds the return path on unauthenticated packets; the documented guard is bypassable

### Code

Three call sites move a live flow's return address, all on packets that have been through
**no** cryptographic check (Mode A holds no keys by construction):

`crates/lb-quic/src/passthrough.rs:555-562` — **long-header Initial**, no gate at all:

```rust
if let Some(entry) = ctx.table.get(dcid) {
    let flow = Arc::clone(entry.value());
    drop(entry);
    flow.touch(Instant::now(), ctx.epoch);
    flow.set_peer(from);
    let _ = flow.backlog_tx.try_send(pkt);
    return;
}
```

`passthrough.rs:765-770` and `:791-796` — short header, gated only by
`forward_short_via` (`:805-834`), which returns `true` immediately when
`strict_source_binding` is false:

```rust
if !forward_short_via(ctx, &flow, pkt, from) {
    return; // strict-source-binding drop
}
flow.touch(Instant::now(), ctx.epoch);
flow.set_peer(from);
```

`reverse_pump` then sends every backend byte to whatever was last stored
(`passthrough.rs:741-742`):

```rust
let peer = flow.get_peer();
if let Err(e) = ctx.listener_sock.send_to(slice, peer).await {
```

### Spec

RFC 9000 §9.3: *"If the recipient permits the migration, it MUST send subsequent packets
to the new peer address and **MUST initiate path validation** (Section 8.2) to verify the
peer's ownership of the address."*

RFC 9000 §9.3.1: *"It is possible that a peer is spoofing its source address to cause an
endpoint to send excessive amounts of data to an unwilling host. If the endpoint sends
significantly more data than the spoofing peer, connection migration might be used to
amplify the volume of data that an attacker can generate toward a victim... Until a
peer's address is deemed valid, an endpoint limits the amount of data it sends to that
address; see Section 8. In the absence of this limit, an endpoint risks being used for a
denial-of-service attack against an unsuspecting victim."*

RFC 9000 §9.3.2: *"an endpoint **MUST revert to using the last validated peer address**
when validation of a new peer address fails."*

**The architecture nullifies the protection at both layers.** The LB performs the address
switch with no validation (it cannot — no keys). The backend, which *does* have keys and
would normally run PATH_CHALLENGE, never sees the change: Mode A gives each flow its own
`connect()`-ed backend socket (`passthrough.rs:634-651`), so the backend observes a
permanently stable 4-tuple. §9's anti-amplification machinery is therefore inert
end-to-end.

### Exploit — no CID guessing required

The attacker uses **its own** legitimate Mode A connection, so it knows every routing key
exactly (the LB-chosen `new_scid` it echoed, and the backend SCID learned at
`passthrough.rs:736`):

1. Attacker opens a normal Mode A flow and requests a large object from the backend.
2. Attacker keeps sending its own ACK packets — which it can encrypt correctly, it is the
   endpoint — but writes the **victim's IP** in the UDP source address.
3. Every such packet hits `forward_short` → `set_peer(victim)`. The backend's entire
   response stream is now delivered by the LB to the victim, from the LB's own IP.
4. Because the attacker keeps ACKing, congestion control never stalls; the reflection is
   *sustained*, not a one-shot burst. Ratio ≈ (object bytes)/(ACK bytes), i.e. 50–100×+
   and unbounded in duration.

The victim sees a QUIC flood sourced from the gateway. The backend sees a healthy
connection. The LB emits no warning: `set_peer` is unconditional and unlogged.

**The documented mitigation does not close it.** `docs/guide/CONFIG.md:56` presents
`strict_source_binding = true` as the hardening knob ("hardens against off-path 4-tuple
confusion"). With it set, steps 2–3 still work verbatim by sending a **long-header
Initial** carrying the same DCID: `handle_initial:555-562` looks the flow up, calls
`set_peer(from)` and returns *before* any `forward_short_via` check exists on that path.
The only precondition is `dcid.len() >= min_client_dcid_len` (8), and the LB-chosen
`new_scid` is 16 bytes. So an operator who has read the foot-gun table and enabled the
guard is still fully exposed.

### Prior art check — the analysis exists and reached the wrong conclusion

`audit/quic/s15-design.md:547-566` §6.3 "Routing-table poisoning" considered a spoofed
short-header packet and concluded: *"Backend decrypts → AEAD fails ... **connection
survives.** Defended at the TLS layer. The threat reduces to a backend CPU-burn DoS."*
That analysis only followed the packet **forward** to the backend. It never considered
that the same packet mutates `FlowEntry.peer` and redirects the **reverse** stream. The
connection does not survive — its return path is captured. §6.3's own knob is then
described as "configurable, default enabled", contradicted three lines later by "default
false in v1" (the code default, `passthrough.rs:101`).

`docs/known-limitations.md` does not mention return-path rebinding at all.

### Would a test catch it? No.

`passthrough.rs::forward_short_via_strict_source_table` (`:1561-1597`) tests
`forward_short_via` in isolation across all four (strict × match) cases — it proves the
gate function works, but never calls `handle_initial` with a mismatched source, and never
asserts anything about `get_peer()` after a mismatched packet. No test observes the
reverse pump's destination address changing.

---

## QUIC-03 — MEDIUM — the Retry token's ODCID binding is verified and thrown away

### Code

`crates/lb-quic/src/passthrough.rs:589-598`:

```rust
let now = Instant::now();
if !tok.is_empty() {
    if let Err(e) = ctx.retry_signer.verify(tok, from, now) {
        ...
        return;
    }
}
```

`RetryTokenSigner::verify` returns `Result<Vec<u8>, RetryError>` where the `Ok` payload is
the ODCID the token was minted for (`lb-security/src/retry.rs:199`). Mode A discards it.
The wire DCID of the second Initial is then used directly as the routing key and, at
`passthrough.rs:625`, as the Maglev hash input:

```rust
let Some(backend) = pick_backend(&ctx, dcid) else { ... };
```

(The H3-terminate router does use it — `router.rs:163` binds `odcid_vec` into
`quiche::accept_with_retry`. The gap is Mode A only.)

### Spec

RFC 9000 §8.1.4: *"Attackers could replay tokens to use servers as amplifiers in DDoS
attacks. To protect against such attacks, servers **MUST ensure that replay of tokens is
prevented or limited**... Servers are encouraged to allow tokens to be used only once, if
possible; tokens MAY include additional information about clients to further narrow
applicability or reuse."*

The 10 s `DEFAULT_RETRY_MAX_AGE` (`retry.rs:26`) is the only limit, so the MUST is
arguably met; the defect is that the token *carries* the narrowing information and the
code declines to use it.

### Exploit — defeat the LB-chosen-CID property, then target one backend

With `mint_retry = true` (the default), the honest flow is: client Initial (DCID `X`) →
LB Retry (SCID = random `new_scid`) → client's second Initial has DCID = `new_scid`.
Because `new_scid` is **LB-chosen**, the client cannot influence
`pick_backend(hash_dcid_for_maglev(dcid))`. That is the entire reason Mode A's routing is
not client-steerable.

Attack: complete **one** Retry round trip from a real address to obtain token `T`. `T` is
bound to the address and to ODCID `X`, but only the address is checked. For the next 10 s
send Initials `(token = T, DCID = D_i)` with attacker-chosen `D_i`. Each passes verify,
each creates a flow, and each is hashed with `D_i`. `hash_dcid_for_maglev`
(`passthrough.rs:202-216`) is a fixed, published, unkeyed FNV-style mix, so the attacker
brute-forces `D_i` offline such that Maglev picks backend *k* every time — concentrating
100% of passthrough traffic onto one backend, with the other backends idle. Each flow also
costs the LB a UDP socket fd + 2 tasks.

The state-exhaustion half of this is **ALREADY-KNOWN**: `docs/known-limitations.md`
"Mode A passthrough relies on the QUIC Retry round-trip" and S38 F-RES-3 both record
"a single real IP can consume the whole budget". The **backend-steering** half is new: it
is the property the LB-chosen CID was supposed to provide, and one round trip buys 10 s of
unrestricted CID choice.

### Would a test catch it? No.

`passthrough.rs::token_verify_reject_then_accept` (`:1334-1378`) mints a token for
`(from, dcid2)` and replays it with the **same** `dcid2`. Nothing asserts that a token
minted for one ODCID is rejected against a different DCID.

### Fix shape

`let odcid = ctx.retry_signer.verify(tok, from, now)?;` then require the wire DCID to be
the `new_scid` the LB issued for that ODCID — which needs the Retry to be stateless-but-
bound (e.g. derive `new_scid = HMAC(secret, odcid || peer)` instead of
`sample_lb_scid()`, so the check stays stateless). That also makes the routing key
verifiable rather than merely random.

---

## QUIC-04 — MEDIUM — v1-only type bits and a v1-only Retry key applied to every version

### Code

`crates/lb-quic/src/public_header.rs:230-235`:

```rust
let ty = match (b0 >> 4) & 0x03 {
    0b00 => LongType::Initial,
    0b01 => LongType::ZeroRtt,
    0b10 => LongType::Handshake,
    _ => LongType::Retry,
};
```

`crates/lb-quic/src/passthrough.rs:37-45, 267-270` — the tag key is a constant, used for
whatever version the attacker/client put on the wire (`build_retry_packet` takes
`version: u32` at `:236` and writes it at `:290` but never branches on it):

```rust
/// RFC 9001 §5.8 — fixed Retry Integrity Tag key for QUIC v1.
const RETRY_KEY_V1: [u8; 16] = [ 0xbe, 0x0c, ... ];
...
let unbound = aead::UnboundKey::new(&aead::AES_128_GCM, &RETRY_KEY_V1)
```

### Spec

RFC 9000 §17.2: *"The header form bit, Destination and Source Connection ID lengths,
Destination and Source Connection ID fields, and Version fields of a long header packet
are version independent. The other fields ... are version specific."* Table 5's
type mapping is explicitly scoped: *"In this version of QUIC, the following packet types
with the long header are defined."*

RFC 9369 (QUIC v2) §3.2: *"All version 2 Long Header packet types are different. The Type
field values are: Initial: 0b01, 0-RTT: 0b10, Handshake: 0b11, **Retry: 0b00**."*

RFC 9369 §3.3.3: the Retry Integrity Tag key/nonce **change** for v2
(`key = 0x8fb4b01b56ac48e260fbcbcead7ccc92`,
`nonce = 0xd86969bc2d7c6d9990efb04a`). RFC 9001 §5.8's constants are v1's.

### Concrete failures

1. **QUIC v2 is totally broken through Mode A.** A v2 Initial (type bits `0b01`) is
   classified `ZeroRtt` (`public_header.rs:232`) → `handle_inbound:512-515` routes it to
   `forward_long_existing` → table miss → **silently dropped**. No flow is ever created,
   so no v2 connection can be established. A v2 Handshake (`0b11`) is classified `Retry`
   and dropped as "client-origin Retry/VN" (`:516-519`). Send:
   `c1 6b 33 43 cf 08 <8 dcid> 00 ...` (v2 = `0x6b3343cf`, type bits `0b01`) — nothing
   happens, forever.
2. **Invalid Retries for any non-v1 version.** For a long header with type bits `0b00`
   and *any* non-zero version — a real v2 Retry, a GREASE version, a future version —
   Mode A takes the Initial branch and calls `build_retry_packet(..., version, ...)`,
   which echoes that version but seals the tag with `RETRY_KEY_V1`. RFC 9001 §5.8 makes
   the tag the client's only check that the Retry came from an entity that saw the
   Initial; a v2 client recomputes with the v2 key, mismatches, and MUST discard. The
   client retries, the LB mints another invalid Retry, and the connection attempt dies at
   the timeout. This also widens QUIC-01: the reflector works for *any* version value on
   the Mode A path, whereas the H3 path is version-limited (`quiche::retry` returns
   `Err(UnknownVersion)` — verified at `quiche-0.29.1/src/packet.rs:762-764`).

### Would a test catch it? No — the test is pinned to v1 by construction.

`crates/lb-quic/tests/passthrough_retry_differential.rs:17-38` defines
`const QUIC_V1: u32 = 0x0000_0001` and passes it to both sides, with the comment
*"quiche only accepts QUIC v1"*. The one place our writer and quiche **diverge** —
non-v1 input, where quiche refuses and we emit — is exactly what the differential cannot
reach. `public_header.rs`'s unit tests all use `version = 1` or `0`.

### Fix shape

Reject non-v1 versions before the type-bit match (return the version to the caller and
let Mode A forward-by-CID only), or gate `build_retry_packet` on
`version == 0x0000_0001`.

---

## QUIC-05 — MEDIUM — RFC 9287 QUIC-bit greasing silently kills Mode A connections

### Code

`crates/lb-quic/src/public_header.rs:141-144`:

```rust
// RFC 9000 §17.2/§17.3 — Fixed Bit MUST be 1.
if b0 & 0x40 == 0 {
    return Err(HeaderError::FixedBitClear);
}
```

`handle_inbound:489-498` drops the datagram on any parse error.

### Spec

RFC 9287 §3: *"An endpoint that advertises the grease_quic_bit transport parameter MUST
accept packets with the QUIC Bit set to a value of 0."*
§3.1: *"Endpoints that receive the grease_quic_bit transport parameter from a peer
**SHOULD set the QUIC Bit to an unpredictable value**... Endpoints can set the QUIC Bit
to 0 on all packets that are sent after receiving and processing transport parameters."*
§1 frames the hazard for exactly this component: *"Where endpoints and the intermediaries
that support them do not depend on the QUIC Bit having a fixed value, sending the same
value in every packet is more of a liability than an asset."*

`grease_quic_bit` (0x2ab2) is a Standards-Track transport parameter, and transport
parameters are inside the encrypted CRYPTO stream — **Mode A cannot observe the
negotiation**.

### Failure scenario

Backend is nginx-quic, msquic, or Google QUICHE (all implement RFC 9287; the Rust
`quiche 0.29.1` used for the *terminate* path does not — `grep -rn grease_quic_bit` in
the vendored crate returns nothing, so the H3 listener is unaffected). Client is Chrome.
Both advertise `grease_quic_bit`; per §3.1 the client SHOULD then randomize bit 0x40 on
its 1-RTT packets. Every such packet arrives at Mode A with `b0 & 0x40 == 0`, hits
`FixedBitClear`, is dropped, and only bumps `header_parse_errors_total`. The handshake
completes (Initials keep the bit set until transport parameters are processed) and then
**the connection dies mid-session** at the idle timeout. Roughly half of all 1-RTT packets
if the client randomizes, all of them if it clears.

The reverse direction is unaffected — `reverse_pump:732` only *reads* the header for SCID
learning and relays the bytes regardless.

This is not an RFC 9000 violation (for un-extended v1 the drop is correct, §17.3), it is
an unobservable-extension incompatibility. It belongs in `docs/known-limitations.md`
alongside the existing backend contracts, and the right code posture is arguably to make
the fixed-bit check advisory for short headers (routing does not depend on it).

### Would a test catch it? No.

No test constructs a short header with the QUIC bit cleared;
`public_header.rs::fixed_bit_clear_short_rejected` (`:431-439`) asserts the *current*
behaviour is a rejection, i.e. it would actively resist the fix.

---

## QUIC-06 — LOW — no Version Negotiation packet is ever sent

`crates/lb-quic/src/router.rs:154-160` has no `quiche::negotiate_version` call
(`rg 'negotiate_version' crates/` → no hits). An Initial with an unsupported version
whose type bits happen to be `0b00` reaches `send_retry`, where `quiche::retry` returns
`Err(UnknownVersion)` (`quiche-0.29.1/src/packet.rs:762`); `dispatch_packet` returns `Err`
and `router_main:117` logs at `debug!`. Nothing is sent to the client.

RFC 9000 §5.2.2: *"If a server receives a packet that indicates an unsupported version
and if the packet is large enough to initiate a new connection for any supported version,
the server **SHOULD** send a Version Negotiation packet... Servers MUST drop smaller
packets that specify unsupported versions."*

Impact: a client offering only a version we do not speak (a future version, or a v2-first
client) waits out its handshake timeout instead of immediately learning to downgrade.
Low, because no deployed client is v2-first today. Note the §5.2.2 companion sentence is
the same 1200-byte anti-amplification rule as QUIC-01 — a VN implementation must carry the
size gate with it, since VN echoes both CIDs and would otherwise be a second reflector.

Not covered by h3spec: the 12 named waivers in `scripts/ci/h3spec-check.sh` are all
transport-parameter validation and QPACK items; h3spec drives a v1 client and never
probes VN.

---

## QUIC-07 — LOW — reverse-path SCID learning bypasses the flow-table cap

`crates/lb-quic/src/passthrough.rs:730-739`:

```rust
if let Ok(PublicHeader::Long { scid, .. }) = parse_public_header(slice, 0) {
    if !scid.is_empty() {
        let key = scid.to_vec();
        // Avoid clobbering an existing entry (a different flow could legitimately own it).
        ctx.table.entry(key).or_insert_with(|| Arc::clone(&flow));
        flow.short_dcid_len.store(scid.len(), Ordering::Relaxed);
    }
}
```

The `max_quic_connections * 2` ceiling is enforced only on the client Initial path
(`:601-623`). This insert has no cap check and no per-flow key limit, so the invariant
documented in `docs/arch/quic-modes.md` ("the routing-table-entry cap is `2 × max`") does
not hold for backend-origin packets.

A malicious or compromised backend — explicitly in the threat model as "semi-trusted"
(`SECURITY.md` trust boundary 2) — emits long-header packets each carrying a fresh random
20-byte SCID. Each adds one `Vec<u8>` key plus DashMap overhead (~80 B) pointing at the
same `Arc<FlowEntry>`. At 500 kpps that is ~40 MB/s of table growth, bounded only by
`flow_idle_timeout` (default 60 s, and `reclaim_flows:409-427` does remove all keys
pointing at the victim `Arc`, so it is reclaimed rather than permanently leaked) —
≈2.4 GB peak per abusive backend, and the LRU scan in `evict_oldest:375-389` is O(table)
so it degrades with it.

Cheap fix: check `ctx.table.len() < cap*2` before the insert, or cap the learned keys per
flow at 2 (which is the documented model — the flow only ever needs the client key plus
one backend SCID).

---

## QUIC-08 — LOW — the RNG-failure fallback emits a predictable connection ID, contrary to its own comment

`crates/lb-quic/src/passthrough.rs:836-847`:

```rust
fn sample_lb_scid() -> [u8; LB_SCID_LEN] {
    let mut scid = [0u8; LB_SCID_LEN];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut scid).is_err() {
        // RNG failure on a supported platform is effectively impossible; fail closed rather than
        // emit a predictable SCID.
        use std::sync::atomic::AtomicU64;
        static FALLBACK: AtomicU64 = AtomicU64::new(0);
        let n = FALLBACK.fetch_add(1, Ordering::Relaxed);
        scid[..8].copy_from_slice(&n.to_be_bytes());
    }
    scid
}
```

The comment says "fail closed rather than emit a predictable SCID"; the code emits a
**monotonic counter in the high 8 bytes and zeros in the low 8** — the most predictable
possible value. `router.rs:344-364`'s `sample_conn_id` has the same shape (a
`subsec_nanos`-derived pattern, ~30 bits of guessable entropy at best, and fully
determined once one CID is observed).

RFC 9000 §8.1: *"an endpoint MAY consider the peer address validated if the peer uses a
connection ID chosen by the endpoint and the connection ID **contains at least 64 bits of
entropy**."* Mode A's whole Retry design leans on `new_scid` being unguessable — it is the
flow's routing key *and* the input to `pick_backend`. Under the fallback an off-path
attacker can enumerate live routing keys and mount QUIC-02 and QUIC-03 without ever being
on-path.

Reachability is near-zero (ring's `SystemRandom` on Linux is `getrandom(2)`), which is why
this is LOW — but the honest options are to propagate the error and drop the packet
(actually fail closed) or to correct the comment. The repo's comment-as-evidence-trail
convention makes the current state a trap for the next reader.

---

## QUIC-09 / QUIC-10 / QUIC-11 — INFO

**QUIC-09** — `public_header.rs:183-185` and `:203-205` reject `dcid_len`/`scid_len > 20`
before the `version == 0` check at `:219`. RFC 9000 §17.2 scopes the 20-byte cap to
version 1 (*"In QUIC version 1, this value MUST NOT exceed 20 bytes"*) and adds *"In order
to properly form a Version Negotiation packet, servers SHOULD be able to read longer
connection IDs from other QUIC versions"*; §17.2.1 adds *"Version-specific rules for the
connection ID therefore MUST NOT influence a decision about whether to send a Version
Negotiation packet."* Today this is inert (Mode A never sends VN — see QUIC-06 — and no
deployed version uses >20-byte CIDs). It becomes live the moment QUIC-06 is fixed.

**QUIC-10** — `public_header.rs:142-144` rejects a clear fixed bit before classifying VN.
RFC 9000 §17.2: *"Fixed Bit: The next bit (0x40) of byte 0 is set to 1, **unless the
packet is a Version Negotiation packet**"*, and §17.2.1: *"The value in the Unused field
is set to an arbitrary value by the server... servers SHOULD set the most significant bit
of this field (0x40) to 1"* — SHOULD, not MUST. Inert today: VN is server-origin, client
-origin VN is dropped anyway (`passthrough.rs:516-519`), and the reverse pump relays
backend packets whether or not the peek parses (`:732-742`). Flagged so a future reuse of
this parser on a server-origin path does not inherit the ordering bug.

**QUIC-11** — `raw_proxy.rs:691` (`let mut buf = vec![0u8; MAX_DGRAM_SIZE]` = 64 KiB per
`pump_dgram_dir` call) and `:826` (up to 16 KiB per `stream_recv` iteration). Both run on
every select wake, and `RELAY_TICK` is 2 ms while any stream or datagram is live, so a
busy Mode B connection allocates and frees ~64 MB/s. Correctness is fine; a reusable
scratch buffer in `run_dual_pump` would remove it.

---

## Clean — checked, with the negative evidence

- **Stateless reset (RFC 9000 §10.3, §21.11).** The gateway never emits one:
  `rg -i 'stateless_reset'` over `crates/` returns only doc/audit prose. `build_server_config`
  (`listener.rs:422-450`) never sets a stateless-reset token, so none is advertised and
  no reset can be generated or forged. §5.2.3's *"Server deployments that use this simple
  form of load balancing MUST avoid the creation of a stateless reset oracle"* is
  satisfied vacuously — Mode A drops unknown-CID short headers
  (`forward_short:800`, "Miss ⇒ drop") rather than answering them.
- **Path validation on the terminate path.** `set_disable_active_migration(true)`
  (`listener.rs:441`) plus quiche owning `recv` means QUIC-02 has no analogue there: an
  injected packet from a new address fails AEAD inside quiche and is discarded
  (`recv_single` funnels decrypt failures through `drop_pkt_on_err` → `Error::Done`).
- **0-RTT replay (RFC 9001 §4.6.1).** `router.rs:169-176` gates every valid-token Initial
  through `ZeroRttReplayGuard::check_0rtt_token` with key = SCID ‖ token[..32]
  (`build_replay_key`, `router.rs:192-199`). The guard is LRU (not FIFO) with a per-instance HMAC key
  (`zero_rtt.rs:1-27`), so neither a unique-token spray nor precomputed digest collisions
  walk through it. Captured victim Initials replayed from another address die earlier, at
  `verify` → `PeerMismatch`. Retransmit false-positives are avoided because the
  `connections` dispatch short-circuit (`router.rs:148-152`) runs before the guard. Mode A
  has no guard, which is the **ALREADY-KNOWN** documented position
  (`audit/quic/s15-design.md` §11.4: 0-RTT anti-replay is the backend's responsibility).
- **Connection-entry leaks.** `CidEntryGuard` (`cleanup_guard.rs`) is moved into the actor
  task (`router.rs:331-341`), so both dispatch keys are removed on clean exit, cancel-drop
  and unwind. The cap check (`router.rs:265-275`) runs before both inserts and after
  token verification. Mode A's `reclaim_flows` (`:401-437`) cancels the flow token first —
  the load-bearing step for an alive-but-silent backend — then removes every key by `Arc`
  identity, and the idle sweeper's Drop-flag assertion
  (`idle_sweep_reclaims_idle_flows_and_frees_them:1487-1501`) proves reclamation rather
  than mere unlinking.
- **Mode B relay bounds.** `STREAM_RELAY_WINDOW` (256 KiB/stream/direction),
  `admit_or_refuse` (`raw_proxy.rs:556-573`) and `BoundedDgramQueue` drop-newest give a
  hard `MAX_RELAY_STREAMS * 2 * 256 KiB` ceiling. Reset propagation uses the
  counterintuitive-but-correct `Shutdown::Write ⇒ RESET_STREAM` / `Shutdown::Read ⇒
  STOP_SENDING` mapping with an idempotency latch (`propagate_cancel`, `raw_proxy.rs:452-482`) and never
  synthesises a clean FIN on a truncated half.
- **`udp_dataplane.rs`.** Clean: `buf.get(..n).unwrap_or(&[])`, cancellation is `biased`
  first, transient recv errors log-and-continue.
- **Upstream leg source filtering.** `connect_and_drive` (`lb-io/src/quic_pool.rs:357`)
  binds without `connect()`, so `recv_from` accepts any source — but quiche funnels
  header-parse and decrypt failures to `Error::Done`, so injected packets are discarded
  and cannot abort the dial. Mode A's per-flow backend socket **is** `connect()`-ed
  (`passthrough.rs:644`), so the kernel filters there.
