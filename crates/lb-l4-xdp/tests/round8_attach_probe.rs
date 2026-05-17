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
//!     `ProbeUnavailable`. The REAL tripwire
//!     (`aya_version_is_pinned_to_the_no_test_run_release`)
//!     introspects the resolved aya version in `Cargo.lock` and FAILS
//!     on any aya bump — forcing a human to re-audit the test_run
//!     surface and wire the real probe. (Replaced the old
//!     `probe_unavailable_is_the_documented_state` const-fn tautology,
//!     which asserted a hardcoded return value and could never fire.)
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

/// Extract the resolved `aya` crate version from a `Cargo.lock`
/// body. Returns the version string of the `[[package]]` whose
/// `name = "aya"` (the program-loader crate, NOT `aya-obj` /
/// `aya-ebpf` / `aya-log`). Pure string parse so the tripwire logic
/// is unit-testable against a *simulated* future lockfile.
fn aya_version_from_lock(lock: &str) -> Option<String> {
    // Cargo.lock packages are TOML array-of-tables; find the block
    // whose name line is EXACTLY `name = "aya"` then read the next
    // `version = "..."` before the block ends (blank line / next
    // `[[package]]`).
    let mut lines = lock.lines().peekable();
    while let Some(line) = lines.next() {
        if line.trim() != "[[package]]" {
            continue;
        }
        let mut name: Option<&str> = None;
        let mut version: Option<&str> = None;
        for body in lines.by_ref() {
            let b = body.trim();
            if b == "[[package]]" || b.is_empty() {
                break;
            }
            if let Some(v) = b.strip_prefix("name = ") {
                name = Some(v.trim_matches('"'));
            } else if let Some(v) = b.strip_prefix("version = ") {
                version = Some(v.trim_matches('"'));
            }
        }
        if name == Some("aya") {
            return version.map(str::to_owned);
        }
    }
    None
}

