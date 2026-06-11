# S41 — Session B: Public-facing documentation

**Goal:** make ExpressGateway *approachable* — write the user/operator and
developer/technical narrative docs that let someone evaluate and understand the
project (the envoy/traefik/HAProxy-style "what is this / how does it work" docs).

**Cardinal rule (docs discipline):** every claim is GROUNDED in verified reality
+ the audit trail. No aspirational or marketing claims. Limitations stated
plainly. Perf numbers from the S39 baseline with their conditions. Security
posture from the S38 audit, honestly.

**Base:** `main` tip `50893462` (S40 report finalization; `2047130a` = the S40
merge, confirmed ancestor). Branch `feature/public-docs-s41`. **No production
source changed.**

**Box:** 2 cores, 7.7 GiB RAM, 32 GiB free (≥15 GB ✓). Writing/synthesis only —
no compile-under-load/soak/perf.

---

## Phase 0 — ground truth gathered (source-of-truth, with citations)

Four independent read-only sweeps + direct reads established the verified facts
the docs will cite. Summary of the source-of-truth (full citations carried into
each doc and re-verified in Phase 3):

### Config schema (`crates/lb-config/src/lib.rs`)
- Top-level: `[[listeners]]`, `[runtime]`, `[observability]`, `[admin]`,
  `[security]`, `[passthrough]`. `#[serde(deny_unknown_fields)]`.
- Key defaults (file:line in lib.rs): `drain_timeout_ms`=10000, `max_keepalive_requests`=100,
  `max_requests_per_h3_connection`=1000, `max_quic_connections`=100000,
  `mint_retry`=true, `tls13_only`=false, `xdp_enabled`=false,
  `per_ip_connection_cap`=1024, `handshake_timeout_ms`=5000,
  HTTP timeouts header=10000/body=30000/total=60000/head=60000,
  websocket `h2_extended_connect`=false / `h3_extended_connect`=false,
  grpc `max_deadline_seconds`=300, backend `protocol`="tcp"/`weight`=1/`tls_verify_peer`=true.
- CLI: config path is **positional arg #1**; no `--config` flag; defaults to `config/default.toml`.
- SIGHUP reload (`crates/lb-config/src/reload.rs`): swappable = backends, HTTP
  timeouts, h2_security, websocket (except h3_extended_connect), alt_svc, grpc,
  `max_keepalive_requests`. Restart-required changes logged + counted, never
  silently applied (honesty contract). SIGUSR1 = TLS cert rotation.
- 9 example configs in `config/examples/`: tcp, tls, h1, h1s, h1s-grpc,
  h1s-websocket, quic-h3, quic-mode-b, passthrough-mode-a.

### Architecture (crates)
- 18 crates. Binary `lb` (artifact `expressgateway`). Production wire parsing is
  **delegated**: H1=hyper, H2=hyper/h2, H3+QUIC=quiche/BoringSSL, TLS=rustls, WS=tungstenite.
- `lb-h1`/`lb-h2`/`lb-h3-testcodec` = test codecs + security-detector types (NOT live parsers).
- `lb-quic` = real QUIC/H3 (quiche 0.29): H3-terminate (`conn_actor.rs`, `h3_bridge.rs`),
  Mode A passthrough (`passthrough.rs`, `public_header.rs`), Mode B raw proxy (`raw_proxy.rs`).
- `lb-l4-xdp` = real XDP/eBPF (compiled ELF `crates/lb-l4-xdp/src/lb_xdp.bin` + aya loader);
  off by default; validated live on Linux 7.0 native ENA xdpdrv.
- `lb-l7` = 9 protocol bridges; `lb-balancer` = 11 algorithms; `lb-security` = DoS detectors.
- H3 recycling: cap (`max_requests_per_h3_connection`, default 1000) → GOAWAY (0x0100) →
  reject new streams (0x010b) → client reopens. (`conn_actor.rs`.)
- R8 bounded relay: 64 MiB req/resp caps + bounded in-flight channel (~64 KiB) independent
  of body size; backpressure by not reading until the channel drains. (`h1_proxy.rs`,
  `h2_proxy.rs`, `h3_bridge.rs`, `raw_proxy.rs`.)

### S39 perf baseline (`audit/perf/s39-perf-baseline.md`, `s39-burnin.md`, `s39-report.md`)
- Box: AWS c6a.2xlarge, 8 vCPU, 15 GiB RAM, co-located loopback (client+gateway+backend
  on one box). 5s warmup + 15s window. Harness `eg-bench` + `oha` cross-validation.
