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

## Version delta — (Phase 1, TBD)
## API breaks adapted — (Phase 1, TBD)
## Fresh h3spec diff vs baseline — (Phase 2, TBD)
## R8 re-proofs — (Phase 2, TBD)
## R13 F-MD-4 bursts — (Phase 2, TBD)
## Re-soak — (Phase 2, TBD)
## h2spec-intact confirm — (Phase 2, TBD)
## Promote decision — (TBD)
## Remaining #222 tiers handoff — (TBD)
