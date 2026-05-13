# Round 1 Inventory — `proto`

Owner: `proto` (HTTP/1.1, HTTP/2, HTTP/3, QUIC, TLS, WS, gRPC conformance).
Scope: discovery only. No source changes. All file:line references are to
`/home/ubuntu/Code/ExpressGateway` at branch `main`, commit `ac58f61`.

Tooling check on this host: `curl` present; `h2spec`, `h3spec`, `nghttp2`,
`wstest` **NOT** on `PATH`. All conformance-tool integrations therefore
**graceful-skip at runtime** (see §7). Findings below are sourced from
reading the code + tests.

---

## 1. Protocol matrix

Per-protocol parser / framing / library / RFC. "Library" is the external
crate the LB depends on for the wire layer; "in-tree codec" is a
hand-rolled codec the LB *also* maintains (used by fuzz harnesses and
some security detectors, but the server data path goes through the
library row).

| Protocol | Listener side | Upstream side | In-tree codec crate | Wire library | Governing RFC(s) |
|---|---|---|---|---|---|
| HTTP/1.1 (h1) | `lb` ListenerMode::H1 (`main.rs:738`) → `lb-l7::h1_proxy::H1Proxy` (`hyper::server::conn::http1`) | `lb-io::pool::TcpPool` + `hyper::client::conn::http1` from `h1_proxy.rs` | `lb-h1` (parse.rs, chunked.rs) | hyper 1.x server + client | RFC 9110, 9112 (formerly 7230/7231) |
| HTTP/1.1 over TLS (h1s) | `ListenerMode::H1s` ALPN-dispatch (`main.rs:770-836`, ALPN `h2, http/1.1`) | same as h1 | `lb-h1` | hyper + tokio-rustls 0.26 | RFC 9110, 8446 |
| HTTP/2 (h2 over TLS) | `lb-l7::h2_proxy::H2Proxy` (`hyper::server::conn::http2`) — selected when ALPN==h2 (`main.rs:1161-1167`) | hyper client h2 via `lb-io::http2_pool::Http2Pool` | `lb-h2` (frame.rs, hpack.rs, security.rs) | hyper 1.x (which wraps the `h2` 0.4 crate) | RFC 9113, 7541 (HPACK) |
| HTTP/2 cleartext (h2c) | **Not supported.** `ListenerMode` enum has no h2c variant; `serve_connection` for the H1 listener does not look for `HTTP2-Settings` / `Upgrade: h2c`. | n/a | n/a | n/a | RFC 9113 §3.1 |
| HTTP/3 (h3 over QUIC) | `lb-quic::QuicListener` → `router.rs` → `conn_actor.rs` → `h3_bridge.rs` | `lb-quic::QuicUpstreamPool` for H3→H3; H1/H2 upstreams reachable from H3 listener via translation in `h3_bridge.rs` | `lb-h3` (frame.rs, qpack.rs, varint.rs, security.rs) | quiche 0.28 (BoringSSL) + tokio-quiche 0.18; HTTP/3 layer is hand-rolled in `h3_bridge.rs` (no quiche::h3 dependency surfaced) | RFC 9114, 9204 (QPACK), 9000-9002 |
| QUIC transport | `lb-quic::QuicListener::spawn` (UDP socket + retry-token signer + `ZeroRttReplayGuard`) — `listener.rs:1-30`, `router.rs:121` | client side from `QuicEndpoint` (`lib.rs`) and `QuicUpstreamPool` | `lb-quic::varint` etc. (mostly in `lb-h3`) | quiche 0.28 | RFC 9000 / 9001 / 9002 / 9221 |
| TLS 1.2 / 1.3 (over TCP) | `build_tls_stack` (`main.rs:214`), `build_h1s_tls_stack` (`main.rs:611`) — rustls 0.23 with **ring** provider; protocol versions = `with_safe_default_protocol_versions()` (`ticket.rs:327`). Both TLS 1.2 and TLS 1.3 enabled because rustls feature `tls12` is on (Cargo.toml). | tokio-rustls connector via hyper. | n/a | rustls 0.23 + tokio-rustls 0.26 + ring | RFC 8446 (TLS 1.3), RFC 5246 (TLS 1.2 — feature-gated on), RFC 8470 (early data) |
| TLS for QUIC | BoringSSL inside quiche; PEM cert + key loaded from `lb-quic::listener` / `lib.rs:392`. Set `application_protos = [LB_QUIC_ALPN]` for the loopback transport-only path; the real H3 listener should set `b"h3"`. | quiche client side. | n/a | quiche / BoringSSL | RFC 9001 |
| WebSocket (RFC 6455) | `lb-l7::ws_proxy::WsProxy` — H1 `Upgrade: websocket` invoked by H1Proxy; H2 RFC 8441 extended CONNECT invoked from H2Proxy (`h2_proxy.rs:284-292`, `ws_proxy.rs:174`). | Client-side handshake via tokio-tungstenite 0.24, then frame-forward. | n/a | tokio-tungstenite 0.24 + futures-util | RFC 6455, RFC 8441 |
| WebSocket over QUIC (RFC 9220) | **Not supported.** Commented as post-v1 in `ws_proxy.rs:34`. | n/a | n/a | n/a | RFC 9220 |
| gRPC | `lb-l7::grpc_proxy::GrpcProxy` invoked from H2Proxy when `Content-Type: application/grpc*` and method == POST. **Requires HTTP/2** end-to-end (the proxy hard-bails on H3 backends — `h2_proxy.rs:307-311`). | hyper H2 client. Backend must be H1+POST? No — see `grpc_proxy.rs:1-30`: gRPC over H2 only. | `lb-grpc` (frame.rs varint length-prefix codec, status.rs, deadline.rs, streaming.rs) | hyper H2 | RFC 9113 + gRPC HTTP/2 spec |

