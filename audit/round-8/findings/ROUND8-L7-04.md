### ROUND8-L7-04 — `X-Forwarded-For` append clobbers all but the first value (Envoy GHSA-ghc4-35x6-crw5 class)

Reference: `audit/round-8/research/envoy.md` lesson 3 (GHSA-ghc4-35x6-crw5, High — RBAC bypassed by repeating headers because matcher saw the comma-joined string). Handoff item 4: "any place we string-join multi-value headers and then run a regex / match / contains on the joined result. Iterate the values list instead." `ref-l7` predicted this *specifically* as a likely undetected divergence.
Our equivalent: `crates/lb-l7/src/h1_proxy.rs:1125-1138` (`append_xff`), `crates/lb-l7/src/h1_proxy.rs:1157-1171` (`append_via`)

Severity: medium
Status:   Verified-Fixed(verifier=verify, 8d2fdcae)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): append_xff/append_via iterate get_all + comma-join all values + single insert. round8_xff_iteration 5/5 PASS. Bypass: 3-header preserved, inner commas preserved, obs-fold rejected by hyper pre-parse; no silent value drop. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: when a header has multiple values, every value must be preserved and iterated; values must not be joined into a single string for matching, and the *first* value must not be the only one read for append.
- **Us**: `append_xff` calls `headers.get(&XFF_NAME)` — `HeaderMap::get` returns *only the first* value. The function then `format!("{prev}, {peer_ip}")` and `headers.insert(...)` — `HeaderMap::insert` **replaces all values**. Effect: every existing XFF header beyond the first is silently dropped from the forwarded request. Same shape in `append_via` (line 1157).

```rust
// h1_proxy.rs:1127
let new_value = headers.get(&XFF_NAME).map_or_else(
    || peer_ip.clone(),
    |existing| existing.to_str().map_or_else(..., |prev| format!("{prev}, {peer_ip}")),
);
// ...
headers.insert(&XFF_NAME, v);    // ← removes any 2nd, 3rd, ... XFF header that was present
```

Impact:
- Trust-chain breakage. Operators relying on the full XFF chain for audit logging or per-hop rate limiting see only `<first_hop>, <us>` — any intermediate proxies' addresses are erased. A multi-tier deployment that uses XFF for client-IP attribution will silently mis-attribute.
- Envoy-class RBAC bypass: any downstream consumer of our forwarded XFF that *iterates* over `get_all("x-forwarded-for")` will see only one entry, while a peer that *joins* sees `"hop1, hop2, …, peer"`. Two consumers disagree on the value set — the exact shape of GHSA-ghc4-35x6-crw5.
- Operationally severe if an upstream WAF authorises based on "the XFF chain must end with one of these trusted hops" — by collapsing to one value we strip the trust evidence.

Reproduction:
```rust
let mut h = HeaderMap::new();
h.append(&XFF_NAME, HeaderValue::from_static("1.1.1.1"));
h.append(&XFF_NAME, HeaderValue::from_static("2.2.2.2"));
let peer: SocketAddr = "9.9.9.9:0".parse().unwrap();
append_xff(&mut h, peer);
assert_eq!(h.get_all(&XFF_NAME).iter().count(), 1);    // currently 1 — should be 3
// expected after fix: ["1.1.1.1", "2.2.2.2", "9.9.9.9"] or one merged comma list with all three
```

Recommendation:
1. Iterate `headers.get_all(&XFF_NAME)`, collect every existing value, append `peer_ip`, and emit a single comma-joined header (or N `append` calls — both are RFC 7230 list-rule compliant). Prefer the explicit append-per-value form so downstream code that iterates sees the same number of entries.
2. Same fix for `append_via` (line 1157).
3. Audit every other `headers.get(...)` site in `lb-l7` and `lb-security`. Tag every multi-value header read as "iterate or known-single-valued".
4. Add a test that confirms multi-value XFF survives transit; pin the count.
