# H3→H2 INDEPENDENT VERIFICATION — verifier-h3h2 (S6 task #5)

- Verifier worktree: `.claude/worktrees/verify-h3h2`, branch `s6/verify-h3h2`
- Verified content: builder tip `da764824` (== `s6/builder-1-h3h2`)
- Date: 2026-05-18

## VERDICT: **H3→H2 BUILT**

All gate conditions met non-vacuously. The H3→H2 cell now clears the
H3→H1 BUILT bar (R8 fixed in-flight window, end-to-end backpressure,
currently-passing real-wire proofs with a non-vacuous memory assertion +
liveness, request body byte-identical at a real H2 backend). One MINOR
non-blocking deviation noted (§ Deviations): fmt diffs confined to the
new TEST file only (production src is fmt-clean, clippy clean).

## 8-CHECK RESULTS

### 1. Real-wire path GENUINE — PASS
`tests/h3_h2_stream_e2e.rs` drives the production `QuicListener::spawn`
+ `.with_h2_backend(h2_pool, backend)` → `router::spawn` →
`conn_actor::run_actor`/`poll_h3` (I3 H2 branch) → `h3_to_h2_stream_resp`
→ `Http2Pool::send_request`. Backend is a REAL hyper H2 server
(`hyper::server::conn::http2::Builder::new(TokioExec).serve_connection(
HyperIo(sock), svc)`) over a REAL accepted `TcpListener` socket; the
`HyperIo` adapter wraps a real `tokio::net::TcpStream` (no hyper-util,
A1 scope held) — NOT a stub. 7/7 deterministic ×3 (runs 1–3, all
`7 passed; 0 failed; 0 ignored`, ~4.9s each). Evidence: `h3h2-run{1,2,3}.txt`.

### 2. Binding cond 1 (request body NOT dropped) — PASS, NON-VACUOUS
Case 2 sends a non-UTF-8 1 MiB+777-byte body over many 48 KiB DATA
frames; asserts `got.len() == payload.len()` AND `got == payload`
(full byte-identical) AND `seen.complete` (clean end) AND the echoed
response equals the payload. Non-vacuity: it asserts FULL-LENGTH
captured-backend-byte equality, not status — a dropped/truncated body
fails on length/equality. Old defect gone: `h3_to_h2_roundtrip`
DELETED; the only `Full::<Bytes>::new(Bytes::new())` left in h3_bridge.rs
(line 2283) is the LEGITIMATE bodyless case (first event `End`/`None` ⇒
content-length 0), never the body-carrying path. Evidence: `h3h2-run1.txt`.

### 3. Binding cond 2 (BOTH directions, non-vacuous) — PASS
Gauges are the REAL feature-compiled crate statics
`lb_quic::h3_bridge::MAX_RETAINED_{RESP,BODY}_BYTES` (not test stubs).
Asserted ceiling = `retained_ceiling = 4×(DEPTH×(CHUNK_MAX+HDR))` and
the test asserts `ceiling == 262_656` (authoritative crate-const
formula, DEPTH=8, CHUNK=8192, HDR=16). Body ≥8× ceiling (4 MiB resp/req;
8 MiB ≥16× in case 5).
**Non-vacuity DEMONSTRATED**: in-worktree I inverted case 3's assertion
to `retained > ceiling` — it FAILED with `retained 73859 is NOT >
ceiling 262656 (body 4194304)`. So the live gauge reached only ~73.8 KB
for a 4 MiB body (~57× under) — the `retained <= ceiling` assertion is
load-bearing and WOULD fail under reintroduced whole-body buffering
(which would push retained to ~4 MiB). Probe reverted; tree clean.
Liveness: cases 3/4/5 also assert `out.body == body` byte-identical
after stalled-peer resume + clean FIN. Evidence:
`nonvacuity-inverted-probe.txt`, `h3h2-run{1,2,3}.txt`.

### 4. Binding cond 3 (gate flag non-skippable) — PASS
The 3 memory/backpressure cases are `#[cfg(feature = "test-gauges")]`
on the test fn. WITHOUT the flag the suite compiles but `cargo test -p
lb-quic --test h3_h2_stream_e2e` runs only **4** cases (the 3 memory
proofs are absent; `retained_ceiling`/`spawn_h2_large_resp` become
dead-code warnings — proving the gauge cases are excluded). A flagless
gate therefore CANNOT execute the non-vacuous memory proofs — invalid
by design, matching the H3→H1 bar property. (Mechanism is `#[cfg]`
exclusion rather than the inventory's "fails to compile"; functionally
equivalent + arguably cleaner — acceptable, matches plan §4.) Evidence:
`no-gauges-compile.txt`, `no-gauges-run.txt`.

