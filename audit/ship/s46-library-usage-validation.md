# S46 — R7 library-usage validation: `Http2Pool` reuse + no-retry

**Scope:** `crates/lb-io/src/http2_pool.rs` reuse pattern and the 502 it produces at
`crates/lb-quic/src/h3_bridge.rs:1336`. Read-only; no cargo invoked.

**Versions under test (Cargo.lock):** hyper **1.10.1**, h2 **0.4.14**, hyper-util **0.1.20**.
All three are vendored at
`/home/ubuntu/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`, so every hyper/hyper-util
citation below is **primary source at the exact locked version**, not documentation.

---

## VERDICT: NEITHER-BUT-A-DESIGN-GAP

hyper behaves correctly and *documents* the race. Our usage is legal, idiomatic, and its liveness
check is **provably identical** to the reference implementation's for h2. What we lack is the
third leg every one of the five references has: a way to tell "the request never reached the
server" apart from "the request failed at the server", and to retry only the former.

This is **not** a hyper bug and **not** a misuse. It is a missing resilience layer.

---

## Q1 — Is `is_closed()` + reuse documented-correct, or a known TOCTOU?

**Both. It is the correct pattern AND an acknowledged TOCTOU — hyper says so in its own docs.**

Your reading of `poll_ready` is **CONFIRMED**, `hyper-1.10.1/src/client/conn/http2.rs:94-103`:

```rust
/// Polls to determine whether this sender can be used yet for a request.
///
/// If the associated connection is closed, this returns an Error.
pub fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<crate::Result<()>> {
    if self.is_closed() {
        Poll::Ready(Err(crate::Error::new_closed()))
    } else {
        Poll::Ready(Ok(()))
    }
}
```

`ready()` (`:108-110`) is just `poll_fn(|cx| self.poll_ready(cx))`. Note `_cx` is **unused** — it
never registers a waker, so it can never actually pend. For h2 it is a pure `is_closed()` test.

**"We forgot `ready()`" is NOT the defect. Say so plainly.** The reference implementation agrees —
`hyper-util-0.1.20/src/client/legacy/client.rs:779-789`:

```rust
fn poll_ready(&mut self, cx: &mut task::Context<'_>) -> Poll<Result<(), Error>> {
    match self.tx {
        PoolTx::Http1(ref mut tx) => tx.poll_ready(cx).map_err(Error::closed),
        PoolTx::Http2(_) => Poll::Ready(Ok(())),
    }
}
```

hyper-util does not even *call* `SendRequest::poll_ready` on the h2 path. It hardcodes ready.

**Our liveness check is equivalent to the reference's.** hyper-util's poolable liveness is
`is_open() = !self.is_poisoned() && self.is_ready()` (`client.rs:854-856`), and for h2
`is_ready()` bottoms out at `dispatch.rs:141-147`:

```rust
pub(crate) fn is_ready(&self) -> bool { !self.giver.is_canceled() }
pub(crate) fn is_closed(&self) -> bool { self.giver.is_canceled() }
```

So on the h2 path `is_ready() == !is_closed()` **exactly**. Our
`PeerEntry::is_alive() = !self.sender.is_closed() && !self.driver.is_finished()`
(`http2_pool.rs:78-80`) is the same test plus a strictly stronger driver-liveness clause. The
only thing hyper-util has that we don't is its `is_poisoned()` flag.

**hyper documents the residual race explicitly**, `http2.rs:112-118`:

```
/// Checks if the connection is currently ready to send a request.
///
/// # Note
///
/// This is mostly a hint. Due to inherent latency of networks, it is
/// possible that even after checking this is ready, sending a request
/// may still fail because the connection was closed in the meantime.
```

The TOCTOU is **unavoidable and known**. No liveness check can close it. Every pooled HTTP client
therefore closes it *downstream* of the check — which is Q2.

---

## Q2 — What do reference implementations do when a request fails on a REUSED connection?

**Yes — "retry once iff the connection was REUSED and the request had not yet been sent" is the
industry-standard behaviour.** Five of five, with the discriminator always being *had the bytes
reached the server*, never *was it slow* or *what status came back*.

### 1. hyper-util 0.1.20 — the reference pooling client for hyper 1.x (STRONGEST citation)

hyper deliberately removed pooling from the core crate, so hyper-util's `legacy::Client` **is**
the reference implementation for exactly our problem. `client.rs:248-271`:

```rust
loop {
    req = match self.try_send_request(req, pool_key.clone()).await {
        Ok(resp) => return Ok(resp),
        Err(TrySendError::Nope(err)) => return Err(err),
        Err(TrySendError::Retryable { mut req, error, connection_reused }) => {
            if !self.config.retry_canceled_requests || !connection_reused {
                // if client disabled, don't retry
                // a fresh connection means we definitely can't retry
                return Err(error);
            }
            trace!("unstarted request canceled, trying again (reason={:?})", error);
            *req.uri_mut() = uri.clone();
            req
        }
    }
}
```

and how `Retryable` is decided, `client.rs:324-341`:

