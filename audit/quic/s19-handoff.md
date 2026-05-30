# SESSION 19 — handoff (post-S18)

> S18 closed **CF-S16-RELAY-STALL** — the one blocker that survived S16 + S17 — and
> **promoted** the stall-closed Mode B core relay (B1/B2/B3) to `main`. Mode B is **NOT
> complete** (B4/B5/B6 remain); native QUIC is **NOT complete**; the system is **NOT
> production ready** (no chaos/soak suite yet). S19 = **finish Mode B (B4/B5/B6) on a fresh
> budget, THEN build the chaos/soak suite.**

## What S18 landed (on `main` via the S18 promote)
- **CF-S16-RELAY-STALL CLOSED + R13-verified.** Root: a relay-side data-drop in
  `crates/lb-quic/src/raw_proxy.rs::pump_dir`. The read gate was not gated on
  `half.src_fin_seen`; after the source FIN was read (quiche *collects* the stream) a
  prior-turn short-write left an undelivered tail in `half.pending`; the next turn re-issued
  `stream_recv` on the collected stream → `Err(InvalidStreamState)` → the generic read-error
  arm ran `half.pending.clear(); half.done=true`, **dropping the tail and never forwarding the
  FIN**. Fix (`1414d656`): one condition — `while !half.src_fin_seen && half.pending.len() <
  STREAM_RELAY_WINDOW`. Single-sourced (fixes both relay legs). + deterministic regression test
  `post_fin_short_write_reread_does_not_drop_tail`.
- **CF-S17-B3-VERIFY-DONE-UNWRAP fixed** (`90ac0b8b`, test-only pump+retry).
- Diagnosis + verification evidence: `audit/quic/s18-logs/p1-diag-findings.md`,
  `p2-verifier-findings.md`, `s18-fix-design.md`; Phase 3 gate `p3-gate-summary.txt`.

### ⚠️ Mechanism note for the SOAK (record this — the user's explicit ask)
The stall was a **loss-recovery-adjacent** bug: the symptom looked like "the LB never
retransmits a lost client-leg tail" (S17 framed it that way for two sessions) but the real
cause was the relay *deleting* its own buffered tail on a spurious post-FIN re-read — so
`bytes_in_flight=0`, `loss_timer=None`, `total_pto=0` were all CORRECT downstream effects, not
the trigger. **The soak must exercise the client-leg send/FIN path under sustained load with
flow-control backpressure** (the short-write-at-FIN race is what triggered it ~11–17%
single-threaded). Watch for: client-short streams, never-forwarded FINs, `pump_dir` drop-arm
firings (`PUMP-READ-ERR InvalidStreamState`), and any stream reclaimed without its tail
delivered. A fix to a deep stream-lifecycle bug like this earns its real confidence only under
soak — keep the env-gated diag accessors pattern (S18 worktrees) handy to re-instrument if a
regression appears.

## PRIORITY 1 — finish Mode B: B4, B5, B6 (s16-plan §4; fresh budget)
Each: plan-approved before source change, real-wire verified (real QUIC client ⇄ LB ⇄ real
QUIC backend, two distinct connections — no proxy unit-test substitute), author≠verifier, R8
bounded-state proofs, R13 layering where timing-sensitive, commit+push continuously.
- **B4 — RawDatagramRelay**: bidirectional QUIC DATAGRAM relay with a **bounded** per-connection
  datagram queue + explicit **drop-newest** full-policy (R8). Real-wire test: datagrams
  round-trip client⇄LB⇄backend; prove the queue bound + drop-policy under flood.
- **B5 — bounded-state flood test**: adversarial stream/datagram flood proving per-connection +
  per-stream state stays bounded (table size cap, per-stream window) and the relay does not OOM
  or unbound; explicit full-policy behaviour asserted.
- **B6 — `lb/src/main.rs` wiring + observability + security proofs**: wire Mode B into the
  binary behind config; `quic_modeb_*` metrics; a full real-wire end-to-end test; the
  **two-connections proof** (client↔LB and LB↔backend are genuinely distinct QUIC connections,
  not a passthrough) and the **0-RTT-rejection proof** (early-data is refused on the
  terminating leg). These are the security-load-bearing proofs — give them full budget, not
  end-of-session pressure (S18 owner ruling).

When B4–B6 land + verified + a clean R1 ×3 gate: Mode B is COMPLETE → native QUIC proxy is
COMPLETE. Promote `--no-ff` with an honest message; native-QUIC-complete ≠ production-ready.

## PRIORITY 2 — THE CHAOS/SOAK SUITE (oldest carry-forward; highest-leverage prod-readiness)
Does not yet exist. Build it + run the first soak of the whole system (Mode A passthrough, the
9 H-matrix cells, H3 termination, XDP datapath, AND Mode B once B4–B6 land). Inject loss,
reorder, latency, partial failure, connection churn; assert no leaks (memory/fd/stream-table),
no stalls (esp. the loss-recovery path per the mechanism note above), bounded state, clean
shutdown. This is the gate to "production ready."

## Then: WS-over-H2 (RFC 8441) + H3 (RFC 9220), gRPC-over-H3 conformance, full h3spec.

## Carry-forwards (unchanged unless noted)
- **CF-DEP-1** (Dependabot advisories — owner; the single oldest program item; ~12 sessions).
- **CF-IGN-1** (18 inherited `#[ignore]` tests; the ×3 gate reports 18 ignored).
- **CF-FCAP-MARGIN**, **F-ESC-1** (multi-kernel CI lane — pair with Mode A XDP tier),
  **N-1** (jumbo-MTU), **Mode A deferred perf tiers** (io_uring v1.1, XDP v1.2),
  **CF-S16-RELAY-GENERIC-ERR** (LOW), plus prior coverage/disk items.
- **R9 hygiene hazard (carry):** 5 stale `git stash` entries from OLD sessions (s7/verifier,
  prod-readiness/round-4) + stray `worktree-agent-*` branches pollute the shared repo — a
  future session owning those branches should drop them. S18 did NOT touch them (not its to
  delete) and added none.

## Start-of-S19 checklist
1. `git checkout main` (now carries S16+S17+S18 Mode B core), branch `feature/quic-proxy-s19`.
2. Confirm tree: `cargo test --workspace --all-features` ×3 green (baseline) + clippy + fmt.
3. `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` exported per-command; ≥25 GB free
   (S18 ended at ~13 GB free after the all-features gate build — clean the target dir first).
