# Plan for PROTO-2-02 — QUIC ALPN must be `b"h3"` (with `b"h3-29"` for compat)

Finding-ref:    PROTO-2-02 (critical, Open)
Files touched:
  - `crates/lb-quic/src/lib.rs` (the `LB_QUIC_ALPN` constant at line 115
    and the `build_config` `set_application_protos` call at line 392;
    note the synthesis §D allocation says
    `crates/lb-quic/src/config.rs` but the only `set_application_protos`
    call in the tree today lives in `lib.rs` — Round-4 will either keep
    it there or relocate to `config.rs` per the synthesis intent)
  - `tests/quic_alpn_h3.rs` (new)
  - Optionally `crates/lb-quic/src/lib.rs` test module (existing) — the
    in-tree loopback transport-only test rig that depended on the old
    `b"lb-quic"` token must move to a `#[cfg(test)]` constant

Approach:
RFC 9114 §3.1 mandates the ALPN identifier `h3` for HTTP/3.
quiche 0.28's `Config::set_application_protos` accepts any byte
strings the caller hands it (verified against quiche 0.28.0
`src/lib.rs` — the function is a pass-through into BoringSSL's
`SSL_CTX_set_alpn_protos`); the ALPN token list is wire-emitted
verbatim in the TLS 1.3 ClientHello / EncryptedExtensions exchange.
quiche imposes **no** restriction on the ALPN token, so `b"h3"` and
`b"h3-29"` are both accepted by the library.

Concrete change:

```rust
// crates/lb-quic/src/lib.rs (replaces lines 115 and 392)

/// Production ALPN tokens advertised by the H3 listener.
/// `h3` is the RFC 9114 IANA-registered identifier (mandatory).
/// `h3-29` is the last pre-RFC draft and still seen from clients
/// pinned to draft-29 (chromium <91, quic-go <0.31). Listed second so
/// negotiation prefers `h3`.
pub const H3_ALPN_PROTOS: &[&[u8]] = &[b"h3", b"h3-29"];

/// Test-only ALPN for the loopback transport-only rig that does not
/// speak H3 over the wire. Never emitted from `Role::Server`.
#[cfg(test)]
pub(crate) const LB_QUIC_TEST_ALPN: &[u8] = b"lb-quic";

// inside build_config:
let protos: &[&[u8]] = match role {
    Role::Server | Role::Client => H3_ALPN_PROTOS,
    #[cfg(test)]
    Role::TransportOnly => &[LB_QUIC_TEST_ALPN],
};
cfg.set_application_protos(protos)?;
```

If a `Role::TransportOnly` variant does not exist today, the test rig
gets a `build_config_for_test` parallel constructor; production
`build_config` always advertises `H3_ALPN_PROTOS`. The decision
between adding a `Role` variant vs. a parallel constructor is a
Round-4 implementation detail — both satisfy the invariant "no
production code path advertises anything other than `H3_ALPN_PROTOS`".

The `b"h3-29"` compat token is optional but cheap; lead may ratify
"`h3` only" if the deployment profile decides draft-29 clients are
out of scope. Default plan: keep both.

Proof:
  - Test: `tests/quic_alpn_h3.rs::server_advertises_h3`
    Invariant: spawn the QUIC listener; use a `quiche::Connection`
    client with `set_application_protos(&[b"h3"])`; complete the
    handshake; assert
    `conn.application_proto() == b"h3"`.
  - Test: `tests/quic_alpn_h3.rs::server_accepts_h3_29_legacy_client`
    Invariant: client sets `&[b"h3-29"]`; handshake succeeds;
    `conn.application_proto() == b"h3-29"`.
  - Test: `tests/quic_alpn_h3.rs::server_rejects_unknown_alpn`
    Invariant: client sets `&[b"lb-quic"]`; handshake fails with the
    quiche `TlsFail` / `no_application_protocol` shape.
  - Test (regression): `tests/quic_alpn_h3.rs::production_alpn_constant_is_h3`
    Invariant: static assertion `H3_ALPN_PROTOS[0] == b"h3"`.

Risk / blast radius:
  - **Breaking** for anything that today connects to the listener with
    the `b"lb-quic"` ALPN. The inventory confirms this is the in-tree
    test rig only; there is no production peer using `b"lb-quic"`.
  - Once `b"h3"` is advertised, real H3 clients (curl --http3,
    browsers, quic-go) will start negotiating; this is the *intent*
    of the fix but it does mean previously-unreachable code paths
    (QPACK encoder/decoder, h3_bridge) now run against external
    traffic. PROTO-2-05's `h3i` harness covers the next conformance
    layer.
  - 0-RTT and SEC-2-05's replay-window concerns become live once
    this lands (sec cross-review §G.2).

Cross-ref:    unblocks PROTO-2-05 (h3i needs a real `h3` ALPN target);
              unblocks SEC-2-05 (0-RTT replay surface becomes live);
              closes PROTO-2-02.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
