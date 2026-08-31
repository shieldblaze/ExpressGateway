# S47 — HTTP/1.1 RFC conformance review (RFC 9110 semantics, RFC 9112 syntax)

Reviewer: rfc-h1. Branch `review/s47-rfc-security` (main @ 01915a77).
Method: read-and-reason only (no cargo — 2 vCPU / 7 GB box). Where behaviour
depends on the library, the pinned source was read directly:
`hyper 1.10.1` (`Cargo.lock:1221`), `httparse 1.10.1`, `http 1.4.1` in
`~/.cargo/registry/src/index.crates.io-*/`.

## Scope and the delegation boundary (read this first)

The live HTTP/1.1 wire parser on the datapath is **hyper 1.10.1 + httparse
1.10.1**, driven from `crates/lb-l7/src/h1_proxy.rs`
(`hyper::server::conn::http1::Builder` inbound at `h1_proxy.rs:457-462`,
`hyper::client::conn::http1::handshake` outbound at `h1_proxy.rs:872-877`).

`crates/lb-h1` is **not linked by any production crate** — verified: the only
`lb-h1` entries across all `Cargo.toml` are the workspace member list
(`Cargo.toml:24,165`), `fuzz/Cargo.toml:23`, and its own manifest. Consumers
are `tests/codec_roundtrip_h1.rs` and three fuzz targets. This matches
`docs/arch/overview.md:33` ("The `lb-h1` / `lb-h2` / `lb-h3-testcodec` crates
are NOT live wire parsers"), so **lb-h1 findings below are library-quality
defects, not live vulnerabilities**, and are severity-capped accordingly.
They are still reported because the crate is a public, `deny(missing_docs)`
API that `docs/research/rfc9112.md:8` presents as this repo's RFC 9112 mapping.

### What is already correct (checked, no finding)

Recorded so the negative results are auditable, not silence:

- **CL/TE precedence, duplicate CL, TE-not-final-chunked** — hyper server
  `role.rs:275-337`: `if is_te { continue }` (TE wins), differing duplicate CL
  → `Parse::content_length_invalid`, `is_te && !is_te_chunked` → 400. CL lexer
  `headers.rs:72-97` is digits-only (`+`/`-`/`0x`/whitespace all reject,
  `checked_mul`/`checked_add` on overflow). Gateway adds
  `SmuggleDetector::check_all_mode` on top (`h1_proxy.rs:740`). Matches
  `audit/protocol/SMUGGLE-MATRIX.md` — ALREADY-KNOWN, still true.
- **Whitespace between field-name and colon (RFC 9112 §5.1 MUST reject)** and
  **obs-fold (§5.2)** — httparse hardcodes `allow_spaces_after_header_name:
  false` and `allow_obsolete_multiline_headers: false` for *requests*
  (`httparse-1.10.1/src/lib.rs:507-509`); the response-side opt-ins are never
  called by the gateway. Both MUSTs met.
- **Chunked coding (RFC 9112 §7.1)** — hyper `decode.rs:271-412`: chunk-size
  via `checked_mul`/`checked_add` (overflow → `InvalidData`), extension state
  after `;`, trailer count and byte limits, bare-LF chunk-size lines rejected
  (only `\r` transitions to `SizeLf`).
- **Response framing for HEAD / 1xx / 204 / 304** — hyper `role.rs:497-522`
  (`can_have_body` / `can_chunked` / `can_have_content_length`) forces
  `Encoder::length(0)`; the client-side decoder (`role.rs:1247-1275`) returns
  `DecodedLength::ZERO` for HEAD, 204, 304 and 2xx-to-CONNECT.
- **Hop-by-hop stripping (RFC 9110 §7.6.1)** — `h1_proxy.rs:37-46` covers
  Connection, Proxy-Connection, Keep-Alive, Proxy-Authenticate,
  Proxy-Authorization, TE, Transfer-Encoding, Upgrade; `strip_hop_by_hop`
  (`h1_proxy.rs:2021-2041`) additionally removes every field *named* inside
  `Connection`, collecting the names before removing `Connection` itself, and
  `HeaderMap::remove` drops all values for a name. `Trailer` is deliberately
  kept (end-to-end, PROTO-2-08). The `StrippedRequest` newtype
  (`stripped_request.rs:43-46`) makes the strip a type-level precondition.
- **Upstream H1 connection is never pooled** — `pooled.take_stream()`
  (`h1_proxy.rs:866-868`) defeats the return-to-pool `Drop`, so the classic
  "upstream sent ≠ Content-Length poisons the next request" reuse hazard
  cannot occur (ROUND8-L7-10). This is the strongest available answer to the
  pool-poisoning question.
- **Request trailers** — `validate_h1_request_trailers` (`h1_proxy.rs:81-97`)
  rejects Content-Length / Transfer-Encoding / Host / Trailer / TE /
  Connection / hop-by-hop *before* forwarding on all three legs, and the H1→H1
  leg's abort injection (`H1PumpAbort`) prevents a truncated request being
  presented as complete. See H1-09 for the residual field-set gap.
- **1xx forwarding** — ALREADY-KNOWN: `audit/deferred.md` PROTO-2-03 (hyper's
  H1 server refuses to emit 1xx, `role.rs:385-390`; the client resolves on the
  first non-1xx). Not re-reported.
- **Response trailers dropped on a streamed H1 downstream** — ALREADY-KNOWN:
  `docs/known-limitations.md` ("gRPC requires an HTTP/2 or HTTP/3 front").

---

## H1-01 — HIGH — absolute-form request-target: no Host agreement, SNI gate keyed on Host only, absolute form forwarded verbatim

**Where**

`crates/lb-l7/src/h1_proxy.rs:701-712` — the only authority-vs-policy check on
the H1 front reads the **Host header only**:

```rust
        if !peer.ip().is_loopback() {
            let authority = parts
                .headers
                .get(hyper::header::HOST)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if let Err(mismatch) =
                crate::sni_authority::check_sni_authority(expected_sni, authority)
```

`crates/lb-l7/src/h1_proxy.rs:845-853` — the request URI is passed to the
upstream client untouched (only version and CL/TE are rewritten):

```rust
        let (mut parts, mut body) = req.into_parts();
        // F-MD-1 — force HTTP/1.1 and STRIP `content-length`/`transfer-encoding`
        parts.version = hyper::Version::HTTP_11;
        parts.headers.remove(hyper::header::CONTENT_LENGTH);
        parts.headers.remove(hyper::header::TRANSFER_ENCODING);
```

Compare `crates/lb-l7/src/h2_proxy.rs:805` — the H2 front **does** have the
gate the H1 front lacks:

```rust
        if let Err(msg) = check_authority_host_agreement(&parts.uri, &parts.headers) {
```

and the H2 SNI check (`h2_proxy.rs:813-828`) prefers `parts.uri.authority()`
with Host only as a *fallback* — the opposite precedence to H1.

**Spec**

- RFC 9112 §3.2.2 (absolute-form): "When a proxy receives a request with an
  absolute-form of request-target, the proxy MUST ignore the received Host
  header field (if any) and instead replace it with the host information of
  the request-target." RFC 9112 §3.2 further requires a client to send a Host
  that matches the absolute-form authority.
- RFC 9110 §7.4/§15.5.20: the effective request URI is the routing authority;
  a disagreement must not be silently resolved in favour of the losing side.

The code does the reverse of §3.2.2 twice, and inconsistently:

| leg | authority that policy checks | authority the next hop uses |
|---|---|---|
| H1 → H1 | `Host` (`h1_proxy.rs:705`) | **absolute-form authority** (see below) |
| H1 → H2 | `Host` | `Host` (`h1_to_h2.rs:42-55` → `:authority`) |
| H1 → H3 | `Host` | `Host` (`h1_to_h3.rs:43-55` → `:authority`) |

**Why the next hop sees the absolute form (H1→H1)**

hyper's H1 client serialises the request-target as `{}` over `http::Uri`
(`hyper-1.10.1/src/proto/h1/role.rs:1200`, `let _ = write!(FastWrite(dst),
"{} ", msg.head.subject.1);`), and `http::Uri`'s `Display`
(`http-1.4.1/src/uri/mod.rs:1032-1049`) emits `scheme://authority` when they
are present. The gateway uses the **low-level** `hyper::client::conn::http1`
(`h1_proxy.rs:872`), which — unlike `hyper_util::client::legacy` — performs no
origin-form rewrite: `dispatch.rs:684` builds
`RequestLine(parts.method, parts.uri)` verbatim. hyper's client also never
synthesises its own Host header, so the client-supplied Host is forwarded
alongside.

**Concrete failure scenario** (h1s listener, cert/SNI `allowed.example`,
vhosted backend):

```
GET http://admin.internal/secret HTTP/1.1\r\n
Host: allowed.example\r\n
\r\n
```

1. `authority::validate_request` (`h1_proxy.rs:600`) validates *both* values
   for illegal bytes and passes — ROUND8-L7-09 sanitises, it does not compare.
2. `check_sni_authority(Some("allowed.example"), "allowed.example")` → Ok. The
   PROTO-2-18 421 gate never sees `admin.internal`.
3. `X-Forwarded-Host: allowed.example` is set (`h1_proxy.rs:766-783`) and the
   trace span records `path=/secret` only.
4. The upstream receives, byte for byte:
   `GET http://admin.internal/secret HTTP/1.1` + `Host: allowed.example`.
   nginx, Apache and any §3.2.2-conformant origin select the vhost from the
   request-target → the request is served by `admin.internal`.

Net effect: the SNI↔Host policy gate (and any future Host-based routing/ACL)
is bypassed by moving the target host from `Host:` into the request-target,
and the same inbound request resolves to two different vhosts depending on
whether the chosen backend is H1 or H2/H3.

**Severity**: HIGH, conditional on deployment — with a single-vhost backend
this degrades to a routing-consistency defect; with an SNI-gated or vhosted
backend it is an authorisation bypass. It is exactly the "must reject, not
silently pick one" class.

**Test coverage**: none. `rg "GET http://"` over `tests/` and
`crates/*/tests/` returns nothing; every H1 probe helper (e.g.
`crates/lb-l7/tests/round8_authority_enforced.rs:67`) sends origin-form. The
prior art that mentions H1 absolute-form (`audit/round-8/verify/fixback.md:189`)
covers value *sanitisation* only.

**Not already known**: `check_authority_host_agreement` has exactly one call
site (`h2_proxy.rs:805`); PROTO-2-01 was scoped to H2; nothing in
`docs/known-limitations.md`, `docs/features.md` or `SECURITY.md` mentions the
request-target form.

---

## H1-02 — MEDIUM — duplicate `Host` header lines are neither rejected nor collapsed, and both are forwarded

**Where**: `crates/lb-l7/src/h1_proxy.rs:701-793` (the only Host readers are
`headers.get(HOST)`, which returns the **first** value) and
`crates/lb-security/src/smuggle.rs:24-104` (`check_all_mode` has checks for
duplicate Content-Length, CL+TE and TE-final-codec — no Host check at all).

hyper's server does not reject duplicates either: `role.rs:257-329` matches
only TRANSFER_ENCODING / CONTENT_LENGTH / CONNECTION / EXPECT / UPGRADE, then
`headers.append(name, value)` for every field (`role.rs:329`). hyper's client
writes every value: `write_headers` iterates `for (name, value) in headers`
(`role.rs:1593-1600`), so two Host lines in ⇒ two Host lines out.

**Spec**: RFC 9112 §3.2 — "A server MUST respond with a 400 (Bad Request)
status code to any HTTP/1.1 request message that lacks a Host header field and
to any request message that contains **more than one Host header field line**
or a Host header field with an invalid field value."

**Concrete failure scenario**:

```
GET /admin HTTP/1.1\r\n
Host: allowed.example\r\n
Host: admin.internal\r\n
\r\n
```

The gateway validates, SNI-checks, logs and sets `X-Forwarded-Host` from
`allowed.example` (first line) and forwards *both* lines to the H1 upstream.
Recipients disagree on which wins (nginx 400s; several application servers and
frameworks take the last value, e.g. anything doing
`request.headers["Host"]` over a last-write-wins map) — a host-confusion
primitive of the same shape as H1-01 with no absolute-form needed.

Note the H1→H2 and H1→H3 legs collapse duplicates (`h1_to_h2.rs:42-46`
`.find(...)` takes the first, and the loop skips all `host` fields), so this is
H1→H1-specific — which also makes the gateway's own behaviour
backend-protocol-dependent.

**Test coverage**: none — `rg` for a second Host line across `tests/` and
`crates/*/tests/` returns nothing.

---

## H1-03 — MEDIUM — a Host-less HTTP/1.1 request is accepted and forwarded

**Where**: `crates/lb-l7/src/h1_proxy.rs:591-800` — no missing-Host gate.
`crates/lb-l7/src/authority.rs:20` states the intent explicitly: "An absent or
empty value is NOT rejected here — **PROTO-2-01 owns that gate**", but
PROTO-2-01 (`check_authority_host_agreement`) is only wired on the H2 path
(`h2_proxy.rs:805`), and even there `(_, _) => Ok(())` passes when either side
is absent (`h2_proxy.rs:2632-2641`). hyper's server does not enforce Host
either (`role.rs:137-355` never looks at `header::HOST`).

**Spec**: RFC 9112 §3.2 — a server MUST respond 400 to an HTTP/1.1 request
lacking Host. RFC 9110 §7.2 — Host is required to reconstruct the target URI.

**Concrete failure scenario**:

```
GET / HTTP/1.1\r\n\r\n
```

- H1 backend: forwarded with no Host at all (hyper's low-level client adds
  none) — the origin now MUST 400 it, so the observable result is a 400 the
  gateway should have produced itself, and any origin that instead falls back
  to its *default* vhost serves a request whose authority nobody validated.
- H2 backend: `build_h1_to_h2_upstream_parts` (`h1_proxy.rs:2138-2152`) finds
  no `:authority` (it is derived from Host in `h1_to_h2.rs:42-46`) and falls
  back to `builder.uri(&translated.uri)` — a path-only URI — so the H2 request
  goes out with neither `:authority` nor `host`, which RFC 9113 §8.3.1 makes
  malformed.
- The SNI gate silently no-ops: `check_sni_authority` returns `Ok(())` on an
  empty authority by construction (`sni_authority.rs:43-45`).

For contrast, the H3 leg *does* enforce this: `crates/lb-quic/src/h3_bridge.rs:2105`
asserts "https request with no :authority and no Host must be rejected".

**Test coverage**: none for H1 (every helper sends Host).

---

## H1-04 — MEDIUM — the WebSocket-upgrade fork returns before the smuggle detector, the security hooks, the underscore policy and the SNI/Host gate

**Where**: `crates/lb-l7/src/h1_proxy.rs:634-638`:

```rust
        if self
            .ws
            .as_ref()
            .is_some_and(|w| w.config().enabled && is_h1_upgrade_request(&req))
        {
            return self.handle_ws_upgrade(req, req_trace).await;
        }
```

Everything below that early return is skipped for an upgrade request:

| check | line | skipped for WS? |
|---|---|---|
| `authority::validate_request` (ROUND8-L7-09) | 600 | no (hoisted above — deliberately) |
| header-underscore policy (ROUND8-L7-05) | 646-676 | **yes** |
| `hooks.inspect_request` (= `SmuggleDetector` in production) | 695 | **yes** |
| SNI ↔ Host 421 gate (PROTO-2-18) | 701-720 | **yes** |
| inline `SmuggleDetector::check_all_mode` (SEC-2-01) | 740 | **yes** |

The production hooks impl is the smuggle detector:
`crates/lb-security/src/hooks.rs:63-77` (`HooksBundle::inspect_request` →
`SmuggleDetector::check_all_mode`), so the WS path loses it twice over.

**Spec / policy**: RFC 9110 §15.5.22 (421) and the gateway's own PROTO-2-18
policy — the SNI↔authority agreement rule is a security gate, and an upgrade
request is not exempt from it. RFC 9112 §6.1's CL+TE rule likewise applies to
any request with content.

**Concrete failure scenario** (h1s listener, SNI `allowed.example`):

```
GET /ws HTTP/1.1\r\n
Host: victim.example\r\n
Upgrade: websocket\r\n
Connection: Upgrade\r\n
Sec-WebSocket-Version: 13\r\n
Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n
\r\n
```

`is_h1_upgrade_request` (`ws_proxy.rs:92-113`) needs only these four headers on
a GET — trivially forged by any TCP client. The 421 the identical *non*-upgrade
request would receive is not emitted; the tunnel to the backend is established.
The same request with `Content-Length: 0` + `Transfer-Encoding: chunked` added
reaches `handle_ws_upgrade` without the detector ever running.

The blast radius stops at the gateway's own policy (`dial_upstream_ws`
`h1_proxy.rs:1943-1946` builds a fresh `ws://{backend_addr}{path}` URI, so the
attacker's Host is not relayed, and `proxy_frames` re-encodes parsed frames so
raw bytes cannot be smuggled into the backend) — but the gate that should have
returned 421 did not.

