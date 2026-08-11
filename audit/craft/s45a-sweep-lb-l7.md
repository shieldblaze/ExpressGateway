# S45A sweep — `sweeper-l7` (lb-l7 / lb-h1 / lb-h2 / lb-h3-testcodec / lb-grpc)

## Headline

| | lines |
|---|---|
| baseline (lead-supplied) | 6,269 |
| measured baseline @ `ff39fa08` | 6,269 |
| after | **3,721** |
| removed | 2,548 (**40.6%**) |

Measurement: `grep -rhE '^\s*(//|/\*|\*)' crates/lb-l7 crates/lb-h1 crates/lb-h2 crates/lb-grpc crates/lb-h3-testcodec --include='*.rs' | wc -l`

Commits (branch `feature/de-slop-s45a`):

- `b9032d5d` h2_proxy.rs
- `e6798ebc` h1_proxy.rs
- `d5310833` rest of lb-l7/src
- `4bf9f8ef` lb-l7/tests
- `8b235f3a` lb-h1 / lb-h2 / lb-h3-testcodec / lb-grpc

## Mandatory self-check — lead-required proof

### 1. `s45a-code-identity.py main` — no lb-l7-area file listed

```
$ python3 audit/craft/s45a-code-identity.py main
S45A code-identity proof — 228 .rs files changed vs main
  2 file(s) with real code changes — each needs justification:
    CODE DIFFERS   crates/lb-observability/src/xdp_metrics.rs
    CODE DIFFERS   crates/lb-quic/src/h3_bridge.rs
```

Neither file is in my area (lb-l7 / lb-h1 / lb-h2 / lb-grpc / lb-h3-testcodec).
Independently re-run across all 53 of my changed `.rs` files: **53 checked, 0 with
code differences.**

### 2. No attribute line removed

```
$ git diff main -- crates/lb-l7 crates/lb-h1 crates/lb-h2 crates/lb-grpc \
      crates/lb-h3-testcodec | grep -E '^-\s*#\['
(no output)
```

Per-file attribute counts (`^\s*#!?\[`) compared main vs HEAD across all 53 changed
files: **identical in every file.** No `#[inline]`, `#[allow]`, `#[cfg]`,
`#[derive]`, `#[must_use]`, `#[test]`, `#[tokio::test]` or `#![...]` line was lost.

### 3. `test-gauges` gated statics intact with attributes

```
crates/lb-l7/src/h1_proxy.rs
2677:#[cfg(any(test, feature = "test-gauges"))]
2678-pub static H1_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
2683:#[cfg(any(test, feature = "test-gauges"))]
2684-pub fn record_retained_h1(n: usize) {

crates/lb-l7/src/h2_proxy.rs
2846:#[cfg(any(test, feature = "test-gauges"))]
2847-pub static H2_REQ_MAX_RETAINED_BODY_BYTES: std::sync::atomic::AtomicUsize =
2852:#[cfg(any(test, feature = "test-gauges"))]
2853-pub fn record_retained(n: usize) {
```

Occurrences of that exact gate string — `h1_proxy.rs` main=6 head=6,
`h2_proxy.rs` main=7 head=7. The root `tests/` crate importers
(`tests/h1h1_md_streaming_verify.rs:386,446,471`, `tests/h2h1_md_coverage_driver.rs`)
still resolve.

### 4. Test-asserted source strings — assertion site and grep proof

`crates/lb-l7/tests/round8_body_overread.rs` reads `crates/lb-l7/src/h1_proxy.rs`:

```
:69   src.contains("ROUND8-L7-10 — take-and-discard upstream stream pattern"),
:76   src.contains("set_reusable(false)"),

$ grep -cF 'ROUND8-L7-10 — take-and-discard upstream stream pattern' crates/lb-l7/src/h1_proxy.rs
1
$ grep -cF 'set_reusable(false)'                                     crates/lb-l7/src/h1_proxy.rs
1
```

`crates/lb-l7/tests/h2_connect_protocol_settings.rs` reads `crates/lb-l7/src/h2_proxy.rs`:

```
:26   src.contains("enable_connect_protocol()"),
:35   src.contains("if self.h2_extended_connect_enabled"),

$ grep -cF 'enable_connect_protocol()'           crates/lb-l7/src/h2_proxy.rs
1
$ grep -cF 'if self.h2_extended_connect_enabled' crates/lb-l7/src/h2_proxy.rs
2
```

`crates/lb-l7/tests/round8_underscore_policy.rs` reads BOTH proxies:

```
:101 / :117   src.contains("ROUND8-L7-05"),
:106 / :122   src.contains("with_header_underscore_policy"),

$ grep -cF 'ROUND8-L7-05'                  crates/lb-l7/src/h1_proxy.rs   -> 5
$ grep -cF 'with_header_underscore_policy' crates/lb-l7/src/h1_proxy.rs   -> 1
$ grep -cF 'ROUND8-L7-05'                  crates/lb-l7/src/h2_proxy.rs   -> 4
$ grep -cF 'with_header_underscore_policy' crates/lb-l7/src/h2_proxy.rs   -> 1
```

