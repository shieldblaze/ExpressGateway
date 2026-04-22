# DoS Mitigation Catalog

## Scope

This document enumerates the denial-of-service (DoS) attack classes that
ExpressGateway defends against, maps each to concrete defenses in the
codebase, and records residual risks. It exists so that a future engineer
can audit a single file and know (a) which attack classes have been
considered, (b) where the mitigation lives, and (c) which test proves the
defense works.

The catalog is organised by OSI / protocol layer. Within each layer,
entries follow a fixed shape: *description*, *reference* (CVE or RFC),
*our defense*, *test*. Attacks we know about but have explicitly deferred
are called out in "Known residual risks" at the end.

Cloud and ISP-layer volumetric attacks (e.g. direct UDP floods sized in
Gbps) are out of scope for this document because they must be absorbed
upstream of any host-level reverse proxy. The gateway's job is to survive
intentionally cheap attacks that fit inside ordinary TCP, TLS, HTTP, or
QUIC traffic profiles.

## Attack taxonomy

### L3 / L4: packet-level floods

**TCP SYN flood.** A half-open connection flood fills the accept queue so
that legitimate clients cannot complete the three-way handshake. Linux
kernels have mitigated this at the IP layer with SYN cookies since 2.6.
Reference: RFC 4987 §3.1. Our defense is configuration-layer rather than
code-layer: PROMPT.md §7 requires listeners to set `SO_BACKLOG=50_000`
and `TCP_FASTOPEN`, and operators are expected to enable
`net.ipv4.tcp_syncookies=1`. The L4 XDP plane (`crates/lb-l4-xdp`) is
currently a userspace simulation; the real eBPF program would be attached
pre-conntrack and could drop malformed SYNs before the kernel allocates
state. See `docs/research/katran.md` for prior-art defense-at-line-rate.

**UDP reflection / amplification (DNS, NTP, memcached).** Spoofed-source
requests to an unprotected UDP service cause the service to emit large
replies to the victim. ExpressGateway does not operate UDP services
beyond QUIC; the QUIC stateless-retry mechanism (RFC 9000 §8) is
delegated to `quiche`, not implemented by us. Reference: CVE-2018-1000115
(memcached).

**L4 conntrack exhaustion.** An attacker opens millions of short-lived
flows to exhaust the flow table. `crates/lb-l4-xdp` caps conntrack
entries at a configurable maximum (`DEFAULT_CONNTRACK_MAX_ENTRIES =
1_000_000`) with FIFO eviction; a full table degrades to Maglev-only
lookups rather than panicking. Test: `tests/l4_xdp_conntrack.rs` and the
`conntrack_eviction_at_capacity` unit test in `lb-l4-xdp/src/lib.rs`.

### L5 / TLS: handshake-layer abuse

**TLS renegotiation attack.** Legacy SSL renegotiation allowed a client
to repeatedly trigger full handshakes on a single connection, forcing
asymmetric CPU cost on the server. TLS 1.3 (RFC 8446) removes
renegotiation entirely. ExpressGateway requires TLS 1.3 by default
(PROMPT.md §8) and plans to disable TLS 1.2 renegotiation when TLS 1.2
is enabled. Reference: CVE-2011-1473.

**TLS fallback / version rollback.** Older clients may be coerced to
TLS 1.0 or SSLv3 by a MITM. rustls (the TLS implementation we use per
PROMPT.md §8) does not implement anything below TLS 1.2 and enforces
`TLS_FALLBACK_SCSV` per RFC 7507.

**0-RTT replay.** TLS 1.3 early data (0-RTT) can be captured and
replayed by an attacker because the server has no transcript binding for
the first application record. RFC 8446 §8 requires replay mitigation.
`lb-security::ZeroRttReplayGuard` in
`crates/lb-security/src/zero_rtt.rs` keeps a bounded ring buffer of
token digests (32-byte hash, 32 independent hash lanes) and rejects
duplicates. Test: `tests/security_zero_rtt_replay.rs` and the
`zero_rtt_replay_detected`, `zero_rtt_eviction_on_capacity`,
`zero_rtt_hash_based_dedup` unit tests.

### L7 / HTTP/1.1

**Slowloris.** The classic Robert Hansen attack from 2009: open many
connections, send headers one byte at a time, never send the terminating
blank line. Each connection costs the server one file descriptor and a
parser state. `lb-security::SlowlorisDetector` enforces two independent
thresholds (see `crates/lb-security/src/slowloris.rs`): an absolute
`header_timeout_ms` and a windowed minimum byte rate
(`min_rate_bytes_per_sec`) computed over rolling one-second buckets.
Windows shorter than 1000 ms are skipped to avoid false positives on
fast-but-tiny requests. Test: `tests/security_slowloris.rs`.

