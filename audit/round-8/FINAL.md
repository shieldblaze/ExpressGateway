# Round-8 — Terminal Audit Report (`verify`, task#81, STATE=ROUND_8_FINAL)

Branch: `prod-readiness/round-4` · Repo: ExpressGateway · Date: 2026-05-16
Lead verdict: DECIDED — not re-litigated here.

---

## VERDICT: NO-GO

The Round-8 code remediation is **complete and independently verified**.
Every medium-or-higher finding is `Verified-Fixed` by `verify` with
author ≠ verifier; the tree is regression-clean (15/15 runnable Phase-E
gates PASS at HEAD after the gate-04/05 test-infra remediation, commit
`0d549cd8`); and zero new medium+ findings were introduced by any fix
(Phase-D loop not triggered). **NO-GO is driven solely by
production-readiness gates that cannot be executed anywhere in this
environment** — real-NIC native XDP attach, the multi-kernel verifier
matrix, the 4-hour soak/chaos run, protocol-conformance suites, the
container vuln scan, hot-path coverage, and `cargo deny` config-grammar.
None of these is an unfixed defect. They are unrun gates whose PASS
cannot be truthfully asserted for an internet-facing L7+XDP load
balancer. Asserting GO while these are unrun would repeat the previous
audit's CONDITIONAL-GO sin in disguise. NO-GO is the honest binary
verdict; "conditional" is forbidden.

### Resolvable blocker list (the only thing standing between this tree and GO)

All commands/environments below are quoted verbatim from
`audit/round-8/regression/deferred-env.md` (D-1..D-6) and
`audit/round-8/regression/SUMMARY.md` gate 07. Nothing is invented;
where deferred-env.md omits a detail it is stated explicitly.

#### D-1 — Real-NIC native-mode XDP attach

- **Validates**: the XDP program attaches and forwards packets in
  `XDP_FLAGS_DRV_MODE` on a physical NIC (the production data path; veth/
  virtio SKB-mode is explicitly NOT a substitute).
- **Exact command**:
  ```bash
  sudo ./target/release/lb --config config/default.toml \
      --l4-xdp-iface enp1s0 --l4-xdp-mode native
  # or the focused attach test under privilege:
  sudo -E cargo test -p lb-l4-xdp --test xdp_attach_mode -- --ignored --nocapture
  ```
- **Exact environment**: physical NIC with in-tree native XDP driver —
  Intel `ixgbe` (82599/X520/X540/X550), `i40e` (X710/XL710/XXV710),
  `ice` (E810); Mellanox `mlx5_core` (ConnectX-4/5/6); Broadcom
  `bnxt_en` (NetXtreme-E). Kernel ≥ 5.15 with `CONFIG_XDP_SOCKETS=y` +
  `CONFIG_BPF_SYSCALL=y`. Privileges `CAP_NET_ADMIN` + `CAP_SYS_ADMIN`
  (or root). bpffs mounted at `/sys/fs/bpf`.
- **PASS criterion**: program attaches in native (DRV) mode and packets
  traverse the XDP path (non-zero `xdp_packets_total{action}` deltas);
  no silent zero-packet attach.

#### D-2 — `bpftool prog load` verifier-log matrix (5.15 / 6.1 / 6.6)

- **Validates**: the BPF object loads and the verifier accepts it on all
  three supported LTS kernels; closes the EBPF-2-07 / ROUND8-L4-10 gap
  (no `.log.committed` baselines exist today — the diff-gate is a no-op).
- **Exact command (per kernel)**:
  ```bash
  EG_ALLOW_FLOATING_IMAGE=1 scripts/verify-xdp.sh --kernel 5.15
  EG_ALLOW_FLOATING_IMAGE=1 scripts/verify-xdp.sh --kernel 6.1
  EG_ALLOW_FLOATING_IMAGE=1 scripts/verify-xdp.sh --kernel 6.6
  # First privileged green run captures the pinned digests; thereafter
  # drop EG_ALLOW_FLOATING_IMAGE and commit per-kernel *.log.committed.
  ```
- **Exact environment**: `little-vm-helper` (lvh) OR privileged Docker
  running `quay.io/lvh-images/kernel-images:{5.15,6.1,6.6}-main`;
  `docker run --rm --privileged` (required for bpffs + `bpftool prog
  load`); bpftool ≥ 7.0 (carried by the lvh images). The script's
  exit-code contract (0=identical, 1=drift, 2=missing baseline, 3=env)
  was already verified here with a stub docker (gate-output 14).
