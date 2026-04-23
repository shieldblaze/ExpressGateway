# ADR: QUIC stack — quinn → quiche + tokio-quiche

- Status: Accepted (spec change; migration work is Pillar 3b)
- Date: 2026-04-23
- Deciders: ExpressGateway team
- Supersedes: ADR-0003 (which selected quinn 0.11 for Pillar 3a)
- Related: `docs/research/pingora.md`, `docs/research/cloudflare-lb.md`, PROMPT.md §5 workspace deps, §8 TLS, §12 H3

## Context

Pillar 3a (commit `a50b6a81`) landed a real QUIC transport in `crates/lb-quic` using quinn 0.11 + rustls 0.23 (ring backend), replacing the former userspace simulation. That satisfied the minimum "no simulation" demand: `tests/quic_native.rs` now exercises a real UDP + TLS 1.3 handshake on loopback.

Before starting Pillar 3b (wiring QUIC into the root binary + adding stateless retry, 0-RTT replay defense, Alt-Svc injection, CID-routed upstream pool, and `curl --http3` interop), the lead re-evaluated the QUIC stack choice. The decision is to swap quinn for **quiche + tokio-quiche** for the reasons below. Pillar 3b will drive the migration.

## Forces

1. **"Learn from Cloudflare" is the project-wide standard.** The Phase A.0 research docs explicitly study pingora and katran as role models. Cloudflare's production HTTP/3 stack is quiche. Adopting quiche keeps the implementation and the research aligned.
2. **Performance evidence.** The community [quic-interop-runner](https://interop.seemann.io/) matrix and third-party benchmarks measurably show quiche ahead of quinn on throughput, and quiche passes interop scenarios quinn currently fails. PROMPT.md §26 targets >200 K HTTP/3 req/s on reference hardware; the margin matters.
3. **Maintenance model.** quinn is maintained by two volunteers working on their own time. quiche is maintained by Cloudflare's networking team as part of the infrastructure that serves Cloudflare's own edge traffic. For CVE response under production load, a vendor-scale team is the safer bet.
4. **Async integration handed to us.** `tokio-quiche` (open-sourced November 2025, 0.18 current) ships `H3Driver`, `ServerH3Driver`, `ClientH3Driver`, and the connection event loop that hides quiche's sans-io callback model behind tokio futures. It backs iCloud Private Relay and Cloudflare's Oxy proxy at millions of H3 rps. We avoid hand-writing the UDP ↔ quiche state-machine plumbing that would otherwise be required, along with every bug class in that plumbing.
5. **MSRV alignment.** quiche 0.28 and tokio-quiche 0.18 both declare `rust-version = 1.85`. The workspace stays on 1.85; the Pillar 3b MSRV bump previously planned for quinn/h3/h3-quinn integration is cancelled.

## Decision

Drop `quinn`, `h3`, `h3-quinn` from PROMPT.md §5 workspace deps. Add `quiche = "0.28"` and `tokio-quiche = "0.18"`. Pillar 3b rewrites `crates/lb-quic` around quiche's `Connection` + tokio-quiche's `H3Driver`, and rewrites `tests/quic_native.rs` accordingly.

`rustls = "0.23"` stays in workspace deps — it is used for the TLS *listener* path (Pillar 3b) and by the TicketRotator (Step 5b). The binary will ship with **two crypto stacks**: rustls (ring) for TLS-over-TCP, BoringSSL (via quiche) for TLS-over-QUIC. This is the same arrangement Pingora has in production, and is explicitly accepted.

## Considered options

1. **Keep quinn, push through Pillar 3b as previously planned.**
   - Pro: code already written for Pillar 3a.
   - Con: loses items 1–4 above; measurable perf hit; ties us to a volunteer maintainer for H3 CVE response.
2. **Adopt quinn for client, quiche for server (dual stack).**
   - Pro: some benchmarks show quinn clients competitive.
   - Con: two H3 implementations in one binary doubles attack surface and maintenance load with no clear gain.
3. **Write our own QUIC on top of rustls + udp crate.**
   - Pro: full control.
   - Con: multi-engineer-year effort; inevitable interop and CVE gaps; not credible for this project's scope.
4. **quiche + tokio-quiche (chosen).**
   - Pro: items 1–5 above.
   - Con: BoringSSL C dependency; `cmake` becomes a hard build requirement; reverting the Pillar 3a quinn code costs roughly 600 LoC of rework.

## Consequences

### Positive

- H3 implementation matches the proven Cloudflare production stack.
- `H3Driver` removes the sans-io integration burden. We write business logic, not event-loop plumbing.
- Workspace MSRV stays at 1.85 (no cascade into bpf-linker, tooling, or CI images).
- Cloudflare's networking team effectively becomes our upstream for CVE response on the QUIC path.
- `h3i` (quiche repo) is directly usable as the RFC 9114 MUST-clause harness in `.review/rfc-matrix.md`.

### Negative

- **BoringSSL C dep**: quiche links BoringSSL. We now ship two crypto stacks (rustls + BoringSSL) in one binary. Attack surface for crypto-implementation bugs doubles; we inherit BoringSSL's CVE stream alongside ring/rustls's. Mitigation: Pingora ships the same pairing in production; we adopt its operational posture (pin BoringSSL version, re-run `cargo deny` + `cargo audit` on BoringSSL updates, document BoringSSL build provenance in `DEPLOYMENT.md`).
- **`cmake` becomes a hard build dependency.** quiche's build.rs drives BoringSSL's cmake build. Anyone building from source needs `cmake` + a C/C++ toolchain on PATH. Document in `DEPLOYMENT.md` and `scripts/` after Pillar 3b lands. musl + aarch64 cross-builds grow more complex.
- **Pillar 3a rework**: `crates/lb-quic/src/lib.rs` (quinn-backed `QuicEndpoint`, ~470 LoC) and `tests/quic_native.rs` (rcgen + rustls ClientConfig, ~130 LoC) must be rewritten. The two manifest-locked test names (`test_quic_datagram_forwarding`, `test_quic_stream_forwarding`) stay.
- **`RUSTSEC-2026-0009`** (time <0.3.47 RFC 2822 DoS) stays ignored while rcgen remains a dev-dep. If Pillar 3b replaces rcgen with BoringSSL's own cert-minting helpers, the advisory drops on its own.

### Neutral

- ADR-0003 is superseded by this ADR; the old file is kept for audit trail.
- `rustls` stays in workspace deps; TicketRotator (Step 5b) is unchanged.

## Implementation notes (Pillar 3b scope)

1. Remove `quinn` from workspace deps; keep `rustls`. Drop the Pillar 3a quinn-client/server pair from `crates/lb-quic/src/lib.rs`.
2. Add `quiche = "0.28"` and `tokio-quiche = "0.18"` to `crates/lb-quic/Cargo.toml`.
3. Rewrite `QuicEndpoint` around `quiche::Config` + `tokio_quiche::ServerH3Driver`; preserve the public `QuicDatagram` / `QuicStream` types so downstream code isn't churned.
4. Rewrite `roundtrip_datagram` / `roundtrip_stream` using `H3Driver` primitives; keep the test function names in `tests/quic_native.rs` (manifest-locked).
5. Stateless retry with token validation → `quiche::Config::enable_retry(true)` + `SocketAddr`-scoped token verifier in the server driver.
6. 0-RTT replay → `quiche::Config::enable_early_data(false)` by default; explicit opt-in per route; existing `crates/lb-security/src/zero_rtt.rs::ZeroRttReplayFilter` wraps the decision.
7. Alt-Svc injection (RFC 7838) → H1/H2 response header from the same listener config that advertises the H3 port.
8. Per-peer CID-routed upstream connection pool → new module `crates/lb-io/src/quic_pool.rs`, applying Pingora EC-16.
9. `curl --http3` interop: new integration test under `tests/` at a non-manifest name (so the manifest hash stays valid). Use `[[test]]` `required-features` or a `#[cfg(target_os = "linux")]` gate if the host lacks `curl --http3`.
10. `h3i` (from the quiche repo) becomes the RFC 9114 MUST-clause harness. Vendor as a dev-dep if crates.io has it; otherwise add a git submodule.

## Follow-ups / open questions

- Decide the BoringSSL version pin strategy. Cloudflare tracks upstream; do we?
- Decide whether to keep rustls-based TLS-over-TCP or migrate TLS also to BoringSSL (single crypto stack). Current decision: keep rustls for TCP path; re-evaluate after Pillar 3b.
- Arm + musl cross-build CI requires cmake in the image. Update `.github/workflows/ci.yml` when the Pillar 3b migration lands.

## Sources

- quiche: <https://github.com/cloudflare/quiche>, <https://docs.rs/quiche>
- tokio-quiche: <https://github.com/cloudflare/quiche/tree/master/tokio-quiche>, <https://crates.io/crates/tokio-quiche> (newest 0.18.0 at time of decision)
- quic-interop-runner: <https://interop.seemann.io/>
- Cloudflare engineering on quiche at scale: <https://blog.cloudflare.com/tag/quiche/>
- Pingora's rustls + BoringSSL pairing: Pingora architecture talks + <https://blog.cloudflare.com/how-we-built-pingora>
- Phase A.0 research: `docs/research/pingora.md`, `docs/research/cloudflare-lb.md`, `docs/research/rfc9000.md`, `docs/research/rfc9114.md`
