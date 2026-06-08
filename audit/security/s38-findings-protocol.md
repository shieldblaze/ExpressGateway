# S38 Findings — protocol-auditor (smuggling / desync / response-splitting / cross-stream)

**Auditor:** protocol-auditor · **Date:** 2026-06-08 · **Base:** main @ b8a99078
**Surface:** request smuggling / desync / response splitting / cross-stream bleed across the 9-cell
front×back matrix (H1/H2/H3) + WS/gRPC upgrade paths. Leads L-PROTO-1..6.
**Method:** static read of the OUR-code translation/relay layer on top of hyper-1.10.1 / quiche-0.29.1;
adversarial PoC authoring. No `cargo build/test/run` (lead executes all PoCs).

---

## TOP — CRITICAL / HIGH (read immediately)

**NONE.** No request-smuggling, response-splitting, desync, host-confusion-bypass, upgrade-ordering, or
cross-stream-bleed finding rises to HIGH/CRITICAL in this pass.

The reason is structural and worth stating up front so the lead can spot-check the invariant rather than
re-derive it: **every code path where attacker- or backend-controlled header/trailer bytes reach a
CRLF-vulnerable (HTTP/1.1) wire is funnelled through hyper's typed `HeaderName` / `HeaderValue` /
`response::Builder`**, all of which reject CR / LF / NUL / non-token names and **fail closed** (skip the
pair, or store the error and surface a 500 at `body()`), never emit the raw bytes. **Every path that
reaches an HTTP/3 wire is QPACK-encoded** (binary, length-prefixed) — CRLF/NUL in a value cannot split a
field. The only hand-rolled response-head/trailer **parser** that consumes raw backend bytes
(`h3_bridge::stream_h1_response` / `stream_h2_response`, the H1/H2-backend → **H3**-front legs) emits
exclusively onto the QPACK wire. That is the load-bearing design fact; the LOW items below are the seams
where it is *almost* — but not quite — violated, plus conformance nits.

---

## Findings table

| ID | Sev | Cell(s) / path | One-line | Disposition |
|---|---|---|---|---|
| F-PROTO-01 | LOW | H1→*, H2→* (smuggle detector input) | CL/TE smuggle detector skips header pairs whose value fails `to_str()` (opaque 0x80–0xFF bytes) | Not exploitable — hyper independently validates CL/TE framing + gateway strips CL/TE on egress; harden by inspecting raw bytes |
| F-PROTO-02 | INFO | gRPC 200 pass-through | `finalize_upstream` forwards upstream response headers verbatim with no hop-by-hop strip | H2-only (binary) → no split; conformance nit |
| F-PROTO-03 | INFO | WS H1 101 | Server echoes client `Sec-WebSocket-Protocol` verbatim without confirming the backend selected it | RFC 6455 §4.1 conformance; not security |
| F-PROTO-04 | INFO | H1/H2-backend → H3-front | Hand-rolled backend response-head parser does not token-validate field names before QPACK encode | QPACK is binary → no split; malformed name → quiche encodes opaquely or stream-resets (avail edge) |

No finding requires a code change for security. F-PROTO-01 is a defense-in-depth hardening suggestion.

---

## Per-finding detail

### F-PROTO-01 · LOW · CL/TE smuggle detector skips non-`to_str()` header values
**Cells:** H1→H1, H1→H2, H1→H3, H2→H1, H2→H2, H2→H3 (the detector call sites).
**Mechanism:** `crates/lb-l7/src/h1_proxy.rs:1061-1069` and `crates/lb-l7/src/h2_proxy.rs:1162-1170` build
the `Vec<(String,String)>` fed to `SmuggleDetector::check_all_mode` by **filtering out** any header whose
value fails `HeaderValue::to_str()`:

```rust
let header_pairs: Vec<(String, String)> = parts.headers.iter()
    .filter_map(|(n, v)| v.to_str().ok().map(|s| (n.as_str().to_owned(), s.to_owned())))
    .collect();
```

`HeaderValue` permits opaque bytes 0x80–0xFF (RFC 9110 obs-text); `to_str()` rejects them. So a header
whose *value* contains such a byte is invisible to `check_duplicate_cl` / `check_cl_te` / `check_te_cl` /
`check_h2_downgrade` (`crates/lb-security/src/smuggle.rs:52-257`), yet the real `parts.headers` (which the
H1 request to the backend is materialised from) still carries it.

