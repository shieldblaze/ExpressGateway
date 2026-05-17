# R7(a) ESCALATION — F-ESC-1 residual: multi-kernel verifier CI lane

Status: 7.0 FIXED this session (real baseline captured, commit
59946e21). The 5.15/6.1/6.6 residual is escalated per R6/R7(a) — NOT
asterisked (R4); tracked as an open escalation in the final report.

## Proven mechanism (why it cannot be fixed in this environment)
Verbatim evidence: audit/foundation-pass/messages/builder-2-to-lead-fesc1.md
1. `scripts/verify-xdp.sh` pinned lvh-image digests are literal empty
   `IMAGE_PIN_DIGEST=""` placeholders → script exits 3.
2. The floating lvh image is a bootable kernel image with no bash/
   bpftool; `docker run` of it shares the HOST 7.0 kernel
   (`uname -r` inside == 7.0.0-1004-aws), so it cannot exercise
   5.15/6.1/6.6.
3. No `lvh`, no `qemu-system-x86_64`, no `virtme-ng`/`vng` on the box;
   `/dev/kvm` absent, zero `vmx|svm` CPU flags
   (`systemd-detect-virt`=amazon, unaccelerated nested guest) — a
   second kernel cannot be booted here at all.
4. Even with a booted kernel, `bpftool prog load` on the shipped ELF
   fails (`libbpf: legacy map definitions ... not supported by libbpf
   v1.0+`); only the aya userspace loader loads it.

This is a CI-infrastructure gap, not a code defect. The shipped eBPF
is proven to load on a real running kernel (7.0, verified_insns=9284,
tag 0x72c34ab7e4f44914) via the production aya path.

## Options
A. Pin real lvh-image digests in verify-xdp.sh + fix its sh/bash
   portability + run the privileged verifier matrix on a VM-capable
   (KVM/nested-virt) CI runner for 5.15/6.1/6.6. Effort ~0.5 day.
B. Reduce the official supported/validated matrix to "kernels with a
   real captured baseline" (currently 7.0) + keep 5.15/6.1/6.6 as
   "claimed, baseline-pending" until A lands. (Honest scoping; this is
   the F-DOC-1 doc change already shipped this session, 3e23ba9e.)
C. Drop 5.15/6.1/6.6 from the support claim entirely (product
   reduction). Not recommended — the ELF has no kernel-version-fragile
   constructs (no ringbuf/kfuncs; CO-RE), effective floor ~5.15.

## Recommendation
A, executed on a VM-capable runner in a follow-up CI-infra workstream
(~0.5d), tracked as an open escalation. This environment physically
cannot host it (no nested virt). Interim posture: B is already in
place via the F-DOC-1 doc alignment — the support window is now
truthfully documented and 7.0 has a real baseline; nothing is
asterisked. This does NOT block the FOUNDATION VERIFIED verdict
because R1 (cargo test/clippy/fmt) and the D-1 native attach are
independent of the multi-kernel verifier-log matrix; F-ESC-1 is a
verification-infrastructure escalation, dispositioned per R6 (7.0
fixed) + R7(a) (residual escalated with mechanism+effort).
