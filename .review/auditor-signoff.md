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

---

## Delta 2026-04-24 — CONTINUE.md items 1–3

- Date: 2026-04-24
- Auditor: auditor-delta (fresh session, no coordination with reviewer-delta)
- HEAD: 8e9a37b7cb92b9f058e9be6e5baede813066964b
- Delta verdict: **PASS**
- Commits audited: `ba7bf635`, `6a72b64a`, `dc866ab8`, `eea6e80b`

Methodology: adversarial read-only walk of the four commits and the files
they touched. Round-1 findings verification first, then threats a–o from
Task #28 on the two new attack surfaces (WebSocket, gRPC). Independent
`rg` / `grep` / `awk` / trufflehog / halting-gate runs; reviewer-delta
signoff intentionally NOT read.

### Round-1 findings closure verification

| # | Finding (round-1) | Closed by | Evidence | Verdict |
|---|-------------------|-----------|----------|---------|
| 1 | QUIC CID-table unbounded growth | `ba7bf635` | `crates/lb-quic/src/router.rs:306` — `cap_entries = max_connections.saturating_mul(2); if connections.len() >= cap_entries { tracing::warn!(..); return Err("router at max_connections") }`. Cap enforced **before** `DashMap::insert`. `RouterParams.max_connections` default 100_000 wired at `listener.rs:226`. | **CLOSED** (with small nit — see residual D-1 below). |
| 2 | ZeroRttReplayGuard non-crypto hash | `ba7bf635` | `crates/lb-security/src/zero_rtt.rs:32-43` — `hmac::sign(&self.key, token)` on HMAC-SHA256. `fresh_secret()` at L54-69 uses `ring::rand::SystemRandom::new().fill(&mut secret)` — 32-byte per-instance key. `key: hmac::Key` at L85 is private, never exposed via public API. Attacker without the key cannot precompute collisions (would need to break HMAC-SHA256). SystemRandom-failure fallback mixes `SystemTime::now` nanos (strictly better than the prior hardcoded source-visible seeds). | **CLOSED**. |
| 3 | H2 detectors not wired into live hyper path | `6a72b64a` | `crates/lb-l7/src/h2_security.rs` introduces `H2SecurityThresholds` with 9 knobs mapped to `hyper::server::conn::http2::Builder`. `H2SecurityThresholds::apply()` at L111 chains: `max_pending_accept_reset_streams`, `max_local_error_reset_streams`, `max_concurrent_streams`, `max_header_list_size`, `max_send_buf_size`, `keep_alive_interval`, `keep_alive_timeout`, `initial_stream_window_size`, `initial_connection_window_size`. In `h2_proxy.rs:165-172`, `apply(&mut builder)` is invoked **before** `serve_connection(TokioIo::new(io), svc)` — so hyper enforces on the wire. `TokioTimer::new()` wired at L166 (required for `keep_alive_interval` to fire). 6/6 tests in `tests/h2_security_live.rs` spawn a live TLS listener and drive real h2 frames; 5 of 6 assert wire-level error codes (COMPRESSION_ERROR / REFUSED_STREAM / FRAME_SIZE_ERROR / PROTOCOL_ERROR / ENHANCE_YOUR_CALM / GOAWAY-remote); `ping_flood_goaway` accepts "burst completed, still alive" as pass (weaker assertion; hyper/h2 0.4 does cap, but the test does not strictly prove it). | **CLOSED** (with ping-flood assertion nit — see residual D-2). |

### Items 2 + 3 attack-surface verdicts

**WebSocket (Item 2 `dc866ab8`):**

