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
   variant, each of which must keep ≥1 doc line. 1,395 of the 4,798 remaining
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

---

## ROUND 2

### Headline

| metric (brief's regex `^\s*(//\|/\*\|\*)`) | value |
|---|---|
| baseline `main` | **9,669** |
| after round 1 | **4,842** |
| after round 2 | **3,028** |
| round-2 cut | **37.5 %** |
| cumulative cut vs `main` | **68.7 %** |

With the tightened regex that excludes the 44 `*`-continuation CODE lines the brief's
pattern misreads as comments (`* lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH`, `*g = …`):
**9,623 → 2,981 = 69.0 %**.

I did **not** reach the 2,500 target. The real number is 3,028; §"Why 3,028 and not 2,500"
below shows the arithmetic of what standing between here and there actually is.

### Mandatory proofs

**Code identity** — `python3 audit/craft/s45a-code-identity.py main`:

```
S45A code-identity proof — 254 .rs files changed vs main
  5 file(s) differ: TOKENS DIFFER = real code change; REFLOW ONLY = rustfmt layout, behaviour-neutral
    TOKENS DIFFER  crates/lb-observability/src/xdp_metrics.rs   <- not my area
    TOKENS DIFFER  crates/lb-quic/src/h3_bridge.rs              <- round-1 lead-ACCEPTED map_err
    TOKENS DIFFER  crates/lb-quic/tests/grpc_h3_e2e.rs          <- NEW, see below
    TOKENS DIFFER  crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs<- NEW, see below
    TOKENS DIFFER  crates/lb-quic/tests/h3_h3_stream_e2e.rs     <- NEW, see below
```

`h3_bridge.rs` is byte-identical (stripped) to its ROUND-1 state — i.e. round 2 added
nothing to it. That was not true at first: I had deleted `// Known-required but not
actionable here.`, the sole content of the `":scheme" => { }` match arm, and rustfmt then
collapsed it to `=> {}` — the exact class of mistake round 1 hit in
`h3_connection_recycle_e2e.rs`. I put the comment back rather than justify it. Verified:

```
round-2 baseline: 0685a11f
  COMMA/LAYOUT ONLY    crates/lb-quic/tests/grpc_h3_e2e.rs
  COMMA/LAYOUT ONLY    crates/lb-quic/tests/h3_h1_resp_stream_e2e.rs
  COMMA/LAYOUT ONLY    crates/lb-quic/tests/h3_h3_stream_e2e.rs
(every other lb-quic file: stripped source IDENTICAL to the round-1 state)
```

**The three NEW entries are rustfmt canonicalisation, not deleted code.** Token streams are
identical once rustfmt's inserted trailing comma is discounted:

```
grpc_h3_e2e.rs:          tokens(main)=11220 tokens(now)=11220 IDENTICAL_IGNORING_COMMAS=True
h3_h1_resp_stream_e2e.rs:tokens(main)=13300 tokens(now)=13300 IDENTICAL_IGNORING_COMMAS=True
h3_h3_stream_e2e.rs:     tokens(main)=16039 tokens(now)=16039 IDENTICAL_IGNORING_COMMAS=True
```

Five struct-enum variants expanded from `ServerStream { per_request: usize },` to block form.
Mechanism: rustfmt SKIPS formatting an item containing a comment it cannot fit inside
`max_width`. Those enums carried such comments, so rustfmt had been leaving the variants
inline; compressing the comments let rustfmt format the enums for the first time. **This is
not revertible** — restoring the inline form turns `cargo fmt --check` RED, verified:

```
$ rustfmt --edition 2024 --check <file with variants forced back inline>
rustfmt exit=1
-    ServerStream { per_request: usize },
+    ServerStream {
+        per_request: usize,
+    },
```

`rustfmt --check` on `main`'s copy of the same file exits 0, confirming the inline form was
never rustfmt-canonical — it was rustfmt-suppressed. Flagging for the lead rather than
silently accepting.

**Attribute lines** — a full census over every lb-quic `.rs` file:

```
ALL attribute counts identical to main
```

Zero attribute lines added or removed anywhere in the area, so no `#[tokio::test]` /
`#[test]` / `#[cfg]` / `#[allow]` can have been dropped. This is structural, not luck: the
applier REFUSES to run if any targeted line — or any replacement line — is not a comment or
blank, and `#[...]` matches neither. I tightened the guard mid-round to drop the `*`-prefix
heuristic entirely (lb-quic has zero `/* */` blocks), because it was matching multiplication
continuations like `* lb_quic::h3_bridge::H3_RESP_CHANNEL_DEPTH`.

**F-S29-1 canary** — `grep -c 'or_insert_with()' crates/lb-quic/src/conn_actor.rs` = **1**.
Surviving text (14 lines on `main` → 10 after round 1 → 6 now):

> F-S29-1 (gRPC-over-H3 large-response trailer drop): `drain_streams_to_conn`'s `retain`
> REMOVES the `Progressive` StreamTx the instant the stream goes terminal, and a stale
> receiver can outlive it. Use `get_mut`, NOT `entry().or_insert_with()`: a fresh StreamTx
> replays the leftover `End`, fires a spurious FIN + RESET, and `stream_shutdown` DISCARDS
> a large response's still-buffered trailer+FIN (gRPC-fatal). A missing StreamTx means the
> stream already terminated correctly: drop the stale receiver, skip.

`cargo fmt -p lb-quic -- --check` is CLEAN.

**Line width.** Compression pushed 36 comment lines past 100 columns. I re-wrapped them;
the area is back to exactly `main`'s 33 over-long lines, all pre-existing and all code.
This also matters mechanically — an over-width comment is what suppresses rustfmt on an
item, which is how the three reflows above happened.

### Per-file

| file | main | after R1 | after R2 | R2 cut | total cut |
|---|---:|---:|---:|---:|---:|
| `src/h3_bridge.rs` | 1440 | 486 | 380 | 22% | 74% |
| `src/raw_proxy.rs` | 1054 | 474 | 332 | 30% | 69% |
| `src/conn_actor.rs` | 828 | 391 | 284 | 27% | 66% |
| `tests/h3_h3_stream_e2e.rs` | 684 | 336 | 153 | 54% | 78% |
| `src/passthrough.rs` | 546 | 304 | 213 | 30% | 61% |
| `tests/h3_h1_resp_stream_e2e.rs` | 454 | 244 | 134 | 45% | 70% |
| `tests/h3_h2_stream_e2e.rs` | 265 | 151 | 86 | 43% | 68% |
| `tests/grpc_h3_e2e.rs` | 218 | 147 | 90 | 39% | 59% |
| `src/ws_tunnel.rs` | 218 | 129 | 89 | 31% | 59% |
| `src/router.rs` | 259 | 115 | 79 | 31% | 69% |
| `tests/s16_b3_reset_propagation_verify.rs` | 171 | 103 | 57 | 45% | 67% |
| `tests/s16_b2_backpressure.rs` | 199 | 101 | 59 | 42% | 70% |
| `src/terminate_loopback.rs` | 176 | 100 | 75 | 25% | 57% |
| `src/public_header.rs` | 154 | 99 | 84 | 15% | 45% |
| `tests/h3_connection_recycle_e2e.rs` | 129 | 89 | 52 | 42% | 60% |
| `src/listener.rs` | 228 | 88 | 66 | 25% | 71% |
| `tests/s19_b4_datagram_verify.rs` | 143 | 84 | 39 | 54% | 73% |
| `src/udp_dataplane.rs` | 122 | 78 | 57 | 27% | 53% |
| `tests/s19_b5_verify.rs` | 129 | 70 | 39 | 44% | 70% |
| `tests/h3_h1_stream_body_errors_e2e.rs` | 120 | 68 | 37 | 46% | 69% |
| `tests/s16_b3_reset_propagation_smoke.rs` | 116 | 65 | 34 | 48% | 71% |
| `tests/s19_b6_zero_rtt_rejection.rs` | 120 | 65 | 42 | 35% | 65% |
| `tests/s16_b2_stream_relay_smoke.rs` | 114 | 64 | 32 | 50% | 72% |
| `tests/s19_b6_metrics_nonvacuous.rs` | 90 | 64 | 27 | 58% | 70% |
| `tests/s16_b1_two_connections.rs` | 104 | 63 | 34 | 46% | 67% |
| `tests/s16_b2_reset_not_fin.rs` | 96 | 61 | 29 | 52% | 70% |
| `tests/h3_h1_stream_body_e2e.rs` | 130 | 59 | 33 | 44% | 75% |
| `tests/listener_lifecycle.rs` | 71 | 56 | 21 | 62% | 70% |
| `tests/s19_b5_stream_flood.rs` | 101 | 55 | 28 | 49% | 72% |
| `tests/h3_h1_trailers_resp_e2e.rs` | 75 | 54 | 30 | 44% | 60% |
| `tests/s16_b2_multistream.rs` | 100 | 53 | 21 | 60% | 79% |
| `src/lib.rs` | 175 | 52 | 36 | 31% | 79% |
| `tests/s19_b6_two_connections.rs` | 103 | 52 | 32 | 38% | 69% |
| `src/h3_config.rs` | 105 | 48 | 33 | 31% | 69% |
| `tests/round8_h3_authority_enforced.rs` | 70 | 47 | 24 | 49% | 66% |
| `tests/h3_graceful_close.rs` | 73 | 44 | 18 | 59% | 75% |
| `tests/router_accept_path.rs` | 76 | 43 | 23 | 47% | 70% |
| `tests/s19_b4_datagram_relay_smoke.rs` | 73 | 41 | 16 | 61% | 78% |
| `tests/s16_raw_proxy_smoke.rs` | 47 | 36 | 20 | 44% | 57% |
| `tests/h3_h1_bridge_e2e.rs` | 63 | 33 | 13 | 61% | 79% |
| `tests/quic_router_leak.rs` | 47 | 31 | 17 | 45% | 64% |
| `tests/public_header_differential.rs` | 45 | 30 | 17 | 43% | 62% |
| `tests/passthrough_retry_differential.rs` | 43 | 24 | 13 | 46% | 70% |
| `src/cleanup_guard.rs` | 34 | 20 | 16 | 20% | 53% |
| `examples/passthrough_linkage_probe.rs` | 39 | 15 | 8 | 47% | 79% |
| `tests/proptest_header.rs` | 22 | 10 | 6 | 40% | 73% |
| **TOTAL** | **9669** | **4842** | **3028** | **37.5%** | **68.7%** |

### What round 2 cut

* **Tests to near-zero-plus-catches: 2,443 → 1,268 (48 %).** The brief's estimate was
  "~1,100 lines across your test files"; the actual cut was 1,175. Every module header
  (9–28 lines each) is now 2–6 lines; every `§n` / `// --- (5b) ... ---` navigation banner
  is gone; every per-case `///` is one or two lines carrying only the BINDING assertion.
* **The move round 1 under-used — multi-line `///` on `pub` items → one line.** In `src/`
  this was the dominant lever: `stream_request_to_h3_upstream` 23 → 15, `pump_dir` 24 → 16,
  `validate_request_pseudo_headers` 14 → 10, `MAX_RELAY_STREAMS` 17 → 10,
  `write_h1_request` 14 → 10, `drain_request_body` 16 → 11, `try_send_pending_goaway`
  14 → 9, `BoundedDgramQueue` 12 → 8, `reclaim_flows` 15 → 10, `build_retry_packet`
  17 → 13, `stream_h1_response` 14 → 9, `stream_h2_response` 12 → 8.
* **`# Errors` sections collapsed** from a 3-line stanza (`/// # Errors`, `///`, text) to a
  2-line one wherever the body was one line — ~40 sites.
* **Narrative deleted outright**: `// B6 (R14/R12): caps now carried on RawBackend; the
  const defaults keep these tests byte-identical` (7 test files), `// SESSION 24 / INC-3:`
  prefixes, `// ----- plumbing -----`, `// Emit on-wire bytes.`, `// Maglev pick.`,
  `// Open per-flow backend UDP socket.`, and every assert-restating comment sitting
  directly above an `assert!` whose message already said the same thing.

### What I refused to cut, and why 3,028 and not 2,500

The gap is 528 lines. Here is where they are, measured rather than asserted:

* **`src/` + `examples/` = 1,752 lines.** Of these, **1,153 are `///`/`//!`** and **387 of
  those are already single-line blocks** — the `#![deny(missing_docs)]` floor on ~130 `pub`
  items plus every `pub` struct field and enum variant. That floor is not reducible without
  dropping the lint. The remaining 766 doc lines sit in 194 multi-line blocks, and I went
  through all 194 individually in this round; what survives at 2–3 lines is contract or
  catch, not prose. The other 599 `src` lines are plain `//`, almost all already 1–2 lines.
* **`tests/` = 1,268 lines** across 32 files — an average of 40 lines per real-wire
  adversarial suite. What remains is one or two lines per case, and each second line is
  carrying the load: "*AND request trailers are DROPPED (backend sees exactly ONE HEADERS
  frame)*", "*so the actor reaps the receiver and the bridge's next send returns
  `ClientGone`*", "*pre-fix the gateway forwarded only `:status` + `content-length`*".
  Deleting those second lines is what 2,500 costs.

