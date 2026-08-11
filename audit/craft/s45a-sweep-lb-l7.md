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
