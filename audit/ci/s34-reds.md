# S34 — main CI reds, diagnosed and categorized

Base: `main @ 0d2bd3e9` (S33 promoted). Failing push run for that commit:
- **Build** (rust.yml) run `26989122703` — 3 red jobs
- **CI** (ci.yml) run `26989122681` — 7 red jobs
- **prod-readiness-gates** run `26989122664` — 6 red jobs (3 are the D2 matrix legs)

Runner: GitHub-hosted `ubuntu-24.04` (4 vCPU / 16 GB / 14 GB disk). Our box is 8 cores / 15 GB / 0 swap.

Categories: **(1) config-lag** — CI config behind the code. **(2) env-artifact** — runner differs from the box. **(3) real-failure** — genuine code/test problem.

The headline: the toolchain is **NOT** the problem. Every job's log shows `rust-toolchain.toml` correctly overriding to **1.88** (`note that the toolchain '1.88...' is currently in use`). The session is green on 1.88; CI runs on 1.88 too. The reds are almost all CI-invocation drift, plus two real code/policy gaps.

---

## ROOT CAUSE A — CI omits `--all-features` (single-sourced; category 1)

**Jobs:** CI/Check, CI/Clippy, CI/Test, CI/MSRV, Build/Check, Build/Clippy, Build/Test (7 jobs).

**Log:**
```
error[E0432]: unresolved imports `lb_l7::h2_proxy::H2_REQ_MAX_RETAINED_BODY_BYTES`, `lb_l7::h2_proxy::record_retained`
```
**Evidence:** `H2_REQ_MAX_RETAINED_BODY_BYTES` / `record_retained` are `#[cfg(any(test, feature = "test-gauges"))]` (lb-l7/src/h2_proxy.rs:3556). The root `lb-integration-tests` package exposes a `test-gauges` feature that forwards `lb-l7/test-gauges`, with the comment *"Off by default; the workspace gate enables it via `--all-features`."* (Cargo.toml:7-12). The session's canonical gate is `--all-features`; CI's `cargo check --workspace --all-targets` / `cargo clippy --workspace --all-targets` / `cargo test --workspace` all omit it, so the integration tests in `tests/*_md_*.rs` reference a symbol that isn't compiled → E0432.

**Category 1 (config-lag).** CI never matched the project's canonical test configuration. Fix: add `--all-features` (and `--all-targets` to `test`) in ci.yml + rust.yml. NOT gate-weakening — it makes CI compile+run *more* (the test-gauges R8 memory tests it currently can't even build). Note `build-and-lint` in prod-readiness-gates already uses `--all-features` and **passes** — proof the fix is right.

## ROOT CAUSE B — MSRV pin is stale 1.85, and the job is a no-op anyway (category 1)

**Job:** CI/MSRV (1.85). **Pre-authorized fix (R7).**

S31 moved the project MSRV to **1.88** (quiche 0.29.1 + tokio-quiche 0.19 hard-require it). The MSRV job pins `dtolnay/rust-toolchain@1.85` — but `rust-toolchain.toml` (channel = "1.88") **overrides it**, so the job actually compiles on 1.88 (log confirms). So the "MSRV 1.85" gate (a) tests the wrong version and (b) doesn't even test 1.85. It also inherits Root Cause A (no `--all-features`).

Stale siblings of the same lag:
- ci.yml:122-123 comment claims *"the repo's rust-toolchain.toml pins 1.85"* — false; it pins 1.88.
- `[workspace.package] rust-version = "1.85"` (Cargo.toml) — real MSRV is 1.88.
- prod-readiness-gates.yml:29 `RUST_MSRV: "1.85"` — overridden to 1.88, misleading.

Fix: bump the MSRV job + rust-version + RUST_MSRV to 1.88; correct the stale comment.

## ROOT CAUSE C — lb-soak missing panic-freedom deny lint (category 3, code fix)

**Job:** CI/Panic Freedom Audit.
```
##[error]Crates missing panic-freedom deny lints:\n  crates/lb-soak/src/lib.rs
```
**Evidence:** lb-soak (S20 soak harness) is the **only** crate of 18 missing `#![deny(clippy::unwrap_used, …)]`. Every other crate — including the other test-only crate `lb-h3-testcodec` — carries it. This is a **real gap the gate correctly caught**, not a CI bug.