**Slow POST (R-U-Dead-Yet).** The body-phase analogue of slowloris: send
a large `Content-Length` header, then trickle the body. Defense lives in
`lb-security::SlowPostDetector`
(`crates/lb-security/src/slow_post.rs`); it enforces both an absolute
body timeout and a windowed rate floor. Test:
`tests/security_slow_post.rs`.

**Header bombs / large headers.** An attacker sends oversized headers
(single 1 MB `Cookie`, thousands of headers) to exhaust memory or CPU
during parsing. HTTP/1.1 parser limits live in `crates/lb-h1/src/parse.rs`
and are policy-level: the parser searches for CRLF boundaries in the
incoming buffer and does not impose a hard cap itself; the boundary is
expected to be enforced by a buffer-size cap in the connection handler.
Known gap: there is no explicit `max_header_bytes` check in the parser
today.

**Request smuggling — CL.TE (Content-Length + Transfer-Encoding,
front-end honors CL, back-end honors TE).** An attacker sends a request
with both headers; the proxy and origin disagree on body length and the
attacker smuggles a second request into the pipeline. Reference: RFC
9112 §6.1 and Watchfire 2005. `SmuggleDetector::check_cl_te` rejects
outright any request carrying both headers. Test:
`tests/security_smuggling_cl_te.rs`.

**Request smuggling — TE.CL.** The attacker sends `Transfer-Encoding`
with a non-`chunked` final coding (e.g. `Transfer-Encoding: gzip,
identity`). `SmuggleDetector::check_te_cl` rejects requests where the
final encoding is not exactly `chunked` (case-insensitive). Test:
`tests/security_smuggling_te_cl.rs`.

**Request smuggling — TE.TE obfuscation.** Variants like
`Transfer-Encoding:\tchunked` or `Transfer-Encoding: chunked,
chunked\x0b` are rejected by the same TE.CL check because the final
encoding after comma-splitting is not a bare `chunked`.

**Request smuggling — duplicate Content-Length.** RFC 9110 §8.6
mandates rejection if multiple `Content-Length` headers have differing
values. `SmuggleDetector::check_duplicate_cl` iterates the header list
and rejects on any disagreement. Test: see unit tests in
`crates/lb-security/src/lib.rs`.

**Request smuggling — negative or chunk-size-overflow.** Parsing a
chunk size with `usize::from_str_radix` in `crates/lb-h1/src/chunked.rs`
fails cleanly on negative values (the leading sign is rejected as a
non-hex character) and on overflow (values above `usize::MAX` return
`InvalidChunkEncoding`). Chunk extensions after `;` are discarded
safely.

**Request smuggling — H2 to H1 downgrade.** When a frontend accepts
HTTP/2 and proxies to an HTTP/1.1 origin, certain headers
(`connection`, `transfer-encoding`, `keep-alive`, `upgrade`,
`proxy-connection`, any pseudo-header, and `te` with a value other than
`trailers`) MUST NOT appear per RFC 9113 §8.2.2.
`SmuggleDetector::check_h2_downgrade` enforces this; test:
`tests/security_smuggling_h2_downgrade.rs`.

### L7 / HTTP/2

**Rapid Reset (CVE-2023-44487).** An attacker opens a stream and
immediately cancels it with `RST_STREAM`, repeated at millions of
streams per second. Each open-then-reset cycle wastes origin
compute. `lb-h2::RapidResetDetector` in
`crates/lb-h2/src/security.rs` uses a two-bucket sliding window (the
same technique as nginx rate limiting) with the `prev_count` weighted
by overlap. Integer-only math keeps the path branch-free and
allocation-free. Test: `tests/security_rapid_reset.rs` plus the
`rapid_reset_boundary_attack_detected` unit test which proves that the
carry-over catches an attacker who straddles a window boundary.
Advisory: https://nvd.nist.gov/vuln/detail/CVE-2023-44487.

**CONTINUATION flood (CVE-2024-27316 / CVE-2024-24549).** An attacker
begins a `HEADERS` frame and sends an arbitrary number of
`CONTINUATION` frames without ever setting `END_HEADERS`, forcing the
server to buffer an unbounded header block. `lb-h2::ContinuationFloodDetector`
caps the per-block CONTINUATION count; `END_HEADERS` resets the
counter. Test: `tests/security_continuation_flood.rs`. Advisory:
https://nvd.nist.gov/vuln/detail/CVE-2024-27316.

