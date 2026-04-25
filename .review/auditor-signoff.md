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

---

## Round-3 Delta 2026-04-25 — closure-commit audit

- Date: 2026-04-25
- Auditor: auditor-delta-3 (fresh session, no coordination with reviewer-delta-3)
- HEAD: `da1ef384176e3b7d77d2128740617b387cbcb3ec`
- Delta verdict: **PASS**
- Commits audited: `2fac6bec` (WS-001), `7954b5ba` (PROTO-001), `22a4f5a5`
  (WS-002 + GRPC-001/002/003), `1fdfeb10` (TEST-001), `7c1d9f99`
  (TEST-002 reclassify), `da1ef384` (FLAKE-002).

Methodology: independent walk of each closure commit's diff and the
files it touched; round-2 D-1..D-7 closure verification first, then
PROTO-001 a–e attack-surface walk, finally always-on (panic-free,
unsafe count, cargo deny, trufflehog, halting-gate, workspace tests).
Reviewer-delta-3 sign-off intentionally NOT read.

### Round-2 findings closure verification

| ID (round-2) | Tracking ID | Closed by | Evidence | Verdict |
|---|---|---|---|---|
| D-4 | **WS-001** | `2fac6bec` | `crates/lb-l7/src/ws_proxy.rs:208-269` — `client_ping_log: VecDeque<Instant>` per connection; on `Message::Ping` the loop pops expired entries and emits `Close 1008` Policy when `> ping_max`. Default 50 / 10 s mirrors `lb_h2::PingFloodDetector`. Test `ws_ping_flood_closes_with_1008` (tests/ws_proxy_e2e.rs) drives 10 Pings vs configured 5, observes Close 1008 within bound. | **CLOSED**. |
| D-3 | **WS-002** | `22a4f5a5` | `crates/lb-l7/src/ws_proxy.rs` — added `read_frame_timeout` (default 30 s); per-direction watchdog wrapping `select!` with `tokio::time::timeout`; on stuck read emits `Close 1008 "ws read frame timeout"` and shuts upstream. Test `ws_read_frame_timeout_closes_with_1008` exercises the path. Bounds per-peer kernel-TCP/tungstenite-buffer dwell. | **CLOSED**. |
| D-5 | **GRPC-001** | `22a4f5a5` | `crates/lb-l7/src/grpc_proxy.rs:236-237` — upstream H2 client's `hyper::client::conn::http2::Builder::max_header_list_size(self.max_header_list_size)` set BEFORE handshake; `DEFAULT_UPSTREAM_MAX_HEADER_LIST_SIZE = 64 * 1024` mirrors listener `H2SecurityThresholds`. Configurable via `with_max_header_list_size`. Test `grpc_upstream_oversize_trailer_rejected_by_gateway` drives the wire path. | **CLOSED**. |
| D-6 | **GRPC-002** | `22a4f5a5` | `grpc_proxy.rs:312-360` — new `ParsedTimeout::{Absent, Ok, Malformed}` enum; `parse_and_clamp_grpc_timeout` distinguishes the three. On `Malformed(raw)` the forward path returns `grpc_error_response(GrpcStatus::InvalidArgument, …)` WITHOUT dialing the backend. Test `grpc_malformed_timeout_returns_invalid_argument` exercises. Spec-compliant per gRPC `Timeout` ABNF. | **CLOSED**. |
| D-7 | **GRPC-003** | `22a4f5a5` | `grpc_proxy.rs:445-528` — `decode_health_check_service` hand-decodes the body's gRPC frame header + tag-1 length-delimited string (no prost). Bounds-checked at every step (`checked_add`, `i..end > payload.len()` returns empty service, `from_utf8` on the byte slice). Empty service → `health_check_serving_response()`; non-empty → `grpc-status: 5 NOT_FOUND` per grpc-health spec. Tests `grpc_health_check_overall_serving` + `grpc_health_check_unknown_service_not_found`. | **CLOSED**. |
| D-1 | **TEST-001** | `1fdfeb10` | `crates/lb-quic/src/router.rs:409-523` — `router_drops_initial_when_cap_reached` prefills the `DashMap` with `2 * max_connections = 4` placeholder senders (`max_connections=2`), mints a real Initial via `quiche::connect`, calls `spawn_new_connection` directly, asserts `Err("router at max_connections")` and unchanged `connections.len() == 4`. Stub `config_factory` returns `TlsFail` so any factory invocation would fail loudly with a different error — proves the cap-check returns BEFORE factory call. | **CLOSED** (well-targeted: invariant + early-return-before-side-effect). |
| D-2 | **TEST-002** | `7c1d9f99` (reclassified) | `tests/h2_security_live.rs:392-427` exercises the path via hand-rolled raw-frame writes and asserts `sent > 0` + no crash. Reclassification rationale (h2 0.4 `SendRequest` does not surface GOAWAY error_code post-teardown) is **partly inaccurate**: `h2::Error::reason() -> Option<Reason>` (h2-0.4.13 src/error.rs:52) DOES return `Some(Reason)` for `Kind::GoAway`, plus `is_go_away()` / `is_remote()` exist (lines 89-106). HOWEVER the test in question doesn't use `SendRequest` at all — it writes raw frames — so the rationale chosen for the gap-analysis entry doesn't match the test. The path IS exercised end-to-end (server doesn't crash, sent>0); a strict GOAWAY-reason assertion would require a frame reader on the test's TLS socket (parse for type=0x07 GOAWAY, last_stream_id, error_code=ENHANCE_YOUR_CALM=0x0b). Author chose to defer this incremental coverage. | **CLOSED** as deferred-with-rationale, with an honesty nit (D-3-1) below. |
| — | **FLAKE-002** | `da1ef384` | `crates/lb-observability/src/lib.rs` — `MetricsRegistry::counter` (and `counter_vec` / `histogram` / `histogram_vec` / `gauge`) now go through `DashMap::entry(name).or_insert_with`-style branch on cache miss. The `Entry::Vacant` arm holds the shard write-lock so `prometheus::Registry::register` runs exactly once per name; concurrent first-callers observe `Entry::Occupied` and clone. Eliminates the `Err(AlreadyReg)` race that dropped increments. Author claims pre-fix 49/50 PASS → post-fix 50/50. | **CLOSED**. |

### PROTO-001 new attack-surface verdict

| ID | Threat | Verdict | Evidence |
|----|--------|---------|----------|
| a | H2 backend TLS verification posture | **PASS-with-disclosed-gap** | `crates/lb-io/src/http2_pool.rs` doc-comment lines 35-38 explicitly state the upstream H2 path is **plaintext only**: "TLS termination on the upstream side... PROTO-001's e2e tests run plaintext H2 backends; production H2 backends behind TLS will need ALPN-aware dial machinery, which is OUT-OF-SCOPE for v1." `Http2Pool::dial_and_handshake` (lines 256-288) calls `Builder::handshake(TokioIo::new(stream))` over a raw TCP stream from `TcpPool::acquire`. No `rustls::ClientConfig`, no SNI, no cert verification. This is a deliberate v1 limitation — backends-behind-TLS were never claimed. Tracked as residual D3-1 below. |
| b | H3 backend SNI handling | **PASS** | `UpstreamBackend::h3(addr, sni)` (`crates/lb-l7/src/upstream.rs:88-94`) requires SNI; `UpstreamProto::H1`/`H2` ignore the field. `request_h3_upstream` (`crates/lb-quic/src/h3_bridge.rs:348-367`) takes `sni: &str` and forwards it to `QuicUpstreamPool::acquire(addr, sni)` (`crates/lb-io/src/quic_pool.rs:250`), which passes it as `Some(sni)` to `quiche::connect` (line 356). Cert verification depends on the caller-supplied `config_factory`; the e2e tests (`tests/proto_translation_e2e.rs:341-342`) use `verify_peer(true)` + `load_verify_locations_from_file(ca_path)`, demonstrating the pool honours real-cert paths when configured. Module doc-comment lines 21-29 makes this responsibility explicit. |
| c | Cross-protocol header injection (hop-by-hop strip in live e2e path) | **PASS** | `H1Proxy::handle` (`crates/lb-l7/src/h1_proxy.rs:337`) and `H2Proxy::handle` (`crates/lb-l7/src/h2_proxy.rs:318`) call `strip_hop_by_hop(&mut parts.headers)` BEFORE the `match backend.proto` dispatch, so the H1↔H2, H1↔H3, H2↔H3 paths all see headers post-strip. The strip removes Connection / Keep-Alive / Proxy-Authenticate / Proxy-Authorization / TE / Trailers / Transfer-Encoding / Upgrade plus any tokens listed inside the Connection header value (`h1_proxy.rs:677-697`). Tested by `hop_by_hop_headers_stripped_from_request` (h1_proxy.rs:1007) and `h2_proxy_hop_by_hop_stripped` (h2_proxy.rs:876). gRPC dispatch happens before strip but the H2 codec's RFC 9113 §8.1.2.2 enforcement at hyper's frame layer rejects connection-specific headers anyway, so an H2 gRPC client cannot smuggle Upgrade/Connection through. |
| d | Backend protocol confusion (H1 client + H1-config + Upgrade: h2c) | **PASS** | `Upgrade` is in `HOP_BY_HOP` (h1_proxy.rs:62). An H1 client sending `Upgrade: h2c\r\nConnection: Upgrade, HTTP2-Settings\r\nHTTP2-Settings: …` first has its `Connection` token list parsed (h1_proxy.rs:680-688) → `HTTP2-Settings` and `Upgrade` are added to the strip set, then `HOP_BY_HOP` removes both `Connection` and `Upgrade` themselves. The hyper H1 client (`proxy_request` line 380) never sees the upgrade tokens. The WebSocket upgrade path (line 322) gates on `is_h1_upgrade_request` which requires `Upgrade: websocket` AND `Connection: upgrade` — `h2c` does not match. Confirmed: an H1 client targeting an H1 backend cannot forge an H2 prior-knowledge handshake on the upstream pool. |
| e | gRPC over H1 listener (should reject; PROMPT.md §13 mandates H2/H3) | **PASS-with-caveat** | `is_grpc_request` is invoked exclusively from `H2Proxy::handle` (h2_proxy.rs:283); `H1Proxy::handle` has zero references to gRPC (verified by grep). A POST `application/grpc` arriving at an H1 listener is forwarded as an opaque H1 body to the backend — no gateway-level gRPC processing (no deadline clamp, no health synthesis, no trailer rewrite). This is *not* a strict reject (no 415 / 400) — it's transparent H1 forwarding. PROMPT.md §13 says gRPC needs H2/H3; the gateway honours that by only attaching `GrpcProxy` to `H2Proxy`. Operators who terminate H1 in front of a gRPC backend get H1 transparent forwarding rather than gateway gRPC features. Acceptable for v1, but worth documenting. Tracked as residual D3-2. |

### New HOLD items (delta)

**None blocking.** No HIGH or CRITICAL severity reached. Three residual
risks below are LOW-to-MEDIUM and are documented in
`docs/gap-analysis.md` (or now flagged for a follow-up entry).

### Residual risks after round-3

| ID | Severity | CVSSv3 | Description | Suggested fix |
|----|----------|--------|-------------|----------------|
| D3-1 | MEDIUM | 4.3 AV:N/AC:H/PR:N/UI:N/S:U/C:L/I:L/A:N | `Http2Pool` dials upstream H2 in PLAINTEXT only. A backend that requires TLS (e.g. Google Cloud-style ALB-to-internal-service H2 with TLS) is unreachable; an operator who *thinks* the upstream is encrypted will get plaintext. The module doc explicitly discloses this, and the tests use plaintext, but the binary's config validator does not reject `protocol = "h2"` + `tls = true`-shaped expectations. Risk is lower than it looks: there is no v1 binary wiring of `UpstreamProto::H2` at all (`grep -n 'Http2Pool\|UpstreamProto' crates/lb/src/main.rs` returns 0 hits). PROTO-001 ships the library API and e2e tests; binary-side wiring is a separate item. | Add a TLS variant to `Http2Pool` using `rustls::ClientConfig` with ALPN `h2`, plus a config-validator check that disallows `protocol = "h2"` until that path lands. |
| D3-2 | LOW | 3.1 N/A | gRPC over H1 listener silently transparent-forwards instead of rejecting. PROMPT.md §13 mandates H2/H3 for gRPC; v1 doesn't enforce a 415 at the H1 listener. Operator misconfiguration risk; not exploitable. | Either reject `application/grpc` at the H1 listener with `415 Unsupported Media Type`, or document the behaviour explicitly in `CONFIG.md`. |
| D3-3 | LOW | N/A | `7c1d9f99` reclassification rationale invokes "h2 0.4 `SendRequest` does not surface GOAWAY error_code post-teardown" but the test in question (`ping_flood_goaway`) does not use `SendRequest` at all — it writes raw frames. The actual h2 API surface (`h2::Error::reason() -> Option<Reason>` for `Kind::GoAway`, plus `is_go_away()` / `is_remote()`) does carry the GOAWAY reason for client-error returns. The path *is* tested end-to-end and the deferral is reasonable, but the reasoning paragraph in `docs/gap-analysis.md` should be edited to say "the test writes raw frames; reading raw frames back to assert GOAWAY ENHANCE_YOUR_CALM is incremental coverage worth tracking but not a missing test". Honesty nit, not a security concern. | Edit the TEST-002 description in `docs/gap-analysis.md` to remove the inaccurate `SendRequest` claim. |
| D3-4 | LOW | 3.1 | Workspace test sweep ran 502 tests / 0 failed in this session. FLAKE-002 author's claim of "pre-fix 49/50 PASS, post-fix 50/50" was not independently re-stress-tested by the auditor (50 sequential runs would take significant time). The fix is sound by construction (DashMap entry guards serialise the create-and-register), so the residual risk is purely coverage. | Optional: a CI sweep (`for i in $(seq 50); do cargo test -p lb-observability thread_safe_increment; done`) on a future cadence. |

Round-2 residuals D-3..D-7 + D-1 + D-2 are fully closed by the round-3
commits. Round-1 residuals D-1 (CID-cap), D-2 (ZeroRTT keyed hash),
D-3 (detector wiring) all already closed in round-2. Outstanding
non-PROTO-001 gap-analysis items (XDP-ADV-001, H3-INTEROP-001,
OBS-001/002, HARNESS-001, POOL-001, FLAKE-001) are unchanged and
deliberately deferred per `SHIP.md`.

### Always-on checks

| Check | Result |
|-------|--------|
| `cargo test --workspace --no-fail-fast` | **502 passed / 0 failed / 0 ignored** in this session |
| `cargo deny check` | **ok** (advisories ok, bans ok, licenses ok, sources ok; same harmless `unmatched license allowance` warnings as round-2) |
| `/tmp/bin/trufflehog git --only-verified` | `chunks 13748, bytes 25413619, verified_secrets 0, unverified_secrets 0` |
| `bash scripts/halting-gate.sh` | **GREEN** (Artifacts 141/141, Tests 59/59, Manifest OK) |
| `unsafe` count | **20** (16 blocks + 4 `unsafe fn`) — **unchanged from round-1/2**. `git diff 8e9a37b7..da1ef384` finds zero new `unsafe` keyword additions in any of the 6 closure commits. SAFETY comments unaffected. |
| Panic-free in production code | Crate-root `#![deny(clippy::unwrap_used, expect_used, panic, indexing_slicing, todo, unimplemented, unreachable)]` enforced (sample: `crates/lb-l7/src/lib.rs:2-11`); halting-gate AWK-skip pass GREEN. New code in 6 commits inspected manually: `Http2Pool` uses `?`/`map_err`/`ok_or_else` consistently; `decode_health_check_service` is bounds-checked at every step; `parse_and_clamp_grpc_timeout` uses let-else; FLAKE-002 fix uses `Entry::Vacant`/`Entry::Occupied` pattern with no `.unwrap()`. |

### Signature

auditor (round-3 delta 2026-04-25) — **PASS** with 4 residual risks
(D3-1..D3-4), 0 HOLD-blocking, 0 HIGH/CRITICAL.

