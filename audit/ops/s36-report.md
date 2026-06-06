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
| `cargo test --workspace --all-features --no-fail-fast` ×3 | ⏳ running |
| main CI gate set green (GitHub) | ✅ prod-readiness-gates / CI / scheduled-scans / Dependabot all `success` |
| sc9 staircase reproduced (pre-fix reference) | ⏳ pending (after ×3) |

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

## Phase 1 — (A) connection recycling
_pending_

## Phase 2 — (B) config management
_pending_

## Phase 3 — (C) hot reload
_pending_

## Phase 4 — (D) latest-deps upgrade
_pending_

## Phase 5 — full re-validation + promote
_pending_

---

## Verdict
_in progress_
