//! PROXY protocol v1 (text/human-readable format).
//!
//! Format: `PROXY TCP4|TCP6|UDP4|UDP6|UNKNOWN [<srcIP> <dstIP> <srcPort> <dstPort>]\r\n`
//! Maximum line length is 108 bytes including the trailing `\r\n`.
//!
//! Reference: HAProxy PROXY protocol specification, section 2.1.

use std::fmt::Write as FmtWrite;
use std::net::{IpAddr, SocketAddr};

use bytes::{Buf, BufMut, BytesMut};
use tracing::trace;

use crate::header::{
    ProxyAddresses, ProxyCommand, ProxyHeader, TransportProtocol, same_family,
};

/// Maximum length of a v1 PROXY protocol line (including CRLF).
pub const V1_MAX_LENGTH: usize = 108;

/// The v1 prefix that begins every v1 header.
pub const V1_PREFIX: &[u8] = b"PROXY ";

/// Errors specific to v1 parsing.
#[derive(Debug, thiserror::Error)]
pub enum V1Error {
    #[error("line exceeds maximum length of {V1_MAX_LENGTH} bytes")]
    LineTooLong,

    #[error("missing CRLF terminator")]
    MissingCrlf,

    #[error("missing PROXY prefix")]
    MissingPrefix,

    #[error("unknown protocol family: {0}")]
    UnknownFamily(String),

    #[error("invalid address: {0}")]
    InvalidAddress(String),

    #[error("invalid port: {0}")]
    InvalidPort(String),

    #[error("wrong number of fields (expected 6, got {0})")]
    WrongFieldCount(usize),

    #[error("non-UTF8 data in header")]
    InvalidUtf8,
}

/// Attempt to decode a v1 PROXY protocol header from the front of `buf`.
///
/// On success the consumed bytes are removed from `buf` and the parsed
/// [`ProxyHeader`] is returned.  The header is automatically normalized
/// (IPv4-mapped IPv6 addresses become plain IPv4).
///
/// Returns `Ok(None)` when there are not enough bytes yet (i.e. no CRLF
/// found within the first 108 bytes).
pub fn decode_v1(buf: &mut BytesMut) -> Result<Option<ProxyHeader>, V1Error> {
    // Find the line terminator.
    let crlf_pos = match find_crlf(buf.as_ref()) {
        Some(pos) => pos,
        None => {
            if buf.len() >= V1_MAX_LENGTH {
                return Err(V1Error::LineTooLong);
            }
            return Ok(None); // need more data
        }
    };

    let line_len = crlf_pos + 2; // include \r\n
    if line_len > V1_MAX_LENGTH {
        return Err(V1Error::LineTooLong);
    }

    // We have a complete line -- parse it.
    let line = &buf[..crlf_pos]; // without CRLF
    let line_str = std::str::from_utf8(line).map_err(|_| V1Error::InvalidUtf8)?;

    let header = parse_v1_line(line_str)?;

    // Consume the bytes.
    buf.advance(line_len);

    trace!(?header, "decoded PROXY protocol v1 header");
    Ok(Some(header))
}

