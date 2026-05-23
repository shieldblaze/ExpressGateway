# S7 J4 — Real-wire H3→H3 verification suite: FAIL report (on-branch only)

- Author: `builder-1` (worktree `/home/ubuntu/Code/s7-builder1`, branch `s7/builder-1`)
- Asset: `crates/lb-quic/tests/h3_h3_stream_e2e.rs`
- Status: **J4 ASSET COMPLETE & CORRECT; the gateway is DEFECTIVE.** 2/7 pass, 5/7 FAIL exposing **F-S7-2**. Committed on `s7/builder-1` ONLY as the reproduction artifact + eventual acceptance test. **MUST NOT reach `feature/h-matrix-s7` until green.**
- J4-G3 honored: a real-wire behaviour contradicting J1/J2/J3 was found; STOPPED, escalated, **NO src patch**, suite **NOT** `#[ignore]`'d / weakened.

---

## 1. Asset construction proof (J4-G1: genuine endpoint)

`h3_h3_stream_e2e.rs` mirrors the verified `h3_h2_stream_e2e.rs` 1:1
in shape (cert/listener/`drive_h3` client harness reused verbatim;
`retained_ceiling` = `4×depth×(chunk+hdr)` copied; gauges = the
feature-gated `lb_quic::h3_bridge::MAX_RETAINED_{RESP,BODY}_BYTES`
statics; invalid-gate-by-design preserved). The ONLY structural delta
vs the H2 template:

- `start_h3_listener_h3` uses the public
  `QuicListenerParams::with_h3_backend(QuicUpstreamPool, addr, sni)`
  (not `.with_h2_backend`).
- The upstream is an **inline genuine `quiche::accept` H3 endpoint**
  (Q-J4a = inline, lead-ruled): binds a real `tokio::net::UdpSocket`,
  `quiche::Header::from_slice` + `quiche::accept`, real handshake
  pump (`conn.send`/`recv`/`timeout`), per-stream `lb_h3`
  `decode_frame`/`QpackDecoder` request capture +
  `encode_frame`/`QpackEncoder` response. **NOTHING below quiche is
  hand-rolled; NO in-process stub.** Extends the proven
  `round8_h3_authority_enforced.rs:215-266` /
  `h3_graceful_close.rs:188-215` real-`quiche::accept` pattern.

`h3_to_h3_stream_resp` is **never called directly** (crate-internal,
J3 ruling) — the suite drives ONLY the public
`QuicListener`/`QuicUpstreamPool` surface, so every result is a
genuine front-listener → router → bridge → real-backend wire result
(J4-G3 path satisfied).

The asset compiles clean under
`cargo test -p lb-quic --features test-gauges --test h3_h3_stream_e2e`;
all isolation probes were stripped before commit; **no `#[ignore]`
anywhere** (the only `#[ignore]` token in the file is the module
doc-comment sentence "No `#[ignore]` anywhere").

## 2. Per-case result (deterministic; `--test-threads=2`, 10.30s)

| # | Case | Result | Why |
|---|---|---|---|
| 1 | `h3h3_e2e_get_response_byte_identical` | **FAIL** | bodyless GET — client RESET, `status=None` (F-S7-2) |
| 2 | `h3h3_e2e_request_body_byte_identical_at_backend` | **FAIL** | F-S7-2 (also carries the trailers-dropped assertion) |
| 3 | `h3h3_e2e_response_memory_bounded_through_stalled_client` | **FAIL** | F-S7-2 |
| 4 | `h3h3_e2e_request_memory_bounded_through_stalled_backend` | **FAIL** | F-S7-2 |
| 5 | `h3h3_e2e_backpressure_stalled_client_pauses_upstream_read` | **FAIL** | F-S7-2 |
| 6 | `h3h3_e2e_upstream_reset_midbody_resets_client_no_fin` | PASS | only asserts the *absence* of a clean complete 200 — holds vacuously while F-S7-2 resets the client |
| 7 | `h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request` | PASS | likewise asserts the *absence* of a clean 200 |

`test result: FAILED. 2 passed; 5 failed; 0 ignored; 0 measured`.

