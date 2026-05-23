# S8 — H2→H1 R8 Streaming Plan — RESOLVED + LEAD-R8-APPROVED

- Author: `lead` (S8). Supersedes the DRAFT `s8-h2h1-plan.md` head-start.
- Base: `feature/h-matrix-s8` @ `8ed1e92c` (= main, S7 promoted). Phase 0 GREEN
  (R1×3 = 1155/0/16 deterministic; fmt/clippy clean).
- Cell: **H2→H1** — HTTP/2 frontend ⇒ HTTP/1.1 backend. Plane: `lb-l7`
  (`crates/lb-l7/src/h2_proxy.rs`).
- **Status: LEAD-R8-APPROVED for build.** Each increment still gets
  plan→build→independent-verify→commit→push.

---

## Resolution of the 4 open questions (R7: lead decision; none is a product fork)

### Q-D1 — validate-before-forward WITHOUT whole-body buffering. **RESOLVED: lookahead-window design. No test weakened, no escalation.**

**The tension (first-hand, `h2_proxy.rs:1296-1398` + `tests/h2_validation_before_forward.rs:326-462`):**
The current code `Limited::collect()`s the whole inbound body *before* dialing, so
malformed-body/trailer requests are rejected with **zero backend dials**. Two
deterministic gate tests assert exactly that:
`content_length_mismatch_never_leaks_backend_body` and
`pseudo_header_in_trailers_never_leaks_backend_body` each assert BOTH (a) the
wire invariant `expect_protocol_error_not_backend_body` (client sees
RST/GOAWAY, never the backend `DATA(2)`), AND (b) `backend_dials == 0`.

For body/trailer-level malformations (content-length≠ΣDATA; pseudo-header in
trailers) the defect is only knowable *after* consuming the body/trailers.
Naïve forward-as-it-arrives streaming dials on the first chunk → breaks the
zero-dial assertion → would force weakening a passing security test (R5 ✗).

**Decision — the in-flight window doubles as a validation lookahead:**
The M-D pump reads inbound H2 `IncomingBody` frames into a bounded buffer whose
size is capped at the fixed in-flight window (Q-D4: 8 × 8 KiB = 64 KiB,
body-size-INDEPENDENT). Two regimes fall out naturally:

- **Whole request ≤ window** (the common case; ALL malformed-probe tests use
  2-byte bodies): the buffer reaches inbound EOF (incl. trailers) *before* the
  window fills. Polling to EOF drives the *identical* hyper/h2 validation that
  `collect()` does today (collect is just poll-to-EOF). content-length mismatch
  surfaces as the terminal `Err`; trailer pseudo-headers are checked on the
  trailers frame. **Validation completes BEFORE the dial → zero-dial preserved
  → both gate tests pass UNCHANGED (R5 ✓).**
