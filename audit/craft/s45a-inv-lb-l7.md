# scanner-l7 — lb-l7 / lb-h1 / lb-h2 / lb-h3-testcodec / lb-grpc
## Summary
files_scanned=84  slop=1  ambiguous=23  load_bearing_notable=31

Method: every `.rs` file in the five crates (79) plus all 5 `Cargo.toml` read in full;
1554 comment blocks classified. Deletion yield is 1 — consistent with the standard's
calibration. Every naive "session marker" hit in this area proved to be a finding-ID
attached to regression rationale. NOTE: three integration tests in this area
`read_to_string` production source and assert on VERBATIM doc-comment text (see
LOAD-BEARING NOTABLES) — editing those comments turns CI red, in lb-l7 AND lb-io.

LEAD CORRECTION APPLIED (`deny(missing_docs)`): all five crates in this area carry
`#![deny(missing_docs)]` (lb-l7/src/lib.rs:10, lb-h1:10, lb-h2:10, lb-h3-testcodec:10,
lb-grpc:10), so a `///` or `//!` on a **public** item is gate-load-bearing and can
never be SLOP. Re-checked: this does not move my numbers. The single SLOP item is a
plain `//` comment on a private `#[cfg(test)] mod tests`, so it is untouched by the
lint. No AMBIGUOUS entry proposed a deletion — but the correction sharpens them: every
one is now explicitly **correct-in-place, never delete**, because most sit on `pub`
items where removal would also break clippy.

Also re-swept the two categories that remain in scope after the correction:
* **Commented-out CODE** — 10 regex candidates, all 10 are wrapped prose (second lines
  beginning `for`/`return`/`assert`/`struct`). **Zero** real commented-out code in the
  area. The calibration table's prediction held exactly.
* **In-body `//` restatements** — one found (the SLOP item below).

## PROPOSED DELETIONS (SLOP)
`SLOP | crates/lb-l7/src/grpc_proxy.rs:611 | // Make the `IncomingBody` type alias usable in tests without exporting it. | Vestigial: sits above `#[cfg(test)] mod tests`; `mod tests { use super::*; }` is ordinary Rust that needs no note, and `IncomingBody` is an import rename (line 39), not a type alias. Zero information.`

## AMBIGUOUS (KEEP + flag)

### A. Orphaned doc blocks — a doc comment is fused onto the WRONG function (fix = move, never delete)
All four functions involved are **private** `fn` (h1_proxy.rs:2928/:2966, h2_proxy.rs:3028/:3061),
so `deny(missing_docs)` does NOT fire on them — which is precisely why the orphaned/undocumented
state went unnoticed. No gate is red today, and moving the block back stays lint-safe.
`AMB | crates/lb-l7/src/h1_proxy.rs:2915 | /// PROTO-2-12 helper: build a `BoxBody` that emits the data bytes followed by an HT | Describes `build_body_with_trailers` but is contiguous with the block at :2921, so rustdoc attaches all of it to `h3_decoded_resp_head_builder` (:2928). `build_body_with_trailers` (:2966) is left undocumented. Needs a MOVE + blank line, not a delete.`
`AMB | crates/lb-l7/src/h2_proxy.rs:3020 | /// PROTO-2-12 helper for the H2 proxy: identical shape to `h1_proxy::build_body_wit | Same defect, same class: describes `build_h2_body_with_trailers` but fuses onto `validate_request_trailers` (:3028). `build_h2_body_with_trailers` (:3061) is undocumented.`

### B. Stale status claims contradicted by the code they annotate
`AMB | crates/lb-l7/src/sni_authority.rs:22 | //! ## Wiring status ... That main.rs handler change is **DEFERRED to Wave-2c** | REFUTED: the validator IS on the hot path — h1_proxy.rs:1051, h2_proxy.rs:1228, crates/lb/src/main.rs:3873. Reads as "this module is dead", inviting a duplicate wire-up. Correct the text; do not delete the RFC 6066/9110 rationale above it.`
`AMB | crates/lb-l7/tests/sni_authority_mismatch.rs:3 | //! Wave-2b-2 lands the validator function only. The TLS-accept-site wiring ... is | Same stale deferral, and directly contradicted by the sibling tests/sni_authority_421.rs:1 ("wired into the H1 hot path").`
`AMB | crates/lb-l7/src/ws_proxy.rs:33 | //! Per-message compression (RFC 7692) ... backend-side reuse and WebSocket-over-QUI | "WebSocket-over-QUIC (RFC 9220) are post-v1" is contradicted 450 lines lower in the same file by `dial_backend_ws` (:482), documented as "SESSION 28 / WS-over-H3 (RFC 9220) Stage C". The RFC 7692 half is still true.`

