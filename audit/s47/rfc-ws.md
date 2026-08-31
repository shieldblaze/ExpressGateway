# S47 — WebSocket RFC conformance review (RFC 6455 / 8441 / 9220)

Branch `review/s47-rfc-security`. Read-and-reason only — no cargo commands were
run (2 vCPU / 7 GB / 11 GB free box).

**Line numbers are against `94eece51`.** The branch advanced under this review
(`01915a77` → `94eece51`); of the files in scope only `crates/lb/src/main.rs`
changed (+144 lines from the S47-SEC-1 TLS key perm-check, shifting everything
below it by 31), and `29199144` bumped h2 `0.4.15` → `0.4.19`. Every citation
below was re-verified against the current tree after that move.

Scope reviewed line-by-line:

- `crates/lb-l7/src/ws_proxy.rs` (761)
- `crates/lb-quic/src/ws_tunnel.rs` (460)
- WS paths in `crates/lb-l7/src/h1_proxy.rs` (`handle_ws_upgrade`,
  `dial_upstream_ws`, `run_h1_ws_splice_task`) and
  `crates/lb-l7/src/h2_proxy.rs` (`handle_ws_extended_connect`)
- `crates/lb-l7/src/security_hooks.rs`, `crates/lb-l7/src/lib.rs`
- Adjacent WS decision points reached from the above:
  `crates/lb-quic/src/conn_actor.rs` (`setup_ws_tunnel`, `pump_ws_tunnels`),
  `crates/lb-quic/src/h3_bridge.rs::validate_request_pseudo_headers`,
  `crates/lb/src/main.rs::build_ws_h3_launcher`

Library versions pinned from `Cargo.lock`: tungstenite 0.29.0,
tokio-tungstenite 0.29.0, hyper 1.10.1, h2 0.4.19.

---

## Summary table

| ID | Sev | Site | Claim |
|---|---|---|---|
| WS-01 | HIGH | `h1_proxy.rs:630`, `h2_proxy.rs:690` | WS upgrade forks BEFORE the SNI↔Host 421 check, the smuggle inspection and the underscore policy — a documented host-confusion control is bypassed |
| WS-02 | HIGH | `h1_proxy.rs:1853` | An established H1 WS session escapes the per-IP conn cap, the inflight semaphore, the gauge and the drain tracker |
| WS-03 | HIGH | `h1_proxy.rs:1946`, `h2_proxy.rs:1012`, `ws_proxy.rs:346` | The upstream handshake is synthesized: `Origin`, `Cookie`, `Authorization`, `X-Forwarded-*`, `Via` and the client `Host` are all dropped |
| WS-04 | MEDIUM | `h1_proxy.rs:1859` | The 101 echoes the CLIENT's first offered subprotocol, not the backend's selection → application desync; and a legal non-echoing backend becomes a 502 |
| WS-05 | MEDIUM | `ws_proxy.rs:172` | The "per-direction read-frame watchdog" (WS-002) is not per-direction; at stock defaults it also makes the documented `1001` idle close unreachable |
| WS-06 | MEDIUM | `h2_proxy.rs:1012` | WS-over-H2 never forwards or echoes `Sec-WebSocket-Protocol` (RFC 8441 §5) |
| WS-07 | MEDIUM | `h2_proxy.rs:690` | Extended CONNECT with `:protocol != websocket` falls through to the ordinary proxy path and is forwarded to a backend |
| WS-08 | LOW | `ws_proxy.rs:103` | `Sec-WebSocket-Version` mismatch draws no `426` + `Sec-WebSocket-Version` (RFC 6455 §4.2.2); H2/H3 do not check the version at all |
| WS-09 | LOW | `ws_proxy.rs:110` | `Sec-WebSocket-Key` accepted as any non-empty string (RFC 6455 §4.2.1 requires base64 → 16 bytes) |
| WS-10 | LOW | `h2_proxy.rs:963` | H2 extended CONNECT does not require `:authority`; currently masked only by an h2-crate quirk |
| WS-11 | LOW | `ws_proxy.rs:174,248,260` | Close teardown drops in-flight frames in the other direction; `biased` can starve backend→client |
| WS-12 | LOW | `ws_tunnel.rs:167` | `poll_read` with `buf.remaining() == 0` overwrites un-drained `leftover` (latent; unreachable via tungstenite) |
| WS-13 | INFO | `h2_proxy.rs:965` | Evidence comment contradicted by h2 0.4.19; the cited test does not establish it |
| WS-14 | INFO | `main.rs:1196` | WS-H3 launcher does not propagate `traceparent`/`tracestate` (H1 and H2 do) — R12 divergence |
| WS-15 | INFO | `ws_proxy.rs:211` | One client Ping produces two Pongs (gateway auto-pong + forwarded backend pong) |

ALREADY-KNOWN (not counted): `ws_autobahn.rs` stub; WS-H2 gate OFF.

---

## WS-01 — HIGH — WebSocket upgrade forks above the SNI↔Host (421) choke point

**Site.** `crates/lb-l7/src/h1_proxy.rs:630-639`:

```rust
if self
    .ws
    .as_ref()
    .is_some_and(|w| w.config().enabled && is_h1_upgrade_request(&req))
{
    return self.handle_ws_upgrade(req, req_trace).await;
}
```

Everything below that `return` is skipped for a WebSocket upgrade:

- `h1_proxy.rs:695` — `self.hooks.inspect_request(&inspect_req, peer.ip())`
  (the `HooksBundle` smuggle detector, `lb-security/src/hooks.rs:63`).
- `h1_proxy.rs:704-721` — **PROTO-2-18, the SNI ↔ `Host` agreement check that
  answers `421 Misdirected Request`.**
- `h1_proxy.rs:724+` — the SEC-2-01 defense-in-depth smuggle site.
- `h1_proxy.rs:645-680` — the ROUND8-L7-05 header-underscore policy.

The H2 sibling has the identical shape: the extended-CONNECT fork returns at
`h2_proxy.rs:696`, while `hooks.inspect_request` is at `h2_proxy.rs:778` and the
SNI ↔ `:authority` 421 check is at `h2_proxy.rs:816-843`.

