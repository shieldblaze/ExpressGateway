# Round-9 Verify — Iteration 2 (verifier, fresh reproduction)

Prior run crashed on transient API 500 before verdict; this is a from-scratch
adversarial reproduction. Author != verifier.

- Working dir: /home/ubuntu/Code/ExpressGateway
- Branch: feature/h3-green
- HEAD: cdb864b5cc2ab9cc6dc2b7e08289088e8ad523b9
- Disk at start: 11G free / 28G (62% used). At end: 11G free.
- Constraints honored: scoped `-p` builds only; no `--workspace`; no
  `cargo llvm-cov`. Every `sudo cargo` run followed by
  `chown -R ubuntu:ubuntu target`.

## VERDICT TABLE

| Item | Subject | Verdict |
|------|---------|---------|
| A | D-1 native ENA DRV attach (3fd06339) | VERIFIED-PASS |
| B | D-4 h2spec harness threading (450b6e80) | VERIFIED-PASS |
| C | metrics_xdp_slots 6 labels (450b6e80) | VERIFIED-PASS |

Verbatim D-1 attach line:
`D-1 attach: mode=Drv, attempts=1, prog_id=100`
`D-1 kernel view: 2: ens5: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 3498 xdp qdisc mq state UP mode DEFAULT group default qlen 1000`
`D-1 data-path: STATS aggregate 0 -> 2 (delta 2 > 0)`

Verbatim external h2spec summary (independent, `-t -k -S`, reproduced twice):
`147 tests, 146 passed, 1 skipped, 0 failed`  (exit 0)

---

## A. D-1 native ENA attach (commit 3fd06339)

### A.1 Source diff review
`git show 3fd06339 --stat`: single file
`crates/lb-l4-xdp/tests/xdp_attach_mode.rs`, `1 file changed, 432
insertions(+)`, **zero deletions**. Diff hunk header:
`@@ -78,3 +78,435 @@ fn test_skb_fallback_logs_warning()` — a single
append-only hunk anchored AFTER the stub; no `-` lines anywhere. The
`test_skb_fallback_logs_warning` stub (eprintln!-only, `#[ignore]`) is
byte-for-byte retained and not weakened. Only a new gated module
`d1_native_attach` was added.

### A.2 Live reproduction (run by verifier as root)
Pre-test baseline (independent): ens5 MTU 9001, Combined 8, driver `ena`
(version 7.0.0-1004-aws), `ip -d link` shows no `xdp`/`prog/xdp`,
/sys/fs/bpf has only pre-existing unrelated `tc`/`ip`/`xdp` symlinks
(no lb pins).

Command: `sudo -E env "PATH=$PATH" cargo test -p lb-l4-xdp --test
xdp_attach_mode -- --ignored --nocapture` (note: `sudo -E` warned
"ignored" but PATH was passed via `env`; test ran as root).

Output:
```
D-1 ena-guard: lowering ens5 MTU 9001 -> 3498 for the native-attach window (restored on teardown)
D-1 ena-guard: lowering ens5 combined channels 8 -> 4 for the native-attach window (restored on teardown)
D-1 attach: mode=Drv, attempts=1, prog_id=100
D-1 kernel view: 2: ens5: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 3498 xdp qdisc mq state UP mode DEFAULT group default qlen 1000
D-1 data-path: STATS aggregate 0 -> 2 (delta 2 > 0)
D-1 teardown: ens5 bare, MTU restored to 9001, combined restored to 8, /sys/fs/bpf clean (detach_verifying ok=true)
test d1_native_attach::drv_mode_attach_to_ens5_proves_live_datapath ... ok
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 1 filtered out
```
- mode = **Drv** (NOT Skb/generic). Kernel `ip -d link` line shows bare
  `xdp` keyword (NOT `xdpgeneric`, NOT `xdpoffload`); the test's
  assertions also require `prog/xdp` present and `xdpgeneric` /
  `xdpoffload` absent — all passed.
- STATS delta strictly positive (0 -> 2).
- `target` chowned back to ubuntu:ubuntu immediately after.

Independent post-test residue check (verifier-run, not test-run):
`ip -d link show ens5` → 0 xdp; `"mtu":9001`; `ethtool -l` Combined 8;
`ethtool -i` driver ena; /sys/fs/bpf → no lb pins (byte-identical to
pre-test baseline). Zero residue.

### A.3 Adversarial false-green analysis
- aya 0.13.1 `Xdp::attach` (xdp.rs:108→attach_to_if_index) passes
  `flags.bits()` (XDP_FLAGS_DRV_MODE) straight into `bpf_link_create`.
  There is **no aya-side SKB fallback**; a DRV-incapable path returns
  `SyscallError`. The kernel does not silently downgrade DRV→SKB when
  the DRV flag is set (returns EOPNOTSUPP).
- `loader.rs:1126`: `XdpModeChoice::Native => &[XdpMode::Drv]` — single
  mode, no Skb in the ladder; exhaustion → `AllAttachModesExhausted`
  which the test `.expect()`s LOUD.
- Triple proof: aya `outcome.mode == XdpMode::Drv` AND kernel
  `ip -d link` cross-check (`prog/xdp` present, `xdpgeneric`/
  `xdpoffload` absent) AND strictly-positive STATS delta.
- RAII `DetachGuard` constructed (with original MTU/Combined recorded)
  BEFORE the attach `.expect`, so an attach failure still restores
  jumbo MTU/channels on unwind. Drop has an unconditional
  `ip link set dev ens5 xdp off` backstop independent of aya state,
  plus post-teardown clean-state assertions (double-panic = loud).
