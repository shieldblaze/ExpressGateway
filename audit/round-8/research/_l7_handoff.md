# L7 reference research — handoff to `div-l7` and `div-ops`

Author: `ref-l7`, 2026-05-14, Round-8 research phase.

Five reference briefs landed alongside this file:

- `audit/round-8/research/pingora.md` (Cloudflare Pingora — 20 lessons)
- `audit/round-8/research/envoy.md`   (Envoy — 20 lessons)
- `audit/round-8/research/haproxy.md` (HAProxy — 21 lessons)
- `audit/round-8/research/nginx.md`   (nginx — 21 lessons)
- `audit/round-8/research/hyper-h2-quinn.md` (Rust ecosystem — 30 lessons)

Total: 112 lessons across 5 references, 25 defensive patterns, 30+
explicit non-goals. Every lesson carries an upstream citation (CVE,
GHSA, CHANGELOG entry, blog post, or RFC clause).

This handoff file gives `div-l7` and `div-ops` the highest-leverage
divergence-search targets, with file paths in our tree where the
inquiry should land.

---

## Top-10 patterns `div-l7` should look for in our codebase

Each of these is a recurring CVE class across ≥2 of the references.
Finding the first one is a bug; finding all ten is an audit.

### 1. Header / framing micro-parsers handling untrusted text

- **Empty header name rejection.** HAProxy CVE-2023-25725 (Critical),
  nginx CVE-2019-9516 — *same primitive, two failure modes*. Audit
  `crates/lb-h1/src/parse.rs` and `crates/lb-h2/src/frame.rs`:
  reject zero-length name field at the *first* opportunity.
- **Content-Length digit-only parser.** hyper GHSA-f3pg-qwvg-p99c
  (`+3` accepted). Audit `crates/lb-h1/src/parse.rs`.
- **Chunk-size hex parser cap + overflow check.** nginx CVE-2013-2028,
  hyper GHSA-5h46-h7hh-c6x9, HAProxy `BUG/MAJOR: mux_h1: fix stack
  buffer overflow in h1_append_chunk_size`. Audit
  `crates/lb-h1/src/chunked.rs` — max 16 hex digits, no silent mod.
- **Transfer-Encoding folding requires `chunked` at tail.** Pingora
  GHSA-hj7x-879w-vrp7, hyper GHSA-6hfq-h8hq-87mf. Audit
  `crates/lb-security/src/smuggle.rs` + `lb-h1/parse.rs`.

### 2. Premature protocol switch on Upgrade / CONNECT

Pingora GHSA-xq2h-p299-vjwv (Critical, CVSS 9.3) and Envoy
GHSA-rj35-4m94-77jh both shipped the same bug independently: bytes
after the Upgrade / CONNECT request were forwarded to upstream
*before* the upstream's `101` / `200`. Audit the Upgrade and CONNECT
paths in `crates/lb-l7/h1_proxy.rs`, `lb-l7/ws_proxy.rs`, and any
CONNECT branch — verify forwarding gated on the upstream's success
response.

### 3. Authority component canonicalisation across protocols

HAProxy fixed empty-name truncation in H1 *and* required a separate
fix to bring H1 authority validation up to H2/H3 parity
(`BUG/MAJOR: http: forbid comma character in authority value` +
`BUG/MEDIUM: h1: Enforce the authority validation during H1
request parsing`). Audit every protocol parser
(`lb-h1`, `lb-h2`, `lb-h3`, `lb-l7/sni_authority.rs`): identical
rules for empty, comma, whitespace, control chars.

### 4. Multi-value header processing — iterate values, do not join

Envoy GHSA-ghc4-35x6-crw5 (High) — RBAC matched on comma-joined string;
duplicate headers bypassed. Audit `lb-security/*.rs` and `lb-l7` for
any place we string-join multi-value headers and then run a regex /
match / contains on the joined result. Iterate the values list
instead. Hot targets: cookie parsing, X-Forwarded-For, auth
headers, trailer.

### 5. HTTP/2 reset accounting AND server-side reset cap

CVE-2023-44487 (rapid reset) + CVE-2025-8671 (MadeYouReset) need
**both** caps:

- `max_pending_accept_reset_streams` (peer-sent RST_STREAM rate)
- `max_local_error_reset_streams` (server-emitted RST_STREAM rate)

Plus a connection-level drain when the cap is hit, plus a recursion
guard so the drain itself can't trigger more drain (Envoy
`drained_due_to_premature_resets_` cascade-fix). Audit
`crates/lb-l7/src/h2_security.rs`: our default is
`DEFAULT_SETTINGS_MAX_PER_WINDOW = 100` for both, which is *tighter*
than hyper's 20 / 1024 — operationally good, but verify all paths
that build an h2/hyper Builder actually apply our config rather than
falling through to hyper defaults (`hyper-util` adapters in
particular).

### 6. UAF after stream reset (deferred-delete model)

