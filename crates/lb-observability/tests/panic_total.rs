//! L-007 / REL-2-15 / CODE-2-02 wiring: `panic_total` Counter is
//! registered through the canonical accessor and starts at zero.
//!
//! Wave-2c main.rs replaces its private `static PANIC_TOTAL:
//! AtomicU64` with a clone of the handle returned here. This test
//! locks the contract — metric name, type, and starting value —
//! so the rewrite is a pure wiring change.

use lb_observability::MetricsRegistry;
use lb_observability::prometheus_exposition::render_text;

#[test]
fn panic_total_starts_at_zero_and_is_exposed() {
    let reg = MetricsRegistry::new();
    let counter = reg
        .panic_total_counter()
        .expect("panic_total registers cleanly");
    // Counter starts at zero before any panic fires.
    assert_eq!(counter.get(), 0, "panic_total must start at zero");

    let body = render_text(&reg);
    // Prometheus text exposition emits `panic_total 0` for an
    // unincremented counter. Operators rely on the metric being
    // present even when there have been zero panics.
    assert!(
        body.contains("# TYPE panic_total counter"),
        "TYPE line missing: {body}",
    );
    assert!(
        body.contains("panic_total 0"),
        "expected panic_total 0 row: {body}",
    );
}

#[test]
fn panic_total_accessor_is_idempotent() {
    let reg = MetricsRegistry::new();
    let a = reg.panic_total_counter().unwrap();
    let b = reg.panic_total_counter().unwrap();
    a.inc();
    b.inc();
    assert_eq!(a.get(), 2, "both handles must share underlying state");
    assert_eq!(b.get(), 2);
    // Only one family is registered (no duplicate).
    let fams = reg.gather();
    let count = fams
        .iter()
        .filter(|f| f.get_name() == "panic_total")
        .count();
    assert_eq!(count, 1);
}