The ROUND8-L7-09 comment at `h1_proxy.rs:597-599` documents that exactly this
fork already caused one bypass ("the WS-upgrade fork below reached
`pick_info()` unvalidated before this was hoisted here"); the hoist fixed the
authority validator only.

**Test coverage**: `crates/lb-l7/tests/round8_authority_enforced.rs` proves the
*authority* validator survives the fork; nothing tests the SNI gate or the
detector against an upgrade request (`crates/lb-l7/tests/sni_authority_421.rs`
uses plain requests).

---

## H1-05 — MEDIUM — a response carrying both Transfer-Encoding and Content-Length is forwarded with the stale Content-Length (RFC 9112 §6.3 MUST remove)

**Where**: `crates/lb-l7/src/h1_proxy.rs:1882-1897`:

```rust
    fn finalize_response(&self, resp: Response<IncomingBody>) -> Response<ClientRespBody> {
        let (mut parts, body) = resp.into_parts();
        strip_hop_by_hop(&mut parts.headers);
```

`strip_hop_by_hop` removes `transfer-encoding` (it is in `HOP_BY_HOP`,
`h1_proxy.rs:43`) but **never touches `content-length`** — the mirror-image of
the F-MD-1 request-leg fix at `h1_proxy.rs:851-853`, which strips both.

**Spec**: RFC 9112 §6.3 — "If a message is received with both a
Transfer-Encoding and a Content-Length header field, the Transfer-Encoding
overrides the Content-Length. … An intermediary that chooses to forward the
message **MUST first remove the received Content-Length field** and process the
Transfer-Encoding prior to forwarding the message downstream."

**Concrete failure scenario** — upstream (malicious, compromised, or merely
buggy) answers:

```
HTTP/1.1 200 OK\r\n
Content-Length: 5\r\n
Transfer-Encoding: chunked\r\n
\r\n
2c\r\n<44 bytes>\r\n0\r\n\r\n
```

hyper's client frames the body per TE (`role.rs:1286-1300`, TE wins) and hands
the gateway a 44-byte body plus a header map still holding `content-length: 5`.
`finalize_response` deletes TE, keeps CL. hyper's server then sees an
unknown-length body plus a CL header and re-frames from the header
(`role.rs:730-757`, `BodyLength::Unknown` + CL ⇒ `Encoder::length(5)`), and
`Kind::Length` truncates writes at the declared limit
(`encode.rs:143-153`). The client receives `content-length: 5` and 5 bytes —
39 bytes silently dropped. With the inequality reversed (CL larger than the
body) `Encoder::end()` returns `NotEof` (`encode.rs:116-126`) and the
connection is aborted mid-response.

Impact is content truncation/corruption and a §6.3 MUST violation, not a
downstream desync (hyper keeps its own framing self-consistent). The fix is one
line next to the existing strip.

**Test coverage**: none — `crates/lb-l7/tests/hop_by_hop_set.rs` and the
in-crate `hop_by_hop_response_strips_te_and_transfer_encoding_keeps_trailer`
test (`h1_proxy.rs:2576-2591`) assert TE removal but never pair it with a CL.

---

## H1-06 — LOW — request bodies on GET / HEAD / CONNECT are silently dropped upstream

**Where**: `crates/lb-l7/src/h1_proxy.rs:851-853` (F-MD-1 strips the inbound
`Content-Length` so hyper picks the framing) combined with hyper's client
`set_length` (`role.rs:1383-1391`):

```rust
                    match head.subject.0 {
                        Method::GET | Method::HEAD | Method::CONNECT => Some(Encoder::length(0)),
                        _ => { te.insert(HeaderValue::from_static("chunked")); Some(Encoder::chunked()) }
                    }
```

**Spec**: RFC 9110 §9.3.1 — content in a GET has no defined semantics and a
client SHOULD NOT send it, but §6.4 framing is method-independent: a proxy that
forwards the request should forward its content or refuse the request, not
forward a *different* request.

**Concrete failure scenario**:

```
GET /search HTTP/1.1\r\nHost: h\r\nContent-Length: 10\r\n\r\n0123456789
```

The gateway strips CL, attaches an unknown-length `StreamBody`, and hyper
selects `Encoder::length(0)` → the upstream receives `GET /search HTTP/1.1`
with **no body and no framing header**. The dispatcher then drops the body
without polling it (`dispatch.rs:384-390`, `can_write_body()` false ⇒
`clear_body = true`), the pump observes `SendOutcome::ReceiverGone` and takes
the F-MD-2 drain-and-validate path (`h1_proxy.rs:967-996`), so the 10 bytes are
read from the client and discarded. No hang, no desync — but the backend sees a
request the client did not send. Had the CL been preserved, hyper would have
used `Encoder::length(10)` and forwarded it, so this is gateway-attributable,
not purely a hyper behaviour.

**Test coverage**: none — the H1 body suites (`tests/h1h1_md_streaming_verify.rs`,
`tests/round8_body_overread.rs`) use POST.

---

## H1-07 — LOW — `Via` emits the protocol-name, which RFC 9110 §7.6.3 excludes for HTTP

**Where**: `crates/lb-l7/src/h1_proxy.rs:2083`:

```rust
    const VIA_TOKEN: &str = "HTTP/1.1 expressgateway";
```

**Spec**: RFC 9110 §7.6.3 — `received-protocol = [ protocol-name "/" ]
protocol-version`; "The protocol-name is excluded if and only if it would be
'HTTP'." The correct token is `1.1 expressgateway` (what nginx, squid and
Envoy emit — and what the gateway's own test fixture uses for *foreign* Via
values: `h1_proxy.rs:2533-2537`, `"1.1 gw1"`).

