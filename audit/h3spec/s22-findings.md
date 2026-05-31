# SESSION 22 — h3spec Conformance Findings (Phase 1)

**Date:** 2026-05-31 · **Base tip:** `b0381286` (S21 shippable v1) · **Branch:** `feature/h3spec-s22`
**Tool:** `h3spec 0.1.13` (Rust client; `h3spec <host> <port>`) · **Run cmd:** `h3spec -n -t 3000 127.0.0.1 8443`
**Target:** `expressgateway` release binary, `protocol = "quic"` H3-terminate listener (raw_proxy ABSENT), self-signed cert (h3spec `-n`), H1 backend.
**Raw run log:** `audit/h3spec/s22-h3spec-run-phase1.log`

## Headline

```
49 examples, 25 failures   (24 pass)
```

This is the program's **first real h3spec run** — h3spec/h3i was DEFERRED through the entire audit (`audit/deferred.md` PROTO-2-05, `FINAL_REPORT.md` "DEFERRED to CI"). 25 findings is the expected shape of a first conformance pass (cf. the foundation h2spec finding); a zero-finding run would have been the surprising result.

## The decisive attribution: TWO LAYERS

The 25 failures split cleanly across the two layers of the gateway's QUIC/H3 stack, and the split determines tractability:

| Layer | Impl | Findings | Verdict |
|---|---|---|---|
| **QUIC transport (RFC 9000)** | **quiche 0.28.0** (delegated; gateway has no own transport code) | #1–10 | Two **quiche-library limitations** — NOT gateway-tractable without forking/upgrading quiche → **ESCALATE / document** (R6 large, R7) |
| **HTTP/3 + QPACK (RFC 9114 / 9204)** | **Hand-rolled** `lb-h3` + `lb-quic` conn_actor (NOT `quiche::h3`) | #11–25 | **Gateway-level**, tractable; #12–15 are **security** (request integrity) → **FIX this session** |

**Why the asymmetry is believable:** the gateway's HTTP/2 server (which reuses **hyper**) passes h2spec **146/147 (1 skip, 0 fail)** — see `audit/h3spec/s22-h2spec-cfign1.log`. The H2 stack is mature and conformant; the H3 stack is a hand-rolled minimal terminator whose code comments explicitly state it "does NOT do full RFC 9110 validation" (`crates/lb-quic/src/h3_bridge.rs:753`). h3spec exposes exactly that gap.

---

## Findings table

Legend — Layer: `Q`=quiche transport, `G`=gateway H3/QPACK. Tier: `T1`=quiche close-suppression (escalate), `T2`=quiche no-validate (escalate), `SEC`=security fix, `COR`=correctness fix.

