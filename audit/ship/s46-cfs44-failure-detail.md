# CF-S44 — failure characterization across all 13 failing jobs

Companion to `audit/ship/s46-cfs44-base-rate.md`. Read-only; all facts from the 142 CI logs already
downloaded to `scratchpad/s46/ci-logs/`. No log was unavailable.

---

## 1. Wall time of every failing test — the 30 s timeout hypothesis is dead

### Coverage jobs — nextest reports per-test wall time directly

| Job | Date | Test | **Wall** | Size |
|---|---|---|---|---|
| 80910684353 | 2026-06-11 | `grpc_h3_trailer_survives_all_response_sizes` | **0.187 s** | `sz=262144` |
| 82299811405 | 2026-06-19 | `grpc_h3_trailer_survives_all_response_sizes` | **0.363 s** | `sz=524288` |
| 82655243223 | 2026-06-22 | `grpc_h3_large_message_roundtrips_byte_identical` | **0.143 s** | 512 KiB |
| 82997390138 | 2026-06-23 | `grpc_h3_trailer_survives_all_response_sizes` | **0.212 s** | `sz=262144` |
| 91093738779 | 2026-07-31 | `grpc_h3_trailer_survives_all_response_sizes` | **0.528 s** | `sz=1048576` |
| 91486316310 | 2026-08-02 | `grpc_h3_trailer_survives_any_frame_granularity` | **0.177 s** | `sz=524288` |
| 91548350113 | 2026-08-02 | `grpc_h3_trailer_survives_all_response_sizes` | **0.232 s** | `sz=262144` |
| 93792425686 | 2026-08-11 | `grpc_h3_large_message_roundtrips_byte_identical` | **0.143 s** | 512 KiB |

**Range 0.143 s – 0.528 s. Nothing exceeds 5 s. Nothing is remotely near 30 s.**

### Test jobs — libtest reports no per-test time; the binary total is a hard upper bound

libtest emits `test <name> ... FAILED` with no duration. The only timing is the binary total. Since
`grpc_h3_e2e` is serialized by `serial_guard!()`, the binary total bounds the failing test from
above:

| Job | Date | `test result:` line (verbatim) | Bound on failing test |
|---|---|---|---|
| 80975636349 | 2026-06-12 | `test result: FAILED. 14 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.02s` | ≤ 5.02 s |
| 82299811430 | 2026-06-19 | `test result: FAILED. 15 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.10s` | ≤ 5.10 s |
| 82655243197 | 2026-06-22 | `test result: FAILED. 14 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 5.29s` | ≤ 5.29 s |
| 82894888272 | 2026-06-23 | `test result: FAILED. 14 passed; 2 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.76s` | ≤ 4.76 s |
| 91501727350 | 2026-08-02 | `test result: FAILED. 15 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.89s` | ≤ 3.89 s |

That bound covers **all 16 tests** in the binary, so the individual failing test is far below it.

**Verdict: 13 of 13 failures are FAST. Zero exceeded ~5 s.** The `Http2PoolError::Timeout` variant
(30 s `send_timeout`) is excluded as the mechanism for every recorded failure. No failure reopens
the timeout hypothesis.

---

## 2. Verbatim failure blocks

Every Coverage block has an identical shape. Representative (job 80910684353); the others differ
only in test name, size and timing:

```
        FAIL [   0.187s] (1255/1565) lb-quic::grpc_h3_e2e grpc_h3_trailer_survives_all_response_sizes

  stdout ───

    running 1 test
    test grpc_h3_trailer_survives_all_response_sizes ... FAILED

    failures:

    failures:
        grpc_h3_trailer_survives_all_response_sizes

    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 15 filtered out; finished in 0.18s

  stderr ───

    thread 'grpc_h3_trailer_survives_all_response_sizes' panicked at crates/lb-quic/tests/grpc_h3_e2e.rs:1242:9:
    assertion `left == right` failed: sz=262144
      left: Some(502)
     right: Some(200)
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
```

The distinct assertion texts across all 13 jobs, verbatim:

