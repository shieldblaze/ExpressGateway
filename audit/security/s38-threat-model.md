# ExpressGateway — Session 38 Threat Model (internet-facing, all-protocol)

**Date:** 2026-06-08 · **Base:** main @ b8a99078 (branched `feature/security-audit-s38`)
**Deployment profile under audit:** internet-facing AND internal; ALL protocols served
(H1/H2/H3/QUIC/gRPC/WS), both QUIC modes (A passthrough, B terminate), the full 9-cell
front×back matrix, the operational layer (config/SIGHUP reload, admin API, L4/XDP).

This document defines WHAT gets attacked. Every surface below is a hypothesis of where an
attacker-controlled byte reaches our code. Per R4, a scope is only "clean" once the defense
is identified AND tested — this model lists the attacks; the findings doc records the verdicts.

---

## 1. Trust boundaries & attacker model

| Boundary | Attacker position | What they control |
|---|---|---|
| **Data plane (untrusted)** | Any internet client | Raw TCP/UDP bytes to listener ports: TLS records, QUIC datagrams, H1/H2/H3 frames, WS frames, request lines/headers/bodies/trailers, pseudo-headers |
| **Backend (semi-trusted)** | A compromised/malicious upstream | Response status/headers/bodies/trailers, H2/H3 frames back, chunked/CL framing, grpc-status trailers |
| **L4 wire (untrusted)** | Any host that can send packets to the NIC | Ethernet/VLAN/IPv4/IPv6/TCP/UDP header fields parsed in XDP |
| **Operator (trusted-but-fallible)** | Local operator / orchestration | The config file, SIGHUP, file perms on cert/key/retry-secret, admin token |
| **Admin plane (untrusted if exposed)** | Anyone who can reach `metrics_bind` | HTTP requests to `/metrics` `/livez` `/readyz` `/startupz` `/healthz`, bearer token guesses |

**Out of scope (this session):** physical/host compromise; the operator deliberately writing a
malicious config to run code they already could run; supply-chain of vetted deps (tracked
separately as CF items); kernel/XDP multi-kernel portability (F-ESC-1, self-hosted).
**In scope:** can a *hostile config* (typo/attacker-influenced values) or a *reload race* cause
harm; can a wire attacker reach memory-unsafety/panic/unbounded-alloc/smuggling/auth-bypass.

---

## 2. Attack surface inventory (wire-reachable parsers — highest value)

Hand-rolled parsers are the crown jewels of the audit (delegated parsers — rustls, h2-crate
delegation, tungstenite, quiche — are lower-yield but still fuzzed at the boundary).

