# FOUNDATION AUDIT — Standing Rules (NON-NEGOTIABLE)

Repo: /home/ubuntu/Code/ExpressGateway   Work branch: audit/foundation-pass
Base: feature/h3-quic-s3   Box: c6a.2xlarge 8 cores, ENA on ens5.

R1. BASELINE GREEN = ALL of, with captured proof:
    - `cargo test --workspace --all-features` passes deterministically,
      8-core parallelism, run 3x, all 3 pass.
    - `cargo clippy --all-targets --all-features -- -D warnings` clean.
    - `cargo fmt --check` clean.
    No test excluded, no asterisk, no "green except".

R2. FLAKE PROTOCOL. Isolation-pass is NOT proof of "environmental".
    Classify a failure ONLY from captured output proving the mechanism:
    actual error, actual contended resource, actual root cause.
    - Wrong frame/status/header/close/body = REAL DEFECT, always.
    - Port/path/global-state collision = REAL DEFECT (fix w/ unique
      ports/paths or serial marker; never "environmental").
    - ONLY a proven scheduling-starvation timeout with zero server-side
      misbehavior may be called environmental, with mechanism shown.

R3. Pre-existing defects are in scope. Provenance documented, never an
    excuse to skip.

R4. NO finding is asterisked out of the gate. Only dispositions:
    (a) fixed-and-verified this session, or
    (b) proven-and-escalated as GENUINELY LARGE per R6.

R5. PROCESS: plan-approval before ANY source change. Author != verifier
    on every fix. Never weaken/skip/#[ignore]/delete a working test to
    pass a gate. Commit every increment; push to origin/audit/foundation-pass.

R6. TIERED FIX:
    - SECURITY/CVE -> FIXED this session always + mandatory regression test.
    - CORRECTNESS/CONFORMANCE tractable -> FIXED this session + regression test.
    - GENUINELY LARGE (multi-day rework) -> prove mechanism, write scoped
      workstream + effort estimate, ESCALATE to owner. Still not asterisked.

R7. Escalate to owner ONLY for: (a) genuinely large per R6, or
    (b) a real product-behavior decision (two valid RFC behaviors).
    Do not escalate what the standing rules already answer.

Auditors: Phase 1 you WRITE ONLY to audit/foundation-pass/. No source
edits. Prove mechanisms from captured output. Document provenance via
`git log feature/h3-quic-s2..feature/h3-quic-s3 -- <file>` etc.
