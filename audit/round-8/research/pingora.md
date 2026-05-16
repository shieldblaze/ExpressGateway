# Pingora — production lessons (Round-8 reference brief)

Author: `ref-l7`, 2026-05-14. **Knowledge only, do not port.**
Pingora is the Rust HTTP proxy framework Cloudflare runs in front of its
fleet. Public artifacts used here: the open-source repo
(`github.com/cloudflare/pingora`), the security-advisories tab, the
launch blog post, and the user guide.

---

## Architecture summary (1 page)

- **Library, not application.** Cloudflare ships Pingora as "the engine
  that powers a car, not the car itself" — `pingora-core` exposes
  `Server` + `ServerApp`, and downstream consumers compose proxies on
  top (`pingora-proxy` is one such consumer). The repo splits the
  surface into `pingora-core` (runtime, listener, signal handling),
  `pingora-proxy` (HTTP proxy phases / filter callbacks), `pingora-http`
  (case-preserving header types), `pingora-pool` (lock-free hot pool
  for per-peer connection reuse), `pingora-cache`, `pingora-rustls`,
  `pingora-boringssl`, and `pingora-timeout`.
- **Multithreaded Tokio, work-stealing.** Cloudflare chose threads over
  processes specifically so the connection pool can be *shared*; nginx's
  per-worker pool was the operational pain point that justified the
  rewrite. (See the launch blog: 87.1% → 99.92% reuse on the migration.)
- **Phase / filter request lifecycle.** Mirrors OpenResty's phases
  (request_filter, upstream_peer, upstream_request_filter,
  upstream_response_filter, response_body_filter, logging) so application
  code can hook each transition without touching proxy mechanics.
- **Custom HTTP, not hyper.** They built their own H1 parser to accept
  non-RFC-compliant traffic from the real internet (status codes
  599-999, garbage cases nginx historically tolerated). H2 wraps the
  `h2` crate. H3 / QUIC arrived later; the public tree does not yet show
  full H3 server parity.
- **Hot upgrade by FD passing.** `Server::run_forever` listens for
  SIGQUIT (graceful upgrade) and SIGTERM (graceful shutdown); on SIGQUIT
  the old process sends listening FDs to the new process over a Unix
  socket configured via `upgrade_sock`, then enters drain for
  `EXIT_TIMEOUT = 300s` (default). `CLOSE_TIMEOUT = 5s` is the window
  the old process waits for the new one to come up before letting go.

---

## Lessons learned (numbered, each with citation)

1. **The default cache key shipped without authority — silent
   cross-origin cache poisoning.** GHSA-f93w-pcj3-rggc / CVE-2026-2836
   (High, CVSS 8.4). Pingora versions <0.8.0 had a default `CacheKey`
   that used only URI path. Multi-tenant deployments could serve
   tenant-A's cached response to tenant-B. Cloudflare's resolution was
   to *remove the default impl entirely* and force every embedder to
   construct a key explicitly with host/authority + upstream TLS scheme
   + method. *Lesson: never ship a one-line default for a primitive
   whose wrong answer is silent.* Citation:
   `github.com/cloudflare/pingora/security/advisories/GHSA-f93w-pcj3-rggc`.

2. **HTTP/1.0 was being close-delimited and multiple Transfer-Encoding
   headers were misparsed — request smuggling.** GHSA-hj7x-879w-vrp7
   (Critical). RFC 9112 says bodies are *never* close-delimited on
   request; Pingora <0.8.0 allowed it on HTTP/1.0 and misparsed
   `Transfer-Encoding: a, chunked` style headers. The fix landed across
   three commits enforcing strict message-length parsing. *Lesson: TE
   parsing must reject anything that doesn't fold to a single `chunked`
   token at the tail, and HTTP/1.0 requests must require explicit CL or
   be assumed zero-body.* Citation:
   `github.com/cloudflare/pingora/security/advisories/GHSA-hj7x-879w-vrp7`.

3. **Premature Upgrade forwarding allowed h2c-style smuggling.**
   GHSA-xq2h-p299-vjwv (Critical, CVSS 9.3). Pingora <0.8.0 began
   forwarding bytes after an Upgrade request *before* receiving `101
   Switching Protocols` from the upstream. Attackers could send a
   second HTTP request after the Upgrade headers and have it routed
   bypassing proxy ACLs. The fix is to keep parsing inbound bytes as
   HTTP/1 until and unless the *upstream* responds 101. *Lesson: the
   protocol switch is decided by the upstream, not the client.*
   Citation:
   `github.com/cloudflare/pingora/security/advisories/GHSA-xq2h-p299-vjwv`.

