//! Lock-free buffer pool for `BytesMut` reuse via `crossbeam::queue::ArrayQueue`.
//!
//! In steady state, this pool produces zero allocations: buffers are returned
//! to the pool when dropped and reacquired without hitting the allocator.
//!
//! The `ArrayQueue` is bounded and lock-free (MPMC), making it safe and fast
//! to share across all worker threads.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::BytesMut;
use crossbeam::queue::ArrayQueue;

/// Default buffer capacity when allocating a new buffer (8 KiB).
const DEFAULT_BUF_CAPACITY: usize = 8192;

/// Default pool capacity (number of buffers).
const DEFAULT_POOL_CAPACITY: usize = 1024;

/// Configuration for the buffer pool.
#[derive(Debug, Clone)]
pub struct BufferPoolConfig {
    /// Number of buffers in the pool (default 1024).
    pub pool_capacity: usize,
    /// Initial capacity of each buffer in bytes (default 8192).
    pub buf_capacity: usize,
}

impl Default for BufferPoolConfig {
    fn default() -> Self {
        Self {
            pool_capacity: DEFAULT_POOL_CAPACITY,
            buf_capacity: DEFAULT_BUF_CAPACITY,
        }
    }
}

/// Lock-free pool of pre-allocated `BytesMut` buffers.
///
/// Uses `ArrayQueue` (bounded MPMC) for zero-lock buffer recycling.
/// In steady state, `acquire()` returns a recycled buffer without allocation.
/// If the pool is empty, a new buffer is allocated on demand.
/// If the pool is full on `release()`, the buffer is dropped.
pub struct BufferPool {
    queue: Arc<ArrayQueue<BytesMut>>,
    buf_capacity: usize,
    hits: AtomicU64,
    misses: AtomicU64,
}

/// Buffer pool statistics.
#[derive(Debug, Clone, Copy)]
pub struct BufferPoolStats {
    /// Number of successful pool acquisitions (buffer reused).
    pub hits: u64,
    /// Number of pool misses (new buffer allocated).
    pub misses: u64,
    /// Current number of buffers in the pool.
    pub available: usize,
    /// Maximum pool capacity.
    pub capacity: usize,
}

impl BufferPool {
    /// Create a new buffer pool with the given configuration.
    ///
    /// Does NOT pre-allocate buffers. Buffers are created on first `acquire()`
    /// and recycled via `release()`. This avoids wasting memory if the pool
    /// is sized for peak load but steady-state usage is lower.
    pub fn new(config: BufferPoolConfig) -> Self {
        Self {
            queue: Arc::new(ArrayQueue::new(config.pool_capacity)),
            buf_capacity: config.buf_capacity,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
        }
    }

    /// Pre-fill the pool with `count` buffers.
    ///
    /// Useful for avoiding allocation thundering herd at startup.
    pub fn prefill(&self, count: usize) {
        for _ in 0..count.min(self.queue.capacity()) {
            let buf = BytesMut::with_capacity(self.buf_capacity);
            if self.queue.push(buf).is_err() {
                break;
            }
        }
    }

    /// Acquire a buffer from the pool.
    ///
    /// Returns a recycled buffer if available (zero allocation), or allocates
    /// a new one if the pool is empty.
    #[inline]
    pub fn acquire(&self) -> BytesMut {
        match self.queue.pop() {
            Some(mut buf) => {
                self.hits.fetch_add(1, Ordering::Relaxed);
                buf.clear();
                buf
            }
            None => {
                self.misses.fetch_add(1, Ordering::Relaxed);
                BytesMut::with_capacity(self.buf_capacity)
            }
        }
    }

    /// Return a buffer to the pool for reuse.
    ///
    /// If the pool is full, the buffer is silently dropped. This provides
    /// natural backpressure: under high load, excess buffers are freed by the
    /// allocator instead of growing the pool unboundedly.
    #[inline]
    pub fn release(&self, mut buf: BytesMut) {
        buf.clear();
        // Shrink buffers that have grown excessively (> 4x initial capacity)
        // to prevent memory bloat from large request bodies sitting in the pool.
        if buf.capacity() > self.buf_capacity * 4 {
            buf = BytesMut::with_capacity(self.buf_capacity);
        }
        let _ = self.queue.push(buf);
    }

