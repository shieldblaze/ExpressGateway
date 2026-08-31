# S47 — Connection-pooling / upstream-IO correctness review

Reviewer: `cr-io-pool`. Branch `review/s47-rfc-security` (from main @ `01915a77`).
Method: read-and-reason only (`rg`/`sed`/`cat`). **No cargo command was run** — per the
hard rule. Every claim below is source-cited; nothing is claimed as "verified by
execution".

Prior art read before writing: `docs/known-limitations.md`, `docs/features.md`,
`docs/arch/backpressure.md`, `docs/guide/{METRICS,observability,RUNBOOK}.md`,
`audit/deferred.md`, `audit/code/`, `audit/h-matrix/s14-*`, `audit/soak/`.

---

## 0. Headline

The single most important structural fact this review found is not on the finding
list format — it frames everything else:

> **Two of the three "connection pools" never pool anything.** Every caller of
> `TcpPool::acquire_async` calls `take_stream()` on the next line, which detaches the
> socket from the return-to-pool `Drop`. Every exit of the H3-upstream connector calls
> `pooled.set_reusable(false)`. Only `Http2Pool` (which caches a hyper `SendRequest`,
> not a socket) actually reuses a connection.

This is *good* for the cross-request-contamination question the brief leads with — it
is the safest possible answer — and it is deliberate and commented (`ROUND8-L7-10`).
It is *bad* for cost, for the config/metrics/doc surface that describes these as pools,
and for the large body of reuse code (`probe_alive`, the QUIC PING-ACK probe, per-peer
LRU, `idle_timeout`, `max_age`, `total_max`) that is **unreachable in production** and
therefore untested by anything except unit tests that construct the state by hand.

---

## 1. The checkout/return contract, by protocol

### 1.1 H1 upstream (`TcpPool` → `hyper::client::conn::http1`)

Call sites, all identical in shape — `acquire_async().await` then `take_stream()` with
**no suspension point between them**, so a cancellation cannot land in the window:

