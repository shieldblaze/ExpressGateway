# S37-C — SIGHUP validate-first / atomic-swap hot reload: design note

Status: READ-ONLY DESIGN PREP (no product source touched, no cargo run).
Owner: reload-eng (workstream C). Builds on workstream B (config). Execute
after lead integrates B and rebases this branch onto it.

All citations are `file:line` against the s37-reload worktree at the time of
writing (branch `s37-c-reload`, HEAD = main `4480fb83`). Re-confirm line
numbers after the B rebase — B widens the config types.

---

## 0. One-paragraph summary

There is already a proven in-repo hot-swap pattern (TLS cert reload over
SIGUSR1: `Arc<ArcSwap<TlsConfigBundle>>` read at accept time, swapped via
`.store()`). We replicate that *shape* for the whole config: a SIGHUP handler
arm in the existing lifecycle-signal loop reads the file via the
already-wired `ConfigManager`, parses + runs the FULL `lb_config::validate_config`
(closing the TOML-shape-only gap), diffs old-vs-new to partition every changed
field into SWAPPABLE vs RESTART-REQUIRED, applies the swappable subset by
`.store()`-ing new per-listener snapshots into `ArcSwap`es read at accept
time, and emits a CLEAR per-field "requires restart, not applied" warning for
the restart-required subset (HONESTY rule — never a silent successful no-op).
In-flight connections keep the snapshot they captured at accept; only NEW
connections observe the new snapshot. The non-reload steady-state path becomes
`arcswap.load()` instead of a baked field read — a drop-in that must be
behavior-identical (R3-after-seam).

---

## 1. The proven cert-reload pattern to mirror

The TLS cert reload is the template. Trace it end to end:

- **The shared type** — `pub type SharedTlsBundle = Arc<ArcSwap<TlsConfigBundle>>;`
  (`crates/lb-security/src/ticket.rs:507`). `TlsConfigBundle`
  (`ticket.rs:475`) is an immutable validated snapshot (cert chain, key,
  built `Arc<rustls::ServerConfig>`). Doc at `ticket.rs:471-474` states the
  contract verbatim: "Every TLS listener holds one of these inside a
  `SharedTlsBundle` (an `ArcSwap`), reading `bundle.load()` per accept so a
  hot-reload swaps the bundle out under new connections without disturbing
  in-flight handshakes."

- **The read (accept time)** — the per-connection task snapshots the bundle
  with `let snapshot = bundle.load_full();` at `crates/lb/src/main.rs:3154`
  (TLS) and `:3222` (H1s). `load_full()` returns an owned
  `Arc<TlsConfigBundle>`; the connection then builds its `TlsAcceptor` from
  THAT snapshot (`:3155`, `:3223`) and holds it for the connection's whole
  life. A concurrent reload that `.store()`s a new Arc leaves this captured
  snapshot untouched (Arc refcount keeps it alive) — this is the exact
  in-flight-isolation guarantee C needs.

- **The swap (reload time)** — `reload_tls_bundle`
  (`crates/lb-security/src/ticket.rs:644`) loads + validates a NEW bundle
  from disk, then `bundle.store(Arc::new(new));` (`ticket.rs:653`). Critically
  it validates BEFORE the store: a botched cert never reaches `.store()`, so
  the previous bundle stays live (the "keeps old live" test at
  `ticket.rs:878`). This is validate-first at the bundle level — C generalises
  it to the whole config.

- **The registry** — `tls_reload_registry: Arc<PlMutex<Vec<TlsReloadEntry>>>`
  (`crates/lb/src/main.rs:2324`). Each TLS/H1s listener pushes a
  `TlsReloadEntry` (`main.rs:174`) holding its `SharedTlsBundle` + cert/key
  paths + alpn + rotator as it spawns (`main.rs:1621`, `:1690`). The signal
  handler iterates this registry on SIGUSR1.

- **The signal handler** — `main.rs:2469-2487`: a `loop` that calls
  `wait_for_lifecycle_signal().await`; on `SigUsr1` it clones the registry,
  calls `reload_all_tls(&entries, cert_metrics)` (`main.rs:235`), logs the
  (ok, fail) tally, and `continue`s (does NOT break the loop — reload is
  non-terminal). Any non-SigUsr1 signal breaks the loop into drain.

