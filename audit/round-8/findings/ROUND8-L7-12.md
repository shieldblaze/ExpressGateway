### ROUND8-L7-12 — N independent H2 abuse counters, no consolidated "glitches" threshold (HAProxy 3.0 lesson)

Reference: `audit/round-8/research/haproxy.md` lesson 13 (`tune.h2.fe.glitches-threshold` — HAProxy 3.0's named primitive for "the client is doing low-grade abuse"; counts CONTINUATION-flood, rapid-reset, malformed HPACK indices into one observable counter). Defensive pattern #1. `ref-l7` prediction #2: "Frame-level HTTP/2 abuse counter is probably N independent counters in our `lb-h2/security.rs`, not a single named threshold that operators can tune."
Our equivalent: `crates/lb-h2/src/security.rs:32-117` (`RapidResetDetector`), `crates/lb-h2/src/security.rs:122-163` (`ContinuationFloodDetector`), `crates/lb-h2/src/security.rs:168-219` (`HpackBombDetector`), `crates/lb-h2/src/security.rs:245-304` (`SettingsFloodDetector`), `crates/lb-h2/src/security.rs:311-368` (`PingFloodDetector`), `crates/lb-h2/src/security.rs:378-430` (`ZeroWindowStallDetector`)

Severity: low
Status:   Proposed-Fix(div-l7, cherry-pick c38c67b0) — Accepted-with-caveat(verifier=verify)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): GlitchesCounter structure shipped per design intent (lb-security/src/glitches.rs, 289 lines, weighted GlitchKinds) but DORMANT — no detector callsites, no Prometheus surface, no proof test. As a low-severity operator-UX finding the structure exists; tracked with L7-07's hyper-2.x follow-up. Stays Proposed-Fix (low). See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: HAProxy 3.0 specifically added `glitches-threshold` because operators could not reason about N knobs. Six counters became one knob and one chart. Operators tune one number; metric dashboards plot one number.
- **Us**: six independent `Detector` types, six independent thresholds, six independent Prometheus counters (if wired). An operator wanting to tune "how aggressive should the H2 abuse defence be" must understand all six and tune them in concert.

Impact:
- Operator UX, not a bug class per se. But the lesson is real: when HAProxy collapsed to one knob, false-positive tuning cycles collapsed, and the metric became actionable. Our current shape is "scientifically correct but operationally hard".

Reproduction:
- Static evidence in the file paths above; count the constants — `DEFAULT_SETTINGS_MAX_PER_WINDOW`, `DEFAULT_PING_MAX_PER_WINDOW`, `DEFAULT_CONTROL_FRAME_WINDOW`, `DEFAULT_ZERO_WINDOW_STALL_TIMEOUT`, plus the four per-detector explicit construction-time params. Six tunables minimum.

Recommendation:
1. Introduce `GlitchesCounter` in `lb-h2/src/security.rs` that owns the six detectors and exposes:
   - `threshold_per_window: u32` (default 200 — the sum of "reasonable" individual events)
   - `record_event(kind: GlitchKind, weight: u32)` — each detector calls this in addition to (or replacing) its individual threshold check.
   - GlitchKind weights: rapid-reset = 5, continuation-cont = 1, settings = 2, ping = 2, hpack-ratio-trip = 10, zero-window-stall = 5.
2. Surface to Prometheus as `lb_h2_glitches_per_connection{listener}` plus per-kind sub-counters.
3. Keep the existing individual detectors for unit-test granularity; the aggregator runs *in addition*.
4. Document the design decision in `audit/decisions/` so the operator-facing knob is durable.

This is a quality-of-implementation finding, not a defect — but it is an unforced divergence from the industry's strongest operator-affordance pattern.
