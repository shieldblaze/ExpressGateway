# S47 — HTTP/2 wire-protocol conformance review (RFC 9113 + RFC 7541)

**Scope:** `crates/lb-h2/src/{frame,hpack,security,error}.rs`,
`crates/lb-l7/src/{h2_proxy,h2_security,h2_to_h1,h2_to_h2,h2_to_h3,h1_to_h2}.rs`.
**Method:** static read + cross-read of the vendored `h2-0.4.14` / `hyper-1.10.1` /
`http-1.4.1` sources in `~/.cargo/registry` (the live codec), against the RFC texts
downloaded to the scratchpad. **Nothing was executed** — no cargo, no gateway boot.
Every claim below cites the line that produces it; the "verify" line on each finding
is the minimal experiment the lead can run in CI to confirm or refute it.

---

## 0. Liveness map — which H2 code is on the wire

This has to come first, because roughly half the code in scope is not reachable from
the binary and a finding against it is worth a fraction of one against the live path.

### 0.1 The live H2 stack

| Layer | Owner |
|---|---|
| H2 frame codec, HPACK, stream state machine (front) | `h2-0.4.14` via `hyper-1.10.1` `server::conn::http2` |
| H2 frame codec, HPACK (upstream) | `h2-0.4.14` via `hyper` client, behind `lb_io::http2_pool::Http2Pool` |
| Everything above the codec | `crates/lb-l7/src/h2_proxy.rs` (3099 lines, all live) |
| Thresholds onto the codec | `crates/lb-l7/src/h2_security.rs::apply` |
| H2→H2 / H2→H3 request head | `h2_to_h2.rs` / `h2_to_h3.rs` via `create_bridge` (`h2_proxy.rs:2155`, `:2511`) |
| H1→H2 request head | `h1_to_h2.rs` via `create_bridge` (`h1_proxy.rs:2114`) |

### 0.2 `crates/lb-h2` — test/fuzz-only (ALREADY-KNOWN)

`lb-l7` links `lb-h2` for exactly two `const`s
(`h2_security.rs:45,46,51,52` → `DEFAULT_SETTINGS_MAX_PER_WINDOW`,
`DEFAULT_ZERO_WINDOW_STALL_TIMEOUT`). `decode_frame`, `encode_frame`, `HpackDecoder`,
`HpackEncoder` and **every detector type** in `security.rs` have **zero production
call sites** — the only callers are `crates/lb-h2/tests/*`, `tests/*`, and
`fuzz/fuzz_targets/*`.

ALREADY-KNOWN: `docs/arch/overview.md:33`, `docs/architecture.md:120-124,204-205`,
`audit/security/s38-recon-leads.md`, `crates/lb-h2/tests/round8_padded_frame.rs:5`
("`decode_frame` is test-only today (the hot path uses hyper)").

### 0.3 `crates/lb-l7/src/h2_to_h1.rs` — ALSO test-only, and NOT recorded anywhere

`create_bridge(Protocol::Http2, Protocol::Http1)` → `H2ToH1Bridge` has exactly one
caller in the whole tree: `tests/bridging_h2_h1.rs:8`. The **live** H2→H1 leg is
`H2Proxy::proxy_request` (`h2_proxy.rs:1096`), which uses hyper's H1 client directly
and never touches `H2ToH1Bridge`.

This matters for the evidence trail, not for the security property:

- `SECURITY.md` and `audit/security/s38-findings-protocol.md:161-176` cite
  `h2_to_h1.rs:83`'s `check_h2_downgrade` and `authority_host_components_agree`
  (`h2_to_h1.rs:133`) as part of the "H2→H1 downgrade smuggling — CLEAN" proof.
  Both are in dead code.
- `crates/lb-l7/tests/h2_to_h1_pseudo_strip.rs` — the file whose name implies it
  covers the live H2→H1 strip — drives only `create_bridge(Http2, Http1)`.
- The property still holds on the live path via a **separate** call site:
  `h2_proxy.rs:793` `SmuggleDetector::check_all_mode(&header_pairs, SmuggleMode::H2)`
  and `h2_proxy.rs:805` `check_authority_host_agreement`. So: no security regression,
  but ~half the cited evidence is for code that cannot run.

**Recommend:** add the `// test-only` banner that `round8_padded_frame.rs:5` and
`lb-h3-testcodec` already carry, and re-point the SECURITY.md / s38 citations at
`h2_proxy.rs:793`/`:805`.

### 0.4 What the delegated codec already enforces (verified by reading h2-0.4.14)

Not findings — recorded so the lead can see which brief items are covered upstream
and need no gateway work:

| RFC 9113 rule | Enforced at |
|---|---|
| §8.2.1 uppercase field name ⇒ malformed | `hpack/header.rs:98` `HeaderName::from_lowercase` |
| §8.2.1 empty field name | `hpack/header.rs:65` |
| §8.3.1 unknown pseudo-header | `hpack/header.rs:94` `InvalidPseudoheader` |
| §8.3 pseudo after regular field / duplicate pseudo | `frame/headers.rs:857-865` `set_pseudo!` |
| §8.2.2 connection-specific fields + non-`trailers` TE (ingress) | `frame/headers.rs:892-905` |
| §8.2.2 connection-specific fields (egress head) | `hyper/proto/h2/mod.rs:43` `strip_connection_headers` (strips, does not reject) |
| §8.1.1 END_STREAM with non-zero content-length | `proto/streams/recv.rs:191-200` |
| §8.5 CONNECT pseudo-header rules | `server.rs:1684-1724` |
| §6.10 CONTINUATION interleave + stream-id match | `codec/framed_read.rs:137,291` |
| CVE-2024-27316 CONTINUATION flood | `codec/framed_read.rs:296-310` + `calc_max_continuation_frames` — with the gateway's `max_header_list_size = 64 KiB` / 16 KiB frames this caps at **5** CONTINUATIONs per block |
| CVE-2023-44487 rapid reset | `max_pending_accept_reset_streams` / `max_local_error_reset_streams`, both set to 100 at `h2_security.rs:45-46` |
| §5.2/§6.9 flow control, §5.1.1 stream-id monotonicity, §4.1 unknown frames, §4.2 frame size | h2 internals; the CI job `Conformance (h2spec --strict + h3spec)` (`.github/workflows/ci.yml:373-377`) covers these at 147/147 |

