# Findings — auditor-1 (H1 / L7 proxy / HTTP-core / drain-shutdown)

Surface: crates/lb-h1, crates/lb-l7, crates/lb-io, drain logic, graceful
shutdown. Plus task #6 (BL-1) re-assigned by lead. Phase 1 = MEASURE
ONLY; proposed fixes written but NOT applied (R5; author!=verifier in P2).
Branch: audit/foundation-pass @ fdb1ef9f. Box: 8-core c6a.2xlarge.

## Baseline status on this surface (context)

`--all-features` baseline (audit/foundation-pass/baseline/, 3x, lead-run)
is NOT deterministically green per R1:
- run 1: FAILED TEST_RUN_1_EXIT=101 — balancer_counter_sync::
  test_no_divergence_under_load (BL-1). cargo aborts on first failure,
  so my H1/L7/IO/drain binaries never ran in run 1.
- run 2: TEST_RUN_2_EXIT=0, 197 ok, 0 FAILED.
- run 3: TEST_RUN_3_EXIT=0, 197 ok, 0 FAILED.
- fmt --check clean; clippy --all-targets --all-features -D warnings
  CLIPPY_EXIT=0.

Targeted re-confirmation under --all-features on my surface (all green,
isolation): lb-h1+lb-io EXIT=0; reload_zero_drop 4/4 x3 runs (Header &
FinOnly both exercised; all 16 H1 iters byte-complete);
h3_s3_inflight_h1_drain_proof 3/3; lb-l7 test_sigterm ok. S2's
UNVERIFIED "verified" PROTO-2-11 H1 / REL-2-02 claims re-confirmed green
under --all-features in isolation — but see F-1/F-2 latent test defects
and BL-1 the real blocker.

## BL-1 — balancer_counter_sync::test_no_divergence_under_load: unsound non-monotonic bracket invariant (TEST DEFECT)

- Surface: tests/balancer_counter_sync.rs (CODE-2-14 proof). Product
  reviewed: crates/lb-core/src/backend.rs (BackendState atomics),
  crates/lb-balancer/src/lib.rs (Backend::sync_from_state). Task #6.

- Verbatim baseline evidence (baseline/test-run-1.log:443):
  thread '...' panicked at tests/balancer_counter_sync.rs:83:9:
  snapshot diverged from atomic at sample 114:
  snapshot=7095, bracket=[7096, 7099]
  test result: FAILED. 2 passed; 1 failed; TEST_RUN_1_EXIT=101

- Deliberate reproduction under CPU load (debug binary
  target/debug/deps/balancer_counter_sync-dad5bd57b14911bf, 24
  busy-spin loops oversubscribing 8 cores, 80 runs; isolation hid it
  0/12 and 0/30 exactly as R2 warns). 5/80 reproduced, verbatim:
  run10: sample 8888: snapshot=8650, bracket=[8649, 8649]
  run28: sample 5099: snapshot=9834, bracket=[9833, 9833]
  run36: sample 4468: snapshot=19,   bracket=[20, 21]
  run56: sample 4228: snapshot=99,   bracket=[100, 101]
  run80: sample 9437: snapshot=13304, bracket=[13305, 13306]

