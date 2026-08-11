# scanner-quic — crates/lb-quic

## Summary

files_scanned=46  slop=1  ambiguous=39  load_bearing_notable=30

(ambiguous breakdown: A deleted-symbol refs=13, B drifted line-refs=16, C stale status claims=6,
D stale version pins=1, E internal contradictions=3)

Read-only pass. No file in `crates/lb-quic/**` was edited.

Calibration held exactly as lead measured. Independent confirmation on this area:

* **Commented-out code: ZERO.** All 9 hits of `^\s*//\s*[a-z_]+.*[;{]$` are wrapped prose
  (second lines of sentences). Not one commented-out statement in 39.5k LOC.
* **Session markers: load-bearing.** Every `S\d+` / `F-S\d+` / `CF-…` marker inspected was a
  finding-ID attached to regression rationale.
* **Signature-restating docs on `pub` items: 3 total**, all `/// The socket address the listener is
  bound to.` on `local_addr()`. KEEP — `deny(missing_docs)`, removal breaks clippy.
* **`unsafe`: ZERO occurrences.** The brief's "lb-quic contains `unsafe` blocks" premise does not
  hold; the MANDATORY-preserve rule for safety comments is vacuous in this area.

**Lead's `deny(missing_docs)` correction: applied, re-checked, SLOP set unchanged (still 1).**
Re-swept the two categories that remain in scope after the correction:

* **Doc-comments on PRIVATE items** (where `missing_docs` does not fire) — 14 candidates found and
  reviewed individually. 13 are KEEP: three carry RFC 9001 §5.8 citations (`RETRY_KEY_V1`,
  `RETRY_NONCE_V1`, `RETRY_INTEGRITY_TAG_LEN`), the rest each add something the signature does not
  (`/// Hash a DCID into a Maglev pick.` names the algorithm; `/// Number of currently-queued
  payloads (never exceeds \`cap\`).` states the R8 bound; `/// Remove and return the front (oldest)
  payload` fixes FIFO order, which is what makes drop-newest meaningful). Exactly one is a pure
  restatement — see "considered and rejected" below; not proposed.
* **In-body `//` restatements** — none found beyond the phase-marker class already rejected below.

One nuance worth passing to the other scanners: `missing_docs` requires the **presence** of a doc
on an item, never any particular content. Deleting a stale *paragraph from inside* a doc block that
remains non-empty cannot trip the lint. The blanket "never propose a `///`/`//!` on a pub item" rule
is the right conservative default, but it over-fires on partial edits — which is the case below.

**The AMBIGUOUS section below is NOT a deletion backlog.** Every AMB item is load-bearing prose
that has gone factually STALE (names a deleted symbol, cites a drifted line number, or asserts a
status that is no longer true). They are flagged for CORRECTION, and every one must be KEPT as-is
by the sweeper. Deleting them would be an R3 knowledge regression; the flag is so the lead can
decide whether a separate accuracy pass is warranted.

## PROPOSED DELETIONS (SLOP)

Exactly one. It is a narrative status block whose claim is refuted, in writing, 1080 lines lower
in the same file, and which I verified false directly (`grep -c '#\[ignore' == 0`).

**Re-checked against the lead's `deny(missing_docs)` correction — it survives on two independent
grounds, either of which is sufficient:**

1. **The lint does not reach this file.** `crates/lb-quic/tests/*.rs` are separate integration-test
   binary crates; `#![deny(missing_docs)]` in `crates/lb-quic/src/lib.rs` does not propagate to
   them, and **none of the 32 test files declares it** (verified: `grep -ln missing_docs tests/*.rs`
   returns nothing). The only crate-level attrs in this file are two `#![allow(...)]` at :45-46.
2. **Even under the lint, this is a partial deletion.** The `//!` block spans :1-43. Removing the
   :31-40 paragraph leaves :1-30 and :41-43 (`//! Every response body carries the non-UTF-8 bytes
   0xFF 0x00 0x80 …`) in place, so the crate root stays documented. `missing_docs` fires on an
   item with *no* docs; it never mandates specific content.

`SLOP | crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs:31-40 | "SCAFFOLD STATUS (builder-1, parallel to P1-A verification): the harness … are `#[ignore]`d with an explicit reason…" | FALSE and self-refuted: the same file at :1118-1125 states "That claim is FALSE at the current tip … there are NO `#[ignore]` attributes anywhere in this file, and ALL of these tests RUN and PASS". Verified: zero `#[ignore]` in the file. Removing the paragraph loses zero correct information; the accurate status + the load-bearing FEATURE GATE note both live at :1115-1136 and MUST be kept. Delete lines 31-40 of the module doc ONLY.`

