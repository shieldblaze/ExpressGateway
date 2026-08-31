# S47 — Request-smuggling / desync red-team pass

Branch `review/s47-rfc-security` (main @ 01915a77). Scope: the 9 bridging cells,
`lb-security::SmuggleDetector` and its wiring, the H1/H2/H3 fronts, the H1/H2/QUIC
upstream pools. Read-and-reason only; no builds run (2 vCPU box). Every PoC below is
written as the byte sequence to send; none were executed.

Dependency sources read to establish the responsibility boundary:
`hyper-1.11.0`, `h2-0.4.15`, `quiche-0.29.1` (from `~/.cargo/registry`).

---

## 0. Executive summary

The H1 and H2 fronts are in good shape: the smuggle guard is genuinely wired on both,
`strip_hop_by_hop` is correct including `Connection`-listed tokens, the authority
choke point is real, and — the load-bearing fact — **no production code path ever
returns an H1 upstream socket to the pool**, so an H1 desync cannot cross clients.

The **H3 front is the hole.** `SmuggleDetector` is not called anywhere in `lb-quic`
(verified by grep across the whole workspace), quiche performs *zero* header-value
validation and says so in its own docs, and the H3→H1 cell materialises the upstream
HTTP/1.1 request line by **string concatenation of the client's `:method` and `:path`**.
That is an unauthenticated, complete request-forgery primitive against the backend.

The reason six prior passes missed it is recorded in the audit trail itself: the S38
report asserts, for all 9 cells, that there is "no string-built request line to inject
into", and the S38 parser findings assert that quiche "enforces RFC 9114 §10.3
field-char rules on decode". **Both statements are false.** See S47-SMG-07.

| ID | Sev | Cell(s) | File:line | Claim |
|----|-----|---------|-----------|-------|
| S47-SMG-01 | **CRITICAL** | H3→H1 | `crates/lb-quic/src/h3_bridge.rs:923-946` | `:method`/`:path` concatenated into the H1 request line, no CR/LF/SP validation |
| S47-SMG-02 | **HIGH** | H3→H1 | `crates/lb-quic/src/h3_bridge.rs:1038-1048` | client `content-length` forwarded without checking it against the DATA byte count (RFC 9114 §4.1.2); no deadline on the H1 leg |
| S47-SMG-07 | **HIGH** | all | `audit/security/s38-report.md:146-148`, `s38-findings-parser.md:213-215` | the "proven-clean, all 9 cells" evidence is false; it is why -01 survived |
| S47-SMG-03 | MEDIUM | H3→H1 | `crates/lb-quic/src/h3_bridge.rs:502-510`, `:644-651` | production chunked/CL lexers accept `+5`/leading-WS; the ROUND8-L7-02 fix landed in a dev-dep-only crate |
| S47-SMG-04 | MEDIUM | H1→*, H2→* | `crates/lb-l7/src/authority.rs:31-33`, `h1_proxy.rs:705-709`, `h2_proxy.rs:2629-2631` | duplicate `Host` accepted (RFC 9112 §3.2 MUST); gateway checks only the first, forwards both |
| S47-SMG-05 | LOW | H3→H1 | `crates/lb-quic/src/h3_bridge.rs:640-656` | hand-rolled H1 response parser: `contains("chunked")`, name-with-SP before `:`, obs-fold, empty field name |
| S47-SMG-06 | LOW | H1→*, H2→* | `h1_proxy.rs:726-734`, `h2_proxy.rs:783-791` vs `hooks.rs:64-67` | two call sites of the same guard disagree on non-visible-ASCII values (currently unexploitable — hyper closes it) |
| S47-SMG-08 | INFO | pool | `crates/lb-io/src/pool.rs:283-286` | "No caller today" doc is stale; and no production path ever re-parks an H1 socket |

Examined and found **clean** (§8) — recorded so the next pass does not re-walk them.

---

## 1. [CRITICAL] H3 `:method` / `:path` are concatenated into the upstream HTTP/1.1 request line (CWE-93, CWE-444)

- **CVSS**: `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:L` — 9.9
- **Cell**: H3→H1 (`quic` / `h3-terminate` listener with TCP backends). Also see §1.5 for the H3→H3 forwarding variant.
- **Location**: `crates/lb-quic/src/h3_bridge.rs:923-946` (`build_h1_head`), reached from
  `crates/lb-quic/src/conn_actor.rs:1042+` → `h3_to_h1_stream_resp` (`h3_bridge.rs:1126`) → `write_h1_request` (`h3_bridge.rs:1053`).
