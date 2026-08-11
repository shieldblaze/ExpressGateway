//! F-COR-7 regression — the ROUND8-L4-05 ENA driver-support blocklist.

use lb_l4_xdp::nic_compat::{DrvSupport, driver_of, drv_supported};

/// (1) lead D1: on THIS box (ena, kernel 7.0 — a NOT-known-bad combo, firmware unresolved)
/// `drv_supported("ens5")` MUST be `Allowed`.
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