```
crates/lb-quic/tests/grpc_h3_e2e.rs:1242:9:
assertion `left == right` failed: sz=262144
  left: Some(502)
 right: Some(200)
```
```
crates/lb-quic/tests/grpc_h3_e2e.rs:1242:9:
assertion `left == right` failed: sz=524288
  left: Some(502)
 right: Some(200)
```
```
crates/lb-quic/tests/grpc_h3_e2e.rs:1242:9:
assertion `left == right` failed: sz=1048576
  left: Some(502)
 right: Some(200)
```
```
crates/lb-quic/tests/grpc_h3_e2e.rs:1198:5:      (and :1111:5 at the older SHA)
assertion `left == right` failed
  left: Some(502)
 right: Some(200)
```
```
crates/lb-quic/tests/grpc_h3_e2e.rs:1285:13:
assertion `left == right` failed: giant frames, sz=524288: grpc-status trailer dropped (F-S29-1)
  left: None
 right: Some("0")
```
```
crates/lb-quic/tests/grpc_h3_e2e.rs:1285:13:
assertion `left == right` failed: small frames, sz=524288: grpc-status trailer dropped (F-S29-1)
  left: None
 right: Some("0")
```

**No gateway/tracing output accompanies any of the 13.** stderr contains only the panic and the
`RUST_BACKTRACE` note. The signature is byte-identical in shape across two months.

---

## 3. The 4 TRAILER-DROP cases — DROPPED (absent), never MANGLED; but NOT provably distinct from the 502

| Job | Type | Date | Test | Backend mode | Size | Assertion |
|---|---|---|---|---|---|---|
| 91486316310 | Coverage | 2026-08-02 | `..._any_frame_granularity` | **giant** | `sz=524288` | `left: None` / `right: Some("0")` |
| 82894888272 | Test | 2026-06-23 | `..._any_frame_granularity` | **giant** | `sz=524288` | `left: None` / `right: Some("0")` |
| 82655243197 | Test | 2026-06-22 | `..._any_frame_granularity` | **small** | `sz=524288` | `left: None` / `right: Some("0")` |
| 91501727350 | Test | 2026-08-02 | `..._any_frame_granularity` | **small** | `sz=524288` | `left: None` / `right: Some("0")` |

**DROPPED, not MANGLED.** All 4 are exactly `left: None`. Not one shows a truncated, empty-string,
or wrong-valued `grpc-status`. The header is absent, never corrupt.

**Both backend modes are affected** (2 giant, 2 small), and **all 4 are at `sz=524288`** — always
the *first* size in that test's inner loop `for sz in [512*1024, 1024*1024]`, so the 1 MiB arm never
executed after the abort.

### The important caveat — this may not be a separate bug

`grpc_h3_trailer_survives_any_frame_granularity` asserts **`grpc-status` first and never asserts
`:status` at all**. And `field()` (`grpc_h3_e2e.rs:329`) resolves a name by searching
`trailer_pairs` then `head_pairs`:

```rust
fn field(&self, name: &str) -> Option<&str> {
    self.trailer_pairs
        .iter()
        .chain(self.head_pairs.iter())
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}
```

A **502 response carries no `grpc-status` in either headers or trailers**, so a 502 in this test
produces *exactly* `left: None, right: Some("0")` — indistinguishable in the log from a genuine
trailer drop on an otherwise-good 200 response.

**Therefore: whether the response was `200`-with-missing-trailer or `502`-with-no-trailer is NOT
DETERMINABLE from these logs.** The test records no `:status` and no body length. Supporting the
"it is just the 502" reading: in *every* test that does assert `:status` first
(`..._all_response_sizes` at :1242, `..._large_message...` at :1198/:1111), the observed value is
`Some(502)` — the logs contain no instance of a confirmed 200-with-dropped-trailer.

An earlier report of mine called this a distinct second failure mode. That was an inference from
`left: None` and is **not supported by the evidence**; all 13 failures are consistent with a single
502 mode. Distinguishing them requires the `RUST_LOG` repro (U3) or an added `:status` assertion.

