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

> **Note on the production wire path.** The `lb-h1`/`lb-h2`/`lb-h3` codec
> crates cited below host the security *detector types* and are the
> framing layer used by the test-codecs; the **live production wire
> parsing** is delegated to hyper (H1/H2), quiche (H3/QUIC), and rustls
> (TLS), with the H2 flood/bomb thresholds applied on the live hyper H2
> builder (`crates/lb-l7/src/h2_security.rs::apply`) and the H1 smuggle
> check in `lb-security::SmuggleDetector`. The S38 audit confirmed these
> live defenses hold adversarially (`audit/security/s38-findings-*.md`).

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
| 12 | Slowloris (slow headers) | L7 H1 | `crates/lb-security/src/slowloris.rs::SlowlorisDetector` | `tests/security_slowloris.rs` |
| 13 | Slow-POST (slow body) | L7 H1 | `crates/lb-security/src/slow_post.rs::SlowPostGuard` | `tests/security_slow_post.rs` |
| 14 | QUIC 0-RTT replay | L5 QUIC | `crates/lb-security/src/zero_rtt.rs::ZeroRttReplayFilter` | `tests/security_zero_rtt_replay.rs` |
| 15 | Upstream stale-connection reuse after peer FIN | Pool | `crates/lb-io/src/pool.rs` non-blocking read-zero probe before reuse (Pingora EC-01) | Unit test `probe_discards_peer_closed_connection` |
| 16 | Unbounded pool growth | Pool | `crates/lb-io/src/pool.rs` `per_peer_max` + `total_max` enforced on `PooledTcp::drop`; `max_age` + `idle_timeout` eviction | Unit tests `per_peer_max_enforced`, `total_max_enforced`, `size_invariant_holds_under_random_ops` |
| 17 | DNS cache poisoning via stale entries after NXDOMAIN | DNS | `crates/lb-io/src/dns.rs` negative-TTL caches NXDOMAIN for 5 s by default; positive TTL capped at 300 s | Unit test `negative_entry_caches_nxdomain` |
| 18 | TLS session-ticket-key compromise | TLS | `crates/lb-security/src/ticket.rs::TicketRotator` rotates daily with an overlap window; `RotatingTicketer` impls `rustls::server::ProducesTickets` | Unit tests `rotate_if_due_swaps_keys_at_interval`, `overlap_preserves_previous_for_decrypt` |
| 19 | Compression bomb (zip-of-zip) | L7 | Out of scope — compression layer removed by L-001 in round-1. Upstream-Accept-Encoding headers are passed through unchanged; the gateway does not decompress responses. | n/a |
| 20 | BREACH / compression-oracle via response body | L7 | Out of scope — see row 19. The gateway never recompresses response bodies; BREACH risk lives at the origin tier where the response is generated. | n/a |
| 21 | Panic-as-DoS (e.g. the Cloudflare 2025 incident) | Whole codebase | Crate-root `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::todo, clippy::unimplemented, clippy::unreachable, clippy::indexing_slicing)]` + halting-gate check 3 grep | ADR-0010 |

## Cross-reference

The full adversarial catalog with prose descriptions, CVE IDs, and mitigation rationale is in `docs/research/dos-catalog.md`. The cross-cutting-themes doc (`docs/research/cross-cutting.md`) discusses why each defense exists where it does. `docs/research/pingora.md` (Cloudflare Pingora study) contains the edge-case catalog — EC-01 (non-blocking read-zero pool probe) is realized in `crates/lb-io/src/pool.rs`; EC-04 (CONTINUATION flood) and EC-05 (Rapid Reset) in `crates/lb-h2/src/security.rs`; EC-11 (ticket-key rotation) in `crates/lb-security/src/ticket.rs`; EC-16 (per-peer pool LRU) in both `crates/lb-io/src/pool.rs` (TCP) and `crates/lb-io/src/quic_pool.rs` (QUIC).

## Session 38 security audit posture

A four-auditor adversarial audit (parser / protocol / resource / infra)
was run against the full internet-facing, all-protocol deployment profile.
Result: **0 CRITICAL, 0 HIGH, 1 MEDIUM, 7 LOW, 4 INFO** — no auth bypass,
no smuggling/desync, no wire-reachable memory unsafety, no LB-down DoS, no
cert/verify bypass, no secret leak. Full record:
`audit/security/s38-findings.md` + the per-role
`s38-findings-{parser,protocol,resource,infra}.md` and
`s38-threat-model.md`.

**What defends the surface (proven adversarially, not assumed):**

- **Delegated wire parsing.** Production H1 is hyper, H2 is hyper/h2, H3
  and QUIC are quiche, WS framing is tungstenite, TLS is rustls/BoringSSL.
  Hand-rolled parsers are confined to test-codecs and a few validators.
- **Typed-header funnel.** Every path where attacker- or backend-controlled
  header/trailer bytes reach an H1 wire goes through hyper's typed
  `HeaderName`/`HeaderValue`/`response::Builder`, which reject CR/LF/NUL and
  fail closed; every H3-egress path is QPACK-encoded (binary). So
  CRLF/header injection cannot split a field on egress.
