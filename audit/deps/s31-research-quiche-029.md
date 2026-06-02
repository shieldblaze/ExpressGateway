# S31 research: quiche 0.28.0→0.29.1 + tokio-quiche 0.18.0→0.19.0 — diff-level inventory

Source-grounded (read-only) analysis. **quiche has no CHANGELOG.md** (verified absent at the
`0.29.1` tree). Authoritative source = git history `0.28.0..0.29.1` (48 commits, via
`gh api .../compare/0.28.0...0.29.1`) + docs.rs API pages for both versions.

> This is a diff-level PRIOR, not proof. Every claim is RE-PROVEN empirically in Phase 2
> (build + ×3 gate + fresh h3spec + R8 + R13 + soak). R15: results only from completed runs.

## 1. Versions
- quiche **0.29.1** (2026-05-27) is latest (0.29.0 = 2026-05-14; 0.28.0 = 2026-04-01). No 0.29.2/0.30.
- tokio-quiche **0.19.0** (2026-05-14) is latest. No 0.19.1/0.20. (quiche 0.27.0 is yanked; we skip.)

## 2. API breaks (compile-affecting) — essentially none for our surface
| Symbol | change | impact on us |
|---|---|---|
| `quiche::Stats` | `#[non_exhaustive]` (commit `2f00a0d6203d`) + new field `amplification_limited_count: u64` (#2432) | NONE — only read `cstats.recv`/`.lost` in 1 test (`crates/lb/src/main.rs:5233`), never construct/match |
| `quiche::PathStats`, `quiche::h3::Stats` | `#[non_exhaustive]` | NONE (don't construct/match) |
| `tokio_quiche::ConnectionParams` | `#[non_exhaustive]`, ctors `new_server`/`new_client` | NONE — only `pub use` re-export (`crates/lb-quic/src/lib.rs:135`) |
| `h3::Config::new` | now `const fn` | NONE (source-compatible) |
| `Connection::{send,recv,close,timeout,peer_cert,peer_transport_params,send_ack_eliciting,stats}` | byte-identical (docs.rs 0.28 vs 0.29.1) | NONE |
| `quiche::{accept,connect,retry,accept_with_retry,RetryConnectionIds}` | unchanged (`lib.rs` +167/−37 doesn't touch them) | NONE |
| `Config::{new,verify_peer,load_verify_locations_from_file}`, `ConnectionId::from_ref`, `Header::from_slice`, `Shutdown::*`, `MAX_CONN_ID_LEN` | unchanged | NONE |
| `h3::Connection::{with_transport,poll,send_response,send_body,send_additional_headers,recv_body,send_request}` | identical sigs | NONE |
| `h3::Header::new`, `NameValue`, `qpack::Decoder::new`, `h3::Config::enable_extended_connect` | unchanged (`qpack/decoder.rs` NOT changed) | NONE |
| `h3::Error` / `h3::Event` variants | all present, unchanged | NONE |

Only ONE live (non-comment) `tokio_quiche` reference repo-wide = the re-export. Our H3
server+client run on `quiche::h3` + raw `quiche::Connection`, NOT the tokio-quiche driver, so the
tokio-quiche driver changes (GOAWAY, idle-close-on-receiver-drop, StreamCtx leak, retry-transient,
DCID iface) cannot affect our runtime.

## 3. Behavior changes (5 risk items) — all SAFE at diff level
1. **"h3: clear streams when send finishes before recv"** — commit `cbc8173cac80` (in 0.29.1).
   Adds `local_finished` + `finish_local_stream()` on FIN send (fixes a stream-map leak for the
   clean symmetric-finish case). **Reset path preserved & STRENGTHENED:** `pop_finished_stream()`
   checks `Err(StreamReset(e))` first → `Event::Reset(e)` before any `Finished`; poll reset arm calls
   `remove_local_finished_stream(s)` (comment: "avoid returning a Finished event later as well");
   new test `collect_reset_streams` asserts `Event::Reset(0)` after server resets a locally-finished
   stream. → F-MD-4 reset-vs-EOF SAFE.
2. **"ignore priority updates for closed streams"** — commit `1acc5babb9ed` (0.29.1). Adds
   `Connection::stream_closed()`; no impact (we don't use PRIORITY_UPDATE).
3. **MAX_PTO** — `MAX_PTO_EXPONENT` introduced (`c5d69d48`/`ca9f01c4`, 0.29.0) then raised 5→20
   (`23154316de76`, 0.29.1). Internal non-pub recovery const (overflow guard), loss-recovery PTO
   backoff only. NOT idle/handshake timeout. No public API. No impact.
4. **recv_body/send_body/poll** — sigs unchanged; `h3/mod.rs` diff touches only stream-cleanup
   bookkeeping; no flow-control/buffering change. R8 bounded relay intact.
5. **server TP validation** — `transport_params.rs` +6/−2 = `MAX_ACK_DELAY_EXPONENT` const-rename +
   local-set clamp; receive-side `ack_delay_exponent>20` check already existed in 0.28; no new
   receive-side validation. No h3spec movement.

**Client-visible shift** (commit `2cccba0516a6`): LB-as-H3-client now rejects illegal control frames
(CANCEL_PUSH/SETTINGS/GOAWAY/MAX_PUSH_ID/PRIORITY_UPDATE) on a request stream with
`H3_FRAME_UNEXPECTED` instead of silently accepting — strictly more conformant, routes through our
existing reset/error path. A well-behaved backend never triggers it.

**Escalations: none.** All five are bug-fixes that strengthen our invariants or no-ops for us.

## 4. h3spec prediction (the 12 carried failures) — closes 0, stays 11, uncertain 1
`transport_params.rs` +6/−2 (const rename, no new validation); ZERO qpack/* files changed; no
reserved-bit/PROTOCOL_VIOLATION check added to packet.rs/frame.rs.
- #1–6, #8 (transport-param receive validation): STAYS (no decode change).
- #7 (`ack_delay_exponent>20`): UNCERTAIN, predicted STAYS (check exists in both; the gap is the
  quiche "CONNECTION_CLOSE suppressed on first-packet error" deviation, unchanged in 0.29).
- #9–10 (reserved-bit PROTOCOL_VIOLATION): STAYS.
- #11–12 (QPACK encoder/decoder stream error): STAYS (no QPACK file changed; consistent with the
  documented "quiche reads-and-discards QPACK instructions, no error variant" limitation).

→ Expect the SAME 12. The upgrade is maintenance/safety, NOT an h3spec-conformance improvement.
CF-QUICHE-UPGRADE stays open (re-verified-at-0.29.1, narrowed), does not close.

## 5. Migration checklist
1. Cargo.toml: line 100 `quiche="0.28"`→`"0.29"`; line 101 `tokio-quiche="0.18"`→`"0.19"`. Workspace
   deps cascade to lb-io/lb-soak/lb-quic/lb.
2. BoringSSL: upstream removed the OpenSSL backend + vendored-BoringSSL option; now boring/boring-sys
   only. We don't enable `openssl` anywhere → expected fine; **watch the first build** (highest risk).
3. No Rust source edits required for the API surface.
4. (Defensive) future Stats `match` / ConnectionParams construction would need `..` / `new_*`.
5. Gate green; 6. fresh h3spec (expect same 12); 7. R8/F-MD-4/reset suites + soak; 8. PR note re
   `H3_FRAME_UNEXPECTED`.

**Net: low-risk, low-churn upgrade; zero required Rust source changes; KEEP-surface verified safe at
diff level; no h3spec movement; only watch-item = BoringSSL build-system overhaul at first compile.**
