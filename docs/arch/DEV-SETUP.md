# Developer Setup

How to clone, build, test, run the gates locally, and run a soak — plus the box
requirements per task and the disk-management discipline that keeps a long-lived
dev box from re-accumulating clutter.

## Box requirements per task

| Task | Box | Why |
|------|-----|-----|
| Editing / docs / git / YAML | small — **t3.medium** (2–4 vCPU), **≥ 15 GB** free disk | file/git work; no heavy compile-under-load |
| `cargo build` / `cargo test` (a single pass) | 4 vCPU, **≥ 16 GB RAM**, **≥ 40 GB** free disk | the full `--all-features` test build is large (CF-DISK-1, ~40 GB); a 15 GB-RAM box must cap parallelism (below) |
| Coverage (instrumented) | 4 vCPU, **≥ 30 GB** free disk | the instrumented `--workspace` build is ~28 GB (CI frees runner disk first) |
| **Soak / perf / burn-in** | **8 vCPU + ENA** — **c6a.2xlarge** | the soak needs the realistic-core + datapath profile; a smaller box gives a non-representative result |
| XDP native attach (L4) | a box whose NIC supports native XDP (ENA `xdpdrv`) | validated live on Linux 7.0; single-kernel (see `docs/guide/DEPLOYMENT.md`) |

CI (GitHub hosted runners) covers everything except the soak and the multi-kernel
XDP matrix — those need dedicated hardware (the soak gate provisions it; F-ESC-1
tracks the kernel matrix).

## Toolchain

- **Rust 1.88** is the MSRV and the pinned channel — `rust-toolchain.toml`
  (`channel = "1.88"`, components `rustfmt` + `clippy`) is honored automatically
  by `rustup` on any cargo command. 1.88 is a **hard** requirement (quiche 0.29.1
  + tokio-quiche 0.19). Do not downgrade.
- **nightly** — only for `cargo fuzz` (the `fuzz-smoke` gate). `rustup toolchain
  install nightly` + `cargo install cargo-fuzz`.
- **eBPF nightly + bpf-linker** — the L4 XDP crate (`crates/lb-l4-xdp/ebpf`) has
  its OWN pinned nightly (`crates/lb-l4-xdp/ebpf/rust-toolchain.toml`) and needs
  `bpf-linker` + LLVM. Only required to **rebuild** the committed ELF
  (`scripts/build-xdp.sh`); normal builds use the checked-in
  `crates/lb-l4-xdp/src/lb_xdp.bin`, so most contributors never install it.

### System dependencies

quiche links BoringSSL (driven by cmake) and bindgen needs libclang:

```bash
sudo apt-get install -y cmake clang libclang-dev llvm pkg-config iproute2
```

(`iproute2` provides `ss`, used by the conformance gate to wait for listeners.)
Full prerequisites + the systemd unit are in `docs/guide/DEPLOYMENT.md`.

## Clone, build, run

```bash
git clone https://github.com/shieldblaze/ExpressGateway && cd ExpressGateway
cargo build --workspace --release        # binary: target/release/expressgateway
./target/release/expressgateway config/default.toml   # positional config arg — no --config flag
```

The binary is named **`expressgateway`** and takes the config path as a
**positional argument** (passing `--config` will not work). With no argument it
loads `config/default.toml`. Pick a starting point from `config/examples/`.

## Test

```bash
# The canonical session gate (mirrors CI's `test` job):
cargo test --workspace --all-features --no-fail-fast
```

- **`--all-features`** is required — it enables `test-gauges`, which the R8 memory
  integration tests read. Off by default.
- **`--no-fail-fast`** — get the full failure set, not first-fail truncation.
- The heavy real-wire e2e binaries (`grpc_h3_e2e`, `ws_*`) self-serialize via an
  in-file `static SUITE_SERIAL` tokio Mutex; `cargo test` runs test binaries
  sequentially. `fcap1_h2_over_cap_upload_yields_413` is CPU-sensitive (it must
  push 66 MiB past the 64 MiB cap) — CI isolates it with a 3-retry; locally on a
  loaded box it can flake on throughput (CF-SATURATION-1), not a real failure.
- **Low-RAM boxes (≤ 16 GB):** cap parallelism or the `--all-features` compile
  OOMs (SIGKILL looks like a compile-fail / exit 101):
  ```bash
  CARGO_BUILD_JOBS=4 cargo test --workspace --all-features --no-fail-fast
  ```

## Run the CI gates locally

