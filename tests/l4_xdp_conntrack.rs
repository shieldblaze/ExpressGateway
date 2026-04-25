//! XDP connection tracking and flow pinning tests.

use lb_l4_xdp::{ConntrackTable, FlowKey};

#[test]
fn test_xdp_conntrack_flow_pinning() {
    let mut ct = ConntrackTable::new();
    let flow = FlowKey {
        src_addr: 0x0A00_0001,
        dst_addr: 0x0A00_0002,
        src_port: 12345,
        dst_port: 80,
        protocol: 6,
    };
    ct.insert(flow.clone(), 2);
    // Same flow always returns same backend
    assert_eq!(ct.lookup(&flow), Some(2));
    assert_eq!(ct.lookup(&flow), Some(2));
    // Different flow returns None
    let other = FlowKey {
        src_addr: 0x0A00_0003,
        dst_addr: 0x0A00_0002,
        src_port: 54321,
        dst_port: 80,
        protocol: 6,
    };
    assert_eq!(ct.lookup(&other), None);
}
