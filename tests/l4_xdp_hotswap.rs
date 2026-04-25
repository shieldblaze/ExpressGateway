//! XDP program hot-swap tests.

use lb_l4_xdp::{FlowKey, HotSwapManager};

#[test]
fn test_xdp_hotswap_no_flow_drop() {
    let backends = vec!["b1".into(), "b2".into(), "b3".into()];
    let mut mgr = HotSwapManager::new(&backends, 65537).unwrap();
    // Establish a flow
    let flow = FlowKey {
        src_addr: 1,
        dst_addr: 2,
        src_port: 100,
        dst_port: 80,
        protocol: 6,
    };
    let original_backend = mgr.route_flow(flow.clone(), 42).unwrap();
    // Swap backends (add a 4th)
    let new_backends = vec!["b1".into(), "b2".into(), "b3".into(), "b4".into()];
    mgr.swap_backends(&new_backends, 65537).unwrap();
    // Existing flow still goes to same backend (via conntrack)
    let after_swap = mgr.route_flow(flow, 42).unwrap();
    assert_eq!(original_backend, after_swap);
}
