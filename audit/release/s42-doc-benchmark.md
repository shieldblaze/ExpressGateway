# Incumbent Doc-Craft Benchmark (S42 research input)

**Purpose.** Establish a concrete, evidence-based bar for the S42 documentation
revision by studying how the five incumbent load-balancer / proxy projects
structure and write their **documentation** (information architecture, teaching
style, diagrams, getting-started UX, cookbooks, architecture deep-dives,
troubleshooting/FAQ/glossary). This is about **doc craft**, not their internal
engineering. It is written to be *followed* by a writing team — every pattern is
tied to who does it, why it works, and which ExpressGateway page it maps to.

**Method.** Each site's actual doc pages were fetched (not recalled). The URLs
read are cited inline per finding and listed in the appendix. Studied:
Envoy, Traefik, HAProxy, nginx (both `nginx.org` and `docs.nginx.com`), Caddy.

**Companion.** This pairs with `audit/release/doc-analysis.md` (the read-only
diagnosis of our current docs). That doc found our set is *correct but hollow*:
~8 facts restated across 4–10 pages each, **zero diagrams in `guide/`**, a
from-source-build-gated H1-only getting-started, and missing cookbook /
troubleshooting / glossary / HA / observability pages. This benchmark says what
"good" looks like for each of those gaps.

---

## 0. How to read this — the three exemplars to clone first

The diagnosis already named three incumbents to match. The research confirms and
sharpens them, and adds two more leverage points:

1. **Envoy — "Life of a Request"** is the gold standard for teaching a request
   path with diagrams. Clone its *structure*, not just "add a picture."
2. **Traefik — the container-first quickstart** with a visible payoff (dashboard
   + `curl` output) is the gold standard for getting-started UX.
3. **nginx (`docs.nginx.com` admin guide) + HAProxy tutorials** are the gold
   standard for the **task-oriented cookbook** paired with a **directive
   reference** (`nginx.org`).
4. **Caddy** is the gold standard for *concision and progressive disclosure*
   (CLI one-liner → config file → API) and the friendliest tone.
5. **Caddy's "Conventions" page** is the single best structural answer to our
   redundancy problem: define each cross-cutting fact **once**, link everywhere.

Everything below is in service of these five moves.

---

## 1. Envoy — the architecture-teaching benchmark

Docs home: <https://www.envoyproxy.io/docs/envoy/latest/>