/// Encode a [`ProxyHeader`] into a v1 text representation and append it to
/// `buf`.
///
/// If the source and destination are not the same IP family, or if addresses
/// are not inet addresses, the `UNKNOWN` family is emitted with no addresses.
///
/// Note: v1 only supports TCP.  UDP addresses are encoded with UDP4/UDP6
/// family strings for compatibility with implementations that support it,
/// but the canonical v1 format only defines TCP4/TCP6/UNKNOWN.
pub fn encode_v1(header: &ProxyHeader, buf: &mut BytesMut) {
    match &header.addresses {
        ProxyAddresses::Inet {
            source,
            destination,
        } if header.command == ProxyCommand::Proxy && same_family(*source, *destination) => {
            let family = match (source.ip(), &header.transport) {
                (IpAddr::V4(_), TransportProtocol::Udp) => "UDP4",
                (IpAddr::V6(_), TransportProtocol::Udp) => "UDP6",
                (IpAddr::V4(_), _) => "TCP4",
                (IpAddr::V6(_), _) => "TCP6",
            };
            // Use a stack buffer to avoid heap allocation.  V1_MAX_LENGTH is
            // 108 bytes, which easily fits on the stack.
            let mut line_buf = [0u8; V1_MAX_LENGTH];
            // Write into a &mut [u8] via fmt::Write on a wrapper.
            let written = {
                let mut cursor = StackWriter {
                    buf: &mut line_buf,
                    pos: 0,
                };
                // This cannot fail -- the buffer is large enough.
                let _ = write!(
                    cursor,
                    "PROXY {} {} {} {} {}\r\n",
                    family,
                    source.ip(),
                    destination.ip(),
                    source.port(),
                    destination.port(),
                );
                cursor.pos
            };
            buf.put_slice(&line_buf[..written]);
        }
        _ => {
            buf.put_slice(b"PROXY UNKNOWN\r\n");
        }
    }
}

/// A `fmt::Write` adapter that writes into a fixed stack buffer.
struct StackWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl FmtWrite for StackWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = self.buf.len() - self.pos;
        if bytes.len() > remaining {
            return Err(std::fmt::Error);
        }
        self.buf[self.pos..self.pos + bytes.len()].copy_from_slice(bytes);
        self.pos += bytes.len();
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

#[inline]
fn find_crlf(data: &[u8]) -> Option<usize> {
    let limit = data.len().min(V1_MAX_LENGTH);
    data[..limit].windows(2).position(|w| w == b"\r\n")
}

