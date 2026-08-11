# Owner actions — copy-paste pack

**Prepared:** 2026-08-02 · **Against:** `main` @ `eb00081d` · **Verified from source**, not from docs.

Two things in this repo can only be done with owner credentials. An agent cannot do either.
Everything else in the program is defended by gates that are **not currently enforced**.

---

## 1. Branch protection — currently NONE

Verified at handoff:

```
$ gh api repos/:owner/:repo/branches/main/protection
{"message":"Branch not protected","status":"404"}
$ gh api repos/:owner/:repo/rulesets
[]
```

**Anyone with write access can push directly to `main`, and zero checks are required.**
The previous session pushed straight to `main` unopposed. Secret scanning and push
protection are also off.

### The 16 required checks

These are the exact job names from the green run `30751410142`, cross-checked against
`.github/workflows/ci.yml`. They must match character-for-character or the check will
never be satisfied and PRs will hang forever.

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

> ⚠️ **Read §3 before requiring `Coverage (per-module hot-path >= 80%)`.** That gate is a
> coin-flip today (3 red / 3 green on identical source). If you require it as-is, roughly
> half of all PRs will be blocked by a measurement artifact rather than by a real
> regression. Either fix the metric first, or add it to the required list afterwards.

### Apply it

```sh
cat > /tmp/protection.json <<'JSON'
{
  "required_status_checks": {
    "strict": true,
    "contexts": [
      "Format",
      "Check",
      "Clippy",
      "Panic Freedom Audit",
      "Doc Lint",
      "Test",
      "MSRV (1.88)",
      "Fuzz Smoke Test",
      "Release Build",
      "Security Audit",
      "cargo-deny (licenses/advisories/bans/sources)",
      "Conformance (h2spec --strict + h3spec)",
      "Chaos Attack Suite",
      "Container Image (build + serve smoke + trivy)",
      "XDP Verifier Smoke (runner kernel)"
    ]
  },
  "enforce_admins": true,
  "required_pull_request_reviews": {
    "required_approving_review_count": 1,
    "dismiss_stale_reviews": true,
    "require_last_push_approval": true
  },
  "restrictions": null,
  "required_linear_history": true,
  "allow_force_pushes": false,
  "allow_deletions": false,
  "required_conversation_resolution": true
}
JSON

gh api -X PUT repos/shieldblaze/ExpressGateway/branches/main/protection \
  -H "Accept: application/vnd.github+json" --input /tmp/protection.json
```

Notes on the choices above — change any of these if you disagree:

- `enforce_admins: true` — without it, admins bypass everything and the gate is decorative.
- `strict: true` — branches must be up to date with `main` before merging. Costs a re-run
  on busy days; catches semantic conflicts that a clean textual merge hides.
- `required_linear_history: true` — **compatible with the repo's squash-only convention**,
  but note R11 promotes with `--no-ff` merge commits. If you keep promoting with `--no-ff`,
  set this to `false` or the promote will be rejected.
- `Coverage (...)` is **deliberately omitted** from the list above pending §3.

### Also worth enabling (separate, one click each in Settings → Code security)

- Secret scanning + **push protection** (currently disabled).
- Dependabot alerts (already producing PRs, but confirm alerts are on).

### Verify it took

```sh
gh api repos/shieldblaze/ExpressGateway/branches/main/protection \
  -q '.required_status_checks.contexts, .enforce_admins.enabled'
```

---

## 2. Release soak gate — un-armed

`.github/workflows/release.yml` provisions an EC2 box, runs the soak, reads a verdict, and
tears the box down. It cannot run: **none of its inputs are set.** The only repo secrets are
`MONGO_CONNECTION_STRING` and `ZOOKEEPER_ADDRESS`, both from 2022 and both unrelated —
they look like leftovers from the pre-Rust project and are probably worth deleting.

Exact names, read from `release.yml:53-63` (all are `SOAK_`-prefixed):

**Secret (1)**

| name | line | notes |
|---|---|---|
| `SOAK_AWS_ROLE_ARN` | 53 | OIDC role to assume; needs a trust policy for this repo |

**Variables (7)**

