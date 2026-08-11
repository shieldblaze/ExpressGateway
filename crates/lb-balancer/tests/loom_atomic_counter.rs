//! Loom model of the accept-site `fetch_add(Release)` against the scheduler's `load(Acquire)`: the
//! loaded value must be `{old, old+1}` and never below `old`. Loom SUBSTITUTES `std::sync::atomic`,
//! so this is gated behind `cfg(loom)` and must never enter a normal test build.

#![cfg(loom)]

use loom::sync::Arc;
use loom::sync::atomic::{AtomicU64, Ordering};
use loom::thread;

#[test]
fn atomic_counter_race_publishes_correctly() {
    loom::model(|| {
        let counter = Arc::new(AtomicU64::new(0));

        let counter_writer = Arc::clone(&counter);
        let t1 = thread::spawn(move || {
            counter_writer.fetch_add(1, Ordering::Release);
        });

        let counter_reader = Arc::clone(&counter);
        let observed = thread::spawn(move || {
            // Acquire pairs with T1's Release.
            counter_reader.load(Ordering::Acquire)
        });

        t1.join().unwrap();
        let v = observed.join().unwrap();

        assert_eq!(counter.load(Ordering::Acquire), 1);
        // 0 is legal (observed before the store); anything else means a lost publication.
        assert!(v == 0 || v == 1, "observed {v}, expected 0 or 1");
    });
}