### A second, independent refutation of size-dependence

In jobs 82655243197 and 91501727350 the failing arm is **`small frames, sz=524288`**. Because the
outer loop runs `giant` first to completion, **the giant-mode 512 KiB *and* 1 MiB transfers both
succeeded moments earlier in the same test execution**, against a gateway in the same process. The
very next 512 KiB transfer then failed. Size cannot be the discriminant.

---

## 4. Co-occurrence of the two signatures

Two jobs show both. In **both cases they are in the SAME job and DIFFERENT tests**:

- **82894888272** (Test, 2026-06-23, run 28008190008): `..._all_response_sizes` 502 at `sz=262144`
  **and** `..._any_frame_granularity` `None` at `giant frames, sz=524288`.
- **82655243197** (Test, 2026-06-22, run 27935209993): `..._all_response_sizes` 502 at `sz=262144`
  **and** `..._any_frame_granularity` `None` at `small frames, sz=524288`.

Both affected tests drive large bodies. Consistent with one underlying fault hitting two tests in
the same binary run, not two independent faults.

---

## 5. The SAME COMMIT both passed and failed — 14 distinct SHAs

Restricting to jobs where the grpc_h3 tests actually executed:

| SHA | Run | Passing job | Failing job |
|---|---|---|---|
| `254c6843` | 28038262159 | Test 82997390188 PASS | Coverage 82997390138 **HANG** |
| `31d6cc92` | 30743993262 | Test 91486316228 PASS | Coverage 91486316310 **FAIL** |
| `3c32d5e8` | 28008190008 | Coverage 82894888199 PASS | Test 82894888272 **FAIL** |
| `499cee2c` | 31571846514 | Test 94035324018 PASS | Coverage 94035323924 **HANG** |
| `4bd93952` | 28354172152 | Test 83993264280 PASS | Coverage 83993264297 **HANG** |
| `50893462` | 27375944617 | Test 80900069234 PASS | Coverage 80900068966 **HANG** |
| `5c9f0752` | 31495640064 | Coverage att2 93816169955 PASS | Coverage att1 93792523664 **HANG** |
| `6c81222f` | 30755744744 | Test att1 91517507558 PASS | Coverage att1 91517507600 **HANG** + att2 91548350113 **FAIL** |
| `76ab0fdb` | 31495609830 | Test 93792425639 PASS | Coverage 93792425686 **FAIL** |
| `90a0dc97` | 31489960428 | Test 93773840615 PASS | Coverage 93773840775 **HANG** |
| `aac0039a` | 27399993282 | Coverage 80975636265 PASS | Test 80975636349 **FAIL** |
| `e9df7cca` | 30749813681 | Test att2 91503547179 PASS | Test att1 91501727350 **FAIL** |
| `ed4da323` | 30611075578 | Test 91093738631 PASS | Coverage 91093738779 **FAIL** |
| `ffac8705` | 27379018511 | Test 80910684273 PASS | Coverage 80910684353 **FAIL** |

Two cases are the strongest possible form — **identical SHA, identical job definition, different
attempt**:

- **`e9df7cca`** (run 30749813681): `Test` attempt 1 **FAILED**, `Test` attempt 2 **PASSED**.
- **`5c9f0752`** (run 31495640064): `Coverage` attempt 1 **HUNG**, `Coverage` attempt 2 **PASSED**.
- **`6c81222f`** (run 30755744744): `Coverage` attempt 1 **HUNG**, attempt 2 **FAILED** at
  `sz=262144` — two *different* failures from one commit.

All runs are on `ubuntu-24.04`. The input is unchanged and the outcome varies: **the failure is
nondeterministic.** "Someone changed the code" is foreclosed.

Only 2 affected runs are *not* in this table (27810605810 / `28a4e6ea` and 27935209993 /
`98a63d18`) — in those, **both** the Coverage and the Test job failed, so no passing observation
exists for that SHA.

---

