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

Two failures, both rooted in **quiche-0.28 recv-path capability gaps** (NOT bugs in the
migration; confirmed from quiche source). NEITHER backend mode sends `content-length`.

1. **`_empty_data_frame_skipped_then_body`** (backend: head + empty DATA(0) + real body
   + FIN). quiche-0.28 does **not re-arm** the `Data` event after a 0-length DATA frame
   while the stream stays readable (`stream.rs::try_consume_data` only `reset_data_event`s
   when `!stream_readable`), so `poll` advances the real DATA frame into `State::Data`
   WITHOUT emitting a fresh `Data` event ⇒ the real body would be stranded.
   **FIXED** (this session): an unconditional post-poll **PASS-3** `recv_body` drain (the
   server `poll_h3` PASS-1 analogue) relays a body that `poll` advanced into `State::Data`
   without a `Data` event. *(re-verify pending the next h3h3 run.)*

2. **`_upstream_premature_eof_mid_data_no_clean_fin`** (backend: head + DATA frame header
   declaring 4096 bytes but only 16 bytes of payload, then a clean QUIC FIN — a truncated
   DATA frame). The hand-rolled parser caught this via H3-frame-completeness
   (`InData{remaining>0}` at FIN ⇒ PrematureEof ⇒ never a clean complete response — the
   response-splitting / truncation guard). **quiche-0.28 does NOT enforce DATA-frame
   completeness at FIN**: `process_finished_stream` (`mod.rs:2845`) pushes the stream to
   `finished_streams` regardless of an incomplete `State::Data`, and `poll` returns a clean
   `Event::Finished` (both `finished_streams` pops re-check only for *reset*, never for an
   incomplete frame). `recv_body` discards the `fin` flag (returns only `usize`), and there
   is **no public quiche API** to observe "stream finished mid-frame". So the gateway
   relays a truncated upstream response to the downstream client as a clean complete
   200+FIN. **This is a genuine `quiche::h3`-client cannot-express-a-KEEP-surface-need
   (R7 / exit-d).** Scope: only the clean-FIN-mid-frame, no-content-length case — the
   common RESET-based truncation IS still caught (the H1/H2→H3 reset-based truncation
   tests pass) and H3 streams are isolated (no H1-style connection desync), so the
   real-world severity is low, but the property is bound by a case-7 test that R5 forbids
   weakening unilaterally. **Escalated to owner before deleting hand-rolled code.**

*(report continues — INC-4 re-verify, INC-5, Phase-3 — after the owner ruling)*
