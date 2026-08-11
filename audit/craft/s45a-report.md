# S45A — repo de-slop: final report

Base `main @ ff39fa08` (16/16 CI green) → branch `feature/de-slop-s45a`, PR #258.
Standard: [s45a-standard.md](s45a-standard.md) · Inventory: [s45a-slop-inventory.md](s45a-slop-inventory.md)
· Independent review: [s45a-verification.md](s45a-verification.md)

## What changed mid-session

The brief asked for an AI-slop removal pass. Phase 0 measured the tree and largely **refuted that
premise**: 7 genuinely slop comments in 28,617 comment lines (0.02%), zero commented-out code, one
`TODO` in production source (a tracked deferral). The owner approved that deletion set, then
redirected: **"I want 90% comment reduction — comment should only be present if reading the code
doesn't make sense or there is a catch."** Everything below Phase 0 executes the second directive.

## Result

| area | before | after | cut |
|---|---:|---:|---:|
| `crates/lb` + `lb-l4-xdp` + `lb-soak` | 6,752 | 1,779 | 73.7% |
| `lb-security` + 8 support crates | 5,927 | 1,613 | 72.8% |
| `crates/lb-quic` | 9,669 | 3,028 | 68.7% |
| `crates/lb-l7` + 4 HTTP crates | 6,269 | 1,979 | 68.4% |
| **`crates/**` total** | **28,617** | **8,399** | **70.7%** |
| `scripts/` + `.github/` | 928 | 567 | 38.9% |

Plus: 20 dead root `run-*.sh` deleted (~538 lines), 8 perf/soak provenance drivers archived to
`scripts/archive/`, and 534 over-long comment lines re-wrapped to ≤100 columns.

