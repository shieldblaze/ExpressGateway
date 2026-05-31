# S22 — Independent Verification: H3 pseudo-header validation + QPACK §4.5.6 fix

- **Verifier role:** independent / adversarial
- **Repo:** /home/ubuntu/Code/ExpressGateway
- **Branch:** feature/h3spec-s22  **HEAD:** 1409eb76 (verified `git rev-parse HEAD`)
- **Scope under test:** h3spec v0.1.13 findings #12 (duplicate pseudo), #14
  (prohibited/unknown pseudo), #15 (pseudo after a regular field) must reset
  the request stream with `H3_MESSAGE_ERROR` (0x010e). #13 (absent `:authority`)
  is a deliberate deviation (SNI substitution) and must REMAIN failing.
- **No source files modified.** `git diff --name-only -- crates/` = empty.
  Only `audit/h3spec/*` written. (Pre-existing unstaged `s22-findings.md` was
  present before this session and untouched by me.)

## OVERALL VERDICT: **AGREE**

All claimed behaviour reproduces. #12/#14/#15 PASS against a live gateway;
#13 fails exactly as intended (the `:authority` deviation). The validator is
RFC-correct for the request-pseudo rules in scope, the negative control is
non-vacuous (proven at the wire level), the QPACK §4.5.6 fix is conformant and
panic-safe, and all regression tests are non-tautological and pass. One
NON-BLOCKING observation (#15 trips an earlier-but-still-correct arm for
h3spec's specific vector) is documented below with an independent control
proving the §4.3-ordering arm is itself reachable and load-bearing.

---

## Item 1 — RFC correctness of `validate_request_pseudo_headers`: **AGREE**

Read `crates/lb-quic/src/h3_bridge.rs` (the new fn, ~L797–L850 of the file).
Single forward pass over the decoded `&[(String,String)]`:

- §4.3 (pseudo before regular): the `if seen_regular { return Err(...) }` is the
  FIRST check inside the pseudo branch — any `:`-prefixed name after a regular
  field is rejected. CORRECT.
- §4.3.1 (exactly one each of the mandatory): `:method` tracked via `Option`
  (dup → reject), `:scheme`/`:path`/`:authority` via `seen_*` bools (dup →
  reject). Post-loop: `None` method → reject; non-CONNECT requires both
  `seen_scheme && seen_path`. CORRECT (exactly-one + presence both enforced).
- §4.3 prohibited/unknown: the `_ =>` arm rejects ANY `:`-name that is not one
  of the four request pseudo-headers — including the response-only `:status`
  and any unregistered `:foo`. CORRECT.
- §4.4 (CONNECT): `Some("CONNECT")` requires `:authority`, forbids
  `:scheme`/`:path`. CORRECT and RFC-faithful (kept correct even though the
  gateway does not otherwise support CONNECT).

Adversarial casing/whitespace check: the QPACK decoder preserves field names
verbatim (no normalization — confirmed by reading `decode`/`decode_qstring`),
so `:Method`, `: method`, `:STATUS` all fall to the `_` prohibited arm →
rejected (secure direction; and HTTP/3 mandates lowercase, so this is not
over-rejection). An empty name `""` has no `:` prefix → treated as a regular
field (sets `seen_regular`) — not a pseudo bypass.

**Logic gaps found:** none that are in-scope defects. NON-BLOCKING: the
validator does not value-check (`:path` non-empty, `:method` non-empty,
`:scheme` ∈ {http,https}); h3spec #12/#14/#15 do not exercise that and it is
out of this fix's scope. The `:authority`-mandatory rule is intentionally
relaxed (deviation #13). Both noted, neither blocks.

## Item 2 — R8 secure direction without a gap / over-reject: **AGREE**

(a) **Negative control is NON-VACUOUS.** Proven two ways:
  - Unit test `pseudo_valid_request_accepted_negative_control` asserts a full
    valid request AND a minimal `GET/https//` both `.is_ok()` — passes.
  - WIRE-LEVEL control (my harness, see Item 5): the production QPACK decoder +
    `validate_request_pseudo_headers` ACCEPT (no reset) both a minimal valid
    request and a valid request WITH `:authority` + a trailing regular field.
(b) **No bypass.** Casing/whitespace/empty-name analysed above; the decoder
  does not normalize, so weird-cased pseudos hit the prohibited arm. The hook
  runs at the SOLE H3 ingress (`poll_h3`, immediately after `rx.feed` returns
  the decoded headers) BEFORE `H3Request::from_headers`, the authority
  sanitiser, and ALL three upstream dial branches — a malformed request is
  reset and forwarded NOWHERE (confirmed by reading conn_actor.rs L825–L848:
  on `Err`, it logs, calls `reset_h3_stream(conn, sid, H3_MESSAGE_ERROR)`,
  removes the rx state, and `break`s out of the recv loop).
(c) **Does not reject legit traffic.** The gateway's own absent-`:authority`
  SNI-substitution flow still works end-to-end: `h3_h3_stream_e2e`
  `h3h3_e2e_absent_authority_substitutes_sni` (omit_authority=true) PASSES, and
  `round8_h3_authority_enforced::h3_valid_authority_passes_validator` PASSES.

## Item 3 — QPACK §4.5.6 fix correctness: **AGREE**

Read encoder None-branch and decoder `0xE0==0x20` branch + `encode_qint`/
`decode_qint`/`decode_qstring`.
- Encoding: `encode_qint(buf, name.len(), 3, 0x20)` then raw name bytes then a
  normal 7-bit-prefix value string. First byte `001NHxxx` with N=H=0 (raw,
  no Huffman) and a 3-bit-prefix name length. This is the RFC 9204 §4.5.6 form.
- Decoding: `decode_qint(slice, 3)` reads the name length from the SAME first
  byte's 3-bit prefix (with varint continuation), then reads `name_len` raw
  bytes, then a 7-bit-prefix value. Pair is self-consistent AND matches the
  external/h3spec encoding.
- **Conformance to a real peer (decisive):** I decoded the EXACT byte blocks
  h3spec v0.1.13 emits (HTTP3Error.hs `illegalHeader1/2`, which use `27 02`
  literal-literal-name) through the production decoder — they decode to the
  intended names (`:autority`, `foo`). The PRE-FIX decoder would have
  mis-parsed these (it skipped the first byte then read a fresh 7-bit length),
  which is precisely why #14/#15 previously failed.
- **Off-by-one / overflow / panic:** none.
  - Varint continuation guard: `decode_qint` errors once `m > 28` (max shift
    applied is `<<28`, value ≤ ~2^35, no usize overflow on 64-bit).
  - Name-length bounds: `pos.checked_add(name_len)` (no pointer overflow) +
    `buf.get(pos..end)` (→ `H3Error::Incomplete`, never a slice panic).
  - 3-byte-name length-prefix continuation (`27 02` → 9) verified correct by
    the `decode_literal_literal_name_length_continuation` test AND my harness.
- **Carry-forward (already documented in-code, not a defect):** a conformant
  peer that Huffman-encodes a literal NAME (`H=1`) is read RAW (the codec is
  raw-only) → CF-S22-QPACK-HUFFMAN. h3spec does not Huffman-encode these names,
  so it does not affect this verification.

## Item 4 — Non-vacuous regression tests: **AGREE**

`cargo test -p lb-quic --all-features --lib pseudo_` →
**8 passed, 0 failed** (7 new `pseudo_*` + 1 pre-existing substring match
`feed_body_rejects_pseudo_header_in_h3_trailers`). The 7 lock each rule with
both reject-AND-accept assertions (e.g. duplicate `:method` AND duplicate
`:path`; missing method/path/scheme each; `:status` AND unknown `:madeup`;
pseudo-after-regular; CONNECT ok/bad-has-path/bad-no-authority; the valid
negative control). Not tautological.

`cargo test -p lb-h3 --lib qpack` → **6 passed, 0 failed** (3 new). The 3 new
tests lock: (a) decode of an externally-produced conformant literal-literal-name
block, (b) the 3-bit-prefix varint continuation for a 9-byte name, (c) a
fixed-format encoder↔decoder round-trip incl. a >7-byte name. The conformant-
decode test (a) is the true interop lock — it fails on the pre-fix decoder.

## Item 5 — Live h3spec (R5): **AGREE**

Gateway: release binary @ 1409eb76 (rebuilt — no-op, already current), config
`audit/h3spec/s22-verify-e1.toml` on UDP 28443 / metrics 29090 / H1 backend
23000 (`audit/h3spec/s22-backend.py`). Managed by PID only; metrics 200-ready
in 1s; gateway + backend killed by PID at end (both confirmed gone).

`h3spec -n -t 3000` for the four match patterns (`audit/h3spec/s22-h3spec-e1.log`):

| # | h3spec case | result | gateway reject reason (from gw.log) |
|---|---|---|---|
| 12 | a pseudo-header is duplicated | **[✔]** | `h3 duplicate :method pseudo-header (§4.3.1)` |
| 13 | mandatory pseudo-header fields are absent | **[✘]** (expected) | (accepted — only `:authority` absent) |
| 14 | prohibited pseudo-header fields are present | **[✔]** | `h3 prohibited/unknown request pseudo-header (§4.3)` |
| 15 | pseudo-header fields exist after fields | **[✔]** | `h3 prohibited/unknown request pseudo-header (§4.3)` |

`4 examples, 1 failure` — the 1 failure is #13, as intended.

**Confirmation that #13 is exactly the documented `:authority` deviation, NOT
an accidentally-passing mandatory-field case:** I fetched h3spec v0.1.13's
`HTTP3Error.hs` (tag-matched) and decoded its exact wire blocks through the
PRODUCTION decoder + validator (harness output reproduced below; this is the
adversarial ground truth — h3spec only checks "a reset occurred", so I verified
WHY):

```
[#13 absent-mandatory]  decode: [:method GET, :scheme https, :path /]      -> ACCEPT (only :authority absent -> deviation -> h3spec ✘)
[#14 prohibited]        decode: [:method, :scheme, :autority, :path, :foo] -> REJECT prohibited/unknown (trips on :autority)
[#15 pseudo-after-field] decode: [:method, :scheme, :autority, foo, :path] -> REJECT prohibited/unknown (trips on :autority)
[#12 duplicate]         decode: [:method, :scheme, :authority, :path, :method] -> REJECT duplicate :method
[ctrl genuine pseudo-after-regular]  [:method, :scheme, foo, :path] -> REJECT "pseudo-header after regular field (§4.3)"
[ctrl VALID minimal]                 [:method, :scheme, :path]      -> ACCEPT
[ctrl VALID with-authority+regular]  [:method, :scheme, :authority, :path, foo] -> ACCEPT
```

NON-BLOCKING OBSERVATION on #15: h3spec's `illegalHeader2` vector contains a
bogus pseudo `:autority` (deliberately misspelled) BEFORE the late `:path`, so
the validator rejects at `:autority` (prohibited/unknown) and never reaches the
ordering check. The h3spec OUTCOME (H3_MESSAGE_ERROR reset) is still correct and
the case passes. To prove the §4.3 ordering arm is not dead code, my
independent `ctrl genuine pseudo-after-regular` control (a VALID regular field
`foo` followed by a VALID `:path`) is rejected with the correct
`"pseudo-header after regular field (§4.3)"` reason — the ordering arm IS
reachable and load-bearing, and `pseudo_15_after_regular_field_rejected` locks
it. No action required.

`H3_MESSAGE_ERROR` code: h3spec asserts `applicationProtocolErrorsIn
[H3MessageError]` and reported ✔ (not a code-mismatch) → the peer received
exactly 0x010e, matching the registered RFC 9114 §8.1 codepoint.

## Item 6 — Regression: **AGREE**

`cargo test -p lb-quic --all-features` → **197 passed, 0 failed** (summed
across all 33 binaries; matches the author's reported 197/0). Spot-checks:
`h3_h3_stream_e2e` 22/0 incl. `h3h3_e2e_absent_authority_substitutes_sni`;
`round8_h3_authority_enforced` 3/0 (valid passes, comma+whitespace rejected).
`cargo fmt --check` and `cargo clippy --all-features` on lb-quic + lb-h3 both
clean (exit 0; the only build warning is the pre-existing unrelated
`FlowEntry` dead_code).

---

## Artifacts
- `audit/h3spec/s22-verify-e1.toml` — gateway config (ports 28443/29090/23000)
- `audit/h3spec/s22-backend.py` — H1 backend
- `audit/h3spec/s22-h3spec-e1.log` — the 4-case h3spec run (12✔ 13✘ 14✔ 15✔)
- `audit/h3spec/s22-h3spec-15only.log` — isolated #15 run
- `audit/h3spec/s22-gw.log` — gateway log w/ per-case reject reasons
- Harness (throwaway, /tmp/s22harness): decoded the exact h3spec v0.1.13 wire
  blocks through the production QPACK decoder + validator (reproduced above).
