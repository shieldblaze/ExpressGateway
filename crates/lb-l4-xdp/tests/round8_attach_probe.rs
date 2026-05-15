//! ROUND8-L4-05 proof: NIC silent-drop blocklist + post-attach probe
//! scaffold (aya #1193 / Cilium lesson 8).
//!
//! What is fully wired and tested here:
//!   - the `(driver, firmware)` blocklist `classify()` decision,
//!   - `ethtool -i` firmware-line parsing,
//!   - the `ProbeOutcome` / `DrvSupport` typed contracts,
//!   - the `xdp_attach_probe_failed_total` userspace counter.
//!
//! What is deferred (with an explicit tripwire test):
//!   - the runtime `BPF_PROG_TEST_RUN` probe — aya 0.13.1 exposes no
//!     public wrapper. `probe_xdp_silent_drop()` returns
//!     `ProbeUnavailable`; `probe_unavailable_is_the_documented_state`
//!     is the tripwire to wire the real probe when aya gains the API.
//!   - the kernel-touching attach-demotion happy path — needs
//!     CAP_BPF + a real (or mlx5-mocked) NIC; that is the CI
//!     privileged-stage fixture.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use lb_l4_xdp::nic_compat::{
    DrvSupport, ProbeOutcome, classify, drv_supported, parse_ethtool_firmware,
    probe_xdp_silent_drop,
};
use lb_l4_xdp::stats_export::{attach_probe_failed_count, record_attach_probe_failed};

#[test]
fn mlx5_pre_ghsa_firmware_is_refused_with_bug_id() {
    match classify("mlx5_core", "16.31.999") {
        DrvSupport::Refuse { reason } => {
            assert!(reason.contains("mlx5_core"), "reason: {reason}");
            assert!(
                reason.contains("aya#1193"),
                "blocklist reason must cite the bug-id: {reason}"
            );
            assert!(
                reason.contains("skb"),
                "reason must give the operator the override: {reason}"
            );
        }
        DrvSupport::Allowed => panic!("pre-GHSA mlx5 firmware must be refused"),
    }
}

#[test]
fn mlx5_patched_firmware_allowed() {
    assert_eq!(classify("mlx5_core", "16.32.1010"), DrvSupport::Allowed);
    assert_eq!(classify("mlx5_core", "20.40.1000"), DrvSupport::Allowed);
}

#[test]
fn unknown_or_virtual_driver_allowed() {
    // dummy0 / veth / virtio in CI have no blocklist row.
    assert_eq!(classify("dummy", "n/a"), DrvSupport::Allowed);
    assert_eq!(classify("veth", ""), DrvSupport::Allowed);
    assert_eq!(classify("virtio_net", "1.0"), DrvSupport::Allowed);
}

#[test]
fn ena_and_ice_blocklist_rows_round_trip() {
    assert!(matches!(
        classify("ena", "2.8.0"),
        DrvSupport::Refuse { .. }
    ));
    assert_eq!(classify("ena", "2.10.0"), DrvSupport::Allowed);
    assert!(matches!(classify("ice", "4.10"), DrvSupport::Refuse { .. }));
    assert_eq!(classify("ice", "4.11"), DrvSupport::Allowed);
}

#[test]
fn ethtool_firmware_line_parsing() {
    let real = "driver: mlx5_core\n\
                version: 6.1.0\n\
                firmware-version: 22.36.1010 (MT_0000000359)\n\
                expansion-rom-version:\n\
                bus-info: 0000:3b:00.0\n";
    assert_eq!(
        parse_ethtool_firmware(real).as_deref(),
        Some("22.36.1010 (MT_0000000359)")
    );
    // No firmware-version line (some virtual devices).
    assert_eq!(parse_ethtool_firmware("driver: veth\n"), None);
}

/// `drv_supported` on a non-existent interface must fail OPEN
/// (Allowed): we never block `Drv` just because sysfs/ethtool
/// introspection failed — the runtime probe + alert is the backstop.
#[test]
fn drv_supported_fails_open_on_unknown_iface() {
    let r = drv_supported("eg-nonexistent-iface-zzz").expect("never hard-errors");
    assert_eq!(r, DrvSupport::Allowed);
}

/// The attach-probe-failed counter is a process-global monotonic
/// atomic projected to `xdp_attach_probe_failed_total`.
#[test]
fn attach_probe_failed_counter_is_monotonic() {
    let a = attach_probe_failed_count();
    record_attach_probe_failed();
    let b = attach_probe_failed_count();
    assert!(b > a, "counter must increase: {a} -> {b}");
}

/// TRIPWIRE: documents the aya 0.13.1 `BPF_PROG_TEST_RUN` API
/// blocker. When aya gains a public `test_run` wrapper on `Xdp`,
/// `probe_xdp_silent_drop()` should run the real synthetic packet
/// and this assertion will (correctly) fail — the signal to wire the
/// kernel-touching probe + flip the CI fixture from `#[ignore]`.
#[test]
fn probe_unavailable_is_the_documented_state() {
    assert_eq!(
        probe_xdp_silent_drop(),
        ProbeOutcome::ProbeUnavailable,
        "aya 0.13.1 has no public BPF_PROG_TEST_RUN; if this fails, \
         aya gained the API — wire the real probe (see nic_compat.rs)"
    );
}

/// CI privileged-stage scaffold: attach to dummy0, pre-insert a
/// synthetic CT entry, run the probe, assert Ok + dst MAC rewritten.
/// Ignored until the aya API blocker clears (shares the EBPF-2-05
/// bpffs fixture).
#[test]
#[ignore = "needs CAP_BPF + aya BPF_PROG_TEST_RUN (aya 0.13.1 API blocker)"]
fn probe_synthetic_packet_round_trip() {
    eprintln!(
        "ROUND8-L4-05 kernel probe scaffold — once aya exposes \
         BPF_PROG_TEST_RUN: attach lb_xdp to dummy0, insert a \
         synthetic CONNTRACK entry, fire a synthetic Eth+IPv4+TCP \
         packet, assert ProbeOutcome::Ok and dst MAC was rewritten; \
         then attach an XDP_PASS-only prog and assert \
         ProbeOutcome::NotExecuted."
    );
}