- Proven mechanism (TEST defect, NOT a product memory-ordering bug):
  The test (balancer_counter_sync.rs:74-91) does three separate,
  non-atomic loads at distinct instants t1<t2<t3:
    pre_atomic  = state.active_connections();   // t1
    backend.sync_from_state();                   // t2: snapshot=atomic@t2
    post_atomic = state.active_connections();   // t3
    assert snapshot in [min(pre,post), max(pre,post)]
  Its written premise (lines 78-82) — snapshot must lie within the
  [min,max] of the two bracket samples — is valid ONLY for a MONOTONIC
  counter. active_connections is NON-monotonic: 4 inc threads + 4 dec
  threads (THREADS=8, i%2==0 ? inc : dec, lines 39-58) churn a
  saturating counter concurrently; the value oscillates. With a
  non-monotonic value the mid sample at t2 is NOT bounded by the t1/t3
  values — it can dip below / rise above and return between the outer
  reads. Decisive capture: snapshot=8650, bracket=[8649,8649] with
  pre_atomic == post_atomic == 8649. A torn read / ordering defect on a
  single AtomicU64 cannot yield a coherent value (8650) that is a real
  intermediate state the live counter passed through — only stale or
  garbage. 8650 is a value the counter genuinely held between t1 and t3
  and oscillated away from. Product read the CORRECT atomic at t2.
  Divergence is bidirectional (3 below, 2 above) — the fingerprint of
  oscillation, not of a one-directional ordering/staleness bug.
  Product is correct: BackendState::active_connections()
  (backend.rs:86-88) is a single AtomicU64::load (atomic, no torn read
  possible); inc_connections AcqRel publish (:111); dec_connections
  saturating compare_exchange_weak loop (:117-129);
  Backend::sync_from_state (lb-balancer/src/lib.rs:126-140) copies that
  single load. CODE-2-14 contract (snapshot == canonical atomic at the
  sync instant) HOLDS. The test asserts a strictly stronger,
  mathematically false property for a non-monotonic counter. Not a
  port/path/global collision; not a starvation timeout (R2). It is a
  WRONG-VALUE assertion from an unsound test invariant — proven from
  captured output, never "environmental".

- R6 tier: CORRECTNESS (test-correctness). Tractable, fix the test in
  Phase 2 (R6: correctness tractable -> FIXED + regression,
  author!=verifier). CI-blocking flaky gate (R1), masks no product bug
  (~1/3 of full-workspace runs red until fixed). Not SECURITY.

- Provenance: test file + Backend::sync_from_state = e3ac961d
  "CODE-2-14 — single source of truth: Backend binds Arc<BackendState>".
  Atomics: crates/lb-core/src/backend.rs = c4c27da6 (CODE-2-04 AcqRel)
  + ac58f613 (main base "ExpressGateway"). git log
  feature/h3-quic-s2..feature/h3-quic-s3 -- (all three files) EMPTY ->
  NOT S3. e3ac961d on every program + pre-program branch -> pre-existing
  pre-S1, R3 in-scope. Never --all-features-gated in S1/S2 (28 GB) — the
  exact S2 verification-gap class the S3 report warned of.

- Proposed fix (Phase 2, not applied): keep the valid CODE-2-14 intent;
  remove only the unsound concurrent bracket.
  1 (preferred, deterministic): delete the mid-flight 3-read bracket;
    keep the existing sound post-join exact-equality check (lines
    99-104, net delta 0, snapshot == final atomic). Add a
    single-threaded monotonic-phase sub-test (inc-only then dec-only)
    where the bracket invariant IS valid, to retain mid-flight coverage.
  2 (if concurrent mid-flight wanted): assert only the genuinely-true
    property — snapshot <= OPS_PER_THREAD * #inc_threads AND
    sync_from_state result equals SOME tight-retry re-read of the atomic;
    never bracket two independent reads of a non-monotonic value.
  Author!=verifier; re-run under the same 24x CPU-oversubscription
  harness >=80x with zero failures before calling the gate green.

## F-1 — H1-drain shared-temp-dir fix applied to H1 ONLY; H2/H3/reload-soak siblings still use fixed shared paths (INCOMPLETE S3 FIX)

- Surface: tests/reload_zero_drop.rs (lb-integration-tests). Drain.

