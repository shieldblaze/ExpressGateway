# Auditor sign-off

- Date: 2026-04-23
- Auditor: auditor (fresh session, no prior context, no coordination with reviewer)
- HEAD: 1418d4b7aa83177434b46bfab1eaafbc0c3a7cdc
- Verdict: PASS

## Methodology

Adversarial read-only walk of the tree at HEAD. Threat model per `SECURITY.md`:
untrusted clients, semi-trusted backends, trusted kernel, trusted operator.
Assume an attacker has code execution against the gateway and wants to
DoS / exfiltrate / pivot. Checklist items 1–12 from Task #23 walked in order.
Independent invocation of `scripts/halting-gate.sh`, independent
`rg`/`awk` greps, independent `trufflehog` scan. No reviewer coordination;
`.review/reviewer-signoff.md` was intentionally not read.

Key skepticism: a test that compiles and passes is necessary but not
sufficient. For every detector type, I checked whether the live HTTP/TLS/
QUIC pipeline actually *invokes* it on a real connection, or whether the
type lives in isolation. This is explicitly called out as a residual risk
in `SECURITY.md` ("Detectors land without wiring... Pillar 3b") — my role
is to verify the disclosure matches reality.

## Attack-surface verdict table

| # | Attack | Layer | Verdict | Evidence |
|---|--------|-------|---------|----------|
| 1 | H1 smuggling (CL.TE, TE.CL, TE.TE, dup CL, negative CL, chunk overflow) | L7 H1 | PASS-with-caveat | `crates/lb-security/src/smuggle.rs::SmuggleDetector` rejects each pattern per RFC 9112 §6.1; tests `tests/security_smuggling_cl_te.rs`, `security_smuggling_te_cl.rs`, `security_smuggling_h2_downgrade.rs` green. `lb-h1::parse_headers_with_limit` enforces `MAX_HEADER_BYTES = 65_536` with `HeadersTooLarge` on overflow. **Caveat**: the production H1 path (`crates/lb-l7/src/h1_proxy.rs`) uses hyper 1.x for parsing; hyper itself rejects CL+TE ambiguity. Neither `SmuggleDetector` nor `lb-h1::parse` is invoked by any downstream consumer — detectors exist but are not wired. Acceptable: hyper covers the primary rejection path. |
| 2 | H2 floods (CONTINUATION CVE-2024-27316, Rapid Reset CVE-2023-44487, SETTINGS, PING, zero-window, HPACK bomb) | L7 H2 | HOLD (documented) | Detectors exist: `crates/lb-h2/src/security.rs::{RapidResetDetector, ContinuationFloodDetector, HpackBombDetector, SettingsFloodDetector, PingFloodDetector, ZeroWindowStallDetector}`. Unit tests all green. Wiring verdict: `rg 'RapidResetDetector\|ContinuationFloodDetector\|HpackBombDetector\|SettingsFloodDetector\|PingFloodDetector\|ZeroWindowStallDetector' crates/lb-l7 crates/lb crates/lb-io` returns **zero** hits. Production H2 (`crates/lb-l7/src/h2_proxy.rs`) uses hyper/h2 codec defaults only. SECURITY.md explicitly discloses this ("Detectors land without wiring... Pillar 3b"). Flagged as documented residual risk, not a regression against the stated posture. |
| 3 | H3 QPACK bomb, stateless reset, 0-RTT replay, CID exhaustion | L5 QUIC | MIXED | QPACK bomb detector (`crates/lb-h3/src/security.rs::QpackBombDetector`) unit-tested (`tests/security_qpack_bomb.rs`) but not wired into listener. 0-RTT replay IS wired: `crates/lb-quic/src/router.rs:194` calls `replay_guard.lock().check_0rtt_token(&replay_key)` on every Initial with retry token. Retry tokens use HMAC-SHA256 via `ring::hmac` with OS-RNG secret (`crates/lb-security/src/retry.rs`). CID exhaustion: see HOLD-1 below. |
| 4 | TLS downgrade / reneg storm / unknown SNI / mTLS no-cert | TLS | PASS-with-gap | `crates/lb-security/src/ticket.rs::build_server_config` uses `rustls::ServerConfig::builder_with_provider(ring).with_safe_default_protocol_versions().with_no_client_auth().with_single_cert(...)`. rustls 0.23 supports TLS 1.2+1.3 only, no insecure renegotiation, no fallback-SCSV downgrade path. SNI: single-cert mode presents the same cert regardless of SNI — no weaker-cert fallback. **Gap**: mTLS is NOT implemented (`with_no_client_auth` is hardcoded); so the "mTLS REQUIRED with no cert" attack is trivially vacuous because mTLS mode doesn't exist. Not a regression; a feature absence, documented nowhere as a gap. |
| 5 | Slowloris / slow-POST | L7 H1 | HOLD (documented) | `crates/lb-security/src/slowloris.rs::SlowlorisDetector` + `slow_post.rs::SlowPostDetector` exist; unit tests green (`tests/security_slowloris.rs`, `security_slow_post.rs`). Wiring: `rg 'SlowlorisDetector\|SlowPostDetector' crates/lb-l7 crates/lb crates/lb-io` returns zero hits. Hyper 1.x's `http1::Builder` in `h1_proxy.rs:202` has a `header_read_timeout` knob but is not set — only `total_timeout` wraps the entire connection via `tokio::time::timeout(total, conn)` at `h1_proxy.rs:205`. A slowloris client can occupy a worker for up to 60s (default `HttpTimeouts::total`) before the total timeout fires. Acceptable per documented posture. Also: SECURITY.md rows 12/13 reference `SlowlorisGuard` / `SlowPostGuard` — the actual types are `SlowlorisDetector` / `SlowPostDetector`. Doc drift, minor. |
| 6 | Pool stale reuse / probe bypass / bounds bypass | Pool | PASS | `crates/lb-io/src/pool.rs::validate_and_upgrade` (line 193) checks `max_age` + `idle_timeout` + `probe_alive` non-blocking read-zero (EC-01) on every acquire. Per-peer LRU + `per_peer_max` + `total_max` enforced on `PooledTcp::drop`. Tests `probe_discards_peer_closed_connection`, `per_peer_max_enforced`, `total_max_enforced`, `size_invariant_holds_under_random_ops` green. |
| 7 | DNS NXDOMAIN poisoning / resolver race | DNS | PASS | `crates/lb-io/src/dns.rs` caches NXDOMAIN with `DEFAULT_NEGATIVE_TTL_SECS = 5` via `ResolverConfig::negative_ttl`; positive TTL capped at 300s. `negative_entry_caches_nxdomain` test green. |
| 8 | Panic-as-DoS | Whole codebase | PASS | Independent re-run of halting-gate check 3 (AWK brace-balanced skip of `#[cfg(test)]` blocks): **zero** hits in `crates/*/src/**/*.rs` outside test modules. Crate-root `#![deny(clippy::unwrap_used, expect_used, panic, indexing_slicing, todo, unimplemented, unreachable)]` is the bedrock. `bash scripts/halting-gate.sh` GREEN (141/141 artifacts, 59/59 tests). |
| 9 | Upstream failures (backend RST mid-body, cert expiry mid-traffic) | L7 | PARTIAL | `proxy_request` (`crates/lb-l7/src/h1_proxy.rs:270`) wraps `send_request` in `tokio::time::timeout(self.timeouts.body, ...)`; RST → `ProxyErr::Upstream(String)` → 502. Backend cert-expiry-mid-traffic is NOT tested and there is no cert-rotation / re-validation hook on upstream TLS connections (backend-side TLS is TLS_H2H1 bridging only; live-traffic cert expiry would surface as a handshake failure on next pooled acquire). Acceptable per SECURITY.md's "semi-trusted backend" posture — backends can misbehave, the proxy surfaces 502. |
| 10 | Admin listener (/metrics loopback only by default) | Admin | PASS-with-caveat | `crates/lb-observability/src/admin_http.rs::serve` accepts any `SocketAddr`. **Default: off** — `crates/lb/src/main.rs:648` only starts the listener if `config.observability.metrics_bind` is `Some`. Config validator (`lb-config/src/lib.rs:308`) parses the address but does NOT enforce loopback. An operator configuring `metrics_bind = "0.0.0.0:9090"` exposes `/metrics` + `/healthz` (no auth, no TLS) publicly. Doc comment at `admin_http.rs:3` says "Intended for loopback scrapes. Operator is expected to bind it to 127.0.0.1." This is operator-trust by documentation; acceptable for a trusted-operator threat model but worth hardening later (reject non-loopback in `validate_observability` unless an explicit override). |
| 11 | Secrets in tree | Whole repo | PASS | `trufflehog git file:///home/ubuntu/Programming/ExpressGateway --only-verified` — `chunks 13440, bytes 25009225, verified_secrets 0, unverified_secrets 0`. |
| 12 | Unsafe blocks | Whole codebase | PASS | `rg -nP '^\s*unsafe\s*\{\|unsafe fn' crates/` returns 20 sites (16 blocks + 4 `unsafe fn`) across `crates/lb-io/src/ring.rs` (8) and `crates/lb-l4-xdp/ebpf/src/main.rs` (12). Independent per-site SAFETY-comment audit: every site has a `// SAFETY:` comment within ≤10 lines of context; `ptr_at`/`ptr_at_mut` at `lb-l4-xdp/ebpf/src/main.rs:234,246` carry the SAFETY justification inside the function body. No missing comments. |

