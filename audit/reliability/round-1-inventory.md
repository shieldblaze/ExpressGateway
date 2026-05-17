# Round 1 — Reliability Inventory (`rel`)

**Scope:** SRE-style operability, observability, and failure-mode audit of
ExpressGateway. Sources: every file under `crates/`, `docker/`, root docs,
and `tests/`. No source changes proposed in this round.

Conventions used below:
- `file:line` references are exact at HEAD of `main` (commit `ac58f61`).
- "**Mismatch**" tags a doc claim that contradicts the source.
- "**Phantom**" tags a code path that exists but is wired to nothing on the
  production path.

---

## 0. Headline gaps (read first)

These are the disagreements the rest of this document expands on:

| # | Doc claim | Code reality |
|---|-----------|--------------|
| H1 | `RUNBOOK.md:31-42` says SIGHUP reloads config via `ConfigManager` + `ArcSwap`. | `crates/lb/src/main.rs` has no `signal::unix::SignalKind::hangup` handler. `ConfigManager` (`crates/lb-controlplane/src/lib.rs:156`) is **phantom** — never instantiated by the binary. `ArcSwap` is not a dep anywhere. |
| H2 | `RUNBOOK.md:49-57` describes TLS cert rotation via SIGHUP + `ArcSwap<TlsStore>`. | No `TlsStore`, no `ArcSwap`. `TicketRotator` rotates the **ticket key only**, not the certificate; the cert is bound at startup in `build_tls_stack` (`main.rs:214`). |
| H3 | `README.md:15` "Zero-downtime reload via `SO_REUSEPORT` and FD passing". | No FD-passing code. `tests/reload_zero_drop.rs` exercises only the in-process `ConfigManager`, not any socket reuse. |
| H4 | `DEPLOYMENT.md:148-149` "Health: no external health endpoint." | `crates/lb-observability/src/admin_http.rs:55` serves `GET /healthz` returning unconditional 200. The endpoint exists but is **liveness-only and lies** (returns 200 even while listeners are dead). |
| H5 | `METRICS.md:47` table lists 7 hot-path metrics as wired. | They are registered (`main.rs:849`), but `http_requests_total` / `http_request_duration_seconds` are **per-connection**, not per-request, despite the label name. Documented under "partial" in `METRICS.md:75` but the dashboard query in the same file (`METRICS.md:131`) implies per-request — operators will get misleading dashboards. |
| H6 | RUNBOOK references the binary as `lb`. | `crates/lb/Cargo.toml:11` declares `[[bin]] name = "expressgateway"`. The Docker image also installs it as `expressgateway`. The string `/usr/local/bin/lb` in `RUNBOOK.md:161` and the unit file in `DEPLOYMENT.md:44` will both 404 in a real install. |
| H7 | `RUNBOOK.md:25` claims `KillMode=mixed` drains gracefully within `TimeoutStopSec=30`. | `crates/lb/src/main.rs:1037-1060` aborts listener tasks **with `JoinHandle::abort()`** and then sleeps a fixed 500 ms before exiting. There is no in-flight tracking; any connection mid-`copy_bidirectional` is severed at 500 ms — well before the 30 s grace. QUIC has a 2 s timeout. SIGTERM is **not** graceful in any meaningful sense. |
| H8 | `crates/lb-health/src/lib.rs` defines a backend `HealthChecker`. | **Phantom.** No file outside the crate's own tests references it; the round-robin balancer picks every backend forever regardless of liveness. A dead backend stays in rotation until the kernel `connect(2)` fails per-request. |
| H9 | RUNBOOK / DEPLOYMENT make systemd-unit claims. | No unit file ships in the repo — `find . -name '*.service'` returns nothing. The docs are aspirational. |
| H10 | DEPLOYMENT mentions JSON logs. | `main.rs:931` uses default `tracing_subscriber::fmt()` — **plain text**, not JSON. There is no `tracing-subscriber/json` feature in deps. |