The assertion sites themselves are untouched: `git diff main` shows **0** changed
`assert` / `src.contains` / `let src` lines in all three test files.

Cross-check for the lb-io sweeper — `round8_body_overread.rs` also asserts two
strings outside my area, both still present:
`'ROUND8-L7-10 — API contract for future H1 upstream reuse'` in `lb-io/src/pool.rs` (1),
`'ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern'` in `lb-io/src/http2_pool.rs` (1).

## No-behaviour-change proof

Every edited file was verified byte-identical to its pre-edit version after
stripping comment lines:

```
git show <base>:$f | grep -vE '^\s*(//|/\*|\*)' > a
grep -vE '^\s*(//|/\*|\*)' $f          > b
diff a b       # empty for all 54 files
```

Four accidental code-line deletions were caught by this check *before* commit and
restored (`glitches_threshold` field, `ProxyErr::BadRequest`, a
`smuggle_matrix.rs` assert pair, a `ws_proxy.rs` observer binding). One further
attempt on `lb-grpc/src/lib.rs` interleaved code with comments; that file was
reverted and left untouched.

`cargo fmt -p lb-l7 -p lb-h1 -p lb-h2 -p lb-grpc -p lb-h3-testcodec` run; its only
effect was reindenting three comment lines in `lb-l7/tests/sni_authority_421.rs`.

## Test-asserted source strings — grep proof

Three integration tests `read_to_string` production source and assert on verbatim
text. All strings survive; the assertion sites themselves were not edited.

```
$ grep -cF 'ROUND8-L7-10 — take-and-discard upstream stream pattern' crates/lb-l7/src/h1_proxy.rs
1
$ grep -cF 'set_reusable(false)'                        crates/lb-l7/src/h1_proxy.rs
1
$ grep -cF 'ROUND8-L7-05'                               crates/lb-l7/src/h1_proxy.rs
5
$ grep -cF 'with_header_underscore_policy'              crates/lb-l7/src/h1_proxy.rs
1
$ grep -cF 'enable_connect_protocol()'                  crates/lb-l7/src/h2_proxy.rs
1
$ grep -cF 'if self.h2_extended_connect_enabled'        crates/lb-l7/src/h2_proxy.rs
2
$ grep -cF 'ROUND8-L7-05'                               crates/lb-l7/src/h2_proxy.rs
4
$ grep -cF 'with_header_underscore_policy'              crates/lb-l7/src/h2_proxy.rs
1
```

`#![deny(missing_docs)]`: every `pub` item in all five crates retains at least one
doc line. No `pub` doc block was reduced to zero; the floor is what caps this
area's percentage (see *Why 40.6%* below).

## Per-file table

| file | before | after | % |
|---|---|---|---|
| lb-l7/src/h2_proxy.rs | 1505 | 722 | 52 |
| lb-l7/src/h1_proxy.rs | 1304 | 724 | 44 |
| lb-l7/src/ws_proxy.rs | 363 | 207 | 43 |
| lb-l7/src/grpc_proxy.rs | 220 | 129 | 41 |
| lb-h2/src/security.rs | 183 | 143 | 22 |
| lb-h2/src/hpack.rs | 118 | 53 | 55 |
| lb-h2/src/frame.rs | 115 | 107 | 7 |
| lb-l7/tests/trailer_passthrough.rs | 112 | 46 | 59 |
| lb-l7/src/trace_ctx.rs | 97 | 60 | 38 |
| lb-l7/tests/informational_responses.rs | 95 | 30 | 68 |
| lb-l7/src/stripped_request.rs | 91 | 54 | 41 |
| lb-h3-testcodec/src/qpack.rs | 90 | 79 | 12 |
| lb-l7/src/sni_authority.rs | 89 | 38 | 57 |
| lb-h1/src/chunked.rs | 82 | 74 | 10 |
| lb-l7/src/lib.rs | 80 | 56 | 30 |
| lb-l7/src/h2_security.rs | 78 | 48 | 38 |
| lb-l7/tests/round8_authority_enforced.rs | 77 | 45 | 42 |
| lb-l7/src/security_hooks.rs | 75 | 38 | 49 |
| lb-l7/src/h2_to_h1.rs | 70 | 45 | 36 |
| lb-l7/src/authority.rs | 70 | 39 | 44 |
| lb-l7/src/upstream.rs | 59 | 32 | 46 |
| lb-l7/tests/sni_authority_421.rs | 56 | 31 | 45 |
| lb-l7/tests/round8_glitches_enforced.rs | 55 | 30 | 45 |
| lb-l7/tests/round8_underscore_policy.rs | 52 | 27 | 48 |
| lb-l7/tests/round8_xff_iteration.rs | 52 | 20 | 62 |
| lb-l7/tests/smuggle_wired.rs | 47 | 23 | 51 |
| lb-l7/tests/smuggle_matrix.rs | 42 | 31 | 26 |
| lb-l7/tests/h2_connect_protocol_settings.rs | 38 | 19 | 50 |
| lb-l7/tests/stripped_request_newtype.rs | 38 | 18 | 53 |
| lb-l7/tests/round8_ws_upgrade_defer.rs | 37 | 23 | 38 |
| lb-grpc/src/deadline.rs | 33 | 24 | 27 |
| lb-l7/tests/round8_body_overread.rs | 32 | 14 | 56 |
| lb-h3-testcodec/tests/proptest_qpack.rs | 31 | 23 | 26 |
| lb-l7/tests/round8_keepalive_count_cap.rs | 29 | 19 | 34 |
| lb-l7/tests/hop_by_hop_set.rs | 29 | 13 | 55 |
| lb-l7/tests/round8_edge_defaults_table.rs | 28 | 19 | 32 |
| lb-l7/tests/s38_h1_header_timeout.rs | 27 | 21 | 22 |
| lb-h3-testcodec/src/varint.rs | 24 | 17 | 29 |
| lb-l7/tests/h2_to_h1_pseudo_strip.rs | 23 | 14 | 39 |
| lb-h2/tests/proptest_hpack.rs | 20 | 13 | 35 |
| lb-l7/tests/h2_authority_host_mismatch.rs | 20 | 16 | 20 |
| lb-l7/tests/sni_authority_mismatch.rs | 17 | 8 | 53 |
| lb-l7/tests/round8_traceparent_propagation.rs | 17 | 11 | 35 |
| lb-h2/tests/round8_padded_frame.rs | 16 | 14 | 12 |
| lb-l7/tests/bridging_*.rs (9 files) | 29 | 0 | 100 |

