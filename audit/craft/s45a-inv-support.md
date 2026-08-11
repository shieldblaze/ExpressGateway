# scanner-support — security / io / config / observability / core / balancer / controlplane / health / cp-client

## Summary

files_scanned=88  slop=2  ambiguous=9  load_bearing_notable=41

Area: `crates/lb-{security,io,config,observability,core,balancer,controlplane,health,cp-client}/**`
(~22.6k LOC). Every `.rs` and `Cargo.toml` in the area was read in full — no file
was sampled, no comment region skipped. Read-only: nothing outside this file was
edited.

### `deny(missing_docs)` rule (lead correction, applied)

All 9 crate roots in my area carry `#![deny(missing_docs)]` (verified in each
`src/lib.rs`), and CI runs `clippy --all-targets --all-features -D warnings`.
**Therefore a `///` or `//!` on a `pub` item is GATE-LOAD-BEARING**: removing it
turns the gate red regardless of how thin the prose is. Every signature-restating
doc-comment in this area (`/// Whether the client is currently connected.`,
`/// Returns the backend identifier.`, the `LbPolicy` variant docs, …) is
**KEEP — deny(missing_docs), removal breaks clippy**. That retires the expected
"redundant doc-comment" category here essentially completely; I proposed none of
them and none need re-classifying.

Both surviving SLOP items were re-checked against this rule and both clear it:
one is a doc on a **private** item (`missing_docs` does not fire), the other is a
clause inside an **integration-test** crate root (`tests/*.rs` are separate crates
and do not inherit the lib crate's `#![deny]` attributes — grep-confirmed: no
`tests/` file in my area declares its own `deny`).

**The lead's calibration held exactly.** Measured on my area:

| Naive signature | Hits in my area | Reality |
|---|---:|---|
| `^\s*//\s*(let\|fn\|if\|for\|while\|return\|match\|use\|impl\|pub)` | 3 | 3/3 wrapped prose (`// return EINVAL. Either way, no panic.`, `// for synthetic (not-established) conns.`, `// for the loop iteration that observes...`). **Zero** commented-out code. |
| `TODO\|FIXME\|XXX\|HACK` | 0 | — |
| `//.*\bS[0-9]+\b` session markers | many | Uniformly finding-IDs with rationale payload (F-RES-2, F-S20-2, F-S26-1, CF-S27-2, CF-BODY-WALLCLOCK, SEC-2-*, ROUND8-*). All load-bearing. |

True-slop yield is **2 items**, both in the same class (dead scaffolding /
review-process narrative). The comment mass in this area is an evidence trail:
RFC citations (9110/9112/9113/9114/9000/9220/3986/5280, W3C trace-context),
attack-model notes (CVE-2023-44487, CVE-2024-27316, CVE-2022-30592), operator-facing
validation-range rationale, and negative-control test intent. Deleting at volume
here would be a straight R3 knowledge regression.

Coverage-gated files in my area (`scripts/ci/coverage-check.sh` lines 117–120):
`lb-balancer/src/[a-z_]+\.rs` (all), `lb-security/src/(hooks|conn_gate|watchdog|ticket|smuggle).rs`,
`lb-observability/src/admin_http.rs`. **Neither proposed deletion touches a gated file.**

## PROPOSED DELETIONS (SLOP)

`SLOP | crates/lb-observability/src/xdp_metrics.rs:217-226 | "/// Add an [IntGaugeVec] convenience to the registry. ... (Method definition l" | Dead scaffolding: `#[doc(hidden)] #[allow(dead_code)] fn _gauge_vec_anchor() {}` — an empty zero-arg fn whose doc states its only purpose is "so reviewers grep for `gauge_vec` and find it on the receiver type". **missing_docs CLEARED: the fn is PRIVATE (no `pub`), so the lint does not fire.** Zero callers (grep-confirmed); the fn names no type and imports nothing, so removal cannot orphan a `use`. Delete the doc-comment AND the fn together — leaving the fn would orphan its `#[allow(dead_code)]` from its reason. File is NOT coverage-gated. Sweeper must re-run `cargo clippy -p lb-observability --all-targets` to confirm behavior-neutral.`

