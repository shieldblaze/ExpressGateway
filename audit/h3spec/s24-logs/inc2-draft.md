# INC-2 ingress draft — exact code to apply once baseline is GREEN

## conn_actor.rs — imports (line 39-44)
Drop `BodyItem, FeedError, StreamRxBuf, encode_h3_response(keep), MAX_REQUEST_BODY_BYTES(keep)`.
Keep: `H3_RESP_CHANNEL_DEPTH, H3Request, MAX_REQUEST_BODY_BYTES, MAX_RESPONSE_BODY_BYTES,
ReqBodyEvent, RespEvent, encode_h3_response, h3_to_h1/h2/h3_stream_resp,
validate_request_pseudo_headers`. Add: `use quiche::h3;` and the H3_BODY_CHUNK_MAX import.

## run_actor (line 197-333) changes
- Add local: `let mut h3: Option<h3::Connection> = None;`
- Replace per-stream maps: DROP `rx_buf_by_stream: HashMap<u64,StreamRxBuf>` and
  `body_pending: HashMap<u64,VecDeque<ReqBodyEvent>>`. ADD
  `let mut body_seen: HashMap<u64, usize> = HashMap::new();`
  `let mut pending_trailers: HashMap<u64, Vec<(String,String)>> = HashMap::new();`
  Keep `body_tx_by_stream`, `stream_response`, `resp_rx_by_stream`, `resp_tasks`,
  `request_tasks`.
- The `if !body_tx_by_stream.is_empty() || !body_pending.is_empty() || ...` short-tick
  gate (line 256): replace `!body_pending.is_empty()` with nothing (drop that term);
  `!body_tx_by_stream.is_empty() || !resp_rx_by_stream.is_empty()` suffices.
- Post-event block (line 318): build h3 lazily, then poll:
```rust
if params.conn.is_established() {
    if h3.is_none() {
        match h3::Connection::with_transport(
            &mut params.conn,
            &crate::h3_config::build_server_h3_config()
                .expect("h3 config builds (INC-0)"),
        ) {
            Ok(c) => h3 = Some(c),
            Err(e) => {
                tracing::warn!(error = %e, "h3 with_transport failed; closing");
                let _ = params.conn.close(true, 0x0102, b"h3 init");
            }
        }
    }
    if let Some(hc) = h3.as_mut() {
        poll_h3(
            &mut params.conn, hc,
            &mut body_tx_by_stream, &mut body_seen, &mut pending_trailers,
            &mut request_tasks, &mut resp_rx_by_stream, &mut resp_tasks,
            &mut stream_response,
            &params.pool, &params.backends,
            params.h3_backend.as_ref(), params.h2_backend.as_ref(),
        );
    }
}
```

