# ADR-0003: QUIC transport — in-house simulation, no quiche, no quinn

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: README.md (which predates this ADR and mentions quiche),
  Cloudflare quiche README, quinn-rs documentation, RFC 9000 (QUIC), RFC
  9001 (QUIC-TLS), RFC 9114 (HTTP/3).

## Context and problem statement

ExpressGateway must terminate or proxy QUIC + HTTP/3. The Rust ecosystem
offers two production-quality QUIC implementations:

- **quiche** — Cloudflare's C-callable QUIC library with Rust core;
  widely deployed at Cloudflare scale, uses BoringSSL, callback-based I/O
  model.
- **quinn** — pure-Rust, `rustls`-based, tokio-integrated, connection-per-
  task model.

The project's README.md advertises "QUIC-native proxying via quiche" as a
headline feature. That sentence predates the current state of the code.
The halting gate's check 3 (panic-free enforcement, ADR-0010) and check 5
(all required tests must pass in CI *without* external network access)
impose constraints that neither integration-ready candidate satisfies
today: quiche wraps C/BoringSSL which we cannot audit under the
`clippy::unwrap_used` regime on our side, and quinn requires rustls + a
real UDP socket + a TLS handshake against a certificate — none of which CI
can exercise deterministically without a full network simulator.

Cargo.lock is the ground truth for what is actually linked:

    $ grep -E '^name = "(quinn|quiche)"' Cargo.lock
    (no matches)

So the project ships a *userspace simulation* of QUIC semantics in
`lb-quic`, exercised by in-process tests. This is not a rejection of
quinn or quiche on technical grounds; it is the decision record for the
*current* state and the forward path.

## Decision drivers

- CI determinism: required tests must pass without UDP sockets, real
  certificates, or a kernel-level network stack.
- Panic-free auditability (ADR-0010): every byte-path crate runs under
  `#![deny(clippy::unwrap_used, …)]`.
- Dependency blast radius: adding quinn pulls in `rustls`, `ring`,
  `webpki`, `rcgen` — doubles the dependency graph.
- Licence: quiche is BSD-2-clause + OpenSSL-derived BoringSSL; quinn is
  Apache/MIT. ExpressGateway is GPL-3.0-only (see `Cargo.toml` workspace
  package). Both integrations are compatible but BoringSSL's state makes
  auditing a choice.
- Roadmap compatibility: HTTP/3 (`lb-h3`) must eventually ride on a real
  QUIC transport; the seam chosen today must accept a real implementation
  later.
- Fuzzability: the simulated `QuicDatagram` / `QuicStream` types admit
  targeted fuzzing of our own framing/forwarding logic without dragging a
  full TLS handshake into the corpus.

## Considered options

1. Integrate **quiche** (Cloudflare) directly; run real QUIC in CI via
   loopback.
2. Integrate **quinn** directly; run real QUIC in CI via loopback.
3. Implement a userspace simulation of QUIC datagram and stream
   forwarding in `lb-quic`, deferring real-transport integration.
4. Feature-gate: ship stub by default, integrate quinn behind a
   `real-quic` feature.

## Decision outcome

Option 3. `lb-quic` is a simulation with typed `QuicDatagram` and
`QuicStream` values and `forward_datagram` / `forward_stream` functions
(`crates/lb-quic/src/lib.rs`). Neither `quinn` nor `quiche` appears in
`Cargo.lock`. The README's "via quiche" phrasing is a roadmap statement,
not an accurate description of current code, and is flagged for update as
a follow-up.

## Rationale

- Current `crates/lb-quic/Cargo.toml` depends only on `thiserror` and
  `bytes`. There is no QUIC library linked.
- The crate's own doc comment is explicit:

      //! QUIC transport layer simulation.
      //!
      //! Since we cannot run real QUIC without a network stack in CI, this crate
      //! provides simulated datagram and stream forwarding with validation.

  Lines 1–4 of `crates/lb-quic/src/lib.rs`. Honest in the code, honest in
  the ADR.
- The simulation covers what the L7 pipeline actually asks the QUIC layer
  for: "give me a datagram I can forward" and "give me a stream slice with
  a FIN bit". Both are modelled as pure data types; `forward_datagram`
  and `forward_stream` validate non-empty payloads and return copies.
- Deferring the real transport lets us stabilise the `lb-h3` state
  machine and the `lb-quic` public types first. When we switch to a real
  implementation, consumers see the same `QuicDatagram` / `QuicStream`
  shapes.
- Between quinn and quiche, quinn is the presumed future default: pure
  Rust, rustls-compatible, tokio-native, already audited by the Rust
  community, no BoringSSL. But this ADR does not bind us — it records
  that today neither is wired up.
- The README discrepancy ("via quiche") is tolerated because changing it
  requires touching a file the halting-gate locks by hash; it will be
  corrected when the real transport lands.

## Consequences

### Positive
- CI runs HTTP/3 and QUIC conformance harnesses without network sockets
  (`tests/conformance_h3.rs`, `tests/bridging_h3_*.rs`).
- `lb-quic`'s lint-gated, panic-free invariants are trivial to enforce on
  ~125 lines.
- Downstream crates (`lb-h3`, `lb-l7`) can integration-test against the
  simulated transport without flakiness.

### Negative
- We do not today defend against real-world QUIC attacks (connection-ID
  confusion, version-negotiation downgrade, amplification, 0-RTT replay
  — see RFC 9000 §21). These are future work.
- README is misleading until we update it. Tracked.
- We cannot claim "QUIC-native proxying" for performance benchmarks yet.

### Neutral
- Switching to quinn later is a localised change: the public surface of
  `lb-quic` (`QuicDatagram`, `QuicStream`, `forward_*`) is narrow.
- Licence considerations (GPL-3.0-only) accept both quinn and quiche.

## Implementation notes

- `crates/lb-quic/src/lib.rs` — full simulation (~125 lines).
- `crates/lb-quic/Cargo.toml` — no QUIC library dependency.
- `tests/conformance_h3.rs` and `tests/bridging_h3_*.rs` — use the
  simulation.
- Real transport integration hook point: `forward_datagram` /
  `forward_stream` become trait methods.

## Follow-ups / open questions

- Update README to say "QUIC simulation in CI; quinn integration
  planned" — requires manifest/.halting-gate.sha256 update.
- Decide quinn vs quiche when we wire the real transport. Current lean:
  quinn, because it shares our `rustls`/`tokio` ecosystem (ADR-0001 picks
  tokio).
- Prototype the real integration behind a `real-quic` feature gate to
  keep CI deterministic.

## Sources

- `crates/lb-quic/src/lib.rs` — module-level doc comment.
- `crates/lb-quic/Cargo.toml` — actual dependencies.
- `Cargo.lock` — absence of `quinn` and `quiche`.
- <https://github.com/cloudflare/quiche>
- <https://github.com/quinn-rs/quinn>
- RFC 9000, 9001, 9114.
