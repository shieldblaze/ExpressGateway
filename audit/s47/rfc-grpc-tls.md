# S47 — gRPC-wire + TLS conformance review

Reviewer: rfc-grpc-tls. Branch `review/s47-rfc-security` (main @ 01915a77).
Method: read-and-reason only (no cargo, per the box constraint). Every claim
below was traced to source; library behaviour claims were checked against the
vendored crate sources under `~/.cargo/registry` at the versions in
`Cargo.lock` (h2 0.4.14, hyper 1.10.1, rustls 0.23.40, http 1.4.1).

Specs: gRPC `doc/PROTOCOL-HTTP2.md` + `doc/statuscodes.md`; RFC 9110/9113;
RFC 8446; RFC 7301; RFC 9000.

Prior art read first: `audit/grpc/s29-findings.md`, `audit/security/s38-*`,
`audit/deferred.md`, `docs/known-limitations.md`, `docs/features.md`,
`SECURITY.md`. Documented limitations (gRPC needs an H2/H3 front; no
server-side mTLS; SEC-2-13 0-RTT-off-by-construction) are NOT re-reported.

**SEC-2-13 status: NOT re-opened.** 0-RTT is still disabled by construction —
see T6, which additionally shows it is disabled on the QUIC path too and gives
a second, independent reason on the TCP path.

Severity ladder: CRITICAL / HIGH / MEDIUM / LOW / INFO.

---

## Summary table

| ID | Sev | Location | Claim |
|----|-----|----------|-------|
| T1 | HIGH | `crates/lb/src/main.rs:799` | `[runtime.tls].tls13_only` has no production consumer — TLS 1.2 stays negotiable |
| T2 | HIGH | `crates/lb/src/main.rs:972` | H3 upstream dials with the retired `lb-quic` ALPN → no real H3 origin can be dialed |
| G1 | HIGH | `crates/lb-l7/src/grpc_proxy.rs:286` | Unbounded request-body buffering on the synthesized health-check path |
| G2 | MEDIUM | `crates/lb-l7/src/h2_proxy.rs:698` | gRPC fork returns before the H2 request-sanitization chain (6 controls skipped) |
| G3 | MEDIUM | `crates/lb-l7/src/grpc_proxy.rs:179` | `grpc-timeout` bounds only time-to-response-head, not the RPC |
| G4 | MEDIUM | `crates/lb-l7/src/grpc_proxy.rs:460` | Repeated upstream response headers collapsed to the last value |
| T3 | LOW | `crates/lb-security/src/retry.rs:194` | Retry-token expiry bypassed across a process restart |
| T4 | LOW | `crates/lb-security/src/ticket.rs:171` | `ticket_lifetime` can exceed the RFC 8446 §4.6.1 7-day MUST |
| T5 | LOW | `crates/lb/src/main.rs:264` | Key-file permission check is boot-only; SIGUSR1 rotation skips it |
| G5 | LOW | `crates/lb-l7/src/grpc_proxy.rs:437` | `grpc-message` is not percent-encoded |
| G6 | LOW | `crates/lb-l7/src/grpc_proxy.rs:150` | Gateway-origin `grpc-message` echoes internal error text |
| G7 | LOW | `crates/lb-grpc/src/deadline.rs:19` | `parse_timeout` panics on a non-ASCII value (not reachable today) |
| G8 | INFO | `crates/lb-grpc/src/status.rs:94` | 4 extra rows beyond the spec's HTTP→gRPC table |
| G9 | INFO | `crates/lb-grpc/src/deadline.rs:25` | `>8`-digit `grpc-timeout` accepted (spec caps at 8) |
| G10 | INFO | `crates/lb-grpc/src/frame.rs:56` | Framing decoder is health-check-only; length-check-before-allocate is CORRECT |
| T6 | INFO | `crates/lb-quic/src/listener.rs:422` | 0-RTT off on BOTH paths; the replay guard is an Initial dedup today |
| T7 | INFO | `crates/lb-security/src/zero_rtt.rs:130` | Replay window is capacity-bounded only; flushable by design |
| T8 | INFO | `crates/lb-security/src/ticket.rs:403` | `not_after` is always `UNIX_EPOCH`; no cert-expiry signal exists |
| T9 | INFO | `crates/lb/src/main.rs:1753` | ALPN posture otherwise correct; `h3-29` residual |
| T10 | INFO | — | No custom cert verifier anywhere; upstream verification sound |

---

# gRPC findings

## G1 — HIGH — unbounded request-body buffering on the synthesized gRPC health check

**Location** `crates/lb-l7/src/grpc_proxy.rs:284-297`

