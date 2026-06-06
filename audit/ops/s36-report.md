# Session 36 — Operational Layer (recycling + config + hot-reload + latest-deps)

Branch: `feature/ops-layer-s36` (base `main` @ `f4d34247`).
Profile: internet-facing AND internal; ALL protocols (H1/H2/H3/QUIC/gRPC/WS).

Mission: add the OPERATIONAL layer "replace nginx/traefik" requires, close the
connection-recycling leak (CF-GRPC-H3-CHURN-RSS), bring deps to latest.
Stage internally by risk; gate each; verified pieces promote, rest carry; main
stays honest-green throughout.

---

## Phase 0 — baseline + swap + sc9 reference

| Check | Result |
|---|---|
| Base tip = `main` @ `f4d34247` | ✅ confirmed (`git rev-parse`) |
| Branch `feature/ops-layer-s36` created + pushed | ✅ |
| Strays (S35) killed | ✅ none present (`ps aux`) |
| Disk ≥ 25 GB free | ✅ 43 GB free at start (`/dev/root` 67 GB) |
| Swap cushion added | ✅ 8 GiB swapfile (`/swapfile`); box is 15 GiB RAM / was 0 swap |
| `clippy --all-targets --all-features -D warnings` | ✅ rc=0 (clean) |
| `fmt --all --check` | ✅ rc=0 |
| `cargo test --workspace --all-features --no-fail-fast` ×3 | ✅ **1512 / 0 / 18** identical ×3 (247 binaries); `audit/ops/s36-baseline/test-run{1,2,3}.log` |
| main CI gate set green (GitHub) | ✅ prod-readiness-gates / CI / scheduled-scans / Dependabot all `success` |
| sc9 staircase reproduced (pre-fix reference) | ✅ **DRIFT** — see below |

### sc9 pre-fix reference (negative control for workstream A) — COMPLETED RUN (R15)
`audit/soak/s36-soak-data/sc9-prefix-reference/` — `sc9_grpc_h3` isolated, 1800 s, **151 samples**.

| metric | verdict | first → last | peak | growth | monotone | note |
|---|---|---|---|---|---|---|
| `rss_kb`   | **DRIFT** | 8 320 → 81 924 KB | 81 924 KB (≈80 MB) | +36.3% (last-third vs first-third median) | 99.3% | slope 290 KB/sample |
| `vmhwm_kb` | **DRIFT** | 8 320 → 86 020 KB | 86 020 KB (≈84 MB) | +38.5% | 99.3% | slope 314 KB/sample |
| `fds`      | BOUNDED | 11 → 12 | 13 | +0.0% | — | **no fd leak** |
| `threads`  | BOUNDED | 10 → 9 | 10 | +0.0% | — | — |
| `panic_total` | BOUNDED (0) | 0 → 0 | 0 | — | — | zero panics across run |

Load over the run: `grpc_h3_sustained` ok=2 966 903 / err=1; `grpc_h3_churn` ok=1 981 432 / err=0
(**≈4.95 M gRPC-over-H3 RPCs, err≈0**). RSS climbed monotonically as a staircase
(8→23→31→39→53→80 MB observed in heartbeats) **while per-request correctness held** — the
isolated `collected`-HashSet leak (S32 root cause). **Workstream A must flip `rss_kb`/`vmhwm_kb`
to BOUNDED with this same load.**

**Phase 0 verdict: COMPLETE — base is honest-green (×3 1512/0/18, clippy/fmt clean, main CI
green) and the sc9 pre-fix DRIFT staircase is captured as the negative control.**

CI gate set (must stay green — S34/S35):
`ci.yml` {fmt, clippy, test, msrv, release-build} +
`prod-readiness-gates.yml` {gate-07 cargo-deny, D6 per-module coverage ≥80%,
D4 h2spec/h3spec, D3a chaos, D5 docker build+smoke+trivy, D2 XDP verifier} +
`scheduled-scans.yml` {geiger, machete}.

