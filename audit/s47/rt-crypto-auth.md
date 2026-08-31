# S47 — Red-team: crypto / secrets / auth surface

**Auditor:** rt-crypto-auth · **Base:** `review/s47-rfc-security` (main @ 01915a77)
**Method:** read + grep + manual data-flow tracing only. **No cargo command was run** (2 vCPU / 7 GB / 11 GB free box — hard rule). Every finding is traced by hand from source to sink and cited `file:line`. No PoC was executed; PoCs below are written for the lead to run in CI.

**Prior art read before reporting:** `SECURITY.md`, `audit/security/s38-findings.md`, `audit/security/s38-findings-infra.md`, `audit/deferred.md` §sec, `docs/known-limitations.md`, `docs/guide/DEPLOYMENT.md`, `README.md`.

**Tally: 0 CRITICAL · 0 HIGH · 1 MEDIUM · 3 LOW · 4 INFO.**

---

## Wiring determinations (required before any severity is meaningful)

| Component | Wired into the `lb` binary? | Evidence |
|---|---|---|
| `lb-cp-client` | **NO — STUB-DEAD, confirmed** | `crates/lb/Cargo.toml` does **not** list it; the only workspace references are `Cargo.toml:21` (members) and `Cargo.toml:162` (workspace dep table). No `use lb_cp_client` anywhere. Its own module doc (`crates/lb-cp-client/src/lib.rs:1-3`) says so: *"No transport is implemented … nothing outside it links against it. `CpClient::connect` only flips a bool."* → **any finding here is INFO by construction.** |
| `lb-controlplane` | **YES, but only as a TOML-shape pre-filter** | `crates/lb/src/main.rs:41`, `:2140-2141` (`FileBackend::new(config_path)` + `ConfigManager::new`), `:337` (`mgr.reload()` on SIGHUP). Its weak `validate()` (`crates/lb-controlplane/src/lib.rs:198-209`: non-empty + parses as `toml::Table`) is **followed** by the full `lb_config::parse_config` + `validate_config` at `main.rs:349,364`, with `rollback_to_previous()` on any failure. → **not a trust-boundary gap.** ALREADY-KNOWN by design (`main.rs:313-315` states it). |
| `AdminAuthGate` / admin HTTP | **YES** | `main.rs:2244` `validate_bind` → `main.rs:2251` `serve_with_auth`. `serve`/`serve_with_probes` (auth-less) have **no production call site** — only `tests/metrics_endpoint.rs` and `crates/lb-observability/tests/health_endpoints.rs`. |
| `RetryTokenSigner` | **YES** | `crates/lb-quic/src/listener.rs:220`, `router.rs:161` (verify), `passthrough.rs:570,591` (mint/verify). |
| `ZeroRttReplayGuard` | **YES (but 0-RTT itself is OFF)** | `router.rs:173`. `enable_early_data()` is never called on any client-facing server config — proven by `crates/lb-quic/tests/s19_b6_zero_rtt_rejection.rs:3-6`; TCP/TLS side is `audit/deferred.md` SEC-2-13. Its live job is retry-token replay dedup, not 0-RTT. |
| `TicketRotator` | **YES** | `main.rs:795`, ticked every 60 s by `spawn_rotator_ticker` (`main.rs:816-846`). |
| `raw_proxy` (Mode B) SNI | **YES** | `main.rs:1037` → `raw_proxy.rs:52` → `raw_proxy.rs:164 dial_dedicated(addr, &backend.sni, …)` → `quic_pool.rs:324` → `quic_pool.rs:373 quiche::connect(Some(sni), …)`. |

---

## Findings

### [MEDIUM] TLS private-key permission check is absent on the cert-**reload** path (CWE-732, CWE-276)

- **CVSS:** `CVSS:3.1/AV:L/AC:L/PR:L/UI:N/S:U/C:H/I:N/A:N` (5.5) — impact is disclosure of the server's TLS private key to any local reader; the gateway's failure is that it silently *accepts* the exposure it is documented to refuse.
- **Location:** `crates/lb/src/main.rs:264` `reload_all_tls` → `:270` `lb_security::reload_tls_bundle(...)` → `crates/lb-security/src/ticket.rs:452-462` → `TlsConfigBundle::load_from_paths_with` (`ticket.rs:~370`). The check exists **only** at `crates/lb/src/main.rs:792`.
- **Class:** incorrect permission assignment / missing check on one of two load paths.
- **Wiring status: LIVE.** Reached by SIGUSR1 (`main.rs:2498`) on every TLS listener in `tls_reload_registry`.

