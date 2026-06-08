# ExpressGateway — Session 38 Security Findings

**Base:** main @ b8a99078 · branch `feature/security-audit-s38` · **Date opened:** 2026-06-08

Every finding: **ID · severity (R6) · surface · PoC · mechanism · disposition**
(FIXED-this-session / proven-tiered-carried / documented-accepted-risk). No finding is
asterisked (R4): proven exploitable or ruled out. A clean scope is recorded under §Proven-Clean
with the defense + the test that proves it.

Severity scale (R6): CRITICAL/HIGH (RCE, auth bypass, smuggling, wire-reachable memory unsafety,
LB-down DoS, TLS/cert bypass) → FIXED ALWAYS · MEDIUM (info leak, bounded DoS, hardening gap) →
fixed-if-tractable else tiered · LOW/hardening → fixed-if-cheap else carried.

---

## Findings table (live — Phase 1 complete, 4 auditors)

**Severity tally: CRITICAL 0 · HIGH 0 · MEDIUM 1 · LOW 7 · INFO 4.** No CRITICAL/HIGH, no
product-fork, no dependency-implicating finding. Clean scopes are PROVEN (defense + test, R4),
not asserted — consistent with a 37-session-hardened codebase whose wire parsing is delegated to
hyper/h2/quiche/rustls/tungstenite. Detail in `s38-findings-{parser,protocol,resource,infra}.md`.

| ID | Sev | Surface | Title | Disposition |
|----|-----|---------|-------|-------------|
| F-RES-1 | **MEDIUM** | H1 slowloris | hyper `header_read_timeout` INERT (no `.timer()` wired) → header phase bounded by 60s `total`, not 10s `header` | **FIX** (h1_proxy.rs:684) |
| F-INFRA-01 | LOW(sec) | TLS/retry secret | retry-secret LOAD path doesn't perm-check existing file (asymmetric vs TLS key) → forge/Mode-A-flood-bypass if world-readable | **FIX** (listener.rs:481, passthrough.rs:~1260) |
| F-RES-5 | LOW | slowloris watchdog | sweeper only logs+removes, never closes socket; `progress()` called once (header) → slow-POST eviction dead | FIX-or-document |
| F-RES-2 | LOW | H2 client | upstream `Http2Pool` builder omits `max_header_list_size` (relies on h2 16KiB default) | **FIX** (parity, http2_pool.rs:425) |
| F-RES-3 | LOW | QUIC | router `max_connections` hardcoded 100k, not config-wired; no per-IP QUIC sub-cap | tier/fix |
| F-PARSE-3 | LOW | test-codec | lb-h1 `parse_chunk_size_hex` `checked_shl` inert behind 16-digit cap (comment wrong) | FIX (comment) |
| F-PROTO-01 | LOW | H1 CL/TE | smuggle detector skips header pairs whose value fails `to_str()` (opaque bytes) — not exploitable | FIX-or-document |
| F-PARSE-1 | LOW | test-codec | lb-h2 hpack `decode_string` add + no bomb cap (no prod call-site) | document |
| F-PARSE-2 | LOW | test-codec | lb-h3-testcodec `decode_frame` total-len add (benign w/ cap) | document |
| F-RES-4 | INFO | doc | `HttpTimeouts::header` stale doc-comment (feeds F-RES-1) | FIX (doc) |
| F-PROTO-02 | INFO | gRPC | 200 path skips hop-by-hop strip (H2 binary, moot) | document |
| F-PROTO-03 | INFO | WS | echoes client subprotocol w/o backend confirm | document |
| F-PROTO-04 | INFO | H3 front | backend-head parser doesn't tchar-validate names (QPACK binary, moot) | document |

**Baseline flake (NOT a finding, pre-existing on main):** `reload_under_traffic::proof_c_restart_required_no_silent_rebind`
failed run 2/3 at the **pre-reload sanity assert (line 704)** — `get_backend_id == None` right after
`boot()`, under full-suite saturation. NOT a reload-honesty bug (it's before the reload). Load-induced
boot-readiness flake; run 1/3 passed. To characterize: isolated re-run (quiet box) — see §Baseline-flake.

---

## Findings detail

<!-- Template:
### F-S38-NN · <SEV> · <surface>
- **Auditor:** <who> · **Reproduced-by:** <verifier>
- **PoC:** <test/file path + how to run; the input>
- **Mechanism:** <exact code path + why it's a bug>
- **Impact:** <what an attacker achieves>
- **Disposition:** FIXED (commit) | TIERED (severity+mechanism+exploitability) | ACCEPTED (rationale)
- **Fix:** <diff summary> · **Regression test:** <path> · **Negative control:** <proves pre-fix FAILS>
- **Single-sourced check (R12):** <siblings re-verified>
-->

---

## Baseline flake (CF-S38-RELOAD-BOOT-FLAKE) — characterized, NOT a finding

`reload_under_traffic::proof_c_restart_required_no_silent_rebind` failed **run 2/3** of the cold
baseline ×3 (verdict `fmt=0 clippy=0 test=2/3`). Root cause: the panic was at **line 704 — the
PRE-reload sanity assert** (`get_backend_id(&listener) == Some("A")` immediately after `boot()`),
not in the reload logic. Under full-suite saturation (3 back-to-back `--all-features` suites each
spawning many in-proc gateways + 4 concurrent Opus auditor agents), the freshly-booted gateway's
first probe got `None` (boot-readiness/CPU-starvation race in the TEST harness's `boot()` probe).
**Confirmation (R2/R15):** isolated re-run of the binary on a quiet box = **5/5 PASS** (10/10 tests
each); runs 1/3 and 3/3 of the ×3 also passed it. → load-induced flake, pre-existing on main, NOT a
reload-honesty defect and NOT session-introduced. Matches the CF-SATURATION-1 / "16 in-proc
gateways" pattern. **Disposition:** do NOT weaken the test (R5). Carry-forward CF-S38-RELOAD-BOOT-FLAKE;
optional deflake = harden `boot()` to do a real request-round-trip readiness wait (test-robustness,
not a product change) — handed to the perf/burn-in phase. The final promote ×3 will be run with
controlled parallelism to avoid the saturation flake.

## Known carry-forwards reviewed (not new findings)

### CF-S7-RHU — `request_h3_upstream` 30s wall-clock cap, fails-safe
- `h3_bridge.rs:164` documents it: the H1→H3 / H2→H3 upstream request has a fixed 30s
  wall-clock cap (vs the response side's idle-reset). A slow-but-legitimate upload can be
  truncated at 30s. **Security verdict: NOT a finding** — it fails CLOSED: truncation returns
  `Err(RespAbort::PrematureEof)` + `Reset`, NEVER `End`, so no partial is presentable as
  complete (no response-splitting / cache-poison). Availability edge only; documented carry-forward.

---

## Proven-clean scopes (defense identified + tested)

<!-- Per R4: a scope is "clean" only when the defense exists AND a test proves it holds
adversarially. Record: surface · the attack tried · the defense · the test that proves it. -->

_(to be filled as auditors prove defenses)_
