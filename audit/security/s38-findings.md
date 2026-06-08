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

## Findings table (live)

| ID | Sev | Surface | Title | Status |
|----|-----|---------|-------|--------|
| _(none filed yet — Phase 1 in progress)_ | | | | |

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