**Concrete failure**: `Via: 1.1 gw1, HTTP/1.1 expressgateway` — a strict
downstream Via parser reads `HTTP/1.1` as the protocol-*version* of a hop whose
protocol-name is absent, i.e. a nonsense version token; loop-detection and
hop-counting logic keyed on Via (RFC 9110 §7.6.3 recommends both) mis-parses
the gateway's own entry.

**Test coverage**: `h1_proxy.rs:2534-2549` (`via_two_lines_preserved`,
`via_appended`) pins the current wrong token, so a fix must update the test.

---

## H1-08 — LOW — CONNECT is proxied as an ordinary request instead of being rejected

**Where**: `crates/lb-l7/src/h1_proxy.rs:591-800` — nothing branches on
`Method::CONNECT`; it falls through to `proxy_request`. hyper's server sets
`wants_upgrade` for CONNECT (`role.rs:233`) but the gateway never calls
`hyper::upgrade::on` outside `handle_ws_upgrade`.

**Spec**: RFC 9110 §9.3.6 — a 2xx to CONNECT establishes a tunnel; a server
that will not tunnel should reject (405/501). RFC 9112 §3.2.3 — authority-form
targets are only for CONNECT.

**Concrete failure**: `CONNECT example.com:443 HTTP/1.1` is forwarded to the
configured backend as a CONNECT (with hyper's client encoding
`Encoder::length(0)`); if the backend answers 2xx, hyper's server writes the
2xx with no framing headers and `is_last = true` (`role.rs:378-380`), then
closes. The client believes a tunnel is open and writes into a closed socket.
This is **not** an open-proxy exposure — the destination is the configured
backend, not the CONNECT target — but the method reaches the backend and the
client is misled.

