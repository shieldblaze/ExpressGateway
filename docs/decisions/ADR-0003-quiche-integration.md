# ADR-0003: QUIC transport — quinn 0.11

- Status: Accepted (realized 2026-04-22 via Pillar 3a)
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Supersedes: the previous revision of ADR-0003 ("in-house simulation, no
  quiche, no quinn"), which documented the pre-Pillar-3a stopgap.
- Consulted: Cloudflare quiche README, quinn-rs documentation, RFC 9000
  (QUIC), RFC 9001 (QUIC-TLS), RFC 9114 (HTTP/3), RFC 8446 (TLS 1.3).

## Context and problem statement

ExpressGateway must terminate or proxy QUIC + HTTP/3. The previous ADR
documented a userspace simulation (`QuicDatagram` / `QuicStream` with
`forward_*` validators) as a stopgap: at the time the halting gate's
panic-free rule plus the "no flaky network in CI" rule made even a
loopback QUIC handshake feel risky. That framing is no longer true.
Quinn 0.11's tokio runtime layer, combined with rcgen for in-process
self-signed certs, gives a deterministic loopback handshake in a single
test process without any external fixture. Pillar 3a makes `lb-quic` a
real transport.

Ground truth (post-Pillar-3a):

    $ grep -E '^name = "(quinn|quiche)"' Cargo.lock
    name = "quinn"
    name = "quinn-proto"
    name = "quinn-udp"

## Decision drivers

- **Fidelity**: integration tests must exercise a real UDP socket and a
  real TLS 1.3 handshake, otherwise the crate cannot defend against real
  QUIC mishandling.
- **CI determinism**: tests must still pass hermetically in sandboxed CI.
  Loopback (`127.0.0.1:0`) + in-process rcgen satisfy this without any
  external fixture.
- **Panic-free auditability** (ADR-0010): every byte-path crate runs
  under `#![deny(clippy::unwrap_used, …)]`. `lb-quic` keeps that header
  and routes all quinn errors through `?` into a typed `QuicError`.
- **Dependency blast radius**: quinn + rustls + ring + rcgen is ~20 new
  crates. Worth it for real transport; the ring backend (not aws-lc-rs)
  keeps the C footprint minimal and the license story simple.
- **License**: ExpressGateway is GPL-3.0-only. quinn (Apache-2.0 OR MIT),
  rustls (Apache-2.0 OR ISC OR MIT), rcgen (MIT OR Apache-2.0) are all
  compatible. ring is ISC AND MIT AND OpenSSL (covered by the existing
  `[[licenses.clarify]]` stanza in `deny.toml`).
- **Roadmap**: `lb-h3` (real H3 codec) must eventually ride on a real
  QUIC stack. Pillar 3a gives it a real transport seam; Pillar 3b wires
  `h3 / h3-quinn` on top.

## Considered options

1. Integrate **quinn** directly; run real QUIC on loopback in CI.
   **Chosen.**
2. Integrate **quiche** (Cloudflare). Rejected for Pillar 3a because it
   wraps BoringSSL (extra audit surface under `unwrap_used`), uses a
   callback I/O model that fights tokio, and offers no material
   advantage for our use cases over quinn's pure-Rust stack.
3. Keep the userspace simulation. Rejected — it is fiction, not a load
   balancer.
4. Feature-gate quinn behind a `real-quic` flag. Rejected — splits CI
   coverage and lets the simulation path rot.

## Decision outcome

Option 1. `lb-quic` adopts quinn 0.11 with rustls 0.23 (ring backend)
and rcgen 0.13 (dev-dep only, for in-process self-signed certs on
`127.0.0.1`).

Public surface:

- `QuicDatagram { connection_id, data }` and
  `QuicStream { stream_id, data, fin }` — unchanged data model.
- `forward_datagram` / `forward_stream` — retained as zero-I/O
  validators; documented as back-compat.
- `QuicEndpoint::server_on_loopback(cert_der, key_der)` and
  `QuicEndpoint::client_on_loopback(Arc<RootCertStore>)` — real UDP +
  TLS endpoints bound to `127.0.0.1:0`.
- `roundtrip_datagram` / `roundtrip_stream` — async functions that
  exercise quinn's datagram and unidirectional-stream APIs end-to-end.

The manifest-locked tests `test_quic_datagram_forwarding` and
`test_quic_stream_forwarding` now drive real quinn roundtrips; both
still pass, by name, under `bash scripts/halting-gate.sh`.

## Rationale

- `crates/lb-quic/Cargo.toml` now depends on `quinn`, `rustls`,
  `rustls-pki-types`, `bytes`, `thiserror`, `tokio`; `rcgen` is a
  dev-dep (test certs only, no cert infrastructure shipped).
- `Cargo.lock` contains `quinn`, `quinn-proto`, `quinn-udp` — the
  transport is not fiction.
- rustls is configured with `default-features = false,
  features = ["ring", "std", "tls12", "logging"]` to avoid pulling in
  `aws-lc-rs` alongside ring.
- rcgen 0.13 is pinned because 0.14 requires Rust 1.83+ and we track
  MSRV 1.85 already; a dev-dep with no binary impact.
- `time 0.3.47` would fix RUSTSEC-2026-0009 but requires Rust 1.88.
  We pin `time 0.3.37` via `Cargo.lock` and ignore the advisory with
  a documented justification in `deny.toml`: rcgen uses `time` for
  internal validity ranges only, not RFC 2822 parsing of untrusted
  input, so the stack-exhaustion vector is not reachable from our
  code.

## Consequences

### Positive
- CI now exercises a real UDP socket and a real TLS 1.3 handshake on
  loopback. The two QUIC tests in `tests/quic_native.rs` are genuine
  integration tests, not assertions over data structures.
- `lb-h3` gains a real transport seam to ride on in Pillar 3b.
- Panic-free invariants are preserved: every quinn error flows through
  a typed `QuicError` variant via `?`.

### Negative
- Dependency footprint grows by ~20 crates (quinn, quinn-proto,
  quinn-udp, rustls, ring, rustls-webpki, rustls-pki-types, rcgen and
  transitives). Release binary size grows accordingly.
- `time` crate ships an advisory we must ignore until MSRV bumps.

### Neutral
- License story unchanged (ring's OpenSSL-derived license was already
  clarified).
- The simulation's `forward_*` validators remain as zero-I/O helpers;
  the previous in-process test surface is preserved.

## Deferred items (explicitly Pillar 3b / 3c scope)

- **Stateless retry with token validation** (RFC 9000 §8.1.2).
- **0-RTT replay protection** (RFC 9001 §9.2, §5.6).
- **Alt-Svc (RFC 7838) injection** on L7 responses — needs the L7
  response path, deferred to Pillar 3c.
- **Connection-ID-routed pool** for upstream reuse — Pillar 3c.
- **Integration into the root binary** (`crates/lb/src/main.rs`) —
  Pillar 3b.
- **`h3` / `h3-quinn` on top of quinn** — Pillar 3b, when we actually
  serve HTTP/3.

## Implementation notes

- `crates/lb-quic/src/lib.rs` — real `QuicEndpoint` + `roundtrip_*`
  functions, plus retained `forward_*` validators and the typed data
  model.
- `crates/lb-quic/Cargo.toml` — adds quinn/rustls/rustls-pki-types as
  workspace deps; rcgen as dev-dep.
- `tests/quic_native.rs` — `#[tokio::test]` against loopback with
  rcgen-generated self-signed certs; manifest-locked test names
  preserved.
- `Cargo.toml` (workspace) — adds `quinn`, `rustls`,
  `rustls-pki-types` to `[workspace.dependencies]`.
- `deny.toml` — adds RUSTSEC-2026-0009 to the documented ignore list
  (see above).

## Sources

- `crates/lb-quic/src/lib.rs` — module-level doc comment.
- `crates/lb-quic/Cargo.toml` — actual dependencies.
- `Cargo.lock` — presence of `quinn`, `quinn-proto`, `quinn-udp`.
- <https://github.com/quinn-rs/quinn>
- <https://github.com/rustls/rcgen>
- RFC 9000, 9001, 9114, 8446.
