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

---

## INC-3 — E1 response egress → quiche::h3 — COMMITTED `660f7d21`

Restructured the H3 server-front RESPONSE egress off the hand-rolled pre-encoded
path onto `quiche::h3`. With INC-2 (ingress) this **completes E1**: both halves of
the server front now ride `quiche::h3::Connection`.

`RespEvent` changed `Bytes(pre-encoded)` → decoded `Head{status,headers}` /
`Body(Bytes)` / `Trailers(list)` / `End` / `Reset`. The three E1 response producers
(`stream_h1_response`, `stream_h2_response`, `H3RespOut::Wire`) emit decoded events;
the actor's `Progressive` egress (`drain_resp_channels` + `drain_streams_to_conn`)
holds a bounded `VecDeque<RespItem>` and drives `send_response` (once, `head_sent`
guard) / `send_body` (partial-write keeps the unsent tail at the queue front — the
egress R8 gate) / `send_additional_headers(is_trailer=true, fin=true)` (the trailing
field section is terminal). Clean end → `send_body(&[], true)`; abort →
`stream_shutdown(Write, H3_INTERNAL_ERROR)`, **never a FIN** (smuggle guard, unchanged).

**R8 re-proven on the ACTUAL migrated egress** (independently, lead + verifier):
`MAX_RETAINED_RESP_BYTES = 73856 B` on a 4 MiB response — FLAT across the stall
(mid == peak), 15.97× under ceiling, ~57× smaller than the body → body-size
independent. The `Progressive` queue holds only bounded decoded items; no
whole-response buffer was introduced.

**F-MD-4 preserved** (verifier-traced): the `reset` terminal is checked *before*
`ended`, so a mid-response abort always RESET_STREAMs and never FINs.

**Test-client adaptation** (S23 risk register, in-scope, no behaviour change): the
migrated egress QPACK-Huffman-encodes response header values; 8 hand-rolled wire-test
clients (7 in `crates/lb-quic/tests` + `tests/proto_translation_e2e`) now decode
response field sections via `quiche::h3::qpack::Decoder`. **No behavioral assertion
weakened** (verifier-confirmed by diff).

**Gate:** lb-quic 201/0; full workspace ×3 = **1451/0** allpass (`inc3-x3-meta.log`);
clippy `-D warnings` + fmt clean; R8 `inc3-r8.log`; independent verify
`inc3-verify.md` = **AGREE**.

---

## E1 COMPLETE — summary

The H3 server termination front (E1) is fully migrated to `quiche::h3::Connection`:
- **Ingress (INC-2 `f3e318f4`)**: `poll`/`recv_body` event loop; `StreamRxBuf` decoder
  (603 LOC) deleted; quiche owns #11/#16-22/#24 by construction.
- **Egress (INC-3 `660f7d21`)**: `send_response`/`send_body`/`send_additional_headers`;
  decoded `RespEvent`.
KEEP-surface (R3) preserved + re-proven at every step: pseudo-header validation
#12-15, authority sanitisation, R8 backpressure (request + response gauges flat,
body-size-independent), F-CAP-1 (64 MiB → 413), F-MD-4 (client RST / Finished-on-reset
→ Reset; egress reset → RESET_STREAM never FIN), trailers. Two independent verifies AGREE.

**LOC delta so far:** `StreamRxBuf` request decoder + moot unit tests deleted (~603 LOC,
INC-2). `encode_h3_*` response encoders are now **egress-dead** but RETAINED — the E2
upstream request encoder (`stream_request_to_h3_upstream`) and the `H3RespOut::Decoded`
arm (H1→H3 / H2→H3 L7 fronts) still call them; they delete with `lb-h3` in S25.

**NOT promoted (R11).** E2 is still hand-rolled, so `lb-h3` is not yet deletable and
this is a half-migration. `main` retains the S22-hardened stack.

---

## S25 HANDOFF

**E2 = the H3→H3 upstream CLIENT** (`h3_bridge.rs::stream_request_to_h3_upstream`,
~800 LOC hand-rolled on `lb_h3`, opens its own control stream + SETTINGS). This is the
second and final endpoint on `lb-h3`.

- **INC-4 (E2):** migrate `stream_request_to_h3_upstream` (+ the sibling upstream pump)
  to a `quiche::h3::Connection` **client** (`with_transport(qconn, &cfg)` with
  `is_server=false`; `send_request` for the head, `send_body` for the request body,
  `poll`/`recv_body` for the response, `Event::Reset`/`Finished` for completion),
  feeding the SAME `H3RespOut`/`RespEvent` decoded sink the rest of the bridge consumes.
  Gate: the H3→H3 suite + its R8 request+response gauges + the H3→H3 RST burst.
- **INC-5 (delete):** remove `crates/lb-h3` (~1.15k LOC + tests) + the now-egress-dead
  `encode_h3_*` + the protocol-layer code; drop the workspace dep. Gains QPACK Huffman
  (CF-S22-QPACK-HUFFMAN). LOC-delta confirmation.
- **Phase 3 (full re-validation):** ×3 deterministic; R8 memory+backpressure re-proven;
  R13 F-MD-4 burst + negative control; **fresh h3spec** proving the S22-carried
  #16-21/#23-25 now PASS by construction (+ #11-15/#22 still pass) — the headline payoff,
  since quiche's internal `conn.close(true,…)` replaces the hand-rolled closes; **re-soak
  with a NEW H3-terminate scenario** (lb-soak lacks one today — must be added, since this
  migration re-points exactly that path); scoped llvm-cov ≥80% session sub-metric.
- **Then PROMOTE** (`--no-ff` to `main`): only when E1+E2+Phase-3 are ALL green and
  `lb-h3` is deleted.

**Verdict: SESSION 24 COMPLETE — E1 migrated to quiche::h3 (ingress INC-2 `f3e318f4`
+ egress INC-3 `660f7d21`), gate-green (×3 1451/0, R8/R12/R13/F-MD-4 preserved+re-proven,
2 independent verifies AGREE), `main` retains the S22-hardened stack; E2 + Phase-3 full
re-validation → S25.**
