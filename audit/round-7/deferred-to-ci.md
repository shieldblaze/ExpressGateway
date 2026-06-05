# Round 7 — Gates Deferred to CI

The gates below cannot be executed in this audit sandbox (no internet
for `cargo install`, no privileged Docker, no real NICs, no nightly
toolchain). Each entry records the exact CI image + command the lead
should wire into the release pipeline.

## Gate 4 — `cargo geiger`

```yaml
image: rust:1.85-slim
steps:
  - cargo install cargo-geiger --locked
  - cargo geiger --workspace --output-format Json > audit/round-7/gate-outputs/geiger.json
```

Local fallback in this run: `grep -rn 'unsafe' crates/` saved to
`audit/round-7/gate-outputs/geiger-grep.txt`; per-site rationale in
`audit/unsafe-justifications.md`.

## Gate 7 — `cargo llvm-cov` (HTML coverage)

```yaml
image: rust:1.85-slim
steps:
  - rustup component add llvm-tools-preview
  - cargo install cargo-llvm-cov --locked
  - cargo llvm-cov --workspace --html --output-dir audit/round-7/coverage-html
  - cargo llvm-cov report --json --output-path audit/round-7/coverage.json
```

Module thresholds: see `audit/coverage-scope.md` (≥80% per hot-path
module).

## Gate 8 — `cargo miri test`

```yaml
image: rustlang/rust:nightly
steps:
  - rustup +nightly component add miri
  - cargo +nightly miri test --workspace
```

Skip targets: `lb-l4-xdp/ebpf` (no_std BPF target, Miri does not
support BPF), `tests/integ_*` requiring real syscalls.

## Gate 11 — `cargo audit`

```yaml
image: rust:1.85-slim
steps:
  - cargo install cargo-audit --locked
  - cargo audit -D warnings
```

## Gate 12 — `cargo deny check`

```yaml
image: rust:1.85-slim
steps:
  - cargo install cargo-deny --locked
  - cargo deny check
```

(`deny.toml` is already committed at the repo root.)

## Gate 13 — `cargo cyclonedx`

```yaml
image: rust:1.85-slim
steps:
  - cargo install cargo-cyclonedx --locked
  - cargo cyclonedx --format json -o audit/sbom.json
```

Local fallback: manual SBOM emitted from `cargo metadata` JSON now at
`audit/sbom.json` (399 components, CycloneDX 1.5 spec).

## Gate 14 — `bpftool` real-kernel attach

```yaml
image: ghcr.io/lvh-io/lvh:latest   # privileged
kernels: [5.15, 6.1, 6.6]
steps:
  - scripts/build-xdp.sh
  - bpftool prog load crates/lb-l4-xdp/src/lb_xdp.bin /sys/fs/bpf/lb_xdp
  - bpftool prog show pinned /sys/fs/bpf/lb_xdp
  - bpftool map show
  - bpftool prog dump xlated pinned /sys/fs/bpf/lb_xdp > audit/round-7/gate-outputs/bpftool-${KVER}.txt
  - bpftool net attach xdpgeneric pinned /sys/fs/bpf/lb_xdp dev <test-veth>
```

## Gate 15 — XDP verifier log matrix

The directory `crates/lb-l4-xdp/ebpf/verifier-logs/` is NOT present in
this branch (sandbox cannot build the eBPF binary, so logs were never
captured). `scripts/verify-xdp.sh` IS present and ready to run.

```yaml
image: ghcr.io/lvh-io/lvh:latest   # privileged + bpffs
steps:
  - for kver in 5.15 6.1 6.6; do
      scripts/verify-xdp.sh "$kver"
    done
  - git diff --exit-code audit/ebpf/verifier-logs/
```

## Gate 16 — `h2spec`

```yaml
image: ghcr.io/summerwind/h2spec:latest
prereq: gateway listening on 8080 (TLS)
steps:
  - h2spec http2 --port 8080 --tls -j audit/round-7/gate-outputs/h2spec.json
```