### Listener `protocol = …` values handled in `crates/lb/src/main.rs::build_listener_mode`

| Config value | Mode | Notes |
|---|---|---|
| `"tls"` | TLS-terminate, then **plain-tcp passthrough** (no ALPN-dispatch app layer) | `main.rs:716-737` |
| `"h1"` | Plain TCP listener serving HTTP/1.1 | `main.rs:738-769` |
| `"h1s"` | TLS-terminate, ALPN={h2, http/1.1}, dispatch h2→H2Proxy / else→H1Proxy | `main.rs:770-836` |
| anything else | falls through to `ListenerMode::PlainTcp` (L4 TCP forwarder) | `main.rs:837` — **silent default**; flagged for findings |
| h3 / quic | Wired separately via `lb-quic::QuicListener::spawn` (not through `build_listener_mode`) | `listener.rs` |

---

## 2. Bridging matrix (in-tree `Bridge` impls and L7 proxy paths)

`lb-l7::create_bridge` (`lb-l7/src/lib.rs:175`) covers all 9 in-tree
combinations. The L7 proxies (`h1_proxy.rs`, `h2_proxy.rs`,
`h3_bridge.rs`) each support a subset of upstream protocols at runtime.

Legend: ✓ wired in the L7 proxy hot path. ⚙ trait-level only (in-tree
`Bridge` impl but not exercised by the proxy data path). ✗ not wired.

| Source → Dest | In-tree Bridge impl | Listener wiring | Notes |
|---|---|---|---|
| H1 → H1 | `H1ToH1Bridge` (h1_to_h1.rs:39) ⚙ | ✓ `H1Proxy::proxy_h1_to_h1` (h1_proxy.rs) | hyper H1 client+server, hop-by-hop strip |
| H1 → H2 | `H1ToH2Bridge` (h1_to_h2.rs) ⚙ | ✓ `H1Proxy::proxy_h1_to_h2` (h1_proxy.rs ~line 470) via `Http2Pool` | adds `:method`/`:scheme`/`:path`/`:authority` |
| H1 → H3 | `H1ToH3Bridge` (h1_to_h3.rs) ⚙ | ✓ `collect_h1_request_to_h3_fieldlist` (h1_proxy.rs:937) + `QuicUpstreamPool` | uses SNI from backend cfg |
| H2 → H1 | `H2ToH1Bridge` (h2_to_h1.rs) ⚙ | ✓ `H2Proxy::proxy_request` (h2_proxy.rs:347-355) | empty :authority → MissingPseudoHeader |
| H2 → H2 | `H2ToH2Bridge` (h2_to_h2.rs) ⚙ | ✓ `H2Proxy::proxy_h2_to_h2` (h2_proxy.rs ~line 360) | **bridge does NOT strip hop-by-hop** — see §3 finding-stub |
| H2 → H3 | `H2ToH3Bridge` (h2_to_h3.rs) ⚙ | ✓ `H2Proxy::proxy_h2_to_h3` (h2_proxy.rs:557+) | |
| H3 → H1 | `H3ToH1Bridge` (h3_to_h1.rs) ⚙ | ✓ via `h3_bridge.rs:140-160` (manual `:authority` → `Host:` synth + H1 client) | |
| H3 → H2 | `H3ToH2Bridge` (h3_to_h2.rs) ⚙ | ✓ via `h3_bridge.rs` + `Http2Pool` | |
| H3 → H3 | `H3ToH3Bridge` (h3_to_h3.rs) ⚙ | ✓ via `h3_bridge.rs` + `QuicUpstreamPool` | **bridge does NOT strip hop-by-hop** |
| WS  → WS (H1 upgrade) | n/a — direct tunnel | ✓ `ws_proxy.rs:218` (`proxy_socket`) | |
| WS  → WS (H2 ext-connect) | n/a | ✓ `h2_proxy.rs:374` (`handle_ws_extended_connect`) — **only H1 backend** | RFC 8441 listener ↔ RFC 6455 backend |
| gRPC → gRPC | n/a | ✓ `grpc_proxy.rs::handle` (h2_proxy.rs:288+) — **H1/H2 backends; rejects H3** | |

