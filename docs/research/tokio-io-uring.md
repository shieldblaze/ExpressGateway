# Async Runtime: tokio + io_uring

## Tokio model

Tokio is a multi-threaded, work-stealing async runtime built on top of
Rust's zero-cost `Future` abstraction. A `Future` is a state machine that
only advances when polled; the runtime is responsible for deciding *when*
to poll, using kernel readiness notifications (epoll on Linux, kqueue on
BSD, IOCP on Windows) to know that a socket has data or a timer has
elapsed.

The multi-thread scheduler carves work into *tasks*. Each task is a boxed
`Future` owned by a worker thread. Workers poll their local run-queue
first and steal from peers when idle; this keeps hot tasks on a single
core (improving cache locality) while load-balancing bursts. Tokio's
`Runtime::block_on` drives one future to completion from the calling
thread; `spawn` submits a task that runs concurrently on any worker. The
single-threaded flavour (`rt` feature, `LocalSet`) is also available
when a codec is not `Send`.

On Linux today, tokio's I/O driver is epoll-based: file descriptors are
registered with `epoll_ctl`, the driver blocks in `epoll_wait`, and woken
tasks are rescheduled. This design has two properties we care about:
it is *readiness*-based (the kernel tells us "this fd can read"), and
it requires one syscall per I/O operation. The alternative described
below, io_uring, is *completion*-based and syscall-batched.

Cancellation in tokio is cooperative: dropping a `Future` effectively
cancels it, but the underlying I/O operation is not always cancelled
cleanly. `tokio-util::CancellationToken` gives us a structured signal
that multiple tasks can `select!` against, which we use for graceful
shutdown.

## io_uring

io_uring (Linux 5.1+, matured through 5.11, essentially feature-complete
by 6.1) is an asynchronous syscall interface built on two ring buffers
shared between user space and the kernel: a *submission queue* (SQ) into
which the application writes `io_uring_sqe` descriptors, and a
*completion queue* (CQ) into which the kernel writes `io_uring_cqe`
results. `io_uring_enter(2)` hands the SQ to the kernel; with `SQPOLL`
the kernel polls the SQ from a dedicated kernel thread and no syscall
per batch is needed.

Operations relevant to a reverse proxy (per PROMPT.md §7) are:

- `IORING_OP_ACCEPT` with multishot: one SQE spawns connections for the
  lifetime of the ring, massively reducing syscall count on a hot
  listener.
- `IORING_OP_RECV` / `IORING_OP_SEND`: analogues of `read`/`write`.
- `IORING_OP_SPLICE`: kernel-internal pipe transfer, zero-copy between
  a client socket and a backend socket via a pipe pair.
- `IORING_OP_TIMEOUT` and `IORING_OP_CANCEL`: native timer and
  cancellation ops, no userspace timer wheel required.
- `IORING_REGISTER_FILES`: pre-register fds by index; avoids the fd
  table lookup on every op.
- `IORING_REGISTER_BUFFERS`: pre-register application buffers; avoids
  per-op pinning / page-locking.

The kernel version matrix matters: our minimum target must assume at
least Linux 5.15 (Ubuntu 22.04 LTS) so we can rely on multishot accept
(5.19), provided buffers (5.7), and SQE128/CQE32 (5.19). On older
kernels we must fall back to epoll.

## Why ExpressGateway uses tokio (and not tokio-uring or glommio)

`crates/lb-io/src/lib.rs` exposes an `IoBackend` enum with `IoUring` and
`Epoll` variants and a `detect_backend()` function. Today that function
always returns `Epoll`; the io_uring path is reserved for a future
runtime-capability probe (the source comment makes this explicit). The
workspace-level `tokio` dependency is pinned to the `full` feature
collection so we can use the high-level ergonomics without forking.

We chose *plain tokio* over the dedicated `tokio-uring` crate and over
`glommio` for three reasons:

1. **Ecosystem compatibility.** `hyper`, `rustls`, `tonic`, `h3`,
   `quiche`, and the rest of the HTTP/TLS stack assume tokio's
   `AsyncRead`/`AsyncWrite`. Switching to `tokio-uring` means rewriting
   adapters; switching to glommio (a thread-per-core model with its
   own `Future` trait) means rewriting the stack.
2. **Deployment portability.** ExpressGateway must run on kernels
   where io_uring is disabled (Docker default-deny `seccomp` profile)
   or unavailable. The epoll path is the correct baseline.
3. **Bounded engineering risk.** The io_uring acceleration is an
   optimisation, not an invariant. We encapsulate the choice behind
   `lb-io::IoBackend` so the rest of the codebase never branches on
   it. When io_uring becomes tractable (i.e. tokio 1.x exposes a
   supported uring driver, or we integrate `tokio-uring` for the L7
   hot path only), we flip the default without rewriting call sites.

A future ADR (`docs/decisions/ADR-0001-io-uring-crate.md` is reserved
for this decision) will record which of `tokio-uring`, `io-uring`
(the low-level crate), or `ringbahn` we adopt when we turn the
io_uring path on. The relevant trade-off is: `tokio-uring` has its
own runtime (not composable with the main tokio runtime), `io-uring`
is just raw SQE/CQE bindings, and `ringbahn` is an abandoned
experiment.

