### ROUND8-L4-05 — Drv→Skb attach fallback never runtime-probes XDP_TX (aya #1193 / MLX5/CX6 silent-drop)

Reference: `audit/round-8/research/aya.md` lesson 6 + handoff item 5; `audit/round-8/research/cilium.md` lesson 8 (ConnectX-4 Lx VF silent fail)
Our equivalent: `crates/lb-l4-xdp/src/loader.rs:655-710` (`attach_with_fallback`); `crates/lb/src/xdp.rs:240-288` (`attach_with_elf`)

Severity: high (lead-arbitrated up from medium per cross-review.md)
Status:   Proposed-Fix (div-l4, task#73, 2026-05-15, commit 5600ee95 `ROUND8-L4-05 — real aya-API-availability tripwire`) — the caveat is addressed: the fake `probe_unavailable_is_the_documented_state` const-fn tautology is REPLACED with a real tripwire `aya_version_is_pinned_to_the_no_test_run_release` that introspects the resolved `aya` version in the workspace `Cargo.lock` and FAILS on any divergence from the audited pin (0.13.1 — the release verified to lack a public Xdp::test_run / BPF_PROG_TEST_RUN), plus a defence-in-depth semver guard that fails on any version strictly greater even if the literal is hand-edited forward. The lock parser is isolated + unit-tested: `tripwire_fires_when_aya_is_upgraded_simulation` exercises the exact assertion via catch_unwind on a synthetic aya=0.14.0 lockfile (proves it WOULD fail on an upgrade); `aya_lock_parser_does_not_confuse_sibling_crates` proves it ignores aya-obj/aya-log. Manually verified: mismatching the pin makes the real test FAIL loudly. Proof: `cargo test -p lb-l4-xdp --test round8_attach_probe` 10 pass + 1 ignored (kernel scaffold). The static NIC blocklist (the active defence) is unchanged and remains REAL/correctly wired. Verify re-checks.
          [VERIFIED-FIXED (verify, task#74, 2026-05-15, sha 5600ee95) — re-check of the hollow caveat. round8_attach_probe 10 PASS + 1 ignored. The const-fn tautology is genuinely REPLACED with a real string-parse + semver tripwire. INDEPENDENT /tmp SIMULATION (not the in-test catch_unwind): copied the real workspace Cargo.lock, bumped ONLY the `[[package]] name = "aya"` block 0.13.1 -> 0.14.0 (aya-obj correctly left at 0.2.1), reproduced the tripwire's exact aya_version_from_lock + assert_eq + semver_tuple logic standalone. RESULT: against the REAL lock the tripwire PASSES (aya 0.13.1, eq+semver both true); against the BUMPED lock the parser correctly extracts "0.14.0" (NOT the sibling aya-obj) and the tripwire FAILS (assertion `left == right` panic, eq_assert_passes=false, semver_guard_passes=false). The tripwire DOES detect an aya upgrade — not theater. NON-BLOCKING for prod (kernel BPF_PROG_TEST_RUN probe deferred behind aya API blocker, documented; static NIC blocklist remains the live defence). See audit/round-8/verify/fixback.md.]
          [Prior: Accepted-with-caveat (verify, task#70, 2026-05-15) — static NIC blocklist REAL and correctly wired (mlx5_core/ena/ice; CX4/CX6 both via mlx5_core), but the "tripwire" claim was INACCURATE: probe_unavailable_is_the_documented_state asserted a const fn's hardcoded return — a manual-impl marker, not an auto-detector. See audit/round-8/verify/l4.md. Original Proposed-Fix below.]
          Proposed-Fix — `crates/lb-l4-xdp/src/nic_compat.rs` (NEW):
          static (driver, firmware) blocklist (mlx5_core < 16.32.1010,
          ena < 2.10, ice <= 4.10) with dotted-version compare +
          `ethtool -i` / sysfs introspection (fail-open). Wired into
          `attach_with_fallback` BEFORE the `Drv` attempt — refuses
          `Drv` on a known-bad combo (a failed-attach check is
          insufficient: the silent-drop attach SUCCEEDS), demotes to
          `Skb`, loud WARN, bumps `xdp_attach_probe_failed_total`
          (process-global atomic; `StatSlot::AttachProbeFailed` = 16,
          userspace-only — deliberately NOT in NUM_SLOTS since the
          BPF program never runs on a silent-drop attach).
          `XdpLoaderError::AttachProbeFailed`. RUNBOOK.md "Known-bad
          NIC + firmware" section with the table + `xdp_mode = "skb"`
          override.

          API BLOCKER (explicit, per Round-8 instruction): aya 0.13.1
          exposes NO public `BPF_PROG_TEST_RUN` / `test_run` wrapper
          on the `Xdp` program type (confirmed: only the raw kernel
          binding constant exists in aya-obj's generated bindings).
          The runtime synthetic-packet probe is shipped as a typed
          scaffold (`ProbeOutcome`, `probe_xdp_silent_drop()` →
          `ProbeUnavailable`) with a tripwire test
          (`probe_unavailable_is_the_documented_state`) that fails
          when aya gains the API — the signal to wire the real probe.
          The static blocklist (defence 1) is FULLY wired today; the
          probe (defence 2 / backstop) is deferred-to-CI with this
          explicit note. Same posture as ROUND8-L4-12's netlink stub.

          Proof `crates/lb-l4-xdp/tests/round8_attach_probe.rs`
          (8 tests + 1 ignored kernel scaffold, green). No BPF source
          change (attach surface only); verifier-log sanity-diff per
          ROUND8-L4-10. Awaiting verification sign-off.

Divergence:
- Aya issue #1193: MLX5/ConnectX-6 in DRV mode silently drops `XDP_REDIRECT`. (`XDP_TX` may share the bug — verified on at least some firmware revisions.) `bpf_link_create` returns success, every map operation reports success, packet path silently goes to /dev/null.
- Us: `attach_with_fallback` ladder is `Drv -> Skb` for `Auto`. The "fallback" trigger is `EOPNOTSUPP` / `EINVAL` from the *attach* syscall. There is no post-attach probe — we cannot detect the silent-drop class of bug.
- Cilium learned this scar and added a runtime fallback. The handoff explicitly called this out (item 5).

Impact:
- A user installs ExpressGateway on a ConnectX-4 Lx / ConnectX-6 host, requests `xdp_mode = "auto"`, the loader picks `Drv`, records `xdp_attach_mode = "drv"` in the metric — *and then forwards zero traffic*. The metric says everything is healthy.
- The `audit/STATE` Round-7 entry counts EBPF-2-04 (attach-mode reporting) as Verified-Fixed. The fix is *correct as far as it goes* but the broader silent-drop class is not addressed.
- This is the classic "metric lies, packet path is dead" failure mode.

Reproduction:
- Needs MLX5/CX6 hardware to actually trigger. The class of bug is reproducible on AWS `c5n` / `m5n` instances with ENA virtualisation in pre-2024 kernels.

Recommendation:
1. After `attach.with_fallback` succeeds with `Drv` mode, run a *liveness probe*: emit a synthetic packet via `XDP_TEST_RUN` (`BPF_PROG_TEST_RUN`) and verify the packet was returned with the expected rewrite applied. If not, demote to `Skb` and re-record.
2. Maintain a known-bad-NIC list (driver name + firmware version) keyed off `/sys/class/net/<iface>/device/driver` and refuse `Drv` attach on those (loud WARN at startup with the bug-id link).
3. Add `xdp_attach_probe_failed_total` counter that fires when the post-attach probe finds no packet return.

This is a `medium` because it's both diagnostic (lie-detector for an existing surface) and *actually* mitigates the known production failure mode. Round-7 graded it via EBPF-2-04 (Verified-Fixed); that grade did not consider the silent-drop case.