**Why it is NOT a smuggle (prove-it):** the only headers the detector cares about are `Content-Length`,
`Transfer-Encoding`, `Connection`, `Upgrade`, `TE`, and `:`-pseudo. All of these are independently
constrained by hyper's *production* H1/H2 parser **before** our detector ever runs:
- `Content-Length` must be ASCII digits — an opaque-byte value is rejected by hyper's framing layer.
- `Transfer-Encoding` is parsed as a token list by hyper; opaque bytes are not a valid TE codec.
- On the H1→* and H2→H1 egress, the gateway **removes** `content-length` + `transfer-encoding` and lets
  hyper re-frame (`h1_proxy.rs:1219-1220`, `h2_proxy.rs:1571-1572`), so even a detector-bypassing framing
  header never reaches the backend wire.
- hyper-1.10.1's H1 server is built with strict defaults (`h1_proxy.rs:684-685`, only `.keep_alive(true)`
  — no `preserve_header_case`, no `allow_obsolete_multiline_headers`, no `ignore_invalid_headers`), so it
  rejects CL+TE conflict, differing-duplicate-CL, and obs-fold itself.

**PoC (lead — expected to be a CLEAN no-op, proves the defense):** H1→H1 cell. Send via raw socket to the
H1 listener:
```
POST / HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunk\x80ed\r\nContent-Length: 5\r\n\r\n0\r\n\r\n
```
Expected backend view: NO request reaches the backend with both CL and TE; hyper rejects the message at
parse (invalid TE token / CL+TE) → 400 from the listener, never a desynced backend. (The detector-bypass
is moot because hyper gates first.) Harness: extend `tests/security_smuggling_cl_te.rs` /
`crates/lb-l7/tests/smuggle_wired.rs` with an opaque-byte TE value asserting a 400 and a single clean
backend request.

**Single-source siblings (R12):** both detector call sites (`h1_proxy.rs:1061`, `h2_proxy.rs:1162`) share
the identical `to_str().ok()` filter; the `H2ToH1Bridge` buffering path (`h2_to_h1.rs:53-66`) builds its
pair list with `v.clone()` from already-`String` values so it is not affected.

**Suggested hardening (optional, LOW):** make the detector input lossless — feed
`String::from_utf8_lossy(v.as_bytes())` instead of dropping the pair, so a CL/TE/Connection name with an
opaque value is still seen by name. One-line change at each call site; no behaviour change for valid input.

**Disposition:** LOW hardening. Not a security finding (hyper + egress-strip are the real gates, both
proven above).

---

### F-PROTO-02 · INFO · gRPC 200 pass-through forwards upstream headers without hop-by-hop strip
**Cell:** gRPC (H2 front, H2 backend), 200-status path.
**Mechanism:** `crates/lb-l7/src/grpc_proxy.rs:586-596` (`finalize_upstream`) copies every upstream
response header (`hdrs.insert(k, v.clone())`) onto the downstream 200 with no `strip_hop_by_hop` call,
unlike the generic `H1Proxy::finalize_response` (`h1_proxy.rs:2529`) / `H2Proxy::finalize_response`
(`h2_proxy.rs:2144-2146`) which DO strip.
**Why INFO not HIGH:** the leg is H2→H2; headers are typed hyper `HeaderValue` clones (no string
round-trip, no CRLF risk), and H2 forbids connection-specific headers on the wire (hyper's h2 client never
delivers `Connection`; `transfer-encoding`/`upgrade`/`keep-alive` are invalid in H2). So nothing
dangerous can actually be present to forward. It is a conformance/cleanliness gap, not a smuggle.
**PoC:** malicious H2 gRPC backend replies 200 with `Connection: x` trailer-of-headers — hyper's h2 codec
rejects connection-specific headers at the backend before delivery, so the forward is a no-op. Clean.
**Disposition:** INFO. Optional: call `strip_hop_by_hop(&mut parts.headers)` in `finalize_upstream` for
parity with the other finalizers.

---