**Spec / stated control.** `crates/lb-l7/src/sni_authority.rs:1-8` states the
threat and asserts the wiring:

```
//! SNI ↔ `:authority` / `Host` agreement validator (RFC 6066 §3 vs RFC 9113
//! §8.3.1). TLS to `attacker.example` then `Host: victim.example` is a
//! host-confusion primitive one layer below PROTO-2-01 ...
//! The validator IS wired on the hot path (`h1_proxy`, `h2_proxy`, and the
//! binary's TLS-accept site).
```

Refusal is `421` per RFC 9110 §15.5.20. The claim "IS wired on the hot path" is
false for the WebSocket branch of both proxies.

**Production reachability.** The check is not inert: `crates/lb/src/main.rs:3158`
captures the live TLS SNI (`tls_stream.get_ref().1.server_name()`) and threads it
into `serve_connection_with_cancel_sni` for both H1 and H2 (`main.rs:3172`,
`main.rs:3184`), so `expected_sni` is `Some(..)` for every SNI-bearing TLS
connection. `check_sni_authority` (`sni_authority.rs:39-57`) short-circuits only
on `None` SNI or an empty authority.

**Exploit.** Against an `h1s` listener serving `a.example.com`:

```
(TLS ClientHello with SNI = a.example.com)
GET /socket HTTP/1.1
Host: b.internal.example.com
Upgrade: websocket
Connection: Upgrade
Sec-WebSocket-Version: 13
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==
```

The same request without the three WebSocket header fields is answered `421`
(`h1_proxy.rs:719`). With them, it reaches `handle_ws_upgrade`, dials a backend
and is answered `101`. The gateway's own stated host-confusion defence is
skipped by adding `Upgrade: websocket`.

**Not speculative — this exact class was already fixed once here.** The
comment at `h1_proxy.rs:597-600` records it:

```rust
// ROUND8-L7-09 — authority-validation CHOKE POINT. MUST stay the FIRST
// statement: the WS-upgrade fork below reached `pick_info()`
// unvalidated before this was hoisted here.
```

`crate::authority::validate_request` was hoisted above the fork; PROTO-2-18,
SEC-2-01 and ROUND8-L7-05 were not.

**Blast radius of the sub-parts.** The smuggle-inspection bypass is the mildest
part: the WS path forwards no client body and synthesizes a fresh upstream
request, so a CL/TE trick has no downstream HTTP message to poison. The
underscore-policy bypass is likewise inert downstream (no client headers are
forwarded — see WS-03). The load-bearing loss is PROTO-2-18.

**Test coverage.** None. `tests/sni_authority_mismatch.rs`,
`crates/lb-l7/tests/sni_authority_421.rs` and
`crates/lb-l7/tests/h2_authority_host_mismatch.rs` contain no `websocket` /
`upgrade` case (grep returns nothing); no `tests/ws_*.rs` sets an SNI/Host
disagreement. A test would catch this only if written.

---

## WS-02 — HIGH — an established H1 WebSocket session escapes connection accounting

**Site.** `crates/lb-l7/src/h1_proxy.rs:1851-1856`:

```rust
// Upstream handshake succeeded: ONLY NOW arm the upgrade and build `101`.
let upgrade_fut = hyper::upgrade::on(&mut req);
tokio::spawn(tracing::Instrument::instrument(
    run_h1_ws_splice_task(upgrade_fut, backend_ws, ws_proxy),
    req_trace.span.clone(),
));
```

A bare `tokio::spawn` — not `st.tracker.spawn`, and outside the per-connection
task's scope.

**Mechanism.** The per-connection task in `crates/lb/src/main.rs:3077-3080`
holds every admission resource for exactly as long as `serve_connection*`
runs:

```rust
st.tracker.clone().spawn(async move {
    let _permit = _inflight_permit;
    let _conn_permit = _admission_permit;
    let _gauge_guard = inflight_gauge_guard;
    st.active_connections.fetch_add(1, Ordering::Relaxed);
```

hyper's `UpgradeableConnection` resolves `Poll::Ready(Ok(()))` the moment the
upgrade is handed off (hyper 1.10.1 `src/server/conn/http1.rs:558-568`:
`// inner is None, meaning the connection was upgraded, thus it's Poll::Ready(Ok(()))`).
So at the instant the `101` is written, `serve_connection_with_cancel_sni`
returns, the task body ends, and all four are released — while the WS session
runs on indefinitely in the detached splice task.

Released early: the `ConnPermit` from `hooks.admit_connection`
(`main.rs:2980`), the per-listener `inflight` semaphore permit
(`main.rs:3009`), the `accept_inflight` gauge guard, the `active_connections`
counter, and the `TaskTracker` registration used by the drain.

**Caps that are defeated.** `main.rs:2334-2338`:

```rust
let per_ip_cap = config.runtime.as_ref().map_or(1_024, |r| r.per_ip_connection_cap);
let conn_gate = ConnGate::new(max_inflight, per_ip_cap, Vec::new());
```

Both are finite and on by default: `per_ip_connection_cap` = 1 024
(`lb-config/src/lib.rs:367-369`), `max_inflight_connections` = 65 536
(`main.rs:2314-2317`).

**Exploit.** One source IP opens 1 024 TCP connections, upgrades each to
WebSocket, then opens 1 024 more — each upgrade returns its permit at the
`101`. Repeat. Concurrent WebSocket sessions from a single IP are bounded only
by file descriptors. Each session pins:

- a client socket and its tungstenite buffers, and
- a **backend** TCP connection permanently removed from the pool
  (`h1_proxy.rs:1938-1940` uses `pooled.take_stream()`, and
  `lb-io/src/pool.rs:298-301` clears `self.pool` so it is never returned).
  `PoolConfig.total_max` (256) caps only *idle* connections;
  `acquire_async` (`pool.rs:129-138`) dials unconditionally when no idle
  connection is available, so there is no outstanding-connection cap either.