### Information architecture
Eight top-level sections, each with one job
(<https://www.envoyproxy.io/docs/envoy/latest/>):
Introduction (concepts/mental models) · Getting Started (hands-on) ·
Configuration reference · Operations and administration · Extending Envoy ·
API · FAQ · Version history. The key move: the **Introduction** section is itself
split four ways — *What is Envoy* (positioning) → *Architecture overview* (how it
works) → *Life of a Request* (narrative) → *Deployment types* (where it fits).
WHAT-IT-IS → HOW-IT-WORKS → WHERE-IT-FITS are separate pages, not one overview.
The Configuration reference is ordered on the **same axis** as the architecture
(Listeners → HTTP → Upstream clusters → Observability → Security → Operations),
so a reader who learned the architecture can predict the reference nav. Almost
no overlap between concept / how-to / reference / diagnosis.

### Teaching patterns
- **Design-intent before features.** *What is Envoy* opens with the belief, not
  the feature list: *"The network should be transparent to applications. When
  network and application problems do occur it should be easy to determine the
  source of the problem."* Features come after the *why*
  (<https://www.envoyproxy.io/docs/envoy/latest/intro/what_is_envoy>).
- **Mechanism → consequence in adjacent sentences.** From *Life of a Request*:
  *"Worker threads rarely share state and operate in a trivially parallel
  fashion. This threading model enables scaling to very high core count CPUs."*
  Every step states *what* then immediately *why it matters*
  (<https://www.envoyproxy.io/docs/envoy/latest/intro/life_of_a_request>).
- **Tradeoffs embedded in prose** (Ring Hash vs Maglev compared inline, not in a
  matrix) — a deliberate depth-over-skimmability choice for algorithm sections.

### Diagram usage — study this closely
*Life of a Request*
(<https://www.envoyproxy.io/docs/envoy/latest/intro/life_of_a_request>) is the
most diagram-dense page in the set (~20+ inline SVGs). Section-by-section:
1. **Terminology** (definitions, no diagram).
2. **Network topology** — 6 *placement* diagrams (sidecar, ILB, edge, hybrid,
   tiered): *where Envoy sits before any request flows.*
3. **Configuration** — the concrete YAML the rest of the page refers back to.
4. **High level architecture** — ONE subsystem box diagram (Listener subsystem →
   HTTP router → Cluster subsystem). This is the mental model everything hangs on.
5. **Request flow** — a compressed 12-point enumeration first ("overview before
   territory"), then one **inline diagram per step**, each a zoom-in of one
   component from the architecture box (lor-listeners, lor-filter-chain-match,
   lor-transport-socket, …).

The reusable recipe (this is the thing to adopt verbatim):
> Ground the walkthrough in a concrete config shown first → split into strictly
> isolated **numbered steps** → **one diagram per step** showing *exactly and
> only* the layer being discussed, placed *inline* with that step's prose → each
> step structured **interface/API → behavior → consequence (why)** → open the
> page with a **topology/placement** section so the walkthrough has a spatial
> frame.

### Getting-started UX
The `start` page is a **navigation hub, not a tutorial** — a weakness. The real
content is in Quick Start subpages
(<https://www.envoyproxy.io/docs/envoy/latest/start/quick-start/run-envoy>),
which are effectively Docker-first, *validate the config* (`configuration ... OK`),
then run and `curl localhost:10000` showing exact expected headers
(`HTTP/1.1 200 OK`, `server: envoy`). Config is taught **prose-then-fragment**
(explain a section, show only that YAML fragment; near-zero inline YAML comments —
prose carries meaning so comments don't drift). Quick Start is a 7-page sequence
ramping static → dynamic filesystem → xDS control plane.

### Cookbook / recipes
**43 Docker-Compose "Sandboxes"**
(<https://www.envoyproxy.io/docs/envoy/latest/start/sandboxes/>): front-proxy,
TLS, TLS-SNI, gRPC bridge, ext_authz, fault injection, local ratelimit, locality
LB, WebSockets, etc. Each: requirements → `docker compose up` → numbered
"test routing / test load balancing / curl the admin" steps, with verbatim
expected output. They are **try-it-and-see** recipes (configs linked, not
annotated inline) — the *concept* explanation lives in the arch overview, the
sandbox makes it real.

### Architecture deep-dive pattern
Two complementary pages: *Architecture overview* (a hierarchical index mirroring
the config reference — concept essays of 300–2000 words) +
*Life of a Request* (the only page that crosses all subsystems, as one narrative).
Index for lookup, narrative for understanding.

### Troubleshooting / FAQ / glossary
**FAQ** organized by **symptom domain** (Build/API/Debugging/Performance/
Configuration/Load Balancing/Extensions/Windows), ~40 Qs
(<https://www.envoyproxy.io/docs/envoy/latest/faq/overview>). Most answers link
out; the Configuration answers are mini-guides that *scope first* ("the most
important timeouts ...") then deliver structured reference. **No standalone
glossary** (terms defined inline in *Life of a Request*). **No troubleshooting
runbook** — gaps we can beat.

---

## 2. Traefik — the getting-started UX benchmark

Docs home: <https://doc.traefik.io/traefik/>

### Information architecture
v3 nav is named by **task/persona, not by object**
(<https://doc.traefik.io/traefik/>): What is Traefik · Features · Getting Started
(Overview, Configuration Introduction, Quick Start) · Setup · **Expose** ·
**Secure** · **Observe** · Extend · Govern · Migrate · Reference · FAQ. The
cookbook is first-class and verb-named — **"Expose"** (not "Routing"),
**"Secure"**, **"Observe"** — each split **Basic** (first working example) →
**Advanced** (TLS, Let's Encrypt, sticky sessions, middleware). Reference catalogs
by concept (Routers/Middlewares/Services) and explicitly **does not teach**.
Migration guides are versioned and include a competitor on-ramp
("Ingress NGINX → Traefik").

### Teaching patterns
- **Define → contrast with the familiar.** *"Traefik is an Edge Router ... it's
  the door to your platform ..."* then *"Where traditionally edge routers ... need
  a configuration file that contains every possible route ... Traefik gets them
  from the services themselves."*
  (<https://doc.traefik.io/traefik/getting-started/concepts/>).
- **Temporal walkthrough of the mental model.** *"First, when you start Traefik,
  you define entrypoints ... Then, connected to these entrypoints, routers analyze
  the incoming requests ... If they do, the router might transform the request
  using ... middleware before forwarding them to your services."*
- **Category before inventory** (providers taught as 4 categories before listing
  15); **config split as a teaching axis** (install-config "doesn't change often"
  vs routing-config "hot-reloaded" — taught *before* any code).

### Diagram usage
Diagrams are reserved for **conceptual** pages and act as the *concept
introduction* (prose is the annotation). Three core ones: "The Door to Your
Infrastructure", "Decentralized Configuration" (the aha-moment: manual-config
proxy vs services-advertise-their-own), and an architecture diagram on
routing/overview showing **Providers → EntryPoints → Routers → Middlewares →
Services**, placed at the top immediately before the prose that walks the same
path. Every diagram has a **title-case caption naming the concept**; cookbook
pages have **zero** diagrams.

### Getting-started UX — the page to clone
Container-first and unambiguous
(<https://doc.traefik.io/traefik/getting-started/docker/>). The exact flow:
1. **~10 lines of `docker-compose.yml`** — three flags as `command:` args, no
   separate config file, each flag commented.
2. **Dashboard payoff at step 2**, *before any service exists*:
   `http://localhost:8080/dashboard/` — the reader sees it's running and healthy.
3. **One-label service**: a `whoami` container with a single
   `traefik.http.routers.whoami.rule=Host(...)` label — proving routing config
   lives on the container.
4. **`curl http://whoami.localhost`** with the **full expected output reproduced
   verbatim** on the page — and the hidden reward is the injected
   `X-Forwarded-*` headers (it worked *and* did something useful, with no config).
5. **`docker-compose up -d --scale whoami=2`** → curl twice → alternating
   hostnames: load balancing demonstrated by a single flag, zero config change.

What makes it work: zero-friction prereqs (Docker only), ~3 commands to first
response, a reward *before* the user does work, output that proves value (not just
"200 OK"), and inline-commented compose YAML.

### Cookbook / recipes
The **Expose/Secure/Observe** sections, parallel tracks per platform
(Docker/Kubernetes/Swarm), each Basic→Advanced. Consistent recipe spine:
Prerequisites → first HTTP service → add routing → enable TLS → Next Steps, and
**every section ends with a "Test the X" block** with an exact `curl` + expected
output. The gRPC guide even ships Go client/server code for a complete
end-to-end recipe.

### Troubleshooting / FAQ / glossary
FAQ is a top-level section; **no standalone glossary** (terms defined inline). A
nice built-in debug surface: the `/api/rawdata` endpoint shown in the quickstart
*is* a live troubleshooting tool (the routing table as JSON).

---

## 3. HAProxy — the cookbook + directive-reference benchmark

Docs home: <https://docs.haproxy.org/>

### Information architecture
Split across two domains by purpose. **docs.haproxy.org** ships exactly three
versioned guides: **Starter Guide** (concept-only — explicitly *"This document
doesn't provide any configuration help"*), **Configuration Manual** (the canonical
directive reference), **Management Guide** (operations). The teaching/cookbook
lives separately at **haproxy.com/documentation/haproxy-configuration-tutorials/**
in 7 task categories: Getting Started · Proxying Essentials · Protocol Support ·
Alerts & Monitoring · Security · Reliability · Performance. The cost of the split:
no inline cross-link from a config block to its reference entry (only "See also"
footers).

### Teaching patterns — two distinct modes
**Directive-reference schema** (rigid, every keyword identical): `keyword <args>`
→ one-line purpose → a **section-applicability table** (yes/no across
defaults|frontend|listen|backend) → `Arguments:` (each option named + prose) →
terse bare `Examples:`. E.g. `balance <algorithm>` documents roundrobin/leastconn/
etc. each with a paragraph, ending in uncommented examples like `balance roundrobin`.

**Tutorial mode**: problem statement (1 sentence) → named subsections per variant,
each *concept prose then a bare (uncommented) config block* → "See also" footer.
E.g. session persistence:
> *"Cookie-based persistence: To enable session persistence based on an HTTP
> cookie, add the `cookie` directive to your backend section."*
> ```
> backend servers
>     mode http
>     cookie SERVER_USED insert indirect nocache
>     server s1 192.168.0.10:80 check cookie s1
>     server s2 192.168.0.11:80 check cookie s2
> ```

### Diagram usage
Diagram-**poor**: the config manual has zero (historically a `.txt` file); the
tutorial pages reviewed had none either (some appear only in blog posts).
Understanding is expected to emerge from prose + config. A gap we can beat.

### Getting-started UX
Fragmented and weak — the Starter Guide is concept-only with no runnable config,
and a runnable config first appears partway into the Backends tutorial. **There is
no single "write this, start it, curl it, expect this" page.** Another gap to beat.

### Cookbook / recipes — the strength
Named scenario pages with **progressive complexity**. Concrete examples:
- **Session persistence** — cookie-based and stick-table/IP-based variants.
- **Rate limiting** (security/traffic-policing) — simple per-IP, then per-URL-path
  via a map file; three escalating configs.
- **Circuit breakers** — `observe layer7` method and a stick-table `gpc0/gpc1`
  method, each a complete backend.
- **Health checks** — 12+ blocks from bare `check` through HTTP/Redis/MySQL/SMTP/
  LDAP/agent checks.
- **Load-balancing algorithms** — roundrobin, ACL multi-backend, `balance
  hash src,ipmask(24)`, `first` + per-server `maxconn`.

Consistent page spine: *problem statement → "Basic X example" (prose + bare
config) → variant subsections of increasing complexity → See also.* Note the
deliberate choice: **one clean config block per concept, explanation in
surrounding prose, never inline `# comments`.**

### Reference ↔ example pairing
**Indirect** — the directive reference and the tutorials live on different domains
and meet only at "See also". The closest thing to a single annotated complete
config is a *blog* post ("The Four Essential Sections of an HAProxy
Configuration") that shows one full global+defaults+frontend+backend config and
gives each directive its own `### keyword` sub-heading + paragraph. **That
co-location is exactly the gap to beat** — one page, complete annotated config,
each block citing the reference.

### Troubleshooting / FAQ / glossary
Troubleshooting lives inside the Management Guide ("Debugging and performance
issues", "Well-known traps to avoid"). **No FAQ, no glossary** anywhere — despite
heavy jargon (stick table, sticky counter, fetch, converter). Beatable.

---

## 4. nginx — the directive-reference + task-cookbook split

Two sites, intentionally non-overlapping.

### Information architecture
- **nginx.org/en/docs** — organized **by C module**. Introduction (Beginner's
  Guide, "How nginx processes a request", "Admin's guide", debugging log, …) +
  **Modules Reference** (70+ pages: `ngx_http_core`, `ngx_http_proxy`,
  `ngx_http_upstream`, `ngx_http_ssl`, …), each page one module
  (<https://nginx.org/en/docs/>).
- **docs.nginx.com/nginx/admin-guide** — organized **by job-to-be-done**:
  Load Balancer · Content Cache · Web Server · Security Controls · Monitoring ·
  High Availability · Dynamic Modules (<https://docs.nginx.com/nginx/admin-guide/>).

The split is the lesson: *"a user configuring load balancing goes to
docs.nginx.com and gets a complete recipe; a user looking up what
`proxy_buffer_size` does goes to nginx.org and finds it under ngx_http_proxy_module."*

### Teaching patterns
- nginx.org assumes you know the task and need exact **syntax**: terse functional
  prose + config, no prerequisites, no "verify" step
  (<https://nginx.org/en/docs/http/load_balancing.html>).
- docs.nginx.com **teaches the task**: concept paragraph → minimal working config
  → per-parameter prose → escalating variants → occasional "verify" step.

### Diagram usage
**None** on either site's technical pages — including the recipe pages. The config
block itself is the visual artifact. (Honest note: this means even nginx, the
most-used server, ships zero datapath diagrams — adding ours puts us ahead.)

### Getting-started UX
Beginner's Guide assumes nginx is already installed (no install→boot→200 flow),
shows control commands (`nginx -s reload`/`stop`/`quit`/`reopen`), then a
config that grows skeleton → static → proxy → FastCGI. "Success" is a stated
outcome ("nginx will send `/data/images/example.png`"), **no `curl`/expected
response** (<https://nginx.org/en/docs/beginners_guide.html>). Beatable.

### Cookbook / recipes — the strength (admin guide)
Each admin-guide page is a self-contained task recipe. **Common grammar across all
of them:**
1. Opening sentence states the task and its value.
2. Prerequisites called out before the first config.
3. **Minimal working config first**, then layered complexity.
4. Each variant block preceded/followed by prose explaining the added parameter.
5. Config blocks are **clean** (no inline comments).
6. "See Also" cross-references at the end.
7. **A "Combined Configuration Example" capstone** assembling the pieces into one
   production-ready config — the definitive copy-and-adapt artifact
   (<https://docs.nginx.com/nginx/admin-guide/content-cache/content-caching/>).

Representative recipe (rate limiting,
<https://docs.nginx.com/nginx/admin-guide/security-controls/controlling-access-proxied-http/>):
Introduction → Limiting Connections → Limiting Request Rate → **Testing the
Request Rate Limit** (a real verify step with dry-run mode) → burst → burst+nodelay
→ burst+delay → Synchronizing Zones → Limiting Bandwidth → See Also. Canonical
block:
```nginx
http {
    limit_req_zone $binary_remote_addr zone=one:10m rate=1r/s;
    server {
        location /search/ {
            limit_req zone=one burst=5 nodelay;
        }
    }
}
```
with the *why* in prose ("`$binary_remote_addr` ... holds the binary
representation ... which is shorter").

### Reference pattern — the gold standard to copy
Every module page: one-sentence module description → **Example Configuration**
block → directives alphabetically, each as the **Syntax / Default / Context**
triple, then an **Embedded Variables** section. Verbatim
(<https://nginx.org/en/docs/http/ngx_http_proxy_module.html>):
```
Syntax:   proxy_connect_timeout time;
Default:  proxy_connect_timeout 60s;
Context:  http, server, location
```
Plus: version notes ("This directive appeared in version 1.5.6"), Plus-only
markers, cross-ref hyperlinks, and parameter tables for multi-flag directives
(the `server` directive lists `weight`/`max_fails`/`fail_timeout`/`backup`/
`slow_start`/`max_conns`/…). A team can generate any reference page by filling
this template.

### Troubleshooting / FAQ / glossary
nginx.org "A debugging log" (terse reference); docs.nginx.com "Debugging NGINX"
is a real procedural workflow (verify debug build → enable debug log → core dumps
→ backtrace → "dump running config" → "Asking for help"). **No FAQ, no glossary**
on either site. Beatable.

---

## 5. Caddy — the concision + progressive-disclosure benchmark

Docs home: <https://caddyserver.com/docs/>

### Information architecture
Five tiers, each a distinct job: **Get Caddy** (install) · **Tutorials**
(Getting Started + 6 Quick-starts) · **Reference** (CLI, API, Caddyfile incl.
"Common Patterns", Modules, JSON, Automatic HTTPS) · **Articles** (narrative:
Architecture, Conventions, Logging, Monitoring, Troubleshooting) · **Developers**.
Note the explicit separation of *tutorials* (follow-along) vs *quick-starts*
(pick a task, fast result) vs *concepts* vs *articles* (explanatory narrative).

### Teaching patterns
Friendly, opinionated, concise. **Mental model first**: the Caddyfile is taught as
*"just converted to JSON for you"* — one sentence is the whole config-adapter
model, no diagram needed. **Teaching by prohibition / contrast**:
> *"Don't do this: stopping and starting the server is orthogonal to config
> changes, and will result in downtime."* (wrong-way-first, then `caddy reload`)
> — <https://caddyserver.com/docs/getting-started>

JSON-vs-Caddyfile is a tight **comparison table**, not paragraphs. Tone sample
(troubleshooting): *"you probably assume it's not DNS. Hint: it's almost always
DNS."*

### Diagram usage
Essentially **none** — by design. Examples are so minimal they self-document
(a 3-line Caddyfile is the mental model). Their diagram-substitute is a
**color-coded legend** on code blocks labeling addresses/directives/matchers.
Honest caveat for us: this only works because the product is simple; a multi-mode
L4/L7 + QUIC system like ours *does* need diagrams.

### Getting-started UX — concision benchmark
Fastest first-success in the field. Getting Started opens with a **checkbox list
of what you'll learn**, then the literal first command is `caddy run` (live on the
admin API, no config file). Then the JSON path: `curl localhost:2019/load -d
@caddy.json` → `curl localhost:2015` → **`Hello, world!` shown verbatim**, then a
zero-downtime `caddy reload`. Each Quick-start has an identical micro-structure:
**Prerequisites (2–4 bullets) → the one-liner first → Test it → a Caddyfile
equivalent → optional HTTPS variant.** The signature is **progressive disclosure
at three levels**: CLI one-liner (`caddy reverse-proxy --from :2080 --to :9000`)
→ Caddyfile → JSON API — the same feature shown at all three so readers enter at
their level. The Caddyfile quick-start in full:
```
localhost

respond "Hello, world!"
```
→ `caddy start` → `curl https://localhost` → `Hello, world!` (and it explains the
password prompt: *all* sites, even local, get HTTPS by default).

### Cookbook / recipes
"Common Patterns" inside the Caddyfile reference
(<https://caddyserver.com/docs/caddyfile/patterns>): static server, reverse proxy
(two variants), PHP, www redirect, trailing slashes, wildcard certs (DNS
challenge), SPA `try_files`, Caddy→Caddy. Each is a named heading + 1–2 sentences
+ one minimal block; explicitly *"not drop-in solutions; you will have to
customize."* Quick-starts double as the first-20-minutes recipe layer.

### Concepts / "hero concept"
**Automatic HTTPS** is treated as the product's *premise*, not a feature, and
taught relentlessly across every quick-start and its own reference page, using a
clean **binary frame** (local/internal → self-signed auto-trusted; public DNS →
public ACME) so every scenario fits one bucket. *Lesson: pick our one central
identity concept and make it pervasive.*

### Troubleshooting / FAQ / glossary / conventions
**Troubleshooting "Strategies"** is *methodology*, not an error list: "What do you
know?" → "Recognize and doubt assumptions" → "Reproduce" → "Explore"
(<https://caddyserver.com/docs/troubleshooting>). **FAQ is a stub** ("under
construction"). **No glossary.** But **"Conventions"**
(<https://caddyserver.com/docs/conventions>) is the standout structural idea:
network addresses, placeholders, **durations** (`250ms`, `90d`), and OS file
locations are defined **once** here and linked from everywhere, instead of
repeating duration syntax in every timeout directive. This is the direct antidote
to our "same fact in 4–10 pages" problem.

---

## 6. PATTERNS TO ADOPT (prioritized)

Each pattern: **who does it → why it works → the ExpressGateway page it drives.**
P0 = highest impact-per-effort and directly closes a diagnosed gap.

### P0 — must-do, closes a named gap

**P0-1. Adopt Envoy's "Life of a Request" structure for our request-lifecycle.**
*Who:* Envoy. *Why:* it's the single most effective doc technique observed — a
grounded config + numbered isolated steps + **one inline diagram per step** +
interface→behavior→consequence, opened by a topology section. *Drives:*
`arch/overview.md` (replace the 9-item numbered "L7 request path" list with a
"Life of a Request" page: a Mermaid **sequence diagram** of the full path plus a
per-stage breakdown) and the 9-cell story in `arch/protocol-model.md` (front
codec → `StrippedRequest` neutral rep → back codec → response streaming back, as
a diagram). This is the headline fix for "no diagrams + request path is a list."

**P0-2. Adopt Traefik's container-first quickstart with a visible payoff.**
*Who:* Traefik (and Caddy's concision). *Why:* zero-friction prereqs, reward
before work, output that proves value, `--scale` to show LB live. *Drives:*
`guide/getting-started.md` — lead with a `docker run` / compose path (our
Dockerfile + `docker/smoke/gateway.toml` already exist), show the **verbatim
`curl -v` 200** and a **`/metrics` snippet** as "what success looks like," then
the from-source path as the alternative. Closes "build-gated, H1-only, no
expected output."

**P0-3. Adopt the nginx/HAProxy task-cookbook with a "Combined Configuration
Example" capstone.** *Who:* nginx admin guide (recipe grammar + capstone) +
HAProxy (scenario taxonomy). *Why:* the operator journey is our weakest; a
reference can't replace task recipes. *Drives:* new `guide/cookbook.md` with 3–5
named scenarios (production HTTPS+H2, gRPC over H2/H3, rate/connection caps +
slowloris floor, graceful drain / reload, H3 rollout via `alt_svc`). Each recipe:
task statement → prerequisites → minimal config → layered variants (each block
explained in prose) → **one complete combined annotated config** → a "verify it
works" `curl`/metrics step → links into `CONFIG.md`. Use nginx's exact spine.

**P0-4. Adopt Caddy's "Conventions" page to kill redundancy — one home per fact.**
*Who:* Caddy. *Why:* directly fixes "~8 facts restated in 4–10 docs." *Drives:* a
single canonical home for each cross-cutting fact (the 9-cell matrix → `features.md`;
delegated-parsers/panic-free → `arch/overview.md`; the limitation set →
`known-limitations.md`; units/timeouts/log-format conventions → a new
`docs/conventions.md` or the glossary). Everyone else links with a one-line gloss,
never a restated paragraph. Pair with a **glossary** (next).

**P0-5. Ship a glossary — every incumbent lacks one.**
*Who:* nobody (all five have none) → our cheapest differentiation. *Why:* our
jargon load is heavy (Mode A/B, 9-cell, R8, lameduck, extended CONNECT, CID,
conntrack-hit, Maglev, CO-RE, GOAWAY). *Drives:* new `docs/glossary.md`, linked
from first use, doubling as a cross-cutting reference (Caddy-Conventions style).

### P1 — strongly recommended

**P1-1. Diagram-as-concept-introduction (Traefik rule).** Every diagram gets a
**title-case caption naming the concept** and sits *immediately before* the prose
that walks the same flow; diagrams live on conceptual pages, cookbook pages stay
text+config. *Drives:* `quic-modes.md` (Mode A route-by-CID vs Mode B two-conn
datapath), `backpressure.md` (bounded-window + read-pause propagation),
`guide/overview.md` (one architecture box). Use Mermaid throughout for
maintainability.

**P1-2. Design-intent-before-features + decision aids (Envoy + Caddy).** Open
`guide/overview.md` and `README.md` with the *belief/pitch* and a "use it when /
don't when" decision aid, not a 50-line feature wall (demote that to
"capabilities at a glance" or `features.md`). Add Caddy-style **teaching by
contrast** and a short decision tree to `comparison.md`. Closes the "link-farm
overview (2.8 links/100w)" and "README feature-wall" findings.

**P1-3. Progressive disclosure for getting-started (Caddy).** Structure the
quickstart as CLI/run → config-file → (optional) deeper, with **minimum-viable
config at every step** and **command-first, explanation-second**. Add a *second*
walkthrough (HTTPS + H2) so a newcomer isn't sent to read `config/examples/`
alone.

**P1-4. Reference schema discipline (nginx Syntax/Default/Context triple).**
Audit `CONFIG.md` so every knob has a consistent atomic entry (key · type/range ·
default · scope/reload-class · one-line description) + an **Example Configuration**
at the top of each section and a parameter table for multi-field knobs. Our
reference is already strong; make it template-consistent.

**P1-5. Symptom-organized troubleshooting + a short methodology preface.**
*Who:* Envoy (symptom domains) + Caddy (methodology). *Drives:* new
`guide/troubleshooting.md` organized by symptom (502s, config rejected at boot,
XDP attach failure, cert reload didn't take, H3 not negotiated), each entry
*symptom → likely cause → check this (`/readyz`, `/metrics`, logs) → fix*, with a
brief "how to debug" preface. Beats Envoy's link-out FAQ and Caddy's stub.

### P2 — adopt opportunistically

**P2-1. Task-named operator sections (Traefik "Expose/Secure/Observe").** When
adding the operator pages, name them by verb/task, not object. Consider an
**Observe** page (`guide/observability.md`: what to monitor, healthy baselines,
example alert rules) — currently `METRICS.md` lists metrics but nothing teaches
*what to watch*.

**P2-2. Migration/positioning on-ramp (Traefik).** A short "coming from
nginx/HAProxy/Envoy" mapping in `comparison.md` lowers the adoption barrier and
plays to our honest-positioning voice.

**P2-3. A "hero concept" through-line (Caddy's Automatic HTTPS).** Pick our one
identity concept — strongest candidate: **the protocol-neutral 9-cell bridge /
"parsing is delegated, we are not a second HTTP stack"** — and make it the
recurring spine across overview, protocol-model, and security pages (replacing
scattered restatement with one deliberate through-line).

**P2-4. "Verify it works" everywhere (Traefik/nginx).** Every getting-started and
cookbook section ends with an exact command + expected output. Make this a house
rule, not an afterthought.

---

## 7. BENCHMARK BAR

For each dimension: the best incumbent, what they actually do, and the target
ExpressGateway must hit to match or beat them.

| Dimension | Best incumbent | What the bar looks like | ExpressGateway target |
|---|---|---|---|
| **Information architecture** | Envoy / Traefik | Concept vs how-to vs reference vs ops cleanly separated; reference ordered on the same axis as the architecture; cookbook is first-class and task-named (Traefik "Expose/Secure/Observe") | One canonical home per fact; guide = teach, reference (`CONFIG.md`/`METRICS.md`) = look up, cookbook = tasks; nav predictable from the arch model |
| **Teaching** | Envoy + Caddy | Design-intent before features; mechanism→consequence sentences; teaching-by-contrast; category-before-inventory; decision aids | Narrative pages each add a mental model / decision aid / worked scenario the reference can't; overview drops below 1.0 links/100w |
| **Diagrams** | Envoy ("Life of a Request") | Grounded config → numbered steps → **one inline diagram per step**; topology section first; conceptual pages only, captioned by concept | Mermaid: request-lifecycle sequence, 9-cell translation, Mode A/B datapath, backpressure window, one arch box — every "how it works" concept is drawn |
| **Getting-started** | Traefik (UX) + Caddy (speed) | Container-first; reward before work (dashboard); **verbatim expected `curl` output**; `--scale` live LB; <~3 commands to first success | `docker run` first (<2 min to 200), verbatim `curl -v` + `/metrics` payoff, second walkthrough (HTTPS+H2), common-first-errors |
| **Cookbook** | nginx admin guide + HAProxy | Task-organized recipes: minimal config → layered variants (prose-explained) → **"Combined Configuration Example" capstone** → "See also"; scenario taxonomy | `guide/cookbook.md`: 3–5 named scenarios, each with a complete annotated combined config + verify step, citing `CONFIG.md`. **Beat them: co-locate reference+annotated example** |
| **Arch deep-dive** | Envoy | Index page (concept essays mirroring reference) + one cross-cutting narrative ("Life of a Request"); interface→behavior→consequence per step | `arch/overview.md` becomes the narrative; per-concept pages stay the index; each request stage drawn + explained why |
| **Troubleshooting** | Envoy (symptom IA) + Caddy (methodology) | Organized by symptom domain; mini-guide answers that scope first; a debugging *methodology* | `guide/troubleshooting.md` by symptom (502/config-reject/XDP/cert-reload/H3), each symptom→cause→check→fix. **Beat them: theirs are link-outs/stubs** |
| **Glossary** | *none of the five* | — (Envoy/Traefik/HAProxy/nginx/Caddy all define terms inline only) | `docs/glossary.md` — a clear win nobody else ships; doubles as cross-cutting reference |

**Net read of the bar:** on **reference depth** we already rival nginx/HAProxy.
The gap is **shape**: diagrams (match Envoy/Traefik), getting-started UX (match
Traefik/Caddy), and the task cookbook (match nginx/HAProxy). On **glossary,
co-located annotated configs, datapath diagrams, and a real troubleshooting
runbook** the incumbents leave the door open — we can *exceed* the bar cheaply.

---

## 8. Where the incumbents leave the door open (exceed the bar here)

These are consistent weaknesses across the five — low-cost ways to look best-in-class:
- **No glossary anywhere** (all five). Ours wins by existing.
- **Getting-started gaps:** Envoy's `start` is a nav hub; HAProxy has no
  end-to-end first-run; nginx's Beginner's Guide assumes it's already installed.
  A truly complete *install → boot → first 200 with shown output* path beats all.
- **Reference and annotated example not co-located** (HAProxy across domains;
  nginx clean-but-uncommented). A cookbook whose complete config is annotated
  *and* links each block to the reference entry is better than any of them.
- **Datapath/sequence diagrams are rare** — only Envoy and Traefik use diagrams
  well; HAProxy/nginx/Caddy ship almost none. Mermaid diagrams immediately
  place us with the best two.
- **FAQ/troubleshooting is thin** (Envoy links out, Caddy is a stub, HAProxy/
  nginx bury it in ops). A symptom-indexed runbook with verify steps beats them.

---

## 9. Concrete mapping to ExpressGateway pages

| ExpressGateway page | Action | Pattern(s) | Source |
|---|---|---|---|
| `guide/overview.md` | Rewrite: pitch + 1 arch diagram + plain request flow + decision aid; cut duplicated capability table | P1-2, P1-1, P0-4 | Envoy *What is Envoy*; Traefik concepts |
| `guide/getting-started.md` | Container-first + verbatim output + 2nd walkthrough + common errors | P0-2, P1-3, P2-4 | Traefik docker quickstart; Caddy quick-starts |
| `guide/capabilities.md` | Reduce to one-screen support matrix or fold "who-it-affects" into `known-limitations.md` | P0-4 | Caddy Conventions (one home) |
| `guide/comparison.md` | Keep; add compression ❌ row + a "pick X if…" decision tree + nginx/HAProxy migration mapping | P1-2, P2-2 | Traefik migrate; Caddy comparison tables |
| `guide/cookbook.md` (new) | 3–5 named annotated scenarios + combined-config capstone + verify | P0-3, P2-4 | nginx admin guide; HAProxy tutorials |
| `guide/troubleshooting.md` (new) | Symptom-indexed runbook + methodology preface | P1-5 | Envoy FAQ; Caddy troubleshooting |
| `guide/observability.md` (new) | What to watch / healthy baselines / example alerts | P2-1 | Traefik "Observe" |
| `guide/deployment-patterns.md` (new) | HA/topology (or drop README's HA claim) | P0-3 IA | Envoy deployment types |
| `docs/glossary.md` (new) | Jargon + cross-cutting conventions, linked from first use | P0-5 | (none ship one) + Caddy Conventions |
| `arch/overview.md` | "Life of a Request" rewrite: sequence diagram + per-stage interface→behavior→consequence; de-dupe delegation here | P0-1, P1-1 | Envoy *Life of a Request* |
| `arch/protocol-model.md` | Add 9-cell translation diagram + bridge pipeline diagram | P0-1, P1-1 | Envoy diagram-per-step |
| `arch/quic-modes.md` | Add Mode A vs Mode B datapath diagrams | P1-1 | Envoy/Traefik datapath |
| `arch/backpressure.md` | Add bounded-window + read-pause diagram | P1-1 | Envoy diagram-per-concept |
| `CONFIG.md` | Enforce Syntax/Default/Context-style consistency + per-section example config | P1-4 | nginx module reference |

Two accuracy fixes flagged in the diagnosis (the "11 algorithms" not
config-selectable, and "active health checking" being passive-only) sit alongside
this work — adopting the cookbook/decision-aid pattern is exactly where the
honest "round-robin/weighted are exposed; the rest are implemented but not yet
selectable" statement belongs.

---

## Appendix — URLs fetched

**Envoy:** docs home `/docs/envoy/latest/`; `/intro/life_of_a_request`;
`/intro/arch_overview/arch_overview`; `/intro/what_is_envoy`;
`/intro/arch_overview/http/http_connection_management`;
`/start/start`; `/start/quick-start/run-envoy`;
`/start/quick-start/configuration-static`; `/start/sandboxes/`;
`/start/sandboxes/front-proxy`; `/faq/overview`; `/faq/configuration/timeouts`.

**Traefik:** `doc.traefik.io/traefik/`; `/getting-started/docker/`;
`/getting-started/quick-start/`; `v2.8/getting-started/concepts/`;
`v2.8/routing/overview/`; `v2.8/providers/overview/`; Expose/Secure/Observe
sections; `/faq/`; `operations/dashboard|cli`, `observability/logs`.

**HAProxy:** `docs.haproxy.org/` (Starter Guide `intro.html`, Configuration
Manual `configuration.html`, Management Guide `management.html`);
`haproxy.com/documentation/haproxy-configuration-tutorials/` (proxying-essentials
backends + session-persistence; security/traffic-policing; reliability/
health-checks + circuit-breakers; configuration-basics); blog "The Four Essential
Sections of an HAProxy Configuration".

**nginx:** `nginx.org/en/docs/`; `/beginners_guide.html`;
`/http/request_processing.html`; `/http/load_balancing.html`;
`/http/ngx_http_proxy_module.html`; `/http/ngx_http_upstream_module.html`;
`/http/ngx_http_core_module.html`; `/debugging_log.html`.
`docs.nginx.com/nginx/admin-guide/`; `/load-balancer/http-load-balancer/`;
`/load-balancer/http-health-check/`; `/security-controls/terminating-ssl-http/`;
`/security-controls/controlling-access-proxied-http/`;
`/content-cache/content-caching/`; `/web-server/compression/`;
`/monitoring/debugging/`.

**Caddy:** `caddyserver.com/docs/`; `/getting-started`;
`/quick-starts/reverse-proxy|https|caddyfile|static-files|api`; `/conventions`;
`/caddyfile/concepts`; `/caddyfile/patterns`; `/automatic-https`;
`/architecture`; `/config-adapters`; `/faq`; `/troubleshooting`.
