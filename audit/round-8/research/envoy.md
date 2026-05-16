# Envoy — production lessons (Round-8 reference brief)

Author: `ref-l7`, 2026-05-14. **Knowledge only, do not port.**
Envoy is the de-facto L7/L4 data plane in service-mesh deployments
(Istio, Consul, AWS App Mesh) and standalone edge proxies. Sources used:
`github.com/envoyproxy/envoy` source tree, security advisories tab,
official architecture docs, threat-model page, edge best-practices.

---

## Architecture summary (1 page)

- **Listener manager + filter chain + connection manager.** Listeners
  accept (TCP/UDP/QUIC) and run *listener filters* (TLS inspector,
  original-destination, original-src) before handing the socket to a
  *network filter chain* (HCM for HTTP, TcpProxy for L4 pass-through,
  PostgresProxy, Kafka, etc.). HTTP connection manager is itself an
  L7 stack with *HTTP filters* (router, rbac, jwt, ext-authz, lua,
  compression, ratelimit, etc.).
- **xDS dynamic config.** All listener / route / cluster / endpoint /
  secret config is delivered dynamically over LDS/RDS/CDS/EDS/SDS. xDS
  is delivered *out of band* — the data path is independent of the
  control plane's availability.
- **Watermark-based flow control.** Every buffer in Envoy has high/low
  watermarks. Hitting the high watermark calls a chain of callbacks that
  ultimately disables reads upstream (TCP `readDisable(true)`, H2 stops
  WINDOW_UPDATE). Reads resume when the buffer drains under the low
  watermark and *all* `readDisable` callers release.
- **Hot restart by socket sharing.** Parent and child overlap; sockets
  are passed via shared memory / FD-passing. Stats are merged across
  parent/child during the overlap window.
- **Threading: one event loop per worker.** Each listener accepts on
  every worker via `SO_REUSEPORT`. State is sharded per-worker;
  cross-worker coordination is the exception (cluster manager updates
  use post-back to all workers).
- **Overload manager.** Independent subsystem that watches heap / CPU /
  FD count and selectively disables features (reject new connections,
  shed requests, stop accepting H2 streams) when thresholds trip.

---

## Lessons learned (numbered, each with citation)

1. **HTTP filter chain ran after stream reset → UAF.** GHSA-84xm-r438-86px
   (Moderate, Mar 2026). On HTTP/2 reset, Envoy set
   `state_.saw_downstream_reset_ = true`, called `onDestroy()` on
   filters, scheduled `ActiveStream` for *deferred* deletion. If a DATA
   frame arrived during the deferred-delete window,
   `FilterManager::decodeData` did *not* check the reset flag and
   re-invoked filter callbacks on already-destroyed filter objects.
   Patch: explicit `if (state_.saw_downstream_reset_) return;` at the
   top of `decodeData`. *Lesson: deferred-delete is a state-machine
   primitive, not a memory primitive. Anything that can fire after the
   "logical death" of a stream must short-circuit, not assume the
   destructor has already run.* Citation: advisory text +
   `source/common/http/filter_manager.cc`.

2. **TCP connection pool crashed on slow-client-closes-after-large-request.**
   GHSA-pq33-4jxh-hgm3 / CVE-2025-62409 (High, Oct 2025). The proof of
   concept was a slow client that sent a large request, then closed the
   socket while upstream data was still being received. The
   buffer-watermark callback was nullptr at the moment of close,
   causing a nullptr deref. *Lesson: every watermark callback registry
   must be explicitly cleared on connection teardown — and the close
   path must run before any further read drives a callback invocation.*
   Citation:
   `github.com/envoyproxy/envoy/security/advisories/GHSA-pq33-4jxh-hgm3`.

3. **RBAC header check bypassed by repeating the header.** GHSA-ghc4-35x6-crw5
   (High, Mar 2026). Envoy's RBAC filter checked the *concatenated*
   value (comma-joined). `internal: true` against an exact-match
   "Deny" rule was blocked; `internal: true` repeated twice became
   `"true,true"` which did not match the rule. Patch:
   per-value validation. *Lesson: any policy filter that consumes
   multi-value headers must iterate the values list, not the joined
   string. This is one of the most pervasive proxy bugs in the
   industry.* Citation: GHSA-ghc4-35x6-crw5 + `HeaderMap` joiner code
   (`delimiterByHeader` returns ",").