### C. References to symbols that no longer exist, stated in the present tense
(References of the form "the FORMER / the OLD `X`" are provenance and are KEEP — only these read as current.)
`AMB | crates/lb-l7/tests/trailer_passthrough.rs:13 | //! The proxy hot path (`h1_proxy::translate_h1_request_to_h2`, ... `h2_proxy::trans | Both named functions were deleted; worse, it describes capture "via `Collected::trailers()` at body-collect time", the exact whole-body collect the R8 streaming rewrite removed. Doubly misleading about a load-bearing invariant.`
`AMB | crates/lb-l7/tests/trailer_passthrough.rs:277 | /// carries a `trailers` field that `h3_response_to_h1` forwards into the `BridgeRes | `h3_response_to_h1` has 0 definitions in the tree (present tense on a removed symbol).`
`AMB | crates/lb-l7/tests/trailer_passthrough.rs:391 | /// (`collect_h{1,2}_request_to_h3_fieldlist`, `h3_response_to_h{1,2}`) now feed `tr | "now feed" — all four symbols are deleted; replaced by build_h{1,2}_to_h3_fieldlist + h{1,2}_decoded_resp_head_builder.`
`AMB | crates/lb-l7/src/h2_proxy.rs:2987 | /// ... for the H2 trailer-capture sites (`translate_h2_request_to_h2`, `collect_h2_ | Names two deleted functions as the current enforcement sites. The paragraph at :2996 does say the caller "was replaced", so the block self-corrects — worth a tidy, not a delete.`
`AMB | crates/lb-l7/src/h2_proxy.rs:4039 | /// RFC 9113 §8.1 enforcement note for the H2 trailer-capture sites (`translate_h2_r | Same dead pair, and here with NO "former" qualifier anywhere in the block.`

### D. Drifted absolute line-number cross-references (measured against the current tree)
`AMB | crates/lb-l7/src/h1_proxy.rs:1661 | /// [`crate::h2_proxy::H2Proxy::proxy_h2_to_h2_request`] (h2_proxy.rs:1964): | `proxy_h2_to_h2_request` is at h2_proxy.rs:2215. Drift ~250 lines. The symbol path is correct, so the note still navigates — the number does not.`
`AMB | crates/lb-l7/src/h1_proxy.rs:2056 | // `proxy_h2_to_h2`'s dispatch arm, h2_proxy.rs:1925-1955). The | Actual dispatch arm is h2_proxy.rs:2176-2206.`
`AMB | crates/lb-l7/src/h2_proxy.rs:2069 | // verbatim and the :1881 verdict-rx backstop is untouched. | The verdict-rx backstop is at :2095 / :2122.`
`AMB | crates/lb-l7/src/h2_proxy.rs:3161 | // the post-error verdict-rx backstop at :3003 (F-CAP-1 wedged-pump | Actual site is :3283.`
`AMB | crates/lb-l7/src/h2_proxy.rs:3276 | // F-CAP-1 (mirror of proxy_request:1822–1849): | The F-CAP-1 arm inside `proxy_request` is at :2082-2109.`
`AMB | crates/lb-l7/src/h2_proxy.rs:3302 | // Validate-before-RESPONSE-relay gate (mirror of 1857–1878): | Actual gate is :2120-2141.`
`AMB | crates/lb-l7/src/h2_proxy.rs:2597 | /// a bodyless-COMPLETE request (`h3_bridge.rs:3309-3313`), so a downstream H2 | crates/lb-quic/src/h3_bridge.rs:3309-3313 is inside a unit test, not the connector contract. Cross-crate cite — coordinate with scanner-quic before renumbering.`

