# S10 / H2→H2 (R8 bounded-incremental streaming) — INDEPENDENT verifier evidence

- **Verifier**: independent (author≠verifier — builder-1 wrote `crates/lb-l7/src/h2_proxy.rs`; this verifier wrote only the proofs).
- **Branch**: `feature/h-matrix-s10`. Builder tip `30918809` (the H2→H2 source change). Lead-report tip `28eb266a` (doc only).
- **Tool**: `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`, `cargo-llvm-cov` (workspace-instrumented, binary-scoped), `--features test-gauges`.
- **Files added (verifier-authored, NO src touched)**:
  - `tests/h2h2_md_streaming_verify.rs`
  - `audit/h-matrix/s10-h2h2-cov.awk`
  - `audit/h-matrix/s10-h2h2-verify.md` (this doc)

## 0. Diff scope — src UNTOUCHED (mandate #1)

```
$ git diff --stat                       # tracked files: EMPTY
$ git diff --stat HEAD -- crates/        # EMPTY
$ git diff --stat 30918809 -- crates/lb-l7/src/h2_proxy.rs   # EMPTY (byte-identical to builder tip)
$ git status --short
?? audit/h-matrix/s10-h2h2-cov.awk
?? tests/h2h2_md_streaming_verify.rs
```

`crates/lb-l7/src/h2_proxy.rs` is byte-identical to the builder tip; only test + audit files added. **Confirmed.**

## 1. Harness

Plaintext h2c (no TLS), mirroring `proto_translation_e2e::proxy_h2_listener_h2_backend`:
genuine hyper-H2 client → real `H2Proxy::serve_connection` listener → `RoundRobinUpstreams`
router → real hyper-H2 backend via `Http2Pool`. The adversarial cases (forbidden
pseudo-header trailer, content-length≠ΣDATA) use a flow-control-aware raw H2 client
(`RawH2`) over a plain TCP socket — a well-behaved h2 client cannot emit those frames.
Retained-memory gauge read = `lb_l7::h2_proxy::H2_REQ_MAX_RETAINED_BODY_BYTES` (shared
with H2→H1; the pump leaf is REUSED, not duplicated). WINDOW = depth(8)×8 KiB = 64 KiB.

## 2. Per-condition results (concrete evidence)