| # | Surface | Crate / entry | Hand-rolled? | Fuzzed today? |
|---|---|---|---|---|
| P1 | H1 request/status line, headers, trailers | `lb-h1/src/parse.rs` (`parse_request_line:73`, `parse_status_line:100`, `parse_headers_with_limit:148`, `parse_trailers:230`) | **YES** | partial (`h1_parser` feeds headers only — **request-line gap**) |
| P2 | H1 chunked decode + chunk-size hex | `lb-h1/src/chunked.rs` (`ChunkedDecoder::feed:56`, `parse_chunk_size_hex:314`) | **YES** | indirect only — **no focused chunk fuzz** |
| P3 | H2 frame decode + padding/priority strip | `lb-h2/src/frame.rs` (`decode_frame:419`, `parse_frame_header:181`, `strip_padding`) | **YES** | yes (`h2_frame`) |
| P4 | H2 HPACK decompression + bomb detector | `lb-h2/src/hpack.rs`, `lb-h2/src/security.rs` (`HpackBombDetector`) | **YES** | **via frame only — HPACK decode not a direct target** |
| P5 | **QUIC public header (Mode A passthrough)** | `lb-quic/src/public_header.rs` (`parse_public_header:165`, `parse_long:183`, `parse_short`, `decode_varint:134`) | **YES** | **NO — `quic_initial` fuzzes quiche, NOT this parser. GAP.** |
| P6 | H3 pseudo-header / extended-CONNECT validator | `lb-quic/src/h3_bridge.rs` (`validate_request_pseudo_headers:489`) | **YES** | no (pure validator — fuzz the header-list → verdict) |
| P7 | H3 frames / QPACK / varint (production) | `quiche::h3` | NO (delegated) | quiche-owned |
| P7t | H3 frames / QPACK / varint (test codec) | `lb-h3-testcodec` (`frame.rs:84`, `varint.rs:27`, `qpack.rs`) | YES (test-only) | frame only — QPACK gap (test-only, lower priority) |
| P8 | QUIC Initial / transport params / TLS-in-Initial | `quiche` + BoringSSL | NO (delegated) | yes (`quic_initial`) |
| P9 | WS frame parse / mask / opcode (all of H1/H2/H3 WS) | `tokio_tungstenite::tungstenite` (`lb-l7/src/ws_proxy.rs` relay) | NO (delegated) | tungstenite-owned; **our caps fuzzable** |
| P10 | TLS ClientHello / SNI | `rustls` + BoringSSL | NO (delegated) | yes (`tls_client_hello`) |
| P11 | Retry token mint/verify | `lb-security` (`RetryTokenSigner`, ring AEAD) | partial | **not fuzzed — verify forge-resistance** |
| P12 | SNI↔authority comparison | `lb-l7/src/sni_authority.rs:95` (`check_sni_authority`) | YES (compare, not parse) | no |

**Fuzz gaps to close in Phase 0 (new targets):**
- **P5 — `quic_public_header` (NEW, HIGH priority):** our hand-rolled parser runs on *every*
  Mode A datagram, internet-facing, no decryption. Highest-yield gap.
- **P2 — `h1_chunked` (NEW):** focused chunk-size/extension/trailer fuzz (CVE-2013-2028 class).
- **P1 — extend `h1_parser`** to also drive `parse_request_line` / full request parse.
- **P4 — `h2_hpack` (NEW):** drive HPACK decode + bomb detector directly.
- **P6 — `h3_pseudo_header` (NEW, cheap):** structured fuzz of the validator (smuggling guard).

---

## 3. Per-surface threats (what each auditor hunts)

### parser-auditor (P1–P12 byte-level)
- **Integer overflow / underflow** in length fields: H2 3-byte frame length, chunk-size hex
  (`checked_shl` claimed — verify), QUIC varint (1/2/4/8-byte), QPACK/HPACK integer prefix
  continuation (RFC 7541 §5.1 multi-byte — unbounded-shift / overflow class).
- **OOB read / slice panic** on truncated input: `&buf[..n]` with attacker n, padding strip
  (`pad_len > payload`), priority block (5 bytes claimed present), DCID/SCID length (≤20),
  varint that claims more bytes than present.
- **Panic-on-malformed = DoS:** any `unwrap`/`expect`/indexing/`unreachable!` reachable from
  wire bytes. `#![deny(indexing_slicing)]` is set in lb-quic (incl. tests) — verify siblings.
- **Header/pseudo-header injection:** CR/LF/NUL in header names/values forwarded to backend;
  `tchar` validation completeness (RFC 9110 §5.6.2); non-ASCII / control bytes.
- **HPACK/QPACK bomb / amplification:** decompressed size vs detector bound; dynamic-table
  eviction correctness; reference to evicted entry.
- **State-machine desync:** chunked decoder state transitions on adversarial interleaving.

### protocol-auditor (smuggling / desync / cross-stream, 9 cells + upgrades)
- **CL/TE smuggling (H1→H1):** duplicate CL, CL+TE conflict, obs-fold, TE: chunked variants,
  whitespace/case tricks — re-attack `SmuggleDetector::check_all_mode` (h1_proxy:1075) with
  fresh eyes (the S22 class).
