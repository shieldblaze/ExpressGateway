# `rel` → `sec` — round 1 handoff

Three items where my round-1 inventory needs your input:

1. **`/healthz` on the metrics admin listener** — `crates/lb-observability/src/admin_http.rs:55`.
   Returns unconditional 200, no auth, no TLS. My posture: fine on
   loopback. Please flag if you disagree (e.g. you want mTLS on the
   admin listener in round 2).

2. **PII in logs.** Round-1 grep finds **no redaction filter** in the
   `lb-l7` crates. Default `RUST_LOG=info` is plain text. Need your
   read on whether request URLs / cookies / `Authorization` headers
   surface anywhere. Specifically check `crates/lb-l7/src/h1_proxy.rs`
   and `h2_proxy.rs` for `tracing::*!(?req)` or similar.

3. **Secrets at rest.**
   - QUIC retry secret: `crates/lb-quic/src/listener.rs:291`
     (`load_or_generate_retry_secret`). My read: writes 32 bytes; mode
     check pending.
   - TLS ticket key (`crates/lb-security/src/ticket.rs::TicketRotator`)
     — is it ephemeral per-process? If yes, every restart invalidates
     issued tickets; if persisted, file mode + rotation overlap need a
     joint policy.

Pointers in my round-1 inventory: §0 H4, §1 F-07/F-19, §5.6, §7.2, §9.

No findings yet — round 2 lands the IDs.
