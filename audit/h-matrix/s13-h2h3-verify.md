# S13 H2→H3 (R8) BUILT-bar — INDEPENDENT VERIFICATION REPORT

**Verifier:** verifier (author≠verifier, R5 strict — builder-1 authored; all claims
independently reproduced: re-run, MUTATE, re-measure).
**Branch tip verified:** `11ed191a` (detached HEAD in `eg-wt-verifier`).
**Tree state at end:** byte-identical to `11ed191a` except this report (every
mutation reverted via `git checkout --`; `git status` clean between each).

## VERDICT: BUILT-bar MET — promote-ready, with ONE non-blocking finding (F-CAP-1 coverage gap)

All load-bearing correctness mutations FAIL-as-required; ×3 workspace gate is
green + deterministic; clippy/fmt clean; session sub-metric 83.1% (≥80%); the
trailer-mandate is a real POSITIVE assertion. One finding: the F-CAP-1 mid-body
over-cap RST *arm* is not wire-exercised by its test (the security invariant it
asserts still holds, via a different mechanism) — detail in §F.

---

## A. Independent suite re-run — PASS
`cargo test --test h2h3_md_streaming_verify --all-features` → **13 passed, 0 failed**
(245 s). Real-wire H2 client → real H2Proxy (h3_upstream wired) → real QUIC/H3 backend.
- Byte-identity BOTH directions: `H2H3_BYTE_IDENTICAL sent=5242880 echoed=5242880 backend_body_bytes=5242880 complete=1`.
- Request trailers forwarded: `saw_req_trailers=1`.
- gRPC response trailer reaches H2 client: `grpc_status=Some("0")`.
- Memory gauge: `in_situ=8192 window=65536 body=4194304`.
- Conn-header not forwarded: `has_connection=false x-keep=Some("v")`.

## B. LOAD-BEARING MUTATION PROOFS — PASS (both)

**Hazard (a) request cancel-race** (`h2_proxy.rs:2467`, the `None && !is_end_stream()`
→ `ReqBodyEvent::Reset` arm). Mutated to send `End{trailers: Vec::new()}`:
`h2h3_fmd4_request_rst_never_complete` → **FAILED**: `complete_after=1`
(`backend_body_seen=262144`) — the truncated H2-RST upload reached the H3 backend
as a clean-FIN COMPLETE (smuggle). The explicit Reset is genuinely load-bearing.
Reverted clean.

**Hazard (b) response truncation** (`h2_proxy.rs:2580`, `H3RespEvent::Reset =>
btx.send(Err(H2RespAbort))`). Mutated to a clean `break` (drop, no Err):
- CHUNKED (no-CL) arm `..._chunked_never_complete` → **FAILED**: `body_res=Ok(len=88684)`
  (a truncated upstream response leaked as a clean Ok body). LOAD-BEARING confirmed.