**Fix in CODE (R3 — do NOT weaken the gate / do NOT exempt lb-soak from the glob).** Add the same deny block + `#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]` that lb-h3-testcodec uses. Tractable: only **~2** of lb-soak's 64 unwrap/expect sites are in non-test code (the rest are in `#[cfg(test)]` modules); those 2 get real error handling or a justified scoped `#[allow]`.

## ROOT CAUSE D — `.cargo/audit.toml` drifted out of sync with deny.toml (category 1)

**Job:** CI/Security Audit (`cargo audit -D warnings`).
```
rustls-pemfile  RUSTSEC-2025-0134  unmaintained
yaml-rust       RUSTSEC-2024-0320  unmaintained
error: 2 denied warnings found!
```
**Evidence:** Both advisories are **already ignored in deny.toml with full justifications** (lines 24-25 rustls-pemfile v2; lines 32-39 yaml-rust via foundations→serde_yaml). `gate-07-cargo-deny` **passes** because of this. But `.cargo/audit.toml` — whose own header says *"Mirrors deny.toml [advisories].ignore … keep them in sync"* — was never updated; it still only has the old `RUSTSEC-2025-0019`. The two ignore lists drifted.

**Category 1 (config-lag/sync).** Fix: mirror the two already-accepted, already-justified entries into `.cargo/audit.toml`. Both are `unmaintained` advisories on transitive deps (rustls-pemfile; yaml-rust via the held foundations 4.5) — not vulnerabilities, no reachable attack surface. This is the project's **documented** exception mechanism, NOT removing `-D warnings`.

## ROOT CAUSE E — D6-coverage references a deleted package (category 1)

**Job:** prod-readiness-gates / D6-coverage.
```
error: package ID specification `lb-h3` did not match any packages
help: a package with a similar name exists: `lb-h1`
```
**Evidence:** `lb-h3` was deleted in S26 (migration to `quiche::h3`). The coverage invocation still lists `--package lb-h3` (prod-readiness-gates.yml:101). Fix: drop `lb-h3` from the package list (align to the current hot-path scope). Then re-verify the `--fail-under-lines 80` threshold still holds.

## ROOT CAUSE F — D3a-chaos uses nextest but never installs it (category 1)

**Job:** prod-readiness-gates / D3a-chaos-attacks.
```
error: no such command: `nextest`
help: a command with a similar name exists: `test`
```
**Evidence:** the step runs `cargo nextest run …` but only the D6 job installs nextest; D3a never does. Fix: add the nextest install step. Then verify the `test(/chaos|rapid_reset|continuation|hpack|slowloris/)` filter actually matches tests and they pass (may need `--all-features`; confirm during the fix).

## ROOT CAUSE G — D4-conformance harness never boots the gateway (category 1/2, involved)

**Job:** prod-readiness-gates / D4-conformance.
```
{"level":"INFO","message":"ExpressGateway v0.1.0",...}     # binary starts...
Error: dial tcp [::1]:8443: connect: connection refused    # ...then h2spec can't connect
kill: (10044) - No such process                            # gateway already exited
```
**Evidence:** the gateway logs its banner then exits before binding 8443 — the 30× boot-wait curl loop all fail, so **no conformance case ever ran**. The workflow's hand-written `conformance.toml` is a TODO scaffold (*"TODO: confirm key names against crates/lb-config/src/lib.rs"*, line 135) whose schema (`protocol = "https"`, nested `[[listeners.h3]]`) does not match the real lb-config. This is harness config drift, **not** a conformance regression.