- **H2→H1 downgrade smuggling:** forbidden hop-by-hop / CL / TE leaking through the H2→H1
  bridge (`check_h2_downgrade`, h2_to_h1:72); CRLF in H2 header values materialized into an H1
  request line; `:authority` vs Host disagreement.
- **H3→H1/H2 downgrade:** pseudo-header → request-line/Host translation; same CRLF / CL/TE class.
- **Response splitting / trailer injection:** backend-controlled headers/trailers forwarded to
  client; the gRPC `grpc-status` trailer path; undeclared-trailer guard (h3_bridge:2273); the
  S29 trailer-drop fix region.
- **Upgrade ordering (F-S27-1 class):** WS H1 (`handle_ws_upgrade`, dial-before-101), WS H2
  (`handle_ws_extended_connect`, dial-before-200), WS H3 (actor waits `Ready` before 200);
  CONNECT / extended-CONNECT pseudo-header rules; 101/200 emitted on upstream failure?
- **Cross-stream / cross-connection bleed:** H2/H3 multiplexing — can stream A's body/headers
  reach stream B? The reload swap (ArcSwap) under concurrency — can a reload cross connections?

### resource-auditor (DoS / exhaustion / R8 under hostile input)
- **Stream-per-connection (RE-ATTACK S36 fix):** prove `max_requests_per_h3_connection` (default
  1000) + GOAWAY actually caps adversarially — a client that opens streams as fast as possible,
  ignores GOAWAY, resets streams; verify the StreamMap-collected leak (CF-S32) stays bounded.
- **MAX_RELAY_STREAMS (256, raw_proxy:671):** flood Mode B relay streams; over-cap refuse path.
- **Slowloris:** trickle request head (header timeout 10s), trickle body (body idle 30s), trickle
  during upgrade dial; the Watchdog sweeper (h1_proxy:1085) progress accounting — can it be
  fooled (1 byte / interval)?
- **R8 response bound:** backend streams an unbounded/huge response — verify no whole-response
  buffering (the gauge); 64 MiB `MAX_RESPONSE_BODY_BYTES` cap; chunk-channel depth backpressure.
- **fd / connection / memory exhaustion:** `max_inflight_connections`, `per_ip_connection_cap`,
  TCP pool caps (per_peer 8 / total 256), the idle reaper (acquire-driven — is there a path with
  no acquire that leaks?), Mode A flow/fd retention (the S21 F-S20-2 region).
- **Datagram queue (BoundedDgramQueue, drop-newest, 1024):** datagram flood.
- **Amplification:** small request → large backend fetch held in memory; H2/H3 settings flood;
  CVE-2023-44487 (rapid reset) — the soak proved 227M resets bounded; re-attack adversarially.
- **Decompression:** if any response/request decompression exists (gzip/br) — bomb. (Verify
  whether the LB decompresses at all or passes through.)

### infra-auditor (TLS / admin / config / reload / XDP / secrets)
- **Config validation bypass:** with `deny_unknown_fields` in, can a *semantically* hostile
  config pass validation and cause harm (e.g. a cap of 0 that disables a defense silently —
  the `max_keepalive_requests` fat-finger class; `max_requests_per_h3_connection=0` re-opens
  the leak; `tls_verify_peer=false`)? Enumerate every "0 = disable" knob.
- **Reload race:** SIGHUP during traffic — ArcSwap store vs connection-task load_full; can a
  half-applied reload cross config between connections, tear an H1s cert/ALPN leg, or leave a
  listener in a bad state? Restart-required-but-applied-anyway (honesty-contract violation)?
- **Admin API auth:** `/metrics` token (SHA-256, constant-time `subtle`) — bypass via missing
  token + loopback assumption; can the data-plane reach the admin bind; timing on the compare;
  probes leak info (`/readyz` during drain)? The `allow_non_loopback` foot-gun guard.
