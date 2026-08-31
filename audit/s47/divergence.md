# S47 — Divergence Analysis vs. Production Load Balancers

**Analyst:** divergence · **Date:** 2026-08-31 · **Base:** `review/s47-rfc-security` (from main @ `01915a77`)
**Lens:** compare this implementation against documented, paid-for lessons from Cloudflare Pingora,
Envoy, HAProxy, nginx, Katran and Cilium. A finding is only listed if it is a **correctness,
security or availability defect in something we DO implement**. Feature gaps and everything already
recorded in `docs/research/*.md`, `docs/known-limitations.md` or `SECURITY.md` are excluded by
construction. Read-and-reason only — **no builds were run**; every claim below is sourced to a file
and line, and the two library claims are sourced to the vendored crate under
`~/.cargo/registry/.../hyper-1.11.0`. **Line numbers are as of this read on 2026-08-31**; other
agents are editing `crates/lb/src/main.rs`, `crates/lb-quic/*` and others in parallel on this
branch, so the quoted code — not the number — is the durable anchor.

Ten findings. Section 3 lists the production lessons I checked and **dropped**, with the reason —
those are as load-bearing as the findings.

---

## 1 · Findings table

| ID | Sev | Component | One-line claim | Production lesson |
|----|-----|-----------|----------------|-------------------|
| D-01 | **HIGH** | `lb-io/http2_pool.rs:180,184,220,233` | Any stream-scoped upstream error or our own per-request deadline calls `evict()`, which aborts the shared H2 connection driver and fails **every concurrent request** to that backend | Envoy: a stream failure is stream-scoped; connection-scoped eviction is reserved for connection-level faults |
| D-02 | **HIGH** | `lb-io/http2_pool.rs:239-274` | Cold-start dial race: `replace_entry` aborts a **live** driver, and `evict` removes by key not by connection identity, so a stale request's error tears down a fresh healthy connection | Envoy per-cluster pool: pending requests queue behind **one** in-flight connection attempt |
| D-03 | **MEDIUM** | `lb-l7/h1_proxy.rs:786`, `h2_proxy.rs:710` | No next-backend attempt on a **pre-write dial failure**, although the codebase already owns the "never reached the peer" discriminator | nginx `proxy_next_upstream error timeout` is **on by default**; Pingora `ErrorType::ConnectError` is always retry-safe |
| D-04 | **MEDIUM** | `lb-io/http2_pool.rs:33,63,297` | Upstream H2 stream window pinned to 64 KiB — a 32× downgrade from hyper's own client default — with no adaptive window; loopback-only perf baseline could never see it | BDP lesson; the 64 KiB figure is the **edge/downstream** number, applied here to the backend leg |
| D-05 | **MEDIUM** | `lb/main.rs:552,1469,1609`; `lb-io/dns.rs:268` | Backend hostnames are resolved once and frozen; `spawn_background_refresh` has no production caller; `lookup.first()` throws away every other A-record | nginx's `resolver` trap; Envoy `STRICT_DNS` + `dns_refresh_rate`; HAProxy `resolvers` |
| D-06 | **MEDIUM** | `lb-quic/passthrough.rs:912` | Mode-A Maglev backend ids embed the **config index**, so the minimal-disruption guarantee holds only for tail removals and two nodes with the same backend *set* in a different *order* compute different tables | Katran/Cilium: the consistent-hash ring must be identity-keyed and must agree fleet-wide |
| D-07 | LOW | `lb-io/pool.rs` (whole file) | `TcpPool` never re-parks a connection in the production binary, so `SECURITY.md` rows 15–16 credit an inert defense; and its idle queue is FIFO, the wrong order for the day reuse is enabled | Pingora EC-01 probe; nginx/HAProxy idle caches are **LIFO** |
| D-08 | LOW | `lb-io/http2_pool.rs:48,297` | `Http2PoolConfig::max_concurrent_streams` is wired to hyper's `max_concurrent_reset_streams` — a different knob. There is no upstream stream-concurrency ceiling at all | Envoy circuit breaker `max_requests` per cluster |
| D-09 | LOW | `lb-l7/h1_proxy.rs:457` | hyper auto-answers `Expect: 100-continue` at the H1 front before the backend has seen the request | Envoy `proxy_100_continue`; nginx relays the origin's `100` |
| D-10 | LOW | `lb-health/ejection.rs:472-480` | Ejection expiry has no jitter, while `lb-core/shutdown.rs:200` already jitters the drain — the same codebase applies the pattern in one place and not the other | Thundering herd on recovery |

---

## 2 · Per-finding detail

### D-01 · HIGH · One stream's failure tears down the shared upstream H2 connection

**Our behaviour.** `Http2Pool` caches exactly one `PeerEntry` per backend `SocketAddr`
(`crates/lb-io/src/http2_pool.rs:115`, `peers: Mutex<HashMap<SocketAddr, PeerEntry>>`), and that
entry owns the task that *drives* the connection:

```
crates/lb-io/src/http2_pool.rs:83
impl Drop for PeerEntry {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

crates/lb-io/src/http2_pool.rs:272
    fn evict(&self, addr: SocketAddr) {
        let mut peers = self.inner.peers.lock();
        peers.remove(&addr);
    }
```