## Async patterns we rely on

**Bounded channels for backpressure.** Any cross-task handoff that an
attacker can fill — e.g. the HTTP/2 demux → stream-processor queue —
must use `tokio::sync::mpsc::channel(cap)` rather than the unbounded
variant. When `cap` is reached, the sender `.await`s, which propagates
backpressure to the producer socket and ultimately to the kernel's TCP
receive window. PROMPT.md §7 pins the water marks at 64 KiB high / 32
KiB low.

**`select!` for multi-source waits with cancellation safety.** Every
loop of the form "read next frame OR observe shutdown OR hit idle
timeout" is a `tokio::select!` over a `CancellationToken::cancelled()`,
a `timeout_at(deadline)`, and the next-frame future. Cancellation
safety requires that each branch of the select is drop-safe; we
achieve this by using `tokio::io::AsyncReadExt::read` (cancellable
mid-read, returns what it has) rather than `read_exact` (not
cancellation-safe, may drop bytes).

**`CancellationToken` for graceful shutdown.** The control plane's
`SIGHUP` handler and the process SIGTERM handler share a root
`CancellationToken`. Every spawned task holds a child token; draining
a listener is a matter of cancelling the root and awaiting the task
join set.

**Timeouts everywhere.** Per PROMPT.md and the slowloris / slow-POST
detectors, every network read has a deadline. `tokio::time::timeout`
wraps the future with an internal timer; the returned `Elapsed` error
is turned into a specific `H1Error::Incomplete` /
`SecurityError::Slowloris*` by the caller.

## Hot-path considerations

**Allocation.** The `bytes` crate's `Bytes` / `BytesMut` give reference-
counted, slice-splittable buffers. The chunked decoder in
`crates/lb-h1/src/chunked.rs` uses `BytesMut::split_to` to carve out
per-chunk slices without copying. Frame encoders do the same:
`ChunkedEncoder::encode` allocates exactly `size_line.len() +
data.len() + 2` bytes up front.

**Splice alternatives on non-uring kernels.** Where io_uring is not
available, zero-copy bidirectional proxying reduces to two
`tokio::io::copy` tasks. On Linux these map to `sendfile(2)` for
file-to-socket and to user-space buffered copies for socket-to-socket.
With io_uring we would use `IORING_OP_SPLICE` through a pipe pair and
halve the per-byte CPU cost, but this optimisation is gated on the
backend selector.

**TLS record coalescing.** rustls emits application data as TLS
records up to 16 KiB. Batching application writes below 16 KiB into a
single `write_all` reduces the record count and improves TLS
throughput substantially on small messages; the L7 proxy is expected
to buffer up to the record boundary before flushing.

**Per-task stacks are small.** Tokio's default task stack allocation
is a single `Box<Future>`, not an OS thread stack. The HTTP/2 stream
state (`H2Frame` enum, `HpackDecoder`, detectors) must be structured
to live in a single `Box` to keep the allocation count low.

## Testing async

**`tokio::test`.** Unit tests that need a runtime use `#[tokio::test]`
with `flavor = "current_thread"` unless the test explicitly needs
multi-core scheduling. This is deterministic: no work stealing, no
surprise thread migrations.

**Synthetic time.** `tokio::time::pause()` and `advance(d)` let us
write slowloris / rapid-reset tests without sleeping. The detectors
in `crates/lb-security` and `crates/lb-h2` are deliberately parameterised
on an external "tick" value so unit tests can drive time without
involving the runtime at all (see `RapidResetDetector::record(tick)`).

**Loom.** Tests of concurrent data structures (the eventual bounded
channel in `lb-io`, the ArcSwap-based config backend in
`lb-controlplane`) should use `loom` to exhaustively explore
interleavings. We do not yet have a loom suite; this is a known gap.

**Integration tests.** The top-level `tests/` directory hosts
black-box scenarios (`tests/security_rapid_reset.rs` etc.) that
construct real detector state and drive it through attack-shaped
input. These run under the normal tokio runtime and prove end-to-end
behaviour.

## Sources

- Tokio docs, runtime overview,
  https://docs.rs/tokio/latest/tokio/runtime/index.html
- Jens Axboe, "Efficient IO with io_uring" (LWN, 2019),
  https://kernel.dk/io_uring.pdf
- Linux kernel io_uring documentation,
  https://www.kernel.org/doc/html/latest/io_uring/index.html
- "io_uring and networking in 2023", Kernel Recipes,
  https://lwn.net/Articles/776703/
- `tokio-uring` crate, https://github.com/tokio-rs/tokio-uring
- glommio, https://github.com/DataDog/glommio
- "Cancellation safety" note in tokio docs,
  https://docs.rs/tokio/latest/tokio/macro.select.html#cancellation-safety
- ExpressGateway PROMPT.md §7 "Module 3: io_uring Async Runtime"
