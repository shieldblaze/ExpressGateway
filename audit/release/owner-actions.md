# Owner actions — copy-paste pack

**Prepared:** 2026-08-15 · **Against:** `feature/ship-blockers-s46` @ `7ebce5be`
(base `main` @ `a63776e9`) · **Verified from source and from the live GitHub API this
session**, not from docs or memory.

Some things in this repo can only be done with **repo-admin credentials**. An agent cannot
do any of them. Everything below is for a human to paste.

> **Revision note.** This file was substantially revised at S46. Three claims in the
> previous revision were wrong or out of date and are corrected here; §5 lists each one
> with what it said, what is true, and the evidence. Corrections are marked rather than
> silently rewritten — the file is part of the repo's evidence trail.

---

## 0. What is done, and what is yours

| # | Action | State | Who |
|---|---|---|---|
| 1 | Branch protection / ruleset on `main` | **NOT DONE** — none exists | **OWNER** (§1) |
| 2 | `SOAK_*` secret + variables | **NOT DONE** — none set | **OWNER** (§2) |
| 3 | Secret scanning, push protection, Dependabot alerts | **NOT DONE** | **OWNER** (§4) |
| 4 | Delete the two dead 2022 secrets | **NOT DONE** | **OWNER** (§4) |
| — | Coverage metric corrected so the gate is no longer a coin flip | **DONE** — `14974c76` | done, no action |
| — | Hang containment: job timeouts + per-test timeout + a hang detector | **DONE** — S46, §3 | verify it yourself (§3.3) |

Verified live this session:

```
$ gh api repos/shieldblaze/ExpressGateway/rulesets                 -> []
$ gh api repos/shieldblaze/ExpressGateway/branches/main/protection -> 404 Branch not protected
$ gh secret   list -R shieldblaze/ExpressGateway
MONGO_CONNECTION_STRING  2022-10-04      # dead, see §4
ZOOKEEPER_ADDRESS        2022-09-30      # dead, see §4
$ gh variable list -R shieldblaze/ExpressGateway
COPILOT_AGENT_FIREWALL_ALLOW_LIST_ADDITIONS   2026-03-07
COPILOT_AGENT_FIREWALL_ENABLED         false  2026-03-07
```

**Anyone with write access can still push directly to `main`, and zero checks are
required.** No `SOAK_*` of either kind is set, so the release soak gate cannot run.

---

## 1. Branch protection — currently NONE

### 1.1 ⚠️ Read this before you require `Coverage`

**The `Coverage` check does not tell you whether the tests pass.**

The Coverage job runs `cargo llvm-cov nextest --workspace --all-features
--ignore-run-fail` (`.github/workflows/ci.yml`). `--ignore-run-fail` is deliberate and
documented in that file — it exists so that a flaky test does not throw away the coverage
measurement, and the pass/fail verdict is the separate `Test` job's responsibility. But
the consequence is blunt, and you should decide with it in front of you:

| run | what nextest reported | what the Coverage check reported |
|---|---|---|
| `31504772378` | 1565 passed | success |
| `30749813681` | 1563 passed, **2 failed** | **success** |
| `31495640064` | 1564 passed, **1 failed** | **success** |

So: **requiring `Coverage` buys you coverage-threshold signal only — not pass/fail
signal.** A PR whose tests fail inside that job will still show a green `Coverage` check.
That is not a bug to fix here; `Test` is the gate that fails on a failing test, and it is
also required. Just do not read a green `Coverage` as "the tests passed".

*(The failures above are the known fcap1/saturation flakes, which is exactly the case the
flag was added for. `Test` runs that test isolated with 3 attempts, so its pass/fail
signal is not lost.)*

**What S46 did add:** until now a *hung* test made this job neither fail nor finish — it
burned toward GitHub's 6-hour ceiling and reported nothing. It now fails, loudly, with the
test named. See §3. **Requiring `Coverage` is safe as of S46**; it was not before.

### 1.2 The 16 required checks

