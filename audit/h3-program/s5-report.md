# Session 5 — H3 RESPONSE streaming: verify S4, then complete

Branch: `feature/h3-quic-s4`. Base tip: `bd2e6dca` (S4 = `49468eaf` code
+ S4 report). S5 continues ON this branch (S5 finishes S4's work).

Verdict: **(in progress)**

---

## Phase 0 — finishing Session 4's verification

### Environment hygiene (R9)

- Repo `/home/ubuntu/Code/ExpressGateway`, user `ubuntu`, 8 cores, ~52 GB
  free at start. No root-owned files anywhere in the repo or target/
  (S4-ENV-1 host failure left no root artifacts in this repo). Stale
  S4 `/tmp/eg-*` worktrees pruned.
- Per-agent git worktrees (R9): lead = main tree (report/baseline,
  no feature code); `eg-verifier` (detached) = verifier; `eg-builder2`
  (detached) = builder-2. All same user, no sudo. Periodic
  `df -h /` + `cargo clean` of stale targets to keep `--all-features`
  trees from exhausting the FS.

### Step 1+2 — R1 baseline + the withheld P1-B regression run

Run on the pristine S4 tip `bd2e6dca` (= P1-B @ `49468eaf` merged),
driver `s5-evidence/run-p0.sh`, evidence `s5-evidence/p0-*.txt`:

| Gate | Result |
|---|---|
| `cargo fmt --check` | clean — `FMT_EXIT=0` |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean — `CLIPPY_EXIT=0` |
| `cargo test --workspace --all-features` ×3, 8-core | `TEST_RUN1/2/3_EXIT=0`, zero FAILED, deterministic |

**R1 GREEN BASELINE established on `bd2e6dca`** — no test excluded, no
asterisk. Because `bd2e6dca` includes P1-B (`49468eaf`), this 3×
deterministic-green workspace run on the pristine S4 tip **is the
withheld S4 P1-B regression run, now executed to completion**:
zero new failures vs S4 Phase 0; no previously-passing test regressed.
**P1-B verdict upgraded structural-only → VERIFIED. R3 satisfied.**

### Step 3 — Task #6 (verifier-owned, real-wire; author ≠ verifier)

Verifier worked from `bd2e6dca` in its own worktree, pushed
`bd2e6dca..56079026`. Evidence: `s5-evidence/task6/` (VERDICT.md +
DEFECT writeup). Per-item (each 3× deterministic, real
listener→router→conn_actor→h3_bridge→real H1 backend, binary/non-UTF-8
bodies 0xFF 00 80):

- **R1** PASS — `#[ignore]` removed; 120 KiB binary body byte-identical,
  clean FIN.
- **R2** PASS — non-vacuous memory proof. Consts DEPTH=8, CHUNK_MAX=8192,
  FRAME_HDR_MAX=16. §1.5 C5 sound channel bound = 8·(8192+16) = **65,664 B**
  (chunk+hdr form, not depth·chunk=65,536 — matches the
  `conn_actor.rs:382-385` gauge exactly). R2 ceiling = 4·65,664 =
  **262,656 B**. Body = 4 MiB ⇒ margin **15.97×** (≥8× met). Observed
  peak `MAX_RETAINED_RESP_BYTES` = **73,859 B**, constant across all
  3 R2 + 3 R3 runs = 0.28× ceiling = 1.76% of body (buffering-trap
  value would be ≈4 MiB). Body-size-independent. Authoritative.
- **R3** PASS — backpressure proof: 4 MiB firehosing CL backend,
  non-reading client 2 s; mid-stall = peak = 73,859 B ≪ ceiling ≪ body
  (16×), only possible if `stream_h1_response`'s `tx.send().await`
  blocked the upstream `read()`. Proven, 3× identical.
- **R4** PASS — `#[ignore]` removed; empty body + zero-len DATA, clean FIN.
- **R5** PASS — hard TCP RST / premature-EOF-before-CL / over-cap each
  ⇒ client RESET_STREAM `error_code == 0x0102 == H3_INTERNAL_ERROR
  != 0x0100`, never FIN, never truncated-as-complete (C1 guard).
- **R6** DEFECT (see below) — regression-locked, not weakened.
- **R7** PASS — `#[ignore]` removed; chunked (1/7/4096/8192/1/100/99999
  split) byte-identical via the new decoder, clean FIN.
- **R8** PASS (no-regression) — `h3_h1_trailers_resp_e2e.rs` pc1+pc2
  GREEN unchanged; in-file R8 placeholder remains `#[ignore]`d (P1-C
  scope — original scaffold, not a working test disabled).
- **C2** 5/6 PASS — UpstreamReset/PrematureEof/ChunkedDecode/OverCap/
  BadHead each ⇒ poisoned upstream not parked (single-slot pool
  idle==0, mirrors `lb_io pool::non_reusable_is_not_parked`) AND
  client RESET_STREAM 0x0102. 6th (ClientGone) = the defect, split
  into the failing lock `c2_clientgone_drops_pooled_upstream`.
- **C3** PASS — decoder unit + new end-to-end malformed-chunked
  (non-hex size / missing CRLF / declared-size overflow / junk after
  zero-terminator) each ⇒ RESET_STREAM 0x0102, never forwarded as
  complete.

Workspace at `56079026`: only `r6_*` and `c2_clientgone_*` fail (the
two intentional defect regression locks); nothing else regressed.

### DEFECT-CLIENTGONE (R6 disposition: fixed this session)

Tier: correctness/conformance + resource-exhaustion/DoS
(security-adjacent); binds C2 + §1.3.4 ClientGone. **Tractably
fixable** (scoped, mirrors existing S2 request-side
`StreamReset|StreamStopped` arms) ⇒ per R6 **fixed this session with a
regression test**, not asterisked (R4), routed to builder-2
(author ≠ verifier, R5), verifier re-verifies.

Mechanism (proven, real-wire, not environmental — server-side
misbehavior present: backend stays writable, read-half never closes;
not a scheduling timeout): bodyless H3 GET (HEADERS+FIN) leaves no
`body_*_by_stream` entry, so the S2 request-side peer-reset handlers
(`conn_actor.rs:861/944`) never re-enter for that stream. Client
STOP_SENDING/RESET on the response stream is observed nowhere:
`resp_rx_by_stream` is removed on no client-cancel path; the
Progressive send branch (~`conn_actor.rs:519`) swallows
`Err(StreamStopped)` with a catch-all `debug!`+`break`; and with an
empty Progressive queue `stream_send` is never called so even that is
unreached. ⇒ producer `tx.send().await` never sees a closed receiver
⇒ never `Err(RespAbort::ClientGone)` ⇒ `stream_h1_response` reads the
pooled upstream forever; pooled conn never freed, `h3_bridge.rs:1813`
C2 teardown never reached. DoS: open-stream-then-STOP_SENDING.

**Fix `ad9374dc` (lead-designed/approved minimal variant; authored by
builder2b — author ≠ verifier).** A single self-contained helper
`reap_client_cancelled_responses(conn, &mut resp_rx_by_stream, &mut
stream_response)` in `conn_actor.rs`, called in `run_actor` between the
`poll_h3` block and the §1.4.3 gate: for each stream with a live
response receiver, `conn.stream_writable(sid, 1)`; on
`Err(StreamStopped|StreamReset)` drop the `Receiver` (⇒ producer's next
`tx.send().await` ⇒ `Err(RespAbort::ClientGone)` ⇒
`h3_to_h1_stream_resp` sets the pooled upstream non-reusable + returns,
stopping the upstream read) and drop the `StreamTx` (never FIN, never
RESET_STREAM — §1.3.4 ClientGone, distinct from the 0x0102 abort path).
`drain_streams_to_conn` signature + the Progressive send branch
UNCHANGED. Covers both empty- and non-empty-queue cancel via the
per-iteration poll. A more invasive builder-2 variant (drain signature
change + in-branch arms) was stood down as redundant/higher-risk; its
analysis independently cross-validated the mechanism + the quiche 0.28
`stream_writable` STOP_SENDING semantics.

**Independent re-verification `ba64cfdd` (verifier; author ≠ verifier
held — product code untouched by verifier): DEFECT-CLIENTGONE
RE-VERIFIED.**
- `r6_*` + `c2_clientgone_*` PASS deterministically 3× in 0.74 s.
  **Causation proven by negative control:** with only the
  `reap_client_cancelled_responses` *call* commented out (helper +
  tests byte-identical) both FAIL with the exact original mechanism
  (endless backend read never closes, 40.68 s); call restored ⇒ PASS
  in 0.74 s. Real teardown asserted (backend read-half closes,
  upstream read stops, single-slot pool idle==0).
- C2 ClientGone parity with the other 5 RespAbort variants — PASS.
- §1.3.4/C1: abort path UNCHANGED — R5 still RESET_STREAM
  `0x0102 == H3_INTERNAL_ERROR != 0x0100`; no RESET_STREAM on
  client-cancel (correct).
- R8 preserved: R2/R3 numbers byte-identical to the pre-fix
  authoritative bound (max_retained 73,859 B; ceiling 262,656 B;
  4 MiB body; 15.97×). §1.4.3 gate + bounded channel untouched.
- No regression: `h3_h1_resp_stream_e2e` 15 pass / 0 fail / 1 ignored
  (the 2 locks flipped; R8 placeholder stays `#[ignore]`d — P1-C
  scope, not weakened); `h3_h1_trailers_resp_e2e` pc1/pc2 GREEN
  (PROTO-2-12); full `cargo test --workspace --all-features` ZERO
  failures (202 ok); clippy `-D warnings` clean; `fmt --check` clean.
  fmt note: 21 pre-existing fmt diffs ALL confined to the
  verifier-authored task-#6 test file (from `56079026`), zero in
  product / the `ad9374dc` fix — normalized by the verifier on its own
  test file (pure formatting, suite re-ran 15/0/1 unchanged); product
  code untouched so author ≠ verifier holds.

### Phase 0 verdict — COMPLETE

S4's response-streaming work (P1-A / P1-A.1 / P1-B) is now fully
verified, the withheld P1-B regression run executed green, task #6's
real-wire R1–R8 + C2 + C3 + non-vacuous memory proof + backpressure
proof all pass, and the one surfaced defect (DEFECT-CLIENTGONE) is
fixed and independently re-verified — no finding asterisked, no test
weakened, no rule bent. **Phase 0 COMPLETE; Phase 1 (P1-C) may begin.**

---

## Open items (carry-forward — not asterisked, tracked)

- **F-ESC-1** — multi-kernel (5.15/6.1/6.6) verifier-log CI lane
  (~0.5 d CI-infra, VM-capable runner). Owner escalation, not S5 scope.
- **N-1** — native XDP jumbo MTU 9001 needs `lb_xdp` rebuilt with
  `xdp.frags`. Deployment-doc constraint.
- **S4-NUANCE-1** — benign: P1-A.1 early-Reset path acquires+discards
  one pooled conn (client bytes identical, smuggling-safe, untested
  path). Optional cheap cleanup, else carry.
</content>
</invoke>
