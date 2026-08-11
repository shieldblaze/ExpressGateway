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

---

# ROUND 2

## Headline

| metric | baseline (`main`) | after round 1 | after round 2 | cut vs baseline |
|---|---:|---:|---:|---:|
| comment lines (all three crates) | 6752 | 2245 | **1099** | **83.7%** |
| `crates/lb` | 2373 | 588 | 193 | 91.9% |
| `crates/lb-l4-xdp` | 3162 | 1256 | 736 | 76.7% |
| `crates/lb-soak` | 1217 | 401 | 170 | 86.0% |

Round 2 alone: 2245 → 1099 (**51.0%** of what round 1 left). Target was "under 1500 if
the rule allows" — landed at 1099.

Measured with `grep -rhE '^\s*(//|/\*|\*)' crates/lb crates/lb-l4-xdp crates/lb-soak --include='*.rs' | wc -l`.
(That regex also matches a handful of code lines starting with a deref `*`, e.g.
`*applied_config = new_config;` — inherited from the round-1 metric, kept for comparability.)

## `// SAFETY:` — 39, unchanged, same distribution

```
     24 crates/lb-l4-xdp/ebpf/src/main.rs
      5 crates/lb-l4-xdp/src/loader.rs
      5 crates/lb-l4-xdp/src/netlink_xdp.rs
      2 crates/lb-l4-xdp/tests/pod_padding.rs
      2 crates/lb-soak/src/gateway.rs
      1 crates/lb-l4-xdp/src/bpffs.rs
```

**I did not touch a single SAFETY block in round 2.** Not one line, not even to merge a
2-line block into 1. Several are 2–3 lines and could have been compressed for ~10 more
lines of reduction; the trade was not worth any risk to the constraint, so they are
byte-identical to round 1.

## Code identity — proof

```
S45A code-identity proof — 239 .rs files changed vs main
  2 file(s) differ: TOKENS DIFFER = real code change; REFLOW ONLY = rustfmt layout, behaviour-neutral
    TOKENS DIFFER  crates/lb-observability/src/xdp_metrics.rs
    TOKENS DIFFER  crates/lb-quic/src/h3_bridge.rs
```

Neither file is in my area (`lb-observability` is sweeper-infra, `lb-quic` is
sweeper-quic-r2). **No file of mine is listed.** Run before every commit; it fired once
on `crates/lb/src/main.rs` and I fixed it properly — see below.

### The one code-identity hit, and why the fix is a doc restore not a deletion

Deleting the doc comments off `ListenerMode`'s and `ReloadableProxies`' variants made
`cargo fmt` re-flow `H1 { proxy: SharedH1Proxy },` into a three-line struct-variant body.
That adds a trailing-comma **token**, so the identity script correctly called it
`TOKENS DIFFER`, not `REFLOW ONLY`.

Empirically (rustfmt 2024 edition, no `rustfmt.toml`): rustfmt keeps a struct variant on
one line only when **every** variant in the enum carries a doc comment; a single
undocumented variant (including a unit variant like `PlainTcp`) expands them all. I
restored one short `///` line per variant on both enums — 5 comment lines total — and the
stripped source is byte-identical to `main` again. Restoring docs, never deleting code,
is the only correct resolution of that class.

## Doc floor

Syntactic `missing_docs` checker over `lb/src` + `lb-l4-xdp/src` (handles module docs
living in the module file, multi-line attributes, and private fields) reports one item:

```
crates/lb/src/xdp.rs:17: pub fn try_attach_xdp(_: &lb_config::RuntimeConfig) -> Option<()> {
```

That is the `#[cfg(not(target_os = "linux"))]` stub, **undocumented on `main` too**
(`git show main:crates/lb/src/xdp.rs`) and compiled out on the CI target. Pre-existing,
not introduced here.

`#[allow(...)]` justifications: 3 trailing-comment justifications + 9 `reason = "..."`
clauses, **identical counts to `main`**.

`cargo fmt -p lb -p lb-l4-xdp -p lb-soak` clean. `crates/lb-l4-xdp/ebpf/` is not a
workspace member, so `cargo fmt --all -- --check` never sees it; it is equally
un-rustfmt-clean on `main` and I left its layout alone.

## Per-file (round-1 → round-2)

