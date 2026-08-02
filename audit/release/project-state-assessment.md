# ExpressGateway — Project State Assessment (ground-truth)

**As of:** 2026-07-04 · **main tip:** `1a4e4fae` (verified `git rev-parse HEAD`) ·
**Method:** verified-from-source (code / Cargo.lock / `gh` CI+API / `cargo audit`), **not** from
memory or docs. Author≠verifier: three assessor passes (reachability / quality / deps-ledger) plus
an independent lead re-verification of every red-cause and every reachability finding against the
same sources. **Read-only — nothing was changed.**

Every claim is tagged **[V]** verified-from-source (checked this session) or **[I]** inferred
(reasoned from evidence, not directly executed). Divergences from docs/memory are flagged **⚠️ DIVERGENCE**.

---

> ## ⚠️ SUPERSEDED IN PART BY S43 (2026-08-02) — read this first
>
> This document was accurate as of **2026-07-04**. `main` did not run CI again until
> **2026-08-02**, so parts of §1 and §5 aged out. S43 (merge `e9df7cca`) changed the
> following. **Everything else in this document still stands.**
>
> - **R1 advisories — CLOSED, by bumping rather than waiving.** Upstream shipped fixes in
>   the interim: anyhow `1.0.102 -> 1.0.104` (RUSTSEC-2026-0190 patched >= 1.0.103) and
>   quinn-proto `0.11.14 -> 0.11.16` (RUSTSEC-2026-0185 patched >= 0.11.15). §7.1's
>   ignore-with-rationale plan was therefore **not** used — **no entry was added to
>   `deny.toml` or `.cargo/audit.toml`**, so the waiver surface is unchanged. Closes R1 **and**
>   R3 (Scheduled Scans).
> - **A THIRD red existed that this document does not list.** Nightly deprecated
>   `AtomicU64::fetch_update`; the Fuzz Smoke lane builds with `cargo +nightly -D warnings`,
>   making it a hard compile error in `lb-quic`. It was invisible here because main's last CI
>   run (`28336426614`, 2026-06-28) predates the nightly that introduced it. Fixed in S43.
> - **R2 is re-characterised.** The "rotating second red / boundary flicker" framing is wrong
>   on both counts. Four samples of byte-identical source measure **79.60 / 79.65 / 79.70 /
>   80.18** against an exact 80.00 floor — marginally **under**, failing 3 of 4, not
>   oscillating. The "flaky Test job costs coverage" theory is **refuted** (Test passed on the
>   run where Coverage failed and vice versa; separate runners). Root cause: `lb-l7` is
>   compiled **twice** and llvm-cov merges both instantiations into one file record
>   (`LF:1887/LH:1502` = 79.60%, but only 1780 `DA:` lines / 1441 hit = 80.96%);
>   `handle_inner` runs **661x** in one instantiation and **0x** in the other, and the 0x copy
>   is the lib unit-test build, which cannot reach the request hot path because
>   `hyper::body::Incoming` has no public constructor. **§7.1's "add an h2_proxy test to lift
>   coverage off the boundary" is therefore not achievable as written.** Evidence + four
>   options: `audit/ci/s43-h2proxy-coverage-dual-instantiation.md`. Still open, deliberately —
>   correcting the metric is an owner-approved gate change needing all 31 modules re-baselined.
> - **§5 deps drift.** Latest as of 2026-08-02: rustls **0.23.43**, quiche **0.29.3**,
>   tokio-quiche **0.19.1**, h2 0.4.15, tokio 1.53.1 (hold `<1.52` still applies).
> - **hyper#4050/#4102 — SHIPPED.** **hyper 1.11.0 (2026-07-20)** contains
>   `http2: avoid buffering Upgraded writes without send capacity (#4102)`. §5's "NOT in any
>   released crate" no longer holds: **CF-S27-2 / the WS-H2 un-gate is now unblocked upstream**
>   and awaits a re-run of the S30 backpressure repro before the default is flipped.
> - **Owner rulings on §6/§7.** The XDP fork is decided: **fix it — it is a bug** (option A,
>   not the re-scope). Health: **wire passive -> picker ejection (G5)**, with a failure
>   threshold, a re-admission path, and a minimum-healthy floor (outlier-detection semantics),
>   proven by four R13 negative controls. Neither is started.
>
> §2 (done-and-verified) and §3 (reachability gaps G1-G10) were re-spot-checked in S43 and
> **hold** — `publish_backends_v4` still has zero callers in `crates/lb/src/`, and
> `_health_seed` is still bound-and-dropped at `main.rs:2582`.

---

