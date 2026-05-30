# SESSION 18 — report (CF-S16-RELAY-STALL → finish Mode B → promote)

Branch `feature/quic-proxy-s18` off `feature/quic-proxy-s17 @ 2c5d2f0b` (S17 PARTIAL tip,
NOT promoted). Box: 8 cores. `CARGO_TARGET_DIR=/home/ubuntu/Code/eg-target` per-command.
Standing rules R1–R15 in force; R15 (no result from an incomplete job) load-bearing.

---

## Phase 0 — baseline + hygiene  (IN PROGRESS)

### Hygiene
- Base tip confirmed `2c5d2f0b`; branch `feature/quic-proxy-s18` created + pushed.
- `ps aux`: no cargo/rustc/quic strays from S17 (only this session's claude proc). No
  leftover worktrees (only the main checkout). Disk: 36 GB free on `/` (≥25 GB ✔).
- Stray branches noted (not this session's to delete): `worktree-agent-aae742917af3c81c2`,
  5 stale `git stash` entries (R9 hazard, carried — owner of those branches drops them).

### Lead system re-grounding (read, not re-derived)
- quiche 0.28 `congestion/recovery.rs::set_loss_detection_timer` clears the loss timer when
  `bytes_in_flight.is_zero() && peer_verified_address` → "no PTO armed" ⟺ bytes_in_flight==0.
- `run_dual_pump`: `RELAY_TICK` short-wait clamp gated on `!streams.is_empty()`; after
  reclaim the client leg parks on `params.conn.timeout()` (~20s idle when no loss timer).
- **The test client driver never calls `client_conn.on_timeout()`** → a lost client→relay
  control frame (ACK / MAX_STREAM_DATA) is never retransmitted by the client. S17's probe
  never touched the client. → primary falsifiable target for Phase 1.

### Findings tracked into the gate (R4)
- CF-S16-RELAY-STALL (BLOCKER, OPEN) → Phase 1 three-bucket diagnosis.
- CF-S17-B3-VERIFY-DONE-UNWRAP (test fragility, OPEN) → Phase 0 fix, author≠verifier.

_(baseline ×3 + test-fix evidence appended as they complete — every claim cites a completed log.)_

---

## Phase 1 — three-bucket diagnosis  (PENDING)
## Phase 2 — stall fix + B4/B5/B6  (PENDING)
## Phase 3 — gates + promote  (PENDING)

## VERDICT: (pending)
