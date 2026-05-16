# Plan for ROUND8-L7-13 — Document path-normalisation stance + ship reject-`%2F` knob (Envoy edge `normalize_path` class)

Finding-ref:   ROUND8-L7-13 (medium, status: Open)
Files touched:
  - `audit/decisions/path-normalisation.md`   (NEW — architectural decision record)
  - `crates/lb-config/src/lib.rs`             (`path_with_escaped_slashes_action` knob)
  - `crates/lb-h1/src/parse.rs`               (apply the knob in `parse_request_line`)
  - `crates/lb-l7/src/h1_proxy.rs`            (thread knob from config)
  - `audit/deferred.md`                       (record `normalize_path = on` as a deferred config knob)
  - new test file: `crates/lb-l7/tests/round8_path_normalisation.rs` (contract tests pinning pass-through behaviour)

Approach (≤500 words):

References: **Envoy edge** lesson 15 — `normalize_path = on`,
`merge_slashes = true`, `path_with_escaped_slashes_action =
REJECT_REQUEST` are all *separate* knobs because path confusion is
many bugs not one. **nginx CVE-2013-4547** — unescaped-space URI
parsing access bypass. The finding observes we have *no* normalisation
and have not *documented* a choice.

Two-part plan:

**Part 1 — Documented stance (the primary deliverable):**

Create `audit/decisions/path-normalisation.md` per the existing
`audit/decisions/` pattern. Content outline:
- *Decision*: ExpressGateway is a transparent forwarder; path
  normalisation is the backend's responsibility for this release.
- *Rationale*: Pingora-ethos pass-through. Backends with path-prefix
  ACLs must run in a normalising mode and validate canonical form
  themselves. The LB does NOT translate path semantics.
- *Caveat*: any future host-based or path-based routing must run a
  normaliser BEFORE the routing decision (else CVE-2013-4547-class
  bypass). Pillar 4b plan must include this gate.
- *Cite*: nginx CVE-2013-4547, Envoy edge best-practices,
  Pingora's transparent-forwarder ethos.
- *Reviewed-by*: list of audit roles that signed off.

Add a contract test that pins the *current* behaviour so a future
silent change to add normalisation will fail CI:

```rust
#[test]
fn path_dot_dot_passed_through_verbatim() {
    let req = b"GET /admin/../public/x HTTP/1.1\r\nHost: x\r\n\r\n";
    // through the L7 proxy; assert backend sees `/admin/../public/x`
}
```

**Part 2 — Cheap reject-`%2F` knob (optional but tractable):**

Even with the pass-through stance, the `path_with_escaped_slashes_action
= REJECT_REQUEST` knob is a one-line check that defends against the
specific `%2F` smuggling class without requiring a full RFC 3986
normaliser. Ship the knob, default OFF, document.

```toml
[runtime]
path_escaped_slash_action = "allow"   # or "reject"
```

```rust
// parse.rs (after parse_request_line):
if policy == PathEscapedSlashAction::Reject {
    if let Some(target) = req.target_bytes() {
        if window_contains_ci(target, b"%2F") || window_contains_ci(target, b"%5C") {
            return Err(H1Error::InvalidUri("escaped slash"));
        }
    }
}
```

Reference pattern: Envoy's `http_connection_manager.proto` field
`path_with_escaped_slashes_action`. The full `normalize_path` knob
is *deferred* per the architectural decision.

**Boundary disclosure:** the optional knob's call-site is in
`lb-h1/parse.rs` (already div-l7's house). No `lb/src/main.rs`
changes.

Proof:
  - `round8_path_normalisation::dot_dot_segment_passed_through` — contract: `/admin/../public/x` arrives at the backend verbatim. Pins the pass-through stance.
  - `round8_path_normalisation::double_slash_passed_through` — contract: `/admin//x` verbatim.
  - `round8_path_normalisation::percent2f_passed_through_default` — contract: with `path_escaped_slash_action = "allow"` (default), `/admin%2Fsecret` verbatim.
  - `round8_path_normalisation::percent2f_rejected_when_configured` — invariant: with `path_escaped_slash_action = "reject"`, `/admin%2Fsecret` returns `400 Bad Request`; the upper-case `%2F` and lower-case `%2f` both reject.
  - `round8_path_normalisation::backslash_escape_rejected_when_configured` — invariant: `%5C` also rejects (Windows-style path-traversal kin).
  - `round8_path_normalisation::valid_paths_unaffected` — regression: `/users/123` and `/api/v1/health?ok=1` pass under both policies.

Risk / blast radius:
  - Documentation-heavy change; behaviour-default is *no change*
    from today (pass-through). The optional knob ships as default-OFF.
  - The contract tests pin pass-through behaviour and will fail any
    future change to add normalisation silently. This is intentional:
    add normalisation only via a new architectural decision record.

Cross-ref:
  - L7-15 (edge-defaults parity) — this knob is one row of the
    canonical edge defaults table.
  - L7-09 (authority validation) — sibling primitive; authority
    sanitisation is required, path normalisation is optional with
    documented stance.
  - For `div-ops`: any future host- or path-based routing pillar
    must run the normaliser BEFORE the routing decision; cite this
    plan's `audit/decisions/path-normalisation.md` as the gating
    artefact.

**Audit-failure-mode theme:**
  - **Theme 3 — Doc-vs-code claim drift**: the audit register does
    not document the pass-through path stance. The
    `audit/decisions/` directory exists for exactly this purpose;
    no entry for path normalisation has ever been written. This
    plan creates the missing artefact.
  - **Theme 4 — Multi-validator audit handoff**: the protocol
    validator audited authority sanitisation; the path predicate
    was an adjacent surface that was never walked.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
