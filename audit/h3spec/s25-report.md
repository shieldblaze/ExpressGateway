# SESSION 25 — CF-S22-QUICHE-H3-MIGRATION: E2 + delete lb-h3 + promote — REPORT

**Branch:** `feature/quiche-h3-migration-s23` · **Base tip:** `1c002f61` (S24 COMPLETE,
E1 migrated). **Main stays** `90915781` (S22-hardened stack) unless E1+E2+Phase-3 ALL
verify (R11; no half-migration promote).

## Phase 0 — baseline (behavioral reference) — COMPLETE
`cargo test --workspace --all-features` ×3 = **1451/0 / 1451/0 / 1451/0**, all exit 0,
deterministic allpass (completed logs `audit/s25-logs/phase0/baseline-run{1,2,3}.log`).
Matches the S24 reference exactly. The H3 wire tests are the no-regression safety net (R3).
Base tip confirmed `1c002f61` (NOT main); zero stray processes; disk monitored.

## INC-4 — E2 H3→H3 upstream CLIENT → quiche::h3 — IN PROGRESS (NOT green)

Migrated `stream_request_to_h3_upstream` (the shared connector for all three →H3 fronts:
H1→H3 `h1_proxy:2275`, H2→H3 `h2_proxy:2617`, H3→H3 `h3_to_h3_stream_resp:2930`) off the
hand-rolled `lb_h3` framing onto a `quiche::h3::Connection` **client**:
`with_transport` → `send_request` (returns the request `stream_id`) → `send_body` (raw
chunk bytes; quiche frames DATA — `encode_h3_data_frame` gone) → `poll` / `recv_body` →
`send_additional_headers(is_trailer=true)` for request trailers. Added
`build_client_h3_config` (symmetric to the server). KEEP-surface preserved: the
`H3RespOut` sink (`Wire`/`Decoded` + cap accounting) UNTOUCHED, the peek/413/502 pre-dial
paths, the bounded `body_rx`, `forward_req_trailers`, the F-S7-6 idle deadline, the single
park point, one-request-per-pooled-conn. `j2_req_event_action::SendData` now carries raw
bytes. **Deleted** the now-dead recv-parser (`parse_frame_header`, `check_block_len`,
`classify_recv_err`/`RecvErrClass`, `FRAME_DATA`/`FRAME_HEADERS`) + their moot unit tests.
lb-quic compiles, clippy `-D warnings` + fmt clean. (WIP commits `7b57ca68`, …)

### H3→H3 wire suite (BOTH migrated endpoints E1+E2) — 22/24 → fixes in flight
PASS (the load-bearing surface): **F-MD-4 MIRROR** single-shot
(`_upstream_reset_midbody_resets_client_no_fin` — backend reset → client reset, no FIN),
BOTH **R8 gauges** (`_response_memory_bounded_through_stalled_client`,
`_request_memory_bounded_through_stalled_backend` — 4 MiB bodies, flat ≤ ceiling,
non-vacuous), **backpressure** (`_backpressure_stalled_client_pauses_upstream_read`),
byte-identity (GET/req-body/full-head), response trailers + pseudo-trailer reject,
request-leg F-MD-4 (`_client_reset_midrequest_*`), and ALL malformed-backend cases now
**quiche-owned** (`_data_before_headers`, `_invalid_qpack`, `_oversized_*`,
`_unknown_response_frame_skipped`) — PASS.

Two initial failures, both rooted in **quiche-0.28 recv-path capability gaps** (NOT bugs
in the migration; confirmed from quiche source + a ground-truth event trace). NEITHER
backend mode sends `content-length`.