**SETTINGS flood.** Repeated `SETTINGS` frames force the peer to
allocate ACKs and adjust per-connection state. The frame codec in
`crates/lb-h2/src/frame.rs` parses SETTINGS but does not currently
rate-limit them. Gap.

**PING flood.** Similar to SETTINGS: `PING` frames require an `ACK`
reply. Parser does not currently rate-limit. Gap.

**Zero-window stall.** An attacker advertises `WINDOW_UPDATE` of zero
and never opens the flow-control window, forcing the server to hold
response bytes indefinitely. The remediation is per-stream and
per-connection send timeouts tied to the flow-control pause; not
implemented in the current codec, which only parses frames.

**HPACK bomb.** A single HPACK-compressed header block decompresses to
megabytes of literal headers (the attacker exploits the dynamic table
to repeat a long value). `lb-h2::HpackBombDetector` caps both the
absolute decoded size (`max_decoded_size`) and the compression ratio
(`max_ratio`). Test: `tests/security_hpack_bomb.rs`. Reference:
CVE-2016-1544 for the analogous SPDY issue.

**Priority cycles.** The HTTP/2 priority tree could contain a stream
depending on itself. RFC 9113 removed the tree-based priority scheme
(now deprecated in favour of RFC 9218 extensible priorities). Our
frame codec parses PRIORITY frames but does not build a dependency
tree, so priority-loop attacks do not apply.

### L7 / HTTP/3 and QUIC

**QPACK bomb.** The QPACK analogue of HPACK bomb: a small encoded
instruction set expands to huge decoded headers.
`lb-h3::QpackBombDetector` (`crates/lb-h3/src/security.rs`) applies
the same ratio-and-absolute-size guardrails as HpackBombDetector.
Test: `tests/security_qpack_bomb.rs`.

**Stateless-reset confusion.** An attacker who observes a stateless
reset token can terminate an unrelated connection. RFC 9000 §10.3
requires servers to pick reset tokens from a CSPRNG; this is
delegated to `quiche` / `rustls` at the QUIC layer (PROMPT.md §11).
Nothing in our code emits stateless-reset packets directly.

**0-RTT replay.** Same failure mode as TLS 1.3 0-RTT above. QUIC
reuses the TLS 1.3 early-data mechanism, so `ZeroRttReplayGuard`
applies equally; test: `tests/security_zero_rtt_replay.rs`.

**Connection-ID exhaustion.** An attacker rotates connection IDs
faster than the server can retire them, bloating the CID table. RFC
9000 §5.1 recommends retiring old CIDs. Handled by `quiche`; no
gateway-level CID state.

### Application-layer

**Slow upstream / backpressure-induced DoS.** A compromised or
overloaded origin accepts bytes very slowly, causing the gateway to
buffer client uploads in memory. The defense is end-to-end
backpressure: bounded per-connection buffers and paused reads on the
client socket once the write buffer to the origin exceeds a high
water mark (PROMPT.md §7, 64 KiB high / 32 KiB low). See
`docs/research/tokio-io-uring.md` for the mechanism.

**Retry storms.** A brief origin outage causes all clients to retry
simultaneously, amplifying the outage. The retry policy (in
`crates/lb-l7`, not yet implemented) must use jittered exponential
backoff and a retry budget. Gap.

**Slow upstream reply (reverse slowloris).** The symmetric attack to
client-side slowloris: a malicious or degraded origin returns
response headers one byte per second. Because the gateway is a
reverse proxy it cannot drop an origin the way it drops a client,
but it *can* apply a bounded deadline to `read_response_headers`
and surface a 504 to the client. The deadline lives in `crates/lb-l7`
(planned). Not yet implemented.

**Compression bomb on request bodies.** A client sends a gzip'd
body that expands 1000:1. The compression layer
(`crates/lb-compression`) must cap both the ratio and the absolute
decompressed size; the test
`tests/compression_bomb_cap.rs` covers this.

**Amplification via Cache-Control / Range stampede.** A client
issues many `Range: bytes=` requests for a large uncached resource;
the gateway serialises origin fetches behind a singleflight lock,
and uncoordinated clients amplify origin load. Out of scope for the
proxy tier (cache-layer concern), but documented here for
completeness.

