# SESSION 25 — CF-S22-QUICHE-H3-MIGRATION: E2 + delete lb-h3 + promote — REPORT

**Branch:** `feature/quiche-h3-migration-s23` · **Base tip:** `1c002f61` (S24 COMPLETE,
E1 migrated). **Main stays** `90915781` (S22-hardened stack) unless E1+E2+Phase-3 ALL
verify (R11; no half-migration promote).

## Phase 0 — baseline (behavioral reference) — COMPLETE
`cargo test --workspace --all-features` ×3 = **1451/0 / 1451/0 / 1451/0**, all exit 0,
deterministic allpass (completed logs `audit/s25-logs/phase0/baseline-run{1,2,3}.log`).
Matches the S24 reference exactly. The H3 wire tests are the no-regression safety net (R3).
Base tip confirmed `1c002f61` (NOT main); zero stray processes; disk monitored.

## INC-4 — E2 H3→H3 upstream CLIENT → quiche::h3 — IN PROGRESS (NOT green)

Migrated `stream_request_to_h3_upstream` (the shared connector for all three →H3 fronts:
H1→H3 `h1_proxy:2275`, H2→H3 `h2_proxy:2617`, H3→H3 `h3_to_h3_stream_resp:2930`) off the
hand-rolled `lb_h3` framing onto a `quiche::h3::Connection` **client**:
`with_transport` → `send_request` (returns the request `stream_id`) → `send_body` (raw
chunk bytes; quiche frames DATA — `encode_h3_data_frame` gone) → `poll` / `recv_body` →
`send_additional_headers(is_trailer=true)` for request trailers. Added
`build_client_h3_config` (symmetric to the server). KEEP-surface preserved: the
`H3RespOut` sink (`Wire`/`Decoded` + cap accounting) UNTOUCHED, the peek/413/502 pre-dial
paths, the bounded `body_rx`, `forward_req_trailers`, the F-S7-6 idle deadline, the single
park point, one-request-per-pooled-conn. `j2_req_event_action::SendData` now carries raw
bytes. **Deleted** the now-dead recv-parser (`parse_frame_header`, `check_block_len`,
`classify_recv_err`/`RecvErrClass`, `FRAME_DATA`/`FRAME_HEADERS`) + their moot unit tests.
lb-quic compiles, clippy `-D warnings` + fmt clean. (WIP commits `7b57ca68`, …)

### H3→H3 wire suite (BOTH migrated endpoints E1+E2) — 22/24 → fixes in flight
PASS (the load-bearing surface): **F-MD-4 MIRROR** single-shot
(`_upstream_reset_midbody_resets_client_no_fin` — backend reset → client reset, no FIN),
BOTH **R8 gauges** (`_response_memory_bounded_through_stalled_client`,
`_request_memory_bounded_through_stalled_backend` — 4 MiB bodies, flat ≤ ceiling,
non-vacuous), **backpressure** (`_backpressure_stalled_client_pauses_upstream_read`),
byte-identity (GET/req-body/full-head), response trailers + pseudo-trailer reject,
request-leg F-MD-4 (`_client_reset_midrequest_*`), and ALL malformed-backend cases now
**quiche-owned** (`_data_before_headers`, `_invalid_qpack`, `_oversized_*`,
`_unknown_response_frame_skipped`) — PASS.

Two initial failures, both rooted in **quiche-0.28 recv-path capability gaps** (NOT bugs
in the migration; confirmed from quiche source + a ground-truth event trace). NEITHER
backend mode sends `content-length`.