4. **TLS SAN matcher truncated at embedded null byte.** GHSA-rwjg-c3h2-f57p
   (Moderate, Dec 2025). When SANs were encoded as BMPSTRING /
   UNIVERSALSTRING, UTF-8 conversion produced a Rust/C string that
   truncated at the first `\0`. A cert with SAN `victim\0attacker.tld`
   matched `match_typed_subject_alt_names: exact: "victim"`. *Lesson:
   when validating X.509 names, do not pass through any
   null-terminated-string API; compare by length-prefixed bytes.*
   Citation: GHSA-rwjg-c3h2-f57p.

5. **CONNECT tunnel bytes were forwarded *before* the upstream 2xx
   response.** GHSA-rj35-4m94-77jh (Low, Dec 2025). RFC 7231 says
   tunnel mode activates only after a 2xx response from the upstream;
   Envoy forwarded eagerly. If upstream returned non-2xx, the tunnel
   was already corrupted by client bytes. Patch added runtime flag
   `envoy.reloadable_features.reject_early_connect_data`. *Lesson:
   exactly the same shape as Pingora's premature-Upgrade smuggling
   (GHSA-xq2h-p299-vjwv) — the protocol switch is decided by the
   upstream, never by the client.* Citation: GHSA-rj35-4m94-77jh.

6. **JsonEscaper off-by-one wrote past `result.size()`.**
   GHSA-56cj-wgg3-x943 / CVE-2026-26309 (Moderate, Mar 2026). When the
   *last* input byte was a control character (`\x00..\x1f`), the
   pre-allocated buffer was filled to capacity by the `\uXXXX` sprintf,
   and the trailing backslash write went out of bounds. Reachable via
   invalid-header reporting. *Lesson: pre-sized output buffers are not
   automatically safe — the per-character path needs to assert the
   write position is in range, or always write through a checked
   append API.* Citation: GHSA-56cj-wgg3-x943.

7. **DNS scoped-IPv6 address crashed `getAddressWithPort`.**
   GHSA-3cw6-2j68-868p / CVE-2026-26310 (Moderate, Mar 2026). Scoped
   IPv6 (`fe80::1%eth0`) crashed Envoy when fed via DNS resolution or
   the original-source filter. *Lesson: parsing user-controlled DNS
   responses requires the same fuzzing rigour as parsing HTTP. Anyone
   who can poison a DNS lookup can crash you.* Citation:
   GHSA-3cw6-2j68-868p.

8. **JWT filter re-entrancy crashed on multiple Authorization
   headers + remote JWKS failure.** GHSA-mp85-7mrq-r866 (Moderate, Dec
   2025). With `allow_missing_or_failed` enabled, two Authorization
   headers triggered a second async JWKS fetch whose `reset()` cleared
   state from the *first* fetch's callback, crashing on response.
   *Lesson: async re-entry in security filters needs explicit per-call
   state; never reset a shared callback by side effect.* Citation:
   GHSA-mp85-7mrq-r866.

9. **Rate-limit filter dual-phase crashed when request-phase failed
   directly.** GHSA-c23c-rp3m-vpg3 / CVE-2026-26330 (Moderate, Mar
   2026). Combining a request-phase RL with a response-phase RL set to
   `apply_on_stream_done` while the RL service was unreachable left
   the gRPC client's inner state un-cleaned-up; the response-phase
   callback then accessed freed state. *Lesson: every filter that
   spawns a sub-request must clean up its sub-state in both success
   and failure branches.* Citation: GHSA-c23c-rp3m-vpg3.

10. **Lua-modified response body that grew over the buffer limit caused
    a UAF.** GHSA-gcxr-6vrp-wff3 (Moderate, Oct 2025). When the script
    inflated the body past `per_connection_buffer_limit_bytes` (default
    1MB), Envoy generated a local reply whose headers overwrote the
    originals, leaving dangling references on every other consumer of
    the original headers. *Lesson: any synchronous transform that can
    swap out headers/body must update every weak reference, or there
    must be a clear "the originals are gone now" event the rest of the
    pipeline observes.* Citation: GHSA-gcxr-6vrp-wff3.

