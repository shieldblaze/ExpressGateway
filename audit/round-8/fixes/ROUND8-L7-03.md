# Plan for ROUND8-L7-03 — Reject empty header name + non-tchar chars in name

Finding-ref:   ROUND8-L7-03 (medium, status: Open)
Files touched:
  - `crates/lb-h1/src/parse.rs`              (`parse_headers_with_limit` line 153–170)
  - `crates/lb-h1/src/chunked.rs`            (`try_read_trailers` line 194 — same fix mirrored)
  - `crates/lb-h1/tests/proptest_parser.rs`  (corpus seed; cross-ref L7-14)
  - new test file: `crates/lb-h1/tests/round8_header_name_rfc9110.rs`

Approach (≤500 words):

References: **HAProxy CVE-2023-25725** (Critical CVSS 9.1, empty
header name truncated parsed list) and **nginx CVE-2019-9516**
(zero-length names exhausted memory). Two failure modes on the same
primitive. RFC 9110 §5.1: `field-name = token`, `token = 1*tchar`.

Add a strict `is_tchar` predicate per RFC 9110 §5.6.2 and apply it
during name extraction in `parse_headers_with_limit`:

```rust
const fn is_tchar(b: u8) -> bool {
    matches!(b,
        b'!' | b'#' | b'$' | b'%' | b'&' | b'\'' | b'*' | b'+'
      | b'-' | b'.' | b'^' | b'_' | b'`' | b'|' | b'~'
      | b'0'..=b'9' | b'a'..=b'z' | b'A'..=b'Z')
}
```

Loop change:
1. After extracting `name = line[..colon_pos]` (raw bytes — *not*
   trimmed; whitespace inside the name is itself a violation), check:
   - `name.is_empty()` → `Err(H1Error::InvalidHeader("empty header name"))`
   - Any byte fails `is_tchar` → `Err(H1Error::InvalidHeader("invalid header name byte"))`
2. Do NOT `.trim()` the name. The current code `line_str.get(..colon_pos)?.trim().to_string()` admits both leading and trailing whitespace, which is wrong per RFC 9112 §5.1 ("no whitespace allowed between header field-name and colon"). Strip the `.trim()` from the name extraction; keep it for the value (RFC 9110 §5.5 OWS allowed around the value).

Mirror the same check in `chunked.rs::try_read_trailers` line 194
(`if let Some(colon) = line_str.find(':')`) — trailer parsing must
not be more permissive than header parsing.

Reference pattern: hyper `proto/h1/role.rs::parse_headers` validates
field-name via `httparse::is_token`. nginx `ngx_http_parse_header_line`
rejects non-token at the lexer-state-machine layer.

Proof:
  - `round8_header_name_rfc9110::empty_name_rejected` — invariant: input `b":value\r\n\r\n"` returns `Err(InvalidHeader)`.
  - `round8_header_name_rfc9110::whitespace_in_name_rejected` — invariant: `b"X Token: v\r\n\r\n"` and `b" X-Token: v\r\n\r\n"` (leading whitespace before name) both `Err`. Note: leading-whitespace on header-line is *line folding* per RFC 9112 §5.2 and is already obs-folded out; this test confirms the predicate behaves correctly on a non-folded line.
  - `round8_header_name_rfc9110::control_char_in_name_rejected` — invariant: `b"X\x01Token: v\r\n\r\n"` returns `Err`.
  - `round8_header_name_rfc9110::trailer_name_validation_mirrors` — invariant: trailer name `:foo` in a chunked trailer block rejects identically.
  - `round8_header_name_rfc9110::valid_token_chars_accepted` — invariant: every tchar from RFC 9110 §5.6.2 is accepted.
  - `round8_header_name_rfc9110::value_whitespace_still_trimmed` — invariant: regression for value-side OWS — `b"X-Token:   v\r\n\r\n"` parses with value `"v"`.

Risk / blast radius:
  - Strict rejection of `.trim()`-stripped header names: any client
    that today sends `" X-Token: v"` (space before name) currently
    parses as `("X-Token", "v")` after the trim. After the fix, it
    rejects. The behaviour was accidental and arguably wrong (RFC
    9112 §5.1 forbids it); fix mirrors hyper and nginx.
  - Tests in the broader workspace that rely on the lax behaviour
    must be audited; grep `lb-h1/tests/` for fixtures containing
    leading-whitespace header names. None expected.
  - `H1Error::InvalidHeader` is converted to `400 Bad Request` by
    the L7 layer; no protocol-level escape needed.

Cross-ref:
  - L7-05 (`headers_with_underscores`) — orthogonal predicate on
    the *same* name field; both apply.
  - L7-14 (proptest CVE corpus) — adds the `:value` seed.
  - L7-09 (authority comma) — distinct predicate (value side, not
    name side).

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: prior rounds
    audited `lb-h1/src/parse.rs` for content-length / transfer-encoding
    smuggling but never re-walked the primitive header-name lexer
    against RFC 9110 §5.1. The HAProxy CVE-2023-25725 reference
    landed during Round 6's window; nobody walked it back into the
    matrix.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