**Test coverage**: none for H1 CONNECT (`crates/lb-l7/tests/h2_connect_protocol_settings.rs`
is H2 extended-CONNECT).

---

## H1-09 — LOW — trailer field allow-list is narrower than RFC 9110 §6.5.1 on the H1→H2 / H1→H3 legs

**Where**: `crates/lb-l7/src/h1_proxy.rs:79-97` rejects only
`content-length`, `transfer-encoding`, `host`, `trailer`, `te`, `connection`
and the hop-by-hop set. The H1→H2 leg (`h1_proxy.rs:1214`, `h1_proxy.rs:1385-1396`) and the H1→H3
leg (`h1_proxy.rs:1601-1621`) then forward every remaining trailer verbatim.

**Spec**: RFC 9110 §6.5.1 — a sender MUST NOT generate a trailer field for
"message framing, routing, request modifiers, authentication, response control
data, or content processing"; §6.5.2 lets a recipient merge trailers into the
header section only when it knows the field is permitted.

**Concrete failure**: a chunked request whose trailer section carries
`authorization: Bearer <token>` or `cache-control: no-store` is forwarded to an
H2/H3 backend as HTTP trailers. A backend framework that merges trailers into
its header view sees an `Authorization` that no inspection layer examined at
head-time.

**Not applicable to H1→H1**: hyper's client filters trailers twice — they are
emitted only if declared in `Trailer:` and only if `is_valid_trailer_field`
passes (`encode.rs:163-280`, which rejects AUTHORIZATION, CACHE_CONTROL,
CONTENT_*, HOST, MAX_FORWARDS, SET_COOKIE, TRAILER, TRANSFER_ENCODING, TE).
The gap is exactly the two legs where the gateway builds the trailer map itself.

