# S42 Corrected-Claims & Reachability Audit (CODE-AUDITOR)

**Scope:** verify that every *headline capability* in `README.md`,
`docs/guide/overview.md`, and `docs/arch/overview.md` is **reachable** by an
operator (config-exposed, env-exposed, documented flag, or unconditionally
active in the live request path) — not merely *present somewhere in the code*.

**Method:** read the actual code, not the docs. "Reachable" = an operator can
invoke/benefit from it. A function that exists but is never called from the live
binary, is gated behind `#[cfg(test)]`, or has no config key to enable it, is
**NOT reachable**.

**Verdict in one line:** the two flagged overclaims are **both confirmed**, plus
**"Standalone and HA modes" is unbacked** in the running binary, plus one more
knob (`header_underscore_policy` drop/allow override) is **wired in the library
but not connected from the binary**. Everything else audited is reachable as
described, and the absence of compression is correct.

---

## 1. CORRECTED CLAIMS

| # | Claim as currently written | Verified reality | Code evidence (file:line) | Corrected wording to use |
|---|---|---|---|---|
| A | **"11 load-balancing algorithms"** … "round-robin, weighted round-robin, random, weighted random, P2C, Maglev, ring hash, EWMA, least connections, least requests, session affinity" — headlined as a marquee capability (README:27-29, guide/overview.md:53, arch/overview.md:84-93). guide/overview.md:56-60 adds "All 11 algorithms are implemented **and selectable**". | **11 are implemented; 0 are operator-selectable.** There is **no config key** to choose an algorithm — `LbPolicy` (the enum naming all 11) is referenced **nowhere outside `lb-core`** (not in `lb-config`, not in the binary). The live paths hard-wire the algorithm: **round-robin** for L7 HTTP and raw-TCP, **Maglev** (consistent hash over QUIC Connection ID) for QUIC Mode A passthrough only. The other 9 are library code with unit tests, unreachable from any live path. `deny_unknown_fields` means an operator literally *cannot* add a `policy=` key (config would be rejected). EWMA's latency input is fed only in a `#[cfg(test)]` test. | 11 algo files `crates/lb-balancer/src/{round_robin,weighted_round_robin,random,weighted_random,least_connections,least_request,p2c,maglev,ring_hash,ewma,session_affinity}.rs`; enum `crates/lb-core/src/policy.rs:7-30`; **`LbPolicy` unused outside lb-core** (grep empty in `crates/lb-config/`); no policy field in `ListenerConfig` (`crates/lb-config/src/lib.rs:619-710`) or `BackendConfig` (`:1250-1264`); `#[serde(deny_unknown_fields)]` (`:50,98,…`); L7 RR `crates/lb-l7/src/upstream.rs:145-177` built at `crates/lb/src/main.rs:1367`; TCP RR `crates/lb/src/main.rs:768,2031,3687-3691`; Maglev `crates/lb-quic/src/passthrough.rs:49,542,1165`; EWMA feed test-only `crates/lb-core/src/lib.rs:79` (inside `#[cfg(test)]` `:33-34`), setter `crates/lb-core/src/backend.rs:162` | "**Eleven backend-selection algorithms are implemented** in the `lb-balancer` library (round-robin, weighted RR, random, weighted random, P2C, Maglev, ring hash, EWMA, least-connections, least-requests, session affinity). In this build the **live data path uses round-robin** (L7 HTTP and raw-TCP listeners) and **Maglev consistent-hashing by QUIC Connection ID** (Mode A passthrough). The remaining algorithms are **not yet operator-selectable** — there is no config key to choose a policy — and EWMA's per-request latency input is not yet fed by the request path." |
| B | **"Active and passive health checking"** (README:30) / "active/passive health checks" (guide/overview.md:53). arch/overview.md:112-113 goes further: "lb-health — **active health checks** (rise/fall thresholds **+ a TTL result cache**)". | **Neither active nor passive health checking affects live traffic.** `HealthChecker` is a purely *passive* consecutive-pass/fail state machine — **no prober, no interval, no probe path, no expected-status, no background task, and no TTL cache.** Its `record_success`/`record_failure` are called **only** in lb-health's own `#[cfg(test)]` — **never** by live traffic. The binary's only use is a startup "seed" whose own comment says *"today nothing reads these … proves the lb-health dep is reachable … active probe loop is Wave-2"*; the seed is bound to `_health_seed` and never consulted by the balancer. So passive tracking is **inert** (never fed, never read) and active probing **does not exist**. | `crates/lb-health/src/lib.rs` (whole file): `record_success:91`, `record_failure:100`, `status:110`; no interval/path/probe code; call sites only `#[cfg(test)]` `:128-172`; binary seed + comment `crates/lb/src/main.rs:2552-2582` ("today nothing reads these", "active probe loop is Wave-2 (REL-2-05)", `let _health_seed` `:2582`); balancer pick has no health filter `crates/lb/src/main.rs:3687-3691`; no health config block (grep empty) | Drop "Active and passive health checking" from the marquee. Honest: "A passive health-status state machine (`HealthChecker`, rise/fall thresholds) is implemented but is **not yet wired into backend selection** in this build — it is seeded at startup but neither fed by live traffic nor consulted by the balancer. **Active probing is not implemented (deferred).**" Also fix arch/overview.md:112-113 — there is **no** active check and **no** TTL result cache. |
| C | **"Standalone and HA modes"** (README:37). | **"HA modes" is unbacked in the running binary.** The only backing is a library `HaPoller` (config-polling from a shared mount, ADR-0008) that is used **only in lb-controlplane's own `#[cfg(test)]`** and is **never imported by the binary** (the binary uses only `ConfigManager`/`FileBackend`). There is **no HA config key, no flag, no clustering / failover / VRRP / state-sharing / active-passive**. `SO_REUSEPORT` (README:41) is side-by-side multi-process and explicitly **not** HA ("the binary does not itself transfer FDs"). | `HaPoller` def `crates/lb-controlplane/src/lib.rs:276-319`, used only at `:454,463` under `#[cfg(test)]` `:321`; binary imports only `ConfigManager, FileBackend` `crates/lb/src/main.rs:50`; no `HaPoller` anywhere in `crates/lb/`; SO_REUSEPORT note README:41-43 | Drop "Standalone and HA modes". Honest (if anything is said at all): "Runs **standalone**. A file-backed control-plane trait and an (unwired) `HaPoller` config-polling primitive exist as a **seam** for future multi-instance coordination, but **no HA / clustering / failover mode is wired into the binary** in this build." |
| D | `header_underscore_policy` (reject/drop/allow) — implied operator-tunable (`[runtime].header_underscore_policy`; CONFIG/default.toml). | **Only the default (`reject`) is active; the `drop`/`allow` override is inert.** The config field, validation, reload-diff, and the proxy-side enforcement + setter all exist, but `build_h1_proxy`/`build_h2_proxy` **never call `with_header_underscore_policy`** — the binary never connects config → proxy, so the proxy always uses its compile-time default `Reject`. Setting `="drop"` or `="allow"` parses/validates but has **no effect**. The "test" that looks like coverage is a **source-grep drift check** (`src.contains("with_header_underscore_policy")`), not a wiring test. | config field `crates/lb-config/src/lib.rs:292`; proxy setter `crates/lb-l7/src/h1_proxy.rs:507`, `crates/lb-l7/src/h2_proxy.rs:630`; enforcement `crates/lb-l7/src/h1_proxy.rs:974`, `crates/lb-l7/src/h2_proxy.rs:1099-1135`; **no production call site** — only doc-comment `crates/lb-config/src/lib.rs:289` + source-grep test `crates/lb-l7/tests/round8_underscore_policy.rs:131,147`; `build_h1_proxy` wires hooks/watchdog/h2/h3/ws but **not** the policy `crates/lb/src/main.rs:1385-1401` | Either **wire it** (one line: `proxy = proxy.with_header_underscore_policy(map(cfg))` in `build_h1_proxy`/`build_h2_proxy`), or document honestly: "header names with underscores are **always rejected** (Envoy-edge default); the `drop`/`allow` settings are **not wired in this build**." |