Envoy GHSA-84xm-r438-86px — DATA frame arriving during the deferred-
delete window re-entered the filter chain on already-destroyed
filters. Audit `crates/lb-l7/h2_proxy.rs` and `lb-l7/h2_security.rs`:
any callback path that processes frames after a stream reset must
short-circuit at the top (`if reset { return; }`), not rely on the
destructor having already run. Tokio's `Drop` ordering for nested
futures is the closest analogue and has its own footguns.

### 7. Range arithmetic overflow

nginx CVE-2017-7529 (High) — `Range:` arithmetic with attacker-
controlled 64-bit values overflowed, leaking memory. Pingora 0.8.0
changed `bytes=` alone to be invalid. Audit `crates/lb-h1` and
`crates/lb-h2` for `Range:` parsing and arithmetic — every addition
involving content-length needs checked overflow.

### 8. TLS / X.509 / SNI matchers handling embedded nulls

Envoy GHSA-rwjg-c3h2-f57p — BMPSTRING / UNIVERSALSTRING SANs were
truncated at the first null byte during UTF-8 conversion. Audit
`crates/lb-security/src/handshake.rs` and `lb-l7/src/sni_authority.rs`:
no `CStr`, no null-terminated comparator, no `as_ptr` slipping into
a string compare.

### 9. Default cache key (if/when we ship a cache)

Pingora GHSA-f93w-pcj3-rggc (High, CVSS 8.4) — the default cache
key omitted authority, enabling cross-origin poisoning. We do not
currently ship a cache (REL-2-01 removed compression and the cache
crate was never added), so this is an architectural non-goal — but
flag it in `audit/deferred.md` so any future cache work cannot ship a
silent-default key.

### 10. Custom certificate verifier — do not allow accept-all

rustls SECURITY.md explicitly discourages custom verifiers. Audit
`crates/lb-security/` for any `ServerCertVerifier` /
`ClientCertVerifier` impl. If present, it must be the opposite of
accept-all; if it must allow accept-all under a flag, the flag must
be named to make accidental use impossible.

---

## Top-5 cross-cutting items for `div-ops` (drain, observability, hot reload)

These cross the L7 / ops boundary. `div-ops` should validate.

### 1. Hot reload semantics — listener handover model

Three reference points:

- Pingora: SIGQUIT → FD passing over Unix socket, parent drains
  EXIT_TIMEOUT=300s, CLOSE_TIMEOUT=5s window for new process to bind.
  Reference: `pingora-core/src/server/mod.rs`.
- HAProxy: master/worker, SO_REUSEPORT + FD inheritance, master
  validates new config before sending the signal. Reference: HAProxy
  3.0 master/worker rework.
- Cloudflare Oxy blog: "use systemd to decouple socket lifetime from
  application lifetime"; built a config-validating Unix-socket
  coordinator rather than signal-based reload.

Audit `crates/lb-controlplane` and `lb/src/main.rs`: which model do
we adopt? Does config get pre-validated *before* the reload signal?
Are sockets inherited or rebound? What is our equivalent of EXIT_TIMEOUT?

### 2. Randomised / probabilistic drain timing

Envoy `drain_manager_impl.cc`: `P(close) = elapsed / drain_timeout`
distributes close events over the first quarter of the drain window
to avoid thundering herds. Audit our drain path — if we close
connections in a synchronous loop, restarts will cause connection
storms at every reload.

### 3. Stats / metrics continuity across reload

HAProxy 3.0: persistent stats across reloads via GUID-assigned config
objects + `dump stats-file`. Envoy hot-restart merges stats across
parent/child overlap window. *A hot reload that breaks Grafana
dashboards is a regression.* Audit `crates/lb-observability` — do
counters survive a reload? If yes, how? If no, document the
trade-off.

### 4. Frame-level (not just TCP-level) timers for HTTP/2 + QUIC

nginx ships `http2_recv_timeout` default 30s as a *frame-arrival*
deadline distinct from TCP idle. HAProxy ships
`tune.h2.fe.glitches-threshold` as a per-connection protocol-abuse
counter. Audit `crates/lb-security/slowloris.rs` and
`crates/lb-h2/security.rs`: is there a per-frame deadline and a
single observable counter for protocol abuse, or many independent
detectors? The HAProxy "glitches" pattern is the simplest operator
interface.

### 5. Defaults audit against edge-deployment guidance

Envoy edge best-practices and nginx defaults converge on:

- `max_concurrent_streams = 100-128`
- `initial_stream_window_size = 64 KiB`
- `initial_connection_window_size = 1 MiB`
- `keepalive_requests = 100` (count cap)
- `keepalive_timeout = 60-75s` (wall-clock cap)
- `headers_with_underscores = REJECT`
- `path normalisation = on` (with `merge_slashes` separate)
- `reset_timedout_connection`: ship as opt-in for RST vs FIN
  trade-off

