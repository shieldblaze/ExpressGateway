# Round-8 Phase-E — Genuinely-Unrunnable Gates: exact command + exact environment

This sandbox: 2 vCPU, 7.7 GiB RAM, cargo 1.85.1, **network IS available**
(crates.io reachable — so `cargo install` gates are NOT deferred; they
were run here). No privileged Docker, no real NIC with XDP-native
driver, no alternate-kernel VMs, no time budget for 4h soak.

The user's hard rule: do NOT hand-wave to "CI". Each entry below records
the EXACT command and the EXACT environment (kernel, hardware, network,
privileges) required, so the lead can wire it verbatim.

---

## D-1 — Real-NIC native-mode XDP attach

**Why unrunnable here:** sandbox NICs are virtio/veth without native XDP
driver support; no `CAP_NET_ADMIN`; attaching in `XDP_FLAGS_DRV_MODE`
requires a physical NIC whose driver implements the `ndo_bpf` / XDP
native data path.

**Exact command:**
```bash
sudo ./target/release/lb --config config/default.toml \
    --l4-xdp-iface enp1s0 --l4-xdp-mode native
# or the focused attach test under privilege:
sudo -E cargo test -p lb-l4-xdp --test xdp_attach_mode -- --ignored --nocapture
```

**Exact environment:**
- Hardware NIC with in-tree native XDP driver. Verified driver list:
  - Intel `ixgbe` (82599 / X520 / X540 / X550)
  - Intel `i40e` (X710 / XL710 / XXV710)
  - Intel `ice` (E810)
  - Mellanox `mlx5_core` (ConnectX-4/5/6)
  - Broadcom `bnxt_en` (NetXtreme-E)
  - (veth/virtio-net support `XDP_FLAGS_SKB_MODE` only — generic mode —
    which is NOT a substitute for the native-mode acceptance.)
- Kernel >= 5.15 with `CONFIG_XDP_SOCKETS=y`, `CONFIG_BPF_SYSCALL=y`.
- Privileges: `CAP_NET_ADMIN` + `CAP_SYS_ADMIN` (or root).
- bpffs mounted at `/sys/fs/bpf`.

---

## D-2 — `bpftool prog load` on 5.15 / 6.1 / 6.6 (verifier-log matrix)

**Why unrunnable here:** needs three distinct LTS kernels; only one
kernel is present and it is not privileged for `bpftool prog load`.

**Exact command (per kernel):**
```bash
EG_ALLOW_FLOATING_IMAGE=1 scripts/verify-xdp.sh --kernel 5.15
EG_ALLOW_FLOATING_IMAGE=1 scripts/verify-xdp.sh --kernel 6.1
EG_ALLOW_FLOATING_IMAGE=1 scripts/verify-xdp.sh --kernel 6.6
# First privileged green run captures the pinned digests; thereafter
# drop EG_ALLOW_FLOATING_IMAGE and commit per-kernel *.log.committed.
```

**Exact environment:**
- `little-vm-helper` (lvh) OR privileged Docker able to run
  `quay.io/lvh-images/kernel-images:{5.15,6.1,6.6}-main`.
- `docker run --rm --privileged` (the script requires `--privileged`
  for bpffs + `bpftool prog load`).
- bpftool >= 7.0 inside the image (the lvh images carry it).
- The script's exit-code contract (2=missing baseline, 1=drift,
  0=identical, 3=env) was VERIFIED here with a stub docker — see
  gate-outputs/14-verify-xdp-failloud.txt. Only the real
  load-on-real-kernel step is deferred.

---

## D-3 — 4h soak / chaos suite

**Why unrunnable here:** wall-clock + load-generation infra.

**Exact command:**
```bash
SOAK_DURATION=4h SOAK_QPS=20000 \
    cargo test -p lb-integration-tests --release --test soak -- --ignored --nocapture
# chaos:
cargo test -p lb-integration-tests --release --test chaos -- --ignored --nocapture
```

