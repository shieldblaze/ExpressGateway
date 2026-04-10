//! PROXY protocol v2 (binary format).
//!
//! The binary header consists of:
//! - 12-byte signature (`\r\n\r\n\0\r\nQUIT\n`)
//! - 1 byte: version (upper nibble, must be 2) + command (lower nibble)
//! - 1 byte: address family (upper nibble) + transport protocol (lower nibble)
//! - 2 bytes: length of the address + TLV section (big-endian)
//! - variable-length address data
//! - variable-length TLV extensions
//!
//! Reference: HAProxy PROXY protocol specification, section 2.2.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use bytes::{Buf, BufMut, BytesMut};
use tracing::trace;

use crate::header::{
    ProxyAddresses, ProxyCommand, ProxyHeader, Tlv, TransportProtocol,
};

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
const AF_UNIX: u8 = 0x30;

/// Transport protocol constants (lower nibble of fam/proto byte).
const PROTO_UNSPEC: u8 = 0x00;
const PROTO_STREAM: u8 = 0x01; // TCP or UNIX stream
const PROTO_DGRAM: u8 = 0x02; // UDP or UNIX dgram

/// IPv4 address block size: 4 + 4 + 2 + 2 = 12.
const ADDR_LEN_INET: usize = 12;
/// IPv6 address block size: 16 + 16 + 2 + 2 = 36.
const ADDR_LEN_INET6: usize = 36;
/// UNIX address block size: 108 + 108 = 216.
const ADDR_LEN_UNIX: usize = 216;

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

    #[error("malformed TLV: need {need} bytes, have {have}")]
    MalformedTlv { need: usize, have: usize },

    #[error("address length exceeds maximum ({0} bytes)")]
    AddressLengthTooLarge(usize),
}

/// Maximum reasonable address + TLV length.  The spec says the total header
/// (including the 16-byte fixed part) must not exceed 65551 bytes
/// (16 + 65535 max payload).  We enforce the payload limit.
const MAX_ADDR_LEN: usize = 65535;

/// Attempt to decode a v2 PROXY protocol header from the front of `buf`.
///
/// On success the consumed bytes are removed from `buf` and the parsed
/// [`ProxyHeader`] is returned.  The header is automatically normalized.
/// TLV extensions are parsed and included in the result.
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

    if addr_len > MAX_ADDR_LEN {
        return Err(V2Error::AddressLengthTooLarge(addr_len));
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

    let transport = match (af, tp) {
        (AF_UNIX, PROTO_STREAM) => TransportProtocol::UnixStream,
        (AF_UNIX, PROTO_DGRAM) => TransportProtocol::UnixDgram,
        (_, PROTO_STREAM) => TransportProtocol::Tcp,
        (_, PROTO_DGRAM) => TransportProtocol::Udp,
        _ => TransportProtocol::Unknown,
    };

    // Extract address data.
    let payload = &buf[V2_HEADER_MIN..total];
    let (addresses, addr_consumed) = parse_addresses(af, command, payload)?;

    // Parse TLVs from remaining payload after the address block.
    let tlv_data = &payload[addr_consumed..];
    let tlvs = parse_tlvs(tlv_data)?;

    // Consume.
    buf.advance(total);

    let mut header = ProxyHeader {
        command,
        transport,
        addresses,
        tlvs,
    };
    header.normalize();

    trace!(?header, "decoded PROXY protocol v2 header");
    Ok(Some(header))
}