- **Class**: HTTP request-line / header injection on a protocol downgrade (CVE-2021-33193 class, H3 flavour).

### The code

```rust
fn build_h1_head(req: &H3Request, framing: &H1BodyFraming) -> Vec<u8> {
    let mut s = String::with_capacity(128);
    s.push_str(&req.method);          // <-- attacker-controlled, unvalidated
    s.push(' ');
    s.push_str(&req.path);            // <-- attacker-controlled, unvalidated
    s.push_str(" HTTP/1.1\r\n");
    if !req.authority.is_empty() {
        s.push_str("Host: ");
        s.push_str(&req.authority);   // <-- validated (lb_core::authority) — the one that IS checked
        s.push_str("\r\n");
    }
    ...
```

### Data flow, source → sink

1. **Source.** `crates/lb-quic/src/conn_actor.rs:892-901` turns quiche's decoded field
   list into `Vec<(String, String)>` with `String::from_utf8_lossy(h.value())`. CR (0x0D)
   and LF (0x0A) are valid UTF-8 and survive `from_utf8_lossy` byte-for-byte.

2. **quiche does not validate.** `quiche-0.29.1/src/h3/qpack/decoder.rs:243-260`
   (`decode_str`) returns a raw `Vec<u8>` — Huffman-decoded or copied — with no character
   check of any kind. `grep -rn "content.length\|10.3\|field.char" src/h3/` returns nothing.
   quiche's own doc, `src/h3/mod.rs:757-760`:
   > `/// The list of received header fields. The application should validate`
   > `/// pseudo-headers and headers.`

3. **The gateway's validator is presence-only.** `h3_bridge.rs:261-360`
   (`validate_request_pseudo_headers`) checks duplication, pseudo-after-regular ordering,
   the mandatory set, and the CONNECT / extended-CONNECT inversion. It never inspects a
   pseudo-header **value**. Its own doc comment even says *"quiche does not validate these,
   so this is the sole authority"* — the sole authority that does not look at the bytes.

4. **`:authority` is validated; `:method` and `:path` are not.**
   `conn_actor.rs:971-990` calls `lb_core::authority::validate(&req.authority)`, which
   rejects `0x00..=0x1F | 0x7F`, SP, HTAB and `,`. There is no equivalent call for
   `:method` or `:path` anywhere in `lb-quic`.

5. **Sink.** `build_h1_head` splices both into the request line.

### Proof of concept

Send an H3 request on a `quic` listener configured with TCP (`h1`) backends. The QPACK
field section is (`\r\n` shown literally; QPACK is length-prefixed so these bytes travel
intact):

```
:method   = GET
:scheme   = https
:authority= gateway.example
:path     = /admin/keys HTTP/1.1\r\nHost: internal-only.svc\r\nX-Forwarded-For: 127.0.0.1\r\nConnection: keep-alive\r\n\r\nGET /health
```

`build_h1_head` emits to the backend socket:

```
GET /admin/keys HTTP/1.1\r\n
Host: internal-only.svc\r\n
X-Forwarded-For: 127.0.0.1\r\n
Connection: keep-alive\r\n
\r\n
GET /health HTTP/1.1\r\n          <-- " HTTP/1.1\r\n" is build_h1_head's own suffix
Host: gateway.example\r\n
Content-Length: 0\r\n
Connection: close\r\n
\r\n
```

**The divergence.** The gateway believes it sent one request for `:path`, to
`Host: gateway.example`, with no `X-Forwarded-For`. The backend sees **two** requests, the
first of which is entirely attacker-authored — method, target, `Host`, and every header.
The gateway then reads the **first** response off the socket and returns it to the attacker
over H3. The attacker gets a full request/response channel to the backend that no gateway
check ever saw.

Minimal variant (proves the primitive without a second request):
`:path = / HTTP/1.1\r\nX-Injected: 1\r\nX-Pad:` — one injected header, one request.

`:method` is the identical sink; `:method = "GET /admin HTTP/1.1\r\nHost: x\r\n\r\nGET"` works the same way.

### Impact

- **Unauthenticated arbitrary request forgery to the backend**, with the response
  reflected to the attacker. Everything the gateway is the trust boundary for is bypassed:
  the `:authority` validator, the `Host`/SNI 421 check, the header-underscore policy,
  the smuggle detector, the 64 MiB body cap.