Catches deliberately preserved (verified present after the sweep):

* `conn_actor.rs` — F-S29-1 (canary, above); the F-MD-4 request-side smuggling guard
  (quiche's FIRST `finished_streams` pop lacks the reset re-check its SECOND performs);
  `goaway_pending` vs `goaway_sent` must stay separate or the admit-past-boundary window
  re-opens; `H3_INTERNAL_ERROR` is deliberately NOT `H3_NO_ERROR` and NOT
  `H3_REQUEST_CANCELLED`; `send_goaway`'s multiple-of-4 precondition holding by construction.
* `h3_bridge.rs` — CF-QUICHE-FRAME-COMPLETENESS + the content-length under-run guard; the
  F-MD-4 response-leg MIRROR; the request-leg `H3_REQUEST_CANCELLED` vs response-leg
  `H3_INTERNAL_ERROR` asymmetry with "*do not fix*"; `RESPONSE_HOP_BY_HOP` as a deliberate
  cross-crate duplicate REQUIRED by RFC 9114 §4.2; request trailers INTENTIONALLY dropped on
  the H3→H1 leg (smuggling vector); the RFC 8441/9220 `:protocol` ⇒ `:scheme`+`:path`
  inversion; the PASS-3 0-length-DATA re-arm gap and the macro-hygienic-label note.
* `raw_proxy.rs` — `Shutdown::Write ⇒ RESET_STREAM` / `Shutdown::Read ⇒ STOP_SENDING`, still
  flagged COUNTERINTUITIVE ("*swapping the arms silently emits the wrong frame*");
  CF-S16-RELAY-STALL and the `!half.src_fin_seen` gate; the three non-interchangeable
  `dgram_send` arms; `MAX_RELAY_STREAMS`'s 128 MiB arithmetic (pinned by a unit test);
  drop-newest-not-drop-oldest.
* `passthrough.rs` — the FlowEntry "holds no key material" SAFETY/INVARIANT block and the
  `_flow_entry_field_audit` compile-error mechanism, including why it is unconditional
  rather than `cfg(debug_assertions)` (S34 release-build breakage); the
  `#[allow(deprecated)]` TOOLCHAIN SHIM on `fetch_update`; LRU-not-FIFO;
  CF-S15-PASSTHROUGH-RETRY-ODCID; F-INFRA-01 on both loaders as deliberate duplicates.
* `public_header.rs` — RFC 9001 §5.4 (every field read is wire-cleartext); the
  VersionNegotiation-folded-onto-Retry arm (the crate denies `unreachable!`); the §A.2-vs-§A.3
  fixture provenance.
* `ws_tunnel.rs` — R8 bounded-by-construction (`PollSender` parks, does not buffer — the
  property WS-over-H2 lacked, CF-S27-2) and the RFC 9220 close-vs-reset mapping.
* `router.rs` / `cleanup_guard.rs` — CODE-2-08 and the denial-of-service-via-panic-exhaustion
  mechanism; the `2 * max_connections` dispatch-entry bound.
* Tests — every negative control and mutation recipe: the `stream_finished()`
  collected-stream witness trap (4 suites), F-S20-1 `full_send=false`, the drop-newest
  control, the `retain`-removed and cap-removed controls, both "VERIFIER mutation" recipes in
  `h3_h3_stream_e2e.rs`, the `test-gauges` FEATURE GATE note in `h3_h1_resp_stream_e2e.rs`,
  the CASE 15 "RE-TIGHTEN to `!(200 && fin)`" instruction, and the SUITE_SERIAL /
  CF-SATURATION-1 rationale in `grpc_h3_e2e.rs`.

Two test bodies consist ONLY of a comment (`c3_unit_supplement_documents_coverage`, and the
`transient — retry next iteration` arm in `h3_connection_recycle_e2e.rs`); both were kept
deliberately, since emptying them is a code change.

### Method

Range-based `(start, end, replacement)` specs applied by a script that REFUSES to run if any
targeted line, or any replacement line, is not a comment or blank — so deleting a
non-comment line is structurally impossible. 15 spec files, 802 edits, 46 files. After every
spec: the code-identity proof, then a per-file attribute census against `main`, then commit
with explicit paths (never `git add -A`). The one code change that slipped through was the
`":scheme" => { }` arm, caught by the round-1-baseline comparison rather than by the
`main` comparison — which is the check I would recommend the other sweepers run too, since
comparing against `main` alone hides a round-2 regression behind a round-1 acceptance.