| # | Case (RFC §) | Expected | Gateway actual (measured) | Layer | Tier |
|---|---|---|---|---|---|
| 1 | TRANSPORT_PARAMETER_ERROR if `initial_source_connection_id` missing (Tx 7.3) | CONNECTION_CLOSE(0x8) | quiche detects (`InvalidTransportParam`) but **suppresses close** | Q | T1 |
| 2 | TPE if `original_destination_connection_id` received (Tx 18.2) | CC(0x8) | same — detected, close suppressed | Q | T1 |
| 3 | TPE if `preferred_address` received (Tx 18.2) | CC(0x8) | same | Q | T1 |
| 4 | TPE if `retry_source_connection_id` received (Tx 18.2) | CC(0x8) | same | Q | T1 |
| 5 | TPE if `stateless_reset_token` received (Tx 18.2) | CC(0x8) | same | Q | T1 |
| 6 | TPE if `max_udp_payload_size` < 1200 (Tx 7.4/18.2) | CC(0x8) | same | Q | T1 |
| 7 | TPE if `ack_delay_exponent` > 20 (Tx 7.4/18.2) | CC(0x8) | same | Q | T1 |
| 8 | TPE if `max_ack_delay` ≥ 2¹⁴ (Tx 7.4/18.2) | CC(0x8) | same | Q | T1 |
| 9 | PROTOCOL_VIOLATION if reserved bits in Handshake hdr non-zero (Tx 17.2) | CC(0xA) | quiche **never validates** reserved bits → silently accepts | Q | T2 |
| 10 | PROTOCOL_VIOLATION if reserved bits in Short hdr non-zero (Tx 17.2) | CC(0xA) | same — not validated | Q | T2 |
| 11 | H3_FRAME_UNEXPECTED if DATA before HEADERS (H3 4.1) | CC(H3_FRAME_UNEXPECTED) | no request-stream frame-sequencing state; not enforced; no error-code close | G | COR |
| 12 | H3_MESSAGE_ERROR if a pseudo-header is duplicated (H3 4.1.1) | CC(H3_MESSAGE_ERROR) | **no validation** — `from_headers` last-wins, request accepted/forwarded | G | **SEC** |
| 13 | H3_MESSAGE_ERROR if mandatory pseudo-header absent (H3 4.1.3) | CC(H3_MESSAGE_ERROR) | **missing `:method`/`:path`/`:scheme` silently defaulted** (GET, /, …); request accepted | G | **SEC** |
| 14 | H3_MESSAGE_ERROR if prohibited pseudo-header present (H3 4.1.3) | CC(H3_MESSAGE_ERROR) | **no validation** — unknown pseudo-header (e.g. `:status`) pushed to `extra` & forwarded | G | **SEC** |
| 15 | H3_MESSAGE_ERROR if pseudo-header after regular fields (H3 4.1.3) | CC(H3_MESSAGE_ERROR) | **no ordering check** | G | **SEC** |
| 16 | H3_MISSING_SETTINGS if first control frame ≠ SETTINGS (H3 6.2.1) | CC(H3_MISSING_SETTINGS) | client control stream **not read/tracked**; not enforced | G | COR |
| 17 | H3_FRAME_UNEXPECTED if DATA on control stream (H3 7.2.1) | CC(H3_FRAME_UNEXPECTED) | control stream not parsed; frames silently skipped | G | COR |
| 18 | H3_FRAME_UNEXPECTED if HEADERS on control stream (H3 7.2.2) | CC(H3_FRAME_UNEXPECTED) | same | G | COR |
| 19 | H3_FRAME_UNEXPECTED if second SETTINGS (H3 7.2.4) | CC(H3_FRAME_UNEXPECTED) | SETTINGS discarded, no dup-tracking | G | COR |
| 20 | H3_SETTINGS_ERROR if HTTP/2 SETTINGS ids included (H3 7.2.4.1) | CC(H3_SETTINGS_ERROR) | SETTINGS not inspected for reserved/H2 ids | G | COR |
| 21 | H3_FRAME_UNEXPECTED if CANCEL_PUSH in request stream (H3 7.2.5) | CC(H3_FRAME_UNEXPECTED) | not enforced | G | COR |
| 22 | QPACK_DECOMPRESSION_FAILED if invalid static index (QPACK 3.1) | CC(QPACK_DECOMPRESSION_FAILED) | QPACK decode error → log + stream drop, **no error-code close** | G | COR |
| 23 | QPACK_ENCODER_STREAM_ERROR if dyn-table capacity > limit (QPACK 4.1.3) | CC(QPACK_ENCODER_STREAM_ERROR) | encoder stream **not monitored** (static-only QPACK) | G | COR |
| 24 | H3_CLOSED_CRITICAL_STREAM if a control stream is closed (QPACK 4.2) | CC(H3_CLOSED_CRITICAL_STREAM) | critical-stream close **not detected** | G | COR |
| 25 | QPACK_DECODER_STREAM_ERROR if Insert Count Increment = 0 (QPACK 4.4.3) | CC(QPACK_DECODER_STREAM_ERROR) | decoder stream **not monitored** | G | COR |

**24 PASSING cases** (transport stack where quiche covers it): FLOW_CONTROL_ERROR, STREAM_LIMIT_ERROR, FRAME_ENCODING_ERROR (unknown frame, invalid MAX_STREAMS, STREAMS_BLOCKED, NEW_CONNECTION_ID variants), PROTOCOL_VIOLATION (no-frames, PATH_CHALLENGE, NEW_TOKEN, HANDSHAKE_DONE, CRYPTO-in-0RTT), STREAM_STATE_ERROR (×6), and all TLS-alert cases (KeyUpdate, no_application_protocol, missing_extension ×2, EndOfEarlyData). **quiche's transport conformance is strong** where it acts; the only gaps are the two below.