- **Header forgery.** The honest H3→H1 path emits *no* `X-Forwarded-For`, `X-Forwarded-Proto`,
  `Via` or `X-Forwarded-Host` at all (`build_h1_head` writes only the request line, `Host`,
  the framing header and `Connection: close`). A backend that authorises on `X-Forwarded-For`
  cannot distinguish a forged one from a real one, because the gateway never sets it here.
- **Response-queue desync is NOT reachable** on this cell: `h3_to_h1_stream_resp`
  (`h3_bridge.rs:1126`) calls `pooled.set_reusable(false)` on every outcome including the
  clean one, so the socket is never re-parked. Impact is confined to the attacker's own
  connection — which is already enough for the CRITICAL.

### 1.5 Forwarding variant (H3→H3)

`h3_to_h3_stream_resp` (`h3_bridge.rs:1551-1556`) builds the upstream field section as
`(":path", req.path.clone())` and ships it through
`quiche::h3::Header::new(n.as_bytes(), v.as_bytes())` (`h3_bridge.rs:1665-1668`) — raw bytes,
no validation. A `:path` containing CR/LF is therefore **forwarded** to the H3 upstream
rather than rejected, in violation of RFC 9114 §4.1.2 (which requires the gateway to treat
such a message as malformed). Harmless against a conforming H3 backend; it re-arms
S47-SMG-01 if the H3 backend is another gateway that downgrades to H1. Fix once, at ingress,
and both are closed.

### Existing test coverage: NONE

- `tests/bridging_h3_h1.rs` tests `lb_l7::h3_to_h1::H3ToH1Bridge`. **That bridge is not on
  the production H3 path.** `create_bridge` is only called from `h1_proxy.rs:2114/2382` and
  `h2_proxy.rs:2175/2511`, i.e. only the H1→H2, H1→H3, H2→H2 and H2→H3 legs. `H3ToH1Bridge`,
  `H3ToH2Bridge`, `H3ToH3Bridge`, `H2ToH1Bridge` and `H1ToH1Bridge` have no production caller.
  The H3→H1 bridging test therefore proves nothing about the H3→H1 cell.
- `tests/security_smuggling_*.rs` are `lb-security` unit tests; no H3 involvement.
- No test in the tree sends a `:path` containing a space or CR/LF over H3.

### Remediation sketch (for the lead's second pass — not applied)

Add a field-value predicate in `lb-core` beside `authority::validate` and call it from
`conn_actor.rs` immediately after `validate_request_pseudo_headers`, for **every** decoded
name and value, not just `:method`/`:path`: reject any byte `< 0x20`, `0x7F`, and SP inside
`:method`/`:path`; reject any uppercase byte in a field name (RFC 9114 §4.1.2). Reject with
`H3_MESSAGE_ERROR` (the stream error the file already uses). Belt-and-braces: make
`build_h1_head` return `Result` and re-check there, so the sink itself is safe.

---

## 2. [HIGH] H3→H1 forwards the client's `content-length` without checking it against the DATA byte count (CWE-444, CWE-400)

- **CVSS**: `CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:L/A:H` — 8.2
- **Cell**: H3→H1
- **Location**: `crates/lb-quic/src/h3_bridge.rs:1038-1048` (framing choice) and `:936-940` (emission)

```rust
Some(ReqBodyEvent::Chunk(b)) => {
    let cl = req.extra.iter().find_map(|(n, v)| {
        if n.eq_ignore_ascii_case("content-length") {
            v.trim().parse::<u64>().ok()
        } else { None }
    });
    match cl {
        Some(n) => (H1BodyFraming::ContentLength(n), Some(b.clone())),
        None    => (H1BodyFraming::Chunked, Some(b.clone())),
    }
}
```

The chosen `n` is written verbatim as `Content-Length: n`, then `write_body_chunk` streams
**whatever DATA bytes actually arrive** (`h3_bridge.rs:988-1000`, unframed in the
`ContentLength` arm). Nothing counts them against `n`.

### Responsibility boundary — this is exactly the check the H2 sibling gets for free

- `h2-0.4.15/src/proto/streams/recv.rs:695` `dec_content_length(frame.payload().len())` and
  `:705` `ensure_content_length_zero()` — the h2 crate rejects a stream whose DATA total
  over- or under-runs `content-length`, as a protocol error. So the **H2 front is protected
  by the library.**
