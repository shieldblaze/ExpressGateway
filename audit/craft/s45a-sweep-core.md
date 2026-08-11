# S45A sweep — sweeper-core (`crates/lb`, `crates/lb-l4-xdp`, `crates/lb-soak`)

## Headline

| metric | before | after | cut |
|---|---:|---:|---:|
| comment lines (all three crates) | 6752 | 2245 | **66.8%** |
| `crates/lb` | 2373 | 588 | 75.2% |
| `crates/lb-l4-xdp` | 3162 | 1256 | 60.3% |
| `crates/lb-soak` | 1217 | 401 | 67.0% |

Measured with `grep -rhE '^\s*(//|/\*|\*)' crates/lb crates/lb-l4-xdp crates/lb-soak --include='*.rs' | wc -l`.

## `// SAFETY:` — the hard constraint

**39 before, 39 after. Equal.** Per file: ebpf/src/main.rs 24, loader.rs 5,
netlink_xdp.rs 5, gateway.rs 2, pod_padding.rs 2, bpffs.rs 1. None deleted, none
weakened; a few had surrounding prose trimmed but every safety condition is intact.

## Why this area lands at 67%, not 90%

Two structural floors, both named in my brief:

1. **`#![deny(missing_docs)]`** on `lb` and `lb-l4-xdp`. Roughly **285 doc lines are
   mandatory** — ~195 `pub` items/fields plus ~90 enum variants across
   `lb/src` + `lb-l4-xdp/src`. `loader.rs` alone carries 93 `pub` items; its
   remaining 362 lines are ~109 one-line mandatory docs plus 252 lines of catch.
2. **`lb-l4-xdp` is unusually catch-dense.** It is the only area with `unsafe`, a
   kernel verifier, and wire-format ABI mirrors. Every surviving multi-line block
   there is a safety condition, a verifier constraint, a kernel/RFC citation, or a
   map-layout invariant.

I re-scanned every remaining multi-line block at the end: all are catches. The
remaining reduction would have to come from deleting catches or breaking the doc
floor, so I stopped here and am reporting the real number.

The no-floor areas were cut much harder, as expected: integration tests reach
81–93% (`quic_passthrough_audit_throttle_saturation.rs` 88→6, `round8_attach_probe.rs`
81→10, `quic_passthrough_e2e.rs` 174→25).

## Catches preserved (compressed, never deleted)

**lb-l4-xdp / eBPF** — `ptr_at` `checked_add` rationale (aya #1562 / CVE-2022-23222
bounds-check elision); "verifier will not accept an unbounded loop" behind the
2-extension-header cap; the "changing BPF source obliges a verifier-log baseline
refresh" gate note; byte-size layout assertions with per-field arithmetic; RFC 1624
checksum formula; RFC 791 §3.1 / RFC 2460 §4.5 fragment guards and the
no-in-XDP-reassembly decision; Katran `is_under_flood()` and Cilium sliding-RST
prune lessons; the `const _` anchor block; zero-IP/zero-port sentinel (Katran lesson
10); EOPNOTSUPP/EINVAL-only ladder fall-through; Native/Hw deliberately skipping the
ladder ("loud failure rather than a silent 10–50x regression to SKB"); the netlink
wire-format diagram + panic-freedom guarantee; "the kernel never hands out prog_id
0"; ROUND8-L4-12 EBUSY-on-redeploy close and why detach-then-attach replaces
BPF_F_REPLACE; F-COR-7 ena driver+kernel fallback key; the aya 0.13.1
`BPF_PROG_TEST_RUN` API-blocker note; `NUM_SLOTS == 16` with `AttachProbeFailed`(16)
deliberately excluded; ENA native-attach preconditions (MTU 3498, half-channels) and
the iproute2 6.19 native-mode proof.

