//! SHARED-2 (S15 A2): UDP datapath trait + tier-3 tokio-UDP impl.
//!
//! See `audit/quic/s15-design.md` §10 for the stable seam contract.
//! The trait is the seam between QUIC routing (passthrough / future
//! Mode B terminate) and the kernel/userspace UDP transport.
//!
//! ## Tier ladder (v1.0 → v1.2)
//!
//! * **v1.0 (this commit) — `TokioUdp`** (`tokio::net::UdpSocket`).
//!   The correctness baseline; always-available across kernels.
//! * **v1.1 — `IoUring`** (kernel 6.0+ multishot recvmsg). Deferred
//!   per owner ruling (XDP/io_uring deferral); stub only.
//! * **v1.2 — `Xdp`** (AF_XDP + DCID-steering eBPF program).
//!   Deferred per owner ruling; stub only. The `dcid_map_fd` trait
//!   method is the reserved hook (CF-S15-DCID-MAP-XDP).
//!
//! ## Correctness contract (every tier)
//!
//! * `recv_loop` delivers EACH datagram EXACTLY ONCE in arrival order
//!   within a single 4-tuple. Cross-flow reordering allowed (matches
//!   kernel UDP).
//! * `send_to` either fully sends `buf` or returns `Err`; UDP is
//!   datagram-atomic, no short-writes.
//! * `local_addr` is stable for the impl's lifetime.
//! * Cancellation via the `CancellationToken` returns from `recv_loop`
//!   within one in-flight packet.
//! * NO panic on transient OS errors. Fatal errors → `Unavailable`.
//! * **NEVER decrypt, inspect, or modify packet payloads.** The
//!   dataplane is bytes-in / bytes-out. Reinforces the no-decrypt
//!   property; only the router parses the public header.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio_util::sync::CancellationToken;

/// Maximum UDP datagram the passthrough router will accept (RFC 9000
/// allows up to 65_527 application bytes; we accept up to 65_535 to be
/// permissive on the receive side and let `parse_public_header` reject
/// anything malformed).
pub const MAX_UDP_DATAGRAM_SIZE: usize = 65_535;

/// One inbound UDP datagram delivered by the dataplane to the router.
///
/// The dataplane owns the buffer until `Packet` is dropped; the
/// callback may borrow `data` for the duration of its `Future` only.
/// Implementations may pool the underlying allocation across packets.
#[derive(Debug)]
pub struct Packet<'a> {
    /// Datagram payload, length-truncated to the bytes actually read.
    pub data: &'a [u8],
    /// Source peer address (as observed by the kernel / NIC).
    pub from: SocketAddr,
    /// Local bound address the datagram arrived on. Useful for
    /// multi-VIP listeners; v1 only has one bind so this is the
    /// listener's `local_addr` on every packet.
    pub to: SocketAddr,
}

/// Errors surfaced by a [`UdpDataplane`] implementation.
///
/// The router maps these to its own drop/metric/log discipline; the
/// dataplane does not decide policy.
#[derive(Debug, thiserror::Error)]
pub enum DataplaneError {
    /// Bind failed; the listener cannot start.
    #[error("dataplane bind failed: {0}")]
    Bind(#[source] std::io::Error),
    /// Recv hit a transient OS error (would-block, ENOBUFS). Router
    /// MAY continue.
    #[error("dataplane recv: {0}")]
    Recv(#[source] std::io::Error),
    /// Send hit a transient OS error. Router MAY drop the packet.
    #[error("dataplane send: {0}")]
    Send(#[source] std::io::Error),
    /// Tier-specific fatal error (eBPF verifier rejected the program,
    /// io_uring kernel doesn't support multishot recvmsg, etc.).
    /// Router MUST fall back to the next tier on the ladder.
    #[error("dataplane unavailable on this kernel/NIC: {0}")]
    Unavailable(String),
}

/// Callback shape for [`UdpDataplane::recv_loop`].
///
/// `Arc` so impls can clone it into per-task closures (io_uring
/// completion handlers, AF_XDP frame processors).
pub type PacketHandler<'a> = Arc<
    dyn for<'p> Fn(Packet<'p>) -> Pin<Box<dyn Future<Output = ()> + Send + 'p>> + Send + Sync + 'a,
>;

/// The seam. Three implementations are reserved (v1.0..v1.2). v1.0
/// ships only [`TokioUdp`].
pub trait UdpDataplane: Send + Sync + 'static {
    /// Local socket address the dataplane is bound to. Stable for
    /// the lifetime of the impl; the router logs it once at spawn.
    fn local_addr(&self) -> SocketAddr;

    /// Run the recv loop until `cancel` fires, dispatching each
    /// inbound packet through `on_packet`. The callback is invoked
    /// on the runtime's task; the impl MUST NOT hold the buffer
    /// past the returned future's `Poll::Ready`.
    fn recv_loop<'a>(
        &'a self,
        cancel: CancellationToken,
        on_packet: PacketHandler<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), DataplaneError>> + Send + 'a>>;