**Independent observation (deferred to findings round):**

* `H2ToH2Bridge`, `H3ToH3Bridge`, `H3ToH2Bridge` (the "same-pseudo-header" pass-throughs in `lb-l7/src/h*_to_h*.rs`) do **not** filter out `connection`/`keep-alive`/`transfer-encoding`/`proxy-*` — RFC 9113 §8.2.2 forbids them entirely on H2/H3, and a proxy that forwards them lets a malicious origin smuggle directives into a downstream H2 stream. The runtime data path goes through `strip_hop_by_hop` in `h2_proxy.rs:332` for the H2 listener, so the live blast radius is "the in-tree Bridge trait is unsafe to call directly". Round-2 candidate: PROTO-2-?? (LOW/MED depending on whether external code consumes the Bridge trait).
* `HOP_BY_HOP` in `h1_proxy.rs:54-63` lists `trailers` — that's not a real header name (RFC 9110 §6.6 uses `Trailer`). Removing a header literally named `Trailers` is a no-op so it's cosmetic, but the `Trailer` (singular) announce header is **never stripped** — if a backend sets a malformed `Trailer:` and the client speaks H1, the gateway forwards it as-is. Probably fine; flagging for §8.

---

## 3. Header normalization

### Hop-by-hop stripping

| Location | Direction | Mechanism |
|---|---|---|
| `lb-l7::h1_proxy::strip_hop_by_hop` (h1_proxy.rs:725-746) | both | Static `HOP_BY_HOP` list (8 names) + dynamic parse of `Connection:` token list, both lowercased. Invoked at h1_proxy.rs:385, 621; h2_proxy.rs:332, 512, 902 (yes — H2 proxy calls the H1-proxy helper). |
| `lb-l7::h1_to_h1::HOP_BY_HOP_HEADERS` (h1_to_h1.rs:7-16) | both | Trait-level Bridge impl, **including TE-trailers preservation** (h1_to_h1.rs:53-58). |
| `lb-l7::h1_to_h2::HOP_BY_HOP_HEADERS` (h1_to_h2.rs:9-18) | request only (response has its own filter) | Adds `host` removal (replaced by `:authority`). |
| `lb-l7::h2_to_h1::RESPONSE_HOP_BY_HOP` (h2_to_h1.rs:10-19) | response only | Drops pseudo-headers + 8 hop-by-hop names. |
| `lb-l7::h3_to_h1::RESPONSE_HOP_BY_HOP` (h3_to_h1.rs:10-19) | response only | Same shape as h2_to_h1. |
| `lb-l7::h2_to_h2::H2ToH2Bridge` | none | **No hop-by-hop strip.** |
| `lb-l7::h3_to_h3::H3ToH3Bridge` | none | **No hop-by-hop strip.** |

### HTTP/2 downcasing

* Bridge impls explicitly lowercase header names (`h1_to_h2.rs:65`,
  `h2_to_h1.rs:44`, etc.). hyper's H2 server already enforces lowercase
  on the wire, so the in-tree downcasing is belt-and-braces.