    /// Number of buffers currently available in the pool.
    #[inline]
    pub fn available(&self) -> usize {
        self.queue.len()
    }

    /// Maximum pool capacity.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.queue.capacity()
    }

    /// Get current pool statistics.
    #[inline]
    pub fn stats(&self) -> BufferPoolStats {
        BufferPoolStats {
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            available: self.queue.len(),
            capacity: self.queue.capacity(),
        }
    }
}

impl Default for BufferPool {
    fn default() -> Self {
        Self::new(BufferPoolConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acquire_returns_buffer() {
        let pool = BufferPool::default();
        let buf = pool.acquire();
        assert!(buf.capacity() >= DEFAULT_BUF_CAPACITY);
        assert!(buf.is_empty());
    }

    #[test]
    fn release_and_reacquire() {
        let pool = BufferPool::default();

        let mut buf = pool.acquire();
        buf.extend_from_slice(b"hello");
        pool.release(buf);

        assert_eq!(pool.available(), 1);

        let buf2 = pool.acquire();
        assert!(buf2.is_empty(), "recycled buffer should be cleared");
        assert!(buf2.capacity() >= DEFAULT_BUF_CAPACITY);
        assert_eq!(pool.available(), 0);
    }

    #[test]
    fn stats_tracking() {
        let pool = BufferPool::default();

        let buf = pool.acquire();
        assert_eq!(pool.stats().misses, 1);
        assert_eq!(pool.stats().hits, 0);

        pool.release(buf);
        let _buf = pool.acquire();
        assert_eq!(pool.stats().hits, 1);
        assert_eq!(pool.stats().misses, 1);
    }

    #[test]
    fn pool_capacity_limit() {
        let config = BufferPoolConfig {
            pool_capacity: 2,
            buf_capacity: 64,
        };
        let pool = BufferPool::new(config);

        let b1 = pool.acquire();
        let b2 = pool.acquire();
        let b3 = pool.acquire();

        pool.release(b1);
        pool.release(b2);
        pool.release(b3); // silently dropped (pool full)

        assert_eq!(pool.available(), 2);
    }

    #[test]
    fn prefill() {
        let config = BufferPoolConfig {
            pool_capacity: 16,
            buf_capacity: 256,
        };
        let pool = BufferPool::new(config);
        pool.prefill(8);

        assert_eq!(pool.available(), 8);

        let buf = pool.acquire();
        assert_eq!(pool.stats().hits, 1);
        assert!(buf.capacity() >= 256);
    }

    #[test]
    fn oversized_buffers_are_replaced() {
        let config = BufferPoolConfig {
            pool_capacity: 4,
            buf_capacity: 64,
        };
        let pool = BufferPool::new(config);

        let mut buf = BytesMut::with_capacity(64 * 5);
        buf.extend_from_slice(&[0u8; 64 * 5]);
        pool.release(buf);

        assert_eq!(pool.available(), 1);
        let recycled = pool.acquire();
        assert!(
            recycled.capacity() <= 64 * 4,
            "oversized buffer should be replaced, got capacity {}",
            recycled.capacity()
        );
    }

    #[test]
    fn concurrent_access() {
        use std::thread;

        let pool = Arc::new(BufferPool::new(BufferPoolConfig {
            pool_capacity: 128,
            buf_capacity: 64,
        }));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let pool = Arc::clone(&pool);
                thread::spawn(move || {
                    for _ in 0..100 {
                        let mut buf = pool.acquire();
                        buf.extend_from_slice(b"test");
                        pool.release(buf);
                    }
                })
            })
            .collect();

        for h in handles {
            h.join().expect("thread should not panic");
        }

        let stats = pool.stats();
        assert_eq!(stats.hits + stats.misses, 800);
    }
}