11. **HTTP/2 rapid reset DoS.** CVE-2023-44487, fixed via h2-codec
    updates and the `premature_reset_total_stream_count` /
    `premature_reset_min_stream_lifetime_seconds` runtime knobs. Envoy
    added an `isPrematureRstStream()` predicate that compares stream
    lifetime against a threshold; an accumulator decides when to drain
    the connection. *Lesson: H2 concurrent-stream limit is necessary
    but not sufficient — rapid reset bypasses it; you need
    reset-per-second accounting and connection-level abort.* Citation:
    `conn_manager_impl.cc::isPrematureRstStream` (line ~1233) +
    `maybeDrainDueToPrematureResets` (line ~1251); the
    `drained_due_to_premature_resets_` recursion guard at lines
    1258-1267 is itself a hard-learned lesson from a follow-up cascade
    bug.

12. **Envoy's default settings are explicitly NOT safe for
    availability.** Threat-model doc: "We do not currently consider
    the default settings for Envoy to be safe from an availability
    perspective." Out-of-scope: brute-force CPU/memory attacks below a
    100x amplification threshold; missing safe defaults for
    watermarks, overload managers, circuit breakers. *Lesson: shipping
    only "secure-by-default" framework code is one option;
    explicitly-documented "you must configure overload/limits" is
    another. Whichever your project chooses, the docs must say so
    loudly.* Citation: Envoy threat-model page.

13. **Edge best-practices: H2 max_concurrent_streams = 100,
    initial_stream_window_size = 64KiB, initial_connection_window_size
    = 1MiB.** These are *Envoy's* recommended numbers for
    edge-exposed deployments. *Lesson: the H2 spec defaults
    (`INITIAL_WINDOW_SIZE=64KB`, `MAX_CONCURRENT_STREAMS` unlimited)
    are not safe for an untrusted client.* Citation: Envoy
    `configuration/best_practices/edge`.

14. **`headers_with_underscores_action = REJECT_REQUEST` recommended
    at the edge.** Headers with underscores are not RFC-violating but
    they are widely treated as equivalent to dashes by some
    application servers, so `X-Internal_Token` could be passed off as
    `X-Internal-Token` to bypass an auth check. *Lesson: edge proxies
    should reject underscore-headers by policy, not normalise them.*
    Citation: same edge best-practices page.

15. **Path-confusion mitigations: `normalize_path`, `merge_slashes`,
    `path_with_escaped_slashes_action`.** Envoy's edge docs make these
    mandatory. *Lesson: any time the proxy and the backend disagree on
    canonical path, you have an authZ bypass; canonicalisation must be
    done at the proxy, and the canonical form must match what the
    backend will compute.* Citation: same.

16. **Header byte size has a *cached* invariant
    (`cached_byte_size_`).** This exists to make over-limit checks
    O(1) instead of O(headers); the existence of
    `verifyByteSizeInternalForTest()` tells us that cache invariant
    has been wrong before. *Lesson: when you cache an aggregate, you
    must invariant-test it under modification.* Citation:
    `source/common/http/header_map_impl.cc`.

17. **HTTP/1 codec rejects TE + CL combination unless
    `allow_chunked_length_` is explicitly set.** Lines 1045-1067 of
    `http1/codec_impl.cc`. *Lesson: this is the canonical
    request-smuggling defense; the opt-out is well-named and
    documented.* Citation: same file.

18. **Hot restart: parent/child overlap window with stats merging.**
    Envoy's hot-restart design has parent and child *both* serving for
    a configurable overlap; counters are summed across them so
    monitoring is continuous. *Lesson: hot restart is not "stop, then
    start"; it is "start, drain in parallel, then stop" — and your
    observability must understand the overlap.* Citation: Envoy
    hot-restart architecture doc (medium blog mirror).

19. **Drain is *randomised* per connection, not synchronous.**
    `drain_manager_impl.cc` uses `P(close) = elapsed / drain_timeout`,
    distributing close events over the first quarter of the drain
    window. This prevents a thundering herd of disconnects from the
    same LB to the same backend. *Lesson: graceful close timing must
    be jittered.* Citation: `source/server/drain_manager_impl.cc`.