Two extra honesty constraints when re-wiring it:
- **h2spec --strict**: the session result is 146 pass / 1 / **0 fail**, so `--strict` should pass once the gateway boots (strict fails on FAILED cases, not skips).
- **h3spec**: the codebase carries ~12 **documented** h3spec failures (quiche limits #23/#25 etc., CF-QUICHE-UPGRADE). `./h3spec localhost 8443` with no skip would report them as failures → the step needs a **waiver list matching the documented carries** (the honest mechanism, parallel to the referenced `docs/conformance/h2spec-waivers.md`), not a blanket pass.

**Most involved fix of the session.** Needs a real lb-config conformance.toml + booting an H1/H2 (and H3/QUIC) listener in CI + the h3spec waiver list.

## ROOT CAUSE H — D2-verifier-matrix action ref broken + likely infeasible on hosted runners (category 2)

**Jobs:** prod-readiness-gates / D2-verifier-matrix (5.15, 6.1, 6.6) — all fail at "Set up job".
```
##[error]Unable to resolve action `danobi/vmtest-action@main`, unable to find version `main`
```
**Evidence:** the kernel-VM matrix uses `danobi/vmtest-action@main`; the `@main` ref no longer resolves. Pinning it is necessary but **not sufficient**: `vmtest` boots QEMU/KVM VMs and standard GitHub hosted runners provide **no nested virtualization (`/dev/kvm`)**, so the 5.15/6.1/6.6 boot matrix cannot run honestly on `ubuntu-latest`. The XDP object path it loads (`./target/bpf/xdp_prog.o`) is also a TODO that may not match the aya build output.

**Category 2 (env-artifact).** This is the one red that may not be honestly green-able on hosted CI. Candidate honest resolutions (owner steer needed): convert D2 to a runner-kernel XDP **load smoke** (build the object + `bpftool prog load` on the runner's own 6.x kernel, which IS possible) and document the full 5.15/6.1/6.6 verifier matrix as self-hosted/D-1-class hardware work — rather than leave a broken `vmtest@main`.

## ROOT CAUSE I — Unused Deps (cargo-machete) flags 12 crates (category 1 or 3; non-blocking)

**Job:** CI/Unused Deps (cargo-machete) — `continue-on-error: true`, so it does **not** fail the CI run.
Flags: lb-health, lb-grpc, lb, lb-security, lb-soak, lb-io, lb-controlplane, lb-balancer, lb-xdp-ebpf, lb-l4-xdp, lb-core, lb-h2. Needs local `cargo machete` to see the specific deps. The ci.yml comment marks this informational (*"known feature-gated re-exports occasionally false-positive"*). Honest handling: per-dep, either remove genuinely-unused deps (cat 3 cleanup) or add justified `[package.metadata.cargo-machete] ignored` entries (the documented false-positive mechanism). Investigate during the fix.

## SIDE FIX — dependabot.yml actions group (pre-authorized R7)

`.github/dependabot.yml` groups github-actions with pattern `"*"`, which sweeps `dtolnay/rust-toolchain` (the #214 bug that bumped it to a bogus `@1.100`). Fix: exclude `dtolnay/rust-toolchain` from the actions group.

---

## Summary table

| Red job | Root cause | Category | Fix |
|---|---|---|---|
| CI/Check, CI/Clippy, CI/Test | missing `--all-features` (test-gauges) | 1 | add `--all-features`/`--all-targets` (ci.yml+rust.yml) |
| Build/Check, Build/Clippy, Build/Test | same as above (duplicate workflow) | 1 | same, single-sourced |
| CI/MSRV (1.85) | stale 1.85 pin (no-op, overridden to 1.88) + no `--all-features` | 1 | bump to 1.88; `--all-features` |
| CI/Panic Freedom Audit | lb-soak missing deny lint | **3** | add lint to lb-soak (code) |
| CI/Security Audit | audit.toml ↛ deny.toml drift (2 already-justified advisories) | 1 | sync audit.toml |
| D6-coverage | `--package lb-h3` deleted in S26 | 1 | drop lb-h3; recheck 80% |
| D3a-chaos-attacks | nextest not installed | 1 | install nextest; verify tests pass |
| D4-conformance | TODO config doesn't boot gateway; h3spec needs waivers | 1/2 | real config + h3spec waiver list |
| D2-verifier-matrix (×3) | `vmtest-action@main` unresolvable + no KVM on hosted | **2** | pin + rescope to runner-kernel smoke (owner steer) |
| Unused Deps (machete) | 12 crates flagged (non-blocking) | 1/3 | investigate; remove or justified-ignore |
| dependabot.yml | actions group sweeps rust-toolchain | 1 | exclude rust-toolchain |

**Category-3 (real code) reds:** exactly one clear one — **lb-soak panic-freedom** (small). Possibly machete (pending investigation). Everything else is config-lag (1) or env-artifact (2). The Security Audit advisories are real but already-accepted policy entries that merely failed to mirror into audit.toml.