```rust
async fn handle_health_check(req: Request<IncomingBody>) -> Response<BoxBody<Bytes, hyper::Error>> {
    // Zero-length body or a decode error ⇒ the overall probe: always SERVING.
    let body_bytes = (req.into_body().collect().await)
        .map_or_else(|_| Bytes::new(), http_body_util::Collected::to_bytes);
```

**Requires vs does.** Not a wire-spec MUST — this is a resource-bound
differential against the gateway's own contract. Every other L7 request path
enforces `MAX_REQUEST_BODY_BYTES` (`crates/lb-l7/src/h2_proxy.rs:41`, 64 MiB)
and answers 413 (`h2_proxy.rs:1152-1153`, `1352`, `1777`; `h1_proxy.rs:983`,
`1189`, `1342`, `1631`). This path applies no cap and buffers the whole body
into a single `Bytes` before looking at it.

**Failure scenario.** Reachable on any listener with a `[listeners.grpc]`
block (`enabled` defaults true, `health_synthesized` defaults true —
`crates/lb-config/src/lib.rs:453-463`). An unauthenticated client opens an H2
stream with `content-type: application/grpc`, `:method POST`,
`:path /grpc.health.v1.Health/Check`, and streams DATA frames without
END_STREAM. `Collected` grows without bound; because `collect()` keeps polling,
hyper keeps issuing WINDOW_UPDATEs, so H2 flow control does not bound the
total — only the rate. The dial is never made (the health branch returns
before `forward`), so there is no backend backpressure either. The only bound
is the connection-level `HttpTimeouts::total` select in
`h2_proxy.rs:494/540-545`; at a 60 s default and a 1 Gbps link that is ~7.5 GB
of resident memory per connection, and streams are concurrent.

**Existing test?** No. `grpc_health_check_synthesized`
(`tests/grpc_proxy_e2e.rs:445`) and `grpc_health_check_overall_serving`
(`:643`) both send a single 5-byte frame. Nothing exercises a large or
never-ending body on this path.

---

## G2 — MEDIUM — the gRPC fork returns before the H2 request-sanitization chain

**Location** `crates/lb-l7/src/h2_proxy.rs:698-726` (the fork) vs `:729-894`
(what it skips).

```rust
if let Some(gp) = self
    .grpc
    .as_ref()
    .filter(|g| g.config().enabled && grpc_proxy::is_grpc_request(&req))
{
    ...
    let (gp_parts, gp_body) = Arc::clone(gp).handle(req, backend.addr).await.into_parts();
    return Response::from_parts(...);
}
```

The `return` is taken before all of the following, each of which runs for
every non-gRPC H2 request:

| Skipped control | Line | Reference |
|---|---|---|
| `header_underscore_policy` (ROUND8-L7-05) | `:729-766` | — |
| `self.hooks.inspect_request` | `:767-779` | SEC-2-01 |
| `SmuggleDetector::check_all_mode(_, SmuggleMode::H2)` | `:782-801` | RFC 9112 §6.1, RFC 9113 §8.2.2 |
| `check_authority_host_agreement` | `:802-811` | PROTO-2-01 / RFC 9113 §8.3.1 |
| `check_sni_authority` → 421 | `:813-843` | PROTO-2-18 / RFC 9110 §15.5.20 |
| watchdog register/progress | `:845-863` | slowloris / slow-POST |
| `strip_into_newtype` (hop-by-hop strip) | `:880` | RFC 9110 §7.6.1, PROTO-2-07 |
| `append_xff` / `set_xfp` / `set_xfh` / `append_via` | `:883-894` | RFC 9110 §7.6.3 |

The choke-point comment at `:672-675` shows this fork was already recognised as
a bypass risk once — `validate_request` was hoisted above it for exactly that
reason — but only that one check was moved.

Sub-impacts, ranked by what is actually reachable through hyper's H2 server:

**(a) `Proxy-Authorization` / `Proxy-Authenticate` forwarded to the backend.**
RFC 9110 §7.6.1 lists both as hop-by-hop; a proxy MUST consume, not forward,
them. `crates/lb-l7/src/h1_proxy.rs:37-47` has them in `HOP_BY_HOP` and
`strip_hop_by_hop` (`:2021-2039`) removes them on every other path. They are
NOT in h2's connection-specific reject list
(`h2-0.4.14/src/frame/headers.rs:893-906` rejects only `connection`,
`transfer-encoding`, `upgrade`, `keep-alive`, `proxy-connection`, and
`te != trailers`), so a client can send `proxy-authorization: Basic …` over
H2 and the gRPC path relays the credential verbatim to the origin.

**(b) `X-Forwarded-For` relayed unmodified.** `append_xff`
(`h1_proxy.rs:2045-2064`) normally appends the real peer IP. On the gRPC path
nothing touches it, so a client-supplied `x-forwarded-for: 10.0.0.1` arrives at
the backend as the only value. Any backend that authorises, rate-limits, or
audits on XFF is spoofable through the gRPC route but not through any other
route on the same listener.