4. **MadeYouReset HTTP/2 — buffers allocated before reset processing.**
   GHSA-393w-9x6h-8gc7 / CVE-2025-8671 (High). Pingora <0.6.0 allocated
   per-stream buffers before draining RST_STREAM signalling; attackers
   could open and reset streams faster than memory was reclaimed. The
   fix was an h2-dep bump that releases connection resources before the
   buffers accumulate. *Lesson: stream reset accounting must run before
   any per-stream allocation in the data path.* Citation:
   `github.com/cloudflare/pingora/security/advisories/GHSA-393w-9x6h-8gc7`.

5. **Cache hit → request smuggling.** GHSA-93c7-7xqw-w357 / fix commit
   `fda3317` (High, CVSS 7.4). On cache hits, Pingora's caching path
   would still consume request body bytes inconsistently versus how the
   upstream framing would have interpreted them; subsequent requests on
   the keepalive connection could be smuggled. *Lesson: cache-hit short
   circuits must still drain and validate the request body framing as
   if the upstream had handled it; never skip the body validation step.*
   Citation:
   `github.com/cloudflare/pingora/security/advisories/GHSA-93c7-7xqw-w357`.

6. **CONNECT proxying enabled by default was a feature, then a
   liability.** 0.8.0 changelog entry: "CONNECT method proxying
   disabled by default; requires explicit opt-in via server options."
   Pingora removed the default-on because operators were unaware that
   exposing a public proxy meant turning their LB into an open relay.
   *Lesson: defaults must be the secure footgun-free behaviour; surface
   the opt-in.* Citation: `pingora/CHANGELOG.md` 0.8.0 breaking changes.

7. **`bytes=` alone is not a Range header.** 0.8.0 breaking change. A
   degenerate `Range: bytes=` (no spec) was previously accepted; this
   is the kind of edge case that produces cache poisoning when two
   parsers disagree. *Lesson: range parsers must reject the empty
   ranges-specifier per RFC 7233 §2.1.* Citation: CHANGELOG 0.8.0.

8. **HTTP/1 downstream session reuse on over-read leaked body bytes
   into the next request.** 0.8.0 fix: "Ensure http1 downstream session
   is not reused on more body bytes than expected." If a client sent
   more body than `Content-Length` advertised, the leftover bytes used
   to stay in the buffer for the *next* request on the same connection
   — classic smuggling primitive. *Lesson: on body over-read, mark the
   session unreusable and close.* Citation: CHANGELOG 0.8.0.

9. **Stream reuse on errors prior to upstream connect.** 0.5.0 fix:
   "Allow reusing session on errors prior to proxy upstream." The
   inverse of #8 — Pingora was being *too* eager to close downstream
   sessions when an upstream pool lookup failed, hurting reuse rate.
   The lesson is that the criterion for reuse is "did the wire framing
   stay coherent", not "did the request succeed". Citation: CHANGELOG
   0.5.0.

10. **H2 client-side read timeouts must cancel via RST_STREAM, not just
    drop the future.** 0.8.0 fix: "Send RST_STREAM CANCEL on
    application read timeouts for h2 client." Otherwise the upstream
    keeps shoveling bytes into a closed stream, wasting upstream socket
    buffer and (worse) keeping the connection slot occupied. *Lesson:
    H2 upstream cancel = explicit RST_STREAM(CANCEL) + mark the
    connection unreusable if the timeout suggests it hung.* Citation:
    CHANGELOG 0.8.0; `proxy_h2.rs` upstream-read-timeout branch.

11. **HTTP/2 panic on multi-stream concurrent use.** 0.4.0 fix: "Fixed
    a panic when using multiple H2 streams in the same H2 connection to
    upstreams." Cloudflare's h2 pool was reusing one mux for multiple
    request actors; concurrent body writes raced and panicked. *Lesson:
    every H2 stream must own its h2-send handle; share only the
    multiplexer.* Citation: CHANGELOG 0.4.0.

12. **Limit the number of downstream session reuses.** 0.8.0 feature:
    "Add the ability to limit the number of times a downstream
    connection can be reused." Even with perfect framing, very long
    keepalive lifetimes accumulate state (TLS session, accounting,
    per-FD memory) that you cannot reclaim. *Lesson: keepalive must
    have both a wall-clock *and* a request-count cap; nginx uses
    `keepalive_requests=100` by default for the same reason.*
    Citation: CHANGELOG 0.8.0.