* `lb-h2::hpack` (the in-tree HPACK codec) does **not** assert that
  decoded names are lowercase — names are stored as raw bytes
  (`hpack.rs`). The codec is currently consumed by `tests/conformance_h2.rs`
  and fuzz targets, not the live server (hyper owns the server-side
  HPACK). Worth verifying it rejects uppercase per RFC 9113 §8.2.1.

### Forbidden-header validation

* `lb-l7::create_bridge` enforces `MAX_HEADERS = 256` (`lib.rs:48`,
  `check_header_count`).
* Empty `:authority` rejection (`h2_to_h1.rs:52-54`, `h3_to_h1.rs:52-54`).
* `lb-security::SmuggleDetector` (`smuggle.rs`) provides `check_cl_te`,
  `check_te_cl`, `check_h2_downgrade`. **None of these are called from
  the L7 proxy.** `grep -rn 'SmuggleDetector\|check_cl_te\|check_te_cl\|check_h2_downgrade' crates/lb-l7 crates/lb` returns zero hits. The
  smuggling tests under `tests/security_smuggling_*.rs` are pure unit
  tests against the detector — **the gateway data path does not run the
  check**. PROTO-2-?? candidate (HIGH if smuggling is otherwise
  unmitigated; needs cross-check with `sec`'s findings about hyper's
  builtin rejection of CL+TE, which is partial).

---

## 4. Routing (Host vs `:authority` vs SNI vs ALPN)

### ALPN

* TLS listener `h1s` advertises `[b"h2", b"http/1.1"]` in
  `build_h1s_tls_stack` (`main.rs:621`). Dispatch on negotiated value:
  `b"h2"` → H2Proxy, else → H1Proxy (`main.rs:1161-1174`). **No reject
  on no-ALPN** — falls through to H1, which is fine for legacy clients
  but means a client that negotiated something exotic just gets H1.
* TLS listener `tls` (passthrough) uses `build_tls_stack` which calls
  `build_server_config(rotator, certs, key, &[])` (empty ALPN slice —
  `main.rs:214+`). No ALPN advertised. Acceptable for the L4 forward
  case.
* QUIC listener advertises `LB_QUIC_ALPN = b"lb-quic"` (`lib.rs:115`)
  for the in-process transport test rig. **The production H3
  application-protocol token is `b"h3"` (RFC 9114 §3.1).** I did not
  find a path that sets `b"h3"` for the real H3 listener; this is
  either a TODO comment that bit reality, or the listener is currently
  only test-grade. Cross-check with `code`. PROTO-2-?? candidate
  (HIGH if real H3 traffic depends on this).

### SNI vs Host vs :authority

* **Inbound TLS:** SNI is not checked against Host/:authority anywhere
  I can see. The TLS layer terminates with a single cert (single-cert
  rustls `ServerConfig`), so SNI mismatch produces handshake failure
  through rustls' default `ResolvesServerCertUsingSni`-ish path. There
  is **no policy enforcement that the Host header / `:authority`
  matches the negotiated SNI** — a client can present
  `Host: attacker.example` while the SNI was `victim.example`, and the
  gateway will forward. RFC 9113 §8.3.1 and RFC 8470 don't strictly
  require this, but consensus practice (Envoy `strict_sni_hostname`)
  is to reject silently-disagreeing requests when a virtual-host map
  is in play. We don't have a vhost map today, so the disagreement is
  benign **for the single-cert deployment** but lurks as soon as SNI
  multiplexing is added. Flag for §8 / cross-talk.
* **Outbound TLS (H3 upstream):** SNI is taken from
  `Backend::sni` (`upstream.rs:62`) and passed through to quiche
  client config — there's a fallback to Host header at
  `h2_proxy.rs:758` and `h1_proxy.rs:950` for synthesising
  `:authority` when the inbound H1 client didn't supply Host. This is
  a "trust the gateway operator's config" model — fine.
* **H1 → H2/H3 :authority synthesis:** `h1_proxy.rs:984-986` inserts
  `:authority` only if absent. **No check that the Host header agrees
  with the :authority being synthesised** — but since :authority is
  *derived from* Host, that's tautological. OK.
* **H2 listener uses URI.authority() with fallback to Host header**
  (`h2_proxy.rs:320-330`). RFC 9113 §8.3.1 says if both are present
  and disagree the request **MUST** be rejected. Current code picks
  URI.authority() and silently ignores Host. PROTO-2-?? candidate
  (MED): silent precedence vs strict reject.

### Connection coalescing

