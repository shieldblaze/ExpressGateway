## SEC — Round 2 Findings

Owner: `sec` (security-reviewer). Round 2 / ROUND_FINDINGS. Branch: `main`.

IDs are stable across rounds. Each Round-1 candidate `S-NN` is promoted
to `SEC-2-NN` with concrete evidence, finalized severity, and citation
of the relevant source line(s). IDs above `SEC-2-12` are new themes
that surfaced when reading code/proto/ebpf/rel Round-1 inputs.

Format per finding:

```
### <ID> — <title>
Severity: critical | high | medium | low | info
Status:   Open
Location: <file>:<line>
Description: ...
Impact:     ...
Reproduction: ...
Recommendation: ...
Cross-ref:  ...
```

Severity definitions (lead-aligned):
- **critical**: remote unauthenticated control / RCE / TLS bypass /
  data exfiltration on the default config.
- **high**: remote DoS reachable on default config, smuggle-class
  request integrity break, kernel-side memory hazard.
- **medium**: requires non-default config / mis-trusted operator /
  partial mitigation already in place / kernel-side hazard limited
  to today's call sites but latent for tomorrow's.
- **low**: hardening item, fixable in a small patch, no live attack
  path on current code.
- **info**: posture/process item; no code change blocks Round 3 plan.

---

### SEC-2-01 — `SmuggleDetector` is dead code on the proxy hot path
Severity: high
Status:   Proposed-Fix(e36b50f, 0c7e16b, e00e85a, 5e7938f) — Wave-2a API (e36b50f, 0c7e16b) + Wave-2b lb-l7 detector wire-up & H2→H1 downgrade guard (e00e85a on round-4 / 0ae776d on worktree) + Wave-2b proof tests (5e7938f on round-4 / e79f4f6 on worktree)
Location: `crates/lb-security/src/smuggle.rs:9-156`; absence in
`crates/lb-l7/src/h1_proxy.rs` + `crates/lb-l7/src/h2_proxy.rs`;
absence in `crates/lb-l7/Cargo.toml` + `crates/lb-h1/Cargo.toml` +
`crates/lb-h2/Cargo.toml` (no `lb-security` dependency); only
`crates/lb/src/main.rs:56` imports `lb_security`, and the import is
restricted to `TicketRotator, build_server_config`.
Description: `SmuggleDetector::{check_duplicate_cl, check_cl_te,
check_te_cl, check_h2_downgrade, check_all}` is a full per-RFC
smuggling check surface but **never invoked on a live request**. The
only call sites are unit tests under `tests/security_smuggling_*`
that exercise the detector in isolation. `lb-l7` (the only crate
that owns the proxy hot path) does not depend on `lb-security` and
has no equivalent inline check. Hyper 1.9.0 (pinned in `Cargo.lock`)
rejects the canonical CL+TE and CL-with-differing-values cases at
the wire-decoder level, **but does not catch**:
1. H2-to-H1 downgrade leaking forbidden hop-by-hop headers or
   pseudo-headers into the H1 upstream request synthesized by
   `H2Proxy::handle` (the static hop-by-hop strip list in
   `h1_proxy.rs:53-64` is the only line of defense, and it does
   **not** strip `:`-prefixed pseudo-headers if they arrive verbatim
   in a malformed h2 frame – hyper rejects most but not all).
2. `Transfer-Encoding: gzip, chunked` is accepted by hyper as a
   chunked request (final encoding is `chunked`); a downstream
   server that does not implement RFC 9112 layering can be
   smuggled by the gzip layer.
3. Mixed-case `TE` / `Transfer-Encoding` reaching an upstream that
   does case-sensitive matching.
4. Duplicate same-value Content-Length where an intermediary
   strips one value (the upstream sees one CL, the gateway saw
   two; hyper accepts because hyper checks values agree).
Impact: a crafted request can desync the gateway↔upstream parser,
yielding **request smuggling**: response queue confusion, cache
poisoning, authentication bypass on the upstream side, persistent
poisoning of keep-alive connections. With H2-to-H1 downgrade also
unguarded, an H2 client can manufacture an H1 request line that the
upstream parses but the gateway never sanitized.
Reproduction: `grep -rln "SmuggleDetector\|check_cl_te\|check_te_cl\|check_h2_downgrade" crates/lb-l7 crates/lb-h1 crates/lb-h2 crates/lb`
returns only the test files and the definition. `cargo tree
-p lb-l7 | grep lb-security` returns nothing. There is no test
case in `tests/` that fires a CL+TE request through a live
`H1Proxy` and expects rejection.
Recommendation: minimum fix is to add `lb-security` as a dependency
of `lb-l7` and call `SmuggleDetector::check_all(&headers, is_h2)`
inside `H1Proxy::handle` and `H2Proxy::handle` **before** the
hop-by-hop strip and before any upstream connection is acquired,
returning 400 on `SecurityError::Smuggle*`. Add an integration test
(live `hyper::server` + crafted body) per detected vector.
Cross-ref:  T1 (synthesis); code Q-CODE-1-01 (dependency graph);
proto #1 (hyper-only coverage matrix); rel F-17 sibling DoS.

