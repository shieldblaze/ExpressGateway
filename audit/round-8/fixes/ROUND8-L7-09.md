# Plan for ROUND8-L7-09 — Uniform authority validation across H1 / H2 / H3 parsers (HAProxy `forbid comma in authority` class)

Finding-ref:   ROUND8-L7-09 (medium, status: Open)
Files touched:
  - `crates/lb-l7/src/authority.rs`         (NEW — single validator)
  - `crates/lb-l7/src/lib.rs`               (pub mod authority)
  - `crates/lb-h1/src/parse.rs`             (call validator in request-line + Host header)
  - `crates/lb-l7/src/h2_proxy.rs`          (`check_authority_host_agreement` calls validator on each side before comparing)
  - `crates/lb-l7/src/sni_authority.rs`     (cross-check: sni validator already exists; ensure path parity)
  - `crates/lb-quic/src/...`                (H3 path; call validator in the QUIC conn_actor's header-callback)
  - new test file: `crates/lb-l7/tests/round8_authority_validation.rs`

Approach (≤500 words):

References: **HAProxy `BUG/MAJOR: http: forbid comma character in
authority value`** + **`BUG/MEDIUM: h1: Enforce the authority
validation during H1 request parsing`** — H1 parser missed the check
that H2/H3 had; protocol-neutral validation requires the validation
to actually run on *every* parser. RFC 9110 §4 authority component
definition.

Design: a single, protocol-neutral validator.

```rust
// crates/lb-l7/src/authority.rs
pub enum AuthorityError {
    Empty,
    Comma,
    Whitespace,
    Control,
    UnbalancedBrackets,
    InvalidPort,
}

pub fn validate(value: &str) -> Result<(), AuthorityError> {
    if value.is_empty() { return Err(AuthorityError::Empty); }
    for b in value.bytes() {
        match b {
            b',' => return Err(AuthorityError::Comma),
            b' ' | b'\t' => return Err(AuthorityError::Whitespace),
            0..=0x1F | 0x7F => return Err(AuthorityError::Control),
            _ => {}
        }
    }
    // IPv6 bracket balance:
    let opens = value.bytes().filter(|&b| b == b'[').count();
    let closes = value.bytes().filter(|&b| b == b']').count();
    if opens != closes || opens > 1 { return Err(AuthorityError::UnbalancedBrackets); }
    // Port suffix: if the value has a final ':<digits>' after the
    // host or after ']', verify digits-only.
    // ...detailed parsing per RFC 3986 §3.2 host = IP-literal / IPv4 / reg-name
    Ok(())
}
```

Call sites:
1. **H1** (`lb-h1/parse.rs`): in `parse_request_line` for the
   request-URI's authority component when present (CONNECT method
   case), AND in `parse_headers_with_limit` for `Host:` header.
   Reject → `H1Error::InvalidHeader`.
2. **H2** (`lb-l7/h2_proxy.rs::check_authority_host_agreement`): run
   `validate` on BOTH the `:authority` pseudo-header AND the `Host:`
   header *before* the agreement comparison. Reject the request
   stream with PROTOCOL_ERROR if either fails.
3. **H3** (`crates/lb-quic/`): the H3 header-callback path (likely in
   `conn_actor.rs` — verify with `grep ':authority' crates/lb-quic`).
   Same validator call.

**SNI cross-check:** the existing `lb-l7/src/sni_authority.rs::
check_sni_authority` already strips port and compares case-
insensitively, but does not run the comma/whitespace/control
predicate. Call `validate(authority)` *before* the agreement compare.

Reference pattern: HAProxy 2.9 `src/http_htx.c::http_authority_validate`
runs identical predicate on every parser. Nginx
`ngx_http_validate_host` is the C cousin.

**Boundary disclosure:** the H3 wire-in touches `lb-quic` — that
remains div-l7's house per the org chart. No `lb/src/main.rs` changes.

Proof:
  - `round8_authority_validation::comma_rejected_h1` — invariant: H1 request with `Host: a,b` returns `400 Bad Request`.
  - `round8_authority_validation::comma_rejected_h2_authority` — invariant: H2 stream with `:authority = a,b` is reset with PROTOCOL_ERROR.
  - `round8_authority_validation::comma_rejected_h2_host_header` — invariant: H2 stream with valid `:authority` but `Host: a,b` is reset.
  - `round8_authority_validation::whitespace_rejected_h1` — invariant: `Host: a b` → 400.
  - `round8_authority_validation::control_char_rejected_h1` — invariant: `Host: \x01host` → 400.
  - `round8_authority_validation::empty_authority_rejected` — invariant: `Host: ` (whitespace-only) → 400.
  - `round8_authority_validation::ipv6_brackets_must_balance` — invariant: `Host: [::1` and `Host: ::1]` both → 400; `Host: [::1]:8080` is OK.
  - `round8_authority_validation::port_digits_only` — invariant: `Host: example.com:abc` → 400.
  - `round8_authority_validation::sni_authority_runs_validator` — invariant: SNI `example.test` + `Host: example.test,attacker.com` → 421 Misdirected Request OR 400 (depending on which check trips first); the validator-before-comparison change must trigger consistently.

Risk / blast radius:
  - Strictly tightens. Any legacy operator with intentionally weird
    authority values (rare; CONNECT to a comma-named pseudo-host)
    rejects. None expected.
  - The bracket-count predicate is a quick approximation of RFC
    3986's full grammar. A formal parse would use the `http::uri`
    crate's authority parser; if available, prefer that.

Cross-ref:
  - L7-13 (path normalisation) — adjacent surface; path canonical
    form is a separate predicate.
  - L7-04 (XFF iteration) — same theme of "trust the wire value,
    don't over-normalise".
  - SEC-2-15/-18 (SNI / Host agreement) — already in register; this
    plan extends those by sanitising the *value* before agreement.

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: SEC-2-15/-18
    audited SNI/Host agreement; comma/control-char value sanitisation
    was never re-walked by the proto validator. HAProxy paid for
    this twice (H2 fix + H1 fix); we have neither.
  - **Theme 3 — Doc-vs-code claim drift**: register implies
    "authority is validated" via SNI agreement; the *value* sanitisation
    was implicitly assumed but never coded.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
