### ROUND8-L7-05 — No `headers_with_underscores` rejection at edge (Envoy edge L#14, nginx default OFF)

Reference: `audit/round-8/research/envoy.md` lesson 14 (Envoy edge best-practice: `headers_with_underscores_action = REJECT_REQUEST`); `audit/round-8/research/nginx.md` lesson "defensive pattern 2" (`underscores_in_headers` defaults OFF). Both converge: edge proxies reject underscore headers because some app servers normalise `_` ↔ `-` and that's an auth-bypass primitive.
Our equivalent: `crates/lb-h1/src/parse.rs:153-170` (no underscore rejection); `crates/lb-l7/src/h1_proxy.rs` (no policy); `crates/lb-config/src/lib.rs` (no config knob)

Severity: medium
Status:   Verified-Fixed(verifier=verify, 11d6b9dd)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): HeaderUnderscorePolicy (Reject default) enforced on H1 hot path in handle_inner post-into_parts (Reject->400/Drop->remove/Allow), H2 mirror, smuggle-strict forces Reject. round8_underscore_policy 6/6 PASS (default-pin + predicate corpus + drift marker). Enforcement code code-inspected on hot path; predicate trivially correct. Caveat: config->proxy enum mapping deferred to lb-binary wave (documented); default ctor already Reject. See audit/round-8/verify/l7.md. -->

Divergence:
- **Envoy**: `headers_with_underscores_action = REJECT_REQUEST` at the edge per official best-practice doc. Default in modern Envoy: REJECT.
- **nginx**: `underscores_in_headers off` is the default; underscore-bearing headers are silently dropped.
- **Us**: no policy. `parse_headers_with_limit` accepts any byte the http crate doesn't reject. `HeaderName::from_static` in our HOP_BY_HOP table only handles dash-named hop-by-hop. An incoming `X-Internal_Token: abc` is forwarded transparently to the upstream, which (Java middleware, some Python frameworks, SAP, weird cloud signing) may normalise it to `X-Internal-Token` — bypassing the auth check the upstream applies on the canonical-dash form.

Impact:
- AuthZ bypass on backends that treat `_` and `-` equivalently. The Envoy edge guidance exists *because* this is a real attack vector (`X-Internal-Token` is the canonical auth header in many shops; `X-Internal_Token` evades the proxy's reject rule but reaches the app as the same thing).
- Even without that specific app behaviour, accepting underscore headers means we cannot ever reject them in the future without breaking traffic — better to set the policy now.

Reproduction:
```rust
let buf = b"X_Auth_Token: secret\r\nX-Auth-Token: legit\r\n\r\n";
let (h, _) = lb_h1::parse_headers(buf).unwrap();
assert!(h.iter().any(|(n, _)| n.contains('_')));  // currently true — no reject
```

Recommendation:
1. Add `[runtime].header_underscore_policy = "reject" | "drop" | "allow"` to `lb-config`. Default `"reject"` (REJECT_REQUEST → 400 Bad Request), `"drop"` strips the header silently (matches nginx default), `"allow"` is the current behaviour kept for migration.
2. In `parse_headers_with_limit`, after extracting the name, reject `name.bytes().any(|b| b == b'_')` when policy = reject.
3. In `lb-security/src/smuggle.rs::check_all_mode`, add an `H1Edge` mode that enforces the reject policy; thread it from `lb-l7` based on the runtime config.
4. Document in `audit/deferred.md` if any operator needs the `allow` option (e.g., specific app integration).
