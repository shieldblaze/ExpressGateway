# S24 — INC-3 (E1 egress → quiche::h3 send_response/send_body) plan

**Base:** INC-2 committed `f3e318f4` (E1 ingress on quiche::h3, egress still raw
`stream_send` of pre-encoded `RespEvent::Bytes`). INC-3 restructures the egress.

## Scope boundary (critical — keep E2 out)
- **IN scope (E1 response egress):** the actor's `RespEvent` consumer
  (`drain_resp_channels`/`StreamTx::Progressive`/`drain_streams_to_conn`) + the THREE
  `RespEvent` producers that feed it: `stream_h1_response` (H3→H1), `stream_h2_response`
  (H3→H2), and `H3RespOut::Wire` (H3→H3 response-to-client leg) + the inline
  502/413 emitted on those producers' abort paths.
- **OUT of scope (E2, → S25/INC-4):** `stream_request_to_h3_upstream`'s REQUEST
  encoding to the upstream (the `encode_h3_data_frame`/`encode_h3_trailers_frame` at
  h3_bridge.rs:3189/3297/3713/3882) and `H3RespEvent` (the lb-l7 `Decoded`-arm sink for
  H1→H3/H2→H3 fronts — a different channel/consumer, untouched).
- **KEEP (do not delete this session):** `encode_h3_response`/`encode_h3_headers_frame`
  /`encode_h3_headers_frame_full`/`encode_h3_data_frame`/`encode_h3_trailers_frame` —
  still used by the E2 request encoder + the `H3RespOut::Decoded` arm. They become
  egress-dead only after INC-4; delete in S25.
- **KEEP raw:** `StreamTx::Buffered` (legacy inline-400 + the `request_tasks` finished
  arm) stays a raw `conn.stream_send` of fully pre-encoded bytes — INC-1 Exp 4 proved
  raw egress coexists with quiche::h3 on a per-stream basis; the inline-400 stream uses
  only Buffered, never Progressive, so there is no mixing on one stream.

## The change
1. **`RespEvent`** (h3_bridge.rs ~190): `Bytes(Bytes)` → decoded variants:
   ```rust
   pub enum RespEvent {
       Head { status: u16, headers: Vec<(String, String)> }, // hop-by-hop already stripped by producer
       Body(Bytes),                  // ≤ H3_RESP_CHUNK_MAX, producer-split
       Trailers(Vec<(String, String)>),
       End,
       Reset,
   }
   ```
2. **Producers** stop calling `encode_h3_*` for the RespEvent path and emit decoded:
   - `stream_h1_response`: `RespEvent::Head{status, fwd_headers}` (fwd_headers already
     hop-by-hop-stripped + content-length-managed exactly as today); `emit_data!` →
     `RespEvent::Body(slice)`; trailers → `RespEvent::Trailers(list)`. The `cap`
     accounting switches from encoded-frame bytes to payload bytes (DoS threshold role,
     not memory — same as the Decoded arm already does).
   - `stream_h2_response`: same shape.
   - `H3RespOut::Wire on_head/on_data/on_trailers/inline`: emit decoded RespEvent (this
     makes Wire's emission identical in SHAPE to Decoded, but on the `RespEvent` channel;
     the hop-by-hop strip in `on_head` Wire arm stays — RFC 9114 §4.2).
3. **Actor consumer** (conn_actor.rs):
   - `StreamTx::Progressive.queue: VecDeque<Bytes>` → a small decoded item queue
     `VecDeque<RespItem>` where `RespItem = Head{..} | Body(Bytes) | Trailers(..)`;
     plus `head_sent: bool`.
   - `drain_resp_channels`: map `RespEvent::{Head,Body,Trailers}` → push RespItem;
     `End`→ended; `Reset`/Disconnected→reset (unchanged terminal logic).
   - `drain_streams_to_conn` Progressive arm: now needs `&mut h3`. On a `Head` item →
     `h3.send_response(conn, sid, &headers_as_h3, false)` once (`head_sent`); `Body` →
     `h3.send_body(conn, sid, &buf, false)` with **partial-write/`Done` retry** (the
     egress R8 gate — on short write keep the unsent tail at queue front, like today's
     `front.split_to(n)`); `Trailers` → `h3.send_additional_headers(conn, sid, &t, true,
     false)`; `ended && queue empty` → `h3.send_body(conn, sid, &[], true)` (FIN);
     `reset` → `conn.stream_shutdown(Write, H3_INTERNAL_ERROR)` (unchanged).
   - Signature: thread `h3: Option<&mut quiche::h3::Connection>` into
     `drain_streams_to_conn` (and the loop's call at conn_actor ~232). Buffered arm
     ignores `h3` (raw `stream_send`).
4. **R8 re-proof (THE central risk):** the Progressive queue must hold only the bounded
   decoded items (≤ depth bodies, each ≤ chunk) — NOT decode a whole response into one
   buffer. `record_resp_retained` re-points to: Σ Body bytes queued + channel occupancy
   (body-size independent). Re-run `h3_h1_resp_stream_e2e` `MAX_RETAINED_RESP_BYTES`
   proof — must stay flat across response sizes, non-vacuous.

## Gate (INC-3 commit bar)
69 wire tests (esp. h3_h1_resp_stream, h3_h2_stream, h3_h3_stream resp legs, trailers,
large-binary-resp) + R8 response gauge non-vacuous/flat + R12 all H3-front cells +
Mode B + R13 + workspace ×3 + clippy -D + fmt. Independent verify. Commit + push.
NOT promoted (R11).

## Risk / fallback
This is the entangled "harder half" (S23 §4). It touches `RespEvent` + 3 producers +
the actor consumer atomically (enum change can't be half-landed). If it proves larger
than the remaining budget or destabilises, INC-2 stands as the clean committed PARTIAL
(exit (b)); never leave the branch non-compiling.
