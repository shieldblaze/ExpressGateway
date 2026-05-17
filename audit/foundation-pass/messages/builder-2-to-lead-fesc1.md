# F-ESC-1 — 5.15 / 6.1 / 6.6 multi-kernel verifier baseline: IN-ENVIRONMENT INFEASIBLE (verbatim mechanism for lead R6/R7(a) escalation)

builder-2. The kernel-**7.0** real baseline WAS captured this session
(`audit/ebpf/verifier-logs/7.0.log.committed`, real aya
`BPF_PROG_LOAD` + `ProgramInfo` + `bpftool prog show id <id>` —
verified_insns=9284, xlated=12800, jited=7264, tag
0x72c34ab7e4f44914, prog loaded for real). NOT escalated, NOT
asterisked (R4).

The 5.15 / 6.1 / 6.6 baselines genuinely **cannot** be captured in
THIS environment. Mechanism, captured verbatim:

## 1. No pinned lvh-images digests (verify-xdp.sh self-reports)

`scripts/verify-xdp.sh` lines 109-120: every kernel's
`IMAGE_PIN_DIGEST=""` (literal empty placeholder, with the in-script
comment `# Placeholder — replace with quay.io/.../kernel-images@sha256:...
after first CI green run`).

Verbatim, `bash scripts/verify-xdp.sh --kernel 5.15`:
```
verify-xdp.sh: FATAL: no pinned digest for kernel 5.15
                   set EG_ALLOW_FLOATING_IMAGE=1 to use the floating
                   quay.io/lvh-images/kernel-images:5.15-main tag (NOT reproducible)
EXIT=3
```

## 2. Floating tag pulls but is not runnable by the script

Verbatim, `EG_ALLOW_FLOATING_IMAGE=1 bash scripts/verify-xdp.sh --kernel 5.15`
(tail):
```
verify-xdp.sh: kernel 5.15; loading lb_xdp.bin via lvh
Unable to find image 'quay.io/lvh-images/kernel-images:5.15-main' locally
5.15-main: Pulling from lvh-images/kernel-images
...
Digest: sha256:c24903a7e9ca7c701c9dfeb7e67615c5c14edeca7f3f38c70d5758a19c392ed1
Status: Downloaded newer image for quay.io/lvh-images/kernel-images:5.15-main
docker: Error response from daemon: failed to create task for container: failed
to create shim task: OCI runtime create failed: runc create failed: unable to
start container process: error during container init: exec: "bash": executable
file not found in $PATH
```
The lvh-images artefact is a **bootable kernel+rootfs image meant to
be launched as a VM by the `lvh` (little-vm-helper) binary**, not a
userspace container. `verify-xdp.sh` hardcodes `docker run ... bash -c`
but the image has **no `bash`** (only `/bin/sh`) and **no `bpftool`**:
```
docker run --rm --entrypoint /bin/sh quay.io/lvh-images/kernel-images:5.15-main \
  -c 'which bpftool || echo NO_BPFTOOL_IN_IMAGE; uname -r'
NO_BPFTOOL_IN_IMAGE
7.0.0-1004-aws        <-- the container reuses the HOST kernel, NOT 5.15
```
Critically: a plain `docker run` of a kernel-image container shares the
**host** kernel (uname inside = `7.0.0-1004-aws`). It does NOT give a
5.15/6.1/6.6 verifier — that requires actually BOOTING the image's
kernel in a VM.

## 3. No VM-boot tooling and no nested virtualization

- `lvh` / `little-vm-helper`: **not installed** (`command -v lvh` →
  not found; not in /usr/local/bin, /usr/bin, ~/go/bin).
- `qemu-system-x86_64`: **not installed** (`/usr/bin/qemu*` absent;
  apt `qemu-system-x86` → `Installed: (none)`).
- `virtme-ng` / `vng`: **not installed**.
- `/dev/kvm`: **absent**; `grep -c 'vmx|svm' /proc/cpuinfo` → `0`;
  `systemd-detect-virt` → `amazon`. This box is itself a c6a.2xlarge
  guest with **no nested KVM**, so even installing qemu/lvh could not
  hardware-accelerate (or, without /dev/kvm, practically boot) a
  second kernel for the matrix.

## 4. The ELF cannot be bpftool-loaded anyway (auditor-3 tooling note, re-confirmed verbatim)

```
sudo bpftool prog load .../lb_xdp.bin /sys/fs/bpf/fesc1probe type xdp
libbpf: elf: legacy map definitions in 'maps' section are not supported by libbpf v1.0+
Error: failed to open object file
```
So even with a booted 5.15/6.1/6.6 kernel, the script's
`bpftool prog load` step would fail — only the aya loader can load
this legacy-map ELF, and aya-on-host pins us to the host kernel (7.0,
already captured).

## Disposition (proposed — lead owns the formal escalation)

7.0: **REAL-CAPTURED this session, fixed, not escalated, not
asterisked.** 5.15/6.1/6.6: residual **R6/R7(a) CI-infra workstream** —
genuinely infeasible in this environment (no lvh/qemu/vng/KVM; image
digests are literal placeholders). Scoped fix (~0.5 day): pin the real
lvh-images digests, fix `verify-xdp.sh` to use `/bin/sh` (or add bash
to the image), and wire the privileged multi-kernel CI matrix stage on
a runner that can boot lvh VMs. Recommend lead formally escalate this
residual lane to the owner with the above captured mechanism + the
~0.5d estimate. Nothing asterisked (R4).
