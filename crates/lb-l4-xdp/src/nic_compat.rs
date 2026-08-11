//! ROUND8-L4-05: known-bad NIC + firmware blocklist for native (`Drv`) XDP, plus the post-attach silent-drop probe scaffold (Linux-only — sysfs/ethtool).
//!
//! The hazard (aya #1193, Cilium lesson 8): on some MLX5/ConnectX firmware, DRV mode silently drops — `bpf_link_create` returns success, every map op reports success, and the packet path
//! goes to /dev/null. Two layered defences: a static `(driver, firmware)` blocklist (wired today, best-effort, goes stale) and a runtime `BPF_PROG_TEST_RUN` probe (the real backstop,
//! blocked on aya 0.13.1 exposing no public wrapper — see [`probe_xdp_silent_drop`]).

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;

/// Outcome of [`probe_xdp_silent_drop`]. `ProbeUnavailable` means the probe could not run at all (aya API blocker): the caller MUST treat it as inconclusive, KEEP the attach, and lean on the static blocklist plus the `xdp_attach_probe_failed_total` alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Synthetic packet round-tripped with the expected rewrite.
    Ok,
    /// `BPF_PROG_TEST_RUN` ran but action != `XDP_TX` (aya #1193).
    SilentDrop,
    /// Action looked right but the program body did not execute.
    NotExecuted,
    /// `BPF_PROG_TEST_RUN` not reachable on this aya version.
    ProbeUnavailable,
}

/// Whether `Drv` mode is safe to attempt on a given interface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrvSupport {
    /// No blocklist match — `Drv` may be attempted (the runtime probe is still the backstop once it is reachable).
    Allowed,
    /// The `(driver, firmware)` combination is known to silently drop, so `Drv` MUST be skipped.
    Refuse {
        /// Operator-facing reason (driver, firmware, bug-id link).
        reason: String,
    },
}

/// Errors from the sysfs / ethtool introspection path. A read failure is NOT fatal — the caller treats it as "could not determine", allows `Drv`, and relies on the probe + alert.
#[derive(Debug, thiserror::Error)]
pub enum NicCompatError {
    /// `/sys/class/net/<iface>/device/driver` could not be read (interface gone, virtual device with no driver symlink, ...).
    #[error("could not resolve driver for {iface}: {source}")]
    DriverUnresolved {
        /// Interface name.
        iface: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// `ethtool -i <iface>` could not be executed or parsed.
    #[error("could not read firmware for {iface}: {reason}")]
    FirmwareUnresolved {
        /// Interface name.
        iface: String,
        /// Why the firmware read failed.
        reason: String,
    },
}

/// A single blocklist row. Only `firmware < first_safe` is ever compared, so the row carries the first KNOWN-GOOD version string plus a human reason.
#[derive(Debug, Clone, Copy)]
struct BlockRow {
    /// Kernel driver name (basename of the `device/driver` symlink).
    driver: &'static str,
    /// First firmware version considered safe (dotted numeric).
    first_safe: &'static str,
    /// Operator-facing reason (driver, firmware, bug-id link).
    reason: &'static str,
    /// F-COR-7: `Some((major, minor))` = the first kernel version at/above which this driver's silent-drop window does NOT apply, used as a driver+kernel fallback key when firmware is
    /// UNRESOLVED (notably `ena` on AWS never reports one via `ethtool -i`, which left the firmware-only key permanently dead and fail-OPEN). `None` = firmware-only row.
    bad_kernel_below: Option<(u8, u8)>,
}

/// ROUND8-L4-05 source-of-truth blocklist. Best-effort — the runtime probe is the always-on backstop. Add rows as new silent-drop firmware windows are confirmed.
const BLOCKLIST: &[BlockRow] = &[
    BlockRow {
        driver: "mlx5_core",
        first_safe: "16.32.1010",
        reason: "mlx5_core firmware < 16.32.1010 silently drops XDP_REDIRECT/\
                 XDP_TX in DRV mode (aya#1193 / GHSA window). Force \
                 runtime.xdp_mode = \"skb\".",
        // mlx5 reports firmware reliably; no kernel fallback needed.
        bad_kernel_below: None,
    },
    BlockRow {
        driver: "ena",
        first_safe: "2.10.0",
        reason: "ena firmware < 2.10 on c5n/m5n silently drops native XDP \
                 in pre-2024 kernels. Force runtime.xdp_mode = \"skb\".",
        // F-COR-7: ena never reports firmware via `ethtool -i` on AWS, so the firmware key alone is permanently dead. The row's documented condition is "pre-2024 kernels" and 6.7 (Jan 2024) is the
        // first 2024 mainline line: kernel < 6.7 with unresolved firmware IS the known-bad combo, while kernel >= 6.7 stays Allowed so there is no fleet-wide native-XDP regression.
        bad_kernel_below: Some((6, 7)),
    },
    BlockRow {
        driver: "ice",
        first_safe: "4.11",
        reason: "ice firmware <= 4.10 has the Cilium-listed native-XDP \
                 regression. Force runtime.xdp_mode = \"skb\".",
        bad_kernel_below: None,
    },
];

/// F-COR-7: `(major, minor)` of the running kernel, or `None` if unresolvable. Used ONLY as the driver+kernel fallback key when firmware is unresolved on a row with `bad_kernel_below`.
fn current_kernel_mm() -> Option<(u8, u8)> {
    let kv = aya::util::KernelVersion::current().ok()?;
    // KernelVersion fields are crate-private; reconstruct (major, minor) from its public LINUX_VERSION_CODE-style `code()` (code = (major<<16) | (minor<<8) | patch, patch clamped 0..=255).
    let code = kv.code();
    let major = ((code >> 16) & 0xff) as u8;
    let minor = ((code >> 8) & 0xff) as u8;
    Some((major, minor))
}

/// `true` iff `(major, minor)` is strictly below `bound`.
fn kernel_below(k: (u8, u8), bound: (u8, u8)) -> bool {
    k.0 < bound.0 || (k.0 == bound.0 && k.1 < bound.1)
}

/// Parse a dotted-numeric version into a comparable `Vec<u64>`. Trailing non-numeric junk (e.g. `16.32.1010 (MT_0000000080)`) is truncated; missing components compare as 0.
fn parse_version(v: &str) -> Vec<u64> {
    let trimmed: String = v
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    trimmed
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u64>().unwrap_or(0))
        .collect()
}

