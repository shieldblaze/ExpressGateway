# S47 — DoS / Resource-Exhaustion Red-Team Findings

**Auditor:** rt-dos · **Base:** `review/s47-rfc-security` (main @ `01915a77`) · **Date:** 2026-08-31
**Method:** static trace only. **No cargo command was run** (2 vCPU / 7 GB / 11 GB free — the lead
verifies in CI). Every finding below is traced source→sink by hand with `file:line`; none is
PoC-executed. Where a PoC is required to move a finding from *traced* to *proven*, the exact PoC is
named.

**Prior art read before starting:** `docs/research/dos-catalog.md`, `docs/known-limitations.md`,
`SECURITY.md`, `audit/deferred.md`, `audit/security/s38-findings-resource.md` (+ the other three
S38 role files), `audit/soak/`, `audit/perf/`. Items already covered there are listed under
**ALREADY-KNOWN** at the end and are NOT re-reported.

---

## Summary table

| ID | Sev | Location | Claim | Amplification |
|----|-----|----------|-------|---------------|
| RT-DOS-01 | **HIGH** | `crates/lb-quic/src/passthrough.rs:775-782` | Unauthenticated, source-spoofable short-header packet forces a full O(N) flow-table scan + alloc + sort on the single-threaded recv task | ~25-byte spoofed UDP datagram → O(N) DashMap iteration, N ≤ 200 000 |
| RT-DOS-02 | **HIGH** | `crates/lb-l7/src/h2_proxy.rs:2404,2424,2449,2464,2468` → `crates/lb-io/src/http2_pool.rs:282,84` | One client's mid-body abort tears down the **shared** pooled upstream H2 connection, aborting every other client's in-flight streams to that backend | 1 RST_STREAM (13 B) → whole upstream H2 conn killed + N concurrent requests 502'd + fresh TCP/TLS handshake |
| RT-DOS-03 | **MEDIUM** | `crates/lb-l7/src/h2_proxy.rs:317,361` + `crates/lb/src/main.rs:1237-1259` | The consolidated H2 glitches abuse counter has **no production caller**; the only per-connection protocol-abuse drain is inert in the shipped binary | 1 H2 connection → unlimited abuse requests for the full 60 s `total`, never GOAWAY'd |
| RT-DOS-04 | **MEDIUM** | `crates/lb-quic/src/router.rs:159-163` + `crates/lb-quic/src/passthrough.rs:571-585` | No RFC 9000 §14.1 1200-byte minimum on inbound Initials before minting + sending a Retry → spoofed-source UDP reflector | ~29-byte spoofed Initial → ~110-byte Retry at a victim (**≈3.8×**) + 1 HMAC-SHA256 + 1 AES-GCM tag per packet |
| RT-DOS-05 | **MEDIUM** | `crates/lb-l7/src/h1_proxy.rs:741,761`, `h2_proxy.rs:794,806,860`, `crates/lb-quic/src/conn_actor.rs:961` + `crates/lb-observability/src/log.rs:73-84` | Unthrottled per-request WARN logging on attacker-chosen rejection paths, into a synchronous globally-mutexed JSON stdout writer | ~80-byte request → ~300-byte JSON line + process-wide stdout lock; blocks a tokio worker if stdout backpressures |
| RT-DOS-06 | **MEDIUM** | `crates/lb-quic/src/conn_actor.rs:1147-1163` | H3 `Event::Reset` does not tear down the response side — the spawned upstream producer task is not aborted and its receiver is not dropped | 1 RESET_STREAM frame → the whole upstream fetch runs to completion (orphaned work, CVE-2023-44487 shape) |
| RT-DOS-07 | LOW | `crates/lb-quic/src/passthrough.rs:375-389, 400-416, 620-627` | At the flow cap, each admitted flow costs **two** full O(N) table scans on the recv task | 1 Retry-validated Initial → 2 × O(N), N ≤ 200 000 |
| RT-DOS-08 | LOW | `crates/lb-security/src/conn_gate.rs:140` | Per-IP concurrent-connection cap keys on the full `IpAddr`; a single IPv6 /64 bypasses it entirely | 1 host with a /64 → 2⁶⁴ distinct "IPs" → the per-IP cap becomes the listener cap |
| RT-DOS-09 | LOW | `crates/lb-quic/src/h3_bridge.rs:583-605, 940-942` | `find_header_sep` rescans the whole accumulated head each read → O(n²) in read count | 64 KiB dribbled 1 B/read → ≈2.1 × 10⁹ byte compares (backend-triggered, semi-trusted) |
| RT-DOS-10 | INFO | `tests/security_*.rs` (6 files), `SECURITY.md` table, `docs/research/dos-catalog.md`, `.github/workflows/ci.yml:391-418` | Six named "security" tests and the CI **Chaos Attack Suite** gate exercise detector types with **zero production callers**; the slowloris leg of that gate matches no test at all | n/a — gate integrity |
| RT-DOS-11 | INFO | `crates/lb-observability/src/label_budget.rs:154-263` | `EnforcedLabelBudget` — the per-emission cardinality guard — has zero call sites | n/a — no live cardinality vector today (`route` is hardcoded `""`) |
| RT-DOS-12 | INFO | `crates/lb-io/src/http2_pool.rs:321-334` | `collect_body_bounded` is accumulate-then-check; zero callers today | n/a — a trap, not a live bug |
| RT-DOS-13 | INFO | `.github/workflows/ci.yml:75-94`, `scripts/halting-gate.sh:18-30` | Panic-gate gap analysis: what the deny-set + CI actually catch, and the five classes nothing catches | n/a — gate coverage |

---

## RT-DOS-01 · HIGH · O(N)-per-packet flow-table scan on unauthenticated, spoofable input (CWE-407)

- **CVSS:** `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H` (7.5)
- **Location:** `crates/lb-quic/src/passthrough.rs:761-800`, hot loop at `:775-782`
- **Class:** Algorithmic complexity / uncontrolled resource consumption
- **Applies to:** deployments with a `[passthrough]` (Mode A QUIC) listener configured. Not the
  default profile — say so in any advisory.

### Code