**(c) No `Via`.** RFC 9110 §7.6.3: "A proxy MUST send an appropriate Via header
field ... in each message that it forwards." `append_via`
(`h1_proxy.rs:2082-2100`) is skipped, so gRPC requests reach the origin with no
Via — an outright MUST violation and a loss of loop detection.

**(d) Conflicting `Content-Length` relayed.** `check_duplicate_cl`
(`crates/lb-security/src/smuggle.rs:26-41`) rejects differing duplicates with
400 on every other path. hyper's H2 server does not reject them — it folds and
returns `None` (`hyper-1.10.1/src/headers.rs:45-70`, called at
`proto/h2/server.rs:267`) — and h2 keeps both values (`fields.append`,
`h2-0.4.14/src/frame/headers.rs:914`). The gRPC path forwards `parts.headers` verbatim into
`sender.send_request`, so both values are re-encoded toward the backend. If the
origin is itself an H2→H1 translator this is a desync primitive.

**(e) No `:authority`/`Host` agreement check and no SNI/authority 421.** The
`:authority` is rewritten to the backend address by
`rewrite_uri_for_upstream` (`grpc_proxy.rs:225-234`), which neutralises the
`:authority` half — but the client's `Host` header survives in
`parts.headers` and reaches the origin, and no 421 is emitted for an
SNI/authority disagreement on this route.

**Existing test?** No. `tests/security_smuggling_h2_downgrade.rs`,
`tests/h2_validation_before_forward.rs` and the PROTO-2-18 tests drive plain
HTTP requests; none sets `content-type: application/grpc`, so none crosses the
fork.

---

## G3 — MEDIUM — `grpc-timeout` bounds only time-to-response-head, not the RPC

**Location** `crates/lb-l7/src/grpc_proxy.rs:178-189`

```rust
let send_fut = sender.send_request(upstream_req);
let upstream_result = if let Some(ms) = deadline_ms {
    let timed = tokio::time::timeout(Duration::from_millis(ms), send_fut).await;
    if let Ok(r) = timed { r } else {
        conn_handle.abort();
        return grpc_error_response(GrpcStatus::DeadlineExceeded, "gateway deadline");
    }
} else {
    send_fut.await
};
```

**Requires vs does.** gRPC PROTOCOL-HTTP2 defines `Timeout` as the deadline for
the *call*; the gateway's own doc comment at `:105-106` says the timeout wraps
"the upstream call ... so a stall synthesises `DEADLINE_EXCEEDED`". hyper's
`SendRequest::send_request` future resolves as soon as the response HEADERS
frame arrives — the body is streamed afterwards through the `BoxBody` returned
by `finalize_upstream` (`:454-468`), which carries no timer. So the deadline
covers the head only.

**Failure scenario.** A server-streaming RPC (or any unary handler that flushes
headers early) whose backend sends `:status 200` + `content-type` and then
stalls forever is never cancelled by the gateway. The client's own deadline
will fire, but the gateway keeps the H2 stream, the spawned connection task
(`:174-176`) and the dedicated upstream TCP connection alive until the
connection-level `HttpTimeouts::total` elapses. With `grpc-timeout: 1S` and a
`total` of 60 s the resource is held 60× longer than the negotiated deadline;
a request that omits `grpc-timeout` entirely takes the `else` branch and has no
per-request bound at all.

**Existing test?** No — and the existing one looks like it covers this.
`grpc_deadline_exceeded_from_gateway` (`tests/grpc_proxy_e2e.rs:427`) uses
`BackendMode::Sleep`, whose handler sleeps *before* returning a response
(`tests/grpc_proxy_e2e.rs:193-201`), i.e. a head stall. A body stall
(headers now, DATA never) is untested.

---

## G4 — MEDIUM — repeated upstream response headers collapsed to the last value

**Location** `crates/lb-l7/src/grpc_proxy.rs:454-468`

```rust
let mut builder = Response::builder().status(parts.status);
if let Some(hdrs) = builder.headers_mut() {
    for (k, v) in &parts.headers {
        hdrs.insert(k, v.clone());
    }
}
```

**Requires vs does.** `HeaderMap::iter()` yields one item per *value*, so a
field name with N values is visited N times; `HeaderMap::insert` replaces all
previous values for that name. Only the last value survives. gRPC metadata is
a multimap — PROTOCOL-HTTP2 `Custom-Metadata → Binary-Header / ASCII-Header`
with no single-value restriction, and every gRPC runtime exposes response
initial metadata as a list per key. This is the same class the repo already
names, cites and defends against elsewhere: `append_xff`
(`crates/lb-l7/src/h1_proxy.rs:2042-2044`) carries the comment *"`HeaderMap::get`
returns only the first and `insert` clobbers the rest — the silent-drop class
of Envoy GHSA-ghc4-35x6-crw5"*, and `append_via` mirrors it.

