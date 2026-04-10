# Performance Optimization Report

## Zero-Allocation Hot Path

### Verified Zero-Alloc Techniques

| Technique | Crate | Implementation |
|-----------|-------|----------------|
| `ArcSwap` for config/backend lists | lb, config | Atomic pointer load instead of `RwLock` read |
| `Bytes`/`BytesMut` buffer reuse | pool, protocol-* | Reference-counted buffers, no copy |
| Lock-free buffer pool | pool | `crossbeam::queue::ArrayQueue` (bounded MPMC) |
| Iterator-based scans | lb | `filter().nth()` instead of `filter().collect()` (no Vec alloc) |
| Static `HeaderName` constants | protocol-grpc | Pre-parsed at compile time, zero-alloc lookup |
| `Bytes::from_static` | protocol-grpc | Health check response from static byte array |
| Stack-allocated encode buffer | proxy-protocol | `[u8; 108]` with `fmt::Write` adapter (no `format!()`) |
| Zero-alloc URI scanning | protocol-http | Byte scan instead of `to_ascii_lowercase()` |
| Inline hop-by-hop array | protocol-http | `[Option<HeaderName>; 8]` instead of `Vec` |
| DashMap for concurrent lookups | lb (sticky), security | Lock-free shard-based reads |

### Lock Elimination

| Before | After | Crate |
|--------|-------|-------|
| `RwLock<Vec<Arc<dyn Node>>>` on every `select()` | `ArcSwap` — single atomic load | lb (all 12 algorithms) |
| Two `RwLock`s for hash ring + nodes | Single `ArcSwap<RingSnapshot>` | lb (consistent hash) |
| `RwLock<HashMap>` for sticky sessions | `DashMap` — lock-free reads | lb (sticky) |
| Mutex for packet rate limiting | CAS-based token bucket | security |
| `Mutex` for session cache refresh | Double-check pattern | health (cache) |

### Atomic Operations

| Usage | Type | Ordering |
|-------|------|----------|
| Connection counter | `AtomicU64` | Relaxed (monotonic, not cross-thread synchronized) |
| Active stream counter | `AtomicU32` | Relaxed with CAS loop (underflow-safe) |
| Flow control window | `AtomicI64` | AcqRel (ensures visibility of consumed bytes) |
| Backpressure pending | `AtomicUsize` | CAS loop with saturating_sub |
| Health check failure counter | `AtomicU32` | Relaxed (advisory) |
| Circuit breaker state | `AtomicU8` | AcqRel (state machine transitions) |
| Rate limit counters | `AtomicU64` | Relaxed (approximate counting acceptable) |
| Buffer pool stats | `AtomicU64` | Relaxed (monitoring only) |

## Underflow Bug Fixes

Critical integer underflow bugs found and fixed (all would cause wraparound to `u64::MAX`):

| Location | Bug | Fix |
|----------|-----|-----|
| `core/node.rs:174` | `fetch_sub(1) - 1` at 0 | CAS loop saturating at zero |
| `runtime/backpressure.rs:76` | `fetch_sub` wraps at 0 | CAS loop with `saturating_sub` |
| `protocol-http/h2/proxy.rs:186` | `fetch_sub(1)` at 0 | CAS loop + error log |
| `protocol-h3/proxy.rs:39` | `fetch_sub(1) - 1` at 0 | CAS loop |
| `protocol-http/h2/flow_control.rs` | Window exceeds 2^31-1 | `MAX_FLOW_CONTROL_WINDOW` constant |

## I/O Optimization

### io_uring (Primary Path)
- SQPOLL: kernel-side poll thread eliminates submit syscalls
- Registered buffers: DMA-capable, avoids kernel copy
- Registered files: fixed fd table, avoids fd lookup per-op
- Multi-shot accept: single SQE → multiple accepts
- Multi-shot recv: continuous receive without re-arming
- Batched submissions: amortize syscall overhead

### Kernel Detection
- Uses `libc::uname(2)` directly instead of `Command::new("uname")` fork+exec
- Zero allocation, no child process

### Socket Tuning
- `TCP_NODELAY`: disable Nagle (latency-sensitive)
- `TCP_QUICKACK`: disable delayed ACKs
- `TCP_FASTOPEN`: reduce connection setup RTTs (qlen=256)
- `SO_REUSEPORT`: kernel-level connection distribution
- Send/recv buffer sizes: configurable (default 256KB)
- Backlog: configurable (default 50,000)

### Buffer Management
- Lock-free buffer pool: `crossbeam::queue::ArrayQueue`
- Pre-filled at startup (`prefill()`)
- Oversized buffer replacement (>4x capacity → new buffer)
- TCP relay buffer: 64KB (`BytesMut`) instead of 8KB stack array
- Hit/miss stats for pool utilization monitoring

## Connection Pooling

- Per-backend, per-protocol pools (H1, H2, TCP, QUIC)
- `drain_node()` / `drain_backend()` for graceful removal
- Background evictor with graceful shutdown
- Capped `VecDeque` capacity to prevent over-allocation

## Data Path Profile

Steady-state forwarding path (after connection established):

```
1. Read from client socket    — io_uring recv (zero-copy with registered buffer)
2. Protocol frame parsing     — zero-alloc (Bytes slice)
3. Header inspection          — inline, no allocation
4. LB selection (if needed)   — ArcSwap::load() (single atomic)
5. Write to backend socket    — io_uring send (zero-copy)
6. Read from backend          — io_uring recv
7. Response forwarding        — io_uring send to client
```

No heap allocations on steps 1-7 in the steady state.

## Metrics

All metric recording is lock-free:
- Atomic counters for connections, requests, bytes, errors
- Pre-registered Prometheus metrics (no allocation on record)
- Histogram buckets pre-allocated at startup
- JSON structured access logging via `tracing` (lazy serialization)
