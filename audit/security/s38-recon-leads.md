# S38 Recon: production-vs-test-codec scoping + concrete starter leads

Lead-level recon before Phase 1. **Read this first** — it tells you where the REAL
internet-facing attack surface is, so you don't burn budget auditing test infrastructure.

## CRITICAL SCOPING: which parsers are actually on the internet-facing data path?

The HTTP/QUIC/TLS/WS **byte parsing is almost entirely DELEGATED** to maintained, upstream-fuzzed
dependencies. Confirmed by dependency graph + call-site grep:

| Surface | PRODUCTION parser | Ours / hand-rolled? |
|---|---|---|
| H1 server (client-facing) | `hyper::server::conn::http1::Builder` (h1_proxy.rs:684) | NO — hyper |
| H1 client (backend) | `hyper::client::conn::http1::handshake` (h1_proxy.rs:1251) | NO — hyper |
| H2 server | `hyper::server::conn::http2::Builder` (h2_proxy.rs:827) | NO — hyper + `h2` crate |
| H3 (client-facing + upstream) | `quiche::h3` | NO — quiche (S26 migration) |
| QUIC transport / Initial / TLS-in-Initial | `quiche` + BoringSSL | NO — quiche |
| TLS termination / ClientHello / SNI | `rustls` + BoringSSL | NO — rustls |
| WS frames (H1/H2/H3) | `tokio_tungstenite::tungstenite` | NO — tungstenite |
| **QUIC public header (Mode A passthrough)** | **`lb_quic::public_header`** | **YES — OURS, every datagram, no-decrypt** |
| H3 backend response status-line | `lb_quic::h3_bridge::parse_status_line` (priv, h3_bridge.rs:625) | YES — backend-facing |
| H3 pseudo-header / ext-CONNECT validator | `lb_quic::h3_bridge::validate_request_pseudo_headers:489` | YES — on quiche-decoded headers |
| SNI↔authority compare | `lb_l7::sni_authority::check_sni_authority:95` | YES (compare, not parse) |
| Retry token mint/verify + 0-RTT replay | `lb_security` (ring AEAD) | YES — crypto |
| L4 packet fields | `lb-l4-xdp/ebpf` | YES — wire, bounds-checked |

**`lb-h1`, `lb-h2`, `lb-h3-testcodec` are TEST CODECS — NOT on the production data path.**
- `lb-h1`: **no crate depends on it** (only root `lb-integration-tests` test usage).
- `lb-h2`: depended on by `lb-l7` **only for SECURITY CONSTANTS + detectors** (`DEFAULT_SETTINGS_MAX_PER_WINDOW`, `PingFloodDetector`, `DEFAULT_ZERO_WINDOW_STALL_TIMEOUT`) that are mirrored into **hyper/h2 server config** (`h2_security.rs`). Its hand-rolled `decode_frame`/`HpackDecoder` have **zero production call-sites**.
- `lb-h3-testcodec`: explicitly test-only (production H3 = quiche::h3 since S26).

### What this means for each auditor
- **Fuzzing the test codecs (h1_parser, h1_chunked, h2_frame, h2_hpack, h1_request_line, h3_frame)
  is defense-in-depth on test tooling — a crash there is a TEST-TOOL bug, NOT a production DoS.**
  Run them (cheap, real code, finds real panics → regression tests), but a finding's PRODUCTION
  severity is LOW unless you can show the same class reaches hyper/quiche. Don't over-rank them.
- **The crown-jewel hand-rolled production parser is `lb_quic::public_header` (Mode A).** Fuzz it
  hard (`quic_public_header` target, added Phase 0).
- **The highest yield overall is the PROXY/RELAY LOGIC we wrote on top of the delegated parsers**
  — header translation, smuggling detection, hop-by-hop strip, CL/TE handling, trailer
  forwarding, upgrade ordering. The parsing is hyper's; the SECURITY DECISIONS are ours. Attack
  those with real-wire PoCs (through hyper), not by fuzzing a test codec.
- **The CONFIGURATION of the delegated parsers is ours** — e.g. does `h2_security.rs` cap
  hyper/h2's `max_header_list_size` / CONTINUATION / reset-flood / settings-flood? Verify the
  limits are actually set, not left at permissive defaults.

---

## Concrete starter leads (found during recon — confirm or refute, don't assume)

### For parser-auditor
- **L-PARSE-1 (production):** `lb_quic::public_header::parse_public_header` — fuzz via the new
  `quic_public_header` target to a real iteration count. It claims "never panics on arbitrary
  input"; prove it. Look at `decode_varint` (public_header.rs:134) length math, the short-header
  caller-supplied `short_dcid_len` path, DCID/SCID len bounds (≤20).
- **L-PARSE-2 (test-codec, lower sev):** `lb_h2` HPACK `decode_integer` (hpack.rs, before :332) —
  RFC 7541 §5.1 multi-byte continuation: is there a cap on continuation bytes + overflow guard?
  A `(val << 7) | ...` without bound can overflow / spin. Confirm. (Test codec → LOW prod sev.)
- **L-PARSE-3:** `lb_quic::h3_bridge::parse_status_line` (h3_bridge.rs:625) + the response
  head/header translation — backend-facing (semi-trusted), but a malicious backend is in scope.

### For protocol-auditor (HIGHEST YIELD)
- **L-PROTO-1:** H2→H1 downgrade — `SmuggleDetector::check_h2_downgrade` (h2_to_h1.rs:72). Try to
  smuggle CL/TE/Connection/Upgrade or a CRLF-bearing header value from an H2 frame into the
  materialized H1 request line/headers sent to an H1 backend. hyper parses the H2; WE translate.
