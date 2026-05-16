# Round-9 Phase-B gate-runner transcripts
HEAD: 369df2ce (feature/h3-green)
Date: 2026-05-16

## STEP 0 — static re-confirm on 369df2ce
- cargo deny check: DENY_EXIT=0  -> "advisories ok, bans ok, licenses ok, sources ok" (only advisory-not-detected warnings for unused waivers)
- cargo fmt --check: FMT_EXIT=0  (no output)
- cargo clippy --all-targets --all-features -- -D warnings: CLIPPY_EXIT=0
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 29.48s
VERDICT STEP 0: PASS (no regression after iteration-2 commits)

## STEP 1 — Task #10: D-3a chaos suite
Documented invocation (audit/round-8/regression/deferred-env.md:80):
  cargo test -p lb-integration-tests --release --test chaos -- --ignored --nocapture

Findings:
- No chaos.rs / soak.rs anywhere in tests/ or crates/*/tests/.
- `find ... -name '*chaos*'` -> 0 results.
- `grep -rl chaos` over all tests dirs -> 0 results.
- `cargo test -p lb-integration-tests --release --test chaos -- --list` -> CHAOS_EXIT=101
    error: no test target named `chaos`.
  (68 available test targets listed; none named chaos or soak)

VERDICT STEP 1: BLOCKED — the D-3a chaos test target is NOT IMPLEMENTED in the
tree at HEAD 369df2ce. This is a missing-artifact blocker, not an infra blocker:
no scaled/local run is possible because there is no chaos test to compile/run.
Exact blocker: test target `chaos` (and `soak`) absent from lb-integration-tests.

## STEP 2 — Task #8: D-6 per-crate scoped llvm-cov sweep
Tooling: cargo-llvm-cov 0.8.7, llvm-tools installed, rustc 1.85.1 (toolchain pin 1.85).
NOTE: cargo-llvm-cov instrument-coverage emits region% not branch% -> Branches column shows '-' for every file (no per-branch data available from this toolchain). Region% used as the branch proxy where relevant; line% is the authoritative gate metric.
NOTE: per-crate `cargo llvm-cov -p <crate>` runs ONLY that crate's own unit tests, NOT the workspace tests/ integration suite — modules whose coverage is closed by integration tests (quic listener/conn_actor, xdp loader) read low here by construction.

### cargo llvm-cov -p (crate=config) — verbatim summary table
```
Filename                                                         Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
/home/ubuntu/Code/ExpressGateway/crates/lb-config/src/lib.rs         420                62    85.24%          98                 4    95.92%        1441               155    89.24%           0                 0         -
----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                                                                420                62    85.24%          98                 4    95.92%        1441               155    89.24%           0                 0         -
```

### cargo llvm-cov -p (crate=balancer) — verbatim summary table
```
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
ewma.rs                            36                 5    86.11%           6                 0   100.00%          83                 4    95.18%           0                 0         -
least_connections.rs               15                 1    93.33%           4                 0   100.00%          39                 1    97.44%           0                 0         -
least_request.rs                   13                 1    92.31%           3                 0   100.00%          29                 1    96.55%           0                 0         -
lib.rs                             22                 1    95.45%           4                 0   100.00%          57                 1    98.25%           0                 0         -
maglev.rs                          85                14    83.53%          13                 1    92.31%         157                10    93.63%           0                 0         -
p2c.rs                             29                 8    72.41%           5                 1    80.00%          53                 9    83.02%           0                 0         -
random.rs                          21                 4    80.95%           5                 1    80.00%          29                 5    82.76%           0                 0         -
ring_hash.rs                       73                11    84.93%          13                 1    92.31%         139                10    92.81%           0                 0         -
round_robin.rs                     14                 0   100.00%           5                 0   100.00%          24                 0   100.00%           0                 0         -
session_affinity.rs                32                 4    87.50%           7                 0   100.00%          47                 2    95.74%           0                 0         -
weighted_random.rs                 27                 5    81.48%           5                 1    80.00%          44                 6    86.36%           0                 0         -
weighted_round_robin.rs            65                 9    86.15%          10                 1    90.00%         102                 7    93.14%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                             432                63    85.42%          80                 6    92.50%         803                56    93.03%           0                 0         -
```

### cargo llvm-cov -p (crate=l7) — verbatim summary table
```
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
authority.rs                       25                 0   100.00%           8                 0   100.00%          64                 0   100.00%           0                 0         -
grpc_proxy.rs                     268               122    54.48%          52                17    67.31%         485               165    65.98%           0                 0         -
h1_proxy.rs                       670               328    51.04%         137                72    47.45%        1403               573    59.16%           0                 0         -
h1_to_h1.rs                        50                 9    82.00%          16                 2    87.50%         102                 8    92.16%           0                 0         -
h1_to_h2.rs                        63                12    80.95%          17                 1    94.12%         113                 8    92.92%           0                 0         -
h1_to_h3.rs                        63                12    80.95%          17                 1    94.12%         113                 8    92.92%           0                 0         -
h2_proxy.rs                       565               394    30.27%         115                75    34.78%        1246               696    44.14%           0                 0         -
h2_security.rs                      9                 1    88.89%           5                 1    80.00%          58                 3    94.83%           0                 0         -
h2_to_h1.rs                        92                18    80.43%          19                 1    94.74%         161                14    91.30%           0                 0         -
h2_to_h2.rs                        17                 2    88.24%           6                 0   100.00%          47                 0   100.00%           0                 0         -
h2_to_h3.rs                        17                 2    88.24%           6                 0   100.00%          47                 0   100.00%           0                 0         -
h3_to_h1.rs                        44                 7    84.09%           9                 0   100.00%          70                 3    95.71%           0                 0         -
h3_to_h2.rs                        17                 2    88.24%           6                 0   100.00%          47                 0   100.00%           0                 0         -
h3_to_h3.rs                        17                 2    88.24%           6                 0   100.00%          47                 0   100.00%           0                 0         -
lib.rs                             66                 7    89.39%          19                 2    89.47%         219                19    91.32%           0                 0         -
security_hooks.rs                  14                 2    85.71%          11                 2    81.82%          44                 6    86.36%           0                 0         -
sni_authority.rs                   55                 1    98.18%          16                 0   100.00%          87                 2    97.70%           0                 0         -
stripped_request.rs                14                 1    92.86%           9                 1    88.89%          43                 3    93.02%           0                 0         -
trace_ctx.rs                       82                 9    89.02%          27                 4    85.19%         190                10    94.74%           0                 0         -
upstream.rs                        31                 2    93.55%          10                 0   100.00%          86                 1    98.84%           0                 0         -
ws_proxy.rs                       172                66    61.63%          37                 2    94.59%         414                66    84.06%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                            2351               999    57.51%         548               181    66.97%        5086              1585    68.84%           0                 0         -
```

### cargo llvm-cov -p (crate=security) — verbatim summary table
```
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
admin_auth.rs                     107                10    90.65%          28                 0   100.00%         163                 0   100.00%           0                 0         -
conn_gate.rs                       39                 5    87.18%          12                 1    91.67%          82                 7    91.46%           0                 0         -
glitches.rs                        74                 3    95.95%          15                 1    93.33%         135                 4    97.04%           0                 0         -
handshake.rs                       33                11    66.67%          11                 3    72.73%         113                18    84.07%           0                 0         -
hooks.rs                           43                 4    90.70%          13                 1    92.31%         105                 3    97.14%           0                 0         -
key.rs                             38                 4    89.47%          12                 0   100.00%          90                 1    98.89%           0                 0         -
lib.rs                             65                 0   100.00%          26                 0   100.00%         146                 0   100.00%           0                 0         -
retry.rs                           83                23    72.29%          17                 4    76.47%         184                31    83.15%           0                 0         -
slow_post.rs                       11                 2    81.82%           2                 0   100.00%          52                 2    96.15%           0                 0         -
slowloris.rs                       11                 0   100.00%           3                 0   100.00%          46                 0   100.00%           0                 0         -
smuggle.rs                         89                 6    93.26%          10                 0   100.00%         106                 1    99.06%           0                 0         -
ticket.rs                         201                49    75.62%          61                22    63.93%         466                81    82.62%           0                 0         -
watchdog.rs                        66                 6    90.91%          17                 2    88.24%         163                 7    95.71%           0                 0         -
zero_rtt.rs                        89                14    84.27%          19                 1    94.74%         163                19    88.34%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                             949               137    85.56%         246                35    85.77%        2014               174    91.36%           0                 0         -
```

### cargo llvm-cov -p (crate=quic) — verbatim summary table
```
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
cleanup_guard.rs                   10                 2    80.00%           3                 1    66.67%          18                 3    83.33%           0                 0         -
conn_actor.rs                     161                59    63.35%          16                 2    87.50%         251                61    75.70%           0                 0         -
h3_bridge.rs                      397               314    20.91%          37                24    35.14%         554               403    27.26%           0                 0         -
lib.rs                            292               274     6.16%          49                43    12.24%         459               413    10.02%           0                 0         -
listener.rs                        93                93     0.00%          19                19     0.00%         216               216     0.00%           0                 0         -
router.rs                         140               118    15.71%          22                18    18.18%         489               252    48.47%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                            1093               860    21.32%         146               107    26.71%        1987              1348    32.16%           0                 0         -
```

### cargo llvm-cov -p (crate=obs) — verbatim summary table
```
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
admin_http.rs                     133                33    75.19%          27                 7    74.07%         258                51    80.23%           0                 0         -
label_budget.rs                    75                 7    90.67%          25                 5    80.00%         224                32    85.71%           0                 0         -
lib.rs                            205                94    54.15%          33                 9    72.73%         332               126    62.05%           0                 0         -
log.rs                             31                14    54.84%           8                 4    50.00%          56                32    42.86%           0                 0         -
probes.rs                          51                 5    90.20%          17                 1    94.12%          94                 4    95.74%           0                 0         -
prometheus_exposition.rs           50                17    66.00%           7                 1    85.71%          65                 8    87.69%           0                 0         -
tracing_propagation.rs            144                20    86.11%          24                 0   100.00%         224                 6    97.32%           0                 0         -
xdp_metrics.rs                    105                11    89.52%          15                 1    93.33%         174                 2    98.85%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                             794               201    74.69%         156                28    82.05%        1427               261    81.71%           0                 0         -
```

### cargo llvm-cov -p (crate=xdp) — verbatim summary table
```
Filename                      Regions    Missed Regions     Cover   Functions  Missed Functions  Executed       Lines      Missed Lines     Cover    Branches   Missed Branches     Cover
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
bpffs.rs                           34                 8    76.47%           7                 1    85.71%          82                14    82.93%           0                 0         -
lib.rs                            188                16    91.49%          37                 1    97.30%         356                10    97.19%           0                 0         -
loader.rs                         364               263    27.75%          61                33    45.90%         680               329    51.62%           0                 0         -
netlink_xdp.rs                    205                86    58.05%          25                11    56.00%         215                98    54.42%           0                 0         -
nic_compat.rs                     108                30    72.22%          24                 5    79.17%         160                43    73.12%           0                 0         -
sim.rs                            108                15    86.11%          22                 0   100.00%         211                 7    96.68%           0                 0         -
stats_export.rs                   111                24    78.38%          31                 4    87.10%         164                25    84.76%           0                 0         -
-----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------
TOTAL                            1118               442    60.47%         207                55    73.43%        1868               526    71.84%           0                 0         -
```

### D-6 scoped hot-path module table (line% is the gate metric; ≥80% line)

| Module | Crate | Line% | Region%(branch proxy) | ≥80%? | Waived in docs/conformance/coverage.md? |
|--------|-------|------:|----------------------:|:-----:|------------------------------------------|
| h1_proxy | lb-l7 | 59.16% | 51.04% | NO | NO — not listed (waiver only covers bridge files) |
| h2_proxy | lb-l7 | 44.14% | 30.27% | NO | NO — not listed |
| bridges::h1_to_h1 | lb-l7 | 92.16% | 82.00% | YES | n/a |
| bridges::h1_to_h2 | lb-l7 | 92.92% | 80.95% | YES | n/a |
| bridges::h1_to_h3 | lb-l7 | 92.92% | 80.95% | YES | n/a |
| bridges::h2_to_h1 | lb-l7 | 91.30% | 80.43% | YES | n/a |
| bridges::h2_to_h2 | lb-l7 | 100.00% | 88.24% | YES | n/a |
| bridges::h2_to_h3 | lb-l7 | 100.00% | 88.24% | YES | n/a |
| bridges::h3_to_h1 | lb-l7 | 95.71% | 84.09% | YES | n/a |
| bridges::h3_to_h2 | lb-l7 | 100.00% | 88.24% | YES | n/a |
| bridges::h3_to_h3 | lb-l7 | 100.00% | 88.24% | YES | n/a |
| loader | lb-l4-xdp | 51.62% | 27.75% | NO | YES — coverage.md row (CAP_BPF/real BPF ELF; Pillar-4a deferral) |
| stats_export | lb-l4-xdp | 84.76% | 78.38% | YES | n/a |
| balancer::* (12 modules) | lb-balancer | 82.76–100% (min p2c 83.02) | min 72.41% | YES (all) | n/a |
| hooks | lb-security | 97.14% | 90.70% | YES | n/a |
| conn_gate | lb-security | 91.46% | 87.18% | YES | n/a |
| watchdog | lb-security | 95.71% | 90.91% | YES | n/a |
| ticket | lb-security | 82.62% | 75.62% | YES | n/a (was 76% waived on older HEAD; now above gate) |
| smuggle | lb-security | 99.06% | 93.26% | YES | n/a |
| validate (in lb-config/src/lib.rs) | lb-config | 89.24% (crate lib.rs) | 85.24% | YES | n/a |
| conn_actor | lb-quic | 75.70% | 63.35% | NO | PARTIAL — coverage.md waives `lb-quic/src/lib.rs`(75.40%) NOT conn_actor.rs by name |
| listener | lb-quic | 0.00% | 0.00% | NO | NO — listener.rs not named; closed only by tests/quic_listener_e2e (integration) |
| admin_http | lb-observability | 80.23% | 75.19% | YES (marginal) | n/a |
| metrics (no metrics.rs; registry in lib.rs/prometheus_exposition/label_budget/xdp_metrics) | lb-observability | lib.rs 62.05% / promexp 87.69% / label_budget 85.71% / xdp_metrics 98.85% | — | MIXED | coverage.md claims lib.rs 100% (STALE — now 62.05% after iteration-2 metrics changes) |

### D-6 VERDICT: FAIL

Scoped hot-path modules below the ≥80% line gate and NOT documented-waived:
1. lb-l7 `h1_proxy` — 59.16% (NOT waived)
2. lb-l7 `h2_proxy` — 44.14% (NOT waived)
3. lb-quic `listener` — 0.00% (NOT waived; listener.rs not named in coverage.md)
4. lb-quic `conn_actor` — 75.70% (NOT explicitly waived; coverage.md waives lib.rs only)
5. lb-observability `metrics` (lib.rs path) — 62.05% (coverage.md asserts 100%, now STALE/wrong)

Documented-waived & still below 80% (acceptable per waiver): lb-l4-xdp `loader` 51.62%.

METHODOLOGY CAVEAT (non-fatal, recorded for honesty): per-crate `cargo llvm-cov -p`
excludes the workspace tests/ integration suite. h1_proxy/h2_proxy are exercised by
tests/h1_proxy_e2e.rs / h2_proxy_e2e.rs; quic listener/conn_actor by
tests/quic_listener_e2e.rs. The authoritative ≥80% determination per coverage-scope.md
("How to run": `cargo llvm-cov --workspace`) requires the workspace run, which is the
known D-6 disk blocker (>28GB). The per-crate scoped numbers above are a TRUE LOWER
BOUND; the gate as scoped cannot be definitively PASSED by the per-crate method because
integration-closed modules cannot be measured per-crate. Verdict therefore: D-6 FAIL on
per-crate evidence; the workspace-level confirmation remains BLOCKED on disk (28GB <
llvm-cov --workspace instrumented footprint).