* No HTTP/2 `ORIGIN` frame emission anywhere in the tree
  (`grep -rn 'ORIGIN' crates/lb-h2 crates/lb-l7` returns zero).
* No certificate-SAN cross-check on coalesced requests. The current
  H2 listener trusts a single cert per listener so there's nothing to
  coalesce against today — but as soon as a multi-SAN cert is loaded,
  this becomes RFC 7838 / 9114 §3.3 territory. Cross-check with
  `code` and `sec`.

---

## 5. TLS

### Cert + key loading

* TLS-over-TCP: `lb-security::build_server_config` (`ticket.rs:319-338`)
  takes `CertificateDer` + `PrivateKeyDer` already parsed; PEM parsing
  is in `crates/lb/src/main.rs::build_tls_stack` / `build_h1s_tls_stack`
  via `rustls-pemfile`. Single cert chain + single key (no SNI-based
  cert resolver). Provider explicitly pinned to `ring`
  (`ticket.rs:325`) — independent of the process default.
* QUIC: `lb-quic::lib.rs:392+` and `listener.rs` load PEM via quiche's
  `BoringSSL` context. SNI override planned (TODO at
  `lib.rs:124-126`) but for v1 the SAN must contain
  `expressgateway.test` (`lib.rs:117-126`).

### ALPN advertisement — see §4.

### Downgrade resistance

* `with_safe_default_protocol_versions()` enables TLS 1.2 + 1.3 (rustls
  feature flags `tls12` and `tls13`). **Pure TLS-1.3-only listeners
  are not configurable** from the current `tls_cfg`. Cross-check with
  `sec` — they may want a hard 1.3-only mode for the gateway-edge case.
* No cipher-suite pinning; relies on rustls/ring safe defaults. Fine.
* No `min_protocol_version` knob exposed via config. PROTO-2-?? minor.
* SSLKEYLOGFILE / debug exporters not enabled — confirmed by grep on
  `keylog`.

### Session-ticket rotation

* `lb-security::ticket::TicketRotator` (`ticket.rs:1-260`) implements
  RFC 5077 / RFC 8446 §4.6.1 ticket rotation with current+previous
  keys for overlap-window decryption, AES-128-GCM via ring AEAD.
* Rustls hookup via `RotatingTicketer: ProducesTickets` (`ticket.rs:271-293`)
  — `encrypt` always uses current; `decrypt` falls back to previous
  during overlap.
* Tickered through the binary via `spawn_rotator_ticker`
  (`main.rs:726, 780`). Rotation cadence comes from config; default not
  yet read.
* Tests: `tls_listener.rs` exercises a real TLS 1.3 handshake +
  resumption; `ticket.rs::tests` cover rotation/decrypt-after-rotate.
* **0-RTT replay defense:** `lb-security::zero_rtt::ZeroRttReplayGuard`
  (`zero_rtt.rs`) is a per-token sliding bloom-ish cache; called by
  `lb-quic` router (`router.rs`). Unit test in
  `tests/security_zero_rtt_replay.rs`. **Not** wired for TLS-over-TCP
  resumption; appears 0-RTT is QUIC-only today. For TLS 1.3 over TCP
  rustls does not enable 0-RTT by default in the ServerConfig builder
  used here, so this is fine.

---

## 6. Trailers / 1xx / Expect / Upgrade (WS, CONNECT)

### Trailers

* `lb-h1::ChunkedDecoder::trailers()` (`chunked.rs:67`) parses H1
  chunked trailers — exercised by `tests/conformance_h1.rs`.
* `lb-grpc::frame.rs` understands the gRPC trailer block; the gRPC
  proxy (`grpc_proxy.rs:404-420`) synthesises trailers
  (`grpc-status`, `grpc-message`) on backend-status non-200.
* H2→H1 / H1→H2 bridges currently **drop** trailers (no Frame::trailers
  forwarding in the non-gRPC path — hyper handles them transparently
  inside H2, but the bridge's `BridgeRequest`/`BridgeResponse` shape
  does not carry trailers at all). Tested by `tests/grpc_*`. For the
  general (non-gRPC) case, trailer pass-through across protocols is
  untested. PROTO-2-?? candidate (MED).

### 1xx informational responses

* No explicit handling for `100 Continue`, `103 Early Hints` (RFC 8297)
  anywhere — `grep -n '100[- ]?continue\|EarlyHints\|103' crates` returns zero.
