//! Optional XDP data-plane attach (Pillar 4b-1).
//!
//! When `[runtime].xdp_enabled = true` in the config AND `CAP_BPF` +
//! `CAP_NET_ADMIN` are present AND the BPF ELF was compiled into the
//! binary (`cfg(lb_xdp_elf)`), `try_attach_xdp` loads and attaches the
//! program, returning an `XdpLoader` guard the caller must keep alive
//! until shutdown. On any missing precondition it logs a warning and
//! returns `None` — never panics, never returns an error.
//!
//! Linux-only; the caller gates this module with `#[cfg(target_os =
//! "linux")]`. The non-Linux path is a trivial stub that always returns
//! `None`.

#[cfg(target_os = "linux")]
pub use linux::try_attach_xdp;

/// SEC-2-11: re-exports for the capability-probe integration test.
///
/// Kept behind `cfg(target_os = "linux")` because the underlying
/// types only exist there, and behind a small `pub` surface because
/// the only consumer is `tests/xdp_cap_probe.rs` (in this crate)
/// which exercises the fallback policy without requiring real
/// capability changes in CI.
#[cfg(target_os = "linux")]
pub mod cap_probe {
    // The binary build does not import these — only the integration
    // test under `crates/lb/tests/xdp_cap_probe.rs` does. Suppress
    // the unused-imports lint that fires for the binary target.
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

    /// EBPF-2-04: translate the operator-facing config enum into the
    /// loader's mode choice. Two separate types avoid a `lb-l4-xdp ↔
    /// lb-config` cyclic dep; the conversion is the only place they
    /// need to stay in sync.
    #[cfg(lb_xdp_elf)]
    const fn cfg_to_loader_mode(c: CfgXdpModeChoice) -> LoaderXdpModeChoice {
        match c {
            CfgXdpModeChoice::Auto => LoaderXdpModeChoice::Auto,
            CfgXdpModeChoice::Native => LoaderXdpModeChoice::Native,
            CfgXdpModeChoice::Skb => LoaderXdpModeChoice::Skb,
            CfgXdpModeChoice::Hw => LoaderXdpModeChoice::Hw,
        }
    }

    /// SEC-2-11: which capability path the probe accepted.
    ///
    /// Modern (≥5.8) kernels split `CAP_BPF` out of `CAP_SYS_ADMIN`,
    /// so the preferred posture is `CAP_BPF + CAP_NET_ADMIN`. Older
    /// kernels (5.4 – 5.7, e.g. RHEL 8 / Amazon Linux 2 derivatives)
    /// do not know `CAP_BPF`; the `caps` crate returns `Ok(false)` for
    /// the bit on those kernels because the kernel bitmap simply has
    /// that index clear. We therefore treat "no CAP_BPF" as a signal
    /// to try the `CAP_SYS_ADMIN` legacy path, not as an immediate
    /// failure.
    ///
    /// The README still documents 5.15 as the *supported* floor.
    /// This fallback exists so that operators on 5.4 distros get a
    /// useful diagnostic ("requires CAP_SYS_ADMIN") instead of an
    /// opaque "missing CAP_BPF".
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CapMode {
        /// `CAP_BPF` + `CAP_NET_ADMIN` (preferred, ≥5.8 kernels).
        BpfPlusNetAdmin,
        /// `CAP_SYS_ADMIN` (legacy, pre-5.8 kernels). When present it
        /// covers both BPF and NET_ADMIN authority.
        SysAdmin,
    }

    /// Outcome of the capability probe: explicit enum so we log the exact
    /// reason we skipped the attach.
    #[derive(Debug)]
    pub enum CapState {
        /// Probe succeeded; carries the accepted mode for the log line.
        Ok(CapMode),
        /// Neither `CAP_BPF` nor `CAP_SYS_ADMIN` is present.
        MissingBpfAndSysAdmin,
        /// `CAP_BPF` is present but `CAP_NET_ADMIN` is not (and we are
        /// not falling back to `CAP_SYS_ADMIN` because that's also
        /// missing). This is the "almost-there" case.
        MissingNetAdmin,
        /// The capability probe itself failed (e.g. capget(2) error).
        ProbeError(String),
    }