| name | line | notes |
|---|---|---|
| `SOAK_REGION` | 54, 57 | also used for the AWS credential config step |
| `SOAK_AMI` | 58 | Ubuntu AMI in that region |
| `SOAK_SUBNET_ID` | 59 | must have egress for the S3 upload |
| `SOAK_SECURITY_GROUP_ID` | 60 | |
| `SOAK_IAM_INSTANCE_PROFILE` | 61 | instance needs S3 write to the bucket below |
| `SOAK_S3_BUCKET` | 62 | soak artifacts/verdict land here |
| `SOAK_INSTANCE_TYPE` | 63 | **optional**, defaults to `c6a.2xlarge` |

```sh
R=shieldblaze/ExpressGateway
gh secret set   SOAK_AWS_ROLE_ARN         -R $R   # prompts, not echoed
gh variable set SOAK_REGION               -R $R --body "us-east-1"
gh variable set SOAK_AMI                  -R $R --body "ami-…"
gh variable set SOAK_SUBNET_ID            -R $R --body "subnet-…"
gh variable set SOAK_SECURITY_GROUP_ID    -R $R --body "sg-…"
gh variable set SOAK_IAM_INSTANCE_PROFILE -R $R --body "…"
gh variable set SOAK_S3_BUCKET            -R $R --body "…"
```

### Then prove it end-to-end before trusting it

The whole point of this gate is that it tears the box down. An untested teardown path is a
standing bill. Do a `workflow_dispatch` dry-run and confirm all four stages:
**provision → soak → verdict → teardown**, then check the console for a surviving instance.

```sh
gh workflow run release.yml -R $R
gh run watch -R $R
aws ec2 describe-instances --region "$SOAK_REGION" \
  --filters "Name=instance-state-name,Values=running,stopped" \
  --query 'Reservations[].Instances[].[InstanceId,Tags]'   # expect: none from the soak
```

---

## 3. Coverage gate — needs a ruling, do not silently waive

Full analysis: `audit/ci/s43-h2proxy-coverage-dual-instantiation.md`.
Six samples now exist on effectively identical production source:

| run | h2_proxy.rs | gate |
|---|---|---|
| 28334977156 (artifact 7938582116) | 79.60% | RED |
| 28334977156 (artifact 7938395373) | 79.70% | RED |
| 28336426614 | 80.18% | green |
| 30745595161 | 79.65% | RED |
| 30749813681 | 80.23% | green |
| 30751410142 | 80.13% | green |

**3 red / 3 green — a coin flip**, not the "~1 in 4" the handoff records, and not the
"3 of 4 fail" the S43 doc records (it had only the first four samples). Mean ≈ 79.92%,
spread ≈ 0.6 pp, against a hard 80.00 floor. `main` is green today by luck.

**Root cause is a measurement artifact, not thin tests.** `lb-l7` is compiled twice; llvm-cov
merges both instantiations into one `SF:` record whose `LF:`/`LH:` summary (which the gate
reads) double-counts lines, while the per-line `DA:` records do not. The second instantiation
is the lib-unit-test build, which *structurally cannot* reach the request hot path —
`hyper::body::Incoming` has no public constructor. Its unhit lines are counted as real misses.

The options, unchanged from the S43 write-up — **(a) is the recommendation**:

- **(a) Score from merged `DA:` records** instead of the `LF:`/`LH:` summary, so each source
  line counts once. h2_proxy then reads **80.96%** and passes honestly. This is a
  *correction* toward the charter's stated metric, not a loosening — every threshold stays
  at 80%. Requires re-baselining all 31 modules and publishing a before/after table so it is
  provably not a relaxation.
- **(b)** De-duplicate the instantiations so only one build is measured. Cleanest in theory,
  fiddly in practice.
- **(c)** Add real-wire tests for the SNI / watchdog / underscore-policy paths. Real test
  value, but does not touch the double-counting, and the uncovered arms need a
  **non-loopback** bind, which is CI-fragile.
- **(d)** Leave it. Honest, but keeps `main` red half the time on a number that is not
  measuring what it claims.

Whatever you choose: it should be a deliberate, documented gate change with the before/after
table published — not folded silently into another PR.
