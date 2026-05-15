### ROUND8-L7-09 — No authority comma / control-char rejection in any protocol parser (HAProxy `BUG/MAJOR: http: forbid comma in authority` class)

Reference: `audit/round-8/research/haproxy.md` lessons 9 + 10 — `BUG/MAJOR: http: forbid comma character in authority value` + `BUG/MEDIUM: h1: Enforce the authority validation during H1 request parsing` (H1 parser was missing the check that H2/H3 had — protocol-neutral validation requires the validation to actually run on *every* parser). `ref-l7` Top-10 #3.
Our equivalent: `crates/lb-l7/src/sni_authority.rs` (SNI-vs-host only), `crates/lb-l7/src/h2_proxy.rs:1218-1263` (`check_authority_host_agreement` — agreement only, no value sanitisation), `crates/lb-h1/src/parse.rs` (no authority validation at all)

Severity: medium
Status:   Proposed-Fix(div-l7, bf22f01a — push-back closed: validator now WIRED on H1+H2 request paths)   <!-- PUSH-BACK RESPONSE (author=div-l7, sha bf22f01a): crate::authority::validate is now called on BOTH parser paths BEFORE upstream selection — H1 at crates/lb-l7/src/h1_proxy.rs (on the Host header, before the SNI check / picker) and H2 at crates/lb-l7/src/h2_proxy.rs handle_inner (on :authority + Host, before check_authority_host_agreement / picker); reject -> 400. Proof crates/lb-l7/tests/round8_authority_enforced.rs 4/4 PASS: comma/whitespace authority -> 400 BEFORE upstream (closed backend would yield 502 if the validator were skipped — proves on-path), valid authority -> reaches upstream (502). Theme-1 "library shipped no caller" resolved. verify re-checks. See audit/round-8/verify/l7.md. -->

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