`SLOP | crates/lb-security/tests/tls_versions.rs:13 | "Sec was OK with the single-line touch in `ticket.rs`; the new" | CLAUSE-SCOPED deletion only. "Sec was OK with the single-line touch in `ticket.rs`;" is inter-agent review-process narrative — the standard's "Updated per review" class. The REST of the sentence ("the new `build_server_config_with_policy` shadows the unchanged `build_server_config` shim so the rest of the codebase doesn't see a rename") is load-bearing API-compat rationale and MUST stay. **missing_docs CLEARED twice over: `tests/tls_versions.rs` is a separate integration-test crate that does not inherit lb-security's `#![deny(missing_docs)]`, AND the edit leaves the `//!` block in place (only a clause inside it goes), so no crate-root doc is removed either way.** Remove the leading clause only, re-capitalising "The new ...". If the sweeper cannot do clause-scoped edits, SKIP this item — the cost of a blunt full-block delete exceeds the gain.`

## AMBIGUOUS (KEEP + flag)

Four of these are additionally **gate-protected** by `deny(missing_docs)` — they
sit on `pub` items, so they could not be deleted even if someone wanted to:
`retry.rs:171-184` (`pub fn mint`), `admin_http.rs:14-15` (`//!` on a `pub mod`),
and both `lb-balancer/src/lib.rs` entries (`pub struct Backend` / `pub` field).
All four are "FIX THE PROSE, do not delete" items regardless.

`AMB | crates/lb-observability/src/lib.rs:518-522 | "// Keep prometheus::core::Collector in scope so trait bounds in the Collector-r" | Same dead-scaffolding shape as the SLOP item above (`#[allow(dead_code)] fn _force_collector_linkage(_: &dyn Collector) {}`, zero callers) BUT it is the only consumer of the `core::Collector` import at lib.rs:29, so removal is a two-site code edit needing a compile. The stated reason is also not evidenced in this file — there is no "Collector-returning helper" here (counter/gauge/histogram return concrete prometheus types). KEEP until someone compiles the removal; do not let a sweeper guess.`

`AMB | crates/lb-security/src/retry.rs:171-184 | "Mint a retry token binding `peer` and `odcid`. Panics at construction time (via" | STALE / SELF-CONTRADICTING, not slop. The prose says `mint` "Panics at construction time (via `assert!`) if `odcid` is longer than RETRY_MAX_ODCID"; the `# Panics` section 6 lines below says "This function never calls `panic!`, `unwrap`, or `expect`" and the code silently truncates to 255. One of the two statements is wrong about a security-relevant input path. FIX the prose, do not delete it. Flagged to lead as a finding.`

`AMB | crates/lb-observability/src/admin_http.rs:14-15 | "Intended for loopback scrapes. No TLS, no auth — the operator is expected to bi" | STALE: SEC-2-06 added bearer-token auth (`serve_with_auth`, `AdminAuthGate`) in the same file. The module header still tells a reader there is no auth. Fix (mention the optional gate), do not delete — the loopback-posture and no-mTLS statements are still true and load-bearing.`

`AMB | crates/lb-balancer/src/lib.rs:46-55 | "The legacy `u64` fields remain as a SNAPSHOT cache used by the scheduler's hot " | OVERCLAIM: says "production call-sites call it [`sync_from_state`] before each pick". Grep shows `sync_from_state` has ZERO production callers — only `tests/balancer_counter_sync.rs` and doc references. KEEP (it documents the intended contract) but the "production call-sites call it" claim is currently false. Finding for lead.`