---

### SEC-2-02 — XDP CONNTRACK / CONNTRACK_V6 are non-LRU `HashMap`s
Severity: high
Status:   Open
Location: `crates/lb-l4-xdp/ebpf/src/main.rs:210-218` (map decl);
`crates/lb-l4-xdp/src/loader.rs:302-326` (userspace accessors).
Description: The conntrack maps are aya `HashMap` (translated to
`BPF_MAP_TYPE_HASH` in the kernel), **not** `LRU_HASH`. Map sizes
are 1_000_000 entries for IPv4 and 512_000 for IPv6. There is no
userspace evictor running in the binary today (no scan loop, no
TTL sweep — `grep -n "delete\|remove" crates/lb-l4-xdp/src/loader.rs`
returns no eviction code).
Impact: a single attacker host that sprays unique 5-tuples (TCP
SYN with rotating ephemeral ports, ~64k unique src-ports per src
IP) fills the IPv4 map in <16 IPs at ≤64k flows each. Once full,
`bpf_map_update_elem` returns `-E2BIG` and **new legitimate flows
cannot pin to a backend** — they fall back to the slow hash path
(Maglev recompute every packet) and at high rates degrade to packet
drop. This is the classic SYN-flood-into-LB-state attack against a
non-LRU map.
Reproduction: from a single host, run `hping3 -S -p 80 --flood
--rand-source <listener-ip>` for ~30 s (1M packets). After the run,
`bpftool map dump name CONNTRACK | wc -l` should approach 1M and
remain there until process restart (no eviction). New
`curl <listener-ip>` will see degraded XDP behavior. (Not runnable
in audit sandbox — no XDP NIC, no `bpftool`.)
Recommendation: change the map declaration to `LruHashMap`
(BPF_MAP_TYPE_LRU_HASH) — eviction is then kernel-managed. If LRU
semantics are not desired, add a userspace evictor that deletes
entries older than `keepalive_timeout` (see `lb-quic/src/listener.rs`
for the matching idle-timeout knob) on a periodic timer. Add a
Prometheus gauge `xdp_conntrack_entries{v=4,v=6}` so saturation
becomes alertable (handed to `rel`).
Cross-ref:  T4 (synthesis); ebpf §2 Maps; rel saturation alert.

---

### SEC-2-03 — `SlowlorisDetector` / `SlowPostDetector` not wired
Severity: medium
Status:   Proposed-Fix(1f7f417) — Wave-2a Watchdog API; Wave-2b wires lb-l7
Location: `crates/lb-security/src/slowloris.rs`, `slow_post.rs`;
absence in `crates/lb-l7/Cargo.toml`; absence in `H1Proxy::handle`
and the TLS accept site at `crates/lb/src/main.rs` (`acceptor.accept`).
Description: same dependency-graph gap as SEC-2-01: detectors
exist, no proxy uses them. The only request-pacing defense is
hyper's `HttpTimeouts::default` (header=10 s, body=30 s,
total=60 s) configured in `h1_proxy.rs:96-104`. Critically,
**the rustls handshake is not wrapped in any timeout** — an
attacker can hold a TCP connection open in `acceptor.accept().await`
forever (no rustls-accept timeout exists anywhere).
Impact: an attacker holding N connections per source IP (no per-IP
cap — see SEC-2-04) consumes one tokio task + one file descriptor
each. Combined with `listener_opts()` backlog of 50_000
(`crates/lb/src/main.rs:114-126`), a small fleet of attackers can
saturate the process fd limit and accept queue while sending one
TLS ClientHello byte per minute.
Reproduction: write a 1-byte-per-30-seconds TLS handshake from
50_000 source ports. Process file descriptor count climbs to the
`ulimit -n` ceiling without ever issuing an HTTP request.
Recommendation: wrap `acceptor.accept()` in `tokio::time::timeout`
(suggest 5 s). Wire `SlowlorisDetector` into the request header
loop in `H1Proxy::handle` to enforce min-bytes-per-tick. Wire
`SlowPostDetector` into the request body loop in `H1Proxy::handle`
on the request side and `H2Proxy::handle` on H2 DATA frames. Pair
with SEC-2-04 (per-IP cap) and rel F-05 (TLS-accept timeout).
Cross-ref:  T1; rel F-05 (TLS accept slowloris); rel F-17.

---

