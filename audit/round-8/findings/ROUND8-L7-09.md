### ROUND8-L7-09 — No authority comma / control-char rejection in any protocol parser (HAProxy `BUG/MAJOR: http: forbid comma in authority` class)

Reference: `audit/round-8/research/haproxy.md` lessons 9 + 10 — `BUG/MAJOR: http: forbid comma character in authority value` + `BUG/MEDIUM: h1: Enforce the authority validation during H1 request parsing` (H1 parser was missing the check that H2/H3 had — protocol-neutral validation requires the validation to actually run on *every* parser). `ref-l7` Top-10 #3.
Our equivalent: `crates/lb-l7/src/sni_authority.rs` (SNI-vs-host only), `crates/lb-l7/src/h2_proxy.rs:1218-1263` (`check_authority_host_agreement` — agreement only, no value sanitisation), `crates/lb-h1/src/parse.rs` (no authority validation at all)

Severity: medium
Status:   Proposed-Fix(div-l7, cherry-pick a24948d7) — PUSH-BACK(verifier=verify)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): PUSH-BACK. Commit adds ONLY crates/lb-l7/src/authority.rs (167-line validator) + lib.rs pub mod. ZERO callsites: grep authority::validate/crate::authority across lb-h1, h2_proxy, h1_proxy, lb-quic returns nothing. Plan-named proof test round8_authority_validation.rs ABSENT. The finding's divergence is precisely "validation must RUN on every parser" (HAProxy BUG/MEDIUM h1: Enforce authority validation) — an uncalled pub fn does NOT close it. Host: a,b still parses unchecked through lb-h1. Textbook Theme-1 (the plan itself invokes this failure mode). Stays Proposed-Fix. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: every protocol parser (H1, H2, H3) must reject `:authority` / `Host` values containing comma, whitespace, control characters, or that are empty. Same predicate, applied uniformly.
- **Us**:
  - H1: `parse_request_line` accepts whatever `Uri::parse` accepts (which is permissive). `Host:` header in `parse_headers` runs through the generic header-parser — no comma or control-char check.
  - H2: `check_authority_host_agreement` only compares `:authority` ↔ `Host`. It does NOT reject `Host: victim.example,attacker.example` if the same value appears in `:authority`.
  - H3: same `:authority`-via-quiche path; no validator wired.

Impact:
- The HAProxy fix exists because downstream intermediaries split on comma and bypass routing decisions. Our `sni_authority::check_sni_authority` strips port and compares case-insensitively — `Host: example.test,attacker.example` after `split_host_port` returns `(example.test,attacker.example, None)` (rsplit_once(':') has no `:` to split on); the comparison then runs against the literal string `"example.test,attacker.example"` vs the SNI `"example.test"` and *correctly* fails — but only because the whole authority is treated as one host literal. The proxy then *routes* on that literal too (the backend picker doesn't see the authority, but if a future change adds host-based routing, the comma will bite).
- Future-proofing: this is a "lesson-not-yet-paid-for" finding. We do not have host-based routing today, so the comma isn't directly exploitable. The day we add it (multi-tenant deployment, vhost routing), the comma becomes a smuggling primitive.

Reproduction:
```rust
let buf = b"GET / HTTP/1.1\r\nHost: example.test,attacker.example\r\n\r\n";
let (_, _, _, n) = lb_h1::parse_request_line(buf).unwrap();
let (h, _) = lb_h1::parse_headers(&buf[n..]).unwrap();
assert!(h.iter().any(|(k, v)| k == "Host" && v.contains(',')));  // currently true — no reject
```

Recommendation:
1. Add `lb_l7::authority::validate(value: &str) -> Result<(), AuthorityError>` enforcing:
   - non-empty after trim
   - no commas
   - no whitespace / tab
   - no control chars (`< 0x20`, `0x7F`)
   - one optional `:port` suffix with digits only
   - bracket-balanced IPv6
2. Call it from:
   - H1: in the H1 request handler before any routing / picker call.
   - H2: inside `check_authority_host_agreement` *before* the agreement compare.
   - H3: in the QUIC conn_actor's header-callback path.
3. Add regression tests with vector `Host: a,b`, `Host: a b`, `Host:` (empty), `Host: \x01host`.
4. Cross-over to `audit/round-8/findings/_ops_summary.md`: when host-based routing lands (any future pillar), this becomes critical.
