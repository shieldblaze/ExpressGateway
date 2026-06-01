# S25 — INC-4 (E2 H3→H3 upstream CLIENT → quiche::h3) implementation plan

**Branch:** `feature/quiche-h3-migration-s23` (S25 continues it; base tip `1c002f61`,
S24 COMPLETE = E1 server front migrated). **Main stays** at `90915781` until
E1+E2+Phase-3 ALL verify (R11; no half-migration promote).

**Author/verifier (R5):** the context-loaded lead implements the delicate hot-path
surgery on `h3_bridge.rs::stream_request_to_h3_upstream`; **independent verifier
subagent(s)** (fresh context) confirm every gate from completed logs AND read the
code — TWO independent verifiers on the R8 + F-MD-4 proofs (S24 pattern). The
KEEP-surface stays bit-identical; only the framing changes (R3).

---

## 0. What E2 is (verified, this session)

`stream_request_to_h3_upstream` (`h3_bridge.rs:3037-3858`, ~820 LOC) is the **single
shared H3→H3 upstream connector** for ALL THREE `→H3` fronts:
- `h1_proxy.rs:2275` — H1→H3 (`H3RespOut::Decoded` sink)
- `h2_proxy.rs:2617` — H2→H3 (`H3RespOut::Decoded` sink)
- `h3_to_h3_stream_resp` → `h3_bridge.rs:2930` — H3→H3 (`H3RespOut::Wire` sink)

So migrating it re-points all three cells at once. It is the gateway acting **as an
H3 client** toward a real H3 backend (the conn comes from `QuicUpstreamPool`, a bare
established `quiche::Connection` in client state, used **once** —
`pooled.set_reusable(false)` on every exit).

`request_h3_upstream` (`:2427`, buffered sibling) is **dead** (only the `lib.rs:184`
re-export references it; every other mention is a doc comment). It uses `lb_h3` ⇒
**delete in INC-5** (with `H3UpstreamResponse`).

---

## 1. What INC-4 REPLACES vs KEEPS (code-grounded)

### REPLACE — the hand-rolled `lb_h3` framing (request encode + response parse)
| Today (hand-rolled) | After (quiche::h3 client) |
|---|---|
| `QpackEncoder::new().encode(&headers)` + `encode_frame(Headers)` + manual `stream_send` on hardcoded `stream_id=0` with FIN logic (`:3144-3200`) | `h3.send_request(qconn, &h3_headers, headers_fin)` → **returns** the request `stream_id` |
| `ship_bodyless_trailers`: `encode_h3_trailers_frame` + manual `stream_send` (`:3209-3245`) | `h3.send_additional_headers(qconn, sid, &trailers, /*is_trailer*/true, /*fin*/true)` |
| `ReqSend::InHand{frame,sent}` = `encode_h3_data_frame(chunk)` + `stream_capacity`-gated `stream_send` (`:3305-3392`) | `ReqSend::InHand{bytes,sent}` = **raw** chunk bytes via `h3.send_body(qconn, sid, &bytes[sent..], false)` (quiche frames DATA itself — `encode_h3_data_frame` gone) |
| `FinNoTrailers`: `stream_send(sid, &[], true)` (`:3794`) | `h3.send_body(qconn, sid, &[], true)` |
| `FinWithTrailers`: `encode_h3_trailers_frame` + `stream_send` (`:3734-3780`) | `h3.send_additional_headers(qconn, sid, &trailers, true, true)` |
| `AbortNoFin`: `stream_shutdown(Write, H3_REQUEST_CANCELLED)` (`:3822`) | **UNCHANGED** (transport-level; case-7 no-FIN guard) |
| recv: `qconn.readable()`+`stream_recv`→`rx_tail`, `RecvState{AwaitingHeader/InData/InBlock/InSkip}`, `parse_frame_header`, `check_block_len`, `QpackDecoder::decode`, `classify_recv_err` (`:3257-3650`) | `h3.poll(qconn)` event loop → `recv_body` into a fixed scratch; **quiche owns** frame reassembly + QPACK decode + the `DEFAULT_MAX_PAYLOAD_SIZE`/`MAX_FRAME_HEADER_BYTES` bounds + frame-sequence rules |
| `upstream_fin` between-frames check (`:3657-3673`) | `Event::Finished` + the F-MD-4 reset-probe (see §4) |

