# SESSION 12 — H-to-H Matrix Closer — Report

Base: main @ `5812eb59` (S11 promoted). Branch: `feature/h-matrix-s12`.
Team: h-matrix-s12 (lead, refactor-eng, builder-1/2, verifier).
Mission: M-C extraction (CF-DEDUP-2) → H1→H3 → H2→H3 (honest-stop gated). Trailer mandate in scope for both →H3 cells.

> Status legend: ✅ done · ⏳ in progress · ⛔ blocked · 🔜 pending

---

## Phase 0 — baseline + environment hygiene

- Base tip confirmed `5812eb59`; branch `feature/h-matrix-s12` cut from it.
- `ps aux`: no stray cargo/rustc/eg processes surviving S11.
- Disk: 26GB→24GB free during baseline (target dir 27G: 24G debug + 3G llvm-cov). Below the 25GB self-floor; monitoring, surgical reclaim of idle llvm-cov-target available if needed (CF-DISK-1).
- Shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` used for all builds.

### R1 ×3 baseline (initial)
| Run | Result | Notes |
|---|---|---|
| #1 | ✅ all pass | 210 suite summaries `ok`, 0 failed, 6:37 wall |
| #2 | ❌ 1 failed | `fcap1_h2_over_cap_upload_yields_413` 504≠413 |
| #3 | ✅ all pass | `fcap1_h2_over_cap_upload_yields_413 ... ok` |
| clippy | ✅ exit 0 | `--all-targets --all-features -D warnings` |
| fmt | ✅ exit 0 | `--check` |

**Not green on first ×3 → R3 regression in a promoted cell. Classified per R2, fixed before Phase 0a (see below). Re-confirmation pending.**

### F-CAP-1 H2→H1 saturation flake (R2 classification + R3 fix)
- **Test:** `fcap1_h2_over_cap_upload_yields_413` (tests/h2h1_md_streaming_verify.rs:1747), promoted S8/S9 H2→H1 cell.
- **Evidence (run #2):** `status=Some(504) written=2162688 backend_body_bytes=2165000`, 30.05s.
- **Mechanism (proven, R2):** test pushes 66 MiB to trip the hardcoded 64 MiB `MAX_REQUEST_BODY_BYTES` (h2_proxy.rs const) and expects 413. The H2-ingress→H1-backend path's `timeouts.body` (default 30s, h1_proxy.rs:214) is a **wall-clock** bound wrapping the whole `send_request` future (h2_proxy.rs:1836-1837); elapsed arm → `ProxyErr::Timeout` (h2_proxy.rs:1867-1871) → 504 (caller :1226). Under 8-core gate saturation the client pushed only 2.06 MiB ≪ 64 MiB before the 30s wall-clock fired, so the Timeout→504 beat the cap-trip→413. Because `written ≪ cap`, the cap was genuinely never reached → **504 is correct product behavior for a stalled upload**; this is a TEST-HARNESS fragility (CF-SATURATION-1), not a product defect. 1/3 flake rate; passes in isolation and in runs #1/#3.
- **Disposition:** R3 fix (not asterisked, not weakened) — gateway listener body timeout raised to 120s for THIS test (other 17 callers keep `HttpTimeouts::default()`), client wait 60s→130s, stale ":1779 60s budget" comment corrected. Assertion `Some(413)` unchanged. Owner: builder-1. **Commit `fe992654`** (pushed). Isolation PASS: `status=413 written=67239936 > 64MiB` → cap genuinely tripped, 8.53s.
- **Validation (load-bearing matched negative control):** pre-fix (30s default) run #2 **FAILED** 504 under full-gate 8-core saturation; **post-fix (120s) R1 ×3 all PASS** under the identical saturating context (`fcap1 ... ok` ×3, 0 failed suites, exit 0 ×3). Same context, old fails / new passes — the negative control is satisfied by real gate data, no synthetic-load experiment needed.

### R1 ×3 baseline (post-fix `fe992654`) — GREEN ✅
| Run | Exit | Failed suites | fcap1 |
|---|---|---|---|
| #1 | 0 | 0 | ok |
| #2 | 0 | 0 | ok |
| #3 | 0 | 0 | ok |

clippy `-D warnings` clean, fmt `--check` clean (pre-fix; only a test file changed since, fmt/clippy re-confirmed clean on it by builder-1). **Phase 0 GREEN — baseline established.**

---

## ESCALATION — CF-BODY-WALLCLOCK (owner-decided: defer to S13)

Surfaced while diagnosing the F-CAP-1 flake; **distinct from** the test fix above. Escalated to owner per R7 (real product-behavior decision + cross-cutting scope); owner chose **mitigate-now + escalate-to-S13** (Option 1). Logged here with full rigor per the owner instruction — NOT asterisked.

- **Proven mechanism:** H1/H2 proxy `timeouts.body` is a **wall-clock** timeout wrapping the entire upstream send future — `tokio::time::timeout(self.timeouts.body, sender.send_request(req))` at **h2_proxy.rs:1836-1837** (also h2_proxy.rs:1519; h1_proxy.rs:1497/1512/1896). For a backend that reads the full request body before responding, this bounds the **entire upload time**, so a slow-but-progressing large upload is 504'd at the deadline (default 30s) even with zero stalling. Directly parallel to the H3 **F-S7-6** issue (an 8 MiB H3 response was truncated at a 5s wall-clock; fixed by converting to an idle/no-progress deadline `H3_RESP_IDLE_TIMEOUT`). CF-S7-RHU tracks the H3 buffering connector's wall-clock twin.
- **Cell scope (R12):** 4 promoted cells — **H1→H1, H2→H1, H1→H2, H2→H2**. They share the wall-clock pattern and must be fixed together in S13. The →H3 cells use/inherit the H3 idle deadline and are not in this finding's scope.
- **Fix template:** mirror **F-S7-6** — replace the wall-clock `send_request` wrap with an idle deadline reset on forward progress (request-DATA egress and/or response ingress), per cell, with a regression test that a slow-but-progressing large upload completes (no false 504) while a genuinely dead upstream still terminates within the idle bound.
- **Effort estimate:** ~1–1.5 days. The pump/select structure differs between h1_proxy and h2_proxy ingress; each of the 4 cells needs the conversion + a slow-progress regression test + the existing F-CAP-1/F-MD-4 suites re-verified that the new idle deadline doesn't perturb cap-trip or abort semantics. Not tractable within S12's matrix-closing budget (R6 large-tier).
- **Known-limitation / operator note (must reach release notes):** This is a **CONFIGURABLE correctness gap with a workaround**, not a ships-broken defect and no security invariant is violated. Operators running large-upload workloads should set `body_timeout_ms` (config → main.rs:825) generously until S13 lands the idle-deadline fix. This belongs in operator-facing release notes when the matrix ships — surfaced, not buried.

---

## Phase 0a — M-C extraction (CF-DEDUP-2)
⏳ extraction in flight (commit 1). Design RECONCILED after builder-1's prep caught a gap in the initial one-param plan:

**RISK-1 (resolved):** `h3_to_h3_stream_resp`'s `resp_tx` emits PRE-ENCODED H3 wire bytes (`RespEvent::Bytes`) for an H3 *client* — unusable by an H1/H2 front, which needs DECODED status/headers/body/trailers. No streaming decoded-response producer existed (decode lived only in the buffering `request_h3_upstream`).

**Mechanism (lead ruling): A2** — shared async driver `stream_request_to_h3_upstream` parameterized by a per-front `H3RespSink` trait. The decode/re-encode are FUSED in one `'evloop` sharing `qconn_mut` + the single park point, so a request-leg-only extraction (B) can't de-fuse (degenerates to duplicating the whole driver), and a decoded-channel approach where H3→H3 re-consumes (A1) rewrites the promoted cell's response leg. A2 moves H3→H3's encode logic VERBATIM behind its sink → zero wire-byte change + R12 convergence achieved. H1/H2 sinks build decoded `H3RespEvent{Head/Body/Trailers/End/Reset}`.