**Test coverage**: `crates/lb-l7/tests/trailer_passthrough.rs` asserts trailers
*survive*; nothing asserts a forbidden name is dropped.

---

## H1-10 — LOW — no connection close after a framing-error rejection (RFC 9112 §6.1)

**Where**: `crates/lb-l7/src/h1_proxy.rs:740-744` returns
`error_response(BAD_REQUEST, "request smuggling")` and nothing marks the
connection non-reusable; the keep-alive cap is the only close trigger
(`h1_proxy.rs:529-556`).

**Spec**: RFC 9112 §6.1 — "A server that receives a request message with a
transfer coding it does not understand SHOULD respond 501. A server MAY reject
a request that contains both Content-Length and Transfer-Encoding … **A server
that rejects such a request MUST respond with a 400 (Bad Request) status code
and close the connection.**"

**Actual behaviour**: hyper attempts a single-poll drain of the unread body and
only closes if the body does not complete in that one poll
(`conn.rs:849-865`, `poll_drain_or_close_read`). For an attack payload already
in the read buffer, the drain succeeds and the connection is kept alive, so any
bytes the attacker placed *after* the chunked terminator (but inside their
declared Content-Length) are parsed as the next pipelined request.

**Concrete failure**: only exploitable behind an L7 device that pools
connections to the gateway *and* frames on Content-Length (a CDN or another
reverse proxy — not the L4 topologies in `docs/guide/deployment-patterns.md`).
Against a direct client the smuggled prefix is the attacker's own next request.
Rated LOW for that reason; the fix (force close on any framing rejection) is
cheap insurance.

