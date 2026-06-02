# SESSION 31 — CF-QUICHE-UPGRADE: quiche 0.28.0→0.29.1 + tokio-quiche 0.18.0→0.19.0

**Branch:** `feature/quiche-0.29-upgrade-s31` (off `main` @ `c09ecbab`, S30 promoted, SPEC COMPLETE)
**Scope:** quiche + tokio-quiche ONLY — ISOLATED from Dependabot PR #222's other 16 crates.
**Status:** IN PROGRESS (Phase 0).

---

## Phase 0 — baseline + hygiene

- Base tip confirmed: `c09ecbab` (S30 promote). Branch `feature/quiche-0.29-upgrade-s31` created + pushed.
- S30 strays: none (clean `ps aux`).
- Disk: 42 GB free on `/` (`eg-target` 5.7 GB), 8 cores. ≥25 GB OK.
- Locked baseline versions: **quiche 0.28.0**, **tokio-quiche 0.18.0** (Cargo.lock).
- ×3 baseline gate on 0.28 reference: RUNNING (`scripts/s31-gate.sh baseline-0.28`).

#### Methodology fix: `--no-fail-fast` (gate completeness)

First baseline run truncated at **83 of 240 test binaries** (493 passed) because `cargo test`
defaults to **fail-fast at the binary level** — it stops launching further test binaries after
the first one fails. The trigger was the known **CF-FCAP1-FLAKE**
(`fcap1_h2_over_cap_upload_yields_413`, `lb-integration-tests::h2h1_md_streaming_verify`,
60.02s timeout race under 8-core saturation). S26's reference gate ran all 240 binaries / 1454
passed only because it happened to be flake-free that pass.

This is a **blocker for a quiche upgrade**: `h2h1` sorts before every critical lb-quic H3 test
(`grpc_h3`, `h3_*`, `s16_*`, `s19_*`, `quic_router_leak`, `round8_h3_authority_enforced`), so a
fail-fast truncation would hide any real H3 regression behind the flake. Fix: add
`--no-fail-fast` to `scripts/s31-gate.sh` so every pass runs all 240 binaries and reports the
COMPLETE failure set (strictly MORE rigorous; R15 — a truncated run is an incomplete job). Known
saturation flakes are then classified by isolation (R2: never weaken an assertion).

#### Baseline ×3 verdict (0.28, completed run `bestk2qzb`) — GREEN

| pass | binaries | passed | failed | ignored |
|---|---|---|---|---|
| PASS1 | 247 | 1511 | **1** (`h2h3_fcap1_over_cap_upload_never_complete`) | 18 |
| PASS2 | 247 | 1512 | 0 | 18 |
| PASS3 | 247 | 1512 | 0 | 18 |

clippy RC=0, fmt RC=0. The single PASS1 failure is the known **CF-FCAP1-FLAKE** family (F-CAP-1
over-cap H2 saturation timeout race) — it passed in PASS2 AND PASS3, the signature of a saturation
flake, not a real defect (R2; isolation-proven in prior sessions). **0.28 reference = GREEN**
(1512/0 modulo the known flake). This is the comparison anchor for the 0.29 gate.

### h3spec baseline (the 0.28 reference for the 0.29 diff)

Source: `audit/h3spec/s26-h3spec-final.log` (S26 = the migrated quiche::h3 stack on 0.28,
which IS the current `main` stack — the precise comparison reference).

**Result: 49 examples, 12 failures.** All 12 are quiche-0.28-internal validation gaps
(= the CF-QUICHE-UPGRADE list). They are:

