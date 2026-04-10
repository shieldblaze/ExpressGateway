//! XDP/eBPF acceleration layer for ExpressGateway.
//!
//! This crate provides in-kernel packet forwarding using eBPF/XDP for L4 load
//! balancing. When XDP is available (Linux with compatible NIC drivers), packets
//! can be forwarded entirely in the kernel, bypassing the normal network stack
//! for dramatically lower latency and higher throughput.
//!
//! # Architecture
//!
//! - **maps**: `#[repr(C)]` structures shared between BPF programs and userspace.
//! - **manager**: Userspace manager that loads, attaches, and configures XDP programs.
//! - **session_cleanup**: Background task for evicting stale UDP sessions.
//! - **fallback**: Detection of XDP availability and fallback warnings.
//!
//! # Platform support
//!
//! XDP is Linux-only. On non-Linux platforms, all manager operations return
//! errors and [`fallback::is_xdp_available`] returns `false`. The crate
//! compiles on all platforms.

pub mod fallback;
pub mod manager;
pub mod maps;
pub mod session_cleanup;

pub use fallback::is_xdp_available;
pub use manager::{XdpManager, XdpMode};
pub use maps::{
    AclKey, AclValue, BackendEntry, TcpConnKey, TcpConnValue, UdpSessionKey, UdpSessionValue,
    XdpStats,
};
pub use session_cleanup::SessionCleanup;