These ten land first because they each invalidate a section of the
existing operator-facing runbook.

---

## 1. Failure-mode catalogue

Twenty-five plausible failure modes. "Designed behaviour" = what the LB
ought to do; "Actual behaviour" = what the code does today; "Site" = the
first source line that owns the decision.

| ID | Failure | Designed | Actual | Site |
|----|---------|----------|--------|------|
| F-01 | Backend `connect(2)` refused | retry next backend, mark unhealthy, increment counter | `proxy_connection` returns error; client connection dropped; balancer **does not skip** the dead peer; `pool_probe_failures_total++` only when pool acquire errors (not on dial failure of a fresh socket) | `crates/lb/src/main.rs:1238` (acquire); `crates/lb-io/src/pool.rs:167` |
| F-02 | Backend `connect(2)` hangs (SYN black-hole) | bounded connect timeout, fail fast | `Runtime::connect` uses blocking `std::net::TcpStream::connect_timeout` with **no caller-supplied bound** (default ≈ 75 s on Linux). spawn_blocking holds a tokio blocking worker the whole time | `crates/lb-io/src/lib.rs::Runtime::connect` |
| F-03 | Backend slow upstream (TCP) | per-direction idle timeout, watchdog | `tokio::io::copy_bidirectional` has no timeout; connection can park forever | `crates/lb/src/main.rs:1255` |
| F-04 | Backend half-closes mid-stream | propagate half-close cleanly, return socket to pool only if reusable | Detected: `set_reusable(false)` on error path | `main.rs:1270` (correct) |
| F-05 | TLS handshake stall on accept | bounded `accept` timeout, drop | `acceptor.accept(client_stream).await` has no wrapper timeout. Slowloris-style: client sends ClientHello bytes one byte at a time → tokio task and rustls state held indefinitely | `main.rs:1136`, `:1154` |
| F-06 | TLS cert rotation | atomic swap, in-flight TLS handshakes use either old or new without race | **Not implemented.** Cert is captured once into `TlsAcceptor`; only reload mechanism is full restart. Doc lies. | `main.rs:214,611` |
| F-07 | Ticket key rotation race | overlap window honours sessions encrypted with previous key | `TicketRotator` keeps prev key for `overlap`; ticker fires every 60 s | `crates/lb-security/src/ticket.rs::TicketRotator`; ticker `main.rs:232` |
| F-08 | Config reload with bad TOML | reject, keep old config, log error, stay healthy | **Not wired.** No SIGHUP handler exists. Operator's only recourse is restart. Doc lies. | (none) |
| F-09 | Config reload with valid-but-wrong (e.g. listener removed) | drain old listener, bind new, no dropped bytes | **Not wired.** | (none) |
| F-10 | Control-plane partition (cp-client crate) | tolerate, serve last-known-good | `lb-cp-client` exists in workspace but is never linked from `crates/lb` (`cargo tree -p lb` does not include it). HA poller in `lb-controlplane` is also phantom. | `crates/lb-controlplane/src/lib.rs:275` |
| F-11 | `accept(2)` returns EMFILE (fd exhaustion) | bounded retry with backoff, page operator, shed | `accept` loop: `tracing::warn!("accept error: {e}"); continue;` — **tight loop**; on EMFILE the warning logs at line speed and the CPU saturates without forward progress | `main.rs:1099-1106` |
| F-12 | Conntrack pressure (kernel) | depend on sysctl `nf_conntrack_max`; emit `xdp.conntrack.entries` | XDP path not wired (Pillar 4b); kernel conntrack uncovered by app metric | DEPLOYMENT.md:99 (sysctl only) |
| F-13 | DNS resolver outage | use cached `positive_ttl_cap` (300 s) entries, then negative cache 5 s | Honoured by `DnsResolver`; but the cache is only refreshed at startup or via `spawn_background_refresh` — **and `spawn_background_refresh` is never called from main.rs**. Stale entries survive only until per-request lookup at acquire time, which is **not done today** — backends are resolved exactly once in `spawn_tcp` and frozen | `crates/lb-io/src/dns.rs:328`; `main.rs:670` |
| F-14 | DNS returns multiple A records | round-robin across all of them | Only `first` is kept (`main.rs:683`). 90% of multi-A backends are wasted. |
| F-15 | DNS returns NXDOMAIN for a backend | fail listener startup with a clear error; never serve traffic to nothing | Honoured: `bail!("resolver returned no addresses for ...")`. | `main.rs:683-685` |
| F-16 | Backend pool exhaustion | shed with bounded queue or block with deadline | Pool `acquire` blocks on a fresh dial when no idle entry is present; no shed-policy; spawn_blocking pool used for dial → tokio blocking worker exhaustion at scale | `main.rs:1238`; `pool.rs:167` |
| F-17 | `tokio::spawn` per connection unbounded | bounded inflight per listener (Semaphore), 503/shed on overflow | **Unbounded.** Every accepted connection spawns; no semaphore, no max-connections gate at the listener level except QUIC's `max_connections: 100_000` | `main.rs:1126`; `lb-quic/src/listener.rs:245` (only QUIC) |
| F-18 | H2 settings-flood / continuation-flood / RST flood | detect and tear down connection; emit metric | Detectors exist (`crates/lb-h2/...`) but `METRICS.md:103-107` shows they emit no metrics; integration not verified here | (review by `proto`/`sec`) |
| F-19 | TLS 1.3 0-RTT replay | reject replays via per-listener guard | `ZeroRttReplayGuard` exists and is held by `QuicListener` only; TCP/TLS listeners do **not** install one | `crates/lb-quic/src/listener.rs:279` (QUIC only) |
| F-20 | XDP attach failure at boot | log, fall through to kernel-TCP path, never panic | Implemented correctly | `crates/lb/src/xdp.rs:59-99` |
| F-21 | XDP map exhaustion (CONNTRACK full) | drop with `xdp.packets.dropped` counter, alert | Not wired into userspace (`METRICS.md:67`). Kernel-side STATS map exists. |
| F-22 | Process panic (panic-free rule violation) | catch in worker, restart, alert | No `std::panic::set_hook`; panics in spawned tasks are silently logged to stderr by tokio default. `RUNBOOK.md:148` claims libs are panic-free but the lints (`clippy::unwrap_used` allowed in tests) do not catch the dozens of `.unwrap_or_else` paths that can still hit infallible branches | `main.rs:920-927` |
| F-23 | `journalctl` filter `liveness probe` | matches log line emitted by pool probe | The probe fires inside `pool.rs::validate_and_upgrade`; the only log lines I find from there are at `tracing::debug!` level; with default `RUST_LOG=info` operators see nothing | `pool.rs:193` |
| F-24 | Admin listener bind fails (port in use) | exit non-zero or skip cleanly | Currently logs error and **continues** (`main.rs:1001-1005`); silent loss of `/metrics`. Reasonable, but the `serve` call returned `Ok` for a non-existent listener if cancellation races — see code review |
| F-25 | OOM under retransmit storm | bounded WS frame size, bounded HPACK table, bounded H2 stream window | Mostly honoured via `H2SecurityThresholds` + `WsConfig::max_message_size`; the **listener-level total** in-flight bytes is not bounded |  `main.rs:493-501`, `:569-605` |