**DELETED with E2's framing** (now dead in this fn; confirm no other caller in INC-5):
`RecvState`, `ReqSend`'s frame encoding, `parse_frame_header` (`:3989`), `check_block_len`
(`:3924`), `classify_recv_err` (`:3975`), `RecvErrClass`. `encode_h3_data_frame` /
`encode_h3_trailers_frame` become **call-dead from E2** (still called by the buffered
dead `request_h3_upstream` and the H3→H1/H2 response legs until INC-5).

### KEEP — bit-identical (the proxy value-add + the R8/F-MD-4/cap surface)
- **The `H3RespOut` sink** (`Wire`/`Decoded`, `:2659-2887`) — UNTOUCHED. `on_head` /
  `on_data` / `on_trailers` / `on_end` / `on_reset` / `inline` and the **cumulative
  response cap** (`total`/`cap` in the sink) stay exactly as-is. E2 still calls them
  in the same order with the same arguments (decoded `fields`, raw body slices,
  validated trailers). This is the KEEP-surface boundary.
- **The first-body-event peek** (`:3094-3123`): `Reset`→`inline(413)`+return,
  bodyless detection, `req_streaming`+`first_chunk`, `bodyless_trailers`. UNCHANGED.
- **The pre-dial error paths** (`:3126-3138`): pool-acquire-fail / empty-handle →
  `inline(502)`+return. UNCHANGED (run BEFORE `with_transport`).
- **The bounded request-body channel** `body_rx` (depth-8) + the M-A pump upstream —
  UNCHANGED. R8 request-direction backpressure rides it.
- **`forward_req_trailers`** semantics (H3→H3 = false ⇒ trailers dropped; L7 fronts =
  true ⇒ forwarded as trailing HEADERS). UNCHANGED behaviour, new API.
- **`j2_req_event_action`** (`:3901`) decision (`Chunk→SendData`, `End→Fin{±trailers}`,
  `Reset→AbortNoFin`) — KEEP the decision; `SendData` now carries **raw** `Bytes`
  (not an encoded frame). Its unit test updates to the new payload type (not weakened).
- **F-S7-6 idle deadline** (`H3_RESP_IDLE_TIMEOUT`, `send_progress!`, reset-on-real-
  progress) — KEEP; reset on `send_body n>0`, `recv_body n>0`, and each successful
  sink relay (head/data/trailers).
- **The single park point** `tokio::select!{ socket.recv_from(timeout) | body_rx.recv
  if AwaitNext | on_timeout }` — KEEP shape; after `recv_from`→`qconn.recv`, then poll.
- **One-request-per-pooled-conn** (`set_reusable(false)` every exit) — KEEP.

---

## 2. New control flow (the migrated fn)

