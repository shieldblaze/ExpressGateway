# S45A sweep report — `crates/lb-quic/**`

## Headline

| metric (brief's regex `^\s*(//\|/\*\|\*)`) | value |
|---|---|
| comment lines BEFORE (baseline `602126ec`) | **9,669** |
| comment lines AFTER | **4,842** |
| reduction | **49.9 %** |

With the tightened regex that excludes deref lines like `*total = …` (44 such
lines, counted as "comments" by the brief's pattern in both directions):
9,625 → 4,798 = **50.1 %**.

Every commit was verified with a mechanical guard: the non-comment, non-blank
lines of each file are byte-identical to `602126ec`.

## Mandatory proofs

**Code identity** — `python3 audit/craft/s45a-code-identity.py main`:

```
S45A code-identity proof — 231 .rs files changed vs main
  2 file(s) with real code changes — each needs justification:
    CODE DIFFERS   crates/lb-observability/src/xdp_metrics.rs   <- not my area
    CODE DIFFERS   crates/lb-quic/src/h3_bridge.rs              <- lead-ACCEPTED
```

The only lb-quic entry is the `map_err` closure the lead accepted (a block
expression returning a value vs the value; a rustfmt artifact of removing the
comment inside the closure).

A THIRD entry, `crates/lb-quic/tests/h3_connection_recycle_e2e.rs`, was listed
before this pass and has been RESTORED rather than justified. My second
mechanical sweep deleted `// transient — retry next iteration`, which was the
sole content of an otherwise-empty match arm, and rustfmt then collapsed
`=> { }` to `=> {}`. I put the comment back, so that file's code is now
byte-identical to main and `cargo fmt --check` stays clean. Cost: one comment
line (the AFTER total below already includes it).

**Attribute lines** — `git diff main -- crates/lb-quic | grep -E '^[-+]\s*#\['`
returns EMPTY: zero attribute lines added or removed. A stronger full census over
all 46 changed files confirms it — 478 attribute lines on main, 478 now, and every
distinct attribute line occurs the same number of times:

```
46 changed .rs files; distinct attribute lines: main=76 now=76
total attribute lines: main=478 now=478
  every attribute line occurs the SAME number of times as on main
```

This is structural, not luck: the applier REFUSES to run if any targeted line —
or any replacement line — is not a comment or blank, and `#[...]` matches
neither. The two mechanical sweeps only ever delete a 1-line plain `//` block.

One apparent discrepancy worth recording: a raw `grep -c '#\[test\]'` shows
93 on main vs 92 now. Both main-side hits are INSIDE the F-COR-8 prose in
`tests/h3_graceful_close.rs`, which mentions `#[test]` twice; my compression
merged them into one mention. That file has no `#[test]` ATTRIBUTE at all — its
tests are `#[tokio::test]` (87 on main, 87 now) and its function count is
unchanged at 7.

**F-S29-1 canary** — `grep -c 'or_insert_with()' crates/lb-quic/src/conn_actor.rs`
= **1**. Surviving text at `conn_actor.rs:416-425`:

> F-S29-1 (gRPC-over-H3 large-response trailer drop): the spawn site inserts the
> `Progressive` StreamTx alongside the receiver, but `drain_streams_to_conn`'s
> `retain` REMOVES it the instant the stream goes terminal, and a stale receiver
> can outlive it. Use `get_mut`, NOT `entry().or_insert_with()`: a fresh StreamTx
> would replay the leftover `End`, fire a spurious FIN + RESET, and
> `stream_shutdown` would DISCARD a large response's still-buffered trailer+FIN
> (small responses raced clear; large ones silently lost the trailing
> `grpc-status` HEADERS — gRPC-fatal). A missing StreamTx means the stream
> already terminated correctly: drop the stale receiver, skip.

`cargo fmt -p lb-quic -- --check` is CLEAN.

## Per-file

| file | before | after | cut |
|---|---:|---:|---:|
| `src/cleanup_guard.rs` | 34 | 20 | 41% |
| `src/conn_actor.rs` | 812 | 375 | 54% |
| `src/h3_bridge.rs` | 1435 | 481 | 66% |
| `src/h3_config.rs` | 105 | 48 | 54% |
| `src/lib.rs` | 175 | 52 | 70% |
| `src/listener.rs` | 228 | 88 | 61% |
| `src/passthrough.rs` | 543 | 301 | 45% |
| `src/public_header.rs` | 154 | 99 | 36% |
| `src/raw_proxy.rs` | 1054 | 474 | 55% |
| `src/router.rs` | 258 | 114 | 56% |
| `src/terminate_loopback.rs` | 175 | 99 | 43% |
| `src/udp_dataplane.rs` | 122 | 78 | 36% |
| `src/ws_tunnel.rs` | 218 | 129 | 41% |
| `examples/passthrough_linkage_probe.rs` | 39 | 15 | 62% |
| `tests/grpc_h3_e2e.rs` | 214 | 143 | 33% |
| `tests/h3_connection_recycle_e2e.rs` | 129 | 88 | 32% |
| `tests/h3_graceful_close.rs` | 72 | 43 | 40% |
| `tests/h3_h1_bridge_e2e.rs` | 63 | 33 | 48% |
| `tests/h3_h1_resp_stream_e2e.rs` | 453 | 243 | 46% |
| `tests/h3_h1_stream_body_e2e.rs` | 129 | 58 | 55% |
| `tests/h3_h1_stream_body_errors_e2e.rs` | 120 | 68 | 43% |
| `tests/h3_h1_trailers_resp_e2e.rs` | 72 | 51 | 29% |
| `tests/h3_h2_stream_e2e.rs` | 264 | 150 | 43% |
| `tests/h3_h3_stream_e2e.rs` | 682 | 334 | 51% |
| `tests/listener_lifecycle.rs` | 71 | 56 | 21% |
| `tests/passthrough_retry_differential.rs` | 43 | 24 | 44% |
| `tests/proptest_header.rs` | 22 | 10 | 55% |
| `tests/public_header_differential.rs` | 45 | 30 | 33% |
| `tests/quic_router_leak.rs` | 47 | 31 | 34% |
| `tests/round8_h3_authority_enforced.rs` | 69 | 46 | 33% |
| `tests/router_accept_path.rs` | 76 | 43 | 43% |
| `tests/s16_b1_two_connections.rs` | 103 | 62 | 40% |
| `tests/s16_b2_backpressure.rs` | 199 | 101 | 49% |
| `tests/s16_b2_multistream.rs` | 100 | 53 | 47% |
| `tests/s16_b2_reset_not_fin.rs` | 96 | 61 | 36% |
| `tests/s16_b2_stream_relay_smoke.rs` | 114 | 64 | 44% |
| `tests/s16_b3_reset_propagation_smoke.rs` | 116 | 65 | 44% |
| `tests/s16_b3_reset_propagation_verify.rs` | 171 | 103 | 40% |
| `tests/s16_raw_proxy_smoke.rs` | 47 | 36 | 23% |
| `tests/s19_b4_datagram_relay_smoke.rs` | 73 | 41 | 44% |
| `tests/s19_b4_datagram_verify.rs` | 143 | 84 | 41% |
| `tests/s19_b5_stream_flood.rs` | 101 | 55 | 46% |
| `tests/s19_b5_verify.rs` | 129 | 70 | 46% |
| `tests/s19_b6_metrics_nonvacuous.rs` | 90 | 64 | 29% |
| `tests/s19_b6_two_connections.rs` | 102 | 51 | 50% |
| `tests/s19_b6_zero_rtt_rejection.rs` | 118 | 63 | 47% |
| **TOTAL** | **9625** | **4797** | **50.2%** |

(The per-file table uses the tightened regex; the two totals differ by the 44
deref lines noted above.)

## What was cut

* **Module headers.** 30 of 46 files led with a 15–70-line `//!` essay
  (`raw_proxy.rs` 70, `h3_bridge.rs` 17, `passthrough.rs` 31, plus a 20–65-line
  header on nearly every test). Each is now 4–28 lines carrying only the
  mechanism and the load-bearing assertions.
* **Doc-block prose.** The dominant move, as the standard predicted: 95 → 21
  (`stream_request_to_h3_upstream`), 55 → 14 (`validate_request_pseudo_headers`),
  41 → 17 (`MAX_RELAY_STREAMS`), 39 → 12 (`stream_h2_response`), 36 → 7
  (`h2_request_body_from_rx`), 34 → 14 (`stream_h1_response`).
* **Session/changelog narrative.** `SESSION n / INC-m:` / `S36-A —` /
  `PROTO-2-12` prefixes and "pre-S12 this did X, post-fix it does Y" blocks
  deleted wholesale where the current code is self-evident; the finding ID was
  kept only where it is the label of a surviving catch.
* **Navigation markers.** 280 single-line markers across the tests —
  `// ─────`, `// 1) Real echo backend.`, `// Tidy up.`, `// Handshake.`,
  `// ----- cert + sockets -----` — removed by two mechanical sweeps that touch
  ONLY 1-line plain `//` blocks (never `///`, never `//!`, never multi-line), so
  no doc comment or rationale block could be caught by them.
* **Stale prose that was actively wrong.** The `MAX_RESPONSE_BODY_BYTES` doc
  claimed `read_h1_response` "reads the whole upstream response to EOF into one
  `Vec` (FULLY BUFFERED)" — contradicting the R8 design the rest of the file
  documents; the `StreamTx` doc described "Two variants … `Buffered` is the
  LEGACY shape" for a one-variant enum; `h3_config.rs` called itself
  "infrastructure only … deletes nothing and changes no live path" while being
  the live H3 config path; `listener.rs` said the WS "frame relay is wired in a
  later stage" 15 lines above `with_ws_relay_launcher`. All four rewritten to
  the truth rather than deleted.

## Catches deliberately preserved

Named canaries, verified present after the sweep:

* `conn_actor.rs` — the F-S29-1 note. The literal `entry().or_insert_with()`
  survives (1 occurrence), as does "use `get_mut` … a fresh StreamTx would
  replay the leftover `End` … `stream_shutdown` would DISCARD a large response's
  still-buffered trailer+FIN (gRPC-fatal)". 14 lines → 10.
* `passthrough.rs` — the `#[allow(deprecated)]` TOOLCHAIN SHIM on `fetch_update`
  (nightly deprecates it, `try_update` does not exist on MSRV 1.88, the
  fuzz-smoke lane builds nightly with `-D warnings`). Kept, 8 → 6 lines.
* `raw_proxy.rs` — `Shutdown::Write ⇒ RESET_STREAM`, `Shutdown::Read ⇒
  STOP_SENDING`, still flagged COUNTERINTUITIVE with "swapping the arms silently
  emits the wrong frame".
* `raw_proxy.rs` — CF-S16-RELAY-STALL: quiche has COLLECTED the stream after the
  source FIN, so a re-issued `stream_recv` returns `InvalidStreamState` and the
  generic arm would DROP the pending tail + FIN; the `!half.src_fin_seen` gate is
  the fix.
* `conn_actor.rs` + `h3_bridge.rs` — both F-MD-4 guards (quiche's FIRST
  `finished_streams` pop lacks the reset re-check its SECOND pop performs; the
  zero-length `stream_recv` probe).
* `h3_bridge.rs` — CF-QUICHE-FRAME-COMPLETENESS (quiche does not enforce
  RFC 9114 §7.1 at FIN) + the content-length under-run guard + the documented
  residual no-content-length gap, in both the source and `h3_h3_stream_e2e.rs`
  CASE 15 including the "RE-TIGHTEN to `!(200 && fin)`" instruction.
* `h3_bridge.rs` — the DELIBERATE request-leg `H3_REQUEST_CANCELLED` vs
  response-leg `H3_INTERNAL_ERROR` asymmetry, "do not fix to a false consistency".
* `h3_bridge.rs` — `RESPONSE_HOP_BY_HOP` is a deliberate cross-crate duplicate
  (reverse-layering ban), "keep the two in sync", and the strip is REQUIRED by
  RFC 9114 §4.2.
* `h3_bridge.rs` — request trailers INTENTIONALLY dropped on the H3→H1 leg
  (smuggling vector), and the mid-body Reset returning before the terminator.
* `passthrough.rs` — the FlowEntry "holds no key material" SAFETY/INVARIANT
  block, the `_flow_entry_field_audit` compile-error mechanism, and why it is
  unconditional rather than `cfg(debug_assertions)` (S34 release-build breakage).
* `passthrough.rs` / `listener.rs` — F-INFRA-01 retry-secret perm gate on both
  loaders, still flagged as deliberate duplicates to keep in sync.
* `conn_actor.rs` — `goaway_pending` vs `goaway_sent` must stay separate or the
  admit-past-boundary window re-opens.
* `public_header.rs` — RFC 9001 §5.4 (every field read is wire-cleartext), the
  VersionNegotiation-folded-onto-Retry arm (the crate denies `unreachable!`), and
  the RFC 9001 §A.2-vs-§A.3 fixture provenance.
* `raw_proxy.rs` — `MAX_RELAY_STREAMS` as defense-in-depth INDEPENDENT of the
  quiche `max_streams` grant, with the 128 MiB arithmetic the unit test pins.
* `ws_tunnel.rs` — R8 bounded-by-construction (`PollSender` parks, does not
  buffer — the property WS-over-H2 lacked) + the RFC 9220 close-vs-reset mapping.
* Tests — every NEGATIVE-CONTROL and mutation recipe: the `stream_finished()`
  witness trap (3 suites), F-S20-1 `full_send=false`, the drop-newest control,
  the `retain`-removed and cap-removed controls, both "NOTE FOR THE VERIFIER"
  mutation recipes in `h3_h3_stream_e2e.rs`, and the `test-gauges` FEATURE GATE
  note in `h3_h1_resp_stream_e2e.rs` (a CI gate omitting the flag silently drops
  the only non-vacuous memory assertions).

## Approved deletion, executed

`tests/h3_h1_resp_stream_e2e.rs:31-40` — the false "SCAFFOLD STATUS … are
`#[ignore]`d" paragraph. Gone (`grep -c SCAFFOLD` = 0). The accurate status and
the FEATURE GATE note that lived at :1115-1136 survive, compressed to 10 lines.

## What I refused to cut, and why the number is 50 % and not 78 %

The standard estimates ≈78 % when clause 2 (catches survive) is honoured. This
area lands at 50 %. Two structural reasons, both measurable:

1. **`#![deny(missing_docs)]` floor.** `src/` has 130 `pub fn/const/static/
   struct/enum/trait/type/mod` items plus every `pub` struct field and `pub` enum
   variant, each of which must keep ≥1 doc line. 1,395 of the 4,797 remaining
   lines are `///` in `src/`, and a large share of those are already
   one-liners that cannot go lower.
2. **Catch density.** This is the crate the standard's own §"Catches that
   specifically survive" list points at: my Phase-0 inventory catalogued 30
   load-bearing notables here, and the crate carries the F-MD-4 guards, the
   quiche §7.1 gap, the two smuggling-parity legs, the Mode A no-decrypt
   invariant, and every reset-vs-FIN arm note. Compressing those to one line each
   is what I did; deleting them is what I would have had to do to reach 78 %.

Specific items I left larger than one line on purpose: the F-S29-1 note (10
lines — the canary requires the literal string plus the mechanism), the
`stream_request_to_h3_upstream` contract (21 — five distinct invariants), the
quiche §7.1 gap + threat model (16 — it carries the re-tighten trigger), the
`MAX_RELAY_STREAMS` rationale (17 — the arithmetic is pinned by a test), and
`propagate_cancel`'s arm mapping (13 — swapping the arms is silent).

I also deliberately did NOT apply the Phase-0 inventory's "AMB items must be KEPT
as-is by the sweeper" instruction where it conflicted with the owner-revised
standard: those entries are load-bearing prose that has gone factually stale
(deleted symbol names, drifted line numbers, refuted status claims). Compressing
them removed the stale claim while keeping the fact — the four rewrites listed
above are the notable ones. Drifted `file.rs:NNNN` line citations were dropped
rather than re-pointed; re-deriving ~40 correct line numbers is an accuracy pass,
not a sweep, and a wrong pointer is worse than none.

## Method

Every edit went through a spec of `(start, end, replacement)` line ranges applied
by a script with a hard guard: it REFUSES to run if any line in any range, or any
line of any replacement, is not a comment or blank. That guard caught a genuine
off-by-one on my first attempt at `h3_bridge.rs` (it would have deleted three
`RespAbort` enum variants); I reverted, rebuilt the ranges from a mechanically
generated block map, and re-applied. Each commit was then checked with
`diff <(git show HEAD:f | grep -v comment) <(grep -v comment f)`.
