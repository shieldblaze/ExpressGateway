//! F-COR-7 regression — the ROUND8-L4-05 ENA driver-support blocklist
//! is no longer DEAD / fail-open on real AWS ENA, and is keyed on
//! driver + kernel-version (lead D1 redirect) so it does NOT fleet-
//! wide-regress native XDP on not-known-bad ENA boxes.
//!
//! Background (auditor-3 A-4): `ethtool -i ens5` returns an EMPTY
//! `firmware-version:` on AWS ENA, so `firmware_of` errored and the
//! old `drv_supported` fail-OPENed to `Allowed` — the ena blocklist
//! row could never fire on the exact platform it protects.
//!
//! Lead D1 redirect: do NOT fail-closed on ALL ena (that would refuse
//! native XDP fleet-wide and contradict the D-1 PASS). Instead key the
//! ena row on driver + kernel-version using the row's OWN documented
//! condition ("pre-2024 kernels"; kernel 6.7 = first 2024 mainline):
//!   - ena + kernel >= 6.7 (this box: ena/7.0, D-1 PASS) → Allowed,
//!   - ena + kernel <  6.7 (synthetic known-bad combo)   → Refuse.
//!
//! This binary exercises the FULL resolution path (`drv_supported`
//! → real sysfs/ethtool), not `classify()` directly, exactly as
//! auditor-3 asked. The synthetic known-bad-combo half is covered by
//! the pure `classify_unresolved_firmware` unit tests in
//! `nic_compat::tests` (ena/pre-6.7 → Refuse) so BOTH lead-D1
//! assertions are proven.

use lb_l4_xdp::nic_compat::{DrvSupport, driver_of, drv_supported};

/// (1) lead D1: on THIS box (ena, kernel 7.0 — a NOT-known-bad combo,
/// firmware unresolved) `drv_supported("ens5")` MUST be `Allowed`.
///
/// Pre-F-COR-7 this was also `Allowed` but for the WRONG reason
/// (blind fail-open on the unreadable firmware — the dead path). Post
/// fix it is `Allowed` because the driver+kernel key proves the combo
/// is not known-bad (kernel 7.0 >= 6.7), preserving the D-1 native
/// xdpdrv capability with the defence path now genuinely evaluated.
///
/// Portable / CI-safe: if the iface is not `ena` (any non-AWS box) the
/// test SKIPs. On this audit box ens5 IS ena so it runs for real.
#[test]
fn drv_supported_ens5_is_allowed_on_this_not_known_bad_ena_box() {
    let iface = "ens5";
    match driver_of(iface) {
        Ok(d) if d == "ena" => {
            let got = drv_supported(iface).expect("drv_supported never Err today");
            assert_eq!(
                got,
                DrvSupport::Allowed,
                "ens5 is ena on kernel 7.0 — a NOT-known-bad combo \
                 (kernel >= 6.7). drv_supported MUST stay Allowed so \
                 native XDP is preserved fleet-wide (D-1 PASS \
                 consistency). Got: {got:?}"
            );
        }
        Ok(other) => {
            eprintln!(
                "SKIP: {iface} driver is {other:?}, not ena — \
                 driver+kernel ena regression not applicable here"
            );
        }
        Err(e) => {
            eprintln!(
                "SKIP: could not resolve {iface} driver ({e}) — \
                 virtual/CI host"
            );
        }
    }
}