**Test coverage**: `tests/security_smuggling_cl_te.rs` asserts the 400; nothing
asserts the connection closes.

---

## H1-11 — INFO — non-UTF-8 header values are invisible to the detector and are dropped when bridging to H2/H3

**Where**: three copies of the same filter —
`h1_proxy.rs:726-734` (detector input), `h1_proxy.rs:2119-2129`
(`build_h1_to_h2_upstream_parts`), `h1_proxy.rs:2383-2394`
(`build_h1_to_h3_fieldlist`) — all use
`.filter_map(|(n, v)| v.to_str().ok().map(...))`.

RFC 9110 §5.5 permits `obs-text` (0x80-0xFF) in field values, and hyper/httparse
admit it (`role.rs:256`, `header_value!` builds the value unchecked). Two
consequences: (a) a header whose value is not UTF-8 is silently *dropped* on
the H1→H2/H1→H3 legs while it survives on H1→H1 (e.g. a Latin-1
`Content-Disposition` filename), and (b) it is invisible to
`SmuggleDetector::check_all_mode`. No live bypass today — a non-UTF-8
`Content-Length` is rejected by hyper unless TE is present, and the H1→H1 leg
strips CL/TE regardless — but the invariant is one refactor away from mattering.

---

## H1-12 — INFO — H1 header-section budget is ~408 KiB, undocumented and not configurable

