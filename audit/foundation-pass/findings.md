# FOUNDATION AUDIT — Consolidated Findings (Phase 1)

Branch audit/foundation-pass @ fdb1ef9f base. Box c6a.2xlarge 8c.
Per-auditor detail: findings-auditor-{1,2,3,4}.md. Standing rules:
STANDING-RULES.md. Baseline: baseline/RESULT.md.

## Phase 1 baseline (R1, pre-fix)
- `cargo fmt --check`: CLEAN.
- `cargo clippy --all-targets --all-features -- -D warnings`: CLEAN.
- `cargo test --workspace --all-features` ×3: run1 FAIL, run2 PASS,
  run3 PASS → **NOT baseline-green** (BL-1 is the sole blocker).
- D-1 native ENA XDP attach on ens5: **PASS** (xdpdrv, no SKB
  fallback, kernel cross-checked, clean detach, full state restore).

Carry-in note: the two carry-in H2 defects did NOT reproduce in the
3 baseline runs (they need concurrent-multi-runtime contention, not
plain spin) — reproduced deliberately by auditor-2. R3/R4: in scope.

---

## SECURITY tier (R6 → FIX this session, mandatory regression test)

### F-SEC-1 (A2-1) — Rapid-Reset GOAWAY lost under load
CVE-2023-44487-adjacent. The carry-in open question is ANSWERED WITH
PROOF: **(B) real mitigation-correctness defect, NOT test-too-strict.**
Mechanism: rapid-reset enforcement delegated to hyper/h2; on trip
hyper queues GOAWAY then resolves the conn future Err; the gateway's
`res = &mut conn` arm at `crates/lb-l7/src/h2_proxy.rs:515` drops `io`
immediately without awaiting the GOAWAY flush. Under starvation TCP
FIN/RST beats the buffered GOAWAY → client `Io(BrokenPipe)`.
Unstarved, the server emits `GoAway(PROTOCOL_ERROR, Remote)` — exactly
the shape `tests/h2_security_live.rs:342` asserts, so the test is NOT
over-strict. 6/48 load runs failed (slowest procs). Provenance:
pre-S1, `ac58f613`; `git log s2..s3` empty (R3 in-scope).
Fix: drive the h2 connection future / graceful_shutdown to flush the
queued GOAWAY before dropping `io`. Regression test must reproduce
the flood under induced contention and assert a real GOAWAY.

**[FIX NOTE v2 — builder-1, F-SEC-1 REOPENED]** Prior commit
`044db8da` (CleanCloseIo break-on-Pending drain) was INSUFFICIENT —
phase3-final/RESULT.md proved the real wire test
`tests/h2_security_live.rs::rapid_reset_goaway` still FAILED ~1/3
under full-workspace 8-core load with the original
`send_err=None / Io(BrokenPipe)` signature (no server GOAWAY at the
client). Refined ROOT CAUSE (proven from hyper-1.9.0 + h2-0.4.13
source): h2 DOES flush the GOAWAY before the FIN
(`State::Closing` → `codec.shutdown` = flush-then-poll_shutdown);
the real defect is the **abortive RST close** — the prior drain
stopped at the first `Poll::Pending` then FINed/dropped while the
abusive client was still streaming, so the kernel emitted an RST that
discarded the client's already-arrived GOAWAY (RFC 1122 §4.2.2.13).
The prior unit proxy false-verified because it only modelled
"finite data then clean EOF", never "peer keeps sending past close".
FIX v2: `CleanCloseIo::poll_shutdown` now does **FIN-first then a
bounded post-FIN drain** (a FIN never RSTs; only dropping with unread
data does) — send the FIN promptly (no teardown-latency regression,
keep-alive-stall close still prompt), then read+discard inbound until
the peer's reciprocal FIN (EOF), hard-bounded by the 256 KiB
`DRAIN_CAP` AND a 1 s `LINGER_DEADLINE`, before letting h2 drop the
io. Client now receives `…GOAWAY…FIN` on a cleanly-closed socket and
decodes the GOAWAY. GATE = the REAL wire test `rapid_reset_goaway`
green ≥15 consecutive UNDER full `cargo test --workspace
--all-features --test-threads=8` 8-core contention (the unit/
corroboration tests are kept as ADDITIONAL coverage, NOT the gate;
none weakened/deleted per R5; the rapid_reset assertion is unchanged).
Source diff: `crates/lb-l7/src/h2_proxy.rs` only. Commit follows once
the gate bar is captured.

---

## CORRECTNESS / CONFORMANCE tier (R6 → FIX this session + reg test)

