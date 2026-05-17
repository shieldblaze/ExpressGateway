### ROUND8-L7-15 — Edge-defaults parity gap (Envoy + nginx canonical edge baseline)

Reference: `audit/round-8/research/_l7_handoff.md` cross-cutting #5 (the eight-item canonical edge baseline); `audit/round-8/research/envoy.md` lesson 13 + 14 (max_concurrent_streams=100, initial_stream_window=64KiB, initial_connection_window=1MiB, headers_with_underscores=REJECT); `audit/round-8/research/nginx.md` lessons 15-20 (header buffer caps, keepalive_requests, lingering_close, http2_max_concurrent_streams=128, http2_recv_timeout=30s, reset_timedout_connection). `ref-l7` prediction #3: "even one missing default is a finding."
Our equivalent: `config/default.toml` (12 lines, two listeners, no security defaults), `crates/lb-l7/src/h2_security.rs:72-97` (`H2SecurityThresholds::default`), `crates/lb-config/src/lib.rs` (config schema)

Severity: medium
Status:   Verified-Fixed(verifier=verify, 42569a1f)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): config/default.toml expanded to canonical edge baseline, docs/edge-defaults.md, audit/decisions/h2-edge-streams.md ADR for max_concurrent_streams=256. round8_edge_defaults_table 2/2 PASS. See audit/round-8/verify/l7.md. -->

Divergence summary table:

| Knob | Envoy edge | nginx default | Us | Verdict |
|------|-----------|---------------|----|---------|
| `max_concurrent_streams` | 100 | 128 | **256** (`h2_security.rs:82`) | Looser than both refs — choose deliberately |
| `initial_stream_window_size` | 64 KiB | 64 KiB (= RFC) | **65_535** (`h2_security.rs:93`) | Matches — OK |
| `initial_connection_window_size` | 1 MiB | varies | **1 MiB** (`h2_security.rs:94`) | Matches — OK |
| `keepalive_requests` (count cap) | n/a (no count) | 100 | **none** | See ROUND8-L7-06 |
| `keepalive_timeout` (wall) | n/a | 75 s | **30 s** (interval), 30 s (timeout) | OK |
| `headers_with_underscores_action` | REJECT_REQUEST | drop | **allow** (no filter) | See ROUND8-L7-05 |
| `normalize_path` / `merge_slashes` | on | on | **off** | See ROUND8-L7-13 |
| `reset_timedout_connection` | n/a | off (FIN, opt-in RST) | **n/a** (no choice exposed) | Defer doc |
| `http2_recv_timeout` (frame-level) | n/a | 30 s | **none** (only PING) | See ROUND8-L7-07 |
| `large_client_header_buffers` cap | various | 4×8 KiB | `MAX_HEADER_BYTES = 65_536` (`lb-h1/parse.rs:12`) | Matches single-buffer 64 KiB |

The `config/default.toml` file is **12 lines** and exposes zero security defaults:
```toml
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"
# (no rate limits, no smuggle policy, no H2 thresholds, no slowloris settings)
```

Severity: medium
Status:   Open

Divergence:
- The `lb-config` schema *exposes* most of these knobs (we have `H2SecurityThresholds::default()` etc.), but `config/default.toml` does not enumerate them — an operator copy-pasting our default config gets a listener with hard-coded compile-time defaults and no documentation of what they are.
- `max_concurrent_streams = 256` is double Envoy edge and 2× nginx default. The choice may be deliberate (we want higher throughput), but is not documented in `audit/decisions/`.

Impact:
- Operator experience: deploying our LB on an edge IP requires reading source code to know what limits they ship with. Industry-standard edge proxies ship documented defaults.
- Audit risk: a future cargo upgrade that drifts the defaults (hyper bumps `max_concurrent_streams` default upstream, say) silently changes our exposed posture.

Reproduction:
- `cat config/default.toml` — 12 lines.
- `cargo doc --no-deps --open -p lb-config` — schema is there but no defaults document.

Recommendation:
1. Expand `config/default.toml` to the canonical edge baseline: enumerate `[runtime]` defaults for max_concurrent_streams, initial windows, slowloris timeouts, smuggle mode, conn-gate caps, keepalive count cap (after ROUND8-L7-06).
2. Add `docs/edge-defaults.md` documenting the choice for each value vs. Envoy / nginx.
3. Pin defaults via a `compile_time_defaults` integration test that asserts each constant against an explicit table — drift detection.
4. Specifically address `max_concurrent_streams = 256` vs Envoy 100 in `audit/decisions/h2-edge-streams.md`: is this a throughput-vs-DoS tradeoff we *chose*, or an oversight?
