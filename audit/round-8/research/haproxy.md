# HAProxy — production lessons (Round-8 reference brief)

Author: `ref-l7`, 2026-05-14. **Knowledge only, do not port.**
HAProxy is the oldest production-grade L7 LB still under active
development (>20 years), single-binary C codebase. Sources: HAProxy.org
CHANGELOG (master, 27K+ lines), haproxy.org news archive, NVD CVE
records, official documentation at docs.haproxy.org, and the HAProxy
Technologies blog.

---

## Architecture summary (1 page)

- **Master / worker model.** A privileged master process forks
  unprivileged worker processes that hold the listening sockets and do
  all the work. The master orchestrates reloads, signal forwarding,
  and (since 3.1) a reworked separation that "eliminates file
  descriptor leaks." Sockets are passed to workers via
  `SO_REUSEPORT` + FD-passing so that reload is seamless.
- **Single-threaded event loop per worker, multithreaded since 1.8.**
  Threads share data via fine-grained locking. Since 2.7, thread groups
  scale to 4096 threads (NUMA-aware). Stick tables moved from spinlock
  to rwlock in 2.7 for an 11× request-rate gain.
- **Two-layer architecture: muxes + analyzers.** Each protocol has its
  own multiplexer (`mux_h1.c`, `mux_h2.c`, `mux_h3.c`/`mux_quic.c`,
  `mux_fcgi.c`). Above the mux, the HTX (HTTP-tx) representation is
  protocol-neutral, so analyzers (header validation, ACLs, stick-table
  lookups, retries) work over a single internal format.
- **Hot reload via master-worker socket inheritance.** Old workers
  enter "soft stop", finish in-flight requests, then exit. New workers
  bind to the same sockets via `SO_REUSEPORT`. CLI-controlled, and
  since 3.0 the master/worker model was reworked specifically to make
  reload deterministic.
- **QUIC stack matured 2.6 → 2.7 → 2.9.** Native QUIC implementation
  (no quiche dependency); BBR and pacing landed experimentally in 3.1.

---

## Lessons learned (numbered, each with citation)

1. **Empty header field names truncated the parsed header list.**
   CVE-2023-25725, CVSS 9.1 Critical. The parser accepted a header
   with an empty name (e.g. `:value`) and treated it as an HTML/2/3
   terminator: subsequent headers *vanished*. Result: an attacker
   could make the security-relevant headers disappear before they
   reached the access-control logic. Fixed in 2.0.31, 2.2.29, 2.4.22,
   2.5.12, 2.6.9, 2.7.3. *Lesson: empty header names are not just
   "malformed" — they break the parser's framing assumption. Any
   parser that delimits header records must reject zero-length name
   fields before accepting the body of the message.* Citation: NVD
   CVE-2023-25725.

2. **Two QUIC parsing crashes in deep packet handling.**
   CVE-2026-26080 + CVE-2026-26081, reported Feb 2026, affecting 3.0+.
   "Specially crafted packets causing process crashes" — i.e. an
   unauthenticated remote attacker could take down the whole worker.
   *Lesson: every QUIC frame-parser branch needs differential fuzzing
   against alternative implementations (quiche, msquic, ngtcp2); the
   QUIC frame format is unforgiving and HAProxy's native parser has
   tripped on it twice in one release window.* Citation: haproxy.org
   news 2026-02-12.

3. **H1 stack buffer overflow in `h1_append_chunk_size`.**
   `BUG/MAJOR: mux_h1: fix stack buffer overflow in h1_append_chunk_size()`.
   Chunk-size encoding for forwarding had a fixed-size stack buffer
   that could overflow with maliciously large hex strings. *Lesson:
   anywhere you print a number into a fixed stack buffer for
   forwarding, the worst-case hex-digit count of u64 (16) is your
   absolute floor; assume the source is hostile.* Citation: HAProxy
   master CHANGELOG.

4. **QPACK length passed unchecked to the Huffman decoder.**
   `BUG/MAJOR: qpack: unchecked length passed to huffman decoder`.
   Same pattern, H3 side: an integer field that bounded a memcpy was
   not validated before use. *Lesson: in protocol decoders, every
   length is hostile until validated against the *remaining* buffer
   length — not just against an upper-bound constant.* Citation:
   HAProxy CHANGELOG.