### E. Comments whose stated reasoning does not match the code beneath them
`AMB | crates/lb-h2/src/security.rs:436 | // All events at tick 0 — count_in_window grows but prev_count is 0. ... estimated = | Contradicts the test (which records at `i * 20`, not tick 0) AND the implementation note at security.rs:84-86 ("`count_in_window` is taken at full weight"). Traced: first record(20) yields estimated=1000, not 0. A negative-control comment that is itself wrong.`
`AMB | crates/lb-l7/tests/round8_edge_defaults_table.rs:79 | /// Sanity-check that the table covers every numeric field of `H2SecurityThresholds` | Claims "Future field additions force a doc update because the new field will lack an assertion above" — `let _ = H2SecurityThresholds::default();` cannot force anything. The block half-concedes ("the real enforcement is ... plus code review"). Overclaims what the test proves (standard rule 7, inverted).`
`AMB | crates/lb-l7/tests/round8_xff_iteration.rs:17 | //! reaching into the helpers directly via a `pub(crate)` re-export shim inside this | Module doc + the block at :25-40 triplicate the same reasoning, end mid-thought ("... tests/ directory? — we can't reach"), and describe a "tiny wrapper that lives behind `cfg(test)`" that does not exist (the file uses a local mirror fn). DO NOT blind-delete: the doc at :45-53 is the load-bearing "this is a MIRROR, prod may diverge" caveat.`

### F. Tests whose comments justify a vacuous assertion (keep the comment; the test is the question)
`AMB | crates/lb-l7/tests/informational_responses.rs:126 | /// Confirms hyper's H1 server-side 100-continue policy is the transparent default | The test body is `let _ = "documented baseline";` — no assertion at all. The comment is the entire artifact. Either the comment is load-bearing documentation in the wrong place, or the test is dead scaffolding; a sweeper should not decide alone.`
`AMB | crates/lb-l7/tests/h2_connect_protocol_settings.rs:57 | // RFC 8441 §3 setting id is `0x8`. Pin the constant so a future commit that hand-r | The assertion is `const SETTINGS_ENABLE_CONNECT_PROTOCOL: u16 = 0x8; assert_eq!(..., 8)` — self-referential, cannot fail. The comment explains the intent (pin the spec literal), which is why it is KEEP rather than SLOP.`

### G. Provenance narrative (kept — commit hashes verified to resolve)
`AMB | crates/lb-l7/src/security_hooks.rs:3 | //! Wave-1 (commit `3dcb6f3`) added the `lb-security = { path = "../lb-security" }` | Two paragraphs of how-this-module-came-to-be. I verified 3dcb6f3 / e36b50f / 1d462c7 / ef54a9d3 / 25d8ad84 all resolve via `git cat-file`, so the provenance is live, not dangling. Flagged only because it is the closest thing to changelog prose in the area; the paragraph at :24-31 (why `NoopHooks` is NOT `#[cfg(test)]`) is a settled-decision guard and is sacred.`

## LOAD-BEARING NOTABLES (explicitly preserved)

### LINT-GATE-READ — `deny(missing_docs)` makes every `///`/`//!` on a `pub` item mandatory
`KEEP | crates/lb-{l7,h1,h2,h3-testcodec,grpc}/src/lib.rs:10 | Each crate root has `#![deny(missing_docs)]`; CI runs clippy `--all-targets --all-features -D warnings`. Every doc-comment on a `pub` item in this area — including the ones that merely re-spell a signature, e.g. the 17 `GrpcStatus` variant docs (lb-grpc/src/status.rs:12-45), the 9 `H2Error` variant docs (lb-h2/src/error.rs), and the per-field docs on `H2SecurityThresholds` (lb-l7/src/h2_security.rs:37-70) — is GATE-load-bearing. Removal fails the lint, not just review.`

