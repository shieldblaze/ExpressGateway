//! Byte-equality differential of the hand-rolled passthrough Retry writer
//! (`build_retry_packet`) against `quiche::retry`, per RFC 9001 §5.8.
//!
//! quiche's argument names are counterintuitive: its `scid` argument becomes the
//! ON-WIRE DCID, its `dcid` argument is the ODCID used only in the Retry
//! Pseudo-Packet for the integrity tag, and `new_scid` is the on-wire SCID. Our
//! `build_retry_packet(odcid, client_scid, new_scid, …)` therefore maps
//! `odcid → dcid`, `client_scid → scid`, `new_scid → new_scid`.
//!
//! quiche only accepts QUIC v1, so the proptest constrains `version` to it.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use lb_quic::passthrough::_test_build_retry_packet;
use proptest::collection::vec as pvec;
use proptest::prelude::*;
use quiche::ConnectionId;

/// Re-derive QUIC v1 wire constant locally rather than depending on a
/// private quiche export.
const QUIC_V1: u32 = 0x0000_0001;

/// On-wire LB-chosen SCID length (matches passthrough's `LB_SCID_LEN`).
const LB_SCID_LEN: usize = 16;

fn run_diff(odcid: &[u8], client_scid: &[u8], new_scid: &[u8; 16], token: &[u8]) {
    // Our writer.
    let mut ours = Vec::with_capacity(1024);
    _test_build_retry_packet(odcid, client_scid, new_scid, QUIC_V1, token, &mut ours)
        .expect("our build_retry_packet should succeed for valid inputs");

    // quiche's writer.
    let mut theirs = vec![0u8; 1024];
    let scid_cid = ConnectionId::from_ref(client_scid);
    let dcid_cid = ConnectionId::from_ref(odcid);
    let new_scid_cid = ConnectionId::from_ref(&new_scid[..]);
    let n = quiche::retry(
        &scid_cid,
        &dcid_cid,
        &new_scid_cid,
        token,
        QUIC_V1,
        &mut theirs,
    )
    .expect("quiche::retry should succeed");
    theirs.truncate(n);

    assert_eq!(
        ours.len(),
        theirs.len(),
        "Retry byte-length mismatch: ours={} theirs={}",
        ours.len(),
        theirs.len()
    );
    assert_eq!(
        ours, theirs,
        "Retry byte-equality mismatch\nours:   {ours:02x?}\ntheirs: {theirs:02x?}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 1000,
        // Per design §A2 — 1000-case budget. Override via PROPTEST_CASES
        // in CI for deeper sampling.
        .. ProptestConfig::default()
    })]

    /// Differential against `quiche::retry`: every valid input shape must
    /// produce byte-identical wire packets. `client_scid` (the on-wire DCID) and
    /// `odcid` are 0..=20 bytes (RFC 9000 §17.2 / §17.2.5), `new_scid` is
    /// exactly the 16 bytes the LB chooses, the token spans the realistic
    /// signer range, and the version is pinned to v1.
    #[test]
    fn quiche_retry_byte_equality(
        odcid in pvec(any::<u8>(), 0..=20),
        client_scid in pvec(any::<u8>(), 0..=20),
        new_scid_v in pvec(any::<u8>(), LB_SCID_LEN..=LB_SCID_LEN),
        token in pvec(any::<u8>(), 0..=512),
    ) {
        let mut new_scid = [0u8; LB_SCID_LEN];
        new_scid.copy_from_slice(&new_scid_v);
        run_diff(&odcid, &client_scid, &new_scid, &token);
    }
}

#[test]
fn quiche_retry_byte_equality_smoke() {
    // Quick sanity case independent of the proptest, exercises the
    // exact RFC 9001 §A.4 worked example shape (ODCID from the
    // appendix; trivial token / new_scid).
    let odcid = [0x83, 0x94, 0xc8, 0xf0, 0x3e, 0x51, 0x57, 0x08];
    let client_scid: [u8; 0] = [];
    let new_scid = [0xaau8; LB_SCID_LEN];
    let token = b"opaque-retry-token";
    run_diff(&odcid, &client_scid, &new_scid, token);
}
