# nginx — production lessons (Round-8 reference brief)

Author: `ref-l7`, 2026-05-14. **Knowledge only, do not port.**
nginx is the dominant edge L7 in the wild (~30% of the top-10M web
sites). Sources: nginx.org security advisories page, NVD CVE records,
the AOSA book chapter on nginx architecture, the official docs at
nginx.org/en/docs/, and the master CVE list.

---

## Architecture summary (1 page)

- **Master / worker, event-driven.** A privileged master process forks
  unprivileged workers; each worker is a single-threaded `epoll`/`kqueue`
  loop that handles thousands of connections. Per-worker memory pools
  amortise allocation. No thread pool by default (added later as
  `aio threads`).
- **Per-worker isolation is both feature and footgun.** Pingora's
  rewrite called out that per-worker connection pools and per-worker
  request handling produce *unbalanced CPU* and *fragmented connection
  reuse* — a request landing on a worker can only use that worker's
  pool. Operationally simpler, but inefficient at scale.
- **Phase-ordered request handling.** Six processing phases: server
  rewrite → location matching → location rewrite → access control →
  try_files → content → logging. Content handlers (proxy_pass, FastCGI,
  uWSGI, static) own the response; output then traverses *response
  filters* (gzip, ssi, sub, header).
- **Memory pools, no malloc per request.** Each connection has a pool;
  pools recycle on connection close. The `ngx_pool_t` model lets nginx
  forget about per-allocation lifetime — but it leaks if you reference
  a pool object after pool destruction (#bugs).
- **Configuration is C-style nested blocks** (main, http, server,
  location). No dynamic config protocol — reload via SIGHUP.
- **Hot reload via master → worker handoff.** Master accepts SIGHUP,
  validates new config, spawns new workers bound to the same listening
  sockets, signals old workers to drain. Listening sockets are
  inherited via fork()-then-execve; no FD-passing socket needed.

---

## Lessons learned (numbered, each with citation)

1. **Stack buffer overflow in chunked transfer parsing.** CVE-2013-2028
   (Major). `ngx_http_parse_chunked` had a signedness error that let an
   attacker craft an oversized chunk-size hex string and write past the
   stack buffer. Pre-auth RCE territory. *Lesson: chunk-size hex parsing
   is the most attacked code path in HTTP/1; this exact class of bug
   has hit hyper (GHSA-5h46-h7hh-c6x9, integer overflow on chunk size)
   and HAProxy (`BUG/MAJOR: mux_h1: fix stack buffer overflow in
   h1_append_chunk_size`). Treat the chunk-size hex decoder as
   security-critical code, fuzz it specifically.* Citation: NVD
   CVE-2013-2028.

2. **Unescaped-space URI parsing access bypass.** CVE-2013-4547. An
   unescaped space in a request URI confused nginx into matching a
   shorter prefix than the rest of the URI, bypassing
   `location`-scoped access controls. *Lesson: request-line parsers
   that tolerate "extra" whitespace before the path will produce
   route-matching inconsistencies — and routing inconsistencies are
   authZ bugs by definition.* Citation: NVD CVE-2013-4547.

3. **Range filter integer overflow leaked memory.** CVE-2017-7529
   (High, CVSS 7.5). A 64-bit integer overflow when computing the
   range size let a crafted `Range:` request bypass length checks and
   read uninitialized worker memory back to the client. *Lesson:
   range-header arithmetic operates on user-controlled 64-bit
   integers; every addition must check for overflow against
   content-length.* Citation: NVD CVE-2017-7529.

4. **HTTP/2 small WINDOW_UPDATE flood (CVE-2019-9511) — CPU exhaustion
   via 1-byte chunks.** An attacker can request large objects across
   many streams, then send WINDOW_UPDATE in 1-byte increments. The
   server queues data in 1-byte chunks per stream, blowing CPU.
   *Lesson: the H2 sender side must not enqueue arbitrarily small
   outbound chunks; coalesce writes to a minimum-frame-size policy.*
   Citation: NVD CVE-2019-9511.

5. **HTTP/2 zero-length headers memory exhaustion.** CVE-2019-9516.
   Stream of headers with zero-length names and values kept the
   allocation alive for the lifetime of the session — every empty
   header still consumed a HeaderMap entry. *Lesson: empty header
   names must be rejected as a parse error; counting them toward the
   header-count budget but accepting them is half-protection.*
   Citation: NVD CVE-2019-9516. Compare with HAProxy CVE-2023-25725
   (empty header names truncated the parse) — same primitive, two
   different failure modes.

6. **DNS resolver 1-byte overwrite via spoofed UDP.** CVE-2021-23017
   (High, CVSS 7.7). The asynchronous DNS resolver had an off-by-one
   when decoding a CNAME response, allowing an attacker who could
   forge UDP from the DNS server to overflow a single byte. *Lesson:
   the DNS resolver is unauthenticated UDP from a network peer; any
   parser bug here is reachable by anyone on the path between you and
   your resolver.* Compare with Envoy GHSA-3cw6-2j68-868p (scoped
   IPv6 crash via DNS). Citation: NVD CVE-2021-23017.

7. **HTTP/3 use-after-free.** CVE-2024-24990 (High, CVSS 7.5). nginx's
   experimental HTTP/3 QUIC module had a UAF reachable by an
   undisclosed request shape. *Lesson: a new protocol module needs
   memory-safety hardening *before* it ships — even with an
   "experimental, off by default" flag, operators who turn it on are
   one CVE away from crash.* Citation: NVD CVE-2024-24990.

8. **HTTP/3 null pointer dereference.** CVE-2024-24989 (Major).
   Companion bug to #7 in the same module. *Lesson: two
   memory-safety CVEs in the same module in one release window
   indicates the module is under-tested. Quiche, h3, and quinn have a
   similar arc — fuzzing must be continuous, not just at GA.*
   Citation: NVD CVE-2024-24989.

9. **HTTP/3 encoder buffer overwrite (CVE-2024-32760) — out-of-bounds
   *write* via crafted encoder instructions.** Severity Medium but
   reachable pre-auth. *Lesson: QPACK encoder instructions are
   attacker-controlled; the decoder must bounds-check every offset.*
   Citation: NVD CVE-2024-32760.

10. **HTTP/3 source-IP spoofing bypassed authZ + rate limiting.**
    CVE-2026-40460 (Medium, CVSS 6.9). QUIC's connection-migration
    feature lets a client appear to come from a different source IP;
    nginx's address-based rules trusted the network-level address. *Lesson:
    in QUIC, the source IP is not a stable identifier; rate-limit and
    authZ must key on the connection ID (or validated original
    address), not the immediate socket address.* Citation: NVD
    CVE-2026-40460.

11. **TLS session-reuse bypassed client-cert auth.** CVE-2025-23419
    (Medium). Multiple server blocks sharing IP:port with `ssl_client_certificate`
    enabled — session tickets/cache configured on the *default* server
    let an attacker resume a session and bypass per-server cert
    requirements. *Lesson: TLS session tickets must be scoped to the
    SNI/SAN they were issued for, and the resumed session must re-check
    the per-server policy.* Citation: NVD CVE-2025-23419.

12. **SSL upstream injection from MITM.** CVE-2026-1642 (CVSS 4.0 = 8.2
    High). When proxying to a TLS upstream, a MITM could inject
    plaintext into the response stream. The root cause is around how
    nginx wrapped/unwrapped TLS records and how the framing was
    revalidated after upstream split-flow. *Lesson: even when
    upstream is TLS, you cannot trust framing boundaries; integrate
    the TLS record demarcation with the HTTP framing layer rather
    than treating them independently.* Citation: NVD CVE-2026-1642.

13. **OCSP heap-UAF in resolver path.** CVE-2026-40701. With
    `ssl_verify_client on/optional` and `ssl_ocsp on` + a resolver,
    an unauthenticated remote could trigger heap-UAF in the OCSP
    response flow. *Lesson: OCSP combines two adversarial inputs
    (the responder over the network, the client's certificate) — the
    state machine here is a magnet for UAFs. Treat it like a parser
    target.* Citation: NVD CVE-2026-40701.

14. **HTTP/2 proxy_http_version=2 + proxy_set_body allowed frame
    injection.** CVE-2026-42926. A configuration meant for advanced
    use cases let an attacker inject frame headers and payload bytes
    into the upstream stream. *Lesson: any feature that lets the
    operator drop down to writing raw bytes into a multiplexed
    protocol is a vulnerability primitive. Validate the constructed
    bytes against the framing layer's escape rules; don't trust the
    operator config implicitly.* Citation: NVD CVE-2026-42926.

15. **`large_client_header_buffers` default `4 8k` is a deliberate
    knob — operators raise it for SAML / JWT-heavy traffic.** The
    default is small *on purpose* to defeat header-flood attacks but
    will reject legitimate traffic from some apps. *Lesson: header
    buffer caps are a deployment knob, not a security boundary; set
    your default tight but make the knob first-class.* Citation:
    nginx.org `large_client_header_buffers` directive.

16. **`keepalive_requests = 100` default — the count cap that
    Pingora 0.8.0 later added.** nginx enforces a per-keepalive
    request count *and* a per-keepalive wall-clock timeout
    (`keepalive_timeout = 75s`). The same defence applied
    independently to two limits. *Lesson: shipping both a count and
    a wall-clock keepalive cap is the established edge-LB norm.
    Pingora's 0.8.0 add is the late catch-up.* Citation: nginx.org
    `keepalive_requests` / `keepalive_timeout`.

17. **`lingering_close = off` by default; `lingering_timeout = 30s`.**
    When nginx closes a connection, by default it does NOT linger to
    read trailing data from the client. Operators must enable
    lingering for clients that pipeline request after a
    Content-Length post. *Lesson: lingering close is a known footgun
    for memory and FD accounting; nginx defaults to off and lets
    operators opt in.* Citation: nginx.org `lingering_close`.

18. **`http2_max_concurrent_streams = 128` default — almost
    identical to Envoy's recommended 100.** *Lesson: the H2 spec's
    "no default upper bound" is unsafe; ship a low default at the
    edge.* Citation: nginx.org `ngx_http_v2_module`.

19. **`http2_recv_timeout = 30s` default.** Independent of TCP-level
    idle timeout, this puts a frame-arrival deadline that defeats
    H2 Slowloris. *Lesson: H2 needs an H2-specific frame-arrival
    deadline; the per-stream and per-connection TCP timers are not
    the same thing.* Citation: nginx.org `http2_recv_timeout`.

20. **`reset_timedout_connection = off` default — RST vs FIN choice.**
    On timeout, nginx sends FIN by default, which still incurs
    TIME_WAIT. Enabling this directive sends RST to skip TIME_WAIT
    and release the socket immediately. *Lesson: under sustained
    attack, TIME_WAIT accumulates and FDs are exhausted; the RST
    behavior is the right edge default — but nginx made it opt-in to
    avoid surprising clients.* Citation: nginx.org
    `reset_timedout_connection`.

21. **Memory pool model = no per-allocation leak, but holding a
    pointer past pool destruction = UAF.** This recurs across the
    nginx CVE list. *Lesson: pool allocators trade per-allocation
    accounting for whole-pool lifetime; every CVE involving "use
    after free" in nginx ultimately traces to a pointer that
    out-lived its pool.* Citation: AOSA book chapter on nginx
    architecture; multiple HTTP/3 CVEs.

---

## Defensive patterns worth comparing against our code

1. **Per-connection HTTP/2 frame-arrival timer (`http2_recv_timeout`).**
   Independent of TCP idle and request idle. We should check
   `lb-h2/lib.rs` and `lb-security/slowloris.rs` for a *frame-level*
   timer, not just a stream timer.

2. **`headers_with_underscores` direction.** nginx's `underscores_in_headers`
   defaults to OFF (i.e. reject) — same as Envoy's edge guidance. Check
   our `lb-h1/parse.rs` default.

3. **Range-header arithmetic must check for overflow.** Lesson 3 above.
   Verify `lb-h1` and `lb-h2` reject `Range: bytes=0-9223372036854775807`
   style requests.

4. **Lingering close as an opt-in directive.** Lesson 17. We should
   document whether we linger and on what footprint.

5. **`reset_timedout_connection` as an opt-in for RST vs FIN.**
   Lesson 20. We should expose the choice and document the TIME_WAIT
   tradeoff.

---

## Explicit non-goals / what nginx chose NOT to do

- **No native multithreaded request handling.** Threads are only used
  for blocking I/O via `aio threads`. The per-worker single-threaded
  model is intentional. Pingora's rewrite specifically targets this
  decision; we should not penalise our LB for choosing one or the
  other if the docs are honest.
- **No first-class hot-config-via-API.** Config reload is SIGHUP +
  fork; no xDS, no admin API for routes. Operators script around it.
- **No first-class metrics endpoint in OSS.** Prometheus exporter is
  third-party (`nginx-prometheus-exporter`). The official offering is
  `stub_status` which exports four counters.
- **No native HTTP/3 GA — it remained "experimental" through 1.26.x.**
  Multiple HTTP/3 CVEs in 2024 (CVE-2024-24989/24990/31079/32760/35200/34161)
  confirm the maturity gap.
- **No mTLS-to-upstream by default; deliberately optional.** Upstream
  TLS is configured per `proxy_ssl_*` block; the CVE-2025-23419
  session-reuse bypass confirms it's a deliberately separate concern.
- **No connection pool *sharing* across workers.** Per-worker pools
  are the architectural choice Pingora explicitly rejected. nginx
  accepts the reuse-rate cost.

---

## Direct comparison ideas for `div-l7`

- **L7-W (nginx L#1, L#3):** Audit chunked-size hex parsing and range
  arithmetic in `lb-h1/chunked.rs` and `lb-h1/parse.rs`. Both are
  recurrent CVE classes; fuzz-test specifically.
- **L7-X (nginx L#2, L#10):** Routing-key validation. In `lb-l7`'s
  router (if any), verify the request URI is canonicalised *before*
  the route-matching ACL — including whitespace, Unicode normalisation,
  and path-confusion edge cases. For H3, verify routing keys on
  *validated original address*, not the immediate UDP socket peer.
- **L7-Y (nginx L#5, vs HAProxy CVE-2023-25725):** Empty header name
  rejection. Two CVEs at two big proxies on the same primitive — must
  be a hard reject early in our parser.
- **L7-Z (nginx L#6):** DNS resolver fuzzing. We don't appear to do
  custom DNS resolution (we use system stub), but `lb-controlplane` or
  `lb-cp-client` may invoke a resolver; if it parses DNS itself, fuzz it.
- **L7-AA (nginx L#11):** TLS session ticket scoping. In
  `lb-security/ticket.rs` confirm tickets are bound to (SNI, ALPN,
  policy) and that a resumed session re-runs per-listener policy.
- **L7-AB (nginx L#15-20):** Defaults audit again. Our `config/`
  defaults must match or exceed nginx's tightening: header buffer caps,
  keepalive request count + wall-clock cap, H2-specific recv timeout,
  `reset_timedout_connection` semantics for FD reclamation under
  attack.

This is the fourth batch of homework for `div-l7`.