| Call site | Front | file:line |
|---|---|---|
| `H1Proxy::proxy_request` | H1→H1 | `crates/lb-l7/src/h1_proxy.rs:858-868` |
| `H1Proxy::dial_upstream_ws` | H1→H1 (WS) | `crates/lb-l7/src/h1_proxy.rs:1936-1941` |
| `H2Proxy` Branch A (fits window) | H2→H1 | `crates/lb-l7/src/h2_proxy.rs:1180-1185` |
| `H2Proxy` Branch B (streaming) | H2→H1 | `crates/lb-l7/src/h2_proxy.rs:1219-1226` |
| `H2Proxy` WS extended-CONNECT | H2→H1 (WS) | `crates/lb-l7/src/h2_proxy.rs:1004-1010` |
| `GrpcProxy::handle` | H2→H2 (ad-hoc, not `Http2Pool`) | `crates/lb-l7/src/grpc_proxy.rs:145-155` |
| `build_ws_h3_launcher` | H3→H1 (WS) | `crates/lb/src/main.rs:1157-1165` |
| `Http2Pool::dial_and_handshake` | (H2 pool's own dial) | `crates/lb-io/src/http2_pool.rs:290-293` |
| `h3_to_h1_stream_resp` | H3→H1 | `crates/lb-quic/src/h3_bridge.rs:1148` (**holds `PooledTcp`**, marks `set_reusable(false)` on every exit at `:1161/:1173/:1178/:1185`) |

**Exit-path table — `H1Proxy::proxy_request` (`h1_proxy.rs:836-1134`), the canonical H1 leg.**
`take_stream()` at `:867` means the pool is out of the picture for every row; the column
records what happens to the *socket*.

| # | Exit path | Line | Upstream socket | Contamination risk |
|---|---|---|---|---|
| 1 | Dial fails | `:858-862` | none acquired | none |
| 2 | `take_stream()` returns `None` (unreachable) | `:866-868` | `PooledTcp` dropped, `pool=None` after take | none |
| 3 | H1 client handshake fails | `:872-877` | dropped with the failed handshake | none |
| 4 | `send_request` errors (`Ok(Err)`) | `:1088-1102` | `conn_handle.abort()` → socket closed | none |
| 5 | Idle (Phase A) / head (Phase B) deadline | `:1103-1108` | `conn_handle.abort()` → socket closed | none |
| 6 | Pump verdict = `BadRequest` / `BodyTooLarge` | `:1119-1125` | `conn_handle.abort()`; response **never relayed** | none |
| 7 | Pump died without a verdict | `:1126-1132` | `conn_handle.abort()` | none |
| 8 | Success | `:1113-1118` | `conn_handle` **detached** (`drop`), ends when the response body is fully read or the client drops it | none — socket is never re-parked |
| 9 | Downstream client disconnects mid-body | (implicit) | `Response<IncomingBody>` dropped → hyper client conn completes → socket closed | none |
| 10 | Downstream connection total deadline (`timeouts.total`, 60 s, `h1_proxy.rs:493-496`) | | service future dropped → `resp`/`sender` dropped → conn task completes | none |
| 11 | Task cancelled between `acquire_async` and `take_stream` | | **impossible** — no `.await` in the window | none |

**Verdict for H1: no cross-request contamination is reachable, because there is no
reuse.** The `set_reusable(false)` RAII pattern the brief asks about exists
(`pool.rs:283-289`) with a load-bearing doc block and a test that pins the doc
(`crates/lb-l7/tests/round8_body_overread.rs:66-79`), but **it has no production caller**
— `reusable: true` is the struct default (`pool.rs:255`), so the pattern is
"close by detaching", not "close by default + `mark_reusable()`". If anyone ever
removes a `take_stream()`, the default flips to *reuse a dirty socket*. That is the
correct thing to worry about; the existing comment says so.

### 1.2 H2 upstream (`Http2Pool` — the only real pool)

Cache = one `PeerEntry { sender, driver }` per `SocketAddr`
(`http2_pool.rs:72-87`). Multiplexed, so "dirty" is not a socket-state question; the
risks are eviction identity and teardown blast radius.

| # | Exit path | Line | Cached entry | Notes |
|---|---|---|---|---|
| 1 | `take_alive_sender` hit | `:255-265` | kept | identity-safe (check + remove under one lock) |
| 2 | `take_alive_sender` sees `!is_alive()` | `:259-262` | removed, driver aborted | correct |
| 3 | Fresh dial + `replace_entry` | `:246-251` | **blindly overwrites** | **IOP-01** |
| 4 | `send_request` → `Ok(Err(e))` | `:179-182` | `evict(addr)` — removes whatever is there now | **IOP-01** |
| 5 | `send_request` → timeout | `:183-186` | `evict(addr)` | same |
| 6 | `send_request_idle` → `Send` error | `:220-222` | `evict(addr)` | same |
| 7 | `send_request_idle` → idle/head deadline | `:223-235` | `evict(addr)` + warn log with phase | correct, phase is log-only |
| 8 | Inbound body truncated (F-MD-4) | `h2_proxy.rs:2404/2424/2449/2464/2468` | `reset_peer(addr)` — whole-connection teardown | documented trade-off; **ALREADY-KNOWN** |
| 9 | Success | `:178` | kept, reused | |
| 10 | Peer sends GOAWAY while draining | — | `is_alive()` still true → request sent → `Send` error → 502, no retry | **IOP-09** |

### 1.3 H3 upstream (`QuicUpstreamPool`)

`stream_request_to_h3_upstream` (`h3_bridge.rs:1619` acquire): **every** exit marks the
connection non-reusable, with an explicit comment at `:2020-2021`:

```rust
// One request per pooled upstream conn — non-reusable on EVERY exit.
pooled.set_reusable(false);
```

Sites: `:1639, :1652, :1668, :1686, :2021`. The only exits that skip it are the two
`pooled.get_mut()`/`connection_mut()` `None` arms (`:1625-1630`), where `Drop` returns
early anyway because `conn` is `None` — harmless.

**Verdict for H3: zero reuse; one full QUIC + TLS handshake per HTTP request** on
H1→H3, H2→H3 and H3→H3.

---

## 2. Findings

Severity ladder: CRITICAL / HIGH / MEDIUM / LOW / INFO. Every item carries an explicit
**blocking for prod** / **non-blocking** flag.

---

### IOP-01 — `Http2Pool` evicts by ADDRESS, not by entry identity: a racing dialer or a stale error tears down a live connection and every request on it

**Severity: HIGH · blocking for prod**

`crates/lb-io/src/http2_pool.rs:239-275`

```rust
async fn acquire_sender(&self, addr: SocketAddr) -> Result<SendRequest<H2ReqBody>, Http2PoolError> {
    if let Some(sender) = self.take_alive_sender(addr) {
        return Ok(sender);
    }
    let (sender, driver) = self.dial_and_handshake(addr).await?;   // <-- lock released across this await
    let entry = PeerEntry { sender: sender.clone(), driver };
    self.replace_entry(addr, entry);                                // <-- clobbers whatever is there
    Ok(sender)
}

fn replace_entry(&self, addr: SocketAddr, entry: PeerEntry) {
    let mut peers = self.inner.peers.lock();
    peers.insert(addr, entry);      // returns Option<PeerEntry>, dropped here
}

fn evict(&self, addr: SocketAddr) {
    let mut peers = self.inner.peers.lock();
    peers.remove(&addr);            // removes the CURRENT entry, not the failed one
}
```

The discarded `Option<PeerEntry>` from `insert`/`remove` is dropped immediately, and
`impl Drop for PeerEntry` (`:83-87`) calls `self.driver.abort()` — which kills the hyper
H2 connection task, closing the socket and failing **every stream on that connection**.

There is no singleflight: `take_alive_sender` returns `None` without inserting a
placeholder, so N concurrent requests to a cold peer all dial.

**Failure scenario A — cold-start thundering herd.**
1. Requests A and B arrive for backend `10.0.0.1:8080`; the peer cache is empty
   (process start, or immediately after any eviction).
2. Both miss `take_alive_sender`, both enter `dial_and_handshake`.
3. A completes first: `replace_entry` installs `entryA`; A returns `senderA` and its
   caller starts `senderA.send_request(...)`.
4. B completes: `peers.insert` returns `Some(entryA)` → dropped → **`driverA.abort()`**.
5. A's in-flight request dies mid-flight with an h2 "connection closed" error.
6. A's error path calls `self.evict(addr)` (`:180`) — which removes **`entryB`**, aborting
   `driverB`, so **B's request dies too**.

Result: a cold start with concurrency N produces a cascade of spurious `502`s
(`ProxyErr::UpstreamUnattributable` → 502) that has nothing to do with the backend. Each
eviction re-opens the window, so a flapping backend can drive a self-sustaining
teardown loop.

**Failure scenario B — stale-error collateral, no concurrency needed at dial time.**
Connection `C1` is cached and a long request `R1` is in flight on it. `C1` breaks.
`R2` arrives, sees `!is_alive()`, removes it, dials `C2`, caches `C2`, and starts
serving. `R1`'s `send_request` future *then* resolves `Err` and calls `evict(addr)` →
removes and aborts **`C2`**, killing `R2` and every other stream on `C2`.

**Distinguish from prior art.** This is **not** the documented
`reset_peer` collateral (`docs/known-limitations.md`, characterised in
`tests/h2h2_md_streaming_verify.rs:1852-1990`): that is a *client*-triggered
whole-connection teardown of the connection the aborting request was actually on. Here
the pool tears down a **different, healthy** connection than the one that failed. It is
also not the documented "failed send on a pooled H2 connection is deliberately not
counted" item — that is about health attribution, not about which entry gets removed.

**Would an existing test catch it?** **No.** `http2_pool.rs`'s in-crate tests are all
single-request (`:486-574`). `tests/h2h2_md_streaming_verify.rs:1852` drives two
concurrent requests but establishes the pooled connection first
(`sleep(400ms)` at `:1911`) precisely to avoid the cold-start race, and asserts only a
security floor. Nothing in `tests/` fires N concurrent cold-start requests at one
backend through `Http2Pool`.

**Fix shape:** give `PeerEntry` a monotonic generation id, return `(sender, gen)` from
`acquire_sender`, and make `evict`/`reset_peer` compare-and-remove on that id. Separately,
make `acquire_sender` singleflight (an `Entry`-API placeholder + `tokio::sync::OnceCell`,
the same shape `dns.rs` already uses) so a concurrent dialer joins instead of clobbering.

---

### IOP-02 — H3-front → H1-backend has NO upstream deadline anywhere, and its task is never aborted: a live-but-silent backend leaks a task + fd + QUIC stream permanently

**Severity: HIGH · blocking for prod**

`crates/lb-quic/src/h3_bridge.rs:574-604` and `crates/lb-quic/src/conn_actor.rs:197, 317, 341`

The H3→H1 response reader has no timer on any await:

```rust
// h3_bridge.rs:594-604
let n = match stream.read(&mut rbuf).await {
    Ok(n) => n,
    Err(_) => { let _ = tx.send(RespEvent::Reset).await; return Err(RespAbort::UpstreamReset); }
};
if n == 0 { let _ = tx.send(RespEvent::Reset).await; return Err(RespAbort::BadHead); }
```

`stream_h1_response` exits only on: read error, `n == 0` (peer FIN), `tx.send` failure
(`RespAbort::ClientGone`), the 64 KiB head cap, or the response cap. A backend that
completes the TCP handshake and then sends **nothing** blocks the first `read` forever;
the `tx.send` "client gone" escape is never reached because the code is parked on the
read, not on a send.

And the task is never cancelled. In `conn_actor.rs`:

```rust
// :197
let mut resp_tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();
// :317  — the ONLY thing done with them
resp_tasks.retain(|h| !h.is_finished());
// :340-342 — only WS tunnels are aborted on actor exit
for (_, st) in ws_tunnels { st.task.abort(); }
```

When the actor loop breaks — QUIC idle timeout, client `CONNECTION_CLOSE`, listener
cancel (SIGTERM drain), or the S36 cap recycle — `resp_tasks` is dropped, which
**detaches** every in-flight upstream relay rather than aborting it.

**Failure scenario.** A `quic` listener with plain/H1 backends
(`select_backend` → `backends[0]`, `conn_actor.rs:1056`). A client sends a request; the
backend accepts the TCP connection and stalls (overloaded thread pool, a stuck DB call,
or a deliberately silent peer). The client then goes away and the QUIC connection idles
out at `max_idle_timeout_ms` (default 30 s, `lb-config/src/lib.rs:677`). The actor exits.
The spawned `h3_to_h1_stream_resp` task is still parked in `stream.read()` holding one
tokio task, one file descriptor and one 8 KiB read buffer. `SO_KEEPALIVE` is on
(`main.rs:705`) but with **system defaults only** (Linux `tcp_keepalive_time` = 7200 s),
and keepalive detects a *dead* peer — a peer that is alive but silent ACKs the probes,
so the leak is **unbounded in time**. Sustained traffic to a hung backend exhausts the fd
table; nothing reclaims it short of process restart.

**Contrast — the same class is bounded on every other leg**, which is what makes this an
outlier rather than a design choice:

| Leg | Upstream deadline |
|---|---|
| H1 front → H1/H2 | `idle_bounded_send`, `timeouts.body` / `timeouts.head` (`h1_proxy.rs:1075-1083`) |
| H2 front → H1/H2 | same, via `drive_h2_upstream_send` (`h2_proxy.rs:2388-2396`) |
| H3 front → H3 | `H3_RESP_IDLE_TIMEOUT = 30 s`, re-armed on progress (`h3_bridge.rs:74, 1592, 1766`) |
| H3 front → H2 | `Http2Pool::send_request` `send_timeout` = 30 s on the head; body bounded by the H2 keep-alive PING (30 s / 10 s) |
| **H3 front → H1** | **none** |

This also qualifies the "single-sourced idle/head deadline" property from S14: it is
single-sourced across the **H1 and H2 fronts**, but the H3 front does not participate —
one leg re-derives it (`H3_RESP_IDLE_TIMEOUT`) and one has nothing.

**Would an existing test catch it?** **No.** Nothing in `tests/` drives an H3 front at a
backend that accepts-then-stalls, and there is no fd/task-count assertion on that path.
`crates/lb-soak/` scenarios cover flood/reset classes, not accept-then-silence.

**Fix shape (two independent halves, both needed):** (a) wrap the H3→H1 relay in the same
two-phase idle/head deadline the H1/H2 fronts use, or at minimum a
`tokio::time::timeout` around the head read; (b) `abort()` `resp_tasks` when the actor
loop exits, exactly as `ws_tunnels` already are at `conn_actor.rs:340-342`.

---

### IOP-03 — `TcpPool` and `QuicUpstreamPool` never reuse a connection; the reuse machinery, the size knobs and the pool metrics are all inert

**Severity: MEDIUM (cost + false operator signal; not a correctness defect) · non-blocking, but should be documented before ship**

`crates/lb-io/src/pool.rs` (whole file), `crates/lb-io/src/quic_pool.rs` (whole file)

Evidence — exhaustive, from the call-site sweep in §1:

* **`TcpPool`**: all 9 `acquire_async` call sites either `take_stream()` on the next line
  (8 of them) or `set_reusable(false)` on every exit (`h3_to_h1_stream_resp`). Nothing in
  the workspace lets a `PooledTcp` drop with `pool: Some(_)` and `reusable: true`.
  Therefore `PooledTcp::return_to_pool` (`pool.rs:303-358`) never parks anything,
  `pop_idle` (`:140-149`) always returns `None`, and `acquire`/`acquire_async` always fall
  through to `dial_new_async`.
* **`QuicUpstreamPool`**: `set_reusable(false)` on every exit (`h3_bridge.rs:2020-2021`).

Consequences:

1. **Dead code on the production path.** `probe_alive` (`pool.rs:369-379`, the "Pingora
   EC-01" liveness probe), `probe_liveness` (`quic_pool.rs:232-272`, the "Pingora EC-16"
   PING-ACK probe), `IdleConn`, the per-peer LRU, `idle_timeout`, `max_age`,
   `per_peer_max`, `total_max` — none of it executes in production. Its only exercise is
   unit tests that seed the queue by hand; `quic_pool.rs:625-627` and `:643-645` say so
   in their own comments ("HONEST LIMITATION: does NOT exercise `per_peer_max`").
2. **Cost.** One TCP three-way handshake per H1-upstream request (H1→H1, H2→H1, H3→H1,
   WS, and gRPC which builds its own H2 client per request at
   `grpc_proxy.rs:159-172`). One **full QUIC + TLS handshake with certificate
   verification per request** on every H3-upstream leg.
3. **No cap on ACTIVE upstream connections.** `total_max = 256` / `per_peer_max = 8`
   bound *idle* entries only. The only backstop is the downstream
   `runtime.max_inflight_connections` (default 65 536, `main.rs:2283-2286`), and an H2
   front multiplies that by its stream concurrency. At scale the gateway hits `EMFILE`,
   surfacing as `502`, with no pool-level admission control or wait queue.
4. **Scope item 4 answered directly:** there is **no wait queue** at all — no fairness
   policy, no queue timeout, no waiter-cancellation bookkeeping. There is therefore also
   no leaked-waiter bug. On reload, `rebuild_l7_proxies` (`main.rs:510+`) builds a fresh
   `Http2Pool` per listener, so removed backends' H2 entries go with the old pool; the
   `TcpPool` is process-wide but holds nothing, so nothing to drain or leak.
5. **The operator signal is wrong.** `docs/guide/METRICS.md:49-51, 85-87` and
   `docs/guide/observability.md:68` present `pool_acquires_total`,
   `pool_probe_failures_total` and `pool_idle_gauge` as pool-reuse signals
   ("high probe-failure ratio means upstream is half-closing idle conns"). In fact:
   * `pool_acquires_total` / `pool_probe_failures_total` are incremented **only** in
     `proxy_connection` (`main.rs:3259, 3278`), the **L4 plain-TCP relay**, which calls
     `TcpStream::connect` directly (`main.rs:3294`) and never touches `TcpPool`. So they
     count L4 dials and L4 dial failures. No L7 pool path increments anything.
   * `pool_idle_gauge` samples `TcpPool::idle_count()` once per second
     (`main.rs:1936`) and is therefore **permanently 0**.
   * METRICS.md:50 says `pool_probe_failures_total` fires "when `TcpPool::acquire` errors"
     — `proxy_connection` never calls `TcpPool::acquire`.

**Would an existing test catch it?** No — and it *cannot* be caught by a unit test,
because each pool's unit tests construct the pooled state directly.

**Recommendation:** decide explicitly. Either (a) document the "one connection per
request, by design" property in `docs/known-limitations.md` and delete or `#[cfg(test)]`-gate
the unreachable reuse machinery and the misleading metrics/doc rows; or (b) enable H1
reuse behind the body-length guard the `ROUND8-L7-10` comment already specifies
(`h1_proxy.rs:827-836`) — in which case IOP-12, IOP-13 and IOP-18 all become live.

---

### IOP-04 — `h2_security.max_concurrent_streams` is fed into hyper's `max_concurrent_reset_streams` on the upstream client: wrong knob, no upstream concurrency cap, and an operator-controlled unbounded reset-stream memory budget

**Severity: MEDIUM · blocking for prod (config-semantics + memory amplification)**

`crates/lb-io/src/http2_pool.rs:295-301`

```rust
builder
    .initial_stream_window_size(self.inner.config.initial_stream_window)
    .max_concurrent_reset_streams(self.inner.config.max_concurrent_streams as usize)
```

`Http2PoolConfig::max_concurrent_streams` is documented at `:48` as "Concurrent streams
per H2 connection", and `main.rs:916-918` populates it from
`h2_security.max_concurrent_streams`, whose config doc
(`crates/lb-config/src/lib.rs:547-549`) reads: *"Cap on concurrent streams the server will
accept."* That same value correctly drives the **front** at
`crates/lb-l7/src/h2_security.rs:71` via `.max_concurrent_streams(...)`. On the upstream
client it lands on a different setting.

Per `h2-0.4.19/src/client.rs:914-932`:

> "Sets the maximum number of concurrent **locally reset** streams... internal state must
> be maintained... this state grows linearly with the number of streams that are locally
> reset... **The default value is currently 50.**"

Two distinct consequences:

1. **There is no client-side cap on outbound concurrent streams.** The knob for that is
   `initial_max_send_streams` (available on hyper's builder,
   `hyper-1.10.1/src/client/conn/http2.rs:353`) and it is never set. The pool also never
   reads the peer's advertised `SETTINGS_MAX_CONCURRENT_STREAMS`, never opens a second
   connection when the first is saturated, and has no per-connection stream accounting at
   all. Excess requests queue inside hyper's dispatcher instead of being admission-controlled.
2. **Memory amplification from a front-side hardening knob.** `max_concurrent_streams` is
   `Option<u32>` with **no validation range** in `lb-config` (`lib.rs:549`; contrast
   `max_inflight_connections` at `:930` and `connect_timeout_ms` at `:936`, which are
   range-checked). An operator raising the front-side cap — a plausible tuning action —
   silently raises the upstream client's per-connection reset-stream retention from h2's
   default 50 to that number, `as usize`.

**Failure scenario.** Operator sets `h2_security.max_concurrent_streams = 100000` to admit
more front-side streams. Every upstream H2 connection now retains state for up to 100 000
locally-reset streams. The gateway resets upstream streams on the F-MD-4 path and on any
client abort; a client that opens-and-aborts in a loop against an H2 backend now grows
h2's reset-stream table by 2000× what the library bounds it to, per pooled connection,
with no counter or gauge on it.

**Would an existing test catch it?** No. `http2_pool.rs:341-349`
(`defaults_match_documented_values`) asserts the *config struct field* equals 256; it does
not assert which h2 builder method receives it. `crates/lb-l7/tests/round8_edge_defaults_table.rs:15`
pins only the front-side default.

**Fix shape:** set `initial_max_send_streams` from `max_concurrent_streams`, leave
`max_concurrent_reset_streams` at the h2 default (or give it its own bounded knob), and
either range-check `h2_security.max_concurrent_streams` in `lb-config` or stop routing a
server-side value into the client builder.

---

### IOP-05 — `runtime.connect_timeout_ms` is ignored by every L7 upstream dial

**Severity: MEDIUM · blocking for prod**

`crates/lb-io/src/pool.rs:42` (doc), `:29` (constant), `crates/lb/src/main.rs:2172`

```rust
// pool.rs:42
/// Dial deadline for `acquire_async`; IGNORED by blocking `acquire`, which gets the kernel default.
pub connect_timeout: Duration,
// pool.rs:28-29
/// Default connect-timeout for a fresh async dial; mirrors `runtime.connect_timeout_ms`.
pub const DEFAULT_CONNECT_TIMEOUT_MS: u64 = 5_000;
```

The claim "mirrors `runtime.connect_timeout_ms`" is false. `main.rs:2172` builds the
process-wide pool with `PoolConfig::default()`, and every other `TcpPool::new` in the tree
does the same. The config value is read at `main.rs:2311-2315` and threaded **only** into
`proxy_connection` (the L4 relay, `main.rs:3294`) and the WS header budget.

**Failure scenario.** An operator sets `runtime.connect_timeout_ms = 500` (valid; the
range is `100..=60_000`, `lb-config/src/lib.rs:936`) to cut the SYN-black-hole tail — the
stated purpose of the default (`lb-config/src/lib.rs:362`). L4 relay dials honour 500 ms.
Every L7 upstream dial — H1→H1, H2→H1, H3→H1, the H2 pool's own TCP dial, gRPC, WS —
silently uses **5000 ms**. With `timeouts.total` at 60 s, a black-holed backend consumes
5 s of every request's budget instead of 0.5 s, and the operator's tail-latency control
appears to do nothing on the path that carries the traffic.

The QUIC pool has the same shape with no knob at all: `QuicPoolConfig::default()` at
`main.rs:994` and `:1033`, and a **hard-coded 5 s** handshake deadline at
`quic_pool.rs:376` (`let deadline = ... + Duration::from_secs(5)`), plus a hard-coded
100 ms `probe_timeout`.

**Would an existing test catch it?** No. `pool.rs:703-724`
(`acquire_async_timeout_fires`) proves the timeout mechanism works against a
`PoolConfig` the test constructs itself; nothing asserts the binary wires the config value
into it.

---

### IOP-06 — `DnsResolver::refresh_all` destroys a good answer on a failed refresh — the exact inverse of the module's stated invariant

**Severity: MEDIUM · non-blocking (latent: no production caller of `spawn_background_refresh`)**

`crates/lb-io/src/dns.rs:1-5` (the claim) vs `:291-299` (the code)

Module doc:

> "A FAILED background refresh leaves the existing value until natural expiry — otherwise
> a flaky resolver flips a healthy pool into a negative-cache storm."

Code:

```rust
pub async fn refresh_all(&self) {
    let snapshot: Vec<CacheKey> = self.inner.cache.iter().map(|kv| kv.key().clone()).collect();
    for key in snapshot {
        let fresh = Arc::new(CacheEntry::new());
        self.inner.cache.insert(key.clone(), fresh.clone());   // <-- good answer discarded FIRST
        let _ = self.fill_and_return(&fresh, &key.hostname, key.port).await;
    }
}
```

The entry is replaced with an empty cell **before** the resolution is attempted, and
`fill_and_return` writes whatever comes back — including `ResolveResult::Negative`
(`:229-241`), which is then cached for `negative_ttl` (`:245`). The previous good answer
is gone. The `let _ =` discards the outcome, so nothing distinguishes success from failure.

There is a second-order effect from the same line: during the refresh window the cache
holds an uninitialised cell, so any concurrent `resolve()` for that name stops being a
cache hit and blocks on the live `getaddrinfo` instead of returning the cached address.

**Failure scenario.** With the background refresher wired (see IOP-07 — it is `pub`,
`#[must_use]`, and clearly intended to be), one transient `SERVFAIL` from the local
resolver during a 60 s tick turns every backend that name resolves to into
`DnsError::NxDomain` for `negative_ttl` (5 s). Any call that resolves in that window —
`rebuild_l7_proxies` on a SIGHUP, `spawn_tcp` on a listener restart — fails with
`"cannot resolve backend: ..."` and `anyhow::bail`s, i.e. **the reload fails** on a DNS
blip that the module explicitly promises to absorb.

**Would an existing test catch it?** No. `refresh_all_re_queries_entries` (`:456-465`)
asserts only that the resolver fires again; there is no test with a resolver that
succeeds then fails.

**Fix shape:** resolve first into a local, and only `insert` on success; on failure leave
the existing entry to expire naturally, which is what the doc already promises.

---

### IOP-07 — Backend DNS is resolved once at listener spawn / reload, only the FIRST address is used, and there is no background refresh; the docs claim TTL re-resolution

**Severity: MEDIUM · blocking for prod (docs) / non-blocking (behaviour, if documented)**

`crates/lb/src/main.rs:520-531, 1420-1434, 1560-1575`; `crates/lb-io/src/dns.rs:266-288`

All three resolution sites have the same shape:

```rust
let lookup = resolver.resolve(host, port).await
    .with_context(|| format!("cannot resolve backend: {}", b.address))?;
let Some(first) = lookup.first().copied() else {
    anyhow::bail!("resolver returned no addresses for {}", b.address);
};
addresses.push(first);
```

Answering the brief's DNS checklist item by item:

* **When is resolution performed?** At listener spawn and at SIGHUP reload only. Never
  per request (good: no per-request latency or DoS surface), and never in the background:
  `spawn_background_refresh` has **zero production callers** (`rg` over `crates/` finds
  only the definition at `dns.rs:268`). So a backend whose IP changes is never picked up
  while the process runs.
* **A SIGHUP within 300 s does not help.** `rebuild_l7_proxies` goes through the same
  cache; inside `positive_ttl_cap` (`DEFAULT_POSITIVE_TTL_CAP_SECS = 300`, `dns.rs:19`)
  it is a cache hit and returns the **stale** address, bumping `dns_cache_hits_total`.
  An operator reloading to pick up a moved backend must wait out the TTL cap with no
  signal that they are being served a cached answer.
* **TTL honouring:** none — flat `positive_ttl_cap`. Documented honestly in the module
  header (`dns.rs:2-3`).
* **Negative caching:** yes, 5 s (`dns.rs:21`). Short enough; not an outage amplifier.
  Rated correct.
* **Blocking syscall on the runtime:** **no** — `to_socket_addrs` is correctly wrapped in
  `tokio::task::spawn_blocking` (`dns.rs:227`). Rated correct.
* **Happy-eyeballs / IPv6:** none. `docs/research/pingora.md:41` records the exact
  failure this prevents ("DNS IPv6-first on a broken v6 path added 5 s per request") as a
  lesson to apply; it is not applied.
* **Multiple A records:** discarded. `resolve()` returns the full `Vec<SocketAddr>`; all
  three call sites take `.first()`. A DNS-round-robin backend pool collapses to one host.
* **Cache bounds:** unbounded `DashMap` with no eviction (`dns.rs:127`). Bounded in
  practice by the config's backend-name count, so not a live DoS. Folded here as INFO.
* **Resolution timeout:** none. `fill_and_return` has no `tokio::time::timeout` around
  the `spawn_blocking`; a wedged `getaddrinfo` (multiple nameservers × `timeout`×`attempts`
  in `/etc/resolv.conf`, tens of seconds) stalls listener startup or a reload with no
  deadline and no log.

**Doc divergence (this is the blocking half).** `docs/guide/observability.md:94-95` —
"hostname backends re-resolve on TTL, so a modest miss rate there is normal" — and
`docs/guide/RUNBOOK.md:495` — "re-resolve on TTL expiry" — describe behaviour that does
not exist. The `LbDnsCacheMiss` alert (`RUNBOOK.md:488-496`) fires on a miss *ratio* that
in steady state has a zero denominator, because `resolve()` is not called in steady state
at all. `docs/known-limitations.md` has no DNS entry.

---

### IOP-08 — S36 H3 recycle: the GOAWAY carries the id of a stream the gateway then processes, so the boundary request is both served and advertised as retryable

**Severity: MEDIUM · blocking for prod (duplicate non-idempotent request)**

`crates/lb-quic/src/conn_actor.rs:936-956`

```rust
// 2) Count BEFORE validation: every new stream lands in quiche's
//    per-connection `collected` set, so the cap must count rejects too.
*requests_served = requests_served.saturating_add(1);
*goaway_last_id = sid;
// 3) At the cap, flip `goaway_pending` ... and try to emit the GOAWAY now
if *requests_served >= u64::from(cap) {
    *goaway_pending = true;
    try_send_pending_goaway(conn, h3, goaway_pending, goaway_sent, *goaway_last_id, ...);
}
```

Execution then **falls through** to pseudo-header validation and the upstream dial —
stream `sid` is processed normally. The admission gate uses strict `>`
(`if *goaway_pending && sid > *goaway_last_id`, `:930`), consistent with "we process
`sid`", but inconsistent with what the GOAWAY tells the client.

RFC 9114 §5.2: *"Requests or pushes with the indicated identifier **or greater** are
rejected by the sender of the GOAWAY."* Sending `GOAWAY(sid)` therefore tells the client
that stream `sid` was **not** processed and may be safely retried elsewhere.

**Failure scenario.** `runtime.max_requests_per_h3_connection = 1000` (the default,
`lb-config/src/lib.rs:172`). A client's 1000th request on a connection is a
`POST /orders`. Its stream id is `4 × 999 = 3996`. The gateway increments to 1000, sets
`goaway_last_id = 3996`, sends `GOAWAY(3996)`, and **then forwards `POST /orders` to the
backend**. A conformant client sees `3996 >= 3996`, treats the request as rejected, and
retries it on a fresh connection. The backend receives the order **twice**. Rate: one per
`cap` requests per connection.

Everything else about the recycle is correct and I verified it: `requests_served` is a
function-local `u64` (`:206`), so it is per-connection by construction and never reset;
`goaway_pending` / `goaway_sent` are correctly separated (`:201-208`) so admission stops
before the frame is queued; `goaway_last_id` is only ever set from a client-initiated bidi
stream so `send_goaway`'s multiple-of-4 precondition holds; the drain-then-recycle gate
(`:319-337`) waits for `body_tx_by_stream`, `resp_rx_by_stream`, `stream_response` **and**
`ws_tunnels` to empty before closing, so a recycle cannot truncate an in-flight response;
`cap == 0` short-circuits the whole block.

**Would an existing test catch it?** No — a test would have to assert the *value* in the
GOAWAY frame against the set of streams the gateway actually served. Scope overlap: the
H3 front is `rfc-h3`'s file; flagged here because the brief assigned the recycle knob.

**Fix shape:** send `GOAWAY(sid + 4)` (the next client-initiated bidi id) when admitting
the cap-th request, or evaluate the cap *before* assigning `goaway_last_id` and reject
`sid` itself. Either way the admission gate and the advertised id must agree.

---

### IOP-09 — A GOAWAY'd-but-draining upstream H2 connection is handed out as alive; the resulting failure is a 502 with no retry, even where the RFC guarantees the request was not processed

**Severity: MEDIUM · non-blocking (availability; adjacent to a documented item)**

`crates/lb-io/src/http2_pool.rs:77-81, 165-188`

```rust
impl PeerEntry {
    fn is_alive(&self) -> bool {
        !self.sender.is_closed() && !self.driver.is_finished()
    }
}
```

Neither predicate observes a received GOAWAY. hyper's `SendRequest::is_closed()` reports
the dispatcher channel, and the driver task only finishes once h2 has drained every
in-flight stream. Throughout the drain window, `is_alive()` is `true`, the pool hands out
the cached sender, and `send_request` fails with an h2 "connection is going away" error →
`Http2PoolError::Send` → `ProxyErr::UpstreamUnattributable` → `502`.

There is **no retry anywhere in the L7 path** (`rg -i "retry|idempotent"` over
`crates/lb-{l7,io,quic}/src` returns only comments, the `Retry-After` header, and
`HealthFilteredPicker`'s bounded pick loop). The module header states the policy:

> "NO retry on send failure (the caller 502s), so the pool never replays a body it does
> not own"

That policy is right for a mid-body failure. It is over-broad here: RFC 9113 §6.8 says
streams above the GOAWAY's Last-Stream-ID "were not, or will not be, processed", so
retrying them on a fresh connection is safe **even for non-idempotent methods** — no
idempotency gate is required, because the server has guaranteed non-processing.

**Failure scenario.** An Envoy or gRPC backend with `max_connection_duration` /
`MAX_CONNECTION_AGE` set (a very common posture) GOAWAYs each upstream connection on a
timer. Every such event produces at least one 502 — and, because `acquire_sender` hands
the same cached sender to all concurrent requests, N concurrent in-flight requests at that
instant all fail. Nothing about the backend was unhealthy.

**Prior art:** `docs/known-limitations.md` ("A failed send on a pooled H2 connection is
deliberately not counted") and `h2_proxy.rs:2582-2596` both describe the *health
attribution* half of this and name the fix — "a `reused` bit out of the pool ... the same
discriminator a safe upstream retry needs". **ALREADY-KNOWN for the attribution half;**
the *retry* half is named as a follow-up but not tracked as a finding, and the
GOAWAY-specific safety argument is not recorded anywhere. Reported so the retry work is
scoped with the RFC guarantee attached.

---

### IOP-10 — gRPC upstream has no deadline unless the client sends `grpc-timeout`, and its ad-hoc H2 client sets no keep-alive PING

**Severity: MEDIUM · blocking for prod**

`crates/lb-l7/src/grpc_proxy.rs:159-198`

```rust
let mut h2_builder = hyper::client::conn::http2::Builder::new(TokioExecutor::new());
h2_builder.max_header_list_size(self.max_header_list_size);   // the ONLY setting
let (mut sender, conn) = h2_builder.handshake::<_, BoxBody<Bytes, hyper::Error>>(...).await?;
let conn_handle = tokio::spawn(async move { let _ = conn.await; });

let send_fut = sender.send_request(upstream_req);
let upstream_result = if let Some(ms) = deadline_ms {
    /* timeout */
} else {
    send_fut.await          // <-- no deadline at all
};
```

`GrpcProxy` builds its own H2 client per request rather than going through `Http2Pool`, so
it inherits neither the pool's 30 s `send_timeout` nor its keep-alive PING configuration
(`http2_pool.rs:302-307`). `deadline_ms` comes from the client's `grpc-timeout` header;
a client that omits it (permitted by the gRPC spec) gets an unbounded upstream wait.

**Failure scenario.** A gRPC client omits `grpc-timeout` and calls a method whose backend
handler hangs. `send_fut.await` never resolves; the `conn_handle` task, the socket and the
H2 client state are held for the life of the process. There is no keep-alive PING to notice
a silent peer and no wall-clock cap. Same leak shape as IOP-02, different path. The H2
front's `timeouts.total` (60 s) tears down the *downstream* connection, which drops the
response future and hence the sender — but only if the request is still owned by that
future; the detached `conn_handle` is not aborted on that path.

**Would an existing test catch it?** No — the gRPC e2e tests all set a deadline.

**Fix shape:** apply `self.timeouts.head` (or the pool's `send_timeout`) as a floor when
`deadline_ms` is `None`, and set `keep_alive_interval` / `keep_alive_timeout` on the
builder to match `Http2PoolConfig`'s defaults.

---

### IOP-11 — The QUIC upstream pool is IPv4-only: an IPv6 H3 backend cannot be dialled

**Severity: LOW · non-blocking**

`crates/lb-io/src/quic_pool.rs:357-361`

```rust
let socket = UdpSocket::bind(SocketAddr::V4(SocketAddrV4::new(
    Ipv4Addr::new(0, 0, 0, 0),
    0,
)))
.await?;
```

`connect_and_drive` is the single source of the upstream dial for both `dial_new`
(pooled H3) and `dial_dedicated` (Mode B re-origination), and it hard-codes an
IPv4 wildcard bind. `local` is then passed to `quiche::connect(Some(sni), &scid, local,
addr, config)` and to every `RecvInfo`.

**Failure scenario.** A backend configured as `[2001:db8::1]:443` with `protocol = "h3"`
resolves fine (`split_host_port` handles bracket form, `main.rs:716`) and reaches
`connect_and_drive`, where either `quiche::connect` rejects the v4-local/v6-peer pair or
the first `socket.send_to(bytes, info.to)` fails with `EAFNOSUPPORT`/`EINVAL`. Either way
every H3-upstream request to a v6 backend 502s. The same applies to Mode B
(`raw_proxy` via `dial_dedicated`).

**Would an existing test catch it?** No — every QUIC pool test uses `127.0.0.1`.

---

### IOP-12 — `QuicPoolConfig::probe_timeout` default of 100 ms would discard every pooled connection to any backend with RTT > 100 ms

**Severity: LOW · non-blocking (inert today — see IOP-03)**

`crates/lb-io/src/quic_pool.rs:30, 252-257`

```rust
pub const DEFAULT_QUIC_PROBE_TIMEOUT_MS: u64 = 100;
...
let recv = tokio::time::timeout(
    self.inner.config.probe_timeout,
    conn.socket.recv_from(&mut in_buf),
).await;
```

The PING-ACK probe waits at most 100 ms for the peer's ACK, and a miss discards the
connection (`acquire` at `:215` bumps `probe_discards` and loops). It is not
configurable — `main.rs:994` and `:1033` both pass `QuicPoolConfig::default()`.

For any cross-AZ or cross-region backend with RTT above 100 ms this would produce a
100 % probe-discard rate: every reuse attempt burns 100 ms *and* a connection, then falls
through to a full handshake anyway — strictly worse than not pooling. Filed as LOW only
because nothing ever re-parks a connection today, so `probe_liveness` is unreachable;
it becomes a live performance cliff the moment H3 reuse is enabled.

---

### IOP-13 — Relaxed check-then-act on `total` lets `total_max` be overshot by the number of concurrent returners

**Severity: LOW · non-blocking (inert today — see IOP-03)**

`crates/lb-io/src/pool.rs:332-355` and `crates/lb-io/src/quic_pool.rs:519-529`

```rust
if pool.total.load(Ordering::Relaxed) >= pool.config.total_max {
    return;
}
...
queue.push_back(idle);
pool.total.fetch_add(1, Ordering::Relaxed);
```

The bound is read and acted on non-atomically, and the increment happens after the
per-peer lock is taken. K threads returning concurrently can each observe
`total == total_max - 1` and all push, so `total` can exceed `total_max` by up to `K - 1`.

**Ordering audit (the brief asks every `Relaxed` to be justified):**

| Site | Ordering | Verdict |
|---|---|---|
| `pool.rs:82, 106, 146, 332, 349, 355` — `total` | Relaxed | Acceptable *as a counter* (the data is under the per-peer `Mutex`), but load-bearing as a **bound** at `:332` — that read should be a `fetch_update`/CAS or move inside the per-peer lock with a global reservation. |
| `quic_pool.rs:146, 175, 226, 519, 528, 529` — `total` | Relaxed | Same. |
| `quic_pool.rs:187, 193, 215, 284, 286, 326` — `probe_discards` / `fresh_dials` / `id_counter` | Relaxed | **Correct.** Pure statistics and a monotonic id with no happens-before requirement. |
| `idle_send.rs:51, 74` — `upload_complete` | **Acquire** | **Correct and load-bearing.** Pairs with the pump's Release store. |
| `idle_send.rs:56, 78` — `last_progress` | Relaxed | **Correct.** The Release/Acquire pair on `upload_complete` orders the final bump; the contract is stated at `idle_send.rs:29-32` and honoured by the pump at `h1_proxy.rs:918-928`. |
| `dns.rs:269-274` — `refresh_running` | AcqRel/Acquire CAS | **Correct.** |
| `h1_proxy.rs`/`h2_proxy.rs` — `in_flight_bytes` | Relaxed | **Correct.** A test-only gauge. |

**Lock-discipline audit (no defects found):** no `parking_lot` guard and no `DashMap`
guard is held across an `.await` in any of `pool.rs`, `quic_pool.rs`, `http2_pool.rs`,
`dns.rs`. `dns.rs:292-293` snapshots the keys before awaiting, with the hazard named in
the comment. `pool.rs:140-149` scopes the `DashMap` `Ref` to a block before `pop_front`.
`PooledTcp::drop` and `PooledQuic::drop` are fully synchronous, so no `Drop`-with-held-lock
ordering hazard exists. `PeerEntry::drop` runs `driver.abort()` while the `peers` guard is
held (`http2_pool.rs:269, 274`); `JoinHandle::abort` does not re-enter the pool, so this is
sound (it is the *identity* of what gets aborted that is wrong — IOP-01).

---

### IOP-14 — `collect_body_bounded` allocates the entire body before checking the cap

**Severity: LOW · non-blocking (dead API today)**

`crates/lb-io/src/http2_pool.rs:320-334`

```rust
pub async fn collect_body_bounded(body: Incoming, max_body: usize) -> io::Result<Bytes> {
    let collected = body.collect().await...;
    let bytes = collected.to_bytes();
    if bytes.len() > max_body {
        return Err(io::Error::new(io::ErrorKind::InvalidData, ...));
    }
    Ok(bytes)
}
```

The cap is checked *after* the unbounded `collect()`. A backend returning a 10 GiB body
allocates 10 GiB before the function reports "body too large". This directly contradicts
`docs/arch/backpressure.md` ("there is **no such path**").

It has **no callers** (`rg collect_body_bounded` across `crates/` + `tests/` finds only
the definition), so it is not a live bug — but it is a `pub` API in the pool crate with a
doc-comment that reads as if it is safe, sitting next to the code any future
buffer-the-response work would reach for.

**Fix shape:** delete it, or rewrite it as a frame-by-frame loop that aborts the moment
the running total crosses `max_body`.

---

### IOP-15 — `TCP_FASTOPEN_CONNECT` is applied after `connect(2)` and can never take effect

**Severity: INFO · non-blocking (inert: hard-coded `false`, not config-exposed)**

`crates/lb-io/src/sockopts.rs:146-150` and `:188-191`

```rust
if cfg.tcp_fastopen_connect {
    const TCP_FASTOPEN_CONNECT: libc::c_int = 30;
    set_int(fd, libc::IPPROTO_TCP, TCP_FASTOPEN_CONNECT, 1)?;
}
```

Both `apply_connected` and `apply_connected_tokio` are called *after* the connection is
established (`pool.rs:184` and `pool.rs:209`). `TCP_FASTOPEN_CONNECT` must be set on the
socket **before** `connect(2)` to have any effect; setting it afterwards is a no-op for
that connection. `backend_opts()` hard-codes it to `false` (`main.rs:709`) and it is not
reachable from config, so nothing is broken today — but the knob is a lie if anyone flips
it.

---

### IOP-16 — TCP keepalive is enabled with system defaults only; no interval knobs, no `TCP_USER_TIMEOUT`, no `SO_LINGER`

**Severity: INFO · non-blocking**

`crates/lb-io/src/sockopts.rs:129-131, 172-173`; `crates/lb/src/main.rs:702-711`

`sock.set_keepalive(true)` enables `SO_KEEPALIVE` with the kernel defaults (Linux:
`tcp_keepalive_time` 7200 s, `intvl` 75 s, `probes` 9 ≈ 2 h 11 m to detect a dead peer).
`BackendSockOpts` exposes no keepalive time/interval/probe-count fields and no
`TCP_USER_TIMEOUT`.

Answering the brief's checklist: `TCP_NODELAY` is set on backend sockets
(`main.rs:704`); `SO_REUSEADDR`/`SO_REUSEPORT` are listener-side only and correctly
`cfg`-gated (`sockopts.rs:67-73`); **there is no `SO_LINGER` anywhere in the tree**, so
the "wrong linger causes RSTs" failure mode does not exist here. Rated correct.

The keepalive gap matters only once pooling is real: the app-level `probe_alive` covers a
FIN'd peer, and nothing covers a black-holed one inside the 60 s idle window. Related:
when pooling is enabled, `DEFAULT_IDLE_TIMEOUT_SECS = 60` (`pool.rs:25`) is **equal to**,
not shorter than, nginx's and Go's common 60 s upstream idle timeouts — so the gateway
would lose the close race roughly half the time. `probe_alive` catches it, but only
because it is a probe-on-checkout design; the default should still be lowered.

---

### IOP-17 — `DnsResolver` cache is unbounded

**Severity: INFO · non-blocking**

`crates/lb-io/src/dns.rs:127` — `cache: DashMap<CacheKey, Arc<CacheEntry>>` has no size
cap and no eviction; entries are only ever replaced, never removed. Bounded in practice
because the only production callers resolve names taken from the config
(`main.rs:520, 1424, 1564`), so the key space equals the backend-name count. Worth a
bound before anything ever resolves an attacker-influenced name.

---

## 3. Verification of the prior fixes the brief asked about

| Fix | Status | Evidence |
|---|---|---|
| **S14 / CF-BODY-WALLCLOCK — two-phase idle/head deadline** | **Present and correct.** | `crates/lb-io/src/idle_send.rs:33-91`. Phase A re-arms on `last_progress`; Phase B anchors `head_deadline_anchor` once (`:54`, `get_or_insert_with`) so the head cannot slide. `biased;` at `:61` makes success win a tie against a spurious timeout. |
| **`idle_bounded_send` stale-complete re-check** | **Present and correct — not regressed.** | `idle_send.rs:71-76`: after the sleep fires, `upload_complete` is **re-loaded** with `Acquire` before declaring `IdleTimeout`, and a post-deadline `last_progress` bump is re-checked at `:78-86`. Non-vacuity is pinned by `arm_ix_lp_zero_bump_then_complete_fires_head_not_idle` (`:350-389`), whose doc says it fails pre-fix. |
| **Single-sourced idle/head deadline across the bridging cells** | **Holds for the H1 and H2 fronts; the H3 front does not participate.** | One implementation, three consumers: `h1_proxy.rs:1075-1083`, `Http2Pool::send_request_idle` (`http2_pool.rs:196-237`) and `drive_h2_upstream_send` (`h2_proxy.rs:2388-2396`). The H3 front re-derives its own (`H3_RESP_IDLE_TIMEOUT`, `h3_bridge.rs:74`) on the H3→H3 leg and has **none** on the H3→H1 leg — **IOP-02**. |
| **S36 H3 connection recycling / `max_requests_per_h3_connection`** | **Present; counter correct; GOAWAY identifier off by one stream.** | Counter is a function-local `u64` (`conn_actor.rs:206`) → per-connection, never reset. `cap == 0` short-circuits. Drain-then-recycle gate at `:319-337` is correct. Off-by-one at `:938` — **IOP-08**. |
| **`ROUND8-L7-10` take-and-discard H1 upstream** | **Present at every H1 call site.** | See §1.1. The `set_reusable` API and its doc block are pinned by `crates/lb-l7/tests/round8_body_overread.rs:66-79`. |
| **R8 bounded-relay constants vs `docs/arch/backpressure.md`** | **Match exactly. No drift.** | `H1_REQ_CHANNEL_DEPTH = 8` / `H1_REQ_CHUNK_MAX = 8 KiB` (`h1_proxy.rs:54, 56`); `H2_REQ_* = 8 / 8 KiB` (`h2_proxy.rs:45, 49`); `H3_BODY_CHANNEL_DEPTH = 8` (`conn_actor.rs:38`), `H3_BODY_CHUNK_MAX = 8 KiB` (`h3_bridge.rs:30`); `STREAM_RELAY_WINDOW = 256 KiB` (`raw_proxy.rs:405`). The one addition the doc does not mention, `H3_BODY_CHANNEL_CAPACITY = DEPTH + 1` (`conn_actor.rs:57`, the CF-S44 reserved terminal slot), is correctly argued in-source not to relax R8. |
| **Backpressure propagation, both directions** | **Correct on the reviewed paths.** | Request leg: bounded `mpsc` → hyper stops pulling → pump stops polling the inbound body (`h1_proxy.rs:889-901`, `h2_proxy.rs:1250-1262`). Response leg: `stream_h1_response`'s `send!` macro awaits on a depth-8 channel (`h3_bridge.rs:580-584`), so a stalled H3 client stops the upstream socket read; H1/H2 fronts propagate through hyper's body polling. No unbounded buffer found on any reviewed path. |

---

## 4. ALREADY-KNOWN (checked, not re-reported)

| Observation | Reference |
|---|---|
| A failed send on a **pooled** H2 connection is not counted for health; a peer-closed-while-idle connection is indistinguishable from a refusing backend | `docs/known-limitations.md` "Health ejection is passive only"; `crates/lb-l7/src/h2_proxy.rs:2582-2596` |
| `reset_peer` is connection-scoped, so a client abort on one H2 stream can collaterally disrupt concurrent streams to the same backend | `ROUND8-L7-10` broad-eviction philosophy; characterised by `tests/h2h2_md_streaming_verify.rs:1852-1990` |
| H3-terminate / Mode B are single-backend (`select_backend` → `backends[0]`); `build_ws_h3_launcher` uses `backends.first()` | `docs/known-limitations.md` "QUIC H3-terminate and Mode B are single-backend"; `crates/lb/src/main.rs:1147` |
| The H3 **front** listener is neither health-filtered nor health-fed | `docs/known-limitations.md` "Health ejection is passive only" |
| No explicit upstream H2 `RST_STREAM(CANCEL)` on an application read timeout; eviction is the available mitigation | `audit/deferred.md` ROUND8-L7-08, lead-decision `R8-L-002` |
| H1 downstream drops trailers on a streamed response | `docs/known-limitations.md` "gRPC requires an HTTP/2 or HTTP/3 front"; `docs/arch/backpressure.md` |
| `TcpPool::acquire` (blocking) is not for production | `crates/lb-io/src/pool.rs:115-116` (CODE-2-09); confirmed: test-only callers, and `pool.rs:727-747` guards the dial path against `spawn_blocking` regressions |

---

## 5. Ranked summary

| ID | Sev | Blocking | file:line | Claim |
|---|---|---|---|---|
| IOP-01 | HIGH | **blocking** | `crates/lb-io/src/http2_pool.rs:239-275` | `Http2Pool` evicts by address, not entry identity: a racing dialer aborts a live connection's driver and a stale error tears down the *current* connection — cascading spurious 502s on cold start and after every eviction |
| IOP-02 | HIGH | **blocking** | `crates/lb-quic/src/h3_bridge.rs:594` + `crates/lb-quic/src/conn_actor.rs:197,317` | H3-front→H1-backend has no deadline on any await and its task is never aborted; a live-but-silent backend leaks a task + fd permanently, surviving the H3 connection's teardown |
| IOP-03 | MEDIUM | non-blocking | `crates/lb-io/src/pool.rs` (whole), `crates/lb-io/src/quic_pool.rs:2021` ref | `TcpPool` and `QuicUpstreamPool` never reuse: probes/LRU/size knobs are unreachable, one TCP (or full QUIC+TLS) handshake per request, no active-connection cap, and the pool metrics measure the L4 relay instead |
| IOP-04 | MEDIUM | **blocking** | `crates/lb-io/src/http2_pool.rs:298` | `h2_security.max_concurrent_streams` is routed into hyper's `max_concurrent_reset_streams`: no upstream concurrency cap, and an unvalidated front-side knob silently replaces h2's 50-entry reset-stream bound |
| IOP-05 | MEDIUM | **blocking** | `crates/lb-io/src/pool.rs:29,42` + `crates/lb/src/main.rs:2172` | `runtime.connect_timeout_ms` is ignored by every L7 upstream dial — hard-coded 5 s — while the doc comment claims it mirrors the config |
| IOP-06 | MEDIUM | non-blocking | `crates/lb-io/src/dns.rs:291-299` | `refresh_all` replaces the cache entry *before* resolving, so a failed refresh destroys a good answer and installs a negative one — the inverse of the module's stated invariant |
| IOP-07 | MEDIUM | **blocking (docs)** | `crates/lb/src/main.rs:520-531` + `crates/lb-io/src/dns.rs:268` | DNS resolves once at spawn/reload, uses only the first A record, has no background refresh and no resolution timeout; a SIGHUP inside the 300 s TTL cap returns the stale address. `observability.md:95` / `RUNBOOK.md:495` claim TTL re-resolution |
| IOP-08 | MEDIUM | **blocking** | `crates/lb-quic/src/conn_actor.rs:938` | S36 recycle sends `GOAWAY(sid)` and then processes `sid`; RFC 9114 §5.2 makes that stream retryable, so the cap-boundary request can be duplicated at the backend |
| IOP-09 | MEDIUM | non-blocking | `crates/lb-io/src/http2_pool.rs:77-81` | `is_alive()` cannot see a GOAWAY'd-but-draining connection; the resulting 502 is never retried even though RFC 9113 §6.8 guarantees the request was unprocessed |
| IOP-10 | MEDIUM | **blocking** | `crates/lb-l7/src/grpc_proxy.rs:178-189` | gRPC upstream has no deadline without a client `grpc-timeout`, and its ad-hoc H2 client sets no keep-alive PING — same unbounded task+fd leak as IOP-02 |
| IOP-11 | LOW | non-blocking | `crates/lb-io/src/quic_pool.rs:357-361` | QUIC upstream dial binds `0.0.0.0:0`: IPv6 H3 backends (and Mode B v6 peers) cannot be reached |
| IOP-12 | LOW | non-blocking | `crates/lb-io/src/quic_pool.rs:30` | 100 ms PING-ACK probe deadline would discard 100 % of pooled connections to any backend with RTT > 100 ms (inert until reuse is enabled) |
| IOP-13 | LOW | non-blocking | `crates/lb-io/src/pool.rs:332` | Relaxed check-then-act on `total` lets `total_max` overshoot by the concurrent-returner count (inert until reuse is enabled) |
| IOP-14 | LOW | non-blocking | `crates/lb-io/src/http2_pool.rs:321-333` | `collect_body_bounded` allocates the whole body before checking the cap — no callers, but a `pub` footgun that contradicts R8 |
| IOP-15 | INFO | non-blocking | `crates/lb-io/src/sockopts.rs:146-150,188-191` | `TCP_FASTOPEN_CONNECT` is set after `connect(2)`, so it can never take effect (inert: hard-coded `false`) |
| IOP-16 | INFO | non-blocking | `crates/lb-io/src/sockopts.rs:129-131` | Keepalive uses kernel defaults only (≈2 h to detect a dead peer); no `TCP_USER_TIMEOUT`; pool `idle_timeout` (60 s) is equal to, not shorter than, common backend idle timeouts. No `SO_LINGER` anywhere — that hazard is absent |
| IOP-17 | INFO | non-blocking | `crates/lb-io/src/dns.rs:127` | Unbounded DNS cache (no size cap, no eviction); bounded today only by the config's backend-name count |

**Clean areas** (reviewed, nothing to report): lock discipline across `.await` in all four
pool modules; `Send`/`Sync` bounds; `Drop`-with-held-lock ordering; the R8 constants vs
`docs/arch/backpressure.md`; backpressure propagation in both directions on the reviewed
paths; `spawn_blocking` placement for `getaddrinfo`; the singleflight in `DnsResolver::resolve`;
negative-TTL sizing; absence of `SO_LINGER`; the `HealthFilteredPicker` fail-open bound; the
S14 two-phase deadline and its stale-complete re-check.
