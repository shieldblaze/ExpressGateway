# Foundation Audit — Auditor-2 Findings (Surface: H2)

Surface: H2 — framing, HPACK, flow control, GOAWAY, rapid-reset, h2spec
conformance. Files: crates/lb-h2, crates/lb-l7 H2 paths (h2_proxy.rs,
h2_security.rs, h2_to_h1.rs, h2_to_h2.rs), tests/h2spec.rs,
tests/h2_security_live.rs.

Phase 1 = MEASURE ONLY. No source edited. Base = feature/h3-quic-s3 @
fdb1ef9f. Box c6a.2xlarge, 8 cores.

Repro method (per pre-existing-h2-defects.md guidance): isolated build
of the carry-in test binaries, then run them as N parallel processes
(each its own multi-thread tokio runtime) + 24-32 CPU spin workers + a
dd/sync disk-churn loop. This reproduces the `cargo test --all-features`
environment (sibling test binaries + linker) = concurrent-multi-runtime
scheduler contention, the real mechanism. Pure single-process CPU spin
does NOT reproduce (6 isolated runs under 16-worker spin all passed);
failures require many contending tokio runtimes and cluster in the
slowest, most-starved process of each batch.

Result: the 2 documented carry-ins are CONFIRMED with mechanism proven;
the h2spec carry-in is the visible face of a LARGER single defect with
>=5 distinct h2spec manifestations (task expectation "expect MORE than
2" met). Three findings total.

---

## FINDING A2-1 — Rapid-Reset mitigation drops the RFC 9113 §6.8 GOAWAY signal under scheduler load (CVE-2023-44487-adjacent)

R6 tier: SECURITY -> MUST fix this session + mandatory regression test.
(Phase 1 measure-only: documented + escalated for fix phase.)

### Proven mechanism
Rapid-reset mitigation is delegated entirely to hyper/h2 via
H2SecurityThresholds::apply() (crates/lb-l7/src/h2_security.rs:111-115:
.max_pending_accept_reset_streams / .max_local_error_reset_streams). The
gateway's own GOAWAY path (h2_proxy.rs:494-523, cancel_fut ->
conn.graceful_shutdown()) is driven ONLY by SIGTERM or the
GlitchConnState abuse counter — rapid-reset never trips that path. When
the threshold is exceeded hyper/h2 internally queues a
GOAWAY(PROTOCOL_ERROR) and resolves the conn future Err. The gateway's
`res = &mut conn` arm (h2_proxy.rs:515-518) returns immediately and
drops the conn future + its TLS/TCP io. The queued GOAWAY sits in
hyper/h2's write buffer; the gateway does not await its flush. Under
concurrent-runtime scheduler starvation the TCP FIN/RST reaches the
client before its read task is polled to decode the buffered GOAWAY, so
the client's next write yields BrokenPipe and it never sees the
protocol signal.

Answer to the doc's open question = (B) real mitigation-correctness
defect, NOT test-too-strict. Proof: in every unstarved run the server
itself delivers GoAway(b"", PROTOCOL_ERROR, Remote) — the exact shape
the test asserts; the test is not stricter than the server's own
intended contract. DoS is still mitigated (connection dies) but the
RFC 9113 §6.8 / CVE-2023-44487 signalling guarantee (tell the peer it
was the abuser so it does not treat the close as transient and
reconnect-retry, re-amplifying the flood) is lost nondeterministically.

### Verbatim evidence
Isolated, no load — PASSES (x3):
  rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: GoAway(b"", PROTOCOL_ERROR, Remote) })))
  test result: ok. 1 passed; 0 failed; ... finished in 0.10s

Concurrent-runtime load (8 parallel h2_security_live procs,
--test-threads=1, +24 spin +disk churn), round 1 = FAILS, identical to
the documented carry-in:
  [r1 p1] test rapid_reset_goaway ... FAILED
  [r1 p1] rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
  [r1 p1] thread 'rapid_reset_goaway' panicked at tests/h2_security_live.rs:342:5:
  [r1 p1] test result: FAILED. 5 passed; 1 failed; ... finished in 1.42s
  [r1 p6] rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
