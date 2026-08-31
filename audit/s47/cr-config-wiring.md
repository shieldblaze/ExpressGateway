# S47 — Config / reload / binary-wiring review (`cr-config-wiring`)

Branch `review/s47-rfc-security`. Read-and-reason only; **no cargo command was
run** (2 vCPU / 7 GB box, per the brief). Every claim below is traced from a
quoted source line to its call site or to the absence of one.

Line numbers are against the working tree at the time of review. `crates/lb/src/main.rs`
gained +24 lines mid-review from a concurrent S47-SEC-1 edit by `rt-crypto-auth`
(SIGUSR1 key-perm check); citations here are post-edit.

---

## 1. WIRED / UNWIRED / TEST-ONLY table

"WIRED" = a call reaches the entry point from `crates/lb/src/main.rs` or from a
listener/proxy path `main.rs` actually constructs.

### `lb-security`

| Module / entry point | Status | Evidence |
|---|---|---|
| `smuggle.rs::SmuggleDetector` | **WIRED** | `HooksBundle` → `lb-l7/src/security_hooks.rs`; also `lb-l7/src/h2_to_h1.rs:54`. |
| `conn_gate.rs::ConnGate` | **WIRED** | `main.rs` builds one process-wide `ConnGate`; consumed at `run_listener` via `state.hooks.admit_connection`. |
| `admin_auth.rs::{AdminAuthGate,validate_bind}` | **WIRED** | `main.rs:2275-2281`; `/metrics` gated, probes exempt by design. |
| `retry.rs::RetryTokenSigner` | **WIRED** | `lb-quic` `listener.rs` / `router.rs` / `passthrough.rs`. |
| `ticket.rs::{TicketRotator,RotatingTicketer,reload_tls_bundle,TlsConfigBundle}` | **WIRED** | `build_tls_bundle`, `spawn_rotator_ticker`, SIGUSR1 `reload_all_tls`. |
| `ticket.rs::build_server_config_with_policy(tls13_only=true)` | **UNWIRED** | Only caller with `true` is `crates/lb-security/tests/tls_versions.rs`. See **CW-02**. |
| `zero_rtt.rs::ZeroRttReplayGuard` | **WIRED** | `lb-quic/src/router.rs`, `listener.rs`. |
| `handshake.rs::timeout_accept` | **WIRED** | `main.rs:3103`/`:3150` (TLS + H1s accept). |
| `key.rs::assert_owner_only` | **WIRED** | `main.rs`, `lb-quic/src/listener.rs`, `passthrough.rs`. |
| `hooks.rs::{HooksBundle,SecurityHooks}` | **WIRED** | one `Arc<HooksBundle>` shared across listeners. |
| `watchdog.rs::Watchdog` | **WIRED (detect-only)** | `main.rs` builds + sweeps it, `H{1,2}Proxy::with_watchdog`. Enforcement is out of scope by design — `ALREADY-KNOWN: SECURITY.md` "Residual risks" F-RES-5. |
| **`glitches.rs::GlitchesCounter`** | **UNWIRED** | `H2Proxy::with_glitches` (`h2_proxy.rs:361`) has exactly one caller: `crates/lb-l7/tests/round8_glitches_enforced.rs:45`. See **CW-05**. |
| **`slowloris.rs::SlowlorisDetector`** | **TEST-ONLY** | No production crate references it. Only `tests/security_slowloris.rs`, which unit-tests the type. See **CW-15**. |
| **`slow_post.rs::SlowPostDetector`** | **TEST-ONLY** | Same; only `tests/security_slow_post.rs`. See **CW-15**. |

### `lb-l7`

| Module / entry point | Status | Evidence |
|---|---|---|
| `security_hooks.rs::DynSecurityHooks` | **WIRED** | `build_h1_proxy`/`build_h2_proxy` `.with_hooks(...)`. |
| `h2_security.rs::H2SecurityThresholds::apply` | **WIRED** | `h2_proxy.rs:529` on the live `hyper::server::conn::http2::Builder`. |
| `sni_authority.rs::{check_sni_authority,misdirected_response}` | **WIRED (h1s only)** | `h1_proxy.rs:711`, `h2_proxy.rs:829`; SNI captured at `main.rs:3158` and passed via `serve_connection_with_cancel_sni`. Closes the PROTO-2-15 wiring deferral. |
| `authority.rs::validate_request` | **WIRED (H1/H2 fronts)** | `h1_proxy.rs:600`, `h2_proxy.rs:675`. Not on the H3 front — `ALREADY-KNOWN`, stated in `crates/lb-quic/tests/round8_h3_authority_enforced.rs:3`. |
| `stripped_request.rs::StrippedRequest` | **WIRED** | typestate on both proxies' hot paths. |

### Other crates

