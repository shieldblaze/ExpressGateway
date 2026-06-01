# S24 — quiche::h3 Migration: E1 ingress (INC-2) — REPORT (in progress)

**Branch:** `feature/quiche-h3-migration-s23` · **Base tip:** `d9417981` (S23 PARTIAL).
**Main stays** at `90915781` (S22-hardened stack) — NOT promoted (R11; half-migration).

## Phase 0 — baseline (behavioral reference)
`cargo test --workspace --all-features` ×3 = **1459 / 0**, allpass (completed logs
`audit/h3spec/s24-logs/baseline-run{1,2,3}.log`, exit 0). The 69 H3 wire tests are
the no-regression safety net (R3).

## INC-2 — E1 ingress → quiche::h3 (poll/recv_body) — COMMITTED `f3e318f4`
Replaced the hand-rolled `StreamRxBuf` request decoder + `conn_actor` stream_recv/
uni-drain loop with `quiche::h3::Connection`: `with_transport` once established
(quiche owns SETTINGS + the server control/QPACK uni streams), then a `poll()` event
loop draining `recv_body` into the existing bounded request-body channel. **Egress
untouched** (RespEvent::Bytes → raw `stream_send`; INC-1 Exp 4 coexistence proof).

KEEP-surface preserved + re-proven on the migrated path (R3, only framing changed):
- pseudo-header validation #12-15 (quiche doesn't; gateway does, sole ingress, R12)
- :authority sanitisation inline-400
- **R8 backpressure** — recv_body capacity-gated; t5 body-size-independent peak PASS
- **F-CAP-1** 64 MiB → 413 (cumulative `body_seen`, was inside deleted `feed_body`)
- **F-MD-4** incl. a measurement-found subtlety: quiche delivers `Event::Finished`
  (not `Reset`) for a stream RESET *after* its last DATA frame; the Finished arm now
  probes `conn.stream_recv(sid,&mut[])` → `StreamReset` ⇒ `ReqBodyEvent::Reset`, never
  a clean End. Proven non-vacuous (instrumented): cancelled→Reset (no chunked
  terminator; backend never sees a complete request), liveness→End→200.
- request trailers as a 2nd `Event::Headers` (pseudo rejected; stashed → End{trailers})

quiche now enforces #11/#16-22/#24 (control/QPACK/frame-seq closes) by construction.

**Deleted** (R5): `StreamRxBuf` + `RxPhase`/`BodyParse`/`BodyItem`/`FeedError` +
`feed`/`feed_body`/`try_parse_frame_header`/`retained_bytes`/`is_too_large` (~603 LOC
in h3_bridge) + their now-moot decoder unit tests (the live `chunk_decoder_*` tests
for the H1 `ChunkDecoder` are KEPT). Kept `MAX_FRAME_HEADER_BYTES` /
`MAX_TRAILER_BLOCK_BYTES` (the E2 upstream connector still uses them).
Two regressions found+fixed by measurement: a test-client uni-stream filter (S23-
flagged, in-scope) and the F-MD-4 Finished/Reset guard above.

**Gate:** lb-quic 201/0 (69 wire + R8 t5 + R13 burst + p1b F-MD-4); workspace ×3 =
**1451/0** allpass (1459 − 8 deleted decoder unit tests); clippy -D + fmt clean
(completed logs `inc2-x3-meta.log`, `inc2-wire4.log`, `inc2-clippy3.log`). diff:
conn_actor +384/-1305, h3_bridge -603. Independent verify: `inc2-verify.md`.

## Remaining (this session / next)
INC-3 = E1 egress restructure (RespEvent::Bytes pre-encoded → decoded → send_response/
send_body; R8 re-proof on the migrated egress is the central risk). E2 (upstream
client) + Phase-3 full re-validation (fresh h3spec, re-soak, lb-h3 deletion) → S25.