Read from `.github/workflows/ci.yml` **as it stands on this branch**, and cross-checked
against the job names GitHub actually reported in green run `31504772378`. They must match
character-for-character — **a required check whose name does not exist blocks every PR
forever.** S46 changed no job's `name:`, so these are unchanged from the previous revision
except that `Coverage` is now included.

```
Format
Check
Clippy
Panic Freedom Audit
Doc Lint
Test
MSRV (1.88)
Fuzz Smoke Test
Release Build
Security Audit
cargo-deny (licenses/advisories/bans/sources)
Coverage (per-module hot-path >= 80%)
Conformance (h2spec --strict + h3spec)
Chaos Attack Suite
Container Image (build + serve smoke + trivy)
XDP Verifier Smoke (runner kernel)
```

Regenerate this list at any time rather than trusting the copy above:

```sh
python3 -c "import yaml;print('\n'.join(b['name'] for b in yaml.safe_load(open('.github/workflows/ci.yml'))['jobs'].values()))"
```

### 1.3 Apply it — ruleset API

```sh
cat > /tmp/eg-main-ruleset.json <<'JSON'
{
  "name": "main — 16-gate protection",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "bypass_actors": [],
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    {
      "type": "pull_request",
      "parameters": {
        "required_approving_review_count": 0,
        "dismiss_stale_reviews_on_push": true,
        "require_code_owner_review": false,
        "require_last_push_approval": false,
        "required_review_thread_resolution": true,
        "allowed_merge_methods": ["merge", "squash"]
      }
    },
    {
      "type": "required_status_checks",
      "parameters": {
        "strict_required_status_checks_policy": true,
        "do_not_enforce_on_create": false,
        "required_status_checks": [
          { "context": "Format" },
          { "context": "Check" },
          { "context": "Clippy" },
          { "context": "Panic Freedom Audit" },
          { "context": "Doc Lint" },
          { "context": "Test" },
          { "context": "MSRV (1.88)" },
          { "context": "Fuzz Smoke Test" },
          { "context": "Release Build" },
          { "context": "Security Audit" },
          { "context": "cargo-deny (licenses/advisories/bans/sources)" },
          { "context": "Coverage (per-module hot-path >= 80%)" },
          { "context": "Conformance (h2spec --strict + h3spec)" },
          { "context": "Chaos Attack Suite" },
          { "context": "Container Image (build + serve smoke + trivy)" },
          { "context": "XDP Verifier Smoke (runner kernel)" }
        ]
      }
    }
  ]
}
JSON

gh api -X POST repos/shieldblaze/ExpressGateway/rulesets \
  -H "Accept: application/vnd.github+json" \
  --input /tmp/eg-main-ruleset.json
```

**Verify it took** — do not skip this; a typo'd context name is silent until it wedges a PR:

```sh
gh api repos/shieldblaze/ExpressGateway/rulesets -q '.[] | [.id,.name,.enforcement] | @tsv'

RS=<id from above>
gh api repos/shieldblaze/ExpressGateway/rulesets/$RS \
  -q '.rules[] | select(.type=="required_status_checks")
      | .parameters.required_status_checks[].context'    # expect exactly the 16 above
```

To change it later use **PUT**, not POST — a second POST creates a *second* ruleset and
both then apply:

```sh
gh api -X PUT repos/shieldblaze/ExpressGateway/rulesets/$RS \
  -H "Accept: application/vnd.github+json" --input /tmp/eg-main-ruleset.json
```

### 1.4 Five choices baked into that JSON — change any you disagree with

1. **`required_linear_history` is NOT set.** *(Corrected at S46 — the previous revision set
   it to `true`.)* `main` has **51 merge commits in 853**, and the last six promotions are
   all merges (`5c9f0752`, `ff39fa08`, `14974c76`, `e9df7cca`, `677dae0e`, `ffac8705`).
   Requiring linear history would reject the repo's actual R11 promote workflow.
   `allowed_merge_methods` keeps `merge` available for exactly that reason.
2. **`required_approving_review_count: 0`, not 1.** *(Corrected at S46 — previously 1.)*
   GitHub forbids approving your own PR. On a single-maintainer repo where you author the
   promote PR, `1` locks you out of your own repository with no second reviewer. Raise it
   to `1` the day a second human reviewer exists. `require_last_push_approval` is `false`
   for the same reason — it is meaningless at count 0.
