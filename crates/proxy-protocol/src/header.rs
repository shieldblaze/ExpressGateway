//! Common types for the PROXY protocol.
//!
//! These types are shared between v1 (text) and v2 (binary) implementations.
//! They represent the parsed content of a PROXY protocol header independent
//! of wire format.

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
    /// TCP (STREAM).
    Tcp,
    /// UDP (DGRAM).
    Udp,
    /// UNIX stream socket.
    UnixStream,
    /// UNIX datagram socket.
    UnixDgram,
    /// Unspecified / unknown.
    Unknown,
}

/// Address information carried by a PROXY protocol header.
///
/// For IP-based transports, `source` and `destination` carry socket addresses.
/// For UNIX sockets, `unix_source` and `unix_destination` carry the raw path
/// bytes (up to 108 bytes per the PROXY protocol v2 spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyAddresses {
    /// No address information (AF_UNSPEC or LOCAL command).
    None,
    /// IPv4 or IPv6 socket addresses.
    Inet {
        source: SocketAddr,
        destination: SocketAddr,
    },
    /// UNIX socket addresses (raw path bytes, up to 108 bytes each).
    Unix {
        source: [u8; 108],
        destination: [u8; 108],
    },
}

/// A single TLV (Type-Length-Value) extension from a v2 header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tlv {
    /// TLV type byte.
    pub tlv_type: u8,
    /// TLV value (the length is implicit from `value.len()`).
    pub value: Vec<u8>,
}

/// Well-known TLV types from the PROXY protocol v2 specification.
pub mod tlv_types {
    /// Application-Layer Protocol Negotiation (ALPN).
    pub const PP2_TYPE_ALPN: u8 = 0x01;
    /// Authority (SNI hostname).
    pub const PP2_TYPE_AUTHORITY: u8 = 0x02;
    /// CRC-32c checksum of the PROXY header.
    pub const PP2_TYPE_CRC32C: u8 = 0x03;
    /// No-op padding.
    pub const PP2_TYPE_NOOP: u8 = 0x04;
    /// Unique connection ID.
    pub const PP2_TYPE_UNIQUE_ID: u8 = 0x05;
    /// SSL/TLS information.
    pub const PP2_TYPE_SSL: u8 = 0x20;
    /// Network namespace.
    pub const PP2_TYPE_NETNS: u8 = 0x30;

    /// Sub-types within PP2_TYPE_SSL.
    pub mod ssl {
        /// SSL version string.
        pub const PP2_SUBTYPE_SSL_VERSION: u8 = 0x21;
        /// SSL CN (Common Name).
        pub const PP2_SUBTYPE_SSL_CN: u8 = 0x22;
        /// SSL cipher name.
        pub const PP2_SUBTYPE_SSL_CIPHER: u8 = 0x23;
        /// SSL signature algorithm.
        pub const PP2_SUBTYPE_SSL_SIG_ALG: u8 = 0x24;
        /// SSL key algorithm.
        pub const PP2_SUBTYPE_SSL_KEY_ALG: u8 = 0x25;
    }

    /// Bit flags for the PP2_TYPE_SSL client field.
    pub mod ssl_flags {
        /// Client presented a certificate.
        pub const PP2_CLIENT_SSL: u8 = 0x01;
        /// Client certificate was verified.
        pub const PP2_CLIENT_CERT_CONN: u8 = 0x02;
        /// Client provided a certificate in the current session.
        pub const PP2_CLIENT_CERT_SESS: u8 = 0x04;
    }
}

/// Parsed PROXY protocol header (common to v1 and v2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyHeader {
    /// The command (LOCAL or PROXY).
    pub command: ProxyCommand,
    /// The transport protocol.
    pub transport: TransportProtocol,
    /// Address information.
    pub addresses: ProxyAddresses,
    /// TLV extensions (v2 only; empty for v1).
    pub tlvs: Vec<Tlv>,
}

impl ProxyHeader {
    /// Return the real client address carried by this header.
    ///
    /// For a `Proxy` command with inet addresses, the source address is the
    /// real client address.  For `Local` commands or UNIX/unspec addresses,
    /// `None` is returned.
    #[inline]
    pub fn real_client_address(&self) -> Option<SocketAddr> {
        match (&self.command, &self.addresses) {
            (ProxyCommand::Proxy, ProxyAddresses::Inet { source, .. }) => Some(*source),
            _ => None,
        }
    }