- No path to a false green identified.

VERDICT A: **VERIFIED-PASS** — reproduced, genuine DRV, no weakening,
clean teardown independently confirmed.

---

## B. D-4 h2spec (commit 450b6e80)

### B.1 Source diff review
`git show 450b6e80 -- tests/h2spec.rs`: ONLY change is
`-#[tokio::test]` → `+#[tokio::test(flavor = "multi_thread",
worker_threads = 2)]` plus a 10-line explanatory comment. No assertion
removed, no `#[ignore]`, no skip added. `out.status.success()` check and
`panic!`+stdout-on-failure path intact (h2spec.rs:196-203).

FINDING (non-blocking, environmental): the working tree was DIRTY on
arrival — `tests/h2spec.rs` carried 65 UNCOMMITTED added lines
(`verifier_hold_listener_for_external_h2spec`, marked VERIFIER-TEMP),
leftover from the prior crashed verifier run. This is NOT part of
commit 450b6e80 and is `#[ignore]`-gated + additive (does not touch
`h2spec_generic_conformance`). I stashed it to test the pristine
committed code, used it (it is purpose-built) for the independent
external corroboration, then reverted `tests/h2spec.rs` to the
committed 205-line state (`git checkout --`). Tree left clean. This
cruft does not weaken the committed test; flagged for lead awareness.

### B.2 In-tree test (committed code, pristine)
h2spec 2.6.0 confirmed on PATH (`/home/ubuntu/.cargo/bin/h2spec`), so
the test does NOT vacuously skip.
`cargo test -p lb-integration-tests --test h2spec
h2spec_generic_conformance -- --nocapture`:
```
h2spec passed (21853 bytes stdout)
test h2spec_generic_conformance ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```
"h2spec passed (21853 bytes stdout)" proves the subprocess genuinely
ran and exited 0 (not a skip-return).

### B.3 Independent external corroboration
Started the real gateway H1/H2 listener via the helper (identical
`build_server_config`/`H2Proxy`/`H1Proxy` wiring as the in-tree test),
port 34191 (confirmed LISTEN via `ss`). Ran installed h2spec directly:
`h2spec -h 127.0.0.1 -p 34191 -t -k -S --timeout 2`. Reproduced TWICE,
deterministic:
```
147 tests, 146 passed, 1 skipped, 0 failed
```
exit 0. (`-S` adds the HPACK suite → 147 vs in-tree 146.) The single
skip is h2spec's own internal classification (not run / N/A), never
counted as failed, exit code 0 — NOT a gateway non-conformance. **Zero
FAILED cases.** Listener stopped, /tmp port file removed, helper
reverted.

VERDICT B: **VERIFIED-PASS** — only the threading attribute changed,
in-tree test genuinely passes, independent external h2spec shows
0 failed.

---

## C. metrics_xdp_slots (commit 450b6e80)

### C.1 Test diff byte-identity
`git show 450b6e80 -- crates/lb-observability/tests/metrics_xdp_slots.rs`:
filtering out all comment lines yields ZERO changed code lines. Only
the module doc-comment and one inline slot-order comment changed; the
`(1..=NUM_SLOTS as u64).collect()` and all assertions are untouched.

### C.2 Label→discriminant position cross-check
Authoritative `#[repr(usize)]` enum in
`crates/lb-l4-xdp/src/stats_export.rs` (NUM_SLOTS=16):
0 Pass, 1 Drop, 2 CtHitV4, 3 L7Divert, 4 ParseFail, 5 TxV4, 6 CtHitV6,
7 TxV6, 8 VlanStripped, 9 V6ExtUnsupported, 10 BackendUnpopulated,
11 V4Fragment, 12 V6Fragment, 13 CtRstPrune, 14 CtFinPrune,
15 NewFlowRateCap, 16 AttachProbeFailed.

`stat_slot_labels()` array in
`crates/lb-observability/src/xdp_metrics.rs` (indices 0..15):
pass, drop, ct_hit_v4, l7_divert, parse_fail, tx_v4, ct_hit_v6, tx_v6,
vlan_stripped, v6_ext_unsupported, backend_unpopulated, v4_fragment,
v6_fragment, ct_rst_prune, ct_fin_prune, new_flow_rate_cap.

Position N == StatSlot discriminant N for all 16 slots. The 6 new
labels (10..15) are in exact discriminant order. AttachProbeFailed=16
correctly excluded (userspace-only atomic, outside NUM_SLOTS). The
committed xdp_metrics.rs diff is whitespace re-alignment of the
existing 10 labels + the 6 correct new entries; no logic change. **No
mislabel.**

### C.3 Test run
`cargo test -p lb-observability --test metrics_xdp_slots`:
```
test label_key_is_action_not_result ... ok
test all_stat_slots_are_exported_at_zero ... ok
test deltas_apply_per_slot ... ok
test result: ok. 3 passed; 0 failed
```
3/3.

VERDICT C: **VERIFIED-PASS** — assertions byte-identical, labels map
exactly to enum discriminants, 3/3 pass.

---

## Final environment state (left clean)
- git tree: clean (no porcelain).
- target/: ubuntu:ubuntu, no root-owned files.
- ens5: 0 xdp progs, MTU 9001, Combined 8 (fully restored).
- /sys/fs/bpf: no lb pins.
- disk: 11G free.

Overall: all three items VERIFIED-PASS. No test weakened. One
non-blocking environmental finding (uncommitted verifier-temp helper in
working tree on arrival, now reverted).