- quiche has no content-length tracking at all (grep over `quiche-0.29.1/src/h3/` returns
  nothing). RFC 9114 §4.1.2 puts the duty on the endpoint. **The gateway does not discharge it.**

### PoC A — under-declare (surplus bytes at the backend)

H3 field section: `:method POST`, `:path /x`, `:authority a.example`, `content-length: 5`.
Then DATA frames totalling 120 bytes:

```
DATA: "AAAAA" + "GET /admin HTTP/1.1\r\nHost: internal\r\nX-Forwarded-For: 127.0.0.1\r\n\r\n"
```

The gateway writes `Content-Length: 5` and then all 120 bytes. The backend reads 5 bytes as
the body; the remaining 115 sit in its receive buffer as a pipelined request.
**Mitigating factor, stated honestly:** `build_h1_head` always appends `Connection: close`,
so an RFC-conforming backend will not process the pipelined request. This variant is
therefore *conditional* on a backend that read-aheads into a pipeline buffer or ignores the
close token on the read side. It is still a message the gateway MUST have rejected.

### PoC B — over-declare (unconditional, no backend assumption)

H3 field section with `content-length: 4294967295`; send exactly one DATA frame of 1 byte,
then FIN the stream cleanly.

- `write_h1_request` sees `ReqBodyEvent::Chunk` then `End` → `clean_end = true` →
  `ReqWriteOutcome::Complete` (`h3_bridge.rs:1080-1085`).
- The gateway has written `Content-Length: 4294967295` and 1 byte, and does **not** close
  the write half. It proceeds to `stream_h1_response`.
- The backend blocks forever waiting for ~4 GiB of body.
- **`stream_h1_response` (`h3_bridge.rs:574` onward) has no deadline of any kind** — I
  grepped lines 570-800 for `timeout|Instant|deadline` and there are zero hits.
  `H3_RESP_IDLE_TIMEOUT` (`h3_bridge.rs:74`) is used only by the →H3 upstream driver
  (`h3_bridge.rs:1592`), never on the H1 leg.

**Impact**: one QUIC packet pins one backend TCP connection plus one gateway task
indefinitely. `max_requests_per_h3_connection` defaults to 1000, so a single QUIC connection
yields up to 1000 pinned backend sockets. This assumes the backend has no body-read timeout
of its own — true for a default hyper backend, false for default nginx
(`client_body_timeout 60s`). Rated HIGH on that basis.

### Secondary defects in the same three lines

- `v.trim().parse::<u64>()` accepts a leading `+`: `content-length: +5` → 5. hyper avoids
  this deliberately — `hyper-1.11.0/src/headers.rs:71`:
  `// cannot use FromStr for u64, since it allows a signed prefix`.
- `find_map` takes the **first** `content-length` that parses. Duplicate `content-length`
  fields with different values are neither detected nor rejected (RFC 9114 §4.1.2 /
  RFC 9110 §8.6 make that malformed).
- `.trim()` on a Rust `&str` strips Unicode whitespace, so `content-length: \u{00A0}5` is
  also accepted as 5.

### Existing test coverage: NONE for H3 — and the H2 sibling IS covered

`tests/h2_validation_before_forward.rs` is a real end-to-end TLS test whose **case 1 is
literally "content-length ≠ Σ DATA lengths (RFC 9113 §8.1.2.6)"**, asserting a
PROTOCOL_ERROR and never the backend body. There is no H3 counterpart. That asymmetry is
the cleanest way to state the gap to the lead.

---

## 3. [HIGH] The S38 "proven-clean, all 9 cells" evidence is false, and is why S47-SMG-01 survived

- **Severity**: HIGH (per the brief's bar: evidence that asserts a mitigation which does not
  exist is worse than no evidence)
- **Location**: `audit/security/s38-report.md:145-148`; `audit/security/s38-findings-parser.md:213-215`

Two load-bearing claims are wrong.

**(a)** `s38-report.md:145-148`, under the heading *"Smuggling / desync (all 9 cells + WS
H1/H2/H3 + gRPC)"*:

> "Every attacker/backend-controlled header byte reaching an HTTP/1.1 wire is funnelled
> through hyper's typed `HeaderName`/`HeaderValue`/Builder (reject CR/LF/NUL, fail-closed)."
> … "H2→H1: … hyper builds the request line (no string-built request line to inject into)."

