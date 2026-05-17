# Plan for ROUND8-L7-04 — XFF / Via iterate-and-append, never get-and-clobber

Finding-ref:   ROUND8-L7-04 (high, status: Open)
Files touched:
  - `crates/lb-l7/src/h1_proxy.rs`        (`append_xff` line 1125–1138, `append_via` line 1157–1171)
  - `crates/lb-l7/src/h2_proxy.rs`        (mirror — H2 has a parallel XFF append; grep `append_xff` in the crate)
  - `crates/lb-security/src/*.rs`         (audit every `headers.get(...)` for multi-value-header consumers — flag with `// iterate-or-known-single-valued` comments)
  - new test file: `crates/lb-l7/tests/round8_xff_iteration.rs`

Approach (≤500 words):

Reference: **Envoy GHSA-ghc4-35x6-crw5** (High, Mar 2026) — RBAC matched
on a comma-joined string built from `headers.get_all(...)`; duplicate
headers bypassed the rule because the joined string did not match the
single-value regex. The handoff (`_l7_handoff.md` Top-10 #4) and
`ref-l7`'s prediction #1 specifically called this out.

Today `append_xff` calls `HeaderMap::get(&XFF_NAME)` — which returns
only the *first* `HeaderValue`. The function then `format!("{prev},
{peer_ip}")` and `HeaderMap::insert(...)` — which *replaces all
values*. Effect: if the request carries `X-Forwarded-For: a` AND
`X-Forwarded-For: b` (two header lines), the forwarded request has
one `X-Forwarded-For: a, <peer>` — `b` is silently dropped.

Fix shape:

```rust
fn append_xff(headers: &mut HeaderMap, peer: SocketAddr) {
    let peer_ip = peer.ip().to_string();
    // Iterate every existing value; collect into a single comma-joined
    // string per RFC 7239 / RFC 7230 list-rule. Skip values that fail
    // to_str (non-ASCII binary) — these are policy violations that
    // get logged separately; we do not silently strip them.
    let mut joined = String::new();
    for v in headers.get_all(&XFF_NAME).iter() {
        match v.to_str() {
            Ok(s) => {
                if !joined.is_empty() { joined.push_str(", "); }
                joined.push_str(s);
            }
            Err(_) => {
                // Log + counter; do not include in the joined string.
                // Optionally fail-closed (drop the request); the
                // policy decision is captured in [runtime].xff_policy
                // (deferred).
            }
        }
    }
    if !joined.is_empty() { joined.push_str(", "); }
    joined.push_str(&peer_ip);
    // Replace all values with the single canonical value.
    if let Ok(hv) = HeaderValue::from_str(&joined) {
        headers.insert(&XFF_NAME, hv);
    }
}
```

Apply the *identical* pattern to `append_via` and any other producer.

**Audit-side hardening (broader scope, capped):**
1. Grep `crates/lb-security/src/` and `crates/lb-l7/src/` for every
   `headers.get(` and `headers.get_all(` site. For each `.get(`
   that reads a header documented as list-valued (XFF, Via,
   Cache-Control, Cookie — though Cookie is special-case per
   RFC 6265 §5.4), either:
   - convert to `.get_all(...).iter()` with explicit iteration, OR
   - add a `// SAFETY: known-single-valued header per <spec ref>`
     comment so the next refactor cannot silently re-introduce the
     bug.
2. Cookie handling (`crates/lb-security/src/cookie.rs` if present)
   stays out of scope for this plan — Cookie's list rule uses `; `
   not `, ` and has its own RFC 6265 §5.4 sequence; ROUND8-L7-04
   does not own that. Flag any Cookie-related divergence as a
   separate finding under Round-9 if discovered during the audit
   pass.

**Boundary disclosure:** the audit-of-audit pass through
`lb-security` is *informational* under this plan — the fix is
strictly the `append_xff` / `append_via` rewrite. The grep audit
produces a list of comment-only changes; if any actual bug is
found, escalate as a new finding (do not silently fix under this
plan).

Proof:
  - `round8_xff_iteration::two_xff_headers_preserved_in_join` — invariant: request with two `X-Forwarded-For` lines (values `1.1.1.1` and `2.2.2.2`) arrives at upstream as one `X-Forwarded-For: 1.1.1.1, 2.2.2.2, <peer>` (or three lines — the test asserts the *number of comma-separated values* not the line-vs-fold form).
  - `round8_xff_iteration::three_xff_headers_count_preserved` — pin the count: `get_all().iter().flat_map(split ',').count() == 4` after fix (3 original + 1 peer).
  - `round8_xff_iteration::via_append_preserves_existing_chain` — same shape for Via.
  - `round8_xff_iteration::single_xff_unchanged_format` — invariant: a request with one XFF header arrives at upstream as `existing, <peer>` (no regression on the common single-value path).
  - `round8_xff_iteration::no_xff_header_inserts_peer_only` — invariant: a request with zero XFF headers arrives with `X-Forwarded-For: <peer>` (no regression on the empty path).

Risk / blast radius:
  - Behaviour change is observable to upstream: previously-dropped
    XFF values are now forwarded. This is the *correct* change; any
    downstream that was unknowingly relying on the silent-drop has
    bigger problems.
  - HeaderValue ASCII-encoding constraint: if any prior hop sent a
    XFF header containing non-ASCII bytes, the `to_str()` call
    fails and we silently skip that entry. The current behaviour
    would `format!` the failure-mode string (`<invalid>`) into the
    new value, which is also wrong. The plan documents
    "fail-closed-by-skip" as the chosen disposition; an explicit
    knob can be added later.

Cross-ref:
  - Bundle: none (this finding stands alone).
  - L7-14 (proptest) — the grep audit's comment-only changes feed
    one new proptest assertion: multi-value-header roundtrip count
    preserved through the proxy.
  - L7-09 (authority comma rejection) — distinct primitive, but
    same theme of "the parser accepts ambiguous list syntax".

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: prior rounds audited
    smuggle predicates (`lb-security/src/smuggle.rs`) but never
    walked the *producer* side (`append_xff` writing to upstream).
    The Envoy GHSA-ghc4-35x6-crw5 advisory landed Mar 2026, after
    the prior rounds; the proto validator's smuggle matrix is
    consumer-side only. Plan adds a producer-side pass.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
