# SEC — Round 1 Inventory

Owner: `sec` (security-reviewer). Round 1 / DISCOVERY. Branch: `main`.

This file enumerates the security-relevant surface of ExpressGateway as it
exists in the repo today. **No source modifications**. References are
`crate/path/file.rs:line` (or git path).

---

## 1. External attack surface map

### 1.1 Production listener binds (attacker-reachable)

Every accepted byte on these listeners is attacker-controlled.

| Listener | File | Line | Default scope | Notes |
|---|---|---|---|---|
| TCP / TLS-over-TCP / H1 / H1s (ALPN h1+h2) | `crates/lb/src/main.rs` | 1077-1090 (`run_listener`); 1084 `io_runtime.listen(parsed, …)` | `config.listeners[i].address` — `config/default.toml` ships `0.0.0.0:8080`, public | Listener picks `ListenerMode` from listener.protocol; backlog 50_000 (`listener_opts()` at 114). |
| QUIC / H3 | `crates/lb-quic/src/listener.rs:181` (`UdpSocket::bind(params.bind_addr)`) | listener.rs:169-205 | `[listeners.quic]` block | quiche 0.28 + tokio-quiche 0.18. Retry signer + 0-RTT replay guard installed (lib.rs:331-360, 347-358). |
| QUIC second variant | `crates/lb-quic/src/lib.rs:291, 314` (`QuicEndpoint::bind`) | lib.rs:255-360 | `bind_addr` arg | Older/parallel listener API; ensure only `QuicListener` path is wired in `crates/lb/src/main.rs`. |
| Admin HTTP (`/metrics`, `/healthz`) | `crates/lb-observability/src/admin_http.rs:97` (`TcpListener::bind(addr)`) | admin_http.rs:80-134 | `observability.metrics_bind` (optional) | **No TLS, no auth, no IP allowlist** — comment line 3 says "loopback is operator responsibility". If operator binds to `0.0.0.0`, /metrics is world-readable cardinality leak. |

There is **no control-plane network listener**: `crates/lb-controlplane` and
`crates/lb-cp-client` are in-process file/SIGHUP backends only. No mTLS, no
gRPC management plane. Confirmed by grep over `lb-controlplane`/`lb-cp-client`
for `TcpListener` / `UdpSocket::bind` — zero hits.

### 1.2 Untrusted-input parsers / decoders

Every entry below consumes attacker-controlled bytes.

| Parser | File:line | Wire role |
|---|---|---|
| HTTP/1.1 request/status line | `crates/lb-h1/src/parse.rs:41,68` | Internal codec; **not on the live hyper path** (hyper owns the wire today). |
| HTTP/1.1 headers + trailers | `crates/lb-h1/src/parse.rs:96-205` | Same as above. `MAX_HEADER_BYTES=65_536`. |
| HTTP/1.1 chunked transfer | `crates/lb-h1/src/chunked.rs:22-180` | Same. |
| HTTP/2 frame header | `crates/lb-h2/src/frame.rs:152` | Codec only — hyper's `h2` 0.4.13 owns the wire. |
| HPACK | `crates/lb-h2/src/hpack.rs:391` (`HpackDecoder`) | Codec only — see above. |
| HTTP/3 frame + varint | `crates/lb-h3/src/{frame.rs, varint.rs}` | Internal codec; tokio-quiche owns the wire. |
| QPACK | `crates/lb-h3/src/qpack.rs` | Internal codec. |
| TOML config | `crates/lb-config/src/lib.rs` (`parse_config`, `validate_config`) | **Trusted input** — operator-supplied. Still parseable from `argv[1]` (main.rs:941-948). |
| Retry token | `crates/lb-security/src/retry.rs:225-…` (`RetryTokenSigner::verify`) | Attacker echoes server-minted token over QUIC; constant-time MAC via `subtle::ConstantTimeEq`. |
| 0-RTT token (replay guard) | `crates/lb-security/src/zero_rtt.rs:125-143` | Fixed-cap HashSet; collision-resistant via process-local HMAC-SHA256. |
| TLS-cert / private-key PEM | `crates/lb/src/main.rs:187-211` (`load_cert_chain`, `load_private_key`) | Operator path, not attacker — but mis-permission means key leak. |
| Compression decode | `crates/lb-compression/src/lib.rs:226-310` (`Decompressor`) | gzip/deflate/brotli/zstd; bomb guard with ratio + absolute cap. Wiring into proxies — see open question Q4. |
| WebSocket frames | `crates/lb-l7/src/ws_proxy.rs` via tokio-tungstenite 0.24 | `max_message_size = 16 MiB` (line 115). Tungstenite owns codec. |
| eBPF packet parser (XDP) | `crates/lb-l4-xdp/ebpf/src/main.rs:337-712` | Hot path on every wire packet; ptr_at bounds. |