`serve_connection_with_cancel_sni` (`h1_proxy.rs:457-462`) sets `keep_alive`,
a timer and `header_read_timeout`, but never `max_buf_size` or
`h1_max_headers`. hyper's defaults therefore apply: 100 header fields
(`role.rs:31`, `DEFAULT_MAX_HEADERS`) and a 417 792-byte read buffer
(`io.rs:23`, `DEFAULT_MAX_BUFFER_SIZE = 8192 + 4096 * 100`) before the
connection is closed. `docs/edge-defaults.md` documents H2's 64 KiB
`max_header_list_size` (row 21) but has no H1 row, and `lb_h1::MAX_HEADER_BYTES`
(64 KiB, `crates/lb-h1/src/parse.rs:9`) is not on the live path. An H1 client
may therefore spend ~6× the H2 header budget per request.

---

## H1-13 — INFO — the gateway answers `100 (Continue)` itself, before the origin sees the request

`hyper::server::conn::http1` auto-emits `HTTP/1.1 100 Continue` when the
service first polls a body whose request carried `Expect: 100-continue`
(parsed at `hyper-1.10.1/src/proto/h1/role.rs:306-311`; emitted from the
`Reading::Continue` state, `conn.rs:409-414`). There is no knob to disable it
in hyper 1.x, and the gateway forwards the `Expect` header upstream unchanged
(it is not hop-by-hop and `proxy_request` removes only CL/TE,
`h1_proxy.rs:851-853`), while hyper's H1 *client* never waits for the
upstream's 100.

RFC 9110 §10.1.1 lets a **proxy** generate its own 100 only "if the proxy
believes … the next inbound server only supports HTTP/1.0"; otherwise it must
forward the request head and let the origin decide. Consequence: a client's
`Expect: 100-continue` never receives an origin verdict, so an origin that
would have answered `417`/`413`/`401` from the head alone still receives the
full upload, and the gateway absorbs up to `MAX_REQUEST_BODY_BYTES` (64 MiB)
first. Library-imposed, already noted as a baseline in
`crates/lb-l7/tests/informational_responses.rs:49-55`
("100-continue auto-handling is wire-level and cannot be disabled"), so it is
recorded here for completeness rather than as a new defect.

---

## lb-h1 (NOT on the datapath — library-quality only)

Severity capped at LOW throughout: no production crate links `lb-h1`.

### H1-14 — LOW — header *values* are unvalidated: bare LF, NUL and CTLs are accepted into the parsed value

`crates/lb-h1/src/parse.rs:173-177`:

```rust
        let value = line_str
            .get(colon + 1..)
            .ok_or_else(|| H1Error::InvalidHeader(line_str.to_string()))?
            .trim()
            .to_string();
```

