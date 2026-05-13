# Plan for PROTO-2-14 — `tls13_only` config switch (TLS 1.2 still enabled by default)

Finding-ref:    PROTO-2-14 (medium, Open — lead synthesis §E.2 retained
                medium severity despite sec's downgrade-to-low in their
                cross-review; lead's call governs)
Files touched:
  - `crates/lb-controlplane/src/config.rs` (new `[tls].min_protocol`
    field, default `"1.2"`)
  - `crates/lb-security/src/ticket.rs` (`build_server_config`
    consumes the new field) — note this is **rel-owned** per
    synthesis §D for REL-2-03 (cert rotation); proto's edit lands
    after rel's, and proto's change is additive (one new parameter
    + one branch on `with_protocol_versions`)
  - `crates/lb/src/main.rs:214,611` (caller sites threading the
    config) — code-owned; proto's parameter add layered after code's
    CODE-2-01/03/05/06
  - `tests/tls_min_protocol.rs` (new)
  - `RUNBOOK.md` / `DEPLOYMENT.md` (rel-owned; cross-ref note)

Approach:
Per lead synthesis §E.2: **add the knob, default to current
behaviour.** Future deprecation is deferred to a later audit
round when proto + sec produce evidence that no relevant
upstream/client requires TLS 1.2.

**Config addition:**

```toml
[tls]
min_protocol = "1.2"   # default; "1.3" enables TLS 1.3 only
```

```rust
// crates/lb-controlplane/src/config.rs
#[derive(Deserialize, Debug, Clone, Default)]
pub struct TlsConfig {
    /// Minimum TLS protocol version: "1.2" (default) or "1.3".
    #[serde(default = "default_min_protocol")]
    pub min_protocol: TlsVersion,
    // … other tls fields …
}

#[derive(Deserialize, Debug, Clone, Copy)]
pub enum TlsVersion {
    #[serde(rename = "1.2")] V1_2,
    #[serde(rename = "1.3")] V1_3,
}

impl Default for TlsVersion {
    fn default() -> Self { TlsVersion::V1_2 }   // lead-mandated default
}

fn default_min_protocol() -> TlsVersion { TlsVersion::V1_2 }
```

**`build_server_config` consumption:**

```rust
// crates/lb-security/src/ticket.rs (additive parameter)
pub fn build_server_config(
    certs: ...,
    keys: ...,
    min_version: TlsVersion,
) -> Result<ServerConfig, ...> {
    let versions: &[&'static rustls::SupportedProtocolVersion] =
        match min_version {
            TlsVersion::V1_3 => &[&rustls::version::TLS13],
            TlsVersion::V1_2 => rustls::ALL_VERSIONS, // [TLS13, TLS12]
        };
    let builder = rustls::ServerConfig::builder_with_provider(
            rustls::crypto::ring::default_provider().into())
        .with_protocol_versions(versions)?
        .with_no_client_auth();
    // … rest unchanged …
}
```

**Startup warning when `min_protocol = 1.2`:** at bind time, emit
`tracing::info!(target = "lb_security::tls",
"TLS 1.2 enabled (min_protocol = \"1.2\"); set to \"1.3\" for
TLS-1.3-only operation per NIST SP 800-52r2 hardening guidance");`.
This is informational, not a warn — the default remains 1.2 per
lead decision and emitting `warn` for default config is noisy.

**Future deprecation pathway** (out of scope for this round, but
documented inline in the config struct as a `// TODO(audit-r4+):`
comment):
  1. Future round flips default to `TlsVersion::V1_3` once
     telemetry from production deployments confirms zero TLS-1.2
     handshakes over a measurement window (sec owns the
     measurement).
  2. The eventual hard-removal of TLS 1.2 support requires the
     workspace-level `tls12` rustls feature to flip off, which is
     a separate dep-change; not blocked on this plan.
  3. Until that future round lands, operators can opt into
     1.3-only with `[tls].min_protocol = "1.3"`.

Proof:
  - Test: `tests/tls_min_protocol.rs::default_accepts_tls_1_2`
    Invariant: default config, `openssl s_client -tls1_2 -connect
    gateway:8443` succeeds (handshake completes). Use rustls
    `ClientConfig` in test rather than openssl shell-out;
    `with_protocol_versions(&[&rustls::version::TLS12])`.
  - Test: `tests/tls_min_protocol.rs::tls13_only_rejects_tls_1_2`
    Invariant: with `min_protocol = "1.3"`, a TLS 1.2 client
    handshake fails with rustls `Error::InappropriateMessage` (or
    the equivalent "no supported versions"). Inspect the alert.
  - Test: `tests/tls_min_protocol.rs::tls13_only_accepts_tls_1_3`
    Invariant: with `min_protocol = "1.3"`, a TLS 1.3 client
    succeeds.
  - Test: `tests/tls_min_protocol.rs::serde_round_trip`
    Invariant: parse `min_protocol = "1.2"` and `"1.3"`; reject
    `"1.1"` and arbitrary strings with a clear error message.
  - Test (regression): `tests/tls_min_protocol.rs::default_is_v1_2`
    Invariant: `TlsConfig::default().min_protocol` is
    `TlsVersion::V1_2`. Locks in the lead-mandated default until a
    future round explicitly changes it.

Risk / blast radius:
  - **No behaviour change for any existing deployment** (the
    default mirrors current behaviour exactly). Only operators who
    explicitly opt into `"1.3"` see a stricter handshake.
  - Forward-compat note: when the future round flips the default,
    the `default_is_v1_2` regression test above is the deliberate
    canary — its failure signals the breaking change to lead.
  - No perf cost; the version list is a const slice resolved at
    `build_server_config` call time.

Cross-ref:    composes with REL-2-03 (cert rotation Round-4 edit
              touches `ticket.rs` first; proto layers the parameter
              add after); informs sec compliance posture
              (FedRAMP / PCI 4.0); closes PROTO-2-14.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
