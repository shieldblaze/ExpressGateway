//! HAProxy PROXY protocol v1/v2 implementation.
//!
//! This crate provides encoding and decoding of the HAProxy PROXY protocol
//! in both text (v1) and binary (v2) formats, plus auto-detection of which
//! version a given byte stream uses.
//!
//! # Features
//!
//! - **v1**: Text format (`PROXY TCP4|TCP6|UDP4|UDP6|UNKNOWN ...`).
//! - **v2**: Binary format with 12-byte signature, supporting:
//!   - AF_INET, AF_INET6, AF_UNIX address families
//!   - STREAM and DGRAM transport protocols
//!   - TLV extensions (ALPN, SSL, Authority, etc.)
//! - **Auto-detection**: Inspect the first 12 bytes to determine version.
//! - **Zero-copy parsing**: Parses directly from `BytesMut` without
//!   intermediate heap allocations on the hot path.
//!
//! # Quick start
//!
//! ```rust
//! use bytes::BytesMut;
//! use expressgateway_proxy_protocol::{detect, ProxyVersion, v1, v2};
//!
//! let mut buf = BytesMut::from("PROXY TCP4 1.2.3.4 5.6.7.8 100 200\r\n");
//! match detect::detect(&buf) {
//!     Some(ProxyVersion::V1) => {
//!         let header = v1::decode_v1(&mut buf).unwrap().unwrap();
//!         println!("client: {:?}", header.real_client_address());
//!     }
//!     Some(ProxyVersion::V2) => {
//!         let header = v2::decode_v2(&mut buf).unwrap().unwrap();
//!         println!("client: {:?}", header.real_client_address());
//!     }
//!     None => println!("no PROXY protocol detected"),
//! }
//! ```

pub mod detect;
pub mod header;
pub mod v1;
pub mod v2;

// Re-export key types at crate root for convenience.
pub use detect::ProxyVersion;
pub use header::{
    ProxyAddresses, ProxyCommand, ProxyHeader, Tlv, TransportProtocol,
    tlv_types,
};