### 1.3 Untrusted → trusted-context crossings

* **Inbound HTTP headers → upstream HTTP headers.** `H1Proxy::handle` and `H2Proxy::handle` strip hop-by-hop (`crates/lb-l7/src/h1_proxy.rs:53-64` static list of 8) and append `X-Forwarded-{For,Proto,Host}` + `Via`. **`SmuggleDetector` is NOT called on this path** — see Finding S-1.
* **eBPF map updates from userspace.** `XdpLoader::insert_acl_deny`, `take_map`, `conntrack_map` (loader.rs:225-…). The maps (`CONNTRACK` 1_000_000, `CONNTRACK_V6` 512_000, `L7_PORTS` 256, `ACL_DENY_TRIE` 100_000) are plain `HashMap`, **not `LRUHashMap`** — eviction is implicit-on-full, not LRU. See Finding S-2.
* **QUIC retry token round-trip.** Token bytes are attacker-controlled; signer parses length-prefixed ODCID + raw peer-addr octets (retry.rs wire format docstring). Truncation, version, peer-kind validation all present.
* **0-RTT replay guard token bytes.** Attacker-controlled but hashed under per-instance HMAC key before storage; cannot precompute collisions.
* **Listener-config strings → kernel.** `address` is parsed to `SocketAddr` (main.rs:1078). Untrusted only if config file itself is. Treated as trusted.

---

## 2. Threat model

### 2.1 Adversary capabilities

* **Network-on-the-wire (default).** Can send arbitrary TCP segments, UDP datagrams, and TLS-encrypted HTTP/1.1, HTTP/2, HTTP/3, gRPC, WebSocket payloads to the public listeners. Owns source IP/port across flows; can spray, slowdrip, fragment, malform.
* **Breached backend (escalation).** If a backend is compromised, the attacker controls response bytes (status line, headers, body, trailers, compression). H1/H2/H3 hop-by-hop scrubbing + the response-strip in `h{1,2}_proxy.rs` are the only defense.
* **Local but unprivileged (lateral).** Can read `config/default.toml` if world-readable, can inspect `/proc/<pid>` if same UID. Cannot reach admin HTTP unless it's bound to `0.0.0.0` (default config has *no* `[observability]` block — admin listener is off by default).
* **Operator misconfiguration (high frequency).** Binds admin listener publicly; ships TLS keys mode 0o644; sets very large `H2SecurityThresholds`; runs without `lb-security` detectors wired into hot path.

### 2.2 Plausible attacker wins given current code

