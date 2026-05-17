# Batched low-severity plans — PROTO-2-06, PROTO-2-08, PROTO-2-13

These three findings are low severity and lend themselves to a single
mechanical Round-4 commit. Each gets its own section below; all three
land together in one PR to minimise file-touch churn in proto's lane.

---

## Plan for PROTO-2-06 — Rename mis-labelled `conformance_h{1,2,3}.rs`

Finding-ref:    PROTO-2-06 (low, Open)
Files touched:
  - `tests/conformance_h1.rs` → `tests/codec_roundtrip_h1.rs` (rename)
  - `tests/conformance_h2.rs` → `tests/codec_roundtrip_h2.rs` (rename)
  - `tests/conformance_h3.rs` → `tests/codec_roundtrip_h3.rs` (rename)
  - Each renamed file: prepend a top-of-file doc block clarifying
    scope (codec round-trip only; not a server-conformance harness)

Approach:
Option (a) from the round-2 review (the immediate fix). Option (b)
— replacing with real server-conformance harnesses — is delivered
separately: PROTO-2-05 (`h3i` for H3) and the existing `h2spec.rs`
runner for H2. The H1 conformance subset is covered by
`tests/h1_proxy_e2e.rs`; no rename or extraction needed for H1.

Top-of-file doc block (uniform across all three renamed files):

```rust
//! Codec round-trip tests for the in-tree `lb-h{1,2,3}` parser/encoder
//! crates. This file does **not** spawn the gateway and is **not** a
//! server-conformance harness — see `tests/h2spec.rs` (H2) and
//! `tests/h3spec.rs` (H3) for those.
```

Proof:
  - Test: `tests/codec_roundtrip_h1.rs` (renamed; existing tests
    still pass). Invariant unchanged.
  - Test: `tests/codec_roundtrip_h2.rs` (renamed). Invariant
    unchanged.
  - Test: `tests/codec_roundtrip_h3.rs` (renamed). Invariant
    unchanged.

Risk / blast radius:
  - Rename only; no semantic change. CI dashboards that referred to
    `conformance_h{1,2,3}` by name need updating. The renamed files
    keep the same `#[test]` function names, so cargo-test filters
    that target specific cases continue to work modulo prefix.

Cross-ref:    paired with PROTO-2-05 (real h3 conformance) and the
              pre-existing `tests/h2spec.rs` (h2 conformance).

---

## Plan for PROTO-2-08 — Remove spurious `trailers` HeaderName entry

Finding-ref:    PROTO-2-08 (low, Open)
Files touched:
  - `crates/lb-l7/src/util/hop.rs` (the new module created by
    PROTO-2-07 — the cleaned `HOP_BY_HOP` list lives here; the
    spurious entry never makes the move)
  - `crates/lb-l7/src/h1_proxy.rs:54-63` (the original list — gets
    deleted entirely as part of the PROTO-2-07 migration; the
    deletion is the fix for PROTO-2-08)
  - Inline doc comment on the new `HOP_BY_HOP` constant: cite
    RFC 9110 §7.6.1 and list the canonical names.

Approach:
The current list at `h1_proxy.rs:60` contains
`HeaderName::from_static("trailers")` which is not a real header
name (the "trailers" token exists only as a *value* inside
`TE: trailers` per RFC 9110 §10.1.4). When PROTO-2-07's
`StrippedRequest` newtype lands, `strip_hop_by_hop` and its
`HOP_BY_HOP` constant relocate to `crates/lb-l7/src/util/hop.rs`.
The relocation drops the `trailers` entry and adds a doc-comment
citing the canonical eight names per RFC 9110 §7.6.1:

```rust
/// Hop-by-hop header names per RFC 9110 §7.6.1. The list is
/// CLOSED: any name appearing in the `Connection` header field is
/// also treated as hop-by-hop and removed at runtime by
/// `strip_hop_by_hop`'s connection-listed-names pass.
const HOP_BY_HOP: &[http::HeaderName] = &[
    http::header::CONNECTION,
    http::HeaderName::from_static("keep-alive"),
    http::header::PROXY_AUTHENTICATE,
    http::header::PROXY_AUTHORIZATION,
    http::header::TE,
    http::header::TRAILER,
    http::header::TRANSFER_ENCODING,
    http::header::UPGRADE,
];
```