Per-session memory is also large by default: `WsConfig::max_message_size` is
16 MiB (`ws_proxy.rs:53`) and `tungstenite_config` (`ws_proxy.rs:74-86`) sets
`max_frame_size` and `max_write_buffer_size` from the same value, in both
directions on both halves.

**Scope.** H1 only. Over H2 the tunnel is one stream on a connection hyper keeps
serving, so the permit is held; over H3 the tunnel lives inside the QUIC
`conn_actor`, likewise held.

**Related, partially known.** `docs/known-limitations.md:306-337` documents that
drain does not emit per-protocol close signals and that "a long-lived connection
still open when the drain budget elapses is force-closed". That assumes the
connection is *tracked*. The splice task is not in the tracker at all, so the
drain neither waits for nor force-closes it — it dies at process exit. The
accounting/cap half is new.

**Test coverage.** None. `tests/ws_h2_burst.rs::ws_h2_upgrade_relay_close_burst_no_leak`
checks fd/task leaks after clean closes over H2, not H1 permit lifetime.

---

## WS-03 — HIGH — the upstream handshake is synthesized; every client header is dropped

**Site (H1).** `crates/lb-l7/src/h1_proxy.rs:1943-1961`:

```rust
let uri = format!("ws://{backend_addr}{path_and_query}")
    .parse()
    .map_err(|e| ProxyErr::Upstream(format!("upstream uri build failed: {e}")))?;
let mut builder = tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
if let Some(protocols) = forwarded_protocols.as_deref() { ... with_sub_protocol(p) ... }
builder = builder.with_header(
    lb_observability::tracing_propagation::TRACEPARENT_HEADER,
    child_traceparent,
);
```

Same shape at `h2_proxy.rs:1011-1026` (traceparent/tracestate only, not even the
subprotocol — see WS-06) and at `ws_proxy.rs:346-360` (`dial_backend_ws`, used
by the H3 launcher: path + subprotocols only).

**What actually goes on the wire upstream.** tungstenite 0.29.0
`src/client.rs:223-245` builds the request from the URI alone:

```rust
let req = Request::builder()
    .method("GET")
    .header("Host", host)
    .header("Connection", "Upgrade")
    .header("Upgrade", "websocket")
    .header("Sec-WebSocket-Version", "13")
    .header("Sec-WebSocket-Key", generate_key())
    .uri(self)
    .body(())?;
```

plus whatever `with_header` / `with_sub_protocol` added. So the backend receives
`Host: <backend-ip>:<port>` and nothing of the client's request except the path
and (on H1/H3) the subprotocol offer.

Dropped: `Origin`, `Cookie`, `Authorization`, every application header, the
client's `Host`, and — unlike every non-WS request — `X-Forwarded-For`,
`X-Forwarded-Proto`, `X-Forwarded-Host` and `Via`, which the ordinary path adds
at `h1_proxy.rs:777-783`:

```rust
append_xff(headers, peer);
set_xfp(headers, self.is_https);
if let Some(h) = host.as_deref() { set_xfh(headers, h); }
append_via(headers);
```

**Spec.**

- RFC 6455 §10.2: "The |Origin| header field [RFC6454] is used to protect
  against unauthorized cross-origin use of a WebSocket server by scripts using
  the WebSocket API in a web browser." A backend cannot apply that protection to
  a header it never receives.
- RFC 8441 §5 (and therefore RFC 9220): "Origin, Sec-WebSocket-Version,
  Sec-WebSocket-Protocol, and Sec-WebSocket-Extensions are used in the CONNECT
  request and response-header fields as defined in [RFC6455]." Over H2/H3 the
  gateway forwards none of the four.
- RFC 9110 §7.6.3: an intermediary is required to add a `Via` entry to the
  message it forwards. The gateway adds `Via` to every proxied request except
  the WebSocket handshake.

**Concrete failures.**

1. *CSWSH protection is neutralised.* A backend that implements the standard
   "reject browser origins not on my allowlist, allow requests with no `Origin`
   (non-browser client)" policy sees every gateway-proxied handshake as
   originless. `evil.example` can open `new WebSocket("wss://gw.example/socket")`
   from a victim's browser (cookies are, per WS-03, not forwarded either — so the
   session-riding half is blocked here by accident, not by design; a backend
   that authenticates by source IP or by a network-level trust assumption is
   fully exposed).
2. *Authenticated WebSocket deployments do not work at all.* Cookie- or
   `Authorization`-based WS auth is impossible through this gateway; the
   backend sees an anonymous handshake. Either the app breaks visibly, or a
   backend with an anonymous fallback silently downgrades every session to
   unauthenticated.
3. *Per-client attribution is lost.* No `X-Forwarded-For` on WS means backend
   rate-limits, abuse logging and geo/ACL decisions all see the gateway's IP for
   every WebSocket client.
4. *Virtual-hosted backends cannot route.* `Host` is the backend's own
   `ip:port`, so a backend serving several hostnames cannot select one.

**Test coverage.** None. `grep -ri 'sec-websocket-protocol|subprotocol|origin|cookie'`
over `tests/ws_*.rs` and `crates/lb-l7/tests/round8_ws_upgrade_defer.rs` returns
nothing. `ws_h2_conformance.rs::upstream_ws_handshake_carries_child_traceparent`
proves the *one* header that is forwarded.

**Note for the fixer.** `Sec-WebSocket-Extensions` being dropped is the one
deliberate, correct part of this: the client's `permessage-deflate` offer is
neither forwarded nor accepted, so RSV1 never appears on either half and the
documented RFC 7692 DEVIATION holds. A fix must keep extensions excluded while
forwarding the rest.

---

## WS-04 — MEDIUM — the H1 `101` reports the client's own offer as the negotiated subprotocol

**Site.** `crates/lb-l7/src/h1_proxy.rs:1858-1872`:

```rust
// v1 mirrors the first offered sub-protocol verbatim.
let echo_protocol = req
    .headers()
    .get(&WS_PROTOCOL)
    .and_then(|v| v.to_str().ok())
    .and_then(|s| s.split(',').next())
    .map(str::trim)
    .filter(|s| !s.is_empty())
    .and_then(|s| HeaderValue::from_str(s).ok());
```