Files not edited: the nine `lb-l7/src/h{1,2,3}_to_h{1,2,3}.rs` bridges, `lb-h1/src/parse.rs`,
`lb-h1/src/error.rs`, `lb-h1/src/lib.rs`, the `lb-h1` tests, `lb-h2/src/error.rs`,
`lb-h2/src/lib.rs`, `lb-h2/tests/round8_h2_cve_corpus.rs`, `lb-h3-testcodec/src/{frame,security,error,lib}.rs`,
`lb-grpc/src/{frame,status,streaming,error,lib}.rs`. These are already at or near
the floor: almost every line is a mandatory single-line `///` on a `pub` item
(9 `H2Error` variants, 17 `GrpcStatus` variants, ~40 `H2Frame`/`H3Frame` field
docs) or a one-line RFC citation.

## Approved Phase-0 edits — all applied

1. **DELETED** `lb-l7/src/grpc_proxy.rs:611` — `// Make the IncomingBody type alias
   usable in tests without exporting it.` Vestigial: `IncomingBody` is an import
   rename, not an alias, and `mod tests { use super::*; }` needs no note.
2. **MOVED (not deleted)** both orphaned doc blocks:
   - `h1_proxy.rs` — the `PROTO-2-12` block that described `build_body_with_trailers`
     but was fused onto `h3_decoded_resp_head_builder`. Compressed and reattached to
     `build_body_with_trailers`; `h3_decoded_resp_head_builder` got its own compressed doc.
   - `h2_proxy.rs` — same defect: the block describing `build_h2_body_with_trailers`
     was fused onto `validate_request_trailers`. Same fix.