## Gate 18 — h3spec + Autobahn (WebSocket)

```yaml
image: crossbario/autobahn-testsuite:latest
steps:
  - wstest -m fuzzingclient -s autobahn-spec.json
```

```yaml
image: ghcr.io/quinn-rs/h3spec:latest
steps:
  - h3spec --port 8443
```

## Gate 19 — Criterion benches

```yaml
image: rust:1.85-slim
steps:
  - cargo bench --workspace
  - tar czf bench-results.tgz target/criterion
```

(Locally executed best-effort if `bench/criterion/` exists — see
`audit/round-7/gate-outputs/bench.txt`.)

## Gate 20 — Soak / chaos (4h)

```yaml
image: ghcr.io/expressgateway/soak-rig:latest
duration: 4h
steps:
  - ./soak-rig --duration 4h --rps 50000 --chaos-pct 5
  - ./soak-rig --verify-no-leak --max-rss-drift 5%
```

## Gate 23 — Container image scan

```yaml
image: aquasec/trivy:latest
steps:
  - trivy image --severity HIGH,CRITICAL --exit-code 1 ghcr.io/expressgateway/lb:latest
  - grype ghcr.io/expressgateway/lb:latest --fail-on high
```

## Gate 25 — Prometheus scrape in docker-compose

```yaml
image: docker:24-cli  # with compose plugin
steps:
  - docker compose -f docker/compose.yml up -d
  - sleep 20
  - curl -sf http://localhost:9090/api/v1/targets | jq '.data.activeTargets[] | select(.health=="up")'
  - docker compose -f docker/compose.yml down
```

---

## Summary table

| Gate | Why deferred                                       |
| ---- | -------------------------------------------------- |
| 4    | `cargo install` requires network                   |
| 7    | `cargo llvm-cov` install requires network          |
| 8    | needs nightly + miri component                     |
| 11   | `cargo install cargo-audit` requires network       |
| 12   | `cargo install cargo-deny` requires network        |
| 13   | `cargo install cargo-cyclonedx` requires network   |
| 14   | needs real kernel + privileged Docker              |
| 15   | needs lvh-built bpf binary + privileged Docker     |
| 16   | h2spec binary not installed                        |
| 18   | h3spec + Autobahn not installed                    |
| 20   | 4-hour soak — out of session budget                |
| 23   | trivy/grype not installed                          |
| 25   | docker-compose / docker daemon not available       |

---

## S34 — CI reconciliation additions

Two items the S34 CI reconciliation makes explicit (referenced by the CI gates):

### D-6 carve-out: `lb-l4-xdp/src/loader.rs` line coverage
The D-6 per-module gate (`scripts/ci/coverage-check.sh`) requires every charter
hot-path module ≥ 80% line coverage. `loader.rs` is carved out **by name**: it
performs the privileged XDP `bpf()` load / map-population syscalls, which a unit
harness cannot exercise without root (full-suite line coverage measures 50.7%).
The load path is instead **smoke-validated** by the `D2-xdp-verifier-smoke` job,
which loads the real committed object into the runner-kernel verifier. Closing
the remaining loader line-coverage needs a privileged/root coverage run
(self-hosted). Every OTHER hot-path module is enforced ≥ 80%, so a regression
elsewhere still turns D-6 red.

### F-ESC-1: full XDP verifier matrix (kernels 5.15 / 6.1 / 6.6)
`D2-xdp-verifier-smoke` validates the XDP object loads on the **runner's own**
6.x kernel only. The full 5.15 / 6.1 / 6.6 verifier matrix needs to boot a VM
per kernel (vmtest / QEMU-KVM); GitHub hosted runners provide **no nested
virtualization (`/dev/kvm`)**, so that matrix cannot run honestly on
`ubuntu-latest`. It is escalated to self-hosted / D-1-class hardware (see
`D1-soak-runbook.md`). This is named, not silently dropped.
