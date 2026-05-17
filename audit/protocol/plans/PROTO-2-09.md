# Plan for PROTO-2-09 — Hard-error on unknown `protocol = …`

Finding-ref:    PROTO-2-09 (high, Open — escalated from medium per
                synthesis §A)
Files touched:
  - `crates/lb/src/main.rs` (the `match listener_cfg.protocol.as_str()`
    final arm at ~line 837, returning `_ => Ok(ListenerMode::PlainTcp)`;
    note synthesis §D allocates this work to proto under
    `crates/lb-l7/src/listener.rs` — `build_listener_mode` actually
    lives in `lb/src/main.rs` today, so Round 4 either moves the
    helper to `lb-l7/src/listener.rs` first (lead-approved file shift)
    or applies the fix in-place in `main.rs` with proto's edits
    serialised after code's CODE-2-01/03/05/06 lands per the
    synthesis §D conflict rule)
  - `crates/lb-controlplane/src/config.rs` (or wherever
    `Config::validate` lives — a fresh `grep -n "fn validate" crates/lb-controlplane/src/*.rs` at Round-4 start identifies it)
  - `tests/listener_protocol_validation.rs` (new)

Approach:
The current code:

```rust
match listener_cfg.protocol.as_str() {
    "plain-tcp" => Ok(ListenerMode::PlainTcp),
    "tls"       => Ok(ListenerMode::Tls(...)),
    "h1"        => Ok(ListenerMode::H1(...)),
    "h1s"       => Ok(ListenerMode::H1s(...)),
    "h3"        => Ok(ListenerMode::H3(...)),
    _           => Ok(ListenerMode::PlainTcp),       // ← BUG
}
```

A typo (`"h2"`, `"https"`, `"h1S"`, `"  h1s"` with whitespace,
empty string, etc.) silently produces a plain-TCP forwarder.

Two-layer fix:

**Layer 1 — `Config::validate` rejects unknown strings before bind.**
In `lb-controlplane::Config::validate` (the entry called during
config load), add a per-listener check:

```rust
const ALLOWED_PROTOCOLS: &[&str] =
    &["plain-tcp", "tls", "h1", "h1s", "h3"];

for (i, lst) in self.listeners.iter().enumerate() {
    if !ALLOWED_PROTOCOLS.contains(&lst.protocol.as_str()) {
        anyhow::bail!(
            "listener[{i}]: unknown protocol {:?}; \
             expected one of: {}",
            lst.protocol,
            ALLOWED_PROTOCOLS.join(", "),
        );
    }
}
```

The allow-list is **exhaustive**; new protocols added later require
explicit updates here (a TODO comment binds the list to the
`ListenerMode` enum variants).

**Layer 2 — `build_listener_mode` becomes total.** The final arm
becomes:

```rust
other => anyhow::bail!(
    "unknown listener protocol {other:?}; \
     this should have been caught at config load — \
     please file a bug. Expected one of: \
     plain-tcp, tls, h1, h1s, h3"
),
```

This is defence in depth: if Layer 1 is bypassed (e.g. a future
hot-reload path that mutates config without re-validating), the
bind still fails loudly rather than silently downgrading to plain-TCP.

**`plain-tcp` is opt-in.** The plain-TCP L4 forwarder remains
available but must be explicit; there is no default fallback. The
parser also rejects `protocol` being omitted (rather than treating
absence as "plain-tcp"); `lb-controlplane` likely already does this
via `#[serde(default)]` absence — verify in Round 4.

Cross-team integration:
  - `code` (CODE-2-01/03/05/06) is editing `main.rs` first per the
    synthesis §D serialisation rule. proto's edit on the
    `build_listener_mode` arm lands after code's rebase.
  - `sec`'s strict-mode proposal (sec cross-review §H.1) wants
    `[runtime].strict_mode = true` as default; this plan does not
    block on that. Even with `strict_mode = false`, an *unknown*
    protocol string is an outright error (operator typo, not a
    relaxed policy choice).

Proof:
  - Test: `tests/listener_protocol_validation.rs::config_validate_rejects_https_typo`
    Invariant: `Config { listener: [{ protocol: "https", ... }] }
    .validate()` returns `Err` with message containing
    `"unknown protocol \"https\""` and the allowed list.
  - Test: `tests/listener_protocol_validation.rs::config_validate_rejects_uppercase`
    Invariant: `protocol = "H1S"` is rejected (case-sensitive
    match by design).
  - Test: `tests/listener_protocol_validation.rs::config_validate_rejects_empty`
    Invariant: `protocol = ""` is rejected.
  - Test: `tests/listener_protocol_validation.rs::config_validate_accepts_plain_tcp`
    Invariant: `protocol = "plain-tcp"` validates.
  - Test: `tests/listener_protocol_validation.rs::build_listener_mode_is_total`
    Invariant: directly call `build_listener_mode` with
    `protocol = "h2"`; assert `Err`. (Belt + braces for Layer 2.)

Risk / blast radius:
  - **Breaking change** for any deployment that currently relies on
    a typo silently producing plain-TCP. By definition such
    deployments are misconfigured; lead-approved escalation rests
    on this being a safe-default fix.
  - Operator must change every TOML where the protocol string is
    not in the allow-list. Mitigation: ship release-note guidance
    naming the allow-list explicitly and the error message that
    surfaces on startup.
  - No runtime cost (config-load-time check).

Cross-ref:    closes PROTO-2-09; feeds sec strict-mode proposal
              (SEC cross-review §H.1); operator-safety pair with
              CODE-2-13 (control-plane wiring).
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
