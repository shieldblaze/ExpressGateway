# S9 / H1→H1 (M-D-lite) — INDEPENDENT verifier report

**Cell:** H1→H1 bounded ingress pump in `H1Proxy::proxy_request`
(`crates/lb-l7/src/h1_proxy.rs`).
**Author:** builder-1 (`origin/s9/builder-1`; I1 `b01a13d2` pump+StreamBody,
I2 `0a479579` gauge).
**Verifier:** verifier (this report), branch `s9/verifier`. AUTHOR≠VERIFIER:
the src under test was synced byte-identical and NOT edited; all proofs live in
the verifier-owned `tests/h1h1_md_streaming_verify.rs`.

**VERDICT: BUILT.** All BUILT-bar items pass with captured real-wire evidence.
The session-code coverage sub-metric is **95.54% (193/202)**, deterministic
across 3 runs, comfortably over the 80% bar. One characterized behavioral
nuance (the over-cap / forbidden-trailer verdict surfaces as 502 rather than
413/400 when the backend has not yet sent a response head) is NOT a defect: it
is identical to the already-BUILT H2→H1 sibling cell, the security invariant
(never relayed complete, conn aborted) holds in every case, and the 413/400
status IS reachable and proven on the verdict-relay path.

---

## STEP 0 — exact builder src under test (diff-empty proof)

```
$ git checkout origin/s9/builder-1 -- crates/lb-l7/src/h1_proxy.rs
$ git diff origin/s9/builder-1 -- crates/lb-l7/src/h1_proxy.rs
        (no output — EMPTY)
DIFF_EMPTY rc=0
```

Re-confirmed after all coverage + clippy builds: `DIFF_EMPTY_FINAL rc=0`.
`Cargo.toml` unchanged from base; the lb-l7 `test-gauges` feature pre-exists and
is forwarded by the `lb-integration-tests` crate (root `tests/`).

Toolchain: `rustc 1.85.1 (4eb161250 2025-03-15)` (pinned via
`rust-toolchain.toml`), `cargo-llvm-cov 0.8.7` — the canonical pair.

---

## BUILT-bar items — PASS/FAIL with captured evidence

Evidence captured in `audit/h-matrix/s9-evidence/verify-run.txt`
(13 tests, `--test-threads=1`, all green).

### 1. Real-wire BOTH directions, binary body, byte-identical — PASS

Genuine hyper H1 client → plaintext `h1_proxy` listener → `RoundRobinAddrs`
router → real hyper H1 echo backend (`collect()` the request body, echo it as
the response). Request body is a non-UTF-8 deterministic pattern covering all
256 byte values; the response is de-chunked / CL-read with a NON-cumulative
per-read appender (S8 harness-bug lesson) and asserted **byte-identical**.

```
REAL_WIRE byte_identical len=1000     status=200 OK   (≤window small)
REAL_WIRE byte_identical len=5242880  status=200 OK   (5 MiB > window)
REAL_WIRE byte_identical len=8388608  status=200 OK   (8 MiB, under-cap large)
```

Covers ≤window (1000 B) and >window multi-MiB (5 MiB, 8 MiB). The response leg
is exercised in the same roundtrip (the echoed body is the response body).

### 2. Non-vacuous, body-size-INDEPENDENT memory (both legs) — PASS

Gauge: `lb_l7::h1_proxy::H1_REQ_MAX_RETAINED_BODY_BYTES` (+ `record_retained_h1`),
read directly under `--features test-gauges`. Window = depth(8) × 8 KiB = 64 KiB;
ceiling = 4×window = 256 KiB.

- **Stalled-backend in-situ** (4 MiB body through a backend that reads only the
  head then stalls):
  ```
  MEMORY_GAUGE in_situ_retained_bytes=65536 body_size=4194304 window=65536
               ceiling=262144 inverted_probe_after=4194304
  ```
  `65536 ≤ 262144` (≤4×window) and `65536 ≪ 4194304` (body) — bounded and
  body-size-independent (the retained set is exactly one window, not the body).