### SEC-2-04 — No per-IP / per-listener concurrent-connection cap
Severity: high
Status:   Proposed-Fix(e36b50f, 8e048c0) — Wave-2a ConnGate API; Wave-2c wires lb/main.rs
Location: `crates/lb/src/main.rs:1077-1126` (`run_listener` accept
loop); `crates/lb/src/main.rs:114-126` (`listener_opts` backlog).
Description: the accept loop spawns one `tokio::task` per accepted
connection with no semaphore, no per-source-IP map, and no
listener-level cap. QUIC has a `max_connections: 100_000` knob;
TCP/H1/H1s/H2 have none. The 50_000 backlog plus unbounded spawn
means a single TCP SYN burst can spawn 50k tasks; sustained, this
is unbounded.
Impact: trivial DoS. Any host with enough source ports can exhaust
the fd table and tokio scheduler. Amplifies SEC-2-03 because every
slow connection is a free fd slot.
Reproduction: `for i in $(seq 1 65535); do (nc -q 999 <listener-ip>
8080 < /dev/zero &); done` from one host fills the fd table within
seconds.
Recommendation: a `tokio::sync::Semaphore` on each listener
configured from `[runtime].max_concurrent_connections` (default
10_000 per listener; configurable) gating the per-accept spawn.
Per-source-IP cap via a `lru::LruCache<IpAddr, AtomicUsize>` sized
to the same limit. Return TCP RST or accept-and-immediate-close on
overflow. **Critical**: also bound `tokio::net::TcpListener::accept`
inside a `loop { … }` with backoff on `EMFILE` (otherwise
`accept` hot-loops on the same error — see rel F-07).
Cross-ref:  T1; rel F-17 (same site); code Q-CODE-1-05 (CancellationToken
plumbing).

---

### SEC-2-05 — 0-RTT replay window collapses under unique-token spray
Severity: medium
Status:   Proposed-Fix(eeae98a) — Wave-2a LRU + 65 536 default window
Location: `crates/lb-security/src/zero_rtt.rs:76-86, 125-143`.
Description: `ZeroRttReplayGuard` is a fixed-cap HashSet (default
cap 1_048_576). Replay detection requires the **earlier** token to
still be in the set when the replay arrives. Under unique-token
spray (attacker generates random 0-RTT garbage at line rate), the
set's oldest entries are evicted to make room. A legitimate 0-RTT
token replayed >cap-tokens later will be **accepted** as fresh.
Impact: bounded replay window collapses from 10 s (`RetryTokenSigner`
max_age) to "however long it takes the attacker to flush the
cap-set". At a modest 100k 0-RTT attempts/sec the 1M window
collapses to ~10 s — same order as `max_age`, so today the
attack is partially absorbed by retry signer expiry. At higher
spray rates, replay window collapses to <1 s, then the only
defense is whether the upstream sees the same request twice (it
will).
Reproduction: send 1M unique random 0-RTT-style tokens, then
replay a legitimately captured 0-RTT request. The guard returns
`Ok(false)` (no replay) because the captured token has been
evicted.
Recommendation: replace fixed HashSet with a **time-windowed
bloom filter** sized to `max_age` × peak-RPS, or use a sharded
ring-buffer keyed by HMAC bucket. Add a counter
`zero_rtt_evictions_total` for observability and refuse 0-RTT
when the eviction rate spikes (a sign of spray).
Cross-ref:  rel F-19 (TCP 0-RTT — withdrawn in SEC-2-0RTT-TCP below).

---

### SEC-2-06 — Admin HTTP listener has no authn / no TLS
Severity: medium
Status:   Proposed-Fix(baa72ca) — Wave-2a AdminAuthGate API; rel REL-2-04 wires admin_http.rs
Location: `crates/lb-observability/src/admin_http.rs:80-134`;
`crates/lb-observability/src/admin_http.rs:3` (operator-trust
comment).
Description: `/metrics` and `/healthz` bind plaintext HTTP with no
auth and no IP allowlist. The crate's own comment delegates this
to the operator. Default config does not enable the admin listener,
so the default attack surface is zero — but an operator who
follows the README and sets `metrics_bind = "0.0.0.0:9090"`
ships a public Prometheus endpoint with full cardinality leak
(backend addresses, listener counts, error counters by code).
`/healthz` returning unconditional 200 also leaks "process is up"
to any prober.
Impact: information disclosure: backend topology, traffic volume,
per-error rates. Useful for an attacker mapping the deployment
before launching SEC-2-04 / SEC-2-02. No direct request integrity
impact.
Reproduction: `curl http://<host>:9090/metrics` from any internet
host once the operator binds publicly.
Recommendation: refuse non-loopback bind in `parse_config` unless
operator sets `unsafe_bind_public = true`; add a bearer-token auth
option; document the supported recipe (mTLS-fronted scrape via
unix socket or loopback + sidecar). `/healthz` should split into
`/livez` (binary up/down, unauth) and `/readyz` (binary ready/not,
unauth) — readyz must flip to 503 during drain.
Cross-ref:  rel F-18 (readiness flip); rel H4 (json logs / labels).

---

