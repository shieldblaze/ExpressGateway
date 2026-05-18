# S7 Phase-0 Baseline — INDEPENDENT RE-BASELINE (LITERAL GREEN)

Verifier: `verifier` (author != verifier, R5 — verifier did not edit any source).
Date (UTC): 2026-05-18T20:42:37Z
Worktree: /home/ubuntu/Code/s7-verifier  (detached HEAD, no merge into s7/verifier)

================================================================
TREE UNDER TEST
================================================================
Integrated commit : e641f2b495f6cddc4cd023064c934d56fefaf0d2
  ("fix(s7/P0): env-scope panic_abort.rs child cargo build")
Parent            : 2c62896f8913604bfc0bc6abf05c30a7ba2a1ef8
  ("Promote S6: H3->H2 cell BUILT + H3->H3 R8 plan approved")

`git diff 2c62896f e641f2b4` — the ONLY delta vs prior S7 base:
  crates/lb/tests/panic_abort.rs | 4 ++++   (1 file, +4 -0)

The 4 added lines insert `.env_remove("CARGO_TARGET_DIR")` on the
child `cargo build --release --quiet` Command in panic_abort.rs so
the probe builds into <tmp>/target (current_dir/target), matching
the run step and keeping it out of the shared workspace target/.
This is exactly the targeted P0 test-harness fix; no product code
changed.

Disk discipline: CARGO_TARGET_DIR exported to the shared
/home/ubuntu/Code/ExpressGateway/target for ALL cargo invocations
below (never `env -u`). Free space stayed 31G throughout (floor 12G).

================================================================
GATES
================================================================
cargo fmt --check ............................. CLEAN (exit 0)
cargo clippy --all-targets --all-features
  -- -D warnings .............................. CLEAN (exit 0)

cargo test --workspace --all-features --no-fail-fast  (R1 x3):
  Run 1 : passed=1132  failed=0  ignored=16   exit 0
  Run 2 : passed=1132  failed=0  ignored=16   exit 0
  Run 3 : passed=1132  failed=0  ignored=16   exit 0
  DETERMINISTIC across 3 runs (counts AND the 16-member ignore-set
  byte-identical run to run). 203 test binaries enumerated.

  LITERAL GREEN: zero failures. No asterisk, no qualifier (R1/R4).

panic_abort pair — now BOTH pass WITH CARGO_TARGET_DIR exported
(this is the fix being validated; prev-failing test is the second):
    release_profile_has_panic_abort ............ ok
    panic_in_tokio_task_aborts_release_process . ok
  => The S7-phase0 sole prior failure is RESOLVED by the P0 fix.
     Root cause (the mandated CARGO_TARGET_DIR being inherited by
     the panic_abort child cargo build) is eliminated; policy
     remains in force everywhere else.

R10 cargo test --workspace --all-features --features test-gauges:
  passed=1132  failed=0  ignored=16   exit 0
  R8/memory + backpressure gauge cases EXECUTED (NOT skipped):
    h2_e2e_request_memory_bounded_through_stalled_backend ...... ok
    h2_e2e_response_memory_bounded_through_stalled_client ...... ok
    r2_response_memory_bounded_through_stalled_client .......... ok
    t5_single_large_data_frame_is_memory_bounded_
        through_stalled_upstream .............................. ok
    h2_e2e_backpressure_stalled_client_pauses_
        h2_upstream_read ...................................... ok
  => R10 gate flag honored; gauge-gated proofs ran with the flag.

================================================================
IGNORED SET (16) — UNCHANGED vs prior S7 base (no new suppressions)
================================================================
accepts_real_bpffs
capture_real_70_verifier_baseline
crates/lb-observability/src/label_budget.rs - label_budget (line 8)
crates/lb-security/src/admin_auth.rs - admin_auth (line 8)
crates/lb-security/src/hooks.rs - hooks (line 13)
crates/lb-security/src/watchdog.rs - watchdog (line 12)
d1_native_attach::drv_mode_attach_to_ens5_proves_live_datapath
detach_verifying_on_real_iface
h2spec_server_conformance_passes
live_query_loopback_has_no_xdp
lru_evicts_oldest_under_flood
probe_synthetic_packet_round_trip
summed_counters_advance_under_load
test_maps_pinned_then_loaded_from_pin
test_skb_fallback_logs_warning
xdp_link_persists_after_id_drop

Compared semantically against the prior base baseline
(commit 7a236f10 audit/h-matrix/s7-phase0-baseline.txt): same 16
members, same documented privileged/CAP_BPF/doctest categories,
identical across all three R1 runs. NO new suppressions introduced
by/around the S7 P0 fix. (memory: s2-verification-gap lineage —
this is a full --all-features run, no isolation hiding.)

================================================================
VERDICT
================================================================
LITERAL GREEN — no qualifier.

  fmt clean; clippy clean; R1x3 = 1132/0/16 deterministic with
  ZERO failures and no asterisk; panic_abort pair both green WITH
  the mandated CARGO_TARGET_DIR exported (P0 fix validated); R10
  test-gauges proofs EXECUTED; 16-member ignore-set unchanged vs
  prior base (no new suppressions).

Phase-0 baseline is clean and unblocks task #3.
