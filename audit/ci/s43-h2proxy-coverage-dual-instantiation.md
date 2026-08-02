# S43 — the h2_proxy coverage red is a dual-instantiation measurement artifact

**Date:** 2026-08-02 · **Status:** finding, needs an owner ruling (no gate was changed)
**Gate:** `scripts/ci/coverage-check.sh` (D-6, per-module hot-path >= 80% lines)
**Module:** `crates/lb-l7/src/h2_proxy.rs`

---

## 1. What was believed

The state assessment and memory record this as a **boundary flicker**: "h2_proxy coverage
oscillates around the 80% floor (79.60% <-> >=80%)", classified as a known non-regression
and lumped in with the fcap1/saturation test flake as a "rotating second red".

Implied fix: "add a real h2_proxy test to lift coverage off the 80% boundary."

## 2. What is actually measured

Four samples, all on **byte-identical production source** (`1a4e4fae` differs from
`677dae0e` only in `audit/release/s42-report.md`; the S43 branch adds only `Cargo.lock` +
a `#[allow(deprecated)]` in `lb-quic`):

| CI run | artifact | h2_proxy.rs | gate |
|---|---|---|---|
| 28334977156 | 7938582116 | 79.60% | RED |
| 28334977156 | 7938395373 | 79.70% | RED |
| 28336426614 | (single)   | 80.18% | green |
| 30745595161 | (single)   | 79.65% | RED |

Mean ~79.78%, spread ~0.6pp, floor exactly 80.00%. **Three of four samples fail.** So it is
not "flickering around" the floor — it sits marginally **under** it, with effectively zero
margin.

A hypothesis that the flaky Test job was costing coverage is **REFUTED**: Test *passed* on
the run where Coverage failed (28334977156) and *failed* on the run where Coverage passed
(28336426614). The two jobs run independently on separate runners; neither causes the other.

## 3. Root cause — the same file is measured twice

`h2_proxy.rs` appears in exactly **one** `SF:` record, but that record declares
`LF:1887 / LH:1502` while containing only **1780 `DA:` lines, 1441 hit**:

```
declared (what the gate reads):  LF:1887  LH:1502  ->  79.60%
emitted per-line DA records:     1780 lines, 1441 hit ->  80.96%
```

The record carries **596 `FN:` entries** for a file with far fewer functions, split across
**two crate disambiguators** — `lb-l7` is compiled twice and llvm-cov merges both
instantiations into one file record:

| instantiation | functions | executed | share |
|---|---|---|---|
| `CsdExPruU9iqX_5lb_l7` | 421 | 271 | 64.4% |
| `CsbnMibX7jh97_5lb_l7` | 175 |  38 | 21.7% |

Decisive single datapoint — `H2Proxy::handle_inner`:

```
hash=dExPruU9iqX   exec_count=661
hash=bnMibX7jh97   exec_count=0
```

The low-coverage instantiation is the **lib unit-test build**. It *cannot* reach
`handle_inner` and the request hot path at all: those take `Request<IncomingBody>` where
`IncomingBody = hyper::body::Incoming` (`h1_proxy.rs:36`), which has **no public
constructor** — it is only obtainable from a real hyper connection. Every real-wire test
lives in the *other* instantiation.

So the gate's denominator includes a whole second copy of the file whose hot path is
**structurally unexecutable**, and its unhit lines are counted as genuine misses.

## 4. Why "just add a test" does not fix it

Two independent blockers:

1. **`Incoming` is not constructible**, so the uncovered hot-path blocks cannot be reached
   by any in-file unit test — which is precisely why they are uncovered in that
   instantiation in the first place.
2. The largest uncovered blocks are **structurally unreachable from the existing
   real-wire tests too**:
   - `1216-1244` SNI/`:authority` mismatch sits behind `if !peer.ip().is_loopback()`, and
     every integration test connects over loopback;
   - `1120-1135` is the `HeaderUnderscorePolicy::Drop` arm — dead because the binary never
     calls `with_header_underscore_policy` (assessment gap **G7**);
   - `1248-1266` is watchdog registration, and tests construct no watchdog.

Adding tests to the *unit-test* instantiation cannot move its hot-path lines at all. Real
coverage would have to come from new real-wire integration tests bound to a non-loopback
address — environment-dependent and CI-fragile.

## 5. Options (owner's call — nothing here was changed)

- **(a) Fix the metric.** Score per-file line coverage from merged `DA:` records rather than
  the `LF:`/`LH:` summary, so a dual-instantiated crate is counted once per source line.
  On today's data h2_proxy reads **80.96%** and passes honestly. This is arguably the
  charter's real metric ("per-module line coverage"), and it is a *correction*, not a
  loosening — every other module keeps its threshold. Risk: it moves numbers for all 31
  modules and must be re-baselined so nothing silently drops below 80%.
- **(b) De-duplicate the instantiations** so llvm-cov measures one build of `lb-l7`
  (e.g. exclude the lib-unit-test instantiation from the report). Cleanest conceptually,
  fiddly in practice.
- **(c) Add real-wire coverage** for the SNI / watchdog / underscore-policy paths. Genuine
  test value, but it does not address the double-counting and the non-loopback requirement
  makes it CI-fragile.
- **(d) Leave it red.** Honest, and consistent with "an honest red beats a dishonest green"
  — but it keeps `main` permanently red on a number that is not measuring what it claims.

**Recommendation: (a), with an explicit re-baseline of all 31 modules under the corrected
metric, so the change is provably a correction and not a relaxation.** It should be a
deliberate, owner-approved gate change with the before/after table published — not folded
silently into another PR.

## 6. Reproduce

```sh
gh api repos/:owner/:repo/actions/runs/<run>/artifacts          # get artifact id
gh api repos/:owner/:repo/actions/artifacts/<id>/zip > a.zip && unzip a.zip
bash scripts/ci/coverage-check.sh coverage.lcov                 # the gate's own number
awk '/^SF:.*h2_proxy\.rs$/{f=1} f&&/^(LF|LH):/{print} /^end_of_record/{f=0}' coverage.lcov
```
