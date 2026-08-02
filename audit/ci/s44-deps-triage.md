# S44 — Dependabot triage (5 open PRs)

**Date:** 2026-08-02 · **Against:** `main` @ `eb00081d` · versions checked against crates.io, not docs.

**Headline: PR #253 must not be merged as-is — it silently breaks the tokio hold, and CI
cannot catch that.** Everything else is mostly stale-red and should clear on a rebase.

---

## 1. Current pins vs upstream latest

| crate | pinned | latest | note |
|---|---|---|---|
| `tokio` | 1.51.1 | 1.53.1 | **HELD < 1.52** — measured ~10× H2→H3 relay collapse (CF-S37-D-TOKIO-1.52-RELAY) |
| `hyper` | 1.10.1 | 1.11.0 | 1.11.0 ships #4102 → unblocks CF-S27-2 WS-H2 (own session; needs the S30 repro) |
| `quiche` | 0.29.1 | **0.29.3** | both open PRs target the stale 0.29.2 |
| `tokio-quiche` | 0.19.0 | 0.19.1 | |
| `rustls` | 0.23.40 | 0.23.43 | |
| `h2` | 0.4.14 | 0.4.15 | |
| `serde_with` | 3.20.0 | 3.21.0 | PR #246 |
| `aya` | (pinned) | 0.14.0 | **breaking** — the cause of #253's compile failure |
| `anyhow` | 1.0.104 | — | S43 bump, advisory cleared |
| `quinn-proto` | 0.11.16 | — | S43 bump, advisory cleared |
| `foundations` | 4.5.0 | — | held-by-upstream (quiche → qlog → foundations ^4) |

## 2. Most of the red is STALE — measured, not assumed

Four of the five PRs are based on commits that **predate S43's three fixes**, so they still
carry reds that no longer exist on `main`:

| PR | base | commits behind main | S43 fixes in base? | verdict on its reds |
|---|---|---:|---|---|
| #253 | `e9df7cca` | 1 | **yes** | **REAL** |
| #251 | `1a4e4fae` | 5 | no | stale |
| #246 | `1a4e4fae` | 5 | no | stale |
| #238 | `ffac8705` | 9 | no | stale |
| #237 | `ffac8705` | 9 | no | stale |

The stale reds are exactly the three S43 closed — `Security Audit` and
`cargo-deny` (RUSTSEC-2026-0185 / -0190, cleared by bumping in `31d6cc92`) and
`Fuzz Smoke Test` (nightly `fetch_update` deprecation, fixed in `340d81fd`) — plus
`Coverage`, which is the coin-flip artifact PR #254 corrects.

**So: rebase before reading any of these verdicts.** Their current check results carry no
information about the bumps themselves.

## 3. PR #253 — reject as-is. Two independent reasons.

### 3.1 It breaks the tokio hold, and no gate would catch it

```diff
-tokio = { version = ">=1.51, <1.52", features = ["full"] }
+tokio = { version = ">=1.51, <1.54", features = ["full"] }
```
```diff
 name = "tokio"
-version = "1.51.1"
+version = "1.53.1"
```

The `<1.52` bound is not cosmetic — it is the guard rail for a **measured ~10× H2→H3 relay
throughput collapse** on 1.52.x. Dependabot widened the bound and crossed it.

**This is the dangerous part: CI would go green.** The regression is a *throughput* collapse
and there is no perf gate in the 16-job CI. If the aya breakage below were fixed, #253 would
merge clean and ship a 10× relay regression silently. The canary
(`h2h3_fcap1` ~30 MiB stall) only shows up in the perf/soak lane, which does not run per-PR.

The hold must be re-validated by measurement on a `c6a.2xlarge`, not by a green checkmark.
A hyper bump does **not** license un-holding tokio.

### 3.2 It does not compile — genuine `aya` breaking change

```
error[E0432]: unresolved import `aya::programs::XdpFlags`
error[E0061]: this method takes 2 arguments but 1 argument was supplied
```

`aya` 0.14.0 moved/renamed `XdpFlags` and changed an attach signature. This needs real code
changes in the XDP loader — not a version bump. Note the irony: the loader this breaks is
currently a **no-op attach** (assessment gap G1), so the fix is cheap to verify but also
low-value until the XDP wiring decision is made.

**Recommended action:** close #253 and let Dependabot re-open it **split** — or split it
manually. The 19 harmless crates should not be held hostage by tokio + aya, and tokio must
be excluded from the group until the hold is lifted on evidence.

Worth doing regardless: add a Dependabot `ignore` for `tokio` so the guard rail cannot be
widened by automation again. The hold has survived since S37 on discipline alone.

## 4. The other four

| PR | what | recommendation |
|---|---|---|
| **#238** `quiche` 0.29.1→0.29.2 | stale target | **close** — take **0.29.3** directly |
| **#237** `quiche` 0.28.0→0.29.2 in `/fuzz` | stale target | **close** — take **0.29.3**; also worth aligning `/fuzz` with the workspace pin so they stop drifting |
| **#246** `serde_with` 3.20→3.21 | low risk, all reds stale | rebase; merge if green |
| **#251** actions group (3 updates) | CI-only, no source impact | rebase; merge if green |

`quiche` is the one to be careful with: it carries the H3 front end, and S31 showed a quiche
bump can move h3spec results. Any quiche bump needs h3spec re-run (12 named waivers must stay
12) — not just a green Test job.

## 5. Recommended sequencing

1. **Land #254** (coverage metric correction) — removes the `Coverage` coin flip that is
   currently adding noise to every one of these PRs.
2. **Rebase #251 and #246** onto `main`; expect most reds to vanish. Merge if green.
3. **Close #238 and #237**; open a single `quiche 0.29.1 → 0.29.3` bump covering both the
   workspace and `/fuzz`, gated on h3spec staying at 12 named waivers.
4. **Close #253**; re-open split, with `tokio` excluded and an `ignore` rule added. Handle the
   `aya` 0.14 breaking change as its own small piece of work.
5. **hyper 1.11.0** is *not* a routine deps bump — it is the CF-S27-2 WS-H2 un-gate and needs
   its own session with the S30 repro, R8/R13 evidence, and h2spec 146/1/0 as a hard blocker.

## 6. Reproduce

```sh
gh pr diff 253 | grep -E '^[+-].*tokio *='          # the widened bound
gh pr view 253 --json baseRefOid -q .baseRefOid      # base commit
git rev-list --count <base>..origin/main             # how stale
curl -s https://crates.io/api/v1/crates/quiche | python3 -c \
  "import sys,json;print(json.load(sys.stdin)['crate']['max_stable_version'])"
```