/// Parse address data from the payload based on address family.
fn parse_addresses(
    af: u8,
    command: ProxyCommand,
    payload: &[u8],
) -> Result<(ProxyAddresses, usize), V2Error> {
    if command == ProxyCommand::Local {
        // LOCAL command: addresses are ignored per spec, but we still need
        // to know how many bytes the address block consumes.
        let consumed = match af {
            AF_INET => ADDR_LEN_INET.min(payload.len()),
            AF_INET6 => ADDR_LEN_INET6.min(payload.len()),
            AF_UNIX => ADDR_LEN_UNIX.min(payload.len()),
            _ => 0,
        };
        return Ok((ProxyAddresses::None, consumed));
    }

    match af {
        AF_INET => {
            if payload.len() < ADDR_LEN_INET {
                return Err(V2Error::AddressDataTooShort {
                    need: ADDR_LEN_INET,
                    have: payload.len(),
                });
            }
            let src_ip = Ipv4Addr::new(payload[0], payload[1], payload[2], payload[3]);
            let dst_ip = Ipv4Addr::new(payload[4], payload[5], payload[6], payload[7]);
            let src_port = u16::from_be_bytes([payload[8], payload[9]]);
            let dst_port = u16::from_be_bytes([payload[10], payload[11]]);
            Ok((
                ProxyAddresses::Inet {
                    source: SocketAddr::new(IpAddr::V4(src_ip), src_port),
                    destination: SocketAddr::new(IpAddr::V4(dst_ip), dst_port),
                },
                ADDR_LEN_INET,
            ))
        }
        AF_INET6 => {
            if payload.len() < ADDR_LEN_INET6 {
                return Err(V2Error::AddressDataTooShort {
                    need: ADDR_LEN_INET6,
                    have: payload.len(),
                });
            }
            // SAFETY: slices are exactly 16 bytes, verified by the length
            // check above.  `try_from` on a 16-byte slice is infallible
            // but returns Result for API uniformity; we convert safely.
            let src_bytes: [u8; 16] = payload[0..16]
                .try_into()
                .map_err(|_| V2Error::AddressDataTooShort {
                    need: 16,
                    have: payload.len(),
                })?;
            let dst_bytes: [u8; 16] = payload[16..32]
                .try_into()
                .map_err(|_| V2Error::AddressDataTooShort {
                    need: 32,
                    have: payload.len(),
                })?;
            let src_ip = Ipv6Addr::from(src_bytes);
            let dst_ip = Ipv6Addr::from(dst_bytes);
            let src_port = u16::from_be_bytes([payload[32], payload[33]]);
            let dst_port = u16::from_be_bytes([payload[34], payload[35]]);
            Ok((
                ProxyAddresses::Inet {
                    source: SocketAddr::new(IpAddr::V6(src_ip), src_port),
                    destination: SocketAddr::new(IpAddr::V6(dst_ip), dst_port),
                },
                ADDR_LEN_INET6,
            ))
        }
        AF_UNIX => {
            if payload.len() < ADDR_LEN_UNIX {
                return Err(V2Error::AddressDataTooShort {
                    need: ADDR_LEN_UNIX,
                    have: payload.len(),
                });
            }
            let mut src = [0u8; 108];
            let mut dst = [0u8; 108];
            src.copy_from_slice(&payload[0..108]);
            dst.copy_from_slice(&payload[108..216]);
            Ok((ProxyAddresses::Unix {
                source: src,
                destination: dst,
            }, ADDR_LEN_UNIX))
        }
        AF_UNSPEC => Ok((ProxyAddresses::None, 0)),
        other => Err(V2Error::UnknownAddressFamily(other)),
    }
}

/// Parse TLV extensions from the remaining payload.
fn parse_tlvs(mut data: &[u8]) -> Result<Vec<Tlv>, V2Error> {
    let mut tlvs = Vec::new();

    while data.len() >= 3 {
        let tlv_type = data[0];
        let tlv_len = u16::from_be_bytes([data[1], data[2]]) as usize;

        if data.len() < 3 + tlv_len {
            return Err(V2Error::MalformedTlv {
                need: 3 + tlv_len,
                have: data.len(),
            });
        }

        let value = data[3..3 + tlv_len].to_vec();
        tlvs.push(Tlv { tlv_type, value });
        data = &data[3 + tlv_len..];
    }

    // If there are 1 or 2 trailing bytes, they're malformed TLV headers --
    // but per the spec, implementations SHOULD ignore them for forward
    // compatibility.

    Ok(tlvs)
}