### F-PROTO-03 · INFO · WS H1 echoes client sub-protocol without backend confirmation
**Cell:** WS H1 upgrade.
**Mechanism:** `crates/lb-l7/src/h1_proxy.rs:2506-2519` (`handle_ws_upgrade`) echoes the client's first
`Sec-WebSocket-Protocol` token verbatim into the 101, and `h2_proxy.rs` mirrors it; the upstream
handshake response (`_resp`) is discarded so the backend's actually-selected sub-protocol is not
propagated. RFC 6455 §4.1 says the server MUST only confirm a sub-protocol it (the backend) selected.
**Why INFO:** the echo goes through `HeaderValue::from_str` (CRLF-safe). Worst case is a client believing
a sub-protocol was negotiated when the backend ignored it — an application-semantics mismatch, not a
gateway security boundary.
**Disposition:** INFO / conformance.

---

### F-PROTO-04 · INFO · H3-front backend-response head parser does not token-validate field names
**Cells:** H1→H3, H2→H3 (the `stream_h1_response` / `stream_h2_response` legs).
**Mechanism:** `crates/lb-quic/src/h3_bridge.rs:1011-1029` parses each backend H1 response header line with
`line.split_once(':')`, lowercases + trims the name, trims the value, and pushes to `fwd_headers` with NO
RFC 9110 §5.6.2 `tchar` validation of the name and no control-char check on the value. These flow to
`conn_actor.rs:845-855` → `quiche::h3::Header::new(n.as_bytes(), v.as_bytes())` →
`quiche::h3::send_response`. Confirmed: quiche-0.29.1's QPACK encoder
(`quiche-0.29.1/src/h3/qpack/encoder.rs:46-170`, `encode_str`) does NOT validate names/values — it
lowercases the name and opaquely Huffman/literal-encodes both, rejecting nothing.
**Why INFO not HIGH:** HTTP/3 is binary/length-prefixed. A bare LF, NUL, space, or control byte that
survived the `\r\n` line split (e.g. a backend `X-Foo: a\nSet-Cookie: b` where `\n` is a *bare* LF) is
encoded as a *single* QPACK field value — it **cannot split a header** on the H3 wire. The H3 client
receives one field `x-foo: "a\nset-cookie: b"`. **No response splitting.** A genuinely malformed *name*
(e.g. containing a space) is either accepted opaquely by the client or causes the client/quiche to
reset the stream — an availability edge a malicious backend already controls (it can reset its own
response), not a cross-tenant boundary.
**PoC (lead — expected clean):** H1-backend → H3-front cell. Backend replies:
`HTTP/1.1 200 OK\r\nX-Foo: a\nInjected: 1\r\n\r\nbody`. Expected H3 client view: ONE response header
`x-foo` with value `a\nInjected: 1` (bare LF retained as a value byte), NO separate `injected` header.
Harness: the 9-cell H3-front e2e (`crates/lb-quic/tests/*h3*` / the h3-front integration suite) with a
crafted raw-bytes H1 backend; assert the client decodes exactly one `x-foo`.
**Single-source siblings (R12):** `stream_h2_response` (h3_bridge.rs:~1214+) shares the parse-then-QPACK
shape from a hyper-typed H2 backend response (already CRLF-safe at the source); the chunked **trailer**
parser (`h3_bridge.rs:855-896`) similarly splits on `\r\n` and feeds QPACK — same binary immunity.
**Disposition:** INFO. Optional defense-in-depth: drop or reject a `fwd_headers`/trailer name that is not
all-`tchar` before queueing (cheap, makes the H3 leg byte-clean and future-proofs against a
non-quiche H3 client lib that mishandles a malformed field name).

---

## Proven-clean cells / paths (defense + test)

R4: each below is clean because a specific defense was *identified*, and the harness that pins it is named.

**1. H2→H1 downgrade smuggling (L-PROTO-1) — CLEAN.**
- *Hop-by-hop / pseudo-leak:* `SmuggleDetector::check_h2_downgrade` (`smuggle.rs:220-257`) rejects
  `connection`/`transfer-encoding`/`keep-alive`/`upgrade`/`proxy-connection`, non-`trailers` `TE`, and any
  `:`-pseudo, wired at the production hot path `h2_proxy.rs:1171` (`SmuggleMode::H2`) and in the buffering
  bridge `h2_to_h1.rs:83`.
