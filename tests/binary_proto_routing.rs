//! D3-1 — binary's per-listener upstream-protocol fan-out wiring.
//!
//! `crates/lb/src/main.rs::build_listener_mode` translates each
//! `BackendConfig::protocol` (validated by `lb_config`) into an
//! `UpstreamBackend` carrying the matching `UpstreamProto`, then builds
//! the per-listener H2 / H3 upstream pools on demand and threads them
//! into `H1Proxy::with_multi_proto` via the fluent
//! `with_h2_upstream` / `with_h3_upstream` setters.
//!
//! The binary's private helpers cannot be imported from an integration
//! test (the `lb` crate is a `[[bin]]`), so we replicate the small
//! partition + construction logic here against the same public
//! building blocks — `lb_l7::upstream::*`, `lb_io::http2_pool::Http2Pool`
//! — and prove:
//!
//! 1. A mixed-protocol listener (one h1 backend + one h2 backend) yields
//!    an `H1Proxy` whose `has_h2_upstream()` is `true`.
//! 2. The picker the proxy holds round-robins through both backends,
//!    each tagged with the matching `UpstreamProto`.
//! 3. An `LbConfig` carrying that listener round-trips through
//!    `lb_config::validate_config` cleanly (protocol parsing matches).

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::net::SocketAddr;
use std::sync::Arc;

use lb_config::{BackendConfig, LbConfig, ListenerConfig};
use lb_io::Runtime;
use lb_io::http2_pool::{Http2Pool, Http2PoolConfig};
use lb_io::pool::{PoolConfig, TcpPool};
use lb_io::sockopts::BackendSockOpts;
use lb_l7::h1_proxy::{H1Proxy, HttpTimeouts};
use lb_l7::upstream::{BackendInfoPicker, RoundRobinUpstreams, UpstreamBackend, UpstreamProto};

fn build_pool() -> TcpPool {
    TcpPool::new(
        PoolConfig::default(),
        BackendSockOpts {
            nodelay: true,
            keepalive: false,
            rcvbuf: None,
            sndbuf: None,
            quickack: false,
            tcp_fastopen_connect: false,
        },
        Runtime::new(),
    )
}

/// Mirror the binary's protocol parser. Kept in lockstep with
/// `crates/lb/src/main.rs::parse_upstream_proto`.
fn parse_proto(s: &str) -> UpstreamProto {
    match s {
        "tcp" | "h1" => UpstreamProto::H1,
        "h2" => UpstreamProto::H2,
        "h3" => UpstreamProto::H3,
        other => panic!("unknown protocol {other:?}"),
    }
}

#[test]
fn mixed_protocol_listener_yields_h1_proxy_with_h2_pool_and_round_robin_picker() {
    // Build the same LbConfig shape main.rs would parse from TOML.
    let listener = ListenerConfig {
        address: "127.0.0.1:0".to_owned(),
        protocol: "h1".to_owned(),
        tls: None,
        quic: None,
        alt_svc: None,
        http: None,
        h2_security: None,
        websocket: None,
        grpc: None,
        backends: vec![
            BackendConfig {
                address: "127.0.0.1:9001".to_owned(),
                protocol: "h1".to_owned(),
                weight: 1,
            },
            BackendConfig {
                address: "127.0.0.1:9002".to_owned(),
                protocol: "h2".to_owned(),
                weight: 1,
            },
        ],
    };
    let cfg = LbConfig {
        listeners: vec![listener.clone()],
        observability: None,
        runtime: None,
    };
    // Validation accepts mixed-protocol backends (lb_config range:
    // tcp/h1/h2/h3).
    lb_config::validate_config(&cfg).expect("config validates");

    // Translate to UpstreamBackend (same shape main.rs builds via
    // build_upstream_backends).
    let resolved: Vec<SocketAddr> = listener
        .backends
        .iter()
        .map(|b| b.address.parse().unwrap())
        .collect();
    let upstreams: Vec<UpstreamBackend> = listener
        .backends
        .iter()
        .zip(resolved.iter().copied())
        .map(|(b, addr)| UpstreamBackend {
            addr,
            proto: parse_proto(&b.protocol),
            sni: None,
        })
        .collect();

    // The mixed-protocol listener has at least one H2 backend, so
    // build_listener_mode would attach an Http2Pool. Replicate that.
    let needs_h2 = upstreams.iter().any(|b| b.proto == UpstreamProto::H2);
    assert!(needs_h2, "fixture must include an H2 backend");
    let tcp_pool = build_pool();
    let h2_pool = Arc::new(Http2Pool::new(Http2PoolConfig::default(), tcp_pool.clone()));

    let picker = Arc::new(RoundRobinUpstreams::new(upstreams).unwrap());
    let proxy = H1Proxy::with_multi_proto(
        tcp_pool,
        Arc::clone(&picker) as Arc<dyn BackendInfoPicker>,
        None,
        HttpTimeouts::default(),
        /* is_https = */ false,
    )
    .with_h2_upstream(h2_pool);

    // Assertion 1: H2 upstream pool is wired.
    assert!(
        proxy.has_h2_upstream(),
        "H1Proxy::with_h2_upstream must be observable via has_h2_upstream()"
    );
    assert!(
        !proxy.has_h3_upstream(),
        "no H3 backend in fixture, pool must not be wired"
    );

    // Assertion 2: the picker round-robins through both backends and
    // tags them with the right protocol.
    let p1 = picker.pick_info().unwrap();
    let p2 = picker.pick_info().unwrap();
    let p3 = picker.pick_info().unwrap();
    assert_eq!(p1.proto, UpstreamProto::H1);
    assert_eq!(p1.addr, resolved[0]);
    assert_eq!(p2.proto, UpstreamProto::H2);
    assert_eq!(p2.addr, resolved[1]);
    // Wraps back to the first backend.
    assert_eq!(p3.addr, resolved[0]);
}