**How the atomic swap works under live handshakes (the property we
replicate):** ArcSwap is a lock-free RCU primitive. `load()`/`load_full()`
returns a consistent Arc snapshot; `store()` atomically replaces the pointer.
A reader that captured a snapshot before the store keeps reading the old Arc
(refcount-pinned) until it drops the guard; a reader after the store sees the
new one. There is NO tearing and NO reader blocking. For TLS this means an
in-flight handshake using the old `ServerConfig` completes on the old cert;
the next accept builds its acceptor from the new cert. We get config-snapshot
isolation for free by capturing the snapshot once at accept and threading the
OWNED Arc into the per-connection task (exactly as `load_full()` at
`main.rs:3154` already does).

---

## 2. Signal lifecycle: where SIGHUP plugs in

- `LifecycleSignal` enum — `crates/lb/src/main.rs:2719-2727`. Today:
  `SigTerm`, `SigInt`, `SigUsr1`. **Add `SigHup`.** Update the `Display`
  impl (`main.rs:2729-2737`).

- `wait_for_lifecycle_signal()` — `main.rs:2741-2781`. Under `#[cfg(unix)]`
  it installs `SignalKind::terminate()`, `interrupt()`, `user_defined1()`
  and `select!`s on them. **Add a 4th stream**
  `unix_signal(SignalKind::hangup())` and a 4th `select!` arm returning
  `LifecycleSignal::SigHup`. Mirror the existing per-handler install-failure
  fallbacks (`main.rs:2745-2769`) so a failed SIGHUP install degrades
  gracefully (fall through to the terminal-signal select rather than abort).

- The signal-servicing loop — `main.rs:2469-2487`. Today the `loop` breaks
  on any non-`SigUsr1` signal. **Add a `SigHup` arm that is also
  non-terminal** (mirrors the `SigUsr1` shape): it calls the new
  `reload_config(...)` routine, logs the partition outcome, and `continue`s.
  The match becomes: `SigUsr1 => cert reload, continue`;
  `SigHup => config reload, continue`; everything else => `break s` into
  drain. NOTE: cert reload and config reload are independent triggers
  (SIGUSR1 vs SIGHUP) — keep them separate so an operator can roll a cert
  without a full config diff and vice versa. (A follow-up could fold
  cert-reload into the config reload, but keeping the proven SIGUSR1 path
  untouched is R3-safer for v1.)

The `reload_config` routine needs, in scope at `main.rs:2469`: the
`ConfigManager` (currently `_config_manager`, `main.rs:2006` — UN-underscore
it), the set of per-listener `ArcSwap` config snapshots (new — see §3), the
metrics registry (for reload counters), and a way to surface the partition
result to the operator (log + metrics). All are already in `async_main`
scope at that point.

---

## 3. The listener-spawn seam: baked → snapshot per served path

The spawn for-loop is `async_main` `main.rs:2330-2389` (TCP family +
quic), plus the passthrough spawn at `:2392-2395`. Each path bakes the
swappable values into a long-lived struct read per-connection. The seam is:
**replace each baked field read with a `.load()` from a per-listener
`ArcSwap<ListenerSnapshot>` populated at spawn and `.store()`-d on reload.**

### 3a. TCP / TLS / H1 / H1s (`spawn_tcp`, `main.rs:1493`)

- The baked struct is `ListenerState` (`main.rs:316-374`), wrapped
  `Arc<ListenerState>` (`main.rs:1552`), cloned into every per-connection
  task (`main.rs:3099` `let st = Arc::clone(&state);`).
- Swappable values it bakes: `backends: Vec<Backend>` (`:319`),
  `balancer: Mutex<RoundRobin>` (`:321`), `addresses: Vec<SocketAddr>`
  (`:323`), `inflight: Arc<Semaphore>` from `max_inflight` (`:354`/`:1567`),
  `connect_timeout` (`:357`), `handshake_timeout` (`:350`), and `mode:
  ListenerMode` (`:346`) which itself bakes the L7 proxies.
- Per-connection reads to convert: balancer pick over `state.backends`
  (`main.rs:3083-3092`), `state.addresses.get(idx)` (`:3094`),
  `state.connect_timeout` (`:3144`,`:3185`), `state.handshake_timeout`
  (`:3160`,`:3228`), and the `match &st.mode` dispatch (`:3138-3284`).
