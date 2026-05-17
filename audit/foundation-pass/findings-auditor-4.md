# Foundation Audit — Auditor-4 Findings (H3 surface, Phase 1 MEASURE-ONLY)

Surface: ALREADY-BUILT H3 only — QUIC listener (crates/lb-quic listener.rs /
router.rs / conn_actor.rs), H3->H1 bridge (crates/lb-quic/src/h3_bridge.rs),
H3 request-body streaming (S1/S2/S3 work), crates/lb-h3. Out of scope (not
tested, not filed): H3 response-body egress (known buffered S2 carry / S4
headline), H3 cross-paths H3->H2/H3->H3, native QUIC proxy, WS/gRPC-over-H3.

Branch audit/foundation-pass @ fdb1ef9f. 67G disk, 8 cores. All runs
--all-features, --test-threads=8, exactly as the H1/H2 auditors run theirs.

---

## Re-verification ledger — S1/S2/S3 H3 "verified" claims

RECONFIRMED under --all-features (3/3 deterministic):

| Claim (source) | Test(s) re-run | Result |
|---|---|---|
| S1 B.1 QUIC listener crate-local coverage | listener_lifecycle (6), router_accept_path (3) | 3/3 PASS |
| S1 B.2 H3->H1 bridge e2e (status/body/Host verbatim) | h3_h1_bridge_e2e (1), quic_listener_e2e (6) | 3/3 PASS |
| S1 ALPN h3/h3-29 accept, unknown reject | quic_alpn_h3 (4), listener_lifecycle alpn_* | 3/3 PASS |
| S1 round8 authority enforcement | round8_h3_authority_enforced (3) | 3/3 PASS |
| S1 0-RTT replay guard shared-instance | router_accept_path zero_rtt_replay_guard_* | 3/3 PASS |
| S1 RETRY round-trip through router | router_accept_path retry_token_round_trip | 3/3 PASS |
| S2 P0 raw-bytes body seam (non-UTF-8, no CL desync) | h3_to_h1_forwards_non_utf8_body_byte_for_byte | 3/3 PASS |
| S2 P1-A incremental request-body + T5 memory bound | h3_h1_stream_body_e2e t1-t5 (6) | 3/3 PASS |
| S2 P1-B body error paths (reset/502/413) | h3_h1_stream_body_errors_e2e p1b_t1-t3 (3) | 3/3 PASS |
| S2 P1-C trailers no-regression + large binary resp | h3_h1_trailers_resp_e2e pc1/pc2 (2) | 3/3 PASS |
| S2 P1-C MAX_RESPONSE_BODY_BYTES OOM cap | read_h1_response_capped_rejects_over_cap | 3/3 PASS |
| S3 H1 in-flight no-drop drain proof (jitter ON) | h3_s3_inflight_h1_drain_proof (3) | 3/3 PASS |
| S3/S1 graceful H3 close on cancel | h3_graceful_close (1) | 3/3 PASS |
| S1 H3 upstream cert verify | h3_upstream_verify (8) | 3/3 PASS |

NO S1/S2/S3 H3 "verified" claim FAILED under --all-features on the built H3
surface. lb-quic full suite 3/3 RC=0; lb-h3 3/3 RC=0; in-scope repo-root H3
binaries 3/3 RC=0. Zero H3/quic FAILED/panicked in any lead baseline log
(baseline/test-run-*.log). fmt clean; clippy --all-targets --all-features
-D warnings clean on lb-quic + lb-h3. Two claims hold behaviourally but
ship with a test-integrity/documentation defect (F-1, F-2), filed per R3/R4.

---

## F-1 — lb-h3 QPACK property tests silently skipped under default features (dead `#![cfg(feature="proptest")]` gate); the EXACT anti-pattern S1 removed from sibling lb-quic but left in lb-h3

R6 tier: CORRECTNESS / coverage-integrity, TRACTABLE. Not SECURITY. R4 not asterisked.

### Proven mechanism (verbatim)

crates/lb-h3/tests/proptest_qpack.rs:15 is `#![cfg(feature = "proptest")]`.
crates/lb-h3/Cargo.toml declares proptest as an UNCONDITIONAL dev-dep
(line 17 `proptest = { workspace = true }`) plus a SEPARATE empty marker
feature `proptest = []` (line 21) gating no code. The property tests
compile-gate themselves out whenever the marker feature is absent:

    === lb-h3 DEFAULT features ===
         Running tests/proptest_qpack.rs (.../proptest_qpack-b60aafd390802c3e)
    running 0 tests
    test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

    === lb-h3 --all-features ===
         Running tests/proptest_qpack.rs (.../proptest_qpack-166970c1e9499cf4)
    running 4 tests
    test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

