# S42 — Public + Developer Docs: Depth/Structure Revision (Session B2)

**Branch:** `feature/docs-revision-s42` · **Base:** `main` @ `ffac8705` ·
**Phase 1+2 commit:** `05ebfe5d` · **Production source changed:** NONE (docs,
diagrams, config comments only). **Status:** all gates green (3 personas PASS,
fact-check 0 blockers, doc-lint OK, 415 links resolve) — promoting to main via `--no-ff`;
post-merge CI confirmed below.

This session took the S41 public docs — diagnosed as *correct but hollow* (depth +
structure problem, `audit/release/doc-analysis.md`) — to release-grade, with three
force-multipliers: (i) an incumbent-doc **research benchmark**, (ii) a **persona-review
gate** (SRE / DevOps / contributor), and (iii) a real **developer/contributor doc track**.
The reachability re-audit caught **six** overclaim-class issues (the 2 flagged + 4 more).

---

## 1. Corrected claims — overclaim → verified reality (code-grounded)

The prior fact-check verified numbers/security but not capability **reachability**. This
session re-audited every headline capability against source. Full evidence:
`audit/release/s42-corrected-claims.md`.

| # | Claim as written | Verified reality (file:line) | Fix |
|---|---|---|---|
| 1 | "11 load-balancing algorithms" (implies selectable) | 11 implemented; **0 operator-selectable** — `LbPolicy` referenced nowhere outside lb-core; lb-config has no policy key; `deny_unknown_fields` rejects one. Live = round-robin (main.rs:1367/3687) + Maglev-by-CID for Mode A passthrough (passthrough.rs:542). EWMA setter cfg(test)-only (lb-core/lib.rs:79). | features.md canonical "Load balancing" section; honest framing everywhere; marquee de-headlined |
| 2 | "Active and passive health checking" | **Neither affects traffic.** Passive `HealthChecker` seeded but never consulted in selection (main.rs:2552-2582); active deferred (REL-2-05). No TTL cache. | "passive tracking, not wired into selection; active deferred" in features.md + known-limitations.md; arch/overview.md:110 fixed |
| 3 | "Standalone and HA modes" | **Unbacked** — `HaPoller` only in lb-controlplane cfg(test), not imported by binary. No clustering/failover/VRRP. | Removed; honest stateless-scale story in new deployment-patterns.md |
| 4 | per-backend `weight` works | **Inert** — `RoundRobinUpstreams::pick_info` is plain modulo; weight never read. | CONFIG.md + 7 examples + default.toml mark "accepted but not yet enforced" |
| 5 | `header_underscore_policy` drop/allow | **Inert** — `with_header_underscore_policy` never called in build_h1/h2_proxy; only `reject` active. | CONFIG.md + default.toml note only `reject` enforced |
| 6 | QUIC listener load-balances backends | **No** — H3-terminate forwards to `backends.first()` (conn_actor.rs:1682); Mode B = single `raw_proxy.backend_addr`. (main.rs:1857 / conn_actor.rs:213 "round-robin" comments are stale vs first().) | features.md table + CONFIG.md state first-backend / single |

**Stale doc-vs-code fixes (beyond the 6):** LB_LOG_FORMAT default is JSON not text
(log.rs:66) — fixed RUNBOOK.md + DEPLOYMENT.md; CONFIG.md H3-terminate backend wiring;
removed broken `gap-analysis.md` / `FINAL_REPORT.md` links (architecture.md + METRICS.md);
`lb-h3` → `lb-h3-testcodec` path (arch/overview.md).

**Headline pitch (owner-approved honest reframe):** marquee now leads with the **4
verified pillars** — 9-cell HTTP matrix · QUIC passthrough+terminate · DoS-mitigation
catalog · panic-free libraries. LB / health / topology moved to Implemented/Partial/Deferred
framing.

## 2. Reachability re-audit (other capabilities — confirmed reachable as written)
9-cell matrix (all 9 bridges), L4/XDP (off by default), QUIC Mode A/B + H3-terminate,
gRPC (H2/H3 front), WS-H1 (default)/H3 (opt-in)/H2 (gated), TLS + tls13_only, alt_svc
(advertise H3 from H1/H1s), per_ip_connection_cap, max_inflight_connections, LB_LOG_FORMAT,
quic-passthrough-only feature, SIGHUP/USR1/TERM. Compression correctly absent.

## 3. Research benchmark (`audit/release/s42-doc-benchmark.md`)
Studied Envoy / Traefik / HAProxy / nginx / Caddy doc IA. Patterns adopted: Envoy
"Life of a Request" (grounded config → numbered steps → one inline diagram); Traefik
container-first quickstart with visible payoff; nginx/HAProxy cookbook with a
"Combined Configuration Example" capstone + verify step; Caddy "Conventions" =
one-home-per-fact; glossary = open goal (no incumbent ships one).