- **The L7 proxies are the natural swap unit.** `ListenerMode::H1 { proxy:
  Arc<H1Proxy> }` (`main.rs:302`) and `H1s { h1_proxy, h2_proxy, ... }`
  (`:308`). The per-conn task does
  `Arc::clone(proxy).serve_connection_with_cancel(...)` (`main.rs:3203`,
  `:3257`, `:3270`). `H1Proxy` (`crates/lb-l7/src/h1_proxy.rs:305`) bakes
  EVERY L7 swappable: `picker` (backends/weights, `:307`), `timeouts`
  (`:309`), `ws` (`:314`), `h2_upstream`/`h3_upstream` (`:318`/`:320`),
  `smuggle_strict` (`:342`), `header_underscore_policy` (`:350`),
  `max_keepalive_requests` (`:366`). Because the per-conn task already
  clones the WHOLE `Arc<H1Proxy>` and serves the connection on it, the
  swap is clean: **hold `Arc<ArcSwap<H1Proxy>>` instead of `Arc<H1Proxy>`,
  `.load_full()` at accept, serve on the captured snapshot.** A reload
  rebuilds an `H1Proxy` from the new config and `.store()`s it. In-flight
  connections keep their captured `Arc<H1Proxy>` (the H2 builder, the
  picker, the timeouts) for their whole life — by-construction isolation.
- **Recommendation:** make the swap unit a NEW immutable
  `ListenerSnapshot` struct holding the per-listener swappable subset:
  `{ backends: Vec<Backend>, addresses: Vec<SocketAddr>, balancer:
  RoundRobin-state, mode: ListenerMode, connect_timeout, handshake_timeout,
  inflight-cap }`. `ListenerState` keeps the NON-swappable infra (metrics,
  io_runtime, pool, resolver, tracker, shutdown_token, listener_label,
  per_conn_drain_jitter_ms) as plain fields PLUS one field `snapshot:
  Arc<ArcSwap<ListenerSnapshot>>`. The per-conn task does
  `let snap = st.snapshot.load_full();` ONCE at the top (mirrors the cert
  `load_full()` at `:3154`) and reads `snap.backends`, `snap.mode`, etc.
  for the rest of the connection. NOTE the balancer is a `Mutex<RoundRobin>`
  with rotating state — see §10 (round-robin cursor) for the swap subtlety.

### 3b. QUIC H3-terminate + Mode B (`spawn_quic`, `main.rs:1198`)

- The chain: `QuicListenerParams` (`crates/lb-quic/src/listener.rs:44`) →
  `RouterParams` (`crates/lb-quic/src/router.rs:50`, built in
  `QuicListener::spawn` at `listener.rs:404`) → `ActorParams`
  (`crates/lb-quic/src/conn_actor.rs:200`, built per-connection in
  `spawn_new_connection` at `router.rs:408`).
- Swappable values: `RouterParams.backends: Arc<Vec<SocketAddr>>`
  (`router.rs:69`), `h3_backend`/`h2_backend` (`:73`/`:76`),
  `max_requests_per_h3_connection` (`:118`), `ws_enabled` (`:104`).
- **The per-connection snapshot read point is `spawn_new_connection`**
  (`router.rs:344`), specifically the `ActorParams { ... }` construction at
  `router.rs:408-438`, which copies `params.backends` (`:414`),
  `params.h3_backend` (`:415`), `params.max_requests_per_h3_connection`
  (`:436`), etc. into the per-conn actor. This runs ONCE per accepted QUIC
  connection — the exact analog of the TCP accept site.
- **Seam:** add `quic_snapshot: Arc<ArcSwap<QuicSnapshot>>` to
  `RouterParams`, where `QuicSnapshot` holds the swappable subset
  (`backends`, `h3_backend`, `h2_backend`,
  `max_requests_per_h3_connection`). In `spawn_new_connection`, do
  `let snap = params.quic_snapshot.load_full();` before building
  `ActorParams` and read `snap.backends` etc. instead of `params.backends`.
  Mode B (`raw_quic_backend.is_some()`) early-dispatches before any H3
  state — its backend is in `RawBackend` (an `Arc`-cheap clone); a Mode-B
  backend swap follows the same pattern but is LOWER priority (bucket it
  restart-required for the first increment — see §6/§10).
