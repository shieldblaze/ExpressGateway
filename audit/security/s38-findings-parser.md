# S38 Parser-Auditor Findings — wire-reachable byte-level parsers

**Auditor:** parser-auditor · **Date:** 2026-06-08 · **Base:** main @ b8a99078
(branch `feature/security-audit-s38`)
**Method:** by-hand line-by-line read of every wire-reachable hand-rolled parser
(production + test codecs), trust-boundary tracing of attacker-controllability,
concrete PoC authoring. NO builds/tests/fuzzers run (lead executes centrally).

---

## (1) CRITICAL / HIGH (read immediately)

**NONE.** No memory-unsafety, no wire-reachable panic-DoS, and no integer
over/underflow→OOB was found on any **production** parser. The crown-jewel
hand-rolled production parser (`lb_quic::public_header`) and the two backend-
facing H3 parsers (`parse_status_line`, the chunked decoder) and the
forge-resistant retry-token verifier are all structurally panic-free with
exact-length copies and checked slicing throughout. See §4 for the per-scope
defense + the test that proves each.

The findings below are **LOW / informational**: two are test-codec-only
robustness notes (no production call-site), one is a hardening observation on a
value-correctness edge (no panic, no security impact). None are exploitable on
the internet-facing path.

---

## (2) Findings table

| ID | Severity | Surface | Class | Prod vs Test |
|---|---|---|---|---|
| F-PARSE-1 | LOW (info) | `lb-h2/hpack.rs::decode_string` (test codec) | `start + len` add + unbounded decompressed output (no bomb cap) | TEST-only (no prod call-site) |
| F-PARSE-2 | LOW (info) | `lb-h3-testcodec/frame.rs::decode_frame` (test codec) | `header_total + payload_len_usize` add could overflow with a pathological `max_payload_size` | TEST-only |
| F-PARSE-3 | LOW (info) | `lb-h1/chunked.rs::parse_chunk_size_hex` | `checked_shl(4)` does not detect lost high nibble on the 16th digit (value wraps mod 2^64) | TEST-only; documented "belt-and-braces" |

No CRITICAL/HIGH/MEDIUM findings.

---

## (3) Per-finding detail

### F-PARSE-1 — `lb_h2::hpack::decode_string` integer add + no decompressed-size bound (TEST CODEC)
- **Severity:** LOW (informational; test-tool only).
- **Surface:** `crates/lb-h2/src/hpack.rs:317-326` (`decode_string`), reached from
  `HpackDecoder::decode` (hpack.rs:409). **Zero production call-sites** — confirmed
  by grep: `lb-l7` depends on `lb-h2` only for the security CONSTANTS +
  `HpackBombDetector` / `PingFloodDetector`, which are mirrored into the
  hyper/h2 server config in `lb-l7/src/h2_security.rs`. The hand-rolled
  `HpackDecoder` itself has no caller outside `lb-h2` tests.
- **Mechanism:** line 320 `let end = start + len;` where `len` is the
  attacker-decoded integer from `decode_integer(buf, 7)`. `decode_integer`
  (hpack.rs:284) caps continuation at `m > 28` (line 303), so `len` is bounded
  to ≈ `34.5e9` — far below `usize::MAX` on 64-bit, so `start + len` does **not**
  overflow on the audited target, and `buf.get(start..end)` then returns `None`
  (→ `H2Error::Incomplete`), no panic. On a hypothetical 32-bit target `len`
  could exceed `u32::MAX` and `start + len` would overflow → debug panic — but
  the deployment is 64-bit and this is a test codec. Separately, `decode()`
  (hpack.rs:409) has **no decompressed-size / ratio bound** (no `HpackBombDetector`
  wired in) — an HPACK bomb against the *test* decoder would allocate unbounded
  `headers`. This is exactly the test-tool gap the recon flagged; production H2
  is hyper/h2 with the bomb detector configured.
- **PoC (the exact test to add, `crates/lb-h2/src/hpack.rs` tests mod):**
  ```rust
  #[test]
  fn decode_string_oversized_len_returns_err_not_panic() {
      // Literal-with-incremental-indexing, new name (index 0), then a
      // string whose 7-bit-prefix length integer is encoded with the max
      // 5 continuation bytes (~34e9) but only a few payload bytes follow.
      // 0x40 = literal w/ incremental indexing, index 0.
      // 0x00 = name string len 0 (empty name).
      // value: 0x7F (prefix=127 max) + continuation 0xFF 0xFF 0xFF 0xFF 0x0F.
      let buf = [0x40u8, 0x00, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F];
      let mut dec = HpackDecoder::new(4096);
      let r = dec.decode(&buf);
      assert!(r.is_err(), "must reject, never panic/over-alloc");
  }
  ```