fn parse_v1_line(line: &str) -> Result<ProxyHeader, V1Error> {
    if !line.starts_with("PROXY ") {
        return Err(V1Error::MissingPrefix);
    }

    let rest = &line["PROXY ".len()..];

    // Parse without allocating a Vec: use split and count fields manually.
    let mut fields = rest.split(' ');

    let family = fields.next().ok_or(V1Error::MissingPrefix)?;

    // UNKNOWN may appear with no trailing fields.
    if family == "UNKNOWN" {
        return Ok(ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Unknown,
            addresses: ProxyAddresses::None,
            tlvs: Vec::new(),
        });
    }

    let transport = match family {
        "TCP4" | "TCP6" => TransportProtocol::Tcp,
        "UDP4" | "UDP6" => TransportProtocol::Udp,
        other => return Err(V1Error::UnknownFamily(other.to_owned())),
    };

    let src_ip_str = fields
        .next()
        .ok_or(V1Error::WrongFieldCount(2))?;
    let dst_ip_str = fields
        .next()
        .ok_or(V1Error::WrongFieldCount(3))?;
    let src_port_str = fields
        .next()
        .ok_or(V1Error::WrongFieldCount(4))?;
    let dst_port_str = fields
        .next()
        .ok_or(V1Error::WrongFieldCount(5))?;

    // Reject extra fields.
    if fields.next().is_some() {
        return Err(V1Error::WrongFieldCount(7));
    }

    let src_ip: IpAddr = src_ip_str
        .parse()
        .map_err(|e| V1Error::InvalidAddress(format!("{}: {e}", src_ip_str)))?;
    let dst_ip: IpAddr = dst_ip_str
        .parse()
        .map_err(|e| V1Error::InvalidAddress(format!("{}: {e}", dst_ip_str)))?;
    let src_port: u16 = src_port_str
        .parse()
        .map_err(|e| V1Error::InvalidPort(format!("{}: {e}", src_port_str)))?;
    let dst_port: u16 = dst_port_str
        .parse()
        .map_err(|e| V1Error::InvalidPort(format!("{}: {e}", dst_port_str)))?;

    let mut header = ProxyHeader {
        command: ProxyCommand::Proxy,
        transport,
        addresses: ProxyAddresses::Inet {
            source: SocketAddr::new(src_ip, src_port),
            destination: SocketAddr::new(dst_ip, dst_port),
        },
        tlvs: Vec::new(),
    };
    header.normalize();
    Ok(header)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn decode_tcp4() {
        let mut buf = BytesMut::from("PROXY TCP4 192.168.1.1 10.0.0.1 56324 443\r\nextra");
        let hdr = decode_v1(&mut buf).unwrap().unwrap();
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
        // remaining data should still be in the buffer
        assert_eq!(&buf[..], b"extra");
    }

    #[test]
    fn decode_tcp6() {
        let mut buf = BytesMut::from("PROXY TCP6 2001:db8::1 2001:db8::2 12345 80\r\n");
        let hdr = decode_v1(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.transport, TransportProtocol::Tcp);
        assert_eq!(
            hdr.source().unwrap().ip(),
            IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[test]
    fn decode_unknown() {
        let mut buf = BytesMut::from("PROXY UNKNOWN\r\n");
        let hdr = decode_v1(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.transport, TransportProtocol::Unknown);
        assert!(hdr.source().is_none());
        assert!(hdr.destination().is_none());
    }

    #[test]
    fn decode_incomplete() {
        let mut buf = BytesMut::from("PROXY TCP4 192.168.1.1 10");
        assert!(decode_v1(&mut buf).unwrap().is_none());
    }

    #[test]
    fn decode_too_long() {
        let long = format!("PROXY TCP4 {} {} 1 2\r\n", "x".repeat(80), "y".repeat(80));
        let mut buf = BytesMut::from(long.as_str());
        assert!(decode_v1(&mut buf).is_err());
    }

    #[test]
    fn decode_extra_fields_rejected() {
        let mut buf = BytesMut::from("PROXY TCP4 1.2.3.4 5.6.7.8 100 200 extra\r\n");
        assert!(decode_v1(&mut buf).is_err());
    }

    #[test]
    fn encode_tcp4() {
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
        encode_v1(&hdr, &mut buf);
        assert_eq!(&buf[..], b"PROXY TCP4 1.2.3.4 5.6.7.8 1000 2000\r\n");
    }

    #[test]
    fn encode_tcp6() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V6("::1".parse().unwrap()), 100),
                destination: SocketAddr::new(IpAddr::V6("::2".parse().unwrap()), 200),
            },
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v1(&hdr, &mut buf);
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.starts_with("PROXY TCP6"));
        assert!(s.ends_with("\r\n"));
    }

    #[test]
    fn encode_udp4() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Udp,
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 1000),
                destination: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)), 2000),
            },
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v1(&hdr, &mut buf);
        assert_eq!(&buf[..], b"PROXY UDP4 1.2.3.4 5.6.7.8 1000 2000\r\n");
    }

    #[test]
    fn encode_mixed_families_produces_unknown() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 100),
                destination: SocketAddr::new(IpAddr::V6("::1".parse().unwrap()), 200),
            },
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v1(&hdr, &mut buf);
        assert_eq!(&buf[..], b"PROXY UNKNOWN\r\n");
    }

    #[test]
    fn encode_no_addresses_produces_unknown() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Local,
            transport: TransportProtocol::Unknown,
            addresses: ProxyAddresses::None,
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v1(&hdr, &mut buf);
        assert_eq!(&buf[..], b"PROXY UNKNOWN\r\n");
    }

    #[test]
    fn roundtrip_v1_tcp4() {
        let original = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            addresses: ProxyAddresses::Inet {
                source: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 11111),
                destination: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)), 22222),
            },
            tlvs: Vec::new(),
        };
        let mut buf = BytesMut::new();
        encode_v1(&original, &mut buf);

        let decoded = decode_v1(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_normalizes_ipv4_mapped() {
        let mut buf =
            BytesMut::from("PROXY TCP6 ::ffff:192.168.1.1 ::ffff:10.0.0.1 1234 5678\r\n");
        let hdr = decode_v1(&mut buf).unwrap().unwrap();
        assert_eq!(
            hdr.source().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))
        );
        assert_eq!(
            hdr.destination().unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
    }
}