**Final interface (as landed, commit `369c5e53`):** `pub async fn stream_request_to_h3_upstream(headers: Vec<(String,String)>, forward_req_trailers: bool, addr, sni, pool, body_rx: Receiver<ReqBodyEvent>, sink: H3RespOut) -> Result<(), RespAbort>` where `enum H3RespOut { Wire(Sender<RespEvent>) | Decoded(Sender<H3RespEvent>) }` (cap lives in the sink, not a separate param — lb-quic has no async-trait, so the per-front response handler is a monomorphic enum, not a trait). `h3_to_h3_stream_resp` is a thin wrapper calling the driver with `H3RespOut::Wire(resp_tx)` + `forward_req_trailers=false` → byte-identical. Request trailers ride in `ReqBodyEvent::End{trailers}` (caller-validated; connector does zero trailer validation). `forward_req_trailers=true` for H1/H2 (post-DATA HEADERS, net-new arm modeled on request_h3_upstream:2521-2543). The Decoded arm + forward arm are net-new pub API, NOT datapath-exercised until Phase 1 (unit-covered now). All re-exported at `lb_quic::` crate root. ONE refactor-only commit; conn_actor.rs, ALL tests, request_h3_upstream UNTOUCHED. C5 gate green: fmt clean, clippy `--workspace --all-targets --all-features -D warnings` clean, h3_h3_stream_e2e 20/20 (test-gauges), lib 26/26, h3_h1 16/16 + h3_h2 10/10. Effort honestly reset to a moderate refactor — tightens H2→H3 honest-stop budget. **Verifier (task #3) re-verifying byte-identical-equivalence now.**

**RISK-3 (resolved):** request-trailer forwarding caller-selectable (above); H1→H3 forwards (Phase 1 adds the missing live test), H3→H3 drops (byte-identical).

### Finding: CF-H3-HEAD (3-cell promoted defect; owner-ruled FIX ALL THREE this session)
The lossy response-head projection (`:status`+`content-length` only, all other non-pseudo headers dropped) exists in **ALL THREE H3-FRONT cells** via three code paths (refactor-eng, confirmed by read): H3→H3 (`on_head` Wire arm), **H3→H1** (`stream_h1_response` h3_bridge.rs:1289, live @ :2213), **H3→H2** (`stream_h2_response` :1599, live @ :2472) — each emits `encode_h3_headers_frame(status, len)`. Proven a defect (not design) by ASYMMETRY (trailer arms + buffering `request_h3_upstream` all preserve the full set). Coverage gap in all three suites (no regular-header round-trip assertion).
- **#10 (commit `bcb4f09a`) fixed H3→H3:** added `encode_h3_headers_frame_full(status, &[(name,val)])` (the status+CL helper kept for inline 413/502); Wire `on_head` now forwards the full non-pseudo set. Load-bearing proven (temp-revert → "content-type MUST round-trip… got [content-length]"); suite 22/22, lib 28/28, conn_actor untouched. **But #10-only created an R12 divergence** (H3→H3 fixed; H3→H1/H3→H2 still lossy).
- **Owner ruling (R7, Option A):** FIX ALL THREE this session — apply the SAME `encode_h3_headers_frame_full` helper (single-source) to `stream_h1_response` + `stream_h2_response` + a per-cell load-bearing round-trip test (temp-revert→fail). R12 goal: all 3 H3-front cells bit-for-bit-equivalent response-header behavior on the same input. (Owner chose this ~3h convergence over the ~10-13h H2→H3 build.)
- **#10/#11/trailer VERIFIED (verifier, independent, tip fd2622c1):** all three PASS, both load-bearing mutations confirmed. #10 mutation (revert on_head to lossy) → round-trip test FAILED ("content-type MUST round-trip… got [content-length]"). #11 (a5a72bdf) R13(b)-concurrency ACCEPTED (8-wide on one current_thread scheduler = the parallel-gate-masks-smuggle config; STRONGER than sequential — proof: the AbortNoFin→clean-FIN mutation failed at **iter 33/60**, not iter 0, surfacing timing a sequential test wouldn't); the AbortNoFin guard mutation → "SMUGGLING: mid-request-abort yielded clean 200+FIN". Trailer/head Decoded-arm unit asserts (fd2622c1) lib 28/28. conn_actor.rs untouched; tree clean post-mutations.
- **CF-H3-HEAD convergence LANDED (commit `76de7113`):** single-source `encode_h3_headers_frame_full` applied to H3→H1 (`stream_h1_response`) + H3→H2 (`stream_h2_response`); a mirrored `is_response_hop_by_hop` strip added to ALL 3 H3-front cells (incl. the H3→H3 Wire on_head). **RFC 9114 §4.2 conformance:** re-encoding to an H3 client MUST NOT carry connection-specific headers — so the strip is required, and it also patched a latent §4.2 gap in `bcb4f09a` (which forwarded the full set un-stripped). content-length forwarded as a regular header only for ContentLength framing (omitted for chunked/EOF, FIN-delimited on H3). Layering: only the 3 wire legs strip (the H1→H3/H2→H3 Decoded path's L7 front already strips RESPONSE_HOP_BY_HOP). Per-cell load-bearing proofs ran (temp-revert→FAIL "got [content-length]"); H3→H1 raw-socket test also proves `Connection: close` is stripped (two-sided). Gate: h3_h1 17/17, h3_h2 11/11, h3_h3 22/22, lib 28/28, clippy/fmt clean, conn_actor untouched. **R12 GOAL MET:** all 3 H3-front cells emit a byte-equivalent H3 HEADERS frame on the same decoded head. (H3→H2's strip not e2e-exercisable via hyper — RFC 9113 §8.2.2 forbids connection-class headers — shares `is_response_hop_by_hop` with the H3→H1 raw-socket proof.)
- **R12-convergence VERIFIED PASS (verifier, independent, tip `0c0727db`):** suites green (h3_h1 17/17, h3_h2 11/11, h3_h3 22/22, lib 28/28, H3→H1 collateral all green). Per-cell load-bearing temp-reverts FAIL→PASS for H3→H1 + H3→H2. **§4.2 strip proven load-bearing:** removing `is_response_hop_by_hop` in stream_h1_response → the H3→H1 test FAILS because `connection: close` LEAKS to the H3 client → restored PASS (asserts the strip, not just forwarding). **R12 bit-for-bit equivalence CONFIRMED** by reading all 3 head-encode paths — identical transform (`:status` + full-non-pseudo MINUS RESPONSE_HOP_BY_HOP, via the one `encode_h3_headers_frame_full`); same decoded head → byte-equivalent H3 HEADERS frame. Scoped llvm-cov **91.30%** (84/92; 100% on encode_h3_headers_frame_full / is_response_hop_by_hop / H3→H2 collect / H3→H3 strip; the 8 H3→H1 misses are pre-existing Reset/BadHead/Eof branches, not CF-H3-HEAD logic). conn_actor.rs untouched, tree clean. **CF-H3-HEAD fully BUILT + load-bearing across all 3 H3-front cells.**
- **Coverage caveat (verifier-flagged):** llvm-cov profiles can go STALE — a first run reported 0% on the new lines (they didn't exist in the prior instrumented build); `cargo llvm-cov clean` + re-run fixed it. The 91.30% (CF-H3-HEAD) was clean-profiled; the earlier H1→H3 90.23% measured lines that existed at measurement time (valid). Use a clean profile for any Phase 3 re-measure.

### Phase 0a verify (task #3) — PASS ✅
Independent verifier (author≠verifier): H3→H3 functionally-equivalent post-extraction `369c5e53`, no regression.
- Suites: h3_h3_stream_e2e 20/20 (test-gauges) · 17/17 no-gauge (3 gauge tests correctly absent) · F-MD-4/no-FIN family 5/5.
- **Pre/post code-read parity (5812eb59 vs 369c5e53):** Wire-sink reproduces the pre-extraction inline path by construction — verbatim field-list order, identical DATA-chunking loop (H3_RESP_CHUNK_MAX slicing, one frame/slice), same encode helpers (incl. the lossy status+CL head — PRESERVED), same cap accounting, same frame ordering, request-trailer drop. Zero wire-byte change. No DATA-boundary shift → ruling-2-(ii) fallback NOT triggered.
- Memory gauges re-confirmed post-channel-relocation (body-size-independent bound + load-bearing inverted probe, both legs). Backpressure green.
- F-MD-4 case-7 PASS (R13 (a) in-gate).

**Finding (R13 gap, promoted cell): CF-… H3→H3 case-7 lacks durable R13 (b)+(c).** The verifier found `h3h3_e2e_client_reset_midrequest_rsts_upstream_no_truncated_request` is a single-shot `#[tokio::test]` — no isolation-burst, no load-bearing negative control (parity gap vs H1→H2; the `parallel-gate-masks-smuggle` config). NOTE: my (lead) GO message wrongly asserted (b)+(c) existed; the verifier corrected it — recorded honestly. Verifier ran a ×12 isolation burst (12/12 PASS, real-wire) as interim proof of no active smuggle. Durable in-suite hardening filed as **task #11** (refactor-eng authors, verifier verifies), scheduled post-H1→H3-verify, budget-gated. NOT asterisked.

## Phase 1 — H1→H3
⏳ author-BUILT (builder-1); independent verification (task #5) IN PROGRESS.

**Author BUILT-bar (tests/h1h3_md_streaming_verify.rs, 8/8 ×3 no flake)** — commits bc8569ba (logic) + 51e04e29/0a6bc897/a4aaa7ac/c1bf78de/2247d655 (tests) + 0541930f (clippy):
- Binary byte-identical BOTH dirs (160KB, ~20× H3_BODY_CHUNK_MAX), complete=1.
- Request-trailer forwarding to H3 backend (saw_req_trailers=1; locks forward_req_trailers=true + the connector's net-new forward arm).
- F-MD-4 RESET-without-FIN, FULL R13: (a) in-gate; (b) 60-iter current_thread isolation-burst (≥50, not the iters=24 gap); (c) load-bearing (clean→complete / truncated→!complete) — request AND response-leg truncation (the H1PumpAbort→hyper-abort guard COMMIT A enables).
- F-CAP-1: over-cap→complete==0 (CF-SATURATION-1 120s listener timeout); upstream-down→502.
- Memory gauge body-size-independent (in_situ=8192 ≤ 4×64KiB ≪ 4MiB) + load-bearing inverted probe; backpressure both legs.
- **TRAILER-MANDATE EMPIRICAL: grpc_status_reaches_h1_client=FALSE** — connector PROPAGATES the trailer; the H1 front drops it (CASE-ii: hyper can't pre-declare Trailer: on a streamed head). Body + status DO reach the client. This is the documented H1-front limitation (parity with H1→H2), NOT a connector defect. → Owner escalation pending the verifier's independent confirm of the propagate-vs-drop split.
- fmt/clippy clean; R3 sanity post-widening: proto_translation_e2e 5/5, trailer_passthrough 8/8, lb-l7 lib 91/91 (verifier independently re-confirms incl. h2h2).

Verifier (task #5) independent bar: gate-zero wiring (drives streaming Decoded, not buffering), real-wire BUILT bar, R13 (c) MUTATION proof, trailer propagate-vs-drop split, scoped llvm-cov ≥80% session, widening R3 on H1→H1/H2→H1/H1→H2/**h2h2**, one full --workspace --all-features run.

Disk note: 21GB free (debug 29G); no surgical reclaim mid-verification (risk>benefit); aggressive reclaim before Phase 3 ×3.

### Phase 1 verify (task #5) — PASS (conditional on #13) ✅
Independent verifier verdict — H1→H3 meets the BUILT bar on every dimension tested:
- **Gate-zero wiring:** proxy_h1_to_h3 (h1_proxy.rs:1988-2296) drives the STREAMING `stream_request_to_h3_upstream` + `H3RespOut::Decoded`; the buffering `request_h3_upstream` is gone from the live path (only doc-comment mentions remain). Cited e2e drives the streaming branch.
- **Suite 8/8 ×3.** Byte-identical both legs (160KB→bumped to ~5MiB in #13), request-trailer forwarding, F-MD-4 request-leg R13 (a)+(b 60-iter current_thread)+(c), F-CAP-1 over-cap→complete==0 + upstream-down→502, memory gauge non-vacuous + inverted probe.
- **Scoped llvm-cov 90.23%** (231/256) session sub-metric over exact new-fn ranges (proxy_h1_to_h3 90.6%, build_h1_to_h3_fieldlist 94.1%, head-builder 79.2% [missed = benign pseudo/hop-by-hop/alt-svc filter branches no backend emits]). fmt/clippy clean.
- CONDITION: **#13** (response-leg truncation R13 test) — **LANDED commit `6cfd003f`** ("BUILT-bar #8 F-MD-4 RESPONSE-leg truncation R13 + 5MiB byte-identity"). Suite 10/10, no flake ×3. Clean arm → Ok(200, complete body) [non-vacuity]; truncating backend (declares CL=1MiB, sends 100KB partial, drops stream no-FIN) → head relayed then body-collect **Err** ("error reading a body from connection"), `false_complete=false` — the response-splitting guard holds. R13 (b) burst 60-iter current_thread all-incomplete. 5MiB byte-identity sent==echoed==5242880, complete=1. fmt/clippy clean. **Independent re-verify + mutation proof IN PROGRESS** (task #2) — incl. the teardown-vs-Reset load-bearing check (builder-1 flagged the symmetric concern to the request-leg `h1h3-fmd4-teardown-not-reset` finding: confirm the H3RespEvent::Reset→H1PumpAbort injection is load-bearing, not mere QUIC connection teardown).

**Task #2 verify found an R13(c) gap (caught by the mutation proof):** the committed CL-declared resp-truncation test (6cfd003f) was NON-load-bearing — it passed even with the guard DELETED, because the gateway forwards the upstream content-length and hyper's client-side CL-underrun detection catches the short body regardless of the guard. The verifier separately PROVED the guard IS real/load-bearing for chunked/no-CL responses (guard-deleted→leak Ok(88684); reverted→PASS). Memory: [[h1h3-resp-trunc-cl-masks-guard]]. **Fix #13b (commit `4a08f04f`):** added a CHUNKED (no-content-length) truncation arm where the guard is the SOLE discriminator (predicate complete:=Ok(_), any clean Ok = leak) + 60-iter chunked burst; CL arm kept + relabeled non-load-bearing. Suite 12/12. **Independent re-confirm (mutation on the chunked arm): PASS ✅** — verifier's proof matrix: guard restored → chunked PASS / CL PASS; guard deleted → chunked **FAIL (Ok leak len=88684)** / CL PASS (masked); reverted → PASS, git diff empty. The Reset→H1PumpAbort injection is proven the actual response-splitting mechanism for chunked/unknown-length responses (not teardown, not CL-underrun). R13(c) gap CLOSED. **H1→H3 = FULLY BUILT across both legs (8/9).**

Full BUILT-bar commit trail: bc8569ba (logic) + 1456ee03 (widening) + 51e04e29/0a6bc897/a4aaa7ac/c1bf78de/2247d655/0541930f (bar #1-7) + 6cfd003f (bar #8 resp-truncation, CL arm) + 4a08f04f (#13b chunked load-bearing arm).

### Trailer-mandate disposition (owner-ruled — Option 1: accept CASE-ii, documented)
Empirical, verifier-confirmed: **the connector PROPAGATES** the H3 backend's response trailers (incl. grpc-status) to the H1 front (proven by code-read at h1_proxy.rs:2196-2207 — `Frame::trailers` sent into the H1 response stream) — the MANDATORY half of the mandate is MET. **The H1 downstream DROPS** them: `grpc_status_reaches_h1_client=FALSE` — hyper's HTTP/1.1 encoder drops trailers on a streamed response absent a pre-declared `Trailer:` header, which a streaming front cannot emit without re-buffering (R8 violation). Precise factual claim: **connector propagates, H1 drops.** This is the documented CF-RESP-1 / CASE-ii limitation, identical to promoted H1→H2 (S11), and matches nginx. Standard gRPC clients use H2/H3 → **H2→H3 is the gRPC-capable →H3 cell where the mandate is fully met downstream**; no gRPC capability is lost. Owner-ruled (R7 escalation) to accept-and-document, not silently defer. **Operator disclosure: [docs/known-limitations.md](../../docs/known-limitations.md) + the gRPC note in docs/architecture.md** (customers hit it in docs, not production). The trailer mandate is SATISFIED for H1→H3: mandatory half met empirically; the unsatisfiable half escalated with mechanism + a defensible disposition + operator-facing docs.

**Coverage note:** the MANDATORY connector-propagation is proven by code-read, not asserted in-suite — a future connector trailer-drop would slip. A connector Decoded-arm trailer-propagation assertion is being added (refactor-eng, with #10/#11).

**Commit A (prerequisite, refactor-only) `1456ee03`:** widen ProxyService::Response body error `hyper::Error → Box<dyn Error+Send+Sync>` (shared `ClientRespBody` alias). Needed because the H1→H3 response-leg F-MD-4 truncation guard must yield a *constructible* body error on `H3RespEvent::Reset` (hyper 1.9 `hyper::Error` has no public ctor; the H1 server's `new_user_body` wraps a body-poll `Err` itself and aborts WITHOUT a clean `0\r\n\r\n` — a dropped/ended body = the false-complete bug). Lossless/behavior-identical (`.map_err(Box::new)`); dual-use request-body builders keep `hyper::Error`, re-boxed only at downstream-response call sites. R3 gate: h1h1 14/14, h2h1 13/13, h1h2 15/15, clippy+fmt clean. **NOTE:** builder-1 widened BOTH listeners (exceeded my "H1 ProxyService only" ruling — possible message-crossing); h2h2 re-confirm pending to close the R3 gap on the H2-front cell the H2-side widening touched. Also flagged: an R9 `git stash` use (resolved/popped; not to recur). Plan final-approved (audit/h-matrix/s12-phase1-h1h3-build-plan.md): rewire proxy_h1_to_h3 off buffering request_h3_upstream onto the connector (forward_req_trailers=true); H1 M-D-lite ingress→ReqBodyEvent (None=End, Some(Err)=Reset F-MD-4 mirror, over-cap→413, validate-before-dial); response leg consumes H3RespEvent (Decoded sink) via the upstream_response_to_h1 streaming-head template → body.boxed(). NEW BUILT-bar coverage (none existed: current H1→H3 tests were bodyless GET + codec-unit) incl. F-MD-4 with FULL R13 (a)+(b)+(c) from the start (learning from the H3→H3 gap), request-trailer-forwarding test, F-CAP-1, gRPC-shaped empirical trailer test, memory gauge, backpressure, CF-SATURATION-1 applied to over-cap tests.

## Phase 2 — H2→H3 — HONEST-STOPPED (owner-ruled) → S13
Owner ruled (R7 budget fork, Option A): honest-stop H2→H3 this session; spend the remaining budget on the CF-H3-HEAD 3-cell convergence (a real correctness fix, ~3h, high-certainty) rather than a ~10-13h H2→H3 build with two net-new risk points (request cancel-race + response-leg load-bearing arm). "One verified cell beats two half-done." H2→H3 is the easiest remaining cell (shares the M-C connector now proven twice — H1→H3 + the H3→H3 Wire path; H2 M-D ingress proven in H2→H1/H2→H2; H2 ProxyService widening already landed; H2 front carries trailers natively so the trailer mandate is FULLY met — no CASE-ii). **S13 plan authored:** `audit/h-matrix/s13-h2h3-plan.md` (builder-1) with the net-new breakdown + the BUILT bar + the TWO named hazards as explicit S13 verify targets + the connector-contract prerequisite. S13 = short session → likely 9/9 cleanly.

## Phase 3 — final gates + promote
🔜 in progress. Plan: reclaim disk; R1 ×3 `--workspace --all-features` deterministic (the CF-H3-HEAD R12 fix MUST be INSIDE the ×3 gate, not isolated); scoped llvm-cov; clippy+fmt; commit report/audit/s13-plan; promote `feature/h-matrix-s12` → main `--no-ff` with the honest message below.

### NEW PROGRAM PRINCIPLE (owner-directed)
A **session-INTRODUCED R12 violation is itself a finding to resolve before close, not a defer.** #10's H3→H3-only fix (correct in isolation) created a new sibling-divergence (H3→H3 forwards full headers; H3→H1/H3→H2 don't). The lead caught it and escalated; the owner ruled fix-all-three. Encode for future sessions: when a fix lands on one cell of a sibling set, check whether it creates divergence on the others and resolve it in-session — do not ship a self-introduced R12 gap.

### R11 promote message (honest — owner-approved text)
> H3-front response-header convergence (R12): all three H3-front cells now forward full response headers (was: :status + content-length only). 8 of 9 H-to-H cells BUILT (H2→H3 honest-stopped with plan ready for S13). NOT production ready — native QUIC proxy, WS/gRPC-over-H3, h3spec conformance, chaos/soak suite still pending.

### S13 handoff (ordered)
1. **H2→H3** (first — bigger correctness step; cell-close to 9/9). Plan: `s13-h2h3-plan.md`. Two named hazards S13 MUST verify against (not discover mid-session): (a) request cancel-race (downstream H2 RST cancels the ingress future before the pump emits Reset — R13 a/b/c); (b) response-leg load-bearing arm (apply the #13b CL-masking lesson — discriminator must be the guard: assert H2 client sees RST_STREAM/Err, not clean END_STREAM). Connector-contract prereq: confirm "body_tx dropped without End == Reset?" at S13 start.
2. **CF-BODY-WALLCLOCK** (after H2→H3) — the 4-cell wall-clock-body-timeout → idle-deadline fix (mirror F-S7-6), owner-deferred from S12.
3. Sweep: CF-R13-ITERS-H1H2 (h1h2 burst 24→≥50), CF-DEP-1 (Dependabot, now 5+ sessions old).

---

## Carry-forwards (tracked)
- **CF-BODY-WALLCLOCK** (NEW, this session) — see escalation above; S13, 4 cells, F-S7-6 template.
- **CF-H3-HEAD** (NEW, 3 cells) — all H3-FRONT cells dropped response headers but :status+content-length (H3→H3 `on_head` Wire; H3→H1 `stream_h1_response`; H3→H2 `stream_h2_response`). Owner-ruled FIX ALL THREE this session (single-source `encode_h3_headers_frame_full` + per-cell load-bearing test). #10 (`bcb4f09a`) fixed H3→H3; H3→H1/H3→H2 in progress. NOT a carry-forward — closed this session (R12 convergence).
- **CF-R13-ITERS-H1H2** (NEW, observation) — h1h2 F-MD-4 burst uses FMD4_SMUGGLE_ITERS=24, below R13's "≥50"; latent sub-R13 magnitude gap in a promoted cell. New H1→H3 test uses ≥50. Sweep promoted H1/H2/H3 F-MD-4 burst counts to ≥50 in S13. (H3→H3 case-7 had NO burst — task #11 this session.)
- CF-SATURATION-1 — addressed for fcap1_h2 this session; sweep remaining tight test-backend timeouts continues.
- CF-RESP-1 (UNIFIED, owner-directed) — two facets under one CASE-ii bullet:
  (i) H3-upstream whole-body read (request_h3_upstream multi-GB ceiling) — CLOSED for the →H3 cells by the extraction's streaming Decoded resp channel (bounded, H3_RESP_CHANNEL_DEPTH × H3_RESP_CHUNK_MAX).
  (ii) **H1-downstream streamed-response trailer drop (CASE-ii)** — applies to BOTH promoted **H1→H2** (S11) and new **H1→H3**: hyper's HTTP/1.1 encoder drops trailers on a streamed response absent a pre-declared `Trailer:` (unknowable at head-time without re-buffering → R8). gRPC implication: an H1 front cannot deliver grpc-status; use an H2/H3 front. Matches nginx. Connector propagates trailers; the loss is the H1 encoding step. Documented operator-facing in docs/known-limitations.md. NOT carried as two separate near-identical CFs.
- CF-DEDUP-2 — the M-C extraction itself (Phase 0a).
- **CF-HOPBYHOP-DUP** (NEW, minor) — `RESPONSE_HOP_BY_HOP` is now duplicated in lb-quic (`is_response_hop_by_hop`, mirrored from `lb_l7::h2_to_h1::RESPONSE_HOP_BY_HOP`) because lb-quic is below lb-l7 (reverse-layering forbids the dep). Kept in sync by comment. S13 cleanup: hoist the canonical set to a shared lower crate.
- F-ESC-1, N-1, S4-NUANCE-1, CF-COV-1/2/S7/S11, CF-DEP-1 (2 Dependabot advisories — owner, now 5 sessions old), CF-IGN-1 (16 inherited #[ignore]), CF-DISK-1, CF-RESETPEER-1 — carried.

## Post-matrix program work (NOT made production-ready by matrix close)
native QUIC proxy · WebSockets-over-H3 · gRPC-over-H3 · full h3spec conformance · chaos/soak suite.

---

## VERDICT — SESSION 12 (pending Phase 3 ×3 gate confirmation)
**What landed (all independently verified, load-bearing-proven):**
- **Phase 0a — M-C extraction (`369c5e53`):** shared streaming H3-upstream connector `stream_request_to_h3_upstream` + `H3RespOut{Wire|Decoded}` sink; H3→H3 byte-identical-equivalent (verified).
- **H1→H3 — FULLY BUILT (cell #8 of 9):** both legs streaming, binary byte-identical (5MiB), request-trailer forwarding, F-MD-4 R13 (a)+(b 60-iter current_thread)+(c) on BOTH legs (request + response truncation, mutation-proven load-bearing), F-CAP-1, memory bounded body-size-independent, scoped llvm-cov 90.23%.
- **CF-H3-HEAD — 3-cell response-header convergence (R12) + RFC 9114 §4.2 conformance:** all 3 H3-FRONT cells (H3→H3/H3→H1/H3→H2) now forward the full non-pseudo response header set minus hop-by-hop, via one helper; was `:status`+content-length only. Load-bearing + §4.2-strip proven.
- **H3→H3 hardening:** case-7 R13 (b)+(c) durable burst (was single-shot); connector Decoded-arm trailer + full-head propagation asserts.
- **Phase 0 R3 fix (`fe992654`):** fcap1_h2 saturation flake (CF-SATURATION-1).
- **Operator docs (`5ad15135`):** gRPC-needs-H2/H3-front (CF-RESP-1 CASE-ii).

**Matrix: 8 of 9 H-to-H cells BUILT.** H2→H3 HONEST-STOPPED (owner-ruled) with `s13-h2h3-plan.md` ready.

**Escalations (owner-ruled this session, all documented, none asterisked):** CF-BODY-WALLCLOCK (→S13), trailer-mandate H1-front CASE-ii (accept+document), CF-H3-HEAD scope (fix all 3 + honest-stop H2→H3). New program principle recorded: a session-introduced R12 violation is a finding to resolve before close.

**Phase 3 gate — GREEN ✅ (R1 ×3, full session tree, CF-H3-HEAD fix inside the gate):**
| Run | Exit | Failed suites | ok suites | fcap1 | h1h3 bursts (req+resp) | H3→H1/H3→H2 conv |
|---|---|---|---|---|---|---|
| #1 | 0 | 0 | 211 | ok | ok / ok | ok / ok |
| #2 | 0 | 0 | 211 | ok | ok / ok | ok / ok |
| #3 | 0 | 0 | 211 | ok | ok / ok | ok / ok |

clippy `--workspace --all-targets --all-features -D warnings` exit 0; fmt `--check` exit 0. Deterministic ×3 all-pass at full 8-core parallelism; the load-bearing smuggle (F-MD-4 both legs), the saturation fix (fcap1), and the H3-egress header convergence all green in every run.

## ✅ VERDICT — SESSION 12 COMPLETE
**H1→H3 BUILT (8 of 9 H-to-H cells); H3-front response-header convergence (R12 + RFC 9114 §4.2); H2→H3 honest-stopped with S13 plan ready (`s13-h2h3-plan.md`). NOT production ready — native QUIC proxy, WebSockets-over-H3, gRPC-over-H3, full h3spec conformance, and a chaos/soak suite remain.**

S13 (ordered): (1) H2→H3 → 9/9 (short session on the proven connector; two named hazards pre-flagged); (2) CF-BODY-WALLCLOCK (4-cell idle-deadline fix); (3) sweeps: CF-R13-ITERS-H1H2, CF-HOPBYHOP-DUP, CF-DEP-1.
