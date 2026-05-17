### ROUND8-L7-13 — No URI / path normalisation (Envoy edge `normalize_path` + `merge_slashes` + `path_with_escaped_slashes_action`)

Reference: `audit/round-8/research/envoy.md` lesson 15 (Envoy edge: path-confusion mitigations are *mandatory*; three independent knobs because path confusion is many bugs not one); `audit/round-8/research/nginx.md` lesson 2 (CVE-2013-4547 unescaped-space URI parsing access bypass). `ref-l7` Top-10 #4 / Defensive pattern #4.
Our equivalent: `crates/lb-h1/src/parse.rs:41-58` (`parse_request_line` — passes URI verbatim to `Uri::parse`), `crates/lb-l7/src/*.rs` (no path normaliser exposed)

Severity: medium
Status:   Verified-Fixed(verifier=verify, 27171e2e)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): Part 1 (primary deliverable) — audit/decisions/path-normalisation.md substantive: transparent-forwarder decision, rationale, CVE-2013-4547/CVE-2021-41773 citations, future-routing normaliser gate. Finding is fundamentally "no documented choice"; ADR closes it. NON-BLOCKING CAVEAT: optional Part-2 reject-%2F knob + round8_path_normalisation.rs contract test NOT delivered, so a future silent normalisation change will not fail CI as the plan intended. Logged. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: edge proxies expose three independent knobs:
  - `normalize_path = on` (percent-decode + dot-segment removal per RFC 3986)
  - `merge_slashes = true` (collapse `//+` to `/`)
  - `path_with_escaped_slashes_action = REJECT_REQUEST` (reject `%2F` in path)
  All three exist because path confusion is many bugs:
  - `/admin/../public/x` ≠ `/public/x` if the backend normalises but we don't (or vice versa)
  - `/admin//x` ≠ `/admin/x` likewise
  - `/admin%2Fsecret` ≠ `/admin/secret` likewise
- **Us**: no normalisation. Whatever the client sends is forwarded verbatim. The H1 parser calls `uri_str.parse::<Uri>()` and accepts what `http::Uri` accepts.

Impact:
- AuthZ bypass on backends that normalise *differently* than the proxy. Pingora explicitly chose NOT to normalise (transparent forwarder ethos) — this is one acceptable design. Envoy explicitly chose to normalise — also acceptable.
- The **divergence** is: we have not *documented* a choice. The Pingora-ethos choice (pass-through) requires explicit docs saying so, plus a per-backend "the backend is responsible for canonical-form matching" caveat.
- Practical attack class: a multi-tier deployment where our LB is one hop and the backend's ACL is path-prefix matching. Without consistent normalisation between us and the backend, the attacker's `/protected/..%2Fpublic/x` slips past either side depending on who normalises.

Reproduction:
- Static evidence: `crates/lb-h1/src/parse.rs:41-58` — `Uri::parse` is the only step; no normalisation pass.
- No `path::normalize` function anywhere in `lb-l7`.

Recommendation:
1. Document the *current* choice in `audit/decisions/path-normalisation.md`: "ExpressGateway is a transparent forwarder; path normalisation is the backend's responsibility. Operators deploying us in front of backends with path-prefix ACLs must run the backend in a normalising mode."
2. Optional: add `[runtime].normalize_path = false | "rfc3986"` as a deferred config knob (does not need to ship now). The reject-on-`%2F` variant is a one-line check in `parse_request_line` and is cheap to add.
3. Add a test that pins the current behaviour: `/admin/../public/x` is forwarded verbatim. This is a *contract test* — operators relying on pass-through must see it not regress.
4. Note for `div-ops`: any future host-based / path-based routing pillar must run the normaliser BEFORE the routing decision (else CVE-2013-4547-class bypass).