| ID | Threat | Verdict | Evidence |
|----|--------|---------|----------|
| a | Unmasked client frame (RFC 6455 §5.2 → 1002) | **PASS** | tungstenite 0.24 `WebSocketConfig::default()` sets `accept_unmasked_frames: false` (registry `tungstenite-0.24.0/src/protocol/mod.rs:84`). Server-role stream rejects unmasked client frames with a Protocol-error close. Our `WsConfig::tungstenite_config()` at `ws_proxy.rs:86` only overrides `max_message_size` + `max_frame_size`, leaving the default guard intact. |
| b | Oversized message (>16 MiB → 1009) | **PASS** | `WsConfig::default()` at L72-77 sets `max_message_size: 16 * 1024 * 1024`. tungstenite enforces this on reassembly (verified at `tungstenite-0.24.0/src/protocol/mod.rs:841` test harness using the same knob). |
| c | Invalid close code (<1000 or 1015–2999) | **PASS** | tungstenite validates via `CloseCode::is_allowed()` at `coding.rs:192`: `Bad`, `Reserved`, `Status`, `Abnormal`, `Tls` disallowed. Reserved range 1016..=2999 enumerated at L253. Read path emits Protocol error when invalid close code arrives. |
| d | Continuation without initial frame (§5.4) | **PASS** | tungstenite reader state machine enforces the (single-opcode → Continuation*) grammar; stray Continuation → Protocol close. Inherited from tungstenite's read loop. |
| e | Mid-message control frame (§5.5) | **PASS** | Same — tungstenite accepts interleaved control frames but enforces the ban on fragmented control frames (FIN=1 required on Ping/Pong/Close). |
| f | Slow-read DoS on forwarder | **PASS-with-caveat** | `proxy_frames` at L208-228 uses `backend_tx.send(msg).await?` / `client_tx.send(msg).await?` — the `await` on a Sink applies producer backpressure directly. No userspace unbounded buffer. Kernel TCP buffer on the stalled side is bounded by default socket buffers. Idle timeout (default 60 s at L73) fires 1001 Going Away on stall. Caveat: buffered bytes inside tungstenite + OS TCP buffer can briefly pin (~MB/peer) until idle_timeout elapses — not a linear-memory-bomb but a slow-drain risk; tracked as residual D-3. |
| g | `Upgrade: websocket` with `Connection: close` | **PASS** | `is_h1_upgrade_request` at `ws_proxy.rs:109` requires `header_contains_token(CONNECTION, "upgrade")` **and** `Upgrade: websocket`. A request with `Connection: close` only fails the token check → returns false → falls through to regular H1 proxy path (will return 400/502 from hyper or upstream). No upgrade performed. |
| h | Endless Ping flood from client | **HOLD-review (MEDIUM)** | `proxy_frames` forwards every inbound `Ping` verbatim to the peer (`backend_tx.send(msg)` at L210). tungstenite auto-responds to Pings on the *receiving* side (so backend responds, NOT the gateway) and does NOT rate-limit incoming Ping frames at the gateway. A client sending 1M Pings/s will force the gateway to forward them to the backend until idle_timeout (default 60s) elapses between the forwarder's `select!` wakes — i.e. never elapses, because Ping traffic counts as activity. No HARD gateway-side cap exists. Exposure: constant-rate forward load on the backend proportional to client send rate; no gateway memory bomb, but a backend-DoS amplifier if backend can't absorb. Flagged as residual D-4. Not blocking — PROMPT.md §14 v1 explicitly defers WS-specific flood caps to a later pillar. |

**gRPC (Item 3 `eea6e80b`):**