5. **QPACK "too large decoded numbers".** `BUG/MEDIUM: qpack:
   correctly deal with too large decoded numbers`. QPACK integers can
   be arbitrarily-long QUIC varints; decoding them into a u32/u64
   without checking against the expected use produced wrong values
   downstream. *Lesson: variable-length-integer decode must return
   the parsed length AND assert it fits the consumer's storage; no
   silent truncation.* Citation: HAProxy CHANGELOG.

6. **H3 must reject unaligned frames (except DATA).**
   `BUG/MEDIUM: h3: reject unaligned frames except DATA`. The H3 spec
   requires HEADERS and other control frames to be sent in one piece
   on the request stream. HAProxy was accepting fragmented frames,
   creating a parser ambiguity. *Lesson: H3 frames are not H2 frames;
   H3 frames are *length-prefixed* QUIC stream chunks and the spec is
   strict about which can be fragmented. Reject anything else.*
   Citation: HAProxy CHANGELOG.

7. **H3 must check body size with content-length on empty FIN.**
   `BUG/MAJOR: h3: check body size with content-length on empty FIN`.
   An H3 request that sent a content-length header and then an empty
   FIN with zero body bytes would slip through CL validation. *Lesson:
   end-of-stream is not enough; the CL must be reconciled with the
   actual number of body bytes received.* Citation: HAProxy CHANGELOG.

8. **H1 → H2 upgrade attempts must be explicitly allowed.**
   `BUG/MEDIUM: mux-h1: Return an error on h2 upgrade attempts if not
   allowed`. Otherwise a client could request `Upgrade: h2c` against a
   listener that didn't enable H2 over cleartext and confuse the
   framing. *Lesson: protocol negotiation must be explicit on both
   sides; the *listener configuration*, not the inbound request,
   decides what protocols are accepted.* Citation: HAProxy CHANGELOG.

9. **Authority parsing must forbid commas.**
   `BUG/MAJOR: http: forbid comma character in authority value`. A
   comma in `:authority` / Host could be split by downstream
   intermediaries and bypass routing decisions. *Lesson: the
   authority component is a single token; any separator character
   (comma, space, tab) must be rejected pre-routing.* Citation: HAProxy
   CHANGELOG.

10. **Authority validation during H1 request parsing.**
    `BUG/MEDIUM: h1: Enforce the authority validation during H1
    request parsing`. The H1 path was not running the same authority
    sanitisation as H2/H3. *Lesson: protocol-neutral validation
    requires the validation to actually run on *every* protocol
    parser; HTX is not a magic equaliser.* Citation: HAProxy CHANGELOG.

11. **H2 body length check on parsing trailers fixed.**
    `BUG/MEDIUM: mux-h2: fix the body_len to check when parsing
    request trailers`. The expected body length was being compared
    against the wrong counter when trailers arrived, allowing an
    attacker to undercount the body before the trailers and bypass
    CL enforcement. *Lesson: counters tracked across stream lifetime
    must have well-typed names; a bug here is a smuggling primitive.*
    Citation: HAProxy CHANGELOG.

12. **DATA frame padding must be properly consumed.**
    `BUG/MEDIUM: mux-h2: Properly consume padding for DATA frames`.
    HAProxy did not advance its read cursor past the padding length;
    subsequent frames were parsed out of phase. *Lesson: H2 DATA
    framing has a 1-byte pad-length followed by padding; consume the
    whole pad before advancing.* Citation: HAProxy CHANGELOG.

13. **"Glitches" counter — HAProxy 3.0's named primitive for "the
    client is doing low-grade abuse".** `tune.h2.fe.glitches-threshold`
    and `tune.h2.be.glitches-threshold` count per-connection protocol
    anomalies (excess CONTINUATION frames, suspicious resets, malformed
    HPACK indices) and break the connection above a threshold. This is
    HAProxy's named defence for HTTP/2 rapid-reset and CONTINUATION-flood
    class attacks. *Lesson: build a named, observable counter for "the
    peer is being naughty even if technically still spec-compliant",
    so you have a single knob to tune at the edge and a single metric
    to chart.* Citation: HAProxy 3.0 announcement + docs.haproxy.org
    config manual.