- Mechanism (proven by code + S3 history): verifier-C round-1
  (s3-report.md:280-291) found a REAL shared-temp-dir race: H1 drain
  test wrote gateway.toml into FIXED temp_dir()/"eg-drain-h1" rewritten
  every iteration; under --workspace parallelism a sibling could
  remove_dir_all it mid-run -> File::create NotFound panic. S3 commit
  9e58bbf2 fixed it via unique_temp_dir() (pid+nanos+seq,
  reload_zero_drop.rs:204-217) but applied it ONLY to
  test_sigterm_drains_h1_with_connection_close (:808). Same defect class
  remains UNFIXED in three siblings (current source):
  - test_sigterm_drains_h2_with_goaway: :921
    temp_dir().join("eg-drain-h2") + create_dir_all .. remove_dir_all
    (:1027).
  - test_sigterm_drains_h3_with_connection_close: :1062
    temp_dir().join("eg-drain-h3") .. remove_dir_all (:1103).
  - test_reload_zero_drop_under_load: :62
    temp_dir().join("eg-test-zero-drop") .. remove_dir_all (:113).
  Lower severity than H1's (H1 rewrote 16x/run, the trigger verifier-C
  hit; siblings write once/process) but still a real fixed-shared-path
  defect class (R2: never "environmental"): a stale leftover dir from a
  crashed/killed prior run or a re-invocation overlap makes
  create_dir_all/cert/key/TOML writes (write_h1s_config_with_self_signed
  :452-488, write_quic_config_with_self_signed :252-293, all .expect())
  panic on NotFound/PermissionDenied. Not reproduced in this baseline
  (runs 2/3 green, run 1 aborted earlier). Latent under-parallelism
  fragility; filed per R3/R4 not asterisked, not environmental. The fix
  primitive (unique_temp_dir) already exists in-file -> mechanical
  completion of 9e58bbf2.

- R6 tier: CORRECTNESS (test infra robustness). Tractable; Phase 2,
  author!=verifier.

- Provenance: unique_temp_dir + H1-only application = 9e58bbf2 (S3).
  Fixed-dir H2/H3 tests = 1f7ab4bb "REL-2-02 — multi-protocol drain
  integration test" (S1/S2-program lineage). reload soak's
  eg-test-zero-drop = ac58f613 (main base) -> pre-existing, R3. git log
  s2..s3 -- reload_zero_drop.rs = 9e58bbf2,e47c55d3 only (siblings
  predate S3, untouched).