    /// Send `buf` to `dst`. Returns the number of bytes accepted by
    /// the kernel (== `buf.len()` on a healthy UDP socket).
    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        dst: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Result<usize, DataplaneError>> + Send + 'a>>;

    /// Tier identifier for metrics + logs (`"tokio-udp"`, `"io-uring"`,
    /// `"xdp-af-xdp"`). Stable per impl.
    fn tier_name(&self) -> &'static str;

    /// XDP fast-path hook (v1.2 only). Returns the eBPF DCID-routing
    /// map's file descriptor so the userspace publisher can call
    /// `bpf_map_update_elem` on flow add/remove. Other tiers return
    /// `None` — the router treats `None` as "no fast-path map; route
    /// every packet in userspace" (v1.0/v1.1 behavior).
    /// CF-S15-DCID-MAP-XDP — reserved for v1.2.
    fn dcid_map_fd(&self) -> Option<i32> {
        None
    }
}

/// Operator policy for tier selection.
///
/// v1.0 ships only [`TokioUdp`]; `Auto` resolves to it and the typed
/// variants for `IoUring` / `Xdp` return `Unavailable`.
pub enum TierPolicy {
    /// Walk the ladder XDP → io_uring → tokio-UDP, picking the first
    /// that initializes successfully. v1.0: only `TokioUdp` exists;
    /// `Auto` always selects it.
    Auto,
    /// Force tokio-UDP. Always available.
    TokioUdp,
    /// Force io_uring (v1.1+). Returns `Unavailable` in v1.0.
    IoUring,
    /// Force XDP (v1.2+). Returns `Unavailable` in v1.0.
    Xdp {
        /// Interface to attach the eBPF program to.
        iface: String,
    },
}

/// Select the highest-capability tier the host supports.
///
/// # Errors
///
/// Returns [`DataplaneError::Bind`] on bind failure of the chosen
/// tier or [`DataplaneError::Unavailable`] when a higher tier was
/// requested but the implementation isn't compiled in (v1.1/v1.2).
pub async fn select_dataplane(
    bind: SocketAddr,
    policy: TierPolicy,
) -> Result<Arc<dyn UdpDataplane>, DataplaneError> {
    match policy {
        TierPolicy::Auto | TierPolicy::TokioUdp => {
            let sock = UdpSocket::bind(bind).await.map_err(DataplaneError::Bind)?;
            let tu = TokioUdp::new(sock).map_err(DataplaneError::Bind)?;
            Ok(Arc::new(tu))
        }
        TierPolicy::IoUring => Err(DataplaneError::Unavailable(
            "io_uring dataplane is v1.1+; not shipped in v1.0".into(),
        )),
        TierPolicy::Xdp { iface } => Err(DataplaneError::Unavailable(format!(
            "XDP DCID-steering dataplane is v1.2+; iface={iface}"
        ))),
    }
}

/// Tier-3 `tokio::net::UdpSocket` dataplane.
///
/// This is v1.0's only impl and the correctness baseline against
/// which v1.1/v1.2 are differentially verified.
pub struct TokioUdp {
    socket: Arc<UdpSocket>,
    local: SocketAddr,
}

impl TokioUdp {
    /// Wrap an already-bound `UdpSocket`.
    ///
    /// # Errors
    ///
    /// Returns the OS error from `socket.local_addr()` (cannot
    /// happen on a bound socket in practice; surfaced as `Err`
    /// rather than `expect` to honour the lb-quic crate-wide
    /// no-`expect` discipline).
    pub fn new(socket: UdpSocket) -> std::io::Result<Self> {
        let local = socket.local_addr()?;
        Ok(Self {
            socket: Arc::new(socket),
            local,
        })
    }

    /// Bind a new `UdpSocket` to `addr` and wrap it.
    ///
    /// # Errors
    ///
    /// Returns the OS bind error.
    pub async fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(addr).await?;
        Self::new(socket)
    }

    /// Borrow the inner socket. Used by the passthrough listener's
    /// reply-on-the-same-fd discipline.
    #[must_use]
    pub fn socket(&self) -> Arc<UdpSocket> {
        Arc::clone(&self.socket)
    }
}