The name side is strictly `1*tchar` (`parse.rs:161-171`, ROUND8-L7-03) but the
value side is only trimmed. RFC 9110 §5.5: "A field value does not include
leading or trailing whitespace… **Field values are constrained to CR, LF and
NUL being disallowed**" (`field-value = *field-content`, and `field-vchar`
excludes CTLs other than HTAB).

Input `b"X: a\nSmuggled: 1\r\n\r\n"` → `find_double_crlf` (`parse.rs:47`) finds
the terminator, the line is split on CRLF only, and the result is the single
pair `("X", "a\nSmuggled: 1")` — a value containing a raw LF. Any consumer that
re-serialises that pair emits two header lines. hyper/httparse treat a bare LF
as a line terminator (`httparse-1.10.1/src/lib.rs:55-67`, RFC 9112 §2.2 MAY),
so this parser and the live one disagree about the message boundary — the
classic bare-LF desync shape.

`crates/lb-h1/tests/round8_header_name_rfc9110.rs` covers eight name-side cases
and zero value-side cases; the fuzz target only checks for panics.

### H1-15 — LOW — `ChunkedDecoder` has no size cap on the chunk-size line or the trailer section

`crates/lb-h1/src/chunked.rs:49-52` appends every fed byte to `self.buf`;
`try_read_size` (`chunked.rs:123-127`) returns `Ok(false)` while no CRLF is
present and `try_read_trailers` (`chunked.rs:166-168`) returns `Ok(false)`
while no CRLFCRLF is present. Feeding `b"5"` repeated N times, or `b"0\r\n"`
followed by N bytes of trailer-ish garbage, grows `self.buf` without bound.
`body_chunks` likewise accumulates unbounded (`chunked.rs:92`), and
`H1Error::BodyTooLarge` (`error.rs:29-36`) is never constructed anywhere in the
crate. `parse_trailers_with_limit` exists (`parse.rs:198-203`) but the chunked
decoder does not use it. RFC 9112 §7.1.2 recommends a trailer-section limit;
hyper enforces both a trailer count and a byte limit (`decode.rs:204-210`,
`put_u8!`).

### H1-16 — LOW — `ChunkedEncoder::finish` interpolates trailers with no CRLF validation

`crates/lb-h1/src/chunked.rs:266-269`:

```rust
        for (name, value) in trailers {
            let line = format!("{name}: {value}\r\n");
            out.put_slice(line.as_bytes());
        }
```

A trailer value of `"a\r\n\r\nHTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n"`
produces a forged second message on the wire. The decoder half validates
trailer *names* (`chunked.rs:190-195`) but the encoder validates neither name
nor value. RFC 9110 §5.5 / §6.5.1.

### H1-17 — INFO — a zero-header section can never be parsed

`parse_headers_with_limit` requires CRLFCRLF (`parse.rs:121`), so the legal
message `GET / HTTP/1.0\r\n\r\n` — whose header section after the request line
is just `"\r\n"` — returns `Err(Incomplete)` forever (`find_double_crlf` needs
4 bytes, `parse.rs:49`), until the caller's cap trips. RFC 9112 §2.1 permits a
message with no header fields.

### H1-18 — INFO — `parse_status_line` accepts `+200` and `0200`

`crates/lb-h1/src/parse.rs:94`, `let code: u16 = code_str.parse()` — Rust's
integer `FromStr` accepts a leading `+` and leading zeros, so
`HTTP/1.1 +200 OK` and `HTTP/1.1 0200 OK` both parse as 200. RFC 9112 §4:
`status-code = 3DIGIT`. (The chunk-size lexer got this exactly right —
`chunked.rs:290-309` rejects signs explicitly — so the status lexer is the
outlier.)

---

## Suggested fix ordering (for triage, no code written)

1. **H1-01** — add a Host↔request-target agreement gate on the H1 front
   (reuse `h2_proxy::check_authority_host_agreement`), feed the SNI check the
   same authority precedence the H2 path uses, and normalise the URI to
   origin-form before `send_request`. One decision to make: reject on
   disagreement (matches H2) vs honour §3.2.2 (absolute-form wins). Rejecting
   is the safer default and matches the "must reject, not silently pick one"
   policy.
2. **H1-02 / H1-03** — one gate: `headers.get_all(HOST).count()` must be
   exactly 1 for HTTP/1.1 (400 otherwise). Both are the same three lines.
3. **H1-04** — move the WS fork below the hooks / underscore / SNI / smuggle
   block, or hoist those four checks above it as ROUND8-L7-09 did.
4. **H1-05** — `parts.headers.remove(CONTENT_LENGTH)` in `finalize_response`
   when the upstream response carried `Transfer-Encoding` (mirror of F-MD-1).
5. **H1-06 … H1-18** — as scoped above.