---

## 2. Lifecycle / spawn inventory

### 2.1 Long-lived spawn sites

| Task | File:line | Started | Shutdown signal | Force-close fallback |
|------|-----------|---------|-----------------|----------------------|
| TLS ticket rotator ticker | `crates/lb/src/main.rs:233` | per TLS/H1s listener | `Arc::strong_count(&rotator) <= 1` — implicit on listener drop | none; relies on Arc count drop |
| Per-listener accept loop | `crates/lb/src/main.rs:701`→`run_listener` at `:1077` | one per non-QUIC listener | `JoinHandle::abort()` at `main.rs:1038` (cancels mid-`await`) | implicit cancel = abrupt drop |
| Hot-path sampler (pool idle + DNS size) | `crates/lb/src/main.rs:892` | once at startup | **none** — task runs until process exits | runs through SIGTERM grace |
| Per-connection worker | `crates/lb/src/main.rs:1126` | per accepted conn | **none** — no cancellation token, no in-flight registry | severed when binary exits |
| Admin HTTP accept loop | `crates/lb-observability/src/admin_http.rs:102` | startup if `[observability].metrics_bind` set | `CancellationToken` (`admin_cancel`) wired from main | per-conn tasks have no cancel |
| Admin HTTP per-connection | `admin_http.rs:120` | per request | **none** | abrupt at process exit |
| QUIC router | `crates/lb-quic/src/router.rs:113` | per QUIC listener | `CancellationToken` (`shutdown`) | 2 s wall in `main.rs:1049` |
| QUIC datagram task | `crates/lb-quic/src/lib.rs:706` | per connection | propagated via router cancel | none explicit |
| QUIC stream task | `crates/lb-quic/src/lib.rs:778` | per stream | propagated | none |
| QUIC conn-actor request tasks | `crates/lb-quic/src/conn_actor.rs:296,307,320` | per stream | implicit on actor stop | none |
| H1 WS upgrade task | `crates/lb-l7/src/h1_proxy.rs:581` | per WS upgrade | **none** | abrupt at process exit |
| H2 driver | `crates/lb-l7/src/h2_proxy.rs:400` | per H2 conn | **none** | abrupt |
| H2 backend conn driver | `crates/lb-l7/src/h2_proxy.rs:487` | per dial | **none** | abrupt |
| H2 pool conn driver | `crates/lb-io/src/http2_pool.rs:284` | per pool conn | **none** | abrupt |
| gRPC backend driver | `crates/lb-l7/src/grpc_proxy.rs:250` | per dial | **none** | abrupt |
| DNS singleflight init | `crates/lb-io/src/dns.rs:340` | per cache fill | terminates naturally | n/a |

