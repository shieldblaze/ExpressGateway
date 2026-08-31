# S47 — HTTP/3 + QPACK RFC conformance review (RFC 9114 / RFC 9204)

Scope: `crates/lb-quic/src/{h3_bridge.rs,h3_config.rs,ws_tunnel.rs}`,
`crates/lb-l7/src/{h3_to_h1,h3_to_h2,h3_to_h3,h1_to_h3,h2_to_h3}.rs`,
`crates/lb-h3-testcodec/`. Read-and-reason only (no builds). Branch
`review/s47-rfc-security` @ main `01915a77`.

---

## 0. The quiche-vs-us boundary (established by reading quiche 0.29.1 source)

Everything below depends on knowing exactly what the library validates. Measured,
not assumed, against
`~/.cargo/registry/src/index.crates.io-*/quiche-0.29.1`:

**quiche 0.29.1 DOES enforce (so a finding blaming our code here would be a false positive):**

| Rule | RFC | quiche site |
|---|---|---|
| Frame sequencing on request streams: DATA-before-HEADERS, DATA-after-trailers, HEADERS-after-trailers | 9114 §4.1 | `src/h3/stream.rs:360-412` `validate_request_frame_type` |
| >2 HEADERS frames (server) | 9114 §4.1 | `src/h3/mod.rs:3007-3020` → `FrameUnexpected` |
| SETTINGS must be first on control stream; no second SETTINGS; DATA/HEADERS/PUSH_PROMISE on control | 9114 §6.2.1, §7.2 | `src/h3/stream.rs:277-311` |
| CANCEL_PUSH/SETTINGS/GOAWAY/MAX_PUSH_ID/PRIORITY_UPDATE on a request stream | 9114 §7.2 | `src/h3/stream.rs:362-373` |
| Second control / QPACK-encoder / QPACK-decoder stream → `H3_STREAM_CREATION_ERROR` | 9114 §6.2.1 | `src/h3/mod.rs:2590-2660` |
| Critical stream closed → `H3_CLOSED_CRITICAL_STREAM` | 9114 §6.2.1 | `close_conn_if_critical_stream_finished` |
| HTTP/2-only SETTINGS ids `0x0/0x2/0x3/0x4/0x5` → `H3_SETTINGS_ERROR` | 9114 §7.2.4.1 | `src/h3/frame.rs:621-623` |
| Field-section size cap (`max_field_section_size`, we set 1 MiB) → `H3_EXCESSIVE_LOAD` | 9114 §4.2.2 | `src/h3/qpack/decoder.rs:117,153,181` + `mod.rs:3037-3046` |
| Invalid static-table index → `QPACK_DECOMPRESSION_FAILED` | 9204 §3.1 | `decoder.rs:203-208` |
| Outbound field-name lowercasing on QPACK egress | 9114 §4.2 | `src/h3/qpack/encoder.rs:77` `encode_str::<true>` |
| Received push stream (pushes never enabled) → connection error | 9114 §4.6 | `mod.rs:2620-2630` (uses `H3_STREAM_CREATION_ERROR`, not the §7.2.5-preferred `H3_ID_ERROR` — safe deviation) |

**quiche 0.29.1 does NOT enforce (so these are OURS to own):**

| Gap | RFC | Proof |
|---|---|---|
| **Any** validation of decoded field-name / field-value BYTES — uppercase names, CR/LF/NUL in values, empty names, connection-specific fields, `TE` != `trailers` | 9114 §4.1.2, §4.2 | `src/h3/qpack/decoder.rs:85-197` returns raw `Vec<u8>` name/value pairs; `src/h3/mod.rs:3007-3085` `process_frame` passes `headers` straight into `Event::Headers` with no inspection |
| `content-length` vs the sum of DATA payload lengths | 9114 §4.1.2 | no `content-length` string appears anywhere in `src/h3/` outside the static table |
| DATA-frame completeness at FIN (truncated DATA at stream end) | 9114 §7.1 | `mod.rs:2871-2890` `process_finished_stream` pushes `Event::Finished` on QUIC FIN regardless of the outstanding `frame_payload_len` — **re-confirmed for 0.29.1** |
| QPACK encoder/decoder uni-stream instruction validation | 9204 §4.1.3, §4.4.3 | `qpack/decoder.rs:79-82` `control()` is still `// TODO: process control instructions` → `Ok(())`; `pub struct Decoder {}` is still empty |
| Reserved-from-HTTP/2 frame types `0x02/0x06/0x08/0x09` → `H3_FRAME_UNEXPECTED` | 9114 §11.2.1 | `stream.rs:360-412` and `stream.rs:277-311` both fall through to `_ => ()` (ignored) |

---

## ALREADY-KNOWN (re-verified, not re-reported)

- **h3spec #23 / #25** (QPACK `4.1.3` dyn-table-capacity, `4.4.3` Insert-Count-Increment=0):
  `CF-QUICHE-UPGRADE`, named in `scripts/ci/h3spec-check.sh` and
  `audit/h3spec/s26-report.md` §5. **Re-verified on 0.29.1:** still inert — `control()`
  is a no-op TODO and `Decoder` holds no dynamic table, and `h3_config.rs:28-30`
  sets `qpack_max_table_capacity(0)` + `qpack_blocked_streams(0)`, so no dynamic
  table is ever allocated on either side. No amplification vector. Correct waiver.
