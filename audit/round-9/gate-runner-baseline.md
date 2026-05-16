# Round-9 BASELINE Scorecard — gate-runner

- **Branch:** feature/h3-green (snapshot context claimed HEAD 4411523b, base prod-readiness/round-4)
- **Git HEAD at run:** `4411523ba5044cde3d668744c635e56f916d6c9b`
- **Date:** 2026-05-16
- **Operator:** gate-runner
- **No source modified by gate-runner.** A temporary `.dockerignore` was created for the D-5 build and **removed** afterward (tree restored).

## ⚠ BASELINE-INTEGRITY CAVEAT (read first)

The working tree was **NOT pristine** at run start. `git status` showed 4 files
with uncommitted modifications (timestamps 13:34–13:35 UTC today — concurrent
"trailer-passthrough" WIP by another agent on the shared branch, ~187 lines):

```
 M crates/lb-l7/src/h1_proxy.rs
 M crates/lb-l7/src/h2_proxy.rs
 M crates/lb-l7/tests/trailer_passthrough.rs
 M crates/lb-quic/src/h3_bridge.rs
```

`git stash list` shows `stash@{0}: On prod-readiness/round-4: other-agent-wip`.
All gates below ran against **HEAD 4411523b + this uncommitted WIP**, not pristine
HEAD. gate-runner did **not** revert it (mission: do not modify source).

## Environment / disk note

Disk is small (28 GB root, ~4 GB free at start). Running the heavy gates
(`cargo build`, `cargo llvm-cov`, `docker build`) concurrently exhausted the
disk and produced spurious `ld: No space left on device` failures. All heavy
gates were therefore **re-run serially** with target dirs / docker cache pruned
between runs. Verdicts below are from the serial (clean) runs.

---

## Toolchain

```
cargo 1.85.1 (d73d2caf9 2024-12-31)
rustc 1.85.1 (4eb161250 2025-03-15)
cargo-deny 0.19.6
cargo-llvm-cov 0.8.7
trivy 0.70.0
clang 21.1.8
bpf-linker 0.10.3
bpftool v7.7.0
h2spec 2.6.0
h3spec 0.1.13
go: NOT installed
nightly-2026-01-15 toolchain present (ebpf pin)
```

Note: STATE warned `cargo-deny>=0.19 needs rustc>=1.88`. **Not a blocker** —
cargo-deny 0.19.6 ran successfully on rustc 1.85.1.

---

## Gate 07 — `cargo deny check`  → PASS

Exit code: **0**

Summary line (verbatim):
```
advisories ok, bans ok, licenses ok, sources ok
```

0 errors. 29 warnings, categorised:
- 6 × `advisory-not-detected` (waivers in deny.toml that no longer match any crate)
- 20 × `duplicate` (e.g. base64 0.21.7 vs 0.22.1 via tonic/opentelemetry vs hyper-util)
- 2 × `license-not-encountered` (CC0-1.0, CDLA-Permissive-2.0 unmatched allowances)
- 1 × `no-license-field` (lb-integration-tests workspace test crate)

advisories/bans/licenses/sources all **ok**. Verdict **PASS**.

---

## Gate — `cargo fmt --check`  → PASS

Exit code: **0**, no output (formatting clean).

## Gate — `cargo clippy --all-targets --all-features -- -D warnings`  → PASS

Exit code: **0**, zero warnings.
```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 21.28s
```

## Gate — `cargo build --workspace`  → PASS

Exit code: **0** (serial run; first concurrent run failed only on disk-exhaustion `ld` error).
```
   Compiling lb v0.1.0 (/home/ubuntu/Code/ExpressGateway/crates/lb)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.59s
```

---

## D-2 — eBPF verifier / ELF build  → FAIL (expected; pre-c2bbfea6 base)

`scripts/build-xdp.sh` and `scripts/verify-xdp.sh` inspected.

1. Committed ELF `crates/lb-l4-xdp/src/lb_xdp.bin` exists (tracked at commit
   ac58f613, ELF 64-bit eBPF). It **fails to load** with modern bpftool:
   ```
   libbpf: elf: legacy map definitions in 'maps' section are not supported by libbpf v1.0+
   Error: failed to open object file
   ```
