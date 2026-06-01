# S26 PLAN — land CF-S22-QUICHE-H3-MIGRATION (lead-approved design)

Base: `feature/quiche-h3-migration-s23` @ `0fdbc9b5`. main stays `90915781`.

## Phase 0 reference (captured)
- Base tip confirmed `0fdbc9b5`; tree clean; 31 GB free; 14 GB RAM; no stray procs.
- S25 behavioral reference (R3 anchor): full-workspace ×3 = **1438/0** each, deterministic.
  Per-suite: h3h3 26/26, h1h3+h2h3 25/25, proto 5/5, lib 84/84.
- lb_h3 production references = **doc-comment prose only** in
  `lb-quic/src/{h3_bridge,h3_config,public_header}.rs` (NO `use`); production is genuinely
  lb_h3-free. These comments get cleaned up with the crate deletion (rustdoc intra-doc links).
- lb_h3 dev-dep declared by: root `lb-integration-tests`, `lb-quic`, `fuzz`.
- lb_h3 public surface used by harnesses: `H3Frame`, `encode_frame`/`decode_frame`/
  `DEFAULT_MAX_PAYLOAD_SIZE`, `QpackEncoder`/`QpackDecoder`, `QpackBombDetector`,
  `encode_varint`/`decode_varint`/`MAX_VARINT`, `H3Error{Incomplete,...}`.

## Key finding driving the design (QPACK direction)
The harnesses ALREADY decode the migrated gateway's (Huffman) headers with
`quiche::h3::qpack::Decoder` (S24/S25 migrated that direction; lb_h3's decoder is raw-only).
What still rides lb_h3:
1. **encode-to-gateway** (`QpackEncoder`, raw static-table — quiche decodes it fine),
2. the self-contained codec round-trip / proptest / fuzz tests (`QpackEncoder`↔`QpackDecoder`),
3. all frame+varint hand-crafting (`encode_frame`/`decode_frame`/varints).

Switching QPACK encode to `quiche::h3::qpack::Encoder` (the handoff's "lighter" option) would
**change the crafted wire bytes (Huffman) at every call site** → an R3 intent-drift risk and
breaks the round-trip encoder↔decoder pairing.

## APPROVED DESIGN — wholesale relocate (R3-safest, byte-identical by construction)
Create one shared **test-only** crate `crates/lb-h3-testcodec` containing lb_h3's modules
**moved unchanged**: `frame.rs`, `varint.rs`, `error.rs`, `qpack.rs`, `security.rs` (~611 LOC).
- Same public surface, same bytes → byte-identity is *by construction* (a literal code move),
  provable by `diff` + the surviving round-trip/proptest/fuzz tests.
- Harness rewrite = pure path change `use lb_h3::{…}` → `use lb_h3_testcodec::{…}`. The
  existing `quiche::h3::qpack::Decoder` call sites are untouched.
- Crate is **test-only**: NOT a `[workspace] members`-linked production dep; added only as a
  dev-dependency (root + lb-quic) and a normal dep of `fuzz`. Production links nothing.
- This is NOT "lb-h3 renamed": lb-h3 (the production-shaped lib + its lib.rs/tests) is DELETED.
  The ~611 LOC are RETAINED as test infra (reported honestly as retained, not "deleted").
  Honest LOC headline = production deletion (~2461 S25 + the crate's production linkage).

## Increments
- **INC-A (test-eng):** create `crates/lb-h3-testcodec` (move the 5 modules verbatim; new
  lib.rs re-exporting the same surface; test-only lints). Add dev-dep to root + lb-quic, dep to
  fuzz. Do NOT yet delete lb-h3. Commit+push. Verifier proves byte-identity (diff + round-trip).
- **INC-B (test-eng):** re-point every harness `use lb_h3` → `use lb_h3_testcodec`
  (15 files + relocate `codec_roundtrip_h3`/`proptest_qpack` semantics + fuzz target). NO
  assertion/byte changes. Commit+push. Verifier confirms each harness's pass/catch is identical
  to the Phase-0 reference.
- **INC-C (migration-eng):** once `grep -rn 'lb_h3\|lb-h3' --include=*.rs --include=*.toml`
  (non-comment) is empty, delete `crates/lb-h3`, the `members` entry, the two dev-deps, fix the
  fuzz dep, and clean the 3 src/ doc-comment refs. Clean `cargo build --workspace --all-features`.
- **INC-D (migration-eng):** NEW H3-terminate scenario in `crates/lb-soak`.
- **Phase 3 (lead+verifiers):** ×3 gate; FRESH h3spec (headline: #16-21,#23-25 PASS by
  construction; #11-15,#22 still pass; Huffman confirmed); re-soak incl. H3-terminate; R8 E1+E2
  + R13 F-MD-4 E1+E2 re-confirm; scoped llvm-cov ≥80%; clippy/fmt. PROMOTE iff all green (R11).

## Execution model (lead decision, R7 industry-safe default)
Single shared working tree + shared `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` (NOT
per-agent worktrees: 3-4×19 GB targets would ENOSPC the 31 GB FS, and divergent worktree
source paths churn cargo fingerprints). Isolation enforced by task sequencing: exactly one
builder edits at a time; same-file work serialized; verifier reads COMMITTED state only and
never edits. Author != verifier on every increment (R5). Two independent verifiers on the
byte-identity proof and the fresh-h3spec headline (R5/R15).
