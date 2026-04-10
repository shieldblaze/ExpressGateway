# ExpressGateway Data Plane Architecture

## Overview

ExpressGateway's data plane is a fully async, non-blocking proxy engine built on
Tokio with a pluggable I/O backend (io_uring primary, epoll fallback) and an
optional XDP kernel-bypass fast path for L4 forwarding.

## Thread Model

```
                        ┌──────────────────────────────┐
                        │       Bootstrap / Main       │
                        │  (signal handling, config)    │
                        └──────────┬───────────────────┘
                                   │ spawn
        ┌──────────────────────────┼──────────────────────────┐
        ▼                          ▼                          ▼
  ┌───────────┐           ┌───────────────┐           ┌───────────┐
  │  Worker 0  │           │   Worker 1    │           │  Worker N  │
  │ (pinned    │           │  (pinned      │           │ (pinned    │
  │  to core)  │           │   to core)    │           │  to core)  │
  └─────┬──────┘           └──────┬────────┘           └─────┬──────┘
        │                         │                          │
        ▼                         ▼                          ▼
  SO_REUSEPORT             SO_REUSEPORT               SO_REUSEPORT
  Listener shard           Listener shard              Listener shard
```

- **worker_threads = num_cpus** (configurable, 0 = auto-detect via `std::thread::available_parallelism`)
- Each worker thread is pinned to a CPU core for cache locality
- `SO_REUSEPORT` distributes incoming connections across worker threads at the kernel level
- Named threads (`eg-worker-N`) for observability

## I/O Backend Architecture

### Backend Trait

```rust
enum IoBackendType { IoUring, Epoll, Auto }
```

Runtime detection in `crates/runtime/src/backend.rs`:
- Probes kernel version via `libc::uname(2)` (zero-allocation, no fork)
- Requires kernel >= 5.11 for io_uring (SQPOLL + multi-shot accept/recv)
- Falls back to epoll on older kernels or when io_uring unavailable
- Per-listener override via config: `listeners[].io_backend = "io_uring" | "epoll" | "auto"`

### io_uring Primary Path

- **SQPOLL mode** (`IORING_SETUP_SQPOLL`): kernel-side submission polling eliminates submit syscalls on steady state
- **Registered buffers** (`IORING_REGISTER_BUFFERS`): pre-registered buffer pool for DMA-capable zero-copy recv/send
- **Registered files** (`IORING_REGISTER_FILES`): fixed file descriptor table for hot connections
- **Multi-shot accept**: single SQE yields multiple CQEs, reducing accept syscall overhead
- **Multi-shot recv**: continuous receive on a socket without re-arming
- **Batched submissions**: queue multiple operations and submit in one syscall

### epoll Fallback Path

- Uses Tokio's built-in mio/epoll reactor
- Activated automatically when:
  - Kernel < 5.11
  - io_uring kernel module unavailable
  - Config override `runtime.backend = "epoll"`
- **Byte-for-byte identical** externally observable behavior

## Connection Lifecycle

```
Client                   ExpressGateway                      Backend
  │                           │                                 │
  │──── SYN ─────────────────▶│                                 │
  │                    [accept via SO_REUSEPORT]                │
  │◀─── SYN+ACK ────────────│                                 │
  │──── ACK ─────────────────▶│                                 │
  │                    [TLS handshake if configured]            │
  │                    [ALPN negotiation → protocol detect]     │
  │                    [protocol sniffing fallback]             │
  │                    [LB algorithm → select backend]          │
  │                           │──── connect ───────────────────▶│
  │                           │                   [pool reuse]  │
  │◀═══ bidirectional proxy ═▶│◀═══ bidirectional proxy ═══════▶│
  │                    [backpressure propagation]               │
  │                    [flow control (H2/H3)]                   │
  │──── FIN ─────────────────▶│                                 │
  │                    [graceful drain]                         │
  │                           │──── FIN ───────────────────────▶│
```

## Buffer Management

- All buffers use `bytes::Bytes` / `bytes::BytesMut` for zero-copy reference counting
- **Lock-free buffer pool** (`crates/pool/src/buffer.rs`):
  - `crossbeam::queue::ArrayQueue` (bounded MPMC)
  - `acquire()` returns recycled `BytesMut`, `release()` returns it
  - Oversized buffers (>4x capacity) replaced to prevent memory bloat
  - Pre-filled at startup via `prefill()`
  - Hit/miss stats for monitoring
- Zero allocations on steady-state forwarding path

## Backpressure

- `crates/runtime/src/backpressure.rs`: Global connection admission control
  - Atomic pending counter with CAS-based `drain()` (underflow-safe)
  - Configurable high/low watermarks
- Per-connection write watermarks in TCP proxy
- H2/H3 stream-level flow control windows propagated across the proxy
- No unbounded buffering anywhere in the pipeline

## Socket Options

Applied via `crates/runtime/src/socket.rs` and `crates/protocol-tcp/src/connection.rs`:
- `TCP_NODELAY` - disable Nagle's algorithm
- `TCP_QUICKACK` - disable delayed ACKs
- `TCP_KEEPALIVE` - with configurable idle/interval/count
- `TCP_FASTOPEN` - reduce connection latency
- `SO_REUSEPORT` - kernel-level load distribution
- Configurable send/recv buffer sizes

## XDP Fast Path

See `docs/xdp-ebpf.md` for full documentation.

When enabled, L4 traffic can bypass the entire userspace stack:

```
NIC → XDP program → [L4 LB decision] → direct forward (DSR)
         │
         └─ L7 / TLS required → pass to userspace
```

## Connection Pooling

`crates/pool/` provides per-backend, per-protocol connection pools:
- **H1 pool**: HTTP/1.1 keep-alive connections with idle timeout
- **H2 pool**: HTTP/2 multiplexed connections with stream tracking
- **TCP pool**: Raw TCP connections for L4 proxying
- **QUIC pool**: QUIC connections for H3 backends
- All pools support `drain_node()` / `drain_backend()` for graceful backend removal
- Idle connection eviction via background `PoolEvictor` task with graceful shutdown