| # | Condition | Test | Result | Evidence |
|---|-----------|------|--------|----------|
| 1 | Req Branch A (1 KiB ≤ window) byte-identical | `req_branch_a_within_window_byte_identical` | PASS | body_len=1024, upstream_len=1024, status=200, verbatim at backend |
| 1 | Req Branch B 5 MiB byte-identical | `req_branch_b_5mib_byte_identical` | PASS | body_len=5242880, upstream_len=5242880, status=200 |
| 1 | Req Branch B 8 MiB byte-identical | `req_branch_b_8mib_byte_identical` | PASS | body_len=8388608, upstream_len=8388608, status=200 |
| 2 | Non-vacuous gauge + load-bearing inverted probe | `memory_gauge_non_vacuous_and_load_bearing` | PASS | in_situ=81920 (80 KiB ≈ 1.25×WINDOW, ≪ 4 MiB body); inverted probe `record_retained(4 MiB)` → after_probe=4194304 > 4×WINDOW (trips) |
| 2 | Live-occupancy (not cumulative), body-independent | `memory_gauge_tracks_live_occupancy_not_cumulative` | PASS | peak=81920 for a 4 MiB stream through a fast echo backend (decrement works) |
| 3 | Req backpressure pause-then-resume (48 MiB) | `req_backpressure_client_paused_while_backend_stalled` | PASS | paused_at=1441792 (1.4 MiB ≪ 48 MiB) under stall; retained gauge=81920; resume → status=200 + backend_read=50331648 (full 48 MiB) |
| 3 | Resume completes byte-identical (8 MiB) | `req_backpressure_resume_completes_byte_identical` | PASS | full 8 MiB byte-identical, complete=true |
| 4 | **F-MD-4 client RST mid-body never complete at H2 upstream** | `fmd4_client_rst_mid_body_never_complete_at_h2_upstream` | **DEFECT (intermittent)** | see §4 — `saw_complete=true` ~25–50% in isolation |
| 5 | F-CAP-1 over-cap (66 MiB) → 413 (NOT 502) | `fcap1_over_cap_upload_yields_413_not_502` | PASS | status=413, backend_drained=67107928 (~67 MiB), 13.6 s |
| 5 | F-CAP-1 forbidden pseudo-header trailer (>window) | `fcap1_over_window_pseudo_header_trailer_yields_400_not_502` | PASS | status=None (RST_STREAM), data_leak=false — see §5 note (RST is the RFC-mandated rejection; NOT 502/200) |
| 5 | content-length ≠ ΣDATA (>window) no leak | `fcap1_over_window_content_length_mismatch_no_leak` | PASS | status=None, data_leak=false, status≠200 |
| 5 | Genuine upstream failure (dead backend) → 502 | `fcap1_genuine_upstream_failure_yields_502` | PASS | status=502 (proves the F-CAP-1 arm does NOT blanket-413) |
| 6 | Resp 8 MiB byte-identical | `resp_8mib_byte_identical` | PASS | resp_len=8388608, got_len=8388608, status=200 |
| 6 | Resp 48 MiB byte-identical | `resp_48mib_byte_identical` | PASS | resp_len=50331648, got_len=50331648, status=200 (no truncation through the streaming relay) |
| 7 | Resp backpressure (slow client, 48 MiB) | `resp_backpressure_slow_client_streams_incrementally` | PASS | got_len=50331648, frames=4301 over a 256 KiB client window → incremental, not one blob (bounded-by-construction; honest argument — no gauge claimed, see §7) |
| 8 | Resp trailers relayed end-to-end (Q-HH-2) | `resp_trailers_relayed_to_client` | PASS | x-resp-trailer="landed" reached the client |
| 9 | Branch A within-window RST → zero-dial reject | `req_branch_a_rst_mid_body_zero_dial_reject` | PASS | backend_requests=0 (F-COR-1) |
| 9 | Branch A valid trailers forwarded | `req_branch_a_valid_trailers_forwarded` | PASS | upstream_body_len=4096 verbatim, complete=true |
| 9 | Branch B valid trailers forwarded | `req_branch_b_valid_trailers_forwarded` | PASS | upstream_body_len=262144, complete=true |
| 9 | Resp alt-svc header injected | `resp_alt_svc_header_injected` | PASS | alt-svc="h3=\":443\"; ma=3600" |

19/20 conditions PASS. **1 DEFECT: F-MD-4 (intermittent request-smuggling exposure).**

## 3. Memory gauge numbers (binding R8 bar)

- In-situ retained under a 4 MiB stall = **81920 B (80 KiB)** ≈ 1.25 × WINDOW (64 KiB), ≪ 4 MiB body.
- Live-occupancy peak for a 4 MiB stream through a fast backend = **81920 B** (decrement at the StreamBody pull works — not cumulative, not the round-1 constant).
- Body-INDEPENDENT: same 80 KiB ceiling for 4 MiB and 48 MiB bodies (`req_backpressure` retained=81920 for 48 MiB).
- INVERTED PROBE load-bearing: `record_retained(4 MiB)` → gauge=4194304 > 4×WINDOW → the assertion would catch a whole-body-buffering regression.

## 4. DEFECT — F-MD-4 intermittent request smuggling (FLAGGED FOR LEAD; not fixed)

**Mechanism (from captured output, R2).** A genuine h2 client streams a 256 KiB
(>window → Branch B) POST then RST_STREAMs mid-body (drop `send_body` WITHOUT
END_STREAM → CANCEL). The H2 backend (fresh, dedicated — no shared flag, no pooled
connection reuse) records `complete = (request body ended cleanly, frame loop saw
`None`, not `Some(Err)`)`.

