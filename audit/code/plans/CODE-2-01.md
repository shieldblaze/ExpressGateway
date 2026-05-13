# Plan for CODE-2-01 — lb-l7 has no lb-security dep; detector wiring shim
Finding-ref:     CODE-2-01 (critical, Open)
Files touched:
  - `crates/lb-l7/Cargo.toml`                       (add dep)
  - `crates/lb-l7/src/lib.rs`                       (re-export `SecurityHooks` trait)
  - `crates/lb-l7/src/security_hooks.rs`            (NEW — trait shim)
  - `crates/lb-l7/src/h1_proxy.rs`                  (call-site insertion point, no logic)
  - `crates/lb-l7/src/h2_proxy.rs`                  (call-site insertion point, no logic)
  - `crates/lb-l7/src/ws_proxy.rs`                  (call-site insertion point, no logic)

Approach:
This plan owns ONLY the dependency edge and the type-plumbing trait
shim. The actual detector logic (SmuggleDetector / SlowlorisDetector /
SlowPostDetector / per-IP cap) is owned by `sec` under SEC-2-01/03/04/10.

Step 1 — Cargo edge. Add to `crates/lb-l7/Cargo.toml`:
```toml
lb-security = { path = "../lb-security" }
```
Verify with `cargo tree -p lb-l7 | grep lb-security` (must show one
hit). Verify no cycle by checking `lb-security/Cargo.toml` has no
back-reference to `lb-l7`.

Step 2 — Trait shim. Add `crates/lb-l7/src/security_hooks.rs`:
```rust
use http::Request;
use std::net::IpAddr;
pub trait SecurityHooks: Send + Sync + 'static {
    /// Called once per request after hyper parses headers, before bridge dispatch.
    /// Return Err(reason) to short-circuit with 400.
    fn inspect_request<B>(&self, req: &Request<B>, peer: IpAddr) -> Result<(), SecurityReject>;
    /// Called once per connection accept; bumps per-IP / per-listener counters.
    fn admit_connection(&self, peer: IpAddr) -> Result<ConnPermit, SecurityReject>;
}
pub struct ConnPermit { /* RAII decrement on Drop */ }
pub enum SecurityReject { Smuggle, RateLimited, SlowHandshake, OverCap }
```
A `NoopHooks` impl is also provided so unit tests compile without
pulling `lb-security`. `lb-security` will provide the production
`HooksBundle: SecurityHooks` impl as part of SEC-2-01.

Step 3 — Insertion points. In `h1_proxy.rs` `proxy_request` body,
immediately after the header-parsed `Request<Incoming>` is available
and before the bridge call, add a single line: `hooks.inspect_request(&req, peer)?;`
Same for `h2_proxy.rs` H2 server adapter and `ws_proxy.rs` upgrade
path. The accept-time `admit_connection` call lives at
`crates/lb/src/main.rs:1126` (owned by CODE-2-05 plan; this plan
publishes the trait that CODE-2-05 calls into).

Proof:
- `cargo tree -p lb-l7 --format '{p}' | grep -q '^lb-security'` → exit 0
  (encoded in `tests/dep_graph.rs`: `test_lb_l7_depends_on_lb_security`).
- New unit test `crates/lb-l7/tests/security_hooks.rs::noop_hooks_compiles`
  ensures the trait is implementable without lb-security as a build dep
  of the test crate.
- The full wiring proof is owned by SEC-2-01.

Risk / blast radius:
Adding the dep edge increases compile time of `lb-l7` by ~3 s (one
crate). No public API change to lb-l7. The `SecurityHooks` trait is
new; only `lb-l7` insertion sites consume it. Risk of a missed
insertion site is contained because the shim returns `Result` and the
`?` will be a compile error if any site is omitted in Round 4.

Cross-ref:    SEC-2-01/03/04/10 (sec owns concrete detectors),
              PROTO-2-10 (hyper-coverage matrix), CODE-2-05 (admit_connection caller)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