- Efficiency (gateway CPU-µs/req, lower=better): **WS ~37 < H2 ~59 < H3 ~101 < H1 ~163**.
- Peak RPS on the co-located box (approximate, ±20–30% at the knee): WS ~42.5k, H2 ~32k,
  H3 ~18.5k, H1 ~14–18k. QUIC Mode A passthrough = harness-bound (~1.65k, single-task echo
  backend) → honest gateway result = ~0.6 ms added latency + ~11% CPU (gateway idle).
- 4-hour burn-in: 11/12 BOUNDED, panic=0 over billions of ops; R8 held (320M H2 rapid-resets,
  49M H3 reset-floods, 261M conn-floods bounded). 1 DRIFT (sc3 slowloris) = analyzer
  false-positive, isolated recheck BOUNDED.
- Deferred perf tiers (NOT characterized): io_uring, XDP offload throughput. Numbers are
  honest current-state on a shared box, not a tuned single-purpose rig.

### S38 security posture (`audit/security/s38-findings*.md`, `SECURITY.md`)
- Verdict: **0 Critical / 0 High / 1 Medium / 7 Low / 4 Info**. No product fork, no
  dependency-implicating finding.
- Wire parsing delegated (above). Only hand-rolled production parser = `lb_quic::public_header`
  (Mode A, every datagram): panic-free by construction (`#![deny(...)]`, `.get()`-checked),
  fuzzed **~670M iterations, 0 crashes**. Full fuzz campaign 9 targets ~1.03B units, 0 crashes/OOMs.
- Fixed: F-RES-1 (Medium — H1 slowloris header timeout was inert / no `.timer()`; now wired),
  F-INFRA-01 (Low — retry-secret file load now perm-checked), F-RES-2 (Low — H2 client
  max_header_list_size), F-PARSE-3/F-RES-4 (comment/doc). Negative-control test per fix (R13).
- Accepted carry-forward: F-RES-3 (no per-source-IP QUIC sub-cap; bounded by global cap +
  Retry address validation), F-PROTO-01..04 (defence-in-depth / binary-encoding-moot),
  no-mTLS (intentional), TLS 1.2 default (downgrade-safe ECDHE+AEAD only).
- DoS catalog enforced + tested: Rapid-Reset (CVE-2023-44487), CONTINUATION flood
  (CVE-2024-27316), HPACK/QPACK bomb, SETTINGS/PING flood, zero-window stall, slowloris,
  slow-POST, CL.TE/TE.CL/H2-downgrade smuggling, QUIC 0-RTT replay, panic-as-DoS (panic-free).
- TLS: rustls+BoringSSL, TLS 1.2+1.3 default (downgrade-safe), `tls13_only` opt-in, no
  server-side mTLS (intentional), upstream verification enforced (H3 default + Mode B always).

### Conformance
- h2spec **147/147** (`tests/h2spec.rs`). h3spec passes with **12 named waivers**
  (`scripts/ci/h3spec-check.sh`, CF-QUICHE-UPGRADE) — quiche-0.29 transport deviations +
  inert QPACK uni-stream items; a new failure outside the list fails CI.

### Existing canonical docs (build ON these; do not duplicate — R12)
- Reference docs already written (Session A): `docs/guide/CONFIG.md`, `DEPLOYMENT.md`,
  `RUNBOOK.md`, `METRICS.md`; `docs/features.md`, `docs/known-limitations.md`,
  `docs/architecture.md` (partly stale — has a status note), `docs/edge-defaults.md`;
  `SECURITY.md`; ADRs under `docs/decisions/`; research under `docs/research/`;
  `docs/arch/DEV-SETUP.md`. The two index READMEs (`docs/guide/README.md`,
  `docs/arch/README.md`) reserve the narrative pages for this session.

### doc-lint gate (`scripts/ci/doc-lint.sh`)
- Tier-1 greps a `FILES=()` array of operator-facing docs for stale patterns (no
  `lb-compression`, no `/usr/local/bin/lb`, no `target/release/lb`, no
  `cargo build --release -p lb` without `--bin expressgateway`, no FD-passing /
  zero-downtime-via-reload claims). New operator-facing docs to be **added to FILES**.
  Binary is `expressgateway`; build = `cargo build --release -p lb --bin expressgateway`.

---

## The DOC PLAN (structure → audience → key claims → sources)

> Principle: the new pages are the **narrative/approachable layer**. Reference
> docs (CONFIG/DEPLOYMENT/RUNBOOK/METRICS/features/known-limitations/SECURITY)
> stay the single source of truth (R12); narrative pages summarize + link, they
> do not re-document.

