### ROUND8-L4-05 — Drv→Skb attach fallback never runtime-probes XDP_TX (aya #1193 / MLX5/CX6 silent-drop)

Reference: `audit/round-8/research/aya.md` lesson 6 + handoff item 5; `audit/round-8/research/cilium.md` lesson 8 (ConnectX-4 Lx VF silent fail)
Our equivalent: `crates/lb-l4-xdp/src/loader.rs:655-710` (`attach_with_fallback`); `crates/lb/src/xdp.rs:240-288` (`attach_with_elf`)

Severity: high (lead-arbitrated up from medium per cross-review.md)
Status:   Proposed-Fix — `crates/lb-l4-xdp/src/nic_compat.rs` (NEW):
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
