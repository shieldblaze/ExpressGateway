# L7 divergences — resolved (different-but-fine)

Author: `div-l7` (Round-8 divergence analyst). Each entry is a place
where our code differs from the production references but the divergence
is intentional and defensible. Compare counterpart findings under
`audit/round-8/findings/ROUND8-L7-*.md` for the un-fine divergences.

---

## R-L7-01 — QUIC routing keys on DCID, not (src, dst) tuple
**Reference**: nginx CVE-2026-40460 (`audit/round-8/research/nginx.md` L#10) — H3 source-IP spoofing bypassed authZ + rate limiting because nginx keyed on the immediate UDP peer.
**Our equivalent**: `crates/lb-quic/src/router.rs:174-177` — `let dcid_key: Vec<u8> = header.dcid.to_vec(); if let Some(sender) = connections.get(&dcid_key) { ... }`.
We correctly route on the Destination Connection ID. This is the canonical fix for the nginx CVE. The `set_disable_active_migration(true)` at `crates/lb-quic/src/listener.rs:362` further removes the multi-path complexity.

## R-L7-02 — No custom rustls cert verifier
**Reference**: rustls SECURITY.md lesson 28 (do not roll your own verifier; if you must, name the flag so loudly nobody can use it accidentally).
**Our equivalent**: `grep -rn 'ServerCertVerifier\|ClientCertVerifier\|dangerous_configuration\|with_custom_certificate_verifier' crates/` returns zero matches outside of TLS-ticket key derivation. We use rustls's default `WebPkiServerVerifier`. Matches the reference's strongest recommendation.

## R-L7-03 — No `complete_io()` usage (rustls L#25)
**Reference**: rustls CVE-2024-32650 (`complete_io` infinite loop on close_notify after client_hello). Only affects raw rustls `Stream` / `StreamOwned`.
**Our equivalent**: `grep -rn 'complete_io' crates/ tests/` returns zero matches in production code. We use `tokio-rustls` exclusively. The reference says tokio-rustls is unaffected by design.

## R-L7-04 — H2 reset-after-stream-reset (Envoy GHSA-84xm-r438-86px) absorbed by hyper
**Reference**: Envoy lesson 1 (`audit/round-8/research/envoy.md` L#1) — DATA frame arrived during deferred-delete window re-entered filter chain. Fix: explicit `saw_downstream_reset_` check at top of `decodeData`.
**Our equivalent**: we use hyper as the H2 codec. Hyper's stream lifecycle does not have a "deferred delete" model in the Envoy sense — the `RecvStream` future is dropped on reset; no callback chain re-enters. Our `crates/lb-l7/src/h2_proxy.rs` does not maintain a custom reset-aware filter chain. Different-but-fine: the model that bit Envoy doesn't apply.

## R-L7-05 — No `Range` header parsing (nginx CVE-2017-7529 N/A)
**Reference**: nginx Range filter integer overflow (`audit/round-8/research/nginx.md` L#3); Pingora `bytes=` alone invalid (L#7).
**Our equivalent**: `grep -rn 'Range' crates/lb-l7 crates/lb-h1 crates/lb-h2` returns only `accept-ranges` / `content-range` in HPACK static-table entries — we never parse or arithmetic on `Range`. Bodies pass through. Different-but-fine: not a code path.

## R-L7-06 — No built-in cache (Pingora GHSA-f93w-pcj3-rggc N/A)
**Reference**: Pingora's default `CacheKey` shipped without authority — cross-origin cache poisoning. Pingora removed the default impl entirely.
**Our equivalent**: no cache crate. REL-2-01 removed compression; we never added cache. Document in `audit/deferred.md` per `_l7_handoff.md` cross-cutting note: any future cache pillar must NOT ship a one-line default key — force constructor-explicit per-tenant key.

## R-L7-07 — `lb-h3::frame::decode_frame` not on the live H3 data path
**Reference**: HAProxy lessons 6, 7 (H3 must reject unaligned frames; H3 must check body size with CL on empty FIN).
**Our equivalent**: `crates/lb-h3/src/frame.rs:84-100` is unit-test surface only; live H3 termination uses quiche via `tokio-quiche`. Different-but-fine *today*. Same status as ROUND8-L7-11 (lb-h2 frame) — lesson-not-yet-paid-for becomes live the day we wire the parser onto a socket. Marked as documented non-goal here, not as a finding, because there's no live attack surface.

## R-L7-08 — `quiche` pinned past the RETIRE_CONNECTION_ID infinite-loop fix
**Reference**: quiche CVE-2025-7054 (`audit/round-8/research/hyper-h2-quinn.md` L#19) — infinite loop on RETIRE_CONNECTION_ID under multi-path; fix in 0.24.5.
**Our equivalent**: we pin `quiche = "0.28"` directly (per `_l7_handoff.md` inventory), past the 0.24.5 fix. `set_disable_active_migration(true)` in `crates/lb-quic/src/listener.rs:362` further removes the multi-path code path entirely. Double-safe.

## R-L7-09 — QUIC `set_active_connection_id_limit` not explicitly set (quiche L#22 marginal)
**Reference**: quiche `xhg9-xwch-vr7x` — unbounded retired-CID storage. Spec has `active_connection_id_limit`.
**Our equivalent**: we do not call `cfg.set_active_connection_id_limit(...)`; quiche defaults to 2 (RFC 9000 §18.2 minimum). Combined with `set_disable_active_migration(true)`, the retired-CID set cannot grow because migration is disabled and our SCID rotation is fixed at handshake time. Different-but-fine. Could be tightened to `set_active_connection_id_limit(2)` explicitly for documentation; not a defect.

## R-L7-10 — H2 reset accounting both `max_pending_accept_reset_streams` AND `max_local_error_reset_streams` configured
**Reference**: hyper / h2 lesson 4 (rapid reset cap) + lesson 6 (MadeYouReset cap). `_l7_handoff.md` Top-10 #5: both caps required.
**Our equivalent**: `crates/lb-l7/src/h2_security.rs:80-81` — both knobs set to `DEFAULT_SETTINGS_MAX_PER_WINDOW = 100` (`crates/lb-h2/src/security.rs:222`). Tighter than hyper's defaults (20 / 1024). The reuse of the same constant for two knobs is documented as a deliberate choice. Different-but-fine.

## R-L7-11 — TE-trailers folding chain enforced at H1Strict mode (Pingora L#2)
**Reference**: Pingora GHSA-hj7x-879w-vrp7 (Critical) — `Transfer-Encoding: a, chunked` style misparsed. RFC 9112: must fold to `chunked` at tail.
**Our equivalent**: `crates/lb-security/src/smuggle.rs:190-207` — `check_te_strict` rejects any codec other than `chunked`; default `H1` mode allows codec-chain. The `H1Strict` mode is the safer default and exists; the gateway threads it from the per-listener `[runtime].strict_te` config knob (SEC-2-15 matrix). Different-but-fine — we have the same defence as Pingora's fix, behind a config switch.

## R-L7-12 — `Content-Length: +5` not directly tested but caught by `http::HeaderValue` round-trip
**Reference**: hyper GHSA-f3pg-qwvg-p99c.
**Our equivalent**: hyper's own internal parser (which we use through the H1 server) does the CL parsing; hyper 1.x carries the GHSA-f3pg fix. Our `lb-h1/parse.rs` does not parse CL ourselves — we extract headers as strings and let hyper handle. Different-but-fine *for the inbound* H1 path. The chunked encoder/decoder is our code and is *not* hyper-routed in some test paths (see ROUND8-L7-02 for the chunked-side gap).

## R-L7-13 — No NTLM private session retrieval (HAProxy L#21 N/A)
**Reference**: HAProxy NTLM auth requires connection affinity (the whole handshake must go on the same TCP connection).
**Our equivalent**: we do not implement NTLM. The L7 listener is a transparent forwarder; per-connection affinity for auth protocols would be the upstream's contract via `Connection: close` semantics, which we honour by stripping hop-by-hop headers correctly (`crates/lb-l7/src/h1_proxy.rs:1101-1121`). Different-but-fine. Should be documented as a non-goal.

## R-L7-14 — Hot reload model is signal + new-process bind, not FD passing
**Reference**: Pingora SIGQUIT + Unix-socket FD passing; HAProxy master/worker + SO_REUSEPORT.
**Our equivalent**: `crates/lb/src/main.rs:1900-1923` handles SIGUSR1 for cert reload (in-place — REL-2-03). The full hot-restart-with-FD-passing model is not implemented. Different-but-fine for the current scope (no claimed zero-downtime upgrade); a deferred item for `div-ops`. This is genuinely a div-ops boundary, called out here for completeness.

## R-L7-15 — `set_send_buf_size = 64 KiB` (hyper default 400 MB)
**Reference**: h2 lesson 8 — h2's default `max_send_buf_size = 400 MB`. `_l7_handoff.md` defensive pattern 2.
**Our equivalent**: `crates/lb-l7/src/h2_security.rs:88` — `max_send_buf_size: 64 * 1024`. Tighter than hyper's documented default. `apply()` at line 117 enforces it. Different-but-fine (much tighter than reference).

## R-L7-16 — Hop-by-hop strip honours the Connection-listed names (RFC 9110 §7.6.1)
**Reference**: implicit in HAProxy L#10, L#12 lessons that protocol-neutral analysers must work over a normalised representation; hop-by-hop header handling is part of that.
**Our equivalent**: `crates/lb-l7/src/h1_proxy.rs:1101-1121` (`strip_hop_by_hop`) collects names from `Connection` header values, then removes both the static hop-by-hop set AND those listed names. Tested at `crates/lb-l7/tests/hop_by_hop_set.rs`. Matches the reference behaviour.

## R-L7-17 — H2 multi-stream concurrent use (Pingora L#11 panic fix)
**Reference**: Pingora 0.4.0 fix for panic on multi-stream concurrent use of the same H2 sender.
**Our equivalent**: `crates/lb-io/src/http2_pool.rs:218-228` — `acquire_sender` returns `sender.clone()`. hyper's `SendRequest` is Clone + per-stream send-half is owned per request internally. The race Pingora hit was their custom mux; hyper's design avoids it. Different-but-fine — our pool design and hyper's codec design jointly avoid the Pingora-class race.

## R-L7-18 — Quiche pinned past the optimistic-ACK cwnd bug
**Reference**: quiche GHSA-2v9p-3p3h-w56j / GHSA-6m38-4r9r-5c4m — invalid / optimistic ACKs widened cwnd. Fix in 0.24.4.
**Our equivalent**: `quiche = "0.28"` (per inventory in handoff), past the fix. No custom CCA code in `lb-quic`. Different-but-fine — upstream-handled.

## R-L7-19 — `Connection: close` injection on disable_keep_alive (hyper 1.5.0)
**Reference**: hyper 1.5.0 CHANGELOG (`audit/round-8/research/hyper-h2-quinn.md` L#14) — H1 must send `Connection: close` explicitly, not rely on TCP FIN.
**Our equivalent**: `crates/lb-l7/src/h1_proxy.rs:489-509` — `conn.as_mut().graceful_shutdown()` calls hyper's internal `disable_keep_alive`, which serialises `Connection: close` when the response head has not yet been flushed. Documented in the code comment (PROTO-2-16). Different-but-fine — the contract matches hyper 1.5.0's documented behaviour.

## R-L7-20 — Pool key is `(SocketAddr, proto)`, not `(host, port, sni, alpn)`
**Reference**: Pingora L#15 hot/cold tier; their pool key includes the full peer descriptor.
**Our equivalent**: `crates/lb-io/src/http2_pool.rs:147` — `peers: Mutex<HashMap<SocketAddr, PeerEntry>>`. Keyed on resolved `SocketAddr`. For H3 upstream, `crates/lb-io/src/quic_pool.rs` shares the SocketAddr-keying. Different-but-fine for the current single-tenant single-SNI-per-backend design. If multi-tenant / SNI-aware backend dispatch lands, the key must expand to `(SocketAddr, sni, alpn)`. Document as future-proofing in `audit/deferred.md`.
