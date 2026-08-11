//! Locks the `panic_total` contract — name, type, and zero start.

use lb_observability::MetricsRegistry;
use lb_observability::prometheus_exposition::render_text;

#[test]
fn panic_total_starts_at_zero_and_is_exposed() {
    let reg = MetricsRegistry::new();
    let counter = reg
        .panic_total_counter()
        .expect("panic_total registers cleanly");
    assert_eq!(counter.get(), 0, "panic_total must start at zero");

    let body = render_text(&reg);
    // The row must exist at zero — operators alert on its absence.
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
    let fams = reg.gather();
    let count = fams.iter().filter(|f| f.name() == "panic_total").count();
    assert_eq!(count, 1);
}