6/48 runs failed. The two failing round-1 procs finished slowest
(1.42s/1.40s vs passing peers 0.93-1.29s) -> failure correlates with
maximal scheduler starvation. --test-threads=1 proves cross-PROCESS
contention, not in-process test cross-talk. Wrong CLOSE FORM (transport
RST/FIN instead of protocol GOAWAY) = REAL DEFECT per R2.

### Provenance
git log feature/h3-quic-s2..feature/h3-quic-s3 -- tests/h2_security_live.rs
= EMPTY; last touched by main base ac58f613. h2_proxy.rs &
h2_security.rs S2..S3 logs both EMPTY. Pre-S1 / pre-program. Confirmed
exactly as pre-existing-h2-defects.md states. NOT an S3 commit.

### Proposed fix (fix phase)
In the h2_proxy.rs serve loop, when `res = &mut conn` resolves with a
locally-initiated GOAWAY (rapid-reset/local-error-reset trip), do NOT
drop io immediately: drive a bounded GOAWAY flush (keep polling conn or
flush io for a short bounded budget) so the queued GOAWAY is written
before teardown — mirror the cancel_fut arm's
tokio::time::timeout(total, conn) discipline; or route the trip through
the existing graceful_shutdown() path (which awaits flush). Regression
test: rapid-reset flood under induced scheduler contention asserting the
client observes is_go_away()/is_remote(), not Io(BrokenPipe). Author !=
verifier.

---

## FINDING A2-2 — H2 request-validation vs proxy-forward race: malformed requests that MUST yield a stream error instead leak the backend response (>=5 distinct h2spec cases)

R6 tier: CORRECTNESS/CONFORMANCE -> fix this session + regression test.
Tractable (single ordering bug, one fix). (Phase 1: documented +
escalated.)

### Proven mechanism
The documented carry-in (h2spec 8.1.2.1#3, pseudo-header in trailers) is
one face of a SINGLE defect. Under concurrent-runtime scheduler load the
gateway forwards the request to the backend and relays the static
backend's HTTP/200 body (DATA Frame length:2 = the literal "ok" 2-byte
body) BEFORE hyper/h2 completes protocol-layer validation of the
malformed request. Whichever h2spec sub-test wins the
validation-vs-forward race in a given run fails — exactly ONE per run
(146 tests, 144 passed, 1 skipped, 1 failed), nondeterministic which.
Distinct manifestations captured verbatim:
- 8.1.2.1#3 pseudo-header field as trailers -> MUST PROTOCOL_ERROR
  (documented carry-in; RFC 9113 §8.1).
- 5.1#9 closed: HEADERS after RST_STREAM -> MUST STREAM_CLOSED (§5.1).
- 8.1.2.6#2 content-length != sum DATA lengths -> MUST PROTOCOL_ERROR.
- 8.1#1 second HEADERS without END_STREAM -> MUST PROTOCOL_ERROR.
- 6.9.1#3 WINDOW_UPDATE pushing stream window >2^31-1 -> MUST RST_STREAM
  FLOW_CONTROL_ERROR; server sends RST_STREAM with unspecified code
  (degraded signal, same class).

Root-cause locus: H2 server is hyper's
(hyper::server::conn::http2::serve_connection, h2_proxy.rs:478-485).
Stream-state/trailer/malformed enforcement is hyper/h2's job BEFORE
ProxyService::call (h2_proxy.rs:583). The DATA-leak proves the gateway's
proxy-forward reached the backend and streamed the response back
before/instead of the protocol rejection landing. Trailer-capture
compounds 8.1.2.1#3: h2_proxy.rs:1168-1179 and :1386-1397 build
trailers_vec with NO starts_with(':') filter (contrast regular-header
path h2_to_h2.rs:19); no bridge rejects pseudo-in-trailers anywhere
(grep across crates/ = empty). Cross-protocol: H3 trailer decode
crates/lb-quic/src/h3_bridge.rs:382-388 pushes BodyItem::Trailers with
the same missing RFC 9114 §4.3 enforcement — same defect on H3 (S2
PROTO-2-12 path).

### Verbatim evidence
Isolated, no load — h2spec PASSES, incl 8.1.2.1#3 explicitly:
  3: Sends a HEADERS frame that contains a pseudo-header field as trailers   v 3: ...
  h2spec passed (21853 bytes stdout)