- **The 10 transport waivers** (`initial_source_connection_id` etc.) — unchanged, quiche-owned.
- **CF-QUICHE-FRAME-COMPLETENESS** — the compensating guard is **still present and still
  needed**: `h3_bridge.rs:1717-1724` + `:1868-1892` captures `:status`/`content-length` at head time and,
  at `Event::Finished`, RESETs downstream when `body_relayed < declared_cl` (skipping
  HEAD/1xx/204/304). The documented residual (no-content-length mid-frame FIN) still stands.
- **F-S29-1 (H3-egress trailer drop) stayed fixed** — `conn_actor.rs:399` still uses
  `get_mut` (not `entry().or_insert_with()`), and `RespItem::Trailers` →
  `send_additional_headers(.., fin=true, ..)` at `conn_actor.rs:595`. Trailers propagate
  end-to-end on all three response legs (H1-backend chunked, H2-backend `frame.trailers_ref()`,
  H3-backend `on_trailers`).
- **Testcodec QPACK decoder is raw-only (no Huffman)** — documented per-harness
  (`crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs:42` etc.); the gateway-output direction
  correctly uses `quiche::h3::qpack::Decoder`. The two remaining raw-decoder call sites
  (`round8_h3_authority_enforced.rs:328`, `quic_listener_e2e.rs:432`) only read `:status`,
  which quiche static-indexes, and both fail LOUDLY (assert/`?`) if a Huffman literal ever
  appears — not vacuous.
- **Testcodec QPACK static table verified byte-identical to quiche's `STATIC_DECODE_TABLE`**
  (99/99 entries, 0 mismatches) — i.e. identical to RFC 9204 Appendix A. Varint codec
  (`varint.rs`) is correct per RFC 9000 §16 including the legal non-minimal encodings.
  `encode_qint`/`decode_qint` implement RFC 9204 §4.1.1 prefix integers correctly, and the
  §4.5.6 literal-literal-name 3-bit-prefix fix from S22 is intact.
- **Error-code constants** (`conn_actor.rs:61-93`) all match RFC 9114 §8.1 exactly, and
  `reset_h3_stream` (`conn_actor.rs:686-695`) maps `Shutdown::Write`→RESET_STREAM and
  `Shutdown::Read`→STOP_SENDING — **no arm swap** of the class found in prior sessions.
- **`lb-l7` `H3ToH1Bridge` / `H3ToH2Bridge` / `H3ToH3Bridge` are not on the production
  datapath** — `create_bridge` is called in production only at `h1_proxy.rs:2114`,
  `h1_proxy.rs:2382`, `h2_proxy.rs:2155`, `h2_proxy.rs:2511`, i.e. only the H1→H2, H1→H3,
  H2→H2 and H2→H3 directions. The H3-front cells build their own field lists in
  `h3_bridge.rs`. Findings against the three H3-source bridges would be INFO by construction;
  none are reported.

---

## H3-C1 — CRITICAL — `:method` / `:path` are written verbatim into a hand-rolled HTTP/1.1 request line ⇒ CRLF request splitting into the H1 backend

**Where:** `crates/lb-quic/src/h3_bridge.rs:923-945` (`build_h1_head`), reached from
`h3_to_h1_stream_resp` (`:1126`) → `write_h1_request` (`:1030`), which is the live H3-terminate
→ `tcp`/`h1` backend cell (`crates/lb/src/main.rs:1376` `wire_h3_terminate_backends`).

```rust
fn build_h1_head(req: &H3Request, framing: &H1BodyFraming) -> Vec<u8> {
    let mut s = String::with_capacity(128);
    s.push_str(&req.method);
    s.push(' ');
    s.push_str(&req.path);
    s.push_str(" HTTP/1.1\r\n");
    if !req.authority.is_empty() {
        s.push_str("Host: ");
        s.push_str(&req.authority);
        s.push_str("\r\n");
    }
```

**Spec:** RFC 9114 §4.1.2 — "A field value MUST NOT contain the zero value (ASCII NUL, 0x00),
line feed (ASCII LF, 0x0a), or carriage return (ASCII CR, 0x0d). ... An intermediary that
receives a malformed request or response MUST NOT forward it." RFC 9110 §5.5 gives the same
rule for any field value. RFC 9112 §2.1 fixes the request-line grammar as
`method SP request-target SP HTTP-version CRLF`.

**Who owns it:** us, entirely. Proven above: quiche's QPACK decoder returns raw bytes and
`process_frame` never inspects them.

**What the code does:** the full ingress path performs **zero** byte-level validation of the
`:method` / `:path` values:
1. `conn_actor.rs:892-901` — `String::from_utf8_lossy(h.value()).into_owned()` (CR/LF are
   valid ASCII and survive verbatim).
2. `h3_bridge.rs:261-357` `validate_request_pseudo_headers` — presence / duplication /
   ordering / CONNECT rules only; it never looks at a value's bytes.
3. `conn_actor.rs:970-984` — `lb_core::authority::validate` runs on `:authority` **only**
   (and it does reject `0x00..=0x1F`, so `:authority` is not a sink).
4. `build_h1_head` string-concatenates `:method` and `:path` onto the wire.

`SECURITY.md:84-88` asserts the opposite invariant — *"Every path where attacker- or
backend-controlled header/trailer bytes reach an H1 wire goes through hyper's typed
`HeaderName`/`HeaderValue`/`response::Builder`"* — and `docs/features.md:16-18` repeats it.
This function is a counterexample: it is a hand-rolled H1 request writer with no typed funnel.

