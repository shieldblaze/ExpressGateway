### ROUND8-L7-14 — Fuzz / proptest harness does not seed empty-name, sign-prefix, oversize-hex (recurrent CVE class)

Reference: `audit/round-8/research/hyper-h2-quinn.md` lessons 1, 2, 3 (hyper's GHSA-f3pg-qwvg-p99c CL `+`, GHSA-5h46-h7hh-c6x9 chunk overflow, GHSA-6hfq-h8hq-87mf TE folding); `audit/round-8/research/nginx.md` lesson 1; `audit/round-8/research/haproxy.md` lessons 1, 3. **All three big proxies + the Rust ecosystem paid for the same three primitives.** Our `crates/lb-h1/tests/proptest_parser.rs` does not seed any of them.
Our equivalent: `crates/lb-h1/tests/proptest_parser.rs:18-60` (proptest harness for H1 parser — only checks no-panic + bounded-consumption); `crates/lb-h2/tests/proptest_hpack.rs`, `crates/lb-h3/tests/proptest_qpack.rs`, `crates/lb-quic/tests/proptest_header.rs`

Severity: low
Status:   Proposed-Fix(div-l7, cherry-pick d7fe37ec) — Accepted-with-caveat(verifier=verify)   <!-- INDEPENDENT VERIFICATION (verifier=verify, author=div-l7): PARTIAL. Only round8_h2_cve_corpus.rs (53 lines, PADDED class, 3/3 PASS) delivered. Plan's round8_h1_cve_corpus.rs, proptest_parser rejects-ill-formed assertions, h3 seeds, l4-xdp scaffold, PROPTEST_CASES CI gate NOT delivered. H1 CVE classes de-facto covered by sibling fixes L7-02 (round8_chunk_size_cve_corpus) + L7-03 (round8_header_name_rfc9110). Low-severity meta-finding; cross-parser/CI consolidation largely undelivered. Stays Proposed-Fix. See audit/round-8/verify/l7.md. -->

Divergence:
- **Reference**: hyper, nginx, HAProxy, quinn all maintain regression seeds for *every* parser CVE they paid for. CI replays the seeds before each release. The proptest grammar covers them by construction.
- **Us**: `proptest_parser.rs` checks "doesn't panic" and "doesn't claim to consume more than fed bytes". It does NOT assert "rejects ill-formed". Specifically:
  - No seed for `:value\r\n` → would catch ROUND8-L7-03.
  - No seed for `Content-Length: +5` → would catch the lb-h1 + behaviour.
  - No seed for chunk-size = `+5\r\n` → would catch ROUND8-L7-02.
  - No seed for chunk-size = `0000000000000005\r\n` (long-zero-pad) → ditto.
  - No grammar for malformed PADDED H2 frames → would catch ROUND8-L7-11.

Impact:
- This is the meta-finding: our parser quality assurance is "doesn't crash" rather than "rejects exactly the CVE-class inputs the references have paid for". The four prior findings (L7-02, L7-03, L7-04, L7-11) are *each* a CVE class that a properly-seeded proptest would have surfaced.
- Industry standard: every parser CVE that closes carries with it a regression seed.

Reproduction:
- `cat crates/lb-h1/tests/proptest_parser.rs | grep -E '":|"\+|empty|overflow|padded'` returns nothing — none of those literal seed shapes exist.

Recommendation:
1. Add a `tests/h1_cve_corpus.rs` (or extend `proptest_parser.rs`) with explicit named regression cases:
   - `empty_header_name_rejected`: input `b":value\r\n\r\n"` must `Err`.
   - `content_length_plus_sign_rejected`: `b"Content-Length: +5\r\n\r\n"` parses headers OK, but a *separate* CL validator (currently absent — see ROUND8-L7-02) must reject `+5`.
   - `chunk_size_plus_rejected`: chunked feed `b"+5\r\n" + payload` must `Err`.
   - `chunk_size_overlong_zero_pad_rejected`: chunked feed `b"00000000000000ff\r\n"` (16+ zeros) — opt for accept (no overflow) but cap is enforced.
2. Add `tests/h2_cve_corpus.rs` for `lb-h2::frame::decode_frame` with PADDED bit combinations.
3. Add proptest *assertions* not just "no panic" — `prop_assume!` the input is supposed to be valid (per a defined grammar) and assert `Ok`; for invalid inputs assert `Err`.
4. CI gate: `PROPTEST_CASES=200000` on the explicit corpus tests should pass *and* the corpus tests should fail on any of the four CVE-class shapes above (currently they would not exist to fail).
