### ROUND8-L7-03 — Empty header name silently accepted (HAProxy CVE-2023-25725 / nginx CVE-2019-9516 class)

Reference: `audit/round-8/research/haproxy.md` lesson 1 (HAProxy CVE-2023-25725, Critical CVSS 9.1 — empty header name truncated the parsed list); `audit/round-8/research/nginx.md` lesson 5 (CVE-2019-9516 — zero-length names exhausted memory). Two big proxies, two different failure modes for the *same* primitive.
Our equivalent: `crates/lb-h1/src/parse.rs:153-170` (`parse_headers_with_limit`)

Severity: medium
Status:   Verified-Fixed(verifier=verify, 5812f593)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): RFC9110 is_tchar; empty + non-tchar name rejected (no .trim() on name); mirrored in chunked.rs trailers. round8_header_name_rfc9110 9/9 PASS. Bypass: embedded-space/NUL/control/leading-ws all rejected; trailer :foo rejects identically; no bypass. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: parsers MUST reject zero-length header name fields at the first opportunity. RFC 9110 §5.1 field-name = token, where token is 1\*tchar.
- **Us**: the loop computes `name = line_str.get(..colon_pos)?.trim().to_string()` and pushes `(name, value)` unconditionally. A header line `: somevalue\r\n` produces `("", "somevalue")`. A line `   : x\r\n` produces `("", "x")` after trim. Subsequent code paths in `lb-l7` and `lb-security/hooks.rs` then loop over `headers` with empty-string names; the smuggling detector checks `name.eq_ignore_ascii_case("content-length")` which is `false` for the empty name → empty-named CL/TE headers leak past the detector entirely.
- Combined: an attacker can send `:Content-Length: 10\r\n` after a real CL header. We accept both; `check_duplicate_cl` only sees the second as `"content-length"`. The empty-name entry is then forwarded verbatim to the upstream, which depending on its parser either drops the line (HAProxy ≤2.5.11 truncation) or treats it differently — classic CL desync.

Impact:
- Cross-proxy smuggling primitive (HAProxy pre-fix grade Critical 9.1). Realistically: defence against a HAProxy / nginx upstream that has its own off-by-one on empty-name handling.
- Header bloat DoS (nginx CVE-2019-9516 angle): an attacker can submit 256+ empty-named headers up to `MAX_HEADER_BYTES = 65_536`, all parked in a `Vec<(String, String)>`. We do not cap header count distinct from byte budget; an attacker who blows the byte budget with empty-named lines is constrained by that, but every accepted line still allocates a String.

Reproduction:
```rust
let buf = b":foo\r\n:bar\r\n\r\n";
let (h, _) = lb_h1::parse_headers(buf).unwrap();   // currently Ok with [("", "foo"), ("", "bar")]
assert!(!h.iter().any(|(n, _)| n.is_empty()));     // FAILS
```

Recommendation:
1. In `parse_headers_with_limit`, after computing `name`, reject `name.is_empty()` with `H1Error::InvalidHeader("empty header name".into())`.
2. Also reject `name.contains(|c: char| !is_tchar(c))` (RFC 9110 tchar set) — covers control chars, whitespace inside the name.
3. Add proptest seed for `:value\r\n` and assert `Err`.
4. Mirror the check in `chunked.rs::try_read_trailers` (line 194 — `if let Some(colon) = line_str.find(':')`) so trailer parsing has the same rejection.