- CL-declared arm `..._cl_never_false_complete` → **still PASSED** (`Err(body error)`,
  hyper's H2 client self-detects the CL underrun). Confirms the load-bearing arm is
  genuinely the no-CL one — the S12 #13b CL-masking trap is correctly avoided.
Reverted clean.

## C. R13 (a)+(b)+(c) on BOTH F-MD-4 legs — PASS
Independently re-run on `current_thread`, `--test-threads=1`:
- REQUEST burst `..._request_rst_burst_current_thread`: **60 iters, dialed=60**
  (every iter forwarded body upstream → non-vacuous), all-incomplete (zero
  `complete==1`).
- RESPONSE chunked burst `..._chunked_burst_current_thread`: **60 iters**, all-incomplete
  (no Ok leak).
Both deterministic, no flake. The historically ~25–50% single-threaded smuggle
([[parallel-gate-masks-smuggle]]) did NOT reproduce.
(c) load-bearing negative controls present and pass (clean upload → complete≥1;
clean response → Ok body).

## D. GAUGE NON-VACUITY scrutiny (builder-1 changed the harness) — PASS, proof NOT weakened
- (i) Body-size-INDEPENDENT: 4 MiB body → gauge 8192 (= one H3_BODY_CHUNK_MAX);
  ≤ 4×WINDOW (262144) and ≪ body. Confirmed.
- (ii) Gauge CATCHES a buffering regression: I mutated the pump's per-chunk
  `pump_in_flight.fetch_sub` away (simulating a whole-body retain). The memory test
  then **FAILED**: `in_situ=4194304` exceeds 4×window — proving the bound is real,
  not vacuous. Reverted clean. (The in-test inverted probe `record_retained(body_size)`
  also fires.)
- (iii) The added `req_body_bytes_live` did NOT make any security assertion vacuous:
  the smuggle assertion is `complete_after==0` reading `obs.complete`, incremented
  ONLY on a clean QUIC FIN (backend `if req_fin && !responded`). `req_body_bytes_live`
  feeds ONLY the `dialed` non-vacuity tally (`body_seen>0`) — it appears in no
  security assertion. The harness change STRENGTHENS the proof (shows body was
  actually forwarded); it does not weaken it.

## E. SCOPED llvm-cov session sub-metric — PASS (≥80%)
`cargo llvm-cov --workspace --all-features --test h2h3_md_streaming_verify --lcov`
(workspace lcov so lb-l7 + lb-quic dep-crate lines are captured). DA per-line over
the EXACT new fn ranges (NOT whole-file %):

| Symbol (lb-l7/src/h2_proxy.rs) | lines | DA covered |
|---|---|---|
| `proxy_h2_to_h3` (incl. pump + response relay) | 2361–2633 | 148/180 = 82.2% |
| `build_h2_to_h3_fieldlist` | 3062–3119 | 53/58 = 91.4% |
| `h2_decoded_resp_head_builder` | 3131–3150 | 15/19 = 78.9% |
| `H2RespAbort` struct + impls | 128–137 | 0/3 = 0% (Display::fmt only) |
| **SESSION lb-l7 aggregate** | — | **216/260 = 83.1%** |

Supporting (shared lb-quic connector, BUILT in H1→H3): Decoded-sink emit lines all
exercised — Head (2918/3002), Body (3038, 15007 hits), Trailers (3069), End (3083);
`stream_request_to_h3_upstream` entry+first-event 3252–3337 = 87.2%.

Uncovered lb-l7 lines (none are the load-bearing F-MD-4 guards — those are proven
by the §B mutations): error-config 502 paths (2371–2374, 2385); forbidden-trailer
Reset (2484); **over-cap Reset arm (2495–2496)** — see §F; connector-drop / proto-error
mid-body arms (2502, 2506–2511); `H2RespAbort::Display::fmt` (132–134, error is
boxed but never formatted); pseudo-skip + alt-svc branches (3139, 3145–3147).

## F. F-CAP-1 — FINDING (non-blocking): the mid-body over-cap RST arm is NOT wire-exercised
`h2h3_fcap1_over_cap_upload_never_complete` PASSES (`sent=27262976 backend_complete=0`)
and the pre-dial-down arm PASSES (`h2h3_upstream_down_yields_502` → 502). The
**security invariant (over-cap upload never reaches the backend as COMPLETE) holds.**

HOWEVER, coverage proves the over-cap Reset branch body (`h2_proxy.rs:2495–2496`) is
**never taken**: `MAX_REQUEST_BODY_BYTES = 64 MiB`, but the echo backend sets
`initial_max_data = 16 MiB`, so the upstream QUIC window fills and backpressure
stalls the H2 client at ~27 MiB (the `poll_capacity` break in the test) — well
before `forwarded_total` crosses 64 MiB. So `complete==0` arises from
backpressure-stall + client give-up, NOT from the cap RST the test's comment claims
("trips MAX_REQUEST_BODY_BYTES MID-body"). The cap arm for H2→H3 is therefore only
code-read, not exercised end-to-end on this cell (the identical shared cap arm IS
covered in the H2→H1/H2→H2 cells).
Impact: LOW. The arm is one line of shared logic mirrored across cells; the
security property is still positively demonstrated. Recommend (not gating): raise
the F-CAP-1 backend's `initial_max_data` above 64 MiB (or lower the cap under a test
cfg) so the over-cap branch is genuinely crossed — or down-scope the test's comment
to "backpressure-bounded, complete==0".
Pre-data 413 path: connector-unit-covered (`h3_bridge.rs:3327` `sink.inline(413,…)`);
not wire-reachable (needs a single inbound frame > 64 MiB). Posture acknowledged.

## G. TRAILER MANDATE — PASS (real POSITIVE assertion)
`h2h3_grpc_response_trailers_reach_h2_client` reads `body.trailers().await`, extracts
`grpc-status`, and asserts `assert_eq!(grpc_status.as_deref(), Some("0"), …)` (a hard
assertion at test line 1161, NOT an `eprintln` record). Observed `grpc_status=Some("0")`.
This is the H2-native `Frame::trailers` relay win (hyper's H2 server encoder flushes
trailers with no `Trailer:` pre-declaration) — the gRPC-capable →H3 cell.

## H. FULL ×3 GATE (R1/R10) + clippy + fmt — PASS, deterministic
`cargo test --workspace --all-features` ×3 (per-run logs captured):
- RUN 1: **1272 passed, 0 failed, 16 ignored**
- RUN 2: **1272 passed, 0 failed, 16 ignored**
- RUN 3: **1272 passed, 0 failed, 16 ignored**
Identical across runs; the h2h3 suite (14 lines) + 8 prior BUILT cells
(bridging h1h3/h3h3, fmd4 markers ×88) all ran; zero `test result: FAILED`.
R3 no-regression to S1–S12 + the 8 prior BUILT cells: confirmed.
`cargo clippy --all-targets --all-features -- -D warnings` → exit 0, clean.
`cargo fmt --check` → exit 0, clean. Both workspace-wide.

## Disk discipline (R9)
Stayed ≥19 GB; cleared `eg-target/debug/incremental` twice when approaching the
floor; ran coverage AFTER the ×3 gate (no ENOSPC; gate evidence never at risk).
