//! Proof for the per-IP / per-listener connection gate (SEC-2-04).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::Arc;
use std::thread;

use lb_security::{ConnGate, IpNet, OverCap};

fn loopback_v4() -> IpAddr {
    Ipv4Addr::LOCALHOST.into()
}

fn loopback_v6() -> IpAddr {
    Ipv6Addr::LOCALHOST.into()
}

#[test]
fn test_listener_cap_enforced() {
    let gate = ConnGate::new(3, 100, Vec::new());
    let p1 = gate.admit(loopback_v4()).unwrap();
    let p2 = gate.admit(loopback_v4()).unwrap();
    let p3 = gate.admit(loopback_v4()).unwrap();
    // Per-IP cap is 100 and nowhere near saturation, so this must fail as Listener, not PerIp.
    let err = gate.admit(loopback_v4()).unwrap_err();
    assert!(matches!(err, OverCap::Listener(3)));
    assert_eq!(gate.current_listener_count(), 3);
    drop(p1);
    drop(p2);
    drop(p3);
    assert_eq!(gate.current_listener_count(), 0);
}

#[test]
fn test_listener_cap_releases_on_drop() {
    let gate = ConnGate::new(2, 100, Vec::new());
    let p1 = gate.admit(loopback_v4()).unwrap();
    let _p2 = gate.admit(loopback_v4()).unwrap();
    assert!(gate.admit(loopback_v4()).is_err());
    drop(p1);
    let _p3 = gate.admit(loopback_v4()).unwrap();
}

#[test]
fn test_per_ip_cap_drops_on_drop() {
    let gate = ConnGate::new(100, 2, Vec::new());
    let p1 = gate.admit(loopback_v4()).unwrap();
    let p2 = gate.admit(loopback_v4()).unwrap();
    assert_eq!(gate.current_peer_count(loopback_v4()), 2);
    let err = gate.admit(loopback_v4()).unwrap_err();
    match err {
        OverCap::PerIp { addr, count } => {
            assert_eq!(addr, loopback_v4());
            assert_eq!(count, 2);
        }
        other => panic!("expected PerIp, got {other:?}"),
    }
    drop(p1);
    let _p3 = gate.admit(loopback_v4()).unwrap();
    drop(p2);
}

#[test]
fn per_ip_full_does_not_consume_listener_slot() {
    // Regression guard for the rollback in ConnGate::admit: without it, a sustained over-cap
    // stream from one attacker silently erodes the listener cap.
    let gate = ConnGate::new(100, 1, Vec::new());
    let _p1 = gate.admit(loopback_v4()).unwrap();
    assert_eq!(gate.current_listener_count(), 1);
    for _ in 0..50 {
        assert!(gate.admit(loopback_v4()).is_err());
    }
    // Still 1 — every per-IP rejection rolled the listener counter back.
    assert_eq!(gate.current_listener_count(), 1);
}

#[test]
fn per_ip_cap_independent_across_ips() {
    let gate = ConnGate::new(10, 1, Vec::new());
    let _p_v4 = gate.admit(loopback_v4()).unwrap();
    let _p_v6 = gate.admit(loopback_v6()).unwrap();
    let other_v4: IpAddr = Ipv4Addr::new(192, 0, 2, 1).into();
    let _p_other = gate.admit(other_v4).unwrap();
    assert!(gate.admit(loopback_v4()).is_err());
    assert!(gate.admit(loopback_v6()).is_err());
    assert!(gate.admit(other_v4).is_err());
}

#[test]
fn per_ip_entry_gcs_when_count_drops_to_zero() {
    let gate = ConnGate::new(10, 4, Vec::new());
    {
        let _p = gate.admit(loopback_v4()).unwrap();
        assert_eq!(gate.current_peer_count(loopback_v4()), 1);
    }
    assert_eq!(gate.current_peer_count(loopback_v4()), 0);
}

#[test]
fn trusted_cidrs_field_round_trips() {
    let cidrs = vec![
        IpNet::new(Ipv4Addr::new(10, 0, 0, 0).into(), 8),
        IpNet::new(Ipv6Addr::new(0x2001, 0xdb8, 0, 0, 0, 0, 0, 0).into(), 32),
    ];
    let gate = ConnGate::new(8, 4, cidrs.clone());
    assert_eq!(gate.trusted_cidrs(), cidrs.as_slice());
}

#[test]
fn trusted_cidrs_do_not_currently_exempt() {
    // Pins the DEFERRED behaviour: `trusted_cidrs` exists but exempts nobody yet. When L-002
    // lands this test must fail loudly.
    let cidrs = vec![IpNet::new(loopback_v4(), 32)];
    let gate = ConnGate::new(10, 1, cidrs);
    let _p = gate.admit(loopback_v4()).unwrap();
    assert!(matches!(
        gate.admit(loopback_v4()).unwrap_err(),
        OverCap::PerIp { .. }
    ));
}

#[test]
fn concurrent_admits_observe_cap() {
    // Concurrent admits must never exceed the cap in total.
    let gate = ConnGate::new(8, 1000, Vec::new());
    let gate = Arc::new(gate);
    let mut handles = Vec::new();
    for i in 0..32 {
        let g = Arc::clone(&gate);
        let addr: IpAddr = Ipv4Addr::new(10, 0, 0, u8::try_from(i % 200).unwrap_or(0)).into();
        handles.push(thread::spawn(move || g.admit(addr).ok()));
    }
    let mut acquired = 0;
    let mut permits = Vec::new();
    for h in handles {
        if let Some(p) = h.join().unwrap() {
            acquired += 1;
            permits.push(p);
        }
    }
    assert!(acquired <= 8, "admitted {acquired} > cap 8");
    assert_eq!(gate.current_listener_count() as usize, permits.len());
    drop(permits);
    assert_eq!(gate.current_listener_count(), 0);
}

#[test]
fn cap_accessors_return_config() {
    let gate = ConnGate::new(100, 5, Vec::new());
    assert_eq!(gate.listener_cap(), 100);
    assert_eq!(gate.per_ip_cap(), 5);
    assert!(gate.trusted_cidrs().is_empty());
}
