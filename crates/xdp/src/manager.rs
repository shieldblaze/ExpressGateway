//! XDP program manager (userspace side).
//!
//! Handles loading, attaching, and detaching XDP programs, as well as
//! populating and reading the shared BPF maps.
//!
//! On Linux, this uses the `aya` crate for pure-Rust eBPF program management.
//! On non-Linux platforms, all operations return `XdpError::PlatformUnavailable`.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::error::{Result, XdpError};
use crate::maps::{AclKey, AclValue, BackendEntry, TcpConnKey, TcpConnValue, XdpStats};

/// XDP attach mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XdpMode {
    /// Driver/native mode -- best performance, requires driver support.
    Driver,
    /// Generic/SKB mode -- works with all drivers, slower.
    Generic,
}

impl std::fmt::Display for XdpMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            XdpMode::Driver => write!(f, "driver"),
            XdpMode::Generic => write!(f, "generic"),
        }
    }
}

/// Manages the lifecycle of XDP programs attached to a network interface.
///
/// The manager holds references to the loaded BPF program and its maps.
/// All map operations check `is_attached()` before proceeding.
pub struct XdpManager {
    interface: String,
    mode: XdpMode,
    attached: AtomicBool,
}

impl XdpManager {
    /// Create a new XDP manager for the given network interface and mode.
    pub fn new(interface: &str, mode: XdpMode) -> Self {
        Self {
            interface: interface.to_owned(),
            mode,
            attached: AtomicBool::new(false),
        }
    }

    /// Returns the network interface name.
    #[inline]
    pub fn interface(&self) -> &str {
        &self.interface
    }

    /// Returns the XDP attach mode.
    #[inline]
    pub fn mode(&self) -> XdpMode {
        self.mode
    }

    /// Returns whether an XDP program is currently attached.
    #[inline]
    pub fn is_attached(&self) -> bool {
        self.attached.load(Ordering::Acquire)
    }

    /// Verify the program is attached before a map operation.
    #[inline]
    fn require_attached(&self) -> Result<()> {
        if !self.is_attached() {
            return Err(XdpError::NotAttached {
                interface: self.interface.clone(),
            });
        }
        Ok(())
    }

    /// Load and attach XDP programs to the configured interface.
    ///
    /// On Linux: Loads the compiled BPF ELF and attaches it with the chosen
    /// XDP flags. If no compiled BPF object is available, returns an error
    /// indicating stub mode.
    ///
    /// On non-Linux: Returns `XdpError::PlatformUnavailable`.
    #[cfg(target_os = "linux")]
    pub fn attach(&self) -> Result<()> {
        if self.attached.load(Ordering::Acquire) {
            return Err(XdpError::AlreadyAttached {
                interface: self.interface.clone(),
            });
        }

        tracing::info!(
            interface = %self.interface,
            mode = %self.mode,
            "Attaching XDP program"
        );

        // In a real implementation this would:
        // 1. Load the compiled BPF ELF via aya::Ebpf::load()
        // 2. Retrieve the xdp program section
        // 3. Attach it to self.interface with the chosen XdpFlags
        //
        // Since we don't have a compiled BPF object, log a warning and
        // report the attach as failed.
        tracing::warn!(
            "No compiled BPF object available; XDP attach is a stub. \
             Falling back to kernel-stack processing."
        );

        Err(XdpError::LoadFailed {
            reason: "no compiled BPF object available (stub)".to_owned(),
        })
    }

