# S23 — quiche::h3 Migration: Phase 0–1 + INC-0/INC-1 (go/no-go) — REPORT

**Verdict: SESSION 23 PARTIAL — H3-front migration validated & de-risked, not yet code-migrated. Main retains the S22-hardened hand-rolled stack (NOT promoted, per R11). Branch `feature/quiche-h3-migration-s23` carries a verified increment (INC-0) + a proven pattern (INC-1 GO) + a sharpened INC-2…INC-5 map to S24.**

**Date:** 2026-05-31 · **Branch:** `feature/quiche-h3-migration-s23` (tip `2b5cbbb9`) · **Base:** main `90915781`.
**quiche:** `0.28.0` (the tree's pin; the prompt said 0.29.1 — `quiche::h3` ships in 0.28.0 and the entire target API is present there; CF-QUICHE-UPGRADE is the orthogonal bump).

This was a **high-risk rip-and-replace**, not a feature build. The mission and owner gate explicitly framed an honest PARTIAL (that does **not** promote a half-migration) as an acceptable outcome. The session executed Phase 0 (baseline) + Phase 1 (the gate-to-deletion plan), then — per the owner's gate decision — landed **INC-0** (config infra, deletes nothing) and ran **INC-1** (a throwaway go/no-go experiment) to validate the load-bearing assumption *before* committing to the deletion-bearing rewrite. INC-1 is **GO**, and two follow-on experiments de-risked INC-2's exact shape. The verified increment + the proven pattern is precisely the owner's stated success bar for a GO-PARTIAL session.

---

## 1. Phase 0 — baseline (the behavioral reference)

`cargo test --workspace --all-features` = **1454 passed / 0 failed** (completed log `s23-baseline-run1.log`, exit 0), byte-identical to promoted main `90915781`. Branch cut + pushed; no S22 strays; disk 27 GB free. This is the behavior the migration must preserve bit-identically on the KEEP-surface (R3).

## 2. Phase 1 — migration plan (gate to deletion) + the headline scope finding

`audit/h3spec/s23-migration-plan.md` (committed `029de504`). The quiche::h3 0.28.0 target API was verified present at the source level (`with_transport`/`poll`/`recv_body`/`send_body`/`send_response`/`send_additional_headers`; the RFC-enforcing internal `conn.close(true,…)`; the R8 backpressure primitives). Mode A/B proven independent of `lb-h3` (dispatch `conn_actor.rs:192-193`) — cannot regress by construction.

**Headline scope finding (revises the S22 investigation):** the migration is **TWO** hand-rolled H3 endpoints on `lb-h3`, not one:
- **E1** = server termination front (`conn_actor::poll_h3` + `StreamRxBuf` + the pre-encoded `RespEvent::Bytes` egress) → quiche::h3 **server**.
- **E2** = H3→H3 upstream **client** (`h3_bridge::stream_request_to_h3_upstream`, ~3448–4250, ~800 LOC, opens its own control stream + SETTINGS at `h3_bridge.rs:4566`) → quiche::h3 **client**.

Deleting `lb-h3` requires migrating **both** ⇒ realistically **2–3 sessions**. The owner approved Option 1: INC-0 + INC-1 experiment first, then reassess with evidence.

## 3. INC-0 — `quiche::h3::Config` builder (infra, no live-path change)

`crates/lb-quic/src/h3_config.rs` + `lib.rs` wiring (committed `46ceefa5`). `build_server_h3_config()` with defaults **matching current hand-rolled behaviour**: `max_field_section_size = 1<<20` (the `lb_h3::decode_frame` HEADERS cap), `qpack_max_table_capacity = 0` + `qpack_blocked_streams = 0` (static-only QPACK, as `lb_h3::qpack` is today). Gated `quic-terminate`; unit test builds the config. **Deletes nothing, wires into no live path** (conn_actor unchanged). `clippy -p lb-quic --all-targets --all-features -D warnings` + `fmt` clean. This is the one **verified production increment** this session.

## 4. INC-1 — throwaway go/no-go experiment — **VERDICT: GO**

`crates/lb-quic/tests/inc1_quiche_h3_experiment.rs` (committed `5805caee` → `2b5cbbb9`). Four real-wire experiments (real quiche handshake, real loopback UDP, real H3/QPACK via quiche::h3, real TCP backend). Evidence: `s23-inc1-evidence.log`, **4/4 pass**.

| # | Experiment | Result |
|---|---|---|
| 1 | `inc1_go_roundtrip_and_r8_bounded` | 1 MiB **and** 4 MiB request bodies round-trip **verbatim** through `with_transport` → real TCP backend → back. **R8 bound:** peak server-held = **147 456 B (= channel ceiling 9×16 KiB) for BOTH bodies — 4× body ⇒ delta 0 B.** A buffering relay would show peak ≈ body. Body-size-independent, non-vacuous (channel genuinely saturated). NOT recv_body-into-unbounded-buffer. |
| 2 | `inc1_go_backpressure_unread_pauses_peer` | Server never `recv_body` ⇒ client pushed only **65 459 B of 1 MiB** (≈ the 64 KiB stream window) before flow control blocked it. **Server-not-reading pauses the peer** — R8 backpressure attaches. |
| 3 | `inc1_interop_handrolled_lbh3_client_vs_quiche_server` | A quiche::h3 server round-trips **both directions** with the **existing non-conformant hand-rolled lb_h3 client** (no control stream/SETTINGS): quiche parses the request, lb_h3 decodes quiche's response verbatim. **⇒ migrating conn_actor keeps the 69 wire tests green — no 69-client rewrite inside INC-2.** |
| 4 | `inc1_hybrid_quiche_ingress_handrolled_egress` | quiche::h3 ingress (poll/recv_body + its own control/QPACK uni streams) **coexists** with raw `conn.stream_send` of `h3_bridge::encode_h3_response` bytes on the same connection. **⇒ INC-2 = ingress-only (small, standalone), egress restructure deferred to INC-3.** |

**Independent verification (author≠verifier, R5):** a separate reviewer re-ran INC-1 ×3 deterministically, confirmed it is a genuine wire test (nothing mocked), and adversarially confirmed the R8 proof is **non-vacuous** — the gauge faithfully mirrors the production `record_retained_for_stream` pattern (`conn_actor.rs:1396-1420`), no unbounded accumulation exists inside the relay boundary (the echo backend's `read_to_end` is legitimately the origin, outside the proxy under test), the round-trip is full-body (truncation would fail `echo==body`), and the backpressure cap (65 459 B) is exactly the configured 64 KiB per-stream window. **Verdict: GO, no hole found.**

**Conclusion:** `with_transport` interoperates in the proxy topology, AND the R8 bounded relay + end-to-end backpressure attach to `recv_body`/`send_body` **in practice**. The rip-and-replace is unblocked; `lb-h3` is still deleted only when a migrated path is gate-green (R11/R12).

## 5. Gate / hygiene

Branch changes are **additive** (INC-0 infra not wired to the live path; INC-1 tests) — the live H3 path is byte-identical to the 1454/0 baseline (R3 no-regression by construction). A full-workspace `--all-features` no-regression gate is run as the confirming evidence (`s23-noregress-gate.log`; result read only from the completed run per R15 — expected ≈ 1454 baseline + INC-0 unit (+1) + INC-1 experiments (+4)). clippy `--all-features -D warnings` + fmt clean throughout. **Not promoted** (R11 — PARTIAL): main keeps the S22-hardened hand-rolled stack.

---

## 6. S24 handoff — the sharpened increment map (proven pattern in hand)

The migration target, the KEEP/MIGRATE/DELETE split, and the R8 attach pattern are now **proven**, not assumed. Remaining work, in order, each individually gate-green against the 69 wire tests:

- **INC-2 (E1 ingress) — proven feasible as ingress-ONLY.** In `run_actor`, hold `Option<quiche::h3::Connection>`; `with_transport(&mut conn, &build_server_h3_config())` once `is_established()`. Rewrite `poll_h3`'s `conn.readable()`+`stream_recv`+`StreamRxBuf::feed` loop into an `h3_conn.poll(conn)` event loop: `Headers{list}`→`validate_request_pseudo_headers`(KEEP)→authority(KEEP)→the existing H1/H2/H3 spawn blocks (KEEP) + body-channel register; `Data`→`recv_body` into the bounded request-body channel **with the "don't read while channel full" gate** (the INC-1-proven R8 pattern, replacing `drain_body_stream`); `Finished`→`ReqBodyEvent::End`; `Reset(code)`→`ReqBodyEvent::Reset` (the **F-MD-4 mapping point**). DELETE: `StreamRxBuf::feed`/`feed_body`/`FeedError`, the uni-stream drain, `decode_into_pending`/`flush_pending`. Re-point the request memory gauge (`record_retained` reads `StreamRxBuf::retained_bytes` → use the fixed `recv_body` buffer bound). **Egress untouched** (hybrid proven, Exp 4). Gate: 69 wire tests + R13 RST burst + R8 request gauge.
- **INC-3 (E1 egress).** `RespEvent::Bytes(pre-encoded)` → decoded `Head/Body/Trailers/End/Reset`; re-point `drain_streams_to_conn`/`drain_resp_channels`/`StreamTx::Progressive` to `send_response`/`send_body`/`send_additional_headers` (partial-write/`Done` retry = the egress R8 gate, Exp 1 pattern); delete `encode_h3_*` frame builders. Gate: 69 wire tests + R8 response gauge + R13.
- **INC-4 (E2 upstream client).** Migrate `stream_request_to_h3_upstream` (+ the sibling pump) to a client `quiche::h3::Connection` (`with_transport` on the upstream `qconn`, `send_request`/`send_body`/`poll`/`recv_body`), feeding the same `H3RespOut`/`RespEvent` sink. Gate: H3→H3 suite + its R8 gauges + the H3→H3 RST burst.
- **INC-5 (delete).** Remove `crates/lb-h3` (~1.15k LOC + 26 tests) + dead protocol code; drop the workspace dep. Gate: workspace ×3 + clippy + fmt; LOC-delta confirmation. Gains QPACK Huffman (CF-S22-QPACK-HUFFMAN).
- **PHASE 3 (revalidate).** ×3 deterministic; R8 memory+backpressure re-proven; R13 F-MD-4 burst + negative control; **fresh h3spec** (carried #16–21/#23–25 now PASS by construction, #11–15/#22 still pass); **re-soak incl. a NEW H3-terminate scenario** (lb-soak has none today — must be added); scoped llvm-cov ≥80% session sub-metric.

**Resolved unknowns the plan flagged (so S24 starts de-risked):** (a) `with_transport` interops in our accept topology — YES; (b) R8 bound + backpressure attach to recv_body/send_body — YES, non-vacuous; (c) existing non-conformant test clients interop with a quiche::h3 server — YES (no 69-client rewrite); (d) raw hand-rolled egress coexists with quiche::h3 ingress — YES (INC-2 = ingress-only). The one assumption still on paper: that quiche::h3's internal `conn.close(true,…)` on frame-seq/QPACK violations produces the SAME h3spec verdicts as the S22 hand-rolled closes — that is the **fresh-h3spec** proof in Phase 3 (and the headline payoff: #16–21/#23–25 close by construction).

## 7. Carry-forwards (unchanged)

CF-QUICHE-UPGRADE (#1–10, quiche>0.28), CF-FCAP1-FLAKE (pre-existing H2-timeout race), CF-DEP-1 (Dependabot — owner), F-ESC-1 (multi-kernel CI), N-1 (jumbo-MTU), Mode A perf tiers, CF-S22-QPACK-HUFFMAN (closed by INC-5). Program-level S24+: WebSockets-over-H2/H3, gRPC-over-H3 — built on whichever H3 stack is current (hand-rolled until this migration completes).