| Module / entry point | Status | Evidence |
|---|---|---|
| `lb-h2/src/security.rs` (Rapid-Reset, CONTINUATION, HPACK, SETTINGS, PING, zero-window detectors) | **TEST-ONLY** | `lb-l7` imports only three `DEFAULT_*` constants (`h2_security.rs:45-52,90,94`); no detector type is constructed in production. `ALREADY-KNOWN: SECURITY.md` "Note on the production wire path" — live enforcement is on the hyper builder. |
| `lb-h1` (whole crate) | **TEST-ONLY** | No production crate depends on it (`crates/*/Cargo.toml`). `ALREADY-KNOWN` (same note). |
| `lb-health/src/ejection.rs` | **WIRED** | per-listener registry in `spawn_tcp`; L7 via `health_filtered`, L4 via `state.health.admits`. H3 front not covered — `ALREADY-KNOWN: docs/known-limitations.md` "Health ejection is passive only". |
| `lb-balancer` | **round-robin only** | `main.rs` imports `round_robin::RoundRobin`; Maglev lives in `lb-quic` passthrough. The other 10 algorithms are unreachable — `ALREADY-KNOWN: docs/features.md` "The algorithm library (implemented, not yet selectable)". |
| `lb-observability/src/label_budget.rs` | **WIRED** | boot gate at `main.rs:2149-2163`, fails boot on overflow. |
| `lb-observability/src/probes.rs` | **WIRED** | `ProbeRegistry::shared()` → admin listener; `set_ready`/`set_draining`. But see **CW-10**. |
| `lb-controlplane::{ConfigManager,FileBackend}` | **LIVE** | drives SIGHUP reload from `main.rs:341-523`. |
| `lb-controlplane::HaPoller` | **TEST-ONLY** | no production caller; only `tests/controlplane_ha.rs`. There is no HA control plane and therefore no split-brain behaviour to review. See **CW-18**. |
| `lb-cp-client` | **STUB-DEAD** | self-documented ("No transport is implemented … nothing outside it links against it"); a **dev**-dependency of `lb` only. See **CW-18**. |
| `lb-l4-xdp::XdpLoader::set_new_flow_cap` | **UNWIRED** | zero callers workspace-wide. See **CW-11**. |
| `lb-l4-xdp::CtInsertGate` | **TEST-ONLY** | only `crates/lb-l4-xdp/tests/round8_synflood_cap.rs`. |
| `lb-l4-xdp::publish_backends_v4` | **UNWIRED** | `ALREADY-KNOWN: audit/deferred.md` ROUND8-L4-04 (deferred to Pillar 4b-3). |

### Config knobs that parse, validate and diff — but reach nothing

| Knob | Status |
|---|---|
| `[runtime.tls].tls13_only` | **UNWIRED** — **CW-02** |
| `[runtime.watchdog].header_deadline_ms` | **UNWIRED** — **CW-13** |
| `[runtime].xdp_new_flow_cap_per_sec_per_cpu` | **UNWIRED** — **CW-11** |
| `[runtime].header_underscore_policy` (`drop`/`allow`) | UNWIRED — `ALREADY-KNOWN: docs/guide/CONFIG.md`, `config/default.toml` |
| `[[listeners.backends]].weight` | UNENFORCED — `ALREADY-KNOWN: docs/features.md` "Backend `weight`" |

---

## 2. Findings

Every item carries `blocking for prod` / `non-blocking`.

---

### CW-01 — SIGHUP with an invalid config OVERWRITES the operator's config file on disk — **CRITICAL** — *blocking for prod*

`crates/lb/src/main.rs:380` and `:389`:

```rust
    let new_config = match lb_config::parse_config(mgr.current_config()) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = %e, "SIGHUP: new config failed to parse into LbConfig — rolling back, keeping live config");
            let _ = mgr.rollback_to_previous();
```

`crates/lb-controlplane/src/lib.rs:208`:

```rust
    pub fn rollback_to_previous(&mut self) -> Result<bool, ControlPlaneError> {
        let Some(prev) = self.previous_config.take() else {
            return Ok(false);
        };
        self.backend.store(&prev)?;
```

`crates/lb-controlplane/src/lib.rs:67` — the backend is a `FileBackend` pointing at
the operator's own config path (`main.rs:2171`: `FileBackend::new(PathBuf::from(&config_path))`):

```rust
    fn store(&self, config: &str) -> Result<(), ControlPlaneError> {
        ...
        std::fs::write(&tmp_path, config).map_err(ControlPlaneError::Io)?;
        std::fs::rename(&tmp_path, &self.path).map_err(ControlPlaneError::Io)?;
```

**Scenario.** Operator edits `/etc/expressgateway/config.toml`, mistyping
`max_keepalv_requests`, and runs `systemctl reload expressgateway`.
`ConfigManager::reload()` succeeds (the file is valid TOML), `lb_config::parse_config`
then fails on `deny_unknown_fields`, and `rollback_to_previous()` **writes the
previous config text back over the operator's file**. The edit is destroyed. The
log line says `rolling back, keeping live config`, which reads as an in-memory
rollback; `docs/guide/CONFIG.md:375` and `docs/guide/RUNBOOK.md:224` both describe
this path as "nothing is applied" and never mention a disk write.

The trigger class is **valid TOML that fails ExpressGateway validation** — a typo'd
key, an out-of-range value, `[listeners.tls]` on an `h1` listener. That is the most
common operator error, not a corner case. (A file that is not valid TOML fails
inside `mgr.reload()` and returns before the rollback, so the file survives — the
destructive path is exactly the likely one.)

Secondary: `std::fs::write` creates the temp file fresh, so the rename **replaces
the file's mode and owner** with the process umask default. A config at `0600`
(it may carry `[admin].api_token_hash`) silently becomes `0644` owned by the
gateway user. The `let _ = ...` also discards a store failure entirely.

**Existing test coverage: none, and the one test that hits this path is blind to it.**
`tests/reload_under_traffic.rs::proof_b_invalid_reload_no_blip` (line 600) writes
exactly this class of bad config, SIGHUPs, and asserts only that traffic keeps
flowing and `config_reload_failed_total` bumps. It never re-reads
`dir.join("gateway.toml")` — which, after the test runs, no longer contains what
the test wrote.

---

### CW-02 — `[runtime.tls].tls13_only` is UNWIRED: the gateway always offers TLS 1.2 — **HIGH** — *blocking for prod*

The knob exists and is validated (`crates/lb-config/src/lib.rs:337-341`), and its
doc comment at `:128` claims the block controls version selection:

