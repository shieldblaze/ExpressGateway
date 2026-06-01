# INC-2V Task E — R8 bounded-memory + backpressure proof for the WS relay

> **CORRECTION (INC-5V, supersedes parts of this document).** Two claims in
> the original INC-2V text below were SUPERSEDED by the `1a308ac3`
> reconciliation and independently re-confirmed at INC-5V:
>
> 1. **"affects H1 + H2" / "R12 sibling on H1" is a FALSE POSITIVE.**
>    WS-over-**H1 is BOUNDED and DOES backpressure.** The relay's
>    `send().await` parks on the raw TCP socket's `WouldBlock`
>    (tokio-tungstenite `Sink` → `poll_flush` Pending), so a non-reading
>    client stalls the producer. Independently re-proven at INC-5V:
>    `tests/ws_r8_backpressure_plateau.rs::h1_backend_flood_plateaus_against_nonreading_client`
>    — a 2048-msg (~512 MiB) flood at a non-reading H1 client PLATEAUS at
>    **17–18 messages** (x3). The original INC-2V "H1 identical" was asserted
>    by code-similarity, NOT measured on H1; it was wrong.
> 2. **Root cause is NOT `WsConfig::max_write_buffer_size = usize::MAX`.**
>    That is one contributing layer, but the true unbounded sink for the H2
>    tunnel is BELOW tungstenite: hyper's `H2Upgraded::poll_write`
>    (hyper-1.9.0 proto/h2/upgrade.rs) feeds an `mpsc::channel(1)` →
>    `UpgradedSendStreamTask` → `h2::SendStream::send_data`, which BUFFERS
>    until window capacity is available (h2-0.4.13 share.rs:49-57) and is NOT
>    bounded by hyper's `max_send_buf_size` for upgraded streams. So the
>    relay's `send().await` never parks on the H2 tunnel → unbounded gateway
>    memory. The bounded `max_write_buffer_size` added in INC-3 (`bd3f991d`)
>    is a defensive hardening, not the fix.
>
> NET (INC-5V): F-S27-2 is **H2-ONLY**, carried as **CF-S27-2** (genuinely
> large fix), and WS-over-H2 is **gated OFF by default** as the owner
> disposition. WS-over-H1 is SAFE/bounded and ships on. The (i) bounded-peak
> result below stands (it was always correct, for the drained case). Read
> this banner as authoritative where it conflicts with the INC-2V body.

---

Independent verifier, SESSION 27 INC-2V. Subject: the shared WS relay core
`crates/lb-l7/src/ws_proxy.rs::proxy_frames` (ws_proxy.rs:217-355), exercised
real-wire over WS-over-H2 (RFC 8441) through `H2Proxy(with_websocket)`.
Harness: `tests/ws_h2_r8_backpressure.rs` (committed: properties (i) +
attribution A/B, both green) + the red backpressure assertion preserved as
`audit/websockets/s27-fs27-1-proof/ws_h2_r8_backpressure.rs.txt`.

## Property (i): bounded in-flight memory — HOLDS

Test `r8_peak_memory_is_message_volume_independent`. Push N=2000 then 10N=
20000 round-trip Text messages, draining each echo so in-flight depth is
O(1). Gauge: VmHWM (peak RSS) from /proc/self/status.

Isolated x3 (captured `s27-r8-console.txt`):

```
run 1: after 2000 msgs VmHWM=16448kB ; after 20000 msgs VmHWM=16620kB ; growth=172kB
run 2: after 2000 msgs VmHWM=16496kB ; after 20000 msgs VmHWM=16716kB ; growth=220kB
run 3: after 2000 msgs VmHWM=16396kB ; after 20000 msgs VmHWM=16628kB ; growth=232kB
```

VERDICT: PASS. Peak memory is message-VOLUME-INDEPENDENT — 10x the message
count grows the peak by ~0.2 MiB (allocator/runtime noise), NOT ~10x the
byte volume. Non-vacuous: a relay that buffered the stream would have grown
the peak with N; here the drained relay forwards incrementally. The relay is
bounded by `WsConfig::max_message_size` per in-flight frame, not by the
number of messages. (i) HOLDS **for a consumer that keeps reading.**

## Property (ii): bidirectional backpressure — FAILS → F-S27-2 (BLOCKER)

Test `r8_backpressure_slow_client_stalls_backend` (the red assertion; in the
audit `.rs.txt`). A real tungstenite flooder backend tries to push 4096 x
64 KiB = 256 MiB toward the client; the client opens the WS-over-H2 tunnel,
sends ONE "go" frame, then NEVER reads. The backend uses a SMALL bounded
write buffer + per-frame flush so its `flush().await` blocks on real TCP
backpressure once the gateway stops draining. A shared `pushed` counter =
frames the backend got past its socket. Timeline (captured):

```
R8(ii): client NOT reading, client h2 window=64KiB
  | t1s pushed=1067 rss=83996kB | t3s pushed=3106 rss=215224kB
  | t6s pushed=4096 rss=282172kB | flood=4096
  R8(ii) VIOLATION: RSS grew ~196632kB under a stalled reader
```

The backend pushed the ENTIRE 4096-frame (256 MiB) flood and process RSS
climbed to ~282 MiB while the client never read a single frame. This is
CONTINUED GROWTH TO COMPLETION, not a bounded plateau.

### Attribution (rigorous — symptom != attribution)