13. **Non-RFC status codes 599-999 exist in the wild.** Launch blog
    explicitly calls this out as the reason Cloudflare did not adopt
    hyper as the HTTP engine: hyper rejects status codes outside RFC
    9110's 100-599 range; Cloudflare needed to *forward* the violations
    rather than convert them into 502s. *Lesson: a transparent proxy
    must not normalise wire violations unless explicitly configured to.*
    Citation: `blog.cloudflare.com/how-we-built-pingora-the-proxy-that-connects-cloudflare-to-the-internet/`.

14. **GOAWAY on graceful shutdown for H2 must drain streams, not yank
    the TCP socket.** 0.4.0 fix: "Shutdown h2 connection gracefully
    with GOAWAYs", and the related "Retry all h2 connection when
    encountering graceful shutdown." Before the fix, in-flight requests
    on an upstream H2 connection that was being upgraded were lost.
    *Lesson: H2 graceful = GOAWAY with last-stream-id, then drain in
    flight, then close.* Citation: CHANGELOG 0.4.0.

15. **Connection pool is hot/cold tiered for lock-contention.**
    `pingora-pool` docs note: "Each connection group has a lock free
    hot pool to reduce the lock contention when some connections are
    reused and released very frequently." Naive Mutex<Vec<Conn>> design
    became a profiler hotspot before they split into hot (atomic,
    SPSC-ish) and cold (mutex-protected overflow). *Lesson: connection
    pool concurrency is a real workload; benchmark under
    take-and-return-immediately patterns.* Citation: `pingora-pool/src/lib.rs`.

16. **Header struct caps at 4096 — way less than the http crate's
    32768.** Pingora's `RequestHeader` enforces a 4,096-header cap
    explicitly. Comment in source: "Any way you cut it, 4,096 headers
    is insane." *Lesson: don't inherit the crate-level upper bound;
    pick a deployment-level one and reject above it before mutation.*
    Citation: `pingora-http/src/lib.rs`.

17. **HTTP/1 → HTTP/1 forwarding must inject `Transfer-Encoding:
    chunked` when downstream is H/1.0 lacking CL.** `proxy_h1.rs:62-65`,
    `h1_response_filter` 862-923. The bug it prevents: a request body
    arrives with no CL via H/2 and is forwarded to a strict H/1
    upstream, which then close-delimits — i.e. exactly the bug
    GHSA-hj7x-879w-vrp7 fixes from the other side. *Lesson: framing
    must be re-derived when crossing protocol versions.* Citation:
    `pingora-proxy/src/proxy_h1.rs`.

18. **Server::run_forever now takes ownership and enforces exit
    semantics.** 0.2.0 breaking change. Earlier versions allowed
    accidentally dropping the server before shutdown completed, leaving
    listeners in inconsistent state. *Lesson: graceful shutdown is a
    state machine; making the entry function consume `self` prevents
    "I already drained, why am I dropping again" footguns.* Citation:
    CHANGELOG 0.2.0.

19. **Persisting keepalive_timeout *between* requests on the same
    stream.** 0.6.0 fix. Without this, every request on a long
    keepalive recomputed the timer from the last header arrival, which
    let abusive clients hold a connection open by trickling a request
    every N-1 seconds where N was the configured timeout. *Lesson:
    keepalive timer must be a *connection-level* deadline, not
    re-armed per request.* Citation: CHANGELOG 0.6.0.

20. **Discard extra upstream body and disable keepalive.** 0.6.0 fix.
    When upstream sends more body than its own CL advertised, Pingora
    used to keep the upstream connection in pool. The fix discards the
    overflow body bytes *and* removes the connection from pool. *Lesson:
    the upstream is the smuggling primitive too; mistrust both ends.*
    Citation: CHANGELOG 0.6.0.

---

## Defensive patterns worth comparing against our code

1. **Case-preserving header storage with a parallel CaseMap.**
   `pingora-http/src/lib.rs` keeps headers in a normal `HeaderMap` for
   lookup but stores a separate `HMap<CaseHeaderName>` so wire output
   preserves the casing the client sent. We use `http::HeaderMap`, which
   discards case. If we ever proxy to backends that rely on case
   (notoriously: some Java middleware, SAP, weird cloud signing), we
   will be the bug. Worth a check in `lb-h1` and `lb-l7/h1_to_h1.rs`.