**Summary.** `assert_key_perm_advisory` has exactly **one** call site in the tree — `crates/lb/src/main.rs:792`, inside `build_tls_bundle`, i.e. startup only:

```rust
fn build_tls_bundle(tls_cfg: &TlsConfig, alpn: &[&[u8]]) -> ... {
    assert_key_perm_advisory(Path::new(&tls_cfg.key_path))?;   // main.rs:792 — the ONLY call site
```

The SIGUSR1 hot-reload path does not go through `build_tls_bundle`. It calls `reload_tls_bundle` directly, which loads cert and key with a bare `std::fs::File::open` and no permission gate:

```rust
// crates/lb/src/main.rs:264,270
fn reload_all_tls(registry: &[TlsReloadEntry], metrics: Option<&CertMetrics>) -> (usize, usize) {
        match lb_security::reload_tls_bundle(          // no perm check anywhere below this
            &entry.bundle, &entry.cert_path, &entry.key_path, &alpn_slices, Some(ticketer),
        ) {
```

**Root cause / data flow.** Two load paths for the same asset; the security check was attached to one of them. `grep -rn 'assert_key_perm_advisory\|assert_owner_only' crates/` returns exactly: `key.rs` (definition + its own tests), `lb-security/src/lib.rs:47` (re-export), `main.rs:769,771,792`, `lb-quic/src/listener.rs:341`, `lb-quic/src/passthrough.rs:994`. Nothing in the reload path.

**Why this is not a re-report of F-INFRA-01.** S38 raised F-INFRA-01 for the *retry secret* and justified it by contrast: `audit/security/s38-findings-infra.md` states *"Contrast the TLS private key, which IS perm-checked on every load (startup **AND SIGUSR1 reload**)"*. That statement is **factually wrong** — the retry secret was hardened (`listener.rs:363-368`, `passthrough.rs:1016-1020`, correctly) while the asymmetry it was compared against still exists in the opposite direction. `SECURITY.md` inherits the same claim. This is a new finding plus a documentation correction.

**Attack.** Attacker = any local unprivileged user on the gateway host (a co-tenant container sharing a volume, a compromised sidecar, a low-privilege ops account). Certbot / cert-manager / a config-management run rewrites `key.pem`; the renewal hook sends `SIGUSR1`. If the new key lands with a wide umask (`0644` is the default for `install`/`cp` without an explicit mode, and for a `tar` restore), the gateway loads it and serves from it with **no warning and no error, even in a release build** — where the startup path would have hard-failed via `KeyPermError::TooPermissive`. The attacker reads the private key. rustls is ECDHE-only so recorded traffic stays safe, but the key permits active impersonation/MITM of the gateway's identity for the certificate's lifetime.

Rotation is precisely where key permissions drift, so the *unchecked* path is the more reachable one.

**PoC (lead runs in CI).**
```sh
# boot with a 0600 key (startup check passes), then simulate a renewal that widens perms
chmod 0644 "$KEY_PATH"      # what a bad umask / restore produces
kill -USR1 "$LB_PID"
# observed today:  "REL-2-03 TLS cert reload succeeded" — no warning, no error
# expected:        the same warn-or-fail the startup path produces for mode 0644
```

**Existing test coverage: NONE.** `tests/cert_rotation.rs` has exactly three tests — `test_sigusr1_rotates_cert_no_drop` (:143), `test_invalid_reload_keeps_old_cert_serving` (:190), `test_in_flight_handshake_sees_pre_rotation_bundle` (:226). None sets file modes. `crates/lb-security/src/key.rs` unit tests cover `assert_owner_only` in isolation, which is exactly why the missing *call* was not caught.

