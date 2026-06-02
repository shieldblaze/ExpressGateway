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

## Fresh h3spec diff vs baseline — (RUNNING)
## Fresh h3spec diff vs baseline — (Phase 2, TBD)
## R8 re-proofs — (Phase 2, TBD)
## R13 F-MD-4 bursts — (Phase 2, TBD)
## Re-soak — (Phase 2, TBD)
## h2spec-intact confirm — (Phase 2, TBD)
## Promote decision — (TBD)
## Remaining #222 tiers handoff — (TBD)
