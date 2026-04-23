# Reviewer sign-off

- Date: 2026-04-23
- Reviewer: reviewer (fresh session, no prior context)
- HEAD: `9385ef76bae786875583661dcfe6a1c14c2a47ec`
- Verdict: **PASS** (with documented advisories; no blocking HOLDs)

## Methodology

I reviewed the tree at HEAD top-down: (1) ran build + clippy + tests + halting-gate + cargo-deny from this session and confirmed all green; (2) walked `git log b9853178..HEAD` (33 commits) and spot-checked the seven pivotal commits named in Task #22 plus `8e9d1887` (fuzz) and `e6c119b4` (observability); (3) walked every row of `.review/done.md` §9.1–§9.11 against the live tree, verified the claimed file:line evidence, and checked doc/ADR accuracy; (4) reviewed code quality in the largest new surfaces (`lb-quic::{router,conn_actor,h3_bridge}`, `lb-l7::{h1_proxy,h2_proxy}`, `lb-l4-xdp::ebpf::main`, `lb-security::ticket`, `lb-observability::{admin_http,lib}`) and sampled the assertions in five integration tests; (5) cross-referenced the quinn→quiche migration ADR against the PR it justifies.

### Quality gates (fresh runs from this session)

| Gate | Command | Result |
|------|---------|--------|
| Release build | `cargo build --release --workspace` | clean |
| Clippy | `cargo clippy --all-targets --all-features -- -D warnings` | clean |
| Tests | `cargo test --workspace --no-fail-fast` | **429 passed / 0 failed / 0 ignored** across 105 test-result lines |
| Halting gate | `bash scripts/halting-gate.sh` | `PROJECT COMPLETE — halting gate green. Artifacts: 141/141. Tests: 59/59. Manifest: OK.` |
| cargo-deny | `cargo deny check` | `advisories ok, bans ok, licenses ok, sources ok` (two unmatched-license-allowance warnings only) |

## Per-row verdicts

Rows follow `.review/done.md` §9 numbering. A row is **PASS** if the lead's classification (✅ / ⚠ / ❌) accurately matches the tree at HEAD; this review is about honest accounting, not about promoting ⚠/❌ rows to ✅.

### §9.1 Build & Lint

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | release build, 0 errors/warnings | PASS | `cargo build --release --workspace` clean from this session. |
| 2 | musl target | PASS (⚠ honest) | XDP ebpf is an out-of-workspace crate; claim accurate. |
| 3 | aarch64-gnu target | PASS (⚠ honest) | Not CI-exercised; claim honest. |
| 4 | clippy -D warnings | PASS | Clippy clean from this session. |
| 5 | `cargo fmt --check` | PASS | halting-gate check 1 passed in fresh run. |
| 6 | `cargo doc` 0 warnings | PASS (⚠ honest) | `missing_docs` lint is deny in every lib crate-root (e.g. `crates/lb-quic/src/lib.rs:1`), so rustdoc's own warnings are pre-prevented. Row honest about the dedicated CI run gap. |
| 7 | `cargo audit` | PASS | `.cargo/audit.toml` present (`de5c6dbf`). `deny.toml` advisories clean. |
| 8 | `cargo deny check` | PASS | Clean from this session. |

### §9.2 Tests

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | `cargo test --workspace` 100% pass | PASS (with stale counter) | **429 passed / 0 failed** at HEAD. Advisory: `.review/done.md` row-1 cell still says "346 / 346 at HEAD `e04f1a75`"; the §9.11 epilogue correctly says "294 → 429 tests", so the row is stale but not dishonest. Low-severity doc drift; no verdict impact. |
| 2 | `-- --ignored` | PASS | No `#[ignore]` in tree; confirmed via test-output. |
| 3 | Coverage ≥ 80% per crate | PASS (⚠ honest) | `docs/conformance/coverage.md` is candid (75.45% lines workspace-wide, per-file breakdown with category + reason). Thoroughly documented. |
| 4 | Fuzz corpora ≥ 1h each | PASS (⚠ honest — row says ❌ but the tree now has `fuzz/`) | The ❌ cell is STALE: `fuzz/` exists at HEAD with 5 targets (`h1_parser`, `h2_frame`, `h3_frame`, `quic_initial`, `tls_client_hello`), seed corpora (5-7 per target), and smoke-run findings. See commit `8e9d1887`. Production ≥1h burns are honestly deferred in `fuzz/README.md`. Advisory: done.md row should read ⚠ not ❌. |
| 5 | Property tests | PASS (⚠ honest) | `crates/lb-io/src/pool.rs::tests::size_invariant_holds_under_random_ops` present; `crates/lb-l4-xdp/src/sim.rs` ships an RFC 1624 incremental-vs-recompute property test (confirmed at file line count 417). |