    #[cfg(not(target_os = "linux"))]
    pub fn attach(&self) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Detach XDP programs from the interface.
    #[cfg(target_os = "linux")]
    pub fn detach(&self) -> Result<()> {
        if !self.attached.load(Ordering::Acquire) {
            return Err(XdpError::NotAttached {
                interface: self.interface.clone(),
            });
        }

        tracing::info!(
            interface = %self.interface,
            "Detaching XDP program"
        );

        self.attached.store(false, Ordering::Release);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn detach(&self) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Update the backend array map from load balancer decisions.
    #[cfg(target_os = "linux")]
    pub fn update_backends(&self, backends: &[BackendEntry]) -> Result<()> {
        self.require_attached()?;

        tracing::debug!(count = backends.len(), "Updating XDP backend table");

        // Real implementation: iterate `backends` and write each entry
        // into the BPF_MAP_TYPE_ARRAY via aya's Array API.
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn update_backends(&self, _backends: &[BackendEntry]) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Insert a new TCP connection mapping into the connection-tracking map.
    #[cfg(target_os = "linux")]
    pub fn insert_tcp_connection(&self, key: &TcpConnKey, value: &TcpConnValue) -> Result<()> {
        self.require_attached()?;

        tracing::trace!(
            src_ip = key.src_ip,
            src_port = key.src_port,
            dst_ip = key.dst_ip,
            dst_port = key.dst_port,
            backend_ip = value.backend_ip,
            backend_port = value.backend_port,
            "Inserting TCP connection into XDP map"
        );

        // Real implementation: aya HashMap::insert(key, value, 0)
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn insert_tcp_connection(&self, _key: &TcpConnKey, _value: &TcpConnValue) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Remove a TCP connection mapping from the connection-tracking map.
    #[cfg(target_os = "linux")]
    pub fn remove_tcp_connection(&self, key: &TcpConnKey) -> Result<()> {
        self.require_attached()?;

        tracing::trace!(
            src_ip = key.src_ip,
            src_port = key.src_port,
            "Removing TCP connection from XDP map"
        );

        // Real implementation: aya HashMap::remove(key)
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn remove_tcp_connection(&self, _key: &TcpConnKey) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Update ACL rules in the LPM trie map.
    #[cfg(target_os = "linux")]
    pub fn update_acl(&self, rules: &[(AclKey, AclValue)]) -> Result<()> {
        self.require_attached()?;

        tracing::debug!(count = rules.len(), "Updating XDP ACL trie");

        // Real implementation: iterate rules and insert into BPF_MAP_TYPE_LPM_TRIE
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn update_acl(&self, _rules: &[(AclKey, AclValue)]) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Mark a port as requiring L7 processing. Packets destined for this port
    /// will receive `XDP_PASS` instead of being forwarded in the kernel.
    #[cfg(target_os = "linux")]
    pub fn add_l7_port(&self, port: u16) -> Result<()> {
        self.require_attached()?;

        tracing::debug!(port, "Marking port for L7 processing (XDP_PASS)");

        // Real implementation: insert port into a BPF_MAP_TYPE_HASH set
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn add_l7_port(&self, _port: u16) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Remove a port from L7 processing.
    #[cfg(target_os = "linux")]
    pub fn remove_l7_port(&self, port: u16) -> Result<()> {
        self.require_attached()?;

        tracing::debug!(port, "Removing port from L7 processing");

        // Real implementation: remove port from BPF_MAP_TYPE_HASH set
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn remove_l7_port(&self, _port: u16) -> Result<()> {
        Err(XdpError::PlatformUnavailable)
    }

    /// Read accumulated XDP statistics.
    ///
    /// On real hardware, this reads per-CPU array map values and sums them.
    #[cfg(target_os = "linux")]
    pub fn read_stats(&self) -> Result<XdpStats> {
        self.require_attached()?;

        // Real implementation: read per-CPU array map and sum the values.
        Ok(XdpStats::default())
    }

    #[cfg(not(target_os = "linux"))]
    pub fn read_stats(&self) -> Result<XdpStats> {
        Err(XdpError::PlatformUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_creation() {
        let mgr = XdpManager::new("eth0", XdpMode::Driver);
        assert_eq!(mgr.interface(), "eth0");
        assert_eq!(mgr.mode(), XdpMode::Driver);
        assert!(!mgr.is_attached());
    }

    #[test]
    fn manager_creation_generic() {
        let mgr = XdpManager::new("lo", XdpMode::Generic);
        assert_eq!(mgr.interface(), "lo");
        assert_eq!(mgr.mode(), XdpMode::Generic);
        assert!(!mgr.is_attached());
    }

    #[test]
    fn xdp_mode_display() {
        assert_eq!(XdpMode::Driver.to_string(), "driver");
        assert_eq!(XdpMode::Generic.to_string(), "generic");
    }

    #[test]
    fn attach_stub_returns_error() {
        let mgr = XdpManager::new("eth0", XdpMode::Driver);
        let result = mgr.attach();
        assert!(result.is_err());
    }

    #[test]
    fn detach_without_attach_fails() {
        let mgr = XdpManager::new("eth0", XdpMode::Driver);
        let result = mgr.detach();
        assert!(result.is_err());
    }

    #[test]
    fn operations_fail_when_not_attached() {
        let mgr = XdpManager::new("eth0", XdpMode::Driver);

        assert!(mgr.update_backends(&[]).is_err());

        let key = crate::maps::TcpConnKey {
            src_ip: 0,
            src_port: 0,
            dst_ip: 0,
            dst_port: 0,
        };
        let value = crate::maps::TcpConnValue {
            backend_ip: 0,
            backend_port: 0,
            state: 0,
        };
        assert!(mgr.insert_tcp_connection(&key, &value).is_err());
        assert!(mgr.remove_tcp_connection(&key).is_err());
        assert!(mgr.update_acl(&[]).is_err());
        assert!(mgr.add_l7_port(80).is_err());
        assert!(mgr.remove_l7_port(80).is_err());
        assert!(mgr.read_stats().is_err());
    }

    #[test]
    fn error_types_are_correct() {
        let mgr = XdpManager::new("eth0", XdpMode::Driver);

        match mgr.update_backends(&[]) {
            #[cfg(target_os = "linux")]
            Err(XdpError::NotAttached { ref interface }) => {
                assert_eq!(interface, "eth0");
            }
            #[cfg(not(target_os = "linux"))]
            Err(XdpError::PlatformUnavailable) => {}
            other => panic!("unexpected result: {other:?}"),
        }
    }
}