**Failure scenario.** A backend calls `SendHeader(metadata.Pairs("x-trace",
"a", "x-trace", "b"))`; the client receives only `x-trace: b`. Silent
application-data loss with no error surfaced anywhere. Also affects
`set-cookie` on a gRPC-Web-adjacent deployment.

Secondary, same site: this path performs no response hop-by-hop strip, unlike
`finalize_response` (`h2_proxy.rs:1514-1527`). h2's codec rejects the five
connection-specific names on receipt, so only `proxy-authenticate` can
actually transit — the mirror of G2(a) in the response direction.

**Existing test?** No. `tests/grpc_proxy_e2e.rs` backends emit single-valued
headers only.

---

## G5 — LOW — `grpc-message` is not percent-encoded

**Location** `crates/lb-l7/src/grpc_proxy.rs:431-439`

```rust
if let Ok(hv) = HeaderValue::from_str(msg) {
    trailers.insert(GRPC_MESSAGE.clone(), hv);
}
```

**Requires vs does.** PROTOCOL-HTTP2: `Status-Message → "grpc-message"
Percent-Encoded`; the value is a percent-encoded UTF-8 string. The gateway
inserts the raw string.

**Failure scenario.** `msg` is client-influenced: `:121-124` builds
`format!("malformed grpc-timeout: {raw}")` where `raw` is the client's own
header value. A client sending `grpc-timeout: 5%41S` gets back
`grpc-message: malformed grpc-timeout: 5%41S`, which a conforming client
percent-*decodes* to `... 5AS` — a corrupted diagnostic. Separately,
`HeaderValue::from_str` accepts bytes ≥ 0x80 (`http-1.4.1/src/header/value.rs`
`is_valid`), so a non-ASCII error string would go out unencoded rather than
percent-encoded. No header-injection risk: CR/LF are rejected by `from_str`.

**Existing test?** No. `grpc_error_response_carries_trailer_status`
(`grpc_proxy.rs:650-657`) asserts the literal, unencoded message.

---

## G6 — LOW — gateway-origin `grpc-message` echoes internal error text

**Location** `crates/lb-l7/src/grpc_proxy.rs:148-151`, `:167-172`, `:195`

```rust
return grpc_error_response(GrpcStatus::Unavailable, &format!("backend dial failed: {e}"));
...
return grpc_error_response(GrpcStatus::Unavailable, &format!("h2 client handshake: {e}"));
...
return grpc_error_response(GrpcStatus::Unavailable, &format!("send_request: {e}"));
```

Upstream `io::Error` / `hyper::Error` Display text reaches the client. The rest
of the codebase is careful here (`ProxyErr::BadRequest` is documented at
`h2_proxy.rs:921` as "400 WITHOUT a dial, so no backend body can leak"). Low
value to an attacker — OS connect errors rarely name the address — but it is a
one-line differential against the gateway's own posture.

---

## G7 — LOW — `GrpcDeadline::parse_timeout` panics on a non-ASCII value

**Location** `crates/lb-grpc/src/deadline.rs:19`

```rust
let (digits_str, unit_char) = value.split_at(value.len() - 1);
```

`str::split_at` takes a **byte** index and panics if it is not a UTF-8 char
boundary. `parse_timeout("5€")` panics (the last byte is a continuation byte).

**Reachability today: none.** The only production caller is
`parse_and_clamp_grpc_timeout` (`grpc_proxy.rs:255-259`), which feeds
`HeaderValue::to_str()` output; `to_str` admits only visible ASCII
(`http-1.4.1/src/header/value.rs:240-250` + `is_visible_ascii`), so the index
is always a boundary. Reported because (i) it is a `pub` API of a crate that
`deny(clippy::panic, clippy::indexing_slicing, …)` (`lb-grpc/src/lib.rs:2-11`)
— a lint set that does not catch `split_at` — and (ii) the obvious next caller
is an H3-side gRPC-awareness pass, where header bytes are typically converted
with `String::from_utf8_lossy` and *are* non-ASCII-capable.

**Existing test?** No. `deadline_invalid_format` (`lb-grpc/src/lib.rs:197-202`)
covers `""`, `"S"`, `"abc"`, `"5x"` — all ASCII.

---

## G8 — INFO — HTTP→gRPC status table has 4 rows beyond the spec