| ID | Threat | Verdict | Evidence |
|----|--------|---------|----------|
| i | Oversized `grpc-message` value (backend → 4 MiB) | **PASS-with-caveat** | Upstream-client H2 handshake at `grpc_proxy.rs:174` uses `hyper::client::conn::http2::handshake` with default `max_header_list_size` (hyper default 2 MiB). The downstream-server H2 listener applies `H2SecurityThresholds.max_header_list_size = 64 KiB` from Item 1. A 4 MiB grpc-message in trailers is accepted on the upstream leg (brief memory pin) then rejected by the downstream H2 codec when forwarded back to the client. Not ideal (backend can briefly occupy gateway memory up to hyper client default) but bounded. Operator has no knob to tighten upstream client. Tracked as residual D-5. |
| j | Malformed `grpc-timeout` ("foo", "-5S", "5") | **PASS** | `GrpcDeadline::parse_timeout` at `deadline.rs:29`: empty → `InvalidTimeout`; `split_at(len-1)` → digits/unit; `digits.parse::<u64>()` fails on `-5` or non-ASCII-digit → `InvalidTimeout`; unknown unit char → `InvalidTimeout`. No panic, no overflow. In `grpc_proxy::clamp_grpc_timeout` at L260 a parse failure returns `None` (treated as "no deadline"). Spec-deviation nit (spec says malformed → error, not no-deadline) but not a security bug; tracked as residual D-6. |
| k | Timeout saturation (`3153600000000H`) | **PASS** | `parse_timeout` uses `saturating_mul` for H/M/S units (deadline.rs:49-51) → `u64::MAX` clamp. `clamp_grpc_timeout` then `parsed_ms.min(max_ms)` with `max_ms = u64::try_from(max.as_millis()).unwrap_or(u64::MAX)` → final deadline ≤ 300 s default. Eventual `Duration::from_millis(ms)` with ms ≤ 300_000 cannot overflow Duration (Duration holds u64 secs + u32 nanos). |
| l | HTTP→gRPC status translation (14-entry table) | **PASS** | `crates/lb-grpc/src/status.rs:102-114` `from_http_status` maps: 200→Ok, 400→Internal, 401→Unauthenticated, 403→PermissionDenied, 404→Unimplemented, 409→Aborted, 429→Unavailable, 499→Cancelled, 500→Internal, 501→Unimplemented, 502..=504→Unavailable, `_`→Unknown. Covers all 9 spec-mandated entries (gRPC status-codes.md) plus 5 extras. `finalize_upstream` at `grpc_proxy.rs:346` synthesises trailers for non-200 upstream via this map. Missing grpc-status on a 200 response from backend is forwarded as-is (client synthesises `Internal (2)` per spec — PASS). |
| m | `application/grpc` on listener with `grpc.enabled = false` | **PASS** | `h2_proxy.rs:219` gates on `g.config().enabled && is_grpc_request(&req)`. If disabled, falls through to regular H2 proxy path — the bytes are forwarded transparently to the backend as regular H2. Not a 415; per v1 design this is correct behavior (H2 proxy is content-type-agnostic). |
| n | `application/grpc-web` confusion | **PASS** | `is_grpc_request` at `grpc_proxy.rs:221`: after `split(';').next().trim()`, checks exact `== "application/grpc"` then `strip_prefix("application/grpc+")`. `application/grpc-web` (hyphen, not plus) fails both; returns false. Verified by walking the match for `application/grpc-web+proto` — strip_prefix returns None. |
| o | Synthesized `/grpc.health.v1.Health/Check` always SERVING | **HOLD-review (LOW, spec deviation)** | `handle_health_check` at `grpc_proxy.rs:280` returns `status = SERVING` unconditionally and does not parse the request body to extract the `service: string` field. Per grpc-proto `health.proto` §HealthCheckRequest, server MUST return `NOT_FOUND` (gRPC status 5) for unrecognized services or `UNKNOWN (2)` serving_status per the reference implementation. Our gateway is a proxy not a service registry, so "everything is SERVING from the gateway's POV" is defensible, but it's a spec deviation. Not a security issue. Flagged as residual D-7. |

### New HOLD items (delta)

**None blocking.** All two `HOLD-review` items (D-4 ping-flood forwarding and D-7 health-check-always-SERVING) are MEDIUM-LOW severity and documented in PROMPT.md §14 / health.proto deviations as deferred post-v1 scope. No severity reaches HIGH or CRITICAL.