`cargo test -p lb-h3` (default — the exact command s1-inventory.md states
it used) silently reports `running 0 tests` for the QPACK CODE-2-11
property suite: a green "ok" asserting nothing. This is byte-for-byte the
dead-gate S1 (s1-report.md:47-49) DELETED from
crates/lb-quic/tests/proptest_header.rs and s1-inventory.md:31 flagged as
coverage-zeroing — never propagated to the sibling lb-h3 file.

### Why a real defect (R2)

R1 gate runs --all-features so the 4 tests DO run in the gate (4 passed) —
NOT a gate-passing hole. It IS a coverage-integrity defect: every per-crate
/ default-feature invocation (dev loop, s1-inventory's own stated method,
any non---all-features CI) silently skips QPACK fuzz with a misleading
`test result: ok`; and it is an inconsistent half-applied S1 fix — the
program already ruled this pattern a defect and removed it next door
(known-defect-not-carried, in scope per R3).

### Provenance

proptest_qpack.rs (CODE-2-11) predates S1/S2/S3; the `#![cfg]` is original.
S1 fixed the twin in lb-quic (25d8ad84) with the rationale ("proptest is an
unconditional dev-dependency") that applies verbatim here (Cargo.toml:17
unconditional dev-dep, :21 empty feature). Fix not propagated to sibling.

### Proposed fix (Phase 2, plan-approval per R5)

Delete line 15 from proptest_qpack.rs (mirror the S1 lb-quic change; case
logic byte-identical; keep PROPTEST_CASES sanity budget). Regression: assert
`cargo test -p lb-h3` (default) reports the 4 tests running (non-zero).
Author != verifier; re-confirm default run shows `4 ...`.

---

## F-2 — S2 exit criterion NOT met: `s2_target_build_h1_request_with_body_*` still `#[ignore]`d with a now-FALSE "UNBUILT" doc-comment, masking a test that PASSES against shipped S2 code

R6 tier: CORRECTNESS / test-integrity + stale-false-documentation,
TRACTABLE. Not SECURITY. R4 not asterisked.

### Proven mechanism (verbatim + code)

crates/lb-quic/src/h3_bridge.rs:1656-1677:

    #[test]
    #[ignore = "S2: request-body forwarding"]
    fn s2_target_build_h1_request_with_body_sets_content_length_and_appends_payload() {
        ...
        let body = b"hello-s2-body";
        let got = build_h1_request(&req, Some(body));
        let expected = format!(
            "POST /submit HTTP/1.1\r\nHost: api.test\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\nhello-s2-body",
            body.len());
        assert_eq!(got, expected.as_bytes());
    }

doc-comment (h3_bridge.rs:1650-1654) still asserts: "It is `#[ignore]`
ONLY because SESSION 2's datapath wiring (conn_actor::poll_h3 accumulating
inbound H3 DATA frames and passing Some(..) here) is UNBUILT ... (no caller
passes Some yet)."

Shipped S2 build_h1_request (h3_bridge.rs:583-615) appends raw body via
`out.extend_from_slice(bytes)` with `Content-Length: {body_len}` +
`Connection: close` — for `Some(b"hello-s2-body")` it produces EXACTLY the
asserted bytes, so the marker test PASSES if un-ignored. The doc-comment is
FALSE: S2 P1-A built DATA-frame accumulation in poll_h3 and the streaming
path forwards the body, proven green 3/3 this session by:

    test t1_multi_data_frame_binary_body_forwarded_byte_identical ... ok
    test t5_single_large_data_frame_is_memory_bounded_through_stalled_upstream ... ok
    test h3_to_h1_forwards_non_utf8_body_byte_for_byte ... ok

Live skip (identical 3/3):

    test h3_bridge::tests::s2_target_build_h1_request_with_body_sets_content_length_and_appends_payload ... ignored, S2: request-body forwarding
    test result: ok. 9 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out

### Why a real defect (R2/R5)