1. **`_empty_data_frame_skipped_then_body`** (backend: head + empty DATA(0) + real body
   + FIN). quiche-0.28 does **not re-arm** the `Data` event after a 0-length DATA frame
   while the stream stays readable (`stream.rs::try_consume_data` only `reset_data_event`s
   when `!stream_readable`); `poll` advances the real DATA frame into `State::Data` WITHOUT
   emitting a fresh `Data` event, and — the actual stall — after a post-poll drain relays
   that body and finishes the stream, the `Finished` event sits queued needing no socket
   I/O while the loop **parks on the socket** → 25 s timeout. **FIXED:** (a) an
   unconditional post-poll **PASS-3** `recv_body` drain (server `poll_h3` PASS-1 analogue)
   relays the stranded body; (b) a per-tick `progressed` flag → `continue 'evloop`
   (re-poll instead of park) so the queued `Finished`/trailer event is collected without a
   timeout. **PASSES** (run5/run6).

2. **`_upstream_premature_eof_mid_data_no_clean_fin`** (backend: head, NO content-length, +
   a DATA frame header declaring 4096 bytes but only 16 bytes of payload, then a clean QUIC
   FIN). The hand-rolled parser caught this via H3-frame-completeness (`InData{remaining>0}`
   at FIN ⇒ PrematureEof). **quiche-0.28 does NOT enforce DATA-frame completeness at FIN
   (RFC 9114 §7.1)**: `process_finished_stream` (`mod.rs:2845`) pushes the stream to
   `finished_streams` regardless of an incomplete `State::Data`, `poll` returns a clean
   `Event::Finished` (both pops re-check only for *reset*), `recv_body` discards the `fin`
   flag, and there is **no public quiche API** to observe "finished mid-frame". This is a
   genuine `quiche::h3`-client cannot-express-a-KEEP-surface-need → **ESCALATED (R7/exit-d)
   before deleting hand-rolled code.** Otherwise green: h3h3 23/24, h1h3+h2h3 25/25, R8 (both
   directions), F-MD-4 mirror, backpressure all pass.