All three actors (backend, gateway, test client) share ONE process, so RSS
sums them. To prove the buffering is in the GATEWAY (not the test client's
own h2 recv buffer), the A/B control `r8_backpressure_attribution_ab` runs
the SAME flood twice with the client never reading, varying ONLY the test
client's h2 receive window:

```
R8(ii)-AB [TINY-64KiB] : after 2s, pushed=2104/4096, RSS delta ~138 MiB
R8(ii)-AB [HUGE-256MiB]: after 2s, pushed=2089/4096, RSS delta ~154 MiB
```

The TINY (64 KiB) and HUGE (256 MiB) client windows produce ESSENTIALLY
IDENTICAL throughput and memory. The client's advertised flow-control window
is IRRELEVANT — the gateway absorbs the flood at the same rate regardless.
=> The buffering is INSIDE THE GATEWAY; client H2 flow control does not
throttle the backend.

### Root cause (code, file:line)

`WsConfig::tungstenite_config()` (ws_proxy.rs:130-136) sets `max_message_size`
and `max_frame_size` but leaves tungstenite's **`max_write_buffer_size` at its
default of `usize::MAX`** (tungstenite-0.24.0 protocol/mod.rs:81). The relay
forwards via `client_tx.send(msg).await` (ws_proxy.rs:333) / `backend_tx.
send(msg).await` (ws_proxy.rs:320), where `futures::SinkExt::send` =
feed+flush on the tungstenite `WebSocketStream`.

tungstenite docs (protocol/mod.rs:47-53): "the write buffer only builds up
past `write_buffer_size` when writes to the underlying stream are failing"
and `max_write_buffer_size` "can provide backpressure" — default UNLIMITED.
When the slow client's H2 window is exhausted, the gateway-side
`hyper::Upgraded` (H2Upgraded) `poll_write` returns Pending (its internal
mpsc(1) + H2 flow control DO backpressure), so tungstenite's flush cannot
drain — but because `max_write_buffer_size = usize::MAX`, tungstenite keeps
ACCEPTING messages into its unbounded write buffer and `send().await` returns
Ready anyway. The relay loop therefore never parks; it keeps reading the
backend (`backend_rx.try_next()`) and piling frames into the gateway-side
write buffer without bound. The single-select-loop structure is correct in
principle (a parked `send` WOULD stop reading the other half) — but the
unbounded write buffer means `send` never parks.

VERDICT: (ii) FAILS. The relay does NOT backpressure a fast producer when
the consumer is slow/stalled; it buffers the producer's stream unbounded in
the gateway. R8 is VIOLATED for the slow-consumer case.

### Severity & scope — F-S27-2 (HIGH, DoS / memory exhaustion)

- A single malicious or merely slow WS client that opens a tunnel to a
  chatty/large-volume backend (or a malicious backend pushing to a slow
  client) forces the gateway to buffer the entire backend output stream in
  RAM — unbounded, per connection. N such connections multiply it. This is a
  remote, low-cost memory-exhaustion DoS. (The classic "slow read" proxy
  attack.)
- R12 SIBLING: the SAME defect exists on the H1 WS path. Both H1
  (`h1_proxy.rs:2648-2649`) and H2 (`h2_proxy.rs:1411-1412`) build the relay
  with the identical `server_ws` + `tungstenite_config()` + `proxy_frames`.
  The unbounded write buffer is shared. A fix MUST be single-sourced in
  `WsConfig::tungstenite_config()`.
- NOT introduced by the F-S27-1 fix — it is pre-existing in the shared relay.
  But it is in scope for SESSION 27 (WS proxying) and is a BLOCKER for any
  claim that the WS relay is R8-bounded under hostile/asymmetric load.

### Suggested fix (for the IMPLEMENTER — verifier does NOT fix)

Set a bounded `max_write_buffer_size` (and a small `write_buffer_size`) in
`WsConfig::tungstenite_config()`, derived from `max_message_size` (e.g.
`max_write_buffer_size = max_message_size + write_buffer_size`, with
`write_buffer_size` small, e.g. 0 or a few KiB). With a bounded write buffer
tungstenite's `send().await` will PARK when the downstream stalls, which
parks the relay's select loop, which stops reading the producer half, which
propagates backpressure to the producer (exactly the chain the test asserts).
Re-run `r8_backpressure_slow_client_stalls_backend` (the audit `.rs.txt`):
post-fix it must show `pushed` PLATEAU far below the flood and RSS bounded;
the A/B control should then show the TINY-window run throttled well below the
HUGE-window run. Verify on BOTH H1 and H2.

## Summary (CORRECTED at INC-5V — see top banner)

- (i) bounded peak memory: **PASS** (volume-independent, x3, growth ~0.2 MiB).
  Unchanged.
- (ii) bidirectional backpressure:
  - WS-over-**H1**: **PASS / BOUNDED** — relay parks on TCP `WouldBlock`;
    re-proven (plateau 17–18 / 2048, x3). The INC-2V "FAIL on H1" was a false
    positive (code-similarity, not measured).
  - WS-over-**H2**: **FAIL → CF-S27-2** (carried). Unbounded gateway buffering
    in hyper's `H2Upgraded` → `h2::SendStream` (NOT `WsConfig`). Owner
    disposition: WS-over-H2 **gated OFF by default**; not a ship blocker for
    S27. INC-3 added a bounded `max_write_buffer_size` + an anti-hang
    `timeout(read_frame, send)`→Close-1008 guard as defense-in-depth.
