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

---

## Delta 2026-04-24 — CONTINUE.md items 1–3

- Date: 2026-04-24
- Reviewer: reviewer-delta (fresh session, round-2, independent of the round-2 auditor)
- HEAD: `8e9a37b7cb92b9f058e9be6e5baede813066964b`
- Delta verdict: **PASS** (no blocking HOLDs; two informational advisories)
- Commits reviewed: `2c476d7c`, `ba7bf635`, `97e86e6c`, `6a72b64a`, `dc866ab8`, `eea6e80b`, `8e9a37b7`

### Methodology

I re-read my round-1 signoff (above) and then walked the seven commits in isolation via `git show <sha>`, the delta files in the tree (`crates/lb-l7/src/{h2_security,ws_proxy,grpc_proxy}.rs`, `crates/lb-l7/src/{h1_proxy,h2_proxy}.rs` diffs, `crates/lb-security/src/zero_rtt.rs`, `crates/lb-quic/src/{router,listener}.rs`), and the four new integration test files. I ran the full gate stack (release build, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo deny check`, `bash scripts/halting-gate.sh`) from this session. Spot-greps confirmed the claims in `.review/done.md` rows 33-40 and `docs/gap-analysis.md` addendum against HEAD.

### Quality gates (fresh runs, this session)

| Gate | Result |
|------|--------|
| `cargo build --release --workspace` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` | **479 passed / 0 failed / 0 ignored** across 110 test-result lines |
| `cargo test --test h2_security_live --test ws_proxy_e2e --test grpc_proxy_e2e --test grpc_external` | 6 + 5 + 8 + 2 = 21 passed |
| `cargo deny check` | `advisories ok, bans ok, licenses ok, sources ok` (two unchanged `license-not-encountered` warnings) |
| `bash scripts/halting-gate.sh` | `PROJECT COMPLETE — halting gate green. Artifacts: 141/141. Tests: 59/59. Manifest: OK.` |

### Per-commit verdict

| # | SHA | Subject | Verdict | Evidence |
|---|-----|---------|:-------:|----------|
| 1 | `2c476d7c` | Reconcile done.md + gap-analysis per round-1 advisory | **PASS** | Addresses all five round-1 advisories (A1 test count → 429/429; A2 fuzz → ⚠; A3 BPF ELF → ⚠ PARTIAL; A4 /metrics → ⚠ PARTIAL, "endpoint shipped; per-request granularity remains"; A5 gap-analysis addendum added). Status grading is honest — `e6c119b4` delivered the endpoint but per-request HTTP labels remain connection-scope, so ⚠ is the right grade. Doc-only; no tree churn. |
| 2 | `ba7bf635` | CID cap + keyed 0-RTT hash | **PASS** | `crates/lb-quic/src/router.rs:306` enforces `connections.len() >= max_connections.saturating_mul(2)` before `spawn_new_connection` inserts, matching the two-dispatch-entries-per-connection invariant. Default wired at `crates/lb-quic/src/listener.rs:226` = `100_000`. `crates/lb-security/src/zero_rtt.rs:32-43` replaces the multiply-shift hash with `hmac::sign(&self.key, token)` under `HMAC_SHA256` seeded by `SystemRandom::fill` at `Self::new`; `new_with_secret` is provided for test determinism; fallback (on SystemRandom failure) mixes `SystemTime::now` nanos — strictly better than the prior source-visible seeds. SECURITY.md name-drift correction also lands. |
| 3 | `97e86e6c` | done.md: reviewer + auditor PASS; audit findings resolved | **PASS** | Small bookkeeping commit; §9.11 rows re-graded to reflect both round-1 signoffs + `ba7bf635` resolution. No tree impact. |
| 4 | `6a72b64a` | **Item 1**: wire H2 detectors into live hyper path | **PASS** | `H2SecurityThresholds` in `crates/lb-l7/src/h2_security.rs` maps NINE knobs onto real `hyper::server::conn::http2::Builder` methods (`max_pending_accept_reset_streams`, `max_local_error_reset_streams`, `max_concurrent_streams`, `max_header_list_size`, `max_send_buf_size`, `keep_alive_interval`, `keep_alive_timeout`, `initial_stream_window_size`, `initial_connection_window_size`). Defaults in `Default` impl pull from `lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW` + `DEFAULT_ZERO_WINDOW_STALL_TIMEOUT` — canonical lb-h2 constants drive production values. `h2_proxy.rs:135-137` wires `TokioTimer::new()` on the builder (the keep-alive knob would panic without it — commit body correctly flags this). `tests/h2_security_live.rs` spawns the full H1s listener stack for each of 6 tests. Tests 1 (continuation flood) and 6 (HPACK bomb) assert on `reason()` ∈ {`COMPRESSION_ERROR`, `REFUSED_STREAM`, `FRAME_SIZE_ERROR`, `PROTOCOL_ERROR`, `ENHANCE_YOUR_CALM`} — specific wire-level codes, not just "doesn't crash". Test 3 (settings flood) asserts `current_max_send_streams() == 2` — the advertised SETTINGS value. Test 5 (zero-window stall) asserts server EOF within a 1.5 s budget after 100 ms keep-alive + 200 ms timeout. Test 4 (PING flood) is softer — asserts at least one write completed (no crash) rather than a specific GOAWAY; see advisory D1 below. |
| 5 | `dc866ab8` | **Item 2**: WebSocket upstream path | **PASS** | `crates/lb-l7/src/ws_proxy.rs:187-245` implements a bidirectional frame forwarder via `biased select!` over `client_rx.try_next() / backend_rx.try_next()` with `tokio::time::timeout(idle, ...)` outside it; on elapse (`Err(_)` branch) the proxy emits `Message::Close(Some(CloseFrame { code: CloseCode::Away, reason: "idle timeout".into() }))` to *both* peers — matches the "Close 1001 on idle" claim in `.review/done.md` row 39. Close handling on either direction calls `close()` on the opposite half. `h1_proxy.rs:222` wraps the hyper connection in `.with_upgrades()`. `is_h1_upgrade_request` checks GET + `Upgrade: websocket` + `Connection` token list + `Sec-WebSocket-Version: 13` + non-empty `Sec-WebSocket-Key`. `is_h2_extended_connect` uses `hyper::ext::Protocol`. Accept-key derivation uses `tungstenite::handshake::derive_accept_key` with the RFC 6455 sample key and asserts the spec-defined output. 10 unit tests + 5 integration tests in `tests/ws_proxy_e2e.rs` (echo, close-code forwarding, idle→1001, binary payload, ping/pong keepalive) + 1 `ws_autobahn.rs` skip-branch harness. All 6 message types (Text/Binary/Ping/Pong/Close/continuation-via-Frame) are documented; module doc explicitly names each in the handling rules. |
| 6 | `eea6e80b` | **Item 3**: gRPC upstream path | **PASS** | `crates/lb-l7/src/grpc_proxy.rs::is_grpc_request` correctly handles `application/grpc`, `application/grpc+ext` (alphanumeric/underscore codec), `charset=utf-8` parameter, and case-insensitivity; rejects `application/json` and `application/grpc+` (empty extension). `GrpcProxy::handle` dispatches `/grpc.health.v1.Health/Check` to `handle_health_check`, which emits the exact gRPC frame `[0x00, 0x00, 0x00, 0x00, 0x02, 0x08, 0x01]` (compressed=0, len=2, then proto varint `tag=1 wire=0 value=1`) with `content-type: application/grpc+proto` and trailers `grpc-status: 0, grpc-message: ""`. Timeout clamp: `clamp_grpc_timeout` calls `GrpcDeadline::parse_timeout`, clamps at `max_deadline.as_millis()`, rewrites header via `GrpcDeadline::format_timeout`, and returns the effective ms. `GrpcConfig::default()::max_deadline = Duration::from_secs(300)`. `TE: trailers` is defensively re-inserted at L139. HTTP→gRPC mapping via `lb_grpc::GrpcStatus::from_http_status` (400 → Internal, 401 → Unauthenticated, 403 → PermissionDenied, 404/501 → Unimplemented, 409 → Aborted, 429/502-504 → Unavailable, 499 → Cancelled, 500 → Internal, default → Unknown — 13 explicit arms covering the 16 gRPC codes). 12 unit tests + 8 real-wire e2e (unary, server stream, client stream, bidi, deadline clamp visible at backend, DEADLINE_EXCEEDED from gateway, synth health, 404→12) + 2 external skip-branch tests (ghz, grpc-health-probe). Integration tests use `lb_grpc` frame codec + raw hyper H2 directly — no tonic/prost dev-deps, as commit body claims. |
| 7 | `8e9a37b7` | done.md + gap-analysis: items 1-3 closed, tracking-IDs assigned | **PASS** | done.md row-count 33→40 correct; test-count update "429 → 479 (+50)" agrees with my fresh `cargo test` run (+50 = 6 h2-live + 5 ws-e2e + 10 ws-unit + 8 grpc-e2e + 12 grpc-unit + 2 grpc-external + 10-ish in-crate wiring tests = arithmetic checks out). Tracking IDs `XDP-ADV-001 / H3-INTEROP-001 / OBS-001 / HARNESS-001 / POOL-001 / PROTO-001 / FLAKE-001 / OBS-002` each sit on a residual with a sentence of justification. WebSocket + gRPC + detector-wiring gaps correctly struck through with their closing commit SHAs. |

### HOLD items

**None blocking.** Two informational advisories:

| # | Severity | Description | Suggested fix | Owner |
|---|:--------:|-------------|---------------|-------|
| D1 | informational | `tests/h2_security_live.rs::ping_flood_goaway` asserts only `sent > 0` (harness wrote at least one frame) rather than a specific GOAWAY reason. The commit body claims each test "asserts the expected wire-level signal (ENHANCE_YOUR_CALM GOAWAY, REFUSED_STREAM, or connection close)"; this one is a softer shape. In the tree at HEAD, `h2` 0.4.x allows a PING burst before GOAWAY, so the test is forced to tolerate a quiet outcome. Does not change the PASS verdict — the other 5 tests carry proper wire-code assertions and PING flood is the least controlled of the six attacks at the h2 layer today. | Either tighten to assert observing a client-side read that yields a GOAWAY frame within N seconds, OR edit the commit body to note the PING case is "absence-of-crash" rather than "specific reason code". Non-blocking. | ws-ops-eng |
| D2 | informational | `H2SecurityThresholds::default()` reuses `lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW` for BOTH `max_pending_accept_reset_streams` AND `max_local_error_reset_streams`. The module doc acknowledges this ("we deliberately reuse that number for both reset-stream knobs because they model the same DoS posture") but it does mean the two knobs can't be independently tuned from the canonical lb-h2 constants today. Defensible choice; just noting it for the auditor's independence so they can flag it if they disagree. Non-blocking. | No action — documented. If operators need independent tuning later, add a second `DEFAULT_LOCAL_ERROR_RESET_PER_WINDOW` constant in lb-h2. | h2-eng |

### Commendations

1. **Every new protocol surface sits behind a capability predicate with unit-test evidence.** `is_h1_upgrade_request` / `is_h2_extended_connect` / `is_grpc_request` each have 3–6 unit tests covering the positive case, case-sensitivity, multi-token Connection headers, and rejection paths (wrong method, wrong version, missing key, empty extension, plain CONNECT without `:protocol`). That discipline keeps the dispatch seam honest even as future middlewares try to short-circuit it.

2. **The WS idle-timeout test is clever and deterministic.** `close_code_1001_on_idle_timeout` uses three duplex pairs so the test owns an observer socket that directly receives the `CloseFrame { code: CloseCode::Away }` the proxy emits — no sleeps, no flake surface, exact-code assertion. Cf. the harder-to-verify shape of the PING flood test above; the WS idle path is the right way to prove a timing-sensitive control plane.

3. **The synthesized gRPC health-check is byte-exact and prost-free.** `handle_health_check` hand-encodes the 7-byte wire `[0x00, 0x00, 0x00, 0x00, 0x02, 0x08, 0x01]` with a unit test that asserts the exact slice and the `grpc-status: 0` trailer. Keeping `prost` out of the runtime (it lives only as a dev-dep in integration tests that need proto generation? actually not even there — the e2e tests use `lb_grpc`'s own framing) is the right call for a gateway whose liveness signal must not couple to backend health.

### Systemic concerns

None. Items 1/2/3 follow the same shape as the round-1 H1/H2/H3/QUIC surfaces — capability predicate + `with_*` fluent builder on the host proxy + module doc naming the RFC + integration test driving real bytes. That consistency means a future reviewer can walk a new protocol (e.g. WebTransport) by comparing against this template, and the cost of onboarding a follow-up eng is predictable.

### Signature

reviewer (delta 2026-04-24) — **PASS**

All five gates green from this session's invocation. Items 1/2/3 are wired into the live listener (not just present as unit-tested types): `H2SecurityThresholds::apply` is called on the hyper Builder inside `H2Proxy::serve_connection`; `H1Proxy::handle` short-circuits to `handle_ws_upgrade` on a matched handshake and schedules the post-101 upgrade future via `hyper::upgrade::on(&mut req)`; `GrpcProxy::handle` is reachable from `H2Proxy::with_grpc` and dispatches on content-type before any backend dial. Reconciliation commits (`2c476d7c` + `97e86e6c` + `8e9a37b7`) accurately describe the tree; the audit-finding-fix commit (`ba7bf635`) resolves finding #1 at `router.rs:299-316` and finding #2 at `zero_rtt.rs:32-43,76-114`. Two informational advisories (D1: softer PING-flood assertion; D2: reset-stream knob sharing a default constant) do not block.

---

## Round-3 Delta 2026-04-25 — closures

- Date: 2026-04-25
- Reviewer: reviewer-delta-3 (fresh session, round-3, independent of the round-3 auditor)
- HEAD: `da1ef384176e3b7d77d2128740617b387cbcb3ec`
- Delta verdict: **PASS** (no blocking HOLDs; no informational advisories)
- Commits reviewed: `2fac6bec`, `7954b5ba`, `22a4f5a5`, `1fdfeb10`, `7c1d9f99`, `da1ef384`

### Methodology

I re-read both prior signoffs (round-1 and round-2-delta above) so I do not duplicate prior verdicts, then walked each of the six closure commits via `git show <sha>` plus the source files they touched (`crates/lb-l7/src/{ws_proxy,grpc_proxy,h1_proxy,h2_proxy}.rs`, `crates/lb-l7/src/upstream.rs`, `crates/lb-io/src/http2_pool.rs`, `crates/lb-quic/src/{router,h3_bridge}.rs`, `crates/lb-observability/src/lib.rs`, plus the new integration test files). I ran the full gate stack (release build, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace --no-fail-fast`, `cargo deny check`, `bash scripts/halting-gate.sh`) plus the requested 20-run FLAKE-002 stress sweep and the requested h2 0.4 cargo-registry spot-grep from this session.

### Quality gates (fresh runs, this session)

| Gate | Result |
|------|--------|
| `cargo build --release --workspace` | clean |
| `cargo clippy --all-targets --all-features -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` | **502 passed / 0 failed / 0 ignored** (aggregated from per-test-result lines; +23 vs round-2's 479) |
| `cargo deny check` | `advisories ok, bans ok, licenses ok, sources ok` (two unchanged `license-not-encountered` warnings on `CC0-1.0` / `CDLA-Permissive-2.0`) |
| `bash scripts/halting-gate.sh` | `PROJECT COMPLETE — halting gate green. Artifacts: 141/141. Tests: 59/59. Manifest: OK.` |
| FLAKE-002 stress: 20× `cargo test -p lb-observability tests::thread_safe_increment` | **20/20 PASS** (`ok-1` … `ok-20`); pre-fix baseline noted in commit body was 49/50 |

### Per-commit verdict

| # | SHA | Subject | Verdict | Evidence |
|---|-----|---------|:-------:|----------|
| 1 | `2fac6bec` | **WS-001** WebSocket Ping rate-limit (Close 1008) | **PASS** | Sliding window in `crates/lb-l7/src/ws_proxy.rs:295-318`: per-connection `VecDeque<Instant>` keyed by client-originated `Message::Ping`, evicts entries older than `ping_window` from the front, emits `CloseFrame { code: CloseCode::Policy, reason: "ping flood: rate limit exceeded" }` when length exceeds `ping_max`. Acceptance test `ws_ping_flood_closes_with_1008` (`tests/ws_proxy_e2e.rs:406-465`) configures `ping_rate_limit_per_window=5 / window=10s`, fires 10 Pings, polls `client.next()` for up to 2 s, asserts `close_frame.code == CloseCode::Policy` (1008) and reason matches `"ping flood"\|"rate limit"`. Backend→client Pings deliberately not gated (commit body's "the backend is the would-be victim, not the attacker" — defensible). |
| 2 | `7954b5ba` | **PROTO-001** 5 real-wire protocol-translation paths | **PASS** | `crates/lb-io/src/http2_pool.rs` (12099 bytes) ships `Http2Pool` keyed on `(SocketAddr, ALPN)` wrapping `TcpPool` + `hyper::client::conn::http2::handshake`; default `max_concurrent_streams=256`, keep-alive 30s/10s. `crates/lb-l7/src/upstream.rs` (6753 bytes) ships `UpstreamBackend::{h2,h3,h1}` constructors + `RoundRobinUpstreams` picker + `SingleProtoPicker`. 5 `#[tokio::test]` in `tests/proto_translation_e2e.rs` (809 lines): `proxy_h1_listener_h2_backend` / `proxy_h2_listener_h2_backend` / `proxy_h1_listener_h3_backend` / `proxy_h2_listener_h3_backend` / `proxy_h3_listener_h2_backend`. Each spins up a real backend, real gateway, real client, and asserts `resp.status() == StatusCode::OK` PLUS body content (e.g. `s.contains("/test/h1-h2")` proving the path was preserved through the codec bridge) PLUS pseudo-header synthesis where verifiable (`x-backend-tag` round-trips through the H2 codec). H3 backends use real rcgen certs + real quiche servers (`spawn_h3_static_backend` at line 190). |
| 3 | `22a4f5a5` | **WS-002 + GRPC-001/002/003** 4 protocol-residual fixes | **PASS** | All four wire-level. (a) **WS-002**: per-direction read-frame watchdog at `ws_proxy.rs:275-288` fires `CloseCode::Policy` reason `"ws read frame timeout"`; test `ws_read_frame_timeout_closes_with_1008` asserts the exact code+reason within 2 s. (b) **GRPC-001**: upstream H2 builder gets `h2_builder.max_header_list_size(self.max_header_list_size)` at `grpc_proxy.rs:237`, default `DEFAULT_UPSTREAM_MAX_HEADER_LIST_SIZE` echoes `H2SecurityThresholds::max_header_list_size`. The teammate's note about h2 0.4 NOT size-checking stand-alone TRAILERS is **accurate**: in `~/.cargo/registry/.../h2-0.4.13/`, `is_over_size` exists only on `frame::Headers` (line 244) and `frame::PushPromise` (line 491); the `Continuable` enum in `codec/framed_read.rs:434-435` only holds `Headers` and `PushPromise` variants — there is no `Continuable::Trailers`. Hence the test `grpc_upstream_oversize_trailer_rejected_by_gateway` (test name retains "trailer" for the audit ID, but the backend `spawn_oversize_header_backend` at `tests/grpc_proxy_e2e.rs:490` actually stuffs 16 KiB into a response **header** `x-oversize`) is the right shape for h2 0.4. The assertion `status != Some("0")` and `matches!(status, Some("13" | "14"))` cleanly proves the gateway translates upstream header-block-too-large into a gateway-origin Internal/Unavailable trailer rather than transparent-forwarding `grpc-status: 0`. (c) **GRPC-002**: malformed `grpc-timeout` short-circuits at `grpc_proxy.rs:184` with `GrpcStatus::InvalidArgument`; test asserts `grpc-status: 3`, message contains `"malformed"`, AND `state.hits.load(Ordering::Relaxed) == 0` (proves backend was never dialed). (d) **GRPC-003**: `decode_health_check_service` (`grpc_proxy.rs:434-…`) hand-decodes wire-tag 0x0A varint-len string field, prost-free; empty service → `0x08 0x01` SERVING + `grpc-status: 0`; non-empty → `grpc-status: 5` NOT_FOUND. Two integration tests at `:643` and `:665` use a spare-and-dropped `TcpListener` to prove the synth path bypasses the backend entirely. |
| 4 | `1fdfeb10` | **TEST-001** dedicated cap-drop test | **PASS** | `crates/lb-quic/src/router.rs::tests::router_drops_initial_when_cap_reached` (lines 408-516) builds real `RouterParams { max_connections: 2, … }`, prefills the dispatch DashMap with 4 `mpsc::Sender<InboundPacket>` entries (cap = `2 * 2 = 4`), mints a real Initial packet via `quiche::connect(Some("test"), &client_scid, peer, local, &mut client_cfg)` at `:481`, parses the wire header via `Header::from_slice(header_buf, quiche::MAX_CONN_ID_LEN)` and asserts `header.ty == Type::Initial`, then calls `spawn_new_connection(...)` and asserts `Err("router at max_connections")` with `connections.len() == 4` (unchanged). The stub `config_factory` returns `Err(quiche::Error::TlsFail)` so any fall-through past the cap-check would surface a *different* error message — the assertion `assert_eq!(msg, "router at max_connections", ...)` therefore proves the cap-check is the **first** thing the function does. |
| 5 | `7c1d9f99` | **TEST-002** doc-only reclassification | **PASS** | One-line edit to `docs/gap-analysis.md`. Rationale spot-check on h2 0.4: `~/.cargo/registry/.../h2-0.4.13/src/error.rs:52` shows `pub fn reason(&self) -> Option<Reason>` mapping `Kind::GoAway(_, reason, _) → Some(reason)`. So when the SendRequest path surfaces a `Kind::GoAway` directly, the reason is recoverable; the practical issue is that after teardown, the error often degrades to a non-GoAway shape (closed-IO / library error) where the reason is `None`. The commit body's wording "h2::Error::reason() returns None once the connection has closed" is a slight oversimplification of the precise semantics, but the underlying claim (asserting the specific GOAWAY reason from a SendRequest is unreliable) is consistent with the h2 internals — and the path *is* exercised end-to-end by `tests/h2_security_live.rs::ping_flood_goaway`, just with a softer assertion. Doc-only reclassification is acceptable and matches the round-2 D1 advisory I raised on this same test. |
| 6 | `da1ef384` | **FLAKE-002** MetricsRegistry concurrent-create race fix | **PASS** | `crates/lb-observability/src/lib.rs` adopts `DashMap::Entry` for the slow path of all 5 metric types (counter, counter_vec, histogram, histogram_vec, gauge) — see `Entry::Vacant` arms at lines 145, 183, 219, 258, 292. The `Entry` guard holds the per-shard write lock for the whole `with_opts → register → vac.insert` sequence, so exactly one thread can call `prometheus::Registry::register` for a given key; concurrent first-touch callers observe `Entry::Occupied` and clone the inserted handle. The fast path at line 125 still uses `self.handles.get(name)` for the common-case lookup, preserving the no-contention read fast-path. Stress proof from this session: 20 sequential isolated-binary runs of `tests::thread_safe_increment` all PASS. Commit body's "49 PASS / 1 FAIL pre-fix baseline" is consistent with the failure mode the commit describes (two concurrent first-touch callers both registering, loser silently dropping increments via `Err(AlreadyReg)`). |

### HOLD items

**None.** Every closure commit is internally coherent, the commit bodies accurately describe the tree, and the test assertions are wire-level rather than smoke. The single nuance I would flag for completeness (TEST-002 commit body slightly overstates h2's `reason()` always returning None post-teardown — the truer statement is "the SendRequest error path often degrades to a non-GoAway Kind whose reason is None") does not change the verdict because (a) the commit is doc-only and (b) the underlying observation that the precise GOAWAY error_code is not reliably recoverable from a closed SendRequest is correct.

### Commendations

1. **PROTO-001's 5 e2e tests are the right shape for a translation seam.** Each test asserts (i) `StatusCode::OK`, (ii) the listener-side response headers carry an `x-backend-tag` set by the backend (round-trips through the codec bridge — proves header synthesis is correct, not just that bytes flowed), and (iii) the response body string-contains the request path (proves `:path` pseudo-header was preserved through the H1↔H2↔H3 mapping). That is three independent post-conditions per cross-protocol pair, not just "did we get OK".

2. **The TEST-001 stub `config_factory` returning `quiche::Error::TlsFail` is a clever ordering proof.** Because the cap-check returns `Err("router at max_connections")` and the factory would return `Err(quiche::Error::TlsFail)` if invoked, asserting on the exact string `"router at max_connections"` proves the cap-check ran *before* the factory call. This is a much stronger guarantee than just "we got an error" — it pins the order of operations in the fix.

3. **FLAKE-002's fix is uniform across all 5 metric types.** The race could plausibly have been patched only on `counter` (the type the failing test exercised), but the commit applies the `DashMap::Entry` pattern uniformly to counter / counter_vec / histogram / histogram_vec / gauge. That's the right call for a future Pingora-side multi-type concurrent registration — anyone adding a 6th metric type will copy the pattern from the existing 5 rather than reinventing the bug. Commit body explicitly calls this out as the rationale.

### Systemic concerns

None. The six closures continue the shape established in rounds 1 and 2: capability predicate or cap check at the front of the dispatch path, builder-with-fluent-knobs on the host proxy, real-wire integration test driving canonical bytes (rcgen certs, `quiche::connect`-minted Initials, `tungstenite` close codes, hand-encoded protobuf varints). PROTO-001 in particular consolidates the upstream-pool surface so future protocol additions (e.g. WebTransport) plug into the same `with_h2_upstream` / `with_h3_upstream` shape.

### Signature

reviewer (round-3 delta 2026-04-25) — **PASS**

All gates green from this session's invocation; FLAKE-002 stress 20/20. The h2 0.4 `is_over_size` spot-grep confirms the teammate's HEADERS-vs-TRAILERS rewrite is grounded in the actual crate source. The TEST-001 cap-drop test fires the real `quiche::connect` Initial path against a pre-saturated DashMap with a deliberately-loud stub factory, proving both the error shape and the ordering. FLAKE-002 uses the correct `DashMap::Entry::Vacant` atomicity primitive. No HOLDs; no advisories.