- **Disposition:** ACCEPT as a known test-codec property; OPTIONAL hardening:
  change `decode_string` line 320 to `let end = start.checked_add(len).ok_or(H2Error::Incomplete)?;`
  for 32-bit-portability hygiene (cheap, defense-in-depth). Production severity
  remains LOW because there is no production call-site.

### F-PARSE-2 — `lb_h3_testcodec::frame::decode_frame` total-length add (TEST CODEC)
- **Severity:** LOW (informational; test-tool only).
- **Surface:** `crates/lb-h3-testcodec/src/frame.rs:99` `let total = header_total + payload_len_usize;`.
  Test-only crate (production H3 = quiche::h3 since S26).
- **Mechanism:** `payload_len_usize` is bounded to `max_payload_size` at line 89
  *before* the add (`if payload_len > max_payload_size as u64 { return Err … }`),
  and `header_total ≤ 16`. So with any sane `max_payload_size` (the callers pass
  small caps) the add cannot overflow and `buf.get(header_total..total)` returns
  `None`→`Incomplete` on truncation. Only a pathological `max_payload_size` near
  `usize::MAX` would let the add overflow. No production reachability.
- **PoC (the exact test, `crates/lb-h3-testcodec/src/frame.rs` tests):**
  ```rust
  #[test]
  fn decode_frame_huge_declared_len_is_err_not_panic() {
      // type=DATA varint(0x00), payload-len varint declaring 2^30-1 (4-byte
      // form 0xBF FF FF FF) but no payload bytes follow. With a sane cap the
      // frame is FrameTooLarge or Incomplete — never a panic.
      let buf = [0x00u8, 0xBF, 0xFF, 0xFF, 0xFF];
      let r = decode_frame(&buf, 1 << 20);
      assert!(r.is_err());
  }
  ```