**The pattern:** only QUIC + the admin listener honour a cancellation
token. Everything else exits because (a) the runtime drops or (b) a
`JoinHandle::abort()` severs it.

### 2.2 SIGTERM entry point

`crates/lb/src/main.rs:1279` — `shutdown_signal()` awaits either
`ctrl_c` or `SignalKind::terminate()`. The handler then:

1. `h.abort()` on each TCP listener handle (`:1038`) — kills accept loop;
   in-flight per-connection tasks **are not tracked**, so abort here
   does not propagate to them.
2. `admin_cancel.cancel()` — clean.
3. QUIC: `listener.shutdown()` → token cancelled; `timeout(2 s)` wait.
4. `sleep(500 ms)` fixed-duration "drain". (`:1060`)
5. Process exits, severing every per-connection task.

**Bounded window:** 2 s (QUIC) + 0.5 s (sleep) = **~2.5 s actual drain
budget** under default config — *not* the 30 s the systemd unit
advertises. The 30 s would only matter if the binary held the process
alive that long; it doesn't.

### 2.3 SIGHUP

**Not wired.** Search `crates/lb/src/main.rs` for
`SignalKind::hangup` — zero hits. The RUNBOOK reload section is
fictional.

---

## 3. Health endpoints

| Endpoint | File | Semantics | K8s mapping |
|----------|------|-----------|------------|
| `GET /healthz` on `metrics_bind` | `crates/lb-observability/src/admin_http.rs:55` | always 200; body `ok\n` | suitable as **process-liveness only** |
| `GET /metrics` | same | text exposition | n/a |
| (none) | n/a | readiness | **missing** |
| (none) | n/a | startup | **missing** |

There is **no distinction** between liveness, readiness, and startup.
Concretely:

- During listener bind failure for one of N listeners, the process may
  still expose 200/healthz while serving partial traffic.
- During SIGTERM drain, `admin_cancel.cancel()` fires at `main.rs:1041`
  — the admin listener stops accepting **at the start of drain**, so an
  external load balancer that polls `/healthz` cannot observe "I am
  draining; route new traffic elsewhere." It either gets 200 or
  connection-refused.

Recommendation in round 2: split into `/livez` (process running),
`/readyz` (every listener bound + at least one backend healthy), and
keep `/healthz` for back-compat returning `/livez`. During drain
`/readyz` must flip to 503 *before* listeners stop accepting.

---

## 4. Hot-reload audit

### 4.1 Certs

- **Wire:** captured at startup (`build_tls_stack` / `build_h1s_tls_stack`).
- **Rotation knob:** only the **ticket key**, not the X.509 chain.
- **Risk:** the only way to roll a cert is a restart. With
  `tests/reload_zero_drop.rs` being a pure ConfigManager unit test
  (no socket reuse), the actual cert roll is a **drop**.

### 4.2 Backend pool

- **Wire:** `addresses: Vec<SocketAddr>` is captured into
  `ListenerState` once and never mutated (`main.rs:686-700`).
- **DNS:** `DnsResolver::spawn_background_refresh` exists but is
  **not invoked** by the binary. So an A-record change is not picked up
  unless the process restarts.

### 4.3 Route table

There is no route table — routing is per-listener round-robin only.
Path/host-based routing is not implemented in `lb-l7`. So "route
reload" is N/A and the runbook line about route table updates is moot.

### 4.4 Mid-frame byte drop risk

- TCP: per-conn task holds the socket; abort severs it mid-`copy_bidirectional`.
  Any partial L7 frame on the wire at SIGTERM will be truncated. With
  H2, this means a stream RST_STREAM is **not sent** — the client sees
  a TCP reset.
- H2: same — H2 GOAWAY frame is **not sent** during drain. RFC 7540 §6.8
  requires GOAWAY for graceful shutdown.
- QUIC: 2 s drain may be too short for in-flight requests; quiche's
  CONNECTION_CLOSE may not flush.

**Verdict:** there is no rolling reload that preserves frames. Every
SIGTERM is a slam.

---

## 5. Observability surface

### 5.1 Prometheus families currently exposed

(via `/metrics` once `observability.metrics_bind` is set)

| Family | Type | Labels | Granularity |
|--------|------|--------|-------------|
| `pool_acquires_total` | Counter | — | per acquire |
| `pool_probe_failures_total` | Counter | — | per pool error |
| `pool_idle_gauge` | Gauge | — | sampled every 1 s |
| `dns_cache_entries` | Gauge | — | 1 s |
| `dns_cache_hits_total` | Counter | — | per resolve |
| `dns_cache_misses_total` | Counter | — | per resolve |
| `http_requests_total` | CounterVec | `version` × `status_class` (4 series) | per **connection** (misleading) |
| `http_request_duration_seconds` | HistogramVec | `version` | per connection |
| `connections_total` | Counter (legacy) | — | per accept |
| `bytes_client_to_backend`, `bytes_backend_to_client` | Counter | — | per conn close |

**No labels for `route`, `backend`, or `listener`.** RED-by-listener is
impossible. The cardinality is so low (≤4 series for the request
counter) that adding a `listener` label would be safe up to a few
hundred listeners.

### 5.2 Bucket coverage

`http_latency_buckets()` (`crates/lb-observability/src/lib.rs`) — let me
re-read; I have not yet verified the exact ladder. Open question for
`code` whether the buckets cover the documented SLO range. For now,
flagging as a follow-up.

### 5.3 Saturation signals

- fd usage: **not exposed**
- backlog queue depth: **not exposed**
- pool wait time: **not exposed**
- per-listener inflight: **not exposed** (no semaphore; can be inferred
  as `connections_total - finished_count`, but `finished_count` is also
  not emitted)