```rust
    /// `[runtime.tls]` policy block; absent means the rustls default `&[&TLS12, &TLS13]`.
    pub tls: Option<RuntimeTlsConfig>,
```

But no code path carries it to a TLS listener. `build_tls_bundle`
(`crates/lb/src/main.rs:819`) receives only the per-listener `TlsConfig` — the
`RuntimeConfig` is not a parameter — and calls:

```rust
    let bundle = lb_security::TlsConfigBundle::load_from_paths_with(   // main.rs:830
```

which unconditionally builds with both versions (`crates/lb-security/src/ticket.rs:408`):

```rust
        let builder = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
```

The only `tls13_only`-aware constructor, `build_server_config_with_policy`, is
called from production exactly once — `ticket.rs:226`, with `false` hard-coded:

```rust
    build_server_config_with_policy(rotator, cert_chain, key_der, alpn_protocols, false)
```

Workspace-wide, the only caller passing `true` is
`crates/lb-security/tests/tls_versions.rs`.

**Scenario.** An operator with a PCI-DSS / FIPS-profile requirement sets
`[runtime.tls] tls13_only = true`, boots cleanly, and believes TLS 1.2 is off.
Every `tls` and `h1s` listener continues to negotiate TLS 1.2. There is no warning
and no metric. The claim is repeated in three operator-facing places:
`docs/guide/CONFIG.md` (`[runtime.tls]` table), `docs/features.md` ("TLS 1.3 only
… for PCI-DSS-style requirements"), and `SECURITY.md` ("Set `tls13_only = true`
for TLS-1.3-only environments").

**Existing test coverage: the PROTO-2-14 test is the reason this looks wired.**
`crates/lb-security/tests/tls_versions.rs` drives `build_server_config_with_policy`
directly and never touches the binary's TLS build path, so it passes whether or not
the knob is connected.

---

### CW-03 — A TCP/TLS/H1/H1s listener that fails to bind dies SILENTLY; the process still reports Ready — **HIGH** — *blocking for prod*

`spawn_tcp` does not bind. It spawns, and returns the handle
(`crates/lb/src/main.rs:1687`):

```rust
    Ok(tracker.spawn(run_listener(listener_cfg.address.clone(), state)))
```

The bind lives inside the spawned task (`main.rs:2895-2902`):

```rust
    let parsed: SocketAddr = bind_addr
        .parse()
        .with_context(|| format!("invalid listen address: {bind_addr}"))?;

    let std_listener = state
        .io_runtime
        .listen(parsed, &listener_opts())
        .with_context(|| format!("failed to bind {bind_addr}"))?;
```

The returned `anyhow::Error` goes into a `JoinHandle` that is **never awaited**.
`listener_handles` is touched exactly once more, at drain (`main.rs:2625`), where a
failed listener is already `is_finished()` and is skipped:

```rust
    for h in &listener_handles {
        if !h.is_finished() {
```

So on `EACCES` (port <1024 without `CAP_NET_BIND_SERVICE`), `EADDRNOTAVAIL` (an
address not present on the host), `EADDRINUSE` (see CW-04), or an unparseable
address:

- no log line is emitted at all — the only signal is the *absence* of `"listener started"`;
- `listener_handles.is_empty()` is false, so the `"no listeners started"` bail at `main.rs:2469` does not fire;
- `probes.set_ready()` runs and `/readyz` returns 200.

**Scenario.** A two-listener config — `h1` on `:8080` and `h1s` on `0.0.0.0:443`
— deployed in a container without `CAP_NET_BIND_SERVICE`. The plaintext listener
comes up, the TLS listener silently does not, `/readyz` is green, and the service
is live with its secure front missing. Nothing distinguishes this from a healthy boot.

The asymmetry is worth noting: `spawn_quic` **does** parse and bind before returning
(`main.rs:1356`, `main.rs:1410` `QuicListener::spawn(...).await?`), so a QUIC bind
failure correctly aborts boot. `spawn_passthrough` likewise. Only the TCP-family path
is silent. TLS cert-load failures also abort boot correctly (`build_listener_mode` runs
inside the awaited `spawn_tcp`) — it is specifically the socket bind that is deferred.

This contradicts `docs/guide/CONFIG.md` "Invalid-config behavior": *"the binary exits
non-zero … it does **not** start a partial listener set."*

**Existing test coverage: none.** `tests/config_boot_matrix.rs` covers positive boots
per protocol and refusal on unserved-protocol / unknown-key; no bind-failure case.

---

### CW-04 — `SO_REUSEPORT` / `SO_REUSEADDR` are applied AFTER `bind()`, so both are inert — **HIGH** — *blocking for prod*

`crates/lb/src/main.rs:711-720` requests them:

```rust
const fn listener_opts() -> ListenerSockOpts {
    ListenerSockOpts {
        reuseaddr: true,
        reuseport: true,
```

`crates/lb-io/src/lib.rs:127-135` binds first:

```rust
    ) -> std::io::Result<std::net::TcpListener> {
        let listener = std::net::TcpListener::bind(addr)?;
        sockopts::apply_listener(&listener, cfg)?;
        Ok(listener)
    }
```

and `crates/lb-io/src/sockopts.rs:65-73` sets the option on the already-bound socket
(its own doc comment says so — *"Apply … to a bound listener"*):

```rust
pub fn apply_listener(socket: &TcpListener, cfg: &ListenerSockOpts) -> io::Result<()> {
    let sock = SockRef::from(socket);
    if cfg.reuseaddr {
        sock.set_reuse_address(true)?;
    }
    #[cfg(any(target_os = "linux", ...))]
    if cfg.reuseport {
        sock.set_reuse_port(true)?;
    }
```

`socket(7)` is explicit: *"this option must be set on each socket (including the
first socket) prior to calling `bind(2)`"*. The kernel latches `tb->fastreuseport`
from `sk->sk_reuseport` at bind time and joins the socket to the reuseport group
there; a later `setsockopt` flips the socket flag but neither updates the bind
bucket nor joins the group. The flag is therefore decorative.

