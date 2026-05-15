### ROUND8-L7-07 — No frame-level H2 read timeout (nginx `http2_recv_timeout` class)

Reference: `audit/round-8/research/nginx.md` lesson 19 (`http2_recv_timeout = 30s` default — independent of TCP-level idle, puts a frame-arrival deadline that defeats H2 slowloris); `audit/round-8/research/haproxy.md` lesson 13 (HAProxy `tune.h2.fe.glitches-threshold` as protocol-abuse counter; equivalent shape). `ref-l7` handoff cross-cutting #4: frame-level timer is a *distinct* knob from TCP idle.
Our equivalent: `crates/lb-l7/src/h2_security.rs:54-63` (we have `keep_alive_interval` + `keep_alive_timeout` — PING-based), `crates/lb-security/src/slowloris.rs` (header-phase only, H1)

Severity: medium
Status:   Proposed-Fix(div-l7, cherry-pick c38c67b0) — PUSH-BACK(verifier=verify)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): PUSH-BACK. The high-severity L7-07 frame-recv timer is NOT wired (no per-connection tokio::time::Interval, no h2_proxy.rs wiring, no config knob). GlitchesCounter (lb-security/src/glitches.rs) ships with FrameRecvTimeout kind but has ZERO detector callsites and no Prometheus surface — Theme-1 "library shipped, no caller". Plan's proof test round8_h2_glitches.rs ABSENT. hyper-1.x deferral for the *timer* is honest, but the finding cannot be Verified-Fixed: the H2 slow-frame-arrival slowloris vector remains open (only PING+total budget, which the finding already deemed insufficient). Stays Proposed-Fix. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference (nginx)**: `http2_recv_timeout = 30s` — if no H2 frame arrives within this window on a peer-driven stream, close the connection. *Separate from* `client_header_timeout` (TCP-level) and `keepalive_timeout`.
- **Us**: we wire hyper's `keep_alive_interval` + `keep_alive_timeout`, which is PING-based. A client that streams a 64 KiB HEADERS frame one byte per 29s will not trigger our timer (PINGs are server-initiated, not gated on inbound frame progress). The `SlowlorisDetector` in `lb-security` is H1-only.

Impact:
- H2 slowloris: 100 streams, each receiving HEADERS at <minimum-rate-per-stream. Hyper's `max_concurrent_streams = 256` is respected but per-stream byte arrival is not policed. With 100 long-lived streams the connection ties up worker memory for the worst-case stream lifetime (limited only by `total` connection budget — our default `total` is the conn-lifetime ceiling, not a frame-arrival ceiling).
- nginx specifically docs `http2_recv_timeout` as the slowloris defence for H2.

Reproduction:
- Static evidence:
  - `crates/lb-l7/src/h2_security.rs:91-92` — `keep_alive_interval`/`keep_alive_timeout` are PING-based.
  - `crates/lb-security/src/slowloris.rs:18-22` — fields are `header_timeout_ms` + `min_rate_bytes_per_sec`; only `lb-h1` consumes it.
  - No `recv_frame_timeout` in `H2SecurityThresholds` or `Http2PoolConfig`.

Recommendation:
1. Add `[runtime].h2_recv_frame_timeout_ms` (default 30_000) to `lb-config`.
2. Plumb through `H2SecurityThresholds`. Hyper's `http2::Builder` does not expose a per-frame timer directly; the practical pattern is a per-connection `tokio::time::Interval` that closes the connection if zero frames have been observed in the interval. Track frame-receipt timestamps in a `ZeroWindowStallDetector`-style structure already in `lb-h2/src/security.rs`.
3. Consider the HAProxy "glitches" pattern (lesson 13): consolidate the per-frame timeout, rapid-reset, continuation-flood, settings-flood into a single named *protocol-abuse score* that operators tune with one knob. See ROUND8-L7-12 for the consolidated counter.