**Exploit (single request, unauthenticated, default `quic` listener + `tcp`/`h1` backend):**
client sends HEADERS with `:method=GET`, `:scheme=https`, `:authority=example.com`, and

```
:path = "/a HTTP/1.1\r\nHost: internal.svc\r\nX-Trusted: 1\r\n\r\nGET /admin/shutdown"
```

`validate_request_pseudo_headers` returns `Ok` (all four pseudos present, correct order, no
duplicates); `authority::validate("example.com")` returns `Ok`. The bytes put on the backend
socket are:

```
GET /a HTTP/1.1\r\nHost: internal.svc\r\nX-Trusted: 1\r\n\r\n
GET /admin/shutdown HTTP/1.1\r\nHost: example.com\r\nContent-Length: 0\r\nConnection: close\r\n\r\n
```

The backend sees **two** requests. Request #1 carries an attacker-chosen `Host` and arbitrary
attacker headers (any header a backend trusts from "the gateway" — `X-Forwarded-For`,
`X-Real-IP`, an internal auth header). Request #2 is a completely attacker-controlled request
(any method, path, headers, body) that the gateway never intended to issue; note the
gateway's own `Connection: close` lands on request **#2**, not #1, so a compliant keep-alive
backend processes both and only then closes. The gateway relays response #1 to the H3 client
and discards the rest.

`:method` is an equivalent sink (`s.push_str(&req.method)` with no token validation).

**Blast radius / what limits it:** `h3_to_h1_stream_resp` calls `pooled.set_reusable(false)`
on every exit path (`:1173`, `:1178`, `:1185`), so the poisoned connection is never returned
to the pool — this is *not* a cross-client pool-poisoning bug. The damage is (a) arbitrary
header + `Host` injection on the forwarded request and (b) execution of a second,
fully attacker-controlled request on the backend, which is an access-control bypass wherever
the gateway is the enforcement point.

**Would a test catch it?** No. There is no CRLF/`:path`-injection test anywhere:
`validate_request_pseudo_headers` has 13 unit tests (`h3_bridge.rs:2080-2325`) and none
inspects a value; `tests/security_smuggling_*.rs` cover H1/H2 only; h3spec 0.1.13 has no
field-value case (its 15 HTTP/3 examples are enumerated in
`audit/h3spec/s26-h3spec-final.log`). `audit/security/s38-findings-parser.md:246-248`
explicitly hands this question off — *"the CRLF/NUL injection question is whether a header
VALUE survives to the backend request line, which is the H3→H1/H2 translation path, NOT this
function"* — and `s38-findings-protocol.md:160-176` then only clears the **H2**→H1 path
(hyper-typed). The H3→H1 request-line path was never checked by either auditor.

---

## H3-H1 — HIGH — no `content-length` ↔ DATA-sum reconciliation on the inbound H3 request

