# Round 8 — div-l4 Phase D status (task #60)

Five L4 findings landed cleanly on `prod-readiness/round-4` this
session. Seven L4 findings remain Open, deferred to a follow-up
session with explicit rationale below.

## Landed

| Plan | SHA | Proof test |
|------|-----|------------|
| ROUND8-L4-10 | `ab3e37a` | `round8_verify_xdp_gate.rs` (5 passes) |
| ROUND8-L4-06 | `536aa38` | `round8_acl_admission.rs` (6 passes) |
| ROUND8-L4-09 | `585c74f` | `round8_ptr_at_bounds.rs` (7 passes) |
| ROUND8-L4-01 | `7f6c9a5` | `round8_backend_sentinel.rs` (7 passes) |
| ROUND8-L4-08 | `29f40f8` | `round8_fragments.rs` (6 passes) |

Total: 31 passing round-8 L4 proof tests, 0 failed.

Cross-bundle peer commit `61e678a` (L4-10 + OPS-09 doc-lint gate
+ canonical systemd unit) authored by div-ops landed concurrently
and complements L4-10.

## Deferred — clean commit boundaries respected

The following plans were not landed in this session. Reasons:

- **ROUND8-L4-02** (TCP-state pruning): requires extending TcpHdr
  in the BPF crate to expose `flags` at offset 13 plus
  `LruHashMap::remove(&key)` API verification. The aya-ebpf 0.1.1
  remove signature needs sandbox-bpf-linker to compile-check.
- **ROUND8-L4-03** (SYN-flood new-flow-rate cap): depends on
  L4-02 landing first (shared TCP-flags parsing) and a new
  `PerCpuArray` map for the rate window. Verifier-tractable but
  not in the time budget.
- **ROUND8-L4-04** (atomic per-VIP backend table swap): largest
  plan in the bundle; introduces a new ~5KB BPF struct
  (`BackendTable`), new map, and a publish API. Cross-references
  L4-01 (sentinel) and L4-07 (flags-field decision); sequence
  matters.
- **ROUND8-L4-05** (BPF_PROG_TEST_RUN probe + mlx5 blocklist):
  needs aya 0.13.x `ProgramTestRun` API exploration plus a NEW
  `nic_compat.rs` module. Disk-space-constrained sandbox could
  not afford the additional ~50 MB rebuild needed to land cleanly.
- **ROUND8-L4-07** (`BackendEntry::flags` doc-vs-code): Option A
  (drop the field) breaks `tests/pod_padding.rs` byte-layout
  assertions and the wire format. Lead-approved but the cascade
  needs careful sequencing with L4-04. Deferred so it can land
  with L4-04 in the same series.
- **ROUND8-L4-11** (bpffs statfs check): requires adding `nix`
  to `lb-l4-xdp` deps (already in workspace `Cargo.lock` for
  other crates but not yet declared here). Bundle peer with
  OPS-07 (systemd hardening) — OPS-07 also not landed yet by
  div-ops in this session.
- **ROUND8-L4-12** (query_xdp / attach_replacing /
  detach_verifying): bundle peer with OPS-04 (drain
  coordinator). The drain coordinator is div-ops Wave-2 work;
  L4-12 should land paired with OPS-04 so the signatures match
  what the coordinator expects to call.

All deferred plans remain `Open` in their respective finding
files. Re-take when the dependency / bundle-peer / sandbox
constraints clear.

## Scope adjustments made vs the plans

- **L4-10 verifier-log baselines**: shipped as
  `HARNESS-CAPTURED-PENDING-CI-RERUN` placeholders rather than
  real logs (sandbox lacked `bpf-linker`). The script's exit
  semantics enforce a refresh on first CI matrix run — the
  expected/intended posture. Documented in
  `audit/ebpf/verifier-logs/README.md`.
- **L4-01 BackendEntry::new signature**: kept infallible `new`
  for back-compat (pod_padding tests deliberately construct
  zero-shape entries); added fallible `try_new` alongside. The
  eBPF runtime sentinel guard is the load-bearing defence — the
  userspace constructor is the upstream admission gate for new
  callers.
- **L4-09 verifier-log refresh**: deferred to first CI matrix
  run (the BPF source change cannot rebuild here). The
  baselines already carry the pending marker, so CI will hard-
  fail until refreshed — correct posture.

## Audit-register updates landed

- `audit/ebpf/round-2-review.md`: EBPF-2-07 status downgraded
  `Verified-Fixed(ffde98c)` → `Verified-Fixed-Partial(ffde98c,
  ROUND8-L4-10)` pending CI baseline refresh.
- `audit/unsafe-justifications.md:109`: claim corrected to
  reflect that the diff-gate exists and hard-fails, with
  baselines in placeholder state pending CI.
- `audit/deferred.md`: new section "ROUND8-L4-08 — Fragmented
  datagrams: pass-to-kernel, no in-XDP reassembly" formally
  declares the design.