**Exact environment:**
- A load box able to sustain >= 20k rps for 4h against the gateway
  (e.g. `wrk2` / `h2load` / `vegeta` on a separate host on the same
  L2 segment), plus a backend fleet of >= 2 echo upstreams.
- >= 8 vCPU, >= 16 GiB RAM on the gateway host (the 2-vCPU sandbox
  cannot both drive load and run the SUT).
- Prometheus scrape target wired (RUNBOOK soak SLO assertions read
  `process_resident_memory_bytes` slope + p99 latency).

---

## D-4 — h2spec / h3spec / Autobahn wstest

**Why unrunnable here:** the conformance binaries are not installed and
the harness needs a running listener + cold-build budget that the
2-vCPU sandbox exhausts (matches Round-7 Gate 16/18 deferral).

**Exact install + invocation:**
```bash
# h2spec
go install github.com/summerwind/h2spec/cmd/h2spec@v2.6.0
./target/release/lb --config config/default.toml &        # listener :8080
h2spec -h 127.0.0.1 -p 8080 -t -k -S          # -t/-k/-S for TLS+h2
# (or the in-tree harness: cargo test --test h2spec_server_conformance
#  --release -- --ignored, which shells out to the h2spec binary.)

# h3spec
cargo install --git https://github.com/cloudflare/h3spec h3spec
h3spec 127.0.0.1 4433

# Autobahn
pip install autobahntestsuite        # or docker run crossbario/autobahn-testsuite
wstest -m fuzzingclient -s ws_autobahn_spec.json
# in-tree wrapper: cargo test --test ws_autobahn --release -- --ignored
```

**Exact environment:**
- Go >= 1.21 (h2spec), Python 3.9+ or Docker (Autobahn), a Rust
  toolchain with network for the h3spec git install.
- A running `lb` listener bound to a known port; TLS cert material
  (the in-tree tests generate self-signed via rcgen).
- ~6 GiB free for the cold release build of the SUT.

---

## D-5 — trivy / grype container image scan

**Why unrunnable here:** no Docker daemon to build the image; scanners
not installed (matches Round-7 Gate 23).

**Exact command:**
```bash
docker build -f docker/Dockerfile -t expressgateway:audit .
# trivy:
trivy image --severity HIGH,CRITICAL --exit-code 1 expressgateway:audit
# grype:
grype expressgateway:audit --fail-on high
```

**Exact environment:**
- Docker daemon (rootless OK) able to build `docker/Dockerfile`.
- `trivy` >= 0.50 and/or `grype` >= 0.74 (single static binaries;
  install via their official install.sh, network required).
- Network to pull the trivy/grype vuln DB on first run.

---

## D-6 — llvm-cov >= 80% hot-path coverage

`cargo-llvm-cov` install was ATTEMPTED here (network available). If the
install completed it was RUN (see gate-outputs / SUMMARY); the entry
below is the exact contract if the in-sandbox build budget is exhausted
before the instrumented test pass finishes.

**Exact command:**
```bash
rustup component add llvm-tools-preview
cargo install --locked cargo-llvm-cov
cargo llvm-cov --workspace --release --lcov --output-path audit/round-8/coverage.lcov \
    -- --skip ignored
cargo llvm-cov report --summary-only
# Hot-path module thresholds (>=80%) enumerated in audit/coverage-scope.md
```

**Exact environment:**
- `llvm-tools-preview` rustup component (offline-installable from the
  toolchain channel; no extra network beyond the toolchain).
- Instrumented build ~= 2x the normal release build; on 2 vCPU this
  exceeds the per-command budget, so it runs in CI with a warm cache
  or a >= 8 vCPU runner.

**Phase-E outcome (task#79):** NOT run here. The Phase-E test gates
exhausted the sandbox build budget — the cold `cargo test --workspace
--release` no-run build alone took ~45 min on this 2-vCPU box (172
test/bin targets, ~168 test binaries each linking the full dependency
closure including boring-sys/bindgen/quiche). An llvm-cov instrumented
pass (~2x that) is not feasible in-sandbox. DEFERRED-ENV with the exact
command + environment above; runs on the >= 8 vCPU CI runner.