3. **`bypass_actors: []`** — nobody bypasses, admins included. This carries forward the
   previous revision's own argument for `enforce_admins: true`: without it, admins bypass
   everything and the gate is decorative. If you want an escape hatch, add it through
   Settings → Rules → Rulesets in the UI; no numeric role id is hard-coded here because a
   wrong one silently grants the wrong bypass.
4. **`strict_required_status_checks_policy: true`** — branches must be up to date before
   merging. Catches semantic conflicts a clean textual merge hides. **Real cost:** a
   16-job, ~21-minute gate re-runs on every rebase, and Dependabot opens PRs regularly.
   Flip to `false` if that becomes painful; it is the one setting here with an ongoing
   time cost.
5. **The `pull_request` rule blocks direct pushes to `main`.** The current promote flow
   (direct `--no-ff` merge pushed straight to `main`) **will stop working.** New flow:
   push the branch → open a PR → 16 checks → merge. That is the point of arming the gate,
   but it is a workflow change, so make it deliberately.

### 1.5 ⚠️ Do not add `paths-ignore:` to `ci.yml` after arming this

Tempting, because a docs-only push runs the whole 16-job gate today — commit `a63776e9`
changed **one markdown file** and ran all 16 jobs for 21 minutes (run `31504772378`).

But **workflow-level `paths-ignore:` is incompatible with required status checks.** If the
workflow does not run, the check is never *reported*, and GitHub blocks the PR forever on
`Expected — Waiting for status to be reported`. What *is* compatible is a **job-level
`if:`** — a job skipped by `if:` reports `skipped`, and branch protection accepts that as
satisfied. If you want the docs-only saving, that is the shape to use, and `Doc Lint` must
stay always-on since it is precisely the gate that should run on a docs change.

Order: **arm the ruleset, confirm one real PR goes green through all 16 checks, then**
consider path filtering. Reversing that order risks a wedged repo.

### 1.6 Alternative: the legacy branch-protection API

If you prefer the older endpoint, it is the same 16 contexts under
`PUT /repos/shieldblaze/ExpressGateway/branches/main/protection` with
`"required_linear_history": false` and `"enforce_admins": true`. **Do not apply both** the
ruleset and legacy protection — they stack, and debugging a merge blocked by two
overlapping policies is unpleasant.

---

## 2. Release soak gate — un-armed

`.github/workflows/release.yml` provisions an EC2 box, runs the 12-scenario soak, reads a
verdict, and tears the box down. **It cannot run: none of its inputs are set.**

> Line numbers below are **post-S46** (this branch). S46 added `timeout-minutes:` and a
> duration-ceiling comment to `release.yml`, which shifted every line in this section by
> +11. The previous revision's citations had drifted for the same reason; rather than
> re-cite and drift again, verify with the command — it is always correct:
> ```sh
> grep -n "SOAK_" .github/workflows/release.yml
> ```

**Secret — 1**

| name | line | notes |
|---|---|---|
| `SOAK_AWS_ROLE_ARN` | `release.yml:55` | OIDC role to assume; needs a trust policy scoped to this repo (the job requests `id-token: write`) |

**Variables — 7 (6 required, 1 optional)**

| name | line | notes |
|---|---|---|
| `SOAK_REGION` | `:56`, `:59` | used both for the AWS credential step and by the script |
| `SOAK_AMI` | `:60` | Ubuntu 24.04 x86_64 AMI **in that region** |
| `SOAK_SUBNET_ID` | `:61` | must have egress to GitHub + S3 |
| `SOAK_SECURITY_GROUP_ID` | `:62` | egress-only is enough |
| `SOAK_IAM_INSTANCE_PROFILE` | `:63` | needs `s3:PutObject` to the bucket below + permission to terminate itself |
| `SOAK_S3_BUCKET` | `:64` | soak artifacts + verdict land here |
| `SOAK_INSTANCE_TYPE` | `:65` | **OPTIONAL** — defaults to `c6a.2xlarge` (`scripts/release-soak.sh:41`) |