* hyper 1.x server passes 100-Continue through automatically for H1
  when `Expect: 100-continue` is on the request; the L7 proxy does
  **not** explicitly forward the upstream's 100 to the downstream
  client. For H2/H3 the situation is similar.
* **Test coverage: 0.** PROTO-2-?? candidate (MED).

### Expect: 100-continue

* No `expect` header inspection in `lb-l7`. The hyper default likely
  works for in-path bodies but the **proxy-side `Expect: 100-continue`
  semantics** (forwarding it to upstream, holding the request body
  until 100 arrives) are not exercised by any in-tree test. Same
  finding as above.

### Upgrade — WebSocket (RFC 6455 + RFC 8441)

* H1 path: `ws_proxy.rs::detect_h1_ws_handshake` checks `Upgrade:
  websocket`, `Connection: Upgrade`, `Sec-WebSocket-Version: 13`,
  non-empty `Sec-WebSocket-Key` (`ws_proxy.rs:141-167`). H1Proxy
  dispatches via `hyper::upgrade::on`.
* H2 path: `ws_proxy.rs::is_h2_extended_connect` checks `:method ==
  CONNECT && :protocol == websocket` via hyper's extensions API
  (`ws_proxy.rs:170-184`). H2Proxy gates on
  `SETTINGS_ENABLE_CONNECT_PROTOCOL` (comment at
  `h2_proxy.rs:243`) — verify the SETTING is actually being sent on
  the H2 server's settings frame; hyper's `http2_enable_connect_protocol`
  builder method may need to be wired explicitly. PROTO-2-?? candidate.
* Tests: `tests/ws_proxy_e2e.rs` (19 kB) covers e2e. Autobahn
  fuzzingclient harness in `tests/ws_autobahn.rs` is a **skip-when-not-
  installed stub** (confirmed: when `wstest` is on PATH it just prints
  a TODO and exits without running the actual cases — see `ws_autobahn.rs:24-34`).

### CONNECT (HTTP/1 tunneling, RFC 9110 §9.3.6)

* No CONNECT handler in `h1_proxy.rs`. The H1 proxy accepts only
  request methods that hyper deserialises into a `Request`. A plain
  CONNECT will land in `proxy_request` and be forwarded as a regular
  H1 method to the upstream — meaning the gateway acts as a "weird
  reverse-proxy that forwards CONNECT" rather than a tunneling
  forward-proxy. Probably correct (LB is a reverse proxy) but worth
  asserting explicitly: a deployed CONNECT request from the client to
  the LB **must not** tunnel to an arbitrary host. Cross-check with
  `sec`.

### Upgrade (other) — h2c, TLS upgrade

* Neither `Upgrade: h2c` (h2c bootstrap via RFC 9113 §3.1 — deprecated
  in 9113 but legacy clients still send it) nor `Upgrade: TLS/1.2`
  (RFC 2817) is intercepted. h2c is also not supported as a listener
  mode (§1), so the practical answer is "h2c is silently downgraded
  to h1". Fine if documented; flag for §8.

---

## 7. Existing conformance test surface

All test files inspected. "Active" = runs assertions on the gateway's
behaviour. "Stub" = guarded by `which $tool` and skips without
exercising the binary. "Unit" = exercises in-tree crates only, no
runtime.