### 5. Cases 5/6/7 sound — PASS
- Case 5 (backpressure): 8 MiB body (~32× ceiling), stalled client;
  asserts `retained <= ceiling` AND `out.body == body` byte-identical
  after resume + FIN — the causal chain (stalled H3 client ⇒ channel
  full ⇒ `stream_h2_response`'s `tx.send().await` parks ⇒ `body.frame()
  .await` not re-polled ⇒ hyper stops H2 WINDOW_UPDATEs ⇒ H2 upstream
  read pauses) held WITHOUT drop/corruption. Not trivially passing
  (proven load-bearing by the §3 inversion on the same gauge).
- Case 7 (lead A2 smuggling parity): client RESETs after ~256 KiB of a
  2 MiB body; asserts `!backend_saw_complete` (the real hyper backend's
  `body.frame()` saw `Some(Err)`, NOT a clean end) AND not a clean
  200+FIN to client. Mechanism: peer RESET → `drain_body_stream`
  surfaces `ReqBodyEvent::Reset` → `H3ReqStreamBody::poll_frame` returns
  `Err(H3ReqAbort)` → hyper RST_STREAMs the H2 upstream. A truncated
  request is never presented as complete.
- Case 6 (response-splitting guard): backend declares CL=1 MiB but
  errors after ~64 KiB; asserts NOT(200+FIN+full body) and that any
  200+FIN body is < declared. `stream_h2_response` maps the
  `body.frame()` Err arm to `RespEvent::Reset` + `RespAbort::
  UpstreamReset` (never `End`). All in `h3h2-run{1,2,3}.txt`.

### 6. R8 code inspection (adversarial) — PASS
- `stream_h2_response` (h3_bridge.rs:1496): `while let Some(frame) =
  body.frame().await` one frame at a time; each DATA split to
  ≤`H3_RESP_CHUNK_MAX` (8 KiB) and sent on bounded
  `mpsc<RespEvent>(H3_RESP_CHANNEL_DEPTH=8)`; in-hand frame dropped
  after split. NO `.collect()`, NO `Vec<u8>` accumulation. `cap`
  (`MAX_RESPONSE_BODY_BYTES`, 64 MiB) is a DoS abort threshold only,
  NOT the memory bound (the bound is depth×chunk).
- `H3ReqStreamBody`/`h2_request_body_from_rx` (h3_bridge.rs:2168/2256):
  custom `hyper::body::Body` polling `body_rx` (the bounded M-A
  `mpsc<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH=8)`) directly — one
  `Frame::data` per `Chunk`, `End`⇒clean EOS, `Reset`/closed⇒`Err`. No
  pump task, no extra queue, no `.collect()`, no pre-size. The peeked
  `first` chunk is a single ≤8 KiB `Bytes`.
- M-A pump (`drain_body_stream` conn_actor.rs:1000): explicit
  backpressure GATE — returns WITHOUT pulling more off quiche while
  `body_pending` non-empty (channel full), so the `VecDeque` only holds
  the decode of ONE 8 KiB `stream_recv`. Bounded, body-size independent.
- `poll_h3` H2 wiring (conn_actor.rs:809–863) is a near-verbatim copy of
  the proven H3→H1 branch with the spawned fn swapped; H3→H1/H3→H3/
  inline-error paths untouched. The residual `decoded_body: Vec<u8>` at
  h3_bridge.rs:2527/2720 is in `request_h3_upstream`/`h3_to_h3_roundtrip`
  (the H3→H3 path) — NOT the H3→H2 path under verification.

### 7. R3 no-regression — PASS
- `h3_h1_resp_stream_e2e --features test-gauges`: 16/16, 0 ignored.
- `h3_h1_stream_body_e2e --features test-gauges`: 6/6, 0 ignored.
- `lb-quic` full (`--features test-gauges`): lib 20/20; every suite
  green incl. trailers/errors; h3_h2_stream_e2e 7/7; 0 ignored anywhere.
- I0.5 blast radius: `lb-io` 44+3 PASS; `proto_translation_e2e` 5/5
  (incl. `proxy_h3_listener_h2_backend` — pre-existing H3→H2 wiring test
  still green with the new streaming path); `h2_proxy_e2e` 3/3;
  `h1_proxy_e2e` 3/3. I0.5 is a pure body-error type-widening
  (`hyper::Error` → boxed `dyn Error`), callers adapt via
  `.map_err(Into::into).boxed()` — non-regressive, proven by the green
  blast radius. Evidence: `r3-*.txt`, `r3-lbquic-all.txt`.

### 8. Flake/mechanism check (R2) — PASS
Zero failures across all runs (7/7 ×3 deterministic; full lb-quic +
blast radius all green). No flakiness observed; no mechanism to
classify. The single intentional failure (my §3 inverted probe) failed
for the EXPECTED mechanism (retained 73859 ≪ ceiling), confirming
boundedness — then reverted.

## DEVIATIONS / CONCERNS (non-blocking)

1. **MINOR — fmt nit, TEST file only.** `cargo fmt -p lb-quic --
   --check` reports diffs ONLY in `crates/lb-quic/tests/h3_h2_stream_e2e
   .rs` (≈10 sites). Production source (`src/h3_bridge.rs`,
   `src/conn_actor.rs`) is fmt-clean (rustfmt exit 0, 0 src diffs) and
   `cargo clippy -p lb-quic --features test-gauges --tests` is clean (0
   errors). This is verification-suite hygiene, not a correctness or
   production-code defect; it does not affect the R8 bar (tests compile,
   run, pass deterministically). Builder should `cargo fmt` the test
   file before the Phase-3 gate. Does NOT block BUILT.
2. cond-3 mechanism is `#[cfg]` exclusion (file compiles, 3 cases
   absent without the flag) rather than the inventory's literal "fails
   to compile". Functionally equivalent gate property (a flagless run
   cannot execute the memory proofs); acceptable and matches plan §4.

## EVIDENCE INDEX (this dir)
- `h3h2-run{1,2,3}.txt` — 7/7 ×3 deterministic
- `nonvacuity-inverted-probe.txt` — load-bearing proof (retained 73859 ≪ 262656)
- `no-gauges-{compile,run}.txt` — cond-3 (4 cases only without flag)
- `r3-h3h1-resp.txt` (16/16), `r3-h3h1-body.txt` (6/6),
  `r3-lbquic-all.txt`, `r3-lbio.txt` (44+3), `r3-proto-trans.txt` (5/5),
  `r3-h2proxy.txt` (3/3), `r3-h1proxy.txt` (3/3)
- `clippy.txt` (0 errors), `fmt.txt` (test-file diffs only)
