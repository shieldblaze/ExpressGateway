# D-1 — Native ENA XDP attach: VERDICT = PASS (with documented deployment constraint)

Auditor: auditor-3 · Branch: `audit/foundation-pass` · Date: 2026-05-17
Box: c6a.2xlarge · NIC: ens5, driver `ena` (ENA device version 0.10,
controller 0.0.1), MTU 9001, qdisc mq, 8/8 combined channels.
Running kernel: **7.0.0-1004-aws** (see FINDING A-3 — outside the
declared 5.15/6.1 support window).

## Verdict

**PASS.** The shipped `lb_xdp.bin` attaches to ens5 in genuine NATIVE
driver mode (kernel `xdp` token, NOT `xdpgeneric`/`xdpoffload`), the
data path is provably live, and all host state is restored. No silent
fallback to generic. The MTU/channel reduction is a real deployment
constraint, not a defect masking — judged per R2/R6 (see Findings).

## Verbatim evidence

### Pre-state (clean)
```
2: ens5: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 9001 qdisc mq state UP mode DEFAULT group default qlen 1000
    link/ether 06:8c:12:3c:60:f7 ... numtxqueues 8 numrxqueues 8 ...
MTU=9001 COMBINED=8
bpffs: (empty)   # sudo ls /sys/fs/bpf/ -> only . and ..
```
No XDP attached (`query_xdp` prog_id=None, no `xdp` token in `ip -d link`).

### Test invocation
```
sudo -E env "PATH=$PATH" cargo test -p lb-l4-xdp --test xdp_attach_mode \
    -- --ignored --nocapture d1_native_attach
```
(privileged ignored test
`xdp_attach_mode.rs::d1_native_attach::drv_mode_attach_to_ens5_proves_live_datapath`)

### During-attach (verbatim test stdout)
```
D-1 ena-guard: lowering ens5 MTU 9001 -> 3498 for the native-attach window (restored on teardown)
D-1 ena-guard: lowering ens5 combined channels 8 -> 4 for the native-attach window (restored on teardown)
D-1 attach: mode=Drv, attempts=1, prog_id=51
D-1 kernel view: 2: ens5: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 3498 xdp qdisc mq state UP mode DEFAULT group default qlen 1000
D-1 data-path: STATS aggregate 0 -> 2 (delta 2 > 0)
D-1 teardown: ens5 bare, MTU restored to 9001, combined restored to 8, /sys/fs/bpf clean (detach_verifying ok=true)
test d1_native_attach::drv_mode_attach_to_ens5_proves_live_datapath ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out
```

Mode cross-check (independent of aya's `AttachOutcome`):
- `ip -d link show ens5` during attach contains bare ` xdp ` token,
  NOT `xdpgeneric`, NOT `xdpoffload`. aya `outcome.mode == Drv`,
  `attempts == 1` (Native ladder is `[Drv]` only — no SKB fallback;
  loader.rs:1126).
- Data path live: aggregate `STATS` PerCpuArray delta `0 -> 2` from
  the off-host SSH control session RX through the ENA driver -> XDP
  hook within the 5 s deadline.

### dmesg (proves the DRIVER path engaged, not generic)
```
[  840.075725] ena 0000:00:05.0 ens5: XDP program is set, changing the max_mtu from 9216 to 3498
```
The `ena` driver only adjusts `max_mtu` when a program is bound in
**native driver** mode. A generic/SKB attach does not touch the
driver's max_mtu. No `ena`/`xdp` ERROR lines. No EOPNOTSUPP/EINVAL.

### Post-state (fully restored)
```
2: ens5: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 9001 qdisc mq state UP ...
MTU=9001 COMBINED=8
bpffs: (empty — only . and ..)
ip route get 8.8.8.8 -> via 172.31.16.1 dev ens5 src 172.31.28.187   # SSH/egress alive
```
`detach_verifying ok=true` (RTM_GETLINK pre-check id==51, real
`Xdp::detach`, post-check prog_id==None) + the `ip link set dev ens5
xdp off` backstop + RAII teardown asserts all passed. SSH session
survived the MTU 9001->3498 and combined 8->4 window (this run
re-verified the prior-round claim live).

## Deployment constraint (NOT a masked defect — R2/R6 judged)

`lb_xdp.bin` is built WITHOUT XDP multi-buffer (frags). The in-tree
`ena` driver therefore refuses native attach unless, for the attach
window: MTU <= 3498 AND combined channels <= max/2. ens5 production
default is MTU 9001 / 8 channels. This is a hard deployment
requirement (documented in the prior round-9 doc, DEPLOYMENT.md,
RUNBOOK.md). It is a CONSTRAINT, not a defect: native XDP genuinely
works once the precondition is met, and the failure mode is a LOUD
kernel reject, never a silent generic downgrade. The correct
long-term fix (rebuild ELF with XDP frags) remains the deferred
follow-up — see FINDING A-2.