### docs/guide/ — user / operator ("Should I use this, and how?")

| Doc | Audience | Key claims | Sources |
|-----|----------|-----------|---------|
| `overview.md` | Evaluator/operator | What ExpressGateway is (Rust L4+L7 LB), the problem it solves, where it fits, headline capabilities, honest 1-paragraph positioning. | README, features.md, S39, SECURITY |
| `getting-started.md` | New user | Prereqs → `cargo build --release -p lb --bin expressgateway` → pick `config/examples/*` → run (`expressgateway <cfg>`) → serve a request → `/metrics`. RUN IT in Phase 3. | README quickstart, config/examples, DEPLOYMENT |
| `capabilities.md` | Evaluator | Consolidated at-a-glance: supported (9 cells, QUIC A/B, WS H1/H3, gRPC-H3) / gated (WS-H2) / waived (12 h3spec) / deferred (perf tiers, distributed CP). Links features.md + known-limitations.md as canonical. | features.md, known-limitations.md |
| `comparison.md` | Evaluator | FACTUAL positioning vs envoy/traefik/HAProxy/nginx: protocol coverage, Rust memory-safety, L4+L7 in one binary, the perf baseline. Comparison, not trash-talk (R7). | features.md, S39, docs/research/* |
| `PERFORMANCE.md` | Evaluator/operator | THE canonical perf doc: S39 baseline numbers WITH box/load/caveats; efficiency ordering; 4h burn-in bounded-memory; deferred tiers stated. | audit/perf/s39-* |

### docs/arch/ — developer / technical ("How does it actually work?")

| Doc | Audience | Key claims | Sources |
|-----|----------|-----------|---------|
| `overview.md` | Developer | Current accurate architecture: 18 crates, L4-XDP + L7 split, the data path; delegated parsers; supersedes stale framing in docs/architecture.md (whose own status-note agrees). | crate sweep, architecture.md status-note |
| `protocol-model.md` | Developer | 9-cell front×back matrix; termination/re-origination; quiche::h3 H3 stack; gRPC-over-H3 (+ H1-front limitation); WS over H1/H2/H3. | features.md, lb-l7, lb-quic, known-limitations |
| `quic-modes.md` | Developer | Mode A passthrough (route-by-CID, no decrypt, public_header) vs Mode B (terminate + re-originate); H3 connection lifecycle + recycling (cap→GOAWAY→drain). | passthrough.rs, raw_proxy.rs, conn_actor.rs, features.md |
| `backpressure.md` | Developer | R8 bounded-relay: 64 MiB caps, bounded in-flight channel independent of body size, read-pause backpressure; proven under soak. | h1/h2_proxy.rs, h3_bridge.rs, raw_proxy.rs, S39 burn-in |
| `security-and-conformance.md` | Developer | Delegated parsers + fuzzing (670M Mode-A, 1.03B total, 0 crashes) + panic-freedom; S38 verdict (0C/0H/1M/7L/4I); h2spec 147/147; h3spec 12 waivers. Links SECURITY.md as canonical. | s38-findings*, SECURITY.md, h3spec-check.sh, tests/h2spec.rs |
| `/CONTRIBUTING.md` (root) | Contributor | How to build/test/run the gates locally (cross-link DEV-SETUP), panic-free rule, the audit trail, PR expectations. | DEV-SETUP.md, scripts/ci/* |

### Wiring updates (no new claims, just navigation)
- `docs/guide/README.md` + `docs/arch/README.md`: add the new pages to their tables.
- Top-level `README.md`: add a "Start here" pointer to `docs/guide/overview.md` +
  `docs/arch/overview.md`.
- `scripts/ci/doc-lint.sh` `FILES=()`: add the new operator-facing guide pages.

### Out of scope (carry-forward, not docs)
Production pilot rollout, owner action items (branch-protection rename, soak
secrets, 2 Dependabot advisories), optional perf tiers (io_uring/XDP).

---

## Framing decisions for owner check-in

1. **Positioning vs envoy/traefik/HAProxy (R7 judgment call).** Proposed: a
   factual capability/protocol comparison table + an honest "where ExpressGateway
   fits / where the mature incumbents are stronger (ecosystem, control-plane
   maturity, battle-tested scale)" paragraph. No "faster than X" / "better than X"
   claims — only measured facts and protocol coverage. Mature competitors'
   strengths stated plainly.

2. **Limitations prominence (R7 product decision).** Proposed: limitations are
   first-class, not buried — a prominent "Capabilities & limitations" page in the
   guide, the gated/waived/deferred items called out in `overview.md` itself, and
   every narrative page that touches a limited feature links the canonical
   `known-limitations.md`. WS-H2-gated, gRPC-needs-H2/H3-backend, Mode-A-Retry,
   WS-H3-opt-in, the 12 h3spec waivers, and the deferred perf tiers are all named.

3. **Perf numbers honesty.** Proposed: publish the S39 figures with the box spec
   ("co-located 8-core box", shared client+gateway+backend), the ±20–30% knee
   variance, the harness-bound Mode-A caveat, and the explicit "deferred tiers not
   characterized" note. No rounding-up, no single-number headline RPS without
   conditions.

---

## Phase 1 + 2 — docs written (11 pages, ~1725 lines)

Two writer agents (author≠verifier) produced, grounded in the Phase-0 ground truth:
- **docs/guide/**: `overview.md`, `getting-started.md`, `capabilities.md`,
  `comparison.md`, `PERFORMANCE.md`.
- **docs/arch/**: `overview.md`, `protocol-model.md`, `quic-modes.md`,
  `backpressure.md`, `security-and-conformance.md`; plus root `CONTRIBUTING.md`.

Wiring (navigation only): `docs/guide/README.md` + `docs/arch/README.md` tables,
top `README.md` "Start here" + Documentation pointers, and the
`scripts/ci/doc-lint.sh` `FILES=()` array (new operator docs added for
stale-pattern protection).

## Phase 3 — fact-check + validate + promote

### Independent fact-check (the "is this TRUE?" pass, R15)
A third agent independently re-derived **every** non-obvious claim from source
(code/config/tests/audit/S39), not from the writers' ledgers. **Verdict: no
BLOCKERS — zero false capability/perf/security claims.** All S39 perf numbers
exact-match `audit/perf/s39-perf-baseline.md`/`s39-burnin.md` with their
conditions; S38 security verdict (0C/0H/1M/7L/4I), the ~670M/1.03B fuzz counts,
delegated-parser set, h2spec 147/147, h3spec **exactly 12** waivers, and all
cited config defaults verified against `crates/lb-config/src/lib.rs`. doc-lint
stale-pattern grep: clean across all 11 docs. All relative links resolve; all 9
example configs parse.

Three WARNINGS found and FIXED:
- **W3 (EWMA).** The latency-weighted `ewma` policy is implemented and selectable,
  but the request path does not feed per-request backend latency into the
  balancer in this build (the latency setter `lb-core/src/backend.rs:162` is
  called only under `#[cfg(test)]`), so `ewma` scores on its load/cold-start
  path, not measured latency. Added an honest caveat to `docs/arch/overview.md`
  and `docs/guide/overview.md` (claim was not false — `ewma` IS a real,
  selectable algorithm — but an evaluator picking it deserved the nuance).
- **W2.** `security-and-conformance.md` cited the F-RES-1 fix at
  `h1_proxy.rs:684` (actual ~`:695`). Dropped the rot-prone line number, kept the
  file reference.
- **W1.** `comparison.md` intro implied all competitor facts came from
  `docs/research/`, but Traefik has no in-repo research file. Reworded the
  sourcing: Envoy/HAProxy/nginx/Katran draw on the in-repo research; Traefik and
  fast-moving details are summarized from each project's current documentation.
  (The Traefik table claims themselves were verified accurate.)

### Examples validated (R5 — run where feasible)
- **Build:** `cargo build --release -p lb --bin expressgateway` → `Finished` in
  11m54s, exit 0, produced `target/release/expressgateway` (real ELF).
- **Quickstart boot test (docs/guide/getting-started.md):** booted
  `expressgateway config/examples/h1.toml` against a `127.0.0.1:3000` backend,
  `curl http://127.0.0.1:8080/` → **HTTP 200, 2676-byte body proxied
  end-to-end**. Clean boot log (config loaded, file-backed control plane ready,
  io_uring runtime, TCP pool). Processes reaped (R9).
- **doc-lint:** `./scripts/ci/doc-lint.sh` → tier-1 OK (incl. the new docs),
  tier-2 OK (52 audit-of-audit claims). GREEN.

### Promote
No production source touched (docs + `doc-lint.sh` FILES array + README pointers
only). **CI-confirmation was waived per owner directive ("ignore CI and
builds")**; promotion rests on local doc-lint green + the change being
docs-only. Merged to `main` `--no-ff`.

**Result: SESSION B COMPLETE** — public guide + arch docs written, every claim
fact-checked-and-sourced, quickstart validated end-to-end, doc-lint green. The
project is now release-friendly: maintainable (Session A) + approachable
(Session B).