### OWNER RULING — Option 1 (accept + content-length defense-in-depth + re-scope) — DONE
The gap requires a malformed/buggy BACKEND (trusted upstream, not an untrusted client);
HTTP/3 streams are isolated so truncation CANNOT desync/smuggle (the structural reason the
S22 #12-15 findings were security-critical does NOT exist here); RESET-based truncation is
still caught (F-MD-4 mirror, all 3 →H3 cells); content-length responses self-detect. ⇒ a
LOW-severity RFC 9114 §7.1 robustness gap, not a security hole. Implemented:
- **Content-length truncation guard** (`h3_bridge.rs`, migrated E2): on a clean `Finished`,
  if a declared `content-length` is present and `body_relayed < content-length` (and the
  response is not bodiless — HEAD / 1xx / 204 / 304), RESET downstream (`PrematureEof`),
  never a clean End. Recovers the COMMON truncation case. **Proven load-bearing** by a NEW
  test `h3h3_e2e_content_length_truncation_resets_no_clean_complete` (+ `HeadCLThen
  TruncatedData` backend: declares 4096, sends 16, clean FIN → gateway resets).
- **Re-scoped** the ONE no-content-length test →
  `_no_cl_truncated_data_delivered_quiche_028_frame_completeness_gap`: asserts the migrated
  documented behaviour, with the §7.1 gap + threat model + compensating guard + the
  **CF-QUICHE-FRAME-COMPLETENESS** carry-forward (tied to CF-QUICHE-UPGRADE: re-tighten when
  a quiche version enforces §7.1) named in the doc. A documented, compensated re-scope —
  NOT a silent weakening.
- The §7.1 gap is documented as a known quiche-0.28 limitation alongside #1-10 + the v1
  release-note item (see §Carry-forward / release notes), and the CL guard is independently
  verifier-confirmed to FIRE (author ≠ verifier).

**INC-4 wire state:** h3h3 **26/26** (incl. the re-scope, the CL-guard test, the empty-DATA
fix, the F-MD-4 mirror burst R13 b+c); h1h3+h2h3 verify **25/25**; lb-quic lib **84/84**;
clippy `-D` + fmt clean.

### Independent verification (author ≠ verifier) — AGREE
A fresh-context verifier READ the migrated `stream_request_to_h3_upstream` in full + the
`drain_resp_body!` macro + the `H3RespOut` sink and confirmed, with `file:line` evidence:
(1) R8 RESPONSE per-chunk into a fixed 8 KiB scratch, **no whole-body `Vec`/`.collect()`/
`.extend`** (adversarially grep-confirmed — the only `.collect()`s are header field-lists),
`on_data().await` is the backpressure point; (2) R8 REQUEST retains the in-hand chunk on
`Done`, fills the depth-8 channel; (3) F-MD-4 mirror maps ALL THREE reset surfaces
(`Event::Reset`, mid-body `recv_body` error, `Finished`-on-reset `was_reset` probe) to
`UpstreamReset`, and `on_end` is reachable ONLY when `response_complete==true` (set only on
a clean `Finished` with `sent_head && !was_reset && !cl_truncated`); (4) the CL guard never
falsely resets a complete CL response and never misses a truncated one, correctly scoped
past HEAD/1xx/204/304; (5) the empty-DATA re-poll is progress-gated + finite (no spin) and
does not bypass backpressure; (6) error contract (`set_reusable(false)` everywhere, 413
pre-dial / 502 dial-failure, case-7 `H3_REQUEST_CANCELLED` no-FIN aborts). **No path found
where a truncated/reset upstream reaches the client as a clean complete response, and no
whole-buffering path.** Verdict: **AGREE**.

### Mutation proof — the CL guard is LOAD-BEARING (owner-required)
With the guard disabled (`cl_truncated = false`), `h3h3_e2e_content_length_truncation_
resets_no_clean_complete` **FAILS** (quiche's clean `Finished` delivers the declared-4096 /
sent-16 response as a clean complete 200+FIN); with the guard, it **PASSES**. So the
content-length defense-in-depth genuinely fires — proven, not asserted (`mutA-clguard.log`;
tree reverted clean). The F-MD-4 mirror has defense-in-depth (Event::Reset + recv_body-error
+ Finished-`was_reset` probe + idle-timeout fallback + the CL guard), so no single mutation
cleanly fails a test; it is verified by the verifier code-read (all 3 reset surfaces →
never `on_end`) + the single-shot + the R13(b)/(c) mirror burst, all green.

### Full-workspace regression (R3) — proto_translation H3 backend (test-harness, fixed)
The first full-workspace gate caught `proxy_h1/h2_listener_h3_backend` → 502. Root cause
(backend trace): the migrated quiche::h3 client correctly prepends a **GREASE** frame
(`0x1f*N+0x21`, RFC 9114 §7.2.8) on the request stream; the minimal `spawn_h3_static_backend`
only checked whether the FIRST frame was HEADERS, so it never responded → E2 idle-timeout →
502. The old hand-rolled E2 sent no GREASE, masking the backend's non-conformance. **Fix
(test-harness only, no production change):** the backend now skips leading GREASE/unknown/
non-HEADERS frames (RFC 9114 §9) to find the request HEADERS — exactly as a conformant H3
server does. `proto_translation_e2e` 5/5 green (commit `d26abe68`).

### Second independent verifier (R8 + F-MD-4 + CL-guard + GREASE-fix) — AGREE
A second fresh-context verifier adversarially cross-checked: (1) R8 — classified EVERY
`Vec`/`.collect()`/`.extend`/`push` in the function: all are header/trailer field-lists or
fixed UDP datagram buffers, **never the body** — neither request nor response body can be
whole-buffered; `on_data().await`→bounded-mpsc `tx.send().await` is the genuine backpressure
point (read in the sink, not the gauge). (2) F-MD-4 — `response_complete` has ONE writer,
`on_end()` ONE call-site gated on it; the ONLY truncated-reaches-downstream path is the
documented no-CL §7.1 residual (bounded by H3 stream isolation). (3) CL-guard mutation
(`mutA`) load-bearing confirmed. (4) The GREASE fix is legitimate — one test file, no
production change, no weakened assertion — and cross-confirmed that the h3h3 backend ALREADY
tolerates unknown frames (`h3_h3_stream_e2e.rs:834` `Ok((_other,c)) => rx_tail.drain(..c)`),
which is why h3h3 passed 26/26 without the fix. **Verdict: AGREE.** Two independent verifies
AGREE on the R8 + F-MD-4 + CL-guard surface (S24 two-verifier pattern + owner requirement).

### INC-4 final full-workspace gate (R3 no-regression)
First full gate caught the proto + `quic_listener_e2e` H3-backend GREASE flakiness (3
hand-rolled H3-backend harnesses lacked the RFC 9114 §9 unknown-frame skip; fixed —
`quic_listener_e2e` 6/6 ×3 + proto 5/5 deterministic). With those fixed the only
failures workspace-wide were resolved; the binding ×3 determinism gate runs in INC-5.

## INC-5 (PRODUCTION-ONLY, owner Option 3) — production is now 100% on quiche::h3

Owner ruled (Option 3): do the BOUNDED production half now; defer the ~20 hand-rolled
wire-test-harness rewrite + the lb-h3 CRATE deletion + Phase-3 to S26 (so the expensive
test rewrite is done carefully, not rushed; main never sees a half-migration; R11 — lb-h3
crate not fully deleted ⇒ NO promote).

**INC-5a (`b6eb93cc`) — inline-400 → decoded egress + legacy raw-byte path deleted.**
The H3 `:authority`-reject inline 400 now emits a DECODED `Head{400}+Body("bad request")
+End` onto the bounded `RespEvent` channel + a `Progressive` `StreamTx` — the SAME
`quiche::h3` `send_response`/`send_body` egress every cell uses since INC-3 (same 400 +
body, byte-for-byte). With its sole producer migrated, the legacy raw-byte path is gone:
`request_tasks` + the `task_wait` future + the `finished` select arm + `StreamTx::Buffered`
+ `StreamTx::new` + the Buffered drain/finished arms. `Progressive` is the SOLE egress.

**INC-5b (`5174a018`) — delete dead lb_h3 framing (~2461 LOC); lb-h3 → dev-dependency.**
With the data path on `quiche::h3`, the hand-rolled `lb_h3` framing is dead in production
and deleted: the 5 `encode_h3_*` encoders, `request_h3_upstream` + `H3UpstreamResponse`,
the dead H1-response read chain (`read_h1_response*`/`H1Response`), `build_h1_request`,
`h3_to_h1_roundtrip`, `h3_to_h1_stream`, the top-level `use lb_h3` import, all moot unit
tests (h3_bridge −950 LOC); whole files `inc1_quiche_h3_experiment.rs` (S23 throwaway) +
`h3_h1_binary_body_e2e.rs`; the `h3_to_h1_stream` cases in `h3_h1_stream_body{,_errors}_e2e`;
the `lib.rs` re-exports; the `H3UpstreamResponse` half of `lb-l7 trailer_passthrough`.
`crates/lb-quic/Cargo.toml`: `lb-h3` `[dependencies](optional)` → `[dev-dependencies]`,
`dep:lb-h3` removed from `quic-terminate`. **`grep 'use lb_h3|lb_h3::' crates/lb-quic/src`
(non-comment) = EMPTY — PRODUCTION IS lb_h3-FREE.** Cargo.lock unchanged; workspace
clippy `-D` + fmt clean.

**Two independent verifiers — SAFE DELETION, no coverage lost.** (1) No dangling code
reference to any of 13 deleted symbols; (2) all deleted fns had zero production caller;
(3) EVERY deleted test's behaviour is covered by a NAMED surviving live-path test (each
*stronger*, real-wire vs synthetic): F-CAP-1 → `h1h3/h2h3_fcap1_over_cap_upload_never_
complete` (real 66 MiB upload → live `conn_actor::drain_request_body`); `stream_h2_response`
framing → `g5_stream_h2_response_over_cap_arms_reset` (h3_h2_stream_e2e); binary-body
byte-identity → `t1_multi_data_frame_binary_body_forwarded_byte_identical` + h3_h1_resp_stream;
inc1 = throwaway (no replacement needed); trailer_passthrough kept the live `H3Request`
assertions; (4) production lb_h3-free + dev-dep correct.

