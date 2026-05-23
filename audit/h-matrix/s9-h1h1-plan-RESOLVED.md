# S9 — H1→H1 R8 Streaming Plan — RESOLVED (lead-approved)

- Author: `lead` (S9). Resolves the DRAFT `s9-h1h1-plan.md` Q-H1..Q-H4 against the
  live tree at `feature/h-matrix-s9` @ 8723c205. **Status: R8-APPROVED for build.**
- Cell: **H1→H1** — HTTP/1.1 front ⇒ HTTP/1.1 backend. Plane `lb-l7`,
  `crates/lb-l7/src/h1_proxy.rs`, `H1Proxy::proxy_request` (currently @ 1073).
- Phase 0a (F-MD-4 sweep) is complete: all 4 BUILT cells PASS by mechanism
  (see `s9-fmd4-sweep.md`). This plan carries the F-MD-4 discipline forward to H1.

---

## Resolution of Q-H1..Q-H4 (each states decision + reasoning; R8-checked)

### Q-H1 (pump reuse) — RESOLVED: **MIRROR the Branch-B pump in `h1_proxy.rs` (option b).**
The M-D pump in `h2_proxy.rs` is a just-verified BUILT cell. Extracting a shared
helper (option a) edits that code → forces a full M-D re-verify and risks
regressing a BUILT cell. And the H1 pump is **Branch-B-only** (no lookahead /
zero-dial / validate-before-forward regime), so a shared helper would need
conditional complexity to serve both H1 and H2 — not a clean DRY win. Mirror the
simpler pump in `proxy_request`. **R8 check:** bounded `mpsc` (depth 8 × 8 KiB =
64 KiB in-flight window) → in-flight-counting `StreamBody` → hyper http1 sender;
retained memory bounded by the FIXED window, **independent of body size and of
`MAX_REQUEST_BODY_BYTES`**. CF-DEDUP-1 carry-forward: a later refactor DRYs the
two pumps once both cells are BUILT and both have independent regression locks.

### Q-H2 (keep-alive cap + single-use upstream contract) — RESOLVED: **pump confined to upstream body delivery; both contracts preserved, regression-locked.**
- The keep-alive request cap (ROUND8-L7-06) is **connection-driver** state
  (`ProxyService.{served,cap,close_signal}`, the `cap_close` arm @ h1_proxy.rs:629,
  `keepalive_cap_termination_counter()` @ :453) — orthogonal to `proxy_request`.
- The single-use upstream contract (ROUND8-L7-10) is the `take_stream()`
  take-and-discard @ h1_proxy.rs:1093, **regression-locked by
  `crates/lb-l7/tests/round8_body_overread.rs`**, which asserts the source
  RETAINS the ROUND8-L7-10 doc-block + the `set_reusable` citation.
- **BUILDER CONSTRAINT:** the pump changes ONLY how the request body reaches
  `sender.send_request` (a `StreamBody` instead of the raw `IncomingBody`). It
  MUST NOT remove/relocate the ROUND8-L7-10 doc-block, MUST keep `take_stream()`
  single-use, MUST NOT pool the upstream. **Verifier proves:**
  `round8_body_overread.rs` green + keep-alive cap behavior unchanged + the 3
  `h1_proxy_e2e` tests green.

### Q-H3 (request trailers) — RESOLVED: **forward trailers faithfully (mandatory, R3); forbidden-framing-field trailer reject only if the live decoder doesn't already enforce it.**
Today `proxy_request` hands the whole `IncomingBody` to hyper, which forwards
chunked request trailers natively; `tests/bridging_h1_h1.rs` exercises a trailer
round-trip. A frame-reading pump MUST forward the inbound trailers `Frame`
through the `StreamBody` or it regresses trailer forwarding (R3 — would fail
`bridging_h1_h1`). The H2 *pseudo-header-in-trailers* reject has no exact H1
analogue (H1 has no pseudo-headers); the equivalent intent — trailers must not
carry message-framing/routing fields (RFC 7230 §4.1.2: `Transfer-Encoding`,
`Content-Length`, `Host`, `Trailer`, `TE`, `Connection`, hop-by-hop) — is a
desync primitive if forwarded. **BUILDER:** (a) forward legitimate trailers
byte-faithfully; (b) FIRST confirm whether hyper's inbound H1 decoder already
strips/rejects framing-fields-in-trailers. If it does, document that (guard is
unnecessary) and stop. If it does NOT, add a minimal forbidden-trailer guard
→ 400, mirroring `validate_request_trailers`' intent, with a real-wire test.
Do NOT speculatively add scope — gate (b) on the decoder fact. Verifier validates
against live decoder behavior either way.

