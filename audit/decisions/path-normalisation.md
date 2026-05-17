# Decision: HTTP request-path normalisation stance

**Status**: documented (closes ROUND8-L7-13 part 1)
**Round**: 8 (Phase D)
**Owner**: div-l7
**Reviewed-by**: team-lead (lead-decision R8-L-007 — bulk plan approval)
**Date**: 2026-05-14

## Decision

ExpressGateway is a **transparent forwarder** for the HTTP request
path. The path bytes received from the downstream client are
forwarded to the upstream backend verbatim. The LB does NOT:

- collapse `//` runs (Envoy's `merge_slashes`),
- resolve `.` / `..` segments (Envoy's `normalize_path = on`,
  RFC 3986 §6.2.2.3 syntax-based normalisation),
- canonicalise percent-encoding (Envoy `path_with_escaped_slashes_action`),
- lower-case the path,
- translate `\` to `/` (Windows-style),
- strip query / fragment components prior to forwarding.

Path semantics are the **backend's** responsibility for this release.

## Rationale

1. **Pingora-ethos pass-through**. ExpressGateway aspires to the
   Pingora-style transparent reverse-proxy default: the gateway
   should not silently rewrite request semantics that the backend
   is configured to trust.
2. **Path-prefix ACLs cannot move from the backend to the gateway**.
   Backends that enforce path-prefix policies (`/admin/*` =
   privileged) MUST be configured with their own canonicalising
   parser. A normalising gateway in front of a backend that runs
   on raw bytes is a known auth-bypass primitive (nginx
   CVE-2013-4547, Apache CVE-2021-41773 class).
3. **Routing not yet on the table**. We have no host-based or
   path-based routing pillar in this release. The day we add one,
   the routing decision MUST run a normaliser *before* selecting
   the backend (Pillar 4b plan gate). This document records that
   future requirement.

## Caveats — what to do when routing arrives

If a future pillar introduces path-based routing:

1. Run RFC 3986 §6.2.2 syntax-based normalisation BEFORE the
   routing decision (collapse multiple slashes, resolve `.` and
   `..`, normalise percent-encoded ASCII tchars).
2. Reject `%2F` / `%5C` inputs unconditionally (Envoy default for
   gateways doing path routing).
3. Forward the *original* path bytes to the selected backend, NOT
   the normalised form. The backend's view of the path must not
   silently change because the gateway routed on a canonical
   alias.
4. Document the routing predicate AND the normalisation predicate
   in this file at the same time.

## Optional knob — `path_escaped_slash_action`

A future config knob `[runtime].path_escaped_slash_action = "allow"
| "reject"` may ship as a tractable defence against the specific
`%2F` smuggling class without requiring a full normaliser. Default
will be `allow` to preserve pass-through semantics; operators
running into auth-bypass concerns can opt in to `reject`. This is
tracked under the L7-13 fix plan but the knob ships in a separate
PR once the config layer accepts the schema change.

## References

- nginx CVE-2013-4547 (unescaped-space URI parsing access bypass).
- Apache CVE-2021-41773 (alias-driven path traversal).
- Envoy edge best-practices: `normalize_path = on`, `merge_slashes
  = true`, `path_with_escaped_slashes_action = REJECT_REQUEST`.
- Pingora documentation on transparent proxy mode.
- `audit/round-8/findings/ROUND8-L7-13.md`.
