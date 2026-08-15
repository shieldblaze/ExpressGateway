# CF-S44 base rate across CI history

Read-only forensics over the complete GitHub Actions history of the `CI` workflow
(`.github/workflows/ci.yml`, workflow id `259389976`), repo `shieldblaze/ExpressGateway`.

## Method and denominator

| Item | Value |
|---|---|
| CI workflow runs in existence (all time) | 164 (2026-04-11 → 2026-08-14) |
| Job rows enumerated (all attempts, `filter=all`) | 2348 |
| Runs containing a `Coverage` job | **66** |
| `Coverage` job logs downloaded and grepped | **71** (66 runs; 5 are attempt-2 re-runs) |
| `Test` job logs downloaded and grepped, same 66 runs | **71** |
| **Total logs examined** | **142** |
| **Logs unavailable / expired** | **0** |

The `Coverage` job first appears 2026-06-11; `crates/lb-quic/tests/grpc_h3_e2e.rs` landed
2026-06-02 (S29, commit `5c7f1c51`). So the Coverage era is fully inside the grpc_h3 era and 66
runs is the honest denominator. Nothing was extrapolated over missing data — no data was missing.

Signals grepped per log: nextest `FAIL [...] lb-quic::grpc_h3_e2e <test>`, cargo-test
`test <grpc_h3_*> ... FAILED`, `thread '<grpc_h3_*>' panicked`, `SLOW [>...] lb-quic::grpc_h3_e2e`,
`Some(502)`, `failed: sz=`, `Terminate orphan process`, `RUST_LOG`.

Note both log formats matter: the `Coverage` job runs `cargo llvm-cov nextest` (nextest output),
the `Test` job runs plain `cargo test` (libtest output). A nextest-only grep silently misses every
`Test`-job failure.

## Headline counts (distinct runs, denominator 66)

| Class | Distinct runs | Rate |
|---|---|---|
| (a) grpc_h3 **502** assertion failure | **9** | 13.6% |
| (b) grpc_h3 **hang** (SLOW ladder, never returns) | **7** | 10.6% |
| (c) grpc_h3 **trailer drop** (`left: None`, F-S29-1 signature) | **4** | 6.1% |
| **Any of the above** | **16** | **24.2%** |
| **Neither / clean** | **50** | 75.8% |

Classes overlap: one run had both a 502 and a hang, two runs had both a 502 and a trailer drop,
one run had a hang on attempt 1 and a 502 on attempt 2.

Job-level: 14 of 71 Coverage jobs (19.7%) and 5 of 71 Test jobs (7.0%) carried a grpc_h3 signal.

## (a)+(c) — every grpc_h3 FAIL in history

`Coverage` jobs (instrumented, `cargo llvm-cov nextest --workspace --all-features --ignore-run-fail`):

| Run | Job | Att | Job concl | Date | Test | Mode | Size |
|---|---|---|---|---|---|---|---|
| 27379018511 | 80910684353 | 1 | success | 2026-06-11 | `grpc_h3_trailer_survives_all_response_sizes` | 502 | `sz=262144` |
| 27810605810 | 82299811405 | 1 | failure | 2026-06-19 | `grpc_h3_trailer_survives_all_response_sizes` | 502 | `sz=524288` |
| 27935209993 | 82655243223 | 1 | failure | 2026-06-22 | `grpc_h3_large_message_roundtrips_byte_identical` | 502 | (single 512 KiB req) |
| 28038262159 | 82997390138 | 1 | cancelled | 2026-06-23 | `grpc_h3_trailer_survives_all_response_sizes` | 502 | `sz=262144` |
| 30611075578 | 91093738779 | 1 | failure | 2026-07-31 | `grpc_h3_trailer_survives_all_response_sizes` | 502 | `sz=1048576` |
| 30743993262 | 91486316310 | 1 | success | 2026-08-02 | `grpc_h3_trailer_survives_any_frame_granularity` | **TRAILER DROP** | `sz=524288` (giant) |
| 30755744744 | 91548350113 | 2 | success | 2026-08-02 | `grpc_h3_trailer_survives_all_response_sizes` | 502 | `sz=262144` |
| 31495609830 | 93792425686 | 1 | success | 2026-08-11 | `grpc_h3_large_message_roundtrips_byte_identical` | 502 | (single 512 KiB req) |

`Test` jobs (**uninstrumented**, plain `cargo test --workspace --all-features --no-fail-fast`):