The backend's actual selection is discarded — `dial_upstream_ws`
(`h1_proxy.rs:1963-1970`) binds the handshake response to `_resp`:

```rust
let (backend_ws, _resp) = tokio_tungstenite::client_async_with_config(...)
```

**Spec.** RFC 6455 §4.2.2 step 4: the server's `/subprotocol/` is "Either a
single value representing the subprotocol the server is ready to use or null …
MUST be derived from the client's handshake, specifically by selecting one of
the values from the |Sec-WebSocket-Protocol| field that the server is willing
to use for this connection". The value the gateway reports is not the value the
server selected; the client and the backend end up on different subprotocols
while frames are relayed verbatim.

**Concrete failure (desync).** Client offers `Sec-WebSocket-Protocol: v2.chat,
v1.chat`. Both are forwarded upstream (`h1_proxy.rs:1947-1953` splits and calls
`with_sub_protocol` per token). Backend supports only `v1.chat` and echoes
`v1.chat`; tungstenite's `verify_response` accepts it (0.29.0
`src/handshake/client.rs:284-290` — the returned value is in the offered list).
The gateway then answers the client `Sec-WebSocket-Protocol: v2.chat`. Client
frames v2 messages, backend parses them as v1.

**Concrete failure (spurious 502).** tungstenite is stricter than RFC 6455 §4.1
item 6, which only requires the client to fail on a subprotocol it did *not*
offer. `src/handshake/client.rs:272-276`:

```rust
if headers.get("Sec-WebSocket-Protocol").is_none() && self.subprotocols.is_some() {
    return Err(Error::Protocol(ProtocolError::SecWebSocketSubProtocolError(
        SubProtocolError::NoSubProtocol,
    )));
}
```

