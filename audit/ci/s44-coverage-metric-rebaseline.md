# S44 — D-6 coverage metric correction + 31-module re-baseline

**Date:** 2026-08-02 · **Status:** implemented, owner-approved option (a)
**Gate:** `scripts/ci/coverage-check.sh` (D-6, per-module hot-path ≥ 80% lines)
**Predecessor:** `audit/ci/s43-h2proxy-coverage-dual-instantiation.md` (diagnosis)

Owner ruling: **(a) fix the metric, with an explicit re-baseline of all 31 modules so the
change is provably a correction and not a relaxation.** This is that re-baseline.

---

## 1. The problem, re-measured

S43 diagnosed the cause correctly but had only four samples. Two more exist now:

| run | h2_proxy.rs (old metric) | gate |
|---|---:|---|
| 28334977156 (artifact 7938582116) | 79.60% | RED |
| 28334977156 (artifact 7938395373) | 79.70% | RED |
| 28336426614 | 80.18% | green |
| 30745595161 | 79.65% | RED |
| 30749813681 | 80.23% | green |
| 30751410142 | 80.13% | green |

**3 RED / 3 green on effectively identical production source** — a coin flip. This corrects
two earlier records: the handoff called it "~1 run in 4" red; S43 called it "three of four
samples fail". With six samples it is ~50/50. `main` was green at handoff by luck.

## 2. Root cause — confirmed independently, not inherited

Re-verified from the RED artifact (`30745595161`), not from the S43 write-up:

```
h2_proxy.rs — SF: records for this file : 1
  declared LF/LH            : 1887/1503  -> 79.65%   <- what the gate read
  distinct DA: line numbers : 1780, hit 1441 -> 80.96%
  FN: entries               : 596
```

107 phantom lines. The 596 `FN:` entries split across two crate disambiguators:

| instantiation | FN: entries |
|---|---:|
| `CsdExPruU9iqX_5lb_l7` | 421 |
| `CsbnMibX7jh97_5lb_l7` | 175 |

and `handle_inner` shows `FNDA:597` under `CsdExPruU9iqX` — the real-wire build — while the
`CsbnMibX7jh97` (lib-unit-test) copy cannot reach the request hot path at all, because
`hyper::body::Incoming` has no public constructor. llvm-cov merges both instantiations into
one `SF:` record; the `LF:`/`LH:` **summary** double-counts shared lines, the per-line `DA:`
records do not. The unreachable copy's unhit lines were scored as genuine misses.

**This is systemic, not an h2_proxy quirk: 29 of the hot-path modules are dual-instantiated.**
h2_proxy was simply the only one sitting on the boundary, so it was the only one that flipped
the gate.

## 3. The fix

Score from **merged `DA:` records**: each source line counted once, hit if **any**
instantiation executed it. Threshold stays 80.0%. Patterns unchanged. The single named
carve-out (`lb-l4-xdp/src/loader.rs`) is unchanged. Nothing is exempted or waived.

Two latent defects were fixed in passing:

- the old parser did `files[cur] = …` per record, so **multiple `SF:` records for one file
  were last-record-wins**; the new one merges them;
- the obvious way to write the merge — `if cnt > d.get(ln, 0): d[ln] = cnt` — is a trap:
  `0 > 0` is False, so a line hit by **no** instantiation is never inserted and silently
  leaves the *denominator*, scoring **every file at 100%**. Caught during verification
  because the expected value (80.96%) was known in advance; a pass/fail-only check would
  have shipped it green. The code uses `d[ln] = max(d.get(ln, 0), cnt)` and says why.

## 4. Re-baseline — all 31 hot-path modules (sample: RED run 30745595161)