- **L-PROTO-2:** H1→H1 CL/TE — `SmuggleDetector::check_all_mode` (h1_proxy.rs:1075). Re-attack the
  S22 class with fresh eyes: duplicate CL, CL+TE, TE:chunked variants, obs-fold, whitespace/case.
- **L-PROTO-3:** Trailer / response-splitting — H3→H1/H2 trailer forwarding (h3_bridge.rs:2273
  undeclared-trailer guard), the gRPC grpc-status path (S29), Alt-Svc/XFF injection
  (h1_proxy.rs:2732-2772). Can a backend-controlled header/trailer inject into the client response?
- **L-PROTO-4:** Upgrade ordering — WS H1 (`handle_ws_upgrade`, h1_proxy.rs:2409, dial-before-101),
  WS H2 (`handle_ws_extended_connect`, h2_proxy.rs:1363), WS H3 (actor waits `Ready`). Is a
  101/200 ever emitted before/without a successful backend handshake (F-S27-1 regression)?
- **L-PROTO-5:** Host/:authority vs SNI — `check_sni_authority` (sni_authority.rs:95) → 421. Bypass?

### For resource-auditor (HIGH YIELD)
- **L-RES-1:** **CONTINUATION flood (CVE-2024-27316).** Production H2 is hyper/h2 — does
  `h2_security.rs` cap `max_header_list_size` and the CONTINUATION/header-frame accumulation?
  Read h2_security.rs end-to-end and confirm hyper is configured with bounded header limits +
  the reset-flood (CVE-2023-44487) + settings-flood caps. PoC if a limit is missing/permissive.
- **L-RES-2:** **RE-ATTACK S36 recycling.** `max_requests_per_h3_connection` (default 1000) +
  GOAWAY — adversarially open streams as fast as possible, ignore GOAWAY, reset streams; prove
  the StreamMap-collected stays bounded (CF-S32). Also `MAX_RELAY_STREAMS` (256, raw_proxy.rs:671).
- **L-RES-3:** **chunked decoder buffer caps (test-codec, but mirror the pattern in hyper path):**
  `ChunkedDecoder::try_read_size` / `try_read_trailers` (chunked.rs:134,170) wait for CRLF /
  double-CRLF with **no internal cap on `self.buf`**. In the test codec the caller must bound it.
  For PRODUCTION, confirm hyper's H1 enforces a header/trailer size cap (it does by default —
  verify the config doesn't disable it). The interesting question: are hyper's H1 limits
  (`max_headers`, header size) left at safe defaults or overridden?
- **L-RES-4:** Slowloris — `HttpTimeouts` (header 10s / body-idle 30s / total 60s) + the Watchdog
  sweeper (h1_proxy.rs:1085). Trickle 1 byte/interval through head, body, and the upgrade dial;
  can the progress accounting be fooled? Confirm the timeouts wrap the hyper serve future.
- **L-RES-5:** R8 response bound under hostile backend — 64 MiB `MAX_RESPONSE_BODY_BYTES`, bounded
  channels. A backend that streams forever / huge chunks — no whole-response buffering?

### For infra-auditor
- **L-INFRA-1:** Enumerate **every "0 = disable" config knob** (`max_keepalive_requests=0`,
  `max_requests_per_h3_connection=0`, `tls_verify_peer=false`, `xdp_new_flow_cap=0`). Each is a
  silent-defense-disable foot-gun. Does validation warn/reject the dangerous ones? (lb-config
  validator, lib.rs:1318+.)
- **L-INFRA-2:** **Reload race** — SIGHUP under traffic. ArcSwap `.store()` vs connection-task
  `load_full()`. Can a half-applied reload cross config between connections, tear an H1s cert/ALPN
  leg (two `.store()` calls, main.rs:704-705), or apply a restart-required change live (honesty-
  contract violation)? reload.rs:218 diff partition + main.rs:409 reload_config.
- **L-INFRA-3:** **Admin API** — `/metrics` token (SHA-256, constant-time `subtle`,
  admin_auth.rs:129). Probes (`/livez` `/readyz` `/startupz` `/healthz`) are UNAUTHENTICATED — do
  they leak (readiness during drain, version)? Bind validation (admin_auth.rs:229) — can the
  data-plane reach the admin bind; `allow_non_loopback` foot-gun.
- **L-INFRA-4:** **TLS** — key file 0600 enforcement (key.rs:97 lax vs strict), retry-secret
  auto-gen perms, no server-side mTLS (gap for the threat profile?), TLS1.2 downgrade,
  upstream cert verification (`tls_verify_peer`/`tls_ca_path`).
- **L-INFRA-5:** **Secrets** — keys not zeroized (rustls Arc); confirm nothing logs key/token
  bytes, no secret in metrics labels or error messages.
- **L-INFRA-6:** **XDP** — packet-field bounds (IHL, ext-headers, ports) in ebpf/main.rs; new-flow
  rate cap (SYN-flood); map-key safety. (Single-kernel only this session; F-ESC-1 carried.)

### Known carry-forward reviewed (not a finding)
- **CF-S7-RHU** (h3_bridge.rs:164): `request_h3_upstream` 30s wall-clock cap can truncate a slow
  H1→H3/H2→H3 upload, but **fails closed** (`Err(PrematureEof)` + Reset, never `End`) → no
  response-splitting. Availability edge only; not a security finding.