/// `a < b` over dotted-numeric versions, shorter side zero-padded.
fn version_lt(a: &str, b: &str) -> bool {
    let pa = parse_version(a);
    let pb = parse_version(b);
    let n = pa.len().max(pb.len());
    for i in 0..n {
        let x = pa.get(i).copied().unwrap_or(0);
        let y = pb.get(i).copied().unwrap_or(0);
        if x != y {
            return x < y;
        }
    }
    false
}

/// Decide whether `Drv` is safe for `(driver, firmware)` against the static blocklist. Pure, so it is unit-testable without a real NIC.
#[must_use]
pub fn classify(driver: &str, firmware: &str) -> DrvSupport {
    for row in BLOCKLIST {
        if row.driver == driver && version_lt(firmware, row.first_safe) {
            return DrvSupport::Refuse {
                reason: format!(
                    "{} (driver={driver} firmware={firmware} < {})",
                    row.reason, row.first_safe
                ),
            };
        }
    }
    DrvSupport::Allowed
}

/// F-COR-7: driver + kernel classification used ONLY when firmware is UNRESOLVED; [`classify`] itself stays pure and UNCHANGED. For a driver carrying a `bad_kernel_below`, an unresolved
/// firmware must NOT silently fail-open — that was the dead path. Kernel below the boundary = the known-bad combo → Refuse; at/above = Allowed. Drivers without the field keep fail-open.
#[must_use]
pub fn classify_unresolved_firmware(driver: &str, kernel: Option<(u8, u8)>) -> DrvSupport {
    for row in BLOCKLIST {
        if row.driver != driver {
            continue;
        }
        let Some(bound) = row.bad_kernel_below else {
            // Firmware-only row with unresolved firmware → fail-open; the probe + alert remain the backstop.
            return DrvSupport::Allowed;
        };
        return match kernel {
            Some(k) if kernel_below(k, bound) => DrvSupport::Refuse {
                reason: format!(
                    "{} (driver={driver} firmware UNRESOLVED, kernel {}.{} \
                     < known-good {}.{}: refusing native XDP on the \
                     known-bad driver+kernel combo rather than fail-open; \
                     ROUND8-L4-05 / F-COR-7)",
                    row.reason, k.0, k.1, bound.0, bound.1
                ),
            },
            // kernel >= boundary → NOT a known-bad combo → Allowed. Kernel unknowable → fail-open: do not fleet-regress on an unprovable guess.
            _ => DrvSupport::Allowed,
        };
    }
    // No blocklist row for this driver → fail-open as before.
    DrvSupport::Allowed
}

/// Resolve the kernel driver name for `iface` from `/sys/class/net/<iface>/device/driver` (a symlink whose basename is the driver).
pub fn driver_of(iface: &str) -> Result<String, NicCompatError> {
    let link = format!("/sys/class/net/{iface}/device/driver");
    let target = fs::read_link(&link).map_err(|source| NicCompatError::DriverUnresolved {
        iface: iface.to_owned(),
        source,
    })?;
    Ok(target
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_owned())
}

