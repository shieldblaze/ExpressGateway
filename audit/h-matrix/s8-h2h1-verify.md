# S8 — H2→H1 (M-D) INDEPENDENT RE-VERIFICATION (round 2, post-remediation)

- Verifier: `verifier2` (author≠builder, strict). Builder-1 wrote
  `crates/lb-l7/src/h2_proxy.rs`; the verifier did NOT edit it. Proof the
  exact builder src is under test:
  `git diff origin/s8/builder-1 -- crates/lb-l7/src/h2_proxy.rs` = EMPTY.
- Builder tip re-verified: **`bc23b9f8`** (remediation: F-MD-1 stale-H2-version
  Branch-B empty-body fix, F-MD-2 early-response 413 regression fix, F-MD-3
  vacuous streaming gauge fix). Synced via
  `git checkout origin/s8/builder-1 -- crates/lb-l7/src/h2_proxy.rs crates/lb-l7/Cargo.toml`
  (keeps the verifier's own test files).
- Plane: `lb-l7`. Toolchain `1.85.1`, `cargo-llvm-cov 0.8.7`, `h2 0.4.13`.
- Verifier test files (NEW/UPDATED; the src is untouched):
  - `tests/h2h1_md_streaming_verify.rs` — authoritative behavioral suite. Now
    ALL-GREEN (14 tests). Round-2 additions: a fixed (per-chunk) response
    reader; the harness-bug adjudication trio
    (`harness_bug_reqwest_independent_512k_byte_identical`,
    `harness_bug_cumulative_release_stall_is_reader_side`); a streaming-phase
    live-occupancy gauge test (`memory_gauge_tracks_live_occupancy_not_cumulative`);
    a real-h2-client smuggle test that now reaches the post-dial abort path.
  - `tests/h2h1_md_coverage_driver.rs` — no-assert clean-exit coverage driver.
  - `audit/h-matrix/s8-md-cov.awk` — re-calibrated to the remediated src line
    ranges (proxy_request [1306-1741], validate_request_trailers [1956-1973],
    concat_chunks [1978-1987], record_retained [2356-2370]).

---

## OVERALL VERDICT: **BUILT**

All three round-1 blocking findings are confirmed FIXED by mechanism, and the
full BUILT bar passes with captured evidence. The single round-1 RED that was
a HARNESS bug (cumulative `release_capacity`) was independently proved to be a
reader bug — NOT a gateway defect — and the harness was fixed.

- **F-MD-1 (was BLOCKING) — FIXED.** Branch-B (>window) bodies now reach the
  backend in full and return the backend's real status. `parts.version` is
  forced to `HTTP_11` and `content-length`/`transfer-encoding` are stripped
  (`h2_proxy.rs:1335-1337`) before `Request::from_parts`, so hyper's HTTP/1.1
  encoder frames the streaming `StreamBody` as chunked instead of mis-framing
  it to an empty body. CONFIRMED:
  `diag_branch_b_echo_body_seen=524288 of 524288, client_status=200`.
- **F-MD-2 (was BLOCKING) — FIXED.** A >window upload to a backend that
  early-responds 401 without reading the body now relays the BACKEND's 401
  (not a manufactured 413). Receiver-drop triggers drain-and-validate
  (`h2_proxy.rs:1605-1631`, invoked at :1639/:1681) instead of `BodyTooLarge`.
  CONFIRMED: `deviation1_early_response_on_over_window_upload` →
  `client_received_status=401`.
- **F-MD-3 (was partially vacuous) — RESOLVED.** The streaming-phase gauge now
  tracks a real in-flight counter (`in_flight_bytes`, incremented on push at
  `h2_proxy.rs:1574`, DECREMENTED when hyper pulls at :1524) plus the lookahead
  remainder, recorded as `lookahead_remaining + in_flight_bytes` at :1581 — not
  a constant. Proved non-vacuous: 4 MiB streamed through a FAST backend peaks
  the gauge at 80 KiB (≤ 4×window), which a constant or no-decrement gauge
  could not produce.

---

## HARNESS-BUG ADJUDICATION (R2 — mechanism PROVED independently, not trusted)

Round-1 `real_wire_large_body_byte_identical` failed with `UnexpectedEof at
~65535 bytes` on the RESPONSE read. The reader did
`release_capacity(got.len())` with the running cumulative total.

**Mechanism, confirmed against the h2 0.4.13 source** (`share.rs:516-539`,
doc: "the caller cannot release more capacity than data has been received"):
the cumulative call over-releases from the 2nd chunk on, returns `Err`
(swallowed by `let _ =`), so no further WINDOW_UPDATE is emitted and the
server stalls at the 65535-byte initial stream window → `UnexpectedEof` on any
response body larger than one window. This is a HARNESS (reader) bug.

**Three independent confirmations the gateway response leg is actually
correct (not masking a real defect):**

1. **Correct per-chunk reader** relays the full 512 KiB / 524288-byte response
   byte-identical: `real_wire_large_body_byte_identical` PASSES.
2. **Independent client (reqwest, its own H2 stack + internal flow control)**
   relays the full 512 KiB byte-identical:
   `harness_bug_reqwest_independent_512k_byte_identical` →
   `reqwest 512KiB byte-identical OK`.
3. **Reader-pattern-dependence, not request-side:** the SAME cumulative pattern
   STALLS the response read for a LARGE response produced by a SMALL (Branch-A)
   request: `harness_bug_cumulative_release_stall_is_reader_side` →
   `cumulative ... stalled=true got=65535 (≪ full 524288)`, while the
   per-chunk reader of the identical case relays `full=524288`.

**Harness fix applied:** the BUILT-bar real-wire reader now calls
`release_capacity(chunk_len)` per chunk. Both directions (request + response),
binary non-UTF-8 bodies, are byte-identical for small (Branch A) and large
(Branch B). If the independent reqwest check had failed it would be a REAL
gateway defect — it did not, so the harness fix is justified.

---

## Per-item results (full BUILT bar)

### 1. Real-wire binary bodies, both directions, byte-identical — **PASS**
Genuine `h2` client over rustls TLS → real h2_proxy listener → router → REAL
hyper H1 echo backend. Non-UTF-8 payload `(i*31+17)%256`.
- ≤ window (Branch A), 8 KiB: `real_wire_small_body_byte_identical` PASS
  (byte-identical, 1 dial).
- > window (Branch B), 512 KiB: `real_wire_large_body_byte_identical` PASS
  (byte-identical, 1 dial) — the round-1 RED, now green after the harness fix.
- Independent reqwest 512 KiB cross-check: PASS (byte-identical).
- Window boundary sweep (`diag_legit_upload_status_across_window_boundary`):
  `size=32768/65536/65537/81920/131072/524288 → status=200 dials=1` for ALL.
  The round-1 413-at-window+1 defect is gone.

### 2. Non-vacuous memory + inverted probe — **PASS (F-MD-3 resolved)**
- In-situ (stalled backend, 4 MiB body):
  `MEMORY_GAUGE in_situ_retained_bytes=81920 body_size=4194304 window=65536`.
  80 KiB ≤ 4×window (256 KiB) ✓ and ≪ 4 MiB ✓.
- Inverted probe (load-bearing): `record_retained(4 MiB)` pushes the gauge
  above 4×window → the `≤256 KiB` assertion WOULD trip a whole-body retain.
- Live-occupancy proof (NEW, the round-1 vacuousness killer):
  `memory_gauge_tracks_live_occupancy_not_cumulative` streams a full 4 MiB
  through a FAST backend and reads the whole response, yet
  `MEMORY_GAUGE_LIVE peak_retained=81920 (≤4×window=262144)`. A constant gauge
  or a no-decrement (cumulative) gauge could not stay bounded while 4 MiB flows
  through — this proves the streaming record site measures REAL retained bytes
  that are released (decrement at :1524 works). Memory bar MET for BOTH the
  ≤window lookahead and the >window streaming regimes.

### 3. Backpressure, both directions, proven causal chain — **PASS (both halves)**
`backpressure_client_send_paused_while_backend_stalled`, 4 MiB body:
- Phase 1 (pause): backend stalled → `BACKPRESSURE phase1 paused_at≈2.9 MiB`
  (< 4 MiB body) — the client send is paused below the body size (backend stall
  → bounded channel fills → pump parks → hyper/h2 withholds WINDOW_UPDATE).
- Phase 2 (RESUME — was blocked by F-MD-1, now reachable): on backend release,
  `BACKPRESSURE phase2 status=200 ... resumed_at=4194304` — the full body
  completes with the backend's 200 and the client resumed past the pause point.

### 4. h2spec ordering + over-window malformed → no leak — **PASS**
- The two existing gate tests in `tests/h2_validation_before_forward.rs`
  (`content_length_mismatch_never_leaks_backend_body`,
  `pseudo_header_in_trailers_never_leaks_backend_body`) pass UNCHANGED (not
  edited), plus `oversized_request_body_rejected_413_not_buffered_unbounded`:
  `test result: ok. 3 passed`.
- NEW over-window adversarial (raw H2 frames, 128 KiB > window):
  `over_window_pseudo_header_trailers_no_leak` and
  `over_window_content_length_mismatch_no_leak` both PASS, no backend DATA frame
  relayed downstream. (`backend_dials=0`: the raw-framed >window body exceeds
  the gateway's default 65535 per-stream H2 receive window and is rejected at
  the H2 receive layer before a dial — the no-downstream-leak invariant holds
  regardless of where the rejection lands.)

### 5. Early-response relay (Deviation #1) — **PASS (F-MD-2 fixed)**
`deviation1_early_response_on_over_window_upload`: 256 KiB binary upload to a
backend returning an early 401 without reading the body →
`client_received_status=401`. The backend's response is relayed (the round-1
413 regression is gone).

