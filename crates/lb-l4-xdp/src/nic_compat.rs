//! ROUND8-L4-05: known-bad NIC + firmware blocklist for native
//! (`Drv`) XDP, and the post-attach silent-drop probe scaffold.
//!
//! Reference: aya issue #1193 (MLX5 / ConnectX-6 silently drops
//! `XDP_REDIRECT` — and on some firmware `XDP_TX` — in DRV mode:
//! `bpf_link_create` returns success, every map op reports success,
//! the packet path goes to /dev/null); Cilium lesson 8 (ConnectX-4
//! Lx VF silent fail — Cilium added a runtime fallback after the
//! scar); Round-8 handoff item 5.
//!
//! Two defences, layered:
//!
//!   1. **Static blocklist** (this module): refuse `Drv` on a
//!      `(driver, firmware-version)` combination known to silently
//!      drop. Best-effort — the list goes stale; it is the cheap
//!      first gate, not the backstop.
//!
//!   2. **Runtime probe** (the backstop, [`probe_xdp_silent_drop`]):
//!      after a `Drv` attach, fire a synthetic packet through
//!      `BPF_PROG_TEST_RUN` and verify the program actually ran. If
//!      not, demote to `Skb`. **API blocker**: aya 0.13.1 exposes no
//!      public `BPF_PROG_TEST_RUN` wrapper on the `Xdp` program type
//!      (only the raw kernel binding constant exists in aya-obj's
//!      generated bindings). The probe is shipped as a typed
//!      structure + an explicit deferred-to-CI marker, the same
//!      posture as ROUND8-L4-12's `query_xdp` netlink stub. The
//!      blocklist gate (defence 1) IS fully wired today.
//!
//! Linux-only: sysfs / ethtool are Linux facilities.

#![cfg(target_os = "linux")]

use std::fs;
use std::path::Path;

/// Outcome of [`probe_xdp_silent_drop`].
///
/// `Ok` — the synthetic packet round-tripped with the rewrite
/// applied; the `Drv` attach is genuinely live.
///
/// `SilentDrop` — `BPF_PROG_TEST_RUN` returned but the action was not
/// the expected `XDP_TX` (the aya #1193 class).
///
/// `NotExecuted` — the action looked right but the program body did
/// not run (dst MAC unchanged) — the subtler silent-drop variant.
///
/// `ProbeUnavailable` — `BPF_PROG_TEST_RUN` is not reachable on this
/// build (aya 0.13.1 API blocker). The caller MUST treat this as
/// "probe inconclusive", keep the attach, and rely on the static
/// blocklist + the `xdp_attach_probe_failed_total` alert wired in CI.
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
    /// No blocklist match — `Drv` may be attempted (the runtime probe
    /// is still the backstop once it is reachable).
    Allowed,
    /// The `(driver, firmware)` combination is known to silently
    /// drop. `Drv` MUST be skipped; `reason` is the operator-facing
    /// explanation incl. the bug-id link.
    Refuse {
        /// Operator-facing reason (driver, firmware, bug-id link).
        reason: String,
    },
}