`crates/lb-grpc/src/status.rs:94-105`. Every row the spec mandates is correct:
400→INTERNAL, 401→UNAUTHENTICATED, 403→PERMISSION_DENIED, 404→UNIMPLEMENTED,
429/502/503/504→UNAVAILABLE. The extras — 500→INTERNAL, 501→UNIMPLEMENTED,
409→ABORTED, 499→CANCELLED — are outside the table, where the spec says
"everything else → UNKNOWN". They are all defensible "closest" mappings and
match common proxy practice; recorded for completeness only.

## G9 — INFO — `>8`-digit `grpc-timeout` accepted

`crates/lb-grpc/src/deadline.rs:25-38`. The grammar is
`TimeoutValue → {positive integer of at most 8 digits}`. The parser accepts any
value that fits `u64` (a 20+-digit value fails `parse::<u64>()` → INVALID_ARGUMENT
via GRPC-002). Harmless: `saturating_mul` prevents overflow and
`parse_and_clamp_grpc_timeout` (`grpc_proxy.rs:262-263`) clamps to
`max_deadline` before the value is used. `0S`/`0m` are also accepted and
behave correctly (immediate DEADLINE_EXCEEDED).

## G10 — INFO — the framing decoder is correct, and is health-check-only

`crates/lb-grpc/src/frame.rs:29-75`. The specific DoS the brief asks about is
**not** present: `msg_len` is compared against `max_message_size` at `:56-61`
*before* any allocation, and the payload is copied only after
`buf.len() < total_len` is checked at `:65-67` — a 4 GiB length in a 9-byte
buffer yields `MessageTooLarge`/`Incomplete`, never a 4 GiB allocation
(`lb-grpc/src/lib.rs:217-229` asserts exactly this). The compressed-flag
validation (`:38-46`) rejects anything but 0/1.

Two notes: (i) `decode_grpc_frame` has exactly one production caller,
`decode_health_check_service` (`grpc_proxy.rs:332`) — proxied gRPC bodies are
relayed opaquely, so no message-size limit applies to traffic, matching the
S29 R7 opaque model; (ii) `GrpcError::MessageTooLarge` is never mapped to
`GrpcStatus::ResourceExhausted` anywhere in the tree — the spec's
RESOURCE_EXHAUSTED-on-oversize behaviour is the endpoints' job here, which is
consistent but worth stating since `DEFAULT_MAX_MESSAGE_SIZE` reads like a
gateway policy knob and is not one.

---

# TLS findings

## T1 — HIGH — `[runtime.tls].tls13_only` has no production consumer

**Location** `crates/lb-config/src/lib.rs:334-343` (the knob),
`crates/lb-security/src/ticket.rs:229-246` (the only consumer),
`crates/lb/src/main.rs:788-813` (what production actually calls).

```rust
// crates/lb-config/src/lib.rs:337
pub struct RuntimeTlsConfig {
    /// Restrict every TLS listener to TLS 1.3. A COMPLIANCE knob (PCI-DSS 4.0 §4.2.1.1) ...
    #[serde(default)]
    pub tls13_only: bool,
}
```

```rust
// crates/lb-security/src/ticket.rs:406-410 — the path production takes
let builder = rustls::ServerConfig::builder_with_provider(provider)
    .with_safe_default_protocol_versions()          // ← TLS 1.2 + 1.3, unconditionally
    .map_err(|e| TlsBundleError::KeyMismatch(e.to_string()))?
    .with_no_client_auth();
```

**Requires vs does.** `build_server_config_with_policy` (`ticket.rs:229-246`)
is the only function that branches on `tls13_only`, and it is called from
exactly one place in the tree: `crates/lb-security/tests/tls_versions.rs`.
Production reaches a `ServerConfig` only through
`TlsConfigBundle::load_from_paths_with` — from `build_tls_bundle`
(`main.rs:799`) at boot and from `reload_tls_bundle` (`ticket.rs:452-463`) on
SIGUSR1 — and that function hardcodes `with_safe_default_protocol_versions()`.
`rg -n "tls13_only" --type rust` over the tree returns hits only in
`lb-config` (the field), `lb-security/src/ticket.rs` (the unused function) and
`lb-security/tests/tls_versions.rs`. The binary never names `RuntimeTlsConfig`.

**Failure scenario.** An operator with a PCI-DSS 4.0 §4.2.1.1 or FIPS-profile
requirement sets `[runtime.tls] tls13_only = true`, boots cleanly, and believes
TLS 1.2 is off. It is not: every listener continues to negotiate TLS 1.2 with
any client that offers it. The knob is documented as effective in six places —
`docs/guide/CONFIG.md:268` ("TLS 1.3 only on every TLS listener"),
`docs/features.md:169`, `docs/known-limitations.md:165`, `SECURITY.md:134`,
`docs/guide/cookbook.md:66`, `README.md:46` — so the failure is silent in both
directions: no warning at boot, and the documentation affirms the wrong
behaviour. (rustls only implements TLS 1.2/1.3, so 1.0/1.1 are unreachable
regardless; the exposure is a policy-compliance failure, not a downgrade to a
broken version.)