## AMBIGUOUS (KEEP + flag)

### A. References to symbols that no longer exist anywhere in the repo

`AMB | crates/lb-quic/src/h3_bridge.rs:378 | "shipped on the wire by [`request_h3_upstream`]" | `request_h3_upstream` is DELETED (0 definitions repo-wide). 6 sites in this file (:163,:164,:378,:428,:2032,:2121) + tests/h3_h1_resp_stream_e2e.rs:1936. Intra-doc links to a nonexistent item.`
`AMB | crates/lb-quic/src/h3_bridge.rs:2947 | "factored out (module-level, like J1's [`check_block_len`])" | `check_block_len` is DELETED with the hand-rolled H3 decoder (S25/INC-4). +6 sites in tests/h3_h3_stream_e2e.rs (:635,:994,:1171,:2365,:2411,:2413).`
`AMB | crates/lb-quic/src/h3_bridge.rs:910 | "[`encode_h3_headers_frame`]`(status, Some(n))` (byte-identical to the legacy HEADERS…)" | Deleted. Siblings `encode_h3_headers_frame_full` (:2024,:2118,:3829), `encode_h3_data_frame` (:2027,:2387,:2539), `encode_h3_trailers_frame` (:2028) are all deleted too. Also referenced from tests/h3_h3_stream_e2e.rs:2127-2129.`
`AMB | crates/lb-quic/src/h3_bridge.rs:124 | "a [`RespEvent::Bytes`] carries a pre-encoded H3 frame" | `RespEvent::Bytes` variant was removed at S24/INC-3 (RespEvent is now Head/Body/Trailers/End/Reset). Sites :124,:129,:908,:1218,:1233 + tests/h3_h1_resp_stream_e2e.rs:80. The surrounding memory-ceiling arithmetic is still correct — only the variant name is stale.`
`AMB | crates/lb-quic/src/h3_bridge.rs:100 | "event forwarded over the per-stream bounded body channel … to [`h3_to_h1_stream`]" | `h3_to_h1_stream` is DELETED (only `h3_to_h1_stream_resp` survives). 13 sites: h3_bridge.rs :100,:1379,:1491,:1496,:1516,:1669; conn_actor.rs:315; tests/h3_h1_stream_body_e2e.rs :5,:440; tests/h3_h1_stream_body_errors_e2e.rs :322,:449,:605; tests/h3_h1_trailers_resp_e2e.rs:4.`
`AMB | crates/lb-quic/src/h3_bridge.rs:45 | "today `read_h1_response` reads the whole upstream response to EOF into one `Vec` (FULLY BUFFERED …) so a malicious upstream could OOM the proxy" | `read_h1_response` does not exist; the path has been incremental/backpressured since S4/P1-B. This actively contradicts the R8 design the rest of the file documents.`
`AMB | crates/lb-quic/src/h3_bridge.rs:292 | "the `StreamRxBuf` internal buffer + every byte still queued in `body_pending` … right after `feed_body` decode" | `StreamRxBuf`, `body_pending`, `feed_body` were deleted at S24/INC-2. +7 sites: h3_bridge.rs:890,:1595; conn_actor.rs:309,:321,:1147,:1222; tests/h3_h1_stream_body_e2e.rs:22,:684,:721,:724.`
`AMB | crates/lb-quic/src/passthrough.rs:239 | "Tests poll this via [`PassthroughListener::dropped`]." | No such method exists; tests read `kv.value().dropped` off the FlowEntry directly.`
`AMB | crates/lb-quic/src/lib.rs:152 | "`pub` so the INC-1 go/no-go experiment (`tests/inc1_quiche_h3_experiment.rs`) can construct it." | That test file does not exist. Same dangling path at src/h3_config.rs:11.`
`AMB | crates/lb-quic/tests/h3_h1_trailers_resp_e2e.rs:15 | "the body-phase `BodyItem::Trailers` parser path does not crash or corrupt the DATA stream" | `BodyItem` is deleted (part of the removed StreamRxBuf decoder). +1 site at :549.`
`AMB | crates/lb-quic/tests/grpc_h3_e2e.rs:6 | "`conn_actor::poll_h3` (H2 branch) → `h3_to_h2_stream` → real hyper H2 gRPC backend" | `h3_to_h2_stream` is deleted; the live symbol is `h3_to_h2_stream_resp`.`
`AMB | crates/lb-quic/tests/h3_h1_stream_body_errors_e2e.rs:446 | "The proxy's `drain_body_stream` surfaces `Err(StreamReset)` …" | `drain_body_stream` is deleted (now `drain_request_body`). Same block cites the deleted `fail502!` macro at :605,:624.`
`AMB | crates/lb-quic/tests/h3_h3_stream_e2e.rs:1156 | "The gateway MUST skip it transparently (`RecvState::InSkip`) and resume parsing" | `RecvState`, `InSkip`, `InData`, `DEFAULT_MAX_PAYLOAD_SIZE` all belonged to the hand-rolled decoder deleted at S25/INC-4. 15 sites in this file. The tests still PASS (quiche::h3 skips unknown frames itself) — only the cited mechanism is gone.`