- *Request-line / Host injection:* the H1 request to the backend is built by hyper's
  `client::conn::http1` from `parts.method` / `parts.uri` / a typed `HeaderMap`; a CRLF in `:path` or
  `:authority` is rejected by hyper's `Uri`/`Authority`/`HeaderValue` types — our code never string-builds
  a request line.
- *`:authority`↔Host confusion:* `check_authority_host_agreement` (`h2_proxy.rs:1194`) +
  `authority_host_components_agree` (`h2_to_h1.rs:175-188`) reject a `Host` that disagrees with
  `:authority` (RFC 9113 §8.3.1), IPv6-bracket-aware.
- *Egress framing:* `h2_proxy.rs:1571-1572` strips CL+TE; hyper re-frames.
- *Tests:* `tests/security_smuggling_h2_downgrade.rs`, `crates/lb-l7/tests/h2_to_h1_pseudo_strip.rs`,
  `crates/lb-l7/tests/smuggle_matrix.rs`, `h2_to_h1.rs` unit tests `bridge_rejects_authority_host_disagreement`.

**2. H1→H1 CL/TE (L-PROTO-2) — CLEAN.**
- hyper-1.10.1 strict-default H1 server (`h1_proxy.rs:684`) rejects CL+TE conflict, differing duplicate
  CL, obs-fold, non-token TE, CR/LF/NUL in values at parse. `SmuggleDetector::check_all_mode`
  (`h1_proxy.rs:1075`) is defense-in-depth (`check_cl_te`/`check_te_cl`/`check_duplicate_cl`, plus
  `check_te_strict` under `smuggle_strict`). Egress strips CL/TE and re-frames (`h1_proxy.rs:1219-1220`).
  H1 upstream connection is single-use (`take_stream`, `h1_proxy.rs:1242-1244` + the Pingora-class
  doc-warning) so an over/under-read backend cannot corrupt a pipelined next request.
- *Tests:* `tests/security_smuggling_te_cl.rs`, `tests/security_smuggling_cl_te.rs`,
  `crates/lb-security/tests/smuggle_strict_te.rs`, `crates/lb-l7/tests/smuggle_wired.rs`,
  `tests/h1h1_md_streaming_verify.rs` (F-MD-4 truncation→no smuggled-complete).

**3. Trailer / response splitting (L-PROTO-3) — CLEAN.**
- *H3-backend → H1-front trailers* go through `HeaderName::from_bytes` + `HeaderValue::from_str`
  (`h1_proxy.rs:2311-2316`, `:2970-2974`) — CR/LF/NUL rejected, pair skipped on error (fail-closed).
- *H3-decoded response head → H1 front* via `h3_decoded_resp_head_builder` (`h1_proxy.rs:2917-2939`) and
  `build_h1_response_with_trailers` (`:3090-3149`): `builder.header(name, value)` stores any
  invalid-name/value error in the `Builder`, surfaced as a 500 at `body()` — fail-closed, never injected.
  The `Trailer:` declaration is `HeaderValue::from_str`-guarded (`:3124`).
- *gRPC `grpc-status` trailers* ride hyper `HeaderMap` trailer frames on an H2 (binary) client
  (`grpc_proxy.rs:413-420`, `:560-569`, `:586-596`); `grpc-message` uses `HeaderValue::from_str` (`:565`).
- *XFF / Via / Alt-Svc / X-Forwarded-* injection:* `append_xff` / `append_via` / `set_xfp` / `set_xfh`
  (`h1_proxy.rs:2732-2790`) and Alt-Svc (`:2535`, `:2934`, `:3046`, `:3133`) all gate on
  `HeaderValue::from_str(..).ok()` — CRLF-safe, and they preserve multi-value list form (the Envoy
  GHSA-ghc4-35x6-crw5 comma-join bug is explicitly avoided).
- *H3-front backend-head trailers* (chunked decode, `h3_bridge.rs:855-896`): split on `\r\n` (no CRLF in a
  line), name lowercased, pseudo-headers rejected (`name.starts_with(':')`), then QPACK-encoded (binary).
- *Truncation guard:* an upstream Reset injects a body error (`H1PumpAbort`, `h1_proxy.rs:2321-2324`) so
  hyper aborts the chunked response WITHOUT a `0\r\n\r\n` terminator — a truncated body is never presented
  as complete (the response-splitting-via-truncation guard).