## 0. EXECUTIVE SUMMARY — the one-paragraph true state

ExpressGateway is a **functionally complete, well-tested L7 proxy** whose **production source and
dependency graph have been byte-frozen since the S38 security audit** (`git diff --stat 18afc8ad
1a4e4fae` touches only the `lb-soak` bench harness + docs; **zero** `Cargo.*` bytes) **[V]**. The
full HTTP protocol matrix (9 cells + QUIC + WebSocket + gRPC) is real and reachable; conformance
(h2spec 146/1/0 strict, h3spec 12-waiver) and the S38 security posture (0C/0H/1M/7L/4I) and S39
perf baseline all remain valid because nothing underneath them moved. **BUT main is CI-RED right
now** on two counts: (1) a **deterministic Security-Audit failure from _two_ untriaged upstream
advisories** (RUSTSEC-2026-0185 quinn-proto + RUSTSEC-2026-0190 anyhow — the S42 report and memory
name only the first), both with negligible production reach; and (2) a **rotating second red** that
is either the h2_proxy coverage boundary-flicker (79.6% vs the 80% floor) or the fcap1/saturation
test flake — both known non-regressions. Separately, the assessment surfaced a **material
reachability gap the docs oversell: the "L4/XDP load balancer" is _not reachable_ in the shipped
binary** — the XDP program attaches but no userspace code ever populates its conntrack/backend maps,
so it XDP_PASSes every packet (stats/no-op attach), and in-kernel Maglev is deferred. The
**headline "highly-available load balancer" is also softer than it reads**: only RoundRobin is live
(9 of 11 algorithms are library-only and not config-selectable), and **both active _and_ passive
health checking are inert** (seeded checkers are never consulted or fed, so unhealthy backends are
never ejected). None of these block a release on core-HTTP-proxy grounds, but they are honest
capability corrections and the XDP/health ones are the most important new findings. Finally, the
**release machinery is not yet armed**: `main` has **no branch protection and no rulesets at all**
(anyone can push, no required checks), and the release soak-gate is wired in YAML but **all its
`SOAK_*` secrets/vars are unset**.

**In one line:** the *product* is release-quality on its HTTP core and frozen-stable; the *release
process, the CI honesty, and several marketed L4/HA capabilities* are the real open work.

---

## 1. RED / BROKEN-NOW  (what is actually failing today)

Latest `main` CI = run **`28336426614`** (push of `1a4e4fae`) = **FAILURE, 14/16 jobs green** (`gh
run view 28336426614`) **[V]**.

| # | What's red | Root cause (verified) | Real / flake | Severity | Fix belongs to |
|---|---|---|---|---|---|
| R1 | **Security Audit** (`cargo audit -D warnings`, `ci.yml:225`) | **TWO** untriaged advisories. `cargo audit -D warnings` locally emits `error: 1 vulnerability found!` **and** `error: 1 denied warning found!` **[V]**: (a) **RUSTSEC-2026-0185** quinn-proto 0.11.14 (7.5 high, OOM via out-of-order stream reassembly); (b) **RUSTSEC-2026-0190** anyhow 1.0.102 (`Error::downcast_mut` unsoundness). **Neither is in `deny.toml:8-42` nor `.cargo/audit.toml:5-40`.** | **Real (deterministic)** | Med (CI-gate; low prod reach — see below) | **deps session** |
| R2 | **Second red — ROTATES** | Same tree, adjacent runs, different red: run `28334977156` → **Coverage** red (`lb-l7/src/h2_proxy.rs` at **79.60%** vs the exact-per-file 80% floor, `scripts/ci/coverage-check.sh`); run `28336426614` → **Test** red (`cargo test` exit 101, the fcap1/saturation flake), Coverage green **[V]**. | **Flake / boundary** (both known non-regressions; prod source frozen) | Low | **code session** (add h2_proxy test to lift off the 80% boundary; harden fcap1 isolation) |
| R3 | **Scheduled Scans / "Dependency Audit (weekly, strict)"** (run `28321775259`) | Same `cargo audit -D warnings` → same R1 advisories; geiger + machete green **[V]**. | Real (= R1) | Med | closes with R1 |

**⚠️ DIVERGENCE (CI honesty):** the S42 stamp commit `1a4e4fae` claims the two reds are "Security
Audit (RUSTSEC-2026-0185) + Coverage (h2_proxy)". Reality on that very commit's own run: **the reds
were Security Audit + _Test_**, and the Security-Audit red is **two** advisories, not one (0190 is
also firing under `-D warnings`). The docs/memory **undercount the advisory red and mis-name the
rotating red.** (agent-D's ledger further mislabels 0190 as "ALLOWED" — the plain `cargo audit`
prints "1 allowed warning", but the CI's `-D warnings` **denies** it; my direct run confirms
`1 denied warning found!` **[V]**.)