```rust
let mut res = match pooled.try_send_request(req).await {
    Ok(res) => res,
    Err(mut err) => {
        return if let Some(req) = err.take_message() {
            Err(TrySendError::Retryable {
                connection_reused: pooled.is_reused(),
                error: e!(Canceled, err.into_error()) ...,
                req,
            })
        } else {
            Err(TrySendError::Nope(e!(SendRequest, err.into_error()) ...))
        };
    }
};
```

Default is **on** — `retry_canceled_requests: true` (`client.rs:1034`), documented at
`client.rs:1541-1551`:

```
/// Set whether to retry requests that get disrupted before ever starting
/// to write.
///
/// This means a request that is queued, and gets given an idle, reused
/// connection, and then encounters an error immediately as the idle
/// connection was found to be unusable.
```

That paragraph is a description of our exact failure mode.

### 2. nginx — `proxy_next_upstream`

Default `proxy_next_upstream error timeout;`, where `error` is "an error occurred while
establishing a connection with the server, **passing a request to it**, or reading the response
header". Gated for unsafe methods by:

> `non_idempotent`: normally, requests with a non-idempotent method (`POST`, `LOCK`, `PATCH`) are
> not passed to the next server **if a request has been sent to an upstream server** (1.9.13);
> enabling this option explicitly allows retrying such requests

<https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_next_upstream>

### 3. Envoy — `x-envoy-retry-on`

> **reset-before-request** — "Equivalent to *reset* but will only retry requests that have not
> been sent to the upstream server (i.e. the headers have not been sent)."
>
> **refused-stream** — "Envoy will attempt a retry if the upstream server resets the stream with a
> REFUSED_STREAM error code. This reset type indicates that a request is safe to retry."

<https://www.envoyproxy.io/docs/envoy/latest/configuration/http/http_filters/router_filter>

Envoy ships a *dedicated policy value* whose entire purpose is the never-sent discriminator.

### 4. HAProxy — `retry-on` / `option redispatch`

Retryable set includes `conn-failure`, `empty-response`, `junk-response`, `response-timeout`,
`0rtt-rejected`, `all-retryable-errors`; `option redispatch` sends the retry to a *different*
server. HAProxy is the most conservative: it recommends turning L7 retries **off** for POST —

> "it's almost never safe to retry a request that writes data to a database, since you may be
> inserting duplicate data. For that reason, you'll often want to add a rule that disables retries
> for POST requests." — with `http-request disable-l7-retry if METH_POST`

<https://www.haproxy.com/blog/haproxy-layer-7-retries-and-chaos-engineering> ·
<https://www.haproxy.com/documentation/haproxy-configuration-tutorials/reliability/retries/>

### 5. Pingora — `error_while_proxy` / `fail_to_connect`

> "This phase may decide to retry a request **if the connection was re-used and the HTTP method is
> idempotent**."

<https://github.com/cloudflare/pingora/blob/main/docs/user_guide/phase.md>

> "In general, idempotent HTTP requests, e.g., `GET`, are safe to retry. Other requests, e.g.,
> `POST`, are not safe to retry **if the requests have already been sent**."
> "When `fail_to_connect()` is called, pingora-proxy **guarantees that nothing was sent upstream**."

<https://github.com/cloudflare/pingora/blob/main/docs/user_guide/failover.md>

Pingora's framing is the cleanest: the framework's job is to *guarantee* the never-sent property so
the retry decision becomes sound.

---

## Q3 — Is retrying this POST safe, and is it even possible for us?

### The universal rule

Every reference turns on the same predicate: **did the request bytes reach the server?** Not
method safety in isolation — nginx and Pingora both name POST, but *only* to say POST must not be
retried **once sent**. If nothing was sent, no reference treats a POST retry as unsafe, because no
duplicate can exist. hyper-util does not check the method at all — it doesn't need to, because
`take_message()` returning the request *is* proof nothing was sent.

hyper states the invariant directly, `hyper-1.10.1/src/client/dispatch.rs:19-24`:

```
/// An error when calling `try_send_request`.
///
/// There is a possibility of an error occurring on a connection in-between the
/// time that a request is queued and when it is actually written to the IO
/// transport. If that happens, it is safe to return the request back to the
/// caller, as it was never fully sent.
```

### Does our code know whether the request was sent? — **It has the information and throws it away.**

Two APIs exist. We call the wrong one for a pooled client.

`hyper-1.10.1/src/client/conn/http2.rs:150-171` — `send_request`, what we use:

```rust
Err(_req) => {
    debug!("connection was not ready");
    Err(crate::Error::new_canceled().with("connection was not ready"))
}
```

The never-sent request is bound to `_req` and **dropped**.

`hyper-1.10.1/src/client/conn/http2.rs:181-204` — `try_send_request`, what hyper-util uses:

```rust
Err(req) => {
    debug!("connection was not ready");
    let error = crate::Error::new_canceled().with("connection was not ready");
    Err(TrySendError { error, message: Some(req) })
}
```

The request is handed **back**. `http2_pool.rs:157-169` calls `send_request` and then flattens
every failure into `Http2PoolError::Send(e.to_string())` — which **conflates "never sent" with
"failed at the server"**, the one distinction all five references are built around.