- accept-loop iteration rate: **not exposed**

### 5.4 Tracing

`grep -r 'traceparent\|opentelemetry\|tracing::info_span' crates/`
returns **zero hits**. There is no W3C trace-context propagation, no
span per request, no upstream-timing child span. `lb-l7` does not
inject or forward `traceparent`. This is a hard prerequisite for
production debuggability.

### 5.5 Log shape

- `tracing_subscriber::fmt()` defaults → **plain text** to stdout.
- `DEPLOYMENT.md:147` claims "JSON to stdout via
  `tracing_subscriber::fmt().with_env_filter(...)`" — false; no JSON
  feature flag is set, no `.json()` call site.
- Hot-path lines (per accept, per-request) at `info` would be O(qps);
  most are at `debug`. The `accept error` line at `main.rs:1103` is
  `warn` and **fires every iteration** when accept is failing → log
  spam under EMFILE.

### 5.6 PII posture

I found no redaction filter anywhere. `lb-l7` may log headers
verbatim — needs review by `sec`. No `tracing::field` redact wrapper
exists in `lb-observability`.

---

## 6. Backpressure & queue inventory

### 6.1 Bounded queues

| Queue | Capacity | Site | Full-policy |
|-------|----------|------|-------------|
| QUIC actor inbound (`mpsc<InboundPacket>`) | `ACTOR_CHANNEL_DEPTH = 32` | `crates/lb-quic/src/router.rs:44,343` | `try_send` → drop on full (verify; see review) |
| QUIC max connections per listener | `100_000` | `crates/lb-quic/src/listener.rs:245` | error string `"router at max_connections"`; client packet dropped (`router.rs:318`) |
| TCP pool per-peer idle | `per_peer_max=8` | `crates/lb-io/src/pool.rs:55` | evict oldest |
| TCP pool total idle | `total_max=256` | `pool.rs:57` | (verify enforcement on drop path) |
| H2 max-concurrent-streams | from `H2SecurityConfig` or default | `crates/lb-l7/src/h2_security.rs` | RST_STREAM |
| HTTP header / body / total timeout | from `[listeners.http]` block | `main.rs:473-477` | terminate request |
| WS max-message-size | from `[listeners.websocket]` | `main.rs:497` | close |
| WS ping-rate window | from same block | `main.rs:498-499` | close |
| TCP backlog | `50_000` listener sockopt | `main.rs:124` | kernel SYN drop |
| Tokio blocking-thread pool | tokio default = 512 | (none — uses defaults) | task waits indefinitely |

### 6.2 Unbounded paths (flag for round 2)

- **Per-listener inflight TCP connections** — `tokio::spawn` per accept
  with no semaphore (`main.rs:1126`). A connection storm spawns
  unbounded tasks; OOM-by-task before OOM-by-bytes.
- **DNS cache size** — `DashMap` with no eviction; an attacker who
  drives backends with rotating hostnames could grow this without
  bound. Today the backend list is config-fixed so the risk is dormant
  but the design lacks a cap.
- **`tokio::task::spawn_blocking` for dial** — uses the default
  512-slot blocking pool. Under a backend stall, all 512 blocking
  workers can park inside `connect_timeout` (default 75 s). Anything
  using blocking IO (cert load, file logs) is starved.
- **Admin HTTP per-conn spawn** — `admin_http.rs:120` spawns one task
  per scrape; trivially DoSable if `metrics_bind` is exposed beyond
  loopback.

### 6.3 Documented full-policy: **none**

There is no document anywhere that describes "what happens when X is
full". This must land in the round-2 RUNBOOK refresh.

---

## 7. Deployment & container

### 7.1 Image

