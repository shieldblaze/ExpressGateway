# Plan for PROTO-2-15 — SNI ↔ Host / `:authority` disagreement reject

Finding-ref:    PROTO-2-15 (high, Open — escalated from medium per
                synthesis §A; RFC anchors: RFC 6066 §3 + RFC 9110
                §7.4 + RFC 9113 §8.3.1 + RFC 9114 §4.3)
Files touched:
  - `crates/lb-l7/src/h1_proxy.rs` (TLS-accept → HTTP-parse boundary;
    extract SNI from `rustls::ServerConnection::server_name()` and
    attach via `http::Extensions`)
  - `crates/lb-l7/src/h2_proxy.rs` (mirror SNI attach for the H2-on-
    TLS listener; the SNI value lives on the TCP-level
    `ServerConnection` and is the same for every H2 stream on that
    connection)
  - `crates/lb-quic/src/conn_actor.rs` (QUIC: SNI lives on the
    `quiche::Connection` via `peer_cert_chain` / handshake state;
    expose via the per-stream `RequestContext`)
  - `crates/lb-l7/src/util/authority.rs` (the shared
    `authority_eq_host` helper from PROTO-2-01; SNI comparison
    re-uses the same host-portion normalisation)
  - `crates/lb-controlplane/src/config.rs` (config knob
    `[tls].enforce_sni_authority_match`)
  - `tests/sni_authority_mismatch_rejected.rs` (new)

Approach:
The "point of agreement" the task brief points to is the
**TLS-accept-then-HTTP-parse boundary** in `lb-l7`. For each
listener kind:

  - **H1 over TLS (`h1s`)**: after `acceptor.accept().await`
    returns a `TlsStream`, extract
    `tls_stream.get_ref().1.server_name()` (this is rustls
    0.23's `ServerConnection::server_name()` which returns
    `Option<&str>`). Stash on a `RequestContext` struct that the
    `H1Proxy` per-connection state already maintains.
  - **H2 over TLS (`h1s` + ALPN upgrade)**: same; SNI is per-TCP-
    connection, so every H2 stream on that connection compares
    against the same cached SNI.
  - **H3 over QUIC**: SNI is the value from the QUIC ClientHello;
    quiche exposes it via the underlying BoringSSL `SSL` handle.
    The h3 connection actor stores it once per `quiche::Connection`
    and exposes it through the per-request context.

After hyper parses the request (post header normalisation, after
PROTO-2-01's `validate_h2_authority`), call:

```rust
fn validate_sni_authority(
    sni: Option<&str>,
    auth_host: &str,
    is_loopback: bool,
    cfg: &TlsEnforcementCfg,
) -> Result<(), http::StatusCode> {
    if !cfg.enforce_sni_authority_match { return Ok(()); }
    if is_loopback { return Ok(()); }   // dev / unit-test exception
    match sni {
        None => Ok(()),                  // SNI absent; client is SNI-less
        Some(s) if authority_eq_host(s, auth_host) => Ok(()),
        Some(_) => Err(http::StatusCode::MISDIRECTED_REQUEST), // 421
    }
}
```

`auth_host` is the host portion of:
  - `:authority` for H2/H3 (PROTO-2-01 already validated it equals
    `Host`)
  - `Host` for H1

The reject status is **421 Misdirected Request** (RFC 9110 §15.5.20)
— this is the spec-canonical response for "the request targets an
authority the server is not configured to serve". It is the
correct status for vhost-confusion attempts; a 421 is also a hint
for HTTP/2 connection coalescing clients to retry on a fresh
connection.

**Loopback exception** (`127.0.0.0/8`, `::1`, plus unix sockets if
they ever arrive): unit tests routinely `curl localhost/` with no
SNI, and dev workflows similarly. Detect via `SocketAddr::ip()
.is_loopback()` on the listener's bind address. The exception is
config-driven (`[tls].enforce_sni_authority_match` defaults true on
non-loopback only) so production deployments cannot accidentally
opt out.

**Config knob:**

```toml
[tls]
enforce_sni_authority_match = true   # default; auto-disabled on loopback
```

**RFC anchor recap (cross-review §G.2):**
  - RFC 6066 §3: SNI carries the *intended* destination authority.
  - RFC 9110 §7.4: `Host` is the H1 target authority.
  - RFC 9113 §8.3.1: `:authority` is the H2 target authority.
  - RFC 9114 §4.3: same for H3.

Four-layer agreement is the invariant; any disagreement is a
host-confusion primitive. The fix enforces three-way agreement
(SNI ↔ Host ↔ `:authority`) at the TLS/HTTP boundary; PROTO-2-01
covers the Host ↔ `:authority` leg.

Proof:
  - Test: `tests/sni_authority_mismatch_rejected.rs::sni_mismatch_returns_421`
    Invariant: open TLS to gateway with `--servername a.test`; send
    `GET / HTTP/1.1\r\nHost: b.test\r\n\r\n` → response `421
    Misdirected Request`.
  - Test: `sni_authority_mismatch_rejected.rs::sni_match_passes`
    Invariant: same SNI as Host → 200 (assuming a real upstream
    backend).
  - Test: `sni_authority_mismatch_rejected.rs::loopback_exempt`
    Invariant: bind on `127.0.0.1`; even with mismatched SNI/Host,
    request passes (loopback exception engaged).
  - Test: `sni_authority_mismatch_rejected.rs::h2_authority_mismatch_returns_421`
    Invariant: H2 client with `:authority = b.test` over TLS with
    SNI `a.test` → stream-level 421 (or HEADERS frame with
    `:status=421`).
  - Test: `sni_authority_mismatch_rejected.rs::h3_authority_mismatch_returns_421`
    Invariant: H3 client over QUIC with mismatched SNI / authority
    → response status 421.
  - Test: `sni_authority_mismatch_rejected.rs::enforcement_disabled_passes_mismatch`
    Invariant: with `enforce_sni_authority_match = false`, a
    mismatch passes through (regression check for the opt-out).
  - Test: `sni_authority_mismatch_rejected.rs::no_sni_client_passes`
    Invariant: connect via IP literal with no SNI (rustls accepts
    SNI-less ClientHellos in some configurations); the
    `sni == None` branch passes the request through. This is the
    "rare and getting rarer" legitimate-SNI-less case the round-2
    review flagged.

Risk / blast radius:
  - **Breaking** for any deployment whose clients legitimately send
    mismatched SNI / Host. In practice this is rare (CDN-prefetch
    paths, SNI-based egress filtering boxes). Mitigation:
    `enforce_sni_authority_match = false` config opt-out plus
    documentation in the deployment guide.
  - Once SNI-based cert resolution (the inventory's §5.1 future
    feature) lands, this finding becomes the gate that prevents
    cert-A-serving-Host-B confusion — load-bearing rather than
    "nice to have".
  - Perf cost: one `Option<&str>` extraction at connection setup +
    one `&str` compare per request. Negligible.

Cross-ref:    composes with PROTO-2-01 (Host ↔ `:authority` leg) via
              the shared `authority_eq_host` helper; joint with sec
              (multi-tenant deployment-mode work, sec cross-review
              §G.2); closes PROTO-2-15.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
