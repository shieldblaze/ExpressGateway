# Public Docs — Critical Diagnostic (pre-revision)

**Scope:** `docs/guide/*` + `docs/arch/*` + `CONTRIBUTING.md` + the READMEs (the
Session B / S41 deliverable), assessed against the actual system.
**Mode:** read-only diagnosis. No docs were changed. **Base:** `main` @ `ffac8705`.
**Premise:** the docs are factually accurate (S41 fact-checked the numbers); the
owner's dissatisfaction is therefore about something else. This report finds
**what** else, with quoted evidence, and says what kind of revision is needed.

> **Headline verdict (read this if nothing else).** The docs are *correct,
> comprehensive, and hollow.* They are large in page count but low in **unique
> information**: the same ~8 facts are restated across 4–10 documents each, and
> the two flagship guide pages (`overview.md`, `capabilities.md`) are
> **link-farms that re-summarize the reference docs instead of teaching
> anything new** — `overview.md` averages 35 links / 2.8 per 100 words. There
> are **zero diagrams in the entire `guide/` set** and only two ASCII sketches
> across all eleven narrative pages. New users get an H1-only hello-world gated
> behind a ~12-minute from-source build with no container path; operators get a
> superb config *reference* but no task-oriented *cookbook*, no troubleshooting,
> no glossary, no deployment-patterns page. And two prominent capability claims
> — **"11 load-balancing algorithms"** and **"active health checking"** — are
> not reachable/true as written (evidence below). This needs a **depth +
> structure pass**, not a polish pass: de-duplicate, make the narrative pages
> teach, add diagrams, add the missing task-oriented pages, fix two overclaims.
> Size ≈ a second Session-B-scale effort, but mostly rewriting/restructuring
> existing material (the facts and the reference layer are already good), not
> net-new research.

---

## 1. The single clearest problem: redundancy without depth (quantified)

The set *feels* like a lot of documentation because it **is** a lot of words.
But most of those words are the same facts repeated. Grepping the eleven
narrative pages + README + the two canonical reference docs:

| Fact restated | # of docs that state it |
|---|---|
| The 9-cell front×back matrix | **10** |
| Panic-free libraries / `panic = "abort"` | **10** |
| WebSocket-over-H2 gated off (CF-S27-2 rationale) | **8** |
| gRPC needs H2/H3 front (`grpc-status` trailers, "matches nginx") | **7** |
| No server-side mTLS (intentional) + upstream verify enforced | **7** |
| Delegated parsers (hyper/quiche/rustls/tungstenite) | **6** |
| Co-located 8-core perf box / ±20–30% caveat | **4** |
| S38 verdict 0C/0H/1M/7L/4I | **4** |

This is the texture that reads as "auto-generated summaries." A reader who
opens `overview.md`, then `capabilities.md`, then `comparison.md`, then
`features.md`, then `known-limitations.md` reads the WS-H2-gating paragraph
**five times**, each slightly reworded. The "delegated parsers / the lb-h1/h2
crates are not a second HTTP stack" point appears nearly verbatim in
`arch/overview.md` **and** `arch/security-and-conformance.md`.

Two consequences:
- **Reading is sideways, never down.** You move from page to page meeting the
  same facts; you never go *deeper* on anything. There's no payoff for reading
  more.
- **It's a maintenance liability.** The S41 fact-check itself had to add the
  EWMA caveat to *two* pages; the "active health checking" overclaim (below)
  lives in ≥3. Every fact with N homes is a fact you must fix in N places.

**What good looks like:** every fact has **one canonical home** (usually a
reference doc), and every other mention is a one-line gloss + link, *not* a
restated paragraph. The narrative pages stop re-explaining and start doing the
one job a reference doc can't.

---

## 2. The narrative pages don't teach — they route

`capabilities.md` states its own design out loud:

> "This page **summarizes and links**; it does not restate the reasoning."

That sentence is the core issue with the whole guide narrative layer. The S41
plan deliberately made the narrative pages "summarize + link, they do not
re-document" (s41-report.md). The result is pages whose dominant content is
pointers. Link density makes it measurable:

| Page | links / 100 words | character |
|---|---|---|
| `guide/overview.md` | **2.8** (35 links in 1250 words) | routing table |
| `guide/getting-started.md` | 2.0 | tutorial-ish |
| `guide/capabilities.md` | 1.6 | annotated index |
| `arch/overview.md` | 1.1 | teaches |
| `guide/PERFORMANCE.md` | **0.5** | teaches |

The correlation is on the nose: **the lowest-link pages are the best pages.**
`PERFORMANCE.md` (0.5) is the strongest doc in the set because it *owns* its
material — it has the data, explains how to read it, and doesn't punt. `overview.md`
(2.8) is the weakest of the guide pages because almost every sentence ends in
"see X" and hands the reader off before teaching them anything.

Look at a representative `overview.md` passage — a table row that is pure
pointer, no explanation:

> "| **gRPC** | Native gRPC proxying — requires an **H2 or H3 front** and an
> `h2`/`h3` backend. | [`../known-limitations.md`] |"

An evaluator learns *that* a constraint exists and is told to go read about it
elsewhere. The page never explains *why* in its own voice, never shows the
config, never says "here's the one example you'd copy." It is a table of
contents wearing the costume of a guide.

**What good looks like:** a narrative page earns its place by giving the reader
something the reference doesn't — a mental model, a decision framework, a worked
end-to-end scenario, a diagram. `PERFORMANCE.md` and `comparison.md` already do
this; `overview.md` and `capabilities.md` do not.

---

## 3. Zero diagrams (the most visible gap vs the incumbents)

Diagram/visual inventory across the eleven narrative pages:

| | guide/* (5 pages) | arch/* (5 pages) | CONTRIBUTING |
|---|---|---|---|
| Mermaid diagrams | **0** | **0** | 0 |
| ASCII diagrams | **0** | 2 (overview L4/L7 box; quic-modes recycle FSM) | 0 |

**The entire user guide has no picture of any kind.** Worse, the topics most
begging for a diagram have none:

- `protocol-model.md` — the **9-cell translation** and the protocol-neutral
  bridge pipeline (`StrippedRequest`) are described in prose and a tick-box
  table. This is *the* concept a developer wants drawn: front codec → neutral
  rep → back codec, with the response streaming back. No diagram.
- `backpressure.md` — the bounded-window read-pause mechanism ("stop reading,
  the transport propagates the pause") is an inherently visual flow-control
  story. Prose only.
- `quic-modes.md` — Mode A (route-by-CID, no decrypt) vs Mode B (two
  connections, raw relay) are exactly the kind of datapath you draw. There's a
  good recycling FSM sketch but **no datapath diagram** for A or B.
- `arch/overview.md` — the L7 request path is a **9-item numbered list** where
  envoy ships a "Life of a Request" *sequence diagram*.

This is the most concrete way the docs fall short of the benchmark set (lens 7,
below). It is also the cheapest high-impact fix.

---

## 4. Per-doc scorecard

Rating scale: **Strong** (substantive, teaches, keep) · **Solid** (does its job)
· **Mixed** (real weaknesses) · **Thin** (mostly pointers/restatement).

### `README.md` — **Mixed**
- **Strengths:** accurate; the "Start here / Building / Quickstart / Reload"
  sections are genuinely useful and correctly warn about the positional-arg /
  no-`--config` foot-gun.
- **Weaknesses:** opens with a **~50-line wall of bullets** (lines 3–52) — a
  spec sheet, not a hook. A first-time reader meets "SO_REUSEPORT", "lameduck
  `/readyz` flip", "`shutdown_aborted_connections_total`" before they know what
  the project *is*. The actual one-line pitch is buried under the feature dump.
- **Specific defect:** line 37 headlines **"Standalone and HA modes"** — there
  is **no doc anywhere** that explains what "HA mode" is or how to run it (grep
  confirms: the phrase exists only in README). A headline capability with zero
  backing doc.
- **Needs:** lead with a 2–3 sentence pitch + a "who is this for / when to use
  it" box; demote the feature wall to a "Capabilities at a glance" section
  lower down or a link to `capabilities.md`. Back or drop the HA claim.

### `docs/guide/README.md` — **Solid**
- Clean index, good "start here → reference" split. Does its job. No change
  needed beyond reflecting any new pages.

### `docs/arch/README.md` — **Solid (with a wrinkle)**
- Good developer index. But the **first substantial block** is a defensive
  blockquote explaining "why some docs live one level up (`docs/…`) and not
  under `docs/arch/`" — repo-hygiene inside-baseball that no developer reading
  for *architecture* cares about. Move it to the bottom or a HACKING note.

### `docs/guide/overview.md` — **Thin → Mixed**
- **Strengths:** the "Where it fits — an honest positioning" and "Key
  limitations to know up front" sections are genuinely good and honest — the
  "not a drop-in replacement for envoy/traefik/HAProxy/nginx … the incumbents
  are stronger" paragraph is the right voice.
- **Weaknesses:** highest link density in the set (2.8/100w); the "Headline
  capabilities" table is a duplicate of `features.md` + README; it restates the
  S38 verdict, the delegated-parser list, the co-located perf caveat, the EWMA
  caveat — all of which live elsewhere. The orientation never *draws* the
  architecture for the reader it's orienting.
- **Needs:** the biggest single rewrite. Turn it into a true orientation: the
  pitch, **one architecture diagram**, a short "how a request flows" in plain
  language, and a decision aid ("use it when… / don't when…"). Cut the
  duplicated capability table down to 4–5 marquee items and link the rest.

### `docs/guide/getting-started.md` — **Solid but narrow**
- **Strengths:** concrete, boot-validated, real `curl`, correct foot-gun
  warnings, the `python3 -m http.server` throwaway-backend trick is exactly
  right.
- **Weaknesses:** (1) **H1 hello-world only** — TLS/H2, H3, gRPC, WS are all
  punted to "pick the matching `config/examples/*.toml`"; there is no *second*
  walkthrough. (2) **Gated behind a ~6–8-min BoringSSL-from-source build** with
  **no container / prebuilt path** offered — even though `docker/Dockerfile` and
  `docker/smoke/gateway.toml` exist and there's a docker-smoke CI gate. (3) **No
  "what success looks like"** — no sample `curl -v` output, no sample `/metrics`
  snippet. (4) No "it didn't work →" troubleshooting.
- **Needs:** a **container quickstart first** (`docker run … expressgateway`)
  for a <2-minute first success, then the from-source path; a **second
  walkthrough** (HTTPS + H2 via `h1s`); sample expected output blocks; a short
  "common first errors" list.

### `docs/guide/capabilities.md` — **Mixed**
- **Strengths:** the **"who does this affect?"** framing on each limitation is
  the single most useful original idea in the guide — that's real value a
  reference table doesn't give.
- **Weaknesses:** the page openly "summarizes and links; does not restate the
  reasoning," yet it **substantially duplicates `known-limitations.md`**
  section-for-section (gRPC front, WS-H2, Mode A retry, h3spec waivers, no
  mTLS, XDP single-kernel) and overlaps `overview.md`'s "Key limitations" and
  `comparison.md`. Three pages cover the limitation set.
- **Needs:** pick one role and own it. Best option: **fold the "who-it-affects"
  framing *into* `known-limitations.md`** (make that the single canonical
  limitations page) and either delete `capabilities.md` or reduce it to a pure
  one-screen support matrix (✅/⛔/☑️/⚠️/⏳) with links — no prose restatement.

### `docs/guide/comparison.md` — **Strong**
- **Strengths:** genuinely additive (no reference-doc equivalent); the
  capability table is concrete; the "Where the mature incumbents are stronger"
  section is honest and well-judged (battle-tested scale, xDS, ecosystem, hot
  restart, WAF). This is the kind of page an evaluator actually wants.
- **Weaknesses:** still link-heavy; **omits a "Compression" row** (envoy/nginx
  have response compression; ExpressGateway does **not** — confirmed: not
  implemented — and an evaluator will look for it); no quick "decision tree."
- **Needs:** light polish — add the compression row (as an honest ❌), maybe a
  3-line "pick X if…" decision aid. Keep.

### `docs/guide/PERFORMANCE.md` — **Strong (the model for the rest)**
- **Strengths:** the best page in the set. Real tables, per-path detail, honest
  "these are the test rig's limits, not the gateway's," teaches the reader *how
  to read* the three signals, states what was **not** characterized. Self-
  contained (0.5 links/100w).
- **Weaknesses:** minor — very dense; could use one small chart. The headline
  efficiency ordering could be a simple bar.
- **Needs:** essentially nothing. Use this page's register as the template for
  rewriting `overview.md` and `capabilities.md`.

### `docs/arch/overview.md` — **Strong**
- **Strengths:** real depth — the "parsing is delegated" framing up front, the
  18-crates-by-layer map, the one good ASCII data-plane diagram, the 9-step
  request path, the concurrency model, the panic-free model. This is a developer
  doc that teaches.
- **Weaknesses:** repeats the delegated-parser + "lb-h1/h2 aren't a second
  stack" point that also lives in `security-and-conformance.md`; only one
  diagram; the request path is a list, not a sequence diagram.
- **Needs:** add a request-lifecycle sequence diagram; de-dupe the delegation
  point to one home (here) and have security-and-conformance link it.

### `docs/arch/protocol-model.md` — **Solid (under-illustrated)**
- **Strengths:** the protocol-neutral `StrippedRequest` pipeline explanation is
  good; gRPC trailers and the WS-transport table are clear.
- **Weaknesses:** **this is the page that most needs diagrams and has none.** The
  9-cell translation and "nine cells share one pipeline" story is the central
  developer concept and it's prose + a checkmark grid.
- **Needs:** a 9-cell translation diagram + a "request through the bridge"
  diagram. This is the centerpiece arch page; make it visual.

### `docs/arch/quic-modes.md` — **Strong**
- **Strengths:** the best deep-dive — Mode A's no-decrypt public-header parse
  enumerated step by step, the recycling FSM drawn, the H3 shutdown-code table.
  Real engineering substance.
- **Weaknesses:** no datapath diagram for Mode A vs Mode B (the recycling FSM is
  the only picture).
- **Needs:** two small datapath diagrams (A: client↔backend TLS end-to-end,
  gateway routes by CID; B: two distinct quiche connections). Otherwise keep.

### `docs/arch/backpressure.md` — **Solid (under-illustrated)**
- **Strengths:** explains *why* (the 10 GB upload → 10 GB alloc DoS), the
  two-layer bound, the per-protocol constant table, the smuggling corollary
  (F-MD-4 clean-EOF-vs-source-died) — substantive.
- **Weaknesses:** zero diagram for an inherently visual flow-control mechanism.
- **Needs:** one diagram of the bounded window + read-pause propagation. Keep
  the prose.

### `docs/arch/security-and-conformance.md` — **Solid**
- **Strengths:** clear "why is this safe" narrative; the fuzzing numbers, the
  panic-free structural argument, the h3spec honesty-gate explanation are good.
- **Weaknesses:** repeats the delegated-parsers list and the panic-free block
  that also live in `arch/overview.md`; no visuals (less needed here).
- **Needs:** de-dupe the shared blocks to a single home + link. Otherwise fine.

### `CONTRIBUTING.md` — **Solid**
- **Strengths:** practical and correct — build/test/gates, the panic-free rule
  with the *why* (`panic = "abort"` → unhandled panic is an outage), PR
  expectations, the shared-tree commit discipline. A contributor can act on it.
- **Weaknesses:** minor jargon leakage (`CF-SATURATION-1`, `F-MD-4`) but
  acceptable for this audience.
- **Needs:** nothing major.

### `docs/arch/DEV-SETUP.md` — **Strong (for its purpose)**
- Detailed, practical box-per-task + disk-discipline guide. Genuinely useful to
  anyone setting up. No change needed.

---

## 5. Reader-journey findings (lens 1)

### A. The skeptical evaluator ("what is this, should I care, how does it compare?") — **mostly served**
`comparison.md` + `PERFORMANCE.md` + the honest-positioning section of
`overview.md` land this well — the honesty ("incumbents are stronger at…") is
exactly right for a skeptic. **Walls:** they meet the same 8 facts 4–5 times;
there's **no architecture diagram** to grok the design in 30 seconds; and they
hit the "11 algorithms" / "active health checking" claims that don't hold up
(§7), which *erodes* the very credibility the honesty was building.

### B. The new user ("get me from zero to running") — **partially served**
`getting-started.md` is concrete and boot-tested, but the journey is: install
cmake/clang → **wait 6–8 min for BoringSSL** → H1 hello-world → done. **Walls:**
the build-from-source wall with no container path; only one protocol; no
expected-output to confirm success; nothing for "it didn't work." A newcomer who
wants HTTPS, gRPC, or H3 is sent to read `config/examples/` on their own.

### C. The operator with a specific need ("configure X for my setup") — **weakest journey**
`CONFIG.md` is an excellent *reference* (every knob, ranges, foot-gun table,
reload classes) — but the set is **reference-rich and task-poor.** There is:
- no **task cookbook** ("set up production HTTPS with H2 + gRPC + rate caps +
  metrics + graceful drain" as one annotated, copy-paste config);
- no answer to **"how do I choose a load-balancing algorithm?"** — and the real
  answer (you can't, from config — §7) is undocumented;
- no **health-check configuration** (the feature as claimed doesn't exist — §7);
- no **HA / topology / deployment-patterns** page (despite the README claim);
- no **troubleshooting** for the common operator failure modes (502s, config
  rejections, XDP attach failures, cert reload).

The operator can look up any single knob but is never guided to assemble knobs
toward a goal. This is the journey the revision should invest in most.

### D. The developer / contributor ("how does it work, can I trust it, can I contribute?") — **best served**
`arch/*` + `CONTRIBUTING.md` + `DEV-SETUP.md` + the ADRs are substantive and
mostly teach. **Walls:** no diagrams (a developer's first ask), and the
delegated-parser point is explained 4×. This audience is over-served on prose
depth and under-served on visuals.

**Net audience-fit finding:** the set **over-serves the developer and the
evaluator** (who get genuine depth in `arch/*`, `comparison.md`, `PERFORMANCE.md`)
and **under-serves the task-driven operator** (lots of reference, no
task-oriented cookbook / troubleshooting / decision guides).

---

## 6. The comparison-class benchmark (lens 7)

Three specific things the incumbents do well, and how this set measures up:

**1. Envoy — architecture deep-dives with diagrams ("Life of a Request").**
Envoy's arch docs walk a request listener → filter chain → router → cluster →
upstream **with a sequence diagram**. ExpressGateway's equivalent
(`arch/overview.md` "The L7 request path") is a **9-item numbered list**, and
`protocol-model.md` describes the 9-cell translation in prose. *To match:* add a
request-lifecycle sequence diagram + a 9-cell translation diagram + Mode A/B
datapath diagrams. The prose is already at envoy-doc quality; it just isn't
drawn.

**2. Traefik — getting-started UX.** Traefik gets you to a running, observable
proxy via a copy-paste container snippet in ~2 minutes, with a "you should now
see…" payoff and a dashboard. ExpressGateway's getting-started is a
**from-source BoringSSL build → H1 hello-world** with no container path and no
"what success looks like." *To match:* lead with a container quickstart and show
the expected `curl -v` + `/metrics` output. (The Dockerfile already exists — it's
just not in the user's path.)

**3. HAProxy / nginx — the config cookbook + directive reference.** HAProxy/nginx
docs pair a complete *directive reference* with **annotated, complete, real-world
configs** for named scenarios (TLS termination, rate limiting, sticky sessions,
blue-green). ExpressGateway has the reference half **done well** (`CONFIG.md` is
close to nginx-directive-reference quality, and the 9 `config/examples/*.toml`
are decently commented) but is **missing the cookbook half**: no narrative that
builds up a non-trivial production config and explains the *why* of each block,
and no "recipe" page. *To match:* add a config-cookbook/recipes page with 3–5
complete annotated scenarios.

Bottom line: on **reference depth** ExpressGateway already rivals nginx/HAProxy
(`CONFIG.md`, `METRICS.md`). On **diagrams** and **task-oriented getting-started/
cookbook UX** it is well behind envoy/traefik. The gap is *shape*, not *facts*.

---

## 7. Gap list — system behaviors that are mis/under-documented (lens 5)

Cross-checked against the code/config inventory. Two of these are **likely
accuracy defects the S41 fact-check did not catch** (it verified numeric/security
claims rigorously, but not capability *reachability*). Flagged ⚠️ — the revision
should confirm in source and correct.

⚠️ **"11 load-balancing algorithms" is not operator-selectable.** README,
`overview.md` ("Load balancing | 11 algorithms…"), and `arch/overview.md` all
headline eleven algorithms. But there is **no config key to select one** —
`CONFIG.md` documents only per-backend `weight`, and the inventory confirms:
*"Algorithm is not user-configurable in config file; hard-wired per listener type
in code."* Only round-robin / weighted-round-robin is operator-reachable via
TOML. The other nine (P2C, Maglev-for-L7, ring hash, EWMA, least-conn,
least-request, random, weighted-random, session affinity) are implemented in
`lb-balancer` but an operator cannot choose them. The EWMA caveat S41 added
addresses one algorithm's *fidelity* but misses the larger point that **none of
the eleven are config-selectable.** *Fix:* either document how to select (if a
key exists that I/the inventory missed) or state plainly "the config currently
selects round-robin/weighted; the other algorithms are implemented but not yet
exposed via config." Selling eleven choices a reader can't make is the exact
"lists features you can't act on" failure.

⚠️ **"Active health checking" appears to be an overclaim.** README:30 ("Active
and passive health checking"), `overview.md`:53 ("active/passive health
checks"). But the code's own comment (`crates/lb/src/main.rs:51`) says
*"lb-health provides per-backend HealthChecker (**passive**…)"*; `lb-health`
exposes only `record_success()` / `record_failure()` (a consecutive-pass/fail
state machine) with **no prober, no interval, no probe path**, and it's reached
only from a dependency-reachability smoke test. There is **no active-health
config** (no interval/path/expected-status) anywhere. The inventory states
active checks are **deferred (REL-2-05)**. *Fix:* correct to "passive
health-status tracking (active probing deferred)," and verify whether even
passive tracking is wired into live backend selection or is currently inert.

**Real capabilities with no narrative / no task guidance (documented only as raw
schema rows, or not at all):**
- **Session affinity / sticky sessions** — listed as 1 of 11 algorithms; **zero**
  mention in `CONFIG.md`/`features.md`/`capabilities.md` of how it's keyed or
  configured. (grep: no `affinity`/`sticky` in operator docs.)
- **Resource-limit DoS knobs** — `per_ip_connection_cap` (1024),
  `max_inflight_connections` (65536), the `[runtime.watchdog]` slowloris/slow-POST
  floor — security-relevant, in `CONFIG.md` only, never surfaced in the
  security narrative where an evaluator would look.
- **`header_underscore_policy`** (reject/drop/allow) — a real
  request-smuggling-adjacent control; schema-only.
- **`alt_svc` / advertising H3 from an H1/H1s listener** — the actual rollout
  path for HTTP/3 — appears as a one-line note; no "how to roll out H3" recipe.
- **`LB_LOG_FORMAT` (json/text/plain)** env var — not in the narrative;
  logging/observability has no narrative home at all.
- **`quic-passthrough-only` cargo feature** (passthrough-only build) — not in
  getting-started/DEPLOYMENT.

**Whole pages a reader expects and that don't exist:**
- **Troubleshooting / FAQ** (config rejected, 502s, XDP attach failures, cert
  reload didn't take) — none.
- **Glossary** — heavy jargon (Mode A/B, 9-cell, R8, lameduck, extended CONNECT,
  CID, conntrack-hit, Maglev, CO-RE, GOAWAY) with no glossary.
- **Deployment patterns / HA / topology** — README headlines "HA modes"; no
  page explains it.
- **Observability narrative** — `METRICS.md` lists every metric, but there's no
  "what to monitor / what healthy looks like / example alert rules / Grafana."
- **Config cookbook / recipes** — see lens 7.

**Correctly omitted (no action, noted for completeness):** compression is *not*
implemented and is correctly absent from `features.md` — but it should appear as
an explicit ❌ in `comparison.md`/`known-limitations.md` since the incumbents
have it and evaluators look for it.

---

## 8. Tone & readability (lens 4)

Competent and professional, but **dry, dense, and over-signposted.** Two
recurring tics:

- **"Honesty" meta-narration.** The docs repeatedly narrate their own
  trustworthiness: "Nothing here is aspirational," "the headline you can trust,"
  "an honest positioning," "Read this first," "honesty contract," "read
  carefully." Once or twice this builds confidence; a dozen times it reads
  *defensive* and paradoxically makes a reader wonder what's being
  over-insisted. State the fact and the limit plainly and let them carry their
  own credibility.
- **Internal-codename leakage into reader-facing docs.** `R8`, `R3`, `CF-S27-2`,
  `F-RES-3`, `F-ESC-1`, `S36/S38/S39`, `ROUND8-L7-05`, `CF-GRPC-H3-CHURN-RSS`
  appear in *user/operator* pages. To an outside reader these are noise that
  signals "internal notes published as docs." Keep them in `audit/` and in
  developer docs; in user-facing pages, describe the behavior and link the audit
  trail by name only where it adds trust (e.g. the S38 verdict).

The arch docs and `PERFORMANCE.md` largely avoid these and read well; the guide
narrative pages are the worst offenders.

---

## 9. The top problems, prioritized

The 3–7 things that would move the set from "not satisfied" to "release-grade":

1. **De-duplicate ruthlessly; give every fact one home.** Collapse the 4–10×
   restatements (§1) into single canonical homes + one-line glosses elsewhere.
   This alone removes the "auto-generated summary" feel and cuts maintenance
   surface. *Good = a reader never meets the same paragraph twice; each fact is
   owned by exactly one page.*

2. **Make the two flagship guide pages teach, not route.** Rewrite `overview.md`
   into a true orientation (pitch + one architecture diagram + plain-language
   request flow + when-to-use decision aid), and resolve `capabilities.md`'s
   overlap (fold its excellent "who-it-affects" framing into a single canonical
   `known-limitations.md`; reduce `capabilities.md` to a one-screen matrix or
   delete it). Use `PERFORMANCE.md`'s self-contained register as the template.
   *Good = overview.md drops from 2.8 to <1.0 links/100w because it now has its
   own content.*

3. **Add diagrams.** At minimum: a request-lifecycle sequence diagram
   (`arch/overview.md`), a 9-cell translation diagram (`protocol-model.md`),
   Mode A/B datapath diagrams (`quic-modes.md`), a backpressure window diagram
   (`backpressure.md`), and one architecture diagram in `guide/overview.md`. This
   is the highest impact-per-effort fix and the clearest gap vs envoy. *Good =
   every "how it works" concept a reader must hold in their head is drawn.*

4. **Fix the two capability overclaims (§7).** "11 algorithms" → document
   selection or state it's not config-exposed; "active health checking" →
   correct to passive-only / deferred. These are credibility leaks in front of
   the exact skeptical evaluator the comparison page is courting. *Good = every
   headlined capability is one an operator can actually reach, or is plainly
   labeled as not-yet-exposed.*

5. **Rebuild getting-started for a fast first success + a second step.** Lead
   with a container quickstart (<2 min, expected output shown), keep the
   from-source path, add a second walkthrough (HTTPS + H2), and a short "common
   first errors." *Good = a newcomer is serving real traffic in minutes without
   reading `config/examples/` on their own.*

6. **Add the missing task-oriented pages for the operator journey.** A config
   **cookbook** (3–5 complete annotated scenarios), a **troubleshooting/FAQ**, a
   **deployment-patterns/HA** page (or drop the HA claim), an **observability**
   narrative (what to watch, example alerts), and a **glossary**. *Good = the
   operator-with-a-task journey (§5C) stops dead-ending in raw schema.*

7. **Drop the tone tics.** Cut the repeated honesty meta-narration; move internal
   codenames out of user-facing pages (§8). *Good = the prose states facts and
   limits plainly and trusts them to land.*

---

## 10. Structure recommendation (keep / add / merge / reorder)

**Keep as-is (strong):** `PERFORMANCE.md`, `comparison.md` (add compression row),
`arch/quic-modes.md`, `arch/security-and-conformance.md`, `CONTRIBUTING.md`,
`DEV-SETUP.md`, `CONFIG.md`, `features.md`. These are the load-bearing, additive
pages.

**Rewrite (content exists, shape is wrong):** `guide/overview.md` (orientation +
diagram, de-link), `README.md` (lead with a pitch, demote the feature wall),
`arch/overview.md` (+ sequence diagram, de-dupe delegation), `arch/protocol-model.md`
(+ diagrams), `arch/backpressure.md` (+ diagram), `getting-started.md` (container
+ 2nd walkthrough).

**Merge / resolve overlap:** fold `capabilities.md`'s "who-it-affects" value into
a single canonical **`known-limitations.md`**; make `capabilities.md` either a
pure one-screen support matrix or remove it. Decide **one** home each for: the
9-cell matrix (→ `features.md`), the delegated-parser/panic-free story (→
`arch/overview.md`), the limitations set (→ `known-limitations.md`). Everyone
else links.

**Add (new pages):** `guide/cookbook.md` (annotated real-world configs),
`guide/troubleshooting.md` (FAQ + common failures), `guide/deployment-patterns.md`
(HA/topology — or delete the README HA claim), `guide/observability.md` (what to
monitor + example alerts), `docs/glossary.md`.

**Reorder:** README → pitch first, feature list last. `arch/README.md` → move the
"why docs live one level up" blockquote to the bottom.

---

## 11. Overall verdict — what kind of revision, how big

**Not a polish pass. Not a factual rewrite. A depth + structure pass.**

- It is **not** primarily a **tone/prose** problem (though §8 is worth a cleanup
  pass). The writing is competent.
- It is **not** a **factual** problem in the numeric/security sense — S41's
  fact-check was sound *there*. (The two capability overclaims in §7 are
  reachability gaps the fact-check's scope didn't cover, and are small to fix.)
- It **is** a **depth + structure + audience-fit** problem:
  - **Depth:** the guide narrative restates the reference layer instead of
    adding a teaching layer; the arch layer teaches but isn't drawn.
  - **Structure:** the same facts have 4–10 homes; the limitations set is spread
    across 3 pages; whole expected pages (cookbook, troubleshooting, glossary,
    deployment-patterns, observability) are missing.
  - **Audience-fit:** the task-driven operator is under-served.

**Size:** substantial — on the order of a **second Session-B-scale effort**, but
the work is **rewriting and restructuring existing material + adding diagrams +
~5 new task-pages**, not net-new research (the facts and the reference layer are
already good and verified). Concretely: ~6 page rewrites, 1 merge/deletion, ~5
new pages, ~5 diagrams, 1 de-duplication sweep, 2 small accuracy fixes. The raw
material to make these docs genuinely good already exists in the repo — the S41
pass assembled it correctly but stopped at "summarize and link." The revision's
job is to make the narrative layer **own and teach** its material, **draw** the
hard concepts, and **fill the task-oriented gaps** — turning a correct-but-hollow
set into one that actually gets a real reader to a real goal.
