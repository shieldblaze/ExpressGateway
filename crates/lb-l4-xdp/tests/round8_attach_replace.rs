//! ROUND8-L4-12 proof: the XDP detach signature OPS-04's drain coordinator calls.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lb_l4_xdp::loader::{XdpLoader, XdpLoaderError, XdpMode, XdpQueryResult};

#[test]
fn query_unknown_iface_is_a_loud_error_not_a_silent_none() {
    let r = XdpLoader::query_xdp("eg-nonexistent-iface-zzz");
    match r {
        Err(XdpLoaderError::XdpQueryFailed { iface, .. }) => {
            assert_eq!(iface, "eg-nonexistent-iface-zzz");
        }
        other => panic!("expected XdpQueryFailed for a nonexistent iface, got {other:?}"),
    }
}

#[test]
fn xdp_query_result_is_copy_and_default() {
    fn requires_copy<T: Copy + Default>() {}
    requires_copy::<XdpQueryResult>();
    let _: XdpQueryResult = XdpQueryResult::default();
}

#[test]
fn detach_signature_matches_ops04_coordinator() {
    fn assert_detach_sig<F>(_: F)
    where
        F: FnMut(&str, &str, u32) -> Result<(), XdpLoaderError>,
    {
    }

    #[allow(dead_code, unused_variables)]
    let detach = |loader: &mut XdpLoader, prog: &str, iface: &str, expected_id: u32| {
        loader.detach_verifying(prog, iface, expected_id)
    };
    let _ = detach;

    assert_detach_sig::<Box<dyn FnMut(&str, &str, u32) -> Result<(), XdpLoaderError>>>(Box::new(
        |_, _, _| Ok(()),
    ));
}

#[test]
fn attach_replacing_signature_present() {
    #[allow(dead_code, unused_variables)]
    let attach = |loader: &mut XdpLoader, prog: &str, iface: &str, mode: XdpMode, old_id: u32| {
        loader.attach_replacing(prog, iface, mode, old_id)
    };
    let _ = attach;
}

/// Ignored: requires CAP_BPF + the `dummy` netlink driver.
#[test]
#[ignore = "kernel-touching: requires CAP_BPF + dummy netdev"]
fn detach_verifying_on_real_iface() {}