RFC 6455 §4.2.2 step 5.5 makes the response header **optional** ("Optionally, a
|Sec-WebSocket-Protocol| header field"), and step 4 allows `/subprotocol/` to be
null. So a fully conformant backend that ignores the offer returns `101` with no
`Sec-WebSocket-Protocol`, tungstenite errors, `dial_upstream_ws` maps it to
`ProxyErr::Upstream`, and the client gets **502** (`h1_proxy.rs:1817-1822`) for
every WebSocket connection that carries a subprotocol offer.

**Cross-transport.** H3 is correct: `ws_proxy.rs:362-367` reads the
upstream-selected value out of the response and `main.rs:1207-1211` echoes
exactly that in the `200`. H2 does neither (WS-06). H1 is the outlier that
actively misreports.

**Test coverage.** None on any transport.

---

## WS-05 — MEDIUM — the "per-direction read-frame watchdog" is not per-direction

**Site.** `crates/lb-l7/src/ws_proxy.rs:170-186`:

```rust
loop {
    // `idle` is the both-sides-silent envelope; the inner per-direction
    // `read_frame` (WS-002) fires even while the other half produces.
    let step = tokio::time::timeout(idle, async {
        tokio::select! {
            biased;
            c = tokio::time::timeout(read_frame, client_rx.try_next()) => c
                .map_or_else(|_| Direction::ReadFrameTimeout, Direction::ClientToBackend),
            b = tokio::time::timeout(read_frame, backend_rx.try_next()) => b
                .map_or_else(|_| Direction::ReadFrameTimeout, Direction::BackendToClient),
        }
    })
    .await;
```

Both inner `timeout(read_frame, …)` futures are **constructed inside the loop
body**, so any frame in *either* direction completes the `select!`, returns from
the `async` block, and the next iteration builds two fresh timers. Neither
direction's deadline survives an event on the other.

**Stated contract (three places), all contradicted.**

- `ws_proxy.rs:27-29`: "Per-direction read-frame watchdog (WS-002). Distinct
  from [`WsConfig::idle_timeout`], which fires only when BOTH are silent."
- `ws_proxy.rs:45-47`: "idle needs BOTH halves silent, this one ANY."
- `lb-config/src/lib.rs:484-486`: "PER-DIRECTION read-frame watchdog (WS-002);
  `idle_timeout_seconds` needs BOTH sides silent."
- `docs/guide/CONFIG.md:227`: "Per-direction read-frame watchdog."

Actual behaviour: `read_frame` fires only when **both** halves are silent for
`read_frame` — i.e. it is a second, shorter copy of `idle_timeout`, not a
per-direction watchdog.

**Consequence 1 — the documented `1001` idle close is unreachable at stock
defaults.** `idle_timeout` = 60 s (`ws_proxy.rs:53`,
`lb-config/src/lib.rs:516-518`), `read_frame_timeout` = 30 s
(`ws_proxy.rs:29,59`, `lb-config/src/lib.rs:532-534`). Both conditions are now
"nothing in either direction", so the 30 s timer always wins. A plain idle
WebSocket is closed with:

```rust
let frame = CloseFrame {
    code: CloseCode::Policy,                                  // 1008
    reason: Utf8Bytes::from_static("ws read frame timeout"),
};
```
(`ws_proxy.rs:200-209`)

not the `1001 Going Away` documented at `ws_proxy.rs:34` and
`docs/guide/CONFIG.md:224`. Every ordinary idle disconnect is reported to the
client, and logged, as a **policy violation**.

**Consequence 2 — a wedged backend is masked indefinitely.** WS-002's purpose is
to reclaim a half that has gone silent. A client sending one frame every 25 s
(well under the 50-Pings-per-10 s flood cap) resets both timers forever, so a
backend that stopped producing hours ago is never detected. The session is
otherwise unbounded: the splice task is detached, so `HttpTimeouts::total` does
not apply to it (see WS-02).

**The cited proof is vacuous for the per-direction claim.**
`tests/ws_proxy_e2e.rs:485-540` (`ws_read_frame_timeout_closes_with_1008`) uses
`spawn_silent_backend()` **and** a client that sends exactly one frame then goes
quiet — both halves silent. Its own comment claims otherwise:

```rust
// idle_timeout deliberately well above the read-frame
// budget so the path that fires is the per-direction
// watchdog, not the all-silent idle path.
```

It proves only that the shorter of two both-silent timers fires first, and that
the 1008 reason string is right. Correspondingly,
`ws_proxy.rs::close_code_1001_on_idle_timeout` (`ws_proxy.rs:546-560`) has to
invert the shipped defaults (`idle_timeout: 150ms`, `read_frame_timeout: 30s`)
to reach the 1001 path at all — which is itself evidence that the 1001 path is
dead at production defaults.

**A test that would catch it:** a chatty client (one frame every
`read_frame/2`) against a silent backend, asserting a Close within
`~read_frame`. Today it would hang until `idle` — and then never, because the
client keeps resetting `idle` too.

---

## WS-06 — MEDIUM — WS-over-H2 never forwards or echoes `Sec-WebSocket-Protocol`

**Site.** `crates/lb-l7/src/h2_proxy.rs:1011-1026` — the upstream builder for the
extended-CONNECT path carries traceparent and tracestate and nothing else:

```rust
let mut builder =
    tokio_tungstenite::tungstenite::client::ClientRequestBuilder::new(uri);
if let Some(tp) = child_traceparent { builder = builder.with_header(TRACEPARENT_HEADER, tp); }
if let Some(ts) = tracestate     { builder = builder.with_header(TRACESTATE_HEADER, ts); }
```

and the success response (`h2_proxy.rs:1088-1093`) carries no header fields at
all:

```rust
Response::builder()
    .status(StatusCode::OK)
    .body(body)
```

**Spec.** RFC 8441 §5: "Origin, Sec-WebSocket-Version, Sec-WebSocket-Protocol,
and Sec-WebSocket-Extensions are used in the CONNECT request and
response-header fields as defined in [RFC6455]." Subprotocol negotiation is
therefore part of the extended-CONNECT exchange; here it is absent in both
directions.

**Concrete failure.** A client sends `:method=CONNECT`, `:protocol=websocket`,
`sec-websocket-protocol: graphql-transport-ws`. The backend never sees the
offer, so it cannot select; the gateway's `200` names no subprotocol, so the
client sees "no subprotocol negotiated". A backend that *requires* the
subprotocol refuses the handshake and the client gets a 502 instead.

**Cross-transport divergence (R12).** H3 forwards the offer
(`conn_actor.rs:1288-1293` → `ws_proxy.rs:346-360`) and echoes the
upstream-selected value (`main.rs:1207-1211`). H1 forwards it but mis-echoes
(WS-04). H2 does neither.

**Status.** Only reachable with `websocket.h2_extended_connect = true`
(default `false`), so it is not live in a default deployment.

**Test coverage.** None; `tests/ws_h2_e2e.rs` and `tests/ws_h2_conformance.rs`
never send a subprotocol.

---

## WS-07 — MEDIUM — extended CONNECT with an unsupported `:protocol` is proxied to a backend

**Site.** `crates/lb-l7/src/h2_proxy.rs:690-697`:

```rust
if self.h2_extended_connect_enabled
    && self
        .ws
        .as_ref()
        .is_some_and(|w| w.config().enabled && is_h2_extended_connect(&req))
{
    return self.handle_ws_extended_connect(req).await;
}
```

`is_h2_extended_connect` (`ws_proxy.rs:114-122`) returns `false` for any
`:protocol` other than `websocket`. Execution then falls through to the ordinary
proxy path — there is no `Method::CONNECT` guard anywhere in `lb-l7`
(`grep -rn 'Method::CONNECT' crates/lb-l7/src/` matches only `ws_proxy.rs`).

**Spec.** RFC 8441 §4 is explicit that advertising
`SETTINGS_ENABLE_CONNECT_PROTOCOL` invites `:protocol` values the server may not
support, and §5 fixes `websocket` as the value for the WebSocket bootstrap. An
unsupported `:protocol` must be refused, not tunnelled or proxied.

**Concrete failure.** With the gate on, a client sends `:method=CONNECT`,
`:scheme=https`, `:path=/x`, `:protocol=mqtt`. hyper accepts it (h2 0.4.19
`server.rs:1686-1694` only requires that `:protocol` accompany `CONNECT`), the
WS fork declines it, and the request is header-stripped, XFF-decorated and sent
to a backend as an H1 `CONNECT /x HTTP/1.1` — a backend dial and a garbage
request for something that should have been rejected at the edge.

**The H3 sibling is correct** — `conn_actor.rs:1255-1270`:

```rust
if !protocol.eq_ignore_ascii_case("websocket") {
    ... spawn_inline_h3_response(..., 501, "unsupported :protocol");
    return;
}
```

**Adjacent, for `rfc-h2`:** a *classic* CONNECT (no `:protocol`) takes the same
fall-through. That is a general H2 gap rather than a WebSocket one; flagging it
here only because the same missing guard covers both.

**Status.** Gated (`h2_extended_connect` default `false`).

**Test coverage.** None.

---

## WS-08 — LOW — `Sec-WebSocket-Version` mismatch is silently demoted, not `426`

**Site.** `crates/lb-l7/src/ws_proxy.rs:103-109`:

```rust
let version_ok = hdrs
    .get(&SEC_WEBSOCKET_VERSION)
    .and_then(|v| v.to_str().ok())
    .is_some_and(|s| s.trim() == "13");
if !version_ok {
    return false;
}
```

**Spec.** RFC 6455 §4.2.2 (version handling): "the server MUST abort the
WebSocket handshake described in this section and instead send an appropriate
HTTP error code (such as 426 Upgrade Required) and a |Sec-WebSocket-Version|
header field indicating the version(s) the server is capable of understanding."
The gateway is the endpoint that terminates this handshake — it computes
`Sec-WebSocket-Accept` itself (`ws_proxy.rs:389-406`) — so the MUST lands here.

**What happens instead.** `is_h1_upgrade_request` returns `false`, so the request
takes the ordinary proxy path, `Upgrade` and `Connection` are stripped as
hop-by-hop (`h1_proxy.rs:37-46`), and the backend receives a plain `GET`. The
client gets whatever the backend says about a non-WebSocket request — typically
`400` or `404` — never `426`, and never the `Sec-WebSocket-Version: 13` header
that tells it which version to retry with.

**H2/H3 do not check the version at all.** `is_h2_extended_connect`
(`ws_proxy.rs:114-122`) tests only method + `:protocol`;
`h2_proxy.rs::handle_ws_extended_connect` and
`conn_actor.rs::setup_ws_tunnel` never read `sec-websocket-version`. Per
RFC 8441 §5 that field "is used … as defined in [RFC6455]", so a tunnel is
established for a client that sent no version or a wrong one.

**Impact in practice is small** — version 13 is universal — which is why this is
LOW rather than MEDIUM.

**Test coverage.** `ws_proxy.rs::rejects_wrong_version` (`ws_proxy.rs:461-473`)
asserts only that the detector returns `false`; it does not assert a `426`, and
no e2e test drives a wrong-version handshake through the proxy.

---

## WS-09 — LOW — `Sec-WebSocket-Key` is accepted as any non-empty string

**Site.** `crates/lb-l7/src/ws_proxy.rs:110-113`:

```rust
hdrs.get(&SEC_WEBSOCKET_KEY)
    .and_then(|v| v.to_str().ok())
    .is_some_and(|s| !s.trim().is_empty())
```

and `ws_proxy.rs:389-393`:

```rust
pub fn build_handshake_response_headers<B>(
    req: &Request<B>,
) -> Option<Vec<(HeaderName, HeaderValue)>> {
    let key = req.headers().get(&SEC_WEBSOCKET_KEY)?.to_str().ok()?;
    let accept = tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
```

**Spec.** RFC 6455 §4.2.1 item 5, in the list of client-handshake properties the
server verifies: "A |Sec-WebSocket-Key| header field with a base64-encoded (see
Section 4 of [RFC4648]) value that, when decoded, is 16 bytes in length." No
base64 decode and no length check is performed; `derive_accept_key` hashes
whatever string arrived.