| File | Claim | Reality |
|---|---|---|
| `tests/conformance_h1.rs` | "HTTP/1.1 conformance" | **Unit** test of `lb-h1::parse_*` only — does **not** spin up a server. ~5 cases. |
| `tests/conformance_h2.rs` | "HTTP/2 conformance" | **Unit** test of `lb-h2::frame` codec round-trip only. ~6 frame types. |
| `tests/conformance_h3.rs` | "HTTP/3 conformance" | **Unit** test of `lb-h3::frame` codec round-trip. |
| `tests/h2spec.rs` | h2spec generic | **Active when h2spec is on PATH; stub otherwise.** Spawns an H2s listener on an ephemeral port, runs `h2spec -t -k -p <port>`, asserts exit 0. Not installed in this sandbox; skipped silently. |
| `tests/h3_upstream_verify.rs` | H3 upstream `verify_peer` + CA | **Active** — config-validator-level test; verifies `BackendConfig` rejects missing CA when `verify_peer=true`. |
| `tests/quic_listener_e2e.rs` | UDP bind + handshake | **Active** — real quiche client handshake against the listener. |
| `tests/quic_native.rs` | (presumed quic native client) | Active. |
| `tests/tls_listener.rs` | TLS-over-TCP + ticket resumption | **Active** TLS 1.3 handshake + session-ticket round-trip with rustls client. |
| `tests/bridging_h{1,2,3}_h{1,2,3}.rs` (9 files) | per-pair `Bridge` trait | **Unit** — exercises the in-tree Bridge impls only. Does NOT exercise hyper / quiche. |
| `tests/h1_proxy_e2e.rs`, `h2_proxy_e2e.rs`, `proto_translation_e2e.rs`, `ws_proxy_e2e.rs`, `grpc_proxy_e2e.rs` | live end-to-end | **Active.** These are the meaningful real-traffic tests. |
| `tests/h2_security_live.rs` (21 kB) | live H2 attack harness using raw `h2 = "0.4"` client | **Active** — exercises CONTINUATION flood, rapid-reset, etc. against the H2 server. |
| `tests/ws_autobahn.rs` | Autobahn fuzzingclient | **Stub.** Even if `wstest` were on PATH, the test only `--help`-probes the binary and `eprintln!`s a TODO (`ws_autobahn.rs:24-34`). Round-2 follow-up: implement the actual fuzzing-client harness. |
| `tests/security_smuggling_cl_te.rs`, `security_smuggling_te_cl.rs`, `security_smuggling_h2_downgrade.rs` | smuggling defenses | **Unit** tests of `lb_security::SmuggleDetector` — but the detector is **never wired into the L7 proxy** (see §3). The tests pass; the gateway is still vulnerable. PROTO-2-?? + cross-talk with `sec`. |
| `tests/security_continuation_flood.rs`, `security_hpack_bomb.rs`, `security_qpack_bomb.rs`, `security_rapid_reset.rs`, `security_zero_rtt_replay.rs` | detector unit tests | Unit. Detectors are wired into the live H2 path via `lb-l7::h2_security` (see `h2_security.rs`) — confirmed for rapid-reset / continuation flood. HPACK-bomb / QPACK-bomb / 0-RTT replay wiring not yet verified end-to-end; defer to round-2 sweep. |
| `tests/security_slow_post.rs`, `security_slowloris.rs` | DoS detectors | Unit. Wiring into H1 listener: not yet verified. |
| `tests/h1_rejects_grpc.rs` | gRPC content-type on H1 should be rejected | Active — confirms gRPC is forced to H2. |
| `tests/binary_proto_routing.rs` | binary protocol routing | Active. |
| `tests/h3spec.rs` | **does not exist** | h3spec is a real tool (https://github.com/kazuho/h3spec, or quic-tracker) but no integration test invokes it. PROTO-2-?? — h3spec gap. |

**Summary**: the names `conformance_h1/h2/h3.rs` are misleading — they
are codec round-trip unit tests, not protocol-conformance tests against
a running server. The real conformance test is `h2spec.rs`, which is
skipped in any environment where `h2spec` isn't installed. There is no
`h3spec` harness. CI must install h2spec to get any signal here.

---

## 8. Open questions / cross-team handoffs

For `sec` (SEC-1-* inventory):
1. **Smuggling-detector wiring gap.** `SmuggleDetector::check_cl_te` /
   `check_te_cl` / `check_h2_downgrade` are unit-tested but never
   called from `lb-l7::h1_proxy` or `lb-l7::h2_proxy`. Is hyper's
   builtin CL+TE rejection (it returns 400 on conflicting headers in
   1.x) considered sufficient, or does the gateway need to invoke
   the in-tree detector explicitly? My read: hyper covers CL+TE on
   the request line, but does NOT catch the TE-final-token case nor
   the H2 downgrade-with-Connection case. PROTO + SEC joint
   finding likely.
2. **No SNI ↔ Host/:authority cross-check.** Cosmetic today
   (single-cert deployments) but blocks any future SNI-multiplex
   vhost feature.
3. **TLS 1.2 still enabled.** No config knob to disable it. Sec's
   call whether a hard 1.3-only mode is in v1 scope.
4. **0-RTT replay window** — what's the configured cache size /
   sliding window for `ZeroRttReplayGuard` in production? Tests use
   `new(1000)`.

For `code` (CODE-1-* inventory):
1. **`LB_QUIC_ALPN = b"lb-quic"`** — is the real H3 listener
   advertising `b"h3"`, or is this test-rig value bleeding into
   prod? Need to trace the H3 listener startup path in
   `crates/lb/src/main.rs` (I see no H3 listener spawn there; it
   may be wired elsewhere).
2. **`ListenerMode` silent fallthrough** to PlainTcp at `main.rs:837`
   for any unrecognised `protocol = …` value. Should be a hard error.
3. **`HOP_BY_HOP` list contains the spurious name `trailers`** —
   noise, not a bug. Note for cleanup.
4. **H2ToH2 / H3ToH3 bridges don't strip hop-by-hop** at the trait
   level. Currently the runtime path goes through `strip_hop_by_hop`
   in `h2_proxy.rs` before the bridge runs, so blast radius is
   "Bridge trait is unsafe for external consumers". Decide whether
   to fix the bridges or document the precondition.

For `ebpf` (EBPF-1-* inventory):
1. **L4 XDP passthrough vs L7 listener mode** — when the user
   configures `protocol = "tls"` for L4-only TLS, the XDP fast path
   must not also try to dispatch on this socket. The current
   `ListenerMode::Tls` does its own TCP accept; confirm there's no
   double-bind.
2. **QUIC UDP socket sharing with XDP** — does the QUIC listener's
   UDP socket compete with any XDP redirect rule?

For `rel` (REL-1-* inventory):
1. **h2spec is not installed in CI by default** — confirmed by
   `tests/h2spec.rs:39-42` (`which h2spec` probe). Reliability
   should pin the install in the CI image.
2. **`wstest` (Autobahn) not installed and the test harness only
   probes `--help`** — same story. Plus the in-tree test wouldn't
   actually run the fuzzing cases even if installed
   (`ws_autobahn.rs:24-34`).
3. **No h3spec harness exists.** Add one once a maintained h3spec
   replacement is chosen (quic-tracker / h3i).
4. **`tests/conformance_h1.rs / conformance_h2.rs / conformance_h3.rs`
   are codec unit tests, not conformance tests** — the names mislead.
   Either rename or add real conformance harnesses.

---

## 9. Findings stubs to expand in Round 2

(Numbering provisional; will be assigned firm `PROTO-2-NN` IDs in
round-2-findings.md.)

* **PROTO-2-A**: smuggling-detector unwired (HIGH). §3 / §8.
* **PROTO-2-B**: H2 listener `URI.authority()` vs `Host` disagreement
  silently picks `URI.authority()` — RFC 9113 §8.3.1 says reject (MED).
* **PROTO-2-C**: H3 listener ALPN appears to advertise `b"lb-quic"` not
  `b"h3"` (HIGH if reaches prod). §4.
* **PROTO-2-D**: `H2ToH2Bridge` / `H3ToH3Bridge` trait impls don't
  strip hop-by-hop (LOW — runtime path covers it; LOW only because
  bridges are not currently re-exported for external use).
* **PROTO-2-E**: No 1xx / 100-Continue forwarding test or explicit
  policy (MED).
* **PROTO-2-F**: `tests/ws_autobahn.rs` is a stub even when `wstest`
  is installed; no Autobahn report parsing (MED).
* **PROTO-2-G**: No `h3spec` integration test (MED — h2spec parity).
* **PROTO-2-H**: `conformance_h{1,2,3}.rs` names imply protocol
  conformance but they're codec round-trip tests (LOW — naming).
* **PROTO-2-I**: TLS 1.2 still enabled with no config knob to
  disable; ALPN no-negotiate silently falls to H1 (LOW/MED — depends
  on sec's deployment-mode call).
* **PROTO-2-J**: `ListenerMode` silent fallthrough to PlainTcp for
  unknown `protocol = …` values (LOW — config-validation gap, but a
  typo'd listener still binds + forwards bytes silently).
* **PROTO-2-K**: No `Trailer` (singular) announce-header forwarding
  contract documented; trailer pass-through across H1↔H2/H3 bridges
  is untested for the non-gRPC case (MED).
* **PROTO-2-L**: H2 ext-CONNECT requires `SETTINGS_ENABLE_CONNECT_PROTOCOL=1`
  on the server's initial SETTINGS frame; verify hyper actually sends
  it (hyper 1.x exposes
  `http2().enable_connect_protocol()` builder — need to confirm it's
  called) (MED).

---

## 10. Files written this round

* `audit/protocol/round-1-inventory.md` (this file)
* `audit/protocol/round-1-cross-review.md` (cross-talk log; created
  during the 30-minute cross-review phase)

No source files modified.