**Existing test?** No. `crates/lb-security/tests/tls_versions.rs` proves
`build_server_config_with_policy` works — it does not prove that the binary
calls it. `tests/tls_listener.rs` claims in its header to "reuse the same
`TicketRotator` + `ServerConfig` wiring the binary uses" but actually calls
`build_server_config`, a third path that production also does not use. Nothing
asserts the *binary's* negotiated version set.

---

## T2 — HIGH — the H3 upstream pool dials with the retired `lb-quic` ALPN token

**Location** `crates/lb/src/main.rs:969-975`

```rust
let factory: Arc<dyn Fn() -> Result<quiche::Config, quiche::Error> + Send + Sync> =
    Arc::new(move || {
        let mut cfg = quiche::Config::new(quiche::PROTOCOL_VERSION)?;
        cfg.set_application_protos(&[b"lb-quic"])?;      // ← not UPSTREAM_H3_ALPN_PROTOS
```

**Requires vs does.** RFC 9114 §3.1 defines `h3` as the ALPN token for HTTP/3;
RFC 7301 §3.2 requires a server with no protocol in common to abort with
`no_application_protocol`. `lb-quic` is not an IANA-registered ALPN identifier
and is not offered by any HTTP/3 server. The correct constant exists and is
used on the sibling path: `build_raw_quic_backend` (Mode B) calls
`config.set_application_protos(lb_io::quic_pool::UPSTREAM_H3_ALPN_PROTOS)`
(`main.rs:1013`), where `UPSTREAM_H3_ALPN_PROTOS = &[b"h3", b"h3-29"]`
(`crates/lb-io/src/quic_pool.rs:19`).

This is the un-fixed half of PROTO-2-02. `tests/quic_alpn_h3.rs:1-23` records
that the *listener* was moved off `lb-quic` onto `h3`/`h3-29`, and
`server_rejects_unknown_alpn` (`:279-300`) asserts that a client offering only
`lb-quic` is rejected with TLS alert 120. The client side of the same gateway
was never migrated.

**Failure scenario.** Any listener with `[[listeners.backends]] protocol =
"h3"` builds this pool (`main.rs:1484`, `:1715`, `:1776`, and the reload path
`:547`). The dial goes `h3_bridge.rs:1619 pool.acquire(addr, sni)` →
`quic_pool.rs:274 dial_new` → `connect_and_drive(..., None)` — `None` means no
ALPN override, so the factory's `["lb-quic"]` is what goes on the wire
(`quic_pool.rs:344-355`; only `dial_dedicated` accepts an override, and only
Mode B uses it). Against nginx/quiche/Caddy/any RFC 9114 origin the handshake
is aborted by the peer and every request through that backend fails.
ExpressGateway cannot even dial *itself*: its own H3 listener rejects
`lb-quic`, which is precisely what `server_rejects_unknown_alpn` proves.

**Existing test?** No — and the suite is structurally unable to catch it. Every
H3-upstream integration test builds its own `config_factory` with
`LB_QUIC_ALPN = b"lb-quic"` and a test backend that speaks the same token:
`tests/h1h3_md_streaming_verify.rs:47,127`,
`tests/h2h3_md_streaming_verify.rs:62,142`,
`tests/proto_translation_e2e.rs:89,236,392`, `tests/quic_listener_e2e.rs:66`.
The production factory in `main.rs` is never exercised against a real H3
origin, so the whole H3-upstream matrix is green on a non-standard token.

---

## T3 — LOW — retry-token expiry is bypassable across a process restart

**Location** `crates/lb-security/src/retry.rs:94-99` and `:193-197`

```rust
Self { key: hmac::Key::new(hmac::HMAC_SHA256, &secret), origin: Instant::now(), max_age: DEFAULT_RETRY_MAX_AGE }
...
let issued_at = self.origin + Duration::from_millis(issued_ms);
let age = now.saturating_duration_since(issued_at);
if age > self.max_age { return Err(RetryError::Expired); }
```

**Requires vs does.** RFC 9000 §8.1.3/§8.1.4 want address-validation tokens to
be short-lived; the module header states "a short expiry" and
`DEFAULT_RETRY_MAX_AGE` is 10 s. `issued_ms` is *relative to a per-process
`Instant`*, but the HMAC secret is persisted to disk across restarts
(`crates/lb-quic/src/listener.rs`, `load_or_generate_retry_secret` /
`write_secret_file` / `check_retry_secret_perms`). After a restart `origin`
advances by the old process's uptime plus downtime (call it D), while a
previously-minted token still carries its old `issued_ms`. `issued_at` is
therefore reconstructed D into the future, `saturating_duration_since` clamps
`age` to 0, and the token verifies as fresh for a further D.