| Attack | Plausibility | Mitigation status |
|---|---|---|
| **Request smuggling (CL/TE, dup-CL, TE-CL, H2-downgrade)** | High — `SmuggleDetector` is dead code on the proxy hot path | Mitigation **defined but not wired** (Finding S-1). Hyper rejects some CL+TE but not all variants and not duplicate same-value-CL → upstream-divergence smuggling. |
| **Slowloris / slow POST** | Medium — only hyper's H1 `header` timeout (10 s in `HttpTimeouts::default`) applies. `SlowlorisDetector` / `SlowPostDetector` are not wired (Finding S-3). | Hyper's body timeout fires at 30 s. No per-IP cap → attacker holds N connections simultaneously. |
| **fd / port / memory exhaustion via concurrent connections** | High — listener backlog 50_000, no per-IP / per-listener cap, no semaphore on accept loop | **No mitigation in code** (Finding S-4). |
| **H2 Rapid Reset / CONTINUATION flood / HPACK bomb / SETTINGS&PING flood / window-stall** | Low — hyper enforces via `H2SecurityThresholds` (`crates/lb-l7/src/h2_security.rs`); `max_concurrent_streams=256`, header list 64 KiB. | Mitigated via hyper config; **per-instance detector types in lb-h2 are statistics-only, not enforcement.** |
| **QUIC amplification + 0-RTT replay** | Low — RetryTokenSigner + ZeroRttReplayGuard wired in `QuicListener::spawn`. | Replay window collapses under unique-token spray (Finding S-5). Retry signer secret persisted to disk mode 0o600 (good). |
| **TLS downgrade / SNI injection / ALPN mismatch** | Low — `tokio-rustls 0.26` + `rustls 0.23.38` with `ring` provider, `tls12` allowed, default ALPN: `h1`-only listener offers no ALPN; `h1s` offers `h2`, `http/1.1`. | No mTLS option visible; cert pinning N/A for a reverse proxy. Session-ticket rotation via `TicketRotator` (`crates/lb-security/src/ticket.rs`). |
| **XDP map exhaustion (conntrack spray)** | Medium — 1M v4 entries, 512K v6, **non-LRU `HashMap`**, eviction = userspace + map-full insert failures | (Finding S-2). |
| **Compression bomb on response** | Low — `BombDetected` ratio + `OutputTooLarge` cap implemented (`compression/lib.rs:36-52, 275-310`). | Wiring into proxies — depends on call-site `max_bytes`; needs cross-check with `proto` teammate. |
| **BREACH on response with reflected input** | Medium — `BreachGuard` exists; depends on call-site. Open question Q4. |
| **Admin HTTP unauthenticated access** | Medium — depends on operator. Recommend documenting deny-by-default bind (Finding S-6). |
| **Memory unsafety in `unsafe` blocks** | Low — inventory below, mostly XDP packet parsing with verifier-checked bounds + libc syscall wrappers. |
| **Supply-chain compromise** | Low-Medium — `deny.toml` ignores 10 advisories (instant, protobuf, rand, rustls-pemfile, dashmap×2, rand-2, rustls-pemfile-2, time, yaml-rust). `cargo-audit` and `cargo-deny` are not installed in this dev environment (Finding S-7). |

---

## 3. Unsafe inventory

Counts by file (all `crates/` matches for `unsafe ` keyword, 71 sites total).

### 3.1 `crates/lb-l4-xdp/ebpf/src/main.rs` (eBPF, kernel-context)

| Line | Construct | One-liner | Verdict |
|---|---|---|---|
| 234-244 | `unsafe fn ptr_at<T>` | Bounds-checked pointer-into-packet; verifier-friendly. | **JUSTIFIED** — but see note: `start + offset + len` is a `usize` add that could in theory wrap on 32-bit, though BPF is effectively 64-bit. Consider `checked_add` for defense-in-depth. |
| 246-256 | `unsafe fn ptr_at_mut<T>` | Same shape as `ptr_at`. | **JUSTIFIED** with same caveat. |
| 261-264 | `unsafe { *slot = (*slot).wrapping_add(1) }` in `incr_stat` | Per-CPU array write. Returned ptr non-null. | **JUSTIFIED**. |
| 337-339, 345-347, 372, 378-380, 397, 400, 406, 408, 411, 422, 438, 463, 465, 471, 473, 481, 491, 493, 514, 516, 570, 573-575, 585, 587, 606, 608, 611, 617, 619, 622, 633, 646, 670, 672, 678, 680, 688, 690, 709, 711 | `unsafe { core::ptr::read_unaligned / write_unaligned / addr_of! / map.get / ptr_at* }` | Packed-struct field reads/writes on previously-validated pointers, plus aya map accesses. | **JUSTIFIED** — but volume warrants a deeper-review pass by `ebpf` teammate; flag aliasing between concurrent `ptr_at_mut` reads on same offset. |

### 3.2 `crates/lb-l4-xdp/src/loader.rs` (userspace)

| Line | Construct | Verdict |
|---|---|---|
| 53 | `unsafe impl Pod for FlowKey` | Marker for aya; `#[repr(C)] + Copy + 'static`. | **JUSTIFIED**. |
| 76 | `unsafe impl Pod for BackendEntry` | Same. | **JUSTIFIED**. |
| 97 | `unsafe impl Pod for FlowKeyV6` | Same. | **JUSTIFIED**. |
| 120 | `unsafe impl Pod for BackendEntryV6` | Same. | **JUSTIFIED — but** confirm no padding bytes leak (cf. `_pad` fields manually zeroed before map insert). Needs **deeper-review**. |

### 3.3 `crates/lb-io/src/ring.rs` (io_uring)