/// Parse a `MAJOR.MINOR.PATCH` semver into a comparable tuple.
fn semver_tuple(v: &str) -> (u64, u64, u64) {
    let mut it = v.split('.').map(|p| {
        // strip any pre-release / build metadata on the patch part
        p.split(['-', '+'])
            .next()
            .unwrap_or("0")
            .parse::<u64>()
            .unwrap_or(0)
    });
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

/// The aya version that is KNOWN to lack a public `BPF_PROG_TEST_RUN`
/// wrapper on the `Xdp` program type (verified by code inspection of
/// aya 0.13.1 — only the raw generated binding constant exists, no
/// safe `Xdp::test_run`). The runtime silent-drop probe stays a
/// scaffold for exactly this version and no other.
const AYA_PINNED_NO_TEST_RUN: &str = "0.13.1";

fn workspace_cargo_lock() -> std::path::PathBuf {
    // crates/lb-l4-xdp -> workspace root is two levels up.
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("Cargo.lock")
}

/// REAL TRIPWIRE (replaces the old const-fn tautology that asserted
/// `probe_xdp_silent_drop() == ProbeUnavailable` — a `const fn`
/// hardcoded to that value, so it could never fail and never
/// detected an aya upgrade).
///
/// This introspects the resolved `aya` version in the workspace
/// `Cargo.lock` and FAILS the moment it differs from the pinned
/// `0.13.1`. Any aya bump is the trigger to (a) check whether the
/// new aya exposes a public `BPF_PROG_TEST_RUN` / `Xdp::test_run`,
/// (b) if so, replace `probe_xdp_silent_drop()` with the real
/// synthetic-packet probe and un-`#[ignore]`
/// `probe_synthetic_packet_round_trip`, then (c) re-pin
/// `AYA_PINNED_NO_TEST_RUN` to the new version (only if it STILL
/// lacks the API). The scaffold can no longer rot silently across
/// an aya upgrade.
#[test]
fn aya_version_is_pinned_to_the_no_test_run_release() {
    // Scaffold-state self-check: the probe is still the scaffold.
    assert_eq!(
        probe_xdp_silent_drop(),
        ProbeOutcome::ProbeUnavailable,
        "probe is still the scaffold; if you wired the real probe, \
         update this tripwire too"
    );

    let lock_path = workspace_cargo_lock();
    let lock = std::fs::read_to_string(&lock_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", lock_path.display()));
    let resolved = aya_version_from_lock(&lock)
        .expect("Cargo.lock must contain an [[package]] name = \"aya\"");

    assert_eq!(
        resolved, AYA_PINNED_NO_TEST_RUN,
        "aya resolved to {resolved} but the silent-drop probe scaffold \
         is pinned to {AYA_PINNED_NO_TEST_RUN} (the release verified to \
         have NO public BPF_PROG_TEST_RUN / Xdp::test_run). aya moved: \
         check whether the new release exposes the API; if so wire the \
         real probe in nic_compat.rs::probe_xdp_silent_drop and \
         un-#[ignore] probe_synthetic_packet_round_trip; then re-pin \
         AYA_PINNED_NO_TEST_RUN. This tripwire MUST be resolved by hand \
         — the scaffold cannot be trusted on an unaudited aya."
    );

    // Defence in depth: even if someone hand-edits the pinned literal
    // forward without auditing, also fail on ANY version strictly
    // greater than the audited one (an upgrade is always the signal).
    let (rmaj, rmin, rpat) = semver_tuple(&resolved);
    let (pmaj, pmin, ppat) = semver_tuple(AYA_PINNED_NO_TEST_RUN);
    assert!(
        (rmaj, rmin, rpat) <= (pmaj, pmin, ppat),
        "aya {resolved} > audited {AYA_PINNED_NO_TEST_RUN}: an upgrade \
         is the signal to re-audit the BPF_PROG_TEST_RUN surface"
    );
}

/// Proves the tripwire above ACTUALLY fails under a simulated
/// "aya gained the API" condition (a future Cargo.lock that resolves
/// aya to a newer release). The old test could never do this — it
/// asserted a `const fn`'s hardcoded return. This one exercises the
/// real parse + comparison path on a synthetic lockfile blob.
#[test]
fn tripwire_fires_when_aya_is_upgraded_simulation() {
    // A synthetic Cargo.lock fragment where aya has moved to a
    // hypothetical 0.14.0 that (per the comment) gained test_run.
    let simulated_future_lock = "\
# This file is automatically @generated by Cargo.
version = 4

[[package]]
name = \"anyhow\"
version = \"1.0.0\"

[[package]]
name = \"aya\"
version = \"0.14.0\"
source = \"registry+https://github.com/rust-lang/crates.io-index\"

[[package]]
name = \"aya-obj\"
version = \"0.14.0\"
";
    let resolved =
        aya_version_from_lock(simulated_future_lock).expect("simulated lock has an aya package");
    assert_eq!(
        resolved, "0.14.0",
        "parser must pick `aya` (0.14.0), NOT `aya-obj`"
    );

    // The exact assertion the real tripwire runs — confirm it WOULD
    // panic on this simulated-future lock instead of silently passing.
    let would_fail = std::panic::catch_unwind(|| {
        assert_eq!(resolved, AYA_PINNED_NO_TEST_RUN);
    })
    .is_err();
    assert!(
        would_fail,
        "tripwire is hollow: it did NOT fail when aya was upgraded \
         to 0.14.0 in the (simulated) lockfile"
    );

    // And the semver guard also fires.
    let (rmaj, rmin, rpat) = semver_tuple(&resolved);
    let (pmaj, pmin, ppat) = semver_tuple(AYA_PINNED_NO_TEST_RUN);
    assert!(
        (rmaj, rmin, rpat) > (pmaj, pmin, ppat),
        "semver guard must classify 0.14.0 as an upgrade over \
         {AYA_PINNED_NO_TEST_RUN}"
    );
}

/// The parser must not be fooled by `aya-obj` / `aya-ebpf` /
/// `aya-log` (whose names START with `aya` but are different crates).
#[test]
fn aya_lock_parser_does_not_confuse_sibling_crates() {
    let lock = "\
[[package]]
name = \"aya-obj\"
version = \"9.9.9\"

[[package]]
name = \"aya\"
version = \"0.13.1\"

[[package]]
name = \"aya-log\"
version = \"8.8.8\"
";
    assert_eq!(aya_version_from_lock(lock).as_deref(), Some("0.13.1"));
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
