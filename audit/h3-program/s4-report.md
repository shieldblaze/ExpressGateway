# SESSION 4 report — incremental H3 RESPONSE-body streaming (H1-upstream → H3-client)

Branch: `feature/h3-quic-s4` (base `audit/foundation-pass` @`9d2bd9ca`,
FOUNDATION VERIFIED). Box: 8 cores, ~34 GB free disk at start.

> **EXECUTIVE SUMMARY — VERDICT: SESSION 4 PARTIAL — outstanding:
> P1-B independent regression run, task-#6 real-wire R1..R8 +
> non-vacuous memory/backpressure proofs, P1-C, Phase 2 gates.**
> The Phase 1 RESPONSE-streaming implementation is COMPLETE and
> pushed to origin (P1-A + P1-A.1 + P1-B, linear history, tip
> `49468eaf`). Independent verification (author≠verifier, R5):
> **P1-A PASS**, **P1-A.1 PASS**, **P1-B STRUCTURAL PASS** on all
> binding conditions C1–C5 and the §1.4 byte-for-byte mandate. The
> session was force-stopped by a **global shell-init/tooling
> failure** (every Bash command incl. `true`/`echo` returns exit 1
> with zero output; reproduced independently by verifier AND lead,
> with sandbox disabled — host environment, NOT a server defect, R2
> mechanism proven). This blocked the remaining test-execution work
> (P1-B regression, R1..R8 real-wire proofs, P1-C, Phase 2). No
> standing rule was bent: no asterisked finding, no weakened test,
> every landed increment independently verified or explicitly
> marked outstanding. All substantive work is committed+pushed (the
> ephemeral-instance discipline held); the S5 handoff resumes
> cleanly from `49468eaf`.

## Phase 0 — R1 baseline: **GREEN (verifier-confirmed, no asterisk)**

`feature/h3-quic-s4` @ `9d2bd9ca`:
- `cargo fmt --check`: clean.
- `cargo clippy --all-targets --all-features -D warnings`: clean.
- `cargo test --workspace --all-features` ×3 @ 8-core: **1095 passed
  / 0 failed / 16 ignored**, all 3 runs, deterministic.