The CI gates are thin wrappers you can run directly (see `.github/workflows/ci.yml`):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
bash scripts/ci/doc-lint.sh                 # operator-doc + audit-of-audit gate
bash scripts/ci/coverage-check.sh <lcov>    # per-module hot-path >= 80% (needs an lcov)
bash scripts/ci/h3spec-check.sh <h3spec> <host> <port>   # h3spec named-waiver gate
IMAGE=expressgateway:ci bash scripts/ci/docker-smoke.sh  # container run+serve smoke
```

## Run a soak

The soak harness is `crates/lb-soak` (binary `eg-soak`), orchestrated by
`scripts/soak/run-soak.sh`. A quick local stability soak (NOT throughput):

```bash
export CARGO_TARGET_DIR=$HOME/Code/eg-target          # shared target (below)
cargo build --release -p lb-soak --bin eg-soak -p lb --bin expressgateway
scripts/soak/run-soak.sh 900 /tmp/soak-out 15 1 sc1_h1h1 sc2_h2h2   # 900s, 2 scenarios
scripts/soak/soak-verdict.sh /tmp/soak-out sc1_h1h1 sc2_h2h2        # PASS iff all BOUNDED + panic=0
```

The **release** soak gate (full 12-scenario, dedicated EC2) is
`scripts/release-soak.sh` — see "Release soak gate" below.

## Disk management (don't let the box fill up)

The two biggest consumers and how to keep them bounded:

- **Shared `CARGO_TARGET_DIR`.** Builds across sessions/worktrees should share one
  target dir to avoid N copies. The convention here is
  `export CARGO_TARGET_DIR=$HOME/Code/eg-target` (a sibling of the repo, ~4–5 GB).
  The in-repo `target/` (~2 GB) is for ad-hoc builds; `target/` is gitignored.
- **Soak output is ephemeral.** Per-scenario time-series CSVs grow large over a
  long soak. **Ship verdicts/summaries, not raw CSVs** (the S37/S39 disk lesson).
  The release gate (`release-soak-onbox.sh`) deletes the CSVs and caps logs after
  computing the verdict; `release-soak-out/` is gitignored.
- **Gateway logs can fill disk.** A soaked gateway's stdout grows unbounded — cap
  or rotate it; never let `gateway.log` run free for hours (the S39 hazard).
- **Reclaiming space when pressured** (in priority order):
  ```bash
  cargo clean                       # or: rm -rf $CARGO_TARGET_DIR/{debug,release}
  rm -rf /tmp/soak-* /tmp/*-out      # ephemeral soak output
  # Old loose session-evidence dirs in ~/Code (eg-s17-evidence, eg-s18-*, s9-gate)
  # are pre-audit/ scratch — confirm the durable record is under audit/ in-repo,
  # then remove. They are tiny (~MB); they are NOT the canonical evidence trail.
  ```
- **Keep ≥ 15 GB free** for any build; the full `--all-features` test build alone
  can transiently consume ~40 GB (CF-DISK-1).

## Worktree & scratch discipline (avoid re-accumulating the mess)

- Prior sessions used `git worktree` for parallel agents. **Prune leftovers:**
  `git worktree prune -v` (removes stale registrations). Verify the live set with
  `git worktree list` — there should normally be just the main checkout.
- On a **shared** working tree, scope every commit with explicit paths
  (`git commit -- <paths>`) so you never sweep in another agent's staged files;
  verify with `git show --stat HEAD` before pushing. Never `git add -A` on a
  shared tree. Never `git stash` on a shared tree.
- Keep session scratch OUT of git: `*.log`, `target/`, `release-soak-out/`,
  fuzz runtime dirs are gitignored. Don't commit gate-run working dirs into
  `audit/` — the committed evidence is the curated report + markers, not raw
  scratch.

## Release soak gate

Release readiness includes a full 12-scenario soak that hosted CI cannot run
(it needs a dedicated, ENA-class box). The flow:

1. **Provision/run/teardown** is `scripts/release-soak.sh` (controller). In CI it
   is the `soak-gate` job in `.github/workflows/release.yml` (manual
   `workflow_dispatch`). It uses **GitHub OIDC → a scoped IAM role** (no
   long-lived keys), provisions a **c6a.2xlarge** (8-core + ENA), runs the soak,
   gates on **every scenario BOUNDED + panic=0** (R8), and **always tears the box
   down** (a trap, even on failure).
2. **On the box**, `scripts/soak/release-soak-onbox.sh` builds, runs the
   12-scenario soak (`scripts/soak/run-soak.sh`), computes the verdict
   (`scripts/soak/soak-verdict.sh`), uploads the compact summary to S3, and
   self-terminates.
3. **Validate the controller without spending money:** `./scripts/release-soak.sh
   --dry-run` prints every AWS command + the rendered user-data and runs nothing.

Required repo configuration (vars/secrets) before the first real run — documented
in the `scripts/release-soak.sh` header: `SOAK_AWS_ROLE_ARN` (secret),
`SOAK_REGION`, `SOAK_AMI`, `SOAK_SUBNET_ID`, `SOAK_SECURITY_GROUP_ID`,
`SOAK_IAM_INSTANCE_PROFILE` (PutObject to the result bucket + self-terminate),
`SOAK_S3_BUCKET`. Recommended flow: dispatch the soak gate, confirm PASS, **then**
tag the release (the tag push runs the build/publish pipeline).