14. **MSGF_BODY_CL preset must match H2_SF_DATA_CLEN at parse-time.**
    `BUG/MAJOR: mux-h2: preset MSGF_BODY_CL on H2_SF_DATA_CLEN in
    h2c_dec_hdrs()`. The H2-to-HTX bridge was setting the body-CL
    flag too late; the analyser then ran without knowing the framing
    constraint. *Lesson: protocol → internal-representation
    translation must set every framing-decision flag *before* the
    body-handling code path can ever run.* Citation: HAProxy CHANGELOG.

15. **0-copy forwarding must be disabled when draining the request.**
    `BUG/MEDIUM: mux-h1: Disable 0-copy forwarding when draining the
    request`. Otherwise a half-drained request body could land in the
    next request's stream, smuggling primitive. *Lesson: zero-copy is
    a performance feature with strict invariants; turn it off whenever
    the framing state is uncertain.* Citation: HAProxy CHANGELOG.

16. **HTTP/1 URI parsing tightened in 3.0.** "Stricter HTTP/1 URI
    parsing now rejects invalid request targets with 400 Bad-Request."
    Before 3.0, certain malformed URIs were accepted and forwarded.
    *Lesson: be conservative in what you accept *especially* when you
    re-emit it — the upstream may interpret differently.* Citation:
    HAProxy 3.0 announcement.

17. **HTX `host` chunked because in-place mutation broke aliasing.**
    `BUG/MAJOR: http-htx: Store new host in a chunk for scheme-based
    normalization`. Scheme-based normalisation rewrote the host
    in-place, but other code held a pointer to the original — UAF.
    *Lesson: any time normalisation may *grow* a field, allocate a new
    buffer; in-place mutation is for shrinking only.* Citation: HAProxy
    CHANGELOG.

18. **FCGI param decoding undercounted size.**
    `BUG/MAJOR: fcgi: Fix param decoding by properly checking its
    size`. Not L7 HTTP itself, but a useful pattern: every length-
    prefixed framing protocol has this class of bug. *Lesson: integer
    fields that bound a memcpy must be validated against remaining
    buffer space.* Citation: HAProxy CHANGELOG.

19. **HTTP/2 PERFORMANCE win — dynamic window sizing in 3.1.**
    `tune.h2.fe.rxbuf` / `tune.h2.be.rxbuf` were added because static
    65535-byte stream windows throttled large POST uploads. Up to 20×
    POST upload speedup. *Lesson: the H2 default 64 KiB initial window
    is for the *spec*, not for production; you must tune it (and
    Envoy says 64 KiB stream window, 1 MiB conn window at the edge —
    different deployment, different number).* Citation: HAProxy 3.1
    announcement.

20. **Persistent stats across reloads via GUIDs.** 3.0 introduced
    config-object GUIDs so that stats survive seamless reload via
    `dump stats-file`. *Lesson: hot reload without stats continuity
    blinds your SRE team to incidents that span the reload boundary.*
    Citation: HAProxy 3.0 announcement.

21. **NTLM private session retrieval bug.**
    `BUG/MEDIUM: http-ana: fix private session retrieval on NTLM`.
    NTLM mandates connection affinity (the same TCP connection must
    carry the whole handshake); HAProxy was losing this affinity in
    some pool paths, breaking auth. *Lesson: when a protocol
    *requires* connection affinity, the connection pool must expose a
    "pin this" primitive — *not* just opportunistically reuse.*
    Citation: HAProxy CHANGELOG.

---

## Defensive patterns worth comparing against our code

1. **The "glitches" counter pattern (`tune.h2.fe.glitches-threshold`).**
   A named, configurable, observable per-connection counter for
   protocol-level naughtiness. Maps to several attack classes
   (rapid reset, CONTINUATION flood, suspicious SETTINGS flood,
   weird HPACK indices) under one knob. We should check whether
   `lb-h2/security.rs` has the equivalent — a single configurable
   threshold rather than N independent counters.

