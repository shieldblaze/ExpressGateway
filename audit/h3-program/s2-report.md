# H3/QUIC Program — SESSION 2 Report

Branch: `feature/h3-quic-s2` (base: `feature/h3-quic-s1` @ a1ed83ea)
Team: lead (coordination/report, no feature code), builder-1, builder-2,
independent verifier. Author ≠ verifier enforced on every increment.
Plan-approval obtained before all source changes.

Commits (all pushed to origin/feature/h3-quic-s2):

| Commit | Increment |
|---|---|
| 423046ce | P0  raw-bytes H3→H1 body seam |
| f2af73c4 | P1-A incremental request-body streaming |
| f862403e | P1-B body error paths |
| 45c7ba61 | P1-C trailers no-regression + response lock-in + OOM cap |
| a93d60a1 | P2  D6 lb-quic coverage + fmt |

---

## PHASE 0 — body-corruption / smuggling fix — VERIFIED-PASS

`build_h1_request` returned a `String` and appended the request body via
`String::from_utf8_lossy`, corrupting non-UTF-8 bodies AND desyncing
byte-length vs `Content-Length` (a request-smuggling primitive). Fixed:
`build_h1_request` now returns `Vec<u8>` and appends raw body bytes via
`extend_from_slice`; the caller writes `&h1_request`.

**Binary round-trip proof** (`crates/lb-quic/tests/h3_h1_binary_body_e2e.rs`):
body `[0xFF,0xFE,0x00,0x80,b'A',0x01,0xFD]` driven through `h3_to_h1_roundtrip`
to a mock TCP backend; backend captures the bytes that physically crossed
the socket and asserts they equal the body **byte-for-byte** and that
`Content-Length == body.len()` (7). Independent verifier reproduced from
scratch AND empirically demonstrated the old path produced **15 bytes for
a 7-byte body** (`from_utf8_lossy` of the non-UTF-8 bytes) → confirmed the
corruption + CL desync the fix removes. S1 byte-identical-bodyless guard
intact; S2 marker still `#[ignore]`.

## PHASE 1 — H3→H1 request-body / DATA-frame forwarding

### P1-A — incremental request-body streaming — VERIFIED-PASS
First implementation buffered the whole body before dispatch; the program
owner rejected that, and the independent verifier **failed** the first
attempt (empirically: a single 1 MiB DATA frame retained 1 MiB —
`decode_frame` needs the whole frame before yielding anything; the gauge
was tautological). Reworked into a true streaming body-phase parser
(`BodyParse`: AwaitingFrameHeader / InData / InTrailers / InSkip): only
the varint frame header (≤16 B) and a bounded (≤64 KiB) trailer block are
ever buffered; DATA payload is emitted+drained per `feed_body`, never
retained whole. Per-stream bounded mpsc + `poll_h3`/`drain_body_stream`
backpressure gate (skip `stream_recv` while pending non-empty → QUIC
stream flow-control window not extended → client paused). Framing:
bodyless → byte-identical to S1; client `content-length` → passthrough
CL; else `Transfer-Encoding: chunked`. Cumulative `MAX_REQUEST_BODY_BYTES`
= 64 MiB cap (const, `// TODO(s3): config`) → 413.

Evidence (T1–T5, every body includes 0xFF/0x00/0x80): T1 multi-DATA-frame
binary reassembled byte-identical; T2 empty body byte-identical to S1
bodyless head; T3 zero-length DATA → no spurious chunk; T4 oversized →
413 + upstream not completed; **T5 non-vacuous memory-bound proof** — a
single 1 MiB DATA frame through a stalled upstream keeps max retained
per-stream ≤256 KiB (verifier independently reverted the parser and
reproduced the 1 MiB failure, then confirmed the fix passes).

### P1-B — body error paths — VERIFIED-PASS
Explicit `quiche::Error::StreamReset|StreamStopped` arms in `poll_h3`
(HEADERS phase — also fixes a `rx_by_stream` leak present at P1-A) and
`drain_body_stream` (Body phase): peer reset tears down all per-stream
state with no leak. On mid-body Reset, `h3_to_h1_stream` marks the
upstream non-reusable and returns **without** the chunked terminator /
completed CL body — a cancelled or oversized upload can never reach the
backend as a complete request (smuggling guard). Upstream reset mid-body
→ decoded H3 502. Tests: client RESET_STREAM mid-body (asserts egress
began, no terminator, + a second request on the same connection succeeds
→ no leak/corruption); upstream reset → decoded 502; oversized-after-
partial-chunked-send → decoded 413 + upstream aborted. Verifier
independently reproduced the smuggling guard (patched Reset arm to emit
the terminator → test fails → restored → passes).

### P1-C — trailers no-regression + response body — VERIFIED-PASS (interim on response streaming)
Request trailers (post-DATA HEADERS, RFC 9114 §4.1) are parsed and
bounded but intentionally **not** forwarded onto the H3→H1/1.1 request
(RFC 9110 §6.5 / 7230 §4.1.2; smuggling guard) — documented in code.
PC1 e2e proves binary body byte-identical at backend, request complete/
well-formed chunked, trailer names+sentinel absent from H1 head/body.
PC2 e2e: 256 KiB+ binary response reassembled byte-identical by the H3
client. `MAX_RESPONSE_BODY_BYTES` = 64 MiB OOM ceiling added behaviour-
preservingly (`read_h1_response` → thin wrapper over
`read_h1_response_capped`; byte-identical for all conformant responses;
>cap → clean 502). **PROTO-2-12 no-regression is test-proven**: the
verifier ran the cross-crate suites on this tree — `lb-l7
trailer_passthrough` 8/8, `bridging_h3_h1`, `lb-security
smuggle_strict_te` 16/16 — all green.

