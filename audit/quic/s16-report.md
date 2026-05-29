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

<< B1..B6 evidence appended as each lands; author≠verifier; pushed continuously >>

## Phase 3 — gates

<< ×3 + scoped llvm-cov + clippy/fmt + R3/R12 regression >>

## Carry-forwards (tracked, not in scope)
CF-DEP-1 (Dependabot, owner, 9 sessions old — oldest unaddressed), CF-IGN-1 (16
inherited #[ignore]), CF-FCAP-MARGIN, F-ESC-1 (multi-kernel CI lane), N-1
(jumbo-MTU xdp.frags), CF-S15-PASSTHROUGH-FEATURE-GATING-DEEP,
CF-S15-PASSTHROUGH-RETRY-ODCID, per-IP Retry rate-limit (v1.1). Program S17+:
WebSockets-over-H2 (RFC 8441), WebSockets-over-H3 (RFC 9220), gRPC-over-H3,
h3spec, chaos/soak.