## Defense mechanisms in ExpressGateway

Mitigation is layered rather than centralized. The gateway crates
that own defenses are:

- `crates/lb-l4-xdp` — conntrack capacity cap, Maglev consistent
  hashing (minimizes connection-blackhole risk on backend churn).
- `crates/lb-h1` — RFC-9112-compliant parser, strict chunked-encoding
  decoder, `find_double_crlf` header delimiter search.
- `crates/lb-h2` — rapid-reset, CONTINUATION-flood, HPACK-bomb
  detectors (all `#![deny(clippy::unwrap_used, ..., panic, ...)]`).
- `crates/lb-h3` — QPACK-bomb detector and QUIC frame parser with
  variable-integer bounds.
- `crates/lb-security` — cross-protocol helpers: slowloris, slow POST,
  smuggling (CL.TE, TE.CL, duplicate CL, H2 downgrade), 0-RTT replay.
- `crates/lb-io` — PROMPT.md §7 mandates bounded buffers and write
  high/low watermarks; the runtime choice is documented in
  `docs/research/tokio-io-uring.md`.

The codebase-wide `#![deny(clippy::panic, clippy::unwrap_used,
clippy::expect_used, clippy::indexing_slicing, ...)]` posture (visible
in every crate's `lib.rs`) is itself a DoS defense: no path through
our code can panic on attacker-controlled input, which is a
requirement for a long-running network daemon.

## Known residual risks

1. **HTTP/1.1 parser has no explicit header-byte cap.** The current
   parser in `crates/lb-h1/src/parse.rs` depends on an upstream
   buffer-size limit. This limit is not yet wired in `lb-io`. Gap.
2. **No global connection-rate limiter.** Per-IP token buckets are
   not implemented in any crate; slowloris and rapid-reset detectors
   operate per-connection and cannot observe multi-connection floods
   from a single source.
3. **HTTP/2 SETTINGS and PING floods are parsed but not rate-limited.**
4. **HTTP/2 zero-window stall has no per-stream send timeout.**
5. **No retry-budget / jitter implementation in `lb-l7`.**
6. **Real eBPF data plane is a stub.** `crates/lb-l4-xdp/ebpf/src/main.rs`
   is a placeholder; the userspace simulation covers Maglev and
   conntrack semantics but does not exercise verifier constraints.
   See `docs/research/aya-ebpf.md`.
7. **No WAF.** ExpressGateway does not inspect request bodies for
   signatures. Upstream WAF (ModSecurity, Coraza) is out of scope.

## Sources

- RFC 9110 (HTTP semantics), https://www.rfc-editor.org/rfc/rfc9110
- RFC 9112 (HTTP/1.1), https://www.rfc-editor.org/rfc/rfc9112
- RFC 9113 (HTTP/2), https://www.rfc-editor.org/rfc/rfc9113
- RFC 9114 (HTTP/3), https://www.rfc-editor.org/rfc/rfc9114
- RFC 9000 (QUIC transport), https://www.rfc-editor.org/rfc/rfc9000
- RFC 8446 (TLS 1.3), https://www.rfc-editor.org/rfc/rfc8446
- RFC 4987 (TCP SYN flooding), https://www.rfc-editor.org/rfc/rfc4987
- CVE-2023-44487 (HTTP/2 Rapid Reset),
  https://nvd.nist.gov/vuln/detail/CVE-2023-44487
- CVE-2024-27316 (Apache httpd HTTP/2 CONTINUATION flood),
  https://nvd.nist.gov/vuln/detail/CVE-2024-27316
- CVE-2024-24549 (Apache Tomcat HTTP/2 CONTINUATION handling),
  https://nvd.nist.gov/vuln/detail/CVE-2024-24549
- CVE-2011-1473 (TLS renegotiation DoS),
  https://nvd.nist.gov/vuln/detail/CVE-2011-1473
- Watchfire, "HTTP Request Smuggling" (2005),
  https://web.archive.org/web/20070202033438/http://www.watchfire.com/
- PortSwigger, "HTTP desync attacks" (2019),
  https://portswigger.net/research/http-desync-attacks-request-smuggling-reborn
- Cloudflare blog, "HTTP/2 Rapid Reset: deconstructing the record
  breaking attack" (2023),
  https://blog.cloudflare.com/technical-breakdown-http2-rapid-reset-ddos-attack/
- Google security blog, "Mitigating the HTTP/2 Continuation Flood"
  (2024), https://bughunters.google.com/blog/5697867388944384/