### SEC-2-07 — Supply-chain CI: cargo-audit lacks `-D warnings`; no cargo-geiger
Severity: medium
Status:   Proposed-Fix(1fbfd14) — audit -D warnings, geiger inventory, machete soft check
Location: `.github/workflows/ci.yml:99-129` (audit + deny jobs);
`.github/workflows/codeql.yml:22-28` (audit only).
Description: CI **does** run `cargo audit` and `cargo deny check`
(verified by reading ci.yml). However:
1. `cargo audit` is invoked **without** `-D warnings` (line 114).
   Yanked crates and unmaintained advisories therefore log a
   warning and exit 0 — the job passes. Recommended invocation
   is `cargo audit -D warnings` (or `cargo audit --deny warnings`).
2. `cargo geiger` is **not** invoked anywhere in CI. The
   `unsafe` inventory (71 sites — `audit/security/round-1-inventory.md`
   §3) is therefore neither tracked nor regression-gated.
3. `.cargo/audit.toml` ignores 9 advisories and `deny.toml`
   ignores 10–11; each has a written justification but the
   justifications are not re-evaluated on a schedule. Some of
   the ignored advisories (e.g. `time`, `rand`, `dashmap`)
   correspond to crates pulled in transitively and could be
   upgraded.
4. There is **no `cargo-machete` step** — code Round-1
   `round-1-machete.txt` flagged 14 suspect-unused deps; this
   should fail CI when new dead deps appear.
Impact: a newly published `RUSTSEC-*` against a transitive dep,
a yanked crate, or an upstream license change can land on `main`
without breaking CI. Combined with SEC-2-01/03 (unwired security
crate), reviewer trust in CI to catch security regressions is
overstated.
Reproduction: not runnable in sandbox (`cargo-audit`,
`cargo-deny`, `cargo-geiger` not present locally). Verification
target: GitHub Actions run logs.
Recommendation: change line 114 to `cargo audit -D warnings`;
add `cargo deny check` already present; add a `cargo geiger
--all-targets --output-format Json --fail-on-warnings false`
step that snapshots the `unsafe` count and gates on regression;
add `cargo-machete` step. Schedule a quarterly review of
`audit.toml` / `deny.toml` ignores.
Cross-ref:  T9; code round-1-machete.txt.

---

### SEC-2-08 — TLS private-key file permissions not asserted at load
Severity: low
Status:   Proposed-Fix(2374ec1) — Wave-2a assert_owner_only helper; code wires lb/main.rs
Location: `crates/lb/src/main.rs:202-211` (`load_private_key`);
`crates/lb/src/main.rs:187-198` (`load_cert_chain`).
Description: `std::fs::File::open(path)` does not check the file's
unix mode bits. An operator-owned `0o644` private-key file is
readable by every local user / container sidecar / privileged
script. Compare with `lb-quic` which writes the retry secret
mode-0o600 (`crates/lb-quic/src/listener.rs:326-337`) — the
asymmetry is the bug.
Impact: if an operator mis-permissions the private key (very
common: editors and CI pipelines drop `0o644` by default), any
local actor with the same UID or a co-tenant container reads the
key. Not reachable from network, but a classic compliance failure
(SOC 2, FedRAMP, PCI).
Reproduction: `chmod 0644 server.key && ./expressgateway -c config.toml`
starts cleanly with no warning.
Recommendation: in `load_private_key`, call
`std::fs::metadata(path)?.permissions().mode() & 0o077` and warn
(or error if `[runtime].strict_mode = true`) if non-zero.
Cross-ref:  rel posture; code Q-CODE-1-03.

---

### SEC-2-09 — `unsafe impl Pod` padding-leak invariant: no constructors, no defenders
Severity: low
Status:   Open
Location: `crates/lb-l4-xdp/src/loader.rs:53` (`FlowKey`),
`crates/lb-l4-xdp/src/loader.rs:76` (`BackendEntry`),
`crates/lb-l4-xdp/src/loader.rs:97` (`FlowKeyV6`),
`crates/lb-l4-xdp/src/loader.rs:120` (`BackendEntryV6`).
Description: each of these structs has explicit `pub pad: [u8;3]`
or `pub pad: u16` fields that the eBPF program (kernel side)
treats as ignorable but the kernel still stores byte-for-byte in
the map. The fields are `pub`, so any future caller constructing
via struct literal could supply non-zero values; there is **no
constructor**, **no `Default`**, and **no `Zeroable` discipline**.

