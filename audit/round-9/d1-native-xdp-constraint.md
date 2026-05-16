# D-1 — Native ENA XDP attach: PASS with documented deployment constraint

Branch: `feature/h3-green` · Commit: `3fd06339` · Date: 2026-05-16
Lead verdict (recorded): **D-1 PASS** for the GO scorecard, subject to
the deployment constraint documented here and in `DEPLOYMENT.md` /
`RUNBOOK.md`.

## What was proven

`lb-l4-xdp/tests/xdp_attach_mode.rs::d1_native_attach::drv_mode_attach_to_ens5_proves_live_datapath`
(privileged, `#[ignore]`d) genuinely:

- attaches the shipped `lb_xdp.bin` to `ens5` (AWS `ena`) in **native
  mode** — `mode=Drv, attempts=1`, **no SKB fallback** (Native ladder
  only; SKB fallback would fail the test);
- cross-checks the kernel view (`ip -d link show ens5` shows bare
  `xdp`, not `xdpgeneric`/`xdpoffload`);
- proves the data path is live: STATS aggregate `0 → 34` (non-zero
  delta from the live external SSH RX path);
- restores all state via RAII teardown — ens5 bare, MTU/channels
  restored, `/sys/fs/bpf` clean, `detach_verifying ok=true`.

Independently re-verified by `verifier` (Task #7).

## The constraint (why this is not an unconditional PASS)

The shipped `lb_xdp.bin` is built **without XDP multi-buffer (frags)**.
The `ena` driver therefore refuses native attach unless, on the target
interface:

1. **MTU ≤ 3498** — else `dmesg`: `the current MTU (9001) is larger
   than the maximum allowed MTU (3498) while xdp is on`.
2. **combined channels ≤ max/2** — else `dmesg`: `the Rx/Tx channel
   count should be at most half of the maximum allowed channel count`.

`ens5` runs the VPC jumbo default (MTU 9001, 8/8 channels). The D-1
test therefore transiently lowers MTU 9001→3498 and combined 8→4 for
the attach window only and restores both on teardown (verified: SSH +
egress survive at the reduced settings; full restore confirmed even on
failing iterations).

**Implication:** native XDP on a jumbo-frame ENA NIC is impossible with
`lb_xdp` *as built*. A production native-XDP deployment must set
`MTU ≤ 3498` and `combined ≤ max/2`, OR accept `skb`/generic mode
(significant performance penalty).

## Known follow-up (NOT done this loop — explicitly deferred)

Rebuild the eBPF object with **XDP multi-buffer / frags** support so
native XDP attaches at the production jumbo MTU without NIC
reconfiguration. This removes constraint (1) entirely and is the
correct long-term fix. Until then the constraint above is a hard
deployment requirement, not a defect masked from the gate.

## Tooling note

iproute2 6.19.0 `bpftool`/`ip` cannot load the aya-emitted ELF (libbpf
rejects "legacy map definitions"); the kernel-side mode cross-check
uses `ip -d link show` token semantics rather than a `bpftool` mode
string. The aya userspace loader (production path) loads it correctly.