`SOAK_GIT_REF` (`:66`) and `SOAK_DURATION_SECS` (`:67`) are **neither** — they come from
the workflow context and the dispatch input. The script's other `SOAK_*` variables
(`SOAK_S3_PREFIX`, `SOAK_GIT_REPO`, `SOAK_SAMPLE_SECS`, `SOAK_MAX_WAIT_SECS`,
`SOAK_KEY_NAME`) all have defaults and are not repo configuration.

`scripts/release-soak.sh:60,63` hard-require exactly `SOAK_REGION` plus the five of
`SOAK_AMI SOAK_SUBNET_ID SOAK_SECURITY_GROUP_ID SOAK_IAM_INSTANCE_PROFILE SOAK_S3_BUCKET`
— which matches the table above.

```sh
R=shieldblaze/ExpressGateway

# 1 SECRET (prompts; not echoed)
gh secret set   SOAK_AWS_ROLE_ARN         -R $R

# 6 REQUIRED VARIABLES
gh variable set SOAK_REGION               -R $R --body "us-east-1"
gh variable set SOAK_AMI                  -R $R --body "ami-…"
gh variable set SOAK_SUBNET_ID            -R $R --body "subnet-…"
gh variable set SOAK_SECURITY_GROUP_ID    -R $R --body "sg-…"
gh variable set SOAK_IAM_INSTANCE_PROFILE -R $R --body "…"
gh variable set SOAK_S3_BUCKET            -R $R --body "…"

# 1 OPTIONAL — omit to accept the c6a.2xlarge default. Note: the S39 perf baseline was
# taken on c6a.2xlarge, so changing this makes soak numbers NOT-COMPARABLE to it.
# gh variable set SOAK_INSTANCE_TYPE      -R $R --body "c6a.2xlarge"

gh secret   list -R $R      # expect SOAK_AWS_ROLE_ARN
gh variable list -R $R      # expect the 6 above
```

### 2.1 ⚠️ The soak cannot exceed a 4-hour duration

`scripts/release-soak.sh:44` polls for `SOAK_MAX_WAIT_SECS = SOAK_DURATION_SECS + 3600`.
At the default `14400` (4 h) that is **18,000 s = 300 min of polling** before provisioning
and teardown. **GitHub's hard job ceiling is 360 min.** S46 set `timeout-minutes: 350`,
the largest safe value.

So: **dispatching with `soak_duration_secs` greater than `14400` cannot complete.** The
runner is killed mid-soak, and because teardown is a shell trap rather than a separate
job, a killed runner **can leave the EC2 instance running** — a standing bill. The
timeout makes this visible; it does not prevent it. A proper fix (input validation, or an
`if: always()` teardown job) was deliberately left undone and is noted here so it is not
forgotten.

### 2.2 Prove the teardown before trusting it

The whole value of this gate is that it tears the box down. An untested teardown path is a
standing bill.

```sh
./scripts/release-soak.sh --dry-run     # free: prints every AWS call + the rendered user-data
gh workflow run release.yml -R $R       # then the real dispatch
gh run watch -R $R
aws ec2 describe-instances --region "$SOAK_REGION" \
  --filters "Name=instance-state-name,Values=running,stopped" \
  --query 'Reservations[].Instances[].[InstanceId,Tags]'   # expect: nothing from the soak
```

Recommended release flow: dispatch the soak gate → confirm PASS → **then** tag.

---

## 3. Hang-as-failure (S46) — done, but verify it yourself

### 3.1 The problem it addresses

In run `30755744744` job `91517507600`, one test wedged under llvm-cov instrumentation.
nextest re-reported it as `SLOW` every 60 seconds — **299 times** — and never killed it.
1564 of 1565 tests finished; that one never returned. The job ran **5 h 06 m** and had to
be cancelled by hand. It never went red: `--ignore-run-fail` means a *failure* is
tolerated, and a *hang* is not even a failure.