## NEW poll_h3 — replaces lines 786-1237
```rust
/// Drain request-body bytes for ONE stream into its bounded channel,
/// capacity-gated (R8 backpressure: stop recv_body while the channel is
/// full → quiche does not extend the QUIC flow-control window → peer
/// paused; in-flight ≈ depth*chunk, body-size independent). Tracks the
/// cumulative 64 MiB cap (F-CAP-1 → 413) and maps any stream error /
/// reset to ReqBodyEvent::Reset (F-MD-4 smuggling guard). The `Finished`
/// event (not this fn) emits `End`.
fn drain_request_body(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    sid: u64,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_seen: &mut HashMap<u64, usize>,
    pending_trailers: &mut HashMap<u64, Vec<(String, String)>>,
) {
    let mut scratch = [0u8; H3_BODY_CHUNK_MAX];
    loop {
        let Some(tx) = body_tx_by_stream.get(&sid) else { return };
        if tx.capacity() == 0 {
            return; // backpressure: leave bytes in quiche, peer paused
        }
        match h3.recv_body(conn, sid, &mut scratch) {
            Ok(0) => return,
            Ok(n) => {
                let seen = body_seen.entry(sid).or_default();
                *seen = seen.saturating_add(n);
                if *seen > MAX_REQUEST_BODY_BYTES {
                    // F-CAP-1: over-cap → Reset (consumer maps to 413).
                    if let Some(tx) = body_tx_by_stream.remove(&sid) {
                        let _ = tx.try_send(ReqBodyEvent::Reset);
                    }
                    body_seen.remove(&sid);
                    pending_trailers.remove(&sid);
                    return;
                }
                // capacity>0 checked above ⇒ try_send accepts (sole producer).
                if let Some(tx) = body_tx_by_stream.get(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Chunk(
                        Bytes::copy_from_slice(scratch.get(..n).unwrap_or(&[])),
                    ));
                }
                #[cfg(any(test, feature = "test-gauges"))]
                record_req_retained(sid, body_tx_by_stream, n);
            }
            Err(quiche::h3::Error::Done) => return,
            Err(e) => {
                // F-MD-4: stream reset/error mid-body → Reset, never EOF.
                tracing::debug!(error = %e, stream_id = sid,
                    "INC-2: recv_body error mid-body; aborting upstream (Reset)");
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Reset);
                }
                body_seen.remove(&sid);
                pending_trailers.remove(&sid);
                return;
            }
        }
    }
}

#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
fn poll_h3(
    conn: &mut quiche::Connection,
    h3: &mut quiche::h3::Connection,
    body_tx_by_stream: &mut HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    body_seen: &mut HashMap<u64, usize>,
    pending_trailers: &mut HashMap<u64, Vec<(String, String)>>,
    request_tasks: &mut Vec<tokio::task::JoinHandle<(u64, Vec<u8>)>>,
    resp_rx_by_stream: &mut HashMap<u64, mpsc::Receiver<RespEvent>>,
    resp_tasks: &mut Vec<tokio::task::JoinHandle<()>>,
    stream_response: &mut HashMap<u64, StreamTx>,
    pool: &TcpPool,
    backends: &Arc<Vec<SocketAddr>>,
    h3_backend: Option<&(QuicUpstreamPool, SocketAddr, String)>,
    h2_backend: Option<&(Http2Pool, SocketAddr)>,
) {
    // PASS 1 — re-arm/backpressure drain. quiche Data is edge-triggered
    // (mod.rs:2797 try_trigger_data_event); since the R8 gate stops
    // recv_body while the channel is full (not draining to Done), Data
    // won't re-fire — so re-attempt the capacity-gated drain every tick
    // for every body-phase stream, independent of poll events. (This is
    // exactly what the old drain_body_stream did.)
    let active: Vec<u64> = body_tx_by_stream.keys().copied().collect();
    for sid in active {
        drain_request_body(conn, h3, sid, body_tx_by_stream, body_seen, pending_trailers);
    }

    // PASS 2 — event loop.
    loop {
        match h3.poll(conn) {
            Ok((sid, quiche::h3::Event::Headers { list, more_frames })) => {
                let headers: Vec<(String, String)> = list
                    .iter()
                    .map(|h| (
                        String::from_utf8_lossy(h.name()).into_owned(),
                        String::from_utf8_lossy(h.value()).into_owned(),
                    ))
                    .collect();

                // A 2nd HEADERS on a stream already in body phase = the
                // RFC 9114 §4.1 trailing field section.
                if body_tx_by_stream.contains_key(&sid) {
                    // §4.3: a pseudo-header in trailers is malformed → Reset.
                    if headers.iter().any(|(n, _)| n.starts_with(':')) {
                        if let Some(tx) = body_tx_by_stream.remove(&sid) {
                            let _ = tx.try_send(ReqBodyEvent::Reset);
                        }
                        body_seen.remove(&sid);
                        pending_trailers.remove(&sid);
                        continue;
                    }
                    pending_trailers.insert(sid, headers);
                    continue;
                }

                // Initial request HEADERS. (#12-15) pseudo validation FIRST.
                if let Err(reason) = validate_request_pseudo_headers(&headers) {
                    tracing::warn!(stream_id = sid, reason,
                        "SESSION 22: malformed H3 request rejected (H3_MESSAGE_ERROR)");
                    reset_h3_stream(conn, sid, H3_MESSAGE_ERROR);
                    continue;
                }
                let req = H3Request::from_headers(headers);
                // authority sanitisation (ROUND8-L7-16) — inline 400.
                if !req.authority.is_empty() {
                    if let Err(e) = lb_core::authority::validate(&req.authority) {
                        tracing::warn!(authority = %req.authority, error = ?e,
                            stream_id = sid, "ROUND8-L7-16: H3 :authority rejected");
                        let resp = encode_h3_response(400, b"bad request").unwrap_or_default();
                        request_tasks.push(tokio::spawn(async move { (sid, resp) }));
                        continue;
                    }
                }
                let fin = !more_frames;
                // Spawn the cell + register the body channel UNIFORMLY;
                // the Finished event emits End (bodyless ⇒ Finished comes
                // right after this Headers in the same poll loop).
                let (btx, brx) = mpsc::channel::<ReqBodyEvent>(H3_BODY_CHANNEL_DEPTH);
                let (resp_tx, resp_rx) = mpsc::channel::<RespEvent>(H3_RESP_CHANNEL_DEPTH);
                resp_rx_by_stream.insert(sid, resp_rx);
                stream_response.insert(sid, StreamTx::progressive());
                if let Some((h2pool, addr)) = h2_backend {
                    let (h2pool, addr) = (h2pool.clone(), *addr);
                    resp_tasks.push(tokio::spawn(async move {
                        if let Err(abort) = h3_to_h2_stream_resp(&req, addr, &h2pool, brx, resp_tx, MAX_RESPONSE_BODY_BYTES).await {
                            tracing::warn!(?abort, stream_id = sid, "H3→H2 resp stream aborted");
                        }
                    }));
                } else if let Some((qpool, addr, sni)) = h3_backend {
                    let (qpool, addr, sni) = (qpool.clone(), *addr, sni.clone());
                    resp_tasks.push(tokio::spawn(async move {
                        if let Err(abort) = h3_to_h3_stream_resp(&req, addr, &sni, &qpool, brx, resp_tx, MAX_RESPONSE_BODY_BYTES).await {
                            tracing::warn!(?abort, stream_id = sid, "H3→H3 resp stream aborted");
                        }
                    }));
                } else {
                    let Some(backend) = select_backend(backends) else {
                        tracing::warn!("no backends available for H3 request");
                        resp_rx_by_stream.remove(&sid);
                        stream_response.remove(&sid);
                        continue;
                    };
                    let pool = pool.clone();
                    resp_tasks.push(tokio::spawn(async move {
                        if let Err(abort) = h3_to_h1_stream_resp(&req, backend, &pool, brx, resp_tx, MAX_RESPONSE_BODY_BYTES).await {
                            tracing::warn!(?abort, stream_id = sid, "H3→H1 resp stream aborted");
                        }
                    }));
                }
                body_tx_by_stream.insert(sid, btx);
                body_seen.insert(sid, 0);
                if fin {
                    // Bodyless HEADERS+FIN: drain (no-op) + the Finished
                    // event (next iteration) sends End. But quiche may not
                    // surface Finished if body never registered? It does —
                    // stream_finished true → finished_streams. Safe.
                }
                // Drain any coalesced body that arrived with the head.
                drain_request_body(conn, h3, sid, body_tx_by_stream, body_seen, pending_trailers);
            }
            Ok((sid, quiche::h3::Event::Data)) => {
                drain_request_body(conn, h3, sid, body_tx_by_stream, body_seen, pending_trailers);
            }
            Ok((sid, quiche::h3::Event::Finished)) => {
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let trailers = pending_trailers.remove(&sid).unwrap_or_default();
                    let _ = tx.try_send(ReqBodyEvent::End { trailers });
                }
                body_seen.remove(&sid);
            }
            Ok((sid, quiche::h3::Event::Reset(code))) => {
                tracing::debug!(stream_id = sid, code,
                    "INC-2 F-MD-4: client reset request stream; Reset to upstream");
                if let Some(tx) = body_tx_by_stream.remove(&sid) {
                    let _ = tx.try_send(ReqBodyEvent::Reset);
                }
                body_seen.remove(&sid);
                pending_trailers.remove(&sid);
            }
            Ok((_sid, _)) => {} // GoAway / PriorityUpdate / Datagram — ignore
            Err(quiche::h3::Error::Done) => break,
            Err(e) => {
                // quiche owns #11/#16-22/#24 — it has already conn.close'd
                // on a protocol violation. Log + stop polling this tick.
                tracing::debug!(error = %e, "INC-2: h3.poll error (quiche closed conn)");
                break;
            }
        }
    }
}
```

