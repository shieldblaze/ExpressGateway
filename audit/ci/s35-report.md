# Session 35 — Repo hygiene report

Branch cleanup + CI workflow standardization + working Docker image. Productization
("make the repo shippable"). Base: `main @ 93d011c7` (S34 honest-green tip).
Branch: `feature/repo-hygiene-s35`. PR: **#227**.

NO production protocol source changed. The only source-tree change is dependency
*removals* (cargo-machete fix) + Docker entrypoint/config wiring.

---

## Phase 0 — baseline
- Base tip confirmed `93d011c7`; branched + pushed `feature/repo-hygiene-s35`.
- Session-local `cargo test --workspace --all-features --no-fail-fast` baseline run 1
  GREEN (247 `test result: ok`, 0 FAILED — matches S34's recorded 247).
- Full CI gate inventory (the "before" set, 24 jobs / 5 workflow files):
  `audit/ci/s35-gate-inventory.md`.
- Disk hygiene (CF-DISK-1): freed the 28 GB regenerable `llvm-cov-target` at start;
  the local verification builds are large (full `--all-features` test build of this
  workspace is ~40 GB with debuginfo), so the session-local ×3 final run uses
  `CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0` (test-identical; CI's `Test` job
  runs full-fidelity on hosted runners). Docker layers pruned between steps.

## Phase 1 — branch cleanup (the careful, irreversible part)
Classification of every non-`main`/`old-main` branch. **Method:** a branch is
"provably merged" (safe to delete) iff `git cherry origin/main origin/<b>` shows
ZERO `+` lines (every commit has a patch-id-equivalent on `origin/main`) OR it has
a MERGED PR. Anything with a genuinely-unmerged commit → KEPT + surfaced (never
`-D`, never delete unmerged).

### Deleted (provably merged — remote)
| Branch | Evidence | Disposition |
|---|---|---|
| `ci/prod-readiness` | cherry: 0 `+` / 1 merged-equiv (prod-readiness-gates.yml on main) | deleted |
| `s6/builder-1-h3h3-plan` | cherry: 0 `+` (s6-h3h3-plan.md on main) | deleted |
| `s6/phase3-regate` | cherry: 0 `+` (phase3-regate evidence on main) | deleted |
| `s7/builder-2` | cherry: 0 `+` (panic_abort.rs fix on main) | deleted |

### Kept + surfaced (genuinely unmerged — NOT deleted)
| Branch | Ahead | Why kept |
|---|---|---|
| `feature/grpc-h3-churn-rss-s32` | 4 | The S32 *fix evidence* (s32-quiche-collected-fix.diff, report, a2-sustained data) IS on main (verified) — but the branch ALSO carries **unpromoted diagnostic probe code** (`crates/lb/src/diag_mem.rs`, +59 env-gated mem-diag lines in conn_actor.rs, the churn-probe example). Carry-forward value for the S36 connection-recycling fix. NOT redundant. |
| `feature/cfbw-s14-verify` | 1 | s14-verify escalation doc (CF-BODY-WALLCLOCK), unmerged. |
| `s11-verify` | 1 | s11 coverage verify (s11-h1h2-cov.awk), unmerged. |
| `s6/phase3-gate` | 1 | s6 phase-3 MANDATORY-GATE doc (FAIL verdict snapshot), unmerged. |
| `s7/verifier` | 21 | H3→H3 verify work, unmerged. |
| `s8/verifier` | 2 | H2→H1 M-D re-measure, unmerged. |
| `gg` / `restructcure` / `rust` | 64 / 204 / 209 | Unrelated histories (no merge-base / pre-rewrite era; `rust` = closed PR #205). Cannot prove merged → kept. |
| `dependabot/cargo/dependencies-197b239d55` | — | OPEN PR #224 — untouched. |
| `dependabot/github_actions/actions-ffa548d3e0` | — | OPEN PR #226 — untouched. |

### Refs / local
- Pruned **25 dead `agents/*` remote-tracking refs** (the `agents` remote is no longer
  configured; purely local cosmetic — no GitHub change).
- Local merged branch `feature/ci-reconcile-s34` deleted via `git branch -d` (safe);
  the unmerged session-branch local copies were correctly *refused* by `-d` and kept.

## Phase 2 — CI workflow standardization (5 → 4 files, 0 gates dropped)
See **`audit/ci/s35-gate-map.md`** for the full per-gate before→after accounting.
- **Single-sourced setup** (R12): `.github/actions/rust-setup` composite action
  (toolchain + cache + system-deps), referenced across ci / prod-readiness /
  scheduled-scans.
- **Folded** `prod-readiness build-and-lint` → ci `fmt`/`clippy` (stable, ==/stricter)
  + `msrv` (compiles-on-1.88) + `release-build` (codegen). (S34-deferred plan.)
- **Folded** `codeql.yml` (misnamed weekly *weak* `cargo audit`) → ci `audit`
  (strict, per-PR) + `scheduled-scans.yml` `audit` (strict `-D warnings`, weekly
  cadence preserved). Strengthened, not weakened.
- **Deleted** `dependabot.yml` no-op `notify` (cruft).
- **Moved** `geiger` + `machete` ci → `scheduled-scans.yml` (weekly, off PR path; R7).
- **Strengthened** `D5-image-scan`: build → RUN+SERVE smoke → Trivy.

### cargo-machete RED → green (honest fix)
17 genuinely-unused deps removed across 12 crates (lb-soak: serde, tracing;
lb-balancer: parking_lot; lb: rustls, rustls-pemfile, rustls-pki-types; lb-health:
tokio; lb-h2: http; lb-security: bytes, tracing; lb-l4-xdp/ebpf: aya-log-ebpf;
lb-l4-xdp: rand; lb-controlplane: serde, serde_json; lb-io: bytes; lb-core: bytes;
lb-grpc: http). **Zero false positives** — each verdict independently grep-confirmed
(path + derive + macro usage all zero) by an author≠analyzer split; `cargo check
--workspace --all-targets --all-features` + `clippy -D warnings` + `fmt` all green
after removal; `cargo machete` exits 0. Cargo.lock: 16 edge deletions, **no version
changes**. No `[package.metadata.cargo-machete]` ignores needed.

## Phase 3 — working Docker image + run-and-serve smoke
- **Root cause found + fixed:** the pre-S35 image could not boot. `CMD ["--config",
  "/etc/expressgateway/config.toml"]` but the binary takes the config path as a
  **positional** arg (`crates/lb/src/main.rs:1910`, `std::env::args().nth(1)` — no
  flag parser). `docker build` (D5) never runs the container, so the broken runtime
  CMD went unnoticed. Fixed CMD to the bare path (exec-form; distroless has no shell).
- Added `CARGO_BUILD_JOBS=4` build-arg default (bounds peak build RAM; hosted CI is
  4-core so it's full-speed there).
- `docker/smoke/gateway.toml`: one plaintext H1 listener (0.0.0.0:8080) → backend.
- `scripts/ci/docker-smoke.sh`: starts a backend + the gateway container on a
  user-defined network, sends a **real HTTP/1.1 request through the running
  container**, asserts 200 + backend body, tears down on exit (trap), rootless-OK.
- **Local proof (R15):** built `expressgateway:smoke`, ran the smoke →
  `gateway returned status=200 body='eg-smoke-ok'` → **PASS** (`audit/ci/s35-docker-smoke.log`).
- CI: folded into `D5-image-scan` (build once → smoke → Trivy).

## Verification & promote
### Branch CI (PR #227) — FULLY GREEN (completed-run reads, R15)
- **CI run `27048271090`: success** — all 10 jobs: Check, Clippy, MSRV (1.88), Test,
  Format, Doc Lint, Panic Freedom, Security Audit, Fuzz Smoke, Release Build.
- **prod-readiness-gates run `27048271091`: success** — all 6 jobs: gate-07-cargo-deny,
  D4-conformance, **D5-image-scan (build + run-and-serve smoke + Trivy)**,
  D2-xdp-verifier-smoke, D3a-chaos-attacks, D6-coverage.
- Every distinct gate from the inventory ran + passed; the composite action works in
  all jobs; the Docker container serves real traffic in CI's D5 (not just locally).

### Session-local ×3 (disk-bounded: debuginfo=0, incremental=0)
- **3 clean full-suite passes** — run1: **247 ok / 0 FAILED** ✅ · run3: **247/0** ✅ · run4: **247/0** ✅
- run2: 246 ok / **1 FAILED** — `t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream`
  (`crates/lb-quic/tests/h3_h1_stream_body_e2e.rs:766`), failure mode
  `timed_out=true sent=1048619/1048619` (100% of the 1 MiB body sent, then the
  round-trip missed its deadline).
- **Characterized as a pre-existing CF-SATURATION-1-family throughput flake**, NOT a
  regression: passed in run1, run3, branch-CI Test, AND **5/5 isolation** (8/9 total).
  Only surfaces under the 4-way in-binary parallel load of the `h3_h1_stream_body_e2e`
  binary. The session's changes (dep removals, workflow YAML, Docker) do not touch the
  H3→H1 streaming path. clippy `-D warnings` + fmt green.
- **CF-S35-T5-FLAKE (carry-forward):** candidate for the same isolate-and-retry
  treatment fcap1 got (or a deadline bump), in a future session — out of S35 scope
  (R2: flake protocol unchanged; no test/protocol mutation this session).

### Promote + post-merge
- Promote (--no-ff merge of PR #227 into main): see git history / S35.md.
- Post-merge main CI green: confirmed in `/home/ubuntu/S35.md` (completed-run id).
- scheduled-scans.yml dispatch-verify (only dispatchable once on the default branch):
  confirmed post-merge — see S35.md.

## Verdict
**SESSION 35 COMPLETE** — branches cleaned (4 provably-merged deleted, unmerged
surfaced); 5→4 workflow files with a single-sourced composite action and **0 distinct
gates dropped** (gate-map verified); cargo-machete RED→green (17 honest removals);
working Docker image that **builds + runs + serves** (run-and-serve smoke green locally
AND in CI's D5). Branch CI fully green (CI `27048271090`, prod-readiness `27048271091`),
session-local 3× clean (+1 characterized pre-existing CF-SATURATION-1 t5 flake). The
post-merge main-CI-green citation is recorded in `/home/ubuntu/S35.md`.
