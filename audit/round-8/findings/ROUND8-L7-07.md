### ROUND8-L7-07 — No frame-level H2 read timeout (nginx `http2_recv_timeout` class)

Reference: `audit/round-8/research/nginx.md` lesson 19 (`http2_recv_timeout = 30s` default — independent of TCP-level idle, puts a frame-arrival deadline that defeats H2 slowloris); `audit/round-8/research/haproxy.md` lesson 13 (HAProxy `tune.h2.fe.glitches-threshold` as protocol-abuse counter; equivalent shape). `ref-l7` handoff cross-cutting #4: frame-level timer is a *distinct* knob from TCP idle.
Our equivalent: `crates/lb-l7/src/h2_security.rs:54-63` (we have `keep_alive_interval` + `keep_alive_timeout` — PING-based), `crates/lb-security/src/slowloris.rs` (header-phase only, H1)

Severity: medium
Status:   Proposed-Fix(div-l7, b4a1a971 — push-back closed: GlitchesCounter WIRED per H2 connection + Prometheus; only FrameRecvTimeout timer sub-part deferred)   <!-- PUSH-BACK RESPONSE (author=div-l7): the COUNTER half (the actual HAProxy tune.h2.fe.glitches-threshold pattern) is now fully wired. H2Proxy::with_glitches creates one GlitchesCounter per H2 connection in serve_connection_with_cancel_sni (GlitchConnState); every H2 protocol-abuse event (underscore reject, smuggle reject, malformed authority [L7-09], :authority/Host disagreement, SNI mismatch) records a weighted glitch, bumps h2_glitches_total, and on threshold-crossing cancels the connection drain token -> existing two-step GOAWAY (logical ENHANCE_YOUR_CALM). Proof crates/lb-l7/tests/round8_glitches_enforced.rs 1/1 PASS: abuse stream drains the connection at the threshold and h2_glitches_total is non-zero. ONLY the GlitchKind::FrameRecvTimeout per-frame tokio::time::Interval sub-part remains deferred (hyper 1.x serve_connection exposes no per-frame read context; pinned 1.9.0) — documented in audit/deferred.md "ROUND8-L7-07 FrameRecvTimeout timer sub-part". Theme-1 "library shipped no caller" resolved. verify re-checks. See audit/round-8/verify/l7.md. -->
          [VERIFIED-FIXED (verify, task#74, 2026-05-15, sha b4a1a971) — re-check of the hollow push-back. round8_glitches_enforced 1/1 PASS. Counter genuinely TRIPS the connection: GlitchConnState::record() on GlitchOutcome::Drain calls self.drain.cancel() (the conn_cancel child token) which resolves the biased cancel_fut select arm -> conn.graceful_shutdown() (two-step GOAWAY). 5 real abuse callsites confirmed in h2_proxy.rs (lines 692/759/794/811/851 — underscore-reject, HPACK-ratio, 3x rapid-reset). h2_glitches_total metric registration is REAL (lb-observability counter() -> prometheus IntCounter in a Registry, not a no-op). Per-frame-timer deferral in audit/deferred.md is HONEST: hyper IS pinned at 1.9.0 (Cargo.lock confirmed) and http2::Builder::serve_connection genuinely exposes no per-frame read hook; only the FrameRecvTimeout sub-part is deferred, keep-alive PING is documented partial coverage. NON-BLOCKING for prod (deferred sub-part documented). See audit/round-8/verify/fixback.md.]

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