---

## VERDICT: SESSION 25 PARTIAL — H3 data path FULLY migrated to quiche::h3 + production
## lb_h3-free; full lb-h3 crate deletion + Phase-3 + PROMOTE → S26

**Landed (verified):**
- **E2 (H3→H3 upstream client) migrated to `quiche::h3`** — with the S24 E1 server front,
  BOTH H3 endpoints + all three →H3 cells (H1/H2/H3→H3) now ride `quiche::h3::Connection`.
  Two independent verifiers AGREE; R8 (both directions, no buffering trap), F-MD-4 mirror
  (single-shot + R13 b/c burst), backpressure, byte-identity all green.
- **The quiche-0.28 RFC 9114 §7.1 frame-completeness gap** handled per owner ruling: a
  content-length truncation guard (mutation-proven load-bearing), the one no-CL test
  re-scoped (documented, not weakened) + `CF-QUICHE-FRAME-COMPLETENESS` carry-forward.
- **PRODUCTION is 100% on `quiche::h3`** — the inline-400 modernised, the legacy raw-byte
  egress deleted, ~2461 LOC of dead `lb_h3` framing deleted, `lb-h3` demoted to dev-dep.
- 3 test-harness RFC 9114 §9 GREASE-conformance regressions fixed + deterministic.
- Wire safety net green: h3h3 26/26, h1h3+h2h3 25/25, proto 5/5, quic_listener ×3, lib 84/84.