- **Disposition:** ACCEPT; OPTIONAL `checked_add` for hygiene. The realistic
  fix is to assert callers never pass a near-`usize::MAX` cap (they don't).

### F-PARSE-3 — `parse_chunk_size_hex` `checked_shl` does not catch lost high nibble (TEST CODEC)
- **Severity:** LOW (informational; the code comment already acknowledges this is
  "belt-and-braces" behind the 16-digit cap).
- **Surface:** `crates/lb-h1/src/chunked.rs:314-333`. Test codec (no production
  call-site; production H1 chunked decode is hyper's).
- **Mechanism:** for a 16-hex-digit chunk size, on the 16th digit
  `value.checked_shl(4)` (line 329) only returns `None` when the **shift amount**
  (4) ≥ the bit-width (64), which it never is — it does **not** detect that the
  top nibble of `value` is shifted out. So `0xF000_0000_0000_000F` parses to a
  wrapped value rather than an error. This is **not a panic and not a security
  issue**: the 16-digit cap (line 318) already bounds the input to the `u64`
  range, the result is only used as a chunk length that hyper/the caller then
  bounds, and the wrap is mod 2^64 (no OOB). Pure value-correctness pedantry.
- **PoC (the exact test, `crates/lb-h1/src/chunked.rs` tests):**
  ```rust
  #[test]
  fn chunk_size_16_digit_top_nibble_silently_wraps() {
      // 16 hex digits, top nibble set: the high 4 bits are dropped by shl.
      // Documents current behaviour; not a panic.
      let got = parse_chunk_size_hex(b"F000000000000001");
      // 0xF000000000000001 fits u64 exactly, so this is actually exact here;
      // the wrap only manifests if a 17th digit were allowed (it is not).
      assert_eq!(got.unwrap(), 0xF000_0000_0000_0001);
  }
  ```
  (Note: with the 16-digit cap the wrap is actually unreachable — a 16-digit
  hex number always fits u64. The finding is that the `checked_shl` guard is
  *inert* given the cap, not that a wrap occurs. Documented for completeness so a
  future reader does not relax the 16-digit cap believing `checked_shl` backstops
  it — IT DOES NOT.)
- **Disposition:** ACCEPT. KEEP the 16-digit cap (it is the real defense); the
  comment at chunked.rs:312-313 should be corrected to note `checked_shl` does
  not backstop a relaxed digit cap.

---

## (4) Proven-clean scopes (defense + the test that proves it)

### P5 — `lb_quic::public_header` (CROWN JEWEL, every Mode A datagram, internet-facing) — CLEAN
This parser CLAIMS "never panics on arbitrary input" (public_header.rs:163). The
claim is **structurally upheld**:
- **Crate-level lint enforcement:** `crates/lb-quic/src/lib.rs:63` sets
  `#![deny(clippy::unwrap_used, expect_used, panic, indexing_slicing)]` and this
  applies in `#[cfg(test)]` too (per `[[lbquic-clippy-indexing-slicing-in-tests]]`).
  Grep of the non-test body confirms ZERO raw indexing, `unwrap()`, `expect`,
  `panic!`, or `unreachable!`. The only `unwrap_or` uses (lines 295/296/307/308/344)
  are the infallible default-providing form, never the panicking `unwrap()`.
- **`decode_varint` (public_header.rs:134):** `len = 1usize << (first >> 6)` where
  `first >> 6 ∈ {0,1,2,3}` ⇒ `len ∈ {1,2,4,8}` — shift can never overflow. Length
  is checked (`buf.len() < len` → `Incomplete`) before the value loop; the loop
  uses `buf.get(i)` (checked). `val = (val << 8) | b` over ≤7 iterations of an
  8-bit shift stays in `u64`. No panic, no overflow.
- **`parse_long` (public_header.rs:183):** every field read is `pkt.get(range)`
  with explicit `TooShort`; DCID/SCID lengths are bounded (`> MAX_CID_LEN=20` →
  `DcidTooLong`/`ScidTooLong`); the cid-end offsets use `checked_add`. The Initial
  token path uses `usize::try_from(tok_len)` mapped to `Truncated`, then
  `checked_add` for `tok_end`, then `tail.get(tok_start..tok_end)`. The length-
  varint completeness check uses `usize::try_from(len_val).unwrap_or(usize::MAX)`
  (so a huge declared length safely yields `usize::MAX`, making
  `after_len.len() < usize::MAX` true → `Truncated`) — no overflow, no panic. VN
  (`version==0`) short-circuits before the type-bit match; the `Retry|VN` arm
  folds VN to avoid `unreachable!`.
- **`parse_short` (public_header.rs:337):** rejects `short_dcid_len==0` or `>20`
  up front; the caller-supplied length is **independently bounded** here even
  though `default_short_dcid_len` (passthrough.rs:743) reads
  `max_dcid_len_routed`, which lb-config validates to `1..=20`
  (`lb-config/src/lib.rs:1952`). The SCID-learned length on the reverse path
  (passthrough.rs:988, `scid.len()`) is already ≤20 because `parse_public_header`
  bounded the SCID. `end = 1usize.checked_add(short_dcid_len)` then
  `pkt.get(1..end)` — checked, no panic.
- **Trust boundary:** the input is one UDP datagram, length-truncated to
  ≤`MAX_UDP_DATAGRAM_SIZE` (65_535, udp_dataplane.rs:44). All attacker bytes;
  fully fuzz-coverable.
- **Test that proves clean (exists + extend):** the differential proptest
  `crates/lb-quic/tests/public_header_differential.rs:169-179`
  (`prop_assert!(res.is_ok(), "parse_public_header panicked on random input")`
  over `buf in any Vec<u8>` × `short_dcid_len in 0..=21`) is the no-panic harness.
  The lead's new `fuzz/fuzz_targets/quic_public_header.rs` libfuzzer target is the
  high-iteration backstop. **What fuzzing might miss / I checked by hand:** the
  Initial double-varint sequencing (token-len varint then length varint, both
  attacker-set, with `tok_end` slicing in between), the `usize::try_from(...)
  .unwrap_or(usize::MAX)` completeness semantics, and the short-header
  caller-supplied-length path (which a buffer-only fuzzer that always passes a
  fixed `short_dcid_len` would under-explore — recommend the fuzz target vary
  `short_dcid_len` across `0..=21`, mirroring the proptest).

### L-PARSE-3 / P-status — `lb_quic::h3_bridge::parse_status_line` + response-head decode — CLEAN
- **Defense:** `parse_status_line` (h3_bridge.rs:625) uses `splitn(3, ' ')` +
  `code.parse::<u16>()` — both total, returning `Result`, no indexing. The head
  read loop (h3_bridge.rs:947-1000) is bounded by `HEAD_CAP = 64 KiB`, slices
  with `split_off(sep+4)`/`truncate(sep)` where `sep` is a real `find` index,
  validates UTF-8 (`from_utf8` → `BadHead`), and parses content-length /
  transfer-encoding with `.parse()` (Err → Reset). No panic path on a malicious
  backend. The chunked decoder (h3_bridge.rs:795-890) bounds the chunk-size line
  (`MAX_CHUNK_SIZE_LINE`) and trailer section (`MAX_TRAILER_SECTION`), uses
  `usize::from_str_radix(.., 16)` (Err on overflow, no panic), and `drain(..nl+2)`
  where `nl ≤ MAX_CHUNK_SIZE_LINE` and the `\r\n` is proven present.
- **Scope:** backend-facing (semi-trusted); a malicious upstream is in scope but
  reaches only `Err(RespAbort::*)` + Reset, never a panic or OOB.
- **Test that proves clean:** add a backend-response fuzz/unit feeding truncated
  status lines, oversized heads (>64 KiB with no `\r\n\r\n`), huge chunk sizes
  (`FFFFFFFFFFFFFFFF\r\n`), and never-terminated trailer sections; assert each
  yields `Err(RespAbort::BadHead|ChunkedDecode)` not a panic.

### P6 — `lb_quic::h3_bridge::validate_request_pseudo_headers` — CLEAN (pure validator)
- **Defense:** operates on an already-decoded `&[(String,String)]` from quiche::h3
  (which enforces RFC 9114 §10.3 field-char rules on decode). Pure
  string-matching state machine: dup detection per pseudo-header, pseudo-after-
  regular ordering (#15), mandatory-set checks (#13), CONNECT / extended-CONNECT
  (RFC 8441/9220) inversion, `ws_enabled` gating of `:protocol`. No indexing, no
  unwrap, no allocation that scales with attacker input; every branch returns a
  static `&str` reason. Cannot panic.
- **Test that proves clean (the structured fuzz to add):**
  ```rust
  // tests/h3_pseudo_header_fuzz-style proptest (or the lead's h3_pseudo_header target)
  // Feed adversarial lists: duplicate :method/:scheme/:path/:authority/:protocol;
  // missing mandatory; pseudo-after-regular; :status (response-only) in a request;
  // :protocol with ws_enabled=false (must reject) and =true with non-CONNECT method;
  // CONNECT with stray :scheme/:path; CRLF/NUL embedded in a name (must NOT match a
  // known pseudo-header and fall to the prohibited arm). Assert: always returns
  // Result, never panics, and the ws-off path is byte-identical-reject for :protocol.
  #[test]
  fn pseudo_validator_never_panics_on_adversarial_lists() {
      use lb_quic::h3_bridge::validate_request_pseudo_headers;
      let cases: &[&[(&str,&str)]] = &[
          &[(":method","GET"),(":method","POST")],            // dup method
          &[("x","y"),(":method","GET")],                     // pseudo after regular
          &[(":status","200")],                                // response-only pseudo
          &[(":method","CONNECT"),(":protocol","websocket")], // ext-CONNECT missing :scheme/:path/:authority
          &[(":method","GET"),(":scheme","http"),(":path\r\nX","/")], // CRLF in name
      ];
      for c in cases {
          let owned: Vec<(String,String)> = c.iter().map(|(a,b)|(a.to_string(),b.to_string())).collect();
          let _ = validate_request_pseudo_headers(&owned, false); // must not panic
          let _ = validate_request_pseudo_headers(&owned, true);
      }
  }
  ```
  (Note for protocol-auditor: this validator is well-formedness only — the
  CRLF/NUL *injection* question is whether a header VALUE survives to the backend
  request line, which is the H3→H1/H2 translation path, NOT this function.)

### P11 — `lb_security::retry::RetryTokenSigner::verify` — CLEAN (forge-resistant AND panic-free)
- **Defense:** the token is attacker-echoed (wire-reachable), but the body is
  parsed **only after** constant-time HMAC verification (`ct_eq`, retry.rs:246) —
  an attacker cannot reach the field parser without forging a valid HMAC over the
  server's secret key. Independently, every byte read is panic-free: a minimum-
  length gate (retry.rs:227, `≥ 49`), then `body.get(a..a+N)` slices whose width
  exactly matches each `copy_from_slice` destination array (`[0u8;8]`, `[0u8;2]`,
  `[0u8;4]`, `[0u8;16]`), so `copy_from_slice` (retry.rs:255,311,315,319) can
  never panic on a length mismatch. `decode_peer` (retry.rs:297) bounds `ip_len`
  to 4/16 by `kind`, slices are checked, the offsets (`cursor+ip_len+2`,
  `cursor+odcid_len`) are small (≤ token len) — no `usize` overflow.
- **Test that proves clean:** existing `verify_rejects_tampered_token`,
  `verify_rejects_expired_token`, `verify_rejects_wrong_peer` (retry.rs:351-376)
  exercise the reject paths. ADD a no-panic fuzz over random `token: Vec<u8>` ×
  random peer asserting `verify` always returns `Result` (never panics) even on
  sub-minimum, garbage-kind, and odcid-len-overrun tokens.

### P12 — `lb_l7::sni_authority::check_sni_authority` / `split_host_port` — CLEAN (ASCII-anchored byte index)
- **Defense:** the risky part is the IPv6-bracket raw `&str` byte indexing
  `&s[..=end+1]` / `&s[end+2..]` (sni_authority.rs:141-142, mirrored in
  h2_proxy.rs:3533-3534 and h2_to_h1.rs). `end` is `find(']')` within
  `s.strip_prefix('[')`, so `]` sits at byte `end+1` in `s` — an ASCII byte, i.e.
  a guaranteed UTF-8 char boundary. `&s[..=end+1]` ends after that ASCII byte
  (boundary); `&s[end+2..]` starts after it (boundary, and `end+2 ≤ s.len()` when
  `]` is last → empty tail). So no `byte index not a char boundary` panic and no
  out-of-range, even though `authority` is the attacker-controlled
  `:authority`/Host (already a valid `http::uri::Authority` from hyper/h2).
- **Test that proves clean:** add a proptest feeding `authority in ".*"` (arbitrary
  UTF-8, including `[`, `]`, multibyte chars adjacent to brackets, `[` with no
  `]`, `[]`, `[::1]`, `[::1]:8080`, trailing `]`) to `split_host_port` /
  `check_sni_authority`; assert no panic. NOTE the THREE duplicated copies — a
  regression test should cover all three (or they should be de-duplicated).

### Test-codec frame/header parsers (lb-h1 parse.rs, lb-h2 frame.rs, lb-h3-testcodec) — CLEAN of panics
- **lb-h1 `parse.rs`:** request/status line + headers + trailers all use checked
  `.get()` slicing, UTF-8 validation, `tchar` field-name validation
  (parse.rs:202), empty-name rejection, and `MAX_HEADER_BYTES` caps checked
  *before* parsing (parse.rs:152-168). No unwrap/index. (Header VALUES are not
  control-char-scrubbed, but CR/LF cannot appear inside a CRLF-delimited line and
  this is a test codec.)
- **lb-h2 `frame.rs`:** `parse_frame_header` requires ≥9 bytes; `strip_padding`
  (frame.rs:136) guards `pad_len+1 > payload.len()` (pad_len ≤255, no overflow)
  and `payload.len()-pad_len` cannot underflow; `decode_frame` bounds
  `hdr.length ≤ max_frame_size` *before* `9+hdr.length` (frame.rs:422,429) so no
  overflow; HEADERS-PRIORITY needs ≥5 bytes (frame.rs:246); SETTINGS loop
  `i+6 ≤ len`; all reads checked. `table_get` (hpack.rs:206) rejects index 0,
  range-checks static, computes `dyn_index = index - static_len - 1` only when
  `index > static_len` (no underflow), and `dynamic.get` returns `Option`
  (evicted/OOB → `Err`, not panic). `decode_integer` caps continuation at
  `m > 28` (overflow guard present).
- **lb-h3-testcodec `varint.rs` / `frame.rs`:** fixed-width varint per 2-bit
  prefix, all `.get()`-checked, shifts ≤56; `decode_frame` length-bounded (see
  F-PARSE-2 for the only sharp edge, which is benign with sane caps).
- These are TEST TOOLS (no production call-site). A panic here would be a
  test-tool bug (LOW prod severity); none was found.

---

## Cross-auditor handoffs (parser surface touched, but the *decision* belongs elsewhere)
- **H3 response header forwarding** (h3_bridge.rs:2682-2691) uses
  `String::from_utf8_lossy` on quiche-decoded backend header names/values — this
  neutralises non-UTF-8 but does not strip CR/LF/NUL. quiche::h3 already validates
  field chars on decode, and the re-encode goes back through quiche::h3 / the
  H1/H2 front validators, so it is not a parser panic. The response-splitting /
  trailer-injection question is **protocol-auditor L-PROTO-3**. Flagged, not
  attributed here.
- The retry-token's *forge-resistance proper* (key rotation, replay/0-RTT) is
  **infra-auditor L-INFRA-4** territory; I only proved the parser is panic-free
  and HMAC-gated.
- `MAX_UDP_DATAGRAM_SIZE` per-datagram `Vec` clone on the Initial path
  (passthrough.rs:718) is a **resource-auditor** flood question, not a parser bug.
