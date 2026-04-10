//! Common types for the PROXY protocol.

use std::net::{IpAddr, SocketAddr};

/// Command carried by a PROXY protocol header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProxyCommand {
    /// Connection was established on purpose by the proxy without being relayed.
    Local,
    /// Connection was established on behalf of another node and the header
    /// contains the original connection endpoints.
    Proxy,
}

/// Transport protocol indicated in the PROXY protocol header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportProtocol {
    Tcp,
    Udp,
    Unknown,
}

/// Parsed PROXY protocol header (common to v1 and v2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyHeader {
    pub command: ProxyCommand,
    pub transport: TransportProtocol,
    pub source: Option<SocketAddr>,
    pub destination: Option<SocketAddr>,
}

impl ProxyHeader {
    /// Return the real client address carried by this header.
    ///
    /// For a `Proxy` command the source address is the real client address.
    /// For a `Local` command there is no meaningful remote address so `None`
    /// is returned.
    pub fn real_client_address(&self) -> Option<SocketAddr> {
        match self.command {
            ProxyCommand::Proxy => self.source,
            ProxyCommand::Local => None,
        }
    }

    /// Normalize any IPv4-mapped IPv6 addresses (`::ffff:x.x.x.x`) to plain
    /// IPv4.
    pub fn normalize(&mut self) {
        self.source = self.source.map(normalize_addr);
        self.destination = self.destination.map(normalize_addr);
    }
}

/// Convert an IPv4-mapped IPv6 address to its IPv4 equivalent, leaving all
/// other addresses untouched.
pub fn normalize_addr(addr: SocketAddr) -> SocketAddr {
    match addr.ip() {
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                SocketAddr::new(IpAddr::V4(v4), addr.port())
            } else {
                addr
            }
        }
        IpAddr::V4(_) => addr,
    }
}

/// Checks whether both addresses (if present) belong to the same IP family.
/// Returns `true` when both are IPv4, both are IPv6, or at least one is `None`.
pub fn same_family(a: Option<SocketAddr>, b: Option<SocketAddr>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => matches!(
            (a.ip(), b.ip()),
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
        ),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    #[test]
    fn normalize_ipv4_mapped_ipv6() {
        // ::ffff:192.168.1.1 -> 192.168.1.1
        let mapped = SocketAddr::V6(SocketAddrV6::new(
            "::ffff:192.168.1.1".parse::<Ipv6Addr>().unwrap(),
            8080,
            0,
            0,
        ));
        let normalized = normalize_addr(mapped);
        assert_eq!(
            normalized,
            SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(192, 168, 1, 1), 8080))
        );
    }

    #[test]
    fn normalize_pure_ipv6_unchanged() {
        let pure = SocketAddr::new(IpAddr::V6("2001:db8::1".parse().unwrap()), 443);
        assert_eq!(normalize_addr(pure), pure);
    }

    #[test]
    fn normalize_ipv4_unchanged() {
        let v4 = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 80);
        assert_eq!(normalize_addr(v4), v4);
    }

    #[test]
    fn same_family_checks() {
        let v4a = Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1));
        let v4b = Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2));
        let v6a = Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 1));
        assert!(same_family(v4a, v4b));
        assert!(same_family(v6a, v6a));
        assert!(!same_family(v4a, v6a));
        assert!(same_family(v4a, None));
        assert!(same_family(None, None));
    }

    #[test]
    fn header_normalize_both_addresses() {
        let mapped_src = SocketAddr::V6(SocketAddrV6::new(
            "::ffff:10.0.0.1".parse::<Ipv6Addr>().unwrap(),
            1000,
            0,
            0,
        ));
        let mapped_dst = SocketAddr::V6(SocketAddrV6::new(
            "::ffff:10.0.0.2".parse::<Ipv6Addr>().unwrap(),
            2000,
            0,
            0,
        ));
        let mut hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            source: Some(mapped_src),
            destination: Some(mapped_dst),
        };
        hdr.normalize();
        assert_eq!(
            hdr.source.unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
        assert_eq!(
            hdr.destination.unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))
        );
    }

    #[test]
    fn real_client_address_proxy() {
        let src = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 5555);
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            source: Some(src),
            destination: None,
        };
        assert_eq!(hdr.real_client_address(), Some(src));
    }

    #[test]
    fn real_client_address_local() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Local,
            transport: TransportProtocol::Unknown,
            source: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1)),
            destination: None,
        };
        assert_eq!(hdr.real_client_address(), None);
    }
}