### CI-GATE-READ COMMENTS — a test asserts this VERBATIM source text. Editing = red CI.
`KEEP | crates/lb-l7/src/h1_proxy.rs:1180 | "ROUND8-L7-10 — take-and-discard upstream stream pattern" is string-matched by tests/round8_body_overread.rs:87. The block encodes the Pingora 0.6.0/0.8.0 upstream-smuggling lesson (single-use H1 upstream sockets).`
`KEEP | crates/lb-l7/src/h1_proxy.rs:1202 | The literal `set_reusable(false)` is string-matched by tests/round8_body_overread.rs:94 — the doc must keep pointing a future reuse-refactor at the mitigation.`
`KEEP | crates/lb-io/src/pool.rs (OUT OF MY AREA — warn the lb-io sweeper) | "ROUND8-L7-10 — API contract for future H1 upstream reuse" is string-matched by crates/lb-l7/tests/round8_body_overread.rs:106.`
`KEEP | crates/lb-io/src/http2_pool.rs (OUT OF MY AREA — warn the lb-io sweeper) | "ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern" is string-matched by crates/lb-l7/tests/round8_body_overread.rs:122.`
`KEEP | crates/lb-l7/src/h1_proxy.rs (any `ROUND8-L7-05` occurrence) | tests/round8_underscore_policy.rs:126 asserts the marker survives; :131 asserts `with_header_underscore_policy`. Removing the L7-05 tag from the underscore-policy block lands red.`
`KEEP | crates/lb-l7/src/h2_proxy.rs (any `ROUND8-L7-05` occurrence) | tests/round8_underscore_policy.rs:142/147 — same pair on the H2 side.`
`KEEP | crates/lb-l7/src/h2_proxy.rs:841 | The exact text `if self.h2_extended_connect_enabled` and `enable_connect_protocol()` are string-matched by tests/h2_connect_protocol_settings.rs:40/50 (CF-S27-2 default-OFF gate). Reflowing this line breaks CI.`

### Sacred "use X not Y because Y does Z" / library-behaviour notes
`KEEP | crates/lb-h2/src/hpack.rs:145 | DynamicTable uses `VecDeque` "instead of the O(n) `Vec::insert(0, ...)` it replaced" — canonical use-X-not-Y.`
`KEEP | crates/lb-h1/src/chunked.rs:312 | F-PARSE-3 (S38): the 16-hex-digit cap is the REAL overflow defense; `checked_shl(4)` is INERT belt-and-braces. Explicitly warns "do not rely on it as the overflow guard; keep the digit cap".`
`KEEP | crates/lb-l7/src/h1_proxy.rs:2723 | "HeaderMap::remove removes ALL values for the name, not just one." — stops a future loop-to-remove-duplicates regression.`
`KEEP | crates/lb-l7/src/h1_proxy.rs:1444 | F-MD-4 H1: `frame()==None` IS the positively-confirmed clean end for `Kind::Chan`, and "do NOT consult `Body::is_end_stream()`" — explicitly the MIRROR-IMAGE of the H2 rule 600 lines away. Deleting this makes the two paths look copy-pasteable.`
`KEEP | crates/lb-l7/src/h2_proxy.rs:1947 | The exact inverse: H2 `frame()==None` is AMBIGUOUS (hyper maps RST_STREAM/CANCEL to None), cites hyper-1.9.0 body/incoming.rs ~L250 and h2-0.4.13 state.rs. This is the request-smuggling guard.`
`KEEP | crates/lb-l7/src/ws_proxy.rs:131 | F-S27-2 scope note: the `max_write_buffer_size` bound is hardening, NOT the full fix; states the two tungstenite 0.24 invariants the value must satisfy and that WS-over-H2 is still unbounded.`

### Panic-freedom / invariant notes
`KEEP | crates/lb-l7/src/h2_proxy.rs:1657 | `// SAFETY: guarded by `is_data()`.` — invariant note on `into_data().unwrap_or_default()`.`
`KEEP | crates/lb-l7/src/h2_proxy.rs:3297 | `// SAFETY: every non-head branch above `return`ed` — justifies the `let Some(resp) = resp else` unreachability.`
`KEEP | crates/lb-grpc/src/frame.rs:52 | `// SAFETY of .get(): we checked len >= GRPC_HEADER_SIZE above.``
`KEEP | crates/lb-l7/src/h2_proxy.rs:432 | CleanCloseIo poll_shutdown `Poll::Pending` arm: "`linger_deadline` is always `Some` here ...; if it were ever absent we still must not resolve early, so yield." — encodes why the None arm cannot take the fast path.`