Note `TRAILER` (singular) **is** included — it is hop-by-hop per
RFC 9110 §7.6.1's reference into RFC 9112's chunked framing
(announces hop-only chunked trailers). The round-2 review's
parenthetical that "Trailer is not hop-by-hop" is incorrect; the
canonical eight per RFC 9110 §7.6.1 do include `Trailer` as
hop-by-hop in the context of HTTP/1.1 chunked framing. PROTO-2-12's
trailer pass-through plumbing uses the separate
`strip_hop_by_hop_trailers` helper which operates on trailer *field
names* (a different set of forbidden names per RFC 9110 §6.6.1).

Proof:
  - Test: `tests/hop_by_hop_canonical_set.rs::hop_by_hop_constant_matches_rfc`
    Invariant: `HOP_BY_HOP` contains exactly the 8 RFC-9110-§7.6.1
    names; does NOT contain `trailers` (plural).
  - Test (regression): the existing
    `tests/bridge_hop_by_hop_stripped.rs::keep_alive_stripped`
    (defined under PROTO-2-07) continues to pass — the relocation
    is behaviour-preserving for every real header name.

Risk / blast radius:
  - Zero runtime — removing a no-op entry from a removal list.

Cross-ref:    folded into PROTO-2-07's edit; closes PROTO-2-08.

---

## Plan for PROTO-2-13 — Wire-level assertion of `SETTINGS_ENABLE_CONNECT_PROTOCOL`

Finding-ref:    PROTO-2-13 (low, Open)
Files touched:
  - `tests/h2_settings_advertisement.rs` (new)

Approach:
Today `crates/lb-l7/src/h2_proxy.rs:246` calls
`builder.enable_connect_protocol()`. Round-2 verified it ships; the
gap is "no CI signal protects this from a future refactor that
drops the call". A wire-level test reads the server's initial
SETTINGS frame and asserts the bit:

```rust
// tests/h2_settings_advertisement.rs

#[tokio::test]
async fn h2_server_advertises_enable_connect_protocol() {
    let (addr, _gw) = spawn_h2_listener_loopback().await;
    let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
    // Send H2 connection preface
    let mut io = tcp;
    io.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n").await.unwrap();
    // Read server's SETTINGS frame; parse via h2::frame::Settings
    let frame = read_one_h2_frame(&mut io).await.unwrap();
    assert_eq!(frame.frame_type(), 0x4); // SETTINGS
    let settings = h2::frame::Settings::load(frame.head(), &frame.payload())
        .expect("parse SETTINGS");
    let enable_connect = settings.iter()
        .find(|(id, _)| *id == 0x8) // SETTINGS_ENABLE_CONNECT_PROTOCOL
        .map(|(_, v)| v);
    assert_eq!(enable_connect, Some(1));
}
```

The frame-parse helper is small; if `h2` crate frame types are
unwieldy in test scope, a hand-rolled 9-byte SETTINGS header
parse suffices (frame length 6, type 0x4, flags 0, stream 0; then
6-byte tuples of (id: u16, value: u32)). The test asserts the
`(0x8, 1)` tuple appears.

For h1s listeners (TLS-fronted HTTP/2), repeat the test wrapped in
a rustls client — the SETTINGS frame is post-handshake; same parse
logic applies.

Proof:
  - Test: `tests/h2_settings_advertisement.rs::h2_server_advertises_enable_connect_protocol`
    Invariant: initial SETTINGS frame on a fresh H2 connection
    contains setting id `0x8` (`SETTINGS_ENABLE_CONNECT_PROTOCOL`)
    with value `1`.
  - Test: `tests/h2_settings_advertisement.rs::h2s_server_advertises_enable_connect_protocol`
    Invariant: same, behind TLS.

Risk / blast radius:
  - Zero source change. Adds two CI tests; runtime <1 s.

Cross-ref:    composes with PROTO-2-04 (WebSocket conformance — both
              touch the extended-CONNECT plumbing on the H2 server
              side); closes PROTO-2-13.

---

# Batch summary
Files touched (all three findings combined):
  - `tests/conformance_h1.rs` → renamed
  - `tests/conformance_h2.rs` → renamed
  - `tests/conformance_h3.rs` → renamed
  - `crates/lb-l7/src/util/hop.rs` (gains the cleaned constant)
  - `crates/lb-l7/src/h1_proxy.rs:54-63` (old list deleted)
  - `tests/hop_by_hop_canonical_set.rs` (new)
  - `tests/h2_settings_advertisement.rs` (new)

Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