**Scenario.** The documented upgrade procedure in `docs/known-limitations.md`
("No binary hot-restart via socket-descriptor handover" → *Mitigation: start the
replacement process side-by-side under `SO_REUSEPORT`, then graceful-drain the old
one with SIGTERM*) cannot work: the replacement process's `bind()` gets
`EADDRINUSE`. **Compounded with CW-03 this is a full outage**: the replacement
process starts, silently fails to bind, flips `/readyz` to Ready, the operator sees
a healthy new instance and SIGTERMs the old one — and the port goes dark.

The UDP side has no reuseport at all: `crates/lb-quic/src/listener.rs:216` is a bare
`UdpSocket::bind(params.bind_addr).await?` with no `UdpSockOpts` applied.

`ALREADY-KNOWN (partially)`: ADR-0009 "Follow-ups" records *"Wire `SO_REUSEPORT`
explicitly via `socket2` … deferred"*. But ADR-0009's "Implementation notes" also
asserts *"the production path uses `socket2::Socket` with `set_reuse_port(true)`
**before** converting to the tokio listener"*, and `docs/known-limitations.md`
states the side-by-side mitigation as working fact. Both are wrong as built.

---

### CW-05 — the H2 protocol-abuse glitch counter is never enabled by the binary — **MEDIUM** — *blocking for prod*

`H2Proxy` defaults the feature off in both constructors
(`crates/lb-l7/src/h2_proxy.rs:284` and `:317`):

```rust
            glitches_threshold: None,
```

and the whole mechanism is gated on it (`h2_proxy.rs:500`):

```rust
        let glitch_state = self.glitches_threshold.map(|threshold| {
            let metric = self.glitches_metrics.as_ref().and_then(|reg| {
                reg.counter("h2_glitches_total", ...)
```

`with_glitches` (`h2_proxy.rs:361`) is the only way to turn it on, and its only
caller in the workspace is `crates/lb-l7/tests/round8_glitches_enforced.rs:45`.
`build_h2_proxy` in `crates/lb/src/main.rs:1238-1294` calls `with_hooks`,
`with_health`, `with_watchdog`, `with_h2_upstream`, `with_h3_upstream`,
`with_websocket`, `with_h2_extended_connect` and `with_grpc` — never
`with_glitches`. There is no config knob either: `rg glitch crates/lb-config/ config/`
returns nothing.

**Scenario.** An attacker opens one HTTP/2 connection to an `h1s` listener and
issues an unbounded stream of requests that trip a protocol-abuse rule — underscore
header rejects, smuggle rejects, malformed `:authority`, `:authority`/Host
disagreement, SNI mismatch. Each is individually rejected, but the HAProxy-style
`tune.h2.fe.glitches-threshold` response (weighted counter → threshold → cancel the
connection drain token → two-step GOAWAY) never fires, so the connection is never
punished or torn down. `h2_glitches_total` is never even registered, so
`/metrics` shows no evidence the abuse is happening.

`audit/deferred.md` (ROUND8-L7-07) states the opposite: *"The COUNTER half … is now
fully WIRED: `H2Proxy::with_glitches` creates one `GlitchesCounter` per H2
connection"*, citing `round8_glitches_enforced.rs` as proof. That test calls
`with_glitches` itself, so it proves the `lb-l7` API works, not that the binary
uses it.

---

### CW-06 — a partially-failed SIGHUP advances `applied_config`, permanently hiding the un-applied change — **MEDIUM** — *blocking for prod*

`crates/lb/src/main.rs:512`, after the per-listener rebuild loop:

```rust
    *applied_config = new_config;
```

This runs unconditionally, including when a rebuild returned `Err` at `main.rs:490-500`
("SIGHUP: L7 swap rebuild failed — keeping previous proxy live") or when no swap
handle existed at `main.rs:466-477`.

**Scenario.** Config has listeners `A` and `B`. The operator edits both backend
pools and SIGHUPs. `A` rebuilds fine; `B`'s rebuild fails (e.g. its new pool is
empty — see CW-08 — or an H3 CA mismatch). `B` keeps serving the **old** backends,
but `applied_config` now records the new ones. On the next SIGHUP,
`ConfigManager::reload()` returns `Ok(false)` for an unchanged file
("config file unchanged — nothing to reload"), and even a later edit that touches
only `A` produces a diff in which `B` is unchanged. **`B` never converges and
retrying the reload cannot fix it** — only a process restart or a further edit that
happens to touch `B`.

The pass also bumps `config_reload_succeeded_total` and `config_reload_applied_version`
(`main.rs:513-519`) whenever `applied_count > 0`, so a partially-failed reload
looks successful on the dashboard, with only a `warn` line to contradict it.

**Existing test coverage: none.** `tests/reload_under_traffic.rs` covers total
rejection (proof b) and total success (proof e), not a per-listener partial failure.

---

### CW-07 — a swappable change that was NOT applied still bumps the "applied" metric and logs "applied live" — **MEDIUM** — *blocking for prod*

`crates/lb/src/main.rs:503-510`:

```rust
    for change in &plan.swappable {
        if let Some(m) = metrics {
            m.applied_swappable_total
                .with_label_values(&[change.field()])
                .inc();
        }
        tracing::info!(field = change.field(), "SIGHUP: {}", change.describe());
    }
```

