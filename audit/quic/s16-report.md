# SESSION 16 — REPORT (native QUIC proxy, Mode B: terminate-and-re-originate)

> Base: `feature/quic-proxy-s16` @ 30cc22f2 (= main, S15 Mode A promoted).
> Mode B TERMINATES the client QUIC connection (reusing the existing quiche
> H3-termination stack), proxies **raw** QUIC streams + datagrams, and
> RE-ORIGINATES a fresh upstream QUIC connection. Two distinct connections.
> Plan: `audit/quic/s16-plan.md`. quiche API ref: `s16-quiche-api-notes.md`.

## VERDICT: << pending >>

---

## Phase 0 — baseline + hygiene + fcap1 disposition

### Hygiene (R9)
- Base tip = `main @ 30cc22f2` CONFIRMED (S15 promoted). `feature/quic-proxy-s16`
  branched off it + pushed to origin.
- `ps aux`: no S15 strays (no heartbeat cron, no disk-guard; only system `cron -f`).
  No user crontab.
- Disk: **33 GB free** (≥25 GB ✓), eg-target 18 GB, steady across the ×3 gate
  (no growth — warm incremental cache). NOTE: `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target`
  is NOT in `.cargo/config.toml` — it must be exported per cargo invocation
  (S15 relied on an exported env var). All build/test commands set it inline.