**Note on cases 6 & 7:** they PASS but **not meaningfully** under
F-S7-2 — they assert a negative ("no clean complete 200") which is
trivially true while every response is reset. They become *real*
proofs only once F-S7-2 is fixed and cases 1-5 are green. They are
NOT evidence the cell works.

## 3. Isolation chain (F-S7-2 localization)

Instrumented the inline upstream + the front-client `drive_h3` with
temporary probes (since stripped). For the SIMPLEST case (case 1,
bodyless GET) the deterministic trace was:

1. `upstream ESTABLISHED +1.9ms (pool handshake OK)` — the gateway's
   `QuicUpstreamPool::acquire` **successfully handshakes** to the
   genuine quiche upstream. Pool / dial / harness wiring is CORRECT;
   it is **not** the failure.
2. `upstream response_started req_fin=true body_len=0 hframes=1` —
   the upstream received the **clean bodyless request** from the
   gateway: exactly ONE HEADERS frame, clean FIN. So
   `h3_to_h3_stream_resp`'s J2 request-send half worked on the wire
   (HEADERS+FIN reached the real upstream).
3. `upstream resp push off=15/15 fin_target=true` — the upstream
   transmitted a **complete, well-formed** HEADERS + DATA(`h3-empty`)
   + FIN `200` response.
4. `drive_h3 exit ... status=None fin=false reset=true
   elapsed_left=24.95s` — the gateway **RESET the front client at
   ~43 ms** without delivering the upstream's valid response.
   `status=None` ⇒ no HEADERS decoded by the client. `reset=true` ⇒
   the gateway sent RESET_STREAM on the client stream. NOT a `502`
   inline ⇒ not a pool/dial error path. ~43 ms ≪ the 5 s recv
   deadline ⇒ an **explicit early `Err(RespAbort::*)`**, not a
   timeout.

**Net:** gateway dials a genuine upstream (OK) → sends it a valid
bodyless request (OK) → upstream replies with a valid complete `200`
(OK) → `h3_to_h3_stream_resp`'s **recv-half** fails to relay it and
the actor RESET_STREAMs the client. The defect is localized to the
gateway's H3→H3 recv-half (J1) when relaying a real pooled-upstream
response — a path no prior test covered (see §4).

Deep mechanism diagnosis is **deliberately NOT done here** — F-S7-2
diagnosis is `verifier`'s (R5: independent of the J1 recv-half
author). This report localizes; it does not root-cause.

## 4. F-S7-3 (the verification gap that hid this)

`round8_h3_authority_enforced.rs:326` sets `h3_backend: None` — it
exercises the H3 authority gate (comma-in-`:authority` → inline
`400`), it **never dials an H3 upstream**. My J3-done report's claim
that round8 (3/3) proved the live H3→H3 path post-rewire was
therefore **unfounded** — round8 cannot exercise
`h3_to_h3_stream_resp` at all. J4 is genuinely the **first** real-wire
exercise of that code (consistent with the J4 plan's "first
`with_h3_backend`/`QuicUpstreamPool` test"). The J3 swap-parity
result (`h3_h2_stream_e2e` 10/10 — H3→H2 + shared tail unregressed)
and the H1-branch-untouched result still hold; only the
"round8 ⇒ live H3→H3 proven" inference was wrong. Recorded as
**F-S7-3** (shared gap: the s6 plan §4, the s7 reconfirm, the J3
self-check, AND the independent J3 audit all carried it);
verifier-owned to independently confirm/characterize.

## 5. Disposition

- Asset committed on `s7/builder-1` ONLY. **Never integrate to
  `feature/h-matrix-s7` until 7/7 green.**
- `builder-1` HOLDS: no diagnose-deep, no src edit, no harness
  hill-climbing. F-S7-2 mechanism diagnosis = `verifier` (R5
  independence). F-S7-3 confirm/characterize = `verifier`.
- A src fix, IF authorized, is a NEW gated increment with
  plan-approval AFTER verifier's proven mechanism (R2) + lead
  severity classification (R6); fix ownership decided then; verifier
  independently verifies any fix against this genuine J4 suite (never
  inference).