    /// Return the source address if this is an inet PROXY header.
    #[inline]
    pub fn source(&self) -> Option<SocketAddr> {
        match &self.addresses {
            ProxyAddresses::Inet { source, .. } => Some(*source),
            _ => None,
        }
    }

    /// Return the destination address if this is an inet PROXY header.
    #[inline]
    pub fn destination(&self) -> Option<SocketAddr> {
        match &self.addresses {
            ProxyAddresses::Inet { destination, .. } => Some(*destination),
            _ => None,
        }
    }

    /// Normalize any IPv4-mapped IPv6 addresses (`::ffff:x.x.x.x`) to plain
    /// IPv4.
    pub fn normalize(&mut self) {
        if let ProxyAddresses::Inet {
            source,
            destination,
        } = &mut self.addresses
        {
            *source = normalize_addr(*source);
            *destination = normalize_addr(*destination);
        }
    }

    /// Look up a TLV by type.  Returns the first matching TLV value.
    #[inline]
    pub fn find_tlv(&self, tlv_type: u8) -> Option<&[u8]> {
        self.tlvs
            .iter()
            .find(|t| t.tlv_type == tlv_type)
            .map(|t| t.value.as_slice())
    }

    /// Get the ALPN value from TLV extensions, if present.
    #[inline]
    pub fn alpn(&self) -> Option<&[u8]> {
        self.find_tlv(tlv_types::PP2_TYPE_ALPN)
    }

    /// Get the authority (SNI) value from TLV extensions, if present.
    pub fn authority(&self) -> Option<&str> {
        self.find_tlv(tlv_types::PP2_TYPE_AUTHORITY)
            .and_then(|v| std::str::from_utf8(v).ok())
    }
}

/// Convert an IPv4-mapped IPv6 address to its IPv4 equivalent, leaving all
/// other addresses untouched.
#[inline]
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

/// Checks whether both addresses belong to the same IP family.
#[inline]
pub fn same_family(a: SocketAddr, b: SocketAddr) -> bool {
    matches!(
        (a.ip(), b.ip()),
        (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddrV4, SocketAddrV6};

    #[test]
    fn normalize_ipv4_mapped_ipv6() {
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
        let v4a = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1);
        let v4b = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 2);
        let v6a = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 1);
        assert!(same_family(v4a, v4b));
        assert!(same_family(v6a, v6a));
        assert!(!same_family(v4a, v6a));
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
            addresses: ProxyAddresses::Inet {
                source: mapped_src,
                destination: mapped_dst,
            },
            tlvs: Vec::new(),
        };
        hdr.normalize();
        assert_eq!(
            hdr.source().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
        assert_eq!(
            hdr.destination().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2))
        );
    }

    #[test]
    fn real_client_address_proxy() {
        let src = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 5555);
        let dst = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)), 80);
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::Inet {
                source: src,
                destination: dst,
            },
            tlvs: Vec::new(),
        };
        assert_eq!(hdr.real_client_address(), Some(src));
    }

    #[test]
    fn real_client_address_local() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Local,
            transport: TransportProtocol::Unknown,
            addresses: ProxyAddresses::None,
            tlvs: Vec::new(),
        };
        assert_eq!(hdr.real_client_address(), None);
    }

    #[test]
    fn find_tlv_present() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::None,
            tlvs: vec![
                Tlv {
                    tlv_type: tlv_types::PP2_TYPE_ALPN,
                    value: b"h2".to_vec(),
                },
                Tlv {
                    tlv_type: tlv_types::PP2_TYPE_AUTHORITY,
                    value: b"example.com".to_vec(),
                },
            ],
        };
        assert_eq!(hdr.alpn(), Some(b"h2".as_slice()));
        assert_eq!(hdr.authority(), Some("example.com"));
    }

    #[test]
    fn find_tlv_absent() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::None,
            tlvs: Vec::new(),
        };
        assert_eq!(hdr.alpn(), None);
        assert_eq!(hdr.authority(), None);
    }
}