- **Request > window**: when the buffer hits the window high-watermark before
  EOF, M-D dials, forwards headers + drains the buffer, and enters streaming
  mode (forward-as-it-arrives, memory pinned at the window). The downstream
  **response-head relay is gated on the inbound body reaching a validated
  terminal state** (clean EOF, or a surfaced protocol `Err` mapped to
  RST/GOAWAY). So even for a >window body that turns malformed at the trailers,
  **no backend response body is ever relayed downstream** — the h2spec wire
  invariant the brief mandates ("framing+header validation before forward; do
  not recreate the validate-vs-forward race") is preserved.

**Residual (documented honestly, NOT gamed):** for a request whose body exceeds
the 64 KiB window AND is malformed only in its body/trailers, the backend IS
dialed (gets a partial/aborted request; its connection is dropped, not pooled).
This is *weaker than zero-dial but exactly what h2spec requires* (no downstream
leak), and it is the **security-preferable** posture: a flood of *cheap* tiny
malformed requests — the actual DoS-amplification vector — stays zero-dial
(fits in window); only a body the attacker already paid ≥64 KiB to send causes
a dial. No existing test asserts zero-dial for >window bodies. The verifier
adds an adversarial >window-malformed case asserting the **no-downstream-leak**
invariant (not zero-dial) for that regime, plus a connection-not-poisoned check.

Why this is a lead decision and not a fork: the brief's binding requirement is
"framing and header validation complete before forward" + "do not recreate the
[downstream-leak] race." Both hold under this design for all sizes. The
zero-dial property the buffering implementation happened to provide is
preserved for every case any test (or any cheap attacker) exercises. No rule is
in tension; R5 and R8 are both satisfied. Per R7 ("don't escalate what the
rules answer") → lead-resolved.

### Q-D2 — plane boundary / egress reuse. **RESOLVED: reuse the IN-CRATE lb-l7 hyper H1 sender; do NOT reach into lb-quic.**
The DRAFT assumed reuse of `h3_bridge::write_h1_request`/`stream_h1_response`.
First-hand: `write_h1_request` is **crate-private to lb-quic** (not `pub`,
`h3_bridge.rs:1877`) → unreachable from lb-l7. `stream_h1_response` is `pub` but
H3-actor-centric (drives a `Sender<RespEvent>` channel/`conn_actor` model) — an
architectural impedance mismatch. The H2→H1 path already uses hyper's H1 client
(`hyper::client::conn::http1::handshake` → `SendRequest`, `h2_proxy.rs:1368`).
**Decision:** M-D keeps the existing lb-l7 hyper H1 sender and changes only the
*body* it sends: from a buffered `BoxBody<Bytes>` (`build_h2_body_with_trailers`)
to a **streaming body fed by a bounded channel** (`http_body_util` stream body
over a `mpsc` receiver). Entirely within `crates/lb-l7` — **no lb-quic / lb-io /
cross-crate edit** (avoids the I0.5-class surprise the DRAFT feared). The
"reuse the verified H3→H1 egress" intent is honored at the *discipline* level
(same depth×chunk window + backpressure causal chain), not by calling H3 code.

### Q-D3 — gauge placement for the lb-l7 plane. **RESOLVED: add a lb-l7 `test-gauges` feature + lb-l7 statics mirroring lb-quic.**
lb-l7 has no gauges/feature today. **Decision:** add `[features] test-gauges = []`
to `crates/lb-l7/Cargo.toml` and define, in `h2_proxy.rs` under
`#[cfg(any(test, feature = "test-gauges"))]`, the statics:
- `H2_REQ_MAX_RETAINED_BODY_BYTES: AtomicUsize` — max instantaneous retained
  inbound-request memory (lookahead buffer + channel occupancy), updated at the
  pump's record point. Mirrors lb-quic `MAX_RETAINED_BODY_BYTES`.
- (request leg only this cell; response leg of H2→H1 reuses hyper's bounded
  `Incoming` — gauge it via `H2_RESP_MAX_RETAINED_BODY_BYTES` if the verifier
  finds the response path needs a non-vacuous proof; see §verify.)
R10: the workspace gate runs `--all-features`, which enables `test-gauges`, so
the memory proof is NOT silently skipped (S7 precedent: gauges subsumed by
`--all-features`). Pin the constants with a sanity-assert test (S7 precedent).

### Q-D4 — in-flight window constants. **RESOLVED: mirror the H3 ceiling discipline.**
`const H2_REQ_CHANNEL_DEPTH: usize = 8;` and `const H2_REQ_CHUNK_MAX: usize = 8 * 1024;`
(mirroring `H3_BODY_CHANNEL_DEPTH=8`, `H3_BODY_CHUNK_MAX=8 KiB`). **Ceiling
formula:** `retained ≤ H2_REQ_CHANNEL_DEPTH × H2_REQ_CHUNK_MAX = 64 KiB`,
body-size-independent and independent of `MAX_REQUEST_BODY_BYTES` (64 MiB). The
non-vacuous memory test drives a multi-MiB body and asserts
`MAX_RETAINED ≤ 4 × 64 KiB = 256 KiB` (S7's generous-multiple convention to
absorb ingest-side slack), with an inverted probe proving the bound is
load-bearing.

---

## Build increments (each: plan→build→independent verify→commit→push)

- **I1 — gauge + constants scaffold (no behavior change).** Add `test-gauges`
  feature, the `H2_REQ_*` constants, the gauge statics + a `record_retained`
  helper, and the constant-pin sanity test. Keeps `collect()` for now. Gate
  stays green. (builder-1)
- **I2 — M-D bounded ingress pump.** Replace `Limited::collect()` with the
  lookahead-window pump feeding a streaming upstream body via the existing
  hyper H1 sender; framing/header validation before dial; response-head relay
  gated on inbound validated terminal state; window-full → dial+stream.
  Update the gauge at the record point. (builder-1)
- **I3 — verification suite** (verifier, author≠builder): real-wire, memory
  (both directions as needed), backpressure (WINDOW_UPDATE withheld), h2spec
  ordering (existing 2 gate tests UNCHANGED + ≥1 new >window adversarial
  no-leak case), smuggling parity, ≥80% independent coverage of M-D session
  sub-metric.

## Verification bar (BUILT) — unchanged from DRAFT §5, with Q-D1 additions
1. Real-wire H2 client → h2_proxy listener → router → real H1 backend; binary
   bodies both directions.
2. Non-vacuous memory: `H2_REQ_MAX_RETAINED_BODY_BYTES ≤ 256 KiB` for a
   multi-MiB body; body byte-identical; inverted probe.
3. Backpressure: downstream/upstream stall ⇒ channel fills ⇒ pump stops polling
   inbound ⇒ hyper/h2 withholds WINDOW_UPDATE ⇒ H2 client paused (proven chain).
4. h2spec ordering: the 2 existing gate tests pass UNCHANGED (zero-dial for
   ≤window malformed); NEW: >window malformed body/trailers ⇒ client sees
   RST/GOAWAY, NO backend body relayed downstream, upstream conn not poisoned.
5. Smuggling parity: client RST mid-request never seen as a complete request at
   the H1 upstream.
6. ≥80% independent canonical coverage of the M-D session sub-metric.
7. R3 no-regression; fmt/clippy clean; R10 gauge flag honored under
   `--workspace --all-features`.