```
isolated (--test-threads=1), repeated:
  saw_complete=false | test result: ok        (safe — RST surfaced as upstream error)
  saw_complete=false | test result: ok
  saw_complete=true  | test result: FAILED     (DEFECT — RST relayed as COMPLETE)
  ...
flap rate ≈ 25–50% of isolated runs; backend_requests=1 (gateway DID dial → non-vacuous);
backend_body_len=262144 (exactly the 256 KiB sent before RST — a TRUNCATED request);
saw_complete=true ⟺ test FAILED (clean 1:1 correlation across 8/8 runs).
```

When `saw_complete=true`, the gateway relayed a client-RST-truncated request to the H2
upstream as a **cleanly-ended (complete) request** — exactly the request-smuggling
condition F-MD-4 is meant to prevent (RFC: a reset mid-body must NEVER be presented as a
finished request upstream; the upstream stream must be RST, not END_STREAM'd).

**Race characterization.** Non-deterministic and timing-sensitive: it manifests when the
smuggle stream runs with minimal contention (isolated, single-threaded) and is MASKED
under the parallel workspace gate (4 threads → all 20 passed ×3, see §5). The suspected
window: when the 256 KiB body has fully drained through the gateway's bounded pump and
the pump is parked on the next `body.frame()`, the client RST arrives; on the racing
interleaving the gateway's upstream body channel closes/ends without converting the
inbound reset into an upstream `PumpAbort`/RST, so hyper sends a clean END_STREAM to the
backend. (The pump's `None` arm checks `body.is_end_stream()`; the exposure is consistent
with `is_end_stream()` racing true, or the lookahead/streaming hand-off not propagating
the reset — for the builder to localize.)

**Per mandate**: STOPPED, did NOT fix (author≠verifier). The test asserts the correct
invariant (`!saw_complete`) and is NON-`#[ignore]`'d so it lands in the gate. Note it is
FLAKY under parallel load (masked); to reproduce deterministically run it isolated:
`cargo test --features test-gauges --test h2h2_md_streaming_verify -- --test-threads=1 fmd4_client_rst_mid_body_never_complete_at_h2_upstream` repeatedly.

The H2→H1 sibling (`smuggling_rst_mid_body_never_complete_at_upstream`) and the H3→H2
sibling (`h2_e2e_client_reset_midrequest_...`) both PASS deterministically, so this is
specific to the S10 H2→H2 request leg, not the shared M-D mechanism.

## 5. F-CAP-1 / forbidden-trailer status note (NOT a defect)

The forbidden pseudo-header-in-trailers case rejects via **RST_STREAM** (status=None,
no backend body leak), not a gateway 400. This MATCHES the pre-existing production test
`h2_validation_before_forward::pseudo_header_in_trailers_never_leaks_backend_body` and the
s10 plan §Q-HH-2 ("pseudo-header in trailers → Err → PumpAbort injected → upstream RST").
hyper's inbound H2 server codec catches the malformed trailer at the protocol layer
(RFC 9113 §8.1 stream error) and RST_STREAMs the client BEFORE the gateway's
`validate_request_trailers` classifier runs; the F-CAP-1 "→400" applies to BadRequest
verdicts the gateway itself surfaces with a still-writable stream. The binding properties
(no leak; NOT a misleading 502; NOT a 200) all hold. The 502-control
(`fcap1_genuine_upstream_failure_yields_502`) proves the arm discriminates: a genuine dial
failure stays 502, NOT blanket-413.

**Over-cap throughput note.** An H2 backend that WITHHOLDS its response head during a
large upload collapses gateway→backend throughput to ~5–25 KiB/s (a hyper-H2-client
behavior, not an S10-code defect), making a 64 MiB cap-trigger impractically slow. The
reliable proof uses a hyper `Full` 66 MiB body (the efficient upload path the
byte-identity tests run at ~MB/s) + an inline-draining backend; the in-pump 64 MiB cap
then fires mid-stream and the verdict-preferred arm returns 413 in ~13.6 s. NEGATIVE
CONTROL: the H2→H1 sibling proved the SAME caller logic load-bearing in S9 by a
revert→502≠413 experiment; this suite re-proves the H2→H2 arm yields 413 and the dead-
backend control yields 502.

## 6. COVERAGE — SESSION sub-metric (binding, R10/CF-DISK-1)

Command (workspace-instrumented, binary-scoped — NOT bare `--workspace` which ENSPC-risked in S9):
```
cargo llvm-cov --workspace --features test-gauges --lcov \
  --output-path /tmp/s10_h2h2.lcov --test h2h2_md_streaming_verify -- --test-threads=2
awk -f audit/h-matrix/s10-h2h2-cov.awk /tmp/s10_h2h2.lcov
```
Per-line DA over the EXACT new/converted fn line ranges (sig→closing line):

```
proxy_h2_to_h2 (orchestrator) [1911-1941]     : 17/22  = 77.27%
proxy_h2_to_h2_request (pump)  [1950-2284]     : 145/176 = 82.39%
build_h2_upstream_request_parts [2335-2414]    : 70/77  = 90.91%
upstream_h2_response_to_h2 (relay) [2528-2560] : 25/30  = 83.33%
SESSION TOTAL (H2->H2 R8)                       : 257/305 = 84.26%  (need >=80%)
```

**SESSION sub-metric = 84.26% ≥ 80% (BINDING bar MET).** Uncovered lines (48) are
defensive/error arms: missing-pool 502 (1917-1920), Timeout-504 arm (2239-2240), send-
error 502 arm (2260), the verdict-relay Err arms (2274-2282, reachable only via the racy
abort), build-error arms (2348-2352, 2401-2402, 2555-2558). Disk after instrumentation:
**27 GB free** (well above the 25 GB floor). llvm-cov-target = 2.5 GB (left in place; no
ENOSPC risk).

## 7. Response-leg backpressure — honest bounded-by-construction argument

`upstream_h2_response_to_h2` does `body.boxed()` on the upstream hyper `Incoming` — there
is NO retained-bytes gauge we own for the response leg, so NO measured gauge is claimed.
Instead: (a) a 48 MiB response streams through INCREMENTALLY to a slow client over a
256 KiB window in **4301 frames** (not one materialised blob), completing byte-identical;
and (b) the relay structurally never `collect()`s (verified in source: `body.boxed()`,
no `.collect()`). Documented honestly.

## 8. R3 NO-REGRESSION

| Suite | Result |
|-------|--------|
| `tests/h2h1_md_streaming_verify` (H2→H1) | 15 passed |
| `tests/h1h1_md_streaming_verify` (H1→H1) | 14 passed |
| `tests/proto_translation_e2e` | 5 passed |
| `crates/lb-quic/tests/h3_h2_stream_e2e` (shared `Http2Pool`) | 10 passed |

**Http2Pool shared by H3→H2 — NO regression.**

## 9. Determinism ×3 (full suite, 4 threads)

```
RUN 1: 20 passed; 0 failed   (39.46s)
RUN 2: 20 passed; 0 failed   (37.69s)
RUN 3: 20 passed; 0 failed   (40.05s)
```
All 20 pass ×3 under the parallel gate — INCLUDING F-MD-4, because the F-MD-4 race is
MASKED under load (see §4). The defect is reproducible only under low-contention
(isolated) runs. This is itself a finding: the gate would NOT catch this smuggling
exposure under normal parallel execution.

## 10. VERDICT

BUILT bar **NOT fully met**: the H2→H2 request leg has an intermittent F-MD-4 request-
smuggling exposure (§4) — a client RST mid-body is, ~25–50% of isolated runs, relayed to
the H2 upstream as a complete request. All other conditions (byte-identity both legs,
bounded/load-bearing memory gauge, request + response backpressure, F-CAP-1 413/502
discrimination, trailers both directions, zero-dial within-window reject) PASS, and the
session-code coverage sub-metric (84.26%) clears the binding 80% bar. **FLAGGED FOR LEAD;
builder to fix the F-MD-4 reset→upstream-RST propagation race, verifier to re-verify.**