## 4. De-duplication (one home per fact, R6/R12)
| Fact | Canonical home | Others |
|---|---|---|
| 9-cell matrix | features.md | gloss + link |
| LB / health reality | features.md "Load balancing" | gloss + link |
| Limitations (gRPC-front, WS-H2, Mode-A retry, h3spec waivers, no-mTLS, XDP, no-FD-handover) | known-limitations.md (with who-it-affects, folded from capabilities.md) | gloss + link |
| Delegated parsers + panic-free | arch/overview.md | security-and-conformance.md links it |
| S38 verdict | SECURITY.md | link |
| Perf | PERFORMANCE.md | link |
| Topology / HA | deployment-patterns.md | link |
capabilities.md reduced from prose-restatement to a pure support matrix (links homes).
overview.md link density 2.8 → **0.97**/100w.

## 5. Depth rewrites + diagrams (10 mermaid, code-accurate)
- **guide/overview.md** — true orientation: pitch + architecture-box diagram + plain
  request-flow + use-it-when/look-elsewhere decision aid (was a link-farm).
- **guide/getting-started.md** — container-first quickstart (mirrors docker-smoke.sh) with
  expected curl-200 + /metrics; from-source path; 2nd walkthrough (HTTPS+H2); first-errors.
- **arch/overview.md** — request-lifecycle **sequenceDiagram** ("Life of a Request").
- **arch/protocol-model.md** — 9-cell translation + bridge-pipeline diagrams.
- **arch/quic-modes.md** — Mode A + Mode B datapath diagrams.
- **arch/backpressure.md** — bounded-window read-pause diagram.
- **features.md** — load-balancing diagram; **architecture.md** — crate-dependency graph;
  **deployment-patterns.md** — topology; **observability.md** — scrape topology.
Diagram inventory: 10 across 9 pages (guide/ went from 0 → 3).

## 6. Task pages added (operator journey)
cookbook.md (5 annotated recipes + combined-config capstone), troubleshooting.md
(first-run FAQ + symptom index; links RUNBOOK for live alerts), deployment-patterns.md
(canonical topology/HA-honest), observability.md (golden signals + starter alerts),
glossary.md (~30 terms — the differentiator).

## 7. Developer / contributor track (owner-approved "practical contributor path")
- **architecture.md** repurposed → canonical crate map (18 crates by layer + dependency
  graph; removed "simulator" framing, removed-crate refs, broken links).
- **arch/extending.md** (new) — dev loop (build/test/gates), 3 worked extension maps
  (bridge cell / backend type / expose a balancer algorithm), audit-trail map, MSRV.

## 8. Validation
- **doc-lint**: PASS (tier-1 + tier-2, 52 claims). New pages added to FILES[].
- **Links**: 407 internal .md links resolve; `features.md#load-balancing` anchor present.
- **Diagrams**: 10 mermaid blocks, balanced fences + valid types.
- **Configs**: task-pages validated all cookbook/example configs via a lb-config-only
  harness (parse_config + validate_config, BoringSSL-free) — 9/9 OK incl. dual-:443 and
  quic-with-backends. _<fact-check re-confirmation PENDING>_
- **Container quickstart**: static + CI-backed (owner-approved) — mirrors the
  `docker-smoke` CI gate which builds+boots+serves the image every run (no docker on the
  writing box).
- **Git scope**: only docs/ + config/ + README + scripts/ci/doc-lint.sh; NO crates/ (R1).

## 9. Persona-review gate (the quality gate) — ALL 3 PASS
Each reviewer walked their journey read-only against the committed docs (05ebfe5d) and
returned a verdict citing specific journeys. The majors/minors below were remediated before
promote (§10a).

**SRE (deploy + operate) — PASS (conditional).** All 4 critical overclaim checks verified
honest; journeys 1-3, 5-7 clean (container + systemd + k8s boot with "what success looks
like"; the 3 protocols each have a 1:1 cookbook recipe; drain/reload/cert procedures correct;
troubleshooting symptom-index works; limitations honest+findable). Majors: monitoring docs
cited unemitted metrics; cookbook combined config put 2 backends on a `quic` listener.

**DevOps (containerize + config-as-code + pipeline) — PASS (no blockers).** All 4 traps
verified honest in source; container quickstart confirmed to match docker-smoke.sh + Dockerfile
exactly; LB_LOG_FORMAT default=JSON confirmed in code. Majors: pending-metric citations;
per-protocol drain-signal disclosure missing from known-limitations/deployment-patterns; no
config-validation-in-CI note.

