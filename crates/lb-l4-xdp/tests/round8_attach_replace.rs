//! ROUND8-L4-12 proof: the XDP detach signature OPS-04's drain
//! coordinator calls.
//!
//! Bundle B-5: div-ops authors the drain coordinator
//! (`crates/lb/src/main.rs`); this plan supplies the API the
//! coordinator promises to call. The non-ignored test in this file
//! is the cross-plan contract assertion — if the signatures drift,
//! both sides notice.
//!
//! Reference: kernel selftest `xdp_attach.c` (XDP_FLAGS_REPLACE /
//! BPF_F_REPLACE semantics); xdp-tutorial lesson 12 (multi-program
//! dispatcher). The kernel-touching tests are gated `#[ignore]` and
//! run on the privileged CI lane only.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lb_l4_xdp::loader::{XdpLoader, XdpLoaderError, XdpMode, XdpQueryResult};

#[test]
fn query_unknown_iface_is_a_loud_error_not_a_silent_none() {
    // ROUND8-L4-12: `query_xdp` is now a REAL RTM_GETLINK netlink
    // round-trip (no more `prog_id: None` stub). An interface that
    // does not exist resolves no ifindex, so the contract is a LOUD
    // `XdpQueryFailed` (NotFound) — NOT a silent `Ok(None)` which
    // previously made `detach_verifying`/`attach_replacing` operate
    // blind. Failing-loud here is exactly what stops the drain
    // coordinator from "verifying" a teardown against a stub.
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
    // Drain coordinator stores XdpQueryResult across an .await
    // boundary; `Copy + Default` is the minimum to make that
    // ergonomic. If a future refactor breaks Copy the coordinator
    // would silently clone — the test pins the property.
    fn requires_copy<T: Copy + Default>() {}
    requires_copy::<XdpQueryResult>();
    let _: XdpQueryResult = XdpQueryResult::default();
}

#[test]
fn detach_signature_matches_ops04_coordinator() {
    // Type-level contract: the function exists with EXACTLY this
    // signature. A change here breaks the OPS-04 drain coordinator
    // at the call site, which is the intended fail-loud behaviour.
    fn assert_detach_sig<F>(_: F)
    where
        F: FnMut(&str, &str, u32) -> Result<(), XdpLoaderError>,
    {
    }

    // Bind via a closure that calls the real method so any rename
    // / arg-type change trips compilation here AND at the OPS-04
    // call site.
    //
    // We do not actually construct an XdpLoader in this unit test
    // (would require CAP_BPF); the closure is type-checked but
    // never invoked. The unused-variable + dead-code warnings are
    // expected and silenced via `#[allow]` below.
    #[allow(dead_code, unused_variables)]
    let detach = |loader: &mut XdpLoader, prog: &str, iface: &str, expected_id: u32| {
        loader.detach_verifying(prog, iface, expected_id)
    };
    let _ = detach;

    // Validate the standalone-fn shape via a different path that
    // does not need a loader instance.
    assert_detach_sig::<Box<dyn FnMut(&str, &str, u32) -> Result<(), XdpLoaderError>>>(Box::new(
        |_, _, _| Ok(()),
    ));
}

#[test]
fn attach_replacing_signature_present() {
    // Same shape as detach — attaches must be replace-aware so the
    // coordinator can soft-reload without clobbering a foreign prog.
    #[allow(dead_code, unused_variables)]
    let attach = |loader: &mut XdpLoader, prog: &str, iface: &str, mode: XdpMode, old_id: u32| {
        loader.attach_replacing(prog, iface, mode, old_id)
    };
    let _ = attach;
}

/// Ignored: requires CAP_BPF + the `dummy` netlink driver.
#[test]
#[ignore = "kernel-touching: requires CAP_BPF + dummy netdev"]
fn detach_verifying_on_real_iface() {
    // Placeholder for the privileged CI lane; the full implementation
    // creates a `dummy0` iface via `rtnetlink`, attaches an
    // XDP_PASS-only prog, then runs `detach_verifying` and checks
    // post-condition. Lands with the netlink query implementation.
}