2. Fresh build via `scripts/build-xdp.sh` (pinned nightly-2026-01-15 present)
   **FAILS at link**:
   ```
   error: linking with `bpf-linker` failed: exit status: 1
     Error: unexpected argument '-g' found
   ```
   The script exports `RUSTFLAGS=… -Clink-arg=-g`; bpf-linker 0.10.3 rejects `-g`.
   This is exactly the bug fixed by the absent commit **c2bbfea6** ("D-1/D-2
   eBPF ELF build").
3. ELF-section tests confirm the defect (from llvm-cov test run):
   - `lb-l4-xdp/tests/elf_sections.rs::license_section_says_gpl` FAILED —
     "BPF ELF must declare a `license` section — see EBPF-2-01"
   - `…::btf_sections_present_and_non_empty` FAILED — "BPF ELF must declare `.BTF`"
   - `lb-observability/tests/metrics_xdp_slots.rs` FAILED — NUM_SLOTS 10 vs 16.

`verify-xdp.sh` additionally requires Docker + lvh-images with **no pinned digest**
(refuses without `EG_ALLOW_FLOATING_IMAGE=1`; exit 3).

**Verdict FAIL** — a loadable lb_xdp ELF cannot be produced on the round-4 base;
the c2bbfea6 fix is absent by design.

---

## D-6 — coverage  → FAIL

Mission command `cargo llvm-cov --workspace --no-fail-fast --summary-only`
**does not emit a coverage table** on this base: 4 test targets fail
(`balancer_counter_sync`, `h2spec`, `lb-l4-xdp/elf_sections`,
`lb-observability/metrics_xdp_slots`) and llvm-cov aborts the report.
(`--ignore-run-fail` is mutually exclusive with `--no-fail-fast` in v0.8.7.)

To obtain a number, re-ran with `--ignore-run-fail` alone (exit 0):

```
TOTAL   Lines 11589/ -3531 = 69.53% | Functions 79.16% | Regions 79.27%
```

Documented baseline (docs/conformance/coverage.md, HEAD de5c6dbf):
**75.45% lines** / 81.57% fn / 85.38% regions.

Current **69.53% lines** = **~6-point regression** vs documented baseline
(note: depressed partly by uncommitted WIP and by counting `lb/src/main.rs`
11.18% & `lb/src/xdp.rs` 17.72%).

Files **< 80% line coverage NOT in the documented waiver list**:
```
lb-l7/src/h2_proxy.rs              60.00%   (likely WIP-affected)
lb-observability/src/admin_http.rs 79.70%
lb-observability/src/lib.rs        64.39%
lb-observability/src/log.rs        54.84%
lb-observability/src/prometheus_exposition.rs 66.00%
lb-quic/src/conn_actor.rs          73.91%
lb-quic/src/h3_bridge.rs           70.03%   (WIP-affected)
lb-quic/src/listener.rs            68.82%
lb-quic/src/router.rs              66.43%
lb-security/src/handshake.rs       66.67%
lb-security/src/retry.rs           72.29%
lb/src/xdp.rs                      17.72%
```

**Verdict FAIL** — exact mission command yields no table (pre-existing test
failures); coverage 69.53% < 75.45% documented baseline; ≥12 files below the
≥80% per-crate gate are unwaived.

Failing-test panic excerpts:
```
balancer_counter_sync.rs:83  snapshot diverged from atomic at sample 411: snapshot=10385 bracket=[10386,10387]
h2spec.rs:188                h2spec failed with exit status Some(1)
elf_sections.rs:31           BPF ELF must declare a `license` section — EBPF-2-01
elf_sections.rs:45           BPF ELF must declare `.BTF` — EBPF-2-01
metrics_xdp_slots.rs:22      stat_slot_labels() len 10 != NUM_SLOTS 16
```

---

## D-5 — docker build + trivy  → FAIL (image cannot be built)

`docker/Dockerfile` (cargo-chef multi-stage). No `.dockerignore` in repo →
3.5 GB build context (target/); created a temporary `target/`+`.git/`
`.dockerignore` (since removed).

`docker build -f docker/Dockerfile` **FAILS** in builder stage:
```
error[E0063]: missing field `trailers` in initializer of `H3Request`
   --> crates/lb-quic/src/h3_bridge.rs:149:9
error: could not compile `lb-quic` (lib)
ERROR: process "… cargo build --release -p lb …" exit code: 101
DOCKER_EXIT=1
```
Cause: cargo-chef `prepare`/`cook` recipe captured a tree state inconsistent
with the **uncommitted h3_bridge.rs WIP** (current source DOES initialise
`trailers` at h3_bridge.rs:159; local `cargo build --workspace` passes clean).
The Dockerfile's cargo-chef layering + dirty tree produce an inconsistent build.

No image produced ⇒ **trivy cannot run**. HIGH/CRITICAL counts: N/A.
Repo has **no `.trivyignore`** (the c2bbfea6 waiver is absent by design).

**Verdict FAIL** — no scannable image on this base (dirty-tree + cargo-chef);
trivy BLOCKED on no-image.

---

## D-1 — native ENA XDP attach feasibility  → BLOCKED (env-capable, test is a stub on this base)

Probes (ens5 untouched, never brought down):
```
ethtool -i ens5 → driver: ena   (version 7.0.0-1004-aws)
ethtool -S ens5 → queue_N_rx_xdp_{aborted,drop,pass,tx,invalid,redirect}  ⇒ ENA supports XDP
/boot/config-7.0.0-1004-aws:
   CONFIG_BPF=y
   CONFIG_BPF_SYSCALL=y
   CONFIG_XDP_SOCKETS=y
   CONFIG_XDP_SOCKETS_DIAG=m
```
`crates/lb-l4-xdp/tests/xdp_attach_mode.rs` exists, **compiles** (cargo test
--no-run exit 0), and the `#[ignore]` test runs (it uses a `dummy0` netdev, not
ens5). But on the round-4 base it is a **STUB**:
> "EBPF-2-04 SKB fallback test stub — full kernel scaffold lands with the
> EBPF-2-05 pinning fixture … Until CI privileged stage is available, the
> always-on coverage is stats_export_round_trip_drv_skb_hw …"
(`sudo` here also drops `-E`: "preserving the entire environment is not supported".)

**Verdict BLOCKED** — environment is fully XDP-capable (ENA + kernel config +
CAPs available) but the actual native-attach test scaffold is not present on
the round-4 base (lands with EBPF-2-05, and the loadable ELF is blocked by D-2).

---

## D-4 — h2spec / h3spec  → PASS (binaries) / DEFERRED (conformance run)

```
h2spec --version → Version: 2.6.0 (70ac229…)         exit 0
h3spec           → Usage: h3spec <host> <port>; -v --version   exit 0
h3spec -v        → h3spec 0.1.13
```
Both binaries functional. Actual conformance run needs a running lb listener
(TLS-configured) — **DEFERRED** per mission (non-trivial to stand up here).
Note: the in-suite `lb-integration-tests/tests/h2spec.rs` test FAILED during
the llvm-cov run (`h2spec failed with exit status Some(1)`) — flagged for the
real D-4 run.

**Verdict PASS** for binary capability; conformance DEFERRED.

---

## SCORECARD

| Gate | Verdict | Evidence (1-line) | Blocker-if-any |
|------|---------|-------------------|----------------|
| gate-07 cargo deny | **PASS** | `advisories ok, bans ok, licenses ok, sources ok` (exit 0; 29 warnings, 0 errors) | — |
| fmt | **PASS** | `cargo fmt --check` exit 0, no diff | — |
| clippy | **PASS** | `--all-targets --all-features -D warnings` exit 0, 0 warnings | — |
| build --workspace | **PASS** | exit 0 (serial run; concurrent run only failed on disk) | — |
| D-2 eBPF ELF | **FAIL** | build-xdp.sh: bpf-linker rejects `-g`; committed ELF lacks license/.BTF, won't load | c2bbfea6 ELF-build fix absent on round-4 base (by design) |
| D-6 coverage | **FAIL** | mission cmd emits no table (4 test fails); coverage 69.53% < 75.45% baseline; ≥12 unwaived <80% files | pre-existing test failures + uncommitted WIP depress coverage |
| D-5 docker+trivy | **FAIL** | docker build fails: `E0063 missing field trailers H3Request` (cargo-chef + dirty tree); no image ⇒ trivy N/A | no .trivyignore on base; dirty-tree breaks cargo-chef layering |
| D-1 native XDP | **BLOCKED** | ENA+CONFIG_XDP_SOCKETS+CONFIG_BPF_SYSCALL all present; test compiles but is a STUB on round-4 | real attach scaffold lands w/ EBPF-2-05 (absent); needs loadable ELF (D-2) |
| D-4 h2spec/h3spec | **PASS / DEFERRED** | h2spec 2.6.0 & h3spec 0.1.13 binaries OK (exit 0); conformance needs running listener | conformance run deferred (no live lb listener) |

### Headline
- Static-quality gates (deny / fmt / clippy / build) **all PASS**.
- All eBPF/data-plane prod gates (D-1, D-2) and packaging/coverage gates
  (D-5, D-6) **FAIL/BLOCKED** — consistent with the round-4 base deliberately
  lacking the c2bbfea6 remediation commit, **plus** an environmental small-disk
  constraint and an unexpected dirty working tree (concurrent WIP) that further
  degrades D-5/D-6.
- D-4 tooling present; real conformance deferred.