Verifying the team-lead's mandatory item #3: `grep -rn
"BackendEntry\s*{\|FlowKey\s*{" crates/lb-l4-xdp/src/loader.rs
crates/lb/src/ crates/lb-controlplane/ crates/lb-cp-client/`
returns **zero** production construction sites today — the
`loader.rs` types are public but the only thing in the binary
that touches them is the typed accessor returning `AyaHashMap`.
Tests in `crates/lb-l4-xdp/src/lib.rs:420-598` and
`tests/l4_xdp_*.rs` construct a **different**
`lb_l4_xdp::FlowKey` (declared at `lb-l4-xdp/src/lib.rs:84`,
no `pad` field), not the `loader.rs` one. So today the leak is
latent.

The hazard is still real: the moment the control-plane wiring
referenced by code's machete output (`lb-controlplane` and
`lb-health` flagged as suspect-unused in the binary) lands, it
will populate CONNTRACK and pad bytes will be written into kernel
memory. If those pad bytes contain stack residue from a previous
call, kernel-side BPF programs that read the full key (e.g.
`bpf_map_lookup_elem` returns a pointer the BPF prog hashes
including pad bytes) could leak userspace stack content into
flow-hash buckets — and userspace can read it back via
`bpf_map_get_next_key` + lookup, exfiltrating stack bytes.
Impact: today: latent. After Pillar 4b-3 control-plane wiring:
local-stack-byte → kernel-map-byte information leak, exploitable
by anyone with `CAP_BPF` on the same host (i.e. typically the LB
operator only — but co-tenant containers in lax setups).
Reproduction: write a constructor that omits `pad` initialization,
insert into the map, dump the map from a sibling process via
`bpf_map_get_next_key`. Pad bytes contain caller's stack
residue.
Recommendation: add a private constructor on each struct
(`pub fn new(...)` taking only the non-pad fields and zero-
initializing `pad`). Mark the pad fields `pub(crate)` not `pub`.
Better: switch to a `bytemuck::Zeroable + Pod` discipline so the
compiler enforces zero-init.
Cross-ref:  T5; ebpf cross-review to-code #3; code padding fix.

---

### SEC-2-10 — TLS-handshake slowloris on `acceptor.accept().await`
Severity: medium
Status:   Proposed-Fix(67697c0) — Wave-2a timeout_accept helper; Wave-2c wires lb/main.rs
Location: `crates/lb/src/main.rs` TLS accept sites; see also rel
F-05.
Description: in the TLS-over-TCP listener path the rustls
acceptor future is awaited with no timeout. Attacker opens TCP,
sends 1 byte of ClientHello, never sends more — the future is
parked forever, holding one fd and one task.
Impact: same as SEC-2-04 but more efficient for the attacker
(one byte holds one resource indefinitely).
Reproduction: see SEC-2-03 reproduction. Same vector, separate
defense knob.
Recommendation: wrap with `tokio::time::timeout(Duration::from_secs(5),
acceptor.accept())`; on `Err(Elapsed)` log and drop.
Cross-ref:  rel F-05; SEC-2-04.

---

### SEC-2-11 — XDP capability probe misses CAP_SYS_ADMIN fallback
Severity: low
Status:   Proposed-Fix(e44117d) — ebpf landed under EBPF-team ownership: `probe_caps_with` closure-based probe with CAP_BPF→CAP_SYS_ADMIN fallback, 7-test mock matrix in `tests/xdp_cap_probe.rs`.
Location: `crates/lb/src/xdp.rs:39-55` (probe site, per ebpf
cross-review §A.2). The probe checks `CAP_BPF` + `CAP_NET_ADMIN`
only.
Description: pre-5.8 Linux kernels do not split `CAP_BPF` out of
`CAP_SYS_ADMIN`. Hosts running 5.4 / 5.6 (RHEL 8 derivatives,
Amazon Linux 2) hold `CAP_SYS_ADMIN` and not `CAP_BPF` — the
probe fails opaquely with "missing CAP_BPF" while the process
actually has full BPF authority.
Impact: operator confusion → either the LB refuses to start on
a supported kernel, or the operator grants `CAP_SYS_ADMIN`
broadly to work around the failure (over-privilege).
Reproduction: run on a 5.4 kernel container with
`--cap-add SYS_ADMIN` but without `--cap-add BPF`. Probe rejects
with a misleading error.
Recommendation: change probe to `(CAP_BPF || CAP_SYS_ADMIN) &&
(CAP_NET_ADMIN || CAP_SYS_ADMIN)`, document the floor as
"5.8 with CAP_BPF, or 5.4–5.7 with CAP_SYS_ADMIN".
Cross-ref:  ebpf cross-review §A.2.

---

### SEC-2-12 — BPF ELF license / loader license-string not set
Severity: medium
Status:   Proposed-Fix(5064a11) — ebpf landed under EBPF-team ownership: `XdpLoader::load_from_bytes` now asserts `.license == "GPL\0"` via `assert_license_is_gpl`, fail-fast `XdpLoaderError::LicenseInvalid` variant + 3 unit tests with hand-crafted ELFs + integration test `tests/loader_license_assert.rs`.
Location: `crates/lb-l4-xdp/ebpf/src/main.rs` (no
`#[link_section = "license"]`); `crates/lb-l4-xdp/src/loader.rs:212`
(`EbpfLoader::new().load(elf)` — no `set_license` call). See ebpf
Round-1 inventory.
Description: kernel BPF subsystem requires the loaded program's
license to be GPL-compatible (`GPL`, `GPL v2`, `Dual MIT/GPL`,
etc.) to call most helpers. Without a `.license` ELF section or
an explicit `EbpfLoader::set_license("GPL")`, aya 0.13's default
may produce an empty license string, which the verifier rejects
on `bpf_prog_load`. ExpressGateway eBPF uses `bpf_map_*` helpers
that have `gpl_only=true` (CONNTRACK lookups, stats array writes).
Impact: program load fails on real NICs (`Operation not permitted`
or `Permission denied`), not detected by tests because the test
suite runs only `parse_object_only` (no kernel attach). Pillar 4b
attach scenario breaks; the LB silently degrades to userspace-only.
Reproduction: `bpftool prog load .../target/bpfel-unknown-none/release/lb-xdp-ebpf /sys/fs/bpf/xdp-test`
on a kernel ≥5.4 — kernel rejects with "program uses GPL-only
function …".
Recommendation: add `#[link_section = "license"] #[used] pub
static LICENSE: [u8; 4] = *b"GPL\0";` to the eBPF crate's root.
Belt-and-suspenders: also call
`EbpfLoader::new().set_license("GPL").load(elf)` in `load_from_bytes`.
Cross-ref:  T5; ebpf inventory §2.

