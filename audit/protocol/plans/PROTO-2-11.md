# Plan for PROTO-2-11 — H2 GOAWAY + H3 CONNECTION_CLOSE on drain

Finding-ref:    PROTO-2-11 (high, Open)
Files touched:
  - `crates/lb-l7/src/h2_proxy.rs` (per-connection
    `CancellationToken`; `graceful_shutdown` call)
  - `crates/lb-quic/src/conn_actor.rs` (H3 GOAWAY emit;
    `quiche::Connection::close` with `H3_NO_ERROR = 0x0100`)
  - `crates/lb-quic/src/listener.rs` (shutdown plumbing per active
    connection)
  - `crates/lb/src/main.rs` (SIGTERM handler — code-owned per
    synthesis §D, but proto contributes the H2/H3 graceful calls
    layered onto code's drain scaffold; proto's edit lands after
    code's CODE-2-03)
  - `tests/h2_graceful_goaway.rs` (new)
  - `tests/h3_graceful_close.rs` (new)

Approach:
Per cross-review §E.1 (lead-ratified):

**HTTP/2 — two-step GOAWAY with `NO_ERROR (0x0)`:**

1. SIGTERM handler signals a per-listener `CancellationToken`.
2. The H2 accept loop stops accepting new TCP connections.
3. For every live H2 connection, call hyper's
   `http2::Connection::graceful_shutdown()`. Hyper internally:
     - emits GOAWAY #1: `last_stream_id = 2^31 - 1`, `NO_ERROR`,
       *immediately* (signals "no new streams").
     - waits for in-flight streams to complete (or until the
       `keep_alive_timeout` / drain budget expires).
     - emits GOAWAY #2: real `last_stream_id` (highest stream
       hyper processed), `NO_ERROR`.
4. The gateway does **not** compute `last_stream_id` manually —
   per RFC 9113 §6.8 and the cross-review §E.1 decision, this is
   delegated to hyper.

Concrete shape in `h2_proxy.rs`:

```rust
// inside serve_h2_conn:
let h2_conn = builder.serve_connection(io, service).into_owned();
let h2_conn = h2_conn.with_upgrades();
tokio::select! {
    res = &mut h2_conn => res?,
    _ = drain_token.cancelled() => {
        h2_conn.graceful_shutdown();
        // poll the connection to completion so GOAWAY #2 flushes
        h2_conn.await?;
    }
}
```

(The exact hyper 1.x API for graceful shutdown on the **server**
connection is `hyper::server::conn::http2::Connection::
graceful_shutdown(Pin<&mut Self>)`; the implementation above is the
canonical shape from hyper's own examples and matches the
`hyper-util` graceful-shutdown helper.)

**HTTP/3 — H3 GOAWAY + QUIC `CONNECTION_CLOSE` with
`H3_NO_ERROR = 0x0100`:**

1. SIGTERM signals the per-QUIC-connection `CancellationToken`
   held by `conn_actor`.
2. The actor calls `h3_conn.send_goaway(max_request_id)` — quiche
   exposes this on the H3 `Connection` type; the `max_request_id`
   parameter is what RFC 9114 §5.2 calls the "stream identifier".
   Quiche computes this internally if the caller passes the
   sentinel value; the gateway uses
   `h3_conn.send_goaway(quiche::h3::Connection::MAX_REQUEST_ID)`
   to delegate (per cross-review §E.1).
3. Wait up to `drain_timeout` for in-flight requests to complete.
4. Call `quiche_conn.close(true, 0x100, b"shutdown")`. The
   `0x100` is `H3_NO_ERROR` per RFC 9114 §8.1; `true` indicates
   "application close" so the QUIC layer emits a
   `CONNECTION_CLOSE` with frame type `0x1d` (application
   close, RFC 9000 §19.19).
5. Continue polling `quiche_conn.send()` until the
   `CONNECTION_CLOSE` is fully transmitted; only then drop the
   actor and release the per-CID DashMap entry (CODE-2-08 cross-cut).

**Drain budget & abort fallback (rel-owned, proto consumes):**
After `[runtime].drain_timeout_ms` (default 10 s — rel sets the
default in REL-2-02), an unconditional `JoinHandle::abort()`
fallback fires for any connection that did not finish gracefully.
The metric `shutdown_aborted_connections_total{protocol}` records
the fallback for ops visibility (rel owns metric definition).

**H1 / plain-TCP** are out of scope for this finding (see cross-
review §E.1 steps 5–6; H1 is `Connection: close` on next
response; plain-TCP is `shutdown(SHUT_WR)`).

Proof:
  - Test: `tests/h2_graceful_goaway.rs::two_step_goaway_observed_on_wire`
    Invariant: spawn gateway with H2 listener. Open an H2 stream
    via raw TCP + `h2 = "0.4"` client; while the stream is in flight,
    trigger drain via a test hook (or send SIGTERM equivalent). Read
    raw bytes off the wire: assert the first GOAWAY frame has
    `last_stream_id = 0x7FFFFFFF` and `error_code = 0x0`; assert the
    second GOAWAY frame (after stream completes) has
    `last_stream_id` equal to the test stream's ID and
    `error_code = 0x0`. Assert no `Connection: close`-style RST.
  - Test: `tests/h2_graceful_goaway.rs::in_flight_stream_completes_before_close`
    Invariant: H2 stream with 1 MB body upload completes its
    response *after* drain is signalled; the connection TCP-FIN
    arrives only after the response trailers.
  - Test: `tests/h3_graceful_close.rs::h3_goaway_then_connection_close`
    Invariant: open H3 connection via quiche client; signal drain;
    capture UDP packets via tracing or pcap; parse with `h3i`:
    assert an H3 GOAWAY frame on the server control stream, then a
    QUIC `CONNECTION_CLOSE` with frame type `0x1d` and error code
    `0x100`.
  - Test: `tests/h3_graceful_close.rs::h3_inflight_request_completes`
    Invariant: H3 request in flight completes after drain signal.
  - Test (negative): `tests/h2_graceful_goaway.rs::abort_fallback_after_budget`
    Invariant: simulate an H2 stream that never sends its body
    (drain budget elapses); after 10 s the connection RSTs and the
    `shutdown_aborted_connections_total{protocol="h2"}` counter
    increments by exactly 1.

Risk / blast radius:
  - **Behaviour change visible to every client** that today sees a
    bare TCP RST/FIN on SIGTERM. Browsers and gRPC clients will
    correctly retry on a different connection rather than retrying
    the in-flight stream — net effect is **fewer** duplicate writes
    to upstream, not more.
  - Drain time budget is operator-tunable but the default 10 s is
    rel-set; if operators set it too low, real requests get
    aborted. This is a known trade-off documented in the runbook.
  - Coupling: code's CODE-2-03 drain scaffolding lands first; proto
    layers GOAWAY/CONNECTION_CLOSE emission on top. The synthesis
    §D ordering (code → rel → proto for `main.rs`) covers this.
  - QPACK encoder state: a panic mid-frame during graceful close
    leaves the peer's QPACK dynamic table in a bad state. CODE-2-02
    (`panic = "abort"`) + CODE-2-08 (`catch_unwind` on actor) close
    that gap; this plan depends on both landing first.

Cross-ref:    joint with REL-2-02 (drain budget owner), CODE-2-03
              (cancellation plumbing), CODE-2-08 (per-CID cleanup);
              composes with PROTO-2-05 case 6 (h3i GOAWAY
              assertion); closes PROTO-2-11.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