**Honest scope:** response egress is still fully buffered
(`read_h1_response` to EOF → single DATA frame via
`JoinHandle<Vec<u8>>`/`StreamTx`) — the mirror of the request-side flaw.
Genuine incremental response egress requires an actor-core rework
(channel-back-into-actor, incremental CL/chunked/EOF response reader,
progressive `StreamTx` with backpressure, P1-B-parity teardown) that does
not fit this session's budget without a risky half-rewrite. It is
**capped for OOM safety** and recommended as the **Session 3 headline**
(design + risk in `audit/h3-program/p1c-response-streaming-assessment.md`).
This is an honest interim, not claimed as streaming.

## PHASE 2 — regression + coverage + CI

| Gate | Result |
|---|---|
| `cargo fmt --check` | **PASS** (rustfmt-only changes confined to this session's lb-quic files; independently audited — no logic, no unrelated crate) |
| `cargo clippy --all-targets --all-features -- -D warnings` | **PASS** workspace-wide (⇒ whole workspace compiles clean) |
| Session-added H3→H1 body/DATA path line coverage | **83.5% — PASS** vs ≥80% bar (independently isolated per-function over P1 code; whole-file h3_bridge.rs 56.8% is pre-existing un-exercised upstream-roundtrip code, not the gate metric) |
| lb-quic full suite + cross-crate PROTO-2-12 + S1 e2e + round8 3/3 | **PASS**, S2 marker still `ignored` |
| D6 CI `lb-quic` coverage package (carried S1 TODO) | **DONE** |
| Full-workspace `cargo test` | **DISK-BLOCKED** — environmental, not a code defect |

The full-workspace `--all-features` *test* build ENOSPCs on this 28 GB
volume (the brief required ≥50 GB for exactly this). It is confirmed
environmental: the same-scope `--all-features` clippy passed workspace-
wide, so the workspace compiles clean; only test-binary linking exhausts
disk. The brief-mandated relevant subset (lb-quic full incl. test-gauges,
cross-crate PROTO-2-12, S1, round8) is green.

---

## What remains (honest)

1. **Full-workspace `cargo test` green** — disk-blocked here; must be run
   on a ≥50 GB volume or in CI. Workspace compiles clean (all-features
   clippy passed workspace-wide); the per-crate subset that interacts
   with S2 is green. Exit-condition (c) for this gate only.
2. **Incremental response-body egress** — explicitly deferred to S3
   (Session 3 headline); interim is correctness-tested + OOM-capped, not
   streaming. Design/risk recorded in the P1-C assessment doc.

## Session 3 build-plan (dependency-ordered)

1. **S3-A (headline): incremental response egress H1→H3.** Replace
   `JoinHandle<(u64,Vec<u8>)>` with a per-stream response channel into
   the actor; incremental `read_h1_response` state machine (Content-
   Length / `Transfer-Encoding: chunked` / EOF-delimited — note: chunked
   *response* parsing does not exist yet); progressive `StreamTx` with
   backpressure into the QUIC send window; P1-B-parity mid-response error
   /teardown (upstream reset, client cancel). Memory-bound proof mirroring
   T5 but on the response side. Drop `MAX_RESPONSE_BODY_BYTES` buffering
   once streaming lands.
2. **S3-B: full-workspace green on a ≥50 GB box / CI** — run
   `cargo test --workspace --all-features` + `cargo llvm-cov` D6 gate
   (now includes `--package lb-quic`) to completion; record numbers.
3. **S3-C: wire `MAX_REQUEST_BODY_BYTES` / `MAX_RESPONSE_BODY_BYTES` into
   listener/actor config** (currently consts with `// TODO(s3)`); make
   the oversized-cap e2e drive the real `poll_h3` path via config.
4. **S3-D: H3 request-trailer → HTTP/1.1 chunked-trailer forwarding**
   (currently intentionally dropped) — only if a downstream consumer
   needs it; keep the smuggling guard.
5. **S3-E: gRPC / WebSocket / SSE** now unblocked by correct binary-safe,
   incremental, backpressured body forwarding (request side complete;
   gate on S3-A for full bidirectional streaming).

---

## VERDICT

Phase 0 verified; Phase 1 request-body/DATA forwarding built and
independently verified as genuinely incremental with backpressure
(P1-A/B/C); Phase 2 fmt/clippy/coverage(83.5%)/CI green and S1 intact —
but the full-workspace test gate is environmentally disk-blocked and
incremental *response* egress is a deliberately-scoped Session 3 item.

**SESSION 2 PARTIAL — remaining: (1) full-workspace `cargo test` green
(disk-blocked, needs ≥50 GB box / CI; workspace compiles clean); (2)
incremental response-body egress (deferred to Session 3, OOM-capped
interim, design recorded).**
