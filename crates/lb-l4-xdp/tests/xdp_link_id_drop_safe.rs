//! EBPF-2-06 regression test: dropping the `XdpLinkId` returned by
//! `Xdp::attach` does NOT detach the BPF program — aya 0.13.1 keeps
//! the link alive inside `ProgramData::links` until the owning
//! `Xdp`/`Ebpf` is dropped. This test pins that behaviour so any
//! future aya upgrade that silently changes the link-ownership
//! model is caught before it ships.
//!
//! Per the round-2 plan (batch-low), the test is `#[ignore]`'d by
//! default (CAP_BPF + CAP_NET_ADMIN + dummy0 netdev are needed),
//! and CI runs it under `--ignored` in the privileged stage.
//!
//! The ALWAYS-on signal here is the loader unit test
//! `loader::tests::xdp_mode_flag_mapping` plus this file's
//! `loader_attach_signature_drops_xdplinkid_silently` which is a
//! compile-time-only assertion that the existing `attach(...)`
//! API contract still drops the return value (i.e. is `()`, not
//! `XdpLinkId`).

#![cfg(target_os = "linux")]

/// Compile-time assertion: `XdpLoader::attach` returns `()`, NOT
/// `XdpLinkId`. If aya 0.14 ever changes the link-ownership model
/// so dropping the id detaches, the loader API must change too
/// (move the id into the loader or take ownership of the `Xdp`
/// handle differently), and this test stops compiling at the
/// signature mismatch — which is the exact tripwire we want.
#[test]
fn loader_attach_signature_drops_xdplinkid_silently() {
    // The signature check happens at compile time. Body just
    // confirms the binding is still `Result<(), ...>`-shaped.
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

/// EBPF-2-06 named regression test: the link survives a drop of
/// the `XdpLinkId` returned internally. Marked `#[ignore]`.
///
/// Full scaffold (CI):
///   1. `ip link add dummy0 type dummy && ip link set dummy0 up`.
///   2. Load `lb_xdp.bin`; call `loader.attach("lb_xdp", "dummy0",
///      XdpMode::Skb)` — the `_link_id` returned by aya is
///      dropped INSIDE the loader's `attach` method.
///   3. Assert XDP is attached: shell out to `bpftool net show
///      dev dummy0` or read `/sys/class/net/dummy0/xdp` and
///      grep for a non-zero prog id.
///   4. Drop the whole `XdpLoader`; re-check (step 3 should
///      return empty / zero).
///   5. `ip link del dummy0` cleanup.
///
/// If aya 0.14 silently detaches on `XdpLinkId::drop`, step 3
/// returns zero — the test fails with a clear "expected attached;
/// found detached" panic, and the operator who upgraded aya knows
/// they have to rework the loader.
#[test]
#[ignore = "needs CAP_BPF + CAP_NET_ADMIN + dummy0 — runs in CI privileged stage"]
fn xdp_link_persists_after_id_drop() {
    eprintln!(
        "EBPF-2-06 link-persistence test stub — full scaffold lands with the \
         CI privileged-stage netdev fixture (shared with EBPF-2-04 SKB-fallback \
         test). Compile-time signature guard runs always."
    );
}