```
peek first body event            (UNCHANGED: 413 / bodyless / streaming / bodyless_trailers)
pool.acquire → upstream conn     (UNCHANGED)
let mut h3 = build_client_h3_config()
      .and_then(|c| quiche::h3::Connection::with_transport(qconn, &c));
   on Err → set_reusable(false); sink.inline(502); return Ok(())   // fail-safe
build h3_headers: Vec<quiche::h3::Header> from `headers` (verbatim, incl pseudo)
let headers_fin = !req_streaming && !ship_bodyless_trailers;
let stream_id = match h3.send_request(qconn, &h3_headers, headers_fin) {
   Ok(id) => id,
   Err(_) => { set_reusable(false); sink.inline(502); return Ok(()) }   // encode/limit
};
if ship_bodyless_trailers:                                            // L7-only
   h3.send_additional_headers(qconn, stream_id, &tr, true, true)  on Err → shutdown+reset
req_send = if req_streaming { InHand{first_chunk} | AwaitNext } else { Ended }
sent_head = false; response_complete = false; outcome = Ok(())

'evloop while now < idle_deadline:
  (a) request pump: if InHand{bytes,sent}:
        match h3.send_body(qconn, sid, &bytes[sent..], false):
          Ok(n)  => sent+=n; if n>0 reset_idle; if sent>=len → AwaitNext
          Done   => keep in hand (BACKPRESSURE — do not pull body_rx)
          Err(e) => stream_shutdown(Write,H3_REQUEST_CANCELLED); outcome=UpstreamReset; break
  (b) flush: while qconn.send(out) Ok((n,info)) → socket.send_to
  (c) response drain: loop { match h3.poll(qconn):
        Headers{list,..}: if !sent_head { send_progress!(sink.on_head(fields)); sent_head=true }
                          else { trailers: reject any `:`→on_reset+BadHead+break'evloop;
                                 else if non-empty send_progress!(sink.on_trailers) }
        Data:    drain_response_body() → recv_body loop into scratch;
                 each slice: sink.on_data(slice).await   // R8 backpressure (await blocks)
                   Ok→reset_idle ; Err(OverCap/ClientGone)→outcome+break'evloop
                 recv_body Err(non-Done) mid-body → F-MD-4 → outcome=UpstreamReset; break'evloop
        Finished: drain_response_body() once more;                    // F-MD-4 MIRROR §4
                  was_reset = matches!(qconn.stream_recv(sid,&mut[]),Err(StreamReset(_)));
                  if was_reset || !sent_head { outcome=UpstreamReset (PrematureEof if !head) }
                  else { response_complete=true }
                  break 'evloop
        Reset(_): outcome=UpstreamReset; break 'evloop                // genuine backend reset
        GoAway/PriorityUpdate/other: ignore
        Err(Done): break (poll drained this tick)
        Err(_):   outcome=UpstreamReset; break 'evloop                // quiche closed conn
      }}
  if response_complete: break 'evloop
  (d) park: select! biased {
        r = timeout(qconn.timeout(), socket.recv_from(in)) =>
              Ok(Ok((n,from))) → qconn.recv(slice); else qconn.on_timeout()
        ev = body_rx.recv(), if AwaitNext => j2_req_event_action(ev,fwd):
              SendData(bytes)      → InHand{bytes,0}
              FinNoTrailers        → send_body(sid,&[],true); reset_idle; Ended
              FinWithTrailers(tr)  → send_additional_headers(sid,&tr,true,true); Ended
                                       on err → shutdown(Write); outcome; break'evloop
              AbortNoFin           → shutdown(Write,H3_REQUEST_CANCELLED); outcome; break'evloop
      }

post-loop:
  set_reusable(false)
  if response_complete { sink.on_end().await?; return Ok(()) }       // clean → FIN client
  if outcome.is_ok()   { sink.on_reset().await; return Err(PrematureEof) }  // idle fell through
  sink.on_reset().await; outcome                                     // aborted: Reset, never End
```

`build_client_h3_config()` (new in `h3_config.rs`) mirrors `build_server_h3_config`
exactly (max_field_section_size 1 MiB, qpack table 0, blocked 0 — static-only,
behaviour-matching, pre-authorized R7). The H3 config is symmetric (no client-only
knob needed; extended-CONNECT is S26). Unit test: builds + constants asserted.

---

## 3. R8 re-proof on the MIGRATED E2 — both directions (non-vacuous, R8/R3)

- **Request leg (gateway→backend body):** `send_body` returns `Done` when the upstream
  stream window is closed ⇒ the in-hand chunk stays, `body_rx` is NOT pulled
  (`want_next=false`), the depth-8 channel fills, the M-A pump pauses the **downstream
  client's upload**. Retained request memory = one ≤8 KiB chunk + the depth-8 channel,
  **request-body-size INDEPENDENT**. Prove with a large request body + a stalled
  backend: peak flat, ≤ a small multiple of one chunk, 1 MiB vs 4 MiB delta ≤ 1 chunk.