`peers.remove(&addr)` drops the `PeerEntry`, which aborts `driver` — the `tokio::spawn(conn.await)`
from `dial_and_handshake` (`http2_pool.rs:313`). Aborting it destroys the H2 connection, so every
outstanding stream on it resolves to `Error::new_canceled()` (hyper answers pending callbacks in
`client/dispatch.rs::Envelope::drop` / `Callback::drop`, so this is a burst of errors, not a hang or
a panic).

`evict()` is called on **four** triggers, at `http2_pool.rs:180,184,220,233`:

```
crates/lb-io/src/http2_pool.rs:174
        let send_fut = sender.send_request(request);
        match tokio::time::timeout(self.inner.config.send_timeout, send_fut).await {
            Ok(Ok(resp)) => Ok(resp),
            Ok(Err(e)) => { self.evict(addr); Err(Http2PoolError::Send(error_chain(&e))) }
            Err(_)     => { self.evict(addr); Err(Http2PoolError::Timeout) }
        }
```

and the same two arms again in `send_request_idle` for `IdleSendError::IdleTimeout` /
`HeadTimeout`. Three of these four are **not** connection-level faults:

- `Http2PoolError::Timeout` from `send_timeout` (30 s, `http2_pool.rs:38`) or from the L7
  idle/head deadlines (`HttpTimeouts` defaults `body = 30 s`, `head = 60 s`,
  `crates/lb-l7/src/h1_proxy.rs:132-141`) — **our** clock ran out, nothing is wrong with the
  connection.
- `Http2PoolError::Send` covers every hyper/h2 error, including a **stream-scoped** `RST_STREAM`
  from the backend for one request.

**The production lesson.** Envoy hit exactly this and split the two scopes: a stream reset resets a
stream, and only connection-level faults (GOAWAY, framing errors, transport loss) drain the
connection. It is also why Envoy abandoned cross-worker pool sharing — the recorded lesson in
`docs/research/envoy.md` is "`Connection: close` from upstream needs cluster-specific pool eviction
— otherwise the same stale connection is reused across workers via pool sharing (which is why Envoy
abandoned pool sharing)". We have gone one step further than the thing Envoy abandoned: not just a
shared pool, but a shared *single connection* whose teardown is triggered by any one participant.

**Why it applies to us given our architecture.** One `Http2Pool` is built per listener
(`build_h2_upstream_pool`, `crates/lb/src/main.rs`) and is shared by every client connection on that
listener via `Arc<Http2Pool>` (`h1_proxy.rs:206`, `h2_proxy.rs:96`). The H2 front admits 256
concurrent streams per client connection (`H2SecurityThresholds::max_concurrent_streams`,
`crates/lb-l7/src/h2_security.rs:47`). So the fan-in onto one upstream connection is unbounded in
practice, and the blast radius of one `evict()` is *every* in-flight request from *every* client on
that listener to that backend.