`docker/Dockerfile`:
- ✅ Distroless final stage (`gcr.io/distroless/cc-debian12:nonroot`)
- ✅ Multi-stage with cargo-chef caching
- ✅ Stripped binary
- ✅ Runs as `nonroot` (UID 65532) — implicit from the `:nonroot` tag
- ❌ **No `USER` line is explicit** — relies on the base image's user; OK but worth pinning
- ❌ **No `--read-only` rootfs hint** anywhere in the repo's k8s manifests / compose
- ❌ **Capabilities** not dropped in the Dockerfile; container relies on the runtime's caps. For a non-XDP deployment the binary needs no caps; the Dockerfile does not drop them.
- ❌ Binary name in `RUNBOOK.md` (`lb`) does not match the image (`expressgateway`)
- ❌ `EXPOSE 80 443 8080 9090` — but binding to <1024 inside a distroless `:nonroot` container requires either `CAP_NET_BIND_SERVICE` granted at runtime, or `net.ipv4.ip_unprivileged_port_start=80` sysctl. Neither is mentioned.

### 7.2 Systemd unit (DEPLOYMENT.md)

- The unit references `ExecStart=/usr/local/bin/lb`. Binary is `expressgateway`. Mismatch.
- Hardening (`ProtectSystem=strict`, `ReadOnlyPaths=/`, `MemoryDenyWriteExecute=true`) is well-chosen.
- `LimitNOFILE=1048576`, `LimitMEMLOCK=infinity` — fine.
- **No `RestrictNamespaces=true`**, no `SystemCallFilter=`. Round-2 cross-ref with `sec`.

### 7.3 Hot-reload binary swap (`SO_REUSEPORT` + FD passing)

- `SO_REUSEPORT=true` is set on every listener (`main.rs:118`).
- **No FD-passing code.** `grep -r 'sd_listen_fds\|systemd::listen_fds\|sendmsg.*SCM_RIGHTS' crates/` returns nothing.
- The README claim is false. The closest thing is "start new binary on the same `SO_REUSEPORT` group, then kill old one" — this works for plain TCP and is acknowledged in `RUNBOOK.md:29`, but is **not zero-drop for in-flight L7** because the old binary still slams its sockets at SIGTERM (see §2.2).

---

## 8. RUNBOOK.md gap analysis

### 8.1 Defined in code but undocumented

- **`/metrics` endpoint and its bind option** — RUNBOOK does not show a scrape config; only METRICS.md does, and only loosely.
- **`/healthz` endpoint** — RUNBOOK does not mention it at all.
- **`POOL` config knobs** — `idle_timeout`, `max_age`, `per_peer_max`, `total_max` (`pool.rs:55-61`) — not in `CONFIG.md` either (verify with `code`).
- **`DnsResolver` background refresh** — defined; not invoked; not documented.
- **H2 security thresholds** — `H2SecurityConfig` is a large struct (`main.rs:569-605`); CONFIG.md coverage thin.
- **QUIC retry secret** persistence path — `[listeners.quic].retry_secret_path`; mentioned in CONFIG presumably, not in RUNBOOK incident response.
- **Ticket rotator interval/overlap** — knob exists; runbook describes the workaround for an emergency rotate but not the *normal* operation pattern.

### 8.2 Documented in RUNBOOK but absent from code

(See §0 headline table.) Re-listed compactly:

- SIGHUP reload — fictional.
- `ArcSwap<TlsStore>` cert rotation — fictional.
- `KillMode=mixed` graceful 30 s drain — actual drain is ~2.5 s and severs L7 frames.
- `Connection: close` on H2 streams during drain — fictional (the comment in `RUNBOOK.md:27` admits this is Pillar 3b, but the surrounding text reads as if it's a working flow).
- Log lines `reload applied` (DEPLOYMENT.md:157) — string does not appear in source.
- Log line `pool probe discarded` (RUNBOOK.md:146) — string does not appear in source.
- Log line `liveness probe` filter (RUNBOOK.md:86) — string does not appear in source.

### 8.3 Alerts without runbooks (TBD round 2)

Once round 2 introduces per-listener/per-backend alert rules
(saturation, error rate, drain duration), every one needs a response
path. Today RUNBOOK has zero alert→action mappings.

---

## 9. Open questions for other teammates

To `code` (CODE-1-XX):
1. What is `http_latency_buckets()`? I haven't verified the ladder; please confirm it covers 100 µs → 30 s with reasonable bucket density.
2. Is the `accept_error` path (`main.rs:1100-1106`) handled anywhere with backoff that I missed? If not, this is your finding.
3. `set_nonblocking` on a pre-bound listener (`main.rs:1087`) — is this actually required given `lb_io::Runtime::listen` already returns a blocking std listener?
4. `unwrap_or_else(|_| fallback_500())` in `admin_http.rs:53` — the comment says "unreachable at runtime"; under what circumstances does `Response::builder().body(...)` fail? Header injection through a non-static path?

To `sec` (SEC-1-XX):
1. Is `/healthz` safe to keep on the metrics listener (no auth)? Likely yes (loopback) but please confirm.
2. PII review: does `lb-l7` log request headers verbatim under `RUST_LOG=debug`?
3. TLS ticket key on disk? — `TicketRotator::new` (`ticket.rs`); does it persist or is it ephemeral per process? If ephemeral, a restart invalidates every session ticket. If persisted, the rotation overlap is undermined unless we coordinate across replicas.
4. QUIC retry-secret on disk (`load_or_generate_retry_secret`, `listener.rs:291`) — readable as 0600? Confirm.

To `ebpf` (EBPF-1-XX):
1. When XDP is wired (Pillar 4b), what is the userspace API to read `STATS`, `CONNTRACK`, `ACL_DENY` maps? I will need it for the saturation panel.
2. What is the cleanup path when the binary panics mid-attach? `RUNBOOK.md:117` says `sudo ip link set dev <iface> xdp off`; can we make the loader install a hook that does this on `Drop`?
3. Verifier log: is the `bpftool prog load` invocation in RUNBOOK reproducible against the bundled `lb_xdp.bin`? Useful for the round-3 panic playbook.

To `proto` (PROTO-1-XX):
1. Does any L7 path emit GOAWAY (H2) or CONNECTION_CLOSE (H3) on SIGTERM today? My read says no. Confirm.
2. Are H2 settings-flood / continuation-flood / RST-flood / HPACK-bomb detectors currently *firing* in production code, or just defined as types? METRICS.md table says "detector exists but not invoked" — I want one finding ID owning the wiring.
3. `lb-quic`: under `ACTOR_CHANNEL_DEPTH=32`, what happens when the per-conn actor's inbox is full? `try_send` → drop, or `send().await` → backpressure?
4. H1 → H2/H3 bridging: do upstream timing spans propagate across protocol transitions? (Today: no, because no tracing.)

To team-lead:
- I want one round-2 finding ID dedicated to the *RUNBOOK truthfulness*
  effort: every fictional flow either gets implemented or struck from
  the doc. Suggest `REL-2-01` "Doc/code drift in RUNBOOK & DEPLOYMENT".

---

## 10. Round-1 prioritisation hints (input to round 2)

Severity hints for round-2 findings I will own:

- **Critical**: F-17 (unbounded TCP spawn — DoS surface), F-11 (accept-EMFILE tight loop), H7 (SIGTERM not graceful, frames severed), F-13 (no DNS re-resolve on the live path).
- **High**: H1/H2 (SIGHUP & cert rotation fictional), H4 (`/healthz` lies), H8 (HealthChecker phantom), F-02 (no connect timeout bound), F-05 (no TLS handshake timeout), no tracing (§5.4).
- **Medium**: H10 (logs not JSON), H6 (binary name mismatch), no readiness/startup split, log spam under EMFILE, sampler not cancelled on shutdown.
- **Low**: image hardening polish, `/healthz` body shape, runbook log-line strings.