- **Response leg (backend→gateway→client):** `recv_body` into a fixed scratch
  (`[u8; H3_RESP_CHUNK_MAX]`); `sink.on_data(slice).await` blocks when the downstream
  channel is full ⇒ we stop calling `recv_body` ⇒ quiche stops extending the response
  stream's flow-control window ⇒ the **backend pauses**. Retained response memory =
  one scratch + the sink channel depth, **response-body-size INDEPENDENT**. Prove with
  a large response body + a stalled downstream client: peak flat (mid==peak), body-size
  independent. The cumulative cap stays a DoS abort only (in the sink), NOT a memory
  bound. **An E2 that `.collect()`s a whole body before forwarding = the buffering trap
  = rejected.** The existing `h3h3_e2e_*memory_bounded*` / `*backpressure*` tests are
  the gauges; re-run them on the migrated path and confirm non-vacuous (gauge fires,
  channel genuinely saturates).

## 4. F-MD-4 MIRROR (E2's highest-risk correctness property — R13)

S24 found: quiche delivers `Event::Finished` (NOT `Reset`) for a stream RESET *after*
its last DATA frame (quiche-0.28 `mod.rs:2072` first `finished_streams` pop lacks the
reset re-check the second pop at `:2106` has). E2 is the **mirror**: when the gateway-
as-client sees the **backend** reset the **response** stream after its last DATA, quiche
delivers `Finished`. Mapping that to a clean `on_end` would FIN the **downstream client**
on a truncated response = response-splitting/smuggle (reversed). Guard (mirror of
`conn_actor.rs:1269`):
```
Event::Finished ⇒ was_reset = matches!(qconn.stream_recv(sid, &mut []), Err(StreamReset(_)));
                  if was_reset → sink.on_reset() + Err(UpstreamReset)   // NEVER on_end
```
Plus the two direct cases: `Event::Reset(code)` (backend reset before/without Finished)
→ `on_reset`+Err; `recv_body` `Err(non-Done)` mid-body (peer RESET_STREAM surfaces here)
→ `on_reset`+Err. And the premature-EOF guard: `Finished` with `!sent_head` → `on_reset`
+Err (never a headerless clean end).