**Concrete failure scenario.** A backend exposes one slow endpoint — a report generator that takes
70 s to first byte, or an upload that stalls for 31 s. Client A requests it. At `head = 60 s` (or
`body = 30 s`) `idle_bounded_send` fires → `evict(addr)` → `driver.abort()` → the shared connection
dies → clients B…Z (any number, up to the listener's inflight cap) receive `502`. Their failures
are classified `Http2PoolError::Send` → `UpstreamUnattributable`
(`h2_proxy.rs:1669`, `h1_proxy.rs:1241`) → `AttemptOutcome::NotAttempted`
(`crates/lb-health/src/ejection.rs:84-89`), so **nothing is ejected, nothing is counted as a backend
failure, and the metric trail shows only a 502 spike with no attributed cause.**

**Would an existing test catch it?** No, and the closest thing is instructive.
`tests/h2h2_md_streaming_verify.rs:1852 reset_peer_collateral_concurrent_stream_probe` probes this
shape and prints "documented tradeoff, NOT a security defect" — but it drives the **`reset_peer`**
path only (a client `RST_STREAM` mid-body, the F-MD-4 smuggle abort). That trigger genuinely has to
be connection-scoped, and `http2_pool.rs:275-283` explains why. The `evict()` triggers above are a
different code path with a different justification, and the probe never reaches them. Also note the
probe *asserts nothing* about collateral (`if !isolated { eprintln!(...) }`), so it cannot regress
in either direction. Every unit test in `http2_pool.rs` issues exactly one request.

**Reference.** Envoy — stream-scoped vs connection-scoped upstream failure; `docs/research/envoy.md`
"Connection pool per cluster, per-worker, per-protocol" + the pool-sharing lesson.
**Our equivalent.** `crates/lb-io/src/http2_pool.rs:180,184,220,233` (the four `evict` call sites)
and `crates/lb-io/src/http2_pool.rs:83-87` (`Drop for PeerEntry`).

---

### D-02 · HIGH · Cold-start dial race aborts a live connection; `evict` is by key, not by identity

**Our behaviour.** `acquire_sender` releases the map lock across the dial:

```
crates/lb-io/src/http2_pool.rs:239
    async fn acquire_sender(&self, addr: SocketAddr) -> Result<SendRequest<H2ReqBody>, Http2PoolError> {
        if let Some(sender) = self.take_alive_sender(addr) {
            return Ok(sender);
        }
        let (sender, driver) = self.dial_and_handshake(addr).await?;
        let entry = PeerEntry { sender: sender.clone(), driver };
        self.replace_entry(addr, entry);
        Ok(sender)
    }

crates/lb-io/src/http2_pool.rs:267
    fn replace_entry(&self, addr: SocketAddr, entry: PeerEntry) {
        let mut peers = self.inner.peers.lock();
        peers.insert(addr, entry);
    }
```

Two independent defects live here.

**(a) The dial race.** `HashMap::insert` returns the previous value; it is discarded, so it is
dropped, so `Drop for PeerEntry` aborts the previous driver. With N concurrent first requests to a
backend, all N miss the cache, all N dial, and each insert kills the connection the *previous*
winner is already sending on. Up to **N-1 spurious 502s** on every cold start: process start, after
a reload rebuilds the pool, after the backend restarts, and after any `evict()` from D-01.

**(b) Evict-by-key, not by identity.** `evict(addr)` removes whatever entry is under that key, not
the entry the failing request was actually using. A request that started on connection C1, took 30 s
to time out, and whose `evict` lands *after* another request installed a healthy C2, tears down C2.
Combined with (a) this is self-sustaining: one timeout evicts, the next concurrent burst all
re-dials, N-1 are killed by the race, each of those N-1 errors calls `evict()` again on the fresh
connection, and the pool oscillates instead of converging.

**The production lesson.** Envoy's per-cluster connection pool queues pending requests behind a
single in-flight connection attempt rather than letting each request race its own dial, and
teardown is scoped to a specific `ActiveClient` instance, never to the address. Pingora's
`pingora-pool` likewise keys and evicts a specific pooled session, not a peer bucket.

**Concrete failure scenario.** A rolling restart of an `h2` backend. The backend's old process
GOAWAYs; `is_alive()` goes false; the next 200 concurrent in-flight requests all miss the cache and
all dial the new process; ~199 of them are aborted mid-send by a later insert and return 502. All
199 are `UpstreamUnattributable`, so ejection sees nothing. From an operator's seat this is "the
gateway 502s a burst on every backend deploy", with no signal naming the cause.

**Would an existing test catch it?** No. Every `http2_pool.rs` unit test is single-request; the
`reset_peer` collateral probe in `tests/h2h2_md_streaming_verify.rs` deliberately establishes the
pooled connection first (`sleep(400ms)` at line 1911) so the second request *reuses* and never
races the dial.

**Reference.** Envoy connection-pool pending-request queue + instance-scoped teardown
(`source/common/http/conn_pool_base.cc`); Pingora `pingora-pool` session identity.
**Our equivalent.** `crates/lb-io/src/http2_pool.rs:239-274`.

---

### D-03 · MEDIUM · No next-backend attempt after a dial failure that provably never reached the peer

**Our behaviour.** Exactly one backend is picked per request, and any upstream failure becomes a
status code:

```
crates/lb-l7/src/h1_proxy.rs:786
        let Some(backend) = self.picker.pick_info() else {
            return error_response(StatusCode::BAD_GATEWAY, "no backend available");
        };
```

(`h2_proxy.rs:710,897,952` are the same shape.) There is no retry anywhere on the L7 path — the only
loop is `HealthFilteredPicker`'s admission skip (`crates/lb-l7/src/upstream.rs:183-193`), which
skips *ejected* backends but never reacts to a failed attempt. `crates/lb-io/src/http2_pool.rs:2`
states the position explicitly: "NO retry on send failure (the caller 502s)".

**The production lesson.** nginx ships `proxy_next_upstream error timeout` **on by default** — a
connect error against one upstream transparently tries the next one. Pingora's error taxonomy
(`pingora-error::ErrorType`) exists precisely so that `ConnectError` can be retried while a
mid-body write error cannot; `docs/research/pingora.md` records it as "Retry state machine with
idempotency guard" and "Error taxonomy with retry class". Envoy's `retry_on: connect-failure` is the
first thing every production Envoy config sets.

**Why it applies to us.** The blanket "no retries" posture is defensible for the *general* case —
retrying after a partial upstream write duplicates side effects for non-idempotent methods, which is
the thing Envoy's retry budgets and Pingora's idempotency guard exist to contain. But a **pre-write
dial failure is safe to retry for any method, including POST**, because the request never left this
process. And we already own that discriminator: `crates/lb-health/src/ejection.rs:43` defines
`UpstreamErrorClass::Transport` as "Dial or TLS/H2/H3 handshake failure against the peer, **on a
connection we just opened**", and `ejection.rs:64-68` says of the missing pool `reused` bit: "This is
ALSO the discriminator a safe upstream retry needs, so it should be built once and consumed twice."
The reasoning is in the tree; the second consumer was never built.

**Concrete failure scenario.** Three healthy `tcp` backends, round-robin, one taken down for a
deploy. Every request that lands on it gets `ECONNREFUSED` → immediate `502`. Ejection needs 5
consecutive failures *against that backend* (`consecutive_failures = 5`), so ~15 requests must flow
before it is ejected — **~5 client-visible 502s per gateway process per ejection round**, repeated
at each failed half-open probe (30 s, 60 s, 120 s, 240 s, 300 s…). nginx in the same topology serves
all of them. Multiply by the number of gateway instances in the fleet.

**Would an existing test catch it?** No — there is no test that asserts a dial failure against one
backend is served by another. This is a deliberate design choice to re-open, not a bug to patch
blindly: the ask is a **`Transport`-only, pre-write-only, bounded-to-one-extra-attempt** next-backend
try, plus (per Envoy) a budget so the retry cannot amplify a full-pool outage.

**Reference.** nginx `proxy_next_upstream` default `error timeout`; Pingora `ErrorType::ConnectError`;
Envoy `retry_on: connect-failure` + `retry_budget`.
**Our equivalent.** `crates/lb-l7/src/h1_proxy.rs:786`, `crates/lb-l7/src/h2_proxy.rs:710`, and the
unbuilt second consumer described at `crates/lb-health/src/ejection.rs:64-68`.

---

### D-04 · MEDIUM · Upstream H2 stream window pinned to 64 KiB — a 32× downgrade from hyper's client default

**Our behaviour.**

```
crates/lb-io/src/http2_pool.rs:33
/// Default H2 initial stream window (RFC 7540 §6.5.2 initial value).
pub const DEFAULT_H2_INITIAL_STREAM_WINDOW: u32 = 65_535;

crates/lb-io/src/http2_pool.rs:295
        builder
            .initial_stream_window_size(self.inner.config.initial_stream_window)
```

`Http2PoolConfig`'s doc calls these "Pingora-aligned defaults" (`http2_pool.rs:45`). Nothing calls
`adaptive_window(true)`, and `initial_connection_window_size` is never set.

**The measured library defaults** (vendored source, `hyper-1.11.0/src/proto/h2/client.rs:48-50`):

```
const DEFAULT_CONN_WINDOW: u32 = 1024 * 1024 * 5;   // 5mb
const DEFAULT_STREAM_WINDOW: u32 = 1024 * 1024 * 2; // 2mb
```

So we deliberately reduce the upstream **stream** window from 2 MiB to 64 KiB — a 32× cut — while
leaving the **connection** window at hyper's 5 MiB. The stated justification, "RFC 7540 §6.5.2
initial value", is the protocol's *pre-SETTINGS* value, not a recommended operating point; a client
that never raises it is exactly what §6.9.2 warns about. The "Pingora-aligned" claim has no source
in-tree.

**The production lesson.** This is the bandwidth-delay-product lesson every HTTP/2 proxy pays once.
A 64 KiB receive window caps a single stream at `65535 / RTT` bytes per second with no window
auto-tuning: ≈ 6.5 MB/s at 10 ms RTT, ≈ 3.2 MB/s at 20 ms. The 64 KiB figure is the correct
*edge/downstream* number — where the peer is an attacker and the window is a memory bound — and it
is what `H2SecurityThresholds` correctly uses for the client-facing server
(`crates/lb-l7/src/h2_security.rs:36`, paired with a 1 MiB connection window). Copying the
attacker-facing number onto the **backend-facing** leg, where the peer is semi-trusted per
`SECURITY.md`'s own trust boundaries, throttles the datapath without buying a corresponding bound.

**Why it applies to us, and why it has never been seen.** `audit/perf/s39-perf-baseline.md:3` records
the baseline topology: "Box: c6a.2xlarge … **Co-located loopback**". At loopback RTT (~0.05 ms) a
64 KiB window permits >1 GB/s, so the ceiling is structurally invisible to every benchmark and every
CI test we have. This is the archetypal lesson-not-yet-paid-for: the bug is in the code waiting for
the first cross-AZ or cross-region backend.

**Concrete failure scenario.** An `h2` backend one AZ away (≈1 ms) serving 50 MB artifacts. Each
stream is capped near 64 MB/s, so a single download takes ~0.8 s of pure window-stall rather than
line rate; at 20 ms (cross-region origin) the same object takes ~16 s. The operator's response is to
raise concurrency — which lands straight on D-08 (no stream ceiling) and D-01 (one shared connection
whose teardown is triggered by any participant's timeout). The three compound.

**Would an existing test catch it?** No. There is no test that measures upstream throughput, and no
test runs against a non-loopback backend.

**Reference.** hyper's own h2 client defaults (`hyper-1.11.0/src/proto/h2/client.rs:48-49`);
RFC 9113 §6.9.2 on the initial window being a starting point; Envoy's edge-vs-upstream window
guidance (edge windows are a memory bound, upstream windows are a throughput knob).
**Our equivalent.** `crates/lb-io/src/http2_pool.rs:33` (the constant), `:63` (the default), `:295`
(the application). The correct, deliberate downstream counterpart is
`crates/lb-l7/src/h2_security.rs:53-54`.

---

### D-05 · MEDIUM · Backend hostnames are resolved once and frozen; only the first address is kept

**Our behaviour.** `BackendConfig::address` is a `String` and `docs/guide/CONFIG.md:152` documents it
as "`String` (socketaddr or `host:port`) … Resolved via `lb_io::DnsResolver` at startup." All three
resolution sites collapse the answer to one address and store it as a plain `SocketAddr`:

```
crates/lb/src/main.rs:1609
        let Some(first) = lookup.first().copied() else {
            anyhow::bail!("resolver returned no addresses for {}", b.address);
        };
        addresses.push(first);
```

(identically at `main.rs:552` in `rebuild_l7_proxies` and `main.rs:1469` in
`wire_h3_terminate_backends`). Those `SocketAddr`s are baked into `RoundRobinUpstreams`,
`HealthRegistry`, and the pools. `DnsResolver` has a full TTL-aware cache and a
`spawn_background_refresh` (`crates/lb-io/src/dns.rs:268`) — **which has no caller anywhere outside
`dns.rs`**, and which would not help anyway, because the datapath holds a snapshot, not a resolver
handle. Re-resolution happens only on SIGHUP.

Two distinct consequences:

1. **Frozen.** A backend whose IP changes (any container/VM reschedule, ASG replacement, k8s pod
   rotation) is black-holed until an operator sends SIGHUP. The negative-TTL and positive-TTL-cap
   logic in `dns.rs` — and `SECURITY.md` defenses-table row 17, which credits it — govern a cache
   that the datapath consults twice in a process lifetime.
2. **Single address.** `lookup.first()` discards every other A/AAAA record. A hostname backing N
   endpoints (a k8s headless Service, DNS round-robin, a multi-A CNAME) contributes exactly one
   endpoint to the pool, so the gateway silently load-balances across 1 of N and fails over to none
   of them.

**The production lesson.** This is nginx's most re-discovered operational trap: `proxy_pass` to a
name resolves once at config load unless you configure a `resolver` and route through a variable.
Envoy's answer is the `STRICT_DNS` cluster with `dns_refresh_rate` (default 5 s), which re-resolves
on a timer *and* keeps every returned address as a distinct host. HAProxy added `resolvers` sections
and `server ... resolvers mydns` for the same reason.

**Concrete failure scenario.** A listener with a single `[[listeners.backends]]` entry
`address = "api.internal:8080"` resolving to three pods. The gateway uses pod #1 only; pods #2 and #3
are discarded at `lookup.first()`. Pod #1 is rescheduled to a new IP. Every request now fails, and
because the registry tracks exactly one address, `can_eject(1, 0, policy)` is `false` — the absolute
"never eject the last backend" rule (`crates/lb-health/src/ejection.rs:460-468`) — so the ejection is
suppressed, `backend_ejections_suppressed_total` increments, and the listener serves 100 % errors
until a human sends SIGHUP. Two healthy pods sit unused the whole time. Nothing in the process ever
retries DNS.

**Would an existing test catch it?** No. Config tests use literal `127.0.0.1:port`, so the
resolution path is a no-op in every test.

**Prior art, stated honestly.** `audit/reliability/round-1-inventory.md` already recorded the
freeze ("`spawn_background_refresh` is never called from main.rs … backends are resolved exactly once
in `spawn_tcp` and frozen"), listing it under "Critical". It was never fixed, never deferred with a
rationale, and never surfaced in `docs/known-limitations.md`, so an operator reading
`docs/guide/CONFIG.md:152` today is told hostnames are supported with no warning attached. The
`lookup.first()` single-address collapse is new here.

**Reference.** nginx `resolver` / re-resolution trap; Envoy `STRICT_DNS` + `dns_refresh_rate`;
HAProxy `resolvers`.
**Our equivalent.** `crates/lb/src/main.rs:552,1469,1609`; `crates/lb-io/src/dns.rs:268`
(uncalled); `docs/guide/CONFIG.md:152` (the claim made to operators).

---

### D-06 · MEDIUM · Mode-A Maglev keys the hash on the backend's config *position*

**Our behaviour.** The passthrough listener builds its Maglev table from ids that embed the index:

```
crates/lb-quic/src/passthrough.rs:907
        let backends: Vec<Backend> = params
            .backends
            .iter()
            .enumerate()
            .map(|(i, sa)| Backend {
                id: format!("backend-{i}-{sa}"),
```

`Maglev::permutation` hashes exactly that id (`crates/lb-balancer/src/maglev.rs:35-42`), and
`Maglev::populate` fills contested slots by iterating the slice **in order**
(`maglev.rs:60-101`). Both the per-backend permutation and the tie-breaking therefore depend on the
backend's position in `[passthrough].backends`.

**Two failures fall out.**

1. **Minimal disruption is defeated for anything but a tail removal.** Insert a backend at position
   0, or delete one from the middle, and every surviving backend's id changes (`backend-0-…` becomes
   `backend-1-…`), so every permutation changes and the whole table is rebuilt from scratch. The
   entire point of Maglev — that removing 1 of N moves ≈1/N of keys — is lost. Note the unit test
   that would have caught this cannot: `maglev.rs::test_minimal_disruption` builds ids as
   `backend-{i}` and then takes `backends_5.iter().take(4)`, i.e. it only ever removes from the
   **tail**, which is the one edit that leaves the surviving ids stable.
2. **Fleet divergence.** Two gateway nodes given the same backend *set* in a different *order*
   compute completely different tables. `[passthrough].backends` is a `Vec<SocketAddr>` parsed
   straight from TOML (`crates/lb-config/src/lib.rs:722`), so order is whatever the config generator
   emitted — and config templating from an unordered source (a k8s endpoint list, a service-discovery
   query, a set/map iteration) reorders freely between renders.

**The production lesson.** Katran and Cilium both key the consistent-hash ring on stable backend
*identity* and build it from a deterministically ordered set, precisely so that every LB node in an
ECMP group computes the same table. `docs/research/katran.md` is in-tree; the fleet-agreement
property is the reason Maglev is used at all, and it is the property this id scheme breaks.

**Why it applies to us.** Mode-A passthrough is a *non-decrypting* flow router: it Maglev-hashes the
QUIC Connection ID so a flow keeps landing on the same backend. That guarantee only has value across
the fleet — a single node could use any stable mapping. The moment two nodes disagree, an ECMP rehash
(link flap, ECMP member change, a client NAT rebind changing the 5-tuple) delivers the same DCID to a
node with a different table, which forwards it to a backend holding no state for that connection.

**Concrete failure scenario.** Rolling upgrade of a 4-node passthrough fleet with a regenerated
config whose backend list order changed. During the roll, old-order and new-order nodes coexist. Any
QUIC flow whose packets reach a differently-ordered node — routinely, since ECMP rehashes when a
member goes down for the upgrade — is delivered to the wrong backend and dies. `[passthrough]` is
`RestartRequiredChange::PassthroughBlock` (`crates/lb-config/src/reload.rs:108`), so this is
specifically a restart/roll-time hazard.

**Would an existing test catch it?** No. `tests/balancer_maglev.rs` and
`maglev.rs::test_minimal_disruption` both remove from the tail only, and no test builds two tables
from the same set in a different order and compares them. That negative control is the cheapest fix
to verify against.

**Reference.** Katran / Cilium consistent-hash ring keyed on backend identity and built from a
deterministic ordering; `docs/research/katran.md`.
**Our equivalent.** `crates/lb-quic/src/passthrough.rs:912` (the id), and the order-sensitive
population at `crates/lb-balancer/src/maglev.rs:60-101`. Note `lb-l4-xdp`'s `MaglevTable` has the
same shape (`crates/lb-l4-xdp/src/lib.rs:280`) but has no production caller, so it is not part of
this finding.

---

### D-07 · LOW · `TcpPool` never re-parks a connection in the running binary

**Our behaviour.** Every production consumer of `TcpPool` detaches or poisons the connection, so
`PooledTcp::return_to_pool` never runs on a live path:

- `crates/lb-l7/src/h1_proxy.rs:867` — `pooled.take_stream()` ("take-and-discard", ROUND8-L7-10).
- `crates/lb-l7/src/h1_proxy.rs:1940` and `crates/lb/src/main.rs:1194` — WS dials, `take_stream()`.
- `crates/lb-io/src/http2_pool.rs:292` — `take_stream()` to hand the socket to the H2 handshake.
- `crates/lb-quic/src/h3_bridge.rs:1185` — the one path that keeps the wrapper calls
  `pooled.set_reusable(false)` on **every** outcome including the clean one ("since the request
  carries `Connection: close`", `h3_bridge.rs:1123`).

Consequently `TcpPool::idle_count()` is 0 for the life of the process, `probe_alive` never runs
against a real reuse, and `per_peer_max` / `total_max` are never approached.

**Why it matters.** Two in-tree claims are inaccurate as a result:

- `SECURITY.md` defenses table **row 15** ("Upstream stale-connection reuse after peer FIN … non-
  blocking read-zero probe before reuse (Pingora EC-01)") and **row 16** ("Unbounded pool growth …
  `per_peer_max` + `total_max`") both credit live defenses against attacks that cannot occur, because
  there is no reuse to defend. The mitigations are correct code with no live surface.
- `docs/guide/DEPLOYMENT.md:137` sizes `nofile` on the premise that "the pool (`per_peer_max=8`,
  `total_max=256` by default) plus client-side listener accept rate can easily consume tens of
  thousands of file descriptors". The pool contributes zero idle fds; the real fd cost is one fresh
  socket per in-flight request, which is a different number.

**Latent divergence for the day reuse is enabled.** `pop_idle` takes from the front and
`return_to_pool` pushes to the back (`crates/lb-io/src/pool.rs:143,354`) — FIFO, so the *oldest*
idle connection is handed out first. nginx's `ngx_http_upstream_keepalive_module` and HAProxy's idle
lists are LIFO for a specific reason: reusing the most-recently-used socket minimises the race against
the origin's own keepalive timeout and lets the tail age out naturally. Our default `idle_timeout` is
60 s (`pool.rs:25`), which is within a hair of nginx's `keepalive_timeout 75s` origin default — the
worst possible pairing under FIFO. `set_reusable` (`pool.rs:287`) also still has no caller, and its
doc comment already carries the warning: "Pingora shipped the body-length-mismatch upstream-smuggle
twice (0.6.0, 0.8.0) and this call before drop is the fix."

**Would an existing test catch it?** The pool's own unit tests exercise parking and the probe
directly, so they pass; nothing asserts that a production path ever parks. The FIFO/LIFO order is
untested.

**Reference.** Pingora EC-01 half-closed pool connection; nginx `ngx_http_upstream_keepalive_module`
LIFO idle cache; HAProxy `http-reuse` idle-connection list.
**Our equivalent.** `crates/lb-io/src/pool.rs` (whole module, inert in production);
`SECURITY.md` rows 15–16; `docs/guide/DEPLOYMENT.md:137`.

---

### D-08 · LOW · The upstream "max concurrent streams" knob is not the concurrency knob

**Our behaviour.**

```
crates/lb-io/src/http2_pool.rs:48
    /// Concurrent streams per H2 connection, via hyper's `max_concurrent_reset_streams`.
    pub max_concurrent_streams: u32,

crates/lb-io/src/http2_pool.rs:297
            .max_concurrent_reset_streams(self.inner.config.max_concurrent_streams as usize)
```

hyper documents that method as "Sets the maximum number of HTTP2 concurrent **locally reset**
streams" (`hyper-1.11.0/src/client/conn/http2.rs:467-476`) — the RUSTSEC-2024-0003 bookkeeping bound
on how many reset streams h2 tracks, not a limit on active streams. hyper's client `Builder` has no
`max_concurrent_streams` at all; a client's concurrency is bounded only by the *server's* SETTINGS.

Two effects: the field name and doc are wrong (an operator or a future maintainer reading them
believes there is a 256-stream ceiling on the upstream leg — there is not), and setting the reset
bookkeeping to 256 is a loosening relative to h2's default of 10.

**Why it matters.** hyper's dispatch channel for h2 is an **unbounded** mpsc
(`hyper-1.11.0/src/client/dispatch.rs:32`), and the http2 `UnboundedSender` does not poll the giver.
So when the backend's `SETTINGS_MAX_CONCURRENT_STREAMS` is exhausted, our excess requests queue in
memory rather than being refused or spread onto a second connection. They then time out one by one —
and each timeout runs D-01's `evict()`. Envoy's answer is the per-cluster circuit breaker
(`max_requests`, `max_pending_requests`) plus opening additional connections; we have neither.

**Would an existing test catch it?** No — nothing asserts the upstream concurrency ceiling, and
`defaults_match_documented_values` (`http2_pool.rs:342`) asserts the *value* 256, not its meaning.

**Reference.** hyper/h2 `max_concurrent_reset_streams` semantics; Envoy per-cluster circuit breakers.
**Our equivalent.** `crates/lb-io/src/http2_pool.rs:31,48,297`.

---

### D-09 · LOW · The gateway answers `Expect: 100-continue` on the backend's behalf

**Our behaviour.** Nothing anywhere in `lb-l7` or `lb-security` inspects, strips or forwards
`Expect` — `rg -i "100-continue|expect"` across `crates/lb-l7/src` and `crates/lb-h1/src` returns
only unrelated `continue` statements. `Expect` is end-to-end, so it is not in `HOP_BY_HOP`
(`h1_proxy.rs:37-47`) and is forwarded verbatim. Meanwhile hyper's H1 server auto-answers it as soon
as the service polls the body: `hyper-1.11.0/src/proto/h1/conn.rs:410-413`,
`// Write the 100 Continue if not already responded... trace!("automatically sending 100 Continue")`.
The H1 front is `hyper::server::conn::http1::Builder` (`crates/lb-l7/src/h1_proxy.rs:457`), and the
request-body pump starts pulling the inbound body as soon as the upstream request is dispatched
(`h1_proxy.rs:917`), so the gateway's own `100` is emitted concurrently with the upstream request and
**never waits for the origin's answer** — which is precisely what `proxy_100_continue` and nginx's
relay exist to do.

**The production lesson.** Envoy makes this explicit and off-by-default (`proxy_100_continue`):
when disabled Envoy answers `100` itself, when enabled it forwards the `Expect` and relays the
origin's own `100`/`417`. nginx relays the upstream's `100`. Both did the work because the origin is
the party entitled to reject the upload.

**Concrete failure scenario.** A client sends `Expect: 100-continue` with a 60 MB body to an
endpoint whose backend would reject it with `401` or `413` on the headers alone. The gateway's `100`
tells the client to proceed immediately, so the client streams the full 60 MB rather than stopping on
the origin's refusal; the bytes cross the client leg and are pumped upstream while the rejection is
still in flight. The client-visible status is eventually correct; the bandwidth and the upstream
body-pump work are spent regardless. Bounded by the 64 MiB cap, so this is amplification, not an
unbounded vector.

**Would an existing test catch it?** No — no test sends `Expect: 100-continue`.

**Note on fixability.** hyper 1.x exposes no opt-out on `http1::Builder`, so an honest resolution is
probably to document this in `docs/known-limitations.md` alongside the other hyper-imposed bounds
rather than to fix it in our code.

**Reference.** Envoy `proxy_100_continue`; nginx upstream `100` relaying; RFC 9110 §10.1.1.
**Our equivalent.** `crates/lb-l7/src/h1_proxy.rs:457` (the server builder), `h1_proxy.rs:37-47`
(the hop-by-hop list `Expect` correctly is not in).

---

### D-10 · LOW · Ejection expiry has no jitter, in a codebase that already jitters the drain

**Our behaviour.** An ejection deadline is `now + backoff(policy, rounds)` with no randomisation:

```
crates/lb-health/src/ejection.rs:472
fn backoff(policy: EjectionPolicy, rounds: u32) -> Duration {
    let shift = rounds.saturating_sub(1).min(16);
    let window = policy.base_ejection
        .checked_mul(2_u32.saturating_pow(shift))
        .unwrap_or(policy.max_ejection);
    window.min(policy.max_ejection)
}
```

so every backend ejected in the same correlated event re-admits at the same offset (30 s, then 60 s,
120 s…), and every gateway instance that observed the same event re-admits in lockstep with the
others.

**The production lesson.** Thundering herd on recovery. The pattern is already understood in this
tree: `crates/lb-core/src/shutdown.rs:200-204` deliberately sleeps `jitter_millis(spec.jitter_max)`
before the drain cancel, with `drain_jitter_ms` a first-class config key
(`crates/lb-config/src/lib.rs:111`), precisely so a fleet does not drain in unison. The same
reasoning applies to re-admission and was not carried across.

**Honest severity assessment.** This is bounded, which is why it is LOW rather than MEDIUM: the
minimum-healthy floor (`min_healthy_percent = 50`) caps the number of simultaneously-ejected backends
at half the pool, so the synchronized wave is at most half the traffic, and one success clears an
ejection outright. It is worth fixing because the fix is a few lines against machinery that already
exists, not because it is currently hurting anyone.

**Would an existing test catch it?** `backoff_is_capped` (`ejection.rs:598`) asserts exact
deterministic values, so adding jitter would *fail* that test — the test would need to become a range
assertion. Worth flagging to whoever takes the fix.

**Reference.** Thundering-herd-on-recovery; the in-tree precedent is `lb-core`'s drain jitter.
**Our equivalent.** `crates/lb-health/src/ejection.rs:472-480`, versus
`crates/lb-core/src/shutdown.rs:200-204`.

---

## 3 · Lessons checked and DROPPED (with the reason)

Recording these so the next pass does not re-walk them.

| Lesson | Verdict |
|---|---|
| **Envoy retry budget / retry storm amplification** | Moot today — we do not retry at all (`http2_pool.rs:2`, and `h1_proxy.rs:786` is a single pick). A budget becomes required the moment D-03 is acted on, and D-03 says so. |
| **Ephemeral-port / TIME_WAIT exhaustion from no upstream keepalive** | Mitigated by documented ops guidance: `docs/guide/DEPLOYMENT.md:114-131` ships `net.ipv4.tcp_tw_reuse = 1`, `ip_local_port_range = 2000 65500`, `tcp_fin_timeout = 15`. Not a finding. |
| **Katran/Cilium conntrack map-full, per-CPU aggregation, flood gating** | Already matched, and cited in-tree: `LruHashMap` chosen over `HashMap` explicitly to evict rather than `ENOMEM` (`ebpf/src/main.rs:270-278`), Cilium-style RST/FIN state-aware prune (`:127-128, :575-627`), Katran `is_under_flood()` per-CPU new-flow cap (`:264-269`), `PerCpuArray` stats (`:293`). Nothing to add. |
| **Graceful restart / connection-refused window during rebind** | Does not apply: reload never rebinds. A changed listener address is `RestartRequiredChange::ListenerAdded`/`Removed` (`lb-config/src/reload.rs:70-80`) and is refused, not applied. Binary hot-restart via fd handover is already in `docs/known-limitations.md`. |
| **Clock discipline — wall clock in a deadline** | Clean. Every `SystemTime` use in the tree is a test helper, an RNG fallback (`zero_rtt.rs:34`, `key.rs:92`, `router.rs:348`), a trace-id nonce (`trace_ctx.rs:60`), or `CtInsertGate` — which documents the analysis: "the refill math only uses deltas, so a wall-clock step backwards merely yields a safe zero-refill tick" (`lb-l4-xdp/src/loader.rs:1079-1083`). All datapath deadlines use `Instant` / `tokio::time::Instant`. |
| **Head-of-line blocking on the H2/H3 *front*** | Properly isolated. The QUIC actor spawns one task per request and prunes finished handles (`lb-quic/src/conn_actor.rs:1024,1041,1059,317`); hyper's H2 server polls stream futures concurrently; no `.await` is held across a shared lock in any pick or pool path. The real HoL problem is on the **upstream** leg — that is D-01/D-08. |
| **HAProxy/nginx post-desync header normalization** | Covered, and covered better than I expected. Production H1/H2 parsing is hyper (obs-fold, whitespace-before-colon and non-token names are rejected there); `strip_hop_by_hop` collects `Connection`-listed names *before* removing `Connection` (`h1_proxy.rs:2021-2040`); `StrippedRequest` makes the strip a type-level invariant; `authority::validate` cites the exact HAProxy bug titles it mirrors; `validate_h1_request_trailers` blocks the trailer-desync primitive; `append_xff` iterates `get_all` citing Envoy GHSA-ghc4-35x6-crw5. No divergence found. |
| **Envoy `max_ejection_percent` / cluster-wide ejection** | Already implemented and *better* argued than Envoy's default: `min_healthy_percent = 50` plus an absolute never-eject-the-last rule (`ejection.rs:460-468`), with the departure from Envoy's inert-at-N<10 10 % default written out at `ejection.rs:115-127`. `HealthFilteredPicker` also fails open (`upstream.rs:183-193`), which is Envoy's panic mode. Nothing to add. |
| **CVE-2023-44487 — resets must cancel upstream work** | Handled. A client reset propagates as a *constructible* body error (`PumpAbort` / `H1PumpAbort` / `H3ReqAbort`) so hyper aborts the upstream rather than presenting a truncated request as complete (`h2_proxy.rs:1379-1396`, `h1_proxy.rs:58-73`, `h3_bridge.rs:1242`), the reset knobs are applied on the live builder (`h2_security.rs:67-78`), and the two opposite triggers are logged distinctly after S46. |
| **Accept-loop EMFILE spin; unbounded connection spawn** | Both fixed since the round-1 inventory: classified accept errors with exponential backoff (`lb/main.rs:2938-2960`) and a `ConnGate` global + per-IP cap plus an inflight `Semaphore` (`lb/main.rs:2338,1645`). |

---

## 4 · Suggested triage order

D-01 and D-02 are one module and one afternoon: tag `PeerEntry` with a monotonic id, evict by
`(addr, id)`, never abort a driver on insert, and narrow the `evict` triggers to connection-level
faults only (keep `reset_peer` exactly as it is — its connection scope is correct and argued).
That single change removes the largest availability defect found here.

D-04 is a one-line default change plus a decision on `adaptive_window`; it needs a non-loopback
measurement to prove, which no current harness provides.

D-03 and D-06 are design questions for the owner, not patches — D-03 asks whether a
`Transport`-only pre-write next-backend attempt is wanted, D-06 asks whether the Mode-A backend id
should become address-only (and whether the list should be sorted before the table is built, which
is the fleet-agreement half).
