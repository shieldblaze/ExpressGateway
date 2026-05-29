# S16 — quiche 0.28.0 API reference for Mode B (builder notes)

Source: `~/.cargo/registry/src/*/quiche-0.28.0/`. Verified by read-only research.

## Streams
- `stream_recv(sid, out) -> Result<(usize, bool)>` — bool = FIN reached. `(0,true)` = drained at FIN.
- `stream_send(sid, buf, fin) -> Result<usize>` — may be partial (< buf.len()).
- `stream_shutdown(sid, dir: Shutdown, err: u64)` — **`Shutdown::Read` ⇒ STOP_SENDING; `Shutdown::Write` ⇒ RESET_STREAM** (counterintuitive!).
- `readable() / writable() -> StreamIter` — SNAPSHOT; refresh every loop turn.
- `stream_finished(sid) -> bool` — true == recv.is_fin(); does NOT mean drained (data may remain).

### Cancellation propagation (B3 — F-MD-4 analog)
- Peer **RESET_STREAM** ⇒ local `stream_recv(sid)` returns `Err(Error::StreamReset(code))` (immediate; buffered data discarded; permanent). Forward: `peer_conn.stream_shutdown(sid, Shutdown::Write, code)`.
- Peer **STOP_SENDING** ⇒ local `stream_send(sid)` returns `Err(Error::StreamStopped(code))` (permanent). Forward: `peer_conn.stream_shutdown(sid, Shutdown::Read, code)`.
- MUST be relayed as reset/stop, NEVER as clean `stream_send(.., fin=true)`.
- `StreamLimit` returned when opening a stream past MAX_STREAMS — backpressure the source until peer grants more credit.

## Datagrams (B4)
- `Config::enable_dgram(enabled, recv_queue_len, send_queue_len)` — the two lens are the per-connection bounds.
- `dgram_recv(&mut buf) -> Result<usize>`; `Err(Done)` = none; `Err(BufferTooShort)`.
- `dgram_send(&buf) -> Result<()>`; `Err(Done)` = **send queue full** (datagram dropped, not queued); `Err(BufferTooShort)` if > `dgram_max_writable_len()`.
- **FULL-QUEUE POLICY = drop-NEWEST** on BOTH recv and send (`dgram.rs push()` returns `Err(Done)` w/o enqueue). Native — matches plan §2.4. Add `quic_modeb_dgrams_dropped_total` counter (no silent loss).
- `dgram_max_writable_len() -> Option<usize>` (None if peer didn't advertise), `dgram_{recv,send}_queue_len()`.

## ALPN mirroring (B1/B6)
- `application_proto() -> &[u8]` after `is_established()` — negotiated client ALPN.
- Upstream dial: `config.set_application_protos(&[client_alpn])` to mirror.

## 0-RTT rejection (B6, owner-mandated mechanism test)
- Reject = do NOT call `Config::enable_early_data` on the client-facing config (current H3 behavior).
- Test construction: client extracts `session()` from a 1st connection → 2nd client `set_session(&buf)` BEFORE handshake → attempts `stream_send/dgram_send` while `is_in_early_data()`. With server NOT enabling early data, client `is_in_early_data()` flips false before `is_established()` → early data is NOT acted on. `is_resumed()` to observe resumption.

## Reusable rigs
- `crates/lb-quic/src/terminate_loopback.rs` — `QuicEndpoint::{server_on_loopback, client_on_loopback}`, `drive()` pump, `roundtrip_datagram()` (enable_dgram(true,1024,1024)). Datagram + stream wire-test model.
- `crates/lb-quic/src/router.rs:502` — raw-ALPN (`LB_QUIC_TEST_ALPN`) direct-quiche client handshake + raw stream (non-H3).
- `crates/lb-io/src/quic_pool.rs:351 dial_new()` — handshake-drive loop (send→recv_from→recv→on_timeout until is_established). **Directly reusable for `dial_dedicated()`**: parameterize socket/config/sni/addr; the pool owns it so no pub changes needed for an in-crate sibling method.