---

## Root-cause mechanisms (measured, not assumed)

### T1 — quiche suppresses CONNECTION_CLOSE on a first-packet transport error (#1–8)
**Measured:** gateway log on case #2 → `DEBUG "quiche recv" error="InvalidTransportParam"` (quiche DID detect). h3spec qlog → received only a **Retry**, then only its own Initial retransmits; **no CONNECTION_CLOSE on the wire**.
**Quiche source (`quiche-0.28.0/src/lib.rs`):**
- `recv()` on error: `self.close(false, e.to_wire(), b"").ok(); return Err(e);`
- `close()`: sets `local_error`, then **`if self.recv_count == 0 { self.mark_closed(); }`** → `self.closed = true`.
- `send_on_path()` top guard: `if self.is_closed() || self.is_draining() { return Err(Done); }` — fires **before** the CONNECTION_CLOSE-writing code, so the close is never emitted.
Because the offending transport param is in the ClientHello (the connection's *first* processed packet → `recv_count == 0`), quiche marks the connection closed and refuses to emit the close. quiche validates **all 8** of these params correctly (`transport_params.rs` rejects server-only ids when `is_server`, and range-checks `max_udp_payload_size`/`ack_delay_exponent`/`max_ack_delay`); the failure is purely that the close packet is suppressed. **The gateway's pump (`drain_conn_send`) is correct** — it loops `conn.send()` until `Done`; quiche returns `Done` with nothing written.

### T2 — quiche does not validate header reserved bits (#9–10)
**Quiche source (`packet.rs` `decrypt_hdr`):** after header-protection removal quiche writes the decrypted first byte back **without checking the reserved bits** (RFC 9000 §17.2). The gateway cannot inspect these bits itself — they are under header protection, decryptable only inside quiche with the HP keys. Not gateway-tractable.

### G (#11–25) — hand-rolled H3 layer enforces no RFC 9114/9204 error conditions
**Measured** (case #13): gateway log → `WARN "no backends available for H3 request"` — the malformed (missing-pseudo-header) request was **accepted and routed**, not rejected; no H3 error-code close. **Architecture** (citations in `audit/h3spec/s22-report.md`):
- `H3Request::from_headers` (`h3_bridge.rs:756`) silently defaults missing pseudo-headers, last-wins on duplicates, pushes unknown pseudo-headers to `extra`, no ordering check.
- Client control stream / SETTINGS are **not read or tracked** (`h3_bridge.rs:395` skips non-HEADERS frames).
- QPACK is static-table-only; encoder/decoder unidirectional streams are not monitored.
- On any H3/QPACK decode error the conn_actor **logs + drops the stream** (`conn_actor.rs:1064`, `:1203`) — it does **not** call `conn.close(false, <H3 code>, …)`.
**Crucial enabler difference from T1:** by the time H3 frames flow the connection is **established** (`recv_count > 0`), so a gateway-initiated `conn.close(false, <code>, …)` **will** be emitted on the wire (the T1 suppression does not apply). The fix is therefore tractable in gateway code.

---

## Tier rollup & disposition

| Tier | Count | Findings | Disposition |
|---|---|---|---|
| **T1** quiche close-suppress | 8 | #1–8 | **ESCALATE** (R6 large / R7). quiche-library; options below. quiche validates correctly — only the close packet is suppressed on first-packet errors. |
| **T2** quiche no-validate | 2 | #9–10 | **ESCALATE / document**. quiche gap; not gateway-inspectable (header protection). Low severity (reserved-bit robustness). |
| **SEC** request integrity | 4 | #12–15 | **FIX this session** (pre-authorized R7). Pseudo-header validation → H3_MESSAGE_ERROR. Smuggling/desync-adjacent: a malformed request is currently forwarded to the backend. |
| **COR** H3/QPACK conformance | 11 | #11, #16–25 | **FIX as budget allows** (pre-authorized R7); tier/carry remainder with mechanism (exit (b)). Needs: error-code close enabler, request-stream frame-sequencing, control-stream state machine, SETTINGS validation, QPACK error-code mapping + uni-stream monitoring. |

### R7 OWNER ESCALATION — T1/T2 (quiche 0.28 limitations)
Both are in **quiche 0.28.0**, not gateway code. Options:
1. **Document as a known v1 conformance limitation tied to quiche 0.28** (no code change). Honest; quiche *does* detect and tear down the bad connection (the client cannot proceed) — only the explicit close *frame* is missing for first-packet transport errors / reserved bits.
2. **Evaluate a quiche upgrade** (does a newer quiche emit the close / add reserved-bit checks?). Large: touches the whole QUIC stack (Mode A/B + H3-front), needs full re-validation + re-soak. Aligns with carry-forward **CF-DEP-1**.
3. **Fork/patch quiche** (change `close()`/`send()` guard; add reserved-bit check). Maintenance burden; not recommended for v1.
**Recommendation:** (1) for this session + open a tracked item to evaluate (2) — do **not** fork. Security impact is low: these are unauthenticated/handshake-phase or robustness checks; the connection does not succeed either way.

**OWNER RULING (2026-05-31): Option 1 — document as a v1 limitation.** quiche DOES tear down the bad connection (the security-relevant outcome happens); only the explicit CONNECTION_CLOSE *frame* is suppressed on a first-packet error, and reserved bits are header-protected (not gateway-inspectable). A quiche-upgrade evaluation is opened as its **own** carry-forward **CF-QUICHE-UPGRADE** — kept DISTINCT from CF-DEP-1 (the separate Dependabot owner task). Do not fork. Do not bolt a foundational dep bump onto a conformance session.

> **CF-QUICHE-UPGRADE** (new carry-forward, S22): evaluate a quiche bump (> 0.28.0) as its own future workstream to recover h3spec #1–10 (first-packet transport-param CONNECTION_CLOSE emission + header reserved-bit validation). Large: touches Mode A/B + H3-front + termination; requires full ×3 re-validation + re-soak. NOT CF-DEP-1.

---

## CF-IGN-1 disposition (14 real `#[ignore]` attrs; "16" in memory drifted to 14)

| Test(s) | Reason | Disposition |
|---|---|---|
| `tests/h2spec_server_conformance.rs` (1) | "h2spec binary not provisioned" | **STALE → FIX-AND-ENABLE.** h2spec IS on PATH; gateway passes **146/147 (1 skip, 0 fail)**. Implement the real shell-out + un-`#[ignore]`. |
| `tests/s14_cfbw_h1h1.rs` (2) | CF-CFBW-CELL-LIVENESS-TEST-FRAGILITY | **KEEP, documented.** Cell-level hyper H1 response-flush timing; the property is proven at the helper level (`idle_send::tests::arm_ii/iii/ix`). Valid, not obsolete. Not h3-relevant. |
| `crates/lb-l4-xdp/tests/*` (11) | CAP_BPF / privileged / dummy netdev | **KEEP, documented.** Genuinely privileged (real BPF_PROG_LOAD / XDP attach / bpffs); run only in the privileged CI lane (F-ESC-1 multi-kernel lane carry-forward). Valid. |

---

## Phase 2 plan (gateway fixes; single-sourced per R12)

## Phase 2 RESULTS — security tier (#12–15) CLOSED

All four pseudo-header findings now pass h3spec (`4 examples, 0 failures`), independently verified (verifier-e1 AGREE; `audit/h3spec/s22-verify-e1.md`):

| # | Result | How |
|---|---|---|
| 12 | ✅ PASS | `validate_request_pseudo_headers` rejects duplicate pseudo → stream reset H3_MESSAGE_ERROR |
| 13 | ✅ PASS | **owner-ruled STRICT** — reject http/https request missing `:authority`/`Host` (RFC 9114 §4.3.1). Prior SNI-substitution tolerance was coverage-only lenience (S7 `absent_authority_substitutes_sni`, added for +15 cov lines), NOT a deployment feature → made conformant; the one `omit_authority` e2e test was converted to assert the rejection. Upstream SNI fallback retained for H1→H3/H2→H3 (different ingress). |
| 14 | ✅ PASS | prohibited/unknown pseudo (`:autority`/`:foo`) rejected — unblocked by the QPACK fix below |
| 15 | ✅ PASS | pseudo-after-field (rejected at the unknown-pseudo `:autority` in h3spec's vector; the §4.3 ordering arm is separately unit-tested + proven load-bearing) |

### New finding discovered: F-S22-QPACK — non-conformant QPACK Literal-Literal-Name (RFC 9204 §4.5.6)
NOT one of the original 25 (h3spec is a client, doesn't test the server's response QPACK encoding), but found while fixing #14/#15. The lb-h3 codec wrote/read a **separate 7-bit length byte** for the literal NAME instead of the **first-byte 3-bit name-length prefix**. Encoder and decoder were **self-consistent** (so internal round-trips + h2spec passed) but **both non-conformant** — every conformant peer (h3spec, browsers) mis-decodes the gateway's literal-named response headers, and the gateway mis-decodes theirs. **Fixed both directions together** (so internal round-trips stay consistent AND interop is correct); 3 lb-h3 regression tests incl. a hand-built conformant-block decode. **Severity: real interop defect** (response-header corruption against conformant clients) — the most valuable thing this pass found.

## Phase 2 RESULTS — E2 frame-sequencing (#11, #21) CLOSED

| # | Result | How |
|---|---|---|
| 11 | ✅ PASS | DATA before HEADERS on a request stream → `H3_FRAME_UNEXPECTED` connection close (RFC 9114 §4.1) |
| 21 | ✅ PASS | CANCEL_PUSH (also SETTINGS/GOAWAY/MAX_PUSH_ID/PUSH_PROMISE) on a request stream → `H3_FRAME_UNEXPECTED` (§7.2). Reserved/grease types still ignored (§7.2.8) |

`StreamRxBuf::feed` now returns a `FeedError` enum: `Decode` (reset the stream, existing) vs `FrameUnexpected` (the caller closes the connection). 3 lb-h3-style unit tests + the live h3spec re-run.

**Measure-first lesson (recorded): application vs transport CONNECTION_CLOSE.** The first attempt closed with `conn.close(false, …)` — `app=false` = a *transport* CONNECTION_CLOSE (frame 0x1c), so h3spec saw the H3 code 0x105 in the *transport* error space and rejected it. H3/QPACK codes are *application* codes → must use `conn.close(true, …)` (frame 0x1d). The qlog showed `error_space: transport` — that's how it was caught. **All remaining connection-error findings (#16–20, #22–25) must use `app=true`.**

### Carry-forward opened
- **CF-S22-QPACK-HUFFMAN** — the codec is raw-only (no Huffman name/value coding); a conformant peer that Huffman-encodes a literal name/value is unsupported (pre-existing codec limitation, surfaced here). Browsers Huffman-encode → needed before real-browser H3. Tier: correctness, own workstream.

Committed: `1409eb76` (#12/#14/#15 + QPACK) and the #13 increment (this commit). Below: the original Phase-2 plan (E0/E1 done; E2–E4 per the adaptive budget).

---

**Enabler E0** — H3 protocol-error close helper: `reset_h3_stream(conn, sid, code)` (RESET_STREAM + STOP_SENDING; pumped by `drain_conn_send`; emitted because the H3 conn is established). Single source for every G finding.

1. **SEC (E1):** strict pseudo-header validation in the request HEADERS path — reject duplicate / missing-mandatory / prohibited / mis-ordered pseudo-headers → `H3_MESSAGE_ERROR` via E0 (#12–15). Negative control: a valid request still succeeds.
2. **COR (E2):** request-stream frame sequencing — DATA-before-HEADERS, CANCEL_PUSH on request stream → `H3_FRAME_UNEXPECTED` (#11, #21).
3. **COR (E3):** client control-stream state machine — first-frame-SETTINGS, reject 2nd SETTINGS, reject DATA/HEADERS on control, H2 SETTINGS ids, critical-stream-close (#16–20, #24).
4. **COR (E4):** QPACK error-code mapping (#22, #25) + encoder-stream capacity check (#23).
Each fix: plan-approved, author≠verifier, a wire/unit regression test locking it, AND the specific h3spec case re-run green. Budget-bounded per exit (b): SEC first, then E2→E3→E4; tier/carry whatever remains with mechanism.