`crates/lb-quic/src/h3_bridge.rs:923` is a string-built request line on the H3→H1 cell that
never touches hyper's typed API. The claim is true for H2→H1 and false for H3→H1; it was
written as covering all nine.

**(b)** `s38-findings-parser.md:213-215`, finding P6:

> "operates on an already-decoded `&[(String,String)]` from quiche::h3 (**which enforces
> RFC 9114 §10.3 field-char rules on decode**)"

quiche 0.29.1 enforces nothing of the sort (§1, step 2). This is the premise on which the
H3 pseudo-header validator was recorded CLEAN.

The same P6 entry then closes with the correct question and routes it away:

> "(Note for protocol-auditor: this validator is well-formedness only — the CRLF/NUL
> *injection* question is whether a header VALUE survives to the backend request line,
> which is the H3→H1/H2 translation path, NOT this function.)"

— and the protocol side answered it with claim (a). The finding fell through the seam
between two auditors, each correctly deferring to the other.

**Also stale, lower stakes:**
- `SECURITY.md` defenses table rows 1, 2 and 4 name `crates/lb-h1/src/parse.rs` as the code
  site. `lb-h1` is a `[dev-dependencies]`-only crate (`Cargo.toml:158,165`); no production
  crate links it. SECURITY.md's own "Note on the production wire path" discloses this, so
  this part is **ALREADY-KNOWN** — but the rows still point at a non-production file.
- `audit/security/round-2-findings.md:527-533` (the Round-8 SEC-2-15 update) claims the
  nginx/hyper/HAProxy chunk-size rejection rules "now run inside our ChunkedDecoder"
  citing `crates/lb-h1/src/chunked.rs::parse_chunk_size_hex`. That decoder is in the
  dev-dep crate. See S47-SMG-03 for the production decoder, which still has the bug.
- The three tests SECURITY.md cites for smuggling (`tests/security_smuggling_cl_te.rs` 15
  lines, `security_smuggling_te_cl.rs` 11 lines, `security_smuggling_h2_downgrade.rs` 11
  lines) are pure `SmuggleDetector::check_*` unit tests. They are **not vacuous** — they
  assert real behaviour of the detector — but they are not evidence that the mitigation is
  wired, and no byte crosses a socket in any of them. The genuine wiring evidence is
  `crates/lb-l7/tests/smuggle_wired.rs`, which is good and includes a negative control
  (lenient bundle accepts what strict rejects, `smuggle_wired.rs:79-86`). SECURITY.md should
  cite that instead.

---

## 4. [MEDIUM] Production chunked / Content-Length response lexers accept non-RFC forms (CWE-444)

- **CVSS**: `CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:C/C:L/I:L/A:N` — 5.8
- **Cell**: H3→H1 (response direction; source is a semi-trusted backend)
- **Location**: `crates/lb-quic/src/h3_bridge.rs:502-510` and `:644-651`

```rust
let hex = std::str::from_utf8(line.get(..hex_end).unwrap_or(line))
    .map_err(|_| RespAbort::ChunkedDecode)?
    .trim();                                        // Unicode-whitespace trim
if hex.is_empty() { return Err(RespAbort::ChunkedDecode); }
let size = usize::from_str_radix(hex, 16).map_err(|_| RespAbort::ChunkedDecode)?;
```

RFC 9112 §7.1 `chunk-size = 1*HEXDIG`. This lexer accepts:

- `+5\r\n` — `usize::from_str_radix("+5", 16) == Ok(5)`. Exactly the nginx CVE-2013-2028 /
  HAProxy `h1_append_chunk_size` primitive that ROUND8-L7-02 was raised for.
- ` 5\r\n`, `\t5\r\n`, and `\u{00A0}5\r\n` — `str::trim` strips Unicode whitespace.

Overflow is safe (`from_str_radix` errors), and `MAX_CHUNK_SIZE_LINE` bounds the line, so
this is a parser **differential**, not memory unsafety. Same defect at `:645`:
`v.trim().parse::<usize>()` for the response `Content-Length` accepts `+100`; and the
response-header loop (`:644-651`) lets a **duplicate** `Content-Length` silently
**last-wins** with no conflict check, where hyper would reject.

**Why it is a finding and not noise**: ROUND8-L7-02 fixed this class in
`crates/lb-h1/src/chunked.rs::parse_chunk_size_hex`, a crate that is a dev-dependency only.
The decoder that actually runs in the binary — this one — was never touched. The audit
trail records the class as fixed.