### Workstream-A surface (recycling) — confirmed during Phase 0
- `crates/lb-quic/src/conn_actor.rs::run_actor` holds `h3: Option<quiche::h3::Connection>`.
- New request = `poll_h3` `Event::Headers` (conn_actor.rs:1107); completion = `Event::Finished` (:1321).
- `graceful_h3_shutdown` (:882) already emits `conn.close(true, H3_NO_ERROR)` + drains.
- quiche 0.29.1 exposes `quiche::h3::Connection::send_goaway(conn, id)` (h3/mod.rs:2159).
- No request cap, no GOAWAY today → one H3 connection accumulates streams unboundedly
  (quiche `StreamMap::collected` insert-only — S32 root cause), the RSS staircase.

---

## Phase 1 — (A) connection recycling — VERIFIED (coverage pending), on `4480fb83`

**Design (owner-decided, single-sourced in the shared H3 actor — covers gRPC-H3 / WS-H3 / plain-H3):**
a **separate, H3-tuned config knob** `max_requests_per_h3_connection` (lb-config `RuntimeConfig`,
u32, **default 1000**, `0`=disabled→documented as re-opening the leak/DoS vector, internal-only).
The H3 actor counts each new request stream; at the cap it sends an H3 **GOAWAY** (RFC 9114 §5.2),
rejects newer streams (`H3_REQUEST_REJECTED` 0x010b), drains in-flight requests/tunnels, then
`graceful_h3_shutdown` → the connection recycles and quiche's per-connection `StreamMap::collected`
set is freed (S32 root cause). A two-flag latch (`goaway_pending` flips at the cap → stop admitting
immediately; `goaway_sent` flips only when the frame lands → gates the drain-close) + a per-tick
`try_send_pending_goaway` retry handles `send_goaway` `StreamBlocked` even if the client goes silent.
Owner chose a separate knob (not reusing the H1/H2 `max_keepalive_requests` 100) because an H3
recycle pays a full QUIC+TLS handshake; tokio-quiche ships `None`/uncapped, so 1000 is the anchor.

Commits: `ea4d87bd` (impl) + `c00bfa19` (tests) + `4480fb83` (GOAWAY `goaway_pending` hardening).
Files: lb-config knob (+122), lb-observability `quic_h3_recycle_metrics.rs` (+96,
`h3_goaway_sent_total`/`h3_connections_recycled_total`), conn_actor (+enforcement), listener/router/main
threading. New tests: `h3_connection_recycle_e2e.rs` (4) + 4 config + 2 metric.

**The leak fix (R13 — the negative control flipped):** `sc9_grpc_h3` isolated, **1800 s / 151
samples, COMPLETED** (R15), at default cap 1000 → **`overall=BOUNDED`**
(`audit/soak/s36-soak-data/sc9-postfix/`).

| metric | pre-fix (Phase 0) | post-fix (`4480fb83`) |
|---|---|---|
| `rss_kb` | **DRIFT** 8 320→81 924 KB, +36.3%, slope **+290**, 99.3% monotone | **BOUNDED** ~22 MB flat, −0.2%, slope **−0.35** |
| `vmhwm_kb` | DRIFT, peak **86 020 KB (~84 MB)** | BOUNDED, peak **23 432 KB (~23 MB)** |
| `fds`/`threads`/`panic` | bounded / bounded / 0 | bounded / bounded / **0** |

**Recycle-count proof + err cross-check (airtight):** ~5.0 M RPCs; sustained ok=2 997 771
**err=2994 ≈ 2998 = sustained_ok ÷ cap(1000)** → err IS the recycle artifact (the non-GOAWAY-aware
sustained client bails+reconnects once per recycle), NOT a regression; churn err=0, no storm. Mid-run
`/metrics`: `h3_goaway_sent_total == h3_connections_recycled_total` (every GOAWAY → clean recycle).

**R3 per-request correctness (independent):** recycle e2e **4/4** (branch provably TAKEN — F-CAP-1
trap avoided; in-flight-at-boundary drains **not** RST; `cap=0` byte-identical; fresh conn serves after
recycle through the real `QuicListener`). Fast smoke **~96/0** (gRPC-H3 16/0, plain-H3 cells incl. the
R8 backpressure/bounded gauges, authority, ws_tunnel seam, quic_listener e2e). **R8 preserved.**

