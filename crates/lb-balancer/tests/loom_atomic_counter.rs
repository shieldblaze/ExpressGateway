//! CODE-2-11 — loom harness for the atomic-counter race the round-2
//! review §CODE-2-04 calls out.
//!
//! Models two threads:
//!
//! * T1 — `fetch_add(active_connections, Release)` (the connection-
//!   accept site that publishes the count).
//! * T2 — `load(active_connections, Acquire)` (the scheduler reading
//!   the count to pick a backend).
//!
//! Invariant: after both threads run, the loaded value is in
//! `{old, old+1}` and never lower than `old` (no underflow / missed
//! publication). Loom explores every legal interleaving under the
//! `Release` / `Acquire` ordering and asserts the invariant holds.
//!
//! ## Profile note
//!
//! Loom replaces `std::sync::atomic` types with its own model so it
//! must NOT be compiled into normal test builds — the workspace
//! `lb-balancer` only declares `loom` under `[target.'cfg(loom)']`.
//! Run with:
//!
//! ```bash
//! RUSTFLAGS="--cfg loom" cargo test -p lb-balancer --test loom_atomic_counter
//! ```
//!
//! The brief says "you do not need to actually run loom this round —
//! adding the scaffolding is sufficient." This file IS the scaffolding;
//! it compiles only when `--cfg loom` is set, so plain
//! `cargo test -p lb-balancer` is unaffected.

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
            // Accept site: publish the new count with Release so a
            // subsequent Acquire-load in the scheduler sees it.
            counter_writer.fetch_add(1, Ordering::Release);
        });

        let counter_reader = Arc::clone(&counter);
        let observed = thread::spawn(move || {
            // Scheduler: read with Acquire so the increment from T1 is
            // observed in causal order.
            counter_reader.load(Ordering::Acquire)
        });

        t1.join().unwrap();
        let v = observed.join().unwrap();

        // Final value is always 1 after both threads complete.
        assert_eq!(counter.load(Ordering::Acquire), 1);
        // Observation made BEFORE T1 stored may read 0; otherwise 1.
        // Either is correct — there is no underflow / missed publication.
        assert!(v == 0 || v == 1, "observed {v}, expected 0 or 1");
    });
}