**lb** — the `rebuild_l7_proxies` HONESTY INVARIANT and its coupling to
`LbConfig::diff`; F-S26-1 H3-terminate backend dispatch; the S37-C signal-loss fix;
OPS-04+L4-12 `biased` cancel arm and the synchronous post-accept tail check
(C-2/C-3/C-15); ROUND8 OPS-02 per-connection drain jitter; F-RES-5
watchdog-is-observability-only; CF-S27-2 WS gating off by default; the R8
wired-tunnel backpressure non-vacuity argument (the client's quiche windows must
stay capped or the test is vacuous); the R13 reset-vs-EOF negative control; SEC-2-11
CAP_BPF→CAP_SYS_ADMIN fallback policy; the "both main.rs and lib.rs declare `mod
xdp`" note.

**lb-soak** — the F-S20-1 cwnd partial-write contract; the F-S20-2 leak
DISCRIMINANT (`fds`, and why `accept_inflight` is excluded as a low-baseline
sawtooth); F-S27-2 "this client READS NORMALLY"; CF-S19 teardown fixture choice;
CF-S15 `mint_retry = false`; RFC 6455 §5.6 binary-not-Text; the BOUNDED/DRIFT 1.8x
calibration and the sawtooth false-leak guard.

## Approved deletions, done

- `lb-l4-xdp/tests/elf_sections.rs` — the "sections absent until build-xdp.sh is
  re-run" note (false today).
- `lb-l4-xdp/tests/round8_conntrack_state.rs` — "NUM_SLOTS bumps from 13 to 15"
  changelog (also wrong; it is 16).
- `lb-soak/src/loadgen.rs` — the `u16::MAX` sentinel comment describing a sentinel
  that does not exist.

## Stale premises rewritten rather than kept or dropped

The three F-S26-1 blocks (`config_gen.rs`, `eg-soak.rs`, `loadgen.rs`) asserted that
the production binary wires no H3 backend. That premise is refuted by
`main.rs::wire_h3_terminate_backends`. I kept the still-true, load-bearing half —
these scenarios deliberately emit no backend, so both observable outcomes are
asserted and the probe is non-vacuous — and dropped the refuted rationale.

Same treatment for `xdp_link_id_drop_safe.rs` (the "`_link_id` is dropped inside
`attach`" scaffold prose, refuted by ROUND8-L4-12 retaining link ids) and the
`stats_export.rs` test header ("10 slots", stale line-range citation).

## Refused to cut

- **`quic_passthrough_e2e.rs:517-561`** — CF-S15-PASSTHROUGH-RETRY-ODCID. This block
  is internally contradictory (says both "closed by this commit" and "until
  resolved"), and claims the test is `#[ignore]`'d when it is not. I compressed it
  to the mechanism that is verifiably true (why plain `quiche::accept(odcid)` is
  insufficient, and that production needs an ODCID side channel) and kept the CF-ID,
  but **the contradiction needs an owner ruling** — I did not invent a resolution.
- Every `pub`-item doc line under the `missing_docs` floor, including ones that are
  pure restatement (`/// Source port (network byte order).`). Deleting them turns
  `clippy -D warnings` red.
