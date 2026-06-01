# INC-3 INDEPENDENT ADVERSARIAL VERIFICATION

**Commit:** 660f7d21 (`feat(s24 INC-3): restructure E1 egress to quiche::h3 (send_response/send_body)`) — confirmed HEAD.
**Scope:** E1 H3 server-front RESPONSE egress migrated from raw `stream_send` of pre-encoded `RespEvent::Bytes` to `quiche::h3::send_response`/`send_body`/`send_additional_headers`. `RespEvent` now decoded `Head/Body/Trailers/End/Reset`; new `RespItem` enum + `StreamTx::Progressive` queue.

## VERDICT: **AGREE** — INC-3 is correct and preserves the load-bearing properties (R8 bound + F-MD-4 truncation guard). No defect found.

---

## 1. HEAD
`git log --oneline -1` ⇒ `660f7d21 feat(s24 INC-3): restructure E1 egress to quiche::h3 ...`. ✓

## 2. lb-quic --all-features  (log: inc3-verify-test.log, TEST_EXIT=0)
**201 passed / 0 failed / 0 ignored** (independently aggregated across all test binaries + doctests).
Named tests confirmed PASS:
- `r2_response_memory_bounded_through_stalled_client`, `r3_slow_client_backpressures_upstream_read`, `c5_resp_retained_ceiling_is_sound_and_much_less_than_1mib` (the 17-test resp_stream binary = 17/17)
- `h3_h1_trailers_resp` PC2 binary, `h3_h2_stream` (g5 trailers + over-cap arms), `h3_h3` (reset/EOF/no-FIN cluster), `h3_h1_bridge`, `h3_graceful_close`
- Mode B `s16_*` (incl. `s16_b2_client_reset_does_not_become_clean_fin_upstream`) + `s19_*` (incl. zero-RTT rejection, two-connections) — all green; Mode B rides H3 termination unaffected.

## 3. Workspace H3-front integration  (log: inc3-verify-integ.log, INTEG_EXIT=0)
`proto_translation_e2e` **5/5** (incl. `proxy_h3_listener_h2_backend`), `quic_listener_e2e` **6/6** (incl. both http3-get-through-proxy + retry/zero-rtt). The ×3-caught regression site is green. ✓

## 4. clippy --workspace --all-targets --all-features -D warnings  (log: inc3-verify-clippy.log)
**CLIPPY_EXIT=0**, zero warnings. ✓

## 5. R8 ADVERSARIAL — the central risk (did the restructure reintroduce buffering?) — **NO**
Structural read of `conn_actor.rs`:
- **(a)** queue is `VecDeque<RespItem>` (conn_actor.rs:599) holding only bounded decoded items — `Head{status,headers}` / `Body(Bytes ≤ H3_RESP_CHUNK_MAX=8192)` / `Trailers`. **No `Vec<u8>` whole-response accumulator** anywhere in the Progressive path. ✓
- **(b)** `drain_resp_channels` (conn_actor.rs:480-487) keeps the "only refill an EMPTY queue / pull exactly ONE event" gate. Chain intact: non-empty queue ⇒ no pull ⇒ channel fills ⇒ producer `tx.send().await` blocks ⇒ upstream read pauses. Actor-loop ordering verified: gate+gauge at bottom (line 383), `drain_streams_to_conn` ships at top of next loop (line 242) — gauge recorded at the largest instant (after refill, before ship). ✓
- **(c)** `send_body` partial/short write (conn_actor.rs:749-754): `let _ = b.split_to(n); break;` — keeps the unsent tail at the queue FRONT, does **NOT** loop to force-drain. `Done`/`StreamBlocked` ⇒ `break` (line 745,755). ✓

**Independent R8 gauge (log: inc3-verify-r8.log, R8_EXIT=0), 4 MiB response:**
```
R2-EVIDENCE: max_retained=73856 B  ceiling=262656 B (C5 channel bound=65664 B)  body=4194304 B  margin=15.97x  retained/ceiling=0.2812
R3-EVIDENCE: mid_stall_retained=73856 B  peak_retained=73856 B  ceiling=262656 B  body=4194304 B
```
- **BOUNDED**: max_retained = **73856 B (~72 KB)** on a **4194304 B (4 MiB)** body ⇒ **~56.8× smaller than the body**, 0.2812× the ceiling. A whole-response buffer would read ≈ 4 MiB here.
- **NON-VACUOUS**: 73856 > 0 (test asserts `max_retained > 0`, line 1319-1320); matches `depth 8 × (chunk 8192 + hdr) + queue`.
- **BODY-SIZE-INDEPENDENT**: R3 mid_stall == peak (73856 == 73856) under a stalled client; bound = constant `depth×(chunk+hdr)`; test asserts `ceiling*8 <= body` (line 1272-1277, 15.97× actual) AND `max_retained <= ceiling` (line 1324-1325). The proof is non-vacuous.