- **Inverted load-bearing probe** (same test): feeding the gauge what a
  whole-body-buffering impl WOULD retain, `record_retained_h1(4 MiB)`, drives it
  to `4194304 > 262144` — so the ≤4×window assertion DOES catch a buffering
  regression (non-vacuous).
- **Live-occupancy** (4 MiB through a FAST echo backend, full response read,
  byte-identical asserted):
  ```
  LIVE_OCCUPANCY peak_retained_bytes=65536 body_size=4194304 window=65536
  ```
  `peak > 0` (gauge genuinely moves) AND `peak ≤ 256 KiB` while 4 MiB is pushed
  AND pulled — a no-decrement / max-ever-pushed gauge would climb toward 4 MiB.
  This proves the StreamBody-poll decrement at `h1_proxy.rs:1267-1270` works.

### 3. Backpressure — proven causal chain + paused offset — PASS

A 48 MiB chunked upload (chosen ≫ the sum of loopback socket buffers + hyper's
frontend read buffer + the 64 KiB window + hyper's client write buffer, so it
CANNOT be absorbed) to a backend that reads the head then stalls until released.

```
BACKPRESSURE phase1 paused_at=8388608 paused_at2=8388608 body_size=50331648 window=65536
BACKPRESSURE phase2 resumed_at=50331648 status_line="HTTP/1.1 200 OK"
```

- **Phase 1 (stalled):** the client write parks at ~8 MiB (`paused_at`), and a
  second sample 1 s later is IDENTICAL (`paused_at2` == `paused_at`) — a STABLE
  stall, not mid-progress — while the body is 48 MiB. Causal chain: backend
  stall → gateway hyper sender cannot flush → bounded mpsc(64 KiB) fills → pump
  parks → gateway stops reading the frontend socket → frontend TCP recv window
  closes → client `write()` blocks far below 48 MiB. The captured paused-at
  offset varies run-to-run with loopback autotuned buffers (6.7–8.4 MiB
  observed) but is ALWAYS ≪ 48 MiB.