**Note on `h2_security.rs:8-9`** ("CONTINUATION flood … and PING flood are enforced
inside `h2` itself"): true and verified for CONTINUATION; for PING, h2 has **no rate
limit** — `PingPong::pending_pong` is a single `Option` slot
(`proto/ping_pong.rs:16-17,100`) and the caller must flush the PONG before reading the
next PING, so the flood is *structurally memory-bounded* but not rate-limited. See
H2-12.

---

## 1. Findings

### H2-01 · MEDIUM · H2→H1 egress emits an absolute-form request-target

**Where:** `crates/lb-l7/src/h2_proxy.rs:1102-1110` (`proxy_request`)

```rust
let (mut parts, mut body) = req.into_parts();
// F-MD-1 — THE CATCH: with `version == HTTP/2.0` and a stale
// `content-length`/`transfer-encoding`, hyper's http1 encoder
// MIS-FRAMES an unknown-length streaming body ...
parts.version = hyper::Version::HTTP_11;
parts.headers.remove(hyper::header::CONTENT_LENGTH);
parts.headers.remove(hyper::header::TRANSFER_ENCODING);
```

`parts.uri` is **not** touched here or anywhere downstream, and it is never
origin-formed before `sender.send_request(req)` (`:1200` branch A, `:1455` branch B).

**Why that produces absolute-form.** For an H2 request, h2 builds the URI from the
pseudo-headers with scheme *and* authority set (`h2-0.4.14/src/server.rs:1668-1706`:
`parts.authority = …`; `parts.scheme = Some(scheme)` whenever `parts.authority.is_some()`).
hyper's low-level H1 client then serialises it verbatim:

- `hyper-1.10.1/src/proto/h1/role.rs:1198` — `let _ = write!(FastWrite(dst), "{} ", msg.head.subject.1);`
- `http-1.4.1/src/uri/mod.rs:1032-1049` — `Display for Uri` writes `scheme://` then `authority` then `path`.
- hyper's own doc, `hyper-1.10.1/src/client/conn/http1.rs:194-203`:
  *"The `Uri` of the request is serialized as-is. — Usually you want origin-form
  (`/path?query`). — For sending to an HTTP proxy, you want to send in absolute-form
  … This is however not enforced or validated and it is up to the user of this method
  to ensure the `Uri` is correct."*
  I grepped hyper's `client/` and `proto/h1/` for any origin-form rewrite: the only
  `uri_mut()` is `dispatch.rs:619`, on the **server** side.

**Spec.** RFC 9112 §3.2.1: *"When making a request directly to an origin server, other
than a CONNECT or server-wide OPTIONS request …, a client MUST send only the absolute
path and query components of the target URI as the request-target."*
RFC 9112 §3.3 then makes it load-bearing: an origin server receiving absolute-form
**MUST ignore the Host header field and use the host information of the request-target**.

**Concrete failure.** Client opens H2 to the gateway with
`:method GET`, `:scheme https`, `:authority app.example`, `:path /admin`.
Backend is an `h1` upstream. The gateway writes:

```
GET https://app.example/admin HTTP/1.1
host: app.example
...
```

- Origins that do not implement absolute-form (Python `http.server`, many WSGI/FastCGI
  front-ends, several embedded servers) treat the whole string as the path → 404/400
  for **every** request through an H2 front. Invisible in this repo's tests because
  every H2→H1 test backend is a hyper H1 server, which accepts absolute-form
  (`tests/h2_proxy_e2e.rs:100-124`, `tests/h2h1_md_streaming_verify.rs:180-212` —
  both `service_fn` handlers that never inspect the request target).
- Conforming origins take the routing key from the client-supplied `:authority` and
  **ignore** the gateway's Host — moving the vhost-selection decision to the field the
  gateway's PROTO-2-01 / SNI checks were built to constrain.

**Existing test coverage:** none. No test in `tests/` or `crates/lb-l7/tests/`
asserts the backend-observed request line for the H2→H1 leg; the CI h2spec job proxies
to `python3 -m http.server` but h2spec asserts frames, not backend status.

**Verify (CI, ~5 lines):** in `tests/h2_proxy_e2e.rs` add a raw-`TcpListener` backend
that reads until `\r\n` and asserts the first line starts with `GET /`. Expected to
fail today with `GET https://…`.

---

### H2-02 · MEDIUM · `Host` is never replaced by `:authority`, and duplicate `Host` fields are forwarded

**Where:** `crates/lb-l7/src/h2_proxy.rs:885-893`

```rust
if let Some(h) = authority.as_deref() {
    set_xfh(headers, h);
    // An H1 upstream requires `Host`; synthesise it from `:authority`.
    if !headers.contains_key(hyper::header::HOST) {
        if let Ok(v) = HeaderValue::from_str(h) {
            headers.insert(hyper::header::HOST, v);
        }
    }
}
```

**Spec.** RFC 9113 §8.3.1, verbatim:

> An intermediary that needs to generate a Host header field (which might be necessary
> to construct an HTTP/1.1 request) **MUST use the value from the ":authority"
> pseudo-header field as the value of the Host field**, unless the intermediary also
> changes the request target. **This replaces any existing Host field to avoid
> potential vulnerabilities in HTTP routing.**

The code does the opposite of "replaces": it writes the `:authority` value **only when
`Host` is absent**.

**Why the agreement check does not cover it.** `check_authority_host_agreement`
(`h2_proxy.rs:2624-2645`) reads `headers.get(hyper::header::HOST)` — `HeaderMap::get`
returns the **first** value only. h2 stores repeated regular fields with `append`, not
`insert` (`h2-0.4.14/src/frame/headers.rs:909` `self.fields.append(name, value)`), and
`host` is a regular field over H2. `crate::authority::validate_request`
(`authority.rs:28-44`) likewise iterates `[uri.authority(), headers.get(HOST)]` — first
only. So a **second, differing** `Host` is never seen by any gate and is never removed:
`strip_hop_by_hop` (`h1_proxy.rs:2021-2040`) does not touch `host`, and the
`contains_key` guard above short-circuits.

**Concrete failure.** H2 HEADERS containing
`:authority: app.example`, `host: app.example`, `host: internal-admin`:

1. `authority::validate_request` — validates `app.example` (first Host). Pass.
2. `check_authority_host_agreement` — `:authority` vs first `Host`. Match. Pass.
3. `check_sni_authority` — compares SNI against `:authority`. Pass.
4. `security_hooks::inspect_request` (`h2_proxy.rs:778`) rebuilds the request with
   **both** hosts; a WAF hook doing `headers().get(HOST)` sees only `app.example`.
5. H2→H1 leg: hyper writes every `(name, value)` pair as its own line
   (`hyper-1.10.1/src/proto/h1/role.rs:1593-1600` — `for (name, value) in headers`), so
   the upstream receives:

```
GET https://app.example/ HTTP/1.1
host: app.example
host: internal-admin
```

RFC 9112 §3.2: *"A server MUST respond with a 400 (Bad Request) status code … to any
request message that contains more than one Host header field line."* An upstream that
instead takes the **last** Host (or a further intermediary that does) routes to
`internal-admin` while every gateway-side check authorised `app.example`.

On the H2→H2 and H2→H3 legs the same duplicate survives:
`build_h2_upstream_request_parts` (`h2_proxy.rs:2176-2186`, `:2219-2222`) and
`build_h2_to_h3_fieldlist` (`:2516-2527`) both iterate `parts.headers.iter()` and
`builder.header(...)` / push, which append.

Note the **dead** `H2ToH1Bridge` at `h2_to_h1.rs:66-80` is the code that *tries* to do
this right ("Drop the existing Host so the inserted one is the sole entry") — but it
uses `.find(...)` and so also removes only the first, leaving a duplicate. The correct
shape is `headers.insert(HOST, authority)` unconditionally (`HeaderMap::insert` removes
**all** existing values for the name).

**Existing test coverage:** none. `crates/lb-l7/tests/h2_authority_host_mismatch.rs`
builds its `HeaderMap` with a single `HeaderMap::insert` (`:11-17`) — no duplicate-Host
case anywhere in the tree.

**Verify:** send an H2 HEADERS block with two `host` literal-header entries via the
`h2` crate (or the raw-HPACK helper already in `tests/ws_h2_conformance.rs:305-311`)
and assert the raw backend sees exactly one `host:` line.

---

### H2-03 · MEDIUM · Classic (non-extended) CONNECT is relayed to the backend with a client-chosen authority

**Where:** `crates/lb-l7/src/h2_proxy.rs:665-940` (`handle_inner`) — there is no
`Method::CONNECT` arm.

The only CONNECT interception is gated on the **extended** form:

```rust
// h2_proxy.rs:688-697
if self.h2_extended_connect_enabled
    && self.ws.as_ref().is_some_and(|w| w.config().enabled && is_h2_extended_connect(&req))
{ return self.handle_ws_extended_connect(req).await; }
```

and `is_h2_extended_connect` (`ws_proxy.rs:118-125`) requires the
`hyper::ext::Protocol` extension. A classic CONNECT (`:method CONNECT`, `:authority
evil.example:443`, **no** `:scheme`, **no** `:path`, no `:protocol`) has no extension,
so it falls straight through to the generic proxy path.

**What reaches the backend.**
- h2 accepts it: `h2-0.4.14/src/server.rs:1684-1723` — for CONNECT without `:protocol`,
  `:scheme` and `:path` are *forbidden*, `:authority` is the URI.
- hyper's H2 server takes the CONNECT branch (`proto/h2/server.rs:277-303`), hands the
  service an empty body, and stashes an `OnUpgrade` the gateway never takes.
- `authority::validate_request` passes (`evil.example:443` has no comma/space/CTL).
- `check_authority_host_agreement` passes (no `Host`).
- SNI check: on an `h1s` listener with SNI present this **does** 421 a mismatched
  authority; with no SNI (`expected_sni == None`) or a loopback peer it is skipped
  (`h2_proxy.rs:829`).
- H2→H1 (`proxy_request`): `Display for Uri` on an authority-only URI yields the bare
  authority (`http-1.4.1/src/uri/mod.rs:1038-1042`; `path()` is `""` when there is no
  scheme and no path data), so hyper writes literally
  `CONNECT evil.example:443 HTTP/1.1` plus the `Host: evil.example:443` the gateway
  synthesised at `:885-893`.
- H2→H2: `build_h2_upstream_request_parts` synthesises `http://evil.example:443/`, and
  h2's client normalises it back — `Pseudo::request` drops scheme+path when
  `method == CONNECT && protocol.is_none()` (`h2-0.4.14/src/frame/headers.rs:561-563`)
  — so a well-formed classic H2 CONNECT with the attacker's `:authority` goes upstream.

**Spec.** RFC 9110 §9.3.6 / RFC 9113 §8.5: CONNECT requests a tunnel to the authority
named in `:authority`. A reverse proxy that does not implement tunnelling should reject
(405/501), not forward. Forwarding turns any backend that *does* implement CONNECT
(an internal egress proxy, a mesh sidecar with a CONNECT listener, Squid, mitmproxy)
into an unauthenticated tunnel endpoint reachable through the gateway.

**Prior art:** raised in `audit/protocol/round-1-inventory.md:287-297` —
*"a deployed CONNECT request from the client to the LB **must not** tunnel to an
arbitrary host. Cross-check with `sec`."* I could not find any round-2/5/6 resolution,
no entry in `audit/deferred.md`, none in `docs/known-limitations.md`, none in
`SECURITY.md`. It appears to have been dropped rather than closed.

**Existing test coverage:** none. Every CONNECT test in the tree
(`ws_h2_gated_off.rs:187`, `ws_h2_conformance.rs:187,382,481`, `ws_h2_e2e.rs:361`, …)
attaches `h2::ext::Protocol::from_static("websocket")`. The one test that asserts
"backend never dialled" (`ws_h2_gated_off.rs:224-229`) is the **extended** case, which
h2 already rejects at `proto/streams/recv.rs:236-241` when connect-protocol is off.
h2spec 2.6.0 has no CONNECT case.

**Verify:** `h2::client` `send_request` with `Method::CONNECT` and an authority-only
URI, no `Protocol` extension, against an accept-counting backend; assert 0 dials.

---

### H2-04 · MEDIUM · The H2 glitch counter / ENHANCE_YOUR_CALM drain is never armed in the shipped binary

**Where:** `crates/lb-l7/src/h2_proxy.rs:361-374` (`with_glitches`) vs
`crates/lb/src/main.rs:1236-1259` (`build_h2_proxy`).

`build_h2_proxy` chains `with_hooks`, `with_health`, `with_watchdog`, `with_h2_upstream`,
`with_h3_upstream`, `with_websocket`, `with_h2_extended_connect`, `with_grpc` — and
**never** `with_glitches`. Repo-wide, `with_glitches` has exactly one caller:
`crates/lb-l7/tests/round8_glitches_enforced.rs:45`. There is no config key either —
`grep -rn -i glitch crates/lb-config/ config/ crates/lb/src/` returns nothing.

Consequence in the running gateway: `self.glitches_threshold` is `None`, so
`glitch_state` at `h2_proxy.rs:500-515` is `None`, so all five
`if let Some(g) = glitch { g.record(...) }` sites (`:683`, `:740`, `:797`, `:808`,
`:838`) are no-ops, the `h2_glitches_total` counter is never registered with the
Prometheus registry, and the two-step GOAWAY drain arm at `:545-556` can only ever be
reached by SIGTERM — never by protocol abuse.

**Why this is a finding and not just dead code:** `audit/deferred.md:166-180` states

> The COUNTER half (the actual HAProxy `tune.h2.fe.glitches-threshold` pattern) is now
> **fully WIRED** … on threshold-crossing cancels the connection drain token → the
> existing two-step GOAWAY path (logical ENHANCE_YOUR_CALM). … the operator knob and
> Prometheus surface are in place ahead of it.

and `audit/round-8/findings/ROUND8-L7-07.md:7` closes the push-back with
*"Theme-1 'library shipped no caller' resolved."* Both are false at the binary
boundary: there is no operator knob, and the Prometheus surface does not exist in a
running gateway. The round-8 verifier confirmed the *library* wiring and the test, and
did not check `main.rs`.

**Existing test coverage:** `round8_glitches_enforced.rs` passes — it constructs
`H2Proxy` directly and calls `with_glitches` itself, so it can never catch this.

**Verify:** boot the gateway from `config/` and `curl -s localhost:9090/metrics | grep h2_glitches_total` → absent.

---

### H2-05 · LOW · Response trailers are forwarded to the H2 client without hop-by-hop / forbidden-field filtering

**Where:** `crates/lb-l7/src/h2_proxy.rs:2076-2087`

```rust
lb_quic::H3RespEvent::Trailers(t) => {
    let mut tm = hyper::HeaderMap::new();
    for (n, v) in &t {
        if let (Ok(name), Ok(val)) = (
            hyper::header::HeaderName::from_bytes(n.as_bytes()),
            HeaderValue::from_str(v),
        ) { tm.append(name, val); }
    }
    let _ = btx.send(Ok(Frame::trailers(tm))).await;
}
```

The only filter is name/value *syntax*. Pseudo-headers happen to be excluded as a side
effect (`':'` maps to `0` in `http-1.4.1/src/header/name.rs:1016`, so
`HeaderName::from_bytes(b":status")` errors) — but `transfer-encoding`, `connection`,
`keep-alive`, `upgrade`, `te`, `content-length`, `host` all parse fine and are appended.

**Nothing downstream strips them.** hyper strips connection headers only from the
response **head** (`hyper-1.10.1/src/proto/h2/server.rs:474`
`strip_connection_headers(res.headers_mut(), false)`); the trailer path
(`hyper-1.10.1/src/proto/h2/mod.rs:219-225`) calls `me.body_tx.send_trailers(...)` with
no strip, and h2's `Send::send_trailers` (`proto/streams/send.rs:312-335`) — unlike
`send_headers` at `:139` — never calls `check_headers`. So the field reaches the wire.

**Spec.** RFC 9113 §8.2.2: *"An endpoint MUST NOT generate an HTTP/2 message containing
connection-specific header fields."* RFC 9110 §6.5.1 additionally bars
`Transfer-Encoding`, `Content-Length`, `Host`, and control data from a trailer section.

**Concrete failure.** An H3 origin answers with a trailer section containing
`transfer-encoding: chunked`. The gateway emits it in the terminal HEADERS frame; the
downstream client's own h2 decoder flags `malformed = true`
(`h2-0.4.14/src/frame/headers.rs:892-899`) and resets the stream with PROTOCOL_ERROR
**after** a fully successful body transfer. Attribution lands on the gateway.

Reachability is H3-upstream-specific: an H2 upstream cannot deliver such a trailer
(h2's ingress `HeaderBlock::load` rejects it), and the H3 connector forwards trailers
verbatim (`lb-quic/src/h3_bridge.rs:1486-1500`, no filtering). The H1 front has the
identical code shape at `h1_proxy.rs:1684-1695` but hyper's H1 encoder drops trailers on
a streamed response anyway (documented limitation), so H2 is where it actually lands.

**Existing test coverage:** none — `crates/lb-l7/tests/trailer_passthrough.rs` asserts
propagation, not filtering. S38's `L-PROTO-3` "trailer / response splitting — CLEAN"
verdict (`audit/security/s38-findings-protocol.md:189-204`) covers CRLF/NUL injection
only; a syntactically valid `transfer-encoding` trailer is outside what it proved.

---

### H2-06 · LOW · HPACK dynamic-table size update is unbounded (RFC 7541 §6.3)

**Where:** `crates/lb-h2/src/hpack.rs:379-383`

```rust
} else if first & 0xE0 == 0x20 {
    let (new_size, consumed) =
        decode_integer(buf.get(pos..).ok_or(H2Error::Incomplete)?, 5)?;
    pos += consumed;
    self.dynamic.set_max_size(new_size);
}
```

`HpackDecoder::new(max_table_size)` (`:314`) stores the protocol limit only as the
*initial* `max_size`; it is never retained for comparison, so a peer-supplied update
overwrites it with any value the integer decoder returns (up to ~2^35, see H2-14).

**Spec.** RFC 7541 §6.3: *"The new maximum size MUST be lower than or equal to the limit
determined by the protocol using HPACK … A value that exceeds this limit MUST be treated
as a decoding error."* Also unenforced: §4.2's rule that a size update must appear at the
start of a field block, and the at-most-two-updates limit.

**Concrete failure.** A block beginning `3F E1 FF FF FF 0F` (5-bit prefix size update,
value ≈ 4 G) sets `max_size` to 4 G; every subsequent literal-with-incremental-indexing
entry is then retained instead of evicted. `crates/lb-h2/src/hpack.rs:110-118` (`evict`)
can never fire, so the table grows with the input at roughly 48-100 heap bytes per
3 input bytes.

**Why it is a finding despite being test-only:** `fuzz/fuzz_targets/h2_hpack.rs:12-13`
states the invariant explicitly — *"4096 is the RFC 7541 default
SETTINGS_HEADER_TABLE_SIZE; **the decoder must keep its dynamic table within this
regardless of attacker input**"* — and `:9` names OOM as the finding. The invariant the
target asserts is not held by the code it drives. Bounded in practice by libFuzzer's
default `max_len`, which is why the target has not flagged it.

**Existing test coverage:** none. `hpack_dynamic_table_eviction` (`hpack.rs:448`) drives
eviction through the encoder API, never through a wire size update.

---

### H2-07 · LOW · HPACK decoder cannot decode Huffman string literals (RFC 7541 §5.2)

**Where:** `crates/lb-h2/src/hpack.rs:243-253`

```rust
/// Decode a string literal; H=1 is treated as raw, best-effort.
fn decode_string(buf: &[u8]) -> Result<(String, usize), H2Error> {
    let (len, int_bytes) = decode_integer(buf, 7)?;
```

`decode_integer(buf, 7)` masks with `max_prefix = 127`, so the H bit (0x80) is consumed
as part of the length prefix and **never inspected**. A Huffman-coded literal is read as
`len` raw octets and then pushed through `core::str::from_utf8` (`:250`), which usually
fails (`HpackError("non-utf8 string")`) and occasionally succeeds with a wrong value.

**Spec.** RFC 7541 §5.2: the H bit selects the encoding, and a decoder must Huffman-decode
when H=1; §5.2 further requires that a padding of more than 7 bits, or any padding not
matching the EOS prefix, or the EOS symbol itself, be treated as a decoding error. None
of that exists.

**Why it matters despite being test-only:**
1. The module doc at `hpack.rs:3-4` claims *"Huffman encoding is deliberately NOT used —
   the H bit is always 0. Fully compliant; Huffman is optional."* Optional for an
   *encoder*; mandatory for a *decoder*. The "fully compliant" claim is wrong.
2. `fuzz/fuzz_targets/h2_hpack.rs:7` lists *"Huffman vs raw string literals"* as a fuzz
   target — that arm is unreachable, so the fuzz claim is vacuous.
3. curl, nghttp2 and every browser Huffman-encode by default, so this codec could not be
   promoted to the data path without a rewrite. Worth pinning before someone tries.

---

### H2-08 · LOW · `decode_frame` performs no per-frame-type stream-id validation, and discards the stream id where it matters most

**Where:** `crates/lb-h2/src/frame.rs:395-421` (`decode_frame`), `:284-384`
(`decode_frame_high`), `:66-104` (the `Settings` / `Ping` / `GoAway` variants).

`parse_frame_header` masks the reserved bit correctly (`:188`, per §4.1 "MUST be
ignored"), but nothing checks the stream id against the frame type:

| RFC 9113 rule | Status |
|---|---|
| §6.1 DATA on stream 0x0 ⇒ connection PROTOCOL_ERROR | accepted, `stream_id: 0` returned |
| §6.2 HEADERS on 0x0 / §6.3 PRIORITY on 0x0 / §6.4 RST_STREAM on 0x0 / §6.10 CONTINUATION on 0x0 | accepted |
| §6.5 SETTINGS on non-zero stream ⇒ PROTOCOL_ERROR | accepted **and unrepresentable** — `H2Frame::Settings { ack, params }` has no `stream_id` field (`:66-72`) |
| §6.7 PING on non-zero stream | same — `H2Frame::Ping { ack, data }` (`:82-88`) |
| §6.8 GOAWAY on non-zero stream | same — `H2Frame::GoAway { … }` (`:89-97`) |
| §6.6 PUSH_PROMISE on 0x0 | accepted |

The last three are the sharp edge: because the variants drop the id, a caller that
*wanted* to enforce §6.5/§6.7/§6.8 could not, short of re-parsing the header. Any future
promotion of this codec inherits a connection-level-error class that is structurally
unreachable.

Related, same function: `H2Error` carries no HTTP/2 error code and no
connection-vs-stream distinction, so §5.4.1/§5.4.2 ("a frame size error in a frame that
could alter the state of the entire connection MUST be treated as a connection error")
cannot be expressed either — every failure is a flat `InvalidFrame(String)`.

**Existing test coverage:** none. `frame.rs`'s own tests and
`crates/lb-h2/tests/round8_h2_cve_corpus.rs` only exercise stream ids 1, 3, 7, 42.

---

### H2-09 · LOW · PUSH_PROMISE ignores the PADDED flag, mis-parsing the promised stream id

**Where:** `crates/lb-h2/src/frame.rs:313-325`

```rust
FRAME_PUSH_PROMISE => {
    if payload.len() < 4 { return Err(...); }
    let promised_raw = read_u32_be(payload.get(0..4).ok_or(H2Error::Incomplete)?)?;
    let promised_id = promised_raw & 0x7FFF_FFFF;
    let header_block = Bytes::copy_from_slice(payload.get(4..).ok_or(H2Error::Incomplete)?);
```

There is no `flags & FLAG_PADDED` branch, even though `FLAG_PADDED`'s own doc comment at
`:21-22` claims coverage: *"PADDED flag (RFC 9113 §6.1 DATA, §6.2 HEADERS, §6.6
PUSH_PROMISE)"*. DATA (`:209-213`) and HEADERS (`:224-228`) both call `strip_padding`;
PUSH_PROMISE does not.

**Spec.** RFC 9113 §6.6: with PADDED set the payload is
`Pad Length (8) | R+Promised Stream ID (32) | Field Block Fragment | Padding`.

**Concrete failure.** PUSH_PROMISE, `flags = 0x08`, `pad_len = 4`, promised id 2:
payload `04 00 00 00 02 <block> 00 00 00 00`. The decoder reads
`read_u32_be([04,00,00,00]) & 0x7FFF_FFFF` = **67 108 864** instead of 2, and appends the
4 NUL padding bytes to the header block fragment, corrupting the HPACK stream for the
whole connection.

**Existing test coverage:** none — `crates/lb-h2/tests/round8_padded_frame.rs` covers
DATA and HEADERS+PRIORITY only.

---

### H2-10 · LOW · WINDOW_UPDATE with a zero increment is accepted

**Where:** `crates/lb-h2/src/frame.rs:358-370` — length is checked (4 bytes) and the
reserved bit masked, but `increment == 0` is returned as a valid frame.

**Spec.** RFC 9113 §6.9: *"A receiver MUST treat the receipt of a WINDOW_UPDATE frame with
a flow-control window increment of 0 as a stream error … of type PROTOCOL_ERROR; errors on
the connection flow-control window MUST be treated as a connection error."*

Same class as H2-08 (no stream/connection error distinction available to express it).
Note `ZeroWindowStallDetector::on_window_update` (`security.rs:341-346`) already treats a
zero increment as "not progress", so the intent exists one layer up; the codec just does
not reject it.

---

### H2-11 · LOW · `informational_responses.rs` — 5 tests whose names claim forwarding, asserting only `StatusCode` properties

**Where:** `crates/lb-l7/tests/informational_responses.rs` (whole file, 55 lines).

`test_100_continue_forwarded` (`:8`) and `test_103_early_hints_forwarded` (`:16`) assert
`status.is_informational()` and `status.as_u16() == 100 / 103`. They construct no request,
boot no listener, and touch no `lb_l7` symbol. `hyper_h1_server_handles_expect_100_continue_internally`
(`:51`) has a body of `let _ = "documented baseline";`. The module doc at `:1-3` says the
opposite of the test names: *"103 Early Hints is DROPPED on H1→H1."*

The 1xx gap itself is **ALREADY-KNOWN and deferred**: `audit/deferred.md:120-137`
(PROTO-2-03). Two notes on that entry:

- It cites this file as *"(5 tests) pins the status-class invariants today"* — accurate as
  written, but a CI reader seeing `test_103_early_hints_forwarded ... ok` will conclude the
  opposite. Rename to `..._is_1xx_class` (or mark `#[ignore]` with the PROTO-2-03 link).
- The entry's rationale says *"RFC 9110 §15.2 / RFC 8297 say MAY forward"*. RFC 9110
  §15.2.1 is stronger for the 100 case, and the gateway does forward `Expect:
  100-continue` upstream (`strip_hop_by_hop`'s `HOP_BY_HOP` set at `h1_proxy.rs:37-46`
  does not include `expect`), so it is the party that must relay the 100 back. Worth
  re-checking the normative strength before the deferral is renewed.

---

### H2-12 · LOW · `SECURITY.md` rows 8 and 9 name SETTINGS/PING flood policies that no live code enforces

**Where:** `SECURITY.md:49-50`; `crates/lb-l7/src/h2_security.rs:41-79`.

Row 8 claims *"SETTINGS flood … `crates/lb-h2/src/security.rs::SettingsFloodDetector`
(100 / 10 s)"*; row 9 claims *"PING flood … `PingFloodDetector` (50 / 10 s)"*. Both
"Reference" columns point at unit tests of the detector types, which have no production
call site (§0.2). The preamble note at `SECURITY.md:31-38` hedges by saying the live
thresholds are *"applied on the live hyper H2 builder
(`crates/lb-l7/src/h2_security.rs::apply`)"* — but `apply` (`:67-78`) sets nine knobs and
**none of them is a SETTINGS or PING rate limit**; hyper's `http2::Builder` exposes no such
knob, and h2 0.4.14 implements neither (`proto/ping_pong.rs` has no counter; §0.4 note).
`DEFAULT_SETTINGS_MAX_PER_WINDOW` is reused at `h2_security.rs:45-46` as the value for the
two **reset-stream** caps — a numeric reuse, not a SETTINGS limit.

The residual posture is not zero: both floods are structurally memory-bounded (one pending
PONG slot, one pending SETTINGS-ACK slot, each flushed before the next frame is read), and
`HttpTimeouts::total` bounds the connection. But the specific published policies are not in
force. Rows 8/9 should say so, the same way the WS-over-H2 row in `docs/features.md:52`
does.

Corroborating: the two tests named for these attacks are honest in their comments but do not
assert the policies — `tests/h2_security_live.rs:351-358` asserts
`max_concurrent_streams` advertisement, not a SETTINGS rate; `:401-405` says outright
*"If GOAWAY doesn't arrive within the bound we accept that the connection simply died —
absence of a crash is the invariant we care about."*

---

### H2-13 · LOW · H1→H2 / H1→H3 upstream `:scheme` is hard-coded `"http"`

**Where:** `crates/lb-l7/src/h1_proxy.rs:2131` — `scheme: Some("http".to_owned())` in
`build_h1_to_h2_upstream_parts`, which `h1_to_h2.rs:48,51` then writes into `:scheme`.

`is_https` is available on the proxy and is already used for `X-Forwarded-Proto`
(`h2_proxy.rs:884` / the H1 equivalent), so the correct value is in scope.

**Spec.** RFC 9113 §8.3.1: *"The ':scheme' pseudo-header field includes the scheme portion
of the request target. The scheme is taken from the target URI … or from the scheme of a
translated request."* For a TLS-terminated H1 front the translated scheme is `https`.

**Impact:** an origin that builds absolute URLs (`Location`, canonical links, HSTS
decisions) from `:scheme` emits `http://` for a client that arrived over TLS —
the classic redirect-downgrade. Mitigated in practice by `X-Forwarded-Proto`, which is set
correctly, so this is LOW and configuration-dependent.

For contrast the H2 fronts are correct: `build_h2_upstream_request_parts:2156-2159` reads
`parts.uri.scheme()` (always present on an H2 request, since h2 requires `:scheme`), and
`build_h2_to_h3_fieldlist:2494-2497` defaults to `https`.

---

## 2. INFO (recorded, no action implied)

- **`crates/lb-h2/src/hpack.rs:220-233`** — `decode_integer`'s `if m > 28` guard runs
  *after* the shift for that iteration, so the accepted range is ~2^35, not 2^28.
  Harmless on 64-bit (the value only ever feeds `table_get`, which bounds-checks, or
  `buf.get(start..end)`, which returns `None`); on a 32-bit target `value +=
  (b & 0x7F) << 28` overflows `usize` — a debug-build arithmetic panic that
  `deny(clippy::panic)` does not catch. Only matters if a 32-bit target is ever added.
- **`crates/lb-h2/src/security.rs:317-365`** — `ZeroWindowStallDetector.last_progress` is
  an uncapped `HashMap<u32, Instant>`; `on_window_update` inserts for any stream id and
  only `remove_stream`/`reset` shrink it. No caller, so no growth today.
- **`crates/lb-h2/src/security.rs:153-177`** — `HpackBombDetector::check(0, n)` skips the
  ratio test entirely (`checked_div` returns `None` and the second branch is
  `if let Some`), so a zero-encoded-size input is only caught by the absolute cap.
- **`crates/lb-l7/src/h2_proxy.rs:2547-2549`** — the doc comment says hyper's H2 server
  *"already rejects connection-specific headers on egress"*. It **strips** them
  (`hyper/proto/h2/mod.rs:43-80`, with a `warn!`), it does not reject. The security
  property is the same or better; only the wording is off. Relevant because H2-05 shows
  the strip does **not** extend to trailers.
- **`crates/lb-l7/src/h2_proxy.rs:784-792`** — the `to_str().ok()` filter feeding
  `SmuggleDetector` drops any header whose value is not visible-ASCII.
  **ALREADY-KNOWN**: `audit/security/s38-findings-protocol.md:44-91` (F-PROTO-01, LOW,
  disposition "not a security finding"). I re-derived the same conclusion independently:
  h2's ingress `HeaderBlock::load` (`frame/headers.rs:892-905`) rejects every header the
  detector cares about before the detector runs, so the H2 front is not exposed.

## 3. Checked and clean

- **Hop-by-hop request strip** — `strip_hop_by_hop` (`h1_proxy.rs:2021-2040`) covers the
  RFC 9110 §7.6.1 set *and* the `Connection`-listed extras, is enforced at the type level
  by `StrippedRequest` (`stripped_request.rs`), and `te: trailers` is correctly re-added
  on the gRPC leg only (`grpc_proxy.rs:128-132`).
- **Request trailers** — `validate_request_trailers` (`h2_proxy.rs:2254-2273`) rejects
  pseudo-headers before forwarding, on all three egress legs, and on rejection injects
  `PumpAbort` rather than closing cleanly (`:1416`, `:1844`, `:2012`) so a
  truncated request can never be presented upstream as complete. This is the F-MD-4 work
  and it reads as correct.
- **`:authority`/Host agreement and SNI/authority 421** — `h2_proxy.rs:805` and `:829`;
  IPv6-bracket-aware, port-elision tolerant, ordered smuggle → authority/Host → SNI as
  documented. The loopback and no-SNI carve-outs are explicit in the code
  (`sni_authority.rs:33-45`) and correct for a co-defence. Connection coalescing is
  handled by exactly this 421 (RFC 9110 §15.5.20); the gateway sends no ORIGIN frame,
  which is the right conservative default.
- **Extended CONNECT (RFC 8441)** — the `:scheme`+`:path` requirement is enforced ahead of
  any dial (`h2_proxy.rs:967-984`), the gate is checked before the SETTINGS bit is
  advertised, and the upstream handshake completes before the 200 (F-S27-1).
- **Two-step GOAWAY on drain + `CleanCloseIo`** — `h2_proxy.rs:543-560` and `:131-236`.
  The FIN-then-bounded-drain ordering is correct for RFC 9113 §6.8 (the GOAWAY must not be
  discarded by an RST), and both the byte cap and the linger deadline are hard-bounded.
- **Flow control / backpressure** — the bounded lookahead
  (`H2_REQ_CHANNEL_DEPTH × H2_REQ_CHUNK_MAX` = 64 KiB) plus the `mpsc` chain gives real
  end-to-end backpressure; the retained-bytes gauge at `:1305-1313` measures live
  occupancy rather than a constant.