---

### SEC-2-13 — 0-RTT on TCP/TLS listener: **disposition of rel F-19**
Severity: info
Status:   Closed-as-not-a-bug (rel F-19 withdrawn)
Location: `crates/lb-security/src/ticket.rs:319-338`
(`build_server_config`).
Description: per lead-mandated check #2, I read `build_server_config`
and confirm: the function constructs a `rustls::ServerConfig` and
**never assigns `max_early_data_size`**. In rustls 0.23.38 the
default for `ServerConfig::max_early_data_size` is `0`, meaning
0-RTT (TLS 1.3 early data) is **disabled by construction** on
every TCP/TLS listener. `grep -rn "max_early_data_size\|early_data"
crates/` returns zero hits across the entire repository.
Impact: none today. There is no 0-RTT attack surface on the TCP
path because 0-RTT is not enabled. If a future change starts
calling `cfg.max_early_data_size = N`, this finding must be
re-opened as **critical** (no replay guard on TCP).
Reproduction: read `build_server_config` lines 319-338; confirm
absence of `early_data` assignment.
Recommendation: rel F-19 withdrawn. Add an `#[cfg(test)]` invariant
in `ticket.rs` that asserts `build_server_config(...).?max_early_data_size
== 0`, so the disposition is regression-protected. Optionally,
write a comment in `build_server_config` documenting that 0-RTT is
intentionally disabled and that any future enablement requires
wiring a `ZeroRttReplayGuard` analogous to the QUIC path.
Cross-ref:  rel F-19 (resolved here).

---

### SEC-2-14 — `lb-compression::Decompressor` is unused (no proxy invokes it)
Severity: medium
Status:   Verified-Fixed(f93c582) — closed by L-001: lb-compression removed from workspace (see Cargo.toml workspace.members comment)
Location: `crates/lb-compression/src/lib.rs:226-310` (bomb-guarded
decoder); `grep -rln "lb_compression\|lb-compression" crates/`
returns only `crates/lb-compression/Cargo.toml` (i.e. **no
dependent crate**).
Description: same anti-pattern as SEC-2-01/03 — a fully-implemented
defense crate that nothing in the production graph depends on.
The bomb-detector (`OutputTooLarge`, `BombDetected`) is correct in
isolation but is never invoked on a live request or response.
Inspection of `crates/lb-l7/src/h{1,2}_proxy.rs` confirms the
proxy streams the upstream response body byte-for-byte without
attempting to decompress for inspection.
Impact: depends on use case. The gateway is a pass-through proxy,
so it does **not** typically decompress responses — the client
does. So bomb attacks against the **client** are out of scope.
HOWEVER:
* If any future filter wants to inspect / modify response bodies
  (e.g. BREACH guard, content rewrite, JSON validation) and uses
  `Decompressor`, the caller-supplied `max_bytes` is the only
  defense. There is no library-side default cap.
* If gRPC / H2 features add server-side decompression of request
  bodies (e.g. to enforce gRPC max-message-size before forwarding),
  the same caller-must-supply-cap discipline applies.
The verification mandated by item #6 (lead-assigned): the cap is
caller-driven, **there is no process-memory-aware bound**. A
caller passing `u64::MAX` (or, more realistically, copying a value
from `H2SecurityThresholds` that the operator over-configured)
gets no protection. Under N concurrent connections each calling
`Decompressor::decompress(stream, max_bytes=1 GiB)` the process
OOMs at N≈(RAM/1 GiB) concurrent attacks.
Reproduction: write a 10-byte gzip whose decompressed output is
10 GiB ("zip bomb"). Pass any cap >= the decompressed size to
`decompress`. The function happily completes and returns 10 GiB.
The caller must enforce N×max_bytes < RAM, which today no caller
does.
Recommendation:
1. Surface the dependency-graph gap: state explicitly that the
   crate is unused. If decompression is in scope for v1, wire the
   bomb-guarded decoder into the body-inspection path; if not,
   delete the crate (or feature-gate it as `pub`-but-unused
   experimental).
