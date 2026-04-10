//! Connection backlog queue for buffering data while backend connects.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use bytes::BytesMut;
use parking_lot::Mutex;

/// Maximum items in the backlog queue.
const DEFAULT_MAX_SIZE: usize = 10_000;

/// Batch drain size.
const DRAIN_BATCH_SIZE: usize = 64;

/// Bounded queue for buffering client data while backend connection establishes.
pub struct ConnectionBacklog {
    queue: Mutex<VecDeque<BytesMut>>,
    size: AtomicUsize,
    max_size: usize,
    memory_pressure: AtomicBool,
}

impl ConnectionBacklog {
    /// Create a new backlog queue with default max size (10,000).
    pub fn new() -> Self {
        Self::with_max_size(DEFAULT_MAX_SIZE)
    }

    /// Create a new backlog queue with the specified max size.
    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            queue: Mutex::new(VecDeque::new()),
            size: AtomicUsize::new(0),
            max_size,
            memory_pressure: AtomicBool::new(false),
        }
    }

    /// Enqueue data. Returns false if the queue is full.
    pub fn enqueue(&self, data: BytesMut) -> bool {
        let effective_max = if self.memory_pressure.load(Ordering::Relaxed) {
            self.max_size / 4
        } else {
            self.max_size
        };

        if self.size.load(Ordering::Relaxed) >= effective_max {
            return false;
        }

        let mut queue = self.queue.lock();
        if queue.len() >= effective_max {
            return false;
        }
        queue.push_back(data);
        self.size.store(queue.len(), Ordering::Release);
        true
    }

    /// Drain up to `DRAIN_BATCH_SIZE` items from the queue.
    pub fn drain_batch(&self) -> Vec<BytesMut> {
        let mut queue = self.queue.lock();
        let count = queue.len().min(DRAIN_BATCH_SIZE);
        let batch: Vec<BytesMut> = queue.drain(..count).collect();
        self.size.store(queue.len(), Ordering::Release);
        batch
    }

    /// Clear all items from the queue.
    pub fn clear(&self) {
        let mut queue = self.queue.lock();
        queue.clear();
        self.size.store(0, Ordering::Release);
    }

    /// O(1) size check.
    pub fn len(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Set memory pressure flag. When active, effective limit is max/4.
    pub fn set_memory_pressure(&self, pressure: bool) {
        self.memory_pressure.store(pressure, Ordering::Release);
    }
}

impl Default for ConnectionBacklog {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_enqueue_dequeue() {
        let backlog = ConnectionBacklog::with_max_size(5);

        for i in 0..5 {
            let data = BytesMut::from(format!("data-{i}").as_bytes());
            assert!(backlog.enqueue(data));
        }
        assert_eq!(backlog.len(), 5);

        // Queue is full
        assert!(!backlog.enqueue(BytesMut::from(&b"overflow"[..])));

        let batch = backlog.drain_batch();
        assert_eq!(batch.len(), 5);
        assert!(backlog.is_empty());
    }

    #[test]
    fn test_memory_pressure() {
        let backlog = ConnectionBacklog::with_max_size(100);

        backlog.set_memory_pressure(true);
        // Effective limit is 100/4 = 25
        for i in 0..25 {
            assert!(backlog.enqueue(BytesMut::from(format!("{i}").as_bytes())));
        }
        assert!(!backlog.enqueue(BytesMut::from(&b"overflow"[..])));

        backlog.set_memory_pressure(false);
        // Now can add more
        assert!(backlog.enqueue(BytesMut::from(&b"ok"[..])));
    }

    #[test]
    fn test_clear() {
        let backlog = ConnectionBacklog::new();
        for i in 0..10 {
            backlog.enqueue(BytesMut::from(format!("{i}").as_bytes()));
        }
        assert_eq!(backlog.len(), 10);
        backlog.clear();
        assert!(backlog.is_empty());
    }
}