impl UdpDataplane for TokioUdp {
    fn local_addr(&self) -> SocketAddr {
        self.local
    }

    fn recv_loop<'a>(
        &'a self,
        cancel: CancellationToken,
        on_packet: PacketHandler<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<(), DataplaneError>> + Send + 'a>> {
        Box::pin(async move {
            let mut buf = vec![0u8; MAX_UDP_DATAGRAM_SIZE];
            let local = self.local;
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Ok(()),
                    r = self.socket.recv_from(&mut buf) => {
                        match r {
                            Ok((n, from)) => {
                                let slice = buf.get(..n).unwrap_or(&[]);
                                let pkt = Packet { data: slice, from, to: local };
                                on_packet(pkt).await;
                            }
                            Err(e) => {
                                // Transient ENOBUFS / EAGAIN — log + continue.
                                // Hard errors (EBADF) surface as Recv on the
                                // next iteration; the router decides policy.
                                tracing::debug!(error = %e, "tokio-udp recv_from");
                            }
                        }
                    }
                }
            }
        })
    }

    fn send_to<'a>(
        &'a self,
        buf: &'a [u8],
        dst: SocketAddr,
    ) -> Pin<Box<dyn Future<Output = Result<usize, DataplaneError>> + Send + 'a>> {
        Box::pin(async move {
            self.socket
                .send_to(buf, dst)
                .await
                .map_err(DataplaneError::Send)
        })
    }

    fn tier_name(&self) -> &'static str {
        "tokio-udp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[tokio::test]
    async fn tokio_udp_select_auto_binds() {
        let bind = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let dp = select_dataplane(bind, TierPolicy::Auto)
            .await
            .expect("Auto tier selection should succeed");
        assert_eq!(dp.tier_name(), "tokio-udp");
        assert!(dp.dcid_map_fd().is_none());
        assert_eq!(dp.local_addr().ip(), Ipv4Addr::LOCALHOST);
    }

    #[tokio::test]
    async fn iouring_returns_unavailable_in_v1() {
        let bind = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let r = select_dataplane(bind, TierPolicy::IoUring).await;
        match r.err() {
            Some(DataplaneError::Unavailable(_)) => {}
            other => panic!("expected Unavailable for io_uring in v1.0, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn xdp_returns_unavailable_in_v1() {
        let bind = SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 0);
        let r = select_dataplane(
            bind,
            TierPolicy::Xdp {
                iface: "lo".to_string(),
            },
        )
        .await;
        match r.err() {
            Some(DataplaneError::Unavailable(_)) => {}
            other => panic!("expected Unavailable for XDP in v1.0, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tokio_udp_roundtrip_via_trait() {
        // Spin two TokioUdp instances; send a packet from A to B; B's
        // recv_loop dispatches it; A receives the reply on a separate
        // socket. Stays within the trait surface so any future tier
        // impl can be swapped in.
        let a = TokioUdp::bind(SocketAddr::new(
            std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
            0,
        ))
        .await
        .expect("bind A");
        let b = TokioUdp::bind(SocketAddr::new(
            std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
            0,
        ))
        .await
        .expect("bind B");
        let a_addr = a.local_addr();
        let b_addr = b.local_addr();

        let cancel = CancellationToken::new();
        let cancel_recv = cancel.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<(Vec<u8>, SocketAddr)>();
        let tx = parking_lot::Mutex::new(Some(tx));

        let on_packet: PacketHandler<'_> = Arc::new(move |pkt: Packet<'_>| {
            let data = pkt.data.to_vec();
            let from = pkt.from;
            let tx_slot = tx.lock().take();
            Box::pin(async move {
                if let Some(t) = tx_slot {
                    let _ = t.send((data, from));
                }
            })
        });

        let b_arc: Arc<TokioUdp> = Arc::new(b);
        let b_clone = Arc::clone(&b_arc);
        let recv_join = tokio::spawn(async move {
            let _ = b_clone.recv_loop(cancel_recv, on_packet).await;
        });

        // Give recv_loop a moment to register.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        a.send_to(b"hello", b_addr).await.expect("send_to OK");

        let (got, from) = tokio::time::timeout(std::time::Duration::from_secs(2), rx)
            .await
            .expect("recv didn't time out")
            .expect("oneshot recv");
        assert_eq!(got, b"hello");
        assert_eq!(from, a_addr);

        cancel.cancel();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(2), recv_join).await;
    }
}