## 6. The 7 hangs — every hung test drives a TINY payload

### Job internals

The `grpc_h3_e2e` binary holds 16 tests. In every hang job the entire rest of the binary completed:

| Job | Date | Hung test | grpc_h3 PASS in same job | Other grpc_h3 FAIL in same job | Global progress |
|---|---|---|---|---|---|
| 80900068966 | 2026-06-11 | `..._without_te_header_still_delivers_trailer` | 15 / 16 | none | 1564/1565 |
| 82997390138 | 2026-06-23 | `..._unary_echo_delivers_status_trailer` | 14 / 16 | **`..._trailer_survives_all_response_sizes` (502, sz=262144)** | 1564/1565 |
| 83993264297 | 2026-06-29 | `..._unary_echo_delivers_status_trailer` | 15 / 16 | none | 1564/1565 |
| 91517507600 | 2026-08-02 | `..._without_te_header_still_delivers_trailer` | 15 / 16 | none | 1564/1565 |
| 93773840775 | 2026-08-11 | `..._trailers_only_immediate_error_preserved` | 15 / 16 | none | 1564/1565 |
| 93792523664 | 2026-08-11 | `..._without_te_header_still_delivers_trailer` | 15 / 16 | none | 1564/1565 |
| 94035323924 | 2026-08-12 | `..._client_stream_relays_all_request_messages` | 15 / 16 | none | 1564/1565 |

Only **one** of the 7 hang jobs also carried a 502 (82997390138). In the other 6, the hang is the
sole grpc_h3 signal and the whole workspace reached 1564/1565.

### Payload sizes — the decisive finding

Read from `grpc_h3_e2e.rs` at SHA `6c81222f`:

| Hung test | Payload driven | Bytes |
|---|---|---|
| `grpc_h3_without_te_header_still_delivers_trailer` | `Bytes::from_static(b"no-te")` | **5 B** |
| `grpc_h3_unary_echo_delivers_status_trailer` | `Bytes::from_static(b"hello grpc over h3")` | **18 B** |
| `grpc_h3_trailers_only_immediate_error_preserved` | `frame_messages(&[Bytes::from_static(b"x")])` | **1 B** |
| `grpc_h3_client_stream_relays_all_request_messages` | 12 msgs `m0`..`m11` | **~30 B** |

**Every hung test drives a payload of tens of bytes. Not one drives a large body.**

Conversely every 502/`None` failure is on a **≥256 KiB** body, and `sz=1` has never failed in any
of the 13 failing jobs.

**The two populations are disjoint:**

- **Hang** → small-payload tests only (max 30 B), 4 distinct tests, Coverage jobs only (0 of 71
  Test jobs ever hung).
- **502** → large-body tests only (≥256 KiB), 3 distinct tests, both Coverage and Test jobs.

No large-body test has ever hung; no small-body test has ever 502'd. **These are very likely two
different bugs and should be written up separately.** The lead's suspicion is confirmed by the
source, not merely by the log.

One further asymmetry: the hang is exclusive to the instrumented `Coverage` job (7/71 Coverage,
0/71 Test), whereas the 502 appears in both (8/71 Coverage, 5/71 Test). The hang may well be
load- or instrumentation-sensitive in a way the 502 is not.

---

## 7. Denominator refinement (supersedes the rate in the base-rate note)

52 of the 142 scanned jobs never executed the grpc_h3 tests at all — they were cancelled early or
failed to compile (e.g. `error[E0432]: unresolved import 'aya::programs::XdpFlags'` on dependabot
branches). Excluding those non-exposed jobs:

| Denominator | Affected | Rate |
|---|---|---|
| 66 runs with a Coverage job (as first reported) | 16 | 24.2% |
| **42 runs where grpc_h3 actually executed** | **16** | **38.1%** |
| **90 jobs where grpc_h3 actually executed** | **19** | **21.1%** |

The 38.1% figure is the honest per-run exposure rate: **when the grpc-over-H3 suite actually runs,
better than one run in three shows a 502 or a hang.**