3. **CORRECTED** `lb-l7/src/sni_authority.rs:22` — the `## Wiring status … DEFERRED
   to Wave-2c` block was REFUTED (the validator runs at `h1_proxy.rs`, `h2_proxy.rs`
   and the binary's TLS-accept site). Block deleted; the RFC 6066 / RFC 9110 §15.5.20
   rationale kept compressed, with one line stating the validator IS wired.

## Other stale claims corrected in place

- `lb-l7/src/ws_proxy.rs` module doc no longer says "WebSocket-over-QUIC (RFC 9220)
  are post-v1" — contradicted 450 lines below by `dial_backend_ws`.
- `lb-l7/tests/sni_authority_mismatch.rs` no longer claims the TLS-accept wiring is
  deferred to Wave-2c — contradicted by its sibling `sni_authority_421.rs`.
- `lb-l7/tests/round8_edge_defaults_table.rs:79` no longer claims
  `let _ = H2SecurityThresholds::default();` "forces a doc update" on a new field.
  It cannot; the doc now says the real enforcement is the sibling test plus review.
- `lb-h2/src/security.rs:436` — the negative-control comment claimed "all events at
  tick 0 … estimated = 0", contradicting both the test (it records at `i * 20`) and
  the implementation note 350 lines above. Replaced with the actual reason ticks are
  spread across the window.
- **Six drifted absolute line-number cross-references dropped** while compressing
  (`h1_proxy.rs:1661`, `:2056`; `h2_proxy.rs:2069`, `:3161`, `:3276`, `:3302`). The
  symbol paths that still navigate correctly were kept; only the stale numbers went.
  The cross-crate `h3_bridge.rs:3309-3313` cite (which pointed into a unit test, not
  the connector contract) was dropped rather than renumbered — no coordination with
  `sweeper-quic` needed.
- Names of deleted functions (`translate_h2_request_to_h2`,
  `collect_h2_request_to_h3_fieldlist`, `h3_response_to_h1`,
  `translate_h1_request_to_h2`) removed from four blocks that stated them in the
  present tense.

## Catches preserved (compressed, fact intact)

Smuggling / framing:
- **F-MD-4 H2 rule** (`h2_proxy.rs`) — `frame()==None` is AMBIGUOUS; hyper maps
  RST_STREAM(CANCEL/NO_ERROR) to `Ready(None)`; `is_end_stream()` is the
  deterministic discriminator. hyper-1.9.0 `body/incoming.rs ~L250` and
  h2-0.4.13 `proto/streams/state.rs is_recv_end_stream` citations kept. 23 → 15 lines.
- **F-MD-4 H1 mirror-image rule** (`h1_proxy.rs`) — for `Kind::Chan`, `None` IS the
  confirmed clean end; a premature close arrives as `Some(Err)` (`IncompleteBody`);
  do NOT consult `is_end_stream()` (`content_length == ZERO` is unreliable for
  chunked). `decode.rs ~L162` / `~L504` citations kept. 34 → 23 lines.
- `inject_abort!` FIFO ordering (hold the sender so hyper polls the injected `Err`
  before the channel-close `None`) and its "this is only HALF the fix" pairing with
  the caller's detached send task + `reset_peer`. 26 → 16 lines.
- `drive_h2_upstream_send` DETACH root cause (a downstream RST cancels the service
  future, dropping the body at a clean frame boundary → hyper finalizes END_STREAM →
  truncated request relayed as COMPLETE). 23 → 12 lines.
- F-MD-1 http1 mis-framing (HTTP/2-versioned parts + stale CL make hyper send an
  empty body and never poll the `StreamBody`). 21 → 8 lines.
- H2→H3 HAZARD (a): the connector treats a `body_tx` dropped without an explicit
  terminal as a bodyless-COMPLETE request, so the pump is DETACHED and always emits
  `End`/`Reset`.
- ROUND8-L7-11 PADDED strip order + the HAProxy "Properly consume padding" primitive.
- SEC-2-01 `check_h2_downgrade` must run on the REGULAR headers only, or it over-fires
  on legitimate pseudo-headers.
- ROUND8-L7-09 choke-point placement + the three forks that previously bypassed it,
  and the explicit "NO loopback exemption here" ruling.

Resource / liveness:
- **F-SEC-1 `CleanCloseIo`** — the flagship 73-line block → 17. The mechanism stays:
  h2 drops the io a microsecond after `poll_shutdown` returns `Ready`; dropping with
  unread inbound makes Linux RST (RFC 1122 §4.2.2.13 / `tcp_close`); the peer then
  discards its whole receive buffer including the GOAWAY. Fix = FIN first, then a
  drain hard-bounded by `DRAIN_CAP` AND `LINGER_DEADLINE`. The `linger_deadline`-is-
  always-`Some` yield note kept.
- F-CAP-1 verdict-over-send precedence (returning 502 would mask a real 413/400 and
  create a 413-vs-502 race) at all three sites.
- F-MD-2 drain-and-validate: a receiver-drop must NOT become a 413.
- F-MD-3 gauge honesty: the old sites recorded a CONSTANT, so a buffering regression
  would not have moved the gauge.
- F-S27-2 `max_write_buffer_size` SCOPE note ("hardening, not the full fix; does NOT
  bound the WS-over-H2 tunnel") plus both tungstenite 0.24 invariants the value must
  satisfy. 35 → 19 lines.
- F-RES-1: a `Timer` must be wired or `header_read_timeout` is INERT.
- F-PARSE-3: the 16-hex-digit cap IS the overflow defense; `checked_shl(4)` is INERT
  belt-and-braces — do not rely on it.
- PROTO-2-16: `graceful_shutdown` cannot retro-fit `Connection: close` onto a flushed head.
- WS-001 / WS-002 ping-rate-limit and per-direction read-frame watchdog rationale.

Spec / provenance:
- ROUND8-L7-10 Pingora single-use rationale + the refactor warning (both pinned strings).
- ROUND8-L7-01 defer-101 (Pingora GHSA-xq2h-p299-vjwv / Envoy GHSA-rj35-4m94-77jh).
- ROUND8-L7-04 XFF/Via multi-line (Envoy GHSA-ghc4-35x6-crw5).
- PROTO-2-08 `trailers`-was-added-in-error / `keep-alive`-was-missing correction.
- SESSION 22 / h3spec #14/#15 RFC 9204 §4.5.6 literal-literal-name fix on encoder AND
  decoder + the CF-S22-QPACK-HUFFMAN carry-forward.
- RFC 8441 §4 `:scheme` reachability measurement (hyper does NOT enforce it; that arm
  is load-bearing, the `:path` arm is defense-in-depth).
- CF-S27-2 default-OFF rationale for WS-over-H2.
- All `// SAFETY:` notes verbatim; all `#[allow]` justifications; `biased;` orderings;
  the nginx CVE-2013-2028 / hyper GHSA-5h46-h7hh-c6x9 / HAProxy chunk-size citations.
- Negative-control intent reduced to one line each, never removed
  (`s38_h1_header_timeout.rs`, the F-S27-2 duplex plateau "reverting the bound flips
  this RED", the R10 `close_backpressure` determinism argument, the closed-port and
  zero-dial-probe mechanism proofs).

## Refused to cut

- The `lb-l7/tests/round8_xff_iteration.rs:45` MIRROR caveat ("if production diverges,
  `h1_proxy::tests` is the source of truth"). Without it the test silently looks like
  it covers production. The surrounding mid-sentence ramble was cut; the caveat stayed.
- `lb-h2/src/hpack.rs:145` `VecDeque` "instead of the O(n) `Vec::insert(0, ...)` it
  replaced" — canonical use-X-not-Y.
- `h1_proxy.rs` `// HeaderMap::remove removes ALL values for the name, not just one.`
  — kept verbatim; it stops a loop-to-remove-duplicates regression.
- The `lb-h2/src/h2_security.rs` attack→knob mapping. The standard lists ASCII tables
  for deletion, but this table IS the content (a reader cannot otherwise know
  `max_pending_accept_reset_streams` is the CVE-2023-44487 knob). Converted from a
  9-row table to a 6-line prose list rather than dropped: 27 → 13.
- `lb-l7/tests/informational_responses.rs` — the inventory flagged
  `informational_responses.rs:126` as a test whose body is `let _ = "documented
  baseline";`, i.e. the comment is the entire artifact. Per the brief I did not decide
  whether the test is dead scaffolding; the prose was compressed 48 → 13 in the module
  doc and the tests left in place. **Open question for the lead.**
- `lb-grpc/src/lib.rs` — an edit there interleaved with code, so the file was reverted
  and left at 11 comment lines rather than risk a silent code deletion.

## Why 40.6% and not 90%

The standard's own measured ceiling is 86.9% *"compress every doc block to 1 line +
delete EVERY plain comment"* and ≈78% honouring clause 2. This area lands lower for
two structural reasons, both measurable:

1. **`#![deny(missing_docs)]` floor is unusually high here.** These five crates are
   type-definition-dense: 17 `GrpcStatus` variants, 9 `H2Error` variants, ~40
   `H2Frame`/`H3Frame` field docs, 9 `H2SecurityThresholds` fields, 4 `StreamingMode`
   variants. Each is already a mandatory one-line `///` that cannot go to zero. Roughly
   900 of the surviving 3,721 lines are at this floor.
2. **Catch density.** `h1_proxy.rs` + `h2_proxy.rs` are 1,446 of the 3,721 surviving
   lines and carry the F-MD-4 smuggling rules, F-SEC-1, F-CAP-1, F-MD-1/2/3, the
   ROUND8-L7-01/04/05/09/10 lessons and the PROTO-2-01/07/16/18 rulings — clause-2
   catches with library-source citations that a reader cannot reconstruct from the
   code. They were compressed hard (52% and 44%) but not deleted.

I report the real number rather than hitting a target by deleting catches or by
dropping `pub` doc lines the lint requires.

## Gates NOT run

Per the brief I did not run `cargo build/clippy/test` (2-core box, four parallel
sweepers). The lead gates centrally. Highest-risk items for that gate, in order:

1. `#![deny(missing_docs)]` across the five crates — I preserved at least one doc line
   on every `pub` item by construction, but clippy is the only real proof.
2. The three source-string-asserting tests — grep-proven above.
3. Nothing else: no code line differs from HEAD in any edited file.

## Push status

Commits are local. `git push` was rejected (remote advanced), and `git pull --rebase`
refuses because other sweepers have uncommitted work in the shared tree — `git stash`
is barred by R9. Retried at the end of each slice; the lead may need to rebase these
five commits, or I can push once the tree is momentarily clean.

---

# ROUND 2

## Headline

| | lines |
|---|---|
| baseline @ `main` (pre-S45A) | 6,269 |
| after ROUND 1 | 3,721 (40.6%) |
| **after ROUND 2** | **1,979** |
| removed vs `main` | **4,290 (68.4%)** |
| removed in round 2 alone | 1,742 (46.8% of what round 1 left) |

Measurement (identical to round 1):

```
grep -rhE '^\s*(//|/\*|\*)' crates/lb-l7 crates/lb-h1 crates/lb-h2 crates/lb-grpc \
     crates/lb-h3-testcodec --include='*.rs' | wc -l
```

Per crate after round 2: lb-l7 1,423 · lb-h2 226 · lb-h1 146 · lb-h3-testcodec 111 · lb-grpc 73.

Round-2 commits on `feature/de-slop-s45a` (all pushed):

- `17f3ff6c` h1_proxy.rs 724 → 383
- `a9d88405` h2_proxy.rs 722 → 424
- `7a909bd8` rest of lb-l7/src (ws/grpc proxies, 9 bridges, trace_ctx, security helpers)
- `052c0a3e` lb-l7/tests to near-zero
- `60faaec2` lb-h1 / lb-h2 / lb-grpc / lb-h3-testcodec
- `75830f2e` final trim (private-field docs) + `cargo fmt`

## Method

Every edit was applied by an exact-literal `(old, new)` replacement script that (a) asserts
each `old` occurs EXACTLY once, and (b) refuses to write unless every non-blank,
non-comment line is byte-identical before and after. No file was ever hand-retyped, so the
round-1 class of accidental code deletion could not recur. 660 replacements across 61 files.

## Mandatory self-check

### 1. `s45a-code-identity.py main` — no file of mine is listed

```
$ python3 audit/craft/s45a-code-identity.py main
S45A code-identity proof — 254 .rs files changed vs main
  5 file(s) differ: TOKENS DIFFER = real code change; REFLOW ONLY = rustfmt layout, behaviour-neutral
    TOKENS DIFFER  crates/lb-observability/src/xdp_metrics.rs
    TOKENS DIFFER  crates/lb-quic/src/h3_bridge.rs
    TOKENS DIFFER  crates/lb-quic/tests/grpc_h3_e2e.rs
    TOKENS DIFFER  crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs
    TOKENS DIFFER  crates/lb-quic/tests/h3_h3_stream_e2e.rs
```

All five belong to `sweeper-quic` / `sweeper-infra`. **Zero files from lb-l7 / lb-h1 /
lb-h2 / lb-grpc / lb-h3-testcodec.**

**Caught and fixed before the final commit:** `cargo fmt` DID move code in three of my
files, because in each case the comment I deleted was the ONLY content of its block, so
rustfmt then collapsed the block:

| file | what collapsed |
|---|---|
| `lb-l7/src/ws_proxy.rs` | `tokio::spawn(async move { … })` re-wrapped, gaining a trailing comma |
| `lb-l7/src/h2_to_h1.rs` | `_ if k.starts_with(':') => { }` → `=> {}` |
| `lb-l7/src/h3_to_h1.rs` | same |

Restoring a one-line comment inside each block returns the token stream to main's exactly
and keeps the tree `cargo fmt --check`-clean. All three are legitimate keeps under the
rule anyway: an empty match arm and a bare `spawn` body do not explain themselves.

### 2. No attribute line removed

```
$ git diff main -- crates/lb-l7 crates/lb-h1 crates/lb-h2 crates/lb-grpc \
      crates/lb-h3-testcodec | grep -cE '^-\s*#!?\['
0
```

### 3. `test-gauges` gated items intact

```
crates/lb-l7/src/h1_proxy.rs:2328  pub static H1_REQ_MAX_RETAINED_BODY_BYTES: …
crates/lb-l7/src/h1_proxy.rs:2333  pub fn record_retained_h1(n: usize)
crates/lb-l7/src/h2_proxy.rs:2562  pub static H2_REQ_MAX_RETAINED_BODY_BYTES: …
crates/lb-l7/src/h2_proxy.rs:2567  pub fn record_retained(n: usize)
```

`#[cfg(any(test, feature = "test-gauges"))]` occurrence counts unchanged from main:
`h1_proxy.rs` 6, `h2_proxy.rs` 7.

### 4. Test-asserted source strings — grep proof

```
$ grep -cF 'ROUND8-L7-10 — take-and-discard upstream stream pattern' crates/lb-l7/src/h1_proxy.rs
1
$ grep -cF 'set_reusable(false)'                        crates/lb-l7/src/h1_proxy.rs
1
$ grep -cF 'ROUND8-L7-05'                               crates/lb-l7/src/h1_proxy.rs
4
$ grep -cF 'with_header_underscore_policy'              crates/lb-l7/src/h1_proxy.rs
1
$ grep -cF 'enable_connect_protocol()'                  crates/lb-l7/src/h2_proxy.rs
1
$ grep -cF 'if self.h2_extended_connect_enabled'        crates/lb-l7/src/h2_proxy.rs
2
$ grep -cF 'ROUND8-L7-05'                               crates/lb-l7/src/h2_proxy.rs
4
$ grep -cF 'with_header_underscore_policy'              crates/lb-l7/src/h2_proxy.rs
1
```

`ROUND8-L7-05` went 5 → 4 in `h1_proxy.rs` (one private-field doc dropped); the tests
assert presence, not a count. The assertion sites themselves are untouched:

```
$ git diff main -- crates/lb-l7/tests/round8_underscore_policy.rs \
      crates/lb-l7/tests/round8_body_overread.rs \
      crates/lb-l7/tests/h2_connect_protocol_settings.rs \
  | grep -cE '^[-+]\s*(assert|src\.contains|let src|\})'
0
```

Cross-crate strings `round8_body_overread.rs` also asserts, both outside my area and both
still present: `ROUND8-L7-10 — API contract for future H1 upstream reuse` in
`lb-io/src/pool.rs` (1), `ROUND8-L7-10 — H2 cousin of the H1 take-and-discard pattern` in
`lb-io/src/http2_pool.rs` (1).

### 5. `#![deny(missing_docs)]`

Mechanically re-checked every `pub` item in all five crates for an immediately preceding
`///`: the only 19 without one are the `pub mod X;` lines in `lb-l7/src/lib.rs`, which are
documented by the `//!` inner doc of the module file — unchanged from main.

`clippy::pedantic` is `allow`-ed in all five crate roots, so `missing_errors_doc` /
`missing_panics_doc` do NOT bind; that is what made the `# Errors` compression safe.

## Per-file table (vs `main`, not vs round 1)

| file | main | now | % |
|---|---|---|---|
| lb-l7/src/h2_proxy.rs | 1505 | 409 | 72 |
| lb-l7/src/h1_proxy.rs | 1304 | 368 | 71 |
| lb-l7/src/ws_proxy.rs | 363 | 103 | 71 |
| lb-l7/src/grpc_proxy.rs | 220 | 71 | 67 |
| lb-h2/src/security.rs | 183 | 76 | 58 |
| lb-h2/src/hpack.rs | 118 | 29 | 75 |
| lb-h2/src/frame.rs | 115 | 83 | 27 |
| lb-l7/tests/trailer_passthrough.rs | 112 | 13 | 88 |
| lb-l7/src/trace_ctx.rs | 97 | 33 | 65 |
| lb-l7/src/stripped_request.rs | 91 | 38 | 58 |
| lb-h3-testcodec/src/qpack.rs | 90 | 40 | 55 |
| lb-l7/src/sni_authority.rs | 89 | 22 | 75 |
| lb-h1/src/parse.rs | 88 | 41 | 53 |
| lb-h1/src/chunked.rs | 82 | 50 | 39 |
| lb-l7/src/lib.rs | 80 | 44 | 45 |
| lb-l7/src/h2_security.rs | 78 | 31 | 60 |
| lb-l7/tests/round8_authority_enforced.rs | 77 | 18 | 76 |
| lb-l7/src/security_hooks.rs | 75 | 22 | 70 |
| lb-l7/src/h2_to_h1.rs | 70 | 26 | 62 |
| lb-l7/src/authority.rs | 70 | 27 | 61 |
| lb-l7/src/upstream.rs | 59 | 21 | 64 |
| lb-l7/tests/informational_responses.rs | 95 | 7 | 93 |
| lb-h1/tests/round8_chunk_size_cve_corpus.rs | 52 | 25 | 51 |
| lb-h3-testcodec/src/frame.rs | 48 | 33 | 31 |
| lb-h1/tests/proptest_parser.rs | 33 | 8 | 75 |
| lb-grpc/src/frame.rs | 35 | 17 | 51 |
| lb-grpc/src/status.rs | 34 | 26 | 23 |
| the eight pass-through `h*_to_h*.rs` bridges | 86 | 34 | 60 |

## What round 2 did that round 1 did not

1. **Every multi-line `///` on a `pub` item collapsed to one line** unless it carries a
   catch. The `# Errors` sections went from a blank-line-separated paragraph to a single
   line, or vanished where they only restated the return type.
2. **Private struct fields have NO doc floor.** Round 1 kept one-line `///` on every
   private field of `H1Proxy`, `H2Proxy`, `ProxyService`, `CleanCloseIo`,
   `GlitchConnState`. Those are gone except where the field's existence is non-obvious
   (`conn_seq` — "combined with the peer IP so two NAT-egress connections stay distinct";
   `linger_deadline`; `close_signal`; `max_keepalive_requests`;
   `keepalive_cap_terminations` — "an atomic, not a metric handle: lb-l7 has no
   metrics-registry dep").
3. **Section banners deleted** — `// ── PROTO-001 cross-protocol translation helpers ──`,
   `// ── PROTO-001 H2-side translation helpers ──`, `// ─── Integer coding ───`,
   `// ── helpers ──`, `// ── F-SEC-1 deterministic gate (D3) ──`,
   `// ── F-COR-1 (b) unit regression ──`, `// ── SESSION 22 … ──`,
   `// Frame type constants`, `// Flag constants`, `// ── header names ──`.
4. **`tests/*.rs` cut to near-zero.** Module docs went from 8–15-line essays to 2–4 lines
   carrying the finding ID plus the pinned claim; in-body narration went entirely.
   Negative controls kept ONE line each, and every one now says the word "negative
   control" or "control" so the intent is greppable.
5. **The two flagship smuggling blocks were re-compressed a second time.** The H1 F-MD-4
   `Kind::Chan` rule went 26 → 10 lines and the H2 F-MD-4 `is_end_stream` rule 23 → 11,
   both retaining the library-source citations (`hyper-1.9.0 body/incoming.rs ~L250`,
   `h2-0.4.13 proto/streams/state.rs is_recv_end_stream`, `decode.rs ~L162 / ~L504`).

## Refused to cut

- **`h1_proxy.rs` / `h2_proxy.rs` F-MD-4 rules.** The two are exact inverses of each other
  (`None` is a confirmed clean end on H1, AMBIGUOUS on H2) and the file-local comment is
  the only place that fact exists. Deleting either invites a "unify these two identical
  loops" refactor that reintroduces a request-smuggling bug. ~21 lines survive between
  them, with the library-source citations that make the claim checkable.
- **`h1_proxy.rs` `// HeaderMap::remove removes ALL values for the name, not just one.`**
  — kept verbatim for the third round. It stops a loop-to-remove-duplicates regression.
- **`h2_proxy.rs` `CleanCloseIo` F-SEC-1 mechanism** (12 lines). The RFC 1122 §4.2.2.13 /
  `tcp_close` fact — that DROPPING a socket with unread inbound emits an RST which makes
  the peer discard its whole receive buffer including the GOAWAY — is not derivable from
  the code, and it is the entire reason the type exists.
- **`inject_abort!` "this is only HALF the fix"** in `h2_proxy.rs`. Without the pairing to
  the caller's detached send task + `reset_peer`, a future reader will move the injection
  and reopen the intermittent smuggle.
- **`h1_proxy.rs` ROUND8-L7-10 refactor warning** — pinned by `round8_body_overread.rs`
  AND load-bearing (it names the guard a pooling refactor must implement first).
- **`chunked.rs` F-PARSE-3** — "the 16-digit cap IS the overflow defense; `checked_shl(4)`
  is INERT belt-and-braces; do NOT rely on it". Deleting it invites removing the cap.
- **`qpack.rs` SESSION 22 §4.5.6 notes** on encoder AND decoder, plus the
  CF-S22-QPACK-HUFFMAN carry-forward. Both halves must stay because the fix is only
  correct as a pair.
- **`round8_xff_iteration.rs` MIRROR caveat** — round 1 refused it; so does round 2. Without
  it the test looks like it covers production when it does not.
- **`h2_security.rs` attack→knob mapping** — a reader cannot otherwise know
  `max_pending_accept_reset_streams` is the CVE-2023-44487 knob.
- **`stripped_request.rs` `compile_fail` doctests.** The brief says to delete `# Examples`,
  but these are executable tests: removing them removes coverage, which the standard
  forbids. The prose around them was cut instead.
- **`h2_proxy.rs` `#[allow(clippy::too_many_arguments)]` justification** and the
  `// pub(crate) so …` notes on `H2_ABORT_OBSERVE_TIMEOUT` / `ProxyErr` — an unexplained
  `allow` or visibility widening is a catch lost.

## Why 1,979 and not ~630 (90%)

Two structural floors, both measurable:

1. **`#![deny(missing_docs)]`.** ~700 of the 1,979 surviving lines are a mandatory
   single-line `///` on a `pub` item that cannot go to zero: 17 `GrpcStatus` variants,
   9 `H2Error` variants, 8 `H1Error` variants, ~40 `H2Frame`/`H3Frame` fields, 9
   `H2SecurityThresholds` fields, 4 `StreamingMode` variants, 7 `WsConfig` fields, the
   `AltSvcConfig`/`HttpTimeouts` pub fields, and every `pub fn` builder. This area is the
   most type-definition-dense in the repo.
2. **Catch density in the two proxies.** `h1_proxy.rs` + `h2_proxy.rs` are 777 of the
   1,979 (39%) and carry F-MD-1/2/3/4, F-SEC-1, F-CAP-1, F-RES-1, F-S27-1/2, the
   ROUND8-L7-01/04/05/06/07/09/10/11 lessons and the PROTO-2-01/07/11/16/18/19 rulings —
   clause-2 catches with library-source citations a reader cannot reconstruct.

The standard's own ceiling is 86.9% for "compress every doc block to 1 line + delete EVERY
plain comment" and ≈78% honouring clause 2. At 68.4% this area is now within ~10 points of
the honest ceiling, up from 40.6%. Closing the rest requires either dropping
`#![deny(missing_docs)]` or deleting the smuggling catches; I did neither.

## Gates NOT run

Per the brief I did not run `cargo build/clippy/test`. `cargo fmt -p lb-l7 -p lb-h1
-p lb-h2 -p lb-grpc -p lb-h3-testcodec -- --check` **passes clean** on the committed tree.
Highest-risk items for the lead's central gate, in order:

1. `#![deny(missing_docs)]` — preserved by construction and mechanically re-checked, but
   only rustc proves it.
2. The three source-string-asserting tests — grep-proven above.
3. Nothing else: the code-identity script shows zero token changes in my area.
