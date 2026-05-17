//! Proof for the per-IP / per-listener connection gate (SEC-2-04).
//!
//! Companion to the `hooks_impl.rs` end-to-end coverage. These tests
//! drive `ConnGate` directly to verify the cap accounting, the
//! RAII permit drop semantics, and the trusted-CIDR field
//! (deferred per L-002 — the field exists, the match doesn't yet).

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

// ---- listener cap ----

#[test]
fn test_listener_cap_enforced() {
    let gate = ConnGate::new(3, 100, Vec::new());
    let p1 = gate.admit(loopback_v4()).unwrap();
    let p2 = gate.admit(loopback_v4()).unwrap();
    let p3 = gate.admit(loopback_v4()).unwrap();
    // Fourth admit must trip the listener cap (per-IP cap is 100, far
    // from saturation — so the failure mode must be Listener, not
    // PerIp).
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
    // Slot freed — the next admit must succeed.
    let _p3 = gate.admit(loopback_v4()).unwrap();
}

// ---- per-IP cap ----

#[test]
fn test_per_ip_cap_drops_on_drop() {
    let gate = ConnGate::new(100, 2, Vec::new());
    let p1 = gate.admit(loopback_v4()).unwrap();
    let p2 = gate.admit(loopback_v4()).unwrap();
    assert_eq!(gate.current_peer_count(loopback_v4()), 2);
    // Third admit from the same IP trips per-IP cap.
    let err = gate.admit(loopback_v4()).unwrap_err();
    match err {
        OverCap::PerIp { addr, count } => {
            assert_eq!(addr, loopback_v4());
            assert_eq!(count, 2);
        }
        other => panic!("expected PerIp, got {other:?}"),
    }
    drop(p1);
    // One slot freed on the same IP — next admit succeeds.
    let _p3 = gate.admit(loopback_v4()).unwrap();
    drop(p2);
}

#[test]
fn per_ip_full_does_not_consume_listener_slot() {
    // Critical regression guard for the rollback path in
    // ConnGate::admit. If the listener counter were not rolled back
    // on per-IP overflow, a sustained over-cap stream from a single
    // attacker would silently erode the listener cap.
    let gate = ConnGate::new(100, 1, Vec::new());
    let _p1 = gate.admit(loopback_v4()).unwrap();
    assert_eq!(gate.current_listener_count(), 1);
    // Try a per-IP overflow 50 times.
    for _ in 0..50 {
        assert!(gate.admit(loopback_v4()).is_err());
    }
    // Listener counter must STILL be 1 — every per-IP rejection
    // rolled the listener counter back.
    assert_eq!(gate.current_listener_count(), 1);
}

#[test]
fn per_ip_cap_independent_across_ips() {
    let gate = ConnGate::new(10, 1, Vec::new());
    let _p_v4 = gate.admit(loopback_v4()).unwrap();
    // Same listener, different IP — per-IP cap independent.
    let _p_v6 = gate.admit(loopback_v6()).unwrap();
    // Different v4 address.
    let other_v4: IpAddr = Ipv4Addr::new(192, 0, 2, 1).into();
    let _p_other = gate.admit(other_v4).unwrap();
    // Second from any of the same IPs trips.
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

// ---- trusted CIDR field (L-002 deferred match) ----

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
    // L-002 documents the deferred behaviour: the trusted_cidrs
    // field is part of the API but does not yet exempt the peer
    // from the per-IP cap. This test pins that current behaviour so
    // that when L-002 lands the test will fail loudly and force a
    // matching update.
    let cidrs = vec![IpNet::new(loopback_v4(), 32)];
    let gate = ConnGate::new(10, 1, cidrs);
    let _p = gate.admit(loopback_v4()).unwrap();
    // Without exemption, the second admit must still trip per-IP.
    assert!(matches!(
        gate.admit(loopback_v4()).unwrap_err(),
        OverCap::PerIp { .. }
    ));
}

// ---- concurrency ----

#[test]
fn concurrent_admits_observe_cap() {
    // Spawn N threads each trying to acquire a permit; assert the
    // total admitted permits never exceeds the cap.
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

// ---- accessors ----

#[test]
fn cap_accessors_return_config() {
    let gate = ConnGate::new(100, 5, Vec::new());
    assert_eq!(gate.listener_cap(), 100);
    assert_eq!(gate.per_ip_cap(), 5);
    assert!(gate.trusted_cidrs().is_empty());
}