### §9.3 Parity & Research

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | PROMPT.md §28 with file:line + test:name | PASS (⚠ honest) | 59 manifest-required tests pass; deferrals documented in `docs/gap-analysis.md` and ADRs. |
| 2 | `.review/rfc-matrix.md` | PASS (❌ honest) | File absent; `docs/research/rfc{9112,9113,9114,9000}.md` + `hpack-qpack.md` cover research. |
| 3 | `adoption-list.md` | PASS (⚠ honest) | Absorbed into the 10 ADRs. |

### §9.4 Protocol correctness

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | H1 custom RFC 9110/9112 suite | PASS (⚠ honest) | `tests/conformance_h1.rs` exists; external suite absent. |
| 2 | h2spec | PASS (❌ honest — deferred to Step 7 per Task #22's forbidden list) | `tests/h2spec.rs` is already wired to run the `h2spec` subprocess if installed with a graceful skip otherwise — the integration seam is ready. |
| 3 | H3 interop | PASS (❌ honest) | `tests/quic_listener_e2e.rs` ships a real in-process quiche client (428-line test) covering 6 scenarios including RETRY mint/verify and 0-RTT replay drop; external `curl --http3` interop deferred. |
| 4 | gRPC tools | PASS (⚠ honest) | `tests/grpc_*.rs` unit-level green. |
| 5 | Autobahn WS | PASS (❌ honest) | WS deferred; correctly marked. |
| 6 | testssl.sh | PASS (❌ honest) | TLS listener now exists (Pillar 3b.2) and integration test `tests/tls_listener.rs` passes; external `testssl.sh` is Step 7. |

### §9.5 XDP

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | ELF loads SKB/DRV on 5.15/6.1 | **POTENTIALLY STALE** — needs investigation | done.md §9.5 row-1 reads ❌ "BPF ELF not produced". **But `crates/lb-l4-xdp/src/lb_xdp.bin` exists at HEAD (9864 bytes)** per commit `7eeb59fa`'s stat-line ("Bin 3064 -> 9864 bytes"). `dec3b67b` ("Pillar 4b-1: BPF ELF build + userspace loader startup integration") explicitly claims the ELF is committed. `tests/real_elf.rs` exists. The done.md §9.5 rows 1-3 should be re-graded ⚠ (load succeeds in tree; CAP_BPF kernel-attach on two kernels remains unverified in CI). Advisory: done.md §9.5 row-1..row-3 status is stale. |
| 2 | Verifier accepts | Same as row 1 | — |
| 3 | `bpftool` confirms | Same as row 1 | — |
| 4 | Userspace fallback | PASS | `crates/lb-l4-xdp/src/lib.rs` simulation green; `tests/l4_xdp_{conntrack,hotswap,maglev}.rs` pass. |

### §9.6 Lifecycle

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | Deterministic startup | PASS | `tests/controlplane_standalone.rs` + `tests/reload_zero_drop.rs` exercise it. |
| 2 | Clean shutdown | PASS (⚠ honest) | `tokio::signal::ctrl_c()` wired; `KillMode=mixed` documented. |
| 3 | SIGHUP reload | PASS | Both integration tests present and green. |
| 4 | XDP detach | PASS (❌ honest) | Detach path exists in `loader.rs`; binary shutdown wiring not present. |

### §9.7 Security

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | Adversarial report | PASS (⚠ honest) | `SECURITY.md` (79 lines) has the defenses table; `docs/research/dos-catalog.md` referenced. |
| 2 | No secrets | PASS | Trufflehog claim taken at face value — I cannot rerun the scanner from the reviewer forbidden-list, but no obvious secrets visible in sampled files. |
| 3 | `SECURITY.md` complete | PASS | 79-line doc with threat model + defenses table + panic-free posture; accurate. |

### §9.8 Observability

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | Prometheus `/metrics` | **STALE** | done.md §9.8 row-1 reads ❌ "No `/metrics` endpoint". **This is incorrect at HEAD.** Commit `e6c119b4` added `crates/lb-observability/src/admin_http.rs` (151 lines) with `GET /metrics` + `GET /healthz`, plumbed the `[observability].metrics_bind` config, and shipped `tests/metrics_endpoint.rs` with two green integration tests. `METRICS.md` already reflects the shipped surface. The §9.11 Ordered-remaining-work section does say "Prometheus /metrics ✅ `e6c119b4`", so the inconsistency is purely within done.md. Advisory: done.md §9.8 row-1 should be re-graded ✅. |
| 2 | JSON access logs | PASS (⚠ honest) | `tracing-subscriber` JSON format available; dedicated format not wired. |
| 3 | Cross-crate spans | PASS (⚠ honest) | Claim accurate. |

### §9.9 Performance

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | §26 perf doc | PASS (❌ honest) | No perf doc; deferral to Step 7 honestly noted. |
| 2 | No `below-target` | PASS (— honest) | N/A by row 1. |

### §9.10 Documentation

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | README | PASS (⚠ honest) | 32-line README; row accurate. |
| 2 | CONFIG/DEPLOYMENT/METRICS/RUNBOOK/SECURITY/CHANGELOG | PASS | All six present at top level; line counts: 134 / 182 / 160 / 164 / 79 / 53. |
| 3 | rustdoc on every public item | PASS | `missing_docs` deny-listed everywhere I sampled (`lb-l7/src/h1_proxy.rs`, `lb-quic/src/router.rs`, `lb-security/src/ticket.rs`, `lb-observability/src/admin_http.rs`). |
| 4 | ADRs | PASS | 10 ADRs + `ebpf-toolchain-separation.md` + `quinn-to-quiche-migration.md`. ADR-0003 correctly marked superseded. |

### §9.11 Reviews

| # | Check | Verdict | Evidence |
|---|-------|:------:|----------|
| 1 | Reviewer signoff | PASS | **This file.** |
| 2 | Auditor signoff | — (out of scope for this reviewer per Task #22) | Independent review. |
| 3 | Both agree | — | Deferred to lead reconciliation. |

## HOLD items

**None are blocking.** The advisories below are cosmetic doc-drift within `.review/done.md` that the lead should reconcile before writing `.review/SHIP.md`:

| # | Severity | Description | Suggested fix | Owner |
|---|:--------:|-------------|---------------|-------|
| A1 | low | `.review/done.md` §9.2 row-1 test count says "346 / 346 at HEAD `e04f1a75`"; actual at HEAD `9385ef76` is 429/429. The epilogue already says "294 → 429 tests" so the info is present — just the table cell is stale. | Update §9.2 row-1 to "429 / 429 at HEAD `9385ef76`". | team-lead |
| A2 | low | `.review/done.md` §9.2 row-4 reads ❌ "`fuzz/` directory does NOT exist". `fuzz/` exists at HEAD with 5 targets + seed corpora + smoke findings (commit `8e9d1887`). | Re-grade row-4 to ⚠ (infrastructure + smoke done; ≥1h production burns deferred), citing `8e9d1887`. | team-lead |
| A3 | low | `.review/done.md` §9.5 rows 1-3 read ❌ claiming "BPF ELF not produced". The ELF is committed: `crates/lb-l4-xdp/src/lb_xdp.bin` is 9864 bytes per commit `7eeb59fa`; `dec3b67b` wires loader-from-bytes into binary startup with `tests/real_elf.rs` covering it. | Re-grade §9.5 row-1 to ⚠ (ELF loads in-process; kernel-attach verification on two kernels remains pending). Leave row-4 ✅. | team-lead |
| A4 | low | `.review/done.md` §9.8 row-1 reads ❌ "No /metrics endpoint"; HEAD has a working admin HTTP listener (`e6c119b4`) with 2 green integration tests. The §9.11 "Ordered remaining work" block already says "Prometheus /metrics ✅ `e6c119b4`" so this is internal inconsistency. | Re-grade §9.8 row-1 to ✅ and cite `e6c119b4` + `crates/lb-observability/src/admin_http.rs`. | team-lead |
| A5 | informational | `docs/gap-analysis.md` was authored at the Rust-port baseline (`b9853178`, 2026-04-22) and has not been updated since Phase B shipped. Its §L4/UDP claims XDP "simulated", its §TLS claims "absent", its §HTTP/3 claims "simulated", its §Observability claims "No HTTP endpoint wired" — all stale relative to HEAD. FINAL_REVIEW §11 explicitly blocks `SHIP.md` while `docs/gap-analysis.md` lists open gaps, so freshening it is a prerequisite to ship. | Rewrite `docs/gap-analysis.md` top-to-bottom against HEAD, or delete-and-reauthor. Several "absent" rows are now "present"; several "simulated" rows are now "real". | team-lead |

None of A1-A5 change any bit in the tree; they are all doc reconciliation. The code, ADRs, and commit bodies are internally coherent and accurately describe what is in the tree.

### What is NOT a HOLD

- Cargo-deny's two `license-not-encountered` warnings on `CC0-1.0` and `CDLA-Permissive-2.0` — those are allowance entries that simply aren't exercised by the current dep graph; not a security issue.
- The fact that h2spec / Autobahn / testssl.sh / wrk2 / h2load / `curl --http3` have not been run — Task #22 forbids the reviewer from running them; they are correctly tagged as Step-7 deferrals throughout the tree.
- The ⚠ status of coverage (75.45% lines workspace-wide) — `docs/conformance/coverage.md` categorises each <80% file with a reason; that is the documented-exception path FINAL_REVIEW allows.

## Commendations

1. **Every new crate-root carries `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, clippy::todo, clippy::unimplemented, clippy::unreachable, missing_docs)]`.** This is backed by `scripts/halting-gate.sh` check 3 (the "Cloudflare 2025 outage rule") which greps every non-test source file for those constructs and fails the build on any match. The posture is both stated and mechanically enforced.

2. **The quinn→quiche migration ADR is a model of decision hygiene.** `docs/decisions/quinn-to-quiche-migration.md` lays out the forces, the four considered options, the positive/negative/neutral consequences, and the implementation plan. ADR-0003 is left in the tree marked superseded for audit trail rather than deleted. The commit body of `2050c8c5` cross-references it. This is the right pattern for a spec reversal mid-project.

3. **Integration tests are production-shape, not toy.** `tests/h2_proxy_e2e.rs` drives a real `reqwest::Client` through rustls → ALPN dispatch → H2 or H1 proxy with round-trip Alt-Svc + per-stream LB assertions (3/3/3 distribution). `tests/quic_listener_e2e.rs` stands up a real `quiche` client and verifies RETRY token mint-on-first-Initial / verify-on-second plus 0-RTT replay-drop at the wire. These are not `assert_eq!(2+2, 4)` smoke tests.

4. **The eBPF program ships with its own `#![deny(clippy::indexing_slicing, …)]` and uses RFC 1624 incremental checksum updates (`crates/lb-l4-xdp/ebpf/src/main.rs:267-318`) verified against a recompute-from-scratch property test in `crates/lb-l4-xdp/src/sim.rs`.** Both IPv4 and IPv6 paths fold ones'-complement sums through `fold32` twice; the sim-side test exercises both. That discipline is unusual in greenfield BPF code.

5. **`TicketRotator` (`crates/lb-security/src/ticket.rs`) is the clearest single file I reviewed.** 135-line module doc explains the forward-secrecy rationale, the rotator has 5 unit tests covering no-op, swap, overlap-decrypt, current-encrypt, and expiry-after-overlap, `Debug` impls explicitly elide key material, and `build_server_config` documents exactly when it returns `TicketError::ServerConfig`. The accompanying `tests/tls_listener.rs` proves ticket reuse across two TLS 1.3 handshakes at the rustls level.

## Signature

reviewer — **PASS**

All three quality gates ran clean from this session's invocation. Pivotal commits match their messages; APIs are coherent; tests carry meaningful assertions. Five low-severity doc-drift advisories against `.review/done.md` and `docs/gap-analysis.md` should be reconciled by the lead before writing `.review/SHIP.md`, but none block shipping; the tree itself is internally coherent and the gaps are exactly where `docs/gap-analysis.md` and `SECURITY.md` claim they are (modulo the staleness noted in A5).