| Line | Construct | Verdict |
|---|---|---|
| 50 | `unsafe { push_sqe(...) }` (NOP) | safe wrapper; small test path. | **JUSTIFIED**. |
| 99, 133, 157, 182 | `unsafe { push_sqe(&mut ring, &entry)? }` for ACCEPT/READ/WRITE/CONNECT | All entries are `squeue::Entry`; pointed-at buffer ownership reasoning depends on caller. | **NEEDS DEEPER REVIEW** — particularly READ/WRITE buffers must outlive the SQE submission. Cross-ref `code` teammate. |
| 110 | `unsafe { sockaddr_storage_to_socketaddr(...) }` | Family-tagged cast, length-checked. | **JUSTIFIED**. |
| 200 | `unsafe fn push_sqe` | Marked `unsafe fn` because the entry may reference memory the caller must keep live. | **JUSTIFIED** but tighten safety doc. |
| 203 | `unsafe { sq.push(entry) }` | io-uring's SubmissionQueue requires unsafe. | **JUSTIFIED**. |
| 255-303 | `unsafe fn sockaddr_storage_to_socketaddr` | Reads union-typed `sockaddr_storage` after family-tag check + length check. | **JUSTIFIED**. |
| 345 | `unsafe { libc::close(accepted_fd) }` (test) | Test-only; we own the fd. | **JUSTIFIED**. |

### 3.4 `crates/lb-io/src/lib.rs` + `crates/lb-io/src/sockopts.rs`

| File:line | Construct | Verdict |
|---|---|---|
| `lib.rs:343` | `unsafe { libc::getsockopt(...) }` | Live fd, stack-local `val`/`len`. | **JUSTIFIED**. |
| `sockopts.rs:124` | `unsafe { libc::listen(fd, backlog) }` | Live bound listener fd. | **JUSTIFIED**. |
| `sockopts.rs:238` | `unsafe { libc::setsockopt(...) }` | Live fd, stack-local `value`. | **JUSTIFIED**. |

### 3.5 cargo-geiger

`cargo geiger` is **not installed** in the dev environment (verified:
`cargo geiger --version` → `error: no such command 'geiger'`). Manual count
above: **71** `unsafe` sites across 5 files, all in `crates/lb-l4-xdp/*`
and `crates/lb-io/*`. **Zero `unsafe` in the L7, security, config,
controlplane, balancer, h1/h2/h3, quic, observability, or compression
crates** — confirmed by grep.

`unsafe-justifications.md` (top-level audit dir) is empty per
`audit/README.md` § "File ownership". This inventory is the seed.

---

## 4. Supply-chain snapshot

### 4.1 Tool availability

* `cargo audit` — **not installed** (`error: no such command 'audit'`). `.cargo/audit.toml` exists with 9 advisory ignores.
* `cargo deny` — **not installed**. `deny.toml` lists 11 advisory ignores + license allowlist (MIT/Apache-2.0/BSD/ISC/Unicode-3.0/Zlib/BSL-1.0/CC0-1.0/GPL-3.0/CDLA-Permissive-2.0).
* `cargo geiger` — **not installed**.

**Finding S-7:** the CI gate that should run `cargo audit && cargo deny check` cannot run in the dev sandbox; verify GitHub Actions workflow does install them before round-2 ends.

### 4.2 Cargo.lock package count

* **398** packages total (`grep -c '^name = ' Cargo.lock`).
* Direct workspace crates: 18.

### 4.3 Critical pinned versions

| Crate | Version | Recency signal |
|---|---|---|
| `hyper` | 1.9.0 | current 1.x line |
| `h2` | 0.4.13 | current (post-CVE-2024-27316 + RUSTSEC-2024-0003) |
| `tokio` | 1.51.1 | current |
| `rustls` | 0.23.38 | current |
| `quiche` | 0.28.0 | current 0.28 line |
| `tokio-quiche` | 0.18.0 | current |
| `tokio-tungstenite` | 0.24.0 | current |
| `tokio-rustls` | 0.26.4 | current |
| `brotli` | 7.0.0 | current 7.x |
| `flate2` | 1.1.9 | current |
| `zstd` | 0.13.3 | current |
| `ring` | 0.17.14 | current 0.17 line |
| `aya` | 0.13.1 | current |
| `io-uring` | 0.7.12 | current |
| `prometheus` | 0.13.4 | current 0.13 |
| `boring` / `boring-sys` | 4.21.2 | via tokio-quiche; transitive BoringSSL — track separately for CVEs that don't ship via RustSec |
| `foundations` | 4.5.0 | **pinned by Cargo.lock note** to preserve MSRV 1.85; rev periodically per the workspace `Cargo.toml` MSRV-pin notes |
| `http` | 0.2.12 *and* 1.x | duplicate-major (h2 still on 0.2.12); deny.toml marks multiple-versions = warn |

