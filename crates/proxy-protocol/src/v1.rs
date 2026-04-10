//! PROXY protocol v1 (text/human-readable format).
//!
//! Format: `PROXY TCP4|TCP6|UNKNOWN <srcIP> <dstIP> <srcPort> <dstPort>\r\n`
//! Maximum line length is 108 bytes including the trailing `\r\n`.

use std::net::{IpAddr, SocketAddr};

use bytes::{Buf, BufMut, BytesMut};
use tracing::trace;

use crate::header::{ProxyCommand, ProxyHeader, TransportProtocol, same_family};

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

    // We have a complete line – parse it.
    let line = &buf[..crlf_pos]; // without CRLF
    let line_str = std::str::from_utf8(line)
        .map_err(|e| V1Error::InvalidAddress(format!("non-UTF8 line: {e}")))?;

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
/// are missing, the `UNKNOWN` family is emitted with no addresses.
pub fn encode_v1(header: &ProxyHeader, buf: &mut BytesMut) {
    match (header.source, header.destination) {
        (Some(src), Some(dst)) if same_family(Some(src), Some(dst)) => {
            let family = match src.ip() {
                IpAddr::V4(_) => "TCP4",
                IpAddr::V6(_) => "TCP6",
            };
            let line = format!(
                "PROXY {} {} {} {} {}\r\n",
                family,
                src.ip(),
                dst.ip(),
                src.port(),
                dst.port(),
            );
            buf.put_slice(line.as_bytes());
        }
        _ => {
            buf.put_slice(b"PROXY UNKNOWN\r\n");
        }
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn find_crlf(data: &[u8]) -> Option<usize> {
    let limit = data.len().min(V1_MAX_LENGTH);
    data[..limit].windows(2).position(|w| w == b"\r\n")
}

fn parse_v1_line(line: &str) -> Result<ProxyHeader, V1Error> {
    if !line.starts_with("PROXY ") {
        return Err(V1Error::MissingPrefix);
    }

    let rest = &line["PROXY ".len()..];
    let parts: Vec<&str> = rest.split(' ').collect();

    // UNKNOWN may appear with no trailing fields.
    if parts.first() == Some(&"UNKNOWN") {
        return Ok(ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Unknown,
            source: None,
            destination: None,
        });
    }

    if parts.len() != 5 {
        return Err(V1Error::WrongFieldCount(parts.len() + 1)); // +1 for PROXY token
    }

    let transport = match parts[0] {
        "TCP4" | "TCP6" => TransportProtocol::Tcp,
        other => return Err(V1Error::UnknownFamily(other.to_owned())),
    };

    let src_ip: IpAddr = parts[1]
        .parse()
        .map_err(|e| V1Error::InvalidAddress(format!("{}: {e}", parts[1])))?;
    let dst_ip: IpAddr = parts[2]
        .parse()
        .map_err(|e| V1Error::InvalidAddress(format!("{}: {e}", parts[2])))?;
    let src_port: u16 = parts[3]
        .parse()
        .map_err(|e| V1Error::InvalidPort(format!("{}: {e}", parts[3])))?;
    let dst_port: u16 = parts[4]
        .parse()
        .map_err(|e| V1Error::InvalidPort(format!("{}: {e}", parts[4])))?;

    let mut header = ProxyHeader {
        command: ProxyCommand::Proxy,
        transport,
        source: Some(SocketAddr::new(src_ip, src_port)),
        destination: Some(SocketAddr::new(dst_ip, dst_port)),
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
            hdr.source.unwrap(),
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)), 56324)
        );
        assert_eq!(
            hdr.destination.unwrap(),
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
            hdr.source.unwrap().ip(),
            IpAddr::V6("2001:db8::1".parse::<Ipv6Addr>().unwrap())
        );
    }

    #[test]
    fn decode_unknown() {
        let mut buf = BytesMut::from("PROXY UNKNOWN\r\n");
        let hdr = decode_v1(&mut buf).unwrap().unwrap();
        assert_eq!(hdr.transport, TransportProtocol::Unknown);
        assert!(hdr.source.is_none());
        assert!(hdr.destination.is_none());
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
    fn encode_tcp4() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            source: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 1000)),
            destination: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(5, 6, 7, 8)), 2000)),
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
            source: Some(SocketAddr::new(IpAddr::V6("::1".parse().unwrap()), 100)),
            destination: Some(SocketAddr::new(IpAddr::V6("::2".parse().unwrap()), 200)),
        };
        let mut buf = BytesMut::new();
        encode_v1(&hdr, &mut buf);
        let s = std::str::from_utf8(&buf).unwrap();
        assert!(s.starts_with("PROXY TCP6"));
        assert!(s.ends_with("\r\n"));
    }

    #[test]
    fn encode_mixed_families_produces_unknown() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Proxy,
            transport: TransportProtocol::Tcp,
            source: Some(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(1, 2, 3, 4)), 100)),
            destination: Some(SocketAddr::new(IpAddr::V6("::1".parse().unwrap()), 200)),
        };
        let mut buf = BytesMut::new();
        encode_v1(&hdr, &mut buf);
        assert_eq!(&buf[..], b"PROXY UNKNOWN\r\n");
    }

    #[test]
    fn encode_missing_addresses_produces_unknown() {
        let hdr = ProxyHeader {
            command: ProxyCommand::Local,
            transport: TransportProtocol::Unknown,
            source: None,
            destination: None,
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
            source: Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)),
                11111,
            )),
            destination: Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
                22222,
            )),
        };
        let mut buf = BytesMut::new();
        encode_v1(&original, &mut buf);

        let decoded = decode_v1(&mut buf).unwrap().unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn decode_normalizes_ipv4_mapped() {
        let mut buf = BytesMut::from("PROXY TCP6 ::ffff:192.168.1.1 ::ffff:10.0.0.1 1234 5678\r\n");
        let hdr = decode_v1(&mut buf).unwrap().unwrap();
        assert_eq!(
            hdr.source.unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))
        );
        assert_eq!(
            hdr.destination.unwrap().ip(),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
    }
}