2. When wired, add a global ceiling `max_bytes = min(caller_supplied,
   process_memory / max_concurrent_decompress)` computed at startup
   from `[runtime].max_decompress_bytes_per_connection` and the
   total connection cap (SEC-2-04). Document the math.
3. Bound the **per-process** concurrent-decompress count with a
   tokio `Semaphore` so N×max_bytes is bounded.
Cross-ref:  T1; proto open question Q4.

---

### SEC-2-15 — Hyper 1.9.0 smuggling defenses: what it catches vs. what it doesn't
Severity: info
Status:   Open (informational reference for SEC-2-01)
Location: `Cargo.lock` `hyper = 1.9.0`; `crates/lb-l7/src/h1_proxy.rs:53-64`
(hop-by-hop strip list).
Description: per item #7, I confirm what hyper 1.9.0 catches at
the wire-decoder level so SEC-2-01's residual risk can be sized.

Hyper 1.9.0 server-side rejections (HTTP/1.1):
* CL+TE both present **with TE not equal to a chunked encoding** —
  rejected as 400 (RFC 9112 §6.1).
* CL+TE both present **and TE is chunked** — hyper drops the CL
  header (TE wins), per the same RFC clause. **The dropped CL is
  not surfaced to the application**, so if an upstream uses the
  original CL (sent over an h1-to-h1 proxy chain) and the gateway
  forwards a chunked body, the upstream desyncs. This is the
  classic smuggle vector hyper does **not** mitigate.
* Two `Content-Length` headers with the same value — hyper
  coalesces and accepts (RFC 9110 §8.6 allows this).
* Two `Content-Length` headers with different values — hyper
  rejects 400.
