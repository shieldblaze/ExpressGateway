# Round 2 — `code` Review Findings

Reviewer: `code` (senior Rust correctness/concurrency/lifecycle).
Scope: Rust source quality across the 18-crate workspace. Stable IDs
`CODE-2-NN`. All cross-refs use the synthesis at
`audit/CROSS-REVIEW-SYNTHESIS-r1.md` and the Round 1 inventories.

Findings are ordered roughly by severity. The atomic-ordering umbrella
appendix is at the bottom (CODE-2-04).

---

### CODE-2-01 — `lb-l7` does not depend on `lb-security`; smuggling/slowloris/slow-POST detectors and per-IP cap are dead code on the hot path
Severity: critical
Blocking-for-prod: yes
Status:   Verified-Fixed(3dcb6f3, dc02517, e00e85a, 5e7938f)   <!-- ebpf round-5: dep-edge + trait shim + detector wire-up present; ConnGate accept-site wired in 4001791 (SEC-2-04). Bypass attempts on trait-shim error propagation + ConnPermit Drop semantics found nothing. -->
Location:
  - `crates/lb-l7/Cargo.toml` (missing `lb-security = { path = "../lb-security" }`)
  - `crates/lb-security/src/{smuggle.rs, slowloris.rs, slow_post.rs}` (detectors exist but no consumer in lb-l7)
  - `crates/lb-l7/src/h1_proxy.rs:285` (H1 server entry), `crates/lb-l7/src/h2_proxy.rs:221` (H2 server entry), `crates/lb-l7/src/ws_proxy.rs`
  - `crates/lb-l7/src/{h1_to_h1, h1_to_h2, h2_to_h1, h2_to_h2}.rs` (bridges: no detector invocation)

Description / Impact:
The workspace ships full implementations of `SmuggleDetector`,
`SlowlorisDetector`, `SlowPostDetector`, and a per-IP / per-listener
connection cap inside `lb-security`. Independently confirmed by
Round 1 inventories (`code` §1.2, `sec` S-1/S-3/S-4, `proto` #1, `rel`
F-17), `lb-l7` does not have `lb-security` in its `[dependencies]`, so
none of these primitives are reachable from the proxy hot path. The
README + RUNBOOK advertise these defenses as live. The result is a
proxy that silently lacks the smuggling/slowloris defenses it claims.

This is a **dependency-graph code-quality finding**; `sec` owns the
per-detector wire-up findings (S-1/S-3/S-4) and `proto` owns the
"hyper-doesn't-cover-this-case" matrix.

Reproduction:
```
$ cargo tree -p lb-l7 --format '{p}' | grep lb-security
$ # (no output)
$ cargo tree -p lb-quic --format '{p}' | grep lb-security
lb-security v0.1.0 (.../crates/lb-security)
$ # only lb-quic and the bin pull lb-security; the L7 proxies do not.
```

Recommendation:
1. Add `lb-security = { path = "../lb-security" }` to
   `crates/lb-l7/Cargo.toml`.
2. Wire `SmuggleDetector` into `proxy_request` after hyper parses the
   request and before it is forwarded — both in `h1_proxy.rs` and
   inside the H2 server adapter where the leaf `Bridge` is selected.
   Reject with 400 on detector failure.
3. Construct one `SlowlorisDetector` per listener (configurable
   `request_read_timeout`) and arm it on the accept future before
   handing to hyper. The accept path is currently
   `crates/lb/src/main.rs:1100–1106`; the per-mode `accept().await` at
   `:1136` and `:1154` is the natural arming site.
4. Construct `SlowPostDetector` per connection and tick it from inside
   the body-reading path. For H1 this means hooking into the hyper
   body stream wrapper at the bridge call site; for H2 this means
   tracking stream-window credits.
5. Per-IP / per-listener concurrent-connection cap: maintain
   `Arc<DashMap<IpAddr, AtomicUsize>>` next to `ListenerState`,
   bump/decrement around the `tokio::spawn` at `main.rs:1126`, refuse
   with `503` (TCP RST for non-HTTP) past the cap.

Cross-ref: synthesis T1; sec S-1/S-3/S-4; proto #1; rel F-17.

---

### CODE-2-02 — `panic = "abort"` not set in `[profile.release]`; default unwind across unsafe boundaries
Severity: critical
Blocking-for-prod: yes
Status:   Verified-Fixed(120e4fa, b6aeea5)   <!-- ebpf round-5: panic=abort in release+bench; dev/test deliberately keep unwind for proptest/loom. init_panic_hook installed before any spawn. panic_total Counter exposed. catch_unwind audit found one site (CODE-2-08) that's dead code under abort — documented. -->
Location:
  - `Cargo.toml:162–166` (`[profile.release]` has no `panic` key)
  - 17 `unsafe` blocks in `crates/lb-io/src/ring.rs` (io_uring SQE submission, sockaddr conversion, raw fd close)
  - 51 `unsafe` blocks in `crates/lb-l4-xdp/ebpf/src/main.rs` (out of workspace, but the userspace loader at `crates/lb-l4-xdp/src/loader.rs` is in-workspace and contains the 4 `unsafe impl Pod` sites at lines 53/76/97/120)

Description / Impact:
With no `panic = "abort"` in release, Rust defaults to `unwind`. A
panic inside any future spawned via `tokio::spawn` is silently caught
by tokio's `JoinError::Panic` machinery. The proxy then continues
running with whatever invariants the panicking task was about to
restore left half-broken. This is *especially* dangerous because:

1. `crates/lb-io/src/ring.rs:345` performs `libc::close` on raw fds on
   error — a panic mid-unwind that touches an already-`close()`'d fd
   becomes a double-close, which is a real `EBADF` race on any thread
   that recycles the fd number. With `panic = "abort"` the process dies
   instead of double-closing.
2. The 4 `unsafe impl aya::Pod` sites assume layout stability. A panic
   that aborts mid-`insert` on a BPF map could leave the map in a
   partial state visible to the kernel — abort is safer than continue.
3. `set_hook` is not installed (sec S-1 / rel F-22 corroborate), so a
   panic doesn't even get logged with a backtrace at level `error!`.

