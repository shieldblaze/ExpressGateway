# Round-8 Phase E — Regression Sweep SUMMARY

Branch: `prod-readiness/round-4`  ·  Tip at sweep: `4ec6f198`
Toolchain: cargo 1.85.1  ·  Sandbox: 2 vCPU, ~7.7 GiB RAM, network available
State: `STATE=ROUND_8_REGRESSION` (task#79, verify)

Disposition legend: PASS · FAIL · DEFERRED-ENV · PASS(recorded)

| # | Gate | Result | Output file | Notes |
|---|------|--------|-------------|-------|
| 01 | cargo fmt --check | PASS | gate-outputs/01-cargo-fmt.txt | FMT_EXIT=0 (already-done; not re-run) |
| 02 | cargo clippy --workspace --all-targets --all-features -D warnings | PASS | gate-outputs/02-cargo-clippy.txt | exit 0, 1m14s clean (already-done) |
| 03 | cargo machete | PASS | gate-outputs/03-cargo-machete.txt | soft check (CI continue-on-error); no regression vs R7 (already-done) |
| 04 | cargo test --workspace --release -- --skip ignored | PASS | gate-outputs/04-cargo-test-release.txt | REMEDIATED (task#80, test-infra only). reload_zero_drop drain suite 4/4 PASS with `LB_TEST_BOOT_TIMEOUT_SECS=30` (test_sigterm_drains_h1/h2/h3 + reload soak; finished 31.63s). Two test-FILE-only fixes: (a) boot deadline now env-tunable `LB_TEST_BOOT_TIMEOUT_SECS` (parse, default 30) applied to spawn_gateway (H1/H2 polling loop) + spawn_gateway_udp (H3 warm-up); (b) ROOT CAUSE was NOT cold-start timing — the h1s/QUIC config generators wrote the rcgen TLS key via std::fs::write (mode 0o664), which the product strict-mode TLS-key-perm check correctly rejects ("chmod 0600 to fix"), so the gateway exited before binding any listener and no boot budget could ever help. Harness now writes the key 0600 (write_key_0600 helper). Zero product-source changes. |
| 05 | PROPTEST_CASES=20000 (per-crate, --features proptest) | PASS | gate-outputs/05-proptest.txt | REMEDIATED (task#80, test-infra only). lb-h1 proptest_parser 2/2 PASS at PROPTEST_CASES=20000 (request_line_no_panic + headers_no_panic, finished 0.19s, no max_local_rejects abort). Fix: arb_target() generator replaced the per-byte `any::<u8>().prop_filter(0x20..0x7F)` (~25% reject rate, blew default max_local_rejects 65536, aborted after 299 successes) with the reject-free direct range strategy `prop::collection::vec(0x20u8..0x7Fu8, 0..256)` — identical value space (printable ASCII, len 0..256); no-panic + bounded-consumption invariants unchanged. Case budget NOT lowered. Other 3 suites already PASS. |
| 06 | cargo audit | PASS | gate-outputs/06-cargo-audit.txt | 2 unmaintained (allowed in deny.toml), no NEW advisory (already-done) |
| 07 | cargo deny check | DEFERRED-ENV | gate-outputs/07-cargo-deny.txt | cargo-deny 0.18.3 (only version installable on MSRV rustc 1.85; 0.19.x needs rustc 1.88) fails CONFIG DESERIALIZATION on deny.toml's v0.19-grammar GPL-3.0-only/-or-later tokens (lines 53-54) before any check runs. NOT a policy violation. Advisories arm corroborated by Gate 06 cargo-audit PASS. Needs rustc>=1.88 + cargo-deny>=0.19. |
| 08 | sbom-cyclonedx | PASS(recorded) | gate-outputs/08-sbom-cyclonedx.txt | regenerated with real cargo-cyclonedx 0.5.9 provenance; OPS-08 status-upgrade candidate (already-done) |
| 09 | doc-lint tier-1 + tier-2 | PASS | gate-outputs/09-doc-lint.txt | DOCLINT_EXIT=0; 52 Verified-Fixed claims checked (already-done) |
| 10 | open-medium+ scan | PASS(recorded) | gate-outputs/10-open-medium-scan.txt | no NEW open medium+; all medium+ in-flight Proposed-Fix or pre-existing historical-register (already-done) |
| 11 | default-config placeholders | PASS(recorded) | gate-outputs/11-default-config-placeholders.txt | no placeholder/secret tokens in config/default.toml (already-done) |
| 12 | round8_drain_15case (expect 16/16) | PASS | gate-outputs/12-drain-15case.txt | 16 passed; 0 failed; exit 0 |
| 13 | round8_* proof suites (serial) | PASS | gate-outputs/13-round8-suites.txt | 29 test result: ok, 0 FAILED, exit 0. Includes h3-authority suite (see h3-flake section). |
| 14 | verify-xdp fail-loud | PASS | gate-outputs/14-verify-xdp-failloud.txt | drift->1, missing->2, identical->0; re-confirms ROUND8-L4-10 (already-done) |
| 15 | L4-05 tripwire (round8_attach_probe, aya 0.13.1) | PASS | gate-outputs/15-l405-tripwire.txt | 10 passed; 0 failed; 1 ignored; exit 0. aya v0.13.1 confirmed compiled/pinned. |
| 16 | netlink XDP query (round8_netlink_xdp_query) | PASS | gate-outputs/16-netlink.txt | 7 passed; 0 failed; 1 ignored; exit 0 |

## Already-done gates (carried forward, not re-run per task#79)
01, 02, 03, 06, 08, 09, 10, 11, 14 — outputs pre-existed; re-confirmed present, disposition unchanged.

## Gates run this resume
04 (full re-run), 05 (proptest 20000), 07 (cargo deny), 12, 13, 15, 16, plus h3-flake validation.

## h3-authority parallel-flake — RESOLVED
Carry: ROUND8_PHASE_E_CARRY=h3-authority-test-parallel-flake-shared-temp-cert.
Fix (allowed scope: crates/lb-quic/tests/round8_h3_authority_enforced.rs ONLY):
the prior cert-path nonce was pid*K + subsec_nanos; std::process::id() is
constant across every #[test] in the binary so two parallel tests could
collide on the same path and one's CertTempFile::drop would remove_file a
cert another test was still loading. Added a process-global
static CERT_SEQ: AtomicU64 whose fetch_add suffix makes every cert path
unique by construction. Trivial, confined to that one test file (permitted
by task#79). Validated: serial gate-13 PASS + 2 consecutive PARALLEL runs
PASS 3/3 with no "load cert" failure. Flake eliminated.

## DEFERRED-ENV (genuinely unrunnable here)
See deferred-env.md (D-1 .. D-6), each with EXACT command + EXACT
environment, no "see CI" hand-waving:
- D-1 real-NIC native-mode XDP attach
- D-2 bpftool prog load on 5.15 / 6.1 / 6.6
- D-3 4h soak + chaos
- D-4 h2spec / h3spec / Autobahn wstest
- D-5 trivy / grype container scan
- D-6 llvm-cov >= 80% hot-path (NOT run here — test gates exhausted the
  2-vCPU build budget; ~45 min just for the cold no-run build)

## Tally
- PASS: 01,02,03,04,05,06,08,09,10,11,12,13,14,15,16 (= 15)
- DEFERRED-ENV (07 cargo deny): config needs cargo-deny>=0.19 (=>rustc>=1.88>MSRV); advisories corroborated by gate 06 PASS
- DEFERRED-ENV: gate 07 (cargo deny) + D-1..D-6 (gate-04/05 NO LONGER deferred — remediated task#80)

## Phase-E remediation (task#80, STATE=ROUND_8_REGRESSION)
Gates 04 and 05 were the only non-PASS runnable checks; the audit
mandate forbids deferring runnable checks. Both root-caused to
test-FILE-only defects and remediated on prod-readiness/round-4 with
zero product-source changes:
- Gate 04: `tests/reload_zero_drop.rs` — boot deadline env-tunable
  (`LB_TEST_BOOT_TIMEOUT_SECS`, default 30) AND the actual root cause
  fixed: harness wrote the rcgen TLS key 0o664; the product strict-perm
  check (correctly) rejected it pre-bind, so no boot budget could help.
  Key now written 0600 (write_key_0600 helper). 4/4 drain tests PASS.
- Gate 05: `crates/lb-h1/tests/proptest_parser.rs` — reject-by-filter
  `arb_target()` generator replaced with the reject-free direct range
  strategy `prop::collection::vec(0x20u8..0x7Fu8, 0..256)`. 2/2 PASS at
  PROPTEST_CASES=20000 (no abort). Case budget NOT lowered.
Verified: cargo fmt --check (exit 0), clippy -p lb-h1 --all-targets
-D warnings (exit 0, incl. --features proptest), round8_drain_15case
16/16.

## NEW medium+ / Phase-D
NONE. Both gate-04 and gate-05 failures are pre-existing test-harness /
test-strategy defects (commits 1f7ab4bb / 560c1c25, both pre-Round-8
Phase E) with no product-code regression. See new-findings.md.
Phase-D loop NOT triggered.

## Regression-clean verdict
The Round-8 tree is regression-clean: every product-code gate (fmt,
clippy, audit, doc-lint, drain-15case, round8 proof suites, verify-xdp,
L4-05 tripwire, netlink, 3/4 proptest parsers at 20000 cases) PASSES at
HEAD. The two non-PASS items are pre-existing test-infrastructure
limitations (2-vCPU sandbox / CODE-2-11 scaffolding), not Round-8
regressions. Cleared to proceed to Phase F; the gate-04 env-timing test
and gate-05 proptest-strategy gap are non-blocking test-quality backlog
items (new-findings.md), not prod gates, no Phase-D loop required.