The loop is over the *plan*, not over what succeeded, and
`SwappableChange::describe()` (`crates/lb-config/src/reload.rs:32-37`) renders
`"listener {address}: L7 config changed ({fields}) — applied live"`.

**Two reachable cases produce a false "applied":**

1. Rebuild failed (`main.rs:495`) — the same pass logs both
   `"L7 swap rebuild failed — keeping previous proxy live"` *and*
   `"listener X: L7 config changed (backends) — applied live"`, and bumps
   `config_reload_applied_swappable_total{field="listener.l7"}`.
2. No swap handle — a `backends` change on a `quic`, `tcp` or `tls` listener is
   classified swappable by `diff_listener` (which does not know the listener's
   protocol has no `ArcSwap`). `main.rs:466` correctly bumps
   `restart_required_fields_total{field="listener.l7.no_handle"}` — and then
   `main.rs:503` then bumps `applied_swappable_total{field="listener.l7"}` for the
   same change and logs "applied live".

This is a direct breach of the honesty contract the reload is built around
(`reload.rs:1-4`: *"every restart-required change must be DETECTED and reported, or
the reload lies about what is running"*). An operator alerting on
`config_reload_applied_swappable_total` is told the opposite of the truth.

---

### CW-08 — a failed reload silently disables passive health ejection for that listener — **MEDIUM** — *blocking for prod*

`crates/lb/src/main.rs:561`, inside `rebuild_l7_proxies`, **before** any fallible step:

```rust
    health.reseed(&addresses);
```

then `main.rs:565` and `:571` can both fail:

```rust
    let upstreams_h1 = build_upstream_backends(new_l, &addresses)?;
    ...
        Some(build_h3_upstream_pool(&collect_h3_backends(new_l))?)
```

`HealthRegistry::reseed` (`crates/lb-health/src/ejection.rs:214-225`) drops every
entry not in the new set:

```rust
        inner.entries.retain(|addr, _| backends.contains(addr));
```

and `record_failure` / `record_success` early-return for an unknown address
(`ejection.rs:291`, `:258`: `let Some(entry) = ... else { return; }`).

**Scenario.** An operator removes all `[[listeners.backends]]` from an `h1`
listener and SIGHUPs. `validate_config` accepts an empty pool (it is documented as
"logs a warning and is skipped" at boot). `diff` marks it a swappable `backends`
change. `rebuild_l7_proxies` resolves zero addresses, calls `health.reseed(&[])`
which wipes the registry, then `build_upstream_backends` bails with
*"listener X has no backends configured"*. The old proxy stays live serving the old
backends — **which the health registry no longer tracks**. Ejection is now
permanently inert for that listener (`admits` fails open via `is_none_or`, so
traffic keeps flowing; failures are simply never recorded). Combined with CW-06 the
state is unrecoverable without a restart.

The comment at `main.rs:557-560` explains why the registry must survive a reload; the
bug is purely that the re-key happens before the steps that can abort.

---

### CW-09 — the per-connection drain grace is `random(0, drain_timeout_ms/4)`, not `drain_timeout_ms`; `drain_jitter_ms = 0` makes it zero — **MEDIUM** — *blocking for prod*

`crates/lb/src/main.rs:3206-3222`:

```rust
                () = conn_cancel.cancelled() => {
                    let jitter = {
                        let ceil = st.per_conn_drain_jitter_ms;
                        if ceil == 0 {
                            Duration::ZERO
                        } else {
                            use rand::RngExt;
                            Duration::from_millis(rand::rng().random_range(0..ceil))
                        }
                    };
                    tokio::select! {
                        biased;
                        r = &mut work => r,
                        () = tokio::time::sleep(jitter) => {
                            ... (None, Err(anyhow::anyhow!("connection cancelled by shutdown")))
```

`conn_cancel` is the **root** shutdown token (`main.rs:3072`), which
`Shutdown::run_drain` phase 5 cancels *before* it starts waiting
(`crates/lb-core/src/shutdown.rs:208-211`):

```rust
        self.tracker.close();
        self.token.cancel();
        let drain_outcome = if spec.inflight_drain_deadline > Duration::ZERO {
            match tokio::time::timeout(spec.inflight_drain_deadline, self.tracker.wait()).await {
```

So `drain_timeout_ms` is the budget for **already-cancelled** tasks to unwind, not
the budget for in-flight requests to finish. The grace an in-flight request actually
gets after cancel is a uniform draw from `[0, per_conn_drain_jitter_ms)`, which
defaults to `drain_timeout_ms / 4` = **2500 ms** (mean ~1250 ms), not 10 s.

**Scenario A (default config).** A request that has been running 30 s (large upload,
slow origin) is aborted a random sub-2.5 s after SIGTERM, while `docs/known-limitations.md`
promises *"waiting a bounded budget for in-flight requests to finish before
force-closing any survivors"* and `docs/guide/RUNBOOK.md:57-61` names
`drain_timeout_ms` (10 s) as that budget.

**Scenario B (foot-gun).** `docs/guide/CONFIG.md` documents `drain_jitter_ms`'s
`0` as *"`0` disables jitter"*. Setting it — a reasonable choice for an operator
who does not want a 75 s random delay after raising `drain_timeout_ms` to 300 s for
a streaming listener, exactly as RUNBOOK "Tuning the drain budget" advises —
sets `ceil == 0` → `Duration::ZERO` → **every in-flight connection is aborted on the
first poll after cancel, with no grace at all**. The knob documented as a
desynchronisation nicety is in fact the entire per-connection drain budget.

**Existing test coverage: none.** `tests/round8_drain_15case.rs` drives
`lb_core::Shutdown` in isolation with `jitter_max: Duration::ZERO` (line 78) and
never exercises `run_listener`'s cancel arm.

`ALREADY-KNOWN (adjacent, not this)`: `docs/known-limitations.md` "Graceful drain
does not emit proactive per-protocol close signals" documents force-close *when the
budget elapses*. It does not document that the budget is a quarter of the advertised
one, randomised, and operator-zeroable.

---

### CW-10 — the admin listener is killed before the drain starts, so `/readyz` is connection-refused (never 503) and no drain metric is scrapeable — **MEDIUM** — *non-blocking*

`crates/lb/src/main.rs:2606-2610`:

```rust
    // Cancel the admin listener BEFORE the coordinator so it does not serve `/readyz` Ready during
    // the settle window.
    admin_cancel.cancel();

    let report = shutdown.run_drain(spec).await;
```

`run_drain`'s **first** phase is `mark_draining` — the closure that calls
`probes.set_draining()` (`main.rs:2597-2601`) — and only then does it sleep for
`readiness_settle`. By the time the flip happens the admin accept loop has already
returned and dropped its listener
(`crates/lb-observability/src/admin_http.rs:183-190`).

**Consequences.**
- `/readyz` never returns 503 to anyone. The lameduck flip is unobservable, contradicting `docs/guide/RUNBOOK.md:42-45` (*"External LBs scraping `/readyz` see 503 on the next probe"*) and the `readiness_settle_ms` tuning table built on top of it.
- All drain-phase telemetry is unreachable during the window it describes: `MetricsDrainObserver` (`main.rs:2852-2893`) writes `shutdown_drain_seconds` into a registry that can no longer be scraped, and RUNBOOK's own troubleshooting instruction — *"If you see traffic landing on a draining pod (`accept_inflight` rising after `entering drain`)"* — cannot be followed.
- An upstream LB that distinguishes connection-refused (hard down) from 503 (graceful) takes the wrong branch.

The stated motive is avoidable: `set_draining()` runs before the settle sleep, so
moving `admin_cancel.cancel()` to after `run_drain` returns gives a 503 for the whole
window with no Ready race. The admin listener is not on the shutdown `TaskTracker`,
so it cannot hold the drain open.

---

### CW-11 — the advertised per-CPU XDP SYN-flood rate cap ships permanently DISABLED — **MEDIUM** — *blocking for prod (for XDP deployments)*

Userspace never writes the map. `XdpLoader::set_new_flow_cap`
(`crates/lb-l4-xdp/src/loader.rs:678`) has **zero callers workspace-wide**, and
`crates/lb/src/xdp.rs::try_attach_xdp` receives the whole `RuntimeConfig` but only
reads `xdp_enabled`, `xdp_interface` and `xdp_mode`.

The eBPF side's fallback for "not yet written" is unreachable
(`crates/lb-l4-xdp/ebpf/src/main.rs:334-343`):

```rust
fn is_under_flood() -> bool {
    let cap = match NEW_FLOW_CAP_CFG.get_ptr(0) {
        Some(p) => unsafe { *p },
        None => DEFAULT_NEW_FLOW_CAP_PER_CPU,
    };
    // Cap of 0 = operator disabled the rate limiter entirely.
    if cap == 0 {
        return false;
    }
```

`NEW_FLOW_CAP_CFG` is a `PerCpuArray<u32>` with `max_entries = 1`
(`ebpf/src/main.rs:318-319`); the kernel zero-fills array maps at creation and
`get_ptr(0)` on a valid index always returns `Some`. So the read is always `0`, the
`None` arm and `DEFAULT_NEW_FLOW_CAP_PER_CPU` are dead code, and
`is_under_flood()` unconditionally returns `false`.

The design comment at `ebpf/src/main.rs:321-324` states the intended invariant and
is the thing that is false: *"Since 0 in the cfg map means 'operator disabled',
not-yet-written is distinguished from disabled by consulting this fallback ONLY when
the slot is unreadable."* The slot is never unreadable.

**Scenario.** An operator enables `[runtime] xdp_enabled = true` and leaves
`xdp_new_flow_cap_per_sec_per_cpu` at its 125 000 default. A SYN flood of unique
5-tuples produces a conntrack miss per packet with no rate cap, which is precisely
the Katran `MAX_CONN_RATE` lesson the code cites. Setting the knob to any value
changes nothing in either direction. `docs/features.md` advertises *"per-CPU
new-flow rate cap"* as a shipped L4 feature, and `docs/guide/CONFIG.md` lists `0`
as a foot-gun disable value for a control that is already off.

**Existing test coverage: none for this path.**
`crates/lb-l4-xdp/tests/round8_synflood_cap.rs` tests only the userspace
`CtInsertGate` token bucket — itself a **TEST-ONLY** type with no production caller —
and asserts a constant. Neither `set_new_flow_cap` nor the eBPF map is touched.

Severity is held at MEDIUM only because XDP is off by default and documented as
single-kernel.

---

### CW-12 — no duplicate-listener-address validation, and the reload diff keys on address — **MEDIUM** — *non-blocking*

`validate_config` (`crates/lb-config/src/lib.rs:833-866`) iterates listeners
independently; there is no cross-listener check of any kind. Two `[[listeners]]`
blocks may share an `address`.

`LbConfig::diff` then matches listeners by address
(`crates/lb-config/src/reload.rs:185-187`):

```rust
        for old_l in &self.listeners {
            match new.listeners.iter().find(|n| n.address == old_l.address) {
```

`find` returns the first match, so with duplicates the diff compares the wrong pair
and reports a change against a listener that did not change. `ListenerReloadEntry`
lookup at `main.rs:471` has the same first-match problem, so a SIGHUP can rebuild
listener-1's proxies from listener-2's config.

At boot the second bind currently fails (CW-04 means `SO_REUSEPORT` is inert) and,
per CW-03, does so silently — so today a duplicated address yields one live listener
and one invisible dead one. If CW-04 is fixed without also adding this check, the
outcome becomes worse rather than better: two listeners with different protocols on
one port, with the kernel splitting new connections ~50/50 — e.g. a copy-paste where
one block is `h1s` and the duplicate is `tcp` would serve half of all connections in
plaintext. Worth fixing alongside CW-04.

---

### CW-13 — `[runtime.watchdog].header_deadline_ms` is validated, logged as wired, and read by nothing — **LOW** — *non-blocking*

`main.rs:2395-2400` logs it under a line claiming the watchdog is wired:

```rust
        header_deadline_ms = watchdog_cfg.header_deadline_ms,
        ...
        "SEC-2-03 Watchdog wired into accept-site + L7 proxies"
```

but the `WatchdogConfig` built earlier (`main.rs:2361-2365`) carries only
`min_rate_bps`, `rate_window` and `max_registered`. The per-connection deadline is
supplied by the proxy from a *different* knob
(`crates/lb-l7/src/h1_proxy.rs:752`):

```rust
            let deadline = std::time::Instant::now() + self.timeouts.header;
```

i.e. `[listeners.http].header_timeout_ms` (default 10 000), not
`[runtime.watchdog].header_deadline_ms` (default 5 000). A workspace grep shows
`header_deadline_ms` appearing only in the config crate, that one log line, and the
test below.

**The "wiring" test is a structural mirror that cannot fail.**
`tests/watchdog_wired.rs:37-43` re-implements `build_watchdog_like_main` locally,
re-implements the sweep loop at lines 62-73, and computes the deadline itself at
line 79 from `cfg.header_deadline_ms`. Its docstring claims *"Anyone who later drops
the `Watchdog::new` call or the sweep spawn from `main.rs` breaks this test"* — the
test never references `main.rs`, so deleting both would leave it green. It also
creates the false impression that `header_deadline_ms` is honoured.

---

### CW-14 — listener `address` is not parsed at validation time — **LOW** — *non-blocking*

`validate_listener` (`crates/lb-config/src/lib.rs:1036-1040`) checks only
non-emptiness, while `validate_observability` (`:868-883`) *does* parse
`metrics_bind` as a `SocketAddr`. A listener `address = "localhost:8080"` — a natural
thing to write, since `[[listeners.backends]].address` genuinely accepts `host:port`
and is DNS-resolved — passes validation and then fails `bind_addr.parse()` at
`main.rs:2895`, which per CW-03 is silent. Port `0` likewise validates and binds an
ephemeral port. Parsing the address in `validate_listener` converts both into a clear
boot-time error.

---

### CW-15 — `SECURITY.md`'s defenses table cites a deleted crate and three names that are unwired or do not exist — **LOW** — *non-blocking*

| Row | Cited site | Reality |
|---|---|---|
| 11 (QPACK bomb) | `crates/lb-h3/src/security.rs::QpackBombDetector` | `lb-h3` was deleted in S26; no such file or symbol exists anywhere. |
| 12 (Slowloris) | `crates/lb-security/src/slowloris.rs::SlowlorisDetector` | Type exists; **TEST-ONLY**, no production caller. |
| 13 (Slow-POST) | `crates/lb-security/src/slow_post.rs::SlowPostGuard` | No such type — it is `SlowPostDetector`, and it is **TEST-ONLY**. |
| 14 (0-RTT replay) | `crates/lb-security/src/zero_rtt.rs::ZeroRttReplayFilter` | No such type — it is `ZeroRttReplayGuard` (that one *is* wired). |

The table is the document an auditor reads to map attack → code. Rows 12 and 13 name
detectors that never run; the actual bounds are the timeout stack, which the
"Residual risks" section describes accurately under F-RES-5. The two halves of
`SECURITY.md` contradict each other.

---

### CW-16 — `config/default.toml` documents wrong values for two drain defaults — **LOW** — *non-blocking*

Under the header *"Runtime defaults (commented — apply automatically when `[runtime]`
is omitted)"*:

```toml
# drain_timeout_ms = 30000
# readiness_settle_ms = 1000
```

The real defaults are `10_000` and `11_000`
(`crates/lb-config/src/lib.rs:344-351`) — off by 3× and 11×, in opposite directions.
`docs/guide/CONFIG.md` has them right, so the shipped file is the outlier. An
operator sizing `terminationGracePeriodSeconds` from this file gets it wrong.
The commented `[listeners.http]` block also omits `head_timeout_ms` (default 60 000).

---

### CW-17 — `xdp_mode = "native"` is documented as failing startup; it warns and boots without XDP — **LOW** — *non-blocking*

`crates/lb-config/src/lib.rs:100-102`:

```rust
    /// XDP attach mode; `"native"` FAILS startup instead of silently degrading to 1-3 Mpps SKB.
```

`crates/lb/src/xdp.rs::attach_with_elf` returns `None` on any attach-ladder error and
`async_main` continues (`main.rs:2225-2229`), so the process boots with the XDP data
plane silently absent. Every failure mode in `try_attach_xdp` — missing capability,
missing ELF, loader parse failure, attach failure — is `warn`-and-continue. That is a
defensible fail-open posture (and the module docstring states it), but it is the
opposite of what the config doc comment promises for `native`.

---

### CW-18 — the control plane: `lb-cp-client` is a stub with no transport, `HaPoller` has no caller — **INFO** — *non-blocking*