* `Transfer-Encoding: gzip` (non-chunked, non-trailing-chunked) —
  hyper rejects (no chunked => can't determine end-of-body for h1).
* `Transfer-Encoding: chunked, gzip` (chunked NOT final) — hyper
  rejects.
* Header field with embedded CR / LF / NUL — hyper rejects.
* HTTP/2 `:method`, `:authority`, `:path`, `:scheme` echoed back
  into an H1 request line by the proxy — hyper does **not** catch
  this because hyper doesn't see the h2-to-h1 translation; the
  gateway code synthesizes the upstream request and is responsible
  for stripping pseudo-headers.
* HTTP/2 frame-level smuggling (CONTINUATION flood, etc.) — hyper's
  `H2SecurityThresholds` (correctly wired in `h2_proxy.rs:204`)
  catches.

Hyper does **not** catch:
* H2-to-H1 downgrade leaking forbidden hop-by-hop (`connection`,
  `transfer-encoding`, `keep-alive`, `upgrade`, `proxy-connection`)
  — the gateway must strip. `h1_proxy.rs:53-64` covers `connection`,
  `transfer-encoding`, `keep-alive`, `upgrade`, `proxy-connection`,
  `proxy-authenticate`, `proxy-authorization`, `te` (8 names). This
  is the correct list. **Confirmed coverage**.
* Pseudo-header leak (`:`-prefixed names) into the H1 upstream —
  hyper rejects `:`-prefixed names at the H1 serializer level
  (`hyper::header::HeaderName` parse rejects `:`), so this is
  implicitly mitigated by the type system.
* TE-CL: `Transfer-Encoding: gzip, chunked` is accepted because
  final encoding is chunked. If an upstream parser does not
  decompress gzip but uses CL (which hyper dropped), smuggle.
Impact: SEC-2-01 residual risk is **TE final-encoding sequence
attacks and H2-to-H1 case-folding edge cases**. With the static
strip list in `h1_proxy.rs:53-64` confirmed correct, the
exploitable surface is narrower than my Round-1 framing suggested
— but non-zero, because TE-with-non-trivial-encoding is unbounded
and upstream parsers vary.
Reproduction: send `Transfer-Encoding: gzip, chunked` with a body
encoded as gzip-over-chunked. hyper accepts; gateway forwards
verbatim (the strip list does not remove `transfer-encoding` from
the **outbound** synthesized request — verify). Upstream may
disagree on length.
Recommendation: revisit SEC-2-01 with this matrix; the wiring fix
should at minimum reject `Transfer-Encoding` with any non-`chunked`
final-encoding sequence, regardless of hyper's already-permissive
decision. Strict mode: reject `Transfer-Encoding` with >1 codec
entirely.
Cross-ref:  SEC-2-01; proto #1; T1.

---

### SEC-2-16 — Atomic-ordering hand-off list for `code`
Severity: info
Status:   Open (advisory; `code` owns per-site reclassification)
Location: enumeration below.
Description: per mandatory item #5, I enumerated every atomic-counter
site in the workspace and classified those that **gate a security
decision** vs. those that are pure statistics. The repo has 5 files
with atomic usage (`grep -rln "atomic::Ordering\|sync::atomic" crates/`).

**Pure statistics — `Relaxed` is correct, no change required**:
* `crates/lb/src/main.rs:1127` `active_connections.fetch_add(1)`
  and `:1212` `fetch_sub(1)` — used for `/metrics` cardinality
  only; **does not gate accept**. If SEC-2-04 wiring lands and
  this counter starts gating accept, ordering must be reviewed.
* `crates/lb-io/src/dns.rs:396, 416, 434-475, 489-530` — DNS
  query counter, observability only.
* `crates/lb-io/src/pool.rs:113-455` — connection-pool idle
  counter. **Relaxed is correct for the read-side** (`total.load`)
  in `Pool::idle_len`. **AcqRel needed** at the `fetch_add` /
  `fetch_sub` pair around `pool.total` in `pool.rs:185` and `:395`
  because the counter gates `if pool.total.load >= total_max`
  in `pool.rs:372`. Today `Relaxed` may permit a torn read
  letting `total_max + N` connections live transiently. Low impact
  (statistical over-provision) but technically incorrect under
  the memory model.
* `crates/lb-io/src/quic_pool.rs:184-587` — same shape as pool.rs.
  Same recommendation.
* `crates/lb-core/src/backend.rs:87-164` — active connection /
  request gauges with `compare_exchange_weak` loops; the CAS uses
  `Relaxed/Relaxed` on success/failure (`backend.rs:104-105,
  132-133`). For a pure-stats counter this is fine. **If** this
  counter ever gates "backend at capacity, shed", the CAS must
  use `AcqRel/Acquire` (and `latency_ns.store` at line 149 should
  remain `Relaxed`).

**Security-gating atomics — `Relaxed` is wrong**:
* `crates/lb-io/src/dns.rs:332-333` — already uses `AcqRel/Acquire`
  on the cache-refresh CAS. **Correct**.
* The H2 detectors (`crates/lb-h2/src/security.rs`,
  `RapidResetDetector`/`ContinuationFloodDetector`/`HpackBombDetector`/
  `SettingsFloodDetector`/`PingFloodDetector`/`ZeroWindowStallDetector`)
  are **not atomic** — they are plain `&mut self` structs owned by
  the per-connection codec. Single-threaded by construction.
  **Correct as written**.
* The 0-RTT replay guard (`crates/lb-security/src/zero_rtt.rs`) uses
  a `parking_lot::Mutex<HashSet>` — not an atomic counter.
  **Correct as written**.
* No per-IP rate-limit counter exists yet (the rate limiter is
  fictional — SEC-2-04). When it lands, the gate counter must use
  `AcqRel/Acquire` because it gates accept.
* No conntrack-inflight count exists in userspace.

Recommendation to `code` (per-site, ordered by priority):
1. `crates/lb-io/src/pool.rs:185, 395, 372` — promote the
   `pool.total` CAS pair to `AcqRel/Acquire`. Low-pri (statistical
   over-provision today; would matter under contention).
2. `crates/lb-io/src/quic_pool.rs:283, 518, 587, 499` — same as
   pool.rs.
3. Future SEC-2-04 wiring: the per-IP / per-listener gate must
   use `AcqRel/Acquire` on the CAS pair; the read in the accept
   loop must use `Acquire`.
Cross-ref:  T6; code Q-CODE-1-02.

---

## End of findings

**Summary by severity** (16 findings):
- critical: 0
- high: 3 — SEC-2-01, SEC-2-02, SEC-2-04
- medium: 6 — SEC-2-03, SEC-2-05, SEC-2-06, SEC-2-07, SEC-2-12, SEC-2-14
- low: 3 — SEC-2-08, SEC-2-09, SEC-2-11
- info: 4 — SEC-2-10 (medium-leaning, kept here for honesty), SEC-2-13, SEC-2-15, SEC-2-16

(Counting SEC-2-10 as medium — it is a real DoS vector even if
the underlying mitigation belongs to rel.)

Revised: critical 0, high 3, medium 7, low 3, info 3.

Round-1 disposition: S-1 → SEC-2-01 (severity finalized **high**
after proto/code dependency-graph confirmation). S-9 → SEC-2-09
(severity finalized **low** because no production map-insert site
exists today; will be re-promoted to **high** when control-plane
wiring lands — flagged for Round 4 re-audit).

Mandatory-item disposition:
1. All S-1..S-12 promoted (SEC-2-01..-12). ✓
2. 0-RTT TCP: rel F-19 withdrawn, evidence in SEC-2-13. ✓
3. `unsafe impl Pod`: no production insert sites yet — latent;
   evidence in SEC-2-09. ✓
4. Supply chain CI: SEC-2-07 specifies `-D warnings` + missing
   `cargo-geiger` + missing `cargo-machete`. ✓
5. Atomic ordering: SEC-2-16 hands `code` a per-site list with
   ordering recommendations. ✓
6. Compression bomb cap: SEC-2-14 documents that the cap is
   caller-driven, not process-memory-aware, and that no caller
   exists today. ✓
7. Hyper 1.9.0 smuggling defenses: SEC-2-15 inventory matrix. ✓

— `sec`, Round 2.