Two lesser sub-parts: `HeaderMap::get` returns the first value, so a duplicated
`Sec-WebSocket-Key` is accepted (RFC 6455 §4.1 specifies a single field), and
the same first-value rule applies to `Sec-WebSocket-Version`.

**Impact.** Interop is unaffected — a conformant client hashes its own key
string and gets the same `Sec-WebSocket-Accept`. What is lost is the check's
purpose: proving the requester deliberately spoke WebSocket rather than being a
non-WS client coerced into emitting header-like bytes. LOW.

**Verified correct alongside it.** The accept computation itself is right:
tungstenite 0.29.0 `src/handshake/mod.rs:117-125` uses the RFC 6455 GUID
`258EAFA5-E914-47DA-95CA-C5AB0DC85B11` with SHA-1 + base64, and
`ws_proxy.rs::handshake_response_headers_includes_accept` (`ws_proxy.rs:525-544`)
pins the RFC 6455 §1.3 vector
`dGhlIHNhbXBsZSBub25jZQ==` → `s3pPLMBiTxaQ9kYGzzhZRbK+xOo=`.

**Test coverage.** `ws_proxy.rs::rejects_missing_key` covers absence only.

---

## WS-10 — LOW — H2 extended CONNECT does not require `:authority`

**Site.** `crates/lb-l7/src/h2_proxy.rs:963-985` checks `:path` and `:scheme` and
stops there:

```rust
// RFC 8441 §4 — extended CONNECT MUST carry `:scheme` and `:path`;
// reject BEFORE any dial rather than defaulting `:path` to "/".
```

`crate::authority::validate_request` (run at `h2_proxy.rs:675`) validates the
*syntax* of an authority when one is present — `authority.rs:28-44` iterates
`.filter(|s| !s.is_empty())` — but never requires presence.