---

## 2. REACHABILITY RE-AUDIT (every headline / notable capability)

| Capability | Reachable? | How it's reached (or why not) | Evidence (file:line) |
|---|---|---|---|
| **9-cell front×back HTTP matrix** | **Yes** | Front protocol = listener `protocol` (`h1`/`h1s`(→H2 via ALPN)/`quic`(→H3)); back protocol = `BackendConfig.protocol` (`tcp`/`h1`→H1, `h2`→H2, `h3`→H3). All 9 bridges dispatched. | bridge files `crates/lb-l7/src/h{1,2,3}_to_h{1,2,3}.rs`; dispatch `crates/lb-l7/src/h1_proxy.rs:1165-1166`, `crates/lb-l7/src/h2_proxy.rs:1328-1329`; proto map `crates/lb/src/main.rs:1087-1089`; listener tokens `crates/lb-config/src/lib.rs:622-647` |
| **L4 / XDP data plane** | **Yes (off by default)** | `[runtime].xdp_enabled=true` + `xdp_interface`; loader attaches in-tree ELF. | `crates/lb-config/src/lib.rs:174`, validation `:1378-1386`, attach `crates/lb/src/main.rs:2592-2593` |
| **QUIC Mode A passthrough** | **Yes** | `protocol="quic"` + passthrough listener; routes by CID via Maglev, no decrypt. | `crates/lb-quic/src/passthrough.rs:49,542`; `quic-passthrough-only` feature `crates/lb/Cargo.toml:106` |
| **QUIC Mode B terminate** | **Yes** | `protocol="quic"` (default `quic-terminate` feature); re-originates upstream QUIC. | `crates/lb-quic/src/raw_proxy.rs`; feature default `crates/lb/Cargo.toml` `default=["quic-terminate"]` |
| **gRPC (H2/H3 front)** | **Yes** | `[listeners.grpc]` on `h1s` (H2 via ALPN) or `quic` (H3); H1 front cannot deliver trailers (documented). | `crates/lb-config/src/lib.rs:677-684`; grpc proxy `crates/lb-l7/src/grpc_proxy.rs` |
| **WebSocket H1** | **Yes (on by default)** | `[listeners.websocket]` on `h1`/`h1s`. | `crates/lb-config/src/lib.rs:670-676`; `crates/lb/src/main.rs:1398-1399` |
| **WebSocket H2** | **Gated off** (as documented) | RFC 8441 ships off (CF-S27-2). | known-limitations; README:20-22 |
| **WebSocket H3** | **Yes (opt-in)** | RFC 9220 tunnel. | `crates/lb-quic/src/ws_tunnel.rs` |
| **TLS (1.2+1.3, `tls13_only`)** | **Yes** | `[listeners.tls]`; `tls13_only` opt-in. | `crates/lb-config/src/lib.rs:532` |
| **alt_svc / advertise H3 from H1/H1s** | **Yes** | `[listeners.alt_svc]` (`h3_port`,`max_age`) → `Alt-Svc: h3=":<port>"; ma=<age>` header injected on H1 responses. | config `crates/lb-config/src/lib.rs:656-659,906-914`; header build + inject `crates/lb-l7/src/h1_proxy.rs:190-202,2543` |
| **per_ip_connection_cap** | **Yes** | `[runtime].per_ip_connection_cap`; enforced via ConnGate `admit_connection`. | config `crates/lb-config/src/lib.rs:259`, validation `:1455-1458`, gate `crates/lb/src/main.rs:3626` |
| **max_inflight_connections** | **Yes** | `[runtime].max_inflight_connections`; per-listener inflight shed. | config `crates/lb-config/src/lib.rs:242-243`, validation `:1438-1441`, shed `crates/lb/src/main.rs:~3668-3681` |
| **[runtime.watchdog] slowloris / slow-POST** | **Yes** (with nuance) | `Watchdog::new` from config; `with_watchdog` on every H1/H2 proxy; per-request `register`. **Nuance:** the *background sweeper is observability-only* (F-RES-5: alerts, does not close sockets); per-request header/body deadlines are enforced via `HttpTimeouts`. Also `header_deadline_ms` config field is **not consumed** (header deadline comes from `HttpTimeouts.header`). | config `crates/lb-config/src/lib.rs:457-489`; instantiate `crates/lb/src/main.rs:2766-2776`; wire `:1390,1552`; register `crates/lb-l7/src/h1_proxy.rs:1096-1115`; sweeper "OBSERVABILITY-only" comment `crates/lb/src/main.rs:~2795+` |
| **header_underscore_policy** | **Partial** | Default `reject` active; **`drop`/`allow` not wired** (see Corrected Claim D). | `crates/lb/src/main.rs:1385-1401` (setter never called) |
| **LB_LOG_FORMAT (json/text/plain)** | **Yes** | Env var read by `init_tracing`, default `json`. | `crates/lb-observability/src/log.rs:4,32-40`; `crates/lb-observability/src/lib.rs:60` |
| **quic-passthrough-only cargo feature** | **Yes** | Build-time feature; default is `quic-terminate`. | `crates/lb/Cargo.toml:106`, `crates/lb-quic/Cargo.toml:112` |
| **SIGHUP reload / SIGUSR1 cert rotate / SIGTERM drain** | **Yes** | Wired in binary (ConfigManager reload, TicketRotator, drain token). | `crates/lb/src/main.rs:50,2481-2484,2772`; reload diff `crates/lb-config/src/reload.rs` |
| **Load-balancing algorithm selection** | **No** | No config key; `LbPolicy` unused outside lb-core; `deny_unknown_fields` rejects any `policy=` key. | See Corrected Claim A |
| **Session affinity / sticky sessions** | **No** | Implemented (`session_affinity.rs`) but not selectable (same as the other 9 non-live algos); no affinity-key config. | `crates/lb-balancer/src/session_affinity.rs`; no config key |
| **EWMA latency weighting** | **No (degrades to load)** | Latency setter is `#[cfg(test)]`-only; never fed in prod (and algo not selectable anyway). | `crates/lb-core/src/lib.rs:79`; setter `crates/lb-core/src/backend.rs:162` |
| **Backend `weight`** | **No (inert)** | Config field parsed but consumed by **no reachable picker** (RR + Maglev-by-CID ignore weight; weighted algos not selectable). | field `crates/lb-config/src/lib.rs:1264`; RR ignores weight `crates/lb-balancer/src/round_robin.rs`, `crates/lb-l7/src/upstream.rs:145-177` |
| **Active health probing** | **No** | Not implemented; deferred to Wave-2/REL-2-05. | See Corrected Claim B |
| **Passive health tracking (live)** | **No (inert)** | Type exists, never fed/read in live path. | See Corrected Claim B |
| **HA / clustering / failover mode** | **No** | `HaPoller` is test-only library code, never imported by binary. | See Corrected Claim C |
| **Response compression** | **N/A — correctly absent** | No gzip/brotli/zstd/deflate/flate2 of HTTP responses. Grep hits are HPACK static-table *tokens*, the gRPC frame `compressed` *flag* (passthrough), and smuggling-detector test fixtures — none is a compression codec. Docs correctly omit it. | `crates/lb-h2/src/hpack.rs:52,72` (tokens); `crates/lb-grpc/src/frame.rs:22` (flag passthrough); ADR-0007 (decision: not implemented) |