**PoC**: backend responds
`HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n+5\r\nHELLO\r\n0\r\n\r\n`.
This gateway serves `HELLO`; a conforming intermediary in the same chain rejects the
response. Divergence between two hops over the same bytes.

---

## 5. [MEDIUM] Duplicate `Host` header accepted on the H1 and H2 fronts (CWE-444)

- **CVSS**: `CVSS:3.1/AV:N/AC:H/PR:N/UI:N/S:C/C:L/I:L/A:N` — 5.8
- **Cells**: H1→H1, H1→H2, H1→H3, H2→H1, H2→H2, H2→H3
- **Locations**:
  - `crates/lb-l7/src/authority.rs:31-33` — `.get(http::header::HOST)` (first value only)
  - `crates/lb-l7/src/h1_proxy.rs:705-709` — SNI/Host agreement reads `.get(HOST)`
  - `crates/lb-l7/src/h2_proxy.rs:2629-2631` — `check_authority_host_agreement` reads `.get(HOST)`

RFC 9112 §3.2 is a MUST: *"A server MUST respond with a 400 (Bad Request) … to any request
message that contains more than one Host header field line."*

hyper does not enforce it. I read `hyper-1.11.0/src/proto/h1/role.rs:258-330`: the request
parse loop matches only `TRANSFER_ENCODING`, `CONTENT_LENGTH`, `CONNECTION`, `EXPECT` and
`UPGRADE`; `HOST` is never inspected, and every field is `headers.append`-ed. So both `Host`
lines reach the gateway, and `Host` is not in `HOP_BY_HOP` (`h1_proxy.rs:36-46`), so hyper's
H1 client re-emits **both** to the backend.

**PoC (H1 front, over TLS with SNI `legit.example`)**:

```
POST /x HTTP/1.1\r\n
Host: legit.example\r\n
Host: internal-admin.svc\r\n
Content-Length: 0\r\n
\r\n
```

- `authority::validate_request` validates `legit.example` only → passes.
- `check_sni_authority(Some("legit.example"), "legit.example")` → passes, no 421.
- The backend receives both lines. Apache httpd takes the first; several frameworks
  (and some WAF/sidecar pairs) take the last.

**Divergence**: the gateway's PROTO-2-18 SNI↔Host control — a documented security control —
is evaluated against a value the backend may not be the one using.

The H2 front has the same shape: HPACK permits repeated `host` field lines, the h2 crate
appends them, and `check_authority_host_agreement` compares `:authority` against
`.get(HOST)` = the first only.

**Mitigating**: there is no path- or host-based routing in this gateway
(`BackendInfoPicker::pick_info()` is round-robin over a fixed list, `upstream.rs:67-104`),
so there is no gateway-side ACL to bypass. Impact is confined to backend-side host confusion.
No prior-art hit: `grep -rn -i "duplicate host|multiple host"` over `audit/ docs/ SECURITY.md`
returns nothing.

---

## 6. [LOW] Hand-rolled H1 response parser accepts forms RFC 9112 forbids

- **Cell**: H3→H1 (response direction)
- **Location**: `crates/lb-quic/src/h3_bridge.rs:640-656`

```rust
let Some((k, v)) = line.split_once(':') else { continue };
let k = k.trim().to_ascii_lowercase();
if k == "content-length" { ... }
else if k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked") { chunked = true; }
else if !is_response_hop_by_hop(&k) { fwd_headers.push((k, v.trim().to_string())); }
```

1. **`contains("chunked")`, not final-codec.** `Transfer-Encoding: chunked, gzip` (chunked
   NOT last) is treated as chunked, and so is `Transfer-Encoding: xchunkedy`. RFC 9112 §6.1
   requires chunked to be the final coding. Fails closed in practice (de-chunking non-chunked
   bytes yields `RespAbort::ChunkedDecode`), which is why this is LOW rather than MEDIUM.
2. **`k.trim()` accepts whitespace before the colon.** `Transfer-Encoding : chunked` is
   honoured. RFC 9112 §5.1 requires rejecting that (it is the classic proxy-differential form).
3. **obs-fold becomes a new header.** A continuation line `\tContent-Length: 100` is parsed
   by `split_once(':')` as a fresh `content-length` field, where RFC 9112 §5.2 says it is a
   continuation of the previous value.
