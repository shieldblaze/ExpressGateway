# Plan for ROUND8-L4-05 — Post-attach XDP_TX liveness probe (mlx5/CX6 silent-drop class)

Finding-ref:     ROUND8-L4-05 (high after lead arbitration in `cross-review.md`; was medium)
Reference:       aya issue #1193 (MLX5/ConnectX-6 silent drop of `XDP_REDIRECT` in DRV mode); Cilium lesson 8 (ConnectX-4 Lx VF silent fail, Cilium added runtime fallback); handoff item 5.
Coverage-gap:    Theme 1 (EBPF-2-04 attach-mode ladder graded Verified-Fixed without the silent-drop probe). Theme 4 (multi-validator handoff: ebpf audited attach-syscall correctness; code/proto never re-walked redirect-actually-works).

Files touched:
  - `crates/lb-l4-xdp/src/loader.rs`                (`XdpLoader::attach_with_fallback`: add post-attach BPF_PROG_TEST_RUN probe; new error variant; known-bad-NIC list)
  - `crates/lb-l4-xdp/src/stats_export.rs`          (`xdp_attach_probe_failed_total` counter; `StatSlot::AttachProbeFailed`)
  - `crates/lb-l4-xdp/src/nic_compat.rs`            (NEW — driver/firmware blocklist for `Drv` mode; reads `/sys/class/net/<iface>/device/driver` + `ethtool -i`)
  - `RUNBOOK.md`                                    (operator instructions for forcing `xdp_mode = "skb"` on known-bad NIC + firmware combos)
  - `crates/lb-l4-xdp/tests/round8_attach_probe.rs` (NEW — proof)

Approach:

1. **Synthetic-packet probe via `BPF_PROG_TEST_RUN`**
   (`crates/lb-l4-xdp/src/loader.rs`):
   - After `xdp.attach(ifname, mode.to_flags())` returns `Ok(_)` in
     `attach_with_fallback`, BEFORE recording the attach mode,
     run a synthetic probe:
     ```rust
     fn probe_xdp_redirect(prog_fd: ProgramFd) -> Result<ProbeOutcome, XdpLoaderError> {
         // build a minimal Ethernet+IPv4+TCP packet matching a known
         // synthetic CT entry pre-inserted by the test fixture.
         let mut input = make_synthetic_packet();
         let mut output = [0u8; 1500];
         let test = aya::programs::xdp::ProgramTestRun {
             prog_fd, data_in: &input, data_out: &mut output,
             repeat: 1, ..Default::default()
         };
         let result = test.run()?;
         if result.retval != aya::programs::xdp_action::XDP_TX {
             return Ok(ProbeOutcome::SilentDrop);
         }
         // verify the dst MAC was actually rewritten — the silent-drop
         // class returns success on the action but doesn't run the
         // program body.
         if &output[0..6] == &input[0..6] {
             return Ok(ProbeOutcome::NotExecuted);
         }
         Ok(ProbeOutcome::Ok)
     }
     ```
   - If probe returns `SilentDrop` or `NotExecuted` in `Drv` mode,
     demote to `Skb` and re-attach. If probe still fails on `Skb`,
     fail loudly (`XdpLoaderError::AttachProbeFailed { mode, reason }`).
   - This mirrors Cilium's runtime fallback (their `XDP_REDIRECT`
     probe).

2. **Known-bad-NIC blocklist** (`crates/lb-l4-xdp/src/nic_compat.rs`,
   NEW):
   - Source-truth table mapping `(driver, firmware_version_range)` to
     `Drv`-unsafe. Initial entries:
     - `mlx5_core` firmware `< 16.32.1010` (matches the GHSA window).
     - `ena` firmware `< 2.10` on `c5n`/`m5n` instance families.
     - `ice` firmware `<= 4.10` (Cilium's regression list).
   - `pub fn drv_supported(iface: &str) -> Result<DrvSupport, NicCompatError>`
     reads `/sys/class/net/<iface>/device/driver` (symlink target)
     and `ethtool -i <iface>` (firmware-version line). If on the
     blocklist: `DrvSupport::Refuse { reason }`.
   - Called by `attach_with_fallback` BEFORE attempting `Drv` mode.
     If `Refuse`, skip `Drv` and log loud WARN with the bug-id link.

3. **New error + counters**:
   - `XdpLoaderError::AttachProbeFailed { mode: XdpMode, reason: String }`.
   - `xdp_attach_probe_failed_total{iface, mode}` Prom counter, slot
     14 (`StatSlot::AttachProbeFailed`).

4. **RUNBOOK.md updates**:
   - New section "Known-bad NIC + firmware combinations for native
     XDP" with the blocklist table + operator override instructions
     (`xdp_mode = "skb"`).
   - Pointer to `xdp_attach_probe_failed_total` and the demotion log
     line so on-call can correlate.

5. **Proof tests** (`crates/lb-l4-xdp/tests/round8_attach_probe.rs`,
   NEW):
   - `probe_synthetic_packet_round_trip` (CI `--ignored`, requires
     CAP_BPF): attach to `dummy0`, pre-populate CT with a synthetic
     entry, run probe, assert `ProbeOutcome::Ok` and dst MAC was
     rewritten.
   - `probe_no_op_program_returns_not_executed` (CI `--ignored`):
     attach an `XDP_PASS`-only prog, run probe, assert
     `ProbeOutcome::NotExecuted` (dst MAC unchanged).
   - `nic_compat_blocklist_table_round_trip`: unit-test parses the
     hardcoded blocklist; spot-check mlx5_core entry; assert
     `drv_supported("eth0")` against a mock sysfs returns
     `Refuse { reason }` matching the GHSA link.
   - `attach_with_fallback_demotes_on_probe_fail` (sim): inject a
     fake probe-fail in the `Drv` attempt; assert the ladder lands
     on `Skb` and `xdp_attach_probe_failed_total{mode="drv"}` ticks
     by 1.

Proof:

- `cargo test -p lb-l4-xdp --test round8_attach_probe` (userspace
  blocklist + ladder); the kernel-touching parts gated `--ignored`.
- Re-capture verifier-log baselines per L4-10 (no BPF change but
  attach surface changes; sanity diff).
- Acceptance evidence: after probe fails on a mlx5 box (or simulated
  fail), the `xdp_attach_mode` gauge correctly reads `skb`, not
  `drv`; the `xdp_attach_probe_failed_total` counter is non-zero.

Risk / blast radius:

- `BPF_PROG_TEST_RUN` requires CAP_BPF on the running process; we
  already need that to load. No new privilege.
- Probe adds ~5-50ms to startup. Acceptable.
- The blocklist will go stale; document the policy that the list is
  best-effort + the probe is the always-on backstop.
- `ethtool -i` shell-out is the cheap path; an aya-native ethtool
  binding would be cleaner. v1 = shell-out, document as a follow-up.

Cross-ref:
- `audit/ebpf/round-2-review.md` EBPF-2-04: status should be amended
  to note the attach-mode ladder is now backed by a runtime probe
  AND a known-bad-NIC table; the "Verified-Fixed" grade narrowly
  meant "ladder exists" — the silent-drop class lands here.
- ROUND8-L4-12: attach replace + `BPF_F_REPLACE` is independent;
  serialise plans so L4-12's `attach_replacing` API can reuse the
  same probe.

Owner:           div-l4
Lead-approval: approved 2026-05-14 team-lead-r8