| module | old (LF/LH) | new (DA) | delta | dual-inst |
|---|---:|---:|---:|---|
| `lb-balancer/src/ewma.rs` | 95.38% | 95.38% | +0.00 | — |
| `lb-balancer/src/least_connections.rs` | 100.00% | 100.00% | +0.00 | — |
| `lb-balancer/src/least_request.rs` | 100.00% | 100.00% | +0.00 | — |
| `lb-balancer/src/lib.rs` | 100.00% | 100.00% | +0.00 | — |
| `lb-balancer/src/maglev.rs` | 92.75% | 93.28% | +0.53 | +4 |
| `lb-balancer/src/random.rs` | 86.21% | 85.71% | **-0.49** | +1 |
| `lb-balancer/src/ring_hash.rs` | 92.00% | 92.62% | +0.62 | +3 |
| `lb-balancer/src/round_robin.rs` | 100.00% | 100.00% | +0.00 | +1 |
| `lb-balancer/src/session_affinity.rs` | 97.78% | 97.67% | **-0.10** | +2 |
| `lb-balancer/src/weighted_random.rs` | 87.80% | 87.50% | **-0.30** | +1 |
| `lb-balancer/src/weighted_round_robin.rs` | 92.55% | 92.31% | **-0.25** | +3 |
| `lb-l4-xdp/src/stats_export.rs` | 83.33% | 84.51% | +1.17 | +8 |
| `lb-l7/src/h1_proxy.rs` | 85.23% | 86.64% | +1.41 | +93 |
| `lb-l7/src/h1_to_h1.rs` | 91.95% | 91.36% | **-0.60** | +6 |
| `lb-l7/src/h1_to_h2.rs` | 92.93% | 92.31% | **-0.62** | +8 |
| `lb-l7/src/h1_to_h3.rs` | 96.97% | 96.70% | **-0.27** | +8 |
| `lb-l7/src/h2_proxy.rs` | 79.65% | 80.96% | +1.30 | +107 |
| `lb-l7/src/h2_to_h1.rs` | 90.97% | 91.72% | +0.76 | +10 |
| `lb-l7/src/h2_to_h2.rs` | 100.00% | 100.00% | +0.00 | +2 |
| `lb-l7/src/h2_to_h3.rs` | 100.00% | 100.00% | +0.00 | +2 |
| `lb-l7/src/h3_to_h1.rs` | 95.45% | 95.16% | **-0.29** | +4 |
| `lb-l7/src/h3_to_h2.rs` | 100.00% | 100.00% | +0.00 | +2 |
| `lb-l7/src/h3_to_h3.rs` | 100.00% | 100.00% | +0.00 | +2 |
| `lb-observability/src/admin_http.rs` | 86.87% | 91.80% | +4.93 | +15 |
| `lb-quic/src/conn_actor.rs` | 85.01% | 84.79% | **-0.21** | +20 |
| `lb-quic/src/listener.rs` | 87.06% | 87.13% | +0.07 | +6 |
| `lb-security/src/conn_gate.rs` | 91.14% | 90.91% | **-0.23** | +2 |
| `lb-security/src/hooks.rs` | 96.84% | 96.84% | +0.00 | — |
| `lb-security/src/smuggle.rs` | 98.97% | 98.94% | **-0.03** | +3 |
| `lb-security/src/ticket.rs` | 84.41% | 85.98% | +1.57 | +14 |
| `lb-security/src/watchdog.rs` | 95.48% | 95.45% | **-0.03** | +1 |

**31 modules — 9 up, 12 DOWN, 10 unchanged. 0 below 80% under the corrected metric.**

### Why this proves it is a correction, not a relaxation

**More modules got stricter than looser.** A relaxation is a one-way ratchet — it can only
move numbers up. This moves 12 modules *down*, because double-counted **hit** lines are
removed from the numerator just as double-counted misses are removed from the denominator.
The gate got *harder* on `random.rs`, `conn_gate.rs`, `conn_actor.rs`, and nine others, and
they still pass on their own merits. The threshold never moved.

## 5. Verification

Gate re-run against all three real artifacts — every value matches an independently written
scorer (`rebaseline.py`), not the gate's own output:

