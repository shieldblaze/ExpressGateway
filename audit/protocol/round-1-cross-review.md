# Round 1 Cross-Review Log — `proto`

Open items for the 30-minute adversarial cross-review at the end of
Round 1 / start of Round 2. Each row will be marked **AGREED** /
**DISPUTED** / **DEFERRED** with the other teammate's initials.

| # | Counterparty | Question / claim | Status |
|---|---|---|---|
| 1 | `sec` | `SmuggleDetector` is unit-tested but never invoked from the L7 proxy data path. Is hyper's built-in CL+TE rejection sufficient, or do we wire the detector? | open |
| 2 | `sec` | TLS 1.2 still enabled with no config knob. Hard 1.3-only listener mode in v1 scope? | open |
| 3 | `sec` | SNI ↔ Host/:authority disagreement is not enforced. OK for single-cert deployments; required before SNI-multiplexed vhost lands. | open |
| 4 | `sec` | 0-RTT replay-guard cache size — what's the production knob? Tests use `new(1000)`. | open |
| 5 | `code` | `LB_QUIC_ALPN = b"lb-quic"` — is the real H3 listener advertising `b"h3"`? I cannot find an H3 listener spawn site in `crates/lb/src/main.rs`. | open |
| 6 | `code` | `ListenerMode` falls through to PlainTcp for unknown `protocol = …` values (`main.rs:837`). Hard error instead? | open |
| 7 | `code` | `H2ToH2Bridge` / `H3ToH3Bridge` trait impls don't strip hop-by-hop. Runtime data path covers it via `strip_hop_by_hop` in `h2_proxy.rs`. Fix at trait level or document precondition? | open |
| 8 | `code` | Cosmetic: `HOP_BY_HOP` in `h1_proxy.rs:54-63` lists `trailers` (not a real header name). Cleanup. | open |
| 9 | `ebpf` | When `protocol = "tls"` for L4 passthrough, does the XDP fast path try to dispatch on the same socket? Confirm no double-bind. | open |
| 10 | `ebpf` | QUIC UDP socket vs any XDP redirect rule — possible competition? | open |
| 11 | `rel` | `h2spec` not in CI image — pin the install. | open |
| 12 | `rel` | `wstest` (Autobahn) not in CI and the test harness only `--help`-probes (`ws_autobahn.rs:24-34`). Add proper fuzzing-client run. | open |
| 13 | `rel` | No `h3spec` harness. Pick a tool (h3i / quic-tracker) and add. | open |
| 14 | `rel` | `tests/conformance_h{1,2,3}.rs` are codec round-trip unit tests, not protocol-conformance tests. Rename or add real harnesses. | open |
| 15 | `code` / `sec` | H2 listener picks `URI.authority()` over `Host` silently; RFC 9113 §8.3.1 says reject when both present and disagree. | open |
| 16 | `code` | Verify hyper's H2 server actually sets `SETTINGS_ENABLE_CONNECT_PROTOCOL=1` (needed for the RFC 8441 ext-CONNECT WebSocket bootstrap). | open |
| 17 | `code` / `sec` | No 1xx / 100-Continue forwarding test or explicit policy. | open |
| 18 | `sec` | Trailer pass-through across H1↔H2/H3 (non-gRPC case) is untested. Risk surface? | open |