- **R8 bounded streaming + 64 MiB caps + 413** on every cell — no whole-body
  buffering; the LB does **not** decompress request/response bodies anywhere
  (Content-Encoding passed through verbatim → no bomb surface).
- **S36 H3 connection recycling** (`max_requests_per_h3_connection`) +
  `MAX_RELAY_STREAMS` (256) + `BoundedDgramQueue` — bound the quiche
  `StreamMap::collected` growth and relay state per connection adversarially.
- **H2 flood config applied on the live builder** — Rapid-Reset,
  CONTINUATION, HPACK-bomb, SETTINGS, zero-window all enforced (with the
  timer wired); h2 ≥ 0.4.14 enforces CONTINUATION internally.
- **Reload honesty-contract** — validate-first; swappable vs
  restart-required is exhaustive; no torn snapshot or cross-connection
  config bleed (per-connection single `load_full` RCU).
- **Admin auth** — SHA-256 + constant-time compare; loopback-default with a
  fail-closed bind guard; probes leak no version/build/config.
- **XDP bounds** — every packet deref is `checked_add` bounds-checked; parse
  failure → `XDP_PASS`; per-CPU new-flow rate cap.

**Fixes applied this session:**

- **F-RES-1 (MEDIUM):** the H1 `header_read_timeout` was inert (no `Timer`
  wired), so a slowloris header trickle was bounded only by the 60 s
  connection `total`, not the intended 10 s header timeout. Now the H1
  server builder wires `.timer(TokioTimer::new()).header_read_timeout(...)`,
  so the 10 s header budget is active.
- **F-INFRA-01 (LOW):** the retry-secret **load** path now perm-checks an
  existing secret file (mirroring the TLS key's `assert_owner_only`),
  closing the asymmetry where a world-readable retry secret loaded silently
  (a Retry-token-forge / Mode-A-flood-bypass vector).
- **F-RES-2 (LOW):** the upstream H2-client builder now sets
  `max_header_list_size` explicitly (parity with the 64 KiB server policy,
  no longer relying on the implicit h2 16 KiB default).

(See "Residual risks" below for the documented accepted-risks and
hardening carry-forwards.)

## Residual risks (accepted / carry-forward)

These are the documented accepted-risks and hardening carry-forwards as of
the S38 audit. Operators should understand them before deploying.

- **No mTLS (server side) — intentional.** The gateway does not request a
  client certificate. Normal posture for an internet-facing reverse proxy.
  Upstream (backend) cert verification IS enforced. (F-INFRA-03)
- **TLS 1.2 allowed by default — downgrade-safe.** rustls's TLS 1.2 suites
  are ECDHE + AEAD only (no SSLv3/TLS1.0/1.1, no RC4/CBC-without-EtM). Set
  `tls13_only = true` for TLS-1.3-only environments. (F-INFRA-03)
- **Key material is not zeroized on free — no reachable leak.** TLS private
  keys live inside `Arc<rustls::ServerConfig>` and the retry secret in a
  `ring::hmac::Key`; neither is `Zeroize`d (rustls/ring do not zeroize).
  This is a defence-in-depth gap, not a reachable leak — there is no
  wire/admin path that reads freed heap, and the *redaction* discipline
  that prevents the reachable leak is proven (every secret-bearing struct
  has a hand-written non-printing `Debug`; no secret is ever logged).
  (F-INFRA-02)
- **QUIC has a global connection cap but no per-source-IP sub-cap.** Mode A
  and Mode B bound the connection table globally (default 100 000) and gate
  on a valid Retry token (so off-path spoofers cannot fill it), but a
  single real IP can monopolize the budget. Hardening carry-forward —
  add a per-IP QUIC cap + a config knob. (F-RES-3, LOW)
- **Slowloris/slow-POST Watchdog is observability-only.** The Watchdog
  sweeper logs + clears its table but does not itself close sockets; the
  *active* bounds are the timeout stack — H1 header-read timeout (now
  wired, F-RES-1), `idle_bounded_send` body-idle (30 s), connection `total`
  (60 s), H2 keepalive PING, and the QUIC 30 s idle timeout — each proven
  non-vacuous. (F-RES-5, LOW)
- **XDP data plane is single-kernel.** The shipped BPF ELF is validated
  against a specific kernel/verifier window (see `docs/guide/DEPLOYMENT.md`);
  multi-kernel CO-RE portability is carried as F-ESC-1. The loader relies
  on the operator/systemd unit for the bpffs pin-dir mode (F-INFRA-04).

## Responsible disclosure

Report security issues to **security@shieldblaze.com** (GPG key published on the project website). Include:

- Reproduction steps (ideally a minimal PoC).
- Affected version (`cargo pkgid` output or commit SHA).
- Proposed severity (CVSS v3.1 vector if known).

We acknowledge within 3 business days. We coordinate public disclosure after a fix is shipped; typical embargo is 30 days for high-severity issues, negotiated case-by-case.

## Secrets

`trufflehog git file://<repo> --only-verified` is a required pre-release gate per FINAL_REVIEW §9.7. Current scan: 0 verified, 0 unverified secrets. Do not commit credentials or tokens; there is no `.env` tracked in this repo and there never should be.