Asked to establish live vs. dead explicitly:

- **`lb-controlplane::ConfigManager` + `FileBackend`: LIVE.** They are the SIGHUP reload path (`main.rs:2171-2192`, `:341-523`). See CW-01/06/07 for their defects.
- **`lb-controlplane::HaPoller`: TEST-ONLY.** No production caller; only `tests/controlplane_ha.rs`, which polls a temp file. There is no HA control plane, no leader election and no second instance — so there is no split-brain behaviour to review. The test's name implies a feature that does not exist.
- **`lb-cp-client`: STUB-DEAD.** Self-documented (*"No transport is implemented — no socket, no protocol, no config exchange anywhere in this crate, and nothing outside it links against it"*); `CpClient::connect` sets a bool. It is a **dev**-dependency of `lb`, not a runtime one. `ALREADY-KNOWN` per S45A.
- **Doc drift:** `SECURITY.md:15` lists *"the `lb-cp-client` channel"* as trust boundary 3 of the threat model. There is no channel. The threat model should say the config surface is the TOML file plus SIGHUP.
- **Config-push validation:** `ConfigManager::validate` (`lb-controlplane/src/lib.rs:185-196`) only checks non-empty + parses as `toml::Table`. That is adequate because `reload_config` re-runs the full `parse_config` + `validate_config` (`main.rs:374-392`) — the comment at `main.rs:337-339` says exactly this. No finding.