### Q-H4 (NEW 64 MiB cap) — RESOLVED: **apply `MAX_REQUEST_BODY_BYTES` (64 MiB) → 413 on exceed. Parity, NOT a product fork.**
`MAX_REQUEST_BODY_BYTES` (h2_proxy.rs:67-71) is a **documented cross-cell
constraint** explicitly naming "H2→H1, H2→H2, H2→H3 and H3" + `lb_quic::
MAX_REQUEST_BODY_BYTES`; H1→H1 is the lone gap. Closing it is consistency, not a
fork — no escalation. Mid-stream enforcement: `forwarded_total >
MAX_REQUEST_BODY_BYTES` → `BodyTooLarge` → 413 (the M-D rule, h2_proxy.rs:1774).
**Receiver-drop ≠ 413 (F-MD-2 carried forward):** a dropped upstream body
receiver = the backend early-responded/finished, NOT "too large" — drain-and-
validate, relay the backend's response; ONLY `forwarded_total` over cap is 413.
Existing `h1_proxy_e2e` tests use small bodies → no regression; verifier adds a
>64 MiB → 413 test.

---

## Carried-forward M-D fixes (mandatory; do NOT re-introduce the traps)
1. **HTTP-version mis-frame (F-MD-1):** when building the upstream `Request`
   around the `StreamBody`, set `parts.version = HTTP/1.1` and STRIP
   `content-length` + `transfer-encoding` so hyper frames the unknown-length
   streaming body itself (an inbound H1 request can carry `content-length`; a
   stale CL + unknown-length StreamBody mis-frames identically to F-MD-1).
2. **Receiver-drop ≠ 413 (F-MD-2):** drain-and-validate on receiver drop; relay
   the backend's early response. See Q-H4.
3. **Non-vacuous gauge (F-MD-3):** `H1_REQ_MAX_RETAINED_BODY_BYTES` (behind the
   lb-l7 `test-gauges` feature) tracks REAL occupancy — increment on push,
   DECREMENT when hyper pulls — never a constant. Must be load-bearing under the
   inverted probe (a whole-body-buffer variant MUST trip the ≤256 KiB ceiling).
4. **F-MD-4 (RST≠EOF) for H1:** a premature client TCP half-close mid-body
   surfaces on `IncomingBody::frame()` as an `Err` (incomplete message), NOT a
   clean `None`. The pump MUST propagate it as a `PumpAbort`-style body `Err`
   (never a clean-EOF drop that writes the upstream terminator); the upstream
   terminator is gated on a positively-confirmed clean end. The upstream conn is
   aborted (single-use `take_stream` already guarantees no pooling). Regression
   test: real-wire premature-close-mid-body → upstream sees INCOMPLETE, `complete=0`.

## Net-new = M-D-lite (ingress pump) + M-E (shared proof harness)
- **M-D-lite:** Branch-B-only bounded pump on the H1 `IncomingBody`.
- **M-E:** gauge (`H1_REQ_MAX_RETAINED_BODY_BYTES`), the stalled-backend
  non-vacuous memory test, slow-client + slow-upstream backpressure tests with
  proven causal chains, the inverted load-bearing probe.

## Verification bar (BUILT) — independent verifier, author≠verifier
1. Real-wire BOTH directions: genuine H1 client → real `h1_proxy` listener →
   router → real H1 backend; binary (non-UTF-8) request AND response bodies;
   byte-identical (per-chunk reader — heed the S8 cumulative-release harness bug).
2. Non-vacuous, body-size-INDEPENDENT memory both directions: multi-MiB body
   through a stalled backend; gauge ≤ ~256 KiB ≪ body; inverted probe trips.
3. Backpressure both directions, proven causal chain (channel fills → pump stops
   polling inbound → frontend TCP read window stalls → client paused; symmetric
   on the response leg).
4. F-MD-4: premature TCP close mid-body never seen complete at the H1 upstream
   (`complete=0`); existing CL/TE smuggle rejections still fire pre-pump.
5. 64 MiB cap → 413 (new on this path; confirm `h1_proxy_e2e` does not regress).
6. ≥80% independent canonical coverage on the M-D-lite session sub-metric.
7. R3: 3 `h1_proxy_e2e` + `round8_body_overread` + smuggle + keep-alive cap +
   `bridging_h1_h1` green; fmt/clippy clean; R10 gauge flag under
   `--workspace --all-features`.

## Increment sequencing (each: plan-approved → build → independent verify → commit → push)
- **I1:** introduce the bounded pump + `StreamBody` in `proxy_request` (forward-
  as-it-arrives, F-MD-1 version/CL strip, F-MD-4 Err-not-EOF, single-use intact).
- **I2:** the `H1_REQ_MAX_RETAINED_BODY_BYTES` gauge (test-gauges) + 64 MiB cap
  (F-MD-2 receiver-drop≠413) + trailers forwarding (Q-H3).
- **I3 (verifier-owned tests):** real-wire both-direction binary, memory gauge +
  inverted probe, backpressure both legs, F-MD-4 premature-close, 64 MiB→413,
  coverage sub-metric.