| # | h3spec finding | category |
|---|---|---|
| 1 | TRANSPORT_PARAMETER_ERROR if initial_source_connection_id missing [Transport 7.3] | transport-param validation |
| 2 | TRANSPORT_PARAMETER_ERROR if original_destination_connection_id received [Transport 18.2] | transport-param validation |
| 3 | TRANSPORT_PARAMETER_ERROR if preferred_address received [Transport 18.2] | transport-param validation |
| 4 | TRANSPORT_PARAMETER_ERROR if retry_source_connection_id received [Transport 18.2] | transport-param validation |
| 5 | TRANSPORT_PARAMETER_ERROR if stateless_reset_token received [Transport 18.2] | transport-param validation |
| 6 | TRANSPORT_PARAMETER_ERROR if max_udp_payload_size < 1200 [Transport 7.4/18.2] | transport-param validation |
| 7 | TRANSPORT_PARAMETER_ERROR if ack_delay_exponent > 20 [Transport 7.4/18.2] | transport-param validation |
| 8 | TRANSPORT_PARAMETER_ERROR if max_ack_delay >= 2^14 [Transport 7.4/18.2] | transport-param validation |
| 9 | PROTOCOL_VIOLATION if reserved bits in Handshake non-zero [Transport 17.2] | reserved-bits |
| 10 | PROTOCOL_VIOLATION if reserved bits in Short non-zero [Transport 17.2] | reserved-bits |
| 11 | QPACK_ENCODER_STREAM_ERROR if dynamic table capacity exceeds limit [QPACK 4.1.3] | QPACK (old #23) |
| 12 | QPACK_DECODER_STREAM_ERROR if Insert Count Increment is 0 [QPACK 4.4.3] | QPACK (old #25) |

(Historical context: S22's hand-rolled stack failed 19; the S23→S26 migration to `quiche::h3`
closed 7 by construction, leaving these 12 quiche-internal gaps. The 0.29 hope per the
S27 handoff: "#23/#25 + several #1-10 flip ✔".)

---

## API surface inventory (what the upgrade must keep compiling)

**tokio-quiche: minimal.** The ONLY direct use is `pub use tokio_quiche::ConnectionParams;`
(`crates/lb-quic/src/lib.rs:135`, `#[cfg(feature="quic-terminate")]`). The gateway does NOT
ride tokio-quiche's connection driver — it drives `quiche::Connection` directly via its own
`udp_dataplane` / `listener` / `conn_actor`. So the "tokio-quiche Stats breaking change" only
bites if `ConnectionParams`' shape changed. (To confirm in Phase 1.)

**quiche::h3 (the migrated H3 front):** `Config::new` + `set_max_field_section_size` +
`set_qpack_max_table_capacity(0)` + `set_qpack_blocked_streams(0)` + `enable_extended_connect`
(`h3_config.rs`); `Connection::with_transport`; `poll` → `Event::{Headers,Data,Finished,Reset}`;
`Header::new` / `NameValue`; `send_response` / `send_body` / `send_additional_headers`;
`recv_body`; `Error::{Done,StreamBlocked}`; `qpack::Decoder::new`.

**KEEP-surface (R8 + F-MD-4) sits on:**
- `recv_body(conn, sid, &mut scratch)` into a FIXED scratch buffer (body-size-independent bound)
  — `conn_actor.rs:1004` (req), `:1600` (WS-H3); any `recv_body` error → upstream Reset
  (F-MD-4 smuggling guard, `conn_actor.rs:984-1037`).
- `send_body(conn, sid, buf, fin)` with partial-write retain (backpressure) — `conn_actor.rs:765,834,1796,1810`.
- `poll(conn)` event loop — `conn_actor.rs:1106`.
- 0.29's "h3: clear streams when send finishes before recv" is exactly in the F-MD-4 area →
  R13 re-prove reset-vs-EOF mapping still holds.

**quiche transport (Mode A passthrough + Mode B + H3 transport):** `accept` / `connect` /
`retry` / `accept_with_retry` / `RetryConnectionIds`; `Config::{new,verify_peer,load_verify_locations_from_file}`;
`Connection::{send,recv,close,timeout,peer_cert,send_ack_eliciting}`; `ConnectionId::from_ref`;
`Header::from_slice`; `RecvInfo`; `Shutdown::{Read,Write}`; `Type::*`; Error variants.

---

## Version delta + API-break analysis (research, diff-level — to be EMPIRICALLY confirmed)

Source-grounded analysis (full agent output: `audit/deps/s31-research-quiche-029.md`). **quiche has
no CHANGELOG.md** — analysis is from the git history `0.28.0..0.29.1` (48 commits) + docs.rs.

**Versions:** quiche 0.29.1 (2026-05-27) and tokio-quiche 0.19.0 (2026-05-14) ARE the latest (no
0.29.2 / 0.20). Pinned to latest of both.

**Two mission hints CORRECTED by source:**
1. "Stats fields moved to `Connection::peer_transport_params()`" — **FALSE.** `peer_transport_params()`
   already existed in 0.28 (unchanged); `Stats` lost zero fields. The actual change: `Stats`/`PathStats`/
   `h3::Stats` became `#[non_exhaustive]` (commit `2f00a0d`) + `Stats` gained `amplification_limited_count`
   (commit #2432). We only **read** `cstats.recv`/`cstats.lost` (in ONE test, `crates/lb/src/main.rs:5233`),
   never construct/exhaustively-match → **no break.**
2. "Expect #23/#25 + several #1-10 to flip ✔ in h3spec" — **LIKELY FALSE.** `transport_params.rs` changed
   only +6/−2 (a `MAX_ACK_DELAY_EXPONENT` const-rename, no new receive-side validation); **no QPACK file
   changed at all.** Prediction: **same 12 failures, closes 0.** (To confirm by fresh h3spec.)

**API BREAKS (compile-affecting): essentially none for our surface.** Every symbol we use
(`quiche::{accept,connect,retry,accept_with_retry,Config::*,Connection::*,ConnectionId,Header,RecvInfo,
Shutdown,Type,Error::*}`, `quiche::h3::{Connection::with_transport,poll,send_response,send_body,
send_additional_headers,recv_body,Header::new,NameValue,Config::*,qpack::Decoder::new,Event::*,Error::*}`,
`tokio_quiche::ConnectionParams`) is signature-identical in 0.29.1. `h3::Config::new` is now `const fn`
(source-compatible). The `#[non_exhaustive]` additions only block external construction / exhaustive match,
neither of which we do (verified: no `quiche::Stats {` match, no `ConnectionParams {`/`::new` in our code).

**BEHAVIOR CHANGES (5 risk items, all verified SAFE at diff level — to RE-PROVE empirically):**
- **(1) "h3: clear streams when send finishes before recv"** (commit `cbc8173`, 0.29.1) — F-MD-4 area.
  **SAFE & strengthened**: `pop_finished_stream()` checks `Err(StreamReset)` FIRST, returns `Event::Reset`
  before any `Finished`; the poll reset arm now calls `remove_local_finished_stream` so a
  locally-finished-then-reset stream can NEVER later surface a spurious `Finished`; new upstream regression
  test `collect_reset_streams`. → R13 re-prove reset-vs-EOF (E1+E2).
- **(2) "ignore priority updates for closed streams"** — NO IMPACT (we don't use PRIORITY_UPDATE).
- **(3) MAX_PTO** (`MAX_PTO_EXPONENT` raised to 20) — internal non-pub const, loss-recovery PTO backoff
  only, no public API, not an idle/handshake timeout. NO IMPACT.
- **(4) recv_body/send_body/poll** — signatures unchanged; NO flow-control/buffering change (diff touches
  only stream-cleanup bookkeeping). → R8 bounded-relay invariant intact. RE-PROVE empirically.
- **(5) server TP validation** — NO new validation added (hence no h3spec movement).
- **One client-visible shift** (commit `2cccba0`): LB-as-H3-client now rejects illegal control frames
  (CANCEL_PUSH/SETTINGS/GOAWAY/MAX_PUSH_ID/PRIORITY_UPDATE) on a request stream with `H3_FRAME_UNEXPECTED`
  instead of silently accepting — strictly more conformant, routes through our existing reset/error path.

**Highest first-compile risk:** quiche dropped the OpenSSL backend + vendored-BoringSSL option; now pulls
BoringSSL exclusively via `boring`/`boring-sys`. We don't enable the `openssl` feature anywhere → expected
fine, but watch the first build for BoringSSL toolchain/feature drift.

## Phase 1 — the upgrade + adaptation (empirical)

### The one real adaptation: MSRV 1.85 → 1.88 (owner-decided)

`cargo update -p quiche --precise 0.29.1 -p tokio-quiche --precise 0.19.0` revealed that **quiche
0.29.1 and tokio-quiche 0.19.0 hard-require Rust 1.88** (the project pinned 1.85 via
`rust-toolchain.toml` + `Cargo.toml rust-version`, the deliberate "MSRV-pin" — foundations 4.5.0 /
idna_adapter 1.1.0 were held back to keep 1.85). There is no way to adopt quiche 0.29 without
bumping the toolchain off 1.85. **Surfaced to owner (R7) — decision: pin EXACTLY 1.88** (quiche's
MSRV; smallest new-lint blast radius vs jumping to stable 1.95/1.96; truthful MSRV declaration).

Applied:
- `rustup toolchain install 1.88` (rustfmt + clippy).
- `rust-toolchain.toml` channel `1.85` → `1.88`.
- `Cargo.toml` `rust-version` `1.85` → `1.88` (workspace + lb-integration-tests).
- MSRV-pin note updated: foundations 4.5.0 + idna_adapter 1.1.0 stay pinned to ISOLATE from #222's
  other tiers (no longer needed to hold MSRV; 1.88 clears their reqs).
- Toolchain bump touches the WHOLE workspace (R7 scope note, owner-accepted): the ×3 gate, h3spec,
  R8/R13, re-soak ALL run on 1.88 now; any new 1.88 clippy lints fixed surgically (mechanical, no
  logic changes); MSRV change documented prominently in the promote message.

### Cargo.lock isolation (verified)

ONLY quiche's subtree moved. **No forbidden #222 crate bumped** (hyper, h2, rand, socket2, rcgen,
toml, tokio-tungstenite, idna_adapter, foundations all UNCHANGED; boring/boring-sys stay 4.21.2):

| moved (quiche subtree) | from → to |
|---|---|
| quiche | 0.28.0 → 0.29.1 |
| tokio-quiche | 0.18.0 → 0.19.0 |
| qlog | 0.17.0 → 0.18.0 |
| darling{,_core,_macro} | 0.21.3 → 0.23.0 |
| serde_with{,_macros} | 3.17.0 → 3.20.0 |
| time / time-core / time-macros | 0.3.37/0.1.2/0.2.19 → 0.3.47/0.1.8/0.2.27 |
| deranged / num-conv | 0.3.11/0.1.0 → 0.5.8/0.2.2 |
| ADDED | zstd, zstd-safe, zstd-sys, flate2, simd-adler32, jobserver, pkg-config, bs58, debug_panic |

Two benign resolver details: (1) qlog 0.18 added a `foundations` dep edge → resolved to the
existing 4.5.0 pin (no new foundations version). (2) socket2 dep edges for the LEGACY pre-migration
`quinn`/`quinn-udp`/`hyper-util` re-unified 0.6.3 → 0.5.10 — both socket2 versions were already in
the lock before and after; no socket2 crate upgrade, and these are not on our quiche path.

### Source changes required: ZERO production changes (research confirmed)

- **`cargo build --workspace --all-features` on 1.88 = clean** (`BUILD_RC=0`). quiche 0.29.1 +
  tokio-quiche 0.19.0 + the reworked BoringSSL build (boring/boring-sys 4.21.2) all compile with
  **zero production source edits**. The BoringSSL build-system overhaul watch-item did NOT bite.
- **5 new Rust-1.88 clippy lints** (surfaced by the toolchain bump, NOT by quiche), all mechanical
  (owner-authorized; verified no logic smuggled in):
  - 3× `uninlined_format_args` (clippy --fix): `pool.rs:881` (test), `h1h3_md_streaming_verify.rs:698`
    (test), `reload_zero_drop.rs:1388` (test) — inline the format var.
  - 1× `io_other_error` (clippy --fix): `grpc_h3_e2e.rs:835` (test) → `std::io::Error::other(_)`.
  - 1× `doc_overindented_list_items` (manual): `h2_proxy.rs:948-950` — canonical markdown list
    indentation in a doc comment (no code).
  - Re-ran `clippy --workspace --all-targets --all-features -- -D warnings` → exit 0.
- **KEEP-surface untouched**: 4 lint fixes are in tests, 1 is a doc comment — no production logic
  changed. The diff-level F-MD-4 / R8 safety claims are re-proven empirically in Phase 2.

### ×3 gate on 0.29/1.88: GREEN (atomic, no asterisks)

Binding R11 gate (`scripts/s31-gate.sh 029-1.88`, completed run `bk9y1rini`, on the final
fmt-fixed tree):

| stage | result |
|---|---|
| BUILD (--no-run) | RC=0 |
| PASS1 | 247 binaries, **1512 passed, 0 failed**, 18 ignored |
| PASS2 | 247 binaries, **1512 passed, 0 failed**, 18 ignored |
| PASS3 | 247 binaries, **1512 passed, 0 failed**, 18 ignored |
| clippy `-D warnings` | RC=0 |
| fmt --check | RC=0 |

**All three passes fully clean** — even cleaner than the 0.28 baseline (which had the fcap1 flake
in PASS1). The fcap1 saturation flake did not fire in any 0.29 pass. No regressions across the 9
cells + both QUIC modes + WS matrix + gRPC-H3 (R3). The ≥50-iter F-MD-4 burst tests + R8 gauge
tests are part of these 1512 and passed ×3 → first-order R8/R13 confirmation on 0.29 (explicit
evidence-capturing re-proofs follow).

## Fresh h3spec diff vs baseline — ZERO DELTA (no regression, no closures)

Fresh h3spec 0.1.13 against the migrated H3-terminate front on 0.29.1 (`scripts/s31-h3spec.sh 029`,
completed run `b602ys3cl`, log `audit/deps/s31-h3spec-029.log`):

**0.29.1 result: 49 examples, 12 failures — BYTE-IDENTICAL to the 0.28 baseline.**

| | 0.28 baseline (S26) | 0.29.1 (S31) | delta |
|---|---|---|---|
| examples | 49 | 49 | 0 |
| failures | 12 (#1–12) | 12 (#1–12) | **0** |
| passing | 37 | 37 | 0 |

The 12 failures are the SAME list, in the same order: transport-param receive validation (#1–8),
reserved-bit PROTOCOL_VIOLATION (#9–10), QPACK encoder/decoder stream error (#11/#12). **Zero
closed, zero NEW failures → zero regression (R3 satisfied).** Exactly as the diff-level research
predicted (`transport_params.rs` was a +6/−2 const rename, no QPACK file changed). The 37 passing
checks — including the pseudo-header (#12–15/#22), frame-seq, QPACK static-table, and Huffman gains
from the S23–S26 migration — are all unchanged.

**CF-QUICHE-UPGRADE verdict: re-verified-at-0.29.1, NARROWED, NOT closed.** These 12 remain
quiche-internal validation gaps in 0.29.1; closing them would require either a still-newer quiche
that adds the checks, an upstream fix, or hand-rolling (which the S23–S26 migration deliberately
deleted). The carry note stays open with the updated status "12 unchanged at 0.29.1". This upgrade
is a maintenance/safety bump, not an h3spec-conformance improvement — and that was the expectation.

## R8 re-proofs (0.29) — PASS, non-vacuous, body-size-independent

Driver `scripts/s31-phase2-reproofs.sh` (completed run `bbpi6s29y`, log
`audit/deps/s31-phase2/r8-reproofs.log`): 6 R8 gauge tests, **6 passed / 0 failed**, gauge
evidence captured (`--nocapture`). The bound is identical to 0.28 (e.g. 73,856 B — the S24 figure):

| path | evidence (0.29) | verdict |
|---|---|---|
| H3→H1 response (R2) | 4 MB body → **max_retained=73,856 B**, ceiling=262,656 B, **margin=15.97×**, retained/ceiling=0.28 | bounded, body-size-INDEPENDENT |
| H3→H1 backpressure (R3) | mid_stall_retained=peak_retained=73,856 B (0.28× ceiling) | backpressure holds |
| H3→H1 single large DATA (t5) | memory-bounded through stalled upstream — ok | bounded |
| Mode B (s16_b2) | PHASE A: client queued 625,152 B, backend echoed 0 while stalled (paused); PHASE B: full 4 MB round-trips byte-identical on resume, no loss/reorder | backpressure both ways + integrity |
| WS-H3 | backend sent 63/512 (ceiling 256) while client not reading, then drains; lost=0 | plateau then drain |
| gRPC-H3 | retained=67,722 B vs total=1,054,098 B (ceiling 262,656) | bounded |

**R8 RE-PROVEN on 0.29** — non-vacuous (4 MB body, 73 KB retained), body-size-independent,
backpressure both directions. 0.29's recv_body/send_body changes (bookkeeping only) did not
reintroduce buffering. (These tests are also among the 1512 that passed ×3 in the gate.)

## R13 F-MD-4 reset-vs-EOF (0.29) — PASS, ≥60-iter bursts ×3, live negative control

Same driver (log `audit/deps/s31-phase2/r13-fmd4-bursts.log`): **33 passed / 0 failed** across the
×3 outer loop (over each test's built-in ITERS=60 → ≥180 effective burst iters) + single-shot
reset/discriminator group. 0.29's **"h3: clear streams when send finishes before recv"** change is
exactly in this area — re-proven SAFE:

| case | evidence (0.29) |
|---|---|
| E1 ingress reset (H3→H3) | `H3H3_CASE7_BURST iters=60 baseline=1 after_burst=1` — client reset never becomes a clean complete |
| E2 egress reset (H3→H3) | `H3H3_FMD4_MIRROR_BURST iters=60 (all reset, none clean-complete)` |
| E2 mid-body reset | `h3h3_e2e_upstream_reset_midbody_resets_client_no_fin` — ok |
| H1→H3 resp truncation | `H1H3_RESP_TRUNC_CL_BURST iters=60 all-incomplete (no false-complete)` + chunked `(no leak)` |
| H1→H3 request F-MD-4 | `H1H3_FMD4_BURST iters=60 baseline=1 after_burst=1` |
| H2→H3 resp truncation | `H2H3_RESP_TRUNC_CHUNKED_BURST iters=60 all-incomplete`; `..._CL false_complete=false` |
| gRPC-H3 reset | `GRPC_H3_RESET reset=true fin=true status=502 grpc_status=None` — NOT laundered to a clean grpc-status; `grpc_h3_burst_50_unary_cycles` ok |
| WS-H3 | `ws_over_h3_burst_50_upgrade_relay_close_cycles` ok |
| Mode B reset | `reset-not-FIN: VERIFIED — backend saw 12950 bytes and NO clean FIN after the mid-transfer client reset` |
| **NEGATIVE CONTROL** | `discriminator: VERIFIED — a genuine clean FIN IS observed (witness is live)` + `r4_empty_response_body_clean_fin` ok → the reset-assertions are non-vacuous |
| §7.1 gap (expect unchanged) | `h3h3_e2e_no_cl_truncated_data_delivered_quiche_028_frame_completeness_gap` ok + `..._content_length_truncation_resets_no_clean_complete` ok |

**R13 RE-PROVEN on 0.29** — reset still maps to reset (502 / reset-stream, never clean EOF) across
E1, E2, all H3 cells, gRPC-H3, WS-H3, Mode B; the live clean-FIN discriminator proves non-vacuity.
CF-QUICHE-FRAME-COMPLETENESS (§7.1 no-CL gap) persists unchanged on 0.29 (carry note, as predicted).

## Re-soak (0.29) — 4 BOUNDED + sc9 DRIFT investigated → warmup-plateau artifact

Run 1: 5 quiche scenarios CONCURRENTLY, 900s, sample=15s (61 samples), scale=1
(`scripts/soak/run-soak.sh`, completed `btgxeg1hn`, data `audit/soak/s31-soak-data/`).
run-soak failures=0; all 5 completed.

| scenario | overall | panic | fds | threads | load (ok/err) |
|---|---|---|---|---|---|
| sc5_modea | BOUNDED | 0 | flat | flat | — |
| sc4_modeb | BOUNDED | 0 | flat | flat | — |
| sc7_h3terminate | BOUNDED | 0 | flat | flat | — |
| sc8c_ws_h3 | BOUNDED | 0 | flat | flat | — |
| **sc9_grpc_h3** | **DRIFT** | 0 | **flat (11→12)** | flat (10→9) | sustained 1,331,228/0 + churn 919,752/0 = **2.25M RPCs, 0 err** |

**sc9 DRIFT analysis — memory-only (rss_kb +46.7%, vmhwm_kb +47.4%), NOT a resource leak:**
fds flat (11→12), threads flat, panic=0, 2.25M RPCs err=0. The RSS time-series is a classic
**warmup-ramp-then-plateau**, not a linear leak:

```
sample  1: 8400 KB (boot)     sample 36: 41360 KB   ← plateau reached
sample  6: 25012 KB (ramp)    sample 41: 41400 KB
sample 11: 27816 KB           sample 46: 41488 KB
sample 21: 33484 KB (step)    sample 51: 41560 KB
sample 31: 34256 KB           sample 56: 41640 KB
sample 36: 41360 KB           sample 61: 41708 KB   ← +0.8% over the last ~6 min (FLAT)
```

The working set establishes over ~9 min (warmup), then plateaus at ~41 MB for the last ~6 min
(samples 36→61: +348 KB total ≈ 14 KB/sample ≈ flat). The analyzer's first-third-vs-last-third
heuristic compares the warmup median (28.3 MB) to the plateau (41.5 MB) → +46.7% DRIFT, but the
TAIL is flat = bounded. VmHWM is a peak-only gauge (monotone by construction — a documented
false-DRIFT). Confounders vs S29's "sc9 no-leak at 1.18M RPCs": this run was 5-scenario CONCURRENT
(saturation) and did 2.25M RPCs (~2×), so a higher, later-plateauing working set is expected.

**Per R2 / "attribution ≠ symptom": re-ran sc9 in ISOLATION.**

Run 2 (isolated, 900s, `audit/soak/s31-soak-sc9-isolated/`): overall DRIFT again, BUT —
fds flat (11→12), threads flat, panic=0, 2.5M RPCs (1,493,500+1,006,392), err=2 (0.00008%).
RSS: warmup → **41,032 KB plateau held flat t=465→825 (~6 min)** → stepped to **54,968 KB at
t=840 and held flat t=840→900 (~1 min)**. Two plateaus; fds/threads flat throughout (12/9). The
**~41 MB plateau is IDENTICAL to Run 1's ~41 MB despite more RPCs** — a leak cannot produce a fixed
work-independent ceiling. The shape (flat plateaus + flat fds/threads) is allocator-arena
working-set establishment (bounded by peak concurrency), not a per-request leak. But the 2nd
plateau held only ~1 min — too short to declare a final ceiling for a PROMOTE gate (R8/R11).

Run 3 (isolated, **1800s** definitive bounded test, `audit/soak/s31-soak-sc9-1800/`): RUNNING
`blfmky9eg`. Verdict rule: RSS plateauing + holding flat for the last ≥15 min ⇒ BOUNDED (allocator
working set); continued stepping ⇒ real 0.29 leak (blocker). fds/threads/panic must stay flat/0.

## h2spec-intact confirm — (TBD)
## Fresh h3spec diff vs baseline — (Phase 2, TBD)
## R8 re-proofs — (Phase 2, TBD)
## R13 F-MD-4 bursts — (Phase 2, TBD)
## Re-soak — (Phase 2, TBD)
## h2spec-intact confirm — (Phase 2, TBD)
## Promote decision — (TBD)
## Remaining #222 tiers handoff — (TBD)