/// Encode a [`ProxyHeader`] into a v2 binary representation and append it to
/// `buf`.
///
/// If source/destination addresses are not available the LOCAL command with
/// `AF_UNSPEC` is used.  TLV extensions are appended after the address block.
pub fn encode_v2(header: &ProxyHeader, buf: &mut BytesMut) {
    // Signature.
    buf.put_slice(&V2_SIGNATURE);

    // Determine command byte.
    let cmd = match header.command {
        ProxyCommand::Local => CMD_LOCAL,
        ProxyCommand::Proxy => CMD_PROXY,
    };
    buf.put_u8(VERSION | cmd);

    // Calculate TLV size.
    let tlv_size: usize = header.tlvs.iter().map(|t| 3 + t.value.len()).sum();

    // Transport nibble.
    let tp = match header.transport {
        TransportProtocol::Tcp | TransportProtocol::UnixStream => PROTO_STREAM,
        TransportProtocol::Udp | TransportProtocol::UnixDgram => PROTO_DGRAM,
        TransportProtocol::Unknown => PROTO_UNSPEC,
    };

    match &header.addresses {
        ProxyAddresses::Inet {
            source,
            destination,
        } if header.command == ProxyCommand::Proxy => {
            match (source.ip(), destination.ip()) {
                (IpAddr::V4(s4), IpAddr::V4(d4)) => {
                    buf.put_u8(AF_INET | tp);
                    buf.put_u16((ADDR_LEN_INET + tlv_size) as u16);
                    buf.put_slice(&s4.octets());
                    buf.put_slice(&d4.octets());
                    buf.put_u16(source.port());
                    buf.put_u16(destination.port());
                }
                (IpAddr::V6(s6), IpAddr::V6(d6)) => {
                    buf.put_u8(AF_INET6 | tp);
                    buf.put_u16((ADDR_LEN_INET6 + tlv_size) as u16);
                    buf.put_slice(&s6.octets());
                    buf.put_slice(&d6.octets());
                    buf.put_u16(source.port());
                    buf.put_u16(destination.port());
                }
                _ => {
                    // Mixed families: fall back to UNSPEC.
                    buf.put_u8(AF_UNSPEC | PROTO_UNSPEC);
                    buf.put_u16(tlv_size as u16);
                }
            }
        }
        ProxyAddresses::Unix {
            source,
            destination,
        } if header.command == ProxyCommand::Proxy => {
            buf.put_u8(AF_UNIX | tp);
            buf.put_u16((ADDR_LEN_UNIX + tlv_size) as u16);
            buf.put_slice(source);
            buf.put_slice(destination);
        }
        _ => {
            // No addresses or LOCAL command.
            buf.put_u8(AF_UNSPEC | PROTO_UNSPEC);
            buf.put_u16(tlv_size as u16);
        }
    }

    // Encode TLVs.
    for tlv in &header.tlvs {
        buf.put_u8(tlv.tlv_type);
        buf.put_u16(tlv.value.len() as u16);
        buf.put_slice(&tlv.value);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::tlv_types;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn make_v2_ipv4_buf(cmd: u8, src: Ipv4Addr, dst: Ipv4Addr, sp: u16, dp: u16) -> BytesMut {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(VERSION | cmd);
        buf.put_u8(AF_INET | PROTO_STREAM);
        buf.put_u16(ADDR_LEN_INET as u16);
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
        buf.put_u16(ADDR_LEN_INET6 as u16);
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
            hdr.source().unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 56324)
        );
        assert_eq!(
            hdr.destination().unwrap(),
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
        assert_eq!(hdr.source().unwrap().ip(), IpAddr::V6(src));
        assert_eq!(hdr.destination().unwrap().ip(), IpAddr::V6(dst));
        assert_eq!(hdr.source().unwrap().port(), 12345);
        assert_eq!(hdr.destination().unwrap().port(), 80);
    }

    #[test]
    fn decode_v2_local() {
        let mut buf = make_v2_local_buf();

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.command, ProxyCommand::Local);
        assert_eq!(hdr.transport, TransportProtocol::Unknown);
        assert!(hdr.source().is_none());
        assert!(hdr.destination().is_none());
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
    fn decode_v2_with_tlvs() {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(VERSION | CMD_PROXY);
        buf.put_u8(AF_INET | PROTO_STREAM);

        // Address (12 bytes) + TLV: ALPN "h2" (3 + 2 = 5) + AUTHORITY "example.com" (3 + 11 = 14)
        let tlv_size = 5 + 14;
        buf.put_u16((ADDR_LEN_INET + tlv_size) as u16);

        // Addresses.
        buf.put_slice(&[1, 2, 3, 4]); // src
        buf.put_slice(&[5, 6, 7, 8]); // dst
        buf.put_u16(1000);
        buf.put_u16(2000);

        // TLV: ALPN = "h2"
        buf.put_u8(tlv_types::PP2_TYPE_ALPN);
        buf.put_u16(2);
        buf.put_slice(b"h2");

        // TLV: AUTHORITY = "example.com"
        buf.put_u8(tlv_types::PP2_TYPE_AUTHORITY);
        buf.put_u16(11);
        buf.put_slice(b"example.com");

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.tlvs.len(), 2);
        assert_eq!(hdr.alpn(), Some(b"h2".as_slice()));
        assert_eq!(hdr.authority(), Some("example.com"));
    }

    #[test]
    fn decode_v2_unix() {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(VERSION | CMD_PROXY);
        buf.put_u8(AF_UNIX | PROTO_STREAM);
        buf.put_u16(ADDR_LEN_UNIX as u16);

        // Source path (108 bytes, null-padded).
        let mut src_path = [0u8; 108];
        let src_str = b"/var/run/src.sock";
        src_path[..src_str.len()].copy_from_slice(src_str);
        buf.put_slice(&src_path);

        // Destination path (108 bytes, null-padded).
        let mut dst_path = [0u8; 108];
        let dst_str = b"/var/run/dst.sock";
        dst_path[..dst_str.len()].copy_from_slice(dst_str);
        buf.put_slice(&dst_path);

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.command, ProxyCommand::Proxy);
        assert_eq!(hdr.transport, TransportProtocol::UnixStream);
        match &hdr.addresses {
            ProxyAddresses::Unix { source, destination } => {
                assert_eq!(&source[..src_str.len()], src_str);
                assert_eq!(&destination[..dst_str.len()], dst_str);
            }
            other => panic!("expected Unix addresses, got {:?}", other),
        }
    }

    #[test]
    fn encode_v2_ipv4() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 1000),
                destination: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)), 2000),
            },
            tlvs: Vec::new(),
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
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V6("fe80::1".parse().unwrap()), 100),
                destination: SocketAddr::new(IpAddr::V6("fe80::2".parse().unwrap()), 200),
            },
            tlvs: Vec::new(),
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
            addresses: ProxyAddresses::None,
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v2(&hdr, &mut buf);

        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded.command, ProxyCommand::Local);
        assert!(decoded.source().is_none());
    }

    #[test]
    fn encode_v2_no_addresses_falls_back() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::None,
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v2(&hdr, &mut buf);

        // Should still decode (as UNSPEC with no addresses).
        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert!(decoded.source().is_none());
    }

    #[test]
    fn decode_v2_normalizes_ipv4_mapped() {
        let src: Ipv6Addr = "::ffff:192.168.1.1".parse().unwrap();
        let dst: Ipv6Addr = "::ffff:10.0.0.1".parse().unwrap();
        let mut buf = make_v2_ipv6_buf(CMD_PROXY, src, dst, 1234, 5678);

        let hdr = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(
            hdr.source().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))
        );
        assert_eq!(
            hdr.destination().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
    }

    #[test]
    fn roundtrip_v2_udp() {
        let original = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Udp,
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)), 9999),
                destination: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 2)), 8888),
            },
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v2(&original, &mut buf);
        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn roundtrip_v2_with_tlvs() {
        let original = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 443),
                destination: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 8443),
            },
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
        let mut buf = BytesMut::new();
        encode_v2(&original, &mut buf);
        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn roundtrip_v2_unix_stream() {
        let mut src_path = [0u8; 108];
        let mut dst_path = [0u8; 108];
        src_path[..5].copy_from_slice(b"/tmp/");
        dst_path[..6].copy_from_slice(b"/sock/");

        let original = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::UnixStream,
            addresses: ProxyAddresses::Unix {
                source: src_path,
                destination: dst_path,
            },
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v2(&original, &mut buf);
        let decoded = decode_v2(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn malformed_tlv_rejected() {
        let mut buf = BytesMut::new();
        buf.put_slice(&V2_SIGNATURE);
        buf.put_u8(VERSION | CMD_PROXY);
        buf.put_u8(AF_INET | PROTO_STREAM);

        // Addr (12) + TLV header (3 bytes: type + 2-byte length claiming 100)
        // + 2 bytes of actual value = 5 TLV bytes total.
        // The TLV claims 100 bytes but only 2 are present in the declared
        // payload, which should trigger a MalformedTlv error.
        let tlv_bytes = 3 + 2; // type(1) + len(2) + value(2)
        buf.put_u16((ADDR_LEN_INET + tlv_bytes) as u16);
        buf.put_slice(&[1, 2, 3, 4]); // src IP
        buf.put_slice(&[5, 6, 7, 8]); // dst IP
        buf.put_u16(1000);
        buf.put_u16(2000);
        // Broken TLV: claims 100 bytes but only 2 available
        buf.put_u8(0x01);
        buf.put_u16(100);
        buf.put_slice(&[0xAA, 0xBB]); // only 2 bytes of the claimed 100

        let result = decode_v2(&mut buf);
        assert!(result.is_err());
    }
}