1. **`_empty_data_frame_skipped_then_body`** (backend: head + empty DATA(0) + real body
   + FIN). quiche-0.28 does **not re-arm** the `Data` event after a 0-length DATA frame
   while the stream stays readable (`stream.rs::try_consume_data` only `reset_data_event`s
   when `!stream_readable`); `poll` advances the real DATA frame into `State::Data` WITHOUT
   emitting a fresh `Data` event, and — the actual stall — after a post-poll drain relays
   that body and finishes the stream, the `Finished` event sits queued needing no socket
   I/O while the loop **parks on the socket** → 25 s timeout. **FIXED:** (a) an
   unconditional post-poll **PASS-3** `recv_body` drain (server `poll_h3` PASS-1 analogue)
   relays the stranded body; (b) a per-tick `progressed` flag → `continue 'evloop`
   (re-poll instead of park) so the queued `Finished`/trailer event is collected without a
   timeout. **PASSES** (run5/run6).

2. **`_upstream_premature_eof_mid_data_no_clean_fin`** (backend: head, NO content-length, +
   a DATA frame header declaring 4096 bytes but only 16 bytes of payload, then a clean QUIC
   FIN). The hand-rolled parser caught this via H3-frame-completeness (`InData{remaining>0}`
   at FIN ⇒ PrematureEof). **quiche-0.28 does NOT enforce DATA-frame completeness at FIN
   (RFC 9114 §7.1)**: `process_finished_stream` (`mod.rs:2845`) pushes the stream to
   `finished_streams` regardless of an incomplete `State::Data`, `poll` returns a clean
   `Event::Finished` (both pops re-check only for *reset*), `recv_body` discards the `fin`
   flag, and there is **no public quiche API** to observe "finished mid-frame". This is a
   genuine `quiche::h3`-client cannot-express-a-KEEP-surface-need → **ESCALATED (R7/exit-d)
   before deleting hand-rolled code.** Otherwise green: h3h3 23/24, h1h3+h2h3 25/25, R8 (both
   directions), F-MD-4 mirror, backpressure all pass.

### OWNER RULING — Option 1 (accept + content-length defense-in-depth + re-scope) — DONE
The gap requires a malformed/buggy BACKEND (trusted upstream, not an untrusted client);
HTTP/3 streams are isolated so truncation CANNOT desync/smuggle (the structural reason the
S22 #12-15 findings were security-critical does NOT exist here); RESET-based truncation is
still caught (F-MD-4 mirror, all 3 →H3 cells); content-length responses self-detect. ⇒ a
LOW-severity RFC 9114 §7.1 robustness gap, not a security hole. Implemented:
- **Content-length truncation guard** (`h3_bridge.rs`, migrated E2): on a clean `Finished`,
  if a declared `content-length` is present and `body_relayed < content-length` (and the
  response is not bodiless — HEAD / 1xx / 204 / 304), RESET downstream (`PrematureEof`),
  never a clean End. Recovers the COMMON truncation case. **Proven load-bearing** by a NEW
  test `h3h3_e2e_content_length_truncation_resets_no_clean_complete` (+ `HeadCLThen
  TruncatedData` backend: declares 4096, sends 16, clean FIN → gateway resets).
- **Re-scoped** the ONE no-content-length test →
  `_no_cl_truncated_data_delivered_quiche_028_frame_completeness_gap`: asserts the migrated
  documented behaviour, with the §7.1 gap + threat model + compensating guard + the
  **CF-QUICHE-FRAME-COMPLETENESS** carry-forward (tied to CF-QUICHE-UPGRADE: re-tighten when
  a quiche version enforces §7.1) named in the doc. A documented, compensated re-scope —
  NOT a silent weakening.
- The §7.1 gap is documented as a known quiche-0.28 limitation alongside #1-10 + the v1
  release-note item (see §Carry-forward / release notes), and the CL guard is independently
  verifier-confirmed to FIRE (author ≠ verifier).

**INC-4 wire state:** h3h3 **26/26** (incl. the re-scope, the CL-guard test, the empty-DATA
fix, the F-MD-4 mirror burst R13 b+c); h1h3+h2h3 verify **25/25**; lb-quic lib **84/84**;
clippy `-D` + fmt clean. *(report continues — INC-4 full gate + independent verify, INC-5,
Phase-3 — below.)*