| Run | Job | Att | Job concl | Date | Test(s) | Mode | Size |
|---|---|---|---|---|---|---|---|
| 27399993282 | 80975636349 | 1 | failure | 2026-06-12 | `grpc_h3_large_message_roundtrips_byte_identical`, `grpc_h3_trailer_survives_all_response_sizes` | 502 | `sz=262144` |
| 27810605810 | 82299811430 | 1 | failure | 2026-06-19 | `grpc_h3_trailer_survives_all_response_sizes` | 502 | `sz=262144` |
| 27935209993 | 82655243197 | 1 | failure | 2026-06-22 | `grpc_h3_trailer_survives_all_response_sizes`, `grpc_h3_trailer_survives_any_frame_granularity` | 502 + **TRAILER DROP** | `sz=262144`, `sz=524288` |
| 28008190008 | 82894888272 | 1 | failure | 2026-06-23 | `grpc_h3_trailer_survives_all_response_sizes`, `grpc_h3_trailer_survives_any_frame_granularity` | 502 + **TRAILER DROP** | `sz=262144`, `sz=524288` |
| 30749813681 | 91501727350 | 1 | failure | 2026-08-02 | `grpc_h3_trailer_survives_any_frame_granularity` | **TRAILER DROP** | `sz=524288` (small) |

Runs 27810605810 and 27935209993 failed in **both** the Coverage and the Test job.

## (b) — every grpc_h3 hang in history (all in `Coverage`; zero in `Test`)

| Run | Job | Job concl | Date | Hung test | Max SLOW reached |
|---|---|---|---|---|---|
| 27375944617 | 80900068966 | cancelled | 2026-06-11 | `grpc_h3_without_te_header_still_delivers_trailer` | `>2940.000s` |
| 28038262159 | 82997390138 | cancelled | 2026-06-23 | `grpc_h3_unary_echo_delivers_status_trailer` | `>21060.000s` |
| 28354172152 | 83993264297 | cancelled | 2026-06-29 | `grpc_h3_unary_echo_delivers_status_trailer` | `>21060.000s` |
| 30755744744 | 91517507600 | cancelled | 2026-08-02 | `grpc_h3_without_te_header_still_delivers_trailer` | `>17940.000s` |
| 31489960428 | 93773840775 | cancelled | 2026-08-11 | `grpc_h3_trailers_only_immediate_error_preserved` | `>780.000s` |
| 31495640064 | 93792523664 | cancelled | 2026-08-11 | `grpc_h3_without_te_header_still_delivers_trailer` | `>4080.000s` |
| 31571846514 | 94035323924 | cancelled | 2026-08-12 | `grpc_h3_client_stream_relays_all_request_messages` | `>21060.000s` |

Four distinct tests hang; `grpc_h3_without_te_header_still_delivers_trailer` is simply the most
frequent (3/7), not the only one. Three jobs reached `>21060s` (5 h 51 m) — the GitHub 6 h ceiling.
Every hang job ends in `Terminate orphan process: ... (cargo-nextest)` and
`... (grpc_h3_e2e-<hash>)`.

## Q4 — gateway tracing: none exists

- `RUST_LOG` appears in **0** of the 142 logs, and `grep -rn RUST_LOG .github/` returns nothing.
- The strings `H3→H2 stream send_request failed`, `upstream dial failed`, `h2 handshake failed`,
  `h2 send_request failed`, `h2 send_request timed out`, `bad gateway` appear in **0 of 142** logs.

No CI run has ever captured gateway tracing around a grpc_h3 failure. U3 cannot be answered from
CI history; it needs a local repro with `RUST_LOG` set.

## Q5 — clustering

- **Runner image:** `ubuntu-24.04` in **all 142** logs. No image split, so no image correlation.
- **Date:** affected runs are 2026-06-11(2), 06-12, 06-19, 06-22, 06-23(2), 06-29, 07-31,
  08-02(3), 08-11(3), 08-12. Per month: June 8/26 (30.8%), July 1/10 (10%), August 7/30 (23.3%).
  Present across the entire window with no onset date and no clustering on a specific day or image.

## What the base rate establishes

1. **Not a 256 KiB threshold.** The 502 fires at `sz=262144`, `sz=524288` and `sz=1048576`, and on
   `grpc_h3_large_message_roundtrips_byte_identical` (a single 512 KiB request).
2. **Not connection reuse.** `grpc_h3_large_message_roundtrips_byte_identical` issues exactly ONE
   request against a freshly spawned backend + gateway + default pool — no prior request exists to
   pool a connection from. It returned 502 in 3 jobs. Likewise the failing
   `grpc_h3_trailer_survives_any_frame_granularity` arm `giant frames, sz=524288` is the FIRST
   request against a gateway created moments earlier in that loop iteration.
3. **Not instrumentation-only.** 5 uninstrumented `Test` jobs carry the same failures.
4. **`sz=1` never fails.** Across all 13 failing jobs, no failure at the 1-byte size. The common
   factor is a large (≥256 KiB) response body, not a specific size and not a reused connection.
5. **A second, distinct failure mode exists**: the grpc-status trailer is *dropped*
   (`left: None, right: Some("0")`) rather than a 502 — the exact F-S29-1 signature the test was
   written to guard, in 4 runs.