**Spec.** RFC 9113 §8.5 requires `:authority` on CONNECT; RFC 8441 §4 alters only
the `:scheme`/`:path` rule ("the `:scheme` and `:path` … MUST also be
included") and leaves `:authority` in force as the tunnel target.

**Currently masked by an h2 implementation detail.** h2 0.4.19
`src/server.rs:1730-1735`:

```rust
// It's not possible to build an `Uri` from a scheme and path. So,
// after validating is was a valid scheme, we just have to drop it
// if there isn't an :authority.
if parts.authority.is_some() {
    parts.scheme = Some(scheme);
}
```

With `:authority` absent, the `:scheme` is dropped from the constructed `Uri`,
so the handler's `req.uri().scheme().is_none()` check fires and the request is
rejected `400` — with the wrong reason ("missing :scheme"), and only because of
a workaround the h2 authors describe as a `Uri`-construction limitation.

**The H3 sibling has the explicit check** —
`crates/lb-quic/src/h3_bridge.rs:327-336`:

```rust
Some("CONNECT") if seen_protocol => {
    if scheme.is_none() {
        Err("h3 websocket extended CONNECT missing :scheme (RFC 8441 §4)")
    } else if !seen_path {
        Err("h3 websocket extended CONNECT missing :path (RFC 8441 §4)")
    } else if !seen_authority {
        Err("h3 websocket extended CONNECT missing :authority (RFC 8441 §4)")
    } else { Ok(()) }
}
```

**Test coverage.** None for the `:authority` case
(`tests/ws_h2_conformance.rs` covers `:path` and `:scheme` only).

---

## WS-11 — LOW — Close teardown discards the other direction; `biased` can starve it

**Site A — frames dropped at Close.** `crates/lb-l7/src/ws_proxy.rs:243-262`:

```rust
match tokio::time::timeout(read_frame, backend_tx.send(msg)).await { ... }
if is_close {
    let _ = client_tx.close().await;
    return Ok(());
}
```

and symmetrically in the `BackendToClient` arm. On the first Close in either
direction the relay returns immediately; the opposite receiver is never polled
again and both `WebSocketStream`s are dropped.

**Concrete failure.** Backend writes `result` then the client independently
sends `Close` (a common RPC-over-WS shutdown race). With `biased` the client
branch is polled first, the Close wins, and the already-transmitted `result`
frame sitting in `backend_rx` is discarded. The client sees a clean close and
never learns a message was lost.

This is not a MUST violation — RFC 6455 §5.5.1 requires only that a Close be
answered with a Close (tungstenite's `do_close`, 0.29.0
`src/protocol/mod.rs:720-740`, queues that echo automatically), and §7.1.1
says the endpoint *SHOULD* wait for the peer. It is bounded, deliberate-looking
data loss, recorded so the lead can rule on it the way the trailer-drop
precedent was ruled.

**Site B — starvation.** `ws_proxy.rs:174` (`biased;`) polls the client branch
first every iteration. A client that always has a complete message ready keeps
that branch `Ready`, so `backend_rx` is never polled, and — because both inner
timers are rebuilt each iteration (WS-05) — the backend direction's watchdog
never fires either. The stall is silent and self-inflicted per connection; no
comment explains why `biased` is needed here.

**Test coverage.** `tests/ws_proxy_e2e.rs::ws_close_code_forwarded` covers the
backend→client Close code only; nothing tests a concurrent
data-plus-Close race or one-way saturation.

---

## WS-12 — LOW — `H3WsTunnel::poll_read` can discard buffered bytes

**Site.** `crates/lb-quic/src/ws_tunnel.rs:145-155` and `:167-193`:

```rust
fn drain_leftover(&mut self, buf: &mut ReadBuf<'_>) -> bool {
    if self.leftover.is_empty() || buf.remaining() == 0 {
        return false;
    }
    ...
}
```

```rust
if self.drain_leftover(buf) {
    return Poll::Ready(Ok(()));
}
match self.reader.poll_recv(cx) {
    Poll::Ready(Some(TunnelInbound::Data(bytes))) => {
        self.leftover = bytes;
```

With a non-empty `leftover` and `buf.remaining() == 0`, `drain_leftover` returns
`false` (the second disjunct), control falls through to `poll_recv`, and
`self.leftover = bytes` **overwrites the undrained tail** — silent loss of WS
frame bytes on an H3 tunnel.

**Reachability.** Not reachable through the current single consumer: the only
reader is tungstenite via `lb_l7::ws_proxy::server_ws`, whose `ReadBuffer` always
presents a non-empty chunk. Reported as a latent contract bug — `AsyncRead`
permits a zero-capacity `ReadBuf`, and a future consumer (or a `read(&mut [])`)
would hit it.

**Fix shape:** guard the `poll_recv` arm on `self.leftover.is_empty()`, or return
`Poll::Ready(Ok(()))` early when `buf.remaining() == 0`.

**Test coverage.** The module's eight unit tests all use non-empty buffers
(`ws_tunnel.rs:270-458`); none exercises `remaining() == 0`.

---

## WS-13 — INFO — stale evidence comment about `:scheme` on extended CONNECT

**Site.** `crates/lb-l7/src/h2_proxy.rs:963-968`:

```rust
// RFC 8441 §4 — extended CONNECT MUST carry `:scheme` and `:path`;
// reject BEFORE any dial rather than defaulting `:path` to "/".
// Measured (tests/ws_h2_conformance.rs): a missing `:scheme` is
// rejected ONLY here — hyper does not require `:scheme` for extended
// CONNECT. The `:path` arm is defense-in-depth (hyper's codec also
// catches it).
```

h2 0.4.19 `src/server.rs:1736-1738` contradicts "hyper does not require
`:scheme` for extended CONNECT":

```rust
} else if !is_connect || has_protocol {
    malformed!("malformed headers: missing scheme");
}
```

With `is_connect == true` and `has_protocol == true` the guard evaluates
`false || true` — the codec *does* reject it. The same conclusion is restated in
`audit/websockets/s27-rfc8441-conformance.md` ("Residual / carried", note 2).

The cited test does not establish the claim either:
`tests/ws_h2_conformance.rs:398-421` accepts a `400` **or** a stream error
**or** a `send_request` error, asserting only that the outcome is neither a
2xx tunnel nor a hang. Since the repo treats comments as an evidence trail, the
claim should be re-measured or narrowed.

---

## WS-14 — INFO — WS-H3 does not propagate W3C trace context

`crates/lb/src/main.rs:1196-1203` calls `dial_backend_ws(stream, backend_addr,
&req.path, req.subprotocols.as_deref(), &ws_cfg)`, and `ws_proxy.rs:335-360` has
no trace-context parameter. H1 (`h1_proxy.rs:1955-1961`) and H2
(`h2_proxy.rs:1015-1025`) both re-emit the child `traceparent`/`tracestate` on
the upstream handshake, and `ws_h2_conformance.rs::upstream_ws_handshake_carries_child_traceparent`
plus `round8_ws_upgrade_defer.rs::upstream_receives_child_traceparent` pin that
behaviour. WS-H3 sessions are therefore untraceable end-to-end — an R12
single-sourcing divergence, not a spec issue.

---

## WS-15 — INFO — one client Ping yields two Pongs

`ws_proxy.rs:1-4` states "`Ping` is forwarded, never answered here: tungstenite
auto-replies on the RECEIVING side." Both happen: tungstenite's server half
queues a Pong to the client (0.29.0 `src/protocol/mod.rs:668-676`,
`self.set_additional(Frame::pong(data.clone()))`) **and** the relay forwards the
Ping to the backend (`ws_proxy.rs:244`), whose Pong is then forwarded back
(`ws_proxy.rs:253-258`). The client receives two Pongs per Ping.

Legal — RFC 6455 §5.5.3 permits unsolicited Pongs — but it doubles control-frame
traffic toward the client and will confuse any client measuring RTT by Pong
count. Bounded by the WS-001 Ping rate limit
(`ws_proxy.rs:211-235`, 50 per 10 s by default).

---

## ALREADY-KNOWN

**`tests/ws_autobahn.rs` is a stub.** ALREADY-KNOWN: PROTO-2-04
(`audit/CROSS-REVIEW-SYNTHESIS-r2.md:71`, "Real Autobahn fuzzingclient CI run"),
D-4 (`audit/round-8/FINAL.md:96,306`, `audit/round-8/regression/deferred-env.md:94`),
re-verified as still-a-stub at `audit/security/round-5-verifies-proto.md:52-56`.
The whole test (40 lines) probes `wstest --help` and prints a message on both
branches; it never runs the suite even when `wstest` is installed. Consequence
for this review: **no RFC 6455 §5 frame-layer conformance evidence exists in
CI** — see the delegation argument under "Verified clean" below, which is a
source-reading argument, not a measurement.

One piece of doc drift worth a one-line fix:
`audit/round-8/regression/deferred-env.md:116` gives the in-tree wrapper as
`cargo test --test ws_autobahn --release -- --ignored`, but the test carries no
`#[ignore]` and contains no `wstest` invocation, so that command runs nothing.

**WS-over-H2 is gated OFF.** ALREADY-KNOWN and deliberate:
`docs/known-limitations.md:130-152`, `docs/features.md:52`,
`audit/websockets/s27-rfc8441-conformance.md`. Not reported as a defect.

I checked the gate for completeness as instructed, and **it is fully enforced**,
in three independent places:

1. `h2_proxy.rs:530-535` — `builder.enable_connect_protocol()` only inside
   `if self.h2_extended_connect_enabled`, so the SETTINGS bit is never sent.
2. `h2_proxy.rs:690-697` — the intercept is gated on the same flag, so a client
   that sends `:protocol` anyway is not tunnelled.
3. h2 0.4.19 `src/proto/streams/recv.rs:246-252` rejects the stream outright:

```rust
if pseudo.protocol.is_some()
    && counts.peer().is_server()
    && !self.is_extended_connect_protocol_enabled
{
    proto_err!(stream: "cannot use :protocol if extended connect protocol is disabled; ...");
    return Err(Error::library_reset(stream.id, Reason::PROTOCOL_ERROR).into());
}
```

`tests/ws_h2_gated_off.rs` proves both halves on the real wire (negotiated-bit
read + a hostile extended CONNECT with a connection-counting backend asserting
zero dials). No gate finding.

**Note for the lead, not a finding:** `Cargo.lock:1221-1224` pins hyper
**1.10.1**. The upstream change recorded in the session memory as unblocking
CF-S27-2 ships in 1.11.0, which is present in the local registry but not in the
lock. The gate's stated rationale therefore still holds on this tree.

---

## Verified clean (recorded so a later round need not redo it)

- **RFC 6455 §5 frame obligations are delegated, and the delegate implements
  them.** The relay is not a byte splice: both halves are `WebSocketStream`s, so
  tungstenite parses and re-frames every message in both directions. Verified in
  tungstenite 0.29.0 `src/protocol/mod.rs`: RSV1/2/3 non-zero → `NonZeroReservedBits`
  (`:641-646`); masked frame from server → `MaskedFrameFromServer` (`:648-651`);
  unmasked frame from client → `UnmaskedFrameFromClient`
  (`src/protocol/frame/mod.rs:223`); control frame fragmented → `FragmentedControlFrame`
  (`:657-659`); control payload > 125 → `ControlFrameTooBig` (`:660-662`);
  reserved control/data opcodes → `UnknownControlFrameType` / `UnknownDataFrameType`
  (`:665-667`, `:692`); continuation without a start → `UnexpectedContinueFrame`,
  new data frame mid-fragment → `ExpectedFragment` (`:683-693`); 1-byte Close
  payload → `InvalidCloseSequence` and non-UTF-8 close reason rejected via
  `Utf8Bytes::try_from` (`src/protocol/frame/frame.rs:281-291`); disallowed close
  codes (1005/1006/1015 and the 1016–2999 reserved range,
  `src/protocol/frame/coding.rs:192-193,253`) normalised to `1002 Protocol` on
  receipt (`src/protocol/mod.rs:723-733`). `WsConfig::tungstenite_config`
  (`ws_proxy.rs:74-86`) starts from `WebSocketConfig::default()` and only changes
  the three size knobs — it never sets `accept_unmasked_frames`.
  **Caveat:** this is a source-reading argument. With `ws_autobahn.rs` a stub,
  nothing measures it.
- **Upstream-before-2xx ordering** is correct on all three transports and is the
  one property with load-bearing tests: H1 `h1_proxy.rs:1798-1848` +
  `crates/lb-l7/tests/round8_ws_upgrade_defer.rs` (502/504/no-smuggle);
  H2 `h2_proxy.rs:1036-1065` + `tests/ws_h2_upgrade_defer.rs`;
  H3 `main.rs:1205-1227` + `conn_actor.rs:1443-1480`, including the
  `TryRecvError::Closed` arm that turns a dropped launcher into a 502 rather
  than a silent 200.
- **RFC 9220 / 8441 pseudo-header inversion on H3** is right:
  `h3_bridge.rs:323-347` requires `:scheme` + `:path` + `:authority` under
  `:protocol`, keeps the opposite rule for classic CONNECT, rejects `:protocol`
  on a non-CONNECT method, and rejects `:protocol` entirely when `ws_enabled`
  is false (`:305`), before any dial.
- **`permessage-deflate` (RFC 7692) is safely non-negotiated.** The client's
  `Sec-WebSocket-Extensions` offer is neither forwarded upstream (the upstream
  request is synthesized) nor echoed in the `101`/`200`, and tungstenite is
  built without compression, so RSV1 can never legitimately appear. Matches the
  documented DEVIATION.
- **`Sec-WebSocket-Accept`**: correct GUID, correct algorithm, RFC 6455 §1.3
  vector pinned by a unit test — see WS-09.
- **`H3WsTunnel` backpressure and close/reset mapping** are sound and
  well-tested (`ws_tunnel.rs:167-208` sticky EOF vs `ConnectionReset`;
  `:211-252` `PollSender::poll_reserve` parking; unit tests at `:268-458`,
  including a load-bearing park-then-resume proof).