### R1 baseline ×3 (full 8-core, default test-threads) — GREEN, DETERMINISTIC
| Step | Result |
|---|---|
| `cargo test --workspace --all-features --no-run` | exit 0 (compile clean) |
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-features --all-targets -- -D warnings` | clean |
| test RUN1 | exit 0 — **1349 passed, 0 failed**, fcap1 ok |
| test RUN2 | exit 0 — **1349 passed, 0 failed**, fcap1 ok |
| test RUN3 | exit 0 — **1349 passed, 0 failed**, fcap1 ok |

3/3 deterministic (not 2-of-3). Logs: `s16-phase0-{compile,fmt,clippy,run1,run2,run3}.log`.

### fcap1 leftover disposition (R2/R4) — CF-SATURATION-1, mechanism captured, RESOLVED
The S15 final report dispositioned a RUN1 failure of
`fcap1_h2_over_cap_upload_yields_413` (tests/h2h1_md_streaming_verify.rs:1860;
H2→H1; cap = `MAX_REQUEST_BODY_BYTES` 64 MiB; test pushes 66 MiB) as a "known
saturation flake, passed 2/3" — **without** recording the captured mechanism,
which R2/R4 require. Re-verified this session:

- **Captured mechanism (S15 RUN1 log)**: `status=Some(502) written=1507328
  backend_body_bytes=1508946` — i.e. only ~1.5 MiB was written, **≪ the 64 MiB
  cap**. The over-cap 413 arm was therefore NEVER reached; the 502 is the
  gateway correctly reporting a genuinely-closed upstream (the draining test
  backend closed mid-upload under scheduling starvation). Zero server-side
  misbehavior → environmental per R2. Matches [[gate-saturation-test-fragility]].
- **Fresh re-verification under 8-core saturation**:
  - In the R1 ×3 gate (real full-workspace saturation): fcap1 **ok 3/3**.
  - Dedicated isolation-burst, 12 iters under 7×`yes` CPU saturation
    (`s16-fcap1-burst.log`): **12/12 PASS**, every run `status=413`,
    `written ∈ {67174400, 67239936}` — i.e. **every run crossed the 64 MiB cap**
    (67108864 B) by 1–2 64 KiB chunks and the 413 arm was TAKEN. This also
    refutes the [[fcap1-overcap-arm-backpressure-masked]] masking worry: the cap
    branch genuinely executes, it is not backpressure-masked.
  - Total: **15/15** saturated observations correct; cap-taken proven.
- **Disposition**: CF-SATURATION-1 (existing class), mechanism captured
  (S15 `written=1.5 MiB ≪ 64 MiB` + 15/15 fresh correct incl. cap-taken). NOT a
  defect, NOT asterisked. Resolved in Phase 0. The thin-margin determinism
  hardening remains the existing LOW-priority CF-FCAP-MARGIN (fast unit test
  driving the Reset arm directly, removing the 64 MiB volume dependency).

**Phase 0: GREEN.**

---

## Phase 1 — Mode B plan + owner ruling

Full plan in `audit/quic/s16-plan.md` (lead-approved). Architecture RESOLVED
(terminate + raw stream/datagram proxy + re-originate; reuses router/actor pump,
RetryTokenSigner, ZeroRttReplayGuard, QuicUpstreamPool dial, SHARED-1/2). Five
design decisions resolved; the one product fork (0-RTT) escalated and ruled:

- **0-RTT OWNER RULING (2026-05-29): reject in v1.** Do NOT `enable_early_data`
  on the client-facing config (inherit current verified H3 behavior — 0-RTT not
  accepted today). Keep `ZeroRttReplayGuard` as defence-in-depth. **Verify the
  rejection by a real-wire mechanism test** (client attempts early data; LB does
  not act on it before handshake; completes via 1-RTT) — same bar as the S15
  no-decrypt proof; an untested "we reject 0-RTT" is the trusted_cidrs stub trap.
  Full 0-RTT passthrough = CONSIDERED-AND-REJECTED on security-vs-value grounds
  (re-originator sends fresh 1-RTT upstream regardless, so client 0-RTT can never
  be 0-RTT to the backend; option-B replay surface buys only half a round trip).
  NOT carried as v2 roadmap debt.

---

## Phase 2 — Mode B increments (per-increment evidence)

### B1 — actor seam + dedicated upstream dial (dual-connection skeleton) — VERIFIED
Author = builder-1 (`f6d0d8e1`); verify = independent (different agent).

**Implementation:**
- `conn_actor.rs`: `ActorParams.raw_quic_backend` seam; `run_actor` early-dispatches
  to `run_raw_proxy_actor` as its FIRST statement → H3 path byte-for-byte unchanged
  when `None` (R3). `router.rs`: threaded through `spawn_new_connection`.
- `raw_proxy.rs` (new): `run_raw_proxy_actor` dual-connection skeleton — Phase 1
  drives client to established, snapshots ALPN/SCID/trace_id, dials a dedicated
  upstream mirroring the ALPN; Phase 2 runs both pumps in one biased `select!`
  (cancel/client-inbound/upstream-recv/2 timeouts); `graceful_close` both. No app
  relay yet (B2/B4). `RawProxyOutcome` + `run_raw_proxy_actor_for_test` hook.
- `quic_pool.rs`: extracted `connect_and_drive` (R12 single-source dial loop);
  `dial_new` delegates (`alpn_override=None`, behavior unchanged); `dial_dedicated`
  → un-pooled `DedicatedQuic`, mirrors ALPN, for 1:1 re-origination.

**Verification (independent):**
- **Two-connections proof (real wire, by mechanism) — PASS.** `tests/s16_b1_two_connections.rs`:
  real quiche client ⇄ Mode B actor ⇄ real backend; backend independently records
  the SCID it observes via `Header::from_slice` before `accept`. Asserted:
  `client_scid ≠ upstream_scid`; distinct `trace_id`s; `negotiated_alpn==h3` mirrored
  (factory installs a bad ALPN → handshake would TLS-fail without the mirror);
  **LOAD-BEARING bridge discriminator**: `backend_observed_scid == upstream_scid
  (c31a52e1…) ≠ client_chosen_scid (c01d57ce…)` — the backend saw the LB's freshly
  sampled SCID, not the client's; a bridge fails this, re-origination passes. Plus
  SCID prefix-independence (≤2 common bytes). 1/1 PASS.
- **H3 no-regression (R3) — PASS.** lb-quic 153/0 (incl. h3_h3_stream_e2e 22/0,
  h3_h2 11/0, h3_h1 17/0, round8 3/0), lb-io quic_pool 7/0, lb-l7 bridging_h3_h3 1/0;
  `quic-passthrough-only` compiles (NEVER-DECRYPTED linkage preserved).
- **clippy** -D warnings clean; **fmt**: builder's source FAILED `fmt --check`
  (defect caught by verifier — builder hadn't run fmt); lead applied `cargo fmt`,
  now clean.
- **R12**: `drain_conn_send` confirmed duplicated (log-string-only delta) →
  single-sourced in B2.

### B2 — raw stream relay + bounded-window backpressure — VERIFIED
Author = builder-1 (`41bf6c90`); verify = independent (different agent).

**Implementation:** bidirectional raw QUIC STREAM relay in `run_dual_pump` (identity
stream-ID map, no translation table). R8 bounded in-flight window
`STREAM_RELAY_WINDOW=256 KiB` **per stream per direction** — reads gated on
`pending < window` so a slow destination stops the relay reading the source →
quiche stops extending that source stream's flow-control window → source peer
pauses (genuine end-to-end backpressure both ways; NOT a body/total cap). FIN
propagated only after pending drains; FIN-only stream under `StreamLimit` retried
(not dropped). RESET_STREAM/STOP_SENDING marked done WITHOUT a clean FIN (F-MD-4
smuggling guard); full peer-propagation deferred to B3. R12: `drain_conn_send`
single-sourced (`conn_actor` `pub(crate)`).

**Verification (independent, --locked, no-commit):**
- **Multi-stream byte-identical — PASS.** `tests/s16_b2_multistream.rs`: 5 concurrent
  client bidi streams, payloads 9/60/200/400/130 KB (the 400 KB > 256 KiB window
  forces the multi-turn pending-carry path); every stream byte-identical + clean FIN.
- **R8 backpressure — PASS.** `tests/s16_b2_backpressure.rs`: backend not-reading →
  relay echoed **0 bytes while stalled** (honors dest flow control), transfer NOT
  complete while stalled (genuinely gated, not buffered-through), full 4 MiB
  byte-identical after resume. 256 KiB bound confirmed by code-read (black-box can't
  read `half.pending.len()`); the verifier caught+fixed a saturation flake in ITS OWN
  test (client send-cursor inflates under CPU starvation — CF-SATURATION-1) and
  redesigned to timing-robust destination-gating assertions; green ×4 under saturation.
- **FIN-retry under StreamLimit — PASS** (builder's 2 unit tests prove open-then-grant).
- **reset-not-FIN — PASS.** `tests/s16_b2_reset_not_fin.rs`: client RESET mid-body →
  backend saw 13012 bytes and **NO clean FIN**; + code-read confirms no `fin=true`
  reachable from reset/stop/error arms.
- **R3 regression — PASS.** lb-quic `--all-features` **159/0** across **8 clean runs**
  (×4 under 8-core saturation). clippy/fmt clean. Cargo.lock unmodified.

NOTE (process): the B2 builder committed local checkpoints against instruction +
spuriously re-resolved Cargo.lock (un-`--locked` cargo); not pushed. Lead reset to
B1 tip, restored Cargo.lock, re-verified green, recommitted clean. Lesson captured;
B3+ prompts mandate `--locked` + no-commit.

### B3 — cancellation propagation (F-MD-4 analog) — built, lead-checked, R13 verify pending
Author = builder-1; lead build/test-checked; independent R13 verify next.

**Implementation (`raw_proxy.rs`):** `RelayHalf` gains `reset_code: Option<u64>` (idempotency
latch). New `propagate_cancel(peer, sid, code, peer_dir, dir)` helper: guard
`if reset_code.is_some() || done { return }`; `peer.stream_shutdown(sid, peer_dir, code)`
(Ok/Done ok, other errors swallowed — no panic); then `pending.clear()`,
`reset_code=Some(code)`, `done=true` — **never a clean FIN** (smuggling guard kept).
Three call sites in `pump_dir`: read-side `Err(StreamReset(code))` →
`propagate_cancel(dst, .., Shutdown::Write, ..)` (RESET_STREAM onward); write-side
+ FIN-block `Err(StreamStopped(code))` → `propagate_cancel(src, .., Shutdown::Read, ..)`
(STOP_SENDING back). Direction correct for both c2u and u2c (src/dst swap); only the
affected unidirectional half is torn down. Generic-error arms keep B2 fail-safe (drop,
no FIN, no synthesized reset) — documented.

**Lead checks (independent, --locked):** build 0 / clippy -D 0 / fmt 0; B3 smoke
(`s16_b3_reset_propagation_smoke.rs`) — backend's `stream_recv` returns
`Err(StreamReset(0xBEEF))`, the exact client code, no clean FIN; B2 reset-not-FIN
negative control still PASS; raw_proxy unit (fin_only) 2/2.

**Test-correctness fix (verified legit, NOT a weakening):** the builder removed a
`stream_finished()` secondary witness from `s16_b2_reset_not_fin.rs`. Confirmed against
quiche 0.28 source (`lib.rs` `stream_finished`: `None => return true`): once B3 correctly
resets+collects the upstream stream it becomes unknown → `stream_finished()` falsely
reports a clean end. The genuine clean-FIN witness (`stream_recv` returning `fin==true`)
is KEPT and is the real smuggling signal — negative control remains load-bearing.

**R13 verify (independent, NEXT):** (a) in ×3 gate, (b) ≥50-iter isolation burst,
(c) load-bearing negative control. Per [[h3h3-fmd4-no-r13-bc]]: confirm a STREAM-level
RESET_STREAM carrying the code (backend `stream_recv`==`Err(StreamReset(code))`), NOT a
stream dying from connection teardown; do not use `stream_finished()` as a witness.

**WATCH (Phase 3, CF-SATURATION-1 class):** `s16_b2_multistream.rs` (B2 echo test, no
reset path) flaked once on its 25s `RELAY_BUDGET` under 4-concurrent-wire-test 8-core
saturation; passes isolated (0.7s) + on rerun. Pre-existing [[gate-saturation-test-fragility]];
bump-don't-weaken if it recurs in the ×3 gate. Not a B3 issue.

### B4..B6 — << appended as each lands >>

## Phase 3 — gates

<< ×3 + scoped llvm-cov + clippy/fmt + R3/R12 regression >>

## Carry-forwards (tracked, not in scope)
CF-DEP-1 (Dependabot, owner, 9 sessions old — oldest unaddressed), CF-IGN-1 (16
inherited #[ignore]), CF-FCAP-MARGIN, F-ESC-1 (multi-kernel CI lane), N-1
(jumbo-MTU xdp.frags), CF-S15-PASSTHROUGH-FEATURE-GATING-DEEP,
CF-S15-PASSTHROUGH-RETRY-ODCID, per-IP Retry rate-limit (v1.1). Program S17+:
WebSockets-over-H2 (RFC 8441), WebSockets-over-H3 (RFC 9220), gRPC-over-H3,
h3spec, chaos/soak.
