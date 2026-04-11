//! Connection pooling for ExpressGateway.
//!
//! Provides protocol-specific connection pools:
//! - [`H1Pool`] — HTTP/1.1 keep-alive (LIFO for warm reuse)
//! - [`H2Pool`] — HTTP/2 multiplexed streams
//! - [`TcpPool`] — L4 TCP connections (FIFO for even aging)
//! - [`QuicPool`] — QUIC multiplexed streams
//! - [`PoolEvictor`] — Shared background eviction task
//! - [`BufferPool`] — Lock-free `BytesMut` buffer reuse via `ArrayQueue`
//! - [`error`] — Crate-specific error types

pub mod buffer;
pub mod error;
pub mod evictor;
pub mod h1;
pub mod h2;
pub mod quic;
pub mod tcp;

pub use buffer::BufferPool;
pub use error::PoolError;
pub use evictor::{Evictable, PoolEvictor};
pub use h1::{H1Pool, H1PoolConfig, PoolStats, PooledConnection};
pub use h2::{H2Pool, H2PoolConfig, H2PoolStats, H2PooledConnection};
pub use quic::{ConnectionId, QuicPool, QuicPoolConfig, QuicPoolStats, QuicPooledConnection};
pub use tcp::{TcpPool, TcpPoolConfig, TcpPooledConnection};