### 6. Smuggling parity — **PASS (now NON-VACUOUS)**
`smuggling_rst_mid_body_never_complete_at_upstream` rewritten to use a REAL h2
client (respects flow control, gets WINDOW_UPDATEs) so a 128 KiB >window stream
actually crosses the window, the gateway DIALS, then the client RSTs mid-body:
`SMUGGLING dials=1 complete_requests=0`. The post-dial abort path is now
exercised (dials=1, unlike round-1's vacuous dials=0) and the security
invariant holds: a truncated (RST mid-body) request is NEVER seen as a COMPLETE
request at the H1 upstream.

Corroborating: `diag_branch_b_body_bytes_reaching_upstream=524865 of 524288,
client_status=502` — the full body (plus chunked-framing overhead) streams to a
raw backend that speaks no HTTP; the 502 is the correct mapping of hyper's H1
parse failure on a non-HTTP backend, not a gateway defect.

### 7. Coverage ≥80% on the M-D session sub-metric — **PASS (81.82%)**
Canonical `cargo-llvm-cov 0.8.7`, toolchain `1.85.1`, `--workspace
--features test-gauges`, driven by the all-green behavioral suite + the
no-assert coverage driver + the existing gate tests. Deterministic across 3
runs (identical SESSION TOTAL and identical uncovered-line set, md5
`ae9ad512c3493232d16bac5d2904df75`).

Command:
```
cargo llvm-cov --workspace --features test-gauges --lcov \
  --output-path /home/ubuntu/Code/s8-eg-cov.lcov \
  --test h2h1_md_streaming_verify --test h2h1_md_coverage_driver \
  --test h2_validation_before_forward -- --test-threads=1
awk -f audit/h-matrix/s8-md-cov.awk /home/ubuntu/Code/s8-eg-cov.lcov
```

M-D net-new line ranges (remediated tip bc23b9f8):
`proxy_request` [1306-1741], `validate_request_trailers` [1956-1973],
`concat_chunks` [1978-1987], `record_retained` [2356-2370].

Deterministic result (3× identical):
```
proxy_request (pump)      : 205/247 = 83.00%
validate_request_trailers : 8/17   = 47.06%
concat_chunks             : 10/10  = 100.00%
record_retained           : 11/12  = 91.67%
SESSION TOTAL (M-D)       : 234/286 = 81.82%   (need >=80% => >=229 covered)
```

**Uncovered session lines (52, deterministic):**
```
1402 1404 1411 1412 1413 1414 1439 1461 1462 1463 1466 1467 1613 1639 1640
1654 1656 1657 1658 1660 1661 1662 1674 1675 1684 1686 1687 1688 1689 1690
1691 1692 1693 1694 1695 1713 1714 1715 1735 1736 1737 1738 1961 1962 1963
1964 1965 1966 1967 1968 1969 2367
```
Honest characterization of the residual uncovered set (all defensive/edge
paths, none of them the central streaming happy path which is now fully
covered):
- 1961-1969 (`validate_request_trailers` loop body): only entered when a
  request carries non-empty trailers; most tests use trailer-less bodies and
  the raw-frame trailer cases are flow-control-rejected before reaching it.
- 1654-1695 (streaming-regime trailers-validation + mid-stream BodyTooLarge cap
  + post-dial inbound-protocol-error verdict): reachable only when a >window
  stream turns malformed/over-cap AFTER the dial via the real h2 path; the raw
  malformed cases reject before crossing the window.
- 1402/1404/1411-1414 (413 total-cap during lookahead / trailers-in-lookahead),
  1439/1461-1467 (Branch-A upstream send error/timeout mapping), 1713-1738
  (Branch-B send_request error/timeout + pump-vanished verdict): error/timeout
  arms.

**Coverage bar: PASS (81.82% ≥ 80%).**

### 8. R3 / fmt / clippy — **PASS**
- `cargo test -p lb-l7 --lib`: `91 passed`.
- `cargo fmt --check`: clean (the verifier test file was re-formatted; the src
  is untouched).
- `cargo clippy --features test-gauges --tests -p lb-integration-tests`: clean,
  no warnings.

---

## Summary table

| BUILT-bar item | Result | One-line evidence |
|---|---|---|
| 1. Real-wire binary, Branch A (8 KiB) | PASS | byte-identical, 1 dial |
| 1. Real-wire binary, Branch B (512 KiB) | PASS | byte-identical, 1 dial (round-1 RED, harness fixed) |
| 1. Independent reqwest 512 KiB | PASS | byte-identical (independent H2 stack) |
| 2. Non-vacuous memory + inverted probe | PASS | in-situ 80 KiB; live-occupancy peak 80 KiB for 4 MiB stream (F-MD-3 resolved) |
| 3. Backpressure pause (Phase 1) | PASS | paused_at≈2.9 MiB < 4 MiB |
| 3. Backpressure resume (Phase 2) | PASS | status=200, resumed_at=4 MiB (F-MD-1 unblocked) |
| 4. Existing 3 gate tests UNCHANGED | PASS | 3 passed, not edited |
| 4. Over-window malformed no-leak (a)+(b) | PASS | no downstream backend DATA relayed |
| 5. Deviation #1 early-response relay | PASS | client got 401 (F-MD-2 fixed) |
| 6. Smuggling parity | PASS | dials=1 (post-dial abort reached), complete=0 |
| 7. Coverage ≥80% session sub-metric | PASS | 81.82% (234/286), 3× deterministic |
| 8. R3 / fmt / clippy | PASS | 91 unit tests; fmt+clippy clean |

## Environment notes
- Shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`. Disk held at ~17 GB
  free through the single instrumented coverage build (above the 14 GB floor;
  did NOT `cargo clean`). One instrumented lcov reused across the awk runs;
  the 3× determinism check re-ran the (cached) instrumented binaries only.
- The harness-bug fix is confined to the verifier's own test file
  (`tests/h2h1_md_streaming_verify.rs`); the builder src under test is the
  exact `origin/s8/builder-1` tip `bc23b9f8`, diff-verified empty.