**Failure scenario.** A gateway that has been up 30 days is restarted (or the
host reboots, resetting the monotonic clock, which makes it worse). Every retry
token minted during those 30 days is accepted as fresh for ~30 more days
instead of 10 s. The peer-address binding (`:188-190`) still holds, so an
off-path spoofer gains nothing — the practical loss is the freshness half of
the control for an attacker at, or sharing a NAT egress with, the original
address. Rated LOW on that basis, not because the mechanism is uncertain.

**Existing test?** No. `verify_rejects_expired_token` (`retry.rs:279-288`) uses
one signer, so `origin` never moves. A cross-restart test needs two
`RetryTokenSigner::new_with_secret` instances built at different `Instant`s.

---

## T4 — LOW — `ticket_lifetime` can exceed the RFC 8446 §4.6.1 7-day MUST

**Location** `crates/lb-security/src/ticket.rs:171-176` + `:198-202`

```rust
fn lifetime_secs(rot: &TicketRotator) -> u32 {
    let total = rot.rotation_interval.saturating_add(rot.overlap);
    u32::try_from(total.as_secs()).unwrap_or(u32::MAX)
}
```

**Requires vs does.** RFC 8446 §4.6.1: "Servers MUST NOT use any value greater
than 604800 seconds (7 days)" for `ticket_lifetime`. rustls passes
`config.ticketer.lifetime()` straight into the NewSessionTicket with no clamp
(`rustls-0.23.40/src/server/tls13.rs:1280`, and `tls12.rs:796` for the TLS 1.2
ticket). Config validation checks only that the interval is non-zero
(`crates/lb-config/src/lib.rs:1280-1283`); the overlap is not validated at all.

**Failure scenario.** `[listeners.tls] ticket_rotation_interval_seconds =
1209600` (14 days) — accepted — makes the gateway advertise
`ticket_lifetime = 1209600 + overlap`, a plain MUST violation that a
conformance scan or a strict client will flag. Defaults (86400 + 3600 = 90000 s)
are well inside the cap, so this is misconfiguration-gated.

**Existing test?** No. The rotator tests assert swap/overlap behaviour, not the
advertised lifetime.

---

## T5 — LOW — the key-file permission check is boot-only; SIGUSR1 rotation skips it

**Location** `crates/lb/src/main.rs:769-786` (the check), `:788-791` (its only
caller), `:264-277` (the reload path that does not call it).

`assert_key_perm_advisory` → `lb_security::assert_owner_only(path, strict)`
with `strict = !cfg!(debug_assertions)`, i.e. fatal in release. It runs once,
from `build_tls_bundle`, at listener construction. `reload_all_tls` →
`reload_tls_bundle` → `TlsConfigBundle::load_from_paths_with` reads the key
file again with no permission check.

**Failure scenario.** A deploy pipeline writes a rotated key with mode 0644 and
sends SIGUSR1. Boot would have refused it (SEC-2-08 strict mode); the hot
rotation loads it silently and the process runs indefinitely on a
world-readable private key. Cert/key rotation is exactly the moment a new file
with fresh permissions appears, so the check is missing where it is most
useful.

**Existing test?** No. `tests/cert_rotation.rs` exercises swap-correctness and
failure-keeps-old-bundle; it does not vary file modes.

---

## T6 — INFO — 0-RTT is disabled by construction on BOTH paths (SEC-2-13 not re-opened)

Three independent facts, all verified:

1. `max_early_data_size` is never assigned anywhere in the tree
   (`rg -n "max_early_data_size|enable_early_data" crates/` → no production
   hit). rustls 0.23 defaults it to 0. This is the original SEC-2-13 finding
   and it still holds.
2. **Second, independent reason on the TCP path.** The gateway installs a
   `RotatingTicketer` as `cfg.ticketer` (`ticket.rs:415`), i.e. *stateless*
   resumption. rustls refuses to advertise early data in that mode:
   `rustls-0.23.40/src/server/tls13.rs:1298-1305` — `if config.max_early_data_size
   > 0 { if !stateless { … } else { warn!("early_data with stateless resumption
   is not allowed"); } }`, with the comment "We implement RFC8446 section 8.1".
   So even setting the knob would not enable 0-RTT without also replacing the
   ticketer.
3. **The QUIC path is off too.** `build_server_config`
   (`crates/lb-quic/src/listener.rs:422-450`) never calls
   `enable_early_data()`, which quiche requires before it will accept 0-RTT.