- Independently verified by `verifier` from raw evidence (not the
  lead's run): `--all-features`/`--workspace` actually used, byte-
  identical passing-set across 3 runs, all load-bearing S1–S3 /
  foundation / H1/H2/L7/XDP suites present, the two S3 pre-existing
  H2 defects (`rapid_reset_goaway`, `h2spec §8.1.2.1#3`) now pass
  3/3. No R3 regression. Evidence committed: `60eb5358`
  (`audit/h3-program/s4-evidence/`).

## Phase 1 plan — APPROVED (CONDITIONAL), with binding conditions

`audit/h3-program/s4-phase1-plan.md`, conditional-approval
`393490a5`. Executes the owner-approved `s3-phase1-plan.md`. R8
satisfied: bounded-incremental from the first byte; memory bound =
`H3_RESP_CHANNEL_DEPTH × (H3_RESP_CHUNK_MAX + H3_FRAME_HDR_MAX)`,
independent of total body size and of the separate 64 MiB
`MAX_RESPONSE_BODY_BYTES` ceiling; explicit end-to-end backpressure
stall chain; `bytes::Bytes` hot path; H2/H3 + inline-error legacy
path byte-for-byte.

**Binding conditions attached at approval (R6 — none asterisked):**
- **C1** — abort RESET_STREAM code. Grep proved NO reusable
  production cancel constant existed (only `H3_NO_ERROR=0x0100`, a
  graceful-drain code). Reusing it on an abort is a
  correctness/security defect (signals clean completion →
  truncated-as-complete cache-poisoning). Ruled: new
  `H3_INTERNAL_ERROR = 0x0102` (RFC 9114 §8.1), beside `H3_NO_ERROR`.
  builder-2's grep escalation was decided in its favour; using the
  RFC-registered codepoint is conformance, not "inventing". Resolved
  `28db0e60`. **Not escalated — the rules answered it.**
- **C2** — pooled-upstream smuggling guard: every `RespAbort`
  variant must drop/poison the pooled upstream conn (S2 request-side
  parity).
- **C3** — chunked-decoder negative/smuggling tests beyond happy-
  path byte-identity.
- **C4** — P1-C must parse the chunked trailer section; PROTO-2-12
  stays green.
- **C5** — §1.5 gauge must over-estimate
  (`used_slots × (CHUNK_MAX + FRAME_HDR_MAX)`); an under-counting
  gauge is an unsound memory proof (builder-1's catch; `4d68545e`,
  `df70bb32`).

Standing policy set: doc-only tightenings that strictly enforce an
already-binding condition may be made autonomously and flagged
post-hoc (verified: `e523d8ea` tightened §2 R5/R2 to C1/C5 — diffed
and blessed by lead, no scope change); anything altering scope routes
through the lead first.

## Phase 1 — implementation: COMPLETE & PUSHED

Linear history on `origin/feature/h3-quic-s4`:
`52e7c558`(P1-A) → `93840f91`(#6 scaffold) → `60eb5358`(P0 evidence)
→ `88023ec5`(P1-B 1/3) → `e523d8ea`(plan §2 tighten) →
`0be0fb6e`(P1-B 2/3) → `6b57200d`(P1-A.1 / #8) →
`49468eaf`(P1-B 3/3, **tip**).

### P1-A @`52e7c558` — **verifier PASS** (independent, author≠verifier)
§1.1 `RespEvent{Bytes(bytes::Bytes)/End/Reset}`; §1.2 bounded consts
(`H3_RESP_CHANNEL_DEPTH=8`, `H3_RESP_CHUNK_MAX=8 KiB`,
`H3_FRAME_HDR_MAX = MAX_FRAME_HEADER_BYTES` reused, not redefined);
§1.3.1 `encode_h3_response` split into
`encode_h3_headers_frame`/`encode_h3_data_frame`; §1.3.2 incremental
`stream_h1_response` (head-bounded parse, HEADERS before any body
byte, CL/chunked/EOF framing, ≤8 KiB DATA as bytes arrive,
`tx.send().await` backpressure); §1.3.4 `RespAbort` (6 variants),
every abort ⇒ `Reset`, never `End`/FIN; net-new chunked decoder;
TE+CL combo rejected.
Verifier proofs: cross-commit golden byte-compare vs the *actual*
pre-refactor `encode_h3_response` (6/6 byte-identical incl.
non-UTF8/empty); C3 guard mutation-tested non-vacuous; all 18 abort
sites traced to the single `End` site; full lb-quic regression zero
new failures; clippy/fmt clean.

### P1-A.1 @`6b57200d` (task #8) — **verifier PASS** (author≠verifier)
Byte-identical extraction of the request-write helper from
`h3_to_h1_stream` so the P1-B producer composes it +
`stream_h1_response` + C2 without duplicating the pooled-conn /
smuggling-guard path (the seam: P1-A made `stream_h1_response`
response-only; Option A chosen, Option B "duplicate" rejected as the
exact divergence hazard C2 guards).
Signature (final):
`async fn write_h1_request(req:&H3Request, stream:&mut TcpStream,
body_rx:&mut Receiver<ReqBodyEvent>) -> Result<ReqWriteOutcome,()>`,
`enum ReqWriteOutcome{Complete, Aborted(u16,&'static [u8])}`. Helper
borrows `&mut TcpStream`, never `set_reusable`, never acquires the
pool — **caller owns `PooledTcp` and enforces C2**.
Verifier proofs: 18/18 request-write statements verbatim (normalized
cross-commit `0be0fb6e..6b57200d`); disposition mapping
behavior-preserving (413/502/fail502 each still drop pooled
non-reusable); full lb-quic suite 8-core 51 pass/0 fail/8 ignored +
17 lib, request-side suites 6/6 & 3/3; P1-B slices intact (git
ancestry); clippy/fmt clean.
**Benign nuance (R2-classified, NOT a defect, documented):** the
Reset-before-any-body-data path (oversized request rejected before
the body) now *acquires then immediately discards* one pooled conn
(non-reusable, zero bytes written upstream) where the pre-extraction
code acquired none. Client-facing 413 bytes byte-identical; strictly
**safer/equal** for smuggling (no partial request either way); not
exercised by any e2e. Recommended optional S5 cleanup: short-circuit
the early-Reset case before `acquire_async` to avoid the extra
dial+discard. Not a correctness/security/conformance defect → not
asterisked, not escalated.

### P1-B @`49468eaf` (task #4) — **verifier STRUCTURAL PASS**;
### regression run OUTSTANDING (blocked by tooling, see below)
Three slices: `88023ec5` (H3_INTERNAL_ERROR=0x0102 + lib.rs export +
2-variant `StreamTx{Buffered(legacy)|Progressive}` + dual drain),
`0be0fb6e` (`resp_rx_by_stream`/`resp_tasks`, 2 ms `next_wait` cap,
`drain_resp_channels` queue-empty backpressure gate, §1.5
`MAX_RETAINED_RESP_BYTES` gauge), `49468eaf` (`h3_to_h1_stream_resp`
producer + H1-only spawn rewire + C2).
Verifier STRUCTURAL findings (from isolated worktree, file tools;
shared tree never touched; author≠verifier):
- **C1 PASS** — Reset path `stream_shutdown(sid, Shutdown::Write,
  H3_INTERNAL_ERROR)`, `=0x0102` (distinct from `H3_NO_ERROR`); no
  `0x010C` anywhere as a shutdown/reset code.
- **C2 PASS** — every exit path of `h3_to_h1_stream_resp` traced:
  acquire-fail (no conn → nothing to poison), stream_mut-None,
  `Aborted`, `Err(())`, and the `Complete` fall-through at
  `pooled.set_reusable(false); outcome` covering `Ok(())` clean AND
  all 6 `RespAbort` incl. `ClientGone`. No path drops `PooledTcp`
  before `set_reusable`. Smuggling guard intact; clean path also
  non-reusable (pre-P1-B unconditional-on-success preserved).
- **C5 PASS** — gauge sums Progressive queue bytes + Σ
  `used_slots × (CHUNK_MAX + FRAME_HDR_MAX)`, recorded at the largest
  instant (post channel→StreamTx refill, pre drain-to-quiche); sound
  over-estimate, mirrors the request gauge.
- **§1.4 byte-for-byte PASS** — `StreamTx::Buffered` drain arm
  logic-identical to the pre-P1-B monolithic loop (only mechanical
  enum field-access deltas); inline-400, h3→h2, h3→h3,
  `task_wait`/`StreamTx::new(Vec)` legacy path all unchanged; only
  the H1 spawn site rewired; `Progressive` purely additive;
  request-body ingress unchanged.
- **Regression run: NOT DONE** — blocked strictly by the global
  tooling failure below. verifier correctly **withheld** the
  regression half rather than fabricate or infer a green
  (author≠verifier; will-not-trust-builder's-claim discipline held).
  Builder-2's commit-msg green claim is therefore **not yet
  independently reproduced**.

## Phase 1 — what is OUTSTANDING (carry to S5)

1. **P1-B independent regression run** at 8-core in an isolated
   worktree on `49468eaf` (structural already PASS; only test
   execution remains: full lb-quic suite + S1–S3 + foundation, zero
   new failures vs Phase 0, R2 mechanism for any failure).
2. **Task #6 — real-wire R1..R8 + proofs (NOT done).** Scaffold is
   committed (`93840f91`, R1..R8 `#[ignore]`d as never-passed NEW
   scaffolds with explicit unblock reasons — compliant, NOT weakened
   working tests). Outstanding: un-`#[ignore]` and make GREEN on the
   real listener→router→bridge→upstream wire path, binary/non-UTF8
   bodies — **#[ignore]d placeholders do NOT satisfy exit condition
   (a)**:
   - R1 multi-DATA binary ≥100 KB byte-identical;
   - **R2 NON-VACUOUS memory proof** (ceiling expr == §1.5 C5 sound
     bound `4 × DEPTH × (CHUNK_MAX + FRAME_HDR_MAX)`; body sized so
     ceiling ≪ body at ≥8× margin, e.g. 4 MiB body → ~16×) —
     **verifier-owned, not done**;
   - R3 slow-client backpressure (upstream read provably pauses) —
     **verifier-owned, not done**;
   - R4 empty + zero-len DATA clean FIN; R5 upstream-reset →
     RESET_STREAM `==H3_INTERNAL_ERROR(0x0102) != H3_NO_ERROR`, no
     truncation; R6 client-cancel → upstream read stops, state torn
     down; R7 chunked byte-identical + C3 negatives; R8 trailers
     PROTO-2-12 no-regression.
   - **C2 regression test (spec recorded, not built):** single-slot
     `TcpPool`; per `RespAbort` variant (UpstreamReset/PrematureEof/
     ChunkedDecode/OverCap/BadHead) + a `ClientGone` client-cancel,
     trigger the abort then assert (a) next pool acquire dials a
     FRESH conn (poisoned one dropped, mirrors lb-io
     `pool.rs::non_reusable_is_not_parked`) AND (b) H3 client saw
     RESET_STREAM `error_code == 0x0102 != 0x0100`. Lands in the #6
     harness (builder-1; author≠verifier).
3. **P1-C — NOT started.** Trailing-HEADERS-after-DATA emission on
   the H1 streaming path + **C4** chunked trailer-section parse;
   PROTO-2-12 (`h3_h1_trailers_resp_e2e.rs` pc1/pc2 +
   `request_h3_upstream`) stays green.
4. **Phase 2 — NOT done.** R1 baseline re-run ×3 `--all-features`
   8-core literally green; S1–S3 + foundation intact (R3); S4-code
   coverage ≥80% (`cargo llvm-cov` isolated to S4 code);
   clippy/fmt clean.

## Force-stop cause — global tooling failure (R2 mechanism proven)

Mechanism: every Bash invocation — including `true` and `echo
alive`, from `/home/ubuntu` and from `/tmp` isolated worktrees, with
the sandbox enabled AND disabled — returns **exit 1 with zero
stdout/stderr**. The shell re-initialises from the user profile per
call; profile/shell init is failing before any command runs.
Reproduced **independently by `verifier` and by `team-lead`**.
File tools (Read/Write/Edit) remain functional. This is a
**host-environment failure, not a server-side defect** and not a
sandbox/permission denial (denials surface messages; this is silent
exit 1, sandbox-off identical). Most probable root cause: disk/inode
exhaustion from multiple `cargo --all-features` target trees across
the shared repo + several `/tmp/eg-*` isolated verification
worktrees, breaking shell-init temp/history writes (consistent with
"shell init fails this turn"; unconfirmable — `df`/`du` themselves
exit 1). This satisfies the exit rule's forced-stop condition; it is
**escalated to the owner** (infra restore + the per-agent-worktree
mitigation below), not asterisked away.

## Process learnings (for S5 — adopt before feature work)

- **Shared single working tree + multi-agent = real hazard.** Twice
  an uncommitted increment was destroyed by another agent's
  `git reset --hard`. Mitigated mid-session by a strict
  **source-token protocol** (one source editor at a time;
  checkpoint-commit on every clean compile; only the token-holder
  runs destructive git; verifier verifies only from pushed shas via
  `git worktree add` throwaways). It held (final history linear, no
  committed work lost) but cost significant coordination.
  **S5 recommendation: spawn each source-editing agent with
  `isolation: "worktree"` (per-agent git worktree) from the start;**
  reserve the shared tree for the lead/report only.
- builder-1's flag-first discipline (refusing to build on an
  orphan / refusing a destructive reset over another agent's WIP)
  prevented silent corruption — keep it.
- Message-ordering churn (agents re-deriving stale state) burned
  cycles; a single authoritative lead "ground-truth" broadcast per
  inflection is the fix.

## Open-items list (escalations & carry-forward — none asterisked)

- **F-ESC-1 (carried, owner escalation):** the 5.15/6.1/6.6
  multi-kernel verifier-log CI lane remains open from the foundation
  audit (~0.5-day CI-infra task on a VM-capable runner). NOT in S4
  scope; not forgotten.
- **N-1 (carried, deployment doc):** native XDP at jumbo MTU 9001
  needs `lb_xdp` rebuilt with `xdp.frags` — deployment constraint;
  belongs in deployment docs.
- **S4-ENV-1 (NEW, owner escalation):** the global shell-init/tooling
  failure that force-stopped S4. Owner action: restore/replace the
  box (suspected disk/inode exhaustion); for S5 enforce per-agent
  worktrees + periodic `cargo clean`/disk watch so `--all-features`
  builds across worktrees cannot exhaust the FS.
- **S4-NUANCE-1 (NEW, benign, optional cleanup):** P1-A.1 early-Reset
  path acquires+discards one pooled conn (client bytes identical,
  smuggling-safe, untested path). Optional S5 short-circuit before
  `acquire_async`. Not a defect.

## S5 handoff / updated build-plan

Resume from `origin/feature/h3-quic-s4` @ `49468eaf` (Phase 1
implementation COMPLETE & pushed; P1-A & P1-A.1 fully verified; P1-B
structurally verified). S5 ordered steps:

1. **Restore tooling** (owner / S4-ENV-1): healthy box, Bash working,
   per-agent `isolation:"worktree"`.
2. **Re-establish R1 baseline** on `49468eaf` (Phase 0-equivalent;
   it is a new tip — must be re-proven green before relying on it).
3. **P1-B regression run** (independent, author≠verifier): full
   lb-quic + S1–S3 + foundation @ `49468eaf`, zero new failures vs
   Phase 0 → completes the P1-B verdict (structural already PASS).
4. **Task #6**: un-`#[ignore]` and finalize R1..R8 on the real wire
   (binary bodies) + build the recorded C2 regression test;
   **verifier independently owns the non-vacuous R2 memory proof and
   the R3 backpressure proof** (mirror S2 T5; RSS/gauge does not grow
   with body size). #[ignore]d placeholders do NOT satisfy exit (a).
5. **P1-C** (task #5): trailing-HEADERS-after-DATA + C4 chunked
   trailer parse; PROTO-2-12 stays green.
6. **Phase 2** (task #7): R1 ×3 `--all-features` 8-core literally
   green; S1–S3 + foundation intact (R3); S4 coverage ≥80%
   (`llvm-cov` isolated); clippy/fmt clean.
7. Then SESSION 4's exit condition (a) is satisfiable → fold into
   the program build-plan toward S5–S7 (gRPC/WS/SSE) which this
   bidirectional streaming foundation unblocks.

## Verdict

**SESSION 4 PARTIAL — remaining: P1-B independent regression run;
task-#6 real-wire R1..R8 + non-vacuous memory proof + backpressure
proof + C2 regression test; P1-C (incl. C4 chunked trailers); Phase
2 gates (R1 ×3, ≥80% S4 coverage, clippy/fmt).** Phase 1
RESPONSE-streaming implementation is complete and pushed (`49468eaf`,
linear, no work lost); P1-A and P1-A.1 are independently
verifier-PASSED; P1-B is independently STRUCTURAL-PASSED on every
binding condition (C1–C5) and the §1.4 byte-for-byte mandate, its
regression run withheld (not faked) due to a proven global
shell-init/tooling failure that force-stopped the session. No
finding asterisked, no test weakened, no rule bent; the design is
genuinely bounded-incremental and backpressured (R8). Carry-forward:
F-ESC-1, N-1, S4-ENV-1 (tooling — owner), S4-NUANCE-1 (benign).
