//! SEC-2-11 proof: the XDP capability probe must fall back from
//! `CAP_BPF` to `CAP_SYS_ADMIN` on pre-5.8 kernels.
//!
//! The production probe consults [`caps::has_cap`] which talks to
//! `capget(2)` and therefore depends on the calling task's real
//! capabilities. CI runners do not have `CAP_BPF` (or
//! `CAP_SYS_ADMIN`), so a test that called the real probe would
//! always observe `MissingBpfAndSysAdmin` and could not exercise the
//! fallback policy.
//!
//! Instead we exercise [`lb::xdp::cap_probe::probe_caps_with`], which
//! takes a closure standing in for the kernel's cap-table. The
//! closure is what tests mock; the policy under test (BPF →
//! NET_ADMIN, fallback to SYS_ADMIN, log the chosen mode) lives
//! between the closure and the [`CapState`] return.

#![cfg(target_os = "linux")]

use caps::Capability;
use lb::xdp::cap_probe::{CapMode, CapState, probe_caps_with};

/// Build a closure that returns `Ok(true)` for any cap whose name is
/// listed in `held` and `Ok(false)` otherwise. Mimics a kernel with a
/// known capability set.
fn fake_caps(held: &'static [Capability]) -> impl FnMut(Capability) -> Result<bool, String> {
    move |cap| Ok(held.contains(&cap))
}

/// SEC-2-11: modern path — CAP_BPF + CAP_NET_ADMIN.
#[test]
fn test_cap_bpf_path() {
    let state = probe_caps_with(fake_caps(&[Capability::CAP_BPF, Capability::CAP_NET_ADMIN]));
    assert!(
        matches!(state, CapState::Ok(CapMode::BpfPlusNetAdmin)),
        "expected CapState::Ok(BpfPlusNetAdmin), got {state:?}",
    );
}

/// SEC-2-11: fallback path — pre-5.8 kernel that has CAP_SYS_ADMIN
/// only. `CAP_BPF` returns `Ok(false)` (the bit is unset because the
/// kernel doesn't know that cap).
#[test]
fn test_cap_sys_admin_fallback() {
    let state = probe_caps_with(fake_caps(&[Capability::CAP_SYS_ADMIN]));
    assert!(
        matches!(state, CapState::Ok(CapMode::SysAdmin)),
        "pre-5.8 kernel with CAP_SYS_ADMIN must accept the legacy path; got {state:?}",
    );
}

/// Neither cap held: the probe must reject with a clear "both
/// missing" state, not a one-sided error.
#[test]
fn test_no_caps_rejects() {
    let state = probe_caps_with(fake_caps(&[]));
    assert!(
        matches!(state, CapState::MissingBpfAndSysAdmin),
        "expected MissingBpfAndSysAdmin, got {state:?}",
    );
}

/// CAP_BPF held but CAP_NET_ADMIN missing, AND CAP_SYS_ADMIN missing:
/// must surface the "almost there" diagnostic. This is the case
/// where the operator granted CAP_BPF but forgot CAP_NET_ADMIN.
#[test]
fn test_bpf_without_net_admin_rejects() {
    let state = probe_caps_with(fake_caps(&[Capability::CAP_BPF]));
    assert!(
        matches!(state, CapState::MissingNetAdmin),
        "expected MissingNetAdmin, got {state:?}",
    );
}

/// CAP_BPF held but CAP_NET_ADMIN missing — however CAP_SYS_ADMIN is
/// held. The probe should accept the legacy path because
/// CAP_SYS_ADMIN implies CAP_NET_ADMIN authority for the BPF attach.
#[test]
fn test_bpf_then_sys_admin_compensates_for_missing_net_admin() {
    let state = probe_caps_with(fake_caps(&[Capability::CAP_BPF, Capability::CAP_SYS_ADMIN]));
    assert!(
        matches!(state, CapState::Ok(CapMode::SysAdmin)),
        "CAP_SYS_ADMIN must compensate for a missing CAP_NET_ADMIN; got {state:?}",
    );
}

/// Probe error on the CAP_BPF check alone must NOT short-circuit:
/// we fall through to CAP_SYS_ADMIN. This is exactly the pre-5.8
/// scenario where the `caps` crate might surface an obscure errno;
/// the operator-facing posture is "try the fallback, report what
/// happens there".
#[test]
fn test_cap_bpf_probe_error_falls_through_to_sys_admin() {
    let mut calls: Vec<Capability> = Vec::new();
    let state = probe_caps_with(|cap| {
        calls.push(cap);
        match cap {
            Capability::CAP_BPF => Err("capget: EINVAL".to_string()),
            Capability::CAP_SYS_ADMIN => Ok(true),
            _ => Ok(false),
        }
    });
    assert!(
        matches!(state, CapState::Ok(CapMode::SysAdmin)),
        "CAP_BPF probe error must fall through to CAP_SYS_ADMIN; got {state:?}",
    );
    assert!(calls.contains(&Capability::CAP_BPF));
    assert!(calls.contains(&Capability::CAP_SYS_ADMIN));
}

/// Probe error on BOTH CAP_BPF and CAP_SYS_ADMIN must surface a
/// composite ProbeError that mentions both — operators need both
/// strings to diagnose the kernel state.
#[test]
fn test_double_probe_error_composes_message() {
    let state = probe_caps_with(|cap| match cap {
        Capability::CAP_BPF => Err("bpf-err".to_string()),
        Capability::CAP_SYS_ADMIN => Err("sysadmin-err".to_string()),
        _ => Ok(false),
    });
    match state {
        CapState::ProbeError(msg) => {
            assert!(
                msg.contains("bpf-err") && msg.contains("sysadmin-err"),
                "composite message must mention both errnos, got: {msg}",
            );
        }
        other => panic!("expected ProbeError(composite), got {other:?}"),
    }
}