`AMB | crates/lb-balancer/src/lib.rs:72-75 | "EWMA is updated on response completion in lb-l7; today still a plain `u64`. Pro" | OVERCLAIM + the EWMA-unfed issue the brief asked me to watch for. `set_latency_ns` / `latency_ewma_ns = ` have ZERO writers outside lb-balancer/lb-core (grep-confirmed across `crates/**`), so `Ewma::pick` always sees `latency_ewma_ns == 0` and falls through to its cold-start branch — every backend scores equally. The comment asserts the opposite ("is updated ... in lb-l7"). KEEP the comment; the limitation is NOT documented anywhere in lb-balancer and should be ADDED, not removed. Primary finding for lead.`

`AMB | crates/lb-core/src/backend.rs:98-101 | "Wave-2 appendix sweep (full table) follows in a subsequent commit on this branch" | Reads as session/plan narrative, but it is what explains why `inc_connections` is AcqRel while `inc_requests`/`dec_*` are Relaxed. Without it a future editor "fixes the inconsistency" in either direction. KEEP; the branch reference is stale wording, the deferral is real.`

`AMB | crates/lb-io/tests/miri_ring.rs:20-23 | "The brief: \"you do not need to actually run miri/loom this round — adding the s" | Quoted-brief narrative, but it is the only record of why this harness is scaffolding-only rather than exhaustive, and it states the dual-mode contract (plain `cargo test` AND miri). KEEP. Same shape at crates/lb-balancer/tests/loom_atomic_counter.rs:27-30 — also KEEP.`

`AMB | crates/lb-io/src/quic_pool.rs:764-808 | "// After three pushes the inner queue has 3 entries because push_into_pool bypas" | A ~40-line narrative block inside `per_peer_max_enforced` explaining, honestly, that the test does NOT test per_peer_max (synthetic conns are never `is_established`, so `PooledQuic::drop` never re-parks them). It reads like slop and is 5x the code it annotates — but deleting it would leave a test whose NAME claims a bound it does not exercise. KEEP. The real fix is the test, not the comment. Finding for lead: `per_peer_max_enforced` and `total_max_enforced` (lines 811-832) are both effectively vacuous.`

`AMB | crates/lb-security/tests/tls_versions.rs:115 | "let _ = AsyncWriteExt::shutdown(&mut tokio::io::stdout()).await; // keep tokio h" | Cargo-cult line with a vague comment that records intent but not mechanism ("keep tokio happy on CI"). Removing the comment orphans an otherwise inexplicable line. KEEP; flag the LINE for a later reviewer.`

(The `lb-cp-client` entry that was here has moved to **STUB / DEAD-CRATE FINDINGS**
below — it was never a comment call.)

## STUB / DEAD-CRATE FINDINGS

Reachability method: grep every symbol the crate exports across `crates/**`,
`tests/**`, `Cargo.toml`s, scripts and workflows; then read the binary's use-site
to distinguish *constructed* from *driven*. No deletions proposed — owner decision.

### (a) `crates/lb-cp-client` — **STUB-DEAD**

- **Not linked into the binary at all.** `crates/lb/Cargo.toml` lists
  `lb-controlplane` (:38) and `lb-health` (:41) but **not** `lb-cp-client`. The
  crate appears only twice in the whole tree outside itself: root `Cargo.toml:21`
  (`members`) and root `Cargo.toml:162` (workspace dep table). Neither creates a
  dependency edge to any binary or library.
- **Zero external references of any kind.** `grep -rn "lb_cp_client\|CpClient"`
  across `*.rs *.toml *.md *.sh *.yml *.yaml` returns hits **only** inside
  `crates/lb-cp-client/src/lib.rs` (definitions + its own 4 unit tests).
- **No test outside the crate.** Nothing in `tests/**` touches it.
- **The implementation is a stub in substance, not just in doc wording.**
  `connect()` (`src/lib.rs:73-79`) performs no I/O: it checks `endpoint.is_none()`
  and sets `self.connected = true`. There is no socket, no protocol, no
  control-plane exchange anywhere in the crate. The module doc's claim —
  "Provides the `CpClient` struct for connecting to and exchanging configuration
  with the control plane" — describes behaviour that does not exist.
- **Verdict: STUB-DEAD.** 119 LOC compiled by CI (it is a workspace member, so
  `--workspace` builds and lints it) and reachable by nothing.

### (b) `crates/lb-controlplane` — **REAL**

- **Driven at runtime in the binary.** `ConfigManager` is the first stage of the
  live S37-C SIGHUP config hot-reload: `crates/lb/src/main.rs:2489-2490`
  constructs `FileBackend` + `ConfigManager::new`, and `reload_config()`
  (`main.rs:409-465`) calls `mgr.reload()`, `mgr.current_config()` and
  `mgr.rollback_to_previous()` on the real error paths. Its call site is the
  `LifecycleSignal::SigHup` arm at `main.rs:3017-3033`.
- Covered by 4 root integration tests: `tests/controlplane_standalone.rs`,
  `tests/controlplane_rollback.rs`, `tests/controlplane_ha.rs`,
  `tests/reload_zero_drop.rs`.
- **Sub-finding — `HaPoller` (src/lib.rs:274-319, ~46 LOC) is test-only.** Its
  only use-site outside the crate is `tests/controlplane_ha.rs:14`; the binary
  never constructs it. Not dead (a test drives it), but it is not production code.
- **Verdict: REAL.** The crate earns its place; one type inside it is test-only.

### (c) `crates/lb-health` — **STUB-BUT-WIRED**

- **Constructed but never driven.** `main.rs:2564-2567` builds one
  `HealthChecker::new(3, 2)` per configured backend into `health_seed`, reads
  `status()` once for a log line (`:2572`), then binds the vector to `_health_seed`
  (`:2583`) purely to keep it in scope.
- **`record_success` / `record_failure` have ZERO callers outside the crate's own
  unit tests** (grep-confirmed across the whole tree). Since those are the only
  ways a checker leaves `HealthStatus::Unknown`, **every seeded checker is
  permanently `Unknown`** and the `initial_unknown` count in the log line is
  always exactly `health_seed.len()`.
- **The binary says so itself**, at `main.rs:2553-2559`: *"today nothing reads
  these (the picker filter wire-in is Wave 2 …). The seed proves the lb-health dep
  is reachable from the binary (round-1 inventory flagged it as UNUSED)."* That
  comment is honest and LOAD-BEARING — keep it — but it is also a candid statement
  that the wiring exists to satisfy a dead-dep inventory check rather than to do
  work. Worth the lead's attention as a pattern: a previous round's UNUSED finding
  was answered with a construction site instead of a caller.
- **Verdict: STUB-BUT-WIRED.** The crate's logic is real and unit-tested
  (thresholds, transitions, clamping); nothing in production drives it.

## LOAD-BEARING NOTABLES (explicitly preserved)

`KEEP | crates/lb-security/src/conn_gate.rs:170-173,224-227 | The AcqRel-per-SEC-2-16 rationale and the per-IP-overflow ROLLBACK of the listener counter. Deleting either invites the "sustained over-cap stream silently erodes the listener cap" bug that tests/conn_gate.rs:77-92 exists to catch.`
`KEEP | crates/lb-security/src/smuggle.rs:167-189 | check_te_strict rationale: WHY the codec chain is collapsed (upstreams mis-implement the decode → length mismatch across the gateway). Also documents WHY SmuggleTECL is reused rather than a new variant — an API-stability decision.`
`KEEP | crates/lb-security/src/smuggle.rs:244-253 | RFC 9113 §8.2.2 TE-must-equal-trailers and the pseudo-header-leak reject. Attack-blocking checks.`
`KEEP | crates/lb-security/src/zero_rtt.rs:10-17,43-50,131-135 | The 2026-04-23 auditor finding: source-visible multiply-shift seeds were a precompute-collision risk; now HMAC-SHA256 under a process-local key. Delete this and the cheap hash comes back.`
`KEEP | crates/lb-security/src/zero_rtt.rs:105-111 | WHY LRU replaced FIFO (SEC-2-05): a FIFO can push the in-flight replayee out of the window under unique-token spray. Canonical "use X not Y because Y does Z".`
`KEEP | crates/lb-security/src/zero_rtt.rs:54-56 | `// SAFETY:` note on the HMAC 32-byte invariant. Safety comment — never touched.`
`KEEP | crates/lb-security/src/zero_rtt.rs:243-248 | Free-list invariant + the "cannot happen under normal use" fallback rationale in `alloc_node`. Panic-freedom/bound note.`
`KEEP | crates/lb-security/src/watchdog.rs:30-59 | The SCOPE block (F-RES-5, S38): the Watchdog DETECTS, the timeout stack ENFORCES; `progress` is called once per request so SlowRate is dormant by design. Removing this makes a future editor "fix" a non-bug or, worse, trust the watchdog as the bound.`
`KEEP | crates/lb-security/src/admin_auth.rs:137-139 | "Never render the digest ... printing it routinely to logs invites grep-then-reuse mistakes." Credential-handling why-note; test at :378 enforces it.`
`KEEP | crates/lb-security/src/admin_auth.rs:17-21 | SHA-256-at-rest + `subtle::ConstantTimeEq` timing rationale.`
`KEEP | crates/lb-security/src/ticket.rs:1-30 | Forward-secrecy collapse argument for ticket-key rotation + why the opaque rustls handle ships instead of the spec's `[u8; 80]`.`
`KEEP | crates/lb-security/src/ticket.rs:98 | "Never render key material, not even indirectly. Elide it fully."`
`KEEP | crates/lb-security/src/ticket.rs:381-403 | REL-2-03 hot-reload contract: failed reloads keep the old bundle live; chain-depth ≤6; not_after is WARN-ONLY because refusing near-expiry certs is exactly wrong during an emergency rotation.`
`KEEP | crates/lb-security/src/ticket.rs:900-904 | CODE-2-04 `// CLIPPY-OK: stats-class` atomic justification pointing at docs/decisions/atomics.md.`
`KEEP | crates/lb-security/src/retry.rs:24-35,242-244 | Retry-token wire format table + constant-time MAC-compare intent (RFC 9000 §8.1.3).`
`KEEP | crates/lb-security/src/glitches.rs:1-24,60-72 | HAProxy 3.0 weight table + the "operators cannot tune six thresholds" rationale; the pinning test at :179 says any change is a public-API break.`
`KEEP | crates/lb-security/src/key.rs:22-34 | `mode & 0o077` policy + the non-Unix no-op carve-out.`
`KEEP | crates/lb-security/tests/conn_gate.rs:77-81 | "Critical regression guard for the rollback path" — negative-control intent. Delete it and a later simplification makes the test vacuous.`
`KEEP | crates/lb-security/tests/zero_rtt_replay_window.rs:38-41 | "The crucial LRU-vs-FIFO distinction" — encodes why the test proves what it proves.`
`KEEP | crates/lb-security/tests/smuggle_strict_te.rs:109-113 | "Regression guard: the lenient default behaviour must NOT change when the strict path is added."`
`KEEP | crates/lb-io/src/idle_send.rs:1-35,127-134,143-159 | CF-BODY-WALLCLOCK two-phase design, the `biased;` load-bearing note, and the S14 CFBW-RECHECK stale-`complete` re-load fix. The re-load comment is what stops Phase B being made unreachable for small bodies again.`
`KEEP | crates/lb-io/src/idle_send.rs:470-485 | Test arm (ix) rationale: "FAILS pre-fix ... PASSES post-fix". Non-vacuity proof.`
`KEEP | crates/lb-io/src/idle_send.rs:104-108 | Pinning-contract rationale for `tokio::pin!` on a non-`Unpin` future.`
`KEEP | crates/lb-io/src/http2_pool.rs:388-411 | F-MD-4 `reset_peer`: WHY dropping the PeerEntry (driver.abort) is used instead of injecting a body error — hyper may END_STREAM the truncated body as complete. Request-smuggling defense.`
`KEEP | crates/lb-io/src/http2_pool.rs:212-224,243-248 | ROUND8-L7-10 broad-eviction policy + the Pingora 0.6.0/0.8.0 upstream-smuggling precedent.`
`KEEP | crates/lb-io/src/http2_pool.rs:62-78 | Why H2ReqBody widened to a boxed error: `hyper::Error` has no public constructor, so a truncated streaming request could not be expressed as an error → smuggling parity.`
`KEEP | crates/lb-io/src/http2_pool.rs:95,436-440 | F-RES-2 (S38) `MAX_HEADER_LIST_SIZE`: caps what a malicious BACKEND can make us decode, for parity with the client-facing policy.`
`KEEP | crates/lb-io/src/pool.rs:390-405 | ROUND8-L7-10 "Do not delete `set_reusable` without first wiring a caller" — an explicit anti-deletion instruction guarding the Pingora body-length-mismatch bug class. Sacred.`
`KEEP | crates/lb-io/src/pool.rs:27-32,492-514 | Pingora EC-01 liveness probe semantics (WouldBlock=healthy, Ok(0)=half-closed) and why the socket is left non-blocking for `from_std`.`
`KEEP | crates/lb-io/src/ring.rs:47-49,92-94,125-128,150-152,175-177,190-195,240-243,248,260-261,276-277 | Every `unsafe` SAFETY comment in the io_uring wrappers. MANDATORY — never removed, never shortened.`
`KEEP | crates/lb-io/src/sockopts.rs:120-123,288-291 | SAFETY comments for `libc::listen` / `libc::setsockopt`.`
`KEEP | crates/lb-io/src/quic_pool.rs:352-356,421-422,450-460 | R12 single-sourcing of `connect_and_drive` — "no duplicate-and-diverge handshake code" across pooled vs dedicated dial.`
`KEEP | crates/lb-io/src/dns.rs:8-34 | TTL-approximation known limitation + singleflight + "a flaky resolver cannot flip a healthy pool into a negative-cache storm".`
`KEEP | crates/lb-config/src/lib.rs:39-48 | S37-B `deny_unknown_fields` rationale + the R3 statement that every previously-VALID config still parses byte-identically.`
`KEEP | crates/lb-config/src/lib.rs:1390-1528 | Every `validate_runtime` range rationale (drain 100..=300_000 vs systemd/k8s defaults; the 10M fat-finger ceilings; the XDP 1k/s floor that would otherwise blackhole traffic). These are the operator-facing WHYs behind each bound.`
`KEEP | crates/lb-config/src/lib.rs:1862-1905 | F-S26-1: raw_proxy XOR backends, and single-backend-family, with the "would be silently ignored / silently dropped" reasons.`
`KEEP | crates/lb-config/src/lib.rs:1186-1197,318-342 | F-S20-2 idle-flow reaper (cites the S20 soak: flows 0→56k, fds→28k, RSS→331MB, evicted=0) and S36-A `max_requests_per_h3_connection` (`0` re-opens the leak/DoS vector).`
`KEEP | crates/lb-config/src/reload.rs:14-18,299-352 | The S37-C HONESTY contract and the "diff's applied-set MUST exactly match what rebuild_l7_proxies consumes" invariant. Breaking this makes a reload silently lie.`
`KEEP | crates/lb-core/src/authority.rs:1-27 | HAProxy `BUG/MEDIUM` lesson: the check must live in ONE leaf crate so a new protocol parser cannot skip it. Plus the deliberate absence of a loopback exemption.`
`KEEP | crates/lb-core/src/shutdown.rs:51-58,193-197,256-263 | Why TaskTracker not JoinSet; the C-10/C-11 idempotency latch; the C-12 panic-path XDP-detach contract that an integration test enforces.`
`KEEP | crates/lb-observability/src/label_budget.rs:202-226 | Open-set vs closed-set cardinality reasoning and the explicit rejection of an `"other"` placeholder "because that masks the bug class". Also `CANONICAL_LABELS` (:48) — a table tests diff the live registry against.`

## Per-file load-bearing counts

Counting rule: one unit = one distinct rationale-bearing comment block (module doc,
item doc carrying a WHY / spec citation / invariant / regression payload, or an
inline why-note). Doc lines that only re-spell a signature are NOT counted.

```
crates/lb-security/src/lib.rs                          : 8
crates/lb-security/src/admin_auth.rs                   : 14
crates/lb-security/src/conn_gate.rs                    : 16
crates/lb-security/src/error.rs                        : 9
crates/lb-security/src/glitches.rs                     : 14
crates/lb-security/src/handshake.rs                    : 11
crates/lb-security/src/hooks.rs                        : 12
crates/lb-security/src/key.rs                          : 12
crates/lb-security/src/retry.rs                        : 22
crates/lb-security/src/slow_post.rs                    : 7
crates/lb-security/src/slowloris.rs                    : 7
crates/lb-security/src/smuggle.rs                      : 14
crates/lb-security/src/ticket.rs                       : 30
crates/lb-security/src/watchdog.rs                     : 22
crates/lb-security/src/zero_rtt.rs                     : 20
crates/lb-security/tests/conn_gate.rs                  : 8
crates/lb-security/tests/hooks_impl.rs                 : 3
crates/lb-security/tests/slowloris_watchdog.rs         : 8
crates/lb-security/tests/smuggle_strict_te.rs          : 9
crates/lb-security/tests/timeout_accept.rs             : 5
crates/lb-security/tests/tls_versions.rs               : 6
crates/lb-security/tests/zero_rtt_replay_window.rs     : 9
crates/lb-security/Cargo.toml                          : 6
crates/lb-io/src/lib.rs                                : 9
crates/lb-io/src/dns.rs                                : 16
crates/lb-io/src/http2_pool.rs                         : 22
crates/lb-io/src/idle_send.rs                          : 24
crates/lb-io/src/pool.rs                               : 20
crates/lb-io/src/quic_pool.rs                          : 26
crates/lb-io/src/ring.rs                               : 18
crates/lb-io/src/sockopts.rs                           : 12
crates/lb-io/tests/miri_ring.rs                        : 4
crates/lb-io/Cargo.toml                                : 3
crates/lb-config/src/lib.rs                            : 96
crates/lb-config/src/reload.rs                         : 28
crates/lb-config/Cargo.toml                            : 0
crates/lb-observability/src/lib.rs                     : 14
crates/lb-observability/src/admin_http.rs              : 16
crates/lb-observability/src/label_budget.rs            : 18
crates/lb-observability/src/log.rs                     : 11
crates/lb-observability/src/passthrough_metrics.rs     : 10
crates/lb-observability/src/probes.rs                  : 13
crates/lb-observability/src/prometheus_exposition.rs   : 6
crates/lb-observability/src/quic_h3_recycle_metrics.rs : 7
crates/lb-observability/src/quic_modeb_metrics.rs      : 8
crates/lb-observability/src/tracing_propagation.rs     : 20
crates/lb-observability/src/xdp_metrics.rs             : 16
crates/lb-observability/tests/health_endpoints.rs      : 6
crates/lb-observability/tests/log_format.rs            : 6
crates/lb-observability/tests/metrics_xdp_conntrack.rs : 4
crates/lb-observability/tests/metrics_xdp_slots.rs     : 5
crates/lb-observability/tests/panic_total.rs           : 4
crates/lb-observability/tests/red_label_budget.rs      : 5
crates/lb-observability/tests/tracing_traceparent.rs   : 5
crates/lb-observability/Cargo.toml                     : 5
crates/lb-core/src/lib.rs                              : 2
crates/lb-core/src/authority.rs                        : 14
crates/lb-core/src/backend.rs                          : 10
crates/lb-core/src/cluster.rs                          : 6
crates/lb-core/src/error.rs                            : 5
crates/lb-core/src/policy.rs                           : 12
crates/lb-core/src/shutdown.rs                         : 45
crates/lb-core/tests/per_connection_drain.rs           : 9
crates/lb-core/tests/round8_drain_coordinator.rs       : 8
crates/lb-core/tests/shutdown.rs                       : 8
crates/lb-core/Cargo.toml                              : 2
crates/lb-balancer/src/error.rs                        : 5
crates/lb-balancer/src/ewma.rs                         : 9
crates/lb-balancer/src/least_connections.rs            : 4
crates/lb-balancer/src/least_request.rs                : 3
crates/lb-balancer/src/lib.rs                          : 14
crates/lb-balancer/src/maglev.rs                       : 14
crates/lb-balancer/src/p2c.rs                          : 7
crates/lb-balancer/src/random.rs                       : 4
crates/lb-balancer/src/ring_hash.rs                    : 12
crates/lb-balancer/src/round_robin.rs                  : 3
crates/lb-balancer/src/session_affinity.rs             : 6
crates/lb-balancer/src/weighted_random.rs              : 7
crates/lb-balancer/src/weighted_round_robin.rs         : 11
crates/lb-balancer/tests/balancer_counter_sync.rs      : 10
crates/lb-balancer/tests/loom_atomic_counter.rs        : 7
crates/lb-balancer/Cargo.toml                          : 4
crates/lb-controlplane/src/lib.rs                      : 22
crates/lb-controlplane/Cargo.toml                      : 0
crates/lb-health/src/lib.rs                            : 12
crates/lb-health/Cargo.toml                            : 0
crates/lb-cp-client/src/lib.rs                         : 10
crates/lb-cp-client/Cargo.toml                         : 0
```

Total load-bearing blocks in area: **1014**.

### Note on the Cargo.toml comments

All 27 comment blocks across `lb-{security,io,observability,core,balancer}/Cargo.toml`
are dependency-edge justifications ("ring is already transitive via rustls; declaring
it direct keeps the retry module independent of rustls's feature set", the
`--check-cfg` loom registration, etc.). They are exactly what a future
`cargo-machete` / dep-audit pass needs. Uniformly LOAD-BEARING — no deletions proposed.