### 4.4 Build-script (`build.rs`) network/FS surface

Not inventoried this round — TODO Q5 to `rel`/`code`: `find . -name build.rs -not -path './target/*' -not -path './.claude/*'` should be cross-checked against transitive deps for fs-writes outside `OUT_DIR` (especially `boring-sys`, `cmake`, `bindgen`).

---

## 5. Secrets / config posture

### 5.1 Secret-loading sites

| Secret | Loader | Mode / source |
|---|---|---|
| TLS cert chain | `crates/lb/src/main.rs:187-198` (`load_cert_chain`) | `std::fs::File::open(path)` — relies on filesystem perms; no perm assertion. |
| TLS private key | `crates/lb/src/main.rs:202-211` (`load_private_key`) | Same — `rustls_pemfile::private_key`; takes the first key. **No mode assertion** (e.g., enforce 0o600). |
| QUIC retry-token secret | `crates/lb-quic/src/listener.rs:291-323` (`load_or_generate_retry_secret`) | Loads 32 bytes from `retry_secret_path` or generates fresh via `ring::SystemRandom` and writes mode **0o600** via `write_secret_file` (unix path, line 326-337). **GOOD**. |
| TLS session-ticket key | `crates/lb-security/src/ticket.rs` (`TicketRotator`) | In-memory rotation, no on-disk persistence. |
| 0-RTT replay-guard HMAC key | `crates/lb-security/src/zero_rtt.rs:54-69` (`fresh_secret`) | Per-instance `SystemRandom`; falls back to time-mixed seed if RNG fails (downgrade explained in code comment). |
| Retry signer test default | `crates/lb-quic/src/router.rs:441` (`[0xa5u8; 32]`) | **Test-only** literal; not reachable from production listener. Confirm.|

### 5.2 Default-config behavior with placeholder secrets

`config/default.toml` is 8 lines:

```toml
[[listeners]]
address = "0.0.0.0:8080"
protocol = "tcp"

[[listeners.backends]]
address = "127.0.0.1:3000"
weight = 1

[[listeners.backends]]
address = "127.0.0.1:3001"
weight = 1
```

* No TLS, no QUIC, no admin listener, no security thresholds.
* Bind `0.0.0.0:8080` → **publicly reachable** if run on a public NIC.
* No backends online → connection forwards fail; not a security issue but useful for `rel`.

### 5.3 Tracing / log redaction

* `RetryTokenSigner::fmt::Debug` redacts the secret (`finish_non_exhaustive`) — good.
* `tracing::info!` lines in `crates/lb/src/main.rs:648-649, 730, 823` log `cert_path`, `key_path`, `retry_secret_path` — **paths only**, no key material. Good.
* `tracing` env filter defaults to `info` (main.rs:931-936) — no body logging at info level.
* **Open**: verify `H1Proxy::handle` does not log full request/response headers (which can contain `Authorization`, `Cookie`) at any level. Cross-ref `code`.

---

## 6. Preliminary findings (will be re-emitted with IDs in Round 2)

* **S-1 (high) — `SmuggleDetector` is defined but not wired into the live proxy hot path.** No call site in `crates/lb-l7/*` or `crates/lb-h1/*`. Hyper's defaults reject some CL+TE combinations but not all variants the detector targets (duplicate same-value CL with intermediate value-stripping by an upstream; TE-CL with non-`chunked` final encoding; H2-downgrade leaking pseudo-headers). Tests at `tests/security_smuggling_*` exercise the detector directly, not via a live request.

* **S-2 (medium) — XDP CONNTRACK maps are non-LRU `HashMap` with finite size.** `crates/lb-l4-xdp/ebpf/src/main.rs:210-218`. Attacker spraying unique 5-tuples fills the 1M v4 / 512K v6 maps, after which legitimate flows can't insert until userspace evicts. Consider `LruHashMap` or a periodic age-based purge.

