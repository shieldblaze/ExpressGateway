# hyper / h2 / quinn / quiche / rustls — production lessons (Round-8 reference brief)

Author: `ref-l7`, 2026-05-14. **Knowledge only, do not port.**

This file consolidates the Rust ecosystem libraries our LB *uses*
(`hyper`, `quiche` + `tokio-quiche`, `rustls`) and the closest peers
(`h2`, `quinn`). Where the upstream community has explicitly issued a
security advisory or strongly-recommended hardening pattern that our
code does or does not adopt, the entry is tagged **OUR-CODE-CHECK:**
with a path.

Inventory of our usage (from `Cargo.toml` and `crates/*`):
- `hyper = "1"` full features — used in `lb-l7`, `lb-h1`, `lb-h2`.
- `h2 = "0.4"` — appears as a dev-dep test harness in
  `tests/h2_security_live.rs`; runtime H2 is via hyper which depends on
  h2 internally.
- `quiche = "0.28"` + `tokio-quiche = "0.18"` — used in `lb-quic`,
  `lb-h3`, `lb-io/quic_pool.rs`. (Migration from quinn documented in
  `docs/decisions/quinn-to-quiche-migration.md`.)
- `rustls = "0.23"` + `tokio-rustls = "0.26"` — used in
  `lb-security/handshake.rs`, `lb-l7/sni_authority.rs`, the TLS-over-TCP
  listener path.

---

## Architecture summary (1 page)

- **hyper 1.x = HTTP only, no I/O.** A pure protocol crate; you compose
  it with `hyper-util` for `TokioIo` adapters or your own I/O. The
  `Builder` per protocol exposes every hardening knob (max_concurrent_streams,
  max_pending_accept_reset_streams, max_local_error_reset_streams,
  keep_alive_interval/timeout, max_header_list_size, max_send_buf_size).
- **h2 = the HTTP/2 codec hyper uses internally.** Adds its own knobs
  beyond what hyper exposes — `max_pending_accept_reset_streams` and
  `max_local_error_reset_streams` were added specifically for
  rapid-reset and reset-after-local-error attacks.
- **quinn vs quiche:** both are QUIC implementations. quinn is
  pure-Rust + rustls; quiche is Cloudflare's, wraps BoringSSL by
  default. We picked quiche; quinn's CVE history is a useful peer
  benchmark.
- **rustls 0.23+ uses `CryptoProvider`.** The provider is a runtime
  dependency injection point (ring vs aws-lc-rs vs custom). FIPS mode
  is signalled by the provider; rustls now varies certain defaults
  (notably `require_ems`) based on FIPS status.

---

## Lessons learned (numbered, each with citation)

### hyper / h2

1. **Lenient `Content-Length` parsing accepted leading `+`.**
   GHSA-f3pg-qwvg-p99c, fixed in hyper 0.14.10. The HTTP/1 parser used
   Rust's `u64::from_str` which accepts `+3`. Combined with a strict
   upstream proxy, this is a request-smuggling primitive. *Lesson:
   never use the language's generic numeric parser on protocol
   integers; write your own digit-only parser that rejects sign and
   whitespace.* Citation:
   `github.com/hyperium/hyper/security/advisories/GHSA-f3pg-qwvg-p99c`.
   **OUR-CODE-CHECK:** `crates/lb-h1/src/parse.rs` — verify the CL
   parser is digit-only.

2. **Integer overflow on chunk size truncated body to low 64 bits.**
   GHSA-5h46-h7hh-c6x9, fixed in 0.14.10. A chunk-size hex string
   longer than 16 digits silently truncated to the rightmost 64 bits.
   Same primitive as nginx CVE-2013-2028 and HAProxy's stack overflow.
   *Lesson: the chunk-size hex parser must enforce a max digit count
   (16 for u64) AND reject overflow on accumulate, not silently mod.*
   Citation: GHSA-5h46-h7hh-c6x9. **OUR-CODE-CHECK:**
   `crates/lb-h1/src/chunked.rs` digit cap + accumulator overflow.