Concurrent-runtime load — distinct failing cases captured:
  5.1 x 9: closed: Sends a HEADERS frame after sending RST_STREAM frame
    -> MUST STREAM_CLOSED ; Actual: DATA Frame (length:2, flags:0x01, stream_id:1)
  8.1.2.6 x 2: content-length != sum of multiple DATA frames payload length
    -> MUST PROTOCOL_ERROR ; Actual: DATA Frame (length:2, flags:0x01, stream_id:1)
  8.1 x 1: Sends a second HEADERS frame without the END_STREAM flag
    -> MUST PROTOCOL_ERROR ; Actual: DATA Frame (length:2, flags:0x01, stream_id:1)
  6.9.1 x 3: WINDOW_UPDATE increasing flow control window above 2^31-1 on a stream
    -> MUST RST_STREAM(FLOW_CONTROL_ERROR) ; Actual: RST_STREAM (length:4, flags:0x00, stream_id:1)
8.1.2.1#3 itself also reproduced under load (8.1.2.1. Pseudo-Header
Fields ... Actual: DATA Frame (length:2, flags:0x01, stream_id:1)),
matching the documented carry-in verbatim. Wrong frame / wrong RST code
returned instead of the mandated stream error = REAL DEFECT per R2.

### Provenance
git log feature/h3-quic-s2..feature/h3-quic-s3 -- tests/h2spec.rs =
EMPTY; last touched pre-S1 450b6e80 (2026-05-16). h2_proxy.rs S2..S3
log EMPTY. Pre-S1 / pre-program. Confirmed exactly as
pre-existing-h2-defects.md states. The H3 h3_bridge.rs trailer gap is
the S2 PROTO-2-12 path — same missing rule, same fix workstream.

### Proposed fix (fix phase)
Single ordering fix: fully protocol-validate the H2 (and H3) request
before forwarding upstream / relaying any backend response. Add explicit
pre-forward rejection for (a) pseudo-header in the trailing field
section -> PROTOCOL_ERROR (validate at trailer-capture sites
h2_proxy.rs:1168/:1386 AND h3_bridge.rs:382), (b) HEADERS on
closed/reset stream -> STREAM_CLOSED, (c) content-length vs DATA
mismatch -> PROTOCOL_ERROR, (d) second non-END_STREAM HEADERS ->
PROTOCOL_ERROR, (e) stream-window overflow -> RST_STREAM
FLOW_CONTROL_ERROR. Investigate whether the gateway races hyper/h2's own
enforcement and gate the forward on hyper request completeness.
Regression test: h2spec under induced scheduler contention asserting 0
failures + targeted unit tests per RFC clause. Author != verifier.

---

## OBSERVATION A2-OBS — tests/h2spec_server_conformance.rs carries a #[ignore]
h2spec_server_conformance_passes is #[ignore = "h2spec binary not
provisioned until Wave-2c CI image; see audit/deferred.md
PROTO-2-04/05"] (tests/h2spec_server_conformance.rs:29). NOT a new
finding: documented pre-existing deferral (provenance pre-S1, commit
de5a93c8, not in S2..S3), a superseded skeleton — live h2spec coverage
is tests/h2spec.rs which DOES run and IS the source of A2-2. Recorded
for completeness.

---

## Re-verification of S1/S2/S3 + pre-program H2 "verified" claims (--all-features, isolated)

| Target | Result |
|---|---|
| cargo test -p lb-h2 --all-features (unit) | 41/41 pass |
| tests/h2_proxy_e2e | 3 pass |
| tests/codec_roundtrip_h2 | 5 pass |
| tests/bridging_h2_h1 / h2_h2 / h2_h3 / h3_h2 / h1_h2 | 1 pass each |
| tests/h2spec_server_conformance | 1 pass, 1 ignored (see OBS) |
| tests/security_smuggling_h2_downgrade | 1 pass |
| cargo clippy -p lb-h2 -p lb-l7 --all-targets --all-features | clean |
| cargo fmt --check (H2 files) | clean |
| tests/h2_security_live (isolated) | 6 pass |
| tests/h2spec (isolated) | pass (146: 144 pass / 1 skip) |

The two carry-ins are the ONLY H2 targets that regress, and ONLY under
concurrent-runtime scheduler load — confirming the S1/S2 verification
gap (never run under the full --all-features concurrent suite, so these
load-sensitive defects were never surfaced). No additional
isolated-pass H2 target found false-green; the two known targets expand
to the 3 findings above.
