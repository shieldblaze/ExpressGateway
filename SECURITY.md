# Security Model & Defenses

Scope: the L4/L7 load balancer process (`lb` binary) and its dependencies. This document is a threat model plus a defenses table mapping attacks → code sites.

## Threat model

**Assets**
- The availability of the proxied service (the gateway must not become the bottleneck or a DoS amplifier).
- The confidentiality and integrity of traffic on the TLS hops.
- The integrity of configuration and the control plane.

**Trust boundaries**
1. **Untrusted clients**: connect over TCP or UDP from the public internet. The gateway treats every byte as hostile until the protocol parser has validated it.
2. **Semi-trusted backend pool**: responses are forwarded largely unchanged; malicious or compromised backends can influence headers and body but not run code in the gateway.
3. **Trusted control plane**: operator-provided TOML config, SIGHUP reload, and the `lb-cp-client` channel.
4. **Trusted kernel**: XDP programs run in the kernel verifier sandbox; userspace loader requires `CAP_BPF`.

**Not in the threat model (yet)**
- Side-channel timing attacks across TLS session tickets (ticket rotator mitigates key compromise, not timing).
- Rowhammer / speculative-execution attacks against the host.
- A malicious operator with shell access.

## Panic-free posture

Every library crate has the `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::todo, clippy::unimplemented, clippy::unreachable, missing_docs)]` header. `scripts/halting-gate.sh` check 3 ("Cloudflare 2025 outage rule") greps every `crates/*/src/*.rs` (excluding `#[cfg(test)]` blocks) for those constructs and fails the build if any appear. The 2025 Cloudflare outage originated from an `.unwrap()` in a dashboard service; we borrow the rule. ADR-0010 records the enforcement architecture.

## Defenses table

Each row maps an attack to the code site that mitigates it, plus a reference.

| # | Attack | Layer | Code site | Reference |
|---|--------|-------|-----------|-----------|
| 1 | HTTP/1.1 CL.TE smuggling | L7 H1 | `crates/lb-h1/src/parse.rs` (rejects Content-Length + Transfer-Encoding combined; RFC 9112 §6.1) | `tests/security_smuggling_cl_te.rs` |
| 2 | HTTP/1.1 TE.CL smuggling | L7 H1 | same parser, same rule | `tests/security_smuggling_te_cl.rs` |
| 3 | HTTP/1.1 → HTTP/2 smuggling via `Upgrade: h2c` | L7 H2 | `crates/lb-security/src/smuggle.rs::SmuggleDetector::check_h2_downgrade` | `tests/security_smuggling_h2_downgrade.rs` |
| 4 | Oversize request headers | L7 H1 | `crates/lb-h1/src/parse.rs::MAX_HEADER_BYTES = 65_536`, `H1Error::HeadersTooLarge` | Unit tests `header_exactly_at_limit_accepted`, `header_over_limit_rejected` |
| 5 | HTTP/2 CONTINUATION flood (CVE-2024-27316) | L7 H2 | `crates/lb-h2/src/security.rs::ContinuationFloodDetector` | `tests/security_continuation_flood.rs` |
| 6 | HTTP/2 Rapid Reset (CVE-2023-44487) | L7 H2 | `crates/lb-h2/src/security.rs::RapidResetDetector` (integer two-bucket sliding window ×1000) | `tests/security_rapid_reset.rs` |
| 7 | HTTP/2 HPACK bomb | L7 H2 | `crates/lb-h2/src/security.rs::HpackBombDetector` | `tests/security_hpack_bomb.rs` |
| 8 | HTTP/2 SETTINGS flood | L7 H2 | `crates/lb-h2/src/security.rs::SettingsFloodDetector` (100 / 10 s) | Unit test `settings_burst_rejected` |
| 9 | HTTP/2 PING flood | L7 H2 | `crates/lb-h2/src/security.rs::PingFloodDetector` (50 / 10 s) | Unit test `ping_burst_rejected` |
| 10 | HTTP/2 zero-window stall | L7 H2 | `crates/lb-h2/src/security.rs::ZeroWindowStallDetector` (30 s) | Unit test `zero_window_stall_fires_after_timeout` |
| 11 | HTTP/3 QPACK bomb | L7 H3 | `crates/lb-h3/src/security.rs::QpackBombDetector` | `tests/security_qpack_bomb.rs` |
| 12 | Slowloris (slow headers) | L7 H1 | `crates/lb-security/src/slowloris.rs::SlowlorisGuard` | `tests/security_slowloris.rs` |
| 13 | Slow-POST (slow body) | L7 H1 | `crates/lb-security/src/slow_post.rs::SlowPostGuard` | `tests/security_slow_post.rs` |
| 14 | QUIC 0-RTT replay | L5 QUIC | `crates/lb-security/src/zero_rtt.rs::ZeroRttReplayFilter` | `tests/security_zero_rtt_replay.rs` |
| 15 | Upstream stale-connection reuse after peer FIN | Pool | `crates/lb-io/src/pool.rs` non-blocking read-zero probe before reuse (Pingora EC-01) | Unit test `probe_discards_peer_closed_connection` |
| 16 | Unbounded pool growth | Pool | `crates/lb-io/src/pool.rs` `per_peer_max` + `total_max` enforced on `PooledTcp::drop`; `max_age` + `idle_timeout` eviction | Unit tests `per_peer_max_enforced`, `total_max_enforced`, `size_invariant_holds_under_random_ops` |
| 17 | DNS cache poisoning via stale entries after NXDOMAIN | DNS | `crates/lb-io/src/dns.rs` negative-TTL caches NXDOMAIN for 5 s by default; positive TTL capped at 300 s | Unit test `negative_entry_caches_nxdomain` |
| 18 | TLS session-ticket-key compromise | TLS | `crates/lb-security/src/ticket.rs::TicketRotator` rotates daily with an overlap window; `RotatingTicketer` impls `rustls::server::ProducesTickets` | Unit tests `rotate_if_due_swaps_keys_at_interval`, `overlap_preserves_previous_for_decrypt` |
| 19 | Compression bomb (zip-of-zip) | L7 | `crates/lb-compression/src/lib.rs::CompressionGuard` bounded decompression | `tests/compression_bomb_cap.rs` |
| 20 | BREACH / compression-oracle via response body | L7 | `crates/lb-compression` refuses to recompress responses that mix secrets and attacker-controlled input (posture) | `tests/compression_breach_posture.rs` |
| 21 | Panic-as-DoS (e.g. the Cloudflare 2025 incident) | Whole codebase | Crate-root `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::unimplemented, clippy::unreachable, clippy::indexing_slicing)]` + halting-gate check 3 grep | ADR-0010 |