### Is the body replayable? — **Yes, in exactly the case where retry is legal.**

`h3_bridge.rs:1274-1288` moves the mpsc receiver into the body:

```rust
Some(ReqBodyEvent::Chunk(b0)) => BoxBody::new(H3ReqStreamBody {
    body_rx,
    first: Some(b0),
    done: false,
}),
```

A `tokio::sync::mpsc::Receiver` is single-consumer and cannot be rewound, so **once the body has
been polled, retry is impossible** — correct, and worth stating.

But that is not the case in question. In the never-sent case hyper returns the whole
`Request<B>` **with the body never polled**: `dispatch.send()` refused it before the h2 task could
touch it. `body_rx` is undrained and `first` still holds `b0`. The request is therefore
byte-for-byte replayable *precisely when* replaying it is safe. The two conditions coincide —
which is why hyper-util needs no separate replay buffer and no idempotency check.

**So retry is feasible here.** The blocker is not the streaming body; it is that
`send_request` destroys the request before we can see it. That reverses the tentative reading in
your prompt, and it matters: the fix is a pool-level API swap, not a body-buffering redesign.

---

## A free, falsifiable prediction for the Phase-1c `RUST_LOG` repro

`hyper::Error`'s `Display` prints **only** `self.description()` (`error.rs:612-616`); the
`.with("connection was not ready")` cause lives in `source()`. Since `http2_pool.rs:163` uses
`e.to_string()`, the cause is dropped and the warn line will read:

```
h2 send_request failed: operation was canceled
```

(`Kind::Canceled => "operation was canceled"`, `error.rs:540`.)

**`Kind::Canceled` is a sound never-sent discriminator on the h2 client path.** `new_canceled()`
has exactly three h2-reachable construction sites, all never-sent: `http2.rs:167` (send_request),
`http2.rs:196` (try_send_request), `dispatch.rs:223` (envelope dropped, "connection closed").
hyper documents it at `error.rs:225-231`: *"This typically happens when a pending request is
dropped before it can be dispatched to the connection, for example because the connection was not
ready."*

Therefore, when the repro fires:

| warn line | Meaning | Implicates the retry gap? |
|---|---|---|
| `h2 send_request failed: operation was canceled` | Request **never sent**; stale pooled connection TOCTOU | **YES** |
| `h2 send_request failed: <anything else>` | Request was sent, upstream genuinely failed | **NO** — the 502 is faithful |
| `upstream dial failed: …` / `h2 handshake failed: …` | Fresh dial failed; nothing was reused | **NO** |
| `h2 send_request timed out` | 30 s elapsed | Already refuted for the 0.232 s CI failure |

This costs nothing to check and settles the attribution in one run.

Minor, separate: `e.to_string()` at `http2_pool.rs:163` discards hyper's cause chain. Using
`{e:#}`-style source-walking, or better `e.is_canceled()`, would name the case outright.

---

## What I would NOT claim

- **NOT** "we forgot `ready()`". Refuted twice over (Q1). Nobody should spend time on it.
- **NOT** that this is a hyper bug. hyper documents the race and ships `try_send_request` as the
  remedy. GENUINELY-UPSTREAM is wrong.
- **NOT** that our liveness check is defective. It is equivalent to hyper-util's for h2, plus an
  extra driver check.
- **NOT** that this explains the CF-S44 502. The mechanism is unproven until the repro names the
  variant. If it is `Dial` or `Handshake`, nothing was reused and this whole finding is irrelevant
  to that failure. Do not let a plausible gap become an attribution — this is the
  [[feedback-symptom-not-attribution]] trap.
- **NOT** that "Envoy/nginx/HAProxy all retry once on a reused-connection failure" — the phrasing
  currently in `audit/ship/s46-report.md:81-82`. That **overstates** three of the five. For a POST:
  nginx will **not** retry once sent unless `non_idempotent` is set; HAProxy recommends
  `disable-l7-retry if METH_POST`; Pingora requires an idempotent method. Only hyper-util retries a
  POST, and only because `take_message()` proves nothing was sent. Suggested rewording:
  *"hyper-util, nginx, Envoy, HAProxy and Pingora all distinguish 'never sent' from 'failed at the
  server' and retry only the former; this pool does not make that distinction at all."*
- **NOT** that a general retry is safe. A retry after the body has been polled is both impossible
  (mpsc is unrewindable) and unsafe (bytes may have reached the backend).

## If a fix is wanted

Smallest correct change, entirely inside `http2_pool.rs`, no caller change:

1. Track reuse — `acquire_sender` already knows (`take_alive_sender` hit vs `dial_and_handshake`);
   return the flag.
2. Call `sender.try_send_request(req)` instead of `send_request`.
3. On `Err(e)` where `e.take_message()` is `Some(req)` **and** the connection was reused: evict,
   dial fresh, send **once** more. Otherwise behave exactly as today.

That is hyper-util's algorithm applied to our pool, and it inherits its safety argument. It is
also strictly a no-op on a fresh connection, so it cannot mask a genuine upstream failure.