- *Tests:* `tests/grpc_proxy_e2e.rs`, `crates/lb-l7/tests/` trailer suites, the H3-front e2e.

**4. Upgrade ordering — F-S27-1 class (L-PROTO-4) — CLEAN.**
- *WS H1:* `handle_ws_upgrade` (`h1_proxy.rs:2409`) dials + completes the upstream RFC 6455 handshake
  INLINE before any 101; failure → 502/504 with **no 101 emitted** (`:2459-2491`); the splice task only
  splices an already-established `backend_ws` (`:2497-2501`).
- *WS H2:* `handle_ws_extended_connect` (`h2_proxy.rs:1363`, dial at `:1445-1505`) is the exact mirror —
  dial + handshake before the 200, 502/504 on failure with no 200 emitted; extended-CONNECT requires
  `:scheme` + `:path` (`:1399-1416`, RFC 8441 §4).
- *WS H3:* `validate_request_pseudo_headers` (`h3_bridge.rs:489-619`) enforces the RFC 8441/9220
  extended-CONNECT rule (`:method=CONNECT` + `:protocol` ⇒ `:scheme`+`:path`+`:authority` required;
  classic CONNECT omits `:scheme`/`:path`); the actor waits `WsUpstreamOutcome::Ready` before the 200
  (`conn_actor.rs:1997-2012`).
- *Tests:* `tests/ws_proxy_e2e.rs`, the WS H1/H2/H3 e2e + `ws_h2_conformance.rs`.

**5. SNI ↔ authority 421 (L-PROTO-5) — CLEAN.**
- `check_sni_authority` (`sni_authority.rs:95-113`): case-insensitive host compare, trailing-dot
  normalised, IPv6-bracket-aware port split, empty-authority deferred to the PROTO-2-01 gate. Wired at
  `h1_proxy.rs:1040` and `h2_proxy.rs:1227` AFTER the `:authority`↔Host agreement check, skipped only for
  loopback peers (documented sec-r5 caveat). No host-confusion bypass found (port is intentionally
  ignored; SNI carries no port).
- *Tests:* `sni_authority.rs` unit module (10 cases incl. IPv6 mismatch),
  `crates/lb-l7/tests/sni_authority_421.rs`, `crates/lb-l7/tests/sni_authority_mismatch.rs`.

**6. Cross-stream / cross-connection bleed (L-PROTO-6) — CLEAN.**
- All H3 per-stream state in the conn actor (`stream_response`, `body_tx_by_stream`, `body_seen`,
  `pending_trailers`, `resp_rx_by_stream`, `ws_tunnels`) is keyed by `sid` in per-connection `HashMap`s
  (`conn_actor.rs:312-344`), accessed via `get_mut`/`remove` — no shared buffer, no key reuse (QUIC stream
  ids are monotonic per connection). The S29 stale-receiver fix is preserved: `drain_resp_channels`
  (`:642-652`) uses `get_mut` (NOT `entry().or_insert_with`) so a stale receiver for a terminated stream is
  dropped, never replayed onto a fresh stream — the large-response trailer-drop / cross-stream-replay class
  cannot recur. H2/H3 multiplexing isolation is enforced by hyper/h2 and quiche respectively.
- *Note (deferred to infra-auditor L-INFRA-2):* the SIGHUP ArcSwap reload race (does a half-applied reload
  cross protocol config between in-flight connections) is owned by infra-auditor; from the protocol angle,
  each connection task `load_full()`s an immutable `Arc<Config>` snapshot at accept, so a mid-flight reload
  cannot mutate an established connection's routing/framing config. No cross-connection protocol-state bleed
  observed.

---

## Notes / scope honesty
- This pass is static + PoC-authoring only (no execution, per the rules). The "CLEAN" verdicts identify the
  defense + the existing harness; the lead should run the named PoCs to confirm the bare-LF-into-H3 (F-PROTO-04)
  and opaque-byte-TE (F-PROTO-01) cases behave as predicted.
- The structural invariant (H1 wire ⇒ hyper typed encode fail-closed; H3 wire ⇒ QPACK binary) is the single
  highest-leverage thing to re-verify if any future change introduces a *string-built* H1 response head or
  request line bypassing hyper's `Builder`/`HeaderValue` — that is exactly where this class would reappear.