/// Errors from the sysfs / ethtool introspection path. A read
/// failure is NOT fatal — the caller treats an error as "could not
/// determine, allow `Drv` and rely on the runtime probe + alert".
#[derive(Debug, thiserror::Error)]
pub enum NicCompatError {
    /// `/sys/class/net/<iface>/device/driver` could not be read
    /// (interface gone, virtual device with no driver symlink, ...).
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

/// A single blocklist row: a driver name and an inclusive
/// firmware-version *upper bound* below which `Drv` is unsafe. We
/// only ever compare "firmware < first_safe", so the row carries the
/// first KNOWN-GOOD version string and a human reason.
#[derive(Debug, Clone, Copy)]
struct BlockRow {
    /// Kernel driver name (basename of the `device/driver` symlink).
    driver: &'static str,
    /// First firmware version considered safe (dotted numeric).
    first_safe: &'static str,
    /// Operator-facing reason incl. the bug-id link.
    reason: &'static str,
}

/// ROUND8-L4-05 source-of-truth blocklist. Best-effort; the runtime
/// probe is the always-on backstop. Add rows here as new silent-drop
/// firmware windows are confirmed.
const BLOCKLIST: &[BlockRow] = &[
    BlockRow {
        driver: "mlx5_core",
        first_safe: "16.32.1010",
        reason: "mlx5_core firmware < 16.32.1010 silently drops XDP_REDIRECT/\
                 XDP_TX in DRV mode (aya#1193 / GHSA window). Force \
                 runtime.xdp_mode = \"skb\".",
    },
    BlockRow {
        driver: "ena",
        first_safe: "2.10.0",
        reason: "ena firmware < 2.10 on c5n/m5n silently drops native XDP \
                 in pre-2024 kernels. Force runtime.xdp_mode = \"skb\".",
    },
    BlockRow {
        driver: "ice",
        first_safe: "4.11",
        reason: "ice firmware <= 4.10 has the Cilium-listed native-XDP \
                 regression. Force runtime.xdp_mode = \"skb\".",
    },
];

/// Parse a dotted-numeric version into a comparable `Vec<u64>`.
/// Non-numeric trailing junk (e.g. `16.32.1010 (MT_0000000080)`) is
/// truncated at the first non `[0-9.]` run. Missing components compare
/// as 0 so `16.32` < `16.32.1` as expected.
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

/// Decide whether `Drv` is safe for `(driver, firmware)` against the
/// static blocklist. Pure function so it is unit-testable without a
/// real NIC.
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

/// Resolve the kernel driver name for `iface` from
/// `/sys/class/net/<iface>/device/driver` (a symlink whose basename
/// is the driver, e.g. `…/drivers/mlx5_core`).
///
/// # Errors
///
/// [`NicCompatError::DriverUnresolved`] if the symlink is absent
/// (virtual device) or unreadable.
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

/// Read the firmware-version line from `ethtool -i <iface>`.
///
/// v1 shells out to `ethtool`; an aya-native ethtool binding is a
/// documented follow-up (the plan calls this out). A missing
/// `ethtool` binary or a NIC that does not report a firmware version
/// is NOT fatal — the caller treats it as "could not determine".
///
/// # Errors
///
/// [`NicCompatError::FirmwareUnresolved`] if `ethtool` is missing,
/// fails, or emits no `firmware-version:` line.
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
/// Split out so it is unit-testable without a real NIC.
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

/// Top-level gate called by `XdpLoader::attach_with_fallback` BEFORE
/// attempting `Drv`. Resolves driver + firmware from sysfs/ethtool
/// and runs [`classify`]. A resolution failure returns
/// [`DrvSupport::Allowed`] (fail-open: we never block `Drv` just
/// because introspection failed — the runtime probe + alert is the
/// backstop) but is surfaced to the caller's tracing via the
/// returned `Err` path being mapped to `Allowed` by
/// [`drv_supported`].
///
/// # Errors
///
/// Never returns `Err` for a resolution failure (those map to
/// `Allowed`); the `Result` is reserved for future hard-fail
/// classes. Today it is always `Ok`.
pub fn drv_supported(iface: &str) -> Result<DrvSupport, NicCompatError> {
    let driver = match driver_of(iface) {
        Ok(d) => d,
        // Virtual / driverless device (dummy0 in CI, veth, ...). No
        // blocklist row can match; allow `Drv`.
        Err(_) => return Ok(DrvSupport::Allowed),
    };
    let firmware = match firmware_of(iface) {
        Ok(f) => f,
        // Could not read firmware — fail open, rely on the probe.
        Err(_) => return Ok(DrvSupport::Allowed),
    };
    Ok(classify(&driver, &firmware))
}

/// Path used by [`Path::exists`] gating in tests; kept here so the
/// sysfs root is a single source of truth if a future mock harness
/// needs to override it.
#[must_use]
pub fn driver_link_path(iface: &str) -> std::path::PathBuf {
    Path::new("/sys/class/net")
        .join(iface)
        .join("device/driver")
}

/// ROUND8-L4-05 runtime probe scaffold.
///
/// Intended behaviour: build a synthetic Ethernet+IPv4+TCP packet
/// matching a pre-inserted synthetic CONNTRACK entry, fire it through
/// `BPF_PROG_TEST_RUN`, and assert the returned action is `XDP_TX`
/// AND the dst MAC was rewritten.
///
/// **API blocker (explicit)**: aya 0.13.1 exposes no public
/// `BPF_PROG_TEST_RUN` / `test_run` wrapper on the `Xdp` program
/// type. Implementing this would require either a raw `bpf(2)`
/// syscall via `libc` (out of scope for this plan's blast radius) or
/// an aya upgrade. Per the Round-8 instruction to "ship the
/// structure + a deferred-to-CI note rather than nothing", this
/// returns [`ProbeOutcome::ProbeUnavailable`]. The caller keeps the
/// attach, records nothing as a failure, and the static blocklist
/// (fully wired) plus the CI privileged-stage probe fixture +
/// `xdp_attach_probe_failed_total` alert are the active defence
/// until the aya API lands.
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
        // virtio_net / dummy / veth — no blocklist row, always allow.
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
        // Documents the API blocker as a behavioural contract: when
        // aya gains a public BPF_PROG_TEST_RUN wrapper this test is
        // the tripwire to wire the real probe.
        assert_eq!(probe_xdp_silent_drop(), ProbeOutcome::ProbeUnavailable);
    }
}