- `TODO(L4-06)` in `loader.rs` — kept, and moved out of the middle of a rustdoc
  block (it was splitting `insert_acl_deny`'s doc in two) into the end of that block.
  Behaviour-neutral.

## `loader.rs` coverage carve-out

My brief asked me to keep the comment explaining the carve-out. **There is no such
comment in `loader.rs`** — the carve-out is documented in `scripts/ci/coverage-check.sh`
(header + the `EXEMPT` constant), which I did not touch. Nothing to preserve; the
gate's honesty contract is unaffected.

## Verification performed

- **No code touched.** `git diff ccdb5aa2 -- crates/lb crates/lb-l4-xdp crates/lb-soak`
  filtered to removed non-comment, non-blank lines yields only two rustfmt empty-block
  collapses (`{ }` → `{}`), each with its matching addition.
- **Doc floor.** A syntactic `missing_docs` checker over `lb/src` and `lb-l4-xdp/src`
  reports clean. The single flagged item (`cfg(not(target_os = "linux"))`
  `try_attach_xdp`) was already undocumented before this sweep and is compiled out on
  the CI target.
- `cargo fmt -p lb -p lb-l4-xdp -p lb-soak` run; no build/clippy/test run (per brief).

### One defect caught and fixed mid-sweep

An early range in `lb-soak/src/timeseries.rs` spanned two comment blocks with two
`let` bindings between them, deleting them. The tree was repaired before I committed,
and I then added a hard rail to the sweeper: a replacement range containing any
non-comment line is refused, and every range is snapped to its enclosing comment
block. It fired 6 more times afterwards (`main.rs` ×5, a test ×1), each time
correctly. The final base-diff check above is the authoritative confirmation that no
code was lost anywhere.

## Per-file table

| file | before | after | cut |
|---|---:|---:|---:|
| `crates/lb/src/main.rs` | 1539 | 452 | 71% |
| `crates/lb-l4-xdp/src/loader.rs` | 851 | 362 | 57% |
| `crates/lb-soak/src/loadgen.rs` | 431 | 169 | 61% |
| `crates/lb-l4-xdp/ebpf/src/main.rs` | 379 | 181 | 52% |
| `crates/lb/tests/quic_passthrough_e2e.rs` | 174 | 25 | 86% |
| `crates/lb-l4-xdp/src/nic_compat.rs` | 225 | 106 | 53% |
| `crates/lb-soak/src/bin/eg-soak.rs` | 168 | 65 | 61% |
| `crates/lb-l4-xdp/src/stats_export.rs` | 201 | 98 | 51% |
| `crates/lb-l4-xdp/tests/xdp_attach_mode.rs` | 176 | 85 | 52% |
| `crates/lb/tests/quic_passthrough_audit_throttle_saturation.rs` | 88 | 6 | 93% |
| `crates/lb-soak/src/config_gen.rs` | 119 | 38 | 68% |
| `crates/lb/tests/quic_passthrough_spoofed_source_e2e.rs` | 96 | 18 | 81% |
| `crates/lb/tests/quic_passthrough_bounded_state.rs` | 80 | 7 | 91% |
| `crates/lb-l4-xdp/tests/round8_attach_probe.rs` | 81 | 10 | 88% |
| `crates/lb-soak/src/timeseries.rs` | 88 | 18 | 80% |
| `crates/lb-l4-xdp/src/lib.rs` | 132 | 65 | 51% |
| `crates/lb/src/xdp.rs` | 112 | 49 | 56% |
| `crates/lb-soak/src/backends.rs` | 76 | 15 | 80% |
| `crates/lb-soak/src/chaos.rs` | 73 | 20 | 73% |
| `crates/lb-l4-xdp/tests/l4_xdp_conntrack.rs` | 68 | 17 | 75% |
| `crates/lb/tests/informational_pass_through_main.rs` | 52 | 2 | 96% |
| `crates/lb-l4-xdp/tests/round8_netlink_xdp_query.rs` | 55 | 5 | 91% |
| `crates/lb-l4-xdp/tests/round8_atomic_backends.rs` | 56 | 8 | 86% |
| `crates/lb/tests/panic_abort.rs` | 49 | 2 | 96% |
| `crates/lb/tests/quic_passthrough_metrics.rs` | 49 | 3 | 94% |
| `crates/lb-l4-xdp/tests/xdp_pin_paths.rs` | 47 | 4 | 91% |
| `crates/lb-l4-xdp/tests/round8_attach_replace.rs` | 44 | 2 | 95% |
| `crates/lb-l4-xdp/src/netlink_xdp.rs` | 115 | 75 | 35% |
| `crates/lb-soak/src/metrics.rs` | 43 | 5 | 88% |
| `crates/lb-soak/src/bench.rs` | 62 | 25 | 60% |
| `crates/lb-l4-xdp/tests/round8_synflood_cap.rs` | 42 | 5 | 88% |
| `crates/lb-l4-xdp/src/sim.rs` | 96 | 59 | 39% |
| `crates/lb-l4-xdp/src/bpffs.rs` | 59 | 22 | 63% |
| `crates/lb/tests/quic_passthrough_strict_source_binding.rs` | 38 | 4 | 89% |
| `crates/lb-soak/src/procstat.rs` | 36 | 3 | 92% |
| `crates/lb-l4-xdp/tests/round8_conntrack_state.rs` | 60 | 27 | 55% |
| `crates/lb-l4-xdp/tests/xdp_link_id_drop_safe.rs` | 44 | 12 | 73% |
| `crates/lb-l4-xdp/tests/round8_ena_kernel_blocklist.rs` | 34 | 2 | 94% |
| `crates/lb-l4-xdp/tests/round8_fragments.rs` | 34 | 3 | 91% |
| `crates/lb/tests/xdp_cap_probe.rs` | 38 | 9 | 76% |
| `crates/lb-l4-xdp/tests/round8_backend_flags.rs` | 30 | 1 | 97% |
| `crates/lb-l4-xdp/tests/round8_ptr_at_bounds.rs` | 29 | 2 | 93% |
| `crates/lb-l4-xdp/tests/round8_bpffs_check.rs` | 29 | 2 | 93% |
| `crates/lb-l4-xdp/tests/round8_acl_admission.rs` | 28 | 1 | 96% |
| `crates/lb/tests/quic_passthrough_cid_migration.rs` | 28 | 2 | 93% |
| `crates/lb-l4-xdp/tests/stats_export.rs` | 38 | 12 | 68% |
| `crates/lb-l4-xdp/tests/loader_license_assert.rs` | 27 | 3 | 89% |
| `crates/lb-soak/src/gateway.rs` | 33 | 10 | 70% |
| `crates/lb-soak/src/lib.rs` | 30 | 8 | 73% |
| `crates/lb-l4-xdp/tests/round8_verifier_baseline_70.rs` | 35 | 14 | 60% |
| `crates/lb/src/lib.rs` | 25 | 7 | 72% |
| `crates/lb-soak/src/sampler.rs` | 22 | 4 | 82% |
| `crates/lb-l4-xdp/tests/pod_padding.rs` | 43 | 27 | 37% |
| `crates/lb-soak/src/bin/eg-bench.rs` | 36 | 21 | 42% |
| `crates/lb-l4-xdp/tests/round8_verify_xdp_gate.rs` | 28 | 15 | 46% |
| `crates/lb-l4-xdp/tests/real_elf.rs` | 15 | 2 | 87% |
| `crates/lb-l4-xdp/tests/elf_sections.rs` | 21 | 9 | 57% |
| `crates/lb-l4-xdp/tests/round8_backend_sentinel.rs` | 21 | 10 | 52% |
| `crates/lb-l4-xdp/build.rs` | 19 | 10 | 47% |
| `crates/lb/build.rs` | 5 | 2 | 60% |
| **TOTAL** | **6752** | **2245** | **66.8%** |
