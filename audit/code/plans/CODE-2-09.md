# Plan for CODE-2-09 + REL-2-11 — Replace spawn_blocking(connect) with tokio::net + timeout
Finding-ref:     CODE-2-09 / REL-2-11 (high, Open) — lead-approved option per §E.4
Files touched:
  - `crates/lb-io/src/lib.rs`                       (`Runtime::connect` → async)
  - `crates/lb-io/src/pool.rs`                      (`acquire` → async, `dial_fresh` → async)
  - `crates/lb-io/src/dns.rs`                       (replace blocking getaddrinfo at :283)
  - `crates/lb-io/src/http2_pool.rs:261`            (drop spawn_blocking wrapper)
  - `crates/lb-l7/src/h1_proxy.rs:419,659`          (drop spawn_blocking wrapper)
  - `crates/lb-l7/src/h2_proxy.rs:410,472`          (drop spawn_blocking wrapper)
  - `crates/lb-l7/src/grpc_proxy.rs:214`            (drop spawn_blocking wrapper)
  - `crates/lb-quic/src/h3_bridge.rs:275`           (drop spawn_blocking wrapper)
  - `crates/lb/src/main.rs:1238`                    (drop spawn_blocking wrapper)
  - `crates/lb-config/src/runtime.rs`               (`upstream_connect_timeout_ms`)

Approach:
Make `pool.acquire(addr)` an async fn that calls
`tokio::net::TcpStream::connect` directly, wrapped in
`tokio::time::timeout`. Setsockopts apply post-connect via
`socket2::SockRef::from(&stream)`.

Step 1 — `Runtime::connect` signature change. In `lb-io/src/lib.rs`:
```rust
pub async fn connect(&self, addr: SocketAddr) -> io::Result<TcpStream> {
    let deadline = self.config.upstream_connect_timeout_ms
                       .map(Duration::from_millis).unwrap_or(Duration::from_secs(5));
    let stream = tokio::time::timeout(deadline, tokio::net::TcpStream::connect(addr))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "connect timeout"))??;
    apply_sockopts(&stream, &self.config.socket_opts)?;
    Ok(stream)
}
fn apply_sockopts(stream: &TcpStream, opts: &SocketOpts) -> io::Result<()> {
    let sock = socket2::SockRef::from(stream);
    sock.set_nodelay(opts.nodelay)?;
    sock.set_keepalive(opts.keepalive_enabled)?;
    if let Some(d) = opts.keepalive_time { sock.set_tcp_keepidle(d)?; }
    // … rest of setsockopt list …
    Ok(())
}
```
`TcpStream::connect` returns a tokio stream directly; no `into_std`
juggling needed because `socket2::SockRef::from(&TcpStream)` accepts
either a std or tokio stream via the `AsFd` trait.

Step 2 — Pool path. `lb-io/src/pool.rs::acquire` becomes
`pub async fn acquire(self: &Arc<Self>, addr: SocketAddr) -> Result<PooledConn>`.
Internal `dial_fresh` mirrors. The CAS-loop capacity gate (per
CODE-2-04 §pool.rs:372) is unchanged in shape but the load-then-add
sequence becomes async-aware.

Step 3 — DNS. `lb-io/src/dns.rs:283` swaps `spawn_blocking(getaddrinfo)`
for `tokio::net::lookup_host(host)` — tokio's resolver runs on its own
blocking pool but is built-in and cancellation-friendly. The DNS-cache
machinery upstream is untouched.

Step 4 — Caller cleanup. Every caller currently shaped like
```rust
let conn = tokio::task::spawn_blocking(move || pool.acquire(addr)).await??;
```
becomes
```rust
let conn = pool.acquire(addr).await?;
```
That's 8 sites (enumerated above). Cancellation propagates naturally:
when the caller's task is cancelled, the connect future is dropped
mid-syscall and tokio aborts the kernel-side connect via socket close.

Step 5 — Config. `RuntimeConfig::upstream_connect_timeout_ms:
Option<u64>` (default 5000). Per-cluster override flows through
existing cluster config struct.

Proof:
- `crates/lb-io/tests/connect_timeout.rs::times_out_on_blackhole`:
  attempt `connect()` to `192.0.2.1:81` (RFC 5737 TEST-NET); assert
  it fails with `ErrorKind::TimedOut` within `timeout_ms + 200 ms`,
  *not* after the kernel's 75-s default.
- `crates/lb-io/tests/connect_cancel.rs::cancel_drops_connect`:
  start `acquire()`, cancel its future after 10 ms; assert no
  blocking-pool thread is still alive (probed via tokio's
  `metrics::Handle::num_blocking_threads`).
- `crates/lb-io/tests/pool_no_spawn_blocking.rs::no_spawn_blocking_in_acquire`:
  static check — `cargo expand -p lb-io` of `pool::acquire` shows no
  `spawn_blocking` token. Encoded as a grep CI step.
- `criterion` benchmark `benches/connect_cold.rs` shows ≥30 % p99
  latency improvement on cold-pool burst (was ~150 ms, target ≤100 ms
  on a localhost target).

Risk / blast radius:
- Async ripple: 8 sites become `.await?` instead of `.await??`. Mechanical.
- `socket2` is already a workspace dep (used elsewhere).
- One non-obvious behaviour: tokio's `TcpStream::connect` does NOT
  set `TCP_NODELAY` by default; the `apply_sockopts` helper restores
  parity. Round-4 must keep an eye on a benchmark regression.
- DNS swap from `getaddrinfo` to `lookup_host` changes resolver
  semantics slightly (caching, IPv6 ordering). `lb-io::dns`
  integration tests cover the contract.

Cross-ref:    REL-2-11 (joint), CODE-2-03 (cancel propagation)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