    /// SEC-2-11: capability probe with `CAP_SYS_ADMIN` fallback.
    ///
    /// `check` is a closure that returns whether the current task
    /// holds the given capability, or an opaque error if the probe
    /// could not run. In production this is wired to
    /// [`caps::has_cap`]; tests substitute a closure that maps a fake
    /// capability set to allow exercising every branch without root.
    ///
    /// Policy:
    /// 1. If `CAP_BPF` is held, also require `CAP_NET_ADMIN`. This is
    ///    the preferred ≥5.8 posture.
    /// 2. Otherwise (no `CAP_BPF`, OR the probe errored out), try
    ///    `CAP_SYS_ADMIN`. On pre-5.8 kernels this is the only legal
    ///    path; it implies both BPF and NET_ADMIN authority so we do
    ///    not require a separate `CAP_NET_ADMIN` check.
    /// 3. If neither path is open, return the most-informative state.
    ///
    /// We deliberately swallow probe errors *for `CAP_BPF` only* and
    /// fall through to the `CAP_SYS_ADMIN` path: `CapsError` is
    /// opaque and a kernel that doesn't know `CAP_BPF` is precisely
    /// the case the fallback exists for. A probe error on
    /// `CAP_SYS_ADMIN` (or `CAP_NET_ADMIN`) does surface as
    /// `ProbeError`, because by then we have no more fallbacks.
    pub fn probe_caps_with<F>(mut check: F) -> CapState
    where
        F: FnMut(Capability) -> Result<bool, String>,
    {
        // Step 1: try CAP_BPF. Treat any error or `Ok(false)` as
        // "not available" — kernels that don't know CAP_BPF report
        // `Ok(false)` (the bit is simply not set in the bitmap) so
        // there's no distinguishable error path here. We capture the
        // error string only so the fallback can include it if both
        // paths fail.
        let bpf_result = check(Capability::CAP_BPF);
        let bpf_ok = matches!(bpf_result, Ok(true));

        if bpf_ok {
            match check(Capability::CAP_NET_ADMIN) {
                Ok(true) => return CapState::Ok(CapMode::BpfPlusNetAdmin),
                Ok(false) => {
                    // We hold CAP_BPF but not CAP_NET_ADMIN. Last
                    // chance: maybe CAP_SYS_ADMIN is also held and
                    // covers NET_ADMIN. Otherwise report the
                    // "almost-there" case.
                    if let Ok(true) = check(Capability::CAP_SYS_ADMIN) {
                        return CapState::Ok(CapMode::SysAdmin);
                    }
                    return CapState::MissingNetAdmin;
                }
                Err(e) => return CapState::ProbeError(e),
            }
        }

        // Step 2: fall back to CAP_SYS_ADMIN. On pre-5.8 kernels this
        // is the only path that can succeed; on a 5.8+ kernel where
        // CAP_BPF was simply not granted, CAP_SYS_ADMIN still works
        // (it implies BPF authority for backwards compatibility).
        match check(Capability::CAP_SYS_ADMIN) {
            Ok(true) => CapState::Ok(CapMode::SysAdmin),
            Ok(false) => CapState::MissingBpfAndSysAdmin,
            Err(e) => {
                // If CAP_BPF errored too, that string is more
                // diagnostic; otherwise surface the CAP_SYS_ADMIN
                // error.
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

    /// Attempt the XDP attach. Returns `Some(loader)` only when everything
    /// worked; logs and returns `None` otherwise.
    pub fn try_attach_xdp(rt: &RuntimeConfig) -> Option<XdpLoader> {
        if !rt.xdp_enabled {
            tracing::debug!("xdp: disabled by config");
            return None;
        }
        let Some(iface) = rt.xdp_interface.as_deref().filter(|s| !s.is_empty()) else {
            // Config validation should have caught this; belt-and-braces.
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
                // README documents the 5.15 floor; this branch is the
                // fallback for 5.4–5.7 distros and for operators who
                // ran the binary with `--cap-add SYS_ADMIN`. We log
                // INFO (not WARN) because the operator's intent is
                // clear when CAP_SYS_ADMIN is granted explicitly.
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

    /// Select the compiled-in ELF path.
    ///
    /// EBPF-2-04: probe the attach-mode ladder (Drv → Skb for
    /// `Auto`; loud-fail for `Native`/`Hw`). The chosen mode is
    /// recorded via `stats_export::record_attach_mode` so the
    /// Prom scrape sees it without re-querying the kernel.
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
                // `record_attach_mode` is also called inside the
                // loader on success; calling it here too is a no-op
                // because the byte store is idempotent. Logging
                // duplicates info::xdp attached from the loader but
                // this line is tied to the lb-side startup sequence.
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