3. **Multiple Transfer-Encoding misparsed as chunked.**
   GHSA-6hfq-h8hq-87mf, fixed in 0.13.10 / 0.14.3. A missing boolean
   assignment let `Transfer-Encoding: chunked` + `Transfer-Encoding:
   cow` be interpreted as chunked. RFC 7230: after folding, must end
   in `chunked` token. *Lesson: TE header folding logic must reject
   anything where the folded tail is not exactly `chunked`. (Same as
   Pingora's 0.8.0 fix.)* Citation: GHSA-6hfq-h8hq-87mf.
   **OUR-CODE-CHECK:** `crates/lb-h1/src/parse.rs` and
   `crates/lb-security/src/smuggle.rs`.

4. **h2 0.3.17: `max_pending_accept_reset_streams` added.** This is
   the canonical defence for CVE-2023-44487 (HTTP/2 rapid reset).
   Hyper exposes it as a Builder method; default in hyper is **20**
   (h2 0.4.0). *Lesson: a server that accepts H2 must cap the rate of
   in-flight resets explicitly. Hyper's own docs say "leaving this
   unset poses potential DOS vulnerabilities."* Citation: h2
   CHANGELOG 0.3.17 + hyper http2 Builder docs.
   **OUR-CODE-CHECK:** `crates/lb-l7/src/h2_security.rs` —
   `max_pending_accept_reset_streams` default is
   `lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW as usize`. **Question for
   div-l7:** what *is* that constant, and is it tighter or looser
   than hyper's 20?

5. **h2 0.3.19: `too_many_resets` GOAWAY opaque debug.** Sister fix
   to the above; when the cap is hit, emit a structured GOAWAY so
   peers can be diagnosed. *Lesson: when you abort a misbehaving
   peer, include enough info in the GOAWAY frame's opaque field to
   diagnose it later.* Citation: h2 CHANGELOG 0.3.19.

6. **h2 0.4.2: "Limit error resets for misbehaving connections" =
   `max_local_error_reset_streams`.** This is the MadeYouReset
   defence (CVE-2025-8671). When the *server* sends RST_STREAM as a
   response to malformed peer behaviour, an attacker can goad the
   server into doing this rapidly to exhaust memory. Hyper default:
   1024. Hyper docs explicitly call out: "leaving this unset poses
   potential DOS vulnerabilities." *Lesson: server-side resets count
   toward the same memory pressure as peer-side resets — cap them.*
   Citation: h2 CHANGELOG 0.4.2 + hyper Builder docs.
   **OUR-CODE-CHECK:** `crates/lb-l7/src/h2_security.rs` sets it but
   the comment cites `DEFAULT_SETTINGS_MAX_PER_WINDOW`. Verify this
   number is at-most hyper's default 1024 and not larger.

7. **h2 0.3.21: "Fix opening of new streams over peer's max
   concurrent limit".** The server's `SETTINGS_MAX_CONCURRENT_STREAMS`
   was being violated when the client raced multiple HEADERS frames
   before the SETTINGS were applied. *Lesson: settings are not
   atomically observed; the codec must self-enforce its own limit
   against the *received* stream-id sequence regardless.* Citation:
   h2 CHANGELOG 0.3.21.

8. **h2 0.3.8: `max_send_buf_size`, default ~400MB.** A
   per-stream-stream send buffer that is *400 megabytes*. *Lesson: the
   default is operator-hostile at scale; if you do N concurrent
   streams, you have N × 400MB worst case. Tune it explicitly.*
   Citation: h2 CHANGELOG 0.3.8. **OUR-CODE-CHECK:** our default
   `max_send_buf_size` is `64 * 1024` (64 KiB) — much tighter than
   hyper's, good. But verify this is enforced everywhere we build
   h2 server/client.

9. **h2 0.3.5: "Fix sending of very large headers".** Single
   headers exceeding the peer's `MAX_FRAME_SIZE` were being sent in
   one frame, violating the spec; the fix splits them across
   CONTINUATION frames. *Lesson: HEADERS+CONTINUATION encoding must
   account for `MAX_FRAME_SIZE`; you can't assume the header fits a
   single frame.* Citation: h2 CHANGELOG 0.3.5.

10. **hyper 1.9.0 (2026-03-31): "validate null pointers before
    dereferencing in request/response functions".** A hardening pass
    against the FFI surface where C clients can hand in nullptr
    transparently. *Lesson: even in a Rust crate, the moment you expose
    a `*mut` / FFI export, every pointer needs to be validated; null
    deref is one ABI call away.* Citation: hyper CHANGELOG 1.9.0.

11. **hyper 1.9.0: "cancel sending client request body on response
    future drop" (HTTP/2).** Previously dropping the client future
    while a body was still being sent left the H2 stream half-open
    upstream; the request kept consuming server resources. *Lesson:
    futures that own a wire-level handle must register a drop guard
    that signals cancel to the peer.* Citation: hyper CHANGELOG
    1.9.0.

12. **hyper 1.9.0: "Non-UTF8 characters in Connection headers no
    longer cause panics".** The Connection header parser called
    `.to_str()` and unwrapped. *Lesson: protocol header parsers must
    never panic on input bytes; always return a parse error.*
    Citation: hyper CHANGELOG 1.9.0.

13. **hyper 1.1.0: rejected "chunked headers missing a digit" and
    restricted "chunked extensions".** Before this, `\r\n` after no
    digits was tolerated, and chunked extensions had no quoting
    enforcement. *Lesson: chunked parser must reject zero-digit
    chunk-size lines and must enforce extension token grammar.*
    Citation: hyper CHANGELOG 1.1.0.

14. **hyper 1.5.0: "send 'connection: close' when connection is
    ending" (HTTP/1).** Before this, hyper closed without the
    explicit header; some clients would keep retrying on the
    presumably-still-open connection. *Lesson: send the explicit
    close signal in HTTP/1, don't rely on the TCP FIN alone.*
    Citation: hyper CHANGELOG 1.5.0.

15. **hyper 1.5.0: "Strip content-length header in response to
    CONNECT requests".** RFC compliance — successful CONNECT responses
    must not have CL. Some upstreams emit it anyway; we have to strip.
    *Lesson: cross-protocol invariants must be re-checked at every hop;
    the proxy is responsible.* Citation: hyper CHANGELOG 1.5.0.

### quinn

16. **quinn-proto 0.11.13: panic on malformed varint in transport
    parameters.** GHSA-6xvm-j4wr-6v98 / CVE-2026-31812, High (CVSS
    8.7). A single crafted Initial packet crashed the server pre-auth.
    Root cause: `unwrap()` on `attacker-controlled varints`. *Lesson:
    QUIC packet parsers must propagate errors, never unwrap. This is
    the closest peer crate to quiche; we should run our quiche use
    against the same exposure surface.* Citation: quinn advisory.
    **OUR-CODE-CHECK:** the equivalent quiche path is the
    `accept_with_retry` and Initial-packet parsing — if a bug surfaces
    in quiche later, our `lb-quic/src/router.rs` is the listener that
    would absorb it.

17. **quinn-proto Endpoint::retry() panic.** GHSA-vr26-jcq5-fjj8 /
    CVE-2024-45311. `retry()` called on unvalidated connections panicked
    when `refuse()` / `ignore()` raced after a duplicate Initial. The
    panic was at `incoming_buffers[..]` indexing. *Lesson: the
    address-validation state machine is racy by design (the peer
    drives the retransmits); every indexing op needs bounds-checking,
    not just the parser.* Citation: quinn advisory.

18. **quinn unknown frame types panic.** GHSA-q8wc-j5m9-27w3 /
    CVE-2023-42805. Receiving an unknown QUIC frame type in a packet
    panicked the connection. RFC 9000 says unknown frame types in
    1-RTT must be PROTOCOL_VIOLATION; quinn panicked instead. *Lesson:
    unknown frame types are a connection-error code path, not a
    crash code path.* Citation: quinn advisory.

### quiche

19. **Infinite loop on RETIRE_CONNECTION_ID under multi-path.**
    GHSA-m3hh-f9gh-74c2 / CVE-2025-7054, High (CVSS 8.7). With path
    migration + multiple active connection IDs, a crafted retire frame
    could make quiche unable to emit a packet without violating "retire
    sequence number ≠ current packet sequence number" — infinite loop.
    Affects 0.15.0+; fix in 0.24.5. *Lesson: any "must not violate
    constraint X" state machine needs a fallback / abort path when no
    legal output exists; the absence of a legal action must close the
    connection, not loop.* Citation: GHSA-m3hh-f9gh-74c2.
    **OUR-CODE-CHECK:** we pin `quiche = "0.28"` which is past the
    fix; but tokio-quiche 0.18 carries its own pin — verify.

20. **Optimistic ACK widens cwnd.** GHSA-2v9p-3p3h-w56j / CVE in
    Jun 2025. An attacker who acks data they haven't received tricks
    congestion control into expanding the window. Fix in 0.24.4.
    *Lesson: congestion control must validate that acknowledged ranges
    were actually delivered (largest-acked tracking, not blind
    accumulation).* Citation: GHSA-2v9p-3p3h-w56j.

21. **Invalid ACK ranges widened cwnd.** GHSA-6m38-4r9r-5c4m,
    Jun 2025, High. Companion bug to #20 — ack ranges that overlap
    or are out-of-order also affected cwnd math. *Lesson: ack range
    validation is a security property, not just a correctness
    property.* Citation: GHSA-6m38-4r9r-5c4m.

22. **Unbounded RETIRE_CONNECTION_ID storage.** GHSA-xhg9-xwch-vr7x,
    Mar 2024, Low. quiche stored retirement information without bound;
    attacker could pump memory by retiring CIDs forever. *Lesson:
    any per-connection-id state must have a hard cap; the QUIC spec
    has `active_connection_id_limit` but the *retired* set is also
    state.* Citation: GHSA-xhg9-xwch-vr7x.

23. **CRYPTO frame flood = unbounded memory.** GHSA-78wx-jg4j-5j6g
    / CVE-2024-1765, Mar 2024. 1-RTT CRYPTO frames after handshake
    completion grew memory without bound. *Lesson: CRYPTO frames are
    not normal "data"; they carry TLS handshake bytes and an attacker
    after handshake completion has no legitimate reason to send any.
    Reject or strictly cap post-handshake CRYPTO.* Citation:
    GHSA-78wx-jg4j-5j6g.

24. **PATH_CHALLENGE queue unbounded.** GHSA-w3vp-jw9m-f9pm /
    CVE-2023-6193, Dec 2023. An attacker who can throttle PATH_RESPONSE
    (e.g. by manipulating cwnd) makes PATH_CHALLENGE accumulate
    unboundedly. Fix: cap the queue. *Lesson: every "respond to peer
    request" queue needs an explicit upper bound; absence is the
    bug.* Citation: GHSA-w3vp-jw9m-f9pm.

### rustls

25. **rustls `complete_io()` infinite loop on close_notify after
    client_hello.** GHSA-6g7w-8wpp-frhj / CVE-2024-32650, High (CVSS
    7.5). The state had `is_handshaking() = true` but
    `wants_read() = wants_write() = false` — `complete_io()` spun.
    *Lesson: if your loop has no progress condition, you have a bug;
    every iteration must either advance state or yield.* Affects
    `rustls::Stream` / `rustls::StreamOwned` only; **tokio-rustls is
    unaffected** because it doesn't use `complete_io()`. Citation:
    GHSA-6g7w-8wpp-frhj. **OUR-CODE-CHECK:** we use `tokio-rustls`
    only — confirm we don't call `complete_io` anywhere.

26. **rustls 0.23.40: ECH client-hello padding correctness.** ECH
    (Encrypted Client Hello) padding has to match an RFC scheme
    exactly to avoid leaking the inner SNI length via the outer SNI.
    *Lesson: any privacy-preserving feature that uses padding to hide
    the inner message length must implement the exact padding scheme;
    a one-off bug is a privacy leak.* Citation: rustls 0.23.40
    release notes.

27. **rustls 0.23.40: `ServerConfig::require_ems` default depends on
    FIPS status of CryptoProvider.** Extended Master Secret is
    required in FIPS mode, optional otherwise. *Lesson: defaults
    should follow the operator's threat model, not be invariant.*
    Citation: rustls 0.23.40 release notes.

28. **rustls policy doc: "We are specifically unfriendly to configure
    a custom certificate verifier."** Maintainers actively discourage
    operators from rolling their own verifier because most "I'll just
    accept-all" verifiers are written. *Lesson: don't expose a "skip
    verification" knob; if you must, name it so loudly nobody can use
    it accidentally (e.g. rustls's `dangerous_configuration` cargo
    feature).* Citation: rustls SECURITY.md.

29. **rustls eschews CBC-mode ciphersuites and RSA encryption.** A
    deliberate non-goal. *Lesson: when retiring a feature, just
    *retire it*; don't ship a "compatibility mode" flag.* Citation:
    rustls SECURITY.md.

30. **rustls supports only 2-year backports.** Security fixes go only
    to releases <2 years old. *Lesson: if your library is part of the
    supply chain, declare your support window explicitly so operators
    pin appropriately.* Citation: rustls SECURITY.md.

---

## Defensive patterns worth comparing against our code

1. **Hyper's H2 Builder default `max_pending_accept_reset_streams =
   20` and `max_local_error_reset_streams = 1024`.** Both are
   deliberate, both come with explicit "unset is a DoS" warnings.
   We have these wired (`crates/lb-l7/src/h2_security.rs`); the
   **div-l7 question** is whether our defaults match upstream or
   diverge.

2. **`max_send_buf_size` default 400 MB in h2 is a footgun.** Our
   `lb-l7` defaults to 64 KiB — much tighter. Worth confirming we
   don't accidentally pass through the h2 default anywhere (e.g.
   if a code path constructs a Builder without applying our config).

3. **Digit-only numeric parsing for `Content-Length` and chunk size.**
   Lesson #1 + #2. Verify `lb-h1/parse.rs` rejects `+`, `-`, leading
   whitespace, and that `lb-h1/chunked.rs` caps hex digits and detects
   overflow on accumulate.

4. **Connection-ID retired-set cap (quiche L#22) and
   active_connection_id_limit enforcement.** Worth confirming our
   `lb-quic` config sets both, and that retired CIDs are bounded.

5. **Avoid `complete_io()` (rustls L#25).** We use `tokio-rustls`
   exclusively — good. Verify no code path drops into raw rustls
   `Stream` / `StreamOwned`.

6. **Defaults vary on FIPS status (rustls L#27).** If we ever ship a
   FIPS build, our defaults must follow. Until then this is N/A but
   document it.

---

## Explicit non-goals / what these libraries chose NOT to do

- **hyper does not own I/O.** No connection pool, no listener, no
  retry. The application wires those.
- **hyper does NOT default-protect from DoS.** The Builder docs are
  explicit: leaving `max_pending_accept_reset_streams` or
  `max_local_error_reset_streams` unset is documented as a DoS
  vulnerability. *Default-safe is not a property hyper claims.*
- **h2 is the codec, not the connection.** Things like pool, retry,
  upstream selection are outside scope.
- **quinn and quiche are protocol stacks, not http3 servers.** For
  H3 you wire a separate crate (`quinn-h3`, `quiche::h3`,
  `tokio-quiche`).
- **rustls eschews RSA-encryption ciphersuites, CBC-mode
  ciphersuites, TLS<1.2.** Explicit non-goals in SECURITY.md.
- **rustls maintainers discourage custom cert verifiers.** Not
  forbidden, but loudly named (`dangerous_configuration` feature) to
  prevent accidental misuse.
- **rustls only supports 2-year-old releases for security backports.**
  Anything older is end-of-life by policy.

---

## Direct comparison ideas for `div-l7`

- **L7-AC (hyper L#1, L#2, L#3):** Audit `lb-h1/parse.rs` and
  `lb-h1/chunked.rs` for: digit-only CL parser, hex-digit cap on
  chunk-size, TE folding requires `chunked` at tail. Three known
  smuggling primitives, all reachable.
- **L7-AD (h2 L#4, L#6):** Verify
  `crates/lb-l7/src/h2_security.rs` defaults — what is
  `lb_h2::DEFAULT_SETTINGS_MAX_PER_WINDOW`? Compared to hyper's 20
  and 1024 defaults? Diverging upward is a vulnerability; diverging
  downward is operationally fragile. Document the decision.
- **L7-AE (h2 L#8):** Audit every site that constructs a hyper H2
  Builder (server or client) — does *every* site apply our
  `max_send_buf_size` cap, or does any path fall through to hyper's
  400MB default?
- **L7-AF (quinn L#16-18 vs quiche):** Run the same fuzzing targets
  that found the quinn panics (transport-parameter varint, Retry
  state machine, unknown frame type) against our quiche-based
  listener in `lb-quic/src/router.rs`. quiche's parser may have the
  same bug class.
- **L7-AG (quiche L#19):** Verify `tokio-quiche = "0.18"` carries a
  quiche fix ≥ 0.24.5 for the connection-ID retire infinite loop.
  Our pin says quiche 0.28 directly; check the transitive dep
  resolution in `Cargo.lock`.
- **L7-AH (quiche L#22):** In `lb-quic`, confirm
  `set_active_connection_id_limit` is configured and the retired-CID
  set has an effective cap.
- **L7-AI (quiche L#23):** Post-handshake CRYPTO frame budget — does
  our use of quiche-via-tokio_quiche pass any explicit
  post-handshake-crypto cap, or do we rely on quiche's internal cap
  (added in 0.19.2)?
- **L7-AJ (rustls L#28):** Search for any custom `ServerCertVerifier`
  or `ClientCertVerifier` in `lb-security`. If present, the
  implementation must be the *opposite* of "accept all".
- **L7-AK (hyper L#11):** Verify `lb-l7/h2_proxy.rs` and
  `lb-l7/h2_to_h2.rs` propagate cancel to the upstream when the
  downstream future is dropped (RST_STREAM(CANCEL)). Pingora's
  CHANGELOG fix for the same issue exists at 0.8.0.
- **L7-AL (rustls L#25):** Search for `complete_io` in the codebase
  — must be zero matches.

This is the fifth and final batch of homework for `div-l7`. The
**OUR-CODE-CHECK** annotations above are the highest-value targets;
each maps directly to a single file in our tree.