Consequence for the brief's question "are early-data requests gated by method
or replayed blindly to the upstream": the question is moot — no early data is
ever accepted, so nothing is forwarded. `ZeroRttReplayGuard` as wired today
(`crates/lb-quic/src/router.rs:168-179`) is a dedup on *Initial* packets keyed
by `SCID || token[..32]`, which is a useful Retry-replay/connection-spam
control, but it is not exercising 0-RTT anti-replay. `SECURITY.md:55` reads as
if it were; worth a wording pass.

## T7 — INFO — the replay window is capacity-bounded only, with no freshness dimension

`crates/lb-security/src/zero_rtt.rs:120-139`. Eviction is pure LRU against
`max_tokens` (default 65 536, `:15`). The module header explains the LRU-vs-FIFO
choice (SEC-2-05): promoting on hit keeps an in-flight replayee observable
under a unique-token spray. That defence is real, but it does not cover the
deliberate case: a target digest that is never *hit* during the flood ages to
`front` and is evicted after `max_tokens` unique inserts, after which the
captured token replays cleanly. RFC 8446 §8.2's ClientHello-recording design
avoids this by binding the record to a freshness window derived from
`obfuscated_ticket_age`, so an attacker cannot force eviction of a
still-in-window entry. There is no time or ticket-age dimension here.

Not currently exploitable (see T6). This must be closed *before* early data is
ever enabled — it is the natural companion to the SEC-2-13 re-open trigger.

## T8 — INFO — `not_after` is always `UNIX_EPOCH`; there is no cert-expiry signal

`crates/lb-security/src/ticket.rs:394-396`: `let not_after =
SystemTime::UNIX_EPOCH;` with the deliberate comment "Left unparsed on purpose:
an x509 crate is a heavy supply-chain edge for one warn-only field." The
decision is defensible, but the module header at `:258-261` describes a
contract — "near-expiry `not_after` is WARN-ONLY because refusing near-expiry
certs is exactly wrong during an emergency rotation" — that no code implements:
`rg -n "not_after" crates/` finds zero consumers outside the struct definition
and its `Debug` impl. Operators reading that comment may believe a
near-expiry warning exists. Either drop the contract from the comment or record
it as a gap.

## T9 — INFO — ALPN posture is otherwise correct

- TLS-over-TCP listener offers `["h2", "http/1.1"]` (`main.rs:1753`) — both of
  which it can serve, dispatching on the negotiated value at `main.rs:3135-3160`.
- No-overlap handling is conformant: rustls selects by *server* preference and
  returns `NoApplicationProtocol` with alert 120 when the client offered ALPN
  and nothing matched (`rustls-0.23.40/src/server/hs.rs:99-119`), which is
  RFC 7301 §3.2.
- A client that offers no ALPN extension at all completes the handshake with no
  protocol selected and falls through to the H1 proxy — the correct default,
  and a protocol the listener does serve.
- QUIC listener offers `["h3", "h3-29"]` with an explicit rationale comment
  (`crates/lb-quic/src/lib.rs:98-101`) and a regression guard
  (`tests/quic_alpn_h3.rs::production_alpn_constant_is_h3`). Residual worth
  recording: quiche 0.29 implements RFC 9114 only, so a QUIC-v1 client offering
  *only* `h3-29` is served RFC 9114 semantics under a draft-29 label. Real
  draft-29 clients speak QUIC version 0xff00001d and are turned away by version
  negotiation before ALPN, so the exposure is narrow.

## T10 — INFO — no custom certificate verifier exists anywhere

`rg -n "dangerous|ServerCertVerifier|ClientCertVerifier|verify_server_cert|
danger_accept"` over `crates/` returns nothing. The three
`quiche::Config::verify_peer(false)` sites are all inside `#[cfg(test)]`
modules — `crates/lb-quic/src/router.rs:433` and `:538` (both in the test module
that uses the `#[cfg(test)]`-only `LB_QUIC_TEST_ALPN`) and
`crates/lb-io/src/quic_pool.rs:546` (below the `#[cfg(test)] mod tests` at
`:536`).

Upstream verification is sound: `build_h3_upstream_pool` (`main.rs:960-991`)
requires a CA bundle whenever `tls_verify_peer` is on, refuses to mix trust
roots in one pool, and sets `verify_peer(true)`; Mode B
(`build_raw_quic_backend`, `main.rs:1008-1022`) always verifies, falling back to
BoringSSL default roots when no CA is given. The verified *name* is
`tls_verify_hostname` when set, else the host part of the backend address
(`main.rs:894-903`) — an explicit override so an IP-literal backend can still be
matched against the name in its cert. Server-side mTLS is absent
(`with_no_client_auth`, `ticket.rs:247` and `:410`) — ALREADY-KNOWN, `docs/known-limitations.md`
"No server-side mTLS".
