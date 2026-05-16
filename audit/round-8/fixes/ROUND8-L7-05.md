# Plan for ROUND8-L7-05 — `headers_with_underscores` policy (default REJECT_REQUEST at edge)

Finding-ref:   ROUND8-L7-05 (medium, status: Open)
Files touched:
  - `crates/lb-config/src/lib.rs`         (new `header_underscore_policy` enum)
  - `crates/lb-h1/src/parse.rs`           (`parse_headers_with_limit` consults policy)
  - `crates/lb-l7/src/h1_proxy.rs`        (policy threaded from runtime config)
  - `crates/lb-l7/src/h2_proxy.rs`        (same policy applies to H2)
  - `crates/lb-security/src/smuggle.rs`   (`check_all_mode` learns `H1Edge` and `H2Edge` variants — or extends existing modes)
  - `config/default.toml`                 (document the new knob)
  - new test file: `crates/lb-l7/tests/round8_underscore_policy.rs`

Approach (≤500 words):

References: **Envoy edge** lesson 14 — `headers_with_underscores_action
= REJECT_REQUEST` is the documented edge best-practice (Envoy default
is `ALLOW`, but the edge-deployment guide *mandates* REJECT). **nginx**
default — `underscores_in_headers off` (silent drop). Both converge:
underscore-bearing headers are an auth-bypass primitive against
backends that normalise `_` ↔ `-` (Java middleware, some Python
frameworks, SAP).

Config schema:
```toml
[runtime]
# "reject" | "drop" | "allow"; default "reject"
header_underscore_policy = "reject"
```

In `lb-config`, add:
```rust
pub enum HeaderUnderscorePolicy { Reject, Drop, Allow }
impl Default for HeaderUnderscorePolicy { fn default() -> Self { Self::Reject } }
```

Plumbing:
1. Add a `policy: HeaderUnderscorePolicy` field to whatever per-listener
   config struct `ProxyService` receives. Pass it into the H1 parser
   on construction.
2. `parse_headers_with_limit` signature gains `policy:
   HeaderUnderscorePolicy`. After name extraction (post the L7-03
   `is_tchar` pass), inspect:
   - `Reject` → if any byte in name is `b'_'`, return `Err(H1Error::
     InvalidHeader("underscore in header name"))` → 400.
   - `Drop` → silently skip this header (don't push to the vec).
   - `Allow` → keep current behaviour.
3. For H2 (`h2_proxy.rs`), hyper does not run our parser — we must
   intercept after hyper parses headers. Iterate `Request::headers()`
   and apply the same predicate (Reject → 400; Drop → `headers.remove(name)`;
   Allow → noop).
4. `lb-security/smuggle.rs::check_all_mode` — if smuggle mode is
   `H1Edge`, force the policy to `Reject` regardless of config (defence
   in depth). Pure-pass-through deployments can opt out by setting
   smuggle mode to `H1Tolerant`.

Reference pattern: Envoy's `http_connection_manager.proto` field
`headers_with_underscores_action` plus the edge best-practices doc.
nginx `ngx_http_parse_header_line` with `underscores_in_headers`
inspecting at the lexer state machine.

`config/default.toml` documents the choice:
```toml
[runtime]
# Per Envoy edge best-practice + nginx default. Underscore in header
# names is rejected because some backends normalise `_` ↔ `-` and
# that is an auth-bypass primitive. Set to "allow" only if you know
# why.
header_underscore_policy = "reject"
```

**Boundary disclosure:** the H2 wire-in touches the response-build
path in `h2_proxy.rs` (already in div-l7's house). No `lb/src/main.rs`
changes — the runtime config struct already flows through to the
service constructor.

Proof:
  - `round8_underscore_policy::reject_underscore_in_name_default` — invariant: with default config, `X_Internal_Token: secret` returns `400 Bad Request`.
  - `round8_underscore_policy::drop_underscore_strips_silently` — invariant: with `policy = "drop"`, the header is removed before forwarding; backend never sees it; response is 2xx (assuming the test request otherwise succeeds).
  - `round8_underscore_policy::allow_underscore_forwards_verbatim` — regression: with `policy = "allow"`, current pass-through behaviour is preserved.
  - `round8_underscore_policy::dash_named_token_unaffected` — invariant: `X-Auth-Token: legit` passes under all three policies.
  - `round8_underscore_policy::h2_path_applies_same_policy` — invariant: send the request over H2; same reject behaviour (HEADERS frame with `:authority` + an `x_token` pseudo or regular header → 400 or stream-level reset).
  - `round8_underscore_policy::smuggle_mode_h1edge_forces_reject` — invariant: with `policy = "allow"` and smuggle mode `H1Edge`, the underscore header is *still* rejected (defence in depth).

Risk / blast radius:
  - Behaviour change for any operator running against backends that
    use underscore-named headers internally (rare; usually only
    legacy SAP / mainframe gateways). Mitigation: the knob defaults
    to `reject` but exists as `allow` for migration.
  - H2 path requires iterating Request headers before forwarding;
    minor allocation churn. Acceptable.

Cross-ref:
  - L7-03 (empty name + non-tchar rejection) — sequential predicates
    on the same field.
  - L7-15 (edge-defaults parity) — this knob feeds the canonical
    edge defaults table.

**Audit-failure-mode theme:**
  - **Theme 4 — Multi-validator audit handoff**: the Envoy edge
    best-practices doc has been canonical for years; nginx default
    is older still. The audit never imported the canonical edge
    baseline as a checklist. L7-15 (edge-defaults parity) is the
    meta-finding; L7-05 is one of its constituents.

Owner:           div-l7
Lead-approval: approved 2026-05-14 team-lead-r8