### RFC / CVE conformance rationale
`KEEP | crates/lb-h1/src/parse.rs:189 | ROUND8-L7-03 / HAProxy CVE-2023-25725 / nginx CVE-2019-9516: header names are raw-byte tchar-validated and deliberately NOT `.trim()`ed (RFC 9112 §5.1).`
`KEEP | crates/lb-h1/src/chunked.rs:204 | The trailer-parser mirror of the same rule — the note is what keeps the two sites in sync.`
`KEEP | crates/lb-h2/src/frame.rs:215 | ROUND8-L7-11 / HAProxy "Properly consume padding for DATA frames": layered PADDED+PRIORITY strip order, with the exact RFC 9113 §6.2 wire layout.`
`KEEP | crates/lb-h3-testcodec/src/qpack.rs:345 | SESSION 22 / h3spec #14/#15: RFC 9204 §4.5.6 literal-literal-name — the NAME length is the first byte's 3-bit prefix, NOT a separate length byte, plus the CF-S22-QPACK-HUFFMAN carry-forward.`
`KEEP | crates/lb-l7/src/h2_proxy.rs:235 | F-SEC-1 CleanCloseIo: ~70 lines of proven mechanism (hyper-1.9.0 + h2-0.4.13 source trace, RFC 1122 §4.2.2.13 / Linux tcp_close RST-on-unread-data). The FIN-first-then-drain ordering is non-obvious and irreproducible from the code.`
`KEEP | crates/lb-l7/src/h2_to_h1.rs:68 | SEC-2-01: why `check_h2_downgrade` runs on the REGULAR headers only — running it on the raw inbound list over-fires because pseudo-headers are `:`-prefixed by design.`
`KEEP | crates/lb-l7/src/h1_proxy.rs:62 | HOP_BY_HOP set: PROTO-2-08 records that `"trailers"` was removed IN ERROR-CORRECTION (not a header name at all) and `keep-alive` added. Prevents re-adding either.`
`KEEP | crates/lb-l7/src/h1_proxy.rs:2386 | ROUND8-L7-01 (Pingora GHSA-xq2h-p299-vjwv / Envoy GHSA-rj35-4m94-77jh, CVSS 9.3): the defer-101 ordering plus the documented behaviour change (one extra RTT, 502/504 instead of 101-then-silent-close).`

### Non-vacuous / negative-control test comments
`KEEP | crates/lb-l7/src/ws_proxy.rs:786 | F-S27-2 duplex-plateau proof: states exactly what flips it RED ("Reverting the bound in `tungstenite_config` flips this RED") and why the e2e tests cannot isolate it.`
`KEEP | crates/lb-l7/src/ws_proxy.rs:876 | R10 close_backpressure proof: spells out the determinism argument (200 ms read_frame vs 30 s idle envelope) so a later "simplification" of the timings cannot silently make it assert the wrong Close code.`
`KEEP | crates/lb-l7/tests/round8_authority_enforced.rs:39 | The closed-port backend is the mechanism: "A 400 therefore proves the authority validator ran BEFORE upstream selection". Plus the probe-backend connection counter at :250 (zero-dial proof for the three bypass forks).`
`KEEP | crates/lb-l7/tests/s38_h1_header_timeout.rs:10 | "NEGATIVE CONTROL: ... FAILS pre-fix ...; PASSES post-fix" — the 1 s header vs 10 s total split is the whole proof.`
`KEEP | crates/lb-l7/tests/round8_xff_iteration.rs:45 | The `append_xff_test_mirror` caveat: this test uses a MIRROR of production, and names `h1_proxy::tests` as the real source of truth if they diverge. Without it the test silently looks like it covers production.`