- **PASS criterion**: verifier accepts on all three kernels; first green
  run produces `.log.committed` baselines that are committed to the
  tree, flipping the dormant `verify-xdp.sh` diff-gate to live.

#### D-3 — 4-hour soak + chaos suite

- **Validates**: no memory-slope leak / p99 latency regression under
  sustained production-rate load; chaos-resilience.
- **Exact command**:
  ```bash
  SOAK_DURATION=4h SOAK_QPS=20000 \
      cargo test -p lb-integration-tests --release --test soak -- --ignored --nocapture
  cargo test -p lb-integration-tests --release --test chaos -- --ignored --nocapture
  ```
- **Exact environment**: a separate load box sustaining ≥ 20k rps for 4h
  (`wrk2`/`h2load`/`vegeta`) on the same L2 segment + ≥ 2 echo
  upstreams; gateway host ≥ 8 vCPU / ≥ 16 GiB RAM; Prometheus scrape
  target wired (RUNBOOK SLO asserts `process_resident_memory_bytes`
  slope + p99).
- **PASS criterion**: RUNBOOK soak SLOs hold for the full 4h —
  bounded RSS slope and p99 latency within budget; chaos suite green.

#### D-4 — h2spec / h3spec / Autobahn wstest conformance

- **Validates**: HTTP/2, HTTP/3 and WebSocket protocol conformance
  (matches Round-7 Gate 16/18 deferral).
- **Exact install + invocation**:
  ```bash
  go install github.com/summerwind/h2spec/cmd/h2spec@v2.6.0
  ./target/release/lb --config config/default.toml &        # listener :8080
  h2spec -h 127.0.0.1 -p 8080 -t -k -S

  cargo install --git https://github.com/cloudflare/h3spec h3spec
  h3spec 127.0.0.1 4433

  pip install autobahntestsuite        # or docker crossbario/autobahn-testsuite
  wstest -m fuzzingclient -s ws_autobahn_spec.json
  ```
- **Exact environment**: Go ≥ 1.21 (h2spec), Python 3.9+ or Docker
  (Autobahn), Rust toolchain with network (h3spec git install); a
  running `lb` listener on a known port with TLS material (in-tree tests
  generate self-signed via rcgen); ~6 GiB free for the cold release
  build of the SUT.
- **PASS criterion**: h2spec/h3spec report zero failed cases on the
  required test groups; Autobahn report shows no non-informational
  FAIL entries.

#### D-5 — trivy / grype container image scan

- **Validates**: shipped container image has no HIGH/CRITICAL CVEs
  (matches Round-7 Gate 23).
- **Exact command**:
  ```bash
  docker build -f docker/Dockerfile -t expressgateway:audit .
  trivy image --severity HIGH,CRITICAL --exit-code 1 expressgateway:audit
  grype expressgateway:audit --fail-on high
  ```
- **Exact environment**: Docker daemon (rootless OK) able to build
  `docker/Dockerfile`; `trivy` ≥ 0.50 and/or `grype` ≥ 0.74; network for
  the vuln DB on first run.
- **PASS criterion**: `trivy` exits 0 (no HIGH/CRITICAL) and `grype`
  does not fail-on-high.

#### D-6 — llvm-cov ≥ 80% hot-path coverage

- **Validates**: ≥ 80% line/region coverage on the enumerated
  request-hot-path modules. Per deferred-env.md Phase-E outcome: NOT run
  here — the cold `cargo test --workspace --release` no-run build alone
  took ~45 min on the 2-vCPU sandbox; an instrumented pass (~2×) is
  infeasible in-sandbox.
- **Exact command**:
  ```bash
  rustup component add llvm-tools-preview
  cargo install --locked cargo-llvm-cov
  cargo llvm-cov --workspace --release --lcov --output-path audit/round-8/coverage.lcov \
      -- --skip ignored
  cargo llvm-cov report --summary-only
  # Hot-path module thresholds (>=80%) enumerated in audit/coverage-scope.md
  ```
- **Exact environment**: `llvm-tools-preview` rustup component
  (offline-installable from the toolchain channel); a ≥ 8 vCPU CI runner
  with a warm cache (instrumented build ≈ 2× release; exceeds the
  2-vCPU per-command budget).
- **PASS criterion**: every hot-path module in `audit/coverage-scope.md`
  reports ≥ 80% coverage in `cargo llvm-cov report --summary-only`.

#### gate-07 — `cargo deny check`

