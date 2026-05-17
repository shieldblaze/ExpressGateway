# SESSION 3 report — incremental H3 response egress

Branch: `feature/h3-quic-s3` (base `feature/h3-quic-s2`).
Box: 60 GB disk, 2 CPU.

## Phase 0 — full-workspace baseline: **BLOCKED (red, understood)**

Command: `cargo test --workspace --all-features`.

Result: one persistent failure —
`drain_tests::test_sigterm_drains_h1_with_connection_close`
(`lb-integration-tests`, `tests/reload_zero_drop.rs`). The gateway
keeps answering `HTTP/1.1 200 OK` but never emits `Connection: close`
after SIGTERM, for all 6 internal retry attempts.

### Diagnosis (corrected — this is NOT a load flake)

An earlier intermediate note in this report called it a load-induced
flake. **That was wrong** and is retracted. Evidence:

| Run | Features | Result |
|-----|----------|--------|
| Full `--workspace` baseline | `--all-features` | FAIL (6/6) |
| Full `--workspace` re-run | `--all-features` | FAIL (6/6) |
| Isolated single test | **default** (no `--all-features`) | PASS (on retry 1) |
| Isolated single test | `--all-features` | **FAIL (6/6), deterministic, ~5 s** |

The failure is **deterministic and correlated with `--all-features`**,
reproducible in full isolation (no host load) in ~5 s. It is not a
scheduling race. The serialization hardening attempted earlier was
premised on the wrong (load) diagnosis, did not help, and has been
fully reverted (`git checkout -- tests/reload_zero_drop.rs`; tree
clean).

### What it is, precisely

- The failing path is the **production `expressgateway` binary's H1
  graceful-drain** (`Connection: close` on SIGTERM) — PROTO-2-11 H1
  half. It is **not** H3/QUIC and not lb-quic (the S3 code area).
- The H3 drain test passes; the H2 drain test passes. Only H1.
- The contract-level unit test
  `lb-l7 h1_proxy::tests::test_sigterm_h1_graceful_shutdown_resolves`
  **passes** — so the in-process state-machine contract is intact;
  the gap is in the end-to-end binary plumbing under `--all-features`.
- `clippy --all-targets --all-features -- -D warnings`: clean.
  `cargo fmt --check`: clean.

### Provenance — this is S1/S2-program work, never validated full

`git branch --contains` shows the drain test and the PROTO-2-11 H1
half are **NOT on `main`**; they were introduced during this
program's S1/S2 lineage:

- `1f7ab4bb` REL-2-02 — multi-protocol drain integration test
- `82551dc5` REL-2-02 — un-ignore drain tests
- `de524167` PROTO-2-11 H1 half — `Connection: close` on shutdown
- `d1e12475` PROTO-2-11 H3 listener cancel — share shutdown token

S2's report records the full-workspace `--all-features` suite was
**never run in S1/S2** (28 GB disk could not). So this is precisely
the hard-rule condition: *"a failing baseline means S1/S2 work
regressed or never fully passed."* Here: **never fully passed under
`--all-features`** — Phase 0 did its job and surfaced it.

### Why I am stopping (hard rule + exit condition c)

- Hard rule: *"Do not proceed to Phase 1 until the baseline is
  green."* The baseline is not green and the red is real, not a
  flake.
- The owner explicitly directed: do not proceed on an asterisked
  baseline.
- The likely real fix is **feature code** (PROTO-2-11 H1: per-conn
  tasks on bare `tokio::spawn` vs `shutdown.tracker()`), which is
  (a) outside the S3 H3 response-streaming scope, (b) non-H3, (c) not
  lead-authorable feature work, and (d) needs author≠verifier.
- Continuing to bisect/patch autonomously past a hard gate would
  violate the session contract.

### Open questions for the owner (need a decision)

1. **Scope**: Is fixing an S2-era PROTO-2-11 H1 (non-H3) regression
   in scope for S3, or should S3's Phase 0 gate be redefined (e.g.
   "full `--all-features` workspace **excluding** the known-broken
   S2 H1 drain integration test, which is filed as an S2 defect")?
2. **Depth**: Want me to (a) bisect exactly which `--all-features`
   feature flips H1 drain, and (b) confirm regression-vs-preexisting
   by rebuilding `main` + s1 + s2 under `--all-features` (each a full
   heavy rebuild on a 2-CPU box)? This is doable but consumes
   significant budget before any S3 feature work starts.
3. **Authority**: If the fix is the PROTO-2-11 H1 `shutdown.tracker()`
   plumbing, that is feature code — confirm a builder may take it
   with a verifier, even though it is non-H3 and outside the stated
   S3 headline.

### Phase 0 status

GREEN: clippy `-D warnings`, `cargo fmt --check`, every deterministic
test **except** the one above, and **all H3/QUIC tests**.
RED/BLOCKED: `test_sigterm_drains_h1_with_connection_close` —
deterministic `--all-features`-only failure in S2-era PROTO-2-11 H1
binary plumbing. Foundation for the S3 *H3* work (lb-quic) is intact;
the blocked item is unrelated H1 code, but it fails the literal
Phase 0 gate, so per the hard rule **Phase 1 has not started**.

## Phase 1 — incremental response egress

NOT STARTED (blocked on Phase 0 gate decision). Design complete and
approved: see `audit/h3-program/s3-phase1-plan.md` (Bytes-pipe
`RespEvent` channel, H1 true streaming, H2/H3 left unchanged,
Content-Length no-regression contract, P1-B-parity teardown, R1–R8
tests, non-vacuous memory gauge).

## Phase 2 — gates + regression

NOT STARTED.

## Verdict

SESSION 3 PARTIAL — blocked at Phase 0: deterministic
`--all-features`-only failure of the S2-era PROTO-2-11 H1
graceful-drain integration test (`test_sigterm_drains_h1_with_
connection_close`); not a flake, not H3, never validated under the
full suite in S1/S2. Phase 1 design approved and ready; awaiting
owner decision on scope/authority for the Phase 0 blocker before any
S3 feature code is written.