---

## 3. HEADLINE-PITCH IMPACT

Three corrections change what can sit in the **marquee** (README bullets +
guide/overview headline table). The honest restatement should:

1. **Demote "11 load-balancing algorithms."** Keep "11 implemented" as a
   *library* fact, but the **headline must not imply operator choice**. The
   live, reachable behavior is **round-robin (L7 + TCP) + Maglev-by-CID (QUIC
   passthrough)**. Pitch it as *"consistent-hash (Maglev) and round-robin load
   balancing, with eight more algorithms implemented in the library pending a
   selection knob,"* not *"11 selectable algorithms."*

2. **Drop "Active and passive health checking" from the marquee.** In this build
   **neither affects traffic** — there is no active prober and the passive
   checker is inert. At most say *"passive health-status state machine
   implemented (not yet wired); active probing deferred."* This also requires
   fixing `arch/overview.md:112-113` ("active health checks + a TTL result
   cache" — both false).

3. **Drop "Standalone and HA modes."** There is **no HA mode** in the binary.
   Say *"standalone; HA is a future seam (unwired `HaPoller`)."*

Net effect: the four genuinely strong, fully-reachable pillars to lead with are
**(1) the 9-cell HTTP matrix with bounded streaming, (2) QUIC passthrough +
terminate, (3) the hand-audited DoS-mitigation catalog, (4) panic-free
libraries.** The load-balancing, health-checking, and HA lines should move from
"capability" framing to "implemented / partial / deferred" framing.

**Secondary (not marquee, but should be corrected where mentioned):**
`header_underscore_policy` advertises `reject/drop/allow` but only `reject`
(the default) is wired — either wire the override (one line in `build_h1_proxy`/
`build_h2_proxy`) or document that drop/allow are not active. The
`[runtime.watchdog]` background sweeper is observability-only (does not evict),
and its `header_deadline_ms` field is unused — fine to ship, but RUNBOOK/CONFIG
prose should not imply the sweeper closes slow connections.

---

### Audit confidence

Every row above was derived by reading source, not docs. The two flagged
overclaims (A, B) and the HA claim (C) are **high-confidence** (the binary
demonstrably never reaches the capability: `LbPolicy` unused outside lb-core;
`record_success`/`record_failure` and `HaPoller` only under `#[cfg(test)]`; the
`_health_seed` comment is self-documenting). Claim D is high-confidence (the
setter has zero production call sites; the only "test" is a source-grep). The
watchdog `header_deadline_ms`-unused and `weight`-inert observations are
medium-confidence supporting details, not headline issues.