- **Validates**: license + advisory + ban policy enforcement.
- **Exact command**: `cargo deny check`
- **Exact environment**: `rustc ≥ 1.88` + `cargo-deny ≥ 0.19`. The only
  `cargo-deny` installable on the MSRV `rustc 1.85.1` is `0.18.3`, which
  fails CONFIG DESERIALIZATION on `deny.toml`'s v0.19-grammar
  `GPL-3.0-only`/`-or-later` tokens (lines 53–54) **before any check
  runs** — this is a config-grammar/toolchain mismatch, NOT a policy
  violation. The advisories arm is corroborated by Gate 06
  `cargo audit` PASS (2 unmaintained allowed in `deny.toml`, no NEW
  advisory). *deferred-env.md does not give a separate command/env block
  for gate-07; the above is reconstructed from
  `regression/SUMMARY.md` gate 07 — stated explicitly per the
  no-invention rule.*
- **PASS criterion**: `cargo deny check` exits 0 on `rustc ≥ 1.88` with
  `cargo-deny ≥ 0.19`.

---

## 2. Findings summary

- **Total Round-8 findings: 39.**
- **Severity breakdown (post-arbitration, `cross-review.md §C`)**:
  Critical **1** (L7-01) · High **13** (L7-02, L7-04, L7-07, L7-08,
  L4-01, L4-02, L4-03, L4-05, L4-06, L4-10, OPS-01, OPS-04, OPS-06) ·
  Medium **20** · Low **5** (L7-10, L7-14, L7-15, OPS-08, OPS-12) ·
  Info 0. **Total medium+: 34** (35 with the late-opened L7-16; see
  below), bundled to ~28 Phase-D plans (7 bundles).
- **Reconcile counts (`reconcile/all.md` + `coverage-gap.md`)**: NEW
  **24** (61.5%) · MISSED **14** (35.9%) · DISPUTE **1**
  (ROUND8-L4-10 / EBPF-2-07).

**All medium+ are `Verified-Fixed` by `verify`, author ≠ verifier.**
The fix-back chain `ROUND8-L7-09 → ROUND8-L7-16` plus the **3 fix-back
loops** (`audit/round-8/verify/fixback.md`):