Audit `config/` defaults and the `lb-config` schema — diverge from
this list only with documented reason. Pingora's 0.8.0 added the
keepalive-request-count cap as a *new feature*, confirming this is
the industry-standard floor.

---

## Reference / lesson map (quick lookup)

For `div-l7` writing findings, here is the citation density per
crate-relevant area:

| Area                            | Pingora | Envoy | HAProxy | nginx | hyper/h2/etc |
|---------------------------------|---------|-------|---------|-------|--------------|
| HTTP/1 smuggling                | L#2,5,8 | L#5,17| L#9-11,16| L#1-2,5 | L#1-3,13     |
| HTTP/2 reset / rapid reset      | L#4,10  | L#11  | L#13    | L#4-5 | L#4-7        |
| HTTP/2 / H3 framing             | L#11,17 | L#13,20|L#11-12,19|L#9   | L#7,9        |
| HTTP/3 / QUIC parser            | —       | —     | L#2,4-7 | L#7-10| L#16-24      |
| Cache key                       | L#1     | —     | —       | —     | —            |
| Premature protocol switch       | L#3     | L#5   | L#8     | —     | —            |
| TLS / SNI / cert verification   | —       | L#4   | —       | L#11-13| L#25-30     |
| Drain / hot reload              | L#14,18 | L#18-19| L#20   | L#16-17| —           |
| Defaults / edge hardening       | L#6,7,12,16,19 | L#12-15 | L#15-20 | L#15-20 | L#8     |
| Connection pool / reuse        | L#9,15,20 | (cited)| L#21  | L#16,17| —          |

Use this table to balance findings — if `div-l7` lands 8 findings on
HTTP/1 smuggling and zero on HTTP/3, that's a gap; HAProxy and nginx
both took multiple HTTP/3 hits and we ship H3 via quiche.

---

## What I expect `div-l7` to find that the previous audit missed

Three predictions for the team-lead (also in my final summary):

1. **Underwhelming multi-value-header policy iteration.** The Envoy
   RBAC bypass (GHSA-ghc4-35x6-crw5) is *very* recent (Mar 2026) and
   probably wasn't on Rounds 1-7's radar; our `lb-security` filters
   may join-then-match instead of iterate-and-match.

2. **Frame-level HTTP/2 abuse counter (HAProxy "glitches") is
   probably N independent counters in our `lb-h2/security.rs`, not a
   single named threshold that operators can tune.** The HAProxy
   pattern shipped specifically because operators couldn't reason
   about N knobs.

3. **Defaults gap vs. Envoy edge best-practices + nginx defaults.**
   The eight items in section 5 of the cross-cutting list above are
   the canonical edge-LB baseline; even one missing default is a
   finding.

## What I want `ref-l4` to know

The L7 references touch L4 in two places worth flagging to `ref-l4`:

1. **`SO_REUSEPORT` listener semantics.** HAProxy (master/worker),
   Envoy (one event loop per worker), and nginx (forked workers) all
   rely on `SO_REUSEPORT` for graceful reload + per-worker affinity.
   `ref-l4` should check our XDP fast path's interaction with
   `SO_REUSEPORT` group hashing — Cloudflare's Unimog / L4Drop notes
   are the canonical reference and Envoy's listener manager assumes
   the kernel's reuseport hashing places all packets of one
   4-tuple on one worker. If our XDP layer redirects packets to a
   socket, the reuseport invariant on a normal listener does not
   hold and a different socket assignment is needed.

2. **QUIC connection ID-based routing.** nginx CVE-2026-40460 (H3
   source-IP spoofing) is the bug class where a kernel-level rate
   limiter that keys on the immediate UDP socket peer is wrong for
   QUIC, because the peer address can move freely. Any L4 LB hashing
   QUIC traffic must hash on connection ID, not on (srcip, srcport).
   This is the standard Cloudflare / Facebook approach and likely
   already known to `ref-l4`, but worth confirming the call-out is
   in their brief.

---

## Source URLs (in case the WebFetch cache evicts)

The five reference files cite every URL inline. The canonical
upstream pages:

- Pingora repo + advisories tab: `github.com/cloudflare/pingora`,
  `/security/advisories`
- Envoy repo + advisories tab: `github.com/envoyproxy/envoy`,
  `/security/advisories`, `/docs/envoy/latest/configuration/best_practices/edge`
- HAProxy: `github.com/haproxy/haproxy` (CHANGELOG), `haproxy.org/news.html`,
  `haproxy.com/blog/announcing-haproxy-3-0`, `docs.haproxy.org/3.0/configuration.html`
- nginx: `nginx.org/en/security_advisories.html`, NVD CVE records
- Rust: `github.com/hyperium/{hyper,h2}/security/advisories`,
  `github.com/cloudflare/quiche/security/advisories`,
  `github.com/quinn-rs/quinn/security/advisories`,
  `github.com/rustls/rustls/security/advisories`,
  `docs.rs/hyper/latest/hyper/server/conn/http2/struct.Builder.html`.

End of L7 reference handoff.