## HOLD items

None require blocking release. Three residual risks below are documented
in `SECURITY.md` and `docs/gap-analysis.md` (per §9.11 expectation), so
the verdict is **PASS** with these called out for operator awareness.

### Residual risk 1 — QUIC CID-table unbounded growth (MEDIUM, 5.3 CVSSv3 AV:N/AC:L/PR:L/UI:N/S:U/C:N/I:N/A:L)

`crates/lb-quic/src/router.rs:111` holds `Arc<DashMap<Vec<u8>, Sender>>`
with no capacity cap. A fleet of legitimate clients (or compromised
botnet endpoints — retry tokens defeat spoofing but not real packet
sources) can spawn many actors and grow the table without bound. Retry
token issuance is address-validating but not rate-limited at the router
level.

Reproduction sketch: N > 100k distinct source addresses complete the
two-trip retry handshake concurrently; memory grows ~O(N × actor-state).

Suggested fix (non-blocking): add a `max_connections` cap and reject
Initial above it, or LRU-evict the oldest idle actor when the table
exceeds a cap.

### Residual risk 2 — ZeroRttReplayGuard uses non-crypto hash with public seeds (MEDIUM, 4.3 CVSSv3 AV:N/AC:H/PR:N/UI:N/S:U/C:N/I:L/A:L)