4. **Empty and malformed field names are forwarded onto the H3 wire.** `: value` yields
   `("", "value")`; `X Foo: bar` yields `("x foo", "bar")`. Both go to the H3 client through
   `quiche::h3::Header::new` unchecked — an H3 field section that RFC 9114 §4.1.2 makes
   malformed for the *client* to receive.

Source is a semi-trusted backend and every variant fails closed or produces a malformed
response rather than a desync, hence LOW. Fixing (1) and (2) is cheap and worth doing while
S47-SMG-03 is being fixed in the same file.

---

## 7. [LOW] Two call sites of the same smuggle guard disagree on non-visible-ASCII header values

- **Cells**: H1→*, H2→*
- **Locations**:
  - `crates/lb-l7/src/h1_proxy.rs:726-734` and `crates/lb-l7/src/h2_proxy.rs:783-791`:
    ```rust
    .filter_map(|(n, v)| v.to_str().ok().map(|s| (n.as_str().to_owned(), s.to_owned())))
    ```
    `HeaderValue::to_str()` fails on any byte outside `0x20..=0x7E` plus HTAB (i.e. on
    obs-text, RFC 9110 §5.5). The **whole (name, value) pair vanishes from the detector's
    view**, while the header itself stays in `parts.headers` and is forwarded.
  - `crates/lb-security/src/hooks.rs:64-67`:
    ```rust
    let value_str = value.to_str().unwrap_or("");
    pairs.push((name.as_str().to_string(), value_str.to_string()));
    ```
    Here the **name survives**, so `check_cl_te` (name-only) and `check_te_cl`
    (`"" != "chunked"`) both still fire. This site is strictly safer.

The same divergence exists in all four production `BridgeRequest` constructions
(`h1_proxy.rs:2117-2129`, `:2389-2398`, `h2_proxy.rs:2178-2186`, `:2518-2526`).

**I attempted to build a bypass from this and it does not work — recording the refutation so
it is not re-hunted.** The obvious chain is: send `Transfer-Encoding: chunked\xFF` +
`Content-Length: 5`, have the TE pair filtered out of `header_pairs` so `check_cl_te` sees
only CL, and rely on hyper decoding chunked while forwarding the stale `Content-Length`.
It dies at hyper: `hyper-1.11.0/src/headers.rs:128-137` (`is_chunked_`) uses the **same**
`value.to_str()`, so an obs-text TE is "not chunked" → `role.rs:341-345`
`if is_te && !is_te_chunked { return Err(Parse::transfer_encoding_invalid()) }` → 400.

Worth noting for the fix pass: hyper does keep a stale `Content-Length` in the HeaderMap when
`Transfer-Encoding` appears **before** `Content-Length` (`role.rs:276-278` only removes CL when
`is_cl` was already true, i.e. CL came first; `role.rs:286-289` then `continue`s past the CL
branch but still `headers.append`s it at `:337`). `check_cl_te` is what stands between that and
a forwarded CL+chunked-body pair. Keep it, and prefer the `hooks.rs` name-preserving shape at
all six sites.

---

## 8. Examined and clean — recorded so the next pass does not re-walk them