| file | r1 | r2 | cut |
|---|---:|---:|---:|
| `crates/lb/src/main.rs` | 452 | **108** | 76% |
| `crates/lb-l4-xdp/src/loader.rs` | 362 | **196** | 46% |
| `crates/lb-l4-xdp/ebpf/src/main.rs` | 181 | **117** | 35% |
| `crates/lb-soak/src/loadgen.rs` | 169 | **58** | 66% |
| `crates/lb-l4-xdp/src/nic_compat.rs` | 106 | **59** | 44% |
| `crates/lb-l4-xdp/src/stats_export.rs` | 98 | 53 | 46% |
| `crates/lb-l4-xdp/tests/xdp_attach_mode.rs` | 85 | 23 | 73% |
| `crates/lb-l4-xdp/src/netlink_xdp.rs` | 75 | 56 | 25% |
| `crates/lb-soak/src/bin/eg-soak.rs` | 65 | 28 | 57% |
| `crates/lb-l4-xdp/src/lib.rs` | 65 | 44 | 32% |
| `crates/lb-l4-xdp/src/sim.rs` | 59 | 41 | 31% |
| `crates/lb/src/xdp.rs` | 49 | 26 | 47% |
| `crates/lb-soak/src/config_gen.rs` | 38 | 13 | 66% |
| `crates/lb-soak/src/timeseries.rs` | 18 | 14 | 22% |
| `crates/lb-soak/src/chaos.rs` | 20 | 8 | 60% |
| `crates/lb-soak/src/bench.rs` | 25 | 8 | 68% |
| `crates/lb-l4-xdp/src/bpffs.rs` | 24 | 13 | 46% |
| `crates/lb-l4-xdp/tests/pod_padding.rs` | 27 | 16 | 41% |
| `crates/lb-l4-xdp/tests/round8_conntrack_state.rs` | 27 | 13 | 52% |
| `crates/lb/tests/quic_passthrough_e2e.rs` | 25 | 9 | 64% |
| everything else | — | ≤14 each | — |

`crates/lb/src/main.rs` at 452 → 108 was the single biggest lever: the binary has **zero
`pub` items**, so its only doc floor is the crate-level `//!`. Everything else there
survived on the catch clause alone.

## The move that produced most of round 2

Round 1 kept a lot of correct-but-explanatory doc prose. Round 2 applied the "one line
unless it is a catch" rule literally:

- **Every multi-line `///` block on a private item became zero or one line.** Private
  items have no floor at all — `main.rs`, the `#[cfg(test)]` modules, `lb-soak` in its
  entirety, and every `tests/*.rs`.
- **Every multi-line `///` block on a `pub` item became one line** unless a second line
  carried a distinct hazard. In `loader.rs` I re-examined the 252 lines I called "catch"
  in round 1: roughly 150 of those were *explanation of* a catch rather than the catch
  itself, and they went. What is left is one line per hazard.
- **Section banners deleted** (`// ── TLS helpers ──`, `// IPv4 path.`, `// Entry point.`,
  `// Verifier-safe packet accessors.`, the `// ── SEC-2-06 proof ──` test dividers).
- **Body narration deleted** — every `// Step N:`-style running commentary that restated
  the next statement.
- **`# Errors` / `# Examples` sections**: there were none in this area to delete.

## Where it genuinely stops: `crates/lb-l4-xdp` at 736

This crate is 67% of what remains, and the floor is structural, not stylistic.

**`loader.rs` — 196 lines, and I claim ~155 of them are not removable.** The file has 77
`pub` items plus ~30 `pub` fields and enum variants under `#![deny(missing_docs)]`; it
now carries exactly **155 `///` lines**. That is very close to one doc line per
documented item — the arithmetic floor. The other ~40 lines are non-doc `//` comments
and every one is a hazard: the netlink-is-REAL-not-a-stub note (the old `prog_id: None`
stub made every ownership check vacuous), the EBUSY detach-then-reattach rationale and
the absent `BPF_F_REPLACE` wrapper in aya 0.13.1, the `XdpLinkId`-is-not-`Clone` borrow
ordering, the aya-stores-IPv4-as-`u32.to_be()` byte-order note, the zero-the-tail-on-
shrink invariant, the atomic single-syscall publication, the drain-contract ordering with
OPS-04, and the `const _` byte-size assertions with their per-field arithmetic.

**`ebpf/src/main.rs` — 117 lines, of which 24 are SAFETY.** Of the remaining ~93, the
verifier and kernel-ABI constraints are the bulk: the `checked_add` /
`llvm.uadd.with.overflow.i64` bounds-check-elision guard citing aya #1562 and
CVE-2022-23222; "the verifier will not accept an unbounded loop" behind the
two-extension-header cap; the two `IMPORTANT: changing this file obliges a verifier-log
baseline refresh` gates; RFC 1624 §3's checksum equation; RFC 791 §3.1's `frag_off` bit
layout; RFC 2460 §4.5 fragment-header presence in *both* first and later fragments; the
Katran `is_under_flood()` and Cilium sliding-RST prune lessons; the Unimog D1 atomic
publication and lesson-3 daisy-chain; the LRU-vs-HashMap map-type rationale; the
`#[map(name = "...")]` lowercase-pin decoupling; the `no_mangle`-keeps-DCE-off-the-license-
symbol note. I re-read the whole diff line by line, twice, and every citation is still
present at ≥1 occurrence — verified by grepping each token against `main` (the only drops
are duplicate mentions collapsed by compression).

Compression on this file was purely prose-to-one-line. **No attribute, no map name, no
constant, no statement was touched** — code identity confirms it, and I never let a range
containing a non-comment line through the cutter.