**Production-reach of the R1 advisories (both low):**
- **quinn-proto (0185):** reaches the shipped binary **not at all**. `reqwest` is a **dev-dependency**
  (`Cargo.toml:158` section, `:195`), pinned `default-features=false, features=["rustls-tls","http2"]`
  — the `http3` feature that pulls quinn/quinn-proto is **off**, so quinn-proto is a lockfile-only
  phantom, not compiled even into test binaries **[V]/[I]**. Cosmetic clear: `cargo update -p
  quinn-proto` → 0.11.16, or ignore-with-rationale.
- **anyhow (0190):** anyhow **is** a direct production dep (`Cargo.toml:82,178`) + foundations-transitive,
  but the advisory is an **unsoundness** (not a remote vuln) in `Error::downcast_mut()`, a path the
  gateway does not exercise adversarially. Triage = ignore-with-rationale (or bump when a fixed
  anyhow ships).

**Not-a-CI-job but release-broken right now:**
- **Branch protection: NONE.** `gh api repos/:owner/:repo/branches/main/protection` → 404
  "Branch not protected"; `gh api .../rulesets` → `[]` **[V]**. `main` accepts direct pushes with
  **no required checks**. The S40 "required-checks rename" gate risk is moot in the worst way — there
  is nothing to rename because nothing is enforced. Secret scanning + push protection also disabled
  (`gh api repos/:owner/:repo` → `security_and_analysis`) **[V]**.
- **Release soak-gate un-armed.** `release.yml` (workflow_dispatch `soak-gate`) requires
  `SOAK_AWS_ROLE_ARN` (secret) + `SOAK_REGION/AMI/SUBNET_ID/SECURITY_GROUP_ID/IAM_INSTANCE_PROFILE/
  S3_BUCKET` (vars). `gh secret list` / `gh variable list` show **only stale 2022 `MONGO_CONNECTION_STRING`
  / `ZOOKEEPER_ADDRESS` secrets and `COPILOT_*` vars** — **none of the `SOAK_*` are set** **[V]**. The
  gate cannot run until they are configured.

---

## 2. DONE-AND-VERIFIED  (genuinely complete + reachable)

All verified from source (`crates/lb/src/main.rs` = the shipped `lb` binary; config schema
`crates/lb-config/src/lib.rs`). "Reachable" = a user can enable it via the positional-arg TOML config
in the shipped binary.