> **The hang itself is NOT fixed.** It remains an open, unexplained bug — intermittent,
> so far only under llvm-cov instrumentation, and now observed across four different
> tests. What follows is **containment**: it converts a silent 6-hour burn into a fast,
> loud, named failure. Do not read a green CI as evidence the hang is gone.
>
> Separately, the **502** seen in the same investigation *is* root-caused and fixed (the
> H3 front dropped a request's terminal `End` on a full channel, producing a
> `RST_STREAM` and a 502 on a request the backend had served correctly; its negative
> control went from ~3.6% to 0.00%). The 502 and the hang are two different bugs — fixing
> one says nothing about the other.

### 3.2 What changed

| change | file | effect |
|---|---|---|
| Per-test timeout | `.config/nextest.toml` (new) | a test running past **20 min** is SIGTERM'd, then SIGKILL'd, and reported as a **failure** |
| Run-level timeout | `.config/nextest.toml`, `[profile.ci]` | the whole nextest run is capped at **40 min** in CI |
| Job timeouts | all 3 workflows | **every one of the 23 jobs** now sets `timeout-minutes:`. Previously **none** did, so each inherited GitHub's 6-hour ceiling |
| Hang detector | `ci.yml`, Coverage job | greps the run log for nextest's `TIMEOUT`/`TMT` status and **fails the job** — because `--ignore-run-fail` would otherwise swallow even a terminated test |

All three layers are needed. The per-test timeout bounds the burn and names the culprit;
the job timeout also covers the *build*, which nextest cannot see; the detector is what
turns either into a red check.

The 20-minute figure is not a guess: the slowest legitimate test in this suite is
`h2h3_fmd4_request_rst_burst_current_thread`, measured under instrumentation at
**244.117 s / 244.970 s / 245.357 s** across three CI runs — a 1.24 s spread, so it is
wall-clock deterministic. 20 min is 4.9× that.

### 3.3 See it fire yourself

A timeout nobody has watched fire is a claim, not a gate. `crates/lb-core/tests/
ci_hang_negative_control.rs` is a fixture that hangs **on demand** — it returns
immediately unless `EG_CI_HANG_PROBE` is set, so it never runs in, or slows, the real
gate.

```sh
# ~40 s. Expect: TIMEOUT [  20.000s] x2, "2 failed", exit code 100.
EG_CI_HANG_PROBE=1 cargo nextest run -P hang-probe \
  -p lb-core --test ci_hang_negative_control
```

That proves the mechanism. To prove the *shipped* 20-minute constant end to end:

```sh
# ~20 min. Expect: TIMEOUT [1200.000s] x2, exit code 100.
EG_CI_HANG_PROBE=1 cargo nextest run -p lb-core --test ci_hang_negative_control
```

Neither of those crosses `--ignore-run-fail`, which is the part that actually matters for
the Coverage job. To prove **the job goes red**, push a throwaway branch that adds one
line to the Coverage step in `ci.yml`:

```yaml
      - name: Coverage (full workspace suite, --all-features)
        env:
          NEXTEST_PROFILE: ci
          EG_CI_HANG_PROBE: "1"      # THROWAWAY BRANCH ONLY — never merge this line
```

Expect the Coverage job to fail at **~11–12 minutes** with
`::error::A test was TERMINATED by the nextest timeout`, instead of running to 6 hours and
reporting green. Then delete the branch.

### 3.4 Does the per-test timeout actually work under `cargo llvm-cov nextest`?

**CONFIRMED — yes.** This matters because CF-S44's hang happened in that exact job; if the
answer were no, the per-test fix would be inert precisely where it is needed.

1. `cargo llvm-cov nextest --help`, quoting the binary: *"This internally calls
   `cargo nextest run`."* llvm-cov is a wrapper — **nextest stays the direct parent of the
   test processes**, so nextest, not llvm-cov, owns the kill.
2. `cargo llvm-cov show-env` lists every variable llvm-cov injects: `LLVM_PROFILE_FILE`,
   `RUSTC_WRAPPER`, `__CARGO_LLVM_COV_RUSTC_WRAPPER*`, `CARGO_LLVM_COV*`. **Not one
   `NEXTEST_*`** (a `strings` sweep of the whole binary finds none either), so llvm-cov
   cannot redirect nextest's profile or config-file discovery.