20. **`readDisable(true)` is a *counter*, not a boolean.** Because
    multiple filter / stream / connection paths can independently want
    backpressure, Envoy refcounts the read-disable. Reads resume only
    when every caller has paired a `false`. *Lesson: backpressure is
    multi-source; a boolean is a bug waiting to happen.* Citation:
    Envoy flow-control doc.

---

## Defensive patterns worth comparing against our code

1. **`isPrematureRstStream` accounting + `drained_due_to_premature_resets_`
   recursion guard.** Two-layer defense: lifetime threshold + counter
   for "too many premature resets" + recursion guard so the drain
   itself cannot recursively trigger more drain. Check our
   `lb-h2/security.rs` for the equivalent — single counter is *not*
   enough; the recursive case bit Envoy.

2. **Watermark counter (`readDisable` is refcounted).** Inverse of the
   "boolean is a bug" lesson. If our code uses a bool for "should we
   read more" anywhere in `lb-l7` or `lb-h2`, we will deadlock or
   leak. Check the flow-control plumbing.

3. **`headers_with_underscores_action`.** Envoy makes this configurable
   and recommends REJECT at the edge. Check our `lb-h1/parse.rs`: does
   it default to reject, allow, or transform? `_` vs `-` is a known
   smuggling primitive.

4. **Path normalisation: `normalize_path` + `merge_slashes` +
   `path_with_escaped_slashes_action`.** Three independent knobs in
   Envoy because path confusion is many bugs, not one. Check our
   `lb-l7` for whether we expose all three or rely on a single
   "canonicalise" flag.

5. **Randomised / probabilistic drain.** `drain_manager_impl.cc`'s
   `P(close) = elapsed / total` is the right shape. If our
   `lb-controlplane` drain logic uses a synchronous close-all loop,
   we will create thundering herds at every restart.

---

## Explicit non-goals / what Envoy chose NOT to do

- **Availability is not part of the default threat model.** Envoy's
  official position: operators must configure overload manager,
  circuit breakers, watermarks. Defaults are *not* DoS-safe. This is
  important context for our gate matrix.
- **xDS authentication is the operator's job.** Wire-level xDS
  exploits ("can a hostile control plane crash Envoy?") are out of
  scope.
- **External authZ / ratelimit services are assumed trusted.** Envoy
  does not defend against a malicious ext-authz response.
- **Brute-force CPU/memory attacks below ~100x amplification are out
  of scope.** Envoy intentionally chooses not to defend against every
  Slowloris-style attack at the protocol layer.
- **No built-in caching with default key.** This is a non-goal for
  Envoy and a hard-won lesson on the Pingora side (GHSA-f93w-pcj3-rggc).
  Envoy avoided the bug by not shipping default cache.

---

## Direct comparison ideas for `div-l7`

- **L7-I (Envoy L#1):** Look for the equivalent of
  `saw_downstream_reset_` in `lb-h2` and `lb-l7/h2_proxy.rs`. Any
  callback path that processes frames after a stream reset must
  short-circuit at the top.
- **L7-J (Envoy L#3):** In `lb-security` and `lb-l7`, find every
  multi-value header read path and make sure it iterates values, not
  the joined string. RBAC, auth header, X-Forwarded-For, Cookie,
  Trailer.
- **L7-K (Envoy L#4):** Audit SAN matching in `lb-security/handshake.rs`
  / `lb-quic`: does the comparator ever pass through a
  null-terminator-aware API (`CStr`, `as_ptr`)?
- **L7-L (Envoy L#11):** In `lb-h2/security.rs`, verify rapid-reset
  defence has BOTH lifetime-based premature detection AND a
  per-second accumulator AND a recursion guard on the drain.
- **L7-M (Envoy L#13-15):** Defaults audit: max_concurrent_streams,
  initial windows, headers_with_underscores, path normalisation knobs.
  Compare our `config/` defaults against Envoy's recommended edge
  numbers.
- **L7-N (Envoy L#19):** Look at our drain logic in
  `lb-controlplane` and `lb-l7` — is there per-connection
  randomisation, or a single synchronous loop?
- **L7-O (Envoy L#20):** Backpressure plumbing — find any boolean
  read-disable flag and check whether multiple producers can race it.

This is the second batch of homework for `div-l7`.