**NOT done → S26 (honest PARTIAL; NOT promoted, R11; main keeps the S22-hardened stack):**
1. **Rewrite the ~20 hand-rolled wire-test harnesses off `lb_h3`'s frame codec** (quiche
   exposes `quiche::h3::qpack::{Encoder,Decoder}` but NO standalone H3 *frame* codec, so
   the harnesses that hand-build/parse DATA/HEADERS frames + varints need a replacement).
   **Recommendation (lighter of the two):** add a small shared `tests/h3_test_codec` support
   module (move `lb_h3`'s `frame.rs` + `varint.rs`, ~480 LOC, there) + switch QPACK to
   `quiche::h3::qpack`; re-point the ~20 imports. Avoid the full quiche::h3 rewrite of each
   harness unless interop fidelity demands it.
2. **Delete the `crates/lb-h3` crate** + drop the dev-dependency once (1) is done.
3. **Phase-3 full re-validation:** ×3 deterministic; R8 E1+E2 + R13 F-MD-4 E1+E2 re-proven;
   the **FRESH h3spec run** proving the 9 carried findings #16-21/#23-25 PASS by construction
   (#11-15/#22 still pass) — the headline payoff still unproven by external reference;
   **re-soak with a NEW H3-terminate scenario** (lb-soak lacks one; this workstream
   re-pointed exactly that path); scoped llvm-cov ≥80%.
4. **PROMOTE** (`--no-ff`) — only when (1)-(3) are all green and `lb-h3` is actually deleted.

CF-S22-QPACK-HUFFMAN: the migrated egress/ingress QPACK-Huffman-encode via `quiche::h3`
(the hand-rolled `lb_h3::qpack` was raw-only) — gained in production; confirm the wire
bit on the fresh h3spec run.

**Carry-forwards (new this session):** `CF-QUICHE-FRAME-COMPLETENESS` (re-tighten the no-CL
truncation test when a quiche ≥ version enforces RFC 9114 §7.1; tied to CF-QUICHE-UPGRADE)
— documented alongside #1-10 + a v1 release-note item (no-content-length backend mid-frame-
FIN truncation is relayed as complete; low severity: needs a malformed backend, H3 streams
are isolated so no desync/smuggling, content-length + reset-based truncation ARE caught).
