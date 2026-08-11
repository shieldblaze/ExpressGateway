//! Optional XDP data-plane attach (Linux-only; the non-Linux path is a stub returning `None`).
//!
//! Attaches only when `[runtime].xdp_enabled`, `CAP_BPF` + `CAP_NET_ADMIN`, and `cfg(lb_xdp_elf)` all hold; the returned `XdpLoader` guard must be kept alive until shutdown. On any missing precondition it logs and returns `None` — never panics, never errors.

#[cfg(target_os = "linux")]
pub use linux::try_attach_xdp;

/// SEC-2-11: re-exports for the capability-probe integration test, which exercises the fallback policy without requiring real capability changes in CI.
#[cfg(target_os = "linux")]
pub mod cap_probe {
    // Only the integration test imports these; suppress the unused-import lint for the binary.
    #[allow(unused_imports)]
    pub use super::linux::{CapMode, CapState, probe_caps_with};
}

#[cfg(not(target_os = "linux"))]
pub fn try_attach_xdp(_: &lb_config::RuntimeConfig) -> Option<()> {
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use caps::{CapSet, Capability, has_cap};
    use lb_config::{RuntimeConfig, XdpModeChoice as CfgXdpModeChoice};
    use lb_l4_xdp::loader::XdpLoader;
    #[cfg(lb_xdp_elf)]
    use lb_l4_xdp::loader::XdpModeChoice as LoaderXdpModeChoice;

    /// EBPF-2-04: translate the operator-facing config enum into the loader's mode choice. Two separate types avoid an `lb-l4-xdp` <-> `lb-config` cyclic dep, so this conversion is the one place they must stay in sync.
    #[cfg(lb_xdp_elf)]
    const fn cfg_to_loader_mode(c: CfgXdpModeChoice) -> LoaderXdpModeChoice {
        match c {
            CfgXdpModeChoice::Auto => LoaderXdpModeChoice::Auto,
            CfgXdpModeChoice::Native => LoaderXdpModeChoice::Native,
            CfgXdpModeChoice::Skb => LoaderXdpModeChoice::Skb,
            CfgXdpModeChoice::Hw => LoaderXdpModeChoice::Hw,
        }
    }

    /// SEC-2-11: which capability path the probe accepted. Kernels >= 5.8 split `CAP_BPF` out of `CAP_SYS_ADMIN`; older kernels do not know the bit at all and the `caps` crate reports
    /// `Ok(false)` for it, so "no CAP_BPF" is treated as a signal to try the legacy `CAP_SYS_ADMIN` path rather than as a failure.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CapMode {
        /// `CAP_BPF` + `CAP_NET_ADMIN` (preferred, ≥5.8 kernels).
        BpfPlusNetAdmin,
        /// `CAP_SYS_ADMIN` (legacy, pre-5.8 kernels).
        SysAdmin,
    }

    /// Outcome of the capability probe — an explicit enum so the skip reason is logged exactly.
    #[derive(Debug)]
    pub enum CapState {
        /// Probe succeeded; carries the accepted mode for the log line.
        Ok(CapMode),
        /// Neither `CAP_BPF` nor `CAP_SYS_ADMIN` is present.
        MissingBpfAndSysAdmin,
        /// `CAP_BPF` present but `CAP_NET_ADMIN` missing (and no `CAP_SYS_ADMIN` fallback).
        MissingNetAdmin,
        /// The capability probe itself failed (e.g. capget(2) error).
        ProbeError(String),
    }

    /// SEC-2-11: capability probe with `CAP_SYS_ADMIN` fallback (`check` is injected so tests can exercise every branch without root). If `CAP_BPF` is held, `CAP_NET_ADMIN` is also required
    /// (the preferred >= 5.8 posture); otherwise fall back to `CAP_SYS_ADMIN`, the only legal path on pre-5.8 kernels. Probe errors are swallowed for `CAP_BPF` ONLY — `CapsError` is opaque and
    /// a kernel that does not know the bit is exactly what the fallback is for; an error on `CAP_SYS_ADMIN` or `CAP_NET_ADMIN` DOES surface, because by then there are no fallbacks left.
    pub fn probe_caps_with<F>(mut check: F) -> CapState
    where
        F: FnMut(Capability) -> Result<bool, String>,
    {
        // Any error or `Ok(false)` means not available — a kernel that does not know CAP_BPF reports `Ok(false)`, so there is no distinguishable error path. The string is captured only so the fallback can include it if both paths fail.
        let bpf_result = check(Capability::CAP_BPF);
        let bpf_ok = matches!(bpf_result, Ok(true));

        if bpf_ok {
            match check(Capability::CAP_NET_ADMIN) {
                Ok(true) => return CapState::Ok(CapMode::BpfPlusNetAdmin),
                Ok(false) => {
                    // We hold CAP_BPF but not CAP_NET_ADMIN. Last chance: CAP_SYS_ADMIN may cover it.
                    if let Ok(true) = check(Capability::CAP_SYS_ADMIN) {
                        return CapState::Ok(CapMode::SysAdmin);
                    }
                    return CapState::MissingNetAdmin;
                }
                Err(e) => return CapState::ProbeError(e),
            }
        }

        // Fall back to CAP_SYS_ADMIN: the only path on pre-5.8 kernels, and still valid on 5.8+ where CAP_BPF simply was not granted.
        match check(Capability::CAP_SYS_ADMIN) {
            Ok(true) => CapState::Ok(CapMode::SysAdmin),
            Ok(false) => CapState::MissingBpfAndSysAdmin,
            Err(e) => {
                // A CAP_BPF error string is more diagnostic; otherwise surface this one.
                if let Err(bpf_err) = bpf_result {
                    CapState::ProbeError(format!(
                        "cap_bpf probe failed ({bpf_err}); cap_sys_admin probe failed ({e})",
                    ))
                } else {
                    CapState::ProbeError(e)
                }
            }
        }
    }

    /// Production wiring: delegates to [`caps::has_cap`].
    fn probe_caps() -> CapState {
        probe_caps_with(|cap| has_cap(None, CapSet::Effective, cap).map_err(|e| e.to_string()))
    }

    /// Attempt the XDP attach. `Some(loader)` only when everything worked; logs otherwise.
    pub fn try_attach_xdp(rt: &RuntimeConfig) -> Option<XdpLoader> {
        if !rt.xdp_enabled {
            tracing::debug!("xdp: disabled by config");
            return None;
        }
        let Some(iface) = rt.xdp_interface.as_deref().filter(|s| !s.is_empty()) else {
            tracing::warn!("xdp_enabled=true but xdp_interface is empty; continuing without XDP");
            return None;
        };

        match probe_caps() {
            CapState::Ok(CapMode::BpfPlusNetAdmin) => {
                tracing::info!(
                    cap_mode = "cap_bpf+cap_net_admin",
                    "xdp: capability probe succeeded (modern ≥5.8 path)"
                );
            }
            CapState::Ok(CapMode::SysAdmin) => {
                // The fallback for 5.4-5.7 distros and for `--cap-add SYS_ADMIN`. INFO, not WARN: granting CAP_SYS_ADMIN explicitly is a clear operator intent.
                tracing::info!(
                    cap_mode = "cap_sys_admin",
                    "xdp: capability probe succeeded via legacy CAP_SYS_ADMIN path \
                     (pre-5.8 kernel or operator-granted)"
                );
            }
            CapState::MissingBpfAndSysAdmin => {
                tracing::warn!(
                    xdp_enabled = false,
                    reason = "missing CAP_BPF and CAP_SYS_ADMIN",
                    "xdp disabled — run the binary with CAP_BPF (kernel ≥5.8) \
                     or CAP_SYS_ADMIN (pre-5.8 fallback), each paired with CAP_NET_ADMIN"
                );
                return None;
            }
            CapState::MissingNetAdmin => {
                tracing::warn!(
                    xdp_enabled = false,
                    reason = "missing CAP_NET_ADMIN",
                    "xdp disabled — CAP_BPF requires CAP_NET_ADMIN for attach; \
                     grant CAP_NET_ADMIN or fall back to CAP_SYS_ADMIN"
                );
                return None;
            }
            CapState::ProbeError(e) => {
                tracing::warn!(
                    xdp_enabled = false,
                    error = %e,
                    "xdp disabled — capability probe failed"
                );
                return None;
            }
        }

        attach_with_elf(iface, rt.xdp_mode)
    }

    /// EBPF-2-04: probe the attach-mode ladder (Drv -> Skb for `Auto`; loud-fail for `Native`/`Hw`), recording the chosen mode so the Prom scrape need not re-query the kernel.
    #[cfg(lb_xdp_elf)]
    fn attach_with_elf(iface: &str, mode: CfgXdpModeChoice) -> Option<XdpLoader> {
        let mut loader = match XdpLoader::load_from_bytes(lb_l4_xdp::LB_XDP_ELF) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!(error = %e, "xdp disabled — loader parse failed");
                return None;
            }
        };
        if let Err(e) = loader.kernel_load("lb_xdp") {
            tracing::warn!(error = %e, "xdp disabled — kernel_load(lb_xdp) failed");
            return None;
        }
        let requested = cfg_to_loader_mode(mode);
        match loader.attach_with_fallback("lb_xdp", iface, requested) {
            Ok(outcome) => {
                tracing::info!(
                    interface = iface,
                    mode = outcome.mode.to_label().as_str(),
                    attempts = outcome.attempts,
                    "xdp: program 'lb_xdp' attached via probe ladder"
                );
                Some(loader)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    interface = iface,
                    requested = ?mode,
                    "xdp disabled — attach ladder failed"
                );
                None
            }
        }
    }

    #[cfg(not(lb_xdp_elf))]
    fn attach_with_elf(_iface: &str, _mode: CfgXdpModeChoice) -> Option<XdpLoader> {
        tracing::warn!(
            "xdp_enabled=true but no ELF was built into this binary; \
             run scripts/build-xdp.sh and rebuild to enable"
        );
        None
    }
}