**Verify (R13):** (a) ×3 gate green; (b) isolation burst ≥50 iter (an H3→H3 backend-RST
burst, current_thread) proving a backend RST mid/after-body relays as a **reset to the
downstream client, not FIN**; (c) load-bearing **negative control** — a clean-EOF
response in the same rig that MUST `on_end`→200 (proves the burst isn't a blanket reset).
Two independent verifiers READ this arm in the migrated code, not just the logs.

## 5. INC-4 commit bar (gating)

1. Build: `cargo check/clippy --all-targets --all-features -D warnings` + `fmt --check` clean.
2. **Safety net (R3):** the H3 wire tests (`h3_h3_stream_e2e`, `h3_h1_*`, `h3_h2_*`,
   `h1h3_md_streaming_verify`, `h2h3_md_streaming_verify`, `proto_translation_e2e`,
   round8 authority, graceful_close, Mode B s16/s19) PASS against the migrated E2.
3. **R12:** H3→H3 re-verified END-TO-END with BOTH migrated endpoints (E1 server + E2
   client — the only cell exercising both at once) + H1→H3 + H2→H3 + Mode B.
4. **R8:** request + response gauges non-vacuous + body-size-independent on migrated E2.
5. **R13:** F-MD-4 mirror burst (×3 + ≥50-iter isolation + negative control).
6. Full workspace ×3 deterministic (R1).
7. Commit + push to origin (R5). Update `s25-report.md`.

If quiche::h3 client cannot express a KEEP need (send_request rejecting a forwarded
field list; a trailer/cap/backpressure need) → **escalate (R7) BEFORE deleting** the
corresponding hand-rolled code.

## 6. Named risks (and the mitigation)

- **send_request field-list compatibility.** The three fronts pass different field
  lists (H3→H3 verbatim pre-S12 order; H1→H3 / H2→H3 built by
  `collect_h*_request_to_h3_fieldlist`). quiche's `send_request` QPACK-encodes them; if
  a front forwards a field quiche rejects (e.g. a connection-specific header), the Err
  path is `set_reusable(false)+inline(502)` — but a test regression would surface it.
  The H1→H3/H2→H3 e2e + `proto_translation` suites are the proof. If a real field-list
  incompatibility appears → escalate before deleting (R7), do NOT silently 502.
- **Edge-triggered `Data`.** quiche `Data` fires once and re-arms after `recv_body`
  drains to `Done`. Since `on_data().await` blocks under backpressure but always
  eventually returns (or the task is correctly parked), each `Data` is drained to
  `Done`, so quiche re-arms on new data. A per-iteration safety drain of the response
  stream (cheap `Done`-returning `recv_body`) backs this up (server PASS-1 analogue).
- **`send_request` `StreamBlocked`.** On a fresh one-shot conn the request stream has
  credit; treat `StreamBlocked`/any Err as the 502 fail-safe (non-reusable).
- **Response head before body.** quiche orders `Headers` before `Data`; `sent_head`
  gates `response_complete`. A `Finished`/EOF with `!sent_head` ⇒ PrematureEof reset.

## 7. Anticipated test-harness adaptations (S24 INC-3 mirror — NOT behaviour changes, R5)

These are harness-side updates the migration forces; **no behavioral assertion is
weakened** (verifier confirms by diff). They are the request-direction analogue of
S24 INC-3 (where the migrated E1 egress Huffman-encoded the *response* head and 8
test clients switched to `quiche::h3::qpack::Decoder`).

1. **Request-head decode → quiche.** The migrated E2 QPACK-**Huffman**-encodes the
   upstream REQUEST head (quiche does Huffman; `lb_h3::QpackDecoder` is raw-only).
   The hand-rolled test BACKENDS that decode the gateway's request head
   (`h3_h3_stream_e2e`, `h3_h1_*`, `h3_h2_*`, `h1h3_md_streaming_verify`,
   `h2h3_md_streaming_verify`, `proto_translation_e2e`) must decode it via
   `quiche::h3::qpack::Decoder` instead of `lb_h3::QpackDecoder`. Behavioral
   assertions (status, body bytes, trailer presence, reset-not-FIN) are UNCHANGED.
2. **Malformed-backend rejection now owned by quiche.** Tests where the test backend
   sends malformed H3 and asserts the gateway's reaction
   (`h3h3_e2e_data_before_headers_closes_h3_frame_unexpected`,
   `_invalid_qpack_static_index_closes_decompression_failed`,
   `_oversized_headers_block_rejected`, `_oversized_unknown_frame_rejected`,
   `_unknown_response_frame_skipped_transparently`, `_empty_data_frame_skipped_then_body`)
   now observe **quiche's** enforcement (conn close on the upstream / event error →
   `outcome=UpstreamReset` → `on_reset` to the downstream client) rather than the
   hand-rolled `RecvState`/`check_block_len`. The SECURITY property (malformed
   upstream response → downstream client reset, never a forwarded/false-complete
   response) is PRESERVED; only the mechanism moved into quiche (same as E1's
   #11/#16-22/#24). Update each assertion to the quiche-owned outcome only where the
   asserted *mechanism* (not the security property) changed; if any such test reveals
   quiche does NOT reject a malformed input the hand-rolled parser did → that is a
   real finding → escalate (R7), do not weaken the test.
3. **Existing F-MD-4 + R8 gauges stay as the proof.** `h3h3_e2e_upstream_reset_midbody_
   resets_client_no_fin` (the F-MD-4 MIRROR), `_upstream_premature_eof_mid_data_no_clean_fin`,
   `_upstream_stop_sending_*_aborts_no_fin`, `_response_memory_bounded_through_stalled_client`,
   `_request_memory_bounded_through_stalled_backend`, `_backpressure_stalled_client_pauses_
   upstream_read`, and the request-leg `_client_reset_midrequest_burst_current_thread`
   are KEPT and must pass against the migrated E2. If the existing backend-RST mirror
   coverage is single-shot, ADD a backend-RST ≥50-iter isolation burst + negative
   control for R13(b)/(c) (the E2-direction analogue of the request-leg burst).