---

### CW-19 — validation gaps with no cross-field consistency check — **INFO** — *non-blocking*

- `validate_http_timeouts` (`lb-config/src/lib.rs:1186-1209`) checks only `> 0`. `header_timeout_ms = 60000` with `total_timeout_ms = 1000` validates clean; the total backstop then silently truncates the header phase. A `header <= total` / `body <= total` check would make the intent enforceable.
- `ticket_rotation_overlap_seconds` has **no** validation at all (`validate_tls_listener`, `lib.rs:1264-1291`, checks only `ticket_rotation_interval_seconds != 0`). Impact is bounded — only one previous key is retained, so a large overlap cannot accumulate keys — but the default `86_400` equals the default rotation interval, so the shipped posture keeps each ticket key decrypt-valid for 48 h, not the 24 h that "rotated daily" suggests.
- `cert_path` / `key_path` existence is not checked at validate time; it surfaces as a boot failure from `build_tls_bundle`, which is correct fail-fast behaviour. No finding.
- There is no `RLIMIT_NOFILE` check against `max_inflight_connections` (default 65 536). `packaging/expressgateway.service:103` sets `LimitNOFILE=1048576`, so systemd deployments are covered; a container run with a 1024 soft limit is not, though `classify_accept_error` handles EMFILE with backoff.

---

### CW-20 — spawn/shutdown hygiene notes — **INFO** — *non-blocking*

- **Untracked spawns.** `admin_http::serve_with_auth` (`crates/lb-observability/src/admin_http.rs:183`, `:201`) uses bare `tokio::spawn` for both the accept loop and each connection. The accept loop honours `admin_cancel`; in-flight admin connections have **no** shutdown path and die on runtime drop. Low impact (loopback-by-default, trusted), but it is the one place in the binary where a spawn is invisible to the `TaskTracker`.
- **Unbiased signal `select!`.** `LifecycleSignals::recv` (`main.rs:2762-2790`) has no `biased;`, so when SIGTERM and SIGHUP are both ready the choice is random and a reload (including its per-backend DNS resolution) can run before the drain. Nothing is lost — tokio's `Signal` streams latch and the loop is installed once outside — but `biased;` with SIGTERM/SIGINT first would make terminal signals win deterministically.
- **Boot ordering is sound.** Config is fully parsed and validated before any listener is constructed, and `probes.set_ready()` (`main.rs:2472`) runs after every listener spawns. Listeners do begin accepting slightly before `set_ready`, which is the safe direction. No half-configured serving window exists — except via CW-03.
- **`C-12` scopeguard is absent but harmless.** `crates/lb-core/src/shutdown.rs:157-159` requires the call site to scopeguard `run_drain` with its own XDP detach on panic; `main.rs` does not. The panic hook (`main.rs:72-101`) calls `std::process::abort()`, so no unwind ever reaches the drain, and the bpf link fd is released by the kernel on process death. Documented here so the contract is not assumed satisfied by design.
- **No privilege drop in-process.** Delegated to `packaging/expressgateway.service` (`User=`/`Group=`/`AmbientCapabilities=`/`NoNewPrivileges=true`). Consistent with the documented deployment model.

---

## 3. Cross-cutting note on test integrity

Four of the findings above are invisible to their own "proof" tests, all in the same
shape — the test re-implements or directly calls the library rather than exercising
the binary's wiring:

| Test | What it actually proves | Finding it misses |
|---|---|---|
| `crates/lb-security/tests/tls_versions.rs` | `build_server_config_with_policy(…, true)` builds | CW-02 (nothing calls it with `true`) |
| `crates/lb-l7/tests/round8_glitches_enforced.rs` | `H2Proxy::with_glitches` works when called | CW-05 (the binary never calls it) |
| `tests/watchdog_wired.rs` | a locally-rebuilt watchdog sweeps | CW-13 (`main.rs` is never referenced) |
| `crates/lb-l4-xdp/tests/round8_synflood_cap.rs` | a test-only `CtInsertGate` token bucket | CW-11 (the eBPF map is never written) |

A wiring test has to start at `main.rs` or at the binary. Any test that constructs
the subject itself can only prove the library, and the four above are each cited in
`SECURITY.md` or `audit/deferred.md` as evidence that a control is live.