**Gate (lead-run; author=recycle-eng ⇒ author≠verifier):**
`cargo test --workspace --all-features --no-fail-fast` **×3 = 1522 / 0 / 18 ALL-PASS** (identical, 248
binaries; +10 vs Phase-0 1512 = the new recycle/config/metric tests, additive). `clippy --all-targets
--all-features -D warnings` **clean**; `fmt --check` **clean**.

**Coverage — charter D-6 gate PASS** (full-workspace `cargo llvm-cov nextest --workspace --all-features`,
`audit/ops/s36-aimpl-cov/`): "31 hot-path modules passed, 0 below". The two charter hot-path modules
this change touches are both ≥80%: **conn_actor.rs 84.79%** (803/947), **listener.rs 86.41%** (248/287).
New-code modules: **lb-config/lib.rs 91.92%** (the knob), **quic_h3_recycle_metrics.rs 92.59%** (the
metric). (A scoped lb-quic-only run first read conn_actor at 65% — a scoping artifact: conn_actor's
WS-tunnel/Mode-B paths are only exercised by workspace-level integration tests; the new recycle happy
+ drain paths are fully covered, only ~20 defensive error arms — StreamBlocked retry / IdError /
post-GOAWAY reject the well-behaved client never triggers — are uncovered, an acceptable module-level
fraction. R4: documented, not asterisked.)

**Phase 1 verdict: VERIFIED — leak FIXED (sc9 BOUNDED at cap 1000), per-request correctness intact,
gate green (×3 + clippy/fmt + D-6). Staged on `feature/ops-layer-s36 @ 4480fb83`; promotes at Phase 5
per the mission (Phase 1 ends commit/push, not a standalone main merge).**

> Process note (honest record): the verifier subagent completed the sc9 BOUNDED soak (13:05 UTC) but
> stalled without reporting — caught ~5.75 h later (idle-billing waste, no compute burned, result intact).
> Lead took over the remaining gate with harness-tracked jobs + a stall watchdog (now standing practice).

## Phase 2 — (B) config management — CARRIED to S37
## Phase 3 — (C) hot reload — CARRIED to S37
## Phase 4 — (D) latest-deps upgrade — CARRIED to S37
## Phase 5 — full re-validation + promote — N/A (A promoted standalone; see verdict)

Owner decision (2026-06-06): promote the fully-verified workstream A standalone now and carry
B/C/D to a fresh focused S37 session. Rationale: A is self-contained and load-bearing (belongs on
main, not stranded on a branch); B/C/D are substantial, and hot reload especially is
correctness-critical operational code (live-connection preservation, validate-before-apply,
rollback) that must not be authored in the exhausted back-half of a 10 h+ session that already
absorbed a ~6 h teammate-stall (the exact conditions a subtle reload race gets introduced —
violates the solid-before-prod bar). See `audit/ops/s37-handoff.md`.

---

## Verdict

**SESSION 36 — PARTIAL→COMPLETE for workstream A: connection-recycling leak FIXED and PROMOTED;
B/C/D carried to S37.**

Workstream A (H3 connection recycling) is fully verified and promoted to `main` (`--no-ff`):
- **CF-GRPC-H3-CHURN-RSS CLOSED as our-code-fixed.** The S32 "quiche `collected` leak" was our
  lifecycle gap (no per-connection request cap / no GOAWAY), not a quiche bug — the cap→GOAWAY→
  drain→recycle fix flips sc9 from DRIFT (8→80 MB staircase) to **BOUNDED (~22 MB flat)** at the
  default cap 1000. Also closes the internet-facing single-connection DoS vector.
- Per-request correctness intact (gRPC-H3/WS-H3/plain-H3): recycle e2e 4/4, smoke ~96/0, R8 held,
  cap=0 byte-identical. Gate green: ×3 1522/0/18, clippy/fmt clean, coverage D-6 PASS.
- New operator knob `max_requests_per_h3_connection` (default 1000, 0=disabled→internal-only).

Carried to S37 (well-scoped, fresh session): B config management, C hot reload, D latest-deps
(the held cluster + PR #224/#226), then full re-soak + the carry-forwards (CF-S27-2/hyper#4050,
F-ESC-1, CF-S35-T5-FLAKE, security audit, docs).