- Proposed fix (Phase 2, not applied): in the three tests replace
  temp_dir().join("eg-drain-h2"|"eg-drain-h3"|"eg-test-zero-drop") +
  create_dir_all with unique_temp_dir("drain-h2"|"drain-h3"|
  "reload-soak") (already create_dir_all's internally); keep the
  matching remove_dir_all. No assertion/timing change — pure isolation
  hardening mirroring 9e58bbf2's H1 change. Author!=verifier; re-run
  --workspace --all-features.

## F-2 — H1 drain test CloseKind::FinOnly discards clean_eof: cannot detect a "byte-complete but socket never FIN-closed" drain defect (TEST CONTRACT HOLE)

- Surface: tests/reload_zero_drop.rs drain_h1_attempt (S3 Option-2
  rework). Drain/shutdown.

- Mechanism (proven by code reading): the S3 report
  (s3-report.md:234-265) sells Option-2 as: byte-identical completion
  AND clean close via Connection: close header OR a clean FIN-only EOF
  (RFC 9110 7.6.1). Test docs assert it: reload_zero_drop.rs:685-687
  ("No header: this MUST be a clean FIN-only EOF ... If we did not
  observe Ok(0) the close was not clean") and :731-735. But the code
  does NOT enforce the FIN: the read loop sets clean_eof=true only on
  Ok(0) (:648-651); a WouldBlock/TimedOut exit (:653-658) leaves
  clean_eof=false and just breaks. close_kind (:681-693):
    if byte_complete { if has_conn_close { Header }
      else { let _ = clean_eof;  // DISCARDED
             FinOnly } } else { None }
  clean_eof is explicitly thrown away. Any byte_complete &&
  !has_conn_close outcome is classified FinOnly even if the socket
  never sent FIN and the read merely timed out after the body arrived.
  The per-iter assertion (:860-873) accepts Header|FinOnly -> PASS. So
  a genuine regression where the gateway delivers the full body but
  FAILS TO CLOSE on SIGTERM (real drain / FD-leak / half-open: conn
  wedges open until client read-timeout) is misclassified as a clean
  FinOnly PASS. The test that exists to guard the close-contract cannot
  fail on the "didn't actually close" defect it claims to cover. The
  proof test shares the parse_outcome weakness
  (h3_s3_inflight_h1_drain_proof.rs:273-288) but is shielded there by
  byte-completeness-centric asserts; in reload_zero_drop.rs the
  close-kind IS the contract, so the hole is load-bearing. Product
  behaviour is correct today (observed close_kind=FinOnly raw_len=4174
  = 78B head + exact 4096 body; proof test historical 31/31) -> LATENT
  test-contract hole, not an active product defect — but per R4/R5 a
  regression test that cannot catch the defect it asserts is itself a
  correctness defect, and the S3 "clean FIN-only EOF" claim is
  over-stated vs what the code verifies.

- R6 tier: CORRECTNESS (test contract). Tractable; Phase 2,
  author!=verifier. Not SECURITY (no product misbehaviour today).

- Provenance: drain_h1_attempt + the let _ = clean_eof; classification
  = e47c55d3 "correct H1 drain test CONTRACT (Option 2) + lock
  in-flight proof" (S3). git log s2..s3 -- reload_zero_drop.rs confirms
  e47c55d3/9e58bbf2 the only S3 touches. The narrowness is INTRODUCED
  by the S3 Option-2 rework itself (fixed header-only, shipped a new
  FIN-unverified hole).

- Proposed fix (Phase 2, not applied): classify FinOnly only when
  byte_complete && !has_conn_close && clean_eof (read loop observed
  Ok(0)). byte_complete && !has_conn_close && !clean_eof -> NEW failure
  outcome ("complete body but connection not closed on drain"), MUST
  fail the assertion not pass as FinOnly. Read window already generous
  (12 s >> jitter+drain) so a real clean FIN is reliably seen and only
  a genuine non-close trips it. Mirror the clean_eof-gated
  classification into h3_s3_inflight_h1_drain_proof.rs::parse_outcome.
  Author!=verifier; add a negative regression (stub completes body but
  holds socket open must FAIL).

## Surface items re-confirmed CLEAN (no finding)

- Product H1 graceful-drain (crates/lb-l7/src/h1_proxy.rs:588-619,
  crates/lb-core/src/shutdown.rs:324-347): S3 diagnosis holds —
  Connection: close on not-yet-flushed head, FIN-only on already-flushed
  head, both RFC 9110 7.6.1-valid; per-conn task tracked on shutdown
  TaskTracker, awaited by run_drain; in-flight not dropped. git log
  s2..s3 -- h1_proxy.rs shutdown.rs = EMPTY (no S3 product change).
- lb-h1 (chunked/parse) + lb-io (pool/dns/ring/sockopts/http2_pool/
  quic_pool) full suites green under --all-features in isolation; clean
  in baseline runs 2 & 3.
- lb-l7 test_sigterm_* unit tests green under --all-features.

## Summary for lead

- BL-1 (task #6): TEST DEFECT, mechanism PROVEN + reproduced 5/80 under
  CPU oversubscription. Product (BackendState / Backend::sync_from_state)
  correct — single AtomicU64 load, AcqRel publish, no torn read. The
  test's mid-flight 3-read bracket invariant is mathematically unsound
  for the non-monotonic (concurrent inc+dec) counter; decisive capture
  snapshot=8650 bracket=[8649,8649] (pre==post) proves an oscillation
  transient, bidirectional divergence confirms — not a memory-ordering
  bug. Pre-existing e3ac961d (pre-S1), NOT S3. R6 CORRECTNESS: fix the
  test in Phase 2. Only R1 baseline blocker on my surface; masks no
  product bug.
- F-1: S3 shared-temp-dir fix (9e58bbf2) INCOMPLETE — H1 only; H2/H3
  drain tests + main-era reload soak still use fixed shared temp paths.
  Mechanical Phase-2 completion (reuse existing unique_temp_dir).
  CORRECTNESS.
- F-2: S3 Option-2 rework (e47c55d3) shipped a contract hole —
  CloseKind::FinOnly discards clean_eof, so "body complete but conn
  never FIN-closed on drain" passes as clean close; test cannot fail on
  the defect it guards. CORRECTNESS; gate FinOnly on observed Ok(0) +
  negative regression in Phase 2.
- Rest of H1/L7/IO/drain surface re-confirmed green under --all-features
  (isolation) and in baseline runs 2 & 3.