- NOTE: `cert/key/retry_secret/idle_timeout/max_recv_udp_payload_size` are
  baked into the `config_factory` closure (`listener.rs:373-383`) and the
  retry signer (`listener.rs:343`) at spawn — these are RESTART-REQUIRED
  for QUIC (transport params + retry secret are connection-establishment
  inputs; changing them mid-life is not a config-swap, it's a re-bind). TLS
  CERT for QUIC is a known gap even in the SIGUSR1 path (the QUIC listener
  is not in `tls_reload_registry`) — document as restart-required for v1.

### 3c. Mode A passthrough (`spawn_passthrough`, `main.rs:1452`)

- Bakes backends into `PassthroughParams.backends` (`main.rs:1459`) at
  spawn; the datapath is flow-keyed (a CID→backend flow table). The backend
  set is consulted when a NEW flow is admitted. Swappable in principle (read
  the backend list per new-flow), but the flow table + per-flow backlog
  make this the deepest-baked path.
- **Recommendation:** bucket Mode A passthrough backends as
  RESTART-REQUIRED for the first increment with honest docs; revisit as an
  expansion item only if needed. (Mode A is a niche datapath; the crown-jewel
  proofs do not require it.)

---

## 4. Drain / lifecycle: in-flight isolation

- `lb_core::Shutdown` (`crates/lb-core/src/shutdown.rs:73`) holds `token`,
  `listener_token` (a `child_token` of `token`, `:80`/`:101`), and a
  `TaskTracker`. Per-connection tasks are spawned via
  `st.tracker.clone().spawn(...)` (`main.rs:3122`) and select on
  `st.shutdown_token` (`main.rs:3111`,`:3299`).
- Reload does NOT cancel any token and does NOT touch the tracker. It only
  `.store()`s new snapshots. Therefore:
  - **In-flight connections** captured their snapshot at accept
    (`load_full()` → owned Arc) and keep it for the connection's whole
    life regardless of any reload. The old snapshot Arc stays alive via its
    refcount until the last in-flight connection holding it drops. ZERO
    cross-talk.
  - **New connections** accepted after the `.store()` `load_full()` the new
    snapshot.
- This is identical to how a SIGTERM-during-handshake already works for the
  cert bundle (`main.rs:3149-3154` comment: "snapshot the bundle live at
  accept time. A SIGUSR1 cert reload concurrent with an in-flight handshake
  leaves this snapshot intact until the connection drops").
- Interaction with drain: a SIGHUP arrives on the SAME loop
  (`main.rs:2469`) as SIGTERM. SIGHUP is non-terminal (continue); a
  subsequent SIGTERM breaks into the existing drain coordinator unchanged.
  No new drain phase. The reload routine must be FAST and non-blocking on
  the signal loop (parse+validate+diff+store are all CPU-cheap; no awaits
  on traffic) so it never delays a following SIGTERM.

---

## 5. ConfigManager: close the TOML-shape-only validation gap

- Today `ConfigManager::reload()` (`crates/lb-controlplane/src/lib.rs:189`)
  loads the file string, early-returns `Ok(false)` if unchanged
  (`:191-193`), then calls `Self::validate(&new_config)` (`:194`) which only
  checks non-empty + `parse::<toml::Table>()` (`:227-237`). It NEVER builds
  an `LbConfig` and NEVER calls `lb_config::validate_config`. This is the
  gap the owner flagged.
- The binary's own startup path is the gold standard: `parse_config` +
  `validate_config` (`main.rs:1953-1954`). Reload must do the SAME.
- **Two wiring options (recommend B):**
  - (A) Make `ConfigManager::reload` itself run the full validation. But
    `lb-controlplane` would need an `lb-config` dep edge (it currently
    works on raw strings, deliberately decoupled). Check the existing dep
    graph before adding this edge.
  - (B) **Keep `ConfigManager` string-level; do the full validation in the
    `lb` binary's `reload_config` routine** (the binary already depends on
    `lb-config`). Flow: `cfg_manager.reload()?` returns
    `Ok(true)` on a changed-and-TOML-valid file → binary reads
    `cfg_manager.current_config()` (`:203`) → `lb_config::parse_config` →
    `lb_config::validate_config`. If parse/validate FAILS, the binary calls
    `cfg_manager.rollback_to_previous()` (`:262`) so the manager's notion of
    "current" matches the still-live in-memory config (the file on disk is
    bad, but we never applied it). Then log the validation error + bump a
    failure counter + DO NOT swap anything (validate-first: old config stays
    fully live — the invalid-reload-no-blip proof, §8b).
  - Option B keeps the proven `ConfigManager` untouched and puts the
    `LbConfig`-typed logic where the `LbConfig` type already lives. Prefer B.
- After a SUCCESSFUL parse+validate, feed the new `LbConfig` into the diff
  (§6) and the swap (§3). The binary holds the LAST-APPLIED `LbConfig`
  (clone the startup `config` into an `applied: LbConfig` it keeps in scope)
  to diff against.

---

## 6. Config DIFF / partition: swappable vs restart-required taxonomy

The partition must be SINGLE-SOURCED (R12) and exhaustive (HONESTY rule:
every changed field is classified — no field silently ignored). Proposal:

- **Where it lives:** a new method on the config types, e.g.
  `impl LbConfig { pub fn diff(&self, new: &LbConfig) -> ReloadPlan }`
  in `crates/lb-config`. `ReloadPlan` carries `swappable:
  Vec<SwappableChange>` and `restart_required: Vec<RestartRequiredChange>`,
  each variant naming the field + listener index so the operator log is
  precise ("listener[0] backends changed: applied"; "listener[1]
  bind-address changed: REQUIRES RESTART, not applied").
- **Diffing prerequisite:** `LbConfig` / `ListenerConfig` do NOT currently
  derive `PartialEq` (`lib.rs:33`, `:592`). Add `PartialEq` to the config
  structs (cheap, additive) OR write explicit field comparisons in `diff`.
  Adding `PartialEq` is the lower-risk choice and helps the verifier.
- **Listener IDENTITY for the diff:** match listeners by `address`
  (the bind addr is the stable identity). A listener present in old but not
  new, or vice versa, OR with a changed `address`/`protocol`, is a
  RESTART-REQUIRED structural change (listener add/remove/rebind/proto).

**Taxonomy (owner-fixed buckets, mapped to concrete config fields):**

SWAPPABLE (apply via ArcSwap, ports stable):
- `ListenerConfig.backends[*].address / weight / protocol` — backend
  set/weights (`lib.rs:674`, `BackendConfig` `:1205`). [first increment]
- `ListenerConfig.http` (HttpTimeouts: header/body/total) (`lib.rs:628`).
- `[runtime].max_keepalive_requests` (`lib.rs:288`) → `H1Proxy.
  max_keepalive_requests`.
- `[runtime].max_requests_per_h3_connection` → `QuicSnapshot`.
- `[runtime].per_ip_connection_cap` (`lib.rs:238`) — per-ip caps (hooks).
- `ListenerConfig.h2_security` (`lib.rs:634`) — H2 security knobs.
- `ListenerConfig.websocket` enabled/limits (`lib.rs:641`) — WS knobs
  (NOTE: toggling WS H3 extended-connect changes the H3 SETTINGS frame, an
  establishment input — treat the H3 `h3_extended_connect` sub-field as
  restart-required even though H1/H2 WS knobs are swappable).
- `[security].strict_te` (`lib.rs:81`) → `H1Proxy.smuggle_strict`.
- TLS cert/key CONTENT (already swappable via SIGUSR1; the SIGHUP path can
  also trigger a `reload_tls_bundle` when the cert/key PATH is unchanged but
  an operator wants a single signal — but keep the proven SIGUSR1 path as
  the primary, see §2).

RESTART-REQUIRED (detect + warn per field, never silently no-op):
- Listener add / remove.
- `ListenerConfig.address` (bind address) — would need re-bind.
- `ListenerConfig.protocol` — changes the entire datapath.
- `[runtime].xdp_enabled` / `xdp_interface` / `xdp_mode` — XDP attach is a
  one-time boot action.
- TLS cert/key PATH change, ALPN set — re-registration territory.
- QUIC transport params (idle timeout, max_recv_udp_payload_size, retry
  secret path) — connection-establishment inputs baked into the
  config_factory / retry signer (`listener.rs:343`,`:362-383`).
- `[observability].metrics_bind` / `[admin]` bind — admin listener bind.
- `[passthrough]` backends (first increment; revisit later — §3c).

The honesty contract in code: `diff` returns the FULL `restart_required`
list; `reload_config` logs EACH at WARN with the field name + listener +
"requires restart, not applied", bumps a
`config_reload_restart_required_fields_total{field}` counter, and STILL
applies the swappable subset. A reload that contains ONLY restart-required
changes is logged as "applied 0 swappable, N require restart" — explicitly
NOT a silent success.

---

## 7. SMALLEST provable first increment

**Recommend: backend set + weights for the H1/H1s L7 path, single listener.**

Rationale:
- It exercises the entire seam end to end (signal → ConfigManager →
  full validate → diff → ArcSwap store → new-conn observes new backend)
  with the LEAST refactor, because the per-conn task already clones the
  whole `Arc<H1Proxy>` (`main.rs:3203`) and serves on it — wrapping that one
  Arc in `ArcSwap` and rebuilding the proxy on reload is a contained change.
- It gives the strongest, most legible proofs: backend-set change is
  directly observable (request after reload hits the NEW backend; the
  negative control — a naive `H1Proxy` field mutation — would race in-flight
  requests).
- It avoids the balancer-cursor subtlety (§10) if the first increment
  changes the backend SET (different addresses) rather than just weights —
  a new backend address is unambiguous to observe.

Concretely for increment 1:
1. `LbConfig::diff` + `ReloadPlan` (the partition skeleton; only the
   backend-change variant wired, the rest classified but routed to the
   restart-required warning path).
2. `H1Proxy` held as `Arc<ArcSwap<H1Proxy>>` in `ListenerMode::H1`/`H1s`;
   accept-time `load_full()`; reload rebuilds the proxy from the new
   listener config (reuse `build_h1_proxy` / `build_listener_mode`
   machinery, `main.rs:1651`,`:1589`) and `.store()`s it.
3. SIGHUP arm in the signal loop (§2) calling `reload_config`.
4. `reload_config` doing validate-first (§5 option B) + the partition log.

**Expansion path** (each adds one `.store()`-able field, no new seam):
timeouts (`http`) → `max_keepalive_requests` → `smuggle_strict` /
`header_underscore_policy` → per-ip caps → H2/WS knobs → QUIC
`QuicSnapshot` (backends + `max_requests_per_h3_connection`) → TLS cert via
SIGHUP. Mode A passthrough backends last (or stay restart-required).

---

## 8. The R13 SIX proofs (real running gateway under traffic)

Harness to mirror: `tests/reload_zero_drop.rs` `mod drain` — it spawns the
REAL binary (`Command::new(bin).arg(config)`, `try_spawn_gateway`
`:338`), spins real in-process backends (`spawn_blocking_h1_backend`
`:430`, `spawn_slow_h1_backend` `:543`), polls TCP readiness, and delivers
signals via raw `kill(2)` (`sigterm` `:389`). Add a `sighup(child)` helper
(SIGHUP = signal `1`) cloning the `sigterm` shape exactly. Also reference
`tests/h3_s3_inflight_h1_drain_proof.rs` for the in-flight-at-signal pattern.

(a) **live-connection-survives-reload + NEGATIVE CONTROL.**
   - Positive: spawn gateway + backend; open a long-lived connection that
     is mid-exchange (a gRPC bidi RPC streaming, AND separately a live WS
     echo session); deliver SIGHUP that changes a swappable field (e.g.
     add a backend); assert the in-flight RPC/WS completes normally (no
     RST, no GOAWAY, no 1006 close) AND that the reload took effect for
     NEW connections. The captured-snapshot design (§4) makes this hold by
     construction; the test PROVES it on the wire.
   - Negative control: build a deliberately-naive swap variant (behind a
     test-only cfg/flag) that mutates the live `H1Proxy`/snapshot
     IN PLACE (or drops + rebinds) instead of ArcSwap-storing a new
     snapshot; show that variant RESETS the in-flight RPC/WS (demonstrating
     the swap discipline is load-bearing, not incidental). Document the
     negative control as a one-shot demonstration, not a permanent gate, if
     wiring a second swap path is too invasive — at minimum, assert that
     the OLD snapshot Arc is still referenced by the in-flight task while a
     NEW one is live (refcount / pointer-identity assertion via a test seam).

(b) **invalid-reload-no-blip.** Open a steady stream of requests; deliver
   SIGHUP after writing an INVALID config to disk (e.g. a backend with an
   unparseable address, or a value out of `validate_config` range). Assert:
   zero request failures across the SIGHUP, the gateway logs a validation
   error + bumps `config_reload_failed_total`, and a follow-up request
   still hits the OLD (pre-SIGHUP) backend set (proving the old config
   stayed fully live; validate-first never partially applied). This is the
   `reload_tls_bundle_invalid_keeps_old_live` property (`ticket.rs:878`)
   lifted to the whole config.

(c) **restart-required-honesty.** Write a config whose ONLY change is a
   bind-address (`ListenerConfig.address`) or a listener add/remove;
   deliver SIGHUP. Assert: (i) the gateway logs a CLEAR per-field "bind
   address change requires restart, not applied" WARN + bumps
   `config_reload_restart_required_fields_total{field="address"}`;
   (ii) the bind did NOT silently change — the gateway is STILL listening on
   the ORIGINAL port and a connect to the would-be-new port is refused;
   (iii) it is NOT reported as a clean success (the log/metric explicitly
   says "applied 0 swappable"). This is the core honesty proof.

(d) **no-cross-talk.** Open connection A (captures snapshot v1); SIGHUP to
   a new swappable value; open connection B (captures snapshot v2); drive
   BOTH concurrently; assert A keeps observing v1 behavior (e.g. routes to
   the v1 backend set) for its whole life while B observes v2 — no leakage
   either direction. Mirrors the per-conn captured-Arc isolation (§4).

(e) **reload-TAKES-EFFECT.** Baseline: request hits backend-1. SIGHUP a
   config that swaps the backend set to backend-2 (a DIFFERENT in-process
   backend on a different port). Assert a NEW connection's request now hits
   backend-2 (observable via which backend's request-counter increments, or
   a distinguishing response header the test backends set). The
   complement of (a)'s "old conns unaffected".

(f) **R3-after-seam.** The existing e2e + soak suite passes UNCHANGED with
   the seam in place AND no SIGHUP ever sent — i.e. the steady-state
   (no-reload) path is behavior-identical. Concretely: run the full `×3`
   workspace test sweep, h2spec, h3spec, the WS + gRPC e2e suites, R8
   backpressure, and a re-soak; all must match the pre-seam baseline. The
   ArcSwap read replacing the baked read is the only steady-state delta —
   §9 enumerates exactly which reads change so the verifier re-proves this
   independently.

---

## 9. R3-AFTER-SEAM RISK SURFACE (the verifier re-proves this list)

These are EXACTLY the steady-state reads that change from baked-field to
`arcswap.load()`. Every other read is untouched. Designed as a drop-in so
the no-reload path is behavior-identical (a single extra `load_full()` at
accept, then identical field reads off the snapshot).

For the FIRST INCREMENT (backend set, H1/H1s):
1. `crates/lb/src/main.rs:3203` (and `:3257`,`:3270`) — `Arc::clone(proxy)`
   becomes `proxy.load_full()` where `proxy: Arc<ArcSwap<H1Proxy>>`. The
   served `Arc<H1Proxy>` is byte-identical at steady state (the ArcSwap
   holds the same proxy that was baked before).

For the FULL build-out (added incrementally, each its own R3 check):
2. `main.rs:3083-3092` — balancer pick over `state.backends` →
   `snap.backends` (`ListenerSnapshot`).
3. `main.rs:3094` — `state.addresses.get(idx)` → `snap.addresses`.
4. `main.rs:3144`,`:3185` — `st.connect_timeout` → `snap.connect_timeout`.
5. `main.rs:3160`,`:3228` — `st.handshake_timeout` →
   `snap.handshake_timeout`.
6. `main.rs:3138-3284` — `match &st.mode` → `match &snap.mode`.
7. QUIC: `crates/lb-quic/src/router.rs:408-438` — `ActorParams` fields read
   from `params.backends`/`h3_backend`/`h2_backend`/
   `max_requests_per_h3_connection` → `snap.*` after a `load_full()` at the
   top of `spawn_new_connection` (`router.rs:344`).

NON-changes to assert stay byte-identical:
- The cert path (`main.rs:3154`,`:3222`) is ALREADY an ArcSwap read —
  unchanged. (Optionally the config snapshot and the TLS bundle live side
  by side; do NOT merge them in v1 — keep the proven SIGUSR1 cert path.)
- All infra fields on `ListenerState` (metrics, io_runtime, pool, resolver,
  tracker, shutdown_token, listener_label, per_conn_drain_jitter_ms) stay
  plain fields — NOT moved into the snapshot.
- The drain coordinator (`main.rs:2490-2600`), tokens, tracker — untouched.
- QUIC config_factory + retry signer (`listener.rs:343`,`:362-383`) —
  untouched (those inputs are restart-required).

Steady-state cost: one extra `ArcSwap::load_full()` per accepted connection
(a single atomic load + Arc clone — the same primitive the cert path
already pays at `:3154`). Negligible; no per-request cost (snapshot captured
once per connection).

---

## 10. Risks / unknowns to flag to lead BEFORE execution

1. **Round-robin balancer cursor across a backend swap.** `ListenerState.
   balancer: Mutex<RoundRobin>` (`main.rs:321`) holds rotating cursor
   state. If the snapshot owns the balancer, a swap RESETS the cursor (mild
   distribution blip, not a correctness bug). Options: (i) keep the
   `Mutex<RoundRobin>` on `ListenerState` (NOT in the snapshot) and only put
   `backends`/`addresses` in the snapshot, re-validating the cursor index
   against the new backend count at pick time (cursor `% len`); (ii) accept
   the reset. Recommend (i) — keeps the cursor stable across swaps. FLAG:
   confirm the owner is OK with a backend swap that may briefly skew RR
   distribution by one rotation regardless. LOW risk.

2. **`H1Proxy` rebuild cost + dep edges on reload.** Rebuilding the proxy
   on reload re-runs `build_h1_proxy`/`build_upstream_backends`/
   `build_h2_upstream_pool` (`main.rs:1641-1664`) which re-resolves DNS and
   may rebuild the H2 upstream pool. This is fine (reload is rare) but means
   reload does async DNS work — so `reload_config` is NOT purely CPU-cheap
   if backends change. FLAG: this puts an `await` (DNS resolve) on the
   SIGHUP servicing path. Mitigation: that path is the signal loop, not a
   traffic path; a slow reload only delays a CONCURRENT SIGTERM. Acceptable,
   but lead should confirm. (Alternative: resolve on a spawned task and
   `.store()` when ready — adds complexity; defer.)

3. **`PartialEq` derive on config types.** Needed for the diff (§6). The
   config types hold `Option<...>` + `Vec<...>` of nested config structs;
   adding `PartialEq` is additive but touches MANY structs in `lb-config`.
   LOW risk, but it is a `lb-config` change in C's scope that overlaps
   workstream B's territory — COORDINATE with config-eng so we do not
   both edit the same derives. FLAG to lead.

4. **QUIC `ActorParams.backends` is `Arc<Vec<SocketAddr>>` already**
   (`router.rs:69`/`:414`) — a swap is cheap (swap the inner Arc). But the
   H3 backend pools (`h3_backend`/`h2_backend`) hold live upstream
   connection pools; swapping them mid-life needs the same in-flight-keeps-
   old-pool discipline. Recommend QUIC backends are in increment 2+, after
   the L7 path proves the pattern. LOW–MED risk; flag the pool-swap detail.

5. **Mode A passthrough deep-baked** (§3c). Recommend bucketing its backends
   restart-required for v1 rather than a risky flow-table refactor. FLAG for
   an owner call: is restart-required Mode-A-backend acceptable for v1?
   (Recommend yes — Mode A is a niche datapath and the crown-jewel proofs
   do not need it.)

6. **No product-fork anticipated.** The whole design reuses the proven
   ArcSwap pattern + existing builders. The only NEW types are
   `ListenerSnapshot`/`QuicSnapshot` (plain data) + `LbConfig::diff` +
   `ReloadPlan`. No upstream-dep fork, no large rip-and-replace. If any
   single swappable field turns out to be deeply baked behind a non-`Arc`
   long-lived handle (the way the QUIC config_factory bakes transport
   params), the fallback is to bucket THAT field restart-required with
   honest docs (§6) rather than force a risky plumbing job — the HONESTY
   rule makes that an acceptable, transparent v1 outcome.

7. **`arc-swap` dep was REMOVED from the workspace** (`Cargo.toml:88-91`,
   CODE-2-12 removed it as unused). It survives only as a per-crate dep in
   `lb-security` (`crates/lb-security/Cargo.toml:35`). C must RE-ADD
   `arc-swap = "1"` at the crate level for `lb` (and `lb-quic` when QUIC
   lands) — the CODE-2-12 comment literally anticipates this ("if a real
   hot-swap site lands later ... re-add it at the crate level under
   lb-security or lb"). Trivial, but note it for the deny.toml / machete
   gates (a re-added dep with a real consumer is clean). FLAG: coordinate
   with workstream D (deps) so the re-add does not collide with a D bump.
