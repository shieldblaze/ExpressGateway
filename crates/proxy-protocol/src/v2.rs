//! PROXY protocol v2 (binary format).
//!
//! The binary header consists of:
//! - 12-byte signature
//! - 1 byte: version (upper nibble, must be 2) + command (lower nibble)
//! - 1 byte: address family (upper nibble) + transport protocol (lower nibble)
//! - 2 bytes: length of the address section (big-endian)
//! - variable-length address data

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use bytes::{Buf, BufMut, BytesMut};
use tracing::trace;

use crate::header::{ProxyCommand, ProxyHeader, TransportProtocol};

/// The 12-byte v2 signature.
pub const V2_SIGNATURE: [u8; 12] = *b"\r\n\r\n\0\r\nQUIT\n";

/// Minimum header size: 12 (signature) + 1 (ver/cmd) + 1 (fam/proto) + 2 (len).
pub const V2_HEADER_MIN: usize = 16;

/// Version value (upper nibble of ver/cmd byte).
const VERSION: u8 = 0x20;

/// Command constants (lower nibble).
const CMD_LOCAL: u8 = 0x00;
const CMD_PROXY: u8 = 0x01;

/// Address family constants (upper nibble of fam/proto byte).
const AF_UNSPEC: u8 = 0x00;
const AF_INET: u8 = 0x10;
const AF_INET6: u8 = 0x20;

/// Transport protocol constants (lower nibble of fam/proto byte).
const PROTO_UNSPEC: u8 = 0x00;
const PROTO_STREAM: u8 = 0x01; // TCP
const PROTO_DGRAM: u8 = 0x02; // UDP

/// IPv4 address block size: 4 + 4 + 2 + 2 = 12.
const ADDR_LEN_INET: u16 = 12;
/// IPv6 address block size: 16 + 16 + 2 + 2 = 36.
const ADDR_LEN_INET6: u16 = 36;

/// Errors specific to v2 parsing.
#[derive(Debug, thiserror::Error)]
pub enum V2Error {
    #[error("invalid v2 signature")]
    InvalidSignature,

    #[error("unsupported version: {0:#x}")]
    UnsupportedVersion(u8),

    #[error("unknown command: {0:#x}")]
    UnknownCommand(u8),

    #[error("address data too short: need {need} bytes, have {have}")]
    AddressDataTooShort { need: usize, have: usize },

    #[error("unknown address family: {0:#x}")]
    UnknownAddressFamily(u8),
}

/// Attempt to decode a v2 PROXY protocol header from the front of `buf`.
///
/// On success the consumed bytes are removed from `buf` and the parsed
/// [`ProxyHeader`] is returned.  The header is automatically normalized.
///
/// Returns `Ok(None)` when there are not enough bytes yet.
pub fn decode_v2(buf: &mut BytesMut) -> Result<Option<ProxyHeader>, V2Error> {
    if buf.len() < V2_HEADER_MIN {
        return Ok(None);
    }

    // Verify signature.
    if buf[..12] != V2_SIGNATURE {
        return Err(V2Error::InvalidSignature);
    }

    let ver_cmd = buf[12];
    let fam_proto = buf[13];
    let addr_len = u16::from_be_bytes([buf[14], buf[15]]) as usize;

    // Check version.
    let version = ver_cmd & 0xF0;
    if version != VERSION {
        return Err(V2Error::UnsupportedVersion(version));
    }

    // Total bytes we need.
    let total = V2_HEADER_MIN + addr_len;
    if buf.len() < total {
        return Ok(None); // need more data
    }

    // Parse command.
    let command = match ver_cmd & 0x0F {
        CMD_LOCAL => ProxyCommand::Local,
        CMD_PROXY => ProxyCommand::Proxy,
        other => return Err(V2Error::UnknownCommand(other)),
    };

    // Parse address family and transport.
    let af = fam_proto & 0xF0;
    let tp = fam_proto & 0x0F;

    let transport = match tp {
        PROTO_STREAM => TransportProtocol::Tcp,
        PROTO_DGRAM => TransportProtocol::Udp,
        _ => TransportProtocol::Unknown,
    };

    // Extract address data.
    let addr_data = &buf[V2_HEADER_MIN..total];
    let (source, destination) = match af {
        AF_INET if command == ProxyCommand::Proxy => {
            if addr_data.len() < ADDR_LEN_INET as usize {
                return Err(V2Error::AddressDataTooShort {
                    need: ADDR_LEN_INET as usize,
                    have: addr_data.len(),
                });
            }
            let src_ip = Ipv4Addr::new(addr_data[0], addr_data[1], addr_data[2], addr_data[3]);
            let dst_ip = Ipv4Addr::new(addr_data[4], addr_data[5], addr_data[6], addr_data[7]);
            let src_port = u16::from_be_bytes([addr_data[8], addr_data[9]]);
            let dst_port = u16::from_be_bytes([addr_data[10], addr_data[11]]);
            (
                Some(SocketAddr::new(IpAddr::V4(src_ip), src_port)),
                Some(SocketAddr::new(IpAddr::V4(dst_ip), dst_port)),
            )
        }
        AF_INET6 if command == ProxyCommand::Proxy => {
            if addr_data.len() < ADDR_LEN_INET6 as usize {
                return Err(V2Error::AddressDataTooShort {
                    need: ADDR_LEN_INET6 as usize,
                    have: addr_data.len(),
                });
            }
            let src_ip = Ipv6Addr::from(<[u8; 16]>::try_from(&addr_data[0..16]).unwrap());
            let dst_ip = Ipv6Addr::from(<[u8; 16]>::try_from(&addr_data[16..32]).unwrap());
            let src_port = u16::from_be_bytes([addr_data[32], addr_data[33]]);
            let dst_port = u16::from_be_bytes([addr_data[34], addr_data[35]]);
            (
                Some(SocketAddr::new(IpAddr::V6(src_ip), src_port)),
                Some(SocketAddr::new(IpAddr::V6(dst_ip), dst_port)),
            )
        }
        _ => (None, None),
    };

    // Consume.
    buf.advance(total);

    let mut header = ProxyHeader {
        command,
        transport,
        source,
        destination,
    };
    header.normalize();

    trace!(?header, "decoded PROXY protocol v2 header");
    Ok(Some(header))
}