- **TLS/cert:** key file perm enforcement (0600, lax vs strict); no mTLS (server) — is that a
  gap for the threat profile; SNI↔authority 421 logic bypass; cert chain validation to upstream
  (`tls_verify_peer`, `tls_ca_path`); downgrade to TLS1.2; retry-secret auto-gen perms.
- **Secrets:** private keys not zeroized (rustls holds in Arc) — info-leak reachability; token
  hash in config; nothing logged; metrics labels don't carry secrets; error messages.
- **XDP:** packet-field bounds (IHL, ext headers, ports) — verifier-checked; eBPF load caps
  (CAP_BPF); the new-flow rate cap (SYN-flood); map key safety; can wire input wedge a map.

---

## 4. Bug-class checklist (applied to every surface)

1. Memory unsafety reachable from wire (slice OOB, integer overflow→alloc, use-after-free in
   pooled buffers/connections) → **CRITICAL/HIGH**.
2. Panic-on-malformed (unwrap/expect/index/unreachable/arithmetic overflow in debug) → **DoS / HIGH**.
3. Request smuggling / desync / response splitting → **HIGH**.
4. Auth bypass (admin token, SNI↔authority, retry-token forge, 0-RTT replay) → **HIGH**.
5. Unbounded allocation from wire (R8 bypass) → **HIGH**.
6. Bounded DoS (a flood that degrades but doesn't crash) → **MEDIUM**.
7. Info leak (secrets in logs/errors/metrics, timing) → **MEDIUM**.
8. Hardening gap (missing default, weak validation) → **LOW/MEDIUM**.

---

## 5. Fuzz campaign plan

| Target | Status | Toolchain | Runtime cap | Watchdog |
|---|---|---|---|---|
| `h1_parser` (extend: +request-line) | exists, extend | nightly-2026-01-15 | per-target time-boxed | harness-tracked bg + reaped |
| `h2_frame` | exists | " | " | " |
| `h3_frame` | exists | " | " | " |
| `quic_initial` (quiche boundary) | exists | " | " | " |
| `tls_client_hello` | exists | " | " | " |
| **`quic_public_header` (NEW)** | **add** | " | " | " |
| **`h1_chunked` (NEW)** | **add** | " | " | " |
| **`h2_hpack` (NEW)** | **add** | " | " | " |
| **`h3_pseudo_header` (NEW)** | **add** | " | " | " |

Rules: cargo-fuzz on nightly-2026-01-15; fuzzers run HOT → staggered from the ×3 gate (OOM
hazard, R9); each campaign time-boxed and harness-tracked; crash inputs → committed regression
corpora + a `proptest`/unit regression test; report iters actually reached (R15 — a killed run ≠
"no crashes"). Corpora managed for disk (CF-DISK-1).

---

## 6. Defenses claimed (each must be PROVEN by an auditor, R4 — not assumed)

Recon surfaced these existing defenses. Listed so auditors prove they work adversarially (a
clean scope needs the defense *tested*, not assumed):
`tchar` header validation; chunk-size `checked_shl` + 16-digit cap; H2 padding/priority bounds;
HPACK/QPACK bomb detectors; QUIC fixed-bit + CID-len≤20 checks; pseudo-header validator (h3spec
#12–15 + RFC 8441/9220); SmuggleDetector (H1 + H2-downgrade); StrippedRequest type-enforced
hop-by-hop strip; F-S27-1 dial-before-upgrade; R8 bounded channels + 64 MiB caps;
MAX_RELAY_STREAMS / max_requests_per_h3_connection / BoundedDgramQueue; slowloris Watchdog +
HttpTimeouts; idle reaper; admin token (constant-time) + loopback default; key-file 0600;
retry-token AEAD + 0-RTT replay guard; SNI↔authority 421; XDP bounds-checked parse + new-flow cap;
config `deny_unknown_fields` + semantic validator + reload honesty-contract.

**The audit's job: try to break each of these. A defense that holds is a proven-clean scope
(documented with the test that proves it). A defense that breaks is a finding.**
