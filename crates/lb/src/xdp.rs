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

    /// Outcome of the CAP_BPF probe: explicit enum so we log the exact
    /// reason we skipped the attach.
    enum CapState {
        HaveBpfAndNetAdmin,
        MissingBpf,
        MissingNetAdmin,
        ProbeError(String),
    }

    fn probe_caps() -> CapState {
        let bpf = match has_cap(None, CapSet::Effective, Capability::CAP_BPF) {
            Ok(v) => v,
            Err(e) => return CapState::ProbeError(e.to_string()),
        };
        if !bpf {
            return CapState::MissingBpf;
        }
        let netadmin = match has_cap(None, CapSet::Effective, Capability::CAP_NET_ADMIN) {
            Ok(v) => v,
            Err(e) => return CapState::ProbeError(e.to_string()),
        };
        if !netadmin {
            return CapState::MissingNetAdmin;
        }
        CapState::HaveBpfAndNetAdmin
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
            CapState::HaveBpfAndNetAdmin => {}
            CapState::MissingBpf => {
                tracing::warn!(
                    xdp_enabled = false,
                    reason = "missing CAP_BPF",
                    "xdp disabled — run the binary with CAP_BPF or as root to enable"
                );
                return None;
            }
            CapState::MissingNetAdmin => {
                tracing::warn!(
                    xdp_enabled = false,
                    reason = "missing CAP_NET_ADMIN",
                    "xdp disabled — attach requires CAP_NET_ADMIN in addition to CAP_BPF"
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
