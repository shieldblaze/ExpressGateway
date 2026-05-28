# SESSION 15 — Owner rulings on Phase 1 design

**Date:** 2026-05-28
**Phase:** 1 → 2 boundary
**Reference:** `audit/quic/s15-design.md` §9 (open items) + design body.
**Binding for Phase 2 build (Mode A passthrough).**

## §9 item 1 — Both-mode architecture
**RULING: CONFIRM.** Proceed to Phase 2. SHARED-1 (quiche-free
public-header parser) + SHARED-2 (UDP datapath trait) are correct;
the quiche-free parser separation is exactly what keeps the
no-decrypt property honest; R12 single-sourcing for S16 reuse
is correct. The short-header DCID-length recovery contract
demonstrates the hard part was engaged.

## §9 item 3 — `min_client_dcid_len` floor
**RULING: TAKE DEFAULT 8.** Uncontroversial; consistent with
quiche+quinn+mvfst client behavior.

## §9 item 4 — `max_quic_connections` default
**RULING: TAKE DEFAULT 100_000.** Matches termination limit
(`router::RouterParams::max_connections`). Consistent operator
budget across both modes.

## §9 item — XDP / io_uring deferral
**RULING: ENDORSE.** v1 = tier-3 tokio-UDP only; io_uring tier-2
→ v1.1; XDP DCID-steering tier-1 → v1.2. Correctness before
performance; tier-3 passthrough IS correct passthrough (routes
by CID, preserves no-decrypt, bounds state). XDP is a perf tier,
not a correctness gap. The SHARED-2 datapath trait makes XDP a
non-disruptive later add. A new ~300-LOC eBPF program deserves
its own dedicated session with a multi-kernel verifier matrix —
tie to F-ESC-1 — NOT a budget-pressured tail of S15.

**ADDENDA (binding in this session):**
1. **SHARED-2 trait contract MUST be documented in `s15-design.md`
   precisely enough that v1.1 io_uring and v1.2 XDP implement
   against a STABLE seam — adding a tier must not require
   changing passthrough logic.** Nail this contract this session
   if not already. Cheap and it is what makes the deferral safe.
2. **v1's tier-3-only status documented as an operator-facing
   characteristic** in `DEPLOYMENT.md` (and/or RUNBOOK.md):
   passthrough works on any kernel via userspace UDP; XDP /
   io_uring fast paths are later performance tiers. Operator-
   facing, not just audit-trail.

## §9 item 1 (clarified) — `strict_source_binding` default
**RULING: PER-POOL CONFIG KNOB, DEFAULT FALSE.**

**Reasoning (owner verbatim):** "The failure modes are
asymmetric. false costs 'backend burns some AEAD CPU on off-path
spoofed packets it then drops' — bounded, and QUIC AEAD auth is
the real cryptographic defense. true costs 'legitimate mobile
clients lose connections during network handoff' — silent
availability failure for real users. Breaking real users by
default is worse than spending bounded CPU on attacker garbage by
default." Per-pool because exposure varies within one LB.

**REQUIREMENTS (binding):**
1. **v1 verify gate runs the DEFAULT (false)** — NAT-rebind
   path-migration test verifies the shipped-default behavior.
2. **Verify case for `strict_source_binding=true` on at least
   one pool** — spoofed-source short-header packet is DROPPED at
   the LB (never reaches backend). Prove the knob works in BOTH
   positions; an untested config option is worse than no option.
3. **Document both modes in operator docs** — security/mobility
   tradeoff, the default, when to flip it.

## §9 item 2 — Retry without quiche, ship in v1?
**RULING: SHIP IN v1** (per quic-eng's own recommendation in §9.2).
Retry is the primary Initial-flood defence; skipping it creates a
v1-only DoS surface that violates R8 (bounded state). ~80 LOC +
RFC 9001 §5.8 differential test against `quiche::retry` is in
scope for Increment A2.

## §9 item 5 — NEVER-DECRYPTED proof mechanism
**RULING: Construction over observation. Three-part proof:**

**PRIMARY (load-bearing, by-construction):**
1. **Linkage:** `cargo bloat -p lb-quic --filter quiche` shows
   ZERO `quiche::Connection` / BoringSSL decrypt symbols on the
   Mode A passthrough path, with
   `cfg(not(feature = "quic-passthrough-only"))` guards around
   any `quiche::Connection` / crypto use. Proves decryption
   machinery is not linked onto the path — structural, static,
   readable.
2. **State:** verifier code-read PROVING the passthrough flow
   state (`FlowEntry` / connection-tracking struct) holds NO key
   material and NO key-derived data — only connection ID +
   routing state. Make this an EXPLICIT assertion in the report,
   type-level if the structs allow it. This proves "the LB
   cannot decrypt" most directly: it never possesses the keys.

**CROSS-CHECK (cheap runtime supplement, NOT load-bearing):**
3. kprobe / bpftrace on `openat` during the real-wire Mode A test
   confirms the LB process opens no cert/key file. Catches a
   regression where crypto is accidentally wired onto the path.

**DROP seccomp.** "Decryption was blocked at runtime" is weaker
than "decryption is impossible because the capability and the
keys are absent." A seccomp filter forbidding a capability that
structurally doesn't exist is belt-on-belt with high fragility
and maintenance cost.

The distinction that matters: **prove the LB CANNOT decrypt (no
keys, no linked crypto), not merely that it DIDN'T decrypt in one
run.** Construction over observation.

---

All five §9 items now resolved. Phase 2 build cleared.