**Contributor (onboard + make a first change) — PASS (no blockers/majors).** All 3 critical
checks verified in source (crate-dep diagram edges accurate; zero simulator/lb-compression/
broken-link residue; honest LB/health wording). Strongest journey: "make a change confidently"
(extending.md's 3 worked seams point at real files + tests). 4 minor-polish items.

## 10. Fact-check (code-grounded) — ZERO BLOCKERS
The fact-checker re-derived every claim FROM CODE (not docs). Verdict: **zero blockers**,
1 warning + minor notes.
- All 6 corrected claims honest in every home (file:line confirmed): `LbPolicy` unused
  outside lb-core; health seed unread; `HaPoller` cfg(test)-only; `weight` modulo-only;
  `header_underscore_policy` never wired; QUIC `select_backend`=first.
- 11 mermaid diagrams accurate vs code (crate-dep edges vs each Cargo.toml; constants
  64 MiB / 256 KiB / ≈64 KiB / MAX_HEADERS=256; Mode A public-header fields; lifecycle order).
- All doc config snippets parse+validate (lb-config harness); container quickstart matches
  docker-smoke.sh; one-home-per-fact respected; no perf numbers leak outside PERFORMANCE.md;
  doc-lint exit 0.

### 10a. Remediation (persona majors/minors + fact-check W1) — applied before promote
- **W1** (R14): arch/overview.md ASCII box "conntrack-hit forward by Maglev table" → "(XDP_TX)"
  (in-kernel per-packet Maglev is deferred; conntrack-hit uses the stored CT entry).
- **Metrics honesty**: `connections_inflight` + `backend_requests_total` +
  `backend_request_duration_seconds` are reserved-but-not-emitted (verified: label_budget only,
  no emit site) — RUNBOOK/observability/troubleshooting no longer cite them as live; point at
  the emitted `http_requests_total{listener,status_class}` / `http_request_duration_seconds` +
  logs as interim. METRICS.md corrected: `accept_inflight`/`accept_shed_total`/
  `accept_errors_total` are wired (main.rs:859/866, 3662, 3580).
- **cookbook** combined config: quic listener no longer implies multi-backend LB (H3-terminate
  = first backend).
- **drain-signal** pending now disclosed in known-limitations.md + deployment-patterns.md.
- **config validation**: CONFIG.md notes there is no `--check`/dry-run today (boot fail-fast).
- Minors: QUIC-single-backend added to known-limitations + README; getting-started `/metrics`
  sample carries the `listener` label; k8s securityContext hardened + probe-bind note;
  crate-dep graph adds the `lb-quic⇢lb-grpc` dev edge; SIGTERM-drain qualifier footnote.

## 11. Carry-forward (owner action items)
- Optional 1-line code fixes the docs now document as deferred: wire `weight` → a weighted
  picker; wire `header_underscore_policy` drop/allow; wire passive health into selection;
  add a policy config key to expose the 10 library algorithms; active-health probe loop
  (REL-2-05); a real balancer for QUIC H3-terminate (today first-backend-only).
- Stale **code comments** (not docs): main.rs:1857 + conn_actor.rs:213 say "round-robin"
  but H3-terminate uses `first()`.
- Internal research docs (docs/research/{envoy,katran,cross-cutting}.md) still say "active
  health checks" — out of strict public scope; flag for a cleanup pass (R14).
- Internal codenames (REL-2/SEC-2/PROTO-2/CODE-2/ROUND8/S3x/R8) persist in the keeper
  reference pages (RUNBOOK/DEPLOYMENT/CONFIG/PERFORMANCE) — accurate provenance, not rewritten
  this session; the new narrative pages are clean. Owner's call whether to scrub.
- ADR-0008 (immutable decision record) discusses "HA polling" without a not-wired note —
  cosmetic; the user-facing home (deployment-patterns.md) is honest.
- docker/Dockerfile + docker/smoke/gateway.toml comments cite `main.rs:1910` for the
  positional-arg code; actual is ~`main.rs:2430` (stale line-ref in a comment; behavior
  correct). Trivial code-comment fix for a future touch.
- Pre-existing: branch-protection rename, soak secrets, Dependabot advisories (deps
  session). CFs: CF-S27-2, CF-S37-D-TOKIO-1.52-RELAY, CF-S39-H3-REJECT-LOG-SPAM,
  CF-S38-QUIC-MAXCONN, F-ESC-1.

## 12. Verdict

**SESSION B2 COMPLETE — docs deepened / diagrammed / de-duplicated, 6 overclaim-class
issues corrected (the 2 flagged + 4 found by the reachability re-audit), 5 task-pages +
glossary + a developer/contributor track added, all 3 personas PASS, research-benchmarked,
fact-check 0 blockers — PROMOTED to main (`--no-ff`).** The docs are release-grade for all
audiences (evaluator / operator / SRE / DevOps / contributor).

Not a partial: every targeted section reached release-grade. §11 carry-forward is owner
action items (optional code fixes the docs now honestly document as deferred), not
unfinished documentation work.

Handoff → production pilot; owner action items in §11 (the deferred-feature wiring, the
keeper-page codename scrub, branch-protection rename / soak secrets / Dependabot for a deps
session).