| sample | h2_proxy old → new | gate |
|---|---|---|
| red 30745595161 | 79.65% → **80.96%** | exit 0 |
| green 30749813681 | 80.23% → **81.57%** | exit 0 |
| green 30751410142 | 80.13% → **81.52%** | exit 0 |

**Load-bearing negative controls** — the gate must still fail when coverage genuinely drops,
or the "fix" is just a green light:

| control | expectation | result |
|---|---|---|
| NC1 — zero 576 of h2_proxy's 1441 hit lines | RED | 48.54%, **exit 1** ✅ |
| NC2a — 1424/1780 hit (exactly 80.00%) | pass | 80.00%, exit 0 ✅ |
| NC2b — 1423/1780 hit (79.94%) | RED | 79.94%, **exit 1** ✅ |
| NC3a — all paths renamed, no pattern matches | fail closed | **exit 1** ✅ |
| NC3b — empty LCOV | fail closed | **exit 1** ✅ |
| NC4 — carve-outs | exactly 1, `loader.rs` | ✅ |

NC2 shows **single-line discrimination at the floor**: one line is the difference between
pass and fail. The gate is non-vacuous.

## 6. What this does NOT fix — stated plainly

The correction raises the *level*, not the *variance*. Run-to-run jitter of ~0.6 pp is still
present (h2_proxy reads 80.96 / 81.57 / 81.52 across the three samples), and its source is
**still unexplained**.

One obvious explanation was tested and **REFUTED**. The coverage job runs
`cargo llvm-cov nextest --ignore-run-fail`, so a flaking test still counts as executed-but-
failed and its lines could plausibly drop out of the profile — and there is a known flake
named for this exact job and module (CF-S37-D6-H2PROXY-FLAKY). The data says otherwise:

| run | tests failed inside the coverage run | h2_proxy (old metric) |
|---|---:|---:|
| 30745595161 (RED) | **0** — 1565/1565 passed | **79.65%** (lowest) |
| 30749813681 (green) | **2** failed | 80.23% |
| 30751410142 (green) | 0 — 1565/1565 passed | 80.13% |

The run with *zero* failures produced the *lowest* coverage, and the run with two failures
produced the highest. The correlation runs opposite to the hypothesis, so flaky test
failures are not the mechanism. Do not re-chase this one.

Remaining candidates, untested: nondeterministic scheduling across the two instantiations,
llvm-cov profile-merge races under `--ignore-run-fail`, or genuinely timing-dependent
branches. Any future attempt should start by diffing the per-line `DA:` sets of two runs to
see *which* lines move, rather than reasoning from totals.

So `h2_proxy.rs` remains the tightest hot-path module. It now sits ~1.0–1.6 pp above the
floor instead of straddling it, which is a real margin rather than a coin flip, but it is
**not** a comfortable one. If it drifts down again the honest fix is real test coverage
(S43 option (c)) — the SNI-mismatch, watchdog, and underscore-policy arms — not another
metric change.

Two of those arms are worth noting as *product* gaps rather than test gaps:
`HeaderUnderscorePolicy::Drop` (lines 1120-1135) is uncovered because the binary never calls
`with_header_underscore_policy` — assessment gap **G7**, an inert knob. Covering it means
wiring it, not writing a test for dead code.

## 7. Effect on branch protection

With this landed, `Coverage (per-module hot-path >= 80%)` is safe to add to the required
checks in `audit/release/owner-actions.md` §1. Before it, requiring that check would have
blocked roughly half of all PRs on a measurement artifact.

## 8. Reproduce

```sh
gh api repos/:owner/:repo/actions/runs/30745595161/artifacts -q '.artifacts[].id'
gh api repos/:owner/:repo/actions/artifacts/8832879049/zip > a.zip && unzip -q a.zip -d red
bash scripts/ci/coverage-check.sh red/coverage.lcov     # exit 0, h2_proxy 80.96%
git stash list  # (none — no stash used; R9)
git show HEAD~1:scripts/ci/coverage-check.sh > /tmp/old.sh
bash /tmp/old.sh red/coverage.lcov                      # exit 1, h2_proxy 79.65%
```