**R8 conclusion: the egress did NOT reintroduce buffering. The bound is flat, non-vacuous, body-size-independent.**

## 6. F-MD-4 / SMUGGLE ADVERSARIAL — **CLEAN, no truncated-as-complete path**
Reset terminal path = `conn.stream_shutdown(sid, quiche::Shutdown::Write, H3_INTERNAL_ERROR)` (conn_actor.rs:800), **never** a FIN. Full trace of every abort:
1. Producer `RespEvent::Reset` (every stream_h1/h2_response abort) ⇒ `reset=true` (line 502) ⇒ RESET_STREAM.
2. Producer **Disconnected with no prior End** ⇒ `reset=true` (line 504-509, explicit "NEVER FIN a possibly truncated body").
3. Egress `send_*` hard error ⇒ `reset=true` (lines 738/758/788) ⇒ RESET_STREAM.
4. Client STOP_SENDING/RESET ⇒ `reap_client_cancelled_responses` drops rx+StreamTx (line 428-430), never FIN / never RESET (ClientGone).
5. **Ordering guard**: `if *reset` (line 795) is checked BEFORE `else if *ended` FIN (line 808) — reset always wins; FIN reachable ONLY when `!reset && ended && queue.is_empty()`. A post-head body truncation ⇒ head (200) already sent, then RESET_STREAM with no FIN = correctly signals incomplete, never clean completion.
Exercised by passing tests: `c3_malformed_chunked_responses_reset_never_forward_truncated` (incl. case-4 trailing-junk-never-smuggled), `r5_upstream_reset_midresponse`, `r6_client_cancel_midresponse`, `c2_every_abort_variant`, `s16_b2_client_reset_does_not_become_clean_fin_upstream`, `h3h3_e2e_upstream_premature_eof_mid_data_no_clean_fin`, `h3h3_e2e_upstream_reset_midbody_resets_client_no_fin`.

## 7. TEST-INTEGRITY — CLIENT-DECODE adaptations only, no weakened assertions
`git diff f3e318f4..660f7d21 -- crates/lb-quic/tests tests/proto_translation_e2e.rs`:
- 7 of 8 changed files: pure swap `lb_h3::QpackDecoder::decode` → `decode_resp_qpack()` (= `quiche::h3::qpack::Decoder`, Huffman-capable) because the migrated egress Huffman-encodes. Frame-parse, `:status`/trailer-name/body-identity/FIN assertions all UNCHANGED.
- `h3_h2_stream_e2e.rs` g5 (the 164-line block): reworked because `cap` now counts DECODED payload/field bytes (actor encodes, not producer). The clean-trailer test still asserts `!Reset`, `End`, byte-identical body, trailer forwarded. The over-cap arm-(c) still asserts the load-bearing guard `saw_reset && !ended && had_head && body_bytes==16 && !saw_trailer` (Reset, NEVER End, full body before trailer trips cap) — and the cap sweep was WIDENED (16..=240 step 2 vs a fixed 11-element list), i.e. STRENGTHENED, not relaxed.
- **No assertion deleted or relaxed to force a pass.** ✓

## CONCERNS (non-blocking, cosmetic)
- **C1 (stale doc-comment, cosmetic):** `h3_bridge.rs` `H3RespOut::on_data`/`on_trailers` doc-comments (lines 2801, 2832) still read "`→ RespEvent::Bytes, byte-identical`". The actual code emits decoded `RespEvent::Body`/`RespEvent::Trailers` (lines 2806-2813, 2837-2846) — correct behaviour, the prose is just not updated. The `inline`/`on_head` doc-comments WERE updated. Recommend a one-line comment fix; **no behavioral impact**.

**Bottom line: AGREE. 201/0/0 unit + 11/0 integration + clippy clean; R8 gauge 73856 B flat on a 4 MiB body (non-vacuous, body-independent); F-MD-4 truncation guard intact (reset-before-ended ordering + Disconnected⇒reset); test changes are decode-adaptations only. One cosmetic stale-comment nit.**
