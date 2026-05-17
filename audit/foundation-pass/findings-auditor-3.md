# findings-auditor-3 — XDP/eBPF + L4 datapath (Phase 1)

D-1 NATIVE ATTACH: **PASS**. Full verbatim evidence in
audit/foundation-pass/d1-native-attach-result.md (commit 82fe5101).
aya outcome.mode==Drv, attempts==1, no SKB fallback; `ip -d link show
ens5` bare `xdp` token (not xdpgeneric/xdpoffload); data path live
(STATS 0->2 from off-host SSH RX); full state restore (MTU 9001,
8/8ch, /sys/fs/bpf clean, detach_verifying ok=true); SSH/egress
survived the MTU/channel window. No silent fallback.

Gate posture re-verified --all-features: `cargo test -p lb-l4-xdp
--all-features` all non-privileged pass (47+ tests/24 binaries),
clippy clean, fmt clean, privileged D-1 PASS. No flakes on surface.

## A-1 — verifier-matrix logs are placeholders, never captured a real baseline
**Tier: MEDIUM. Disposition: candidate fix-or-escalate.**
audit/ebpf/verifier-logs/{5.15,6.1,6.6}.log.committed all still carry
`HARNESS-CAPTURED-PENDING-CI-RERUN`; last touched ab3e37a3, never
refreshed. The declared multi-kernel verifier validation has never run
against the shipped ELF on any supported kernel. Only real evidence is
the live aya load on kernel 7.0 (D-1). Mechanism captured (placeholder
files), pre-existing (R3). Real-kernel verifier log on 7.0 capturable
on this box now; 5.15/6.1/6.6 need privileged lvh/Docker (~0.5 day).

## A-2 — native XDP impossible at jumbo MTU 9001 (no xdp.frags in lb_xdp.bin)
**Tier: documented DEPLOYMENT CONSTRAINT, not a defect (D-1 PASSED).**
Re-verified live, same as round-9. Loud kernel-reject failure mode.
Long-term fix = rebuild ELF with xdp.frags (~1 day), owned by ebpf/
div-l4. Per the d1-native-xdp-constraint doc this is an accepted,
documented follow-up — not a masked gate defect. Recorded, not an
open fix-finding.

## A-3 — kernel support-window doc inconsistency
**Tier: LOW. Disposition: tractable doc fix this session; one sub-part
is an R7 product call.** DEPLOYMENT.md §36 says 5.15/6.1; verifier
README + CI add 6.6; audit box runs 7.0 (outside both). ringbuf NOT
used, no kfuncs, BTF/CO-RE load on 7.0; effective floor ~5.15. Doc
alignment is fixable; "is 7.x officially supported / extend matrix"
is a product decision → R7 escalate that sub-part only.

## A-4 — ENA driver-support blocklist is DEAD / fail-open on real AWS ENA
**Tier: MEDIUM, SECURITY-adjacent (defense layer inert). Disposition:
FIX this session + regression test (R6 correctness/security, ~0.5d).**
Proven mechanism: `ethtool -i ens5` returns empty `firmware-version:`
→ `parse_ethtool_firmware` returns None → `firmware_of` returns Err →
`drv_supported` fail-opens to Allowed. The ROUND8-L4-05 ENA blocklist
row can never fire on the exact platform it protects; its unit test
passes only by calling classify() directly, bypassing the dead
resolution path. Runtime BPF_PROG_TEST_RUN backstop also inert (aya
0.13.1 API → ProbeUnavailable), so on a bad ENA fw/kernel combo BOTH
defense layers are non-functional. Not a D-1 blocker (7.0+ENA not
silent-dropping here). Fix: key ENA blocklist on driver+kernel or
fail-closed on empty firmware for fragile drivers + real
drv_supported("ens5") integration test.

## Verified-correct (mechanism proven, no defect)
ptr_at checked_add bounds (IPv6 ext-hdr off re-bounded each iter);
LRU_HASH conntrack + per-CPU is_under_flood cap + RST/FIN-ACK prune;
single-syscall atomic backend publish + daisy-chain; panic-free
netlink parser; attach_replacing/detach_verifying real RTM_GETLINK
ownership checks (detach→attach race window documented+bounded);
bpffs statfs check; GPL assert. Tail calls/prog-array: NONE (N/A).
AF_XDP zero-copy: NOT used (N/A).

## Tooling note
bpftool/ip-link cannot load the shipped ELF (libbpf rejects legacy
map definitions; iproute2 6.19.0). Production path is aya-only and
works (D-1 proved it). Contributes to A-1 (no local verifier baseline
without the privileged lvh lane).