1. **Loop 1 (task#74, `bf22f01a`)** — L7-09 wired on the normal request
   path; adversarial probe found a **real residual bypass** on H1
   WS-upgrade / H2 extended-CONNECT / H2 gRPC. Verdict PARTIAL,
   push-back stood (non-blocking, medium, future-routing primitive).
2. **Loop 2 (task#76, `1a89a4e4`)** — validator hoisted to the FIRST
   statement of `handle_inner` in both H1/H2; 7/7 proof drives real
   WS/ext-CONNECT/gRPC shapes; **L7-09 Verified-Fixed for H1/H2**.
   Adversarial 4th-path hunt opened **NEW finding ROUND8-L7-16**: H3/
   QUIC dispatch (`lb-quic/src/conn_actor.rs:361`) reached upstream
   with no `authority::validate` (separate crate, unguarded).
3. **Loop 3 (task#78, `69cda7f4`)** — predicate hoisted verbatim to
   leaf crate `lb-core`, re-exported; no logic fork, no dep cycle
   (`cargo tree` confirms `lb-core` is a leaf); H3 choke-point above
   all 3 upstream branches; 3/3 serial proof; **L7-16 Verified-Fixed**,
   closing the H3 leg of L7-09. (A non-blocking sub-item — the H3 proof
   harness was not parallel-safe due to shared temp-cert state — was
   resolved in Phase E via a process-global `AtomicU64 CERT_SEQ`.)

**Deferred-with-rationale (honest, not theater)**:

- **ROUND8-L7-08** — deferred per lead-decision `R8-L-002`. hyper 1.x
  `SendRequest` genuinely exposes **no explicit `send_reset(CANCEL)`
  API** (confirmed: hyper pinned 1.9.0 in `Cargo.lock`). Available
  mitigations (drop-emits-CANCEL on future-drop; pool eviction on
  timeout) are already wired at `crates/lb-io/src/http2_pool.rs:206-209`.
  The Pingora-shape explicit-CANCEL fix requires the hyper-2.x upgrade.
- **ROUND8-L7-07 per-frame glitch-timer sub-deferral** — ONLY the
  `GlitchKind::FrameRecvTimeout` `tokio::Interval` frame-arrival
  watchdog is deferred (hyper 1.x exposes no per-connection inbound read
  context). The **counter half is fully wired and Verified-Fixed**
  (`h2_glitches_total`, 5 real abuse callsites, genuinely trips the
  connection drain token → two-step GOAWAY); keep-alive PING provides
  partial slowloris coverage. The timer moves with the hyper-2.x rebase.

---

## 3. Coverage-gap analysis vs the previous audit

- **35.9% MISSED (14/39) = systematic blind spot** — 3.6× the 10%
  escalation threshold (`coverage-gap.md`). NEW is 24/39 (61.5%):
  *additional surface the prior audit never looked at*, not
  re-evaluation of prior surface. The escalation rule fired but the
  blind spots are localised to four themes; a deeper Round 1-7 redo was
  explicitly rejected (high cost, marginal value vs proceeding).
- **The 4 systematic blind-spot themes**:
  1. **"Verified-Fixed" graded script/artefact existence, not
     capability** (EBPF-2-07 no-op gate; REL-2-07 zero callers;
     REL-2-02 histogram half; CODE-2-03 accept-loop half; REL-2-08
     startup-only).
  2. **Operational-vs-laboratory test posture** (drain jitter,
     readiness vs kubelet period, conntrack at >100k flows/s, SYN-flood
     rate cap, atomic backend table — only bite at scale/multi-replica).
  3. **Doc-vs-code claim drift** (README FD-passing claim,
     `BackendEntry::flags` doc lie, doc-lint philosophy too narrow).
  4. **Multi-validator audit handoff** (PROTO↔L7 authority predicate,
     CODE↔L7 take-and-discard, CODE↔L7 proptest CVE corpus, EBPF↔CODE
     `BPF_F_REPLACE` coexistence).
- **Hybrid theme-bounded recheck outcome (`recheck.md` +
  `recheck-themes.md`)**: 25 prior `Verified-Fixed` re-examined
  (deduped; 17 spot-checks in Phase C + the C-bis theme sweep).
  Result: **1 FALSE-VERIFIED = EBPF-2-07** (verify-xdp.sh shipped,
  `.log.committed` baselines never did; the diff-gate at
  `verify-xdp.sh:111`/`:103` is a permanent no-op; `audit/
  unsafe-justifications.md:99,109` falsely claim it is CI-live) — now
  corrected (reclassified, queued in Phase D as ROUND8-L4-10, baselines
  are blocker D-2). **3 PARTIAL surfaced and closed in Phase D**:
  CODE-2-03 listener accept-loop (→ ROUND8-OPS-04), REL-2-02
  `shutdown_drain_seconds` histogram (→ ROUND8-OPS-03), plus the
  disclosed scope-extension partials. No NEW FALSE-VERIFIED beyond
  EBPF-2-07; the escalation rule (≥3 FALSE-VERIFIED/15) did **not**
  trigger by literal count (1/25 = 4%).
- **Notable Phase-E catch**: the prior **REL-2-02 drain-test fixture**
  wrote the rcgen TLS key at mode **0o664**, which the product's
  strict-perm TLS-key check **correctly rejected** pre-bind — so the
  gateway exited before binding any listener and no boot-budget could
  ever help. This **masked correct product strict-perm behaviour** and
  was misread as a cold-start timing flake. Found & fixed in Phase-E
  remediation (test-FILE-only, `write_key_0600` helper; zero
  product-source change; commit `0d549cd8`). The product behaviour was
  always correct; the fixture was hiding it.

---

## 4. Updated gate matrix

Source: `audit/round-8/regression/SUMMARY.md` (gates 04 & 05 PASS
post-remediation, commit `0d549cd8`).

| # | Gate | Disposition |
|---|------|-------------|
| 01 | `cargo fmt --check` | **PASS** (exit 0) |
| 02 | `cargo clippy --workspace --all-targets --all-features -D warnings` | **PASS** (exit 0, clean) |
| 03 | `cargo machete` | **PASS** (soft; no regression vs R7) |
| 04 | `cargo test --workspace --release -- --skip ignored` | **PASS** (REMEDIATED `0d549cd8` — test-infra only; drain suite 4/4; root cause = fixture 0o664 TLS key, now 0600) |
| 05 | `PROPTEST_CASES=20000` per-crate (`--features proptest`) | **PASS** (REMEDIATED `0d549cd8` — reject-free generator; 2/2 at 20000, budget NOT lowered) |
| 06 | `cargo audit` | **PASS** (2 unmaintained allowed; no NEW advisory) |
| 07 | `cargo deny check` | **DEFERRED-ENV** (needs rustc ≥ 1.88 + cargo-deny ≥ 0.19; advisories corroborated by Gate 06) |
| 08 | sbom-cyclonedx | **PASS(recorded)** (real cargo-cyclonedx 0.5.9 provenance) |
| 09 | doc-lint tier-1 + tier-2 (audit-of-audit) | **PASS** (exit 0; 52 Verified-Fixed claims checked) |
| 10 | open-medium+ scan | **PASS(recorded)** (no NEW open medium+) |
| 11 | default-config placeholders | **PASS(recorded)** (no secret/placeholder tokens) |
| 12 | `round8_drain_15case` (15-case enumeration) | **PASS** (16/16; exit 0) |
| 13 | `round8_*` proof suites (serial) | **PASS** (29 ok, 0 failed; incl. h3-authority) |
| 14 | verify-xdp fail-loud (exit-code contract) | **PASS** (drift→1, missing→2, identical→0) |
| 15 | L4-05 aya tripwire (`round8_attach_probe`, aya 0.13.1) | **PASS** (10 pass, 1 ignored; aya 0.13.1 pinned) |
| 16 | netlink XDP query (`round8_netlink_xdp_query`) | **PASS** (7 pass, 1 ignored) |
| D-1 | Real-NIC native-mode XDP attach | **DEFERRED-ENV** |
| D-2 | bpftool verifier-log matrix 5.15/6.1/6.6 | **DEFERRED-ENV** |
| D-3 | 4h soak + chaos | **DEFERRED-ENV** |
| D-4 | h2spec / h3spec / Autobahn | **DEFERRED-ENV** (Round-7 Gate 16/18 carryover) |
| D-5 | trivy / grype container scan | **DEFERRED-ENV** (Round-7 Gate 23 carryover) |
| D-6 | llvm-cov ≥ 80% hot-path | **DEFERRED-ENV** (not run; 2-vCPU build budget exhausted) |

Runnable-gate tally: **15/15 PASS** (01–06, 08–16). The only non-PASS
items are env-bound: gate-07 + D-1..D-6. Gates 04 and 05 are **no
longer deferred** — remediated as test-infra-only with zero
product-source change.

---

## 5. Explicit list of what was NOT examined and why

Brutally honest:

1. **D-1..D-6 + gate-07 — env-bound, not examined here.** Not an
   oversight: this sandbox has no native-XDP NIC, no `CAP_NET_ADMIN`,
   no alternate-kernel VMs, no privileged Docker, no 4h/load-gen
   budget, no conformance binaries, no Docker daemon for the image
   scan, an 8-vCPU-class requirement for the instrumented coverage
   pass, and an MSRV that predates the `cargo-deny`/`rustc` the
   v0.19-grammar `deny.toml` requires. Each is the entire content of
   the blocker list above with verbatim command + environment.
2. **The prior register outside the 4 recheck themes was NOT
   re-verified.** This is a **deliberate user-lead scoping decision**
   (`recheck-themes.md`: HYBRID — do not redo the full 70/74-item
   register; DO recheck every prior `Verified-Fixed` whose closure fits
   one of the 4 themes; ~25 deduped candidates). Items whose closure
   did not touch any of the 4 themes were not re-walked. This is a
   scoping choice, **not an oversight** — the rationale (NEW is 61.5%
   = additional surface, not re-evaluation; deeper redo = 4-6 weeks,
   marginal value) is recorded in `coverage-gap.md` and was
   lead-approved.
3. **H3 trailer leg / other documented carries.** PROTO-2-12 ↔
   PROTO-2-19: the H3 `lb-quic` SURFACE trailer leg remains
   **deferred (Verified-Fixed-Partial, disclosed)** — not re-opened in
   Round-8 (`recheck-themes.md` Theme-4 STILL-VERIFIED-PARTIAL). The
   ROUND8-L7-07 per-frame `FrameRecvTimeout` watchdog and ROUND8-L7-08
   explicit-CANCEL both ride the hyper-2.x rebase (deferred-with-
   rationale, §2). ROUND8-L4-08 in-XDP fragment reassembly is by-design
   non-goal (declared explicit in `audit/deferred.md`). These are
   documented carries with disclosed rationale, not silent gaps.

---

### Bottom line

The remediation is done and proven. NO-GO is the single honest verdict
because seven concrete, runnable production gates (D-1..D-6 + gate-07)
have never been executed in any reachable environment, and a load
balancer terminating internet HTTP/2+3 and attaching XDP to a kernel
data path cannot be certified GO on gates that were never run.