### B. Hardcoded line-number cross-references that have drifted (all verified pointing at unrelated code)

`AMB | crates/lb-quic/tests/h3_h3_stream_e2e.rs:309 | "(h3_bridge.rs :2876-2877 — the `if req.authority.is_empty()` TRUE branch)" | :2876 is now `Ok(()) => {` inside send_additional_headers error handling. 21 `h3_bridge.rs :NNNN` citations in this file (:309,:318,:372,:530,:620×3,:625,:629,:636,:642,:652,:685×2,:690,:1933,:1998,:2059,:2198,:2365,:2412,:2469,:2538,:2790); spot-verified :2806/:2876/:3062/:3189/:3284 — all land on unrelated code. Fix the pointers, keep the prose.`
`AMB | crates/lb-quic/src/h3_bridge.rs:2747 | "Mirror of conn_actor.rs:1269." | conn_actor.rs:1269 is a poll_h3 PASS-1 comment; the F-MD-4 Finished-on-reset probe it means is at conn_actor.rs:1571-1606.`
`AMB | crates/lb-quic/src/conn_actor.rs:574 | "Mirrors the S2 request-side StreamReset|StreamStopped arms (~conn_actor.rs:861/:944)." | :861 is a `send_response` error arm; :944 is inside the FIN block. Both drifted.`
`AMB | crates/lb-quic/src/h3_bridge.rs:3383 | "`H3ReqAbort`'s `Display`/`Error` impls (h3_bridge.rs:2145-2147)" | :2145 is inside `H3RespOut::on_head`; the impls are at :1779-1784.`
`AMB | crates/lb-quic/src/h3_bridge.rs:3403 | "(h3_bridge.rs:2267 empty-authority fallback; 2274-2277 pseudo-header skip + regular-header copy loop)" | Those lines are inside `on_head`; `h2_request_body_from_rx` is at :1891-1937. Same drift at :3447 ("2269").`
`AMB | crates/lb-quic/src/h3_bridge.rs:3465 | "`h3_to_h2_stream_resp` pre-dial inline arms (h3_bridge.rs:2351-2356 + the 2340 `inline` happy branch)" | :2351 is inside `stream_request_to_h3_upstream`'s doc; the fn is at :1959-2011. Same drift at :3515 ("2354-2356").`
`AMB | crates/lb-quic/tests/h3_h2_stream_e2e.rs:1265 | "the trailers branch (h3_bridge.rs:1571-1599), the over-cap `Reset`/`OverCap` arms (1527-1528, 1546-1548, 1593-1595), and the `send!` `ClientGone` arm (1503)" | All five ranges land inside `write_h1_request` / the request-trailer-drop note; `stream_h2_response` is at :1250-1375. Repeated at :1370 and :1517.`
`AMB | crates/lb-quic/tests/router_accept_path.rs:334 | "computed in `router::build_replay_key` from the client's SECOND Initial (see router.rs:206-211)" | :206-211 is the end of `router_main`; `build_replay_key` is at :277-283.`
`AMB | crates/lb-quic/tests/router_accept_path.rs:347 | "(listener.rs:236 `Arc::clone(&replay_guard)` into `RouterParams`)" | :236 is inside `with_backends`; the RouterParams construction is at :404-437.`
`AMB | crates/lb-quic/tests/s19_b6_zero_rtt_rejection.rs:10 | "`crates/lb-quic/src/listener.rs:426` `build_server_config` (the production client-facing config)" | :426 is a `quic_modeb_metrics` field init; `build_server_config` is at :579-609. Repeated at :139.`
`AMB | crates/lb-quic/tests/s19_b6_two_connections.rs:41 | "created by `quiche::accept_with_retry` in the router — `crates/lb-quic/src/router.rs:351`" | :351 is a fn parameter line; `accept_with_retry` is at :379. (The sibling citation `raw_proxy.rs:287` in the same block IS still correct.)`
`AMB | crates/lb-quic/src/passthrough.rs:1471 | "the lcov-uncovered 599-674 / 787-848 / 994-1037 clusters … Lifts passthrough.rs cov 75.91% -> >=80%" | Those line clusters no longer correspond to the named functions (the file grew through F-S20-2/S38). Same at :1972 ("(994-1037)").`
`AMB | crates/lb-quic/src/h3_bridge.rs:727 | "mirrors the request-side trailer-block ceiling rationale (`h3_bridge.rs` ~:86-87)" | MAX_TRAILER_BLOCK_BYTES is now at :92-97.`
`AMB | crates/lb-quic/tests/h3_graceful_close.rs:85 | "Mirrors round8_h3_authority_enforced.rs:75-81." | The cited nonce rationale is at :66-74 in that file.`
`AMB | crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs:1237 | "Mirrors S2 T5 (`h3_h1_stream_body_e2e.rs:791`)" | Drifted; also :1257 cites "conn_actor.rs:382-385" for `record_resp_retained`, which is at conn_actor.rs:692-724.`
`AMB | crates/lb-quic/tests/passthrough_retry_differential.rs:6 | "quiche's `retry()` signature (lib.rs:1878) … (per packet.rs:756)" | External line refs into quiche's source, pinned to 0.28; the tree is on quiche 0.29.1.`

### C. Status / behaviour claims that are no longer true

`AMB | crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs:1716-1724 | "C2 sixth variant — ClientGone. Currently FAILS: it is the regression lock for the proven product defect … Keep failing until fixed; do NOT weaken or ignore." | The defect WAS fixed: conn_actor.rs:562-602 `reap_client_cancelled_responses` ("SESSION 5 / DEFECT-CLIENTGONE … binding C2 / §1.3.4") implements exactly the teardown this test asserts, and the file carries no `#[ignore]`. A doc declaring a green test as expected-to-fail is materially misleading in both directions. HIGHEST-VALUE flag in this area — but I did not RUN the suite, so I am not asserting the test passes, only that the fix it waits on has landed.`
`AMB | crates/lb-quic/src/h3_config.rs:5-13 | "This module is **infrastructure only**: … It deletes nothing and changes no live path; it is exercised by the INC-1 go/no-go experiment … The production wiring into `conn_actor` lands only if INC-1 is GO" | INC-1 went GO and the wiring landed: conn_actor.rs:443 calls `build_server_h3_config` on every established connection, and h3_bridge.rs:2487 calls `build_client_h3_config`. This is now the live H3 config path, not infra-only.`
`AMB | crates/lb-quic/src/conn_actor.rs:750-759 | "Two variants. `Buffered` is the LEGACY shape … it is **unchanged** and still serves H2/H3 round-trips and the inline 400/502/413 error responses" | `StreamTx` has ONE variant; `Buffered` was deleted at S25/INC-5 — as the very next doc-comment (:762-765) itself states. Self-contradictory within 10 lines. Same stale description at :795-804 ("The `Buffered` arm raw-`stream_send`s pre-encoded bytes").`
`AMB | crates/lb-quic/src/h3_bridge.rs:55-56 | "// TODO(s3): config + incremental egress (replace this buffer-and-cap with a channel back into the actor + progressive `StreamTx`)." | The channel + Progressive StreamTx half SHIPPED at S4/P1-B. The `config` half is still open (MAX_RESPONSE_BODY_BYTES is a hardcoded const), so this is a half-done TODO, not a dead one — narrow it rather than delete it. (The sibling `// TODO(s3)` at :39 is still fully valid.)`
`AMB | crates/lb-quic/src/listener.rs:290-292 | "The actual frame relay is wired in a later stage; this knob only governs the handshake-acceptance surface." | The relay was wired at S28; `with_ws_relay_launcher` is 15 lines below in the same impl block.`
`AMB | crates/lb-quic/src/h3_config.rs:88-89 | "H3 has no client-only knob the gateway needs today (extended-CONNECT / WebSockets-over-H3 is an S26 item)." | WS-over-H3 landed in S27/S28, not S26.`

### D. Stale dependency version pins in rationale prose

`AMB | crates/lb-quic/src/lib.rs:1 | "QUIC transport layer backed by [`quiche`] 0.28 over `BoringSSL`." | Cargo.lock pins quiche 0.29.1 (MSRV moved to 1.88 at S31 for exactly this). 23 `quiche 0.28` / `quiche-0.28` sites across 10 files (src: lib.rs, conn_actor.rs, h3_bridge.rs, router.rs, terminate_loopback.rs; tests: h3_h3_stream_e2e.rs, s16_b2_reset_not_fin.rs, s16_b3_reset_propagation_smoke.rs, s16_b3_reset_propagation_verify.rs, s19_b6_zero_rtt_rejection.rs). Several are load-bearing behavioural claims about a specific quiche version (the §7.1 frame-completeness gap, `stream_finished()`-on-collected-stream, the `finished_streams` double-pop) that were verified against 0.28 and re-asserted, but not re-verified against 0.29.1 in any comment I found. Do NOT bulk-rewrite: each needs a re-check.`

### E. Internally contradictory or misplaced blocks

`AMB | crates/lb-quic/src/router.rs:561-564 | "Keep the receiver alive so the channel is \"open\" … we hold the rxs to be unambiguous." | The code is `let (tx, _rx) = mpsc::channel(1);` — `_rx` drops at the end of each loop iteration, so the receivers are NOT held. The immediately following comment (:567-568) says the opposite ("Receiver dropped at end of iteration is fine"). One of the two is wrong; the test is unaffected either way (the cap check runs before any forwarding).`
`AMB | crates/lb-quic/src/conn_actor.rs:1021-1038 | "Repeatedly call `quiche::Connection::send` … / SESSION 22 — reset an H3 request stream with an application error `code` …" | Two distinct doc blocks are FUSED into one `///` run attached to `fn reset_h3_stream` (:1039). The first half documents `drain_conn_send`, which sits at :1112 with NO doc of its own. Needs splitting, not deleting — both halves are load-bearing (the R12 single-source note and the queued-reset-is-inert-until-flush lesson).`
`AMB | crates/lb-quic/src/lib.rs:155-165 | "SESSION 27 / WS-over-H3 (RFC 9220) Stage B — bounded `AsyncRead + AsyncWrite` tunnel adapter …" | This block describes `ws_tunnel` but sits directly above `mod listener;`; `pub mod ws_tunnel;` is declared two lines later with no doc. Move, don't delete.`

## LOAD-BEARING NOTABLES (explicitly preserved)

`KEEP | crates/lb-quic/src/conn_actor.rs:628-641 | THE canonical F-S29-1 note: `get_mut` NOT `entry().or_insert_with()` on a stale response receiver — a fresh StreamTx would replay a buffered End, fire a spurious RESET, and stream_shutdown would DISCARD a large response's still-buffered trailer+FIN (gRPC-fatal). Sacred per the standard §1.`
`KEEP | crates/lb-quic/src/conn_actor.rs:1571-1586 | F-MD-4 smuggling guard: quiche's FIRST `finished_streams` pop returns `Finished` WITHOUT the reset re-check its SECOND pop performs; the zero-length `stream_recv` probe is what stops a truncated request reaching the backend as complete.`
`KEEP | crates/lb-quic/src/h3_bridge.rs:2737-2751 | The response-direction F-MD-4 MIRROR (same quiche double-pop mechanism, reversed: a clean FIN on a reset response stream would response-split the downstream client).`
`KEEP | crates/lb-quic/src/h3_bridge.rs:2567-2581 + :2752-2767 | CF-QUICHE-FRAME-COMPLETENESS: quiche does not enforce RFC 9114 §7.1 DATA completeness at FIN; the content-length under-run cross-check is the compensating guard, with the residual no-content-length gap and its threat model documented.`
`KEEP | crates/lb-quic/src/h3_bridge.rs:65-83 | The DELIBERATE request-leg (H3_REQUEST_CANCELLED 0x010c) vs response-leg (H3_INTERNAL_ERROR 0x0102) asymmetry — "must NOT be 'fixed' to a false consistency". Exactly the "use X not Y because Y does Z" shape.`
`KEEP | crates/lb-quic/src/conn_actor.rs:68-86 | Why the abort code is 0x0102 and deliberately NOT 0x0100 (truncated-as-complete / cache poisoning) and NOT 0x010c (misattributes a gateway failure to the client).`
`KEEP | crates/lb-quic/src/conn_actor.rs:356-362 | `goaway_pending` vs `goaway_sent`: admission must stop the instant the cap trips, but recycle must wait until the GOAWAY frame is actually queued. Collapsing the two flags re-opens the admit-past-boundary window.`
`KEEP | crates/lb-quic/src/raw_proxy.rs:1290-1295 | CF-S16-RELAY-STALL: after the source FIN quiche has COLLECTED the stream, so a re-issued `stream_recv` returns `InvalidStreamState` and the generic error arm would DROP the pending tail + the FIN. The `!half.src_fin_seen` read-gate is the one-line fix.`
`KEEP | crates/lb-quic/src/raw_proxy.rs:714-728 | `Shutdown::Write` ⇒ RESET_STREAM, `Shutdown::Read` ⇒ STOP_SENDING — flagged "counterintuitive in quiche". Swapping the arms silently sends the WRONG frame.`
`KEEP | crates/lb-quic/src/passthrough.rs:178-200 + :273-316 | The SAFETY/INVARIANT block ("FlowEntry holds no key material — passthrough never decrypts") plus the `_flow_entry_field_audit` destructuring + type-witness audit that makes adding an unenumerated field a COMPILE ERROR (CF-S15-FLOWENTRY-FIELD-AUDIT, owner ruling §9.5).`
`KEEP | crates/lb-quic/src/passthrough.rs:282-289 | Why the field audit is unconditional and not `#[cfg(debug_assertions)]`: release builds compile out the test caller, so `backend` read nowhere ⇒ `field is never read` under `-D warnings` broke `cargo build --release` (S34).`
`KEEP | crates/lb-quic/src/passthrough.rs:530-536 | The narrow `#[allow(deprecated)]` on `fetch_update`: nightly deprecates it, `try_update` does not exist on MSRV 1.88, and the fuzz-smoke lane builds nightly with `-D warnings`. Orphaning this allow breaks a CI lane.`
`KEEP | crates/lb-quic/src/public_header.rs:22-29 | RFC 9001 §5.4 reasoning that every field this parser reads is wire-cleartext (NOT header-protected) — the basis for the Mode A no-decrypt property.`
`KEEP | crates/lb-quic/src/public_header.rs:317-324 | Why VersionNegotiation is folded onto the Retry match arm: keeps the match exhaustive without `unreachable!`, which the crate denies.`
`KEEP | crates/lb-quic/src/public_header.rs:362-378 | Fixture provenance: RFC 9001 §A.2 vs §A.3 and why the unprotected bytes are used, so a future reader can re-verify every byte from the RFC tables.`
`KEEP | crates/lb-quic/src/h3_bridge.rs:633-649 | RESPONSE_HOP_BY_HOP is a DELIBERATE cross-crate duplicate of `lb_l7::h2_to_h1::RESPONSE_HOP_BY_HOP` (reverse-layering ban) + RFC 9114 §4.2 makes the strip REQUIRED, not tidiness. "Keep the two in sync."`
`KEEP | crates/lb-quic/src/h3_bridge.rs:1591-1615 | Why H3→H1 request trailers are INTENTIONALLY dropped: forwarding them would need chunked + a `Trailer:` announcement, and smuggling peer-controlled fields into the H1 head is a request-smuggling vector.`
`KEEP | crates/lb-quic/src/h3_bridge.rs:139-168 | F-S7-6 idle-vs-wall-clock deadline: the previous fixed 5 s wall-clock truncated a valid 8 MiB response at ~4.37 MiB. Names exactly which events reset it and which must NOT (R-S76-5).`
`KEEP | crates/lb-quic/src/listener.rs:481-488 + :634-667 | F-INFRA-01 (S38) retry-secret perm gate + the three tests, incl. the explicit "NEGATIVE CONTROL: a world-readable (0644) existing secret is REJECTED in strict mode. Pre-fix this loaded silently."`
`KEEP | crates/lb-quic/src/router.rs:125-134 + :353-356 | The `max_connections` auditor bound (2026-04-23) and the 2×-entries-per-connection arithmetic behind the cap.`
`KEEP | crates/lb-quic/src/cleanup_guard.rs:1-21 | CODE-2-08: why the RAII guard exists — the pre-fix explicit removes were skipped on unwind, pinning 2 entries per panicked actor into a denial-of-service via panic exhaustion. Includes why it is dead code under `panic = "abort"` and kept for dev/test.`
`KEEP | crates/lb-quic/src/ws_tunnel.rs:20-52 | R8 bounded-by-construction (PollSender parks, does not buffer — the property WS-over-H2 lacked, CF-S27-2) + the RFC 9220 close-vs-reset mapping.`
`KEEP | crates/lb-quic/src/h3_config.rs:15-34 | The static-table-only QPACK default rationale (RFC 9204 §3.2.2, blocked_streams=0 is the only consistent value) — pre-authorized defaults, documented not silent.`
`KEEP | crates/lb-quic/src/raw_proxy.rs:640-671 | MAX_RELAY_STREAMS as defense-in-depth INDEPENDENT of the quiche `max_streams` grant, with the 128 MiB worst-case ceiling arithmetic that raw_proxy.rs:2604-2620 pins as a test.`
`KEEP | crates/lb-quic/tests/s16_b2_reset_not_fin.rs:259-274 (and s16_b3_reset_propagation_smoke.rs:227-234, s16_b3_reset_propagation_verify.rs:28-31) | NEGATIVE-CONTROL WITNESS TRAP: `stream_finished()` returns `true` for an unknown/collected stream, and a correctly-RESET stream becomes unknown — so it FALSE-POSITIVES a clean end. Only `stream_recv` returning `fin == true` is a valid smuggling witness. Deleting this invites a "simplification" that makes three suites vacuous.`
`KEEP | crates/lb-quic/tests/s16_b2_stream_relay_smoke.rs:563-582 + :822-827 | F-S20-1: the S20 "relay stall" was REFUTED as a load-client artifact (single-shot `stream_send` ignoring partial writes). The `full_send=false` case is retained as a LOAD-BEARING NEGATIVE CONTROL that must leave a stream incomplete — "If this ever completed all 4, the positive tests would be vacuous."`
`KEEP | crates/lb-quic/src/raw_proxy.rs:2255-2259 + :2488-2499 | B4 drop-newest and B5 cap negative controls stated as falsifiable claims ("an unbounded queue would hold all cap+K and report dropped == 0 — this test fails it"; "Remove the gate and the final assert flips from == cap to == OPEN").`
`KEEP | crates/lb-quic/tests/h3_h3_stream_e2e.rs:1605-1610 + :1791-1797 | "NOTE FOR THE VERIFIER" mutation recipes — the exact source flip that must make each R13(c) burst FAIL. These encode why the tests prove what they prove.`
`KEEP | crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs:1127-1135 | FEATURE GATE (load-bearing): R2/R3 reference a `test-gauges`-gated static, so a CI gate omitting `--features test-gauges` SILENTLY DROPS the only non-vacuous memory assertions. Standard §8 (a gate reads this).`
`KEEP | crates/lb-quic/tests/grpc_h3_e2e.rs:63-71 | The suite-wide serial lock rationale: 16 concurrent in-process gateways over-saturate an 8-core box under the all-features gate and the upstream dial times out into a 502 (CF-SATURATION-1). Explains a deliberate throughput sacrifice.`

## Per-file load-bearing counts

Counted as contiguous comment blocks (a run of consecutive comment lines = 1 block). Totals below
are blocks classified LOAD-BEARING, i.e. all blocks in the file minus any SLOP/AMB blocks listed
above.

```
crates/lb-quic/examples/passthrough_linkage_probe.rs           : 4
crates/lb-quic/src/cleanup_guard.rs                            : 5
crates/lb-quic/src/conn_actor.rs                               : 164
crates/lb-quic/src/h3_bridge.rs                                : 270
crates/lb-quic/src/h3_config.rs                                : 10
crates/lb-quic/src/lib.rs                                      : 20
crates/lb-quic/src/listener.rs                                 : 57
crates/lb-quic/src/passthrough.rs                              : 164
crates/lb-quic/src/public_header.rs                            : 57
crates/lb-quic/src/raw_proxy.rs                                : 202
crates/lb-quic/src/router.rs                                   : 71
crates/lb-quic/src/terminate_loopback.rs                       : 44
crates/lb-quic/src/udp_dataplane.rs                            : 32
crates/lb-quic/src/ws_tunnel.rs                                : 63
crates/lb-quic/tests/grpc_h3_e2e.rs                            : 68
crates/lb-quic/tests/h3_connection_recycle_e2e.rs              : 46
crates/lb-quic/tests/h3_graceful_close.rs                      : 20
crates/lb-quic/tests/h3_h1_bridge_e2e.rs                       : 11
crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs                  : 96
crates/lb-quic/tests/h3_h1_stream_body_e2e.rs                  : 31
crates/lb-quic/tests/h3_h1_stream_body_errors_e2e.rs           : 25
crates/lb-quic/tests/h3_h1_trailers_resp_e2e.rs                : 21
crates/lb-quic/tests/h3_h2_stream_e2e.rs                       : 63
crates/lb-quic/tests/h3_h3_stream_e2e.rs                       : 134
crates/lb-quic/tests/listener_lifecycle.rs                     : 23
crates/lb-quic/tests/passthrough_retry_differential.rs         : 7
crates/lb-quic/tests/proptest_header.rs                        : 2
crates/lb-quic/tests/public_header_differential.rs             : 8
crates/lb-quic/tests/quic_router_leak.rs                       : 12
crates/lb-quic/tests/round8_h3_authority_enforced.rs           : 16
crates/lb-quic/tests/router_accept_path.rs                     : 9
crates/lb-quic/tests/s16_b1_two_connections.rs                 : 34
crates/lb-quic/tests/s16_b2_backpressure.rs                    : 40
crates/lb-quic/tests/s16_b2_multistream.rs                     : 34
crates/lb-quic/tests/s16_b2_reset_not_fin.rs                   : 27
crates/lb-quic/tests/s16_b2_stream_relay_smoke.rs              : 40
crates/lb-quic/tests/s16_b3_reset_propagation_smoke.rs         : 30
crates/lb-quic/tests/s16_b3_reset_propagation_verify.rs        : 57
crates/lb-quic/tests/s16_raw_proxy_smoke.rs                    : 15
crates/lb-quic/tests/s19_b4_datagram_relay_smoke.rs            : 28
crates/lb-quic/tests/s19_b4_datagram_verify.rs                 : 35
crates/lb-quic/tests/s19_b5_stream_flood.rs                    : 33
crates/lb-quic/tests/s19_b5_verify.rs                          : 32
crates/lb-quic/tests/s19_b6_metrics_nonvacuous.rs              : 31
crates/lb-quic/tests/s19_b6_two_connections.rs                 : 27
crates/lb-quic/tests/s19_b6_zero_rtt_rejection.rs              : 29
TOTAL                                                          : 2253  (of 2290 blocks scanned)
```

## Explicitly considered and REJECTED as slop

Recorded so the sweeper does not re-litigate these:

* **Section banners** (`// ==== Constants ====`, `// ==== Parameters ====`, `// ─── §3 Cases ───`,
  111 across the area). Not repeated verbatim across files; each is a local navigation marker in a
  2000–3900-line file, and most carry real content ("FlowEntry — the routing-table value, NO key
  material", "Retry-secret loader (mirrors listener.rs pattern)"). KEEP.
* **Phase markers in long test drivers** (`// Tidy up.`, `// Flush outbound.`, `// Handshake pump.`,
  `// 1) Real echo backend.`). Dozens of these. They restate little, but they delimit phases inside
  400–800-line async test bodies. Deleting them is churn across ~25 files for no gain, and the
  standard is explicit that a retained mediocre comment costs nothing. KEEP.
* **ALL `///` and `//!` doc-comments on `pub` items** (lead's correction). `deny(missing_docs)` —
  removal breaks clippy. This retires the "redundant doc duplicating the signature" category
  wholesale in this area; the concrete instances were `/// The socket address the listener is bound
  to.` ×3 (listener.rs:455, passthrough.rs:1243, terminate_loopback.rs:226). KEEP.
* **`/// \`true\` iff no payloads are queued.`** on the PRIVATE `BoundedDgramQueue::is_empty`
  (raw_proxy.rs:1047). The single genuine pure-restatement doc on a private item in the whole area —
  `missing_docs` does not fire, so it is technically eligible. **Not proposed**: it is one line, its
  five sibling accessors on the same impl all carry docs (deleting only this one makes the block
  inconsistent), and the standard's tiebreak is explicit that a retained mediocre comment costs
  nothing. Recorded so the sweeper sees the category was checked and found effectively empty, not
  skipped.
* **Provenance tags** (`(S34)`, `(S44)`, `SESSION 24 / INC-3:` prefixes). Per the standard's
  provenance clause these are cheap and are never the whole payload here. KEEP.