Reproduction (today's behaviour): inject `panic!()` inside any spawned
handler at `crates/lb/src/main.rs:1126`. The process continues
accepting traffic; the only observable evidence is a `JoinError` value
nobody reads (the `JoinHandle` is dropped).

Recommendation:
```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = "symbols"
panic = "abort"   # add this line
```
And install a `std::panic::set_hook` early in `main` that logs at
`error!` level with the panic info + a backtrace, then re-aborts.

Note: `panic = "abort"` interacts with `catch_unwind` (none used in
this workspace — confirmed by grep). It also disables landing pads in
the release binary which is a small code-size win.

Cross-ref: sec §B.2 (rel F-22 cross-ref); rel F-22.

---

### CODE-2-03 — TCP/H1/H1s accept loop has no graceful drain; SIGTERM aborts in-flight connections via `JoinHandle::abort()`
Severity: critical
Blocking-for-prod: yes
Status:   Verified-Fixed-Partial(9ff2b9b, fc050b0)   <!-- ebpf round-5: Shutdown primitive + drain budget wired into main.rs; SIGTERM triggers cancel + drain. 5 `tokio::spawn` sites in main.rs remain NOT routed through tracker.spawn (per-connection handler at :2074 is the most consequential). Follow-on tracked under same ID per audit margin note. XdpLoader Drop ordering verified safe across drain boundary. -->

Location:
  - `crates/lb/src/main.rs:701` (`spawn_listener` — `JoinHandle` stored, never paired with a token)
  - `crates/lb/src/main.rs:1099–1106` (accept loop has no `select! { _ = cancel.cancelled() => break, … }`)
  - `crates/lb/src/main.rs:1126` (`tokio::spawn(async move { … })` per-connection handler — not tracked by an inflight counter, no `CancellationToken` passed in)
  - `crates/lb/src/main.rs:892` (1-second metrics sampler `tokio::spawn` — no cancel)
  - `crates/lb/src/main.rs:233` (TLS-ticket rotator — exits only when `Arc::strong_count <= 1`; cooperative but indirect)
  - `crates/lb/src/main.rs:1037–1060` (shutdown path: relies on `JoinHandle::abort` + a fixed sleep)
  - Per-direction L7 spawn sites with no cancel (from Round 1 inventory §2.2):
    `crates/lb-l7/src/h1_proxy.rs:432, 581, 659`
    `crates/lb-l7/src/h2_proxy.rs:400, 487`
    `crates/lb-l7/src/grpc_proxy.rs:250`
    `crates/lb-quic/src/conn_actor.rs:296, 307, 320`
    `crates/lb-io/src/dns.rs:340`, `crates/lb-io/src/http2_pool.rs:284`

Description / Impact:
Round 1 enumerated 33 `tokio::spawn` sites. Only 3 honour a
`CancellationToken` (admin HTTP, QUIC listener, QUIC router). The TCP
accept loop, every per-connection handler, the metrics sampler, the
TLS-ticket rotator, and every L7 upstream-connection driver have no
cooperative shutdown — they exit only when the future returns
naturally (stream close, response complete, idle timeout) or when the
runtime is dropped.

On SIGTERM, the documented "30-second graceful drain with FD passing"
collapses to `JoinHandle::abort()` (rel H7), which severs every
in-flight `await` mid-call. From the client's perspective this is a
TCP RST or an aborted HTTP/2 stream with no GOAWAY. Roll-out / blue-
green deploys lose in-flight requests for the entire abort fan-out.

This is the *implementation* side of rel H7. `rel` owns the SIGTERM
finding and the drain-budget decision (lead set 10 s default); `proto`
owns H2 GOAWAY emission. **This finding is the CancellationToken
plumbing across the spawn fleet.**

Recommendation:
1. Build a single `CancellationToken` in `main` and pass it (cloned)
   into every long-lived spawn:
    - `spawn_listener` clones it into `ListenerState`.
    - The accept loop becomes
      `tokio::select! { _ = cancel.cancelled() => break, acc = listener.accept() => acc }`.
    - The per-connection handler at `main.rs:1126` is moved behind a
      `tokio_util::task::TaskTracker` (preferred over hand-rolled
      `JoinSet` because it lets you `wait()` for the trailing fan-out).
    - Metrics sampler at `:892`: replace the open `loop { ticker.tick() }`
      with `select! { _ = cancel.cancelled() => return, _ = ticker.tick() => …, }`.
    - TLS-ticket rotator at `:233`: same pattern.
2. Inside `lb-l7`, the upstream-connection drivers spawned at
   `h1_proxy.rs:432`, `h2_proxy.rs:487`, `grpc_proxy.rs:250` should
   accept a cancel token so the proxy can stop pushing into a
   bidirectional copy when the listener is draining.
3. `shutdown_signal()` (`main.rs:1279`) must trigger `cancel.cancel()`,
   wait on `task_tracker.wait()` with a deadline (lead: 10 s), then
   only abort what's left.

Cross-ref: synthesis T2; rel H7 + rel→code handoff #1/#3/#8; proto H2 GOAWAY.

---

### CODE-2-05 — Unbounded `tokio::spawn` per accept; no semaphore / max-inflight gate on TCP listener
Severity: critical
Blocking-for-prod: yes
Status:   Verified-Fixed(f07cf44)   <!-- ebpf round-5: per-listener Arc<Semaphore> sized from runtime.max_inflight_connections; try_acquire_owned at accept site (main.rs:2025); permit moved into spawn closure (drops on task exit / panic). 503 path + accept_shed_total counter present. Two proof tests at :2432 + :2562. Bypass attempts (permit leak before move, fast-path skip) both failed. -->
Location: `crates/lb/src/main.rs:1126` (`tokio::spawn(async move { … })`)

Description / Impact:
Every accepted TCP connection unconditionally spawns a tokio task.
The only soft bound is `active_connections.fetch_add(1, Relaxed)` at
`:1127`, which is a *gauge* — nothing reads it to refuse new spawns.
QUIC has `max_connections = 100_000`; TCP has *zero*. Two failure
modes:

1. **Connect-flood DoS.** An attacker SYN-floods (or completes 3WHS)
   faster than handlers finish. Memory grows linearly per pending
   handler (typically 64 KB of task stack + per-handler buffers).
   No 503 path exists; the proxy will OOM-kill before the kernel's
   `somaxconn` is involved (somaxconn only bounds the *accept queue*,
   not the accepted-but-not-yet-completed handlers).
2. **Slow-handshake amplification.** Combined with no slowloris
   timeout (CODE-2-01 / sec S-3 / S-10), each spawn lives for the
   duration of the attacker's slow handshake.

Reproduction: `ab -c 50000 -n 50000 -t 0 http://lb/` against a single
listener with a slow upstream. The handler count grows monotonically;
RSS climbs proportionally.

Recommendation:
1. Per-listener `Arc<Semaphore>` sized from
   `[runtime].max_inflight_connections` (default 65 536, configurable).
2. Replace `tokio::spawn(...)` with
   ```rust
   let permit = match semaphore.clone().try_acquire_owned() {
       Ok(p) => p,
       Err(_) => { metrics.shed_total.inc(); drop(client_stream); continue; }
   };
   tracker.spawn(async move {
       let _permit = permit; // dropped on task exit
       /* … existing body … */
   });
   ```
3. Expose `listener_inflight_gauge` (rel owns the metric definition).

Cross-ref: synthesis T7; rel→code #1; sec S-4 (per-IP cap is the second axis).

---

### CODE-2-06 — Accept loop tight-loops on EMFILE/ENFILE; no exponential backoff
Severity: critical
Blocking-for-prod: yes
Status:   Verified-Fixed(f07cf44)   <!-- ebpf round-5: classify_accept_error at main.rs:1879 + next_accept_backoff at :1895. Doubling sequence 10ms→1s with ±25% jitter, .max(1) floor prevents collapse-to-zero, saturating_mul prevents overflow. accept_errors_total{kind} registered. test_emfile_no_busy_loop validates 20-iter sequence. Bypass attempts on overflow + negative-jitter + fatal-wedge all failed. -->
Location: `crates/lb/src/main.rs:1100–1106`

Description / Impact:
```rust
let (client_stream, client_addr) = match listener.accept().await {
    Ok(conn) => conn,
    Err(e) => {
        tracing::warn!("accept error: {e}");
        continue;
    }
};
```
On `EMFILE` (per-process fd exhaustion) or `ENFILE` (system-wide fd
exhaustion), `accept(2)` returns immediately with the error and the
loop body busy-spins at 100 % CPU on that worker thread, emitting
a `warn!` per iteration. This is the classic "accept-loop wedge"
that NGINX, HAProxy, and Envoy each have a documented mitigation for.

Worse: the `warn!` log line is unbounded — tracing's default subscriber
will allocate a `String` per event, accelerating heap fragmentation
exactly when the process is fd-starved.

Reproduction: `ulimit -n 64` + drive 64 simultaneous connections; the
65th accept returns `EMFILE` and the worker pegs.

Recommendation:
```rust
let mut backoff = Duration::from_millis(5);
loop {
    match listener.accept().await {
        Ok(conn) => { backoff = Duration::from_millis(5); /* reset */ … }
        Err(e) => match e.raw_os_error() {
            Some(libc::EMFILE) | Some(libc::ENFILE) => {
                tracing::warn!(error = %e, backoff_ms = backoff.as_millis(), "fd exhausted, backing off");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(Duration::from_secs(1));
                continue;
            }
            Some(libc::ECONNABORTED) | Some(libc::EINTR) => continue,
            _ => {
                tracing::error!(error = %e, "fatal accept error");
                return Err(e.into());
            }
        }
    }
}
```
Also: `tracing::warn!` should be rate-limited (use the
`tracing-subscriber::filter` or an explicit token bucket — `rel` owns
the observability finding).

Cross-ref: rel→code handoff #2; synthesis T7.

---

### CODE-2-07 — `Pod` constructor zero-init invariant unenforced for FlowKey/BackendEntry padding bytes
Severity: high
Blocking-for-prod: yes
Status:   Open   <!-- ebpf round-5: confirmed Open. e3ac961 in the audit assignment was actually CODE-2-14 (BackendState binding), not CODE-2-07. No FlowKey::new / BackendEntry::new constructors landed; no `const _: () = assert!(size_of::<FlowKey>() == 16)` size assertions. All literal sites still write explicit `pad: [0; 3]` / `pad: 0` so current behaviour is correct; the future-contributor risk the finding flagged is unmitigated. -->
Location:
  - `crates/lb-l4-xdp/src/loader.rs:33–53` (`FlowKey { pad: [u8; 3] }`)
  - `crates/lb-l4-xdp/src/loader.rs:55–76` (`BackendEntry { pad: u16 }`)
  - `crates/lb-l4-xdp/src/loader.rs:78–97` (`FlowKeyV6 { pad: [u8; 3] }`)
  - `crates/lb-l4-xdp/src/loader.rs:99–120` (`BackendEntryV6 { pad: u16 }`)
  - Construction sites in tests / sim:
    `crates/lb-l4-xdp/src/lib.rs:420, 448, 478, 485, 575, 598`
    `crates/lb-l4-xdp/src/sim.rs:131, 312`
    `crates/lb-l4-xdp/ebpf/src/main.rs:427` (eBPF side, same struct shape)

Description / Impact:
All four `unsafe impl aya::Pod` types are `#[repr(C)]` structs whose
binary layout is the lookup key the kernel uses for `BPF_MAP_LOOKUP_ELEM`.
The kernel compares the *entire* key buffer byte-for-byte. The Rust
side has explicit `pad` fields; the BPF side has matching `pad`
arrays. As long as both sides write *the same byte values* to `pad`,
lookups succeed.

Today there are **no constructors** that force `pad` to zero. Every
construction site uses a struct literal:
```rust
let key = FlowKey { src_addr: …, dst_addr: …, src_port: …, dst_port: …,
                    protocol: 6, pad: [0; 3] };
```
This is *fine right now* because every visible site explicitly writes
`pad: [0; 3]` or `pad: 0`. But:

1. The `Pod` marker invites callers to use `MaybeUninit::<FlowKey>::zeroed().assume_init()`
   or `mem::zeroed()`; that *is* safe under `Pod`, but if anyone in
   the future uses `MaybeUninit::uninit().assume_init()` instead, the
   `pad` bytes become indeterminate and every lookup misses.
2. `BackendEntry` and `BackendEntryV6` have a `pad: u16` *between*
   `backend_port` and `backend_mac`. A future field reorder by an
   inattentive contributor could quietly shift `pad` next to a
   different field, breaking ABI with the eBPF struct without any
   compile-time signal.
3. The eBPF side at `ebpf/src/main.rs:427` writes its `FlowKey` in a
   loop over packet bytes; if the userspace side ever lands a non-zero
   `pad` write (e.g. someone passes the protocol family there as a
   "free byte"), the eBPF and userspace views diverge — half the
   inserts go to a different bucket than half the lookups.

Sec **S-9** flags the same root cause from a security angle; ebpf
flagged the same in its cross-review to-`code` #3.

Recommendation:
1. Add a private constructor per Pod type that zero-initialises padding:
   ```rust
   impl FlowKey {
       pub fn new(src_addr: u32, dst_addr: u32, src_port: u16, dst_port: u16, protocol: u8) -> Self {
           Self { src_addr, dst_addr, src_port, dst_port, protocol, pad: [0; 3] }
       }
   }
   ```
   Same for `FlowKeyV6`, `BackendEntry`, `BackendEntryV6`.
2. Replace every struct-literal site with the constructor.
3. Add a *compile-time* size assertion that matches the eBPF side
   byte-for-byte:
   ```rust
   const _: () = assert!(core::mem::size_of::<FlowKey>() == 16);
   const _: () = assert!(core::mem::size_of::<BackendEntry>() == 24);
   ```
4. Optional: derive a runtime test that allocates a `FlowKey` via
   `MaybeUninit::uninit()`, writes only the non-pad fields, inserts
   into the aya `HashMap`, then queries with a fresh
   `FlowKey::new(...)` constructed via the same logical key. The
   query must *miss* — if it hits, the kernel side is hashing
   ignoring padding (it isn't, but the assertion documents the
   invariant).

Cross-ref: synthesis T5(b); sec S-9; ebpf cross-review to-`code` #3.

---

### CODE-2-08 — Per-CID actor leaks DashMap entries on panic; reaper bound is "until next packet on the dead CID"
Severity: high
Blocking-for-prod: no
Status:   Verified-Fixed(17dd4eb)   <!-- ebpf round-5: CidEntryGuard at crates/lb-quic/src/cleanup_guard.rs:36 with Drop impl at :64. Guard wraps both router_key + header_dcid_key and is moved into the spawn closure at router.rs:382 — Drop runs on normal exit, await cancel, panic unwind, or under panic=abort the process dies first. Bypass attempts on double-panic + bypass-spawn + key-aliasing all failed. -->
Location: `crates/lb-quic/src/router.rs:373–379`

Description / Impact:
```rust
let connections_for_cleanup = Arc::clone(connections);
tokio::spawn(async move {
    let _ = Box::pin(run_actor(actor)).await;
    // Best-effort cleanup once the actor exits.
    connections_for_cleanup.remove(&router_key);
    connections_for_cleanup.remove(&header_dcid_key);
});
```
If `run_actor` panics, the `await` unwinds and the two `remove` calls
never run. The actor's `mpsc::Sender` is dropped (so subsequent
`try_send` returns `Closed`), and the `dispatch_to_actor` helper at
`router.rs:252–254` then reaps the entry. Reaping requires *another
inbound packet* targeted at the dead CID — for a CID that never sees
a second packet (idle connection, NAT rebind, attacker-injected DCID
that doesn't repeat), the entries are pinned for the lifetime of the
router. Two entries leak per panicking actor.

Bound on the leak: at most `2 * max_connections` entries by design
(cap-check at `router.rs:309`). Once at the cap, *new* legitimate
connections are dropped (lb-quic refuses to spawn — see the
`router_drops_initial_when_cap_reached` test at `:430`). So the failure
mode is **denial of service via panic exhaustion**, not unbounded
memory growth: an adversary who can reliably panic the actor caps the
router at `2 * N` rotted entries and refuses all new connections until
the router restarts.

Today `run_actor` is reasonably panic-free, but `lb-quic` calls into
`quiche::Connection` (FFI to BoringSSL) and the H3 bridge (which calls
into hyper) — both have non-zero panic surface, especially under
malformed-input fuzzing.

The comment at `router.rs:370–372` calls this "acceptable"; it is
acceptable for short-lived actor crashes but *not* if the actor
panics in a way that takes the channel state with it. With
`panic = "abort"` (CODE-2-02) this would degrade to "process dies on
panic", which is preferable to the DoS.

Recommendation:
1. Wrap the actor body in `AssertUnwindSafe` + `catch_unwind` so the
   cleanup always runs:
   ```rust
   tokio::spawn(async move {
       let result = std::panic::AssertUnwindSafe(Box::pin(run_actor(actor)))
           .catch_unwind()
           .await;
       if let Err(panic) = result {
           tracing::error!(?router_key, ?panic, "quic actor panicked");
       }
       connections_for_cleanup.remove(&router_key);
       connections_for_cleanup.remove(&header_dcid_key);
   });
   ```
   With `panic = "abort"` per CODE-2-02 this is moot (the whole
   process dies); without it, the dashmap is freed.
2. Add a periodic reaper sweep (every 60 s) that drops entries whose
   `mpsc::Sender` returns `Err(_)` on a zero-byte `try_send`. This
   handles the "actor exits cleanly but cleanup tokio::spawn dies"
   edge case.
3. Emit a `quic_actor_panics_total` counter so the operator sees the
   leak rate.

Cross-ref: sec to-`code` Q-1-09 (sec asked code to confirm
acceptability — code says: acceptable *if* CODE-2-02 lands, otherwise
high).

---

### CODE-2-09 — `pool.acquire()` runs blocking `connect(2)` on tokio's global blocking pool; cold-path stall on starved pool
Severity: high
Blocking-for-prod: no
Status:   Verified-Fixed(f07cf44, fc42d60)   <!-- ebpf round-5: TcpPool::acquire_async at pool.rs:210 using tokio::net::TcpStream::connect under PoolConfig::connect_timeout. Static no_spawn_blocking_in_pool_dial_path test at :877. Live timeout test acquire_async_timeout_fires at :845. All upstream dial sites migrated: lb-l7 h1/h2/grpc proxies + lb-io http2_pool + lb-quic h3_bridge. Cancellable (drop-cancels native connect). connect_timeout-from-runtime plumb deferred 1-line follow-up — behaviour unchanged.   --><!-- Round-4 follow-on lands the lb-io / lb-l7 pool rework: `TcpPool::acquire_async(addr)` uses `tokio::net::TcpStream::connect` under a `PoolConfig::connect_timeout` deadline (defaulting to 5 000 ms, matching `runtime.connect_timeout_ms`). New sockopts helper `apply_connected_tokio(&tokio::net::TcpStream, &BackendSockOpts)` keeps the socket on the tokio reactor instead of round-tripping through `set_nonblocking`/`from_std`. Every upstream dial site now calls `acquire_async`: `lb-l7::h1_proxy::proxy_request`, `lb-l7::h1_proxy::run_h1_ws_upgrade_task`, `lb-l7::h2_proxy::proxy_request`, `lb-l7::h2_proxy::ws upgrade`, `lb-l7::grpc_proxy::proxy`, `lb-io::http2_pool::dial_and_handshake`, `lb-quic::h3_bridge::h3_to_h1_roundtrip`. Static-source proof `tests::no_spawn_blocking_in_pool_dial_path` asserts no `spawn_blocking` calls live in `pool.rs`. `tests::acquire_async_timeout_fires` proves the deadline kicks in by dialing TEST-NET-1 (192.0.2.1:1) and asserting <2 s wall. `acquire` retained as a sync entry point for non-tokio embedders / unit tests but marked deprecated for production callers (doc-string + module doc). The `PoolConfig::connect_timeout` plumb from `runtime.connect_timeout_ms` through `lb/src/main.rs::TcpPool::new` is a 1-line follow-up call-site change (currently defaults to 5 000 ms which matches the config default, so behaviour is unchanged); deferred so this commit does not race with `rel-r4w2c-cert`'s main.rs cert-rotation work. The deferred plumb is the only remaining gap. -->
Location:
  - `crates/lb-io/src/pool.rs:158–168` (doc-comment admits blocking)
  - `crates/lb-io/src/pool.rs:222–227` (`fn dial_fresh` calls `std::net::TcpStream::connect`)
  - `crates/lb-io/src/lib.rs:202–208` (`Runtime::connect` → `connect_socket` → `std::net::TcpStream::connect`)
  - Callers using `spawn_blocking`: `crates/lb/src/main.rs:1238`,
    `crates/lb-l7/src/h1_proxy.rs:419, 659`,
    `crates/lb-l7/src/h2_proxy.rs:410, 472`,
    `crates/lb-l7/src/grpc_proxy.rs:214`,
    `crates/lb-io/src/http2_pool.rs:261`,
    `crates/lb-quic/src/h3_bridge.rs:275`

Description / Impact:
Every proxy direction acquires a backend connection via
`tokio::task::spawn_blocking(move || pool.acquire(addr))`. The
acquire path:
1. Tries the hot LRU (lock-free fast path).
2. On miss, calls `Runtime::connect` which calls
   `std::net::TcpStream::connect` (blocking syscall).
3. Applies setsockopts.
4. Returns the std-stream (caller converts to tokio).

`tokio::task::spawn_blocking` uses tokio's **global** blocking-pool
(default 512 threads, configurable via `Builder::max_blocking_threads`).
This pool is shared with `lb-io::dns::DnsResolver` (which also runs
`spawn_blocking` for synchronous getaddrinfo at `dns.rs:283`) and
hyper internals. Three observed risks:

1. **Pool exhaustion under burst.** A 1000-RPS cold-pool burst spawns
   1000 blocking tasks, each holding a thread for one TCP RTT
   (~1–50 ms) plus TLS handshake (~100 ms on a cold pool). At
   500 ms p99 connect, 1000 RPS saturates 500 threads — within
   default budget but two callsite bursts (proxy + DNS) coupled to the
   *same* incident easily breach it.
2. **Latency tail from queueing.** Once the blocking pool fills,
   subsequent `spawn_blocking` calls block on the pool's internal
   queue. The blocking happens *on the tokio worker* until the
   future is polled the next time — so the calling worker is fine,
   but the *task* sits in `Pending` until a blocking thread frees up.
   At p99 this becomes the dominant latency component.
3. **No cancellation.** `spawn_blocking` futures cannot be cancelled
   — the blocking thread runs to completion regardless of whether
   the awaiting task is dropped. Combined with CODE-2-03 (no drain),
   SIGTERM cannot cut connect attempts mid-flight.

`rel` flagged this in F-22-adjacent area; team-lead asked `code` to
evaluate the alternative.

Analysis of alternatives:
| Option | Cancel-able | Setsockopt timing | Pool isolation | Cost |
|---|---|---|---|---|
| `tokio::net::TcpStream::connect` | yes | post-connect via `set_*` on the std-fd or socket2 wrappers | shares tokio reactor (preferred) | medium refactor — `Runtime::connect_socket` returns std-stream today |
| Dedicated bounded blocking executor | no | unchanged | isolated from DNS/hyper | small change, doesn't fix cancel |
| Current (global blocking pool) | no | works | shared | zero change |

Recommendation:
1. **Preferred** — make `Runtime::connect` async, using
   `tokio::net::TcpStream::connect` for the syscall and applying
   sockopts via `socket2::Socket::from(stream.into_std()?)` then
   `Socket::set_*`. This eliminates `spawn_blocking` from the hot
   path, gives cancellation, and removes the DNS-vs-connect pool
   contention. It is a medium refactor: `pool.acquire` becomes
   `async fn`, every caller drops the `spawn_blocking` wrapper.
2. **Acceptable interim** — keep blocking but use a dedicated bounded
   blocking pool (`tokio::runtime::Builder::new_multi_thread()` with
   `max_blocking_threads(N)` where N is sized by listener; run
   `pool.acquire` via `runtime.spawn_blocking_on(...)`). Doesn't fix
   cancellation; does fix exhaustion coupling.
3. **Wrong** — keep current and document. Pingora-parity argument
   does not apply: Pingora has its own work-stealing scheduler tuned
   for this; we're sharing tokio's general-purpose pool.

This finding does **not** block prod on its own (operationally
mitigated by oversized `max_blocking_threads`), but it underpins
CODE-2-03's drain story. Resolution is owed by Round 3 plan.

Cross-ref: rel→code handoff (analysis-owed); synthesis T7.

---

### CODE-2-10 — XDP attach: `XdpLinkId` drop semantics verified against aya 0.13; recommend integration test to prevent silent regression
Severity: info
Blocking-for-prod: no
Status:   Verified-Fixed(854ebdb)   <!-- ebpf round-5 (self-batch): test file at crates/lb-l4-xdp/tests/xdp_link_id_drop_safe.rs present. Compile-time signature-assertion (loader_attach_signature_drops_xdplinkid_silently) catches aya signature changes; #[ignore]'d runtime test exercises real attach/detach under CAP_BPF. XdpLoader Drop ordering across drain (CODE-2-03) confirmed safe. -->
Location:
  - `crates/lb-l4-xdp/src/loader.rs:273–276`
  - aya 0.13.1 source: `aya-0.13.1/src/programs/xdp.rs:108–162` (`attach` → `attach_to_if_index` → `self.data.links.insert(XdpLink::new(...))`)

Description / Impact:
Reviewed the aya 0.13.1 source path. The loader comment is *correct*:

```rust
// attach() returns XdpLinkId; we drop it intentionally — aya keeps the
// link alive as long as the Xdp handle exists inside self.ebpf.
let _link_id = xdp.attach(ifname, mode.to_flags())?;
```

Behaviour in aya 0.13.1:
- `Xdp::attach_to_if_index` constructs an `XdpLink` (an
  `XdpLinkInner::FdLink(...)` on kernel ≥ 5.9), stores it inside
  `self.data.links: ProgramLinkInfo<XdpLink>`, and returns only the
  *index* `XdpLinkId` as a handle for the caller to *later detach*.
- Dropping `XdpLinkId` does NOT remove the `XdpLink` from
  `self.data.links` (the inner storage is keyed by the id).
- The owning `Xdp` struct lives inside `EbpfManager.ebpf:
  aya::Ebpf` (the loader holds it in `self.ebpf`).
- On `Drop` of the `Xdp`, aya calls `detach` on each managed link
  (comment at aya 0.13.1:182).

So dropping `_link_id` is safe; the attach survives as long as `self.ebpf`
lives. Good.

**However** this is a behaviour we depend on across aya versions and
the comment is the only documentation. A future aya bump (e.g. 0.14)
could change semantics silently and the attach would detach on
construction. Recommendation is operational, not correctness:

Recommendation:
1. Add an integration test under `crates/lb-l4-xdp/tests/` (gated by
   `#[cfg(target_os = "linux")]` and a `CAP_BPF` probe with skip):
   - Build the test ELF, call `XdpLoader::attach(...)`, drop the
     `XdpLinkId`, sleep 100 ms, query `/sys/kernel/debug/tracing/...`
     or use `ip link show <ifname>` to verify the XDP program is
     still attached. If the probe returns no program, fail loudly
     with a message naming the aya version expected.
2. Pin `aya = "0.13"` (currently `aya = "0.13"`; fine). On bump,
   require a developer to acknowledge the change in
   `docs/decisions/`.
3. Comment in the loader cites the *exact* aya source lines we relied
   on (e.g. `aya-0.13.1/src/programs/xdp.rs:160 — self.data.links.insert(...)`).

Cross-ref: synthesis T5(a). Severity is `info` because the comment
matches reality today; the recommendation is a regression guard.

---

### CODE-2-11 — Zero proptest / loom / miri usage across the workspace
Severity: high
Blocking-for-prod: no
Status:   Verified-Fixed-Partial(560c1c2)   <!-- ebpf round-5: proptest harnesses present at lb-h1/lb-h2/lb-h3/lb-quic; loom harness at lb-balancer/tests/loom_atomic_counter.rs (#![cfg(loom)], Release/Acquire two-thread model). miri-runnable lb-io test not located in worktree — Wave-2 follow-on per audit margin note. -->
Location: workspace-wide. Grep summary (from Round 1 inventory §4.3):
  - `proptest` references in `crates/`: 0
  - `loom` references in `crates/`: 0
  - `cfg(miri)` guards in `Cargo.toml` files: 0

Description / Impact:
Property-based and concurrency-model testing are *absent*. Every
parser, codec, and concurrent data structure in the workspace is
tested with example-based unit tests + tiny fuzz corpora (5-7 inputs
per fuzz target — Round 1 §4.4). The risk surface this leaves
uncovered:

1. **Parsers without proptest:**
   - `lb-h1` request/status-line + chunked decoder
   - `lb-h2` HPACK / SETTINGS / frame parser
   - `lb-h3` QPACK / varint / frame parser
   - `lb-grpc` frame parser, deadline parser
   - `lb-quic` retry-token decode, header parse
   - `lb-compression` decompressor (bomb-cap interaction)
   - `lb-config` TOML schema (already partially tested via fixtures)

2. **Concurrent code without loom:**
   - `lb-core::backend.rs` saturating-decrement CAS loops at lines
     98–110 and 125–137 (loom would expose the
     `(Relaxed, Relaxed) compare_exchange_weak` reorder hazard)
   - `lb-io::pool.rs` `total.load >= total_max` gate at line 372
     (classic check-then-act under Relaxed; loom would catch overshoot)
   - `lb-io::quic_pool.rs` evict-then-replace at line 516 (transient
     `total` decrement observable)
   - `lb-h2` Rapid-Reset detector (counter-vs-threshold race)
   - `lb-quic::router.rs` dashmap insert/remove ordering

3. **`unsafe` blocks without miri:**
   - `lb-io::ring.rs` (17 unsafe blocks around io_uring SQE submission)
   - `lb-l4-xdp::loader.rs` (`unsafe impl Pod` × 4 — miri can't run
     under linux io_uring but *can* validate the Pod-conversion
     round-trip)
   - The `aya::Pod` invariant assertion (memcpy round-trip of
     FlowKey/BackendEntry through a `[u8]`) is exactly miri's domain.

Recommendation:
1. Add `proptest = "1"` to dev-dependencies and write generators for
   each parser:
   - `lb-h1`: request-line / chunked-frame strategies.
   - `lb-h2`: frame-shape strategy with HPACK indexing.
   - `lb-h3`: varint + QPACK strategy.
   - `lb-compression`: well-formed (header, body) tuple strategy
     bounded by bomb-cap.
   - `lb-config`: schema-driven generator using `arbitrary` over the
     TOML AST.
   Each generator runs a *round-trip* assertion (decode → encode →
   decode == identity) plus a "no panic, no unwrap_used" assertion.

2. Add `loom = "0.7"` to dev-dependencies of the three crates with
   non-trivial lock-free patterns:
   - `lb-core`: model the BackendState fetch_add/fetch_sub pair as a
     2-thread loom test.
   - `lb-io::pool`: model total-counter gate.
   - `lb-h2::security`: model Rapid-Reset accumulator vs reset-emitter.
   Loom tests live in `tests/loom_*.rs` behind
   `#[cfg(loom)] #[test]` with `cargo test --cfg loom`.

3. Add a `cargo miri test -p lb-l4-xdp -p lb-io --lib` CI job. miri
   doesn't support io_uring syscalls so the ring tests will mostly
   be skipped; the goal is to catch the *Pod conversion* and the
   `addr_of!` reads in the userspace loader before they hit kernel
   memory.

`rel` will own the CI integration. This finding tracks the source-side
additions.

Cross-ref: synthesis T9; rel CI matrix.

---

### CODE-2-12 — `arc-swap` workspace dependency declared but unused; remove to shrink build graph
Severity: low
Blocking-for-prod: no
Status:   Verified-Fixed(f93c582)   <!-- ebpf round-5: arc-swap removed from [workspace.dependencies] (comment at Cargo.toml:67–70 documents the removal). Re-added at crate level in crates/lb-security/Cargo.toml:36 for REL-2-03 cert rotation (commit 334b69a) — matches the original recommendation. -->
Location:
  - `Cargo.toml:61` (`arc-swap = "1"` in `[workspace.dependencies]`)
  - No `arc_swap::` references anywhere in `crates/` (Round 1 inventory §3.4; manual grep)

Description / Impact:
The README and rel's H2 / sec's §5.1 findings both reference an
`ArcSwap<TlsStore>` hot-swap as the mechanism for TLS cert rotation.
The dependency line exists. Zero call sites. `cargo machete` does not
flag this because it scans per-crate manifests, not workspace-level
declarations.

Two harms: (a) the declaration creates the *impression* of hot-swap
support (rel H2), (b) it adds a transitive build edge that buys
nothing today.

Recommendation: drop the line. When the *real* hot-swap is implemented
(per rel H2 / sec §5.1), re-add it at the crate level (probably in
`lb-security` or `lb`).

Cross-ref: rel H2; sec §5.1.

---

### CODE-2-13 — `lb` binary declares unused `lb-controlplane` and `lb-health` deps; confirms control-plane + active-health wiring is missing
Severity: medium
Blocking-for-prod: no
Status:   Verified-Fixed-Partial(1fe53ed)   <!-- ebpf round-5: FileBackend + ConfigManager constructed at main.rs:1357-1371 (rejects on validation failure); per-backend HealthChecker::new(3,2) seed at :1418-1421. Both deps now referenced — cargo machete output flips. Active-probe loop + picker filter wire-in deferred to REL-2-05 / Wave-2 per audit margin note. -->
Location:
  - `cargo machete` output in `audit/code/round-1-machete.txt`
  - `crates/lb/Cargo.toml` (declares `lb-controlplane`, `lb-health` but no `use lb_controlplane::` or `use lb_health::` in `crates/lb/src/main.rs`)
  - `crates/lb-health/src/lib.rs` (HealthChecker has no call sites in the workspace — rel→code handoff #7)

Description / Impact:
machete flagged both crates as suspect-unused; manual grep confirms.
The binary advertises (via docs / runbook) live config reload and
active health-checking; the implementations are present in their
respective crates but never *called*. This is the corollary of T9 +
the rel→code handoff: the `expressgateway` binary today is a
listener-only proxy; control plane is mocked, health is passive.

Recommendation:
1. Either wire these in (Round 3 plan owns the wiring decision):
   - `lb-controlplane::ConfigManager` reads file changes on SIGHUP
     and rolls back on validation failure. Integration point is
     after `parse_config` in `async_main`.
   - `lb-health::HealthChecker` runs a per-cluster tokio task that
     calls each backend's health endpoint and reports state into
     `lb-core::BackendState`.
2. Or remove the deps and stop advertising the features in the
   README until the wiring lands.

Cross-ref: rel→code #4, #7; synthesis §C; machete output.

---

### CODE-2-14 — `lb-balancer` and `lb-core` carry duplicate backend-counter fields; risk of divergence between scheduler and gauge
Severity: medium
Blocking-for-prod: no
Status:   Verified-Fixed-Partial(e3ac961)   <!-- ebpf round-5: lb-balancer adds lb-core dep + Backend.state: Option<Arc<BackendState>> field. with_state constructor + sync_from_state() (three Acquire loads) refresh the snapshot before each pick. AcqRel ↔ Acquire pairing with CODE-2-04 confirmed. Named race-test tests/balancer_counter_sync.rs is ABSENT in worktree — only loom_atomic_counter.rs exists; the loom model covers the underlying publication, which is stricter. -->
Location:
  - `crates/lb-core/src/backend.rs` (`BackendState` atomics: `active_connections`, `active_requests`, latency EWMA)
  - `crates/lb-balancer` (its own `Backend` struct with overlapping `u64` fields per algorithm — Round 1 inventory §1.2 / §4.5)
  - `crates/lb-l7/src/upstream.rs` (yet a third `RoundRobinUpstreams` not consuming `lb-core::BackendState`)

Description / Impact:
There are three independent representations of "current load on a
backend" in the workspace:
1. `lb-core::BackendState` — atomic counters consumed by metrics.
2. `lb-balancer::Backend` — `u64` counters consumed by the scheduler.
3. `lb-l7::upstream::RoundRobinUpstreams` — neither.

When the H1/H2 proxy picks a backend via (3) and increments the gauge
via (1), the *scheduler* in (2) sees neither. Result: P2C and
least-connections scheduling decisions are made against a stale view.
The metric on the admin endpoint reflects the live truth (1), so an
operator sees "evenly distributed" while the scheduler is asymmetric.

Recommendation:
1. Adopt `lb-core::BackendState` as the single source of truth.
2. `lb-balancer` becomes a thin scheduler that takes
   `&[Arc<BackendState>]` and uses `state.active_connections.load(Acquire)`
   (see CODE-2-04 for the ordering requirement).
3. `lb-l7::upstream` either re-exports `lb-balancer` or is deleted
   in favour of consuming `lb-balancer` directly.

This is a scope question (`proto` Q-CODE-1-07 also flagged): a
proto-side decision on whether `lb-balancer` is the canonical
scheduler or a legacy crate. Lead synthesis defers to Round 3.

Cross-ref: synthesis §C; proto Q-CODE-1-07.

---

### CODE-2-15 — `lb-h1` crate has no in-workspace consumer outside the fuzz target
Severity: low
Blocking-for-prod: no
Status:   Verified-Fixed(f93c582)   <!-- ebpf round-5: crates/lb-compression directory does not exist; Cargo.toml carries the explanatory comments at :26 + :146. lb-h1 keep-or-delete remains open for a future round per audit body. -->

Location:
  - `crates/lb-h1/` (full HTTP/1.1 codec)
  - Only consumer is `fuzz/fuzz_targets/h1_parser.rs` (out of workspace)
  - `lb-l7::h1_proxy.rs` uses `hyper`'s parser, not `lb-h1`'s.

Description / Impact:
The crate is a complete HTTP/1.1 codec (`#![deny(clippy::unwrap_used,
…, clippy::indexing_slicing)]` — excellent lint posture). Today it's
exercised only by fuzzing. Either it's intended as a cross-verification
parser (run hyper-parsed and lb-h1-parsed in shadow, compare results,
alert on divergence — a *good* idea for a security-critical proxy)
or it's dead.

Recommendation:
1. **If shadow-verify** — wire it into `h1_proxy.rs` as a parallel
   parse: hyper drives the wire, `lb-h1::Parser` consumes the same
   bytes and emits a metric on parser divergence
   (`http_parser_divergence_total{kind="cl_te"}` etc.). This is the
   cleanest defence-in-depth against hyper-vs-attacker parser
   disagreement (request smuggling).
2. **If dead** — delete the crate. The fuzz target stays useful only
   if the parser is reachable from production.

Asked `proto` (Q-CODE-1-06) and `sec` (the smuggle-detector wiring) for
their preference. `proto` is the natural owner of the shadow-parse
design.

Cross-ref: proto Q-CODE-1-06; sec S-1.

---

### CODE-2-04 — Every atomic uses `Ordering::Relaxed` (50 sites, 0 Acq/Rel/SeqCst); enforcement-gating counters need Acquire/Release
Severity: high
Blocking-for-prod: yes
Status:   Verified-Fixed-Partial(c4c27da)   <!-- ebpf round-5: scripts/ci/atomic-lint.sh present + docs/decisions/atomics.md policy doc; lb-core::BackendState::inc_connections converted to AcqRel at backend.rs:111. Live `bash scripts/ci/atomic-lint.sh` exits 1 with one outstanding unannotated S-site (crates/lb-security/src/ticket.rs:900 — ticket-ID monotonic counter, stats-only). Wave-2 appendix-B sweep + ticket.rs annotation outstanding under same ID. BPF-map ordering parity unaffected (kernel-side syscall sequencing). -->
Location: 50 atomic sites. See appendix below.

Description / Impact:
Round 1 inventory §2.5 enumerated every atomic in the workspace. The
result: **50 ordering tokens, all `Relaxed`. Zero `Acquire`,
`Release`, `AcqRel`, or `SeqCst`.** No `compare_exchange`,
`fetch_update`, or `store` carries any memory ordering beyond
"single-location atomicity".

This is fine for *pure counters* that are only read for observability
(metrics gauges). It is **wrong** when a counter is read to *make a
decision*. Three categories of decision-making counters exist:

1. **Enforcement gates** — sites where a `.load()` decides whether
   to admit or reject a connection / request. Example:
   `lb-io::pool::total.load(Relaxed) >= total_max` at
   `pool.rs:372`. Under Relaxed, the increment by another thread
   can be reordered relative to whatever state change the increment
   was meant to publish; the gate may admit a connection whose
   prior state is not yet visible. Needs **Acquire on load, Release
   on the matching `fetch_add`**.
2. **Capacity / threshold counters** — `lb-h2::security` Rapid-Reset
   accumulator (per sec S-?), per-IP rate-limit buckets (when those
   land per CODE-2-01 / sec S-4), zero-RTT replay-ring counters. All
   need Acquire/Release pairing because the gate decision must
   observe *all* writes that happened-before the increment, including
   the bytes being counted.
3. **Lifecycle flags** — there are none today (no `AtomicBool` /
   `AtomicUsize` shutdown signal — that's the CancellationToken's
   job in CODE-2-03). If one is introduced, it must be `SeqCst` on
   store + `Acquire` on load.

`sec` agreed to hand a per-site requirements list during cross-review
(synthesis §A, T6). They have not produced one yet (round 2 just
opened); I am proposing the classification below based on source
review. **`sec` to confirm or override per detector in Round 2
cross-review.**

Reproduction (theoretical):
For the `pool.rs:372` gate, construct a loom test with two threads:
T1 increments `total` and pushes a new connection into the per-peer
deque; T2 loads `total` and reads the deque. Loom will model an
execution where T2 sees `total = N` but the deque has fewer than `N`
entries because the `total.fetch_add(1, Relaxed)` was reordered
*before* the deque push. Today's tests pass because x86 is strongly
ordered; on aarch64 / ARMv8 this becomes observable.

Recommendation:
1. Audit + reclassify per the appendix. Move every "enforcement gate"
   to Acquire on load + Release on the corresponding fetch_add /
   compare_exchange / store. Use AcqRel on read-modify-write where the
   RMW participates in *both* a release publication and an acquire
   dependency.
2. For loom coverage on the three highest-risk lock-free patterns
   (CODE-2-11 already opens this).
3. Document the ordering policy in `docs/decisions/atomics.md` so the
   choice survives future churn.

Cross-ref: synthesis T6; sec (per-detector list pending).

#### Appendix A — per-site classification

Categories: **S** = stats-only (Relaxed OK); **G** = enforcement gate
(needs Acquire/Release pair); **R** = needs review by sec.

| File:line | Atomic | Op | Category | Reasoning |
|---|---|---|---|---|
| `lb-core/src/backend.rs:87` | `active_connections` | `fetch_add` | G | Read by scheduler (P2C/least-conn). Currently isolated via duplicate counters (CODE-2-14), but the intent is scheduling — needs Release. |
| `lb-core/src/backend.rs:92,98,104,105` | `active_connections` saturating-CAS | `compare_exchange_weak (Relaxed, Relaxed)` | G | CAS loop; success ordering should be `Release`, failure `Relaxed`. |
| `lb-core/src/backend.rs:116,121,126,132,133` | `active_requests` | same | G | same |
| `lb-core/src/backend.rs:144,149,162,163,164` | latency EWMA | `fetch_add` / `load` | S | EWMA is a monotonic estimator, used only by metrics. Relaxed OK. |
| `lb/src/main.rs:1127` | `active_connections` | `fetch_add` | S | Per-connection gauge, only consumed by metrics endpoint. Relaxed OK *iff* nothing else reads it; check CODE-2-05's new semaphore doesn't repoint this. |
| `lb/src/main.rs:1212` | `active_connections` | `fetch_sub` | S | same |
| `lb-io/src/pool.rs:113` | `idle_count` | `fetch_add` | S | gauge |
| `lb-io/src/pool.rs:141` | `idle_count` | `load` | S | gauge |
| `lb-io/src/pool.rs:185` | `idle_count` | `fetch_sub` | S | gauge |
| `lb-io/src/pool.rs:372` | `total` | `load` | **G** | Capacity gate — `Acquire` on load required. |
| `lb-io/src/pool.rs:389` | `total` | `fetch_add` | **G** | Matching `Release`. |
| `lb-io/src/pool.rs:395` | per-peer counter | `fetch_add` | G | same logic, scoped to peer. |
| `lb-io/src/pool.rs:455` | total `fetch_sub` on evict | `fetch_sub` | G | Release-ordered with the queue removal that precedes it. |
| `lb-io/src/quic_pool.rs:184,217,230,236,271,283` | varies | various | R | sec to confirm — quic_pool capacity gate has the same TOCTOU risk as `pool.rs:372`. |
| `lb-io/src/quic_pool.rs:403,405,499,516,518,566,587` | varies | various | R | evict-then-replace path: needs ordering review. |
| `lb-io/src/dns.rs:396,416,434,438,451,467,475,489,508,530,532` | DNS cache counters | varies | S | cache size + hit/miss counters; metrics-only. Relaxed OK. |

Per-detector items deferred to `sec`:
- `lb-h2::security::RapidResetDetector` — counter-vs-threshold gate.
  sec to confirm Acquire on load.
- `lb-h2::security::ContinuationFloodDetector` — same.
- `lb-h2::security::HpackBombDetector` — counter on accumulated bytes
  vs threshold.
- `lb-security::ZeroRttReplayGuard` — replay-ring write index vs probe
  read. Almost certainly needs SeqCst on the ring CAS.
- per-IP rate-limit bucket (when added per CODE-2-01) — needs Acquire
  on bucket load.

— `code`, Round 2.