2. **HTX as protocol-neutral internal representation.** HAProxy's
   trick: every analyzer (ACL, header-rewrite, stick-table, auth)
   sees the same HTX struct regardless of whether it came from H1,
   H2, or H3. This is *the* reason HAProxy can apply identical security
   rules across protocols. We should check our `lb-l7` cross-protocol
   handlers: do they share a normalised request type, or does each
   `hX_to_hY.rs` re-parse?

3. **Authority sanitisation runs on *every* protocol path.**
   Bug 10 in the list: the H1 parser was missing the check that H2/H3
   already had. We should grep `lb-h1`, `lb-h2`, `lb-h3` for identical
   authority validation — divergence = bug.

4. **0-copy forwarding gated on framing certainty.** Bug 15. When
   draining or in any partial-state, fall back to copy. We should
   check whether `lb-io` or `lb-l7` has a zero-copy fast path that
   could fire during half-drain.

5. **Persistent stats across reload (GUID-based).** Practical
   operations advice: hot reload that breaks dashboards is a
   regression. Check `lb-observability` and our drain flow for whether
   `/metrics` counters survive a reload.

---

## Explicit non-goals / what HAProxy chose NOT to do

- **No native config validation by signal-based reload.** As HAProxy
  itself discovered (and Cloudflare's Oxy article documented), they
  ship `haproxy -c` for explicit pre-reload validation rather than
  trying to recover from a bad config inside the running master.
- **No xDS / dynamic config.** Dynamic config is via the Runtime API
  (Unix socket CLI) — explicit operator action, no push protocol.
- **No HTTP caching layer in OSS.** Cache is a separate product
  (HAProxy Enterprise has it). Stays focused on L4-L7 proxying.
- **No transparent rewrite of non-RFC traffic.** HAProxy 3.0 tightened
  HTTP/1 URI parsing to *reject* what it used to forward; the
  philosophy is "reject early, return 400" rather than "pass through
  and let the backend decide".
- **No userland TCP stack.** All packet I/O goes through the kernel;
  contrast with Cilium / Katran which do XDP. HAProxy explicitly
  delegates L4 fast-path to whatever the OS provides.

---

## Direct comparison ideas for `div-l7`

- **L7-P (HAProxy L#1):** Audit `lb-h1/parse.rs` and `lb-h2/frame.rs`
  for empty-header-name rejection. The HAProxy bug is a critical-grade
  smuggling primitive; this must be tested. fuzz target candidate.
- **L7-Q (HAProxy L#9, L#10):** Authority parser uniformity. Find every
  protocol's authority extraction (`lb-h1`, `lb-h2`, `lb-h3`,
  `lb-l7/sni_authority.rs`) and confirm they reject comma, whitespace,
  empty.
- **L7-R (HAProxy L#11, L#12):** Body-length accounting across trailers
  and DATA padding. `lb-h2/frame.rs` must check that consumed bytes ==
  Content-Length even when trailers arrive, AND that DATA pad bytes
  advance the read cursor.
- **L7-S (HAProxy L#13):** Look for the "glitches counter" equivalent
  in `lb-h2/security.rs`. If we have N independent counters for
  rapid-reset, CONTINUATION, SETTINGS-flood, etc., consolidating them
  into one "protocol abuse score" gives operators a single dial.
- **L7-T (HAProxy L#15):** Verify any zero-copy / `sendfile` path in
  `lb-io` cannot fire while we are in a half-drain or
  partial-keep-alive state.
- **L7-U (HAProxy L#17):** Search for in-place HTTP-field mutation that
  could grow the field — these are the in-place-UAF candidates.
- **L7-V (HAProxy L#21):** Check `lb-l7/upstream.rs` connection-affinity
  primitives. Does NTLM (or any auth-handshake-pinned protocol) survive
  pool reuse? If we don't implement NTLM, this is N/A but worth a
  documented non-goal in our scope.

These are the third batch of homework for `div-l7`.
