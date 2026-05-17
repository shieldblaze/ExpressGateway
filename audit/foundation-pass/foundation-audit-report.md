# FOUNDATION AUDIT + HARDENING — Final Report

Branch: `audit/foundation-pass` (off `feature/h3-quic-s3`) · final HEAD `39e7ebe9`
Box: c6a.2xlarge, 8 cores, ENA on ens5 · all work pushed to origin.
Companion docs: `findings.md` (consolidated), `findings-auditor-{1..4}.md`,
`baseline/RESULT.md`, `phase3-final-v2/RESULT.md`, `ESCALATION-F-ESC-1.md`,
`d1-native-attach-result.md`, `STANDING-RULES.md`.

## VERDICT: **FOUNDATION VERIFIED**
R1 baseline literally green AND every finding fixed-and-verified or
proven-and-escalated per R6. Zero findings in limbo. No finding
asterisked (R4). One tracked open R7(a) escalation (F-ESC-1 residual),
independent of R1.

## R1 baseline (final, HEAD c0a12097, verified by verifier-final2 on a
proven-uncontended 8-core box)
| Gate | Result |
|---|---|
| `cargo fmt --check` | CLEAN (exit 0) |
| `cargo clippy --all-targets --all-features -- -D warnings` | CLEAN (exit 0, 0 warnings) |
| `cargo test --workspace --all-features -- --test-threads=8` | **5/5 PASS** deterministic (R1 min is 3; ran 5 for the two low-rate items) |
`rapid_reset_goaway` and `test_sigterm_drains_h2_with_goaway` green in
all 5 runs. Pre-fix the baseline was red (BL-1 1/3; then F-SEC-1 1/3
and F-COR-9 1/19 surfaced under true full-workspace 8-core load — each
proven a REAL DEFECT per R2, none environmental).

## D-1 native ENA XDP attach (auditor-3, re-verified)
**PASS.** aya `mode=Drv, attempts=1`, no SKB fallback; `ip -d link show
ens5` bare `xdp` (not xdpgeneric/xdpoffload); live datapath proven;
full state restore (MTU 9001, 8/8ch, /sys/fs/bpf clean); SSH/egress
survived. No silent fallback. Evidence: `d1-native-attach-result.md`.

## Findings & resolutions (author != verifier on every fix, R5)
| ID | Tier | Author | Verifier | Resolution |
|---|---|---|---|---|
| F-SEC-1 rapid-reset GOAWAY lost | SECURITY | builder-1 | verifier-final2 | FIXED `77fa3879` (FIN-first + bounded post-FIN drain; mechanism revised w/ evidence per R2); wire test green 5/5 + 16/16 self-proof |
| F-COR-1 H2 validate-vs-forward + no-pseudo-trailers (H2&H3) | CONFORMANCE | builder-1 | verifier(6b2f8d84) | FIXED `9e41d07f` (buffer-then-forward, 64MiB cap→413; RFC9113§8.1/RFC9114§4.3 reject) |
| F-COR-2 / BL-1 unsound counter bracket | CORRECTNESS | builder-2 | verifier(6b2f8d84) | FIXED `469052ec` (unsound assertion removed, sound checks retained+added; product proven correct) |
| F-COR-3 incomplete temp-dir race fix | CORRECTNESS | builder-2 | verifier(6b2f8d84) | FIXED `20e22560` |
| F-COR-4 FinOnly clean_eof hole | CORRECTNESS | builder-2 | verifier(6b2f8d84) | FIXED `05d801c1` (classify_close + BodyCompleteNoClose + neg reg) |
| F-COR-5 lb-h3 dead proptest gate | CORRECTNESS | builder-2 | verifier(6b2f8d84) | FIXED `366be028` |
| F-COR-6 stale-false S2 #[ignore] | CORRECTNESS | builder-1 | verifier(6b2f8d84) | FIXED `7770de99` |
| F-COR-7 ENA blocklist fail-open | CORR/sec-adj | builder-2 | verifier(6b2f8d84) | FIXED `a7b6aacf` (driver+kernel key per lead D1 redirect — no fleet regression, D-1-consistent) |
| F-COR-8 h3_graceful_close nonce | LOW/latent | builder-2 | verifier(6b2f8d84) | FIXED `e42657f6` |
| F-COR-9 reload_zero_drop TLS flake | CORRECTNESS | builder-3 | verifier-final2 | FIXED `c0a12097` (TLS+ALPN-faithful readiness gate; TOCTOU proven per R2); green 5/5 + 30/30 self-proof |
| F-DOC-1 kernel-support doc drift | LOW/doc | builder-2 | verifier(6b2f8d84) | FIXED `3e23ba9e`; 7.x-matrix sub-part = open R7 item |
| F-ESC-1 verifier-log placeholders (7.0) | MEDIUM | builder-2 | verifier(6b2f8d84) | FIXED `59946e21` (real kernel-7.0 baseline captured) |
| R1 hygiene (fmt+clippy in builder code) | gate | builder-hygiene | verifier-final2 | FIXED `9ca82196` (no logic change) |

## Escalated (R6/R7(a)) — tracked open, NOT asterisked
**F-ESC-1 residual: 5.15/6.1/6.6 multi-kernel verifier CI lane.**
Proven infeasible in this environment (no /dev/kvm, no nested-virt, no
lvh/qemu/vng, verify-xdp.sh image digests are empty placeholders) —
mechanism captured verbatim. 7.0 baseline fixed this session. Residual
is a ~0.5d CI-infra workstream on a VM-capable runner. Recommendation:
option A (pin real digests + privileged matrix on a KVM runner);
interim posture B (truthful matrix docs) already shipped via F-DOC-1.
Independent of R1 and the FOUNDATION VERIFIED verdict. Full detail:
`ESCALATION-F-ESC-1.md`.

## Documented constraint — not a defect (D-1 PASSED)
**N-1:** native XDP impossible at jumbo MTU 9001 (lb_xdp.bin built w/o
xdp.frags); loud kernel-reject, documented deployment requirement +
known long-term follow-up (rebuild w/ xdp.frags, ~1d). Recorded, not
an open fix.

## Process integrity
- Plan-approval before every source change (R5); lead wrote no source.
- Author != verifier on every fix and the final gate (R5).
- Every failure classified ONLY on a proven mechanism from captured
  output (R2); isolation-pass never accepted as "environmental".
- Two false "VERIFIED-FIXED" claims (F-SEC-1 proxy verification; the
  fmt/clippy "clean" checkpoints) were caught by the independent final
  gate and corrected — the audit's intended function.
- Pre-existing defects fixed regardless of provenance (R3); nothing
  asterisked out of the gate (R4).

**One-line verdict: FOUNDATION VERIFIED.**