**Where:** `crates/lb-quic/src/conn_actor.rs:779-839` (`drain_request_body` — tracks
`body_seen` only against `MAX_REQUEST_BODY_BYTES`), `crates/lb-quic/src/h3_bridge.rs:1038-1050`
(framing chosen from the client's declared `content-length`), `h3_bridge.rs:988-1002`
(`write_body_chunk` writes raw bytes with no running total).

```rust
let cl = req.extra.iter().find_map(|(n, v)| {
    if n.eq_ignore_ascii_case("content-length") { v.trim().parse::<u64>().ok() } else { None }
});
match cl {
    Some(n) => (H1BodyFraming::ContentLength(n), Some(b.clone())),
    None => (H1BodyFraming::Chunked, Some(b.clone())),
}
```

**Spec:** RFC 9114 §4.1.2 — "A request or response is also malformed if the value of a
content-length header field does not equal the sum of the DATA frame payload lengths that form
the content ... An intermediary that receives a malformed request or response MUST NOT forward
it." Detection is a stream error of type `H3_MESSAGE_ERROR`.

**Who owns it:** us. quiche never reads `content-length` and (proven above) does not enforce
DATA completeness at FIN either.

**Failure 1 — over-delivery becomes a pipelined request on the H1 backend wire.**
Client sends `content-length: 0` in the field section, then a DATA frame of N attacker-chosen
bytes, then FIN. `write_h1_request` picks `H1BodyFraming::ContentLength(0)`, emits
`Content-Length: 0\r\n`, and then `write_body_chunk(.., chunked=false)` writes all N bytes raw.
The backend reads a bodyless request followed by N bytes it will parse as a pipelined request.
(Our request #1 carries `Connection: close`, so a strictly compliant backend discards the
pipelined bytes — that is what keeps this HIGH and not CRITICAL — but the gateway is
nevertheless emitting a framing violation of its own construction, and read-ahead
implementations do process it.)

**Failure 2 — under-delivery deadlocks the leg with no deadline.**
Client sends `content-length: 1000000`, one DATA byte, then FIN. `Event::Finished` →
`ReqBodyEvent::End` → `write_h1_request` sets `clean_end = true` and returns
`ReqWriteOutcome::Complete` (`h3_bridge.rs:1116`) even though only 1 of 1 000 000 declared bytes
was written. `h3_to_h1_stream_resp:1168` then calls `stream_h1_response`, which parks in
`stream.read(&mut rbuf).await` (`h3_bridge.rs:596`) while the backend parks waiting for the
remaining 999 999 request bytes. See H3-H2 for why nothing ever breaks that deadlock.

**Would a test catch it?** No. `crates/lb-quic/tests/h3_h1_stream_body*.rs` exercise cap/abort
paths (F-CAP-1, F-MD-4) but never a `content-length` that disagrees with the delivered DATA.

---

## H3-H2 — HIGH — the H3→H1 and H3→H2 legs have no deadline, and response tasks are never aborted at actor teardown

**Where:** `crates/lb-quic/src/h3_bridge.rs:574-808` (`stream_h1_response`) and `:1030-1117`
(`write_h1_request`) contain **no** `tokio::time` construct; `h3_to_h2_stream_resp` (`:1303`)
likewise. `crates/lb-quic/src/conn_actor.rs:317` is the only lifecycle management:
`resp_tasks.retain(|h| !h.is_finished());` — the tasks are reaped when they finish and
**never aborted**. The teardown at `conn_actor.rs:339-341` aborts WS tunnel tasks only.

**Contrast (this is the asymmetry that makes it a defect, not a design):** the `→H3` connector
does have a deadline — `H3_RESP_IDLE_TIMEOUT` (30 s, `h3_bridge.rs:74`), armed at `:1592` and
enforced by `'evloop: while tokio::time::Instant::now() < idle_deadline` at `:1766`. The
H1/H2-upstream cousins of the same function got none.

**Spec/impact:** not a wire-format rule but the direct amplifier of H3-H1. Chained:
1. client sends HEADERS(`content-length: 1000000`) + 1 DATA byte + FIN;
2. the gateway writes a `Content-Length: 1000000` head + 1 byte and declares the request complete;
3. `stream_h1_response` parks forever in `read()`; the pooled `TcpStream` stays checked out;
4. the client immediately closes the QUIC connection — `reap_client_cancelled_responses`
   (`conn_actor.rs:351`) drops `resp_rx`, but the task is blocked in `read()`, not in
   `tx.send()`, so it never observes `ClientGone`;
5. the actor returns without aborting `resp_tasks`.

Each iteration leaks one tokio task plus one backend TCP connection until the *backend's* own
timeout fires. Repeating it drains `TcpPool` capacity against the backend — a cheap
remote resource-exhaustion primitive that needs no sustained connection from the attacker.
The same shape is reachable without H3-H1 by any slow/hung backend.

**Would a test catch it?** No — there is no H3-front timeout or task-leak test.

---

## H3-M1 — MEDIUM — RFC 9114 §4.2 field-section rules are not enforced on inbound H3 requests

**Where:** `crates/lb-quic/src/h3_bridge.rs:261-357` (`validate_request_pseudo_headers`) — the
file's own doc-comment calls it "the sole authority" because "quiche does not validate these".
Regular (non-`:`) fields only ever set `seen_regular = true` / `seen_host = true`:

```rust
} else {
    seen_regular = true;
    if name.eq_ignore_ascii_case("host") {
        seen_host = true;
    }
}
```

**Spec:** RFC 9114 §4.2 — "An endpoint MUST NOT generate an HTTP/3 field section containing
connection-specific fields ... Any message containing connection-specific fields MUST be
treated as malformed. The only exception ... is the TE header field, which MAY be present in an
HTTP/3 request if its value is 'trailers'." And: "characters in field names MUST be converted to
lowercase prior to their encoding. A request or response containing uppercase characters in
field names MUST be treated as malformed."

**Who owns it:** us (proven §0 — quiche performs no name/value inspection).

**Unenforced, concretely:** `connection`, `keep-alive`, `proxy-connection`, `transfer-encoding`,
`upgrade` in an H3 request field section; `te` with any value other than `trailers`; uppercase
field names (`Content-Length`, `X-Foo`); empty field names; CR/LF/NUL inside any regular field
value. All are accepted and the request is proxied.

**Downstream effect:** partly neutralised, which is why this is MEDIUM and not higher —
H3→H1 and H3→H3 drop `req.extra` entirely (see H3-M6), and on H3→H2 hyper strips
connection-specific headers on its own client send path (`hyper-1.11.0/src/proto/h2/client.rs:709`
→ `strip_connection_headers`, which also drops non-`trailers` `TE`). What remains is the
conformance violation itself: the gateway forwards a message RFC 9114 requires it to reject,
and it silently depends on a library detail it does not assert anywhere.

**Would a test catch it?** No. The h3spec waiver script's 15 HTTP/3 examples contain no
field-section case (`audit/h3spec/s26-h3spec-final.log`), and none of the 13
`validate_request_pseudo_headers` unit tests uses a regular header other than `("host", ..)`.

---

## H3-M2 — MEDIUM — `:authority` / `Host` agreement and non-emptiness are never checked (RFC 9114 §4.3.1)

**Where:** `crates/lb-quic/src/h3_bridge.rs:314-320` tracks `seen_authority` and `seen_host` as
booleans and never compares the two values; `conn_actor.rs:970` runs
`lb_core::authority::validate` only under `if !req.authority.is_empty()`.

**Spec:** RFC 9114 §4.3.1 — "If the `:scheme` pseudo-header field identifies a scheme that has a
mandatory authority component ... the request MUST contain either an `:authority` pseudo-header
field or a `Host` header field. **If these fields are present, they MUST NOT be empty. If both
fields are present, they MUST contain the same value.**"

**Three concrete violations:**

1. **`:authority` ≠ `Host` accepted and both forwarded.** On the H3→H2 cell,
   `h2_request_body_from_rx` (`h3_bridge.rs:1266-1281`) builds
   `uri = format!("{scheme}://{authority}{path}")` from `:authority` **and** re-emits every
   entry of `req.extra` — including the client's `host` — via `builder.header(n, v)`. The H2
   backend therefore receives `:authority: X` together with `host: Y`. RFC 9113 §8.3.1 makes
   that malformed at the backend; whether the backend rejects it or picks one is outside our
   control, which is exactly the routing-confusion condition the check exists to prevent.
2. **Empty `:authority` accepted.** An empty `:authority` sets `seen_authority = true`, so the
   §4.3.1 mandatory-authority branch passes; `authority::validate` is then skipped by the
   `is_empty()` guard; and `build_h1_head:930` skips the `Host:` line altogether — the gateway
   emits an HTTP/1.1 request with **no** `Host` header, which RFC 9112 §3.2 requires a server
   to answer with 400.
3. **Empty `:path` accepted.** §4.3.1 — "This pseudo-header field MUST NOT be empty for http or
   https URIs." `validate_request_pseudo_headers` only checks presence (`seen_path`), so an
   empty `:path` produces the request line `GET  HTTP/1.1` (two spaces).

**Related, LOW, reported here for completeness:** the H2 front runs
`check_authority_host_agreement` (`h2_proxy.rs:805`) and `check_sni_authority` →
421 Misdirected (`h2_proxy.rs:829-840`); the H3 front runs neither. The security impact of the
missing SNI check is small — the `quic` listener is single-cert
(`crates/lb/src/main.rs`, `quic_cfg.cert_path`) and does no SNI-based routing — but the
posture difference between fronts is undocumented.

**Would a test catch it?** No. `crates/lb-quic/tests/round8_h3_authority_enforced.rs` covers
`authority::validate` rejections (comma, control chars) only.

---

## H3-M3 — MEDIUM — the H3→H1 response reader mishandles 1xx informationals and bodiless responses

**Where:** `crates/lb-quic/src/h3_bridge.rs:574-808` (`stream_h1_response`). The function does
not take the request method and contains no status-code special-casing at all: it takes the
first `\r\n\r\n` as the head and picks framing purely from `Content-Length`/`Transfer-Encoding`
(`:658-668`).

**Spec:**
- RFC 9110 §15.2 — "A proxy MUST forward 1xx responses unless the proxy itself requested the
  generation of the 1xx response."
- RFC 9112 §6.3 rule 1 — "Any response to a HEAD request and any response with a 1xx, 204, or
  304 status code is always terminated by the first empty line after the header fields ... and
  thus cannot contain a message body."

**Three concrete failures:**

1. **1xx (notably 103 Early Hints).** A backend that emits `HTTP/1.1 103 Early Hints\r\nLink:
   ...\r\n\r\n` before the final response: `find_header_sep` stops at the 103's terminator,
   `parse_status_line` yields 103, no CL and no TE ⇒ `RespFraming::Eof` ⇒ the gateway emits
   `:status 103` as the **final** H3 response and streams the raw bytes of the real response
   (`HTTP/1.1 200 OK\r\n...`) to the client as the 103's **body**. The client never sees the
   real status or headers. 103 is emitted unsolicited by real origins.
2. **HEAD is broken on this cell.** For `HEAD`, the backend replies with
   `Content-Length: N` and no body. `RespFraming::ContentLength(N)` makes the reader wait for N
   body bytes; the backend closes (it received our `Connection: close`), `nr == 0` ⇒
   `RespAbort::PrematureEof` ⇒ `RespEvent::Reset` ⇒ the client's stream is RESET. A HEAD request
   through H3→H1 can never succeed. (The `→H3` connector *does* handle this — `req_is_head` at
   `h3_bridge.rs:1720-1724` — which shows the omission here is an oversight, not a policy.)
3. **304 with `Content-Length`** takes the same path as (2) and RESETs. 204/304 without CL
   happen to work, but only via `RespFraming::Eof` waiting for the backend's close.

**Would a test catch it?** No — a grep for `HEAD`, `103`, `204`, `304` across
`crates/lb-quic/tests/` and `tests/bridging_h3_h1.rs` returns nothing.

---

## H3-M4 — MEDIUM — a malformed request trailer is signalled as 413 / 502 / `H3_INTERNAL_ERROR`, never `H3_MESSAGE_ERROR`

**Where:** `crates/lb-quic/src/conn_actor.rs:905-919`:

```rust
// RFC 9114 §4.3: a pseudo-header in a trailing field section is malformed.
if headers.iter().any(|(n, _)| n.starts_with(':')) {
    tracing::warn!(stream_id = sid, "INC-2: H3 trailer pseudo-header rejected (RFC 9114 §4.3)");
    if let Some(tx) = body_tx_by_stream.remove(&sid) {
        let _ = tx.try_send(ReqBodyEvent::Reset);
    }
    body_seen.remove(&sid);
    pending_trailers.remove(&sid);
    continue;
}
```

**Spec:** RFC 9114 §4.1.2 — "Malformed requests or responses that are detected MUST be treated
as a stream error of type `H3_MESSAGE_ERROR`."

**What happens instead:** the rejection is signalled only by pushing `ReqBodyEvent::Reset` down
the body channel; the request stream is never reset with `H3_MESSAGE_ERROR`. What the client
observes then depends on the backend protocol:
- **H3→H1:** `write_h1_request` maps a `Reset` to `ReqWriteOutcome::Aborted(413, b"payload too
  large")` (`h3_bridge.rs:1096`) — the client gets **`413 Payload Too Large`** for a malformed
  trailer, which is both the wrong signal and actively misleading.
- **H3→H2:** `H3ReqStreamBody::poll_frame` errors, hyper RST_STREAMs, `send_request` fails →
  inline **502**.
- **H3→H3:** `J2ReqAction::AbortNoFin` → `RespAbort::UpstreamReset` → the client stream is reset
  with `H3_INTERNAL_ERROR` (`0x0102`) at `conn_actor.rs:615`.

Three different wire signals for one malformed-message condition, none of them the mandated one.
The head-validation path immediately above it gets this right
(`reset_h3_stream(conn, sid, H3_MESSAGE_ERROR)`, `conn_actor.rs:967`), which is what makes the
trailer path an inconsistency rather than a policy.

**Would a test catch it?** No. Only the *response*-direction pseudo-trailer rejection is tested
(`crates/lb-quic/tests/h3_h3_stream_e2e.rs:1835`); the request direction has no assertion on the
resulting status or error code.

---

## H3-M5 — MEDIUM — ORACLE: both "QPACK bomb" security tests assert a test-only helper that is dead in production

**Where:** `tests/security_qpack_bomb.rs` (whole file) and
`tests/codec_roundtrip_h3.rs:137-159` (`test_h3_qpack_bomb_mitigation`), both exercising
`lb_h3_testcodec::QpackBombDetector` (`crates/lb-h3-testcodec/src/security.rs`).

```rust
//! Exercises the `QpackBombDetector` from `lb-h3-testcodec` to verify that
//! excessive decompression ratios are detected and rejected.
use lb_h3_testcodec::QpackBombDetector;

#[test]
fn test_qpack_bomb_detection() {
    let detector = QpackBombDetector::new(100, 65536);
    assert!(detector.check(1000, 2000).is_ok());
    ...
}
```

**Why it is an oracle defect:** `QpackBombDetector` is referenced nowhere outside those two
tests and its own in-crate unit tests — a repo-wide grep for `QpackBombDetector` returns exactly
`crates/lb-h3-testcodec/src/security.rs`, `tests/security_qpack_bomb.rs:10` and
`tests/codec_roundtrip_h3.rs:142`. `lb-h3-testcodec` is a **dev-dependency only**
(`crates/lb-quic/Cargo.toml:66-68`). So a test file named for a gateway security property
asserts nothing whatsoever about the gateway; it is a self-test of unreachable code, duplicated
in two places, and it duplicates the crate's own unit tests
(`security.rs` `normal_input_ok` / `ratio_exceeded` / `size_exceeded` / `zero_encoded`).

**What is actually defending the surface, and is untested:** `h3_config.rs:24`
`set_max_field_section_size(1 << 20)`, enforced inside quiche
(`qpack/decoder.rs` `left.checked_sub(..).ok_or(Error::HeaderListTooLarge)` →
`mod.rs:3037-3046` → `Error::ExcessiveLoad` → `conn.close`). A grep for
`MAX_FIELD_SECTION_SIZE` outside `h3_config.rs` finds **no** test — nothing pins that the 1 MiB
envelope is actually applied on the wire, and nothing would catch a regression that dropped
the `set_max_field_section_size` call (quiche's default is `u64::MAX`, i.e. unbounded).

Secondary (INFO): `QpackBombDetector::check` itself has a hole — when `encoded_size == 0` and
`decoded_size <= max_decoded_size`, `checked_div` yields `None`, the `if let Some(ratio)` arm is
skipped, and the check returns `Ok` on an infinite ratio.

---

## H3-M6 — MEDIUM — the H3→H1 and H3→H3 cells drop every client request header

**Where:**
- H3→H1: `h3_bridge.rs:923-945` — `build_h1_head` emits only the request line, `Host`, one
  framing header and `Connection: close`. `req.extra` is never read (only consulted at `:1038`
  to *pick* the framing). The field's doc-comment says so: *"Non-pseudo headers; not emitted on
  the H1 leg, hence the `dead_code` allow"* (`h3_bridge.rs:197-199`).
- H3→H3: `h3_bridge.rs:1551-1556`:

```rust
let headers: Vec<(String, String)> = vec![
    (":method".to_string(), req.method.clone()),
    (":scheme".to_string(), "https".to_string()),
    (":authority".to_string(), authority),
    (":path".to_string(), req.path.clone()),
];
```

  Four pseudo-headers, and nothing else — no comment explains the omission on this leg.

**Spec:** RFC 9110 §7.6 / RFC 9114 §4.2 — an intermediary removes connection-specific fields and
forwards the rest. Dropping `Authorization`, `Cookie`, `Content-Type`, `Accept`,
`Accept-Encoding`, `Range`, `If-None-Match`, `User-Agent`, … is not a permitted transformation.

**Impact:** the H3→H1 and H3→H3 rows of the "✅ 9-cell matrix" in `docs/features.md:20-24` do not
actually proxy HTTP requests — every authenticated, content-typed, conditional or ranged request
arrives at the backend stripped. The direction of failure is mostly safe (a backend sees an
unauthenticated request and 401s rather than the reverse), which is why this is MEDIUM rather
than HIGH, but `Content-Type`-based CSRF checks and `Range`/`If-*` semantics silently change.
Only H3→H2 forwards `req.extra` (`h3_bridge.rs:1275-1281`).

**Would a test catch it?** No — no H3-front test asserts that any client request header reaches
the backend (`grep` for `x-custom|user-agent|authorization|cookie` across
`crates/lb-quic/tests/h3_h1_*.rs` / `h3_h3_*.rs` returns nothing). `docs/known-limitations.md`
does not record it either.

---

## H3-L1 — LOW — hand-rolled H1 response-head parsing in `stream_h1_response` diverges from RFC 9112

**Where:** `crates/lb-quic/src/h3_bridge.rs:639-668` (head fields) and `:494-513`
(chunk-size line). This is a second hand-rolled H1 parser, independent of hyper and of
`lb-h1`, and no `SmuggleDetector` is wired to it.

```rust
for line in lines {
    let Some((k, v)) = line.split_once(':') else { continue };
    let k = k.trim().to_ascii_lowercase();
    if k == "content-length" {
        match v.trim().parse::<usize>() { Ok(n) => content_length = Some(n), ... }
```

1. **Duplicate `Content-Length` — last wins.** RFC 9112 §6.3: a message with an invalid
   `Content-Length` "MUST be treated as an unrecoverable error". Two differing values silently
   resolve to the last. (`Content-Length: 5, 5` *is* caught — `parse::<usize>()` fails ⇒
   `BadHead`.)
2. **Signed forms accepted.** Rust's `FromStr`/`from_str_radix` accept a leading `+`, so
   `Content-Length: +10` parses as 10 and a chunk-size line `+5` parses as 5, against
   RFC 9112 §6.2 (`1*DIGIT`) and §7.1 (`1*HEXDIG`). The chunk-size hex is additionally
   `.trim()`ed (`:509`), accepting leading/trailing whitespace the grammar forbids.
3. **obs-fold is promoted to a new header.** RFC 9112 §5.2 requires a proxy to replace an
   obs-fold in a response with SP or reject the message; here a continuation line
   `\r\n  evil: value` is split on `\r\n`, `k.trim()` strips the leading whitespace, and the
   fold becomes a first-class `evil: value` header forwarded to the client.
4. **Field lines without a colon are silently skipped** (`continue`) rather than treated as
   malformed.
5. **CR/LF in a forwarded field value.** Lines are split on `\r\n` only, so a bare `LF` inside a
   value survives into `fwd_headers` and is QPACK-encoded verbatim (quiche's
   `encode_str::<false>` does not validate values), producing an H3 field section that
   RFC 9114 §4.1.2 defines as malformed and forbids an intermediary from forwarding. The same
   applies to the `Wire` arm of `H3RespOut::on_head` (`h3_bridge.rs:1415-1440`) for values
   coming from an H3 backend.

**Why LOW:** `h3_to_h1_stream_resp` marks the pooled connection non-reusable on every exit
(`:1173`/`:1178`/`:1185`) and the request carries `Connection: close`, so 1–4 cannot desync a
*subsequent* request; the observable damage is a truncated or mis-framed response on the one
stream, plus (5) a malformed field section delivered to the client.

---

## H3-L2 — LOW — the H3→H3 cell silently drops request trailers while H1→H3 and H2→H3 forward them

**Where:** `crates/lb-quic/src/h3_bridge.rs:1562`:

```rust
stream_request_to_h3_upstream(headers, false, addr, sni, pool, body_rx, sink).await
```

versus `crates/lb-l7/src/h1_proxy.rs:1659` and `crates/lb-l7/src/h2_proxy.rs:2047`, both
`/* forward_req_trailers = */ true`.

**Spec:** RFC 9114 §4.1 permits a trailing field section on a request; RFC 9110 §6.5 expects an
intermediary to forward trailers it does not consume.

The machinery to forward them already exists and is used by two other cells
(`J2ReqAction::FinWithTrailers` → `send_additional_headers`, `h3_bridge.rs:1966-1990`); the
H3→H3 leg just passes `false` with no rationale in the doc-comment. The H3→H1 and H3→H2 drops
*are* justified in-code (`h3_bridge.rs:1077-1081`: forwarding them onto H1 needs chunked plus a
`Trailer:` announcement and is a smuggling vector) — H3→H3 has no such excuse, since the
upstream is HTTP/3.

---

## H3-L3 — LOW — ORACLE: the test codec's QPACK decoder rejects a valid never-indexed literal with static name reference

**Where:** `crates/lb-h3-testcodec/src/qpack.rs:285-348`.

**Spec:** RFC 9204 §4.5.4 (Literal Field Line with Name Reference) has the pattern `01NTxxxx`,
where bit 5 is `N` (never-indexed) and bit 4 is `T` (static). Both `N=0` (`0101xxxx`, `0x50`)
and `N=1` (`0111xxxx`, `0x70`) are valid with `T=1`.

The dispatch only matches `first & 0xF0 == 0x50`, so a `0x7x` first byte falls past every arm
(`0x70 & 0xE0 == 0x60 != 0x20`) and lands in the final `else`:

```rust
return Err(H3Error::QpackError(format!("unknown QPACK instruction byte: {first:#04x}")));
```

Never-indexed is exactly how conformant encoders mark sensitive fields (`authorization`,
`cookie`, `set-cookie`), so a hand-built or third-party-generated conformance block that uses it
fails to decode. Two arms are also mislabelled: `first & 0x80 == 0x80` (`10xxxxxx`) is
§4.5.2 Indexed Field Line with `T=0` (**dynamic** table), not "post-base indexed"; and
`first & 0xF0 == 0x40` is §4.5.4 with `T=0` (dynamic name ref), not "post-base name reference".
Both return an error, which is the right outcome for a static-only codec, but the messages will
mislead the next person debugging a crafted block.

**Why LOW:** the encoder never emits `N=1`, so the codec's own round-trips pass, and the
gateway-output direction uses `quiche::h3::qpack::Decoder`. It is a latent oracle limit, not an
active false-green.

---

## H3-L4 — LOW — reserved-from-HTTP/2 frame types are treated as ignorable, in the test codec and in quiche

**Where:** `crates/lb-h3-testcodec/src/frame.rs:132-136`:

```rust
// RFC 9114 §7.2.8: unknown frame types MUST be ignored, not rejected.
_ => H3Frame::Unknown { frame_type, payload: Bytes::copy_from_slice(payload_buf) },
```

**Spec:** RFC 9114 §11.2.1 — frame types `0x02` (PRIORITY), `0x06` (PING), `0x08`
(WINDOW_UPDATE) and `0x09` (CONTINUATION) "are reserved ... These frame types MUST NOT be sent,
and their receipt MUST be treated as a connection error of type `H3_FRAME_UNEXPECTED`." That is
a distinct rule from §7.2.8's "ignore unknown/GREASE types", and the comment conflates the two.

**Both sides are affected.** quiche 0.29.1 also ignores them: `stream.rs:360-412`
`validate_request_frame_type` lists only CANCEL_PUSH / SETTINGS / GOAWAY / MAX_PUSH_ID /
PRIORITY_UPDATE, and the control-stream arm (`stream.rs:296-310`) ends in `(_, true) => ()`. So
sending `0x02` on a request or control stream is silently ignored by the gateway today. This is
a **quiche deviation not currently on the `CF-QUICHE-UPGRADE` waiver list** — h3spec 0.1.13 has
no case for it, so the honest-green gate never sees it. No exploit path (the frames are
discarded, and §7.2.8 bounds the payload), but the waiver list should record it so "h3spec green"
is not read as "§11.2.1 conformant".

---

## H3-I1 — INFO — `H3RespOut::Decoded::on_head` skips the hop-by-hop strip and the cap accounting its `Wire` twin applies

`crates/lb-quic/src/h3_bridge.rs:1441-1458`: the `Decoded` arm filters `:`-pseudos only,
whereas the `Wire` arm (`:1417-1439`) additionally applies `is_response_hop_by_hop` and the
`total`/`cap` DoS accounting (its `total`/`cap` fields are destructured away as `..`).

Not a live defect: the H1 front re-strips via
`h1_proxy.rs:2204` (`crate::h2_to_h1::RESPONSE_HOP_BY_HOP`) and the H2 front relies on hyper's
encoder (documented at `h2_proxy.rs:2547-2549`); the byte budget is separately bounded by
quiche's 1 MiB `max_field_section_size`. Flagged only because the two arms of one enum
implement different policies, which is the shape that produces a real bug the next time a front
is added.

## H3-I2 — INFO — dead constant and stale comment

- `crates/lb-quic/src/h3_bridge.rs:43` `pub const MAX_TRAILER_BLOCK_BYTES: usize = 64 * 1024;`
  has **zero** references anywhere in the workspace. Its doc still describes a bound on a
  body-phase trailing HEADERS block that the hand-rolled framing used to enforce; the real bound
  is now quiche's 1 MiB `max_field_section_size`. A reader will believe trailing sections are
  capped at 64 KiB.
- `crates/lb-quic/src/h3_bridge.rs:197-199` — `H3Request::extra` carries
  `#[allow(dead_code)]` but is read in three places (`:1038` content-length lookup, `:1275`
  H3→H2 forwarding, `conn_actor.rs:1289-1293` WS subprotocol).

## H3-I3 — INFO — two documentation claims are falsified by H3-C1 / H3-M6

- `SECURITY.md:84-88` and `docs/features.md:16-18` assert that every path reaching an H1 wire
  goes through hyper's typed header types. `build_h1_head` is a raw `String` request-line
  builder on the live H3→H1 cell (H3-C1). `docs/glossary.md:32` repeats the claim.
- `docs/features.md:20-24` marks H3→H1 and H3→H3 "✅"; neither forwards client request headers
  (H3-M6). `docs/known-limitations.md` has no entry for it.

---

## Coverage note for the lead

The h3spec gate is doing what it claims, but its HTTP/3 half is 15 examples
(`audit/h3spec/s26-h3spec-final.log`) covering pseudo-header well-formedness, control-stream
rules, SETTINGS and two QPACK uni-stream items. It contains **no** case for field-value bytes,
uppercase names, connection-specific fields, `content-length` reconciliation, 1xx, HEAD, or
trailers. Every finding above sits in that blind spot — which is why "h3spec green + 16/16 CI"
and these findings are not in contradiction.