The sweep itself reached 73.0%, but collapsing blocks onto single physical lines left 616 comment
lines over 100 columns (`main`'s baseline: 16) — the content was right but the shape was not
something a person would type, which betrayed the tool and undercut the goal. Re-wrapping those
restores the line breaks and moves the honest figure to **70.7%**. All five swept crates are now at
zero over-100-column comment lines.

**70.7%, not 90%.** The gap is structural and was measured, not guessed:

* All 18 crates carry `#![deny(missing_docs)]`, so every `pub` item, enum variant and struct field
  must keep **at least one** doc line. Compressing every doc block to one line *and* deleting every
  plain comment is an **86.9% ceiling** — and that bound already requires deleting the SAFETY notes
  and the F-S29-1 canary, which the owner's own "or there is a catch" clause protects.
* `lb-l4-xdp` is catch-dense by nature: 74 `unsafe` blocks, kernel-verifier constraints, wire-format
  ABI mirrors. Nearly every surviving multi-line block there is a safety condition or a verifier
  constraint.

Reaching 90% requires dropping `deny(missing_docs)` from all 18 crates — a policy change, not a
comment pass, and it was not in scope.

## Behavior neutrality — proven, not asserted

`s45a-code-identity.py` strips every comment from both sides and compares the remainder. Across
**254 changed `.rs` files, exactly 5 differ**, each verified by hand and independently re-verified:

| file | change | verdict |
|---|---|---|
| `lb-observability/src/xdp_metrics.rs` | `_gauge_vec_anchor()` + its attrs removed | authorized; private empty fn, no callers |
| `lb-quic/src/h3_bridge.rs` | `map_err(\|_\| { X })` → `map_err(\|_\| X)` | identical |
| `lb-quic/tests/{grpc_h3_e2e,h3_h1_resp_stream_e2e,h3_h3_stream_e2e}.rs` | rustfmt enum-variant reflow | identical |

`unsafe` blocks: **95 before, 95 after**. Blocks lacking a nearby SAFETY note: **32 → 31** (coverage
improved). Tests: **1,582 across 308 files, per-file counts identical** — nothing silently dropped.

## The sweep deleted real code five times. All five were caught.

This is the honest headline of the session. An aggressive comment sweep deletes code that sits
adjacent to, or interleaved with, the comments being cut:

| deleted | consequence had it shipped |
|---|---|
| `#[map(name = "new_flow_cap_cfg")]` (eBPF) | map never emitted → ROUND8-L4-03 new-flow cap silently untunable |
| `let first_half` / `let second_half` (`lb-soak/src/timeseries.rs`) | still used by the DRIFT condition — would not compile |
| `let r = read_stats();` (`lb-l4-xdp/tests/stats_export.rs`) | still matched on — would not compile |
| `record_pin_reused("future_map", true)` | the test's only statement; body left empty, **passing vacuously** |
| `// Degenerate path` (`lb-core/src/shutdown.rs`) | exposed `clippy::collapsible_else_if` → gate red |

Two would have failed the build loudly. The vacuous test would not have — **no diff review would
have caught it**; the code-identity tool did. That tool is the durable artifact of this session.

## Knowledge preserved

Independent verification sampled 150 catalogued load-bearing items: **4 lost, 2 of them material**,
both restored before promote:

* **F-1** `lb/src/main.rs` — the S37-C/R6 rationale for installing lifecycle-signal streams ONCE
  outside the loop. Without it the hoist reads as ordinary and a future editor re-installing per
  iteration silently re-opens a measured SIGTERM-loss bug. Restored.
* **F-2** the WS-over-H3 client-FIN test — the sweep kept the setup narration and dropped the half
  that says what the test proves (`ws_handle_client_fin` maps FIN to a clean EOF, **not** a Reset,
  because RFC 6455 §7.1.5 makes the Close handshake the only clean close). Restored.

Explicitly preserved (compressed, never deleted): the **F-S29-1** `get_mut`-not-`or_insert_with`
note; the XFF append-not-insert note naming the **Envoy GHSA-ghc4-35x6-crw5** silent-drop class;
smuggling defenses; zero-RTT LRU-not-FIFO and HMAC-not-multiply-shift; the `biased;` select-ordering
contract; F-MD-4 `reset_peer`; kernel-verifier constraints and `ptr_at` bounds rationale; h3spec
waiver rationales; and the strings `round8_body_overread.rs`, `h2_connect_protocol_settings.rs` and
`round8_underscore_policy.rs` read out of production source and assert on.

13 distinct identifiers (5 RFC, 5 CF-, 2 F-S, 1 ROUND8-) no longer appear anywhere. Each was checked
by hand; the underlying fact survives in compressed form — e.g. the 103-Early-Hints limitation kept
its mechanism and `audit/deferred.md` pointer but dropped the RFC 8297 number. Listed as advisory
output by `s45a-invariant-census.sh`.

## Three stale claims corrected (findings, exit condition (c))

1. **`lb-security/src/retry.rs`** — the doc said `mint` panics via `assert!` on an over-long `odcid`;
   the `# Panics` section said it never panics; the code silently truncates. Contradiction on a
   security-relevant input path, now resolved to what the code does.
2. **`lb-observability/src/admin_http.rs`** — said "no auth"; SEC-2-06 had added an optional
   bearer-token gate in that same file. Now states the real posture.
3. **`lb-balancer/src/lib.rs`** — claimed EWMA latency "is updated on response completion in lb-l7".
   Nothing outside `lb-balancer`/`lb-core` writes it, so `Ewma::pick` always takes its cold-start
   branch. Now documented plainly: **selecting `LbPolicy::Ewma` silently gives you
   least-connections.** This corroborates the S41 "EWMA-unfed" note with grep evidence.

## Reported, not changed (owner ruled: carry forward)

* **`crates/lb-cp-client` is STUB-DEAD** — 119 LOC, absent from `crates/lb/Cargo.toml`, zero
  references tree-wide, `connect()` sets a bool without performing I/O, and its module doc claims it
  exchanges configuration with the control plane. Compiled and linted by CI; reachable by nothing.
* **`crates/lb-health` is STUB-BUT-WIRED** — constructed per backend, bound to `_health_seed` to stay
  in scope, never driven. `record_success`/`record_failure` have no callers outside the crate's own
  tests, so every checker is permanently `Unknown`. A prior round's UNUSED finding was answered with
  a construction site rather than a caller.
* **Two vacuous tests** in `lb-io/src/quic_pool.rs` (`per_peer_max_enforced`, `total_max_enforced`)
  do not exercise the bounds they name.
* **89 stale-but-load-bearing comments** catalogued in Phase 0 — load-bearing in intent, misstating
  the code. All kept. A correction pass is a different session with a different risk profile.

## Gates

Local: `cargo clippy --workspace --all-targets --all-features -D warnings` **PASS** ·
`cargo fmt --all --check` **PASS** · code-identity **5/254 files, all benign** ·
invariant census **binding checks pass, 5 canaries ok**.

R1's local ×3 at full parallelism was **not feasible** on this box (2 cores / 7.8 GB / 67 GB): a cold
`cargo test --workspace --all-features --no-run` reached 13 GB of `target/` with 2.9 GB free and was
stopped to avoid ENOSPC. The owner approved CI as the binding gate. Note clippy *is* affordable
locally (~1–2 GB) because it analyzes without linking; it is the test-binary link step that is not.

CI on `0c734b1a`: **16/16 jobs success** — Test, Coverage, Conformance (h2spec `--strict` + h3spec),
Chaos Attack Suite, XDP Verifier Smoke, Container Image, Panic Freedom, MSRV, Security Audit,
cargo-deny, Doc Lint, Fuzz Smoke, Check, Clippy, Format, Release Build.

### Coverage: gate PASS, but not literally unmoved

The D-6 gate passes — *31 hot-path modules passed, 0 below* — and every module remains ≥80%. But
8 of the 32 modules moved slightly against the `ff39fa08` baseline, in **both** directions:

| module | ff39fa08 | now | Δ |
|---|---:|---:|---:|
| `lb-l7/src/h2_proxy.rs` | 80.96% | 81.43% | +0.47 |
| `lb-quic/src/conn_actor.rs` | 84.58% | 84.77% | +0.19 |
| `lb-quic/src/listener.rs` | 87.13% | 86.13% | **−1.00** |
| `lb-l7/src/h1_proxy.rs` | 86.64% | 86.58% | −0.06 |
| `lb-l7/src/h1_to_h1.rs` | 91.36% | 91.03% | −0.33 |
| `lb-l7/src/h1_to_h2.rs` | 92.31% | 92.13% | −0.18 |
| `lb-l7/src/h3_to_h1.rs` | 95.16% | 95.00% | −0.16 |
| `lb-l7/src/h1_to_h3.rs` | 96.70% | 96.63% | −0.07 |

R10 says a coverage move means something behavioral changed — so it was investigated rather than
waved through. **All eight files are token-identical to `main` modulo comments** (none appears in
the code-identity differ list), so no behavior in them changed. Removing comment lines shifts source
line numbers, which shifts how llvm-cov attributes regions at the margins; combined with the
run-to-run variance this gate is documented to have (`audit/ci/s44-coverage-metric-rebaseline.md`
records it flipping 3 RED / 3 green on byte-identical source before the S44 metric fix), that
accounts for sub-1% movement in both directions.

Stated plainly: coverage is **not bit-identical**, the gate passes with margin, and the code under
those modules is provably unchanged. Claiming "unmoved" would have been wrong.

## New evidence for CF-S44-GRPC-H3-TE-HANG — it reaches the Coverage job too

The post-merge run on `main @ 5c9f0752` hit the carried hang. Same code, two outcomes:

| run | job | duration |
|---|---|---|
| PR `0c734b1a` | Coverage | **11 min** (12:57:39 → 13:08:57Z) |
| main `5c9f0752` | Coverage | **hung >76 min** in the `--all-features` suite step, cancelled |

The merge commit's tree differs from the PR head by **exactly one markdown file**
(`audit/craft/s45a-report.md`) — zero code difference — so this is not attributable to the merge.
Two things worth carrying:

* The hang is **not confined to the `test` job**; the Coverage job runs the same suite under
  `llvm-cov nextest` and hangs there too, while `test` passed on the very same commit.
* **The `coverage` job has no `timeout-minutes`**, so a hang burns to GitHub's 6-hour job ceiling
  silently. `test` has the same exposure. Adding a `timeout-minutes` to both would convert a 6-hour
  silent stall into a fast, legible failure — cheap, and it does not weaken any gate.

Re-running the cancelled job (`gh run rerun --failed`, preserving the 15 green jobs) is the workaround
until the diagnosis session lands.