```rust
// crates/lb-quic/src/passthrough.rs:761
async fn forward_short(ctx: &RouterCtx, pkt: &[u8], default_dcid: &[u8], from: SocketAddr) {
    if let Some(entry) = ctx.table.get(default_dcid) {   // fast path: O(1) hit
        ...
        return;
    }

    // Multi-length fallback: collect distinct known short_dcid_lens.
    let mut lens: Vec<usize> = ctx
        .table
        .iter()                                           // <-- :777  FULL TABLE WALK
        .map(|kv| kv.value().short_dcid_len.load(Ordering::Relaxed))
        .filter(|&l| l > 0 && l <= MAX_CID_LEN && l != default_dcid.len())
        .collect();                                       // <-- heap alloc, per packet
    lens.sort_unstable();                                 // <-- :781
    lens.dedup();                                         // <-- :782
```

### Data flow

`UdpSocket::recv` → `PassthroughListener::spawn`'s `on_packet` closure
(`passthrough.rs:940-947`) → **`handle_inbound(...).await` — awaited INLINE on the single recv
task**, not spawned → `parse_public_header` → `PublicHeader::Short { dcid }`
(`passthrough.rs:521-525`) → `forward_short`.

Nothing between the socket and line 777 authenticates anything:

- The Retry-token gate (`handle_initial`, `passthrough.rs:566-598`) is on the **Initial** branch
  only. Short-header packets never reach it.
- `forward_short_via`'s `strict_source_binding` check runs only **after** a table hit
  (`:765`, `:791`) — a miss never reaches it.
- `min_client_dcid_len` (`:544`) is likewise Initial-only.

So the *trigger* is a first byte with the high bit clear plus `max_dcid_len_routed` (default **20**,
`lb-config/src/lib.rs:776`) bytes of arbitrary DCID. **The source address is irrelevant and can be
spoofed** — no handshake, no token, no reply required.

### Attacker cost vs. gateway cost

| | Attacker | Gateway |
|---|---|---|
| per packet | 1 UDP datagram, ~25 bytes, source spoofable | full `DashMap::iter()` over N entries (shard locks + atomic load each), one `Vec<usize>` alloc, `sort_unstable`, `dedup` |
| N | — | up to `2 × max_quic_connections` = **200 000** (`passthrough.rs:601`, cap default 100 000) |

At N = 200 000 that is ~200 000 shard-guard acquisitions + atomic loads for a 25-byte packet:
roughly a **10⁴–10⁵× work amplification per byte**. Because `handle_inbound` is `await`ed inline in
the *single* recv-loop task (`passthrough.rs:938-950`), the scan directly gates packet reception —
while it runs, no legitimate flow's packets are read, the socket receive buffer fills, and real
QUIC traffic is dropped. This is full availability loss of the passthrough datapath from a
one-way, spoofed packet spray.

Note the fallback is usually *futile* as well as expensive: the gateway mints its own
`LB_SCID_LEN = 16` SCIDs (`passthrough.rs:32`), so most flows share one `short_dcid_len` and the
`l != default_dcid.len()` filter frequently empties `lens` — the O(N) walk happens anyway.

### Why S38 missed it