* **S-3 (medium) — `SlowlorisDetector` / `SlowPostDetector` not wired into HTTP listeners.** Only hyper's coarse `HttpTimeouts::header=10s, body=30s, total=60s` (`h1_proxy.rs:96-104`) defends. No per-IP / per-listener cap.

* **S-4 (high) — No per-IP / per-listener concurrent-connection cap.** Listener backlog 50_000 (main.rs:114-126). Accept loop spawns one task per connection (main.rs:1099 area) with no semaphore. Attacker exhausts fd table; also amplifies S-3.

* **S-5 (low-medium) — 0-RTT replay window collapses under spray.** `ZeroRttReplayGuard` is a fixed-cap ring buffer (`zero_rtt.rs:76-86`); attacker generating unique 0-RTT tokens evicts legitimate ones before the real replay arrives. Mitigated by RetryTokenSigner's `max_age` (10 s) on the QUIC handshake path, but worth a time-windowed bloom filter or larger cap.

* **S-6 (medium-info) — Admin HTTP listener has no authentication.** `crates/lb-observability/src/admin_http.rs:1-5`. Operator-trust model. Document bind-to-loopback as the supported configuration; consider failing validation when `metrics_bind` is non-loopback unless an explicit `unsafe_bind_public = true` flag is set.

* **S-7 (info) — `cargo audit` / `cargo deny` / `cargo geiger` are not installed locally.** CI workflow must run them. `deny.toml` ignore list is long (10 RUSTSEC IDs); each has a written justification — still merits a re-read with `rel` in round 2.

* **S-8 (info) — TLS key file permissions not asserted at load.** `load_private_key` (`main.rs:202-211`) reads PEM with no mode check. If an operator ships 0o644 the file is readable by any local user. Consider warn-on-load when mode is `> 0o600`.

* **S-9 (info) — `unsafe impl Pod`** on `BackendEntry{,V6}` (`crates/lb-l4-xdp/src/loader.rs:76,120`): the structs contain padding bytes (`pad: u16`, `pad: [u8;3]`) that, if not zeroed before `map.insert`, would leak userspace stack bytes into the kernel map. Verify all insert sites memset/zero-init. Cross-ref `ebpf`.

---

## 7. Open questions for the team

* **Q1 → `proto`:** Where does smuggle detection happen on the live proxy path? Is hyper-only mitigation considered sufficient, or is `SmuggleDetector` waiting on a wiring task? (Cross-ref S-1.)
* **Q2 → `proto` / `code`:** Why are `SlowlorisDetector` / `SlowPostDetector` exported but unused? (Cross-ref S-3.)
* **Q3 → `rel`:** Is the absence of per-IP / per-listener concurrent-connection limits a known gap with a planned mitigation (token bucket / semaphore)? (Cross-ref S-4.)
* **Q4 → `proto`:** Which proxy paths (H1, H2, H3, gRPC, WS) invoke `lb-compression::Decompressor` for **request** bodies (where the bomb attack lives)? Are caller-supplied `max_bytes` consistently set to something derived from `H2SecurityThresholds`?
* **Q5 → `rel` / `code`:** Inventory of `build.rs` scripts in direct + transitive deps that write outside `OUT_DIR` or fetch from the network. `boring-sys` invokes `cmake` + `bindgen` — please verify hermeticity.
* **Q6 → `ebpf`:** Are CONNTRACK / CONNTRACK_V6 maps intentionally non-LRU? Is there a userspace evictor? (Cross-ref S-2.)
* **Q7 → `ebpf`:** Confirm padding-zero invariant on `BackendEntry{,V6}` inserts. (Cross-ref S-9.)
* **Q8 → `proto`:** TLS-listener mTLS-client-cert support — out of scope for v1, or planned? Cert-pinning between gateway and backends?
* **Q9 → `code`:** `crates/lb-io/src/ring.rs:99-182` SQE pushes — do the READ/WRITE buffers live as long as the kernel needs them? Looking for `Pin` / `Box::leak` / per-connection buffer-pool ownership.

---

## 8. Round-2 readiness

Inventory complete. Detectors-vs-wiring delta is the single largest unknown
and needs `proto` confirmation before I can finalize the H/L1/L2/L3
smuggle and slowloris findings. All other tracks (unsafe, supply chain,
secrets) are ready to harden into Round-2 findings.

— `sec`, Round 1.