## Per-file load-bearing counts
`crates/lb-l7/src/authority.rs : 6`
`crates/lb-l7/src/grpc_proxy.rs : 62`
`crates/lb-l7/src/h1_proxy.rs : 227`
`crates/lb-l7/src/h1_to_h1.rs : 9`
`crates/lb-l7/src/h1_to_h2.rs : 10`
`crates/lb-l7/src/h1_to_h3.rs : 10`
`crates/lb-l7/src/h2_proxy.rs : 261`
`crates/lb-l7/src/h2_security.rs : 19`
`crates/lb-l7/src/h2_to_h1.rs : 13`
`crates/lb-l7/src/h2_to_h2.rs : 4`
`crates/lb-l7/src/h2_to_h3.rs : 4`
`crates/lb-l7/src/h3_to_h1.rs : 8`
`crates/lb-l7/src/h3_to_h2.rs : 4`
`crates/lb-l7/src/h3_to_h3.rs : 4`
`crates/lb-l7/src/lib.rs : 36`
`crates/lb-l7/src/security_hooks.rs : 7`
`crates/lb-l7/src/sni_authority.rs : 10`
`crates/lb-l7/src/stripped_request.rs : 9`
`crates/lb-l7/src/trace_ctx.rs : 23`
`crates/lb-l7/src/upstream.rs : 18`
`crates/lb-l7/src/ws_proxy.rs : 66`
`crates/lb-l7/tests/bridging_h1_h1.rs : 1`
`crates/lb-l7/tests/bridging_h1_h2.rs : 4`
`crates/lb-l7/tests/bridging_h1_h3.rs : 4`
`crates/lb-l7/tests/bridging_h2_h1.rs : 5`
`crates/lb-l7/tests/bridging_h2_h2.rs : 3`
`crates/lb-l7/tests/bridging_h2_h3.rs : 2`
`crates/lb-l7/tests/bridging_h3_h1.rs : 5`
`crates/lb-l7/tests/bridging_h3_h2.rs : 3`
`crates/lb-l7/tests/bridging_h3_h3.rs : 2`
`crates/lb-l7/tests/h2_authority_host_mismatch.rs : 6`
`crates/lb-l7/tests/h2_connect_protocol_settings.rs : 4`
`crates/lb-l7/tests/h2_to_h1_pseudo_strip.rs : 5`
`crates/lb-l7/tests/hop_by_hop_set.rs : 8`
`crates/lb-l7/tests/informational_responses.rs : 8`
`crates/lb-l7/tests/round8_authority_enforced.rs : 17`
`crates/lb-l7/tests/round8_body_overread.rs : 3`
`crates/lb-l7/tests/round8_edge_defaults_table.rs : 9`
`crates/lb-l7/tests/round8_glitches_enforced.rs : 12`
`crates/lb-l7/tests/round8_keepalive_count_cap.rs : 10`
`crates/lb-l7/tests/round8_traceparent_propagation.rs : 5`
`crates/lb-l7/tests/round8_underscore_policy.rs : 7`
`crates/lb-l7/tests/round8_ws_upgrade_defer.rs : 7`
`crates/lb-l7/tests/round8_xff_iteration.rs : 4`
`crates/lb-l7/tests/s38_h1_header_timeout.rs : 11`
`crates/lb-l7/tests/smuggle_matrix.rs : 16`
`crates/lb-l7/tests/smuggle_wired.rs : 6`
`crates/lb-l7/tests/sni_authority_421.rs : 16`
`crates/lb-l7/tests/sni_authority_mismatch.rs : 5`
`crates/lb-l7/tests/stripped_request_newtype.rs : 7`
`crates/lb-l7/tests/trailer_passthrough.rs : 14`
`crates/lb-h1/src/chunked.rs : 32`
`crates/lb-h1/src/error.rs : 13`
`crates/lb-h1/src/lib.rs : 1`
`crates/lb-h1/src/parse.rs : 19`
`crates/lb-h1/tests/proptest_parser.rs : 10`
`crates/lb-h1/tests/round8_chunk_size_cve_corpus.rs : 18`
`crates/lb-h1/tests/round8_header_name_rfc9110.rs : 5`
`crates/lb-h2/src/error.rs : 23`
`crates/lb-h2/src/frame.rs : 74`
`crates/lb-h2/src/hpack.rs : 90`
`crates/lb-h2/src/lib.rs : 1`
`crates/lb-h2/src/security.rs : 53`
`crates/lb-h2/tests/proptest_hpack.rs : 6`
`crates/lb-h2/tests/round8_h2_cve_corpus.rs : 4`
`crates/lb-h2/tests/round8_padded_frame.rs : 9`
`crates/lb-h3-testcodec/src/error.rs : 13`
`crates/lb-h3-testcodec/src/frame.rs : 28`
`crates/lb-h3-testcodec/src/lib.rs : 1`
`crates/lb-h3-testcodec/src/qpack.rs : 27`
`crates/lb-h3-testcodec/src/security.rs : 4`
`crates/lb-h3-testcodec/src/varint.rs : 4`
`crates/lb-h3-testcodec/tests/proptest_qpack.rs : 5`
`crates/lb-grpc/src/deadline.rs : 7`
`crates/lb-grpc/src/error.rs : 9`
`crates/lb-grpc/src/frame.rs : 10`
`crates/lb-grpc/src/lib.rs : 16`
`crates/lb-grpc/src/status.rs : 23`
`crates/lb-grpc/src/streaming.rs : 6`

TOTAL load-bearing (blocks 1554 − 1 slop − 23 ambiguous) = **1530**
