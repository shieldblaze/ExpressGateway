//! XDP Maglev consistent hashing tests.

use lb_l4_xdp::MaglevTable;

#[test]
fn test_xdp_maglev_consistent_hash() {
    let backends = vec!["backend-1".into(), "backend-2".into(), "backend-3".into()];
    let table = MaglevTable::new(&backends, 65537).unwrap();
    // Same key always maps to same backend
    let b1 = table.lookup(12345).unwrap();
    let b2 = table.lookup(12345).unwrap();
    assert_eq!(b1, b2);
    // Different keys should distribute (test multiple keys)
    let mut counts = [0u32; 3];
    for i in 0..3000 {
        let idx = table.lookup(i).unwrap();
        counts[idx] += 1;
    }
    // Each backend should get some traffic
    for count in &counts {
        assert!(*count > 0);
    }
}
