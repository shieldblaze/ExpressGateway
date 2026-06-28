# S42 Docs Revision — CONTEXT PACK (read this first, every writer)

You are a writer on the ExpressGateway S42 documentation-revision team. The docs are
factually correct but HOLLOW (depth + structure problem) and contain FIVE verified
overclaims. This session makes them release-grade. Read this pack fully before writing.

**Three source reports — READ the ones relevant to your scope:**
- `audit/release/doc-analysis.md` — the diagnostic (what's wrong, per-page scorecard).
- `audit/release/s42-corrected-claims.md` — code-grounded overclaim corrections (file:line).
- `audit/release/s42-doc-benchmark.md` — incumbent doc IA + patterns-to-adopt + page-by-page map.

## OWNER-APPROVED DECISIONS (non-negotiable)
1. **Honest reframe** of the marquee (see CORRECTED CLAIMS below). Lead with the 4 verified
   pillars. Recast LB/health/topology under Implemented/Partial/Deferred framing. Do NOT delete
   real-but-unwired info; do NOT patch-in-place keeping the overclaiming structure.
2. **Developer docs = practical contributor path** (crate map + dep diagram, how-to-extend, the
   build/test/gate dev loop, audit-trail map).
3. **Container quickstart = static + CI-backed** (mirror `scripts/ci/docker-smoke.sh`; cite the
   `docker-smoke` CI gate as boot-evidence; no local docker on the writing box).
4. **HIGHEST PRIORITY (owner): the config-doc lies.** `weight` and `header_underscore_policy`
   drop/allow are PARSED-BUT-INERT but documented in CONFIG.md + example configs as if live.
   "A lying config is worse than a lying headline." These MUST be corrected.

## CORRECTED CLAIMS — exact reality (use this wording; cite nothing you can't ground)
The canonical wording below is APPROVED. Use it verbatim (or tighter) wherever the topic appears.

- **Load balancing.** "Round-robin (L7 HTTP and raw-TCP) is the live selection policy today;
  QUIC Mode-A passthrough additionally uses Maglev hashing over the Connection ID. Ten further
  algorithms (weighted round-robin, P2C, ring-hash, EWMA, least-connections, least-request,
  random, weighted-random, session-affinity, and Maglev-for-L7) are implemented in the
  `lb-balancer` library but are **not yet selectable via configuration** (there is no policy key;
  the config schema rejects unknown keys). EWMA's latency input is fed only in tests."
  → CANONICAL HOME: `docs/features.md` (new "Load balancing" section). Everyone else: ≤1-line
  gloss + link. NEVER write "11 algorithms" as a marquee capability or "selectable".

- **Health.** "ExpressGateway implements **passive** per-backend health tracking
  (consecutive-success/failure state). In this build it is **not yet wired into live backend
  selection** (the balancer does not consult it), and **active probing** (interval/path/
  expected-status) is **deferred (REL-2-05)**." → CANONICAL HOME: `docs/features.md` (LB section)
  + `docs/known-limitations.md`. NEVER write "active health checks" or "TTL result cache".

- **Topology / HA.** "ExpressGateway is **stateless** — run N independent instances behind any
  L4/L3 load balancer to scale horizontally. `SO_REUSEPORT` lets a supervisor run a replacement
  process side-by-side for handover. There is **no built-in clustering, failover, or state
  sharing**; the control-plane `HaPoller` is a future seam, not wired into the binary."
  → CANONICAL HOME: new `docs/guide/deployment-patterns.md`. NEVER write "HA modes" as a feature.

- **`weight` (per-backend).** PARSED BUT INERT — no weighted picker is wired; round-robin and
  Maglev-by-CID ignore it. In CONFIG.md + every example: mark "accepted but not yet enforced
  (round-robin ignores weight in this build)". Do NOT present weight as a working knob.
  getting-started.md must not imply weighted selection.

- **`header_underscore_policy` drop/allow.** INERT — only the default `reject` is active; the
  proxies never apply drop/allow. In CONFIG.md: document that only `reject` is enforced today;
  drop/allow are accepted but not yet wired.

- **The 4 VERIFIED PILLARS to lead the marquee with:** (1) full **9-cell HTTP matrix** with
  bounded-memory streaming; (2) **QUIC passthrough (Mode A) + terminate (Mode B)**; (3) the
  hand-audited **DoS-mitigation catalog** (S38: 0C/0H/1M-fixed/7L/4I); (4) **panic-free
  libraries** (CI-enforced). These are all reachable-as-described.

- **CONFIRMED REACHABLE (state plainly, no hedging):** 9-cell matrix, L4/XDP (off by default,
  `xdp_enabled`), QUIC Mode A/B, gRPC (H2/H3 front), WS-H1 (default)/H3 (opt-in)/H2 (gated),
  TLS + tls13_only, `alt_svc` (advertise H3 from H1/H1s), `per_ip_connection_cap`,
  `max_inflight_connections`, `LB_LOG_FORMAT` env, `quic-passthrough-only` feature, SIGHUP/
  SIGUSR1/SIGTERM. Compression is correctly ABSENT (pass-through only).

## ONE HOME PER FACT (R6/R12) — canonical homes + everyone-else-links
| Fact | CANONICAL HOME (owns the prose) | Everyone else |
|---|---|---|
| 9-cell front×back matrix | `docs/features.md` "Protocol matrix" | ≤1-line + link |
| LB/health reality | `docs/features.md` "Load balancing" | ≤1-line + link |
| Panic-free + delegated-parsers | `docs/arch/overview.md` "parsing is delegated" / panic model | link, don't restate |
| Limitations set (gRPC-front, WS-H2 gate, Mode-A retry, h3spec waivers, no-mTLS, XDP) | `docs/known-limitations.md` (fold in capabilities.md's "who-it-affects") | ≤1-line + link |
| S38 verdict | `SECURITY.md` | link |
| Perf caveat / numbers | `docs/guide/PERFORMANCE.md` | link |
| Topology / HA-honest | `docs/guide/deployment-patterns.md` | link |
| Glossary terms | `docs/glossary.md` | link first use |
RULE: a reader must never meet the same paragraph twice. If your page needs a fact owned
elsewhere, write ONE sentence of gloss and link the canonical home — do not re-explain it.

## TEACHING BAR (what "good" looks like — match these)
- The model page is `docs/guide/PERFORMANCE.md` (0.5 links/100w; owns its material; states
  conditions; teaches how to read the data). `comparison.md` and `arch/quic-modes.md` also teach.
- A narrative page EARNS its place by giving what the reference can't: a mental model, a decision
  framework, a worked end-to-end example, a diagram. NOT a table of links.
- Benchmark patterns to apply (detail in s42-doc-benchmark.md): Envoy "Life of a Request"
  (grounded config → numbered steps → one inline diagram per step → interface→behavior→
  consequence); Traefik container-first quickstart with a visible curl-200 payoff; nginx/HAProxy
  cookbook with a "Combined Configuration Example" capstone + a verify step; Caddy
  design-intent-before-features + decision aids; one captioned diagram to introduce each concept.
- DROP the tics: no "honesty" meta-narration ("Nothing here is aspirational", "the headline you
  can trust", "Read this first", "honesty contract"). State the fact and the limit plainly.
- MOVE internal codenames out of user/operator pages: NO R8/R3/CF-S27-2/F-RES-3/S36/S38/
  ROUND8-* in guide/* user pages (developer/arch pages + the S38 verdict-by-name in SECURITY are
  OK). Describe the behavior; link the audit trail only where it adds trust.

## DIAGRAMS — mermaid fenced blocks (```mermaid), GitHub-native render. Each gets a one-line
caption. Author inline in your page; must be ACCURATE TO CODE. Specs (per page) are in your
individual prompt. The lead + fact-checker verify every diagram vs code afterward.

## HARD RULES
- **NO production source changes.** Docs + diagrams + config EXAMPLES + comments only. (You may
  edit `config/examples/*.toml` COMMENTS and `docs/**`, `README.md`, `CONTRIBUTING.md`,
  `SECURITY.md`. You may NOT edit anything under `crates/`.) If a doc fix seems to need a code
  change, DON'T — document reality and flag it to the lead.
- **NO git.** Do not commit, push, stash, checkout, or branch. The LEAD owns all git. Just edit
  files and report what you changed.
- **doc-lint** (`scripts/ci/doc-lint.sh`) greps a FILES[] list for stale patterns. NEVER write:
  `lb-compression`, `/usr/local/bin/lb` (binary is `expressgateway`), `target/release/lb`,
  `cargo build --release -p lb` without `--bin expressgateway`, "FD-passing", "zero-downtime …
  reload/FD". If you CREATE a new public doc, tell the lead so it's added to doc-lint FILES[].
- **Every claim grounded.** Cite code/config/test/audit/PERFORMANCE where non-obvious. If you
  can't ground it, don't write it. Do not regress the strong pages (PERFORMANCE, comparison,
  quic-modes, security-and-conformance, CONFIG, features, DEV-SETUP, CONTRIBUTING).
- **Report back** (your final message to the lead): list every file you changed/created, a 2-3
  line summary per file of what you did, any fact you could NOT ground (so the fact-checker
  re-checks it), and any cross-page dependency you noticed.

## KEY CODE FACTS (grounded; for accuracy)
- Live balancer: `crates/lb/src/main.rs` builds `RoundRobin` (L7 via `RoundRobinUpstreams`
  main.rs:1367; raw-TCP via `lb_balancer::RoundRobin` main.rs:3687). Maglev-by-CID for
  passthrough: `crates/lb-quic/src/passthrough.rs:542`. `LbPolicy` (11 variants) lives in
  lb-core, referenced nowhere outside it. EWMA setter cfg(test)-only (lb-core/lib.rs:79).
- Health: `crates/lb-health` HealthChecker = passive record_success/record_failure; called only
  in lb-health's own tests. Binary seeds it (main.rs:2552-2582) but nothing reads it.
- Parsing delegated: hyper (H1/H2, lb-l7/src/h1_proxy.rs + h2_proxy.rs), quiche/BoringSSL
  (H3/QUIC/TLS, lb-quic/), rustls (TLS), tungstenite (WS). Only hand-rolled production parser:
  `lb_quic::public_header` (Mode A, fuzzed). lb-h1/lb-h2/lb-h3-testcodec are TEST codecs +
  security-detector types, NOT live wire parsers.
- 18 crates (see arch/overview.md "The 18 crates by logical layer" for the accurate grouping —
  NOT the stale architecture.md which calls XDP a "simulator" and references the removed
  lb-compression crate).
- Container: `docker/Dockerfile` (distroless, positional config arg, EXPOSE 9090), proven boot
  recipe in `scripts/ci/docker-smoke.sh` (build image → run backend + gateway on a user network,
  mount config, publish port, curl 200 through it).
- 9-cell bridges: `crates/lb-l7/src/h{1,2,3}_to_h{1,2,3}.rs`; protocol-neutral via StrippedRequest
  + typed HeaderName/HeaderValue (H1/H2) or QPACK (H3); MAX_HEADERS=256.