**Remediation sketch (for the lead's second pass).** Call `assert_key_perm_advisory(&entry.key_path)` at the top of the `reload_all_tls` loop body and treat `Err` as a reload failure (`entry` keeps the old bundle live — the fail-safe path already exists and is tested by `test_invalid_reload_keeps_old_cert_serving`), counting it under a new `cert_rotation_failed_total{reason="key_perm"}` label. Regression test: extend `tests/cert_rotation.rs` with a `chmod 0644`-then-SIGUSR1 case asserting the old bundle stays live. Also correct the claim in `audit/security/s38-findings-infra.md` and `SECURITY.md`.

**Adjacent doc nit (same area, no impact):** `main.rs:182` registers `cert_rotation_succeeded_total` with help text `"…reloads (SIGUSR1 or inotify)"`, but no inotify/notify watcher exists anywhere in the tree (`rg -i 'inotify|notify::|watcher'` over `crates/**/src/**` returns only an unrelated `tokio::sync::Notify` in `h1_proxy.rs:446`). SIGUSR1 is the only trigger.

---

### [LOW] Retry-token expiry is anchored to a per-process `Instant`, while the signing secret is persistent — the 10 s lifetime does not survive a restart (CWE-613, CWE-672)

- **CVSS:** `CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:U/C:N/I:L/A:L` (3.6) — degrades an anti-spoofing control; no confidentiality impact.
- **Location:** `crates/lb-security/src/retry.rs:96` (`origin: Instant::now()`), `:128-130` (mint), `:192-196` (verify).
- **Class:** insufficient expiration / time-anchor confusion.
- **Wiring status: LIVE.** `crates/lb-quic/src/router.rs:161` and `crates/lb-quic/src/passthrough.rs:591`, both with `mint_retry` defaulting **true** (`lb-config/src/lib.rs:781`).

**Summary.** The token's timestamp is milliseconds since a **process-local monotonic origin**, but the HMAC secret it is authenticated under is **persisted to `retry_secret_path`** and reloaded verbatim on the next boot (`crates/lb-quic/src/listener.rs:363-382`). Any two processes sharing that secret disagree about what `issued_ms` means, and the disagreement fails **open**.

```rust
// retry.rs:96  — a NEW origin every process start, even when `secret` came off disk
Self { key: hmac::Key::new(hmac::HMAC_SHA256, &secret), origin: Instant::now(), max_age: DEFAULT_RETRY_MAX_AGE }

// retry.rs:128 — mint: uptime-relative, not wall-clock
let issued_ms = u64::try_from(now.saturating_duration_since(self.origin).as_millis()).unwrap_or(u64::MAX);

// retry.rs:192 — verify: re-anchored against THIS process's origin
let issued_at = self.origin + Duration::from_millis(issued_ms);
let age = now.saturating_duration_since(issued_at);       // saturates to 0 when issued_at is in the future
if age > self.max_age { return Err(RetryError::Expired); }
```

**Root cause / data flow.** Attacker input → `verify(token, peer, now)`. The MAC is checked first and correctly (constant-time, `retry.rs:166-169`), so `issued_ms` is authentic. But `issued_at = origin_new + issued_ms` places a token minted at old-process uptime *U* at *U* milliseconds **in the future** of the new process. `saturating_duration_since` then yields `age = 0`, and the expiry branch is never taken. The token stays valid for a further *U* + `max_age`.

Concretely: a process up for one hour mints a token; the process restarts; that token is accepted for **one hour** after the restart instead of 10 seconds. The same arithmetic applies across a fleet whenever an operator distributes one `retry.secret` to several nodes (the only way LB-minted Retry tokens survive ECMP/anycast rehashing) — node B verifies node A's tokens against B's own uptime.

**Impact.** RFC 9000 §8.1.3 requires a short token lifetime specifically to bound reuse of an address-validation credential. The peer binding (`retry.rs:189 token_peer != peer`) still holds, so the token is only usable from the address it was minted for. The realistic gain is: an attacker who once legitimately held address `X:P` (an ephemeral cloud IP since reassigned, a CGNAT port since recycled) keeps a valid address-validation token for `X:P` across gateway restarts, and can spend it — repeatedly, since there is no per-token replay cache on this path — to push spoofed Initials past the Mode-A Initial-flood gate (`passthrough.rs:570-596`), each one allocating flow state. Bounded by `max_quic_connections` (`passthrough.rs:601-620`). Hence LOW, not MEDIUM.

**PoC (lead runs).** Deterministic, no timing needed — it is pure arithmetic:
```rust
// with a FIXED secret, mint on signer A at uptime U, verify on a FRESH signer B (simulates restart)
let secret = [0x5au8; RETRY_SECRET_LEN];
let a = RetryTokenSigner::new_with_secret(secret).with_max_age(Duration::from_secs(10));
let t0 = a.origin_for_test();                      // or: sleep, then mint
let token = a.mint_at(peer, b"odcid", t0 + Duration::from_secs(3600));   // A up 1 h
let b = RetryTokenSigner::new_with_secret(secret);  // restart: new origin
assert!(b.verify(&token, peer, Instant::now()).is_ok());  // PASSES TODAY — should be Expired
```
(`origin` is private; the lead can add a `#[cfg(test)]` accessor or drive it through two `load_or_generate_retry_secret` calls on the same file.)

**Existing test coverage: NONE for this case.** `retry.rs:281-289 verify_rejects_expired_token` uses a **single** signer, so the origin always matches and the bug is invisible. There is no cross-signer / cross-process test anywhere.

**Remediation sketch.** Put an absolute time on the wire: replace `issued_ms` with wall-clock `SystemTime::now().duration_since(UNIX_EPOCH).as_secs()` (or keep ms) and verify against `SystemTime::now()`, rejecting both `age > max_age` **and** `issued_at > now + skew` (a small forward-skew allowance, so a future timestamp fails **closed** instead of saturating to age 0). Bump `RETRY_TOKEN_VERSION` (`retry.rs:22` — the file already documents that this is what the version byte is for) so old-format tokens are rejected rather than misread. Regression test = the PoC above.

---

### [LOW] `raw_proxy.sni` is not empty-validated, unlike its sibling `tls_verify_hostname` — an empty SNI clears the upstream hostname check (CWE-297)

- **CVSS:** `CVSS:3.1/AV:N/AC:H/PR:N/UI:R/S:U/C:H/I:H/A:N` (6.8 with UI:R for the required operator misconfiguration) — a config foot-gun, not a default.
- **Location:** `crates/lb-config/src/lib.rs:692` (`pub sni: String`, no validator) vs `crates/lb-config/src/lib.rs:1253-1259` (the sibling **is** checked). Sink: `crates/lb-io/src/quic_pool.rs:373 quiche::connect(Some(sni), …)`.
- **Class:** improper certificate validation (hostname).
- **Wiring status: LIVE** (Mode B raw-QUIC proxy).

**Summary.** The H3-backend SNI override is empty-checked:

```rust
// lb-config/src/lib.rs:1253 — validate_backend_h3_tls
if let Some(sni) = backend.tls_verify_hostname.as_deref() {
    if sni.trim().is_empty() {
        return Err(ConfigError::Validation(format!("listener {i} backend {j} tls_verify_hostname is empty")));
```

`RawQuicProxyConfig::sni` gets no such check. `validate_quic_listener` (`lb-config/src/lib.rs:~1305-1350`) validates `cert_path`, `key_path`, `retry_secret_path`, `max_idle_timeout_ms`, `max_recv_udp_payload_size`, and the `raw_proxy`-vs-`backends` mutual exclusion — and never touches `raw_proxy.sni`.

**Root cause / data flow.** `sni = ""` in TOML → `RawQuicProxyConfig.sni` → `main.rs:1037` → `raw_proxy.rs:52` → `raw_proxy.rs:164 dial_dedicated(backend.addr, &backend.sni, …)` → `quic_pool.rs:324 connect_and_drive` → `quic_pool.rs:373 quiche::connect(Some(""), …)`. quiche's `set_host_name` feeds the name to both `SSL_set_tlsext_host_name` and `X509_VERIFY_PARAM_set1_host`; per the OpenSSL/BoringSSL contract, an **empty** host clears the verification host list and hostname checks are no longer performed on the peer certificate.

**Impact.** Mode B is the one upstream path documented as having **no verification off-switch** — `main.rs:1014-1019`: *"Backend-trust: verify_peer is ALWAYS on, never silently disabled. Without a CA bundle, fall back to BoringSSL default roots."* With `backend_ca_path` absent the trust anchor is therefore the **public** root store, so if the hostname binding is dropped, *any* holder of a publicly-trusted certificate that can get on-path to the backend address can impersonate the Mode B backend. The chain check alone is not an identity check.

**Honest limits on this finding.** I could not execute anything on this box, so the BoringSSL empty-host semantics are asserted from the documented `X509_VERIFY_PARAM_set1_host` contract, **not** observed. Two outcomes are possible and the lead should settle it with one CI run: either `SSL_set_tlsext_host_name(ssl, "")` errors and `quiche::connect` fails closed (in which case this collapses to an availability foot-gun), or the handshake proceeds with hostname verification off (the finding as written). The **verifiable-by-inspection** half stands either way: the validator checks one SNI field and not its sibling, and that asymmetry is a real gap.

**Existing test coverage: NONE.** `lb-config/src/lib.rs:1447-1476 raw_proxy_minimal_toml_defaults_caps` only asserts `rp.sni == "backend.test"` deserializes; no empty-value case anywhere.

**Remediation sketch.** Add the four-line mirror of `lb-config/src/lib.rs:1253-1259` for `raw_proxy.sni` inside `validate_quic_listener`, plus a rejection unit test. Consider hardening the sink too (`quic_pool.rs:373` reject an empty `sni` before dialling) so the invariant does not rest on the validator alone.

---

### [LOW] Admin listener has no timeout stack and no connection cap — pre-authentication socket exhaustion (CWE-770, CWE-400)

- **CVSS:** `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:L` (5.3) on a non-loopback bind; `AV:L` (3.3) on the default loopback bind.
- **Location:** `crates/lb-observability/src/admin_http.rs:200-206`.
- **Class:** allocation of resources without limits, pre-auth.
- **Wiring status: LIVE** (`main.rs:2251`).

**Summary.** Each accepted admin connection is served by a bare hyper builder with no timer and no budget:

```rust
// admin_http.rs:200-206
tokio::spawn(async move {
    let io = TokioIo::new(stream);
    if let Err(e) = hyper::server::conn::http1::Builder::new()
        .keep_alive(true)
        .serve_connection(io, svc)
        .await
```

No `.timer(TokioTimer::new())`, no `.header_read_timeout(...)`, no total-connection deadline, no `ConnGate`, no accept-rate limit. Contrast the data-plane H1 server, which was given exactly this treatment by S38's F-RES-1 fix (`h1_proxy.rs:684`).

**Attack.** The bearer-token gate is enforced **per request**, inside `route()` (`admin_http.rs:64-73`). A connection that never sends a request byte never reaches it, so the auth gate provides no protection here at all. An attacker who can reach the admin port opens N sockets and sends nothing; each costs an fd and a task **indefinitely**. Because fds are a process-wide resource, exhausting them starves the *data-plane* listeners too — the blast radius is the whole gateway, not just `/metrics`.

**Severity is bounded by the bind guard, which is sound.** `AdminAuthGate::validate_bind` (`admin_auth.rs:145-161`) is called before every production bind (`main.rs:2244`) and hard-fails startup via `?`; a non-loopback bind requires `allow_non_loopback = true` **and** a token hash. Every shipped config and `README.md`/`docs/guide/DEPLOYMENT.md:240` use `127.0.0.1:9090`; `metrics_bind` defaults to `None` (no listener at all). So the remote-attacker case requires the operator to have deliberately taken the documented public-bind escape — for whom the token gives a false sense of protection, since it is bypassed by simply not sending a request.

**Existing test coverage: NONE.** `tests/metrics_endpoint.rs` uses the auth-less `admin_http::serve`; the only `serve_with_auth` test in the tree is `admin_http.rs:236 test_admin_403_without_token`, a single well-behaved request. No slowloris/concurrency case.

**Remediation sketch.** `.timer(TokioTimer::new()).header_read_timeout(Duration::from_secs(5))` on the builder, plus a small semaphore (e.g. 64 concurrent admin connections) around the accept loop — the admin surface has no legitimate need for more.

---

### [INFO] `sample_lb_scid` fails **open** with a predictable SCID while its comment claims "fail closed"

- **Location:** `crates/lb-quic/src/passthrough.rs:836-847`.
- **Wiring status: LIVE, but the branch is unreachable in practice.**

```rust
fn sample_lb_scid() -> [u8; LB_SCID_LEN] {
    let mut scid = [0u8; LB_SCID_LEN];
    if ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut scid).is_err() {
        // "RNG failure on a supported platform is effectively impossible; fail closed rather than
        // emit a predictable SCID."
        static FALLBACK: AtomicU64 = AtomicU64::new(0);
        let n = FALLBACK.fetch_add(1, Ordering::Relaxed);
        scid[..8].copy_from_slice(&n.to_be_bytes());   // …emits a fully predictable SCID
    }
    scid
}
```

The comment asserts the opposite of what the code does: the fallback is a monotonic counter in the first 8 bytes with the remainder zero — maximally predictable. The same shape appears at `crates/lb-quic/src/router.rs:344-360` (time-derived fallback) and `crates/lb-security/src/zero_rtt.rs:29-44` (`fresh_secret`, time-mixed fallback, at least honestly labelled *"guessable but beats a fixed seed"*).

**Why INFO and not higher.** `getrandom(2)` on a supported Linux kernel does not fail after boot, so the branch is dead in practice; and for `zero_rtt.rs` the key is only used as an HMAC key, where knowing the key still does not yield SHA-256 collisions, so the stated threat ("precompute digest collisions") does not actually materialise. Recorded because the comment would mislead a future reader into thinking the failure mode is safe. Cheapest correct fix: propagate the error and refuse to mint, which is what "fail closed" means.

**Randomness posture is otherwise clean.** Every security-relevant value — retry secret (`retry.rs:85`, `listener.rs:391`, `passthrough.rs:1044`), 0-RTT HMAC key (`zero_rtt.rs:31`), QUIC SCIDs, TLS ticket keys (via `rustls::crypto::ring::Ticketer::new`, `ticket.rs:39`) — draws from `ring::rand::SystemRandom`. `rand::rng()` appears only in load-balancing (`lb-balancer/src/{p2c,random,weighted_random}.rs`) and accept/drain jitter (`main.rs:2803`, `:3186`) — non-security. `StdRng::seed_from_u64` appears **only** under `#[cfg(test)]` (`pool.rs:644`, and the balancer test modules). No time-, PID- or counter-seeded RNG feeds any security token.

---

### [INFO] The 0-RTT replay guard's key includes a client-chosen field, and its LRU window is cheaply flushable

- **Location:** `crates/lb-quic/src/router.rs:186-194` (`build_replay_key`), `crates/lb-security/src/zero_rtt.rs:120-137` (`check_and_record`).
- **Wiring status: guard is LIVE; 0-RTT is DISABLED by construction, so live impact is nil.**

`build_replay_key` is `scid || token[..32]`. The SCID is picked freely by the client, so an attacker replaying a captured Initial need only vary it to produce a fresh key and walk past the dedup. Separately, the LRU window (default 65 536, `zero_rtt.rs:14`) evicts on insert, so ~65 k unique Initials flush any stored digest — the module doc's claim that LRU-over-FIFO defeats a unique-token spray holds only for a digest that has *already* been replayed once and promoted (`zero_rtt.rs:123-127`), not for the victim's stored entry, which is the one that matters.

**Neither is exploitable today.** `enable_early_data()` is never called on a client-facing server config (`crates/lb-quic/tests/s19_b6_zero_rtt_rejection.rs:3-6` proves it by construction *and* on the wire), and `rustls`'s `max_early_data_size` defaults to 0 (`audit/deferred.md` SEC-2-13). A mutated-SCID replay also fails quiche's Initial AEAD, since the long header is AAD. Recorded so that the **SEC-2-13 re-open trigger** ("if a future change enables 0-RTT this must be re-opened as critical") arrives with these two weaknesses already on the record — they must be fixed *before* early data is ever enabled, not after.

**Doc drift, same area:** `SECURITY.md` defenses table row 14 cites `zero_rtt.rs::ZeroRttReplayFilter`; the type is named `ZeroRttReplayGuard`. No such symbol as `ZeroRttReplayFilter` exists.

---

### [INFO] No audit trail on admin authentication failure

- **Location:** `crates/lb-observability/src/admin_http.rs:70-72`.

```rust
if gate.authorize(header).is_err() {
    return plain(StatusCode::FORBIDDEN, "forbidden\n");
}
```

A rejected bearer token is not logged, not counted in any metric, and not rate-limited. A brute-force or credential-probing campaign against the admin token leaves **zero** evidence in journald or Prometheus. Hardening, not a vulnerability (the credential is a 256-bit preimage). Suggest a throttled `tracing::warn!` with the peer address plus an `admin_auth_rejected_total` counter.

Note the response is `403 Forbidden` where `401 Unauthorized` + `WWW-Authenticate: Bearer` is the RFC 9110 §15.5.2 answer for a missing/invalid credential. Cosmetic; it does not weaken the gate.

---

### [INFO] `lb-controlplane` / `lb-cp-client` trust-boundary assessment — no live gap

Recorded because the task asked for an explicit determination.

- **`lb-cp-client`: STUB-DEAD.** Not a dependency of `crates/lb/Cargo.toml`. `connect()` performs no I/O — it sets a bool (`lib.rs:66-72`). There is no control-plane *channel*, therefore no channel authentication or encryption to assess, and no path by which a remote control plane can influence the gateway. **Any finding in this crate is INFO by construction.** If a transport is ever implemented, the whole crate must be re-audited as a CRITICAL-severity trust boundary.
- **`lb-controlplane`: live, but correctly subordinated.** `ConfigManager::validate` (`lib.rs:198-209`) checks only non-empty + parseable TOML — far weaker than `lb_config`'s `deny_unknown_fields` + range validators. That is **not** a gap, because `reload_config` re-runs the full parser and validator afterwards and rolls back on any failure (`main.rs:348-368`), which the function's own doc comment states (`main.rs:313-315`). Config source is `std::env::args().nth(1)` (`main.rs:2097`) — operator-controlled, no untrusted input. `FileBackend::store` writes a same-directory temp file then renames (`lib.rs:70-84`) — atomic, correct; it uses `std::fs::write` (umask-dependent mode) but the config file holds no plaintext secret (`api_token_hash` is a digest).
- **Config-reload honesty holds for this surface.** `[admin]` and `[observability]` changes are classified **restart-required** (`lb-config/src/reload.rs:103-106,131-134,206-212`) and logged per-field (`main.rs:376-383`), so an operator rotating the admin token is told the change was not applied rather than being left believing the old token is dead. Correct and worth preserving.

---

## Proven-clean scopes (attacked, held — defence + the code that proves it)

Recorded per R4 so a future auditor does not re-tread them.

| Scope | Attack tried | Defence | Evidence |
|---|---|---|---|
| Admin auth comparison | Timing oracle on the bearer token | SHA-256 then `subtle::ConstantTimeEq`; plaintext never stored | `admin_auth.rs:84-86,131-142`; hex decode is length- and charset-validated (`:66-80`) |
| Admin path/method auth bypass | `OPTIONS`/`HEAD`/`POST`, `/metrics/../livez`, case-mangling, absolute-form target | Method check **precedes** the auth gate (`admin_http.rs:57-59`); the gate and the router both key off the *same* `request.uri().path()` with *exact* matches, so no normalisation differential exists (`:64-73` vs `:74-84`) | `admin_http.rs:57-84` |
| Admin public-bind foot-gun | Bind `0.0.0.0` / `::` unauthenticated | `validate_bind` hard-fails startup; `0.0.0.0`/`::`/`::ffff:127.0.0.1` are all correctly non-loopback; override still requires a token | `admin_auth.rs:145-161`, called at `main.rs:2244` with `?`; tests `admin_auth.rs:186-208`, `main.rs:5337-5351` |
| Admin bind default | `0.0.0.0` shipped default | `metrics_bind` defaults to `None`; every shipped config and doc uses `127.0.0.1` | `lb-config/src/lib.rs:85-88`, `docs/guide/DEPLOYMENT.md:240`, `README.md` |
| Retry-token forgery / tamper | Flip authenticated bytes, forge a peer, move the MAC boundary | HMAC-SHA256 over the **entire** body, constant-time compare, verify-**then**-parse ordering | `retry.rs:160-196`; tests `:263-300` |
| Retry-secret file perms | Pre-place a world-readable secret | F-INFRA-01 fix is real and correct on **both** load paths, strict in release | `lb-quic/src/listener.rs:339-368`, `passthrough.rs:990-1020`; tests `listener.rs:455-503` |
| Upstream cert verification | Disable backend verification | H3: `verify_peer(true)` default, `tls_ca_path` mandatory unless an explicit documented opt-out, tls_* knobs **rejected** on non-H3 backends. Mode B: `verify_peer(true)` unconditionally. `verify_peer(false)` in `quic_pool.rs:546` is inside `#[cfg(test)]` (module starts `:536`) | `main.rs:960-991,1013-1019`; `lb-config/src/lib.rs:1233-1260` |
| Upstream hostname binding | Chain-only verification (MITM by any CA-signed host) | SNI is always `Some` and quiche binds it to `X509_VERIFY_PARAM_set1_host`; the H3 override is empty-checked | `quic_pool.rs:373`, `main.rs:894-905,1488-1492`, `lb-config/src/lib.rs:1253-1259`. *(The Mode-B sibling is the LOW finding above.)* |
| Secrets in `Debug` / logs / errors | Grep every `Debug` impl, `tracing` macro and `thiserror` variant on the secret path | Hand-written non-printing `Debug` on **every** secret-bearing type: `AdminTokenHash` (`admin_auth.rs:89-94`), `TicketKey` (`ticket.rs:57-62`), `TicketRotator` (`:157-166`), `RotatingTicketer` (`:185-189`), `RetryTokenSigner` (`retry.rs:70-78`), `TlsConfigBundle` (`ticket.rs:~330`), `ConnPermit` (`conn_gate.rs:67-73`). No error variant embeds key material — `TicketError`/`TlsBundleError`/`RetryError`/`AdminAuthError` carry only paths, modes, lengths and closed reason strings. The two `retry_secret =` log fields are **paths**, not bytes | `admin_auth.rs:89`, `ticket.rs:57,157,185`, `retry.rs:70`, `conn_gate.rs:67` |
| Metrics content leak | Scrape `/metrics` for secrets / config / high-cardinality client data | Label values are closed sets plus listener/backend addresses; `route_label` is hardcoded `""` (`main.rs:3216`) so no request path reaches a label; a boot-time cardinality budget refuses to start an over-wide shape (`main.rs:2108-2132`) | `main.rs:2033-2035,3216-3231` |
| Cert/key rotation atomicity | Mismatched cert+key from two separate reads mid-rotation | Two separate `File::open`s, but rustls's `with_single_cert` smoke-build catches the mismatch → `KeyMismatch` → reload fails → **old bundle stays live** | `ticket.rs:~355-400,452-462`; tests `ticket.rs tls_bundle_mismatched_key_rejected`, `tests/cert_rotation.rs:190` |
| Ticket-key lifecycle | Stalled rotation extending forward-secrecy exposure | Driven every 60 s; `previous` demoted on rotation and dropped after `overlap`; a demoted key cannot decrypt post-rotation tickets | `main.rs:816-846`, `ticket.rs:118-140`; tests `ticket.rs:~440-520` |
| Inbound header trust | Forge a source IP via `X-Forwarded-For` / `X-Real-IP` / `X-Forwarded-Proto` | **No gateway decision reads any inbound `X-Forwarded-*`.** `append_xff` only appends the real socket peer (correctly iterating *all* values — Envoy GHSA-ghc4-35x6-crw5); `set_xfp`/`set_xfh` **replace** via `insert`. Rate limiting and caps key off the real socket peer (`ConnGate::admit(peer)`). No `X-Forwarded-Client-Cert` / XFCC handling exists at all | `h1_proxy.rs:2042-2077`; `conn_gate.rs:120-152` |
| `ConnGate` trusted-CIDR bypass | Spoof into a "trusted" exemption | The field is stored but matched by nothing, and a test **pins** that deferred state | `conn_gate.rs:30-44,77-90`; `lb-security/tests/conn_gate.rs:109 trusted_cidrs_do_not_currently_exempt` |
| Config-reload honesty for auth | Rotate `api_token_hash` and have it silently not apply | Classified restart-required and logged per-field | `lb-config/src/reload.rs:103-106,206-212`; `main.rs:376-383` |

---

## Explicitly NOT re-reported (documented prior art)

- **ALREADY-KNOWN: F-INFRA-02** — no `zeroize` on key material. `grep -rn zeroize crates/` is still empty; TLS keys live in `Arc<rustls::ServerConfig>`, the retry secret in `ring::hmac::Key`, ticket keys behind `Arc<dyn ProducesTickets>` — none zeroized on drop. Unchanged since S38, correctly rated a defence-in-depth gap with no reachable read path. `SECURITY.md` §Residual risks.
- **ALREADY-KNOWN: F-INFRA-03** — no server-side mTLS (`ticket.rs:247,410` `with_no_client_auth()`); TLS 1.2 permitted unless `tls13_only` (`ticket.rs:238-246`). Both intentional and documented.
- **ALREADY-KNOWN: F-INFRA-01** — retry-secret load-path perms. Verified **fixed** and correct on both sites.
- **ALREADY-KNOWN: SEC-2-13** — 0-RTT disabled on TCP/TLS listeners. Re-verified; also disabled on QUIC by construction.
- **ALREADY-KNOWN: L-002** — `ConnGate::trusted_cidrs` carried but unmatched. Deferred with a pinning test.
