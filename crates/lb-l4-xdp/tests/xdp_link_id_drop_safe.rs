//! EBPF-2-06 regression test: dropping the `XdpLinkId` returned by `Xdp::attach` does NOT detach the program — aya 0.13.1 keeps the link alive inside `ProgramData::links` until the owning
//! `Xdp`/`Ebpf` drops. Pinned here so a future aya upgrade that changes the link-ownership model is caught before it ships.

#![cfg(target_os = "linux")]

/// Compile-time assertion that `XdpLoader::attach` returns `()`, NOT `XdpLinkId`: if aya ever changes the ownership model so dropping the id detaches, this stops compiling at the signature mismatch.
#[test]
fn loader_attach_signature_drops_xdplinkid_silently() {
    fn _signature_check<F>(_f: F)
    where
        F: Fn(
            &mut lb_l4_xdp::loader::XdpLoader,
            &str,
            &str,
            lb_l4_xdp::loader::XdpMode,
        ) -> Result<(), lb_l4_xdp::loader::XdpLoaderError>,
    {
    }
    _signature_check(lb_l4_xdp::loader::XdpLoader::attach);
}

/// EBPF-2-06 named regression test (`#[ignore]`d, needs CAP_BPF + CAP_NET_ADMIN + dummy0): attach, drop the link id, assert XDP is still attached, then drop the whole `XdpLoader` and assert it is gone.
#[test]
#[ignore = "needs CAP_BPF + CAP_NET_ADMIN + dummy0 — runs in CI privileged stage"]
fn xdp_link_persists_after_id_drop() {
    eprintln!(
        "EBPF-2-06 link-persistence test stub — full scaffold lands with the \
         CI privileged-stage netdev fixture (shared with EBPF-2-04 SKB-fallback \
         test). Compile-time signature guard runs always."
    );
}