`audit/security/s38-findings-resource.md` F-RES-3 evaluated Mode A's **Initial** flood and correctly
credited the Retry-token address validation as the defense ("an off-path spoofed-source attacker
cannot fill the table — they must complete the RETRY round-trip"). The short-header branch is not
Retry-gated and was not examined. The finding is about *CPU per packet*, not table growth, so the
`max_quic_connections` cap does not bound it — it **scales** it.

### Existing test coverage

None. `passthrough.rs:1600 forward_short_multi_length_fallback_hits` drives the fallback with a
2-entry table and asserts a *hit*; it measures no cost and uses no realistic N. `crates/lb-soak/`
has no short-header-miss scenario.

### PoC the lead should run (non-destructive, local)

Bind a Mode A listener; populate the table to N ∈ {1 000, 50 000} via the existing
`passthrough.rs` cap fixtures; then send k = 10 000 datagrams of `[0x40, <20 random bytes>]` from a
loopback socket. Measure recv-task CPU and the drop rate of a concurrent legitimate flow. Expect
per-packet cost to scale linearly in N — that linearity *is* the finding.

---

## RT-DOS-02 · HIGH · One client's abort tears down the shared upstream H2 connection (CWE-404)

- **CVSS:** `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:H` (7.5). Scope kept **U** deliberately —
  the collateral stays inside the gateway's own authority — but note the damage lands on *other*
  clients' in-flight requests, not the attacker's.
- **Location:** `crates/lb-l7/src/h2_proxy.rs:2404, 2424, 2449, 2464, 2468` →
  `crates/lb-io/src/http2_pool.rs:282-284` → `crates/lb-io/src/http2_pool.rs:83-87`
- **Class:** Attacker-triggerable teardown of a shared resource / cross-request DoS
- **Applies to:** every listener with an `h2` upstream — i.e. the H1→H2 and H2→H2 cells.

### Code

```rust
// crates/lb-io/src/http2_pool.rs:112
struct Http2PoolInner {
    ...
    peers: Mutex<HashMap<SocketAddr, PeerEntry>>,   // ONE H2 connection per backend addr
}

// crates/lb-io/src/http2_pool.rs:83
impl Drop for PeerEntry {
    fn drop(&mut self) {
        self.driver.abort();          // <-- kills the hyper H2 connection task outright
    }
}

// crates/lb-io/src/http2_pool.rs:282
pub fn reset_peer(&self, addr: SocketAddr) {
    let _evicted = self.inner.peers.lock().remove(&addr);   // -> PeerEntry::drop -> abort()
}
```

`take_alive_sender` (`http2_pool.rs:255-265`) hands out `entry.sender.clone()` — hyper's
`SendRequest` clones share **one** connection. Every concurrent request to backend X on a listener
therefore multiplexes on a single H2 connection. Aborting the driver task drops that connection's
state and socket; every other cloned `SendRequest` and every in-flight `Incoming` response body on
it fails.

### Data flow (attacker input → sink)

**H2 front (cheapest):**

```
client HEADERS(POST, no END_STREAM) + 1 byte DATA + RST_STREAM
  -> hyper maps the reset to Ready(None) with is_end_stream()==false
  -> h2_proxy.rs:1390-1396  verdict_tx.send(Err(ProxyErr::BadRequest("... reset mid-body")))
  -> h2_proxy.rs:2400-2408  Ok(Err(e)) arm  =>  pool_for_task.reset_peer(backend_addr)
  -> http2_pool.rs:283 remove  ->  :85 driver.abort()  ->  shared upstream conn dies
```

**H1 front (equally cheap):** `POST / HTTP/1.1\r\nHost: x\r\nContent-Length: 100\r\n\r\nA` then
close the socket → `h1_proxy.rs:1417-1425` `Some(Err(e))` arm → the same
`verdict Err(BadRequest)` → `drive_h2_upstream_send` → `reset_peer`. A malformed chunk size works
identically.

Four other reachable triggers hit the same sink: `ProxyErr::BodyTooLarge` (exceed the 64 MiB cap),
invalid request trailers, a dropped verdict channel (`h2_proxy.rs:2420-2429`), and any send error
(`:2449`, `:2464`, `:2468`).

### Amplification

| Attacker | Gateway |
|---|---|
| one 13-byte `RST_STREAM` (or a socket close) on a stream already opened | pooled upstream H2 connection destroyed; **every** concurrently multiplexed request from **every other client** to that backend errors out (502); next request pays a fresh TCP connect + (if TLS) a full handshake |

Sustained at even a few hundred aborts/second the pooled H2 connection never survives long enough
to amortise: the gateway degrades to connection-per-request against the backend, and unrelated
clients see a continuous 502 stream. This is an availability attack **on other tenants** — the
attacker degrades service for everyone else, not for itself.

### Is it a known/accepted trade?

Partly, and this is the important nuance for triage. The comment at `http2_pool.rs:277-281` states
the trade explicitly:

> "Connection-scoped teardown is the deliberate trade: **an L7 abort is rare**, a smuggled-complete
> request is not recoverable."

The *correctness* reasoning (F-MD-4: injecting a body error lets hyper END_STREAM a truncated
request as complete) is sound and must not be regressed. What is not sound is the premise **"an L7
abort is rare"** — it is rare in benign traffic and free for an attacker. S38's L-RES-5 examined
this code path only for *leaks* ("broad teardown, **no per-stream leak**") and rated it clean;
`audit/deferred.md` ROUND8-L7-08 frames pool eviction as the *mitigation* for a timeout, not as an
attacker-reachable lever. Neither considered a hostile client deliberately driving it. Ordinary
browser behaviour (`fetch()` abort, navigating away mid-upload) also trips it, so this has a
self-DoS character under normal traffic too.

### Existing test coverage

None asserts collateral damage. `tests/h2_rapid_reset_goaway_under_load.rs` asserts a GOAWAY
reaches the *abusive* client — a protocol-signal assertion, not a resource or blast-radius
assertion. `http2_pool.rs`'s own tests cover pool-size invariants, not concurrent-stream survival
across a `reset_peer`.

### PoC the lead should run

Two clients, one backend (`protocol = "h2"`). Client A starts a long streaming request (slow
backend body). Client B sends `POST` + 1 byte + `RST_STREAM`. **Assert client A's response fails.**
Pre-fix that assertion passes (collateral proven); any fix must flip it while
`tests/h2h2_md_streaming_verify.rs`'s F-MD-4 smuggle arms stay green.

---

## RT-DOS-03 · MEDIUM · The H2 glitches abuse counter is dead in the shipped binary (CWE-1188)

- **CVSS:** `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L` (5.3)
- **Location:** `crates/lb-l7/src/h2_proxy.rs:284, 317` (both constructors set
  `glitches_threshold: None`), `:361-373` (`with_glitches`), `:500` (the `map` that is never
  entered) vs. `crates/lb/src/main.rs:1237-1259` (the production construction chain)

### Evidence

Repo-wide, `with_glitches` has exactly one caller:

```
crates/lb-l7/tests/round8_glitches_enforced.rs:45:        .with_glitches(THRESHOLD, registry),
```

The binary builds its `H2Proxy` at `crates/lb/src/main.rs:1237-1259`:

```rust
let mut proxy = H2Proxy::with_multi_proto(pool.clone(), picker, alt_svc, timeouts, is_https, security);
proxy = proxy.with_hooks(hooks);
proxy = proxy.with_health(Arc::clone(health));
if let Some(wd) = watchdog { proxy = proxy.with_watchdog(wd); }
if let Some(h2)  = h2_pool { proxy = proxy.with_h2_upstream(h2); }
if let Some(h3)  = h3_pool { proxy = proxy.with_h3_upstream(h3); }
if let Some(ws)  = ws_cfg  { ... }
if let Some(grpc)= grpc_cfg{ ... }
// <-- with_glitches is never called
```

And there is **no config knob**: `rg 'glitches' crates/lb-config/` returns nothing, even though
`audit/round-8/fixes/L7-07-L7-12.md:60` specifies `[runtime].h2_glitches_threshold_per_window = 200`
as part of the fix.

Consequently `glitches_threshold` is `None`, `glitch_state` at `h2_proxy.rs:500` is `None`, every
`record_glitch` call is a no-op, the `h2_glitches_total` counter is never registered (so it never
appears in `/metrics`), and the threshold-crossing `drain.cancel()` → two-step GOAWAY path is
unreachable in production.

### Why this matters for DoS

The glitches counter is the **only** mechanism that terminates an H2 connection for sustained
protocol abuse. With it inert, a client can hold one H2 connection for the full 60 s `total`
(`h2_proxy.rs:493, 540`) at `max_concurrent_streams = 256` and pipeline unlimited abuse requests —
smuggle rejects (`:794`), `:authority`/Host mismatches (`:806`), underscore-policy rejects, SNI
mismatches — each costing a HEADERS decode, validation, an error-response build, and an unthrottled
WARN log (see RT-DOS-05, which this directly amplifies). Nothing escalates, and the operator has no
metric showing it is happening.

### Contradicted claim

`audit/deferred.md:165-175` states the counter half is "now fully WIRED", cites
`crates/lb-l7/tests/round8_glitches_enforced.rs` as proof, and declares *"Theme-1 'library shipped
no caller' resolved."* The test constructs its own `H2Proxy` and calls `with_glitches` itself, so it
proves the **library** works while the **binary** never turns it on — which is precisely Theme-1.
This is a documentation-integrity defect as much as a security one.

### Existing test coverage

`crates/lb-l7/tests/round8_glitches_enforced.rs` passes and is non-vacuous *for the library*. What
is missing is a binary-level assertion — e.g. a config-boot test asserting `h2_glitches_total` is
present in `/metrics` after an H2 abuse request.

---

## RT-DOS-04 · MEDIUM · QUIC Retry reflection: no RFC 9000 §14.1 minimum-datagram check (CWE-406)

- **CVSS:** `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:N/I:N/A:L` (5.8 — the victim is a third party)
- **Location:** `crates/lb-quic/src/router.rs:145-163` (H3-terminate) and
  `crates/lb-quic/src/passthrough.rs:566-585` (Mode A)
- **Class:** UDP reflection / amplification; also an RFC 9000 §14.1 **MUST** violation
- **Note:** dual-classed — the `rfc-quic` auditor may file the spec half. This entry is the DoS half.

### Code

```rust
// crates/lb-quic/src/router.rs:139
    if header.ty != Type::Initial { return Ok(()); }
    let token_nonempty = header.token.as_ref().is_some_and(|t| !t.is_empty());
    if !token_nonempty {
        return send_retry(&header, peer, local, params).await;   // <-- no size gate above this
    }
```

```rust
// crates/lb-quic/src/passthrough.rs:544  (Mode A) — only a DCID-LENGTH floor, not a datagram floor
    if dcid.len() < ctx.params.min_client_dcid_len { ... return; }
    ...
// :571
    if tok.is_empty() && ctx.params.mint_retry {
        let retry_token = ctx.retry_signer.mint(from, dcid);
        ... ctx.listener_sock.send_to(&out, from).await
```

RFC 9000 §14.1: *"A server MUST discard an Initial packet that is carried in a UDP datagram with a
payload that is smaller than the smallest allowed maximum datagram size of 1200 bytes."* Neither
path checks the datagram length. `rg '1200|1_200' crates/lb-quic/src` finds only
`lb-config/src/lib.rs:1333`, which validates the configured **maximum** receive size — the opposite
direction.

### Amplification arithmetic

Retry token layout (`crates/lb-security/src/retry.rs:1-5, 124-140`):
`version(1) | issued_ms(8) | peer_kind(1) | peer_addr(4|16) | port(2) | odcid_len(1) | odcid(0..255) | mac(32)`.
With an IPv4 peer and a 20-byte ODCID: **69 bytes**.

| | bytes |
|---|---|
| Attacker's minimal parseable Initial (first byte 1 + version 4 + dcid_len 1 + dcid 20 + scid_len 1 + token_len 1 + length varint 1) | **≈29** |
| Retry reply (first byte 1 + version 4 + dcid_len 1 + dcid 0 + scid_len 1 + new SCID 16 + token 69 + integrity tag 16) | **≈108** |

**≈3.8× byte amplification** to a spoofed victim, plus a per-packet CPU cost on the gateway of one
HMAC-SHA256 (`retry.rs:137 hmac::sign`) and one AES-128-GCM retry-integrity tag (`quiche::retry` /
`build_retry_packet`). Retry minting is stateless, so there is no rate limit and no table growth to
throttle it — the gateway will answer every packet.

### Existing test coverage

None. `passthrough.rs`'s Retry tests all use well-formed handshakes; no test asserts that an
undersized Initial is discarded.

### Fix shape (for the lead's second pass)

Reject before minting: in `router.rs::dispatch_packet` and `passthrough.rs::handle_initial`, drop
`Type::Initial` whose **datagram** length is `< 1200`. Both call sites already have the full
datagram (`pkt: &mut [u8]` / `data: Vec<u8>`), so this is a length check, not a plumbing change.
Add a `quic_initial_undersized_total` counter so it is observable.

---

## RT-DOS-05 · MEDIUM · Unthrottled per-request WARN logging into a synchronous locked writer (CWE-779)

- **CVSS:** `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L` (5.3; **A:H** where stdout is a
  backpressuring pipe)
- **Location (emitters, all per-request and attacker-chosen):**
  `crates/lb-l7/src/h1_proxy.rs:741` (`h1 smuggle rejected`), `:761` (`h1 watchdog evicted`);
  `crates/lb-l7/src/h2_proxy.rs:794` (`h2 smuggle rejected`), `:806`
  (`h2 :authority/Host mismatch rejected`), `:860` (`h2 watchdog evicted`);
  `crates/lb-quic/src/conn_actor.rs:961` (`SESSION 22: malformed H3 request rejected`).
  **Location (sink):** `crates/lb-observability/src/log.rs:73-84`.

### Code

```rust
// crates/lb-observability/src/log.rs:73 — default writer, no non-blocking layer, no rate limit
LogFormat::Json => fmt().json().flatten_event(true)
    .with_current_span(true).with_span_list(false).with_target(true)
    .with_env_filter(filter).try_init(),
```

`tracing_subscriber::fmt()`'s default writer is `std::io::Stdout`, which serialises every event on
a process-wide `Mutex` and writes line-by-line. There is no `tracing_appender::non_blocking` and no
per-event rate limiting anywhere on the L7 or H3 request paths. The default filter is `info`
(`log.rs:43`), so every one of the sites above is emitted.

### Amplification

An ~80-byte request carrying both `Content-Length` and `Transfer-Encoding` (smuggle reject) or a
mismatched `:authority`/`Host` produces one ~300-byte structured JSON line **and takes the
process-wide stdout lock**. Because the front-end connection is keep-alive and the abuse never
escalates (RT-DOS-03: the glitches drain is inert), a single H2 connection can pipeline these at
line rate across 256 streams. Cost profile:

- CPU: JSON serialisation + `write` syscall per request.
- **Contention:** all tokio workers serialise on one `StdoutLock`.
- **Blocking:** if stdout is a pipe to a container log collector that applies backpressure, the
  write blocks the tokio **worker thread**, stalling every unrelated connection scheduled on it.
  This is the escalation to A:H.

### The team already solved this elsewhere

`crates/lb-quic/src/passthrough.rs:819-826` and `:605-616` use `audit_allow(...)` — an explicit
one-warn-per-window throttle, with the comment *"so an injection flood cannot drown the log."* The
same discipline is not applied on the L7/H3 request paths.

### Existing test coverage

None. No test bounds log volume under abuse.

### Fix shape

Either (a) demote these to `debug!` and surface the condition through the existing
`accept_reject_total` / `h2_glitches_total` counters (the operator-facing signal should be a metric,
not a log line), or (b) reuse the `audit_allow` throttle, or (c) install
`tracing_appender::non_blocking` so a slow consumer drops rather than blocks. (a) + (c) is the
cheapest correct combination.

---

## RT-DOS-06 · MEDIUM · H3 stream reset does not cancel the upstream fetch (CWE-404 / CVE-2023-44487 shape)

- **CVSS:** `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L` (5.3)
- **Location:** `crates/lb-quic/src/conn_actor.rs:1147-1163`

### Code — the gap, next to the two places that get it right

```rust
// conn_actor.rs:1147 — the H3 request-stream reset handler
Ok((sid, quiche::h3::Event::Reset(code))) => {
    if let Some(st) = ws_tunnels.get_mut(&sid) { ws_handle_client_reset(sid, st); continue; }
    tracing::debug!(stream_id = sid, code, "INC-2 F-MD-4: client reset request stream; Reset to upstream");
    if let Some(tx) = body_tx_by_stream.remove(&sid) {
        let _ = tx.try_send(ReqBodyEvent::Reset);        // request side only
    }
    body_seen.remove(&sid);
    pending_trailers.remove(&sid);
    // MISSING: resp_rx_by_stream.remove(&sid);
    // MISSING: stream_response.remove(&sid);
}
```

Compare the write-error path, which **does** tear the response side down:

```rust
// conn_actor.rs:360-377
Err(quiche::Error::StreamStopped(code)) | Err(quiche::Error::StreamReset(code)) => { cancelled.push(sid); }
...
for sid in cancelled {
    resp_rx_by_stream.remove(&sid);   // Drop the Receiver => producer's next send => ClientGone
    stream_response.remove(&sid);
}
```

and the WebSocket path, which aborts its task outright (`conn_actor.rs:1565-1569`:
`if let Some(st) = ws_tunnels.remove(&sid) { ... st.task.abort(); }`).

### Why the request-side reset is not enough

`ReqBodyEvent::Reset` is only observed while the producer is polling `body_rx` — the
`j2_req_event_action` / `H3ReqStreamBody` paths at `h3_bridge.rs:1088, 1242, 1293, 1609, 2064`. Once
the request body reaches a terminal state (client sent FIN, or the body was fully forwarded), the
producer stops polling `body_rx` and moves to reading the upstream response. A `RESET_STREAM`
arriving after that point:

1. finds `body_tx_by_stream` already empty (removed by the `Event::Finished` arm,
   `conn_actor.rs:1131`), so nothing is signalled;
2. leaves `resp_rx_by_stream[sid]` alive, so the producer's `send!` macro
   (`h3_bridge.rs:580-584`) never returns `ClientGone`;
3. leaves the spawned task in `resp_tasks` (`conn_actor.rs:1024/1041/1059`), which is only
   `retain`ed on `is_finished()` (`:317`) — never `abort()`ed.

Result: the whole upstream request — dial, send, and full response read — runs to completion for a
request the client cancelled. Per RFC 9000 the server's send direction survives the client's
`RESET_STREAM`, so a client that resets **without** `STOP_SENDING` never trips the `StreamStopped`
path that would have cleaned up. That asymmetry is the exploitable detail: a browser sends both, an
attacker sends only `RESET_STREAM`.

### Amplification

`HEADERS(GET /large-object) + FIN`, then one `RESET_STREAM` frame (a few bytes) → a full backend
fetch plus a response the client never reads. The attacker can additionally withhold stream
flow-control credit, in which case the producer parks on `tx.send().await` (unbounded — the `send!`
macro at `h3_bridge.rs:580` has no deadline) while **pinning the upstream connection**: an H1
upstream socket taken out of the pool via `take_stream`, or a stream on the shared pooled H2
connection. `max_requests_per_h3_connection` (default **1000**, `lb-config/src/lib.rs:172`) caps how
many such streams one QUIC connection can create, so the per-connection ceiling is ~1000 pinned
upstream resources — well above the 16-stream downstream concurrency limit
(`lb-quic/src/listener.rs:439`), because resetting a stream returns its QUIC stream credit while
the gateway-side task lives on.

Memory is genuinely bounded (R8 holds: `H3_RESP_CHANNEL_DEPTH = 8` × `H3_BODY_CHUNK_MAX = 8 KiB`);
what is not bounded is **upstream work and upstream file descriptors**, which is the actual
CVE-2023-44487 damage.

### Existing test coverage

None asserts cancellation. The H3 reset tests assert frame propagation
(`streamreset-vs-streamstopped` discipline), not that the producer task stops. A test that only
observes the RESET frame is vacuous for this property.

### Fix shape

Add `resp_rx_by_stream.remove(&sid); stream_response.remove(&sid);` to the `Event::Reset` arm —
byte-identical to the `:374-376` cleanup — so the producer's next `send!` yields `ClientGone`. For
a producer parked mid-`await`, additionally give `send!` a deadline or hold the `JoinHandle`
per-sid so it can be `abort()`ed, mirroring the WS path at `:1565-1569`.

---

## RT-DOS-07 · LOW · Two more O(N) table scans per admitted flow at the Mode A cap

- **Location:** `crates/lb-quic/src/passthrough.rs:375-389` (`evict_oldest`), `:400-416`
  (`reclaim_flows`), driven from `:620-627`

```rust
// :620 — at the cap, evict in a loop
while ctx.table.len() >= cap.saturating_mul(2) {
    if evict_oldest(&ctx) == 0 { break; }
}
```

`evict_oldest` walks the whole table to find the LRU victim (`:378 for entry in ctx.table.iter()`),
then calls `reclaim_flows`, which walks it **again** to collect the victim's keys
(`:401-411 ctx.table.iter().filter_map(...)`). So each admitted flow past the cap costs 2 × O(N),
N ≤ 200 000, on the same single-threaded recv task as RT-DOS-01.

Unlike RT-DOS-01 this requires a **valid Retry token**, so the source cannot be spoofed — hence LOW.
It compounds RT-DOS-01: an attacker who fills the table with real flows makes every subsequent
short-header miss maximally expensive.

**Fix shape:** keep an intrusive LRU (or a `BTreeMap<last_seen_ms, sid>` side index) so eviction is
O(log N), and store the victim's key list on the `FlowEntry` so `reclaim_flows` does not re-scan.

---

## RT-DOS-08 · LOW · Per-IP connection cap is bypassed by any single IPv6 /64

- **Location:** `crates/lb-security/src/conn_gate.rs:49` (`per_ip: DashMap<IpAddr, u32>`), `:140`
  (`self.inner.per_ip.entry(peer)`)

The gate keys on the full `IpAddr`. A residential or cloud IPv6 assignment is routinely a /64 —
2⁶⁴ source addresses on one host. Each address gets its own `per_ip_cap` (default **1024**,
`docs/guide/CONFIG.md:255`) budget, so one machine can consume the entire `max_inflight_connections`
(default 65 536) budget while never tripping the per-IP cap. The DashMap also grows one entry per
distinct source address until the permits drop (`conn_gate.rs:160-171` GCs correctly at count 0, so
this is not a leak — but the *concurrent* entry count tracks the concurrent connection count).

The gate logic itself is correct: the CAS loop at `:124-138` and the per-IP rollback at `:144` are
sound, `Drop` (`:157-174`) decrements both counters, and the `debug_assert!(prev > 0)` at `:173` is
unreachable because every permit increments exactly once. The weakness is purely the key choice.

**Fix shape:** key on a configurable prefix — `/32` for IPv4 (or `/24`), `/64` for IPv6 — matching
nginx `limit_conn_zone`'s `$binary_remote_addr` guidance and HAProxy's `src_conn_cur` with a
netmask. The `trusted_cidrs: Vec<IpNet>` field already carried on `GateInner` (`:53`, deferred by
L-002) is the natural home for the prefix-length knob.

Not in `docs/known-limitations.md` or `SECURITY.md` — the QUIC per-IP gap (F-RES-3) is documented,
this TCP-side one is not.

---

## RT-DOS-09 · LOW · O(n²) response-head scan against a dribbling backend

- **Location:** `crates/lb-quic/src/h3_bridge.rs:583-605` (the loop), `:940-942`
  (`find_header_sep`)

```rust
fn find_header_sep(buf: &[u8]) -> Option<usize> { buf.windows(4).position(|w| w == b"\r\n\r\n") }

// :587
let sep = loop {
    if let Some(p) = find_header_sep(&head) { break p; }   // rescans from index 0 every iteration
    if head.len() > HEAD_CAP { ... }                        // HEAD_CAP = 64 KiB
    let n = stream.read(&mut rbuf).await ...;
    head.extend_from_slice(rbuf.get(..n).unwrap_or(&rbuf));
};
```

A backend that emits one byte per TCP segment forces 65 536 iterations, each rescanning up to
65 536 bytes: **≈2.1 × 10⁹ byte comparisons** (~1–2 s of a core) for 64 KiB of response head, per
request. Correctness is fine — `windows(4).position` guarantees `sep + 4 <= head.len()`, so the
`head.split_off(sep + 4)` at `:611` cannot panic.

Rated LOW because the trigger is the **backend**, which is semi-trusted in the threat model
(`SECURITY.md` boundary 2), and 64 KiB caps the blowup. It becomes MEDIUM in a deployment where
backends are less trusted than the model assumes.

**Fix shape:** track a `scan_from = head.len().saturating_sub(3)` cursor so each byte is examined
once, making the scan O(n).

---

## RT-DOS-10 · INFO · Six "security" tests and the CI Chaos gate exercise dead code

This is a gate-integrity finding, not a vulnerability. It matters because it is the evidence three
prior passes leaned on.

**None of these detector types has a production caller.** Verified by
`rg 'RapidResetDetector|ContinuationFloodDetector|HpackBombDetector|SettingsFloodDetector|PingFloodDetector|ZeroWindowStallDetector|QpackBombDetector|SlowlorisDetector|SlowPostDetector' crates tests`
— every hit is a test, a re-export, or the type's own module. In `lb-h2` they survive only as the
source of the `DEFAULT_*` constants consumed by `crates/lb-l7/src/h2_security.rs:44-56`.

| Test file | Exercises | Live path it is cited as proving |
|---|---|---|
| `tests/security_rapid_reset.rs` | `lb_h2::RapidResetDetector` — a counter | hyper `max_pending_accept_reset_streams` |
| `tests/security_continuation_flood.rs` | `lb_h2::ContinuationFloodDetector` — a counter | enforced inside `h2` 0.4.19 |
| `tests/security_hpack_bomb.rs` | `lb_h2::HpackBombDetector` | hyper `max_header_list_size` |
| `tests/security_qpack_bomb.rs` | `lb_h3_testcodec::QpackBombDetector` (**a test-only crate**) | quiche QPACK |
| `tests/security_slowloris.rs` | `lb_security::SlowlorisDetector` | hyper `header_read_timeout` + `total` |
| `tests/security_slow_post.rs` | `lb_security::SlowPostDetector` | `idle_bounded_send` Phase-A |

`SECURITY.md`'s defenses table cites each of these in its "Reference" column as the proof for the
corresponding live defense. Rows 12/13 additionally name `lb-security::SlowlorisDetector` and
`SlowPostGuard` as the *code sites* — and `SlowPostGuard` does not exist (the type is
`SlowPostDetector`). `docs/research/dos-catalog.md` is further out of date: it cites
`crates/lb-h3/src/security.rs::QpackBombDetector` in a crate deleted at S26, describes the
`RapidResetDetector` window algorithm as the mitigation with no delegation caveat, and lists
"SETTINGS flood / PING flood — Gap" and "zero-window stall — not implemented", all of which
`h2_security.rs` has since covered on the live builder.

**The CI gate is the sharper half.** `.github/workflows/ci.yml:416-418`:

```
cargo nextest run --all-features --package lb-h2 --package lb-l7 \
  -E 'test(/chaos|rapid_reset|continuation|hpack|slowloris/)'
```

The job is named *"Chaos Attack Suite: Rapid Reset, CONTINUATION flood, HPACK bomb, slowloris."*
Restricted to `--package lb-h2 --package lb-l7`, the filter resolves to **11 in-crate unit tests in
`crates/lb-h2/src/security.rs` and `hpack.rs`** — the dead detector types — and **zero** tests match
`slowloris` in either package (`crates/lb-l7/tests/s38_h1_header_timeout.rs`, which *is* the
genuinely non-vacuous slowloris test, is named
`h1_partial_head_closed_at_header_timeout_not_total` and does not match). The real live-wire tests
(`tests/h2_security_live.rs`, `tests/h2_rapid_reset_goaway_under_load.rs`) are in the root
`lb-integration-tests` package and are **excluded** by the `--package` filter — they do run, but in
the general `test` job, not this gate.

So the named DoS gate can stay green while every live H2 abuse defense regresses. Fix: point the
filter at the root package too and add `s38_h1_header_timeout` (or rename it) so slowloris is
actually covered; then correct the two docs.

*Also observed:* `scripts/halting-gate.sh` is not invoked by any workflow
(`rg 'halting-gate' .github/ scripts/ci/` is empty) — its check-3 panic grep contributes nothing in
CI. That is fine in itself, because `cargo clippy --workspace --all-targets --all-features -- -D warnings`
(ci.yml:73) is the real enforcement, but SECURITY.md presents the halting-gate grep as the
enforcement mechanism.

---

## RT-DOS-11 · INFO · `EnforcedLabelBudget` has zero call sites

- **Location:** `crates/lb-observability/src/label_budget.rs:154-263`; only reference outside the
  module is the re-export at `crates/lb-observability/src/lib.rs:43`.

**There is no live metric-cardinality vector today** — I checked every dynamic
`with_label_values(&[...])` argument in `crates/lb-observability/`, `crates/lb/src/main.rs` and
`crates/lb-l7/`, and each is config-derived or a closed enum (`kind.as_label()`, `change.field()`,
`timing.phase.as_label()`, `reason`). Critically, the `route` label on `http_requests_total` /
`http_request_duration_seconds` is **hardcoded empty** at the single emit site:

```rust
// crates/lb/src/main.rs:3247
let route_label = "";
```

So the family is bounded by `listeners × 1 × 4 versions × 5 status classes`. The startup
`LabelBudget::check` (`label_budget.rs:120-148`) is wired and correct.

The finding is that the *per-emission* guard built to protect against the next dynamic label is
attached to nothing. `main.rs:1897` even carries the comment "`route` is capped by
MAX_ROUTES_BUDGET so a hostile path cannot explode the series count" — but nothing enforces
`MAX_ROUTES_BUDGET` at emit time; the safety comes entirely from `route` being a constant. If
anyone ever populates `route` from the request URI, the protection they will believe is present is
not.

---

## RT-DOS-12 · INFO · `collect_body_bounded` is accumulate-then-check (zero callers)

- **Location:** `crates/lb-io/src/http2_pool.rs:321-334`

```rust
pub async fn collect_body_bounded(body: Incoming, max_body: usize) -> io::Result<Bytes> {
    let collected = body.collect().await ...;      // <-- whole body into memory FIRST
    let bytes = collected.to_bytes();
    if bytes.len() > max_body { return Err(...); } // <-- cap checked AFTER
```

Exactly the antipattern the brief names. It is currently **dead** — `rg 'collect_body_bounded'`
finds only this definition, and the live response legs stream (`h2_proxy.rs:2144-2158`
`finalize_response`, `h3_bridge.rs:814` "never `.collect()`"). Reported so it is deleted or fixed
rather than picked up later: a `pub` helper with an unbounded allocation is a loaded gun in a crate
whose whole R8 discipline is "no whole-body buffering."

*Adjacent naming trap, same file:* `Http2PoolConfig::max_concurrent_streams` (`:48-49`, doc-comment
"Concurrent streams per H2 connection") is passed to `.max_concurrent_reset_streams(...)` at
`:298` — a different h2 knob that sizes the **reset-stream cache**, not stream concurrency. There is
no client-side cap on outstanding upstream streams (the backend's SETTINGS governs). Harmless today,
but the field name asserts a bound that does not exist.

---

## RT-DOS-13 · INFO · Panic-gate gap analysis

**What is actually enforced.** `.github/workflows/ci.yml:64-73` runs
`cargo clippy --workspace --all-targets --all-features -- -D warnings`, and every library crate
carries `#![deny(clippy::unwrap_used, expect_used, panic, indexing_slicing, todo, unimplemented,
unreachable, missing_docs)]`. I verified all 18 crate roots; the only `allow` overrides are
`#[cfg(test)]`-scoped or on `mod tests` (`lb-core/src/authority.rs:73`,
`lb-l7/src/authority.rs:47`, `lb-io/src/{pool,dns,http2_pool}.rs`, `lb-l7/src/{h1_proxy,h2_proxy,
grpc_proxy,ws_proxy,upstream,trace_ctx}.rs`, `lb/src/main.rs:4238,4256`). That posture is real and
covers indexing and slicing, which is more than most projects do.

The `panic-freedom` job (ci.yml:75-94) only greps that the deny attribute is *present* in each
`crates/*/src/lib.rs`. It is a tautology given the clippy job — worth knowing when reading its
green tick as evidence.

**Five classes nothing catches.** I hunted each across the in-scope crates and found the code
currently clean; the point is that no gate would catch a regression.

1. **`assert!` / `assert_eq!` / `debug_assert!`** — not in the deny set, not in the halting-gate
   grep. Only two exist in production code, both `debug_assert!` (inert under `panic = "abort"`
   release): `conn_gate.rs:173`, `handshake.rs:42`. Both are unreachable as analysed. A future
   `assert!` on a wire-derived length would be a single-packet abort with no gate objection.
2. **Arithmetic overflow/underflow** — `clippy::arithmetic_side_effects` is not denied and
   `overflow-checks` is off in release (`Cargo.toml:214-225`), so a bad subtraction *wraps* rather
   than panicking: a wrong limit, not a crash. The code is disciplined (`saturating_*` /
   `checked_*` everywhere I traced, e.g. `retry.rs:162`, `watchdog.rs:157-168`,
   `h2_proxy.rs:1306`), but by convention only.
3. **Panicking slice/`Bytes`/`Vec` methods outside `indexing_slicing`'s reach** — `split_at`,
   `split_to`, `split_off`, `advance`, `copy_from_slice`, `copy_to_bytes`, `Vec::insert/remove/
   drain/swap_remove`, `chunks(0)`, `windows(0)`, `step_by(0)`, `Duration::from_secs_f64`. I audited
   every occurrence in `lb-l7`, `lb-quic`, `lb-io`, `lb-h2`, `lb-security`, `lb-observability`,
   `lb` (36 sites) and **every one is `min()`-clamped or `.get()`-guarded**:
   `h1_proxy.rs:944,1303,1568` and `h2_proxy.rs:1301,1732,1967` use
   `data.len().min(H2_REQ_CHUNK_MAX)`; `h3_bridge.rs:611 head.split_off(sep + 4)` is safe because
   `find_header_sep` uses `windows(4).position`; `ws_tunnel.rs:154` uses `min(buf.remaining())`;
   `retry.rs:177,231,235,239` each `copy_from_slice` from an exactly-sized `.get(range)?`;
   `conn_actor.rs:577,1546` and `raw_proxy.rs:826` are bounded by the byte count just read.
   **Clean, and unguarded.**
4. **Division / modulo by zero** — `watchdog.rs:161` correctly uses `checked_div`; the only other
   divisor I found is the constant `6` in `lb-h2/src/frame.rs:303`. Clean.
5. **Dependency panics triggered by our arguments** — chiefly
   `tokio::time::interval(Duration::ZERO)` (panics: *"interval period must be non-zero"*). All six
   call sites are constants or config-validated: `main.rs:822,1925,2011,2449` are literals,
   `main.rs:2340` reads `sweep_interval_ms` which `lb-config/src/lib.rs:963` pins to `100..=60_000`,
   and `passthrough.rs:467` is `.clamp(1 s, 10 s)`. Clean, but a new config-driven interval with a
   permissive range would be a boot-time or SIGHUP-time abort.

**Recommendation:** add `clippy::arithmetic_side_effects` at `warn` (not `deny` — the churn is
large) and add `assert`/`assert_eq`/`debug_assert`/`split_at`/`copy_from_slice`/`from_secs_f64` to
the halting-gate grep, *and wire the halting-gate script into CI* (see RT-DOS-10) so the grep is
more than documentation.

---

## ALREADY-KNOWN (verified still accurate, not re-reported)

- **F-RES-1 — H1 `header_read_timeout` inert.** FIXED. `h1_proxy.rs:455-461` now wires
  `.timer(TokioTimer::new()).header_read_timeout(self.timeouts.header)`, and
  `crates/lb-l7/tests/s38_h1_header_timeout.rs` is a genuine negative control.
- **F-RES-2 — upstream `max_header_list_size`.** FIXED, `http2_pool.rs:43, 300`.
- **F-RES-3 — QUIC global cap hardcoded, no per-IP QUIC sub-cap.**
  `ALREADY-KNOWN: SECURITY.md "Residual risks" + docs/known-limitations.md "Mode A passthrough
  relies on the QUIC Retry round-trip".` Still true. RT-DOS-01/04 are *different* vectors on the
  same listener and are not covered by it.
- **F-RES-5 — Watchdog is observability-only, `SlowRate` dormant.**
  `ALREADY-KNOWN: SECURITY.md "Residual risks"`, and now self-documented at
  `crates/lb-security/src/watchdog.rs:1-6`. The table is bounded (`max_registered = 100 000`,
  `:126-129`). One live consequence *is* new and folded into RT-DOS-05: `register`'s `false` return
  is ignored at `h1_proxy.rs:753` / `h2_proxy.rs:853`, so at the cap every request logs
  `WatchdogError::Unknown` at WARN.
- **R8 streaming + 64 MiB caps + 413.** Verified no cell regressed:
  `MAX_REQUEST_BODY_BYTES` / `MAX_RESPONSE_BODY_BYTES` (`h2_proxy.rs:71`, `h3_bridge.rs:26`) are
  enforced *incrementally in the pumps* before forwarding (`h1_proxy.rs:1342`,
  `h2_proxy.rs:1351`), never after a `.collect()`. Chunk sizes are `min()`-clamped. Clean.
- **H2 server flood/bomb config applied on the live builder.** Verified:
  `h2_proxy.rs:527-529` (`builder.timer(...)` then `self.security.apply(&mut builder)`),
  `h2_security.rs:66-77`. `h2` is 0.4.19 and `hyper` 1.10.1, so CONTINUATION (CVE-2024-27316) is
  enforced inside `h2`. Clean.
- **WS-over-H2 gated off / unbounded write.**
  `ALREADY-KNOWN: docs/known-limitations.md "WebSocket over HTTP/2 (RFC 8441) is gated OFF"`.
- **`accept(2)` EMFILE/ENFILE handling.** Correct — `main.rs:2907-2932` classifies and applies
  `next_accept_backoff` rather than hot-spinning. No finding.
- **DNS.** `crates/lb-io/src/dns.rs` has no cache size cap and no expired-entry eviction, but
  `resolve()` is only ever called with **config-derived** hostnames (`main.rs:525, 1426, 1566`) —
  no request-path caller exists. Not attacker-reachable; noted only so a future
  "resolve the Host header" feature is understood to make the cache unbounded. (Also observed:
  `spawn_background_refresh` has no production caller, so `refresh_all`'s serial O(N)
  `spawn_blocking` walk never runs.)
- **TCP pool.** `per_peer_max` / `total_max` bound the **idle** queue only (`pool.rs:332, 347`);
  there is no cap on concurrently *in-use* upstream sockets — normal for a reverse proxy, and the
  fd budget is an operator concern documented at `docs/guide/DEPLOYMENT.md:135-137`
  (`nofile = 1_048_576`). No finding, but there is no boot-time check that
  `RLIMIT_NOFILE >= 2 × max_inflight_connections`.
- **Mode A flow/fd leak (S21 F-S20-2), `BoundedDgramQueue`, `MAX_RELAY_STREAMS`, S36 H3
  recycling.** Re-checked; the bounds cited in S38 L-RES-2/L-RES-5 still hold.
- **No body decompression anywhere.** Re-confirmed: no compression crate is wired to a body, so
  there is no bomb surface.
