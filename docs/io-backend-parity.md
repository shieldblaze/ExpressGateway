# io_uring vs epoll Feature Parity Matrix

## Backend Selection

```toml
[runtime]
backend = "auto"   # auto | io_uring | epoll
```

- **auto** (default): Probes kernel via `libc::uname(2)`. Uses io_uring if kernel >= 5.11, otherwise epoll.
- **io_uring**: Forces io_uring. Fails at startup if unavailable.
- **epoll**: Forces epoll fallback.

Per-listener override:
```toml
[[listeners]]
name = "http-tls"
bind = "0.0.0.0:443"
io_backend = "io_uring"   # override global setting for this listener
```

## Feature Matrix

| Feature | io_uring | epoll | Notes |
|---------|----------|-------|-------|
| **Accept** | Multi-shot accept (1 SQE → N CQEs) | `accept4()` per connection | io_uring: fewer syscalls |
| **Read** | Multi-shot recv with registered buffers | `read()` / `recv()` | io_uring: zero-copy capable |
| **Write** | Registered buffer send | `write()` / `send()` | io_uring: zero-copy capable |
| **Splice** | `IORING_OP_SPLICE` | `splice(2)` syscall | Both zero-copy for TCP relay |
| **Connect** | `IORING_OP_CONNECT` | Non-blocking + epoll wait | Functionally identical |
| **Close** | `IORING_OP_CLOSE` | `close(2)` | io_uring: batched |
| **Timeout** | `IORING_OP_TIMEOUT` | `timerfd` or Tokio timer | io_uring: no extra fd |
| **Poll** | Kernel-side SQPOLL thread | `epoll_wait()` syscall | io_uring: zero-syscall steady state |
| **Batching** | Submit queue batching | One op per syscall | io_uring: amortized syscall cost |
| **Buffer registration** | `IORING_REGISTER_BUFFERS` | N/A | io_uring: pre-pinned DMA buffers |
| **File registration** | `IORING_REGISTER_FILES` | N/A | io_uring: fixed fd table |
| **TCP keep-alive** | Via socket options | Via socket options | Identical |
| **TCP_NODELAY** | Via socket options | Via socket options | Identical |
| **TCP_FASTOPEN** | Via socket options | Via socket options | Identical |
| **SO_REUSEPORT** | Supported | Supported | Identical |

## Performance Characteristics

| Metric | io_uring | epoll |
|--------|----------|-------|
| Syscalls per request (steady state) | ~0 (SQPOLL) | ~4 (epoll_wait + read + write + ...) |
| Accept throughput | Higher (multi-shot) | Standard |
| Memory copies on data path | 0 (registered buffers) | 1+ (kernel ↔ user) |
| Context switches | Minimal (SQPOLL) | Per-epoll_wait |
| Latency (P99) | Lower | Standard |
| CPU usage (saturated) | Lower | Standard |

## Externally Observable Behavior

**Guarantee: Both backends produce byte-for-byte identical externally observable behavior.**

This means:
- Same HTTP responses for same requests
- Same header ordering and values
- Same error responses and status codes
- Same timeout behavior
- Same connection lifecycle (keep-alive, drain, close)
- Same protocol negotiation results (ALPN, SNI, etc.)

The only differences are internal:
- Syscall patterns
- CPU/memory efficiency
- Latency distribution

## Kernel Version Requirements

| Kernel | io_uring Features Available |
|--------|-----------------------------|
| < 5.1 | None (io_uring not available) |
| 5.1-5.3 | Basic read/write/accept |
| 5.4-5.5 | Timeout, poll |
| 5.6-5.10 | Splice, provide buffers |
| **5.11+** | **SQPOLL, multi-shot accept/recv, registered buffers/files** |
| 5.19+ | Multi-shot recv |
| 6.0+ | Zero-copy send |

ExpressGateway requires **5.11+** for the io_uring backend.

## Fallback Detection

At startup, the runtime:
1. Calls `libc::uname(2)` (zero-allocation, no fork)
2. Parses major.minor from `utsname.release`
3. If >= 5.11 and config allows: uses io_uring
4. Otherwise: falls back to epoll
5. Logs: `"I/O backend: io_uring (kernel 6.1.0)"` or `"I/O backend: epoll (kernel 4.19.0)"`

## Testing

Both backends are tested through the same integration test suite. The test
framework runs each test against both backends to verify parity.