`crates/lb-security/src/zero_rtt.rs::hash_token` is a 32-lane
multiply-shift with **hardcoded public seeds** (lines 21–54), reduced to
32 output bytes by taking only the low byte of each lane. Because the
seeds are compile-time constants in a public repo, an attacker can
precompute token collisions offline. Two attack vectors:

1. False negative (replay slips through): craft a token whose digest
   collides with a prior token that has already been evicted from the
   ring — the replay is not detected.
2. False positive (legitimate-token DoS): craft a token that collides
   with a victim's future token, causing the victim's 0-RTT to be
   rejected as a replay.

The inline comment acknowledges "not for security-sensitive hashing,"
but this *is* a security-sensitive path. Suggested fix: use a keyed
`ring::hmac` or BLAKE3 keyed hash with a server-side random secret
rotated alongside the ticket key.

### Residual risk 3 — H1/H2/H3 flood & smuggle detectors not wired (MEDIUM–LOW, already disclosed)

Per SECURITY.md's "Residual risks" section, the Rapid Reset,
CONTINUATION flood, HPACK/QPACK bomb, SETTINGS/PING/zero-window, and
Slowloris/Slow-POST detectors are tested-but-unwired types. Production
HTTP paths (`lb-l7/src/{h1,h2}_proxy.rs`) rely on hyper 1.x codec
defaults. Hyper's defaults are reasonable (bounded CONTINUATION frames,
stream count caps, HPACK dynamic-table caps), so the exposure is
less-severe than it appears, but a deployed listener does NOT get the
project-specific detectors' stricter thresholds. This is transparently
disclosed in SECURITY.md and slated for Pillar 3b follow-up.

## Independent checks performed

Commands actually run by the auditor (not copied from halting-gate
output):

```
git rev-parse HEAD                           # 1418d4b7aa83177434b46bfab1eaafbc0c3a7cdc
git log --oneline -5
ls crates/                                   # enumerate 18 crates
wc -l crates/lb-h1/src/parse.rs              # 335
grep -rn "SmuggleDetector" crates/           # 0 hits in lb-l7/lb/lb-h1/lb-h2/lb-io
grep -rn "RapidResetDetector" crates/lb-l7 crates/lb crates/lb-io   # 0 hits
grep -rn "SlowlorisDetector|SlowPostDetector" crates/lb-l7 crates/lb crates/lb-io  # 0 hits
grep -rn "lb_h1|lb-h1" crates/*/Cargo.toml   # only workspace declaration
grep -rn "lb_h2|lb-h2" crates/*/src/         # only doc reference in lb-io
rg -n '^\s*unsafe\s*\{|unsafe fn' crates/    # 20 sites; all have SAFETY comments
find crates/ -name '*.rs' -not -path '*/tests/*' | xargs awk '...'  # 0 panic hits
/tmp/bin/trufflehog git file:///home/ubuntu/Programming/ExpressGateway --only-verified
                                             # verified=0, unverified=0
bash scripts/halting-gate.sh                 # GREEN, 141/141, 59/59
```

## Signature

auditor — PASS
