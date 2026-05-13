//! Integration coverage for the production [`HooksBundle`] —
//! every [`SecurityReject`] arm is exercised end-to-end through the
//! public [`SecurityHooks`] trait.
//!
//! Companion to the in-crate unit tests under `src/hooks.rs::tests`.
//! Lives in `tests/` so it consumes the crate as a downstream
//! Wave-2b caller will, asserting the public API surface is
//! self-sufficient (no `pub(crate)` reach-through).

use std::net::{IpAddr, Ipv4Addr};

use http::{HeaderValue, Method, Request, Version};
use lb_security::{ConnGate, HooksBundle, SecurityHooks, SecurityReject, SmuggleMode};

fn req(headers: &[(&'static str, &'static str)], version: Version) -> Request<()> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("http://example.com/")
        .version(version)
        .body(())
        .unwrap();
    for (n, v) in headers {
        req.headers_mut()
            .append(*n, HeaderValue::from_str(v).unwrap());
    }
    req
}

fn peer() -> IpAddr {
    Ipv4Addr::LOCALHOST.into()
}

#[test]
fn smuggle_cl_te_is_rejected_as_smuggle() {
    let gate = ConnGate::new(8, 4, Vec::new());
    let hooks = HooksBundle::new(gate, SmuggleMode::H1);
    let r = req(
        &[("content-length", "5"), ("transfer-encoding", "chunked")],
        Version::HTTP_11,
    );
    match hooks.inspect_request(&r, peer()) {
        Err(SecurityReject::Smuggle(_)) => {}
        other => panic!("expected Smuggle rejection, got {other:?}"),
    }
}

#[test]
fn smuggle_duplicate_cl_differing_is_rejected() {
    let gate = ConnGate::new(8, 4, Vec::new());
    let hooks = HooksBundle::new(gate, SmuggleMode::H1);
    let r = req(
        &[("content-length", "10"), ("content-length", "20")],
        Version::HTTP_11,
    );
    assert!(matches!(
        hooks.inspect_request(&r, peer()),
        Err(SecurityReject::Smuggle(_))
    ));
}

#[test]
fn smuggle_h2_downgrade_connection_header_rejected() {
    let gate = ConnGate::new(8, 4, Vec::new());
    let hooks = HooksBundle::new(gate, SmuggleMode::H1);
    let r = req(&[("connection", "keep-alive")], Version::HTTP_2);
    assert!(matches!(
        hooks.inspect_request(&r, peer()),
        Err(SecurityReject::Smuggle(_))
    ));
}

#[test]
fn smuggle_strict_te_gzip_chunked_rejected_only_under_strict() {
    let gate1 = ConnGate::new(8, 4, Vec::new());
    let gate2 = ConnGate::new(8, 4, Vec::new());
    let lenient = HooksBundle::new(gate1, SmuggleMode::H1);
    let strict = HooksBundle::new(gate2, SmuggleMode::H1Strict);
    let r = req(&[("transfer-encoding", "gzip, chunked")], Version::HTTP_11);
    assert!(lenient.inspect_request(&r, peer()).is_ok());
    assert!(matches!(
        strict.inspect_request(&r, peer()),
        Err(SecurityReject::Smuggle(_))
    ));
}

#[test]
fn admit_returns_permit_and_drop_releases() {
    let gate = ConnGate::new(4, 4, Vec::new());
    let hooks = HooksBundle::new(gate.clone(), SmuggleMode::H1);
    let p1 = hooks.admit_connection(peer()).unwrap();
    assert_eq!(gate.current_peer_count(peer()), 1);
    drop(p1);
    assert_eq!(gate.current_peer_count(peer()), 0);
}

#[test]
fn admit_listener_full_rejected_with_overcap() {
    let gate = ConnGate::new(2, 100, Vec::new());
    let hooks = HooksBundle::new(gate, SmuggleMode::H1);
    let _p1 = hooks.admit_connection(peer()).unwrap();
    let _p2 = hooks.admit_connection(peer()).unwrap();
    match hooks.admit_connection(peer()) {
        Err(SecurityReject::OverCap(_)) => {}
        other => panic!("expected OverCap rejection, got {other:?}"),
    }
}

#[test]
fn admit_per_ip_full_rolls_back_listener_count() {
    let gate = ConnGate::new(100, 1, Vec::new());
    let hooks = HooksBundle::new(gate.clone(), SmuggleMode::H1);
    let _p1 = hooks.admit_connection(peer()).unwrap();
    assert_eq!(gate.current_listener_count(), 1);
    match hooks.admit_connection(peer()) {
        Err(SecurityReject::OverCap(_)) => {}
        other => panic!("expected per-ip OverCap, got {other:?}"),
    }
    // Per-ip overflow must roll back the listener counter so it
    // doesn't leak. Otherwise the listener cap would erode under any
    // sustained over-cap stream.
    assert_eq!(gate.current_listener_count(), 1);
}

#[test]
fn clean_request_passes_inspect() {
    let gate = ConnGate::new(8, 4, Vec::new());
    let hooks = HooksBundle::new(gate, SmuggleMode::H1);
    let r = req(&[("host", "example.com")], Version::HTTP_11);
    assert!(hooks.inspect_request(&r, peer()).is_ok());
}