## DELETE
- conn_actor.rs: old poll_h3 body, `drain_body_stream`, `decode_into_pending`,
  `flush_pending`, `record_retained_for_stream` (replace with `record_req_retained`).
- h3_bridge.rs: `StreamRxBuf` (struct+impl), `feed`, `feed_body`, `try_parse_frame_header`,
  `retained_bytes`, `is_too_large`, `RxPhase`, `BodyParse`, `BodyItem`, `FeedError`,
  `MAX_FRAME_HEADER_BYTES`(if only used there — CHECK: H3_FRAME_HDR_MAX aliases it, KEEP),
  `MAX_TRAILER_BLOCK_BYTES`(if now unused). Their unit tests (lines ~4750-4900 feed_body
  tests). KEEP: encode_h3_* (egress uses them), H3_BODY_CHUNK_MAX, MAX_REQUEST_BODY_BYTES.

## NEW gauge (replaces record_retained_for_stream)
```rust
#[cfg(any(test, feature = "test-gauges"))]
fn record_req_retained(
    sid: u64,
    body_tx_by_stream: &HashMap<u64, mpsc::Sender<ReqBodyEvent>>,
    last_read: usize,
) {
    // quiche holds un-read body bytes flow-control-bounded (NOT proxy-
    // retained); the proxy retains only the bounded channel occupancy +
    // the just-read scratch chunk. Upper bound, body-size independent.
    let chan = body_tx_by_stream.get(&sid)
        .map_or(0, |tx| tx.max_capacity().saturating_sub(tx.capacity()));
    crate::h3_bridge::record_retained(
        chan.saturating_mul(crate::h3_bridge::H3_BODY_CHUNK_MAX) + last_read,
    );
}
```

## OPEN VERIFY POINTS
1. reset_h3_stream(conn,...) on an h3-owned stream — transport-level shutdown; confirm
   the h3 conn tolerates it (poll returns Err next, handled). Verify via pseudo-reject test.
2. Bodyless: confirm quiche emits Finished after Headers(more_frames=false) so End fires.
3. H3_BODY_CHANNEL_DEPTH must still be imported/defined in conn_actor (it is, line 54).
4. `Bytes` import already present (conn_actor line 35).
5. MAX_FRAME_HEADER_BYTES / MAX_TRAILER_BLOCK_BYTES — grep all uses before deleting.
