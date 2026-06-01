# SESSION 26 HANDOFF — finish CF-S22-QUICHE-H3-MIGRATION (delete lb-h3 crate + Phase-3 + PROMOTE)

**Branch:** `feature/quiche-h3-migration-s23` (continue it — do NOT re-branch from main).
**Base tip (S25 PARTIAL):** see `git log` — INC-5b `5174a018` + report `b736bfd5` + the
INC-5 ×3 gate confirmation commit. **main stays** `90915781` (S22-hardened) until PROMOTE.

## STATE AT S26 START (S25 landed)
- **H3 data path FULLY on `quiche::h3`** — E1 server front (S24) + E2 upstream client (S25);
  all three →H3 cells (H1/H2/H3→H3) ride `quiche::h3::Connection`. 2 verifiers AGREE; R8
  (both dirs), F-MD-4 mirror (single-shot + R13 burst), backpressure, byte-identity green.
- **PRODUCTION is lb_h3-FREE** — inline-400 on the decoded egress, legacy raw-byte path
  deleted, ~2461 LOC dead `lb_h3` framing deleted, `lb-h3` is a **dev-dependency** only.
- The quiche-0.28 §7.1 frame-completeness gap: CL truncation guard (mutation-proven) +
  re-scoped no-CL test + `CF-QUICHE-FRAME-COMPLETENESS` carry-forward.
- Test-harness GREASE (RFC 9114 §9) conformance fixed in 3 H3 backends (proto,
  quic_listener, + the md-verify/h3_h3 backends already tolerant).

## S26 WORK (in order)

### 1. Rewrite the ~20 hand-rolled wire-test harnesses off `lb_h3`'s frame codec
`crates/lb-quic/src` is lb_h3-free; ONLY tests still use `lb_h3` (dev-dep). They hand-build
/parse H3 DATA/HEADERS frames + varints + QPACK header blocks. quiche exposes
`quiche::h3::qpack::{Encoder,Decoder}` (public) but **NO standalone H3 frame codec**.
**RECOMMENDED (lighter):** add a shared test-support module
`crates/lb-quic/tests/common/h3_test_codec.rs` (or a tiny path crate) holding the
frame+varint codec MOVED from `lb_h3` (`frame.rs` ~329 + `varint.rs` ~151 ≈ 480 LOC) +
re-export `quiche::h3::qpack` for QPACK. Then re-point each test's `use lb_h3::{…}` to the
support module / `quiche::h3::qpack`. Files (grep `lb_h3`):
`crates/lb-quic/tests/{h3_h1_bridge,h3_h1_stream_body,h3_h1_stream_body_errors,
h3_h1_resp_stream,h3_h1_trailers_resp,h3_h2_stream,h3_h3_stream,round8_h3_authority}_e2e.rs`,
`tests/{quic_listener,proto_translation,codec_roundtrip_h3,h1h3_md_streaming_verify,
h2h3_md_streaming_verify}.rs`, `crates/lb-h3/tests/proptest_qpack.rs`,
`fuzz/fuzz_targets/h3_frame.rs`, `tests/security_qpack_bomb.rs` (`QpackBombDetector`).
Keep behaviour assertions bit-identical (the GREASE-skip fixes already landed).
**Heavier alt (only if interop fidelity demands):** rewrite each hand-rolled client/backend
as a real `quiche::h3::Connection` endpoint. Not recommended for the bulk.

### 2. Delete the `crates/lb-h3` crate
After (1), `grep -rn 'lb_h3\|lb-h3' --include=*.rs --include=*.toml` (non-comment) is empty
⇒ delete `crates/lb-h3`, drop the `[dev-dependencies] lb-h3` + the `members` entry +
`fuzz/` target. Confirm `cargo build --workspace --all-features` + the gate. THIS is the
final "~1.15k LOC actually deleted" for R11 (combined with the ~2.5k S25 production delete).

### 3. Phase-3 full re-validation (the migration's external proof)
- **R1 ×3** full workspace deterministic (completed runs only, R15). Watch ENOSPC: keep the
  shared `eg-target` under ~40G; `cargo clean` if it accumulates (S25 hit a 100%-disk
  ENOSPC link-fail mid-gate — the `gate.sh` redirect was also fixed to `>> LOG 2>&1`).
- **R8** E1 egress (carried) + E2 both directions — non-vacuous, body-size-independent.
- **R13** F-MD-4 RST mapping on BOTH E1 + E2 (bursts + negative controls) — present, re-run.
- **FRESH h3spec** run: prove the 9 carried findings #16-21,#23-25 now PASS BY CONSTRUCTION
  (quiche owns the control/QPACK/frame-seq closes via `conn.close(true,…)`); #11-15,#22 still
  pass; no new failures. THE HEADLINE — the on-paper S23/S24 assumption proven by external
  reference. Confirm CF-S22-QPACK-HUFFMAN (quiche Huffman-encodes; lb_h3 was raw-only).
- **RE-SOAK** with a NEW H3-terminate scenario in `crates/lb-soak` (it covers h1/h2/ModeA/
  ModeB but NOT H3-terminate — and this workstream re-pointed exactly that path): bounded
  RSS/fd/state over the run, panic=0, F-MD-4 paths exercised under sustained load.
- scoped llvm-cov ≥80% on the migrated E2 surface.

### 4. PROMOTE (`--no-ff` to main) — iff ALL of §2-§3 are green from COMPLETED runs.
Honest message: H3-front fully migrated to quiche::h3, N findings closed by construction,
~Xk LOC deleted, Huffman gained; remaining for full spec = WS-over-H2(RFC 8441)/H3(RFC 9220)
+ gRPC-over-H3 (program-level, on quiche::h3, ~2 sessions). Else PARTIAL again.

## CARRY-FORWARDS
- `CF-QUICHE-FRAME-COMPLETENESS` (re-tighten the no-CL truncation test when quiche enforces
  RFC 9114 §7.1; tied to `CF-QUICHE-UPGRADE`). Documented as a known quiche-0.28 limitation
  alongside #1-10 + a v1 release-note item.
- `CF-S22-QPACK-HUFFMAN` closes on the fresh-h3spec confirmation (Huffman gained in prod).
- prior program carry-forwards: CF-FCAP1-FLAKE, CF-DEP-1 (Dependabot), F-ESC-1, N-1,
  Mode-A perf tiers + CF-S15-PASSTHROUGH-RETRY-ODCID.
