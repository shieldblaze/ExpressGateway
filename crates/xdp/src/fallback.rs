//! Fallback detection for XDP availability.
//!
//! When XDP is not available (non-Linux platform, missing driver support, etc.),
//! ExpressGateway falls back to normal kernel-stack packet processing.

/// Returns `true` if XDP is available on the current platform.
///
/// On Linux, this checks whether the platform is capable of running XDP programs.
/// On non-Linux platforms, this always returns `false`.
pub fn is_xdp_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        // On Linux, XDP is theoretically available. A more thorough check would
        // probe the kernel version and driver capabilities, but for now we report
        // true and let attach() surface actual errors.
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Log a warning indicating that XDP is not available and the gateway will
/// operate in fallback (kernel-stack) mode.
pub fn warn_fallback() {
    tracing::warn!(
        "XDP acceleration is not available; falling back to kernel-stack packet processing. \
         L4 forwarding will use standard socket I/O."
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdp_available_matches_platform() {
        let available = is_xdp_available();
        if cfg!(target_os = "linux") {
            assert!(available, "XDP should be available on Linux");
        } else {
            assert!(!available, "XDP should not be available on non-Linux");
        }
    }

    #[test]
    fn warn_fallback_does_not_panic() {
        // Just ensure calling the function doesn't panic.
        warn_fallback();
    }
}