/// Encode a [`ProxyHeader`] into a v2 binary representation and append it to
/// `buf`.
///
/// If source/destination addresses are not available the LOCAL command with
/// `AF_UNSPEC` is used.
pub fn encode_v2(header: &ProxyHeader, buf: &mut BytesMut) {
    // Signature.
    buf.put_slice(&V2_SIGNATURE);

    // Determine command byte.
    let cmd = match header.command {
        ProxyCommand::Local => CMD_LOCAL,
        ProxyCommand::Proxy => CMD_PROXY,
    };
    buf.put_u8(VERSION | cmd);

    // Transport nibble.
    let tp = match header.transport {
        TransportProtocol::Tcp => PROTO_STREAM,
        TransportProtocol::Udp => PROTO_DGRAM,
        TransportProtocol::Unknown => PROTO_UNSPEC,
    };

    match (header.source, header.destination) {
        (Some(src), Some(dst)) if header.command == ProxyCommand::Proxy => {
            match (src.ip(), dst.ip()) {
                (IpAddr::V4(s4), IpAddr::V4(d4)) => {
                    buf.put_u8(AF_INET | tp);
                    buf.put_u16(ADDR_LEN_INET);
                    buf.put_slice(&s4.octets());
                    buf.put_slice(&d4.octets());
                    buf.put_u16(src.port());
                    buf.put_u16(dst.port());
                }
                (IpAddr::V6(s6), IpAddr::V6(d6)) => {
                    buf.put_u8(AF_INET6 | tp);
                    buf.put_u16(ADDR_LEN_INET6);
                    buf.put_slice(&s6.octets());
                    buf.put_slice(&d6.octets());
                    buf.put_u16(src.port());
                    buf.put_u16(dst.port());
                }
                _ => {
                    // Mixed families – fall back to LOCAL/UNSPEC.
                    buf.put_u8(AF_UNSPEC | PROTO_UNSPEC);
                    buf.put_u16(0);
                }
            }
        }
        _ => {
            // No addresses or LOCAL command.
            buf.put_u8(AF_UNSPEC | PROTO_UNSPEC);
            buf.put_u16(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn make_v2_ipv4_buf(cmd: u8, src: Ipv4Addr, dst: Ipv4Addr, sp: u16, dp: u16) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(VERSION | cmd);
        buf.put_u8(AF_INET | PROTO_STREAM);
        buf.put_u16(ADDR_LEN_INET);
        buf.put_slice(&src.octets());
        buf.put_slice(&dst.octets());
        buf.put_u16(sp);
        buf.put_u16(dp);
        buf
    }

    fn make_v2_ipv6_buf(cmd: u8, src: Ipv6Addr, dst: Ipv6Addr, sp: u16, dp: u16) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(VERSION | cmd);
        buf.put_u8(AF_INET6 | PROTO_STREAM);
        buf.put_u16(ADDR_LEN_INET6);
        buf.put_slice(&src.octets());
        buf.put_slice(&dst.octets());
        buf.put_u16(sp);
        buf.put_u16(dp);
        buf
    }

    fn make_v2_local_buf() -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(VERSION | CMD_LOCAL);
        buf.put_u8(AF_UNSPEC | PROTO_UNSPEC);
        buf.put_u16(0);
        buf
    }

    #[test]
    fn decode_v2_ipv4() {
        let mut buf = make_v2_ipv4_buf(
            CMD_PROXY,
            Ipv4Addr::new(192, 168, 1, 1),
            Ipv4Addr::new(10, 0, 0, 1),
            56324,
            443,
        );
        buf.put_slice(b"trailing"); // extra data after header

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.command, ProxyCommand::Proxy);
        assert_eq!(hdr.transport, TransportProtocol::Tcp);
        assert_eq!(
            hdr.source.unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 56324)
        );
        assert_eq!(
            hdr.destination.unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443)
        );
        assert_eq!(&buf[..], b"trailing");
    }

    #[test]
    fn decode_v2_ipv6() {
        let src: Ipv6Addr = "2001:db8::1".parse().unwrap();
        let dst: Ipv6Addr = "2001:db8::2".parse().unwrap();
        let mut buf = make_v2_ipv6_buf(CMD_PROXY, src, dst, 12345, 80);

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.command, ProxyCommand::Proxy);
        assert_eq!(hdr.source.unwrap().ip(), IpAddr::V6(src));
        assert_eq!(hdr.destination.unwrap().ip(), IpAddr::V6(dst));
        assert_eq!(hdr.source.unwrap().port(), 12345);
        assert_eq!(hdr.destination.unwrap().port(), 80);
    }

    #[test]
    fn decode_v2_local() {
        let mut buf = make_v2_local_buf();

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.command, ProxyCommand::Local);
        assert_eq!(hdr.transport, TransportProtocol::Unknown);
        assert!(hdr.source.is_none());
        assert!(hdr.destination.is_none());
        assert!(buf.is_empty());
    }

    #[test]
    fn decode_v2_incomplete() {
        let mut buf = BytesMut::from(&V2_SIGNATURE[..8]);
        assert!(decode_v2(&mut buf).unwrap().is_none());
    }

    #[test]
    fn decode_v2_bad_signature() {
        let mut buf = BytesMut::from(&b"BADBADBADBAD\x21\x11\x00\x00"[..]);
        assert!(decode_v2(&mut buf).is_err());
    }

    #[test]
    fn decode_v2_bad_version() {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(0x31); // version 3 instead of 2
        buf.put_u8(0x00);
        buf.put_u16(0);
        assert!(decode_v2(&mut buf).is_err());
    }

    #[test]
    fn encode_v2_ipv4() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            source: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 1000)),
            destination: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)), 2000)),
        };
        let mut buf = BytesMut::new();
        encode_v2(&hdr, &mut buf);

        // Decode what we just encoded.
        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, hdr);
    }

    #[test]
    fn encode_v2_ipv6() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            source: Some(SocketAddr::new(IpAddr::V6("fe80::1".parse().unwrap()), 100)),
            destination: Some(SocketAddr::new(IpAddr::V6("fe80::2".parse().unwrap()), 200)),
        };
        let mut buf = BytesMut::new();
        encode_v2(&hdr, &mut buf);

        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, hdr);
    }

    #[test]
    fn encode_v2_local() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Local,
            transport: TransportProtocol::Unknown,
            source: None,
            destination: None,
        };
        let mut buf = BytesMut::new();
        encode_v2(&hdr, &mut buf);

        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.command, ProxyCommand::Local);
        assert!(decoded.source.is_none());
    }

    #[test]
    fn encode_v2_no_addresses_falls_back() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            source: None,
            destination: None,
        };
        let mut buf = BytesMut::new();
        encode_v2(&hdr, &mut buf);

        // Should still decode (as LOCAL-like with no addresses).
        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert!(decoded.source.is_none());
    }

    #[test]
    fn decode_v2_normalizes_ipv4_mapped() {
        let src: Ipv6Addr = "::ffff:192.168.1.1".parse().unwrap();
        let dst: Ipv6Addr = "::ffff:10.0.0.1".parse().unwrap();
        let mut buf = make_v2_ipv6_buf(CMD_PROXY, src, dst, 1234, 5678);

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(
            hdr.source.unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))
        );
        assert_eq!(
            hdr.destination.unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
    }

    #[test]
    fn roundtrip_v2_udp() {
        let original = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Udp,
            source: Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)),
                9999,
            )),
            destination: Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(172, 16, 0, 2)),
                8888,
            )),
        };
        let mut buf = BytesMut::new();
        encode_v2(&original, &mut buf);
        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, original);
    }
}