- **H2→H1 downgrade (the brief's highest-risk item): clean.**
  `SmuggleDetector::check_all_mode(&header_pairs, SmuggleMode::H2)` is genuinely wired at
  `h2_proxy.rs:793`, before the strip and before the upstream acquire. The upstream request
  is built with hyper's typed `Request`/`http::Uri`, so a `:path` with SP/CR/LF cannot reach
  a request line — the h2 crate rejects a malformed `:path` at HEADERS decode. Connection-specific
  headers are additionally rejected by the h2 crate on the send side
  (`h2-0.4.15/src/proto/streams/send.rs:80-96`). `content-length` vs DATA is enforced by the
  h2 crate on recv (`recv.rs:695,705`). `check_authority_host_agreement` (`h2_proxy.rs:2624`)
  runs before the strip.
- **H3→H2: fails closed.** `h2_request_body_from_rx` (`h3_bridge.rs:1268-1305`) builds
  `format!("{scheme}://{authority}{path}")` and hands it to `Request::builder().uri(...)`;
  `http::Uri` parsing rejects SP/CR/LF, `builder.body()` returns `Err`, mapped to 502
  (`h3_bridge.rs:1333-1341`). It would be better as a 400, but there is no injection.
- **WS-over-H3 → H1 upstream handshake: fails closed.** `dial_backend_ws`
  (`crates/lb-l7/src/ws_proxy.rs:342-344`) does `format!("ws://{addr}{path}").parse()` into a
  `http::Uri`; the parse rejects, yielding a 502. Same for the H1/H2 WS dials
  (`h1_proxy.rs:1944`, `h2_proxy.rs:1188`).
- **H3 response → H1 front: fails closed.** `h3_decoded_resp_head_builder`
  (`h1_proxy.rs:2193-2215`) funnels every name/value through `Response::builder().header()`;
  a CR/LF value makes `builder.body()` fail and `build_h1_streaming_response`
  (`h1_proxy.rs:2218-2228`) substitutes a 500. No downstream response splitting.
- **Connection-pool poisoning across clients: not reachable for H1 upstreams.** Every
  production `TcpPool` acquire either detaches with `take_stream()` — `h1_proxy.rs:867`,
  `:1940`, `h2_proxy.rs:1184`, `:1225`, `grpc_proxy.rs:154`, `main.rs:1187` — or marks
  `set_reusable(false)` on every path (`h3_bridge.rs:1161,1173,1178,1185`). Nothing ever
  re-parks an HTTP upstream socket, so `PooledTcp::return_to_pool` (`pool.rs:303`) is
  effectively unreachable in production and a desync cannot cross clients.
- **`strip_hop_by_hop` (`h1_proxy.rs:2021-2040`): correct.** It collects `Connection`-listed
  token names *before* removing `Connection` itself, then removes all 8 static names plus the
  listed extras. `HeaderMap::remove` drops all values for a name, so duplicates are covered.
- **Request trailers: correct on all legs.** `validate_h1_request_trailers`
  (`h1_proxy.rs:81-97`) rejects `Content-Length`/`Transfer-Encoding`/`Host`/`Trailer`/`TE`/
  `Connection` in inbound H1 trailers; the H3→H1 and H3→H2 legs drop request trailers
  entirely with an explicit comment saying why (`h3_bridge.rs:1085-1090`).
- **`SmuggleDetector` itself.** `check_cl_te` rejects any CL+TE (stricter than RFC 9112 §6.1
  and stricter than hyper — correct). `check_te_cl` requires the final codec of **every** TE
  field to be `chunked`. `check_duplicate_cl` allows identical values (RFC 9110 §8.6) and
  rejects differing ones. One residual: a **single** `Content-Length: 5, 6` field is not
  caught by `check_duplicate_cl` (it stores the whole `"5, 6"` string and never compares) —
  but hyper closes it, splitting on `,` and returning `None` on conflict
  (`hyper-1.11.0/src/headers.rs:53-60`), which is a 400. Defence-in-depth gap only.
- **`check_h2_downgrade`'s pseudo-header arm is dead by design** at the `h2_to_h1.rs:54`
  call site (it runs on `regular_headers`, after pseudo extraction) — this is deliberate and
  documented at `h2_to_h1.rs:50-53`. Noted, not a finding. Separately, `H2ToH1Bridge` has no
  production caller at all, so that call site is library-surface only.
- **Expect: 100-continue.** Handled entirely inside hyper (`role.rs:317-322`); no gateway-side
  state, no interaction with the framing decision. `tests/informational_responses.rs:52`
  records that auto-handling is wire-level and cannot be disabled.
- **Absolute-form request targets / dot-segment normalisation.** No exploitable surface:
  the gateway performs **no** path- or host-based routing (`upstream.rs:67-104` is
  round-robin over a fixed backend list), so there is no "route on X, forward Y" gap.

## 9. Non-security observation worth a line in known-limitations

The H3→H1 and H3→H3 cells forward **no client request headers at all** — `build_h1_head`
writes only the request line, `Host`, the framing header and `Connection: close`, and
`h3_to_h3_stream_resp` ships only the four pseudo-headers. `H3Request::extra` carries an
`#[allow(dead_code)]` with the comment *"not emitted on the H1 leg"*. `Authorization`,
`Cookie`, `Content-Type`, `Accept` and the whole `X-Forwarded-*` set are silently dropped on
those two cells (H3→H2 does forward them). `docs/known-limitations.md` does not mention this.
Not a smuggling finding; it does bound the impact of S47-SMG-01 (there are no honest headers
for an injected one to contradict) and it is a surprising production behaviour to leave
undocumented.