S2's report sets the explicit exit criterion ("Drop the #[ignore] on
s2_target_build_h1_request_with_body_..."); S1-report.md:66 promised it
would be dropped when S2 lands DATA-frame accumulation. S2 P1-A DID land it
(e2e-proven) but the #[ignore] was NOT dropped and the "UNBUILT / no caller
passes Some" comment NOT corrected — now stale and false. This is the
stale-false-comment class that derailed S3 Phase-0 triage
(s3-report.md:84-88, 371-378). It does NOT break the R1 gate (e2e proves
the behaviour) so it is a test-integrity/false-documentation finding, real
per R2 (never-run "verified" assertion behind a false "unbuilt"
justification), in scope per R3. Fix is a source edit -> Phase 2 per R5.

### Provenance

#[ignore] introduced S1 B.2 (162e5a59) as a legitimate unbuilt-target
marker. S2 P1-A (f2af73c4) built the gated datapath but did not retire the
marker/comment — same S2 verification-gap family s3-report.md:364-381 flagged.

### Proposed fix (Phase 2, plan-approval per R5)

Drop `#[ignore = "S2: request-body forwarding"]` (h3_bridge.rs:1657);
rewrite doc-comment (:1645-1655) to state datapath is BUILT (cite e2e
proofs). Assertion byte-identical (already passes — confirmed by reading
build_h1_request). Verify `cargo test -p lb-quic --all-features` shows the
test `... ok` and ignored count drops to 0. Author != verifier.

---

## Shared-state / parallel-flake audit (R2, task #2) — NEGATIVE, one LATENT note

Task #2 required proving no remaining shared-state collisions of the round8
CERT_SEQ / S3 shared-temp-dir family on the H3 surface. Result: NO live
collision; mitigations correctly applied.

- Per-call atomic-counter temp dirs (correct pattern): listener_lifecycle.rs
  :41/:65 DIR_COUNTER, quic_listener_e2e.rs:70/:91, quic_alpn_h3.rs:39/:64,
  quic_native.rs:31/:51 CERT_DIR_COUNTER, h3_h1_bridge_e2e.rs:60,
  h3_h1_stream_body_e2e.rs:61, h3_h1_stream_body_errors_e2e.rs:61,
  h3_h1_trailers_resp_e2e.rs:59, router_accept_path.rs:31. All UNIQUE
  per call — no parallel collision possible.
- round8_h3_authority_enforced.rs:75-81 documented static CERT_SEQ + per-call
  seq suffix + distinct prefix (lb-quic-l7-16-...-{seq}). Mitigation intact.
- h3_upstream_verify.rs:151/:170 "127.0.0.1:3000" is a BackendConfig STRING
  for config-rejection tests — never bound (no bind/listen/TcpListener on a
  fixed port in that file). Not a collision.
- listener_lifecycle.rs:266 "127.0.0.1:40000" is a synthetic SocketAddr arg
  to RetryTokenSigner mint/verify — never a bind. All real binds use
  127.0.0.1:0 / (LOCALHOST,0). Not a collision.

LATENT note (NOT filed — no collision possible with the current test set,
so not an R2 REAL DEFECT today): single-test binary
crates/lb-quic/tests/h3_graceful_close.rs:71-80 builds its cert path as
lb-quic-proto-2-11-cert-{pid*K+subsec_nanos}.pem with NO atomic counter —
the exact pre-fix nonce scheme round8 documented as collision-prone
(round8_h3_authority_enforced.rs:66-79). Safe ONLY because (a) that binary
has exactly ONE #[test] (test_h3_connection_close_emitted_on_cancel,
h3_graceful_close.rs:172) and (b) its prefix is unique across the H3
surface. Becomes a real defect the instant a second #[test] is added.
Recommend (low-priority robustness; fold into the F-1/F-2 Phase-2
changeset if convenient): add the same static CERT_SEQ + -{seq} suffix its
sibling round8 file uses. Recorded so a future session does not reintroduce
the round8 race.

---

## Summary

- Every S1/S2/S3 H3 "verified" claim on the BUILT surface RECONFIRMED under
  --all-features, 3/3 deterministic — none failed.
- 2 findings, both CORRECTNESS-tier test-integrity/documentation (F-1 dead
  proptest gate in lb-h3, S1 fix not propagated; F-2 S2 exit criterion
  unmet, stale-false "UNBUILT" comment on a now-passing #[ignore]d test).
  Neither breaks R1; in scope per R3/R4; fixes deferred to Phase 2 per R5.
- Shared-state audit NEGATIVE — no live collision; round8/S3 mitigations
  in place. One LATENT robustness note on h3_graceful_close recorded.
- fmt clean; clippy --all-targets --all-features -D warnings clean on
  lb-quic + lb-h3; zero H3/quic failure in lead baseline logs.