### Residual risks after round-2

| ID | Severity | CVSSv3 | Description | Suggested fix |
|----|----------|--------|-------------|----------------|
| D-1 | LOW | 3.1 AV:N/AC:H/PR:L/UI:N/S:U/C:N/I:N/A:L | CID-cap drop path has no integration test. The cap logic (`router.rs:306`) is guarded by an `if` with `saturating_mul(2)`, but no test in `crates/lb-quic/tests/` drives >100_000 Initial packets to exercise the drop branch. If a refactor mutates the inequality (`>=` → `>`) or the multiplier, halting-gate wouldn't catch it. | Add a unit test for `spawn_new_connection` with a pre-populated `DashMap` at capacity, asserting the `Err("router at max_connections")` return. |
| D-2 | LOW | 3.1 | `ping_flood_goaway` in `tests/h2_security_live.rs:392` accepts "completed 1024 PINGs without crash" as pass — does NOT strictly assert hyper's PING cap fires on the wire. Hyper/h2 0.4 DOES cap; the test is just weaker than its siblings. | Drive enough PINGs (10_000+) with a lower ENHANCE_YOUR_CALM threshold and assert GOAWAY arrives. |
| D-3 | LOW | 3.1 AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L | WS slow-read stalls can pin kernel TCP buffer + tungstenite internal buffer for up to `idle_timeout` (60 s default) per peer. Per-peer memory footprint O(MB); at 10k concurrent stalled peers → 10 GB. | Add a `max_write_buffer_size` on `WebSocketConfig` mapped to tungstenite's field + aggressive half-close on slow peer. |
| D-4 | MEDIUM | 4.3 AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L | No gateway-side Ping rate-limit in `ws_proxy::proxy_frames`. Client Ping flood is forwarded to backend, amplifying backend load. | Track inbound Pings per 10 s window, emit 1008 Policy Violation close above N/s. |
| D-5 | LOW | 3.1 | Upstream H2 client in gRPC path has no `max_header_list_size` knob (hyper default 2 MiB). Malicious backend trailers up to 2 MiB transit gateway memory briefly before downstream rejection. | Apply `H2SecurityThresholds.max_header_list_size` to the upstream `http2::Builder` in `grpc_proxy.rs:174`. |
| D-6 | LOW | N/A | `clamp_grpc_timeout` silently drops malformed `grpc-timeout` → no-deadline. Spec says malformed → error. | Return `Some(0)` (immediate DEADLINE_EXCEEDED) or synthesise an INVALID_ARGUMENT trailer on parse failure. |
| D-7 | LOW | N/A | Synthesized `/grpc.health.v1.Health/Check` ignores the `service: string` field in request body; always returns `SERVING`. gRPC health spec expects `NOT_FOUND` / `SERVICE_UNKNOWN`. | Parse the 1–2 byte proto body, return `NOT_FOUND` for non-empty `service` (gateway doesn't know any service by name in v1). |

### Always-on checks

| Check | Result |
|-------|--------|
| Panic-free grep (AWK-skipped `#[cfg(test)]`) | **0 hits** outside tests |
| `cargo deny check advisories` | **ok** (5 `advisory-not-detected` warnings are allowlist entries for advisories that do not match any crate — harmless) |
| `trufflehog git --only-verified` | `chunks 13618, bytes 25217713, verified_secrets 0, unverified_secrets 0` |
| `unsafe` blocks count | **20** (16 blocks + 4 `unsafe fn`) — **unchanged from round-1**. No new `unsafe` introduced in `ba7bf635` / `6a72b64a` / `dc866ab8` / `eea6e80b`. |
| `bash scripts/halting-gate.sh` | **GREEN** (141/141 artifacts, 59/59 tests) |

### Signature

auditor (delta 2026-04-24) — **PASS** with 7 residual risks (D-1..D-7),
0 HOLD-blocking, 0 HIGH/CRITICAL.