| Capability | Status | Key citation |
|---|---|---|
| **9-cell HTTP matrix** (H1/H2/H3 front × H1/H2/H3 backend) | **Reachable** — all 9 cells wired | fronts `lib.rs:619-647`; H2-via-ALPN dispatch `main.rs:2016,2164`; H3-terminate backends `wire_h3_terminate_backends main.rs:1774+`; upstream `with_h2/h3_upstream main.rs:1555-1558` |
| **QUIC Mode A** (L4 passthrough, no-decrypt) + **`mint_retry`** | **Reachable** | `[passthrough]` `lib.rs:1134`, spawn `main.rs:2903`; `mint_retry` default true `lib.rs:1184`, consumed `passthrough.rs:797` |
| **QUIC H3-terminate** ("Mode B" in prompt) | **Reachable** | `protocol="quic"` no-raw_proxy, `main.rs:1758` |
| **QUIC raw-proxy re-originate** (code's own "Mode B") | **Reachable** | `[listeners.quic.raw_proxy]` `lib.rs:1041`, `main.rs:1687` |
| **WebSocket over H1** (RFC 6455) | **Reachable, open** | `[listeners.websocket]` `lib.rs:760`, `with_websocket main.rs:1398` |
| **WebSocket over H2 / H3** | **Reachable but GATED off** | H2 flag `h2_extended_connect` default false `lib.rs:803`, `main.rs:1567`; H3 flag `h3_extended_connect` default false `lib.rs:814`, `main.rs:1718` |
| **gRPC (deadline-clamp + synth health)** | **Reachable, h1s-only** | `GrpcListenerConfig` validator restricts to `protocol="h1s"` `lib.rs:1639` |
| **gRPC over H3** | **Reachable (opaque forward, no dedicated knob)** | rides H3-front→H2/H3-backend cells (S29) |
| **TLS termination + ticket rotation** | **Reachable** | `[listeners.tls]` `lib.rs:975`, `build_tls_bundle main.rs:1005`, rotator `main.rs:1010` |
| **Config (positional-arg CLI) + SIGHUP hot reload** | **Reachable** (reload swaps `backends`/`http`/`h2_security`/`websocket` only; rest restart-required) | CLI `main.rs:2430`; SIGHUP `main.rs:3330`, swap subset `main.rs:657-692` |
| **H3 connection recycling** (`max_requests_per_h3_connection`, default 1000, 0=off) | **Reachable** | `lib.rs:341,388`; enforced `conn_actor.rs:363` (cap→GOAWAY); off on raw-proxy `main.rs:1694` |
| **Conformance** | h2spec **146/1/0** strict; h3spec **12-waiver** (both CI-green in `28336426614`) | `audit/h3spec/s22-h2spec-strict.log:263`; `scripts/ci/h3spec-check.sh:42-57` |
| **Security posture (S38)** | 0C/0H/1M/7L/4I; prod wire-parsing delegated to hyper/h2/quiche/rustls/tungstenite; only hand-rolled prod parser = `lb_quic::public_header` — **still accurate** (source frozen) | `audit/security/s38-findings.md:18` |
| **Perf baseline (S39)** | eff WS<H2<H3<H1; 4h burn-in 11/12 BOUNDED, R8 held over billions of ops — **still the valid reference** (deps+source frozen) | `audit/perf/s39-report.md:64-110` |

**⚠️ Honest caveats on the "done" cells (all [V]):**
- **H2 front is TLS-only** (served via `h1s` ALPN; there is no h2c cleartext listener token).
- **H3→H2 and H3→H3 backends use only the _first_ resolved address** (`main.rs:1867,1879`
  `addresses.first()`) — those two cells are **not load-balanced across multiple backends**, unlike
  every other cell.
- **"Mode B" is overloaded**: the code labels the *raw-QUIC re-originate* path "Mode B"
  (`main.rs:1748`), while docs/memory use "Mode B" for *H3-terminate*. Three distinct QUIC datapaths
  exist (passthrough / H3-terminate / raw_proxy) — a real doc-hazard.

---

## 3. REACHABILITY GAPS  (code-present but NOT user-reachable — the "docs oversell" list)

Independently lead-verified by grep of the binary crate vs test dirs (all **[V]** unless noted):

| # | Gap | Verdict | Proof |
|---|---|---|---|
| G1 | **XDP L4 load-balancing (CT-hit → XDP_TX)** | **UNREACHABLE in the shipped binary — NEW, HIGH** | `publish_backends_v4` / `CtInsertGate` / `set_new_flow_cap` have **zero callers** in `crates/lb/src/`, control-plane, or cp-client — called only in `lb-l4-xdp/tests/*` + internally (the `lb-config` hits at `:352,363` are doc **comments**, not calls). `_xdp_loader` is **create-then-drop** (`main.rs:2592,3262`). Conntrack map is never populated → every packet is CT-miss → `XDP_PASS` → kernel stack → normal userspace path. In-kernel Maglev **deferred** (`ebpf/src/main.rs:23,239`). The XDP program runs as a **stats/no-op attach.** |
| G2 | **In-kernel Maglev** | UNREACHABLE (deferred) | not built/published; the only **live** Maglev is userspace **Mode-A passthrough** (`passthrough.rs:1165`) — matches memory |
| G3 | **9 of 11 LB algorithms** | **UNREACHABLE — not config-selectable** | **No `algorithm`/`policy`/`lb_*` key exists in `lb-config`** (grep clean; schema has only TLS-policy + header-underscore-policy). Binary hardwires `RoundRobin` (`main.rs:768,2031`) + `RoundRobinUpstreams` (`main.rs:1367,1524`); `LbPolicy`/`Cluster` used only in `lb-core` tests. ⚠️ confirms the S42 "11 algos NOT config-selectable" overclaim-fix, and sharpens it: only **RoundRobin** (+ Mode-A Maglev) is live. |
| G4 | **Active health checking** | **Not implemented** | "active probe loop is Wave-2 (REL-2-05)" — no loop exists `main.rs:2552-2577` |
| G5 | **Passive health checking** | **INERT — SHARPER THAN KNOWN** | seeded checkers bound to `let _health_seed = health_seed;` (`main.rs:2582`) and **never read**; `record_failure`/`record_success` have **no production caller** (grep clean). **Unhealthy backends are never ejected** — even passively. |
| G6 | **`weight` knob** | INERT | `RoundRobin::pick` = `counter % len` ignores weight (`round_robin.rs:24`); L7 `UpstreamBackend` has **no weight field** (`upstream.rs:54-63`) |
| G7 | **`header_underscore_policy`** | INERT | binary never calls `with_header_underscore_policy` (grep clean); proxies default `Reject` |
| G8 | **EWMA/P2C latency routing** | UNFED + un-instantiated | `Ewma` never built in prod; `set_latency_ns` called only in a `#[test]` (`lb-core/src/lib.rs:79`) → `latency_ewma_ns` stays 0 |
| G9 | **`[runtime].xdp_new_flow_cap_per_sec_per_cpu`** | INERT — **NEW** | `set_new_flow_cap` never called from the binary → SYN-flood cap never reaches the program |
| G10 | **`[runtime.tls].tls13_only`** | INERT/UNREACHABLE — **NEW** | binary hardcodes `with_safe_default_protocol_versions()` (TLS1.2+1.3); `tls13_only` has **zero reads** in `crates/lb/src/` (grep clean) — the tls13-aware builder has only a test caller |

**⚠️ DIVERGENCE:** the public docs market a **"High-Performance, Scalable, and Highly-Available Load
Balancer"** with **L4 XDP**. In the shipped binary: the **L4/XDP load-balancer is a no-op attach
(G1)**, **HA/health-based ejection does not happen (G4+G5)**, and **algorithm choice is not
user-selectable (G3)**. These are honest capability corrections beyond the 6 already fixed in S42
(G1, G5-sharpened, G9, G10 are **new** this assessment).

---

## 4. CARRY-FORWARD LEDGER  (every open item, consolidated)

No single ledger file exists in-repo; assembled from `audit/**` + memory. **None blocks a release on
core-HTTP-proxy grounds.** Sev = practical severity; "Blocks?" = blocks a first release/pilot.

### A. Dependency / supply-chain
| Code | What it is | Sev | Open? | Blocks? | Source |
|---|---|---|---|---|---|
| **RUSTSEC-2026-0185** | quinn-proto 0.11.14 OOM; dev-only, http3-off, **not compiled** | High upstream / **nil prod** | Yes (lockfile) | **No** — but reds CI R1 | `Cargo.lock:2541`; `cargo audit` |
| **RUSTSEC-2026-0190** | anyhow 1.0.102 `downcast_mut` unsoundness; direct dep, **denied** under `-D warnings` | Warn / low | Yes (untriaged) | **No** — but reds CI R1 | `cargo audit -D warnings` |
| CF-S37-D-TOKIO-1.52-RELAY | tokio held `<1.52` (H2→H3 relay ~10× collapse) | Perf | Yes | No | `Cargo.toml:73-78` |
| CF-S37-D-REQWEST-0.13 | reqwest held 0.12 (dev-only feature churn) | Low | Yes | No | `Cargo.toml:189-195` |
| CF-QUICHE-COLLECTED-UNBOUNDED | quiche `StreamMap::collected` insert-only set → sustained-H3 growth; **mitigated in-code** by H3 recycling | Med upstream / mitigated | Yes | No | `audit/soak/s32-report.md` |
| CF-QUICHE-UPGRADE | h3spec #1-10 transport + #23/#25 QPACK + §7.1 = documented quiche deviations, inert | Low | Yes | No | S22/S26 reports |
| CF-QUICHE-FRAME-COMPLETENESS | quiche doesn't enforce RFC9114 §7.1 DATA-completeness at FIN (no-CL truncation undetectable) | Low (malformed-backend only) | Yes | No | `audit/s25-logs` |
| CF-DEP-1 / Dependabot backlog | PRs #243 (group, CI-red), #238 (quiche 0.29.2), #237 (quiche/fuzz), #233 (actions) | Info | Yes | No | `gh pr list` |

### B. Protocol / conformance
| Code | What it is | Sev | Open? | Blocks? | Source |
|---|---|---|---|---|---|
| **CF-S27-2** | WS-over-H2 un-gate blocked; fix = **hyper#4050, merged ~2026-06-23 but UNRELEASED** (see §5) | DoS-class (inert while gated off) | Yes | No (feature off by default) | S30 memo; hyper#4050 |
| CF-S28-WSH3-WAKEUP | 2 ms busy-poll wakeup in WS-over-H3 pump | Low | Yes | No | S28 report |
| CF-S15-PASSTHROUGH-RETRY-ODCID | Mode-A Retry ODCID handling when `mint_retry=true` w/ real quiche backend | Low | Yes | No | `audit/soak/s21-report.md` |
| PROTO-2-12 (H3 leg) | H3 cross-bridge trailers not carried | Low | Yes | No | `audit/deferred.md:83` |
| PROTO-2-03 | 103 Early Hints / 1xx forwarding not wired | Low | Yes | No | `audit/deferred.md:120` |

### C. Reliability / operability
| Code | What it is | Sev | Open? | Blocks? | Source |
|---|---|---|---|---|---|
| CF-S39-H3-REJECT-LOG-SPAM | H3-terminate logs a WARN per rejected request (~24.8M lines/3.3G over 4h) — log amplification | Low | Yes | No | `audit/perf/s39-report.md` |
| CF-S38-QUIC-MAXCONN (F-RES-3) | QUIC `max_connections` hardcoded 100_000; no per-IP cap / config knob | Low | Yes | No (global cap exists) | `audit/security/s38-findings.md:239` |
| ROUND8-L7-08 | Upstream H2 RST_STREAM(CANCEL) on read-timeout; deferred to hyper-2.x | Low | Yes | No | `audit/deferred.md:154` |
| ROUND8-L7-07 (timer half) | Per-frame H2 arrival watchdog; deferred to hyper-2.x | Low | Yes | No | `audit/deferred.md:165` |
| **[NEW] G4/G5 health** | active health unimplemented **and** passive health inert (no ejection) | Med (operability) | Yes | No (RR still serves) | §3 G4/G5 |
| **[NEW] G1 XDP** | L4/XDP LB unreachable (no-op attach); skb userspace path works | Med (marketed feature) | Yes | No (HTTP path unaffected) | §3 G1 |

### D. CI / test-infra flakes (all isolation-proven non-regressions)
| Code | What it is | Open? | Blocks? |
|---|---|---|---|
| CF-FCAP1-FLAKE / CF-SATURATION-1 | H2-timeout / heavy-e2e saturation flakes; isolated+retry in CI | Yes | No |
| CF-S37-D6-H2PROXY-FLAKY | h2_proxy coverage oscillates around the 80% floor (79.60% ↔ ≥80%) | Yes | No (but reds CI R2) |
| CF-S35-T5-FLAKE / CF-S38-RELOAD-BOOT-FLAKE | t5 throughput + reload-boot race under load | Yes | No |
| CF-DISK-1 | full `--all-features` test build ~40GB (ENOSPC risk) | Yes | No |
| F-ESC-1 | multi-kernel XDP verifier CI lane never stood up (single-kernel proven) | Yes | No |

### E. Infra / deployment (from `audit/deferred.md`)
| Item | What it is | Open? | Blocks? |
|---|---|---|---|
| **D-1 native XDP on ENA** | aya `bpf_link` DRV attach fails on ENA; needs netlink-XDP fallback + 3-kernel matrix | **Yes (open)** | Blocks native-XDP-on-ENA path only; skb works |
| D-2 bpftool literal path | aya-ebpf legacy `maps` section refused by libbpf-1.0+ | Yes | No (aya loader works) |
| D-5 / CVE-2026-0861 waiver | `.trivyignore` waives a glibc CVE (Debian shipped no FixedVersion) | Yes (documented) | No |

**Recently CLOSED — do NOT re-chase:** CF-GRPC-H3-CHURN-RSS (S36, our-code-fixed via H3 recycling);
CF-S37-SC9-PLATEAU (S39, fragmentation not leak); F-RES-1 + F-INFRA-01 (S38 fixed).

---

## 5. DEPS + HELD-CLUSTER + hyper#4050

**Locked vs latest** (crates.io `max_stable_version`, 2026-07-04) **[V]**:

| Crate | Locked | Latest | Action |
|---|---|---|---|
| quiche | 0.29.1 | **0.29.2** | Safe, non-urgent — 0.29.2 CVEs are **C-FFI-only** (we use the Rust API, `ffi` feature off). PR #238. |
| tokio-quiche | 0.19.0 | 0.19.0 | current |
| hyper | 1.10.1 | 1.10.1 | current — but the WS-H2 fix is post-1.10.1 (below) |
| h2 | 0.4.14 | **0.4.15** | patch bump available (in PR #243) |
| tokio | **1.51.1** (HELD `<1.52`) | 1.52.3 | **hold applies** (CF-S37-D). Free win: **1.51.3 within the held band** (locked is 1.51.1) |
| rustls | 0.23.40 | **0.23.41** | patch bump available |
| tokio-tungstenite | 0.29.0 | 0.29.0 | current |
| prometheus | **direct 0.14.0** / transitive 0.13.4 | 0.14.0 | **already latest (direct)** — see divergence |
| foundations | 4.5.0 (HELD-by-upstream) | 5.7.4 | blocked: quiche 0.29 → qlog 0.18 → foundations ^4; can't move to 5.x until quiche/qlog do |

**⚠️ DIVERGENCE (deps):** memory/ground-truth says **"prometheus held at 0.13.4 (0.14 dropped)."**
Reality: `Cargo.toml:146` declares `prometheus = "0.14"`, resolving to **0.14.0** as the direct dep;
0.13.4 exists **only** as a foundations-4.5.0 transitive pin (dual-versioned, `deny multiple-versions
= warn`) **[V]**. The S33 "0.14 dropped" note **did not persist** — the direct dep is already on latest.

**Held cluster — why:** tokio held `<1.52` (S37-D measured 1.52.x collapsing H2→H3 relay ~10×;
`h2h3_fcap1` forwards ~30 MiB/60s on 1.52.3 vs 64 MiB/5.7s on 1.51.1) **[V, hold still applies;
regression-through-1.52.3 = I]**. foundations held-by-upstream via quiche/qlog **[V]**. reqwest held
0.12 (dev-only) **[V]**.

**hyper#4050 (CF-S27-2 / WS-H2 backpressure):** **merged upstream ~2026-06-23 but NOT in any released
crate** **[V page + I dates]**. The bug (H2 Extended-CONNECT tunnel writing on a `Pending`
`poll_capacity` → unbounded buffering) was introduced in hyper 1.8.0; the fix + follow-up #4102 are on
`master`. hyper **1.10.0 (2026-05-27) and 1.10.1 (2026-05-29) both predate the merge**, and crates.io
shows no release after 1.10.1. **∴ the locked hyper 1.10.1 does NOT contain the fix; WS-H2 must stay
gated OFF until hyper cuts a release > 1.10.1.** This is the one carry-forward with a **concrete,
imminent unblock trigger** — watch for the next hyper release.

**Low-risk patch bumps available now (no held-cluster conflict):** rustls 0.23.40→.41, h2 0.4.14→.15,
quiche 0.29.1→.2, tokio 1.51.1→1.51.3.

---

## 6. REPO / DOCS / DX / CI STRUCTURE

- **CI structure (post-S40):** 3 workflows — `ci.yml` (16 jobs), `release.yml` (soak-gate + publish),
  `scheduled.yml` (geiger/machete/weekly-audit). Clean; the gate-map holds (0 gates dropped) **[V]**.
- **Doc set (post-S42):** substantial and release-shaped — `docs/guide/` (14 files incl overview,
  getting-started, capabilities, comparison, PERFORMANCE, cookbook, troubleshooting, deployment-patterns,
  observability, RUNBOOK, CONFIG, DEPLOYMENT, METRICS), `docs/arch/` (8), `docs/decisions/` (ADRs),
  `docs/research/`, plus top-level `features.md` / `known-limitations.md` / `glossary.md` **[V]**.
- **⚠️ `docs/architecture.md` is NOT stale** (the task flagged it as possibly-old): it was updated in
  S42 (`ff3a8858`), is the canonical **crate-map developer reference**, and is linked from 5 docs
  (arch/overview, arch/extending, arch/README, guide/README, README) **[V]**. No action.
- **⚠️ Docs-vs-reality debt:** the reachability gaps in §3 (XDP no-op, health inert, algos
  not-selectable, 3 new inert knobs) mean the capability docs still **overstate L4/HA**. This is the
  S42 "6 overclaims" pattern recurring — a doc/reachability re-pass is warranted (G1/G5/G9/G10 are new).
- **DX:** `docs/arch/DEV-SETUP.md` present; positional-arg CLI; `eg-bench` harness (S39).
- **Branch state:** working tree clean except **2 untracked files** (`audit/soak/.../release-soak-summary.txt`
  ×2 — harmless S40-era soak artifacts) **[V]**. **17 stale local branches** (s6/s7/s8/feature-* — squash-repo,
  so "ahead" ≠ unmerged) and remote dependabot branches + `origin/old-main` (pre-migration snapshot) —
  housekeeping only, not release-blocking **[V]**.
- **18 crates** confirmed (`crates/`); lb-h3 deleted (S26), lb-h3-testcodec retained **[V]**.

---

## 7. THE GAP LIST  (now → each milestone)

**NOW → "main honest-green":**
1. **Triage the two advisories (R1/R3).** Add RUSTSEC-2026-0185 + RUSTSEC-2026-0190 to `deny.toml`
   **and** `.cargo/audit.toml` with rationale (0185 = dev-only/http3-off/not-compiled; 0190 =
   unsoundness, path unexercised), **or** `cargo update -p quinn-proto`→0.11.16 + bump/ignore anyhow.
   Clears Security Audit **and** Scheduled Scans. → **deps session.**
2. **Lift h2_proxy off the 80% boundary (R2).** Add a real test to `lb-l7/src/h2_proxy.rs` so
   coverage isn't decided by ~0.4% jitter; and/or further harden the fcap1 isolation. → **code session.**

**→ "first release cuttable":**
3. **Arm branch protection / a ruleset** on `main` with the real 16 job names as required checks
   (currently **none** enforced).
4. **Configure the `SOAK_*` secrets/vars** (`SOAK_AWS_ROLE_ARN` + 6 vars) and **dry-run the release
   soak-gate** (`workflow_dispatch`) to prove it provisions→soaks→verdicts→tears-down.
5. Optionally take the safe patch bumps (§5) so the first tag ships current deps.

**→ "production pilot":** (beyond docs/perf/security, which are done)
6. **Health-based ejection (G4/G5).** Decide whether a pilot can ship with RoundRobin-only + no
   health ejection, or wire the seeded passive checkers into the pickers first. This is the biggest
   *operability* gap for a real LB pilot.
7. **Decide the XDP story (G1).** Either (a) wire the userspace conntrack/backend publication so the
   L4/XDP LB is actually reachable, or (b) **honestly re-scope the docs** to "L4 XDP = attach/stats,
   LB deferred" and market the HTTP proxy as the product. Today the marketed L4 LB is a no-op.
8. Per-IP QUIC connection cap (CF-S38-QUIC-MAXCONN) + H3-reject log-spam (CF-S39) — small operability
   hardening for a hostile-internet pilot.

**→ "deferred nice-to-haves":**
9. Config-selectable LB algorithms (G3) + weight (G6) + EWMA feed (G8) — unlock the 9 dormant algos.
10. Un-gate WS-H2 when hyper > 1.10.1 ships #4050 (CF-S27-2).
11. In-kernel Maglev (G2), native-XDP-on-ENA (D-1), multi-kernel verifier lane (F-ESC-1), 103 Early
    Hints (PROTO-2-03), H3 trailers cross-bridge (PROTO-2-12), tls13_only wiring (G10), header_underscore
    wiring (G7), xdp_new_flow_cap wiring (G9).

---

## 8. RECOMMENDED NEXT-WORK ORDERING (prioritized punch-list)

1. **Deps/CI-honesty session (small, high-value):** triage the 2 advisories + take the safe patch
   bumps → clears R1+R3 and closes half the deps ledger. Then add the h2_proxy test → clears R2.
   **Outcome: main goes honest-green.** *(Do first — it's the cheapest path to a truthful CI and
   unblocks everything downstream.)*
2. **Release-arming session:** branch protection/ruleset with the real check names + `SOAK_*`
   secrets + a soak-gate dry-run. **Outcome: first release becomes cuttable.**
3. **Capability-honesty doc re-pass:** fold §3 (XDP no-op, health inert, algos-not-selectable, 3 new
   inert knobs) into the docs — the S42 overclaim pattern is still live. **Outcome: docs match code.**
   *(Cheap; prevents the next "docs proven wrong" cycle.)*
4. **Operability session (for pilot):** wire passive health→picker ejection (G5), then decide the XDP
   scope (G1). **Outcome: a defensible HA-LB pilot.**
5. Everything else = nice-to-haves, sequence by demand (WS-H2 gated on hyper release; LB-algo
   selection; native XDP).

**Why this order:** #1 is small and makes the CI *tell the truth*, which every later gate depends on;
#2 arms the release machinery (pure infra, no code risk); #3 is a cheap doc pass that stops the
recurring overclaim problem; #4 is the only item touching production behavior and is the real gate for
a *load-balancer* pilot (vs an HTTP-proxy pilot). The product's HTTP core is frozen-stable and needs
no further work to ship.

---

### Verification provenance
- CI red-causes, branch-protection, release-secrets, `cargo audit -D warnings`, prometheus
  dual-version, XDP/health/tls13 reachability greps: **lead-verified directly this session** (`gh`,
  `cargo audit`, `git`, grep). 
- Feature reachability map, deps-vs-latest, hyper#4050 timing, carry-forward ledger: **assessor
  passes, key claims lead-re-verified** (XDP no-op, passive-health inert, LB-algo non-selectable,
  advisory count, prometheus, source-frozen-since-S38 all independently re-run).
- **[I]-tagged** items (regression-persists-through-tokio-1.52.3; reqwest http3-feature layout;
  hyper release dates) were not independently re-executed and are marked inferred.