**`nic_compat.rs`, `netlink_xdp.rs`, `stats_export.rs`, `bpffs.rs`** are the same shape:
`deny(missing_docs)` + kernel/wire ABI. `netlink_xdp.rs` retains the RTM_GETLINK wire-
format diagram (a reader cannot follow `parse_ifinfo_payload` without it) and the
panic-freedom guarantee; that diagram alone is 15 of its 56 lines, which is why it shows
the smallest percentage cut in the table.

## Catches deliberately preserved (each now one or two lines)

**lb** — the `rebuild_l7_proxies` HONESTY INVARIANT and its coupling to `LbConfig::diff`;
OPS-04+L4-12 case C-3 (the SYNCHRONOUS post-accept tail check, and why `select!` alone
leaks an fd); the `biased` cancel arms; SEC-2-04 admit-before-semaphore ordering;
ROUND8 OPS-02 intra-pod vs cross-replica drain jitter; F-RES-5 watchdog-is-observability-
only; CF-S27-2 WS-over-H2 off by default; the two-independent-RCU-swaps no-tearing
argument; `_xdp_loader` drops after the drain settles; admin listener cancelled before the
coordinator; the ROUND-8 OPS-11 readiness-settle fallback must match
`lb_config::default_readiness_settle_ms()`; the `LifecycleSignals::recv` disjoint-borrow
note; the R8 wired-tunnel non-vacuity argument (the client's quiche windows MUST stay
capped or the test is vacuous); the R13 reset-vs-EOF positive/negative controls;
CF-S15-PASSTHROUGH-RETRY-ODCID; SEC-2-11 CAP_BPF→CAP_SYS_ADMIN fallback policy.

**lb-soak** — F-S20-1 cwnd partial-write contract (and the 4×4096-over-13.5 KB test that
exercises it); the F-S20-2 leak DISCRIMINANT `fds`, including why `accept_inflight` is
excluded as a low-baseline sawtooth; F-S27-2 "this client READS NORMALLY"; the BOUNDED/
DRIFT 1.8× calibration and the sawtooth false-leak guard; CF-S15 `mint_retry = false`;
CF-S19 teardown fixture choice; RFC 6455 §5.6 binary-not-Text; the non-vacuity argument
for the backendless-H3 two-outcome assertion and its load-bearing negative control; the
`loop { break id }` panic-freedom-lint workaround; the S21 "a load client must HONOR flow
control" lesson; StreamReset-vs-StreamStopped with `H3_REQUEST_CANCELLED = 0x10C`.

## Refused to cut

- **Every SAFETY block, untouched.** See above.
- **Every `pub`-item doc line under the `missing_docs` floor**, including pure
  restatements like `/// Source port (network byte order).` — deleting them turns
  `clippy -D warnings` red.
- **The `loader.rs` `TODO(L4-06)` tracked deferral** (still at `loader.rs:838`).
- **The netlink wire-format diagram** and the ELF-header byte-layout comments in the
  `license` tests — the code is unreadable without them. (I did convert four standalone
  `// e_type = ET_REL` lines in one test to trailing comments on their own statements,
  matching the sibling helper's existing style; comment-only, code-identical.)
- **`quic_passthrough_e2e.rs:418+`** — the CF-S15-PASSTHROUGH-RETRY-ODCID block. I
  compressed it further to the verifiable mechanism, but the round-1 finding stands: the
  original text was internally contradictory and **still needs an owner ruling**. I did
  not invent a resolution.

## `loader.rs` coverage carve-out — unchanged finding

The carve-out is documented in `scripts/ci/coverage-check.sh` (header comment line 9 plus
the `EXEMPT` constant), **not** in `loader.rs`. I did not touch that script and nothing in
`loader.rs` referenced the carve-out to begin with, so the gate's honesty contract is
unaffected.

## Two content defects found and fixed mid-sweep

1. A compression of the `NEW_FLOW_RATE` block in `ebpf/src/main.rs` truncated the
   `IMPORTANT:` verifier-log note mid-sentence, dropping the
   `audit/ebpf/verifier-logs/*.committed` path. Restored in full on one line.
2. Two replacement strings leaked a doubled backslash (`\\0` in the ELF shstrtab layout
   note, `\\` in the `eg-bench` usage block). Both corrected.

Both were caught by re-reading the produced comments, not by a gate — worth noting for
whoever reviews the other sweepers' output.

## Verification performed

- `python3 audit/craft/s45a-code-identity.py main` before **every** commit; output above.
- SAFETY count + per-file distribution after every batch: 39 throughout.
- Syntactic `missing_docs` floor check over `lb/src` + `lb-l4-xdp/src`.
- `#[allow]` justification counts compared against `main`: identical.
- Dangling-doc scan (a `///` with no item after it): none. rustfmt parses every touched
  file, including the out-of-workspace ebpf crate.
- Token-presence check against `main` for every kernel/CVE/RFC/reference-implementation
  citation in the ebpf tree.
- `cargo fmt -p lb -p lb-l4-xdp -p lb-soak`. No build/clippy/test run (per brief).
