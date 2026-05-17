
---
# builder-2 Phase-2 implementation checkpoints

## F-COR-2 / BL-1 (THE R1 BLOCKER) — DONE
- Commit: 469052ec  (tests/balancer_counter_sync.rs only)
- Pre-fix: original binary, 24x CPU oversubscription, 80 runs -> 4/80
  FALSE failures incl. decisive snapshot=9880 bracket=[9881,9881]
  (pre==post; single AtomicU64 cannot tear -> coherent oscillation value).
- Post-fix: deleted unsound non-monotonic bracket; KEPT sound post-join
  exact-equality (CODE-2-14 contract, unchanged); ADDED deterministic
  single-threaded monotonic mid-flight sub-test. 24x oversubscription,
  85 runs -> 0/85 failures, all 4 tests pass.
- Negative guard (demonstrated, NOT committed): stubbed sync_from_state
  stale-by-one -> new sub-test FAILED ("snapshot=0, atomic=1"); reverted.
- Unsoundness proof recorded verbatim in the commit message. NOT
  test-weakening (R5): deleted a provably-false assertion.

## F-COR-3 — DONE
- Commit: 20e22560 (tests/reload_zero_drop.rs only)
- Replaced fixed shared "eg-test-zero-drop"/"eg-drain-h2"/"eg-drain-h3"
  with unique_temp_dir (drain::unique_temp_dir for the file-top-level
  soak test; in-scope unique_temp_dir for the H2/H3 mod-drain_tests).
- Compiles (path resolution confirmed); test_reload_zero_drop_under_load
  green x3. Pure isolation hardening; mirrors verified H1 fix 9e58bbf2.

## F-COR-4 — DONE
- Commit: 05d801c1 (tests/reload_zero_drop.rs only)
- Extracted pure classify_close(); added CloseKind::BodyCompleteNoClose;
  FinOnly now REQUIRES observed Ok(0); tightened the per-iter assertion;
  added pure truth-table test + deterministic stub-server negative
  regression (complete body, no header, socket held open / never FIN).
- Pre-fix (old `let _ = clean_eof` simulated): negative regression
  FAILED — misclassified Some(FinOnly) (left Some(FinOnly) / right
  Some(BodyCompleteNoClose)). Reverted.
- Post-fix: classifier => Some(BodyCompleteNoClose); both new tests
  pass; full reload_zero_drop binary 6/6. No product source change.

## F-COR-5 — DONE
- Commit: 366be028 (crates/lb-h3/tests/proptest_qpack.rs only)
- Removed dead #![cfg(feature="proptest")]; mirrored S1 25d8ad84
  doc-comment. Pre-fix (auditor-4 captured) default `running 0 tests`;
  post-fix default `running 4 tests` 4 passed; --all-features 4 passed.

## F-COR-8 — DONE
- Commit: e42657f6 (crates/lb-quic/tests/h3_graceful_close.rs only)
- Added static CERT_SEQ AtomicU64 + -{seq} suffix mirroring
  round8_h3_authority_enforced.rs:75-81. test green (1 passed).
  Latent collision class structurally closed.

## F-COR-7 — DONE (lead D1 REDIRECT implemented; NOT escalated)
- Commit: a7b6aacf (crates/lb-l4-xdp/src/nic_compat.rs + new
  tests/round8_ena_kernel_blocklist.rs)
- Driver+kernel key was expressible WITHOUT a fleet-affecting product
  guess: the ena BlockRow's own reason already documents the condition
  ("pre-2024 kernels"); kernel 6.7 (Jan 2024) = first 2024 mainline →
  bad_kernel_below: Some((6,7)). No new fleet guess; did not escalate.
- classify() left pure/UNCHANGED. New pure classify_unresolved_firmware
  used only on the firmware-unresolved branch of drv_supported.
- BOTH lead-D1 assertions proven:
  (1) real-box drv_supported("ens5") == Allowed on this ena/7.0 box
      (full resolution path, ran for real, no SKIP — native XDP
      preserved, D-1-consistent, no fleet regression);
  (2) synthetic known-bad ena/pre-6.7 → Refuse (dead path now live).
- Pre-fix (dead fail-open simulated): known-bad combo wrongly Allowed,
  regression FAILED. Post-fix: lib 13/13, integration 1/1, clippy
  -p lb-l4-xdp --all-targets -D warnings clean.

## F-DOC-1 — DONE
- Commit: 3e23ba9e (DEPLOYMENT.md + audit/ebpf/verifier-logs/README.md)
- Effective floor ~5.15, 5.15/6.1/6.6 validated window, 7.0 validated
  live (D-1), 7.x = OPEN R7 product decision (not gate-blocking,
  not asterisked). Docs only, no code/script change.

## F-ESC-1 — DONE (7.0 fixed this session; 5.15/6.1/6.6 infeasible -> escalate)
- Commit: 59946e21 (7.0.log.committed REAL + capture test +
  builder-2-to-lead-fesc1.md verbatim infeasibility)
- 7.0: real aya BPF_PROG_LOAD on running kernel; ProgramInfo
  verified_insns=9284 xlated=12800 jited=7264 tag 0x72c34ab7e4f44914;
  bpftool prog show id <id> --json on the loaded prog. Privileged
  capture test passes 1/1 under sudo. NOT asterisked.
- 5.15/6.1/6.6: genuinely infeasible here — no lvh/qemu/vng, no
  /dev/kvm + zero vmx/svm (unaccelerated guest), verify-xdp.sh digests
  are literal "" placeholders, floating lvh-image has no bash/bpftool
  and docker-run shares the HOST 7.0 kernel, and bpftool cannot load
  the legacy-map ELF anyway. Full verbatim mechanism in
  builder-2-to-lead-fesc1.md. LEAD: please formally escalate this
  residual multi-kernel CI lane (R6/R7(a), ~0.5d: pin real digests +
  fix verify-xdp.sh sh/bash + privileged CI matrix stage).

## ALL ITEMS COMPLETE — commit ledger (no push; lead pushes)
- F-COR-2/BL-1  469052ec
- F-COR-3        20e22560
- F-COR-4        05d801c1
- F-COR-5        366be028
- F-COR-8        e42657f6
- F-COR-7        a7b6aacf  (lead D1 redirect implemented, NOT escalated)
- F-DOC-1        3e23ba9e
- F-ESC-1        59946e21  (7.0 fixed; 5.15/6.1/6.6 -> escalate residual)
NOTE: crates/lb-l7/src/h2_proxy.rs has an uncommitted change that is
builder-1's F-SEC-1 work (different builder, parallel) — I did NOT
touch or commit it; left for builder-1.