3. Evidence from our own CI: the CF-S44 job emitted nextest's `SLOW [>17460s]` markers
   **while running under `cargo llvm-cov nextest`**, and green Coverage logs print
   `Nextest run ID … with nextest profile: default`. The slow-timeout machinery and
   profile resolution are demonstrably live in that job — only `terminate-after` was
   unset. `terminate-after` is evaluated on the same 60-second tick that emitted those
   markers.

**What is not proven:** the SIGTERM itself has not yet been observed under instrumentation.
The decisive test is the throwaway-branch run in §3.3 — it exercises the real job, the
real instrumentation and the detector together. **The job-level `timeout-minutes` is the
backstop and holds regardless**, since it is independent of nextest entirely.

One accepted cost: a SIGKILL'd test flushes no profile data, so a terminated test
contributes no coverage. It contributed none before either — it never exited.

---

## 4. Also worth doing — one click or one command each

- **Delete the two dead secrets.** `MONGO_CONNECTION_STRING` (2022-10-04) and
  `ZOOKEEPER_ADDRESS` (2022-09-30) are referenced by **no** workflow, script, crate or doc
  (verified by grep this session). They predate the Rust rewrite.
  ```sh
  gh secret delete MONGO_CONNECTION_STRING -R shieldblaze/ExpressGateway
  gh secret delete ZOOKEEPER_ADDRESS       -R shieldblaze/ExpressGateway
  ```
- **Secret scanning + push protection** — Settings → Code security. Currently off.
- **Dependabot alerts** — it is already opening PRs; confirm alerts themselves are on.

---

## 5. Corrections to the previous revision of this file

This file previously claimed the following. Each is corrected above.

| # | Previously said | Truth | Evidence |
|---|---|---|---|
| C1 | *"⚠️ Read §3 before requiring `Coverage`… that gate is a coin-flip today (3 red / 3 green on identical source)"*, and omitted `Coverage` from the required list — **15** contexts, not 16 | **Resolved.** The metric was corrected: `scripts/ci/coverage-check.sh:18` now scores **merged `DA:` records**, never the `LF:`/`LH:` summary, so the double-counting that caused the coin flip is gone. `Coverage` is the 16th required check | commit `14974c76`; before/after table in `audit/ci/s44-coverage-metric-rebaseline.md` |
| C2 | An entire §3 headed *"Coverage gate — needs a ruling, do not silently waive"*, presenting options (a)–(d) | **The ruling was made and shipped** — option (a). §3 is removed as an instruction; its provenance lives in the commit and the rebaseline doc. **A different Coverage caveat replaces it** (§1.1): the gate is honest now, but it still measures coverage, not pass/fail | as C1 |
| C3 | `"required_linear_history": true`, hedged as *"compatible with the repo's squash-only convention"* | **Wrong for this repo.** `main` has 51 merge commits in 853 and the last six promotions are all merges; the rule would reject the actual promote workflow. Now omitted | `git log --merges --oneline main` |
| C4 | `SOAK_*` names *"read from `release.yml:53-63`"* with per-variable line numbers 53–63 | **Names were all correct; the line numbers had drifted** (they were 44–54 before S46, and are 55–65 after). §2 now cites current lines **and** gives a grep so the next drift self-corrects | `grep -n "SOAK_" .github/workflows/release.yml` |
| C5 | `SOAK_INSTANCE_TYPE` present but the surrounding prose treated the set as 7 items | Kept and re-confirmed **optional**, defaulting to `c6a.2xlarge`. The full set is **1 secret + 7 variables (6 required + 1 optional)** | `scripts/release-soak.sh:41` |
| C6 | *"Against: `main` @ `eb00081d`"* | Stale header; this revision states its own base | `git rev-parse` |

`audit/release/project-state-assessment.md` carries the same superseded Coverage claims at
its lines 34 and 98 (the "R2 rotating red" entry). Its structural claims — no branch
protection, no `SOAK_*` set, 3 workflows — **remain true** as of this session.
