# F-S7-3 — J3 VERIFICATION-GAP: round8 never exercised the live H3→H3 path

Verifier: `verifier` (independent confirmation + honest self
re-characterization of my own J3 audit). [[s2-verification-gap]]-class
recurrence.

## INDEPENDENT CONFIRMATION (read, not inferred)
`crates/lb-quic/tests/round8_h3_authority_enforced.rs:326-327`:
```
        h3_backend: None,
        h2_backend: None,
```
Both backends are `None`. In `conn_actor::poll_h3` the J3-rewired LIVE
H3→H3 branch is `if let Some((qpool, addr, sni)) = h3_backend { …
h3_to_h3_stream_resp … }`. With `h3_backend: None` that branch is
NEVER entered, so `h3_to_h3_stream_resp` is NEVER called.

round8's own module header (lines 1-23) states its actual purpose:
prove the H3 authority validator trips BEFORE upstream selection. Its
two assertions:
 1. comma-in-`:authority` ⇒ `:status 400` + ZERO backend connections
    (gate trips before upstream selection);
 2. well-formed `:authority` ⇒ reaches the **TCP probe backend**
    (`TcpPool`, lines 38/156) — i.e. the valid path runs the
    `select_backend(backends)` H1/TcpPool fallback, NOT the H3→H3
    branch.
Therefore round8 STRUCTURALLY CANNOT exercise the post-J3 live H3→H3
path. It proves the authority gate, full stop.

## WHAT WAS CLAIMED vs WHAT IS TRUE
- s6 plan §4, s7 reconfirm, J3 builder self-check AND **my own J3
  independent verifier audit** (audit/h-matrix/s7-h3h3-verify.md
  lines 356-357: "THE no-regression proof: the now-LIVE H3-backend
  actor path driving h3_to_h3_stream_resp"; line 420 similar) all
  cited `round8 3/3` as proof the LIVE post-rewire H3→H3 path
  works / "the no-regression proof for the swap".
- That claim is FALSE for the H3→H3 path. round8 cannot reach it.
  The live H3→H3 path was NEVER actually proven by any
  pre-J4 artifact; J4 (genuine real-wire) is the first exercise that
  could — and it shows the path BROKEN (F-S7-2).

## HONEST RE-CHARACTERIZATION — what J1/J2/J3 audits ACTUALLY proved
STILL VALID (unaffected by this gap):
 * J1/J2 unit tests (s7_j1_recv_half_frame_machinery,
   s7_j2_request_send_decision) — pure codec/decision-table proofs;
   sound but socket-less, never a real wire.
 * J3 token-parity: the new H3→H3 conn_actor branch IS a
   token-for-token clone of the verified H3→H2 branch with only the 3
   authorized deltas (independently re-derived) — STILL TRUE.
 * R3 no-regression for H3→H1 and H3→H2: h3_h1_*/h3_h2_stream_e2e
   green — STILL TRUE (those cells have genuine real-wire suites).
 * conn_actor/h3_bridge byte-identity & roundtrip-deletion proofs —
   STILL TRUE.

NO LONGER CLAIMED (withdrawn):
 * "round8 proves the live post-rewire H3→H3 path works / is a
   no-regression proof for the swap" — WITHDRAWN. round8 proves only
   the H3 authority gate; with h3_backend:None it cannot and does not
   exercise h3_to_h3_stream_resp. J1/J2/J3 acceptance partly rested on
   this inference, which did not hold. "H3→H3 works end-to-end" was
   UNPROVEN until J4 — which proves it BROKEN (see F-S7-2).

ROOT OF MY AUDIT ERROR: I accepted round8 as the live-path proof from
the test NAME + the s6-plan/s7-reconfirm framing, and ran it green ×3,
without READING its `h3_backend`/`h2_backend` wiring to confirm it
drives the rewired branch. Token-parity (which I did do rigorously) is
necessary but NOT sufficient: a verbatim-correct clone of a correct
template can still be reached via a code path no in-tree test
exercises, and the underlying recv-half (J1, unverified on a real
wire) was itself defective (F-S7-2).

## BINDING FORWARD RULE
Any "swap/rewire/clone no-regression" or "live path works" claim MUST
cite a test whose BACKEND WIRING HAS BEEN READ and confirmed to drive
the specific rewired branch (e.g. `h3_backend: Some(..)` for the
H3→H3 branch) — never inferred from the test name or a plan's
assertion. Token/structural parity is necessary but not sufficient;
the cited test must actually traverse the changed code path.

## STATUS
F-S7-3 CONFIRMED. Lesson persisted to verifier memory. Recorded for
the s7-report ledger. Does not by itself change J1/J2/J3's still-valid
proofs, but the H3→H3 cell is NOT BUILT (F-S7-2) and the prior
"round8 = live-path proof" citation is formally withdrawn.