### F-COR-1 (A2-2) — H2 validation-vs-forward race + missing
no-pseudo-header-in-trailers rule (cross-protocol H2 *and* H3)
The carry-in h2spec 8.1.2.1#3 is ONE FACE of a single larger defect:
under load the gateway relays the backend 200 `DATA(2)` body before
hyper/h2 finishes protocol validation. Reproduced verbatim across
FIVE distinct h2spec cases (8.1.2.1#3, 5.1#9, 8.1.2.6#2, 8.1#1,
6.9.1#3), exactly one fails per run, nondeterministic which.
Additionally confirmed: the no-pseudo-in-trailers rule (RFC 9113
§8.1 / RFC 9114 §4.3) is absent from BOTH H2 trailer-capture
(`h2_proxy.rs:1168`/`:1386`) and H3 (`h3_bridge.rs:382-388`) — no
rejection anywhere in crates/. Provenance pre-S1 `450b6e80`.
Tractable: one ordering fix (forward backend body only after protocol
validation) + add the trailer pseudo-header rejection on both
surfaces. Regression test reproduces the h2spec cases under contention.

**[FIX NOTE — builder-1, directive D1, commit follows]** F-COR-1
fixed via the buffer-then-forward approach (per D1, the consistent
in-scope fix; the streaming-preserving variant is recorded as optional
future work only, NOT an asterisk — R4). BEHAVIORAL CHANGE: the H2→H1
request direction (`H2Proxy::proxy_request`) is now BUFFERED, not
streamed — the inbound H2 request body is fully received +
protocol-validated (`http_body_util::Limited` + `collect()`, driving
hyper/h2 validation to completion) BEFORE the upstream is dialed. This
is consistent with the already-shipped H2→H2 (`translate_h2_request_
to_h2`) and H2→H3 (`collect_h2_request_to_h3_fieldlist`) sibling paths,
which already `collect()` before forwarding. The buffer is HARD-CAPPED
by a NEW named constant `lb_l7::h2_proxy::MAX_REQUEST_BODY_BYTES`
(64 MiB, mirroring `lb_quic::MAX_REQUEST_BODY_BYTES`); exceeding it
yields `413 Payload Too Large` (new `ProxyErr::BodyTooLarge`), never an
unbounded allocation. A malformed inbound request now surfaces as
`ProxyErr::BadRequest` → `400`, returned BEFORE any backend dial, so it
can never leak the backend 200 body — the validate-vs-forward race is
closed STRUCTURALLY (deterministic, scheduler-independent), not merely
narrowed. Pseudo-header-in-trailers is rejected at the H2→H1 inline
capture, the shared `capture_request_trailers_rejecting_pseudo` helper
(H2→H2 / H2→H3 sites), and the H3 `feed_body` `InTrailers` arm
(RFC 9114 §4.3 → `ReqBodyEvent::Reset`). Optional future work (out of
scope this session, recorded only): a streaming-preserving variant that
withholds the response until the inbound stream is validated without
buffering the request body.

### F-COR-2 (BL-1) — balancer_counter_sync unsound concurrent bracket
(THE R1 BASELINE BLOCKER). `tests/balancer_counter_sync.rs:74-91`
asserts the sync-moment snapshot lies within `[min(pre,post),
max(pre,post)]` — a containment interval valid ONLY for a MONOTONIC
counter. The counter is non-monotonic (4 inc + 4 dec threads), so the
sync value legitimately oscillates outside the outer bracket.
Decisive capture: `snapshot=8650, bracket=[8649,8649]`, pre==post==
8649 — a coherent oscillation, impossible from a torn read of a
single AtomicU64; divergence bidirectional. **Product is correct**
(single atomic load, AcqRel inc, saturating-CAS dec). Independently
re-confirmed by lead reading the source. Reproduced 5/80 under
deliberate CPU oversubscription (isolation hid it 0/42 — R2).
Provenance: `e3ac961d`/`c4c27da6`/`ac58f613`, pre-S1, `git log
s2..s3` empty (R3). R6 CORRECTNESS (test-correctness). Fix: replace
the unsound concurrent bracket with the already-present sound
post-join exact-equality check + add a single-threaded monotonic
mid-flight sub-test. NOT test-weakening (R5): a provably-unsound
assertion producing false failures is corrected, real coverage
preserved+strengthened; verifier independently re-confirms both
product-correctness and unsoundness.

### F-COR-3 (A1-F1) — S3 temp-dir race fix applied to H1 only
S3 `9e58bbf2` gave H1 drain tests per-iteration unique temp dirs but
`test_sigterm_drains_h2_with_goaway` (:921),
`test_sigterm_drains_h3_with_connection_close` (:1062),
`test_reload_zero_drop_under_load` (:62) still use a fixed shared
`temp_dir()` path → path-collision class (R2: REAL DEFECT, not
environmental). Incomplete fix. Fix: reuse existing `unique_temp_dir`.

### F-COR-4 (A1-F2) — S3 Option-2 FinOnly clean_eof hole
`reload_zero_drop.rs:688` `CloseKind::FinOnly` does `let _ =
clean_eof;` so a "body complete but socket never FIN-closed on drain"
regression is misclassified as a clean PASS — the test cannot fail on
the close-defect it claims to guard. Latent (product correct today).
Fix: gate FinOnly on observed `Ok(0)` + negative regression. Same
file as F-COR-3's reload test → serialize.

### F-COR-5 (A4-F1) — lb-h3 proptest_qpack dead feature gate
`crates/lb-h3/tests/proptest_qpack.rs:15` `#![cfg(feature="proptest")]`
with proptest an unconditional dev-dep + empty marker feature →
`cargo test -p lb-h3` (default) silently runs 0 QPACK tests; only
`--all-features` runs them. Exact dead-gate S1 deleted from sibling
`lb-quic/tests/proptest_header.rs` (`25d8ad84`), never propagated.
Coverage-integrity defect (does not break the R1 --all-features gate).
Fix: remove the dead cfg gate (mirror S1's sibling fix).

### F-COR-6 (A4-F2) — stale-false S2 #[ignore] "UNBUILT"
`crates/lb-quic/src/h3_bridge.rs:1657` `s2_target_build_h1_request_
with_body_*` still `#[ignore]`d with a doc-comment claiming the
datapath is UNBUILT — FALSE: S2 P1-A built it (e2e-proven). S2's own
exit criterion (drop the #[ignore]) unmet; stale-false comment is the
class that derailed S3 Phase-0. Fix: un-ignore (assertion passes).
Same file as F-COR-1's h3_bridge change → serialize.

### F-COR-7 (A3-A4) — ENA driver-support blocklist dead / fail-open
SECURITY-adjacent (a defense layer rendered inert on real AWS ENA).
Proven: `ethtool -i ens5` empty `firmware-version:` → parse None →
firmware_of Err → drv_supported fail-opens Allowed. ROUND8-L4-05 ENA
blocklist can never fire on the platform it protects; unit test
passes only by bypassing the dead path. Runtime BPF_PROG_TEST_RUN
backstop also inert (aya 0.13.1). Not a D-1 blocker. Tractable
(~0.5d). Fix: key ENA blocklist on driver+kernel OR fail-closed on
empty firmware for fragile drivers + real drv_supported("ens5")
integration test.

### F-COR-8 (A4-F3, latent) — h3_graceful_close nonce no counter
`crates/lb-h3/.../h3_graceful_close.rs:71` pre-fix `pid+subsec_nanos`
nonce, no counter — safe only by one-test/unique-prefix coincidence;
re-introduces the round8 race if a 2nd #[test] is added. LOW, latent.
Cheap hardening (per-test counter, mirror round8 CERT_SEQ). Filed to
prevent re-derailing a future session.

---

## LOW / DOC tier (R6 → fix this session; one R7 sub-part)

### F-DOC-1 (A3-A3) — kernel support-window doc inconsistency
DEPLOYMENT.md §36 (5.15/6.1) vs verifier README+CI (adds 6.6); audit
box 7.0 outside both. Effective floor ~5.15 (no ringbuf/kfuncs; CO-RE
loads on 7.0). Doc-alignment fixable this session. SUB-PART = R7
product call: "officially support 7.x / extend the verifier matrix"
is a product decision, recorded as an open R7 item, not gate-blocking.

---

## ESCALATION CANDIDATE

### F-ESC-1 (A3-A1) — verifier-matrix logs are placeholders
`audit/ebpf/verifier-logs/{5.15,6.1,6.6}.log.committed` all still
`HARNESS-CAPTURED-PENDING-CI-RERUN` (last `ab3e37a3`). The declared
multi-kernel verifier validation has NEVER run on the shipped ELF;
only real evidence is the live aya load on kernel 7.0 (D-1). R4: not
asterisked. Disposition: capture a REAL verifier baseline on kernel
7.0 this session (tractable, the box can do it). The full
5.15/6.1/6.6 lane needs a privileged lvh/Docker CI lane — attempt
in-session; if proven infeasible in this environment, ESCALATE per
R6/R7(a) as a scoped CI-infra workstream (~0.5d) with effort estimate.

---

## DOCUMENTED CONSTRAINT — not a fix-finding

### N-1 (A3-A2) — native XDP impossible at jumbo MTU 9001
`lb_xdp.bin` built without xdp.frags; ena refuses native attach at
MTU>3498. LOUD kernel-reject failure mode (not silent). D-1 PASSED
with the documented transient MTU/channel workaround. Per the
round-9 d1-native-xdp-constraint doc this is an accepted documented
deployment requirement + a known long-term follow-up (rebuild ELF
with xdp.frags, ~1d, owned by ebpf/div-l4). Not a masked gate defect;
recorded for completeness, not an open fix.

---

## Disposition summary
| ID | Tier | Disposition |
|---|---|---|
| F-SEC-1 | SECURITY | FIX this session + reg test |
| F-COR-1 | CONFORMANCE | FIX this session + reg test |
| F-COR-2 (BL-1) | CORRECTNESS | FIX this session (R1 blocker) |
| F-COR-3 | CORRECTNESS | FIX this session |
| F-COR-4 | CORRECTNESS | FIX this session |
| F-COR-5 | CORRECTNESS | FIX this session |
| F-COR-6 | CORRECTNESS | FIX this session |
| F-COR-7 | CORRECTNESS/sec-adj | FIX this session + reg test |
| F-COR-8 | LOW/latent | FIX this session (cheap hardening) |
| F-DOC-1 | LOW/doc | FIX docs; R7 sub-part recorded |
| F-ESC-1 | MEDIUM | in-session 7.0 capture; escalate residual if infeasible |
| N-1 | constraint | recorded, not a fix-finding |

No finding asterisked (R4). Zero in limbo.