2. **Connection-pool hot/cold tier.** `pingora-pool` separates lock-free
   hot pool from mutex-protected cold pool. Our `lb-l7/upstream.rs`
   should be checked: do we have a single `DashMap` that serializes hot
   reuse? If so, a per-peer ringbuffer-of-N is the standard fix.

3. **Reuse limit (count + wall-clock).** 0.8.0's per-connection reuse
   cap is the defence against the long-tail memory accumulation in
   keepalive. We should look at `lb-h1` and `lb-h2` server impls for
   *both* request-count and absolute-deadline caps on the downstream
   connection.

4. **Drain timeout for partial request body before keepalive reuse.**
   Pingora's `total_drain_timeout` exists because a slowloris-on-body
   would otherwise pin a downstream session forever. Map this to
   `lb-security/slow_post.rs` — does it actually force-close after the
   configured threshold, or only mark and continue?

5. **RST_STREAM(CANCEL) on H2 upstream read timeout + mark connection
   unreusable.** Pingora explicitly does both. Worth checking
   `lb-l7/h2_to_h2.rs` and `lb-l7/h1_to_h2.rs` — do we send an explicit
   reset and evict the H2 multiplexer from the pool?

---

## Explicit non-goals / what Pingora chose NOT to do

- **No built-in admin REST API.** Pingora is a *library*; observability
  is via Prometheus + tracing, not via an admin endpoint à la Envoy.
  Don't fault our LB if it elects the same minimalism — though we *do*
  expose `/metrics` and `/admin` according to docs.
- **No xDS / dynamic config protocol.** Configuration is whatever the
  embedder wires up. Cloudflare runs their own control plane out of
  tree.
- **No portable Windows / non-Unix support.** Roadmap explicitly excludes
  this. "Non-Unix operating systems are not currently on the roadmap."
- **No transparent compression / decompression.** Pingora forwards
  bodies; if the embedder wants gzip/brotli, they do it in a phase
  callback. (Mirrors our REL-2-01 "compression removed because
  pass-through".)
- **Does not normalise non-RFC status codes / unusual headers.** It is a
  transparent forwarder; the embedder is expected to enforce stricter
  policy if needed.
- **No native HTTP/3 server in the open-source release as of the
  scanned tree.** The QUIC story upstream is incomplete; H3 termination
  needs a separate engine (Cloudflare ships their own internal one).

---

## Direct comparison ideas for `div-l7`

Things to grep our code for, with the matching Pingora lesson number:

- **L7-A (Pingora L#1, L#5, L#19, L#20):** In `lb-l7/h1_proxy.rs` and
  `h1_to_h1.rs`, verify (a) over-read of CL body forces session
  unreusable, (b) cache-hit short-circuit still drains downstream body
  framing, (c) keepalive timer is a connection-level deadline rather
  than reset on each header, (d) over-read of upstream CL evicts the
  upstream from pool.
- **L7-B (Pingora L#2):** Search `lb-h1/parse.rs` and
  `lb-security/smuggle.rs` for the canonical TE-CL conflict resolution:
  TE present + valid `chunked` tail → drop CL and disable keepalive on
  framing error; TE present + non-chunked tail → reject.
- **L7-C (Pingora L#3):** Find the Upgrade handling in `lb-l7` and
  verify byte forwarding only after upstream 101.
- **L7-D (Pingora L#4, L#10, L#11):** In `lb-h2`, confirm RST_STREAM
  accounting is *before* per-stream alloc (MadeYouReset), and that
  upstream H2 read timeouts emit explicit RST_STREAM(CANCEL).
- **L7-E (Pingora L#6, L#7):** Audit our default config (`config/`) for
  any feature that ships on-by-default that has open-relay or
  cache-poisoning consequences (CONNECT, `Range: bytes=`).
- **L7-F (Pingora L#12):** Verify both per-connection request count and
  wall-clock keepalive caps are *configured* and *enforced* in `lb-h1`
  and `lb-h2`.
- **L7-G (Pingora L#16):** Check our header-count limit in `lb-h1` and
  `lb-h2/frame.rs` — is it deployment-policy (~256-1024), or did we
  inherit `http::HeaderMap`'s 32k?
- **L7-H (Pingora L#17):** Cross-protocol H2→H1 framing — does
  `lb-l7/h2_to_h1.rs` add chunked when downstream lacks CL but the
  body is unknown-length?

These are the divergence-analyst homework items.