## Cross-reference

The full adversarial catalog with prose descriptions, CVE IDs, and mitigation rationale is in `docs/research/dos-catalog.md`. The cross-cutting-themes doc (`docs/research/cross-cutting.md`) discusses why each defense exists where it does. `docs/research/pingora.md` (Cloudflare Pingora study) contains the edge-case catalog — EC-01 (non-blocking read-zero pool probe) is realized in `crates/lb-io/src/pool.rs`; EC-04 (CONTINUATION flood) and EC-05 (Rapid Reset) in `crates/lb-h2/src/security.rs`; EC-11 (ticket-key rotation) in `crates/lb-security/src/ticket.rs`; EC-16 (per-peer pool LRU) in both `crates/lb-io/src/pool.rs` (TCP) and `crates/lb-io/src/quic_pool.rs` (QUIC).

## Residual risks

These are tracked in `docs/gap-analysis.md` and the relevant ADRs; operators should understand them before deploying:

- **XDP path is source-only**: the L4 eBPF program at `crates/lb-l4-xdp/ebpf/src/main.rs` is real aya-ebpf source but is not compiled into a loadable BPF ELF in this drive (`bpf-linker` requires rustc ≥ 1.88; MSRV is 1.85). Until the build lands, L4 ACL / conntrack / rate-limit defenses documented above run in the userspace simulation only. See ADR-0004.
- **Detectors land without wiring**: the SETTINGS / PING / zero-window / ticket-rotator detectors exist as tested types but are not yet invoked from a live H2 / TLS connection state machine. That wiring is Pillar 3b. Meanwhile, they cover codec-level unit tests but do not protect a deployed listener.
- **`cargo audit` advisory `RUSTSEC-2026-0009`**: the `time` crate below 0.3.47 has an RFC 2822 parser stack-exhaustion DoS. It is pulled in via `rcgen` → `yasna` → `time`. Our code never feeds untrusted RFC 2822 strings to `time`; it is used only for certificate-validity ranges. Both `deny.toml` and `.cargo/audit.toml` document the ignore with justification. Pillar 3b bumps MSRV to 1.88 and drops the ignore.

## Responsible disclosure

Report security issues to **security@shieldblaze.com** (GPG key published on the project website). Include:

- Reproduction steps (ideally a minimal PoC).
- Affected version (`cargo pkgid` output or commit SHA).
- Proposed severity (CVSS v3.1 vector if known).

We acknowledge within 3 business days. We coordinate public disclosure after a fix is shipped; typical embargo is 30 days for high-severity issues, negotiated case-by-case.

## Secrets

`trufflehog git file://<repo> --only-verified` is a required pre-release gate per FINAL_REVIEW §9.7. Current scan: 0 verified, 0 unverified secrets. Do not commit credentials or tokens; there is no `.env` tracked in this repo and there never should be.