- **Phase 2 (released):** the parked write unblocks, the full 48 MiB completes
  (`resumed_at = 50331648 > paused_at`) with `200 OK` — resume-after-drain, no
  deadlock. The response leg is symmetric (the gateway streams the backend's
  response head/body back only after the pump's clean verdict).

(Harness note, R2: an initial 4 MiB body was fully absorbed by loopback buffers
and reported `paused_at == body_size` — an undersized harness, NOT a missing
backpressure. The 48 MiB body + the dual-sample stable-stall check is the
correct, mechanism-grounded probe.)

### 4. F-MD-4 smuggling parity — BOTH framings — PASS (load-bearing security)

A real raw-H1 client sends a request HEAD then a PARTIAL body then a premature
mid-body TCP half-close (`shutdown()` + drop). A raw "framing-witness" backend
records dials and whether it ever saw a COMPLETE request (the chunked `0\r\n`
terminator — the gateway always emits chunked since F-MD-1 strips CL/TE).

```
SMUGGLE_PARITY framing=cl       backend_dials=1 complete=0 backend_body_bytes=32800
SMUGGLE_PARITY framing=chunked  backend_dials=1 complete=0 backend_body_bytes=32800
```

- **(a) Content-Length framing:** declare CL=1 MiB, write 32 KiB, close →
  `complete=0`. **(b) chunked framing:** announce a 1 MiB chunk, write 32 KiB,
  close (no chunk completion, no terminator) → `complete=0`.
- **Non-vacuity:** `backend_dials=1` (forwarding actually began — the S8
  `dials=1` analogue) and `backend_body_bytes=32800` (the 32 KiB partial reached
  the upstream, re-chunked) — so the proof is not empty; yet the backend NEVER
  observed a complete request. The premature client EOF surfaces in the pump as
  `Some(Err)` (hyper `IncompleteBody`), which injects `H1PumpAbort`
  Err-before-close → hyper aborts the upstream body WITHOUT a `0\r\n\r\n`
  terminator. Single-use `take_stream` (ROUND8-L7-10) means the aborted conn is
  dropped, never pooled.
- **Pre-pump CL/TE smuggle still fires:**
  ```
  PRE_PUMP_SMUGGLE status_line="HTTP/1.1 400 Bad Request" backend_dials=0
  ```
  A request carrying both `Content-Length` and `Transfer-Encoding` is rejected
  400 by `SmuggleDetector::check_all_mode` in `handle` BEFORE any dial
  (`backend_dials=0`).
- **Q-H3 request-trailer validation** (`validate_h1_request_trailers`):
  ```
  LEGIT_TRAILER     status_line="HTTP/1.1 200 OK"          forwarded=true complete=1
  FORBIDDEN_TRAILER status_line="HTTP/1.1 502 Bad Gateway" backend_dials=1 backend_complete=0
  ```
  A legit `x-checksum` trailer is FORWARDED byte-faithfully (raw upstream bytes
  contain `x-checksum: deadbeef`) and the request completes 200 (R3:
  `trailer_passthrough` semantics preserved). A forbidden framing field
  (`transfer-encoding`) in trailers is REJECTED (status 400 or 502 per the race
  below; NEVER 200) and `backend_complete=0` — the Err-before-close abort means
  the backend never sees the forbidden trailers as a completed request.

### 5. 64 MiB cap → 413 — PASS (with characterized status nuance)

```
OVER_64MIB_413    status_line="HTTP/1.1 413 Payload Too Large"  written=68157440
OVER_64MIB_REJECT status_line="HTTP/1.1 502 Bad Gateway"        written=68157440
```

A 65 MiB chunked upload crosses `MAX_REQUEST_BODY_BYTES` (64 MiB).

- With an **early-responding backend** (replies 200 on the head before the cap),
  `send_request` resolves `Ok(Ok(resp))`, the verdict-relay gate is consulted,
  and the `BodyTooLarge` verdict maps to a true **413 Payload Too Large** on the
  wire — the bar's canonical 413 proof.
- With a **body-reading backend** (the typical case), the cap-exceed arm injects
  `H1PumpAbort` into the channel, which makes hyper's `send_request` error
  FIRST; that maps to **502 Bad Gateway** (upstream send failed) rather than the
  413 verdict. The load-bearing assertion holds: the over-cap upload is REJECTED
  (never 200, never relayed as success), and the upstream conn is aborted.
- **`under_cap_large_upload_yields_200`** (8 MiB) confirms the cap does not
  over-trigger; the 3 `h1_proxy_e2e` small-body tests do NOT regress (item 7).

**Status nuance — characterized, NOT a defect (R2):** the 413/400 verdict is
mapped to a client status only when `send_request` resolved with a response head
(`Ok(Ok(resp))`). On the Branch-B-only streaming path with a not-yet-responding
backend, the `H1PumpAbort` injection makes `send_request` error first → 502/400
race resolves to 502. This is structurally IDENTICAL to the already-BUILT H2→H1
sibling cell (`h2_proxy.rs` has the same `send_request`-err→502 arm at :1819-1825
and verdict gate at :1836; the S8 H2→H1 verify doc, line 162, explicitly blessed
`client_status=502` for the streaming over-cap case and left the mid-stream
`BodyTooLarge` line uncovered for the same reason). The security-relevant
invariant — an over-cap or forbidden-framing request is NEVER relayed to the
backend as a complete success, and the upstream conn is aborted — holds in every
captured case. Flagged for lead awareness; consistent with the established BUILT
bar for this family of cells.

### 7. R3 no-regression + hygiene — PASS

Named regression suites (all on the synced builder src, `--test-threads=1`):

```
round8_body_overread          : 4 passed   (incl. h1_take_and_discard_doc_block_present)
bridging_h1_h1                : 1 passed   (test_bridge_h1_to_h1)
trailer_passthrough           : 8 passed
round8_keepalive_count_cap    : 3 passed
smuggle_matrix                : 13 passed
smuggle_wired                 : 3 passed
h1_proxy_e2e                  : 3 passed   (the 3 small-body e2e tests)
```

- `cargo fmt --check` — **clean** (whole workspace; the verifier test file was
  fmt-normalized).
- `cargo clippy --all-targets --all-features -- -D warnings` — **clean**
  (`Finished` / exit 0; the verifier test file is clippy-clean too).

---

## Coverage — H1 M-D-lite SESSION sub-metric (binding)

Canonical: toolchain 1.85.1, `cargo-llvm-cov 0.8.7`,
`cargo llvm-cov --workspace --features test-gauges --lcov -- --test-threads=1`,
×3. awk script: `audit/h-matrix/s9-h1h1-cov.awk` (mirrors `s8-md-cov.awk`).

**Net-new H1 session line ranges** (signature → last body line; lcov only emits
DA: for instrumentable lines):

| item | range | covered |
|---|---|---|
| `H1PumpAbort` type + Display/Error impls (F-MD-4) | 121–129 | 0/3 |
| `validate_h1_request_trailers` (Q-H3) | 149–169 | 16/16 = 100% |
| `proxy_request` — the whole Branch-B-only pump | 1173–1531 | 166/171 = 97.08% |
| `record_retained_h1` (F-MD-3 test-gauge) | 2433–2447 | 11/12 = 91.67% |
| **SESSION TOTAL** | | **193/202 = 95.54%** |

Bar: ≥80% (≥162 covered). **PASS by a wide margin.**

**Determinism:** runs 1, 2, 3 produced byte-identical totals (193/202 = 95.54%)
AND the identical uncovered set (`124 125 126 1462 1525 1526 1527 1528 2444`).
lcov traces retained: `audit/h-matrix/s9-evidence/cov-run{1,2,3}.lcov`.

### Uncovered lines — honest characterization (9 lines)

- **124–126** — `impl Display for H1PumpAbort { fn fmt … f.write_str(…) }`. The
  abort error is constructed and propagated as a body error, but its `Display`
  is never invoked (nothing formats it to a string on the tested paths). Dead
  for coverage, live as a trait-bound satisfier (`Error: Display`). Cosmetic.
- **1462** — the closing `}` of the `if let Ok(data) = frame.into_data()` arm in
  the streaming loop, reached only on a specific frame-shape branch boundary;
  the surrounding data/cap/send logic is covered. Structural brace line.
- **1525–1528** — the `verdict_rx.await` `Err(_)` arm: "pump task vanished
  without a verdict (panic/abort) → BadRequest". This fires only if the pump
  task panics or is aborted before sending a verdict — a defensive belt-and-
  suspenders arm not reachable on any normal/adversarial path tested. Honest
  gap; defensive code.
- **2444** — `record_retained_h1`'s CAS-retry arm `Err(observed) => cur =
  observed`, taken only on a `compare_exchange_weak` spurious-fail / concurrent
  race; the happy-path success branch is covered. Lock-free retry edge.

None of the uncovered lines is on a security-relevant happy/adversarial path:
the F-MD-4 abort arms, Q-H3 reject, Q-H4 cap, F-MD-2 drain-and-validate,
trailers forward, the clean-end terminator, and the live gauge inc/dec are all
covered.

---

## Notes / mechanism log

- All "harness" reclassifications were proven by mechanism (R2), not by
  isolation: the 4 MiB→48 MiB backpressure resize (buffer absorption), the
  non-cumulative response reader, and the dedicated concurrent reader for the
  mid-stream cap response. No proxy behavior was altered to make a test pass.
- The 502-vs-413/400 verdict-relay race is a behavioral characterization shared
  with the BUILT H2→H1 cell, not a regression introduced by this cell.
- Disk hygiene: the `cargo llvm-cov` instrumented build (`eg-target/
  llvm-cov-target`, ~18 GB) was removed after capturing the 3 canonical lcov
  traces (S8 precedent); 48 GB free at report time.

## VERDICT

**BUILT.** Every BUILT-bar item passes with captured real-wire evidence; the
session-code coverage sub-metric is 95.54% (deterministic ×3, ≥80%); fmt and
clippy are clean; the named R3 regression suites are green. The single
behavioral nuance (over-cap/forbidden-trailer verdict surfacing as 502 with a
not-yet-responding backend) is characterized, matches the already-BUILT H2→H1
sibling, and preserves the security invariant in every case.