/// Read the firmware-version line from `ethtool -i <iface>`. A missing `ethtool` binary or a NIC that reports no firmware is NOT fatal — the caller treats it as "could not determine".
pub fn firmware_of(iface: &str) -> Result<String, NicCompatError> {
    let out = std::process::Command::new("ethtool")
        .arg("-i")
        .arg(iface)
        .output()
        .map_err(|e| NicCompatError::FirmwareUnresolved {
            iface: iface.to_owned(),
            reason: format!("ethtool spawn failed: {e}"),
        })?;
    if !out.status.success() {
        return Err(NicCompatError::FirmwareUnresolved {
            iface: iface.to_owned(),
            reason: format!("ethtool exited {:?}", out.status.code()),
        });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_ethtool_firmware(&text).ok_or_else(|| NicCompatError::FirmwareUnresolved {
        iface: iface.to_owned(),
        reason: "no `firmware-version:` line in ethtool -i output".to_owned(),
    })
}

/// Extract the `firmware-version:` value from `ethtool -i` text.
#[must_use]
pub fn parse_ethtool_firmware(ethtool_out: &str) -> Option<String> {
    for line in ethtool_out.lines() {
        if let Some(rest) = line.strip_prefix("firmware-version:") {
            let v = rest.trim();
            if !v.is_empty() {
                return Some(v.to_owned());
            }
        }
    }
    None
}

/// Top-level gate called by `XdpLoader::attach_with_fallback` BEFORE attempting `Drv`. A resolution failure maps to [`DrvSupport::Allowed`] — `Drv` is never blocked merely because introspection failed.
pub fn drv_supported(iface: &str) -> Result<DrvSupport, NicCompatError> {
    let driver = match driver_of(iface) {
        Ok(d) => d,
        // Virtual / driverless device (dummy0 in CI, veth). No row can match; allow `Drv`.
        Err(_) => return Ok(DrvSupport::Allowed),
    };
    let firmware = match firmware_of(iface) {
        Ok(f) => f,
        // F-COR-7: firmware unresolved. This used to fail-OPEN unconditionally, which made the ena blocklist row permanently dead on real AWS; now it keys on driver+kernel.
        Err(_) => {
            return Ok(classify_unresolved_firmware(&driver, current_kernel_mm()));
        }
    };
    Ok(classify(&driver, &firmware))
}

/// Sysfs root as a single source of truth, so a future mock harness can override it.
#[must_use]
pub fn driver_link_path(iface: &str) -> std::path::PathBuf {
    Path::new("/sys/class/net")
        .join(iface)
        .join("device/driver")
}

/// ROUND8-L4-05 runtime probe scaffold: fire a synthetic packet through `BPF_PROG_TEST_RUN` and assert the action is `XDP_TX` AND the dst MAC was rewritten.
///
/// API BLOCKER: aya 0.13.1 exposes no public `test_run` wrapper on the `Xdp` program type, so this returns [`ProbeOutcome::ProbeUnavailable`] and the caller keeps the attach. The static blocklist plus the CI probe fixture and `xdp_attach_probe_failed_total` alert are the active defence until the aya API lands.
#[must_use]
pub const fn probe_xdp_silent_drop() -> ProbeOutcome {
    ProbeOutcome::ProbeUnavailable
}

#[cfg(test)]
#[allow(clippy::panic)] // crate-level lint, intentional in test code
mod tests {
    use super::*;

    #[test]
    fn version_compare_basic() {
        assert!(version_lt("16.31.0", "16.32.1010"));
        assert!(version_lt("16.32.1009", "16.32.1010"));
        assert!(!version_lt("16.32.1010", "16.32.1010"));
        assert!(!version_lt("16.33.0", "16.32.1010"));
        assert!(version_lt("2.9", "2.10.0"));
        assert!(!version_lt("2.10", "2.10.0"));
        assert!(!version_lt("2.11", "2.10.0"));
    }

    #[test]
    fn version_parse_truncates_junk() {
        assert_eq!(
            parse_version("16.32.1010 (MT_0000000080)"),
            vec![16, 32, 1010]
        );
        assert_eq!(parse_version("4.10"), vec![4, 10]);
        assert_eq!(parse_version(""), Vec::<u64>::new());
    }

    #[test]
    fn mlx5_old_firmware_refused() {
        match classify("mlx5_core", "16.31.0") {
            DrvSupport::Refuse { reason } => {
                assert!(reason.contains("mlx5_core"), "reason: {reason}");
                assert!(reason.contains("aya#1193"), "must cite bug-id: {reason}");
            }
            DrvSupport::Allowed => panic!("old mlx5 firmware must be refused"),
        }
    }

    #[test]
    fn mlx5_new_firmware_allowed() {
        assert_eq!(classify("mlx5_core", "16.35.2000"), DrvSupport::Allowed);
    }

    #[test]
    fn unknown_driver_allowed() {
        assert_eq!(classify("virtio_net", "1.0"), DrvSupport::Allowed);
        assert_eq!(classify("dummy", ""), DrvSupport::Allowed);
    }

    #[test]
    fn ena_and_ice_rows() {
        assert!(matches!(
            classify("ena", "2.9.5"),
            DrvSupport::Refuse { .. }
        ));
        assert!(matches!(classify("ice", "4.10"), DrvSupport::Refuse { .. }));
        assert_eq!(classify("ice", "4.11"), DrvSupport::Allowed);
    }

    #[test]
    fn ethtool_firmware_parse() {
        let sample = "driver: mlx5_core\nversion: 5.15.0\n\
                      firmware-version: 16.32.1010 (MT_0000000080)\n\
                      bus-info: 0000:01:00.0\n";
        assert_eq!(
            parse_ethtool_firmware(sample).as_deref(),
            Some("16.32.1010 (MT_0000000080)")
        );
        assert_eq!(parse_ethtool_firmware("driver: foo\n"), None);
    }

    #[test]
    fn probe_reports_unavailable_on_aya_013() {
        // Documents the API blocker as a behavioural contract: when aya gains a public BPF_PROG_TEST_RUN wrapper this test is the tripwire to wire the real probe.
        assert_eq!(probe_xdp_silent_drop(), ProbeOutcome::ProbeUnavailable);
    }

    #[test]
    fn classify_unchanged_is_pure_and_untouched() {
        // classify() itself MUST stay pure/unchanged by F-COR-7: firmware-keyed behaviour is identical.
        assert!(matches!(
            classify("ena", "2.9.5"),
            DrvSupport::Refuse { .. }
        ));
        assert_eq!(classify("ena", "2.11"), DrvSupport::Allowed);
        assert_eq!(classify("virtio_net", ""), DrvSupport::Allowed);
    }

    #[test]
    fn ena_unresolved_fw_modern_kernel_stays_allowed() {
        // NOT-known-bad ena box (kernel >= 6.7) with unresolved firmware stays Allowed — native XDP preserved, no fleet regression.
        assert_eq!(
            classify_unresolved_firmware("ena", Some((7, 0))),
            DrvSupport::Allowed
        );
        assert_eq!(
            classify_unresolved_firmware("ena", Some((6, 7))),
            DrvSupport::Allowed,
            "6.7 is the known-good boundary (inclusive) → Allowed"
        );
        // Kernel unknowable → cannot prove known-bad → fail-open.
        assert_eq!(
            classify_unresolved_firmware("ena", None),
            DrvSupport::Allowed
        );
    }

    #[test]
    fn ena_unresolved_fw_prebad_kernel_refuses() {
        // A synthetic KNOWN-BAD ena/kernel combo (the row's pre-2024 window) → Refuse: the previously-dead defence path genuinely fires.
        for k in [(6, 6), (6, 1), (5, 15), (5, 10)] {
            match classify_unresolved_firmware("ena", Some(k)) {
                DrvSupport::Refuse { reason } => {
                    assert!(
                        reason.contains("ena") && reason.contains("ROUND8-L4-05"),
                        "reason must cite ena + bug-id: {reason}"
                    );
                    assert!(
                        reason.contains("F-COR-7"),
                        "reason must mark the driver+kernel path: {reason}"
                    );
                }
                DrvSupport::Allowed => {
                    panic!("ena kernel {k:?} (pre-6.7, unresolved fw) must Refuse")
                }
            }
        }
    }

    #[test]
    fn non_kernel_keyed_driver_unresolved_fw_still_fail_open() {
        // Drivers that report firmware reliably (bad_kernel_below=None) keep the original fail-open on unresolved firmware, as do drivers with no row at all.
        assert_eq!(
            classify_unresolved_firmware("mlx5_core", Some((5, 15))),
            DrvSupport::Allowed
        );
        assert_eq!(
            classify_unresolved_firmware("ice", Some((5, 15))),
            DrvSupport::Allowed
        );
        assert_eq!(
            classify_unresolved_firmware("dummy", Some((5, 15))),
            DrvSupport::Allowed
        );
    }

    #[test]
    fn current_kernel_mm_resolves_on_this_box() {
        // aya KernelVersion must resolve a plausible (major, minor) so drv_supported keys correctly on the real path.
        let k = current_kernel_mm().expect("kernel version must resolve");
        assert!(k.0 >= 3, "implausible kernel major {}", k.0);
    }
}
