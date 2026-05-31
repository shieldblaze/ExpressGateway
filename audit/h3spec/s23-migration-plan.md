# S23 — quiche::h3 Migration Plan (mapping + increment sequence)

**Date:** 2026-05-31 · **Branch:** `feature/quiche-h3-migration-s23` · **Base:** `90915781` (S22 promoted, h3spec-hardened).
**quiche:** `0.28.0` (NOTE: the S23 prompt says 0.29.1; the tree pins **0.28.0** — `quiche::h3` ships in 0.28.0 and the whole API below is verified present in 0.28.0; CF-QUICHE-UPGRADE is the orthogonal bump).
**Phase 0 baseline:** `cargo test --workspace --all-features` = **1454 passed / 0 failed** (completed log `s23-baseline-run1.log`, exit 0) — byte-identical to promoted main; this is the behavioral reference the migration must preserve.

> This document is the **gate to deletion** (mission Phase 1 check-in). No hand-rolled code is deleted or rewritten until it is approved. Every claim is grounded `file:line`.

---

## 0. HEADLINE SCOPE FINDING — there are TWO hand-rolled H3 endpoints, not one

The S22 investigation (`s22-h3-arch-investigation.md`) characterized the **server** termination front. Reading the bridge for this plan surfaced a **second** full hand-rolled H3 implementation that also sits entirely on `lb-h3`:

| # | Endpoint | Role | Code | Drives | Migrates to |
|---|----------|------|------|--------|-------------|
| **E1** | H3 termination front (downstream-facing) | **server** | `conn_actor.rs::poll_h3` + `StreamRxBuf` (ingress) + the `RespEvent::Bytes` pre-encoded egress (`stream_h1_response`/`stream_h2_response`/`H3RespOut`) | the **actor's** `quiche::Connection` | `quiche::h3::Connection` (server) via `with_transport` |
| **E2** | H3→H3 upstream client (origination) | **client** | `h3_bridge.rs::stream_request_to_h3_upstream` (`:3448`–`~4250`, ~800 LOC) — encodes request + opens its own control stream + SETTINGS (`:4566`) + decodes response, all hand-rolled on `lb-h3` | the **upstream pool's** `quiche::Connection` (`qconn_mut`, `:3571`) | `quiche::h3::Connection` (client) via `with_transport` |

**`lb-h3` cannot be deleted until BOTH E1 and E2 migrate.** E2 is comparable-scope to E1 (full request-encode + response-decode H3 client loop with its own timer/recv pump). This roughly **doubles** the irreducible bridge rewrite relative to the S22 headline ("re-point E1"). It is the single biggest reason this is a multi-session migration, not a one-session one.

Mode A (`passthrough.rs`) and Mode B (`raw_proxy.rs`) are **fully independent** of `lb-h3`/`StreamRxBuf` — verified: zero references; dispatch at `conn_actor.rs:192-193` returns into `run_raw_proxy_actor` before any H3 state is built. The migration **cannot regress Mode A/B by construction** (re-soak still exercises them for R3).

---

## 1. Verified migration target API (quiche::h3 0.28.0)

All confirmed present in `~/.cargo/.../quiche-0.28.0/src/h3/mod.rs`:

| Need | quiche::h3 API | mod.rs line |
|---|---|---|
| Config | `h3::Config` + `set_max_field_section_size` / `set_qpack_max_table_capacity` / `set_qpack_blocked_streams` | 565, 596, 603, 610 |
| Wrap the existing `quiche::Connection` | `h3::Connection::with_transport(conn, &cfg)` — **sends SETTINGS + opens local control + QPACK enc/dec streams itself** | 1060 |
| Drive ingress | `poll(conn) -> Result<(u64, Event)>` | 2030 |
| Events | `Event::{Headers{list,more_frames}, Data, Finished, Reset(u64), PriorityUpdate, GoAway}` | 755–804 |
| Read body (bounded) | `recv_body(conn, sid, out: &mut [u8]) -> Result<usize>` — caller-sized; `Data` event NOT re-armed until drained to `Done` → **not reading = no flow-control extension = peer paused** (the R8 backpressure mechanism) | 1761 |
| Send response head | `send_response(conn, sid, &[T: NameValue], fin)` | 1188 |
| Send body (bounded) | `send_body(conn, sid, body, fin) -> Result<usize>` — returns bytes written, `Done`/short on blocked stream, retry-when-writable (R8 egress backpressure) | 1511 |
| Send trailers | `send_additional_headers(conn, sid, &[T], is_trailer_section, fin)` | 1300 |
| Header type | `h3::Header::new(&[u8],&[u8])`; `NameValue::{name,value} -> &[u8]` | 717, 724/727 |
| RFC enforcement (closes #11,#16–22,#24) | `poll`→`process_readable_stream` calls `conn.close(true, code, …)` internally on control/QPACK/frame-seq violations | 2545, 2572, 2611, 2632, 2933, 2965 |

**The R8 backpressure primitives exist and are the same mechanism** the gateway hand-built — this is the load-bearing precondition (R8). `recv_body` is caller-sized (no forced buffering); `send_body` is partial-write/`Done` (egress pause). The migration must consume them this way and re-prove the gauges.

---

## 2. KEEP / MIGRATE / DELETE inventory (code-grounded)

### KEEP (do not touch — the proxy value-add)
- `validate_request_pseudo_headers` (`h3_bridge.rs:890–982`) — takes a **decoded** header list, framing-independent (#12–15). quiche::h3 deliberately does NOT validate pseudo-headers (`QH/mod.rs:758-759`).
- `H3Request`/`from_headers` (`:783-801`), authority sanitisation (`conn_actor.rs:904-917`).
- The R8 relay shape: per-stream bounded `mpsc` request-body + response channels, the §1.4.3 backpressure gate (`conn_actor.rs:403-498`), the retained-bytes gauges (`record_retained`/`record_resp_retained`, `h3_bridge.rs:728/765`).
- conn_actor↔backend wiring: H3→H1/H2/H3 cell selection, `TcpPool`/`Http2Pool`/`QuicUpstreamPool` dial paths, lifecycle/cleanup, `reap_client_cancelled_responses`.
- The H3→H1 and H3→H2 **backend** legs (`stream_h1_response`, `stream_h2_response`) **as backend I/O** — but their **H3-frame ENCODING tail changes** (see MIGRATE).

### MIGRATE (re-point off lb-h3)
**E1 ingress** — replace with `h3_conn.poll()`:
- `StreamRxBuf::feed` (`:419-473`, the HEADERS decode via `decode_frame`+`QpackDecoder`), `feed_body` (`:494-662`, the streaming DATA/trailer parser + `try_parse_frame_header`), `FeedError` (`:373-393`).
- `poll_h3`'s `stream_recv`+feed loop + the uni-stream drain (`conn_actor.rs:825-1236`), `drain_body_stream` (`:1246`), `decode_into_pending` (`:1335`).

**E1 egress** — the structural change. Today the bridge produces **pre-encoded H3 wire bytes** (`RespEvent::Bytes(encoded_frame)`) and the actor blind-`stream_send`s them (`drain_streams_to_conn` `StreamTx::Progressive`). After migration the actor must call `send_response`/`send_body`/`send_additional_headers`, so:
- `RespEvent` (`:204-214`) changes from `Bytes(encoded)` → `Head{status,headers}` / `Body(raw)` / `Trailers(list)` / `End` / `Reset`.
- `encode_h3_response`/`encode_h3_headers_frame[_full]`/`encode_h3_data_frame`/`encode_h3_trailers_frame` (`:1138-1290`) — **deleted**; their callers (`stream_h1_response` `:1678/1698/1802`, `H3RespOut::on_head/on_data/on_trailers` `:3170/3212/3248`, the inline 400/502/413 at `:2488/2745`) emit **decoded** `RespEvent` instead.
- `StreamTx::Progressive` + `drain_streams_to_conn` (`:560-675`) + `drain_resp_channels` (`:421`) — re-pointed to call `h3_conn.send_body` (with its partial-write/`Done` retry) instead of `conn.stream_send`; the §1.4.3 gate's "only refill an empty queue" rule maps to "only `recv` more when `send_body` accepted the prior chunk".

**E2 (H3→H3 upstream client)** — replace `stream_request_to_h3_upstream` (`:3448`–`~4250`) and the sibling upstream pump (`~2904`) with a `quiche::h3::Connection` (client, `with_transport(qconn, &cfg)` with `is_server=false`): `send_request` for the head, `send_body` for the request body, `poll`/`recv_body` for the response, `Event::Reset`/`Finished` for completion — feeding the SAME `H3RespOut`/`RespEvent` decoded sink the rest of the bridge consumes.

### DELETE (after E1+E2 migrate, all callers gone)
- Entire `crates/lb-h3` (frame 329 + qpack 515 + varint 151 + security 89 + error/lib = **~1151 LOC** + 26 unit/proptests).
- The E1/E2 protocol-layer code listed under MIGRATE once dead.
- The H3 error-code constants in `conn_actor.rs:62-109` (quiche::h3 supplies `to_wire()` codes) **except** any the KEEP-surface still raises itself (e.g. `H3_INTERNAL_ERROR` on the egress abort, `H3_MESSAGE_ERROR` on pseudo-header reject — those stay, the app raises them via `conn.close`/stream reset).

Target deleted LOC ≈ **1.2k (lb-h3) + the protocol half of poll_h3/h3_bridge** — confirmed by `git`-diff at the end, not bypassed-and-left-dead (R3/Phase-3 LOC delta).

---

## 3. Increment sequence (each individually gated; STOP at last-green)

> Ordering principle: install infra with **zero** behavior change first; migrate the **server ingress+egress (E1) as one connected unit** (it cannot be half-installed — `with_transport` takes over SETTINGS + control/QPACK streams, and ingress+egress share the one `quiche::Connection`); then E2; then delete. Each increment must keep the **69 H3 wire tests** green (the safety net) before commit.

- **INC-0 (infra, no behavior change).** Add `h3::Config` construction (industry-safe defaults matching current behavior: `max_field_section_size` = current `decode_frame` 1<<20 cap; static-only QPACK → `qpack_max_table_capacity=0`, `qpack_blocked_streams=0`). Plumb a constructed-but-unused `h3::Connection` is NOT possible without it taking over streams — so INC-0 is config + a feature scaffold only. Gate: workspace build + ×1. **Low risk.**

- **INC-1 (EXPERIMENT, throwaway, gates the whole plan).** On a scratch branch, drive **one** real H3→H1 wire test (`h3_h1_bridge_e2e`) through `h3::Connection::with_transport` + `poll` for ingress and `send_response`/`send_body` for egress, end to end. Purpose: prove (a) `with_transport` on our post-`accept_with_retry` `quiche::Connection` interoperates with our existing quiche **test client** (which currently speaks the hand-rolled wire — it may need its own client-side `quiche::h3` too); (b) the SETTINGS/control-stream the server now sends does not break the test client; (c) backpressure via `recv_body`/`send_body` actually pauses. **If the test client must also move to `quiche::h3`, that is in-scope and noted here.** This is the **go/no-go** for E1. Result documented; no production code kept.

- **INC-2 (E1 ingress).** Replace `poll_h3`'s recv/feed loop with an `h3_conn.poll()` event loop: `Headers`→`validate_request_pseudo_headers`(KEEP)→authority(KEEP)→spawn the existing bridge task (KEEP); `Data`→`recv_body` into the SAME bounded request-body channel (KEEP the channel + the gate); `Finished`→`End`; `Reset`→`ReqBodyEvent::Reset` (the F-MD-4 mapping point — quiche signals client RST as `Event::Reset(code)`, map it to the channel `Reset` exactly as today's `StreamReset` arm does). Egress temporarily still hand-rolled IF INC-1 proved raw `stream_send` coexists with quiche::h3 on a client-bidi send side; otherwise INC-2 and INC-3 fuse. Gate: full H3 wire suite + R13 burst.

- **INC-3 (E1 egress).** Change `RespEvent` to decoded variants; re-point `drain_streams_to_conn`/`drain_resp_channels` to `send_response`/`send_body`/`send_additional_headers`; delete the `encode_h3_*` frame builders. Re-prove R8 egress gauge + backpressure. Gate: full H3 wire suite + R8 response-memory proof + R13.

- **INC-4 (E2 upstream client).** Migrate `stream_request_to_h3_upstream` to a client `quiche::h3::Connection`. Gate: H3→H3 suite + its R8 request+response gauges + the H3→H3 RST burst (`h3h3_e2e_client_reset_midrequest_burst_current_thread`).

- **INC-5 (delete).** Remove `crates/lb-h3` + dead protocol code; drop the workspace dep. Gate: workspace ×3 + clippy + fmt; LOC-delta confirmation.

- **PHASE 3 (revalidate).** ×3 deterministic; R8 memory+backpressure re-proven non-vacuous; R13 F-MD-4 burst + negative control; **fresh h3spec** (carried #16–21/#23–25 now PASS by construction, #11–15/#22 still pass, + QPACK Huffman); **re-soak incl. a NEW H3-terminate scenario** (lb-soak has no H3-terminate scenario today — must be added, since this session re-points exactly that path); scoped llvm-cov ≥80% session sub-metric.

---

## 4. Risk register (the migration's central hazards)

| Risk | Why | Mitigation / gate |
|---|---|---|
| **R8 backpressure re-point** (most-at-risk per rules) | egress moves from opaque `stream_send(pre-encoded)` to `send_body`(partial/Done); a naive `recv_body`-into-unbounded-buffer reintroduces the buffering trap | INC-3/INC-4 re-run the EXACT body-size-independent retained-bytes gauges (`MAX_RETAINED_BODY_BYTES`/`MAX_RETAINED_RESP_BYTES`) under `--features test-gauges`; non-vacuous (assert the gauge is actually exercised, not 0) |
| **F-MD-4 RST↔EOF mapping** | quiche::h3 signals client reset as `Event::Reset(code)` / `recv_body`→`Err`; must still relay as a backend reset, never clean EOF (smuggling guard) | INC-2/INC-4 gate the 8 RST tests (incl. the two burst current_thread tests) + R13 ≥50-iter isolation burst + negative control |
| **Pre-encoded→decoded egress restructure** | every response encoder + `StreamTx::Progressive` + both drain fns change together | staged: INC-3 isolated to egress; the 69-test net is the diff oracle |
| **E2 second endpoint** (the scope surprise) | doubles the rewrite; the client `quiche::h3` role + its pump | isolated to INC-4; H3→H1/H2 unaffected; can land/stop independently |
| **Server now sends SETTINGS/control stream** | `with_transport` changes server wire behavior (a conformance *improvement*, but the existing quiche **test clients** may not expect it) | INC-1 experiment is the gate; test clients move to `quiche::h3` if needed (in-scope) |
| **`accept_with_retry` interop** | confirm `with_transport` accepts our retry-validated server `quiche::Connection` | INC-1 |
| **No H3-terminate soak scenario exists** | re-soak can't prove the re-pointed path without one | Phase 3 adds it to `lb-soak` |

---

## 5. Honest scope / effort assessment

- **E1 (server)**: INC-1 experiment + INC-2 ingress + INC-3 egress restructure — each a multi-hour, deeply-entangled rewrite of the most security-sensitive hot path, gated by the full 69-test wire suite + R8 + R13 at every step.
- **E2 (client)**: INC-4 — a comparable ~800-LOC rewrite the S22 headline under-counted.
- **Phase 3**: ×3 + a NEW soak scenario (net-new lb-soak code) + fresh h3spec + re-proven gauges.

This is realistically a **2–3 session** migration (consistent with the S22 investigation's own "multi-session rewrite of the H3-front" and the prompt's explicit PARTIAL exit). **R11 forbids promoting a half-migration** — main keeps the S22-hardened hand-rolled stack until E1+E2+Phase-3 are ALL green. INC-0 and the INC-1 experiment are the safe, high-value first steps that de-risk the rest without touching the working path; the go/no-go on the full rewrite is owner-relevant (it commits the session to a high-risk path that, by the rules, only promotes if it fully completes).

**Recommendation:** approve INC-0 (infra) + INC-1 (throwaway experiment) now — they delete nothing and produce the empirical go/no-go (does `with_transport` interoperate end-to-end with our accept/retry path + test clients, and does the backpressure hold). Decide on committing to INC-2…INC-5 (the deletion-bearing rewrite) after INC-1's result, with eyes open that full completion this session is unlikely and an honest PARTIAL (plan + INC-0/INC-1 evidence carried to S24) is an explicitly acceptable outcome that keeps the working stack on main.
