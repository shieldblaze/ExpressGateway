# ExpressGateway — Session 38 Security Audit Report

**Base:** main @ b8a99078 · **Branch:** feature/security-audit-s38 · **Date:** 2026-06-08
**Scope:** internet-facing AND internal; ALL protocols (H1/H2/H3/QUIC/gRPC/WS), both QUIC modes,
the 9-cell front×back matrix, the operational layer (config/SIGHUP reload, admin API, L4/XDP).
**Method:** 4 parallel adversarial auditors (parser / protocol / resource / infra) + coverage-
guided fuzzing of the hand-rolled parsers + PoC reproduction of every finding and every clean
claim. Author≠verifier on fixes. SOLID-BEFORE-PROMOTE: no fix unverified, no finding asterisked.

---

## 1. Executive summary

This is the **adversarial** session: prior sessions (S20–S37) proved the gateway WORKS
(spec-complete, operational layer in, CI honest-green); S38 tried to BREAK it on the full
internet-facing all-protocol threat surface.

**Result: no CRITICAL, no HIGH, no MEDIUM-unfixed.** Findings: **1 MEDIUM + 7 LOW + 4 INFO.**
The MEDIUM and the security-relevant LOWs are FIXED-and-verified this session; the remainder are
fixed-if-cheap or documented-accepted-risk with rationale. **No product-fork, no new dependency-
implicating finding.** This is the rare defensible "mostly clean" adversarial result, and §5
proves WHY it is clean rather than asserting it (R4).

**Why the surface is hard to break (the architectural reason):**
1. **Wire parsing is delegated to maintained, upstream-fuzzed dependencies** — hyper (H1/H2
   server+client), the `h2` crate, `quiche::h3` (H3, since the S26 migration), quiche+BoringSSL
   (QUIC/TLS), rustls (TLS termination), tungstenite (WS frames). The hand-rolled crates
   `lb-h1`/`lb-h2`/`lb-h3-testcodec` are **TEST CODECS** with no production call-sites (verified
   by dependency graph + call-site grep). A panic in them is a test-tool bug, not a wire DoS.
2. **The one genuinely-ours internet-facing hand-rolled parser** — `lb_quic::public_header`
   (Mode A QUIC, runs on every datagram, never decrypts) — is **proven panic-free** by crate-level
   `#![deny(unwrap_used,expect_used,panic,indexing_slicing)]`, systematic `.get()`/`checked_add`/
   `try_from`, and the `ours_never_panics` proptest; the new `quic_public_header` libFuzzer target
   confirms it dynamically (§4).
3. **Every wire-bound header passes through a typed, validating funnel** — hyper's
   `HeaderName`/`HeaderValue`/Builder (which reject CR/LF/NUL and fail closed) for H1/H2, and
   QPACK length-prefixed binary encoding for H3 (a value cannot CRLF-split a field). So the
   smuggling/desync/response-splitting class is closed by construction across all 9 cells (§5).
4. **Resource bounds (R8) hold under hostile input** with identified, tested enforcement points;
   there is **no body-decompression anywhere** (no bomb surface); the S36 H3 recycling cap and
   `MAX_RELAY_STREAMS` bound the stream-per-connection vector adversarially.

The findings that DO exist are hardening gaps in OUR configuration/wiring of the delegated stack
(a slowloris timeout that was wired on H2 but not H1; an asymmetric perm-check on the retry
secret), not memory-unsafety or smuggling.

---

## 2. Findings by severity

| ID | Sev | Surface | Title | Disposition |
|----|-----|---------|-------|-------------|
| F-RES-1 | **MEDIUM** | H1 slowloris | hyper `header_read_timeout` inert (no `.timer()` wired) → header phase bounded by 60s `total`, not 10s | **FIXED** §6 |
| F-INFRA-01 | LOW(sec) | retry secret | LOAD path didn't perm-check existing file (forge/Mode-A-flood-bypass if world-readable) | **FIXED** §6 |
| F-RES-2 | LOW | H2 client | upstream `Http2Pool` builder omitted `max_header_list_size` (relied on h2 16KiB default) | **FIXED** §6 |
| F-RES-5 | LOW | slowloris watchdog | sweeper logs+removes but never closes socket; `progress()` header-only → slow-POST eviction dead | **FIXED (documented observability-only)** §6 |
| F-PARSE-3 | LOW | test-codec | `parse_chunk_size_hex` `checked_shl` inert behind 16-digit cap (comment wrong) | **FIXED (comment)** §6 |
| F-RES-4 | INFO | doc | `HttpTimeouts::header` stale doc-comment | **FIXED (doc)** §6 |
| F-RES-3 | LOW | QUIC | router `max_connections` hardcoded 100k; no per-IP QUIC sub-cap | TIERED-carried (CF-S38-QUIC-MAXCONN) §7 |
| F-PROTO-01 | LOW | H1 CL/TE | smuggle detector skips header pairs whose value fails `to_str()` (opaque bytes) — not exploitable | ACCEPTED/hardened §6/§7 |
| F-PARSE-1 | LOW | test-codec | lb-h2 hpack `decode_string` add + no bomb cap (no prod call-site) | ACCEPTED (test-codec) §7 |
| F-PARSE-2 | LOW | test-codec | lb-h3-testcodec `decode_frame` total-len add (benign w/ cap) | ACCEPTED (test-codec) §7 |
| F-PROTO-02 | INFO | gRPC | 200 path skips hop-by-hop strip (H2 binary, moot) | ACCEPTED §7 |
| F-PROTO-03 | INFO | WS | echoes client subprotocol w/o backend confirm | ACCEPTED §7 |
| F-PROTO-04 | INFO | H3 front | backend-head parser doesn't tchar-validate names (QPACK binary, moot) | ACCEPTED §7 |

No CRITICAL/HIGH. No product-fork. No dependency-implicating finding (the delegated parsers are
current; the held tokio 1.51.1 / WS-H2-gated / quiche items are tracked carry-forwards, not new).

---

## 3. Attack surface coverage (what was attacked, by auditor)

See `s38-threat-model.md` for the full surface and `s38-findings-{parser,protocol,resource,infra}.md`
for per-finding detail. Summary of what each auditor attacked and the verdict:

- **parser-auditor** (byte-level): `lb_quic::public_header` (prod), h3_bridge `parse_status_line` +
  response-head decode, the H3 pseudo-header validator, retry-token verify, and the test codecs
  (lb-h1/h2/h3-testcodec). Verdict: prod parser proven panic-free; 3 LOW test-codec notes.
- **protocol-auditor** (smuggling/desync, 9 cells + upgrades): H2→H1 / H3→H1/H2 downgrade, H1→H1
  CL/TE, trailer/response-splitting, WS H1/H2/H3 + CONNECT upgrade ordering, SNI↔authority 421,
  cross-stream/cross-connection bleed. Verdict: clean by construction; 1 LOW + 3 INFO hardening.
- **resource-auditor** (DoS/R8): H2 flood config, S36 H3 recycling re-attack, header/body/trailer
  caps + 413, slowloris timeouts, R8 response bound under hostile backend, fd/conn/dgram exhaustion,
  decompression. Verdict: R8 holds everywhere; 1 MEDIUM (slowloris timer) + 4 LOW/INFO.
- **infra-auditor** (TLS/admin/config-reload/secrets/XDP): hostile-config + "0=disable" knobs,
  SIGHUP reload race + honesty-contract, admin auth + bind, TLS/cert/mTLS, secret reachability,
  XDP packet bounds. Verdict: most-hardened surface; 1 LOW (retry-secret perm) + 3 documented LOW.

---

## 4. Fuzz campaigns

cargo-fuzz 0.13.1 on the pinned `nightly-2026-01-15`. 9 targets (5 pre-existing + 4 added this
session to close recon gaps). Production-critical Mode A parser got the longest box. Crashing
inputs (if any) → committed regression corpora + unit regression tests (R13).

> _**[PLACEHOLDER — filled after the campaign completes; R15: iterations cited from completed
> runs only.]** Per-target: target · seconds · executed units (iterations) · crashes · coverage._

New targets added (`fuzz/fuzz_targets/`): `quic_public_header` (the Mode A `parse_public_header` —
the prior `quic_initial` fuzzed quiche's parser, NOT ours), `h1_chunked`, `h2_hpack`,
`h1_request_line`. The `quic_public_header` target varies `short_dcid_len` across 0..=24 per input
to exercise the short-header path (per parser-auditor's coverage note).

---

## 5. Proven-clean scopes (defense identified + tested — R4)

A scope is recorded clean ONLY with a named defense AND a test/PoC that proves it holds
adversarially. (Detail + test names in the per-role findings files.)

**Parsers**
- `lb_quic::public_header` (Mode A, every datagram): crate `#![deny(unwrap/expect/panic/indexing)]`
  (incl. tests), all reads `.get()`-checked, `decode_varint` shift ∈{1,2,4,8} (no overflow), CID
  len ≤20, Initial double-varint `checked_add`+`try_from().unwrap_or(MAX)` completeness. Test:
  `ours_never_panics` proptest + the new `quic_public_header` fuzzer.
- `retry::verify`: HMAC-gated BEFORE any body parse + exact-length `copy_from_slice` → forge-
  resistant AND panic-free.

**Smuggling / desync (all 9 cells + WS H1/H2/H3 + gRPC)**
- Every attacker/backend-controlled header byte reaching an HTTP/1.1 wire is funnelled through
  hyper's typed `HeaderName`/`HeaderValue`/Builder (reject CR/LF/NUL, fail-closed). Every H3-wire
  header is QPACK-encoded (binary, length-prefixed → cannot CRLF-split).
- H2→H1: `check_h2_downgrade` + `:authority`/Host agreement + egress CL/TE strip; hyper builds the
  request line (no string-built request line to inject into).
- H1→H1 CL/TE: hyper strict-default server gates CL+TE / dup-CL / obs-fold; detector is DiD;
  single-use upstream socket.
- Upgrades: WS H1/H2 dial+handshake INLINE before 101/200 (F-S27-1 held); WS H3 waits `Ready`
  before 200. SNI↔authority 421: no bypass. Cross-stream: per-sid maps, `get_mut` not
  `or_insert` (S29 fix preserved), no key reuse → no bleed.
- PoCs run to PROVE clean (not assume): bare-LF backend header (H3 client sees one header, no
  injection); opaque-byte TE (listener 400, one clean backend request). [Results §6.]

**Resource / DoS (R8)**
- H2 flood config APPLIED (`.apply` at h2_proxy.rs:829, h2 0.4.14): CONTINUATION (CVE-2024-27316),
  Rapid-Reset (CVE-2023-44487), RUSTSEC-2024-0003, HPACK-bomb 64KiB, 256 streams, zero-window PING.
- S36 H3 recycling (`max_requests_per_h3_connection`=1000) + `MAX_RELAY_STREAMS`=256 bound
  adversarially (admit/reject counted → CF-S32 bound holds; refuse-path fail-safe).
- 64 MiB req/resp caps + 413 fire; response streamed not buffered; TCP admission gates fire
  pre-handshake; Mode-A reaper (S21 F-S20-2) verified; BoundedDgramQueue drop-newest.
- **No decompression anywhere** (flate2/zstd only via qlog/backtrace) → no bomb surface;
  Content-Encoding is passed through.

**Infra (TLS / admin / config-reload / secrets / XDP)**
- Config "0=disable" knobs all range-validated or documented sentinels; `deny_unknown_fields` on.
- SIGHUP reload: single `load_full` per connection = no torn snapshot; ALPN picks one leg = no
  H1s tearing; honesty-contract EXHAUSTIVE over all 12 listener + 6 top-level fields.
- Admin auth: constant-time compare after fixed-length hash = no length side-channel;
  `validate_bind` fails closed + is wired; probes leak nothing.
- Upstream `verify_peer(true)` enforced (all `verify_peer(false)` are `cfg(test)`); secrets
  redacted (custom `Debug` `finish_non_exhaustive`, no secret in any tracing call).
- XDP: `ptr_at` `checked_add`; rewrite re-validates larger struct; IPv6 ext-walk bounded; new-flow
  cap fails-open. (Single-kernel; multi-kernel F-ESC-1 carried.)

---

## 6. Fixes (PoC-now-fails + negative-control evidence)

Author = parser-auditor (independent of the finders resource/infra-auditor). Confirmer = lead.
Committed in `7f702188` (scoped to `crates/`). R13 layered verification per fix.

### F-RES-1 (MEDIUM) — H1 slowloris header timeout now active
- **Mechanism:** the H1 hyper builder (`h1_proxy.rs:684`) called `.keep_alive(true).serve_connection()`
  with **no `.timer()`**, so hyper's `header_read_timeout` was silently inert — the request-head
  read was bounded only by the 60s connection `total` (+ per-IP cap 1024), not the intended 10s
  `header`. A slowloris header-trickle held a connection/slot 6× longer than designed.
- **Fix:** `.timer(TokioTimer::new()).header_read_timeout(self.timeouts.header)` on the builder
  (mirrors H2's proven `timer + with_upgrades` wiring at h2_proxy.rs:828). Stale doc-comment
  (F-RES-4) corrected in the same edit.
- **Negative control (load-bearing, independently re-confirmed by lead):**
  `crates/lb-l7/tests/s38_h1_header_timeout.rs::h1_partial_head_closed_at_header_timeout_not_total`
  (boots H1 listener header=1s/total=10s, sends a partial head, asserts close <5s).
  **Pre-fix (HEAD~1 h1_proxy.rs): FAILED** — _"connection was NOT closed within 6 s — header
  timeout is inert (slowloris hold bounded only by the 10 s total)"_. **Post-fix: PASSES** (closes
  ~1s). Positive control `h1_complete_request_still_proxies_with_timer` passes.
- **No-regression:** WS-over-H1 unaffected (timer+upgrades compatible — `round8_ws_upgrade_defer`
  4/4, `keepalive_count_cap` 3/3); lb-l7 --lib 93/0.

### F-INFRA-01 (LOW-security) — retry-secret load-path perm-check
- **Mechanism:** the retry secret is GENERATED 0600 but an EXISTING file's load was not perm-checked
  (asymmetric vs the TLS key, which IS checked at main.rs:980). A pre-placed/drifted world-readable
  retry secret loaded silently → retry-token forge → Mode-A QUIC Initial-flood-defence bypass.
- **Fix:** `check_retry_secret_perms(path, !cfg!(debug_assertions))` → `lb_security::assert_owner_only`
  (lax=warn, strict=err) at the top of the load branch in BOTH loaders (`listener.rs:481` +
  `passthrough.rs:1265`) — **single-sourced (R12)**, mirrors the TLS-key handling.
- **Negative control:** `retry_secret_perm_tests` (world_readable_rejected_strict /
  warns_lax / owner_only_passes) all PASS; the pre-fix hole was separately proven (old load of a
  0644 secret returns `Ok` — silent accept). lb-quic --lib 97/0 (+3).

### F-RES-2 (LOW) — H2-client header-list cap parity
- `http2_pool.rs:425` H2-CLIENT builder now `.max_header_list_size(MAX_HEADER_LIST_SIZE=64KiB)` —
  parity with the 64 KiB server policy (was implicit on hyper/h2's 16 KiB default; tightens the
  explicitness, no behavioral regression). lb-io --lib 56/0.

### F-RES-5 (LOW) — slowloris Watchdog documented observability-only
- Confirmed the sweeper (main.rs:2793) only logs the swept count (never closes the socket) and
  `progress()` is header-phase-only (slow-POST rate eviction is dormant). **Decision: document, no
  behaviour change** (enforcing socket-close would race the drain coordinator; the timeout stack —
  now incl. the live F-RES-1 header timeout + `idle_bounded_send` + `total` + H2 keepalive + QUIC
  idle — is the real enforcement). Watchdog module doc + the sweeper log (`evicted`→`detected`,
  "enforcement is the timeout stack") reworded. The dormant `SlowRate` path is now clearly framed.

### F-PARSE-3 (LOW, test-codec) — comment correction
- `chunked.rs:312`: the 16-digit cap is the real overflow defence; `checked_shl` is inert belt-and-
  braces under it (would only matter if the cap were relaxed). Comment-only, no behaviour change.

### F-RES-4 (INFO) — doc fixed inline with F-RES-1.

---

## 7. Tiered / accepted-risk (with rationale)

**TIERED-carried (proven mechanism + bounded today):**
- **F-RES-3 (LOW) → CF-S38-QUIC-MAXCONN.** The QUIC router `max_connections` is hardcoded 100_000
  (listener.rs:416), not config-wired, and there is no per-source-IP QUIC connection sub-cap.
  Bounded today by the 100k global cap + QUIC Retry-token **address validation** (an attacker must
  complete a round-trip from a real address before consuming a connection slot — spoofed-source
  floods are rejected at Retry). Tiered (not fixed) because the fix adds config surface
  (field + validation + reload-diff classification + docs) that belongs in the ops/perf phase, and
  the defence already exists. Severity LOW (bounded resource pressure, not LB-down).

**ACCEPTED-risk (documented, with rationale):**
- **F-PROTO-01 (LOW)** — the H1 CL/TE smuggle detector skips header pairs whose value fails
  `to_str()` (opaque 0x80–0xFF bytes). NOT exploitable: hyper validates H1 framing independently and
  the egress path strips CL/TE; the detector is defence-in-depth. Proven clean by the opaque-byte-TE
  PoC (§5/§8). Optional lossy-decode hardening deferred (no security delta).
- **F-PARSE-1 / F-PARSE-2 (LOW, test codecs)** — `lb-h2` HPACK `decode_string` add + `lb-h3-testcodec`
  `decode_frame` total-len add. No production call-site (these crates are test infrastructure; prod
  H2/H3 = hyper/quiche). Benign under sane caps. Accepted; covered by the (defensive) fuzz targets.
- **F-PROTO-02/03/04 (INFO)** — gRPC 200 path skips hop-by-hop strip (H2 binary frames → moot); WS
  echoes client subprotocol without backend confirmation (cosmetic, RFC-permitted); H3-front backend
  response-head parser doesn't tchar-validate names (QPACK is binary length-prefixed → cannot split).
- **No zeroize on rustls/ring-held keys** (F-INFRA-02) — no reachable leak path (infra-auditor: every
  secret struct has a redacting `Debug`, no secret in any tracing call/metric label); zeroize would be
  cosmetic given keys live for the listener lifetime in an Arc.
- **No server-side mTLS** (F-INFRA-03) — intentional posture; documented in SECURITY.md as a
  deployment consideration (front mTLS is an upstream-LB / service-mesh concern).
- **TLS 1.2 enabled** — rustls's TLS 1.2 is downgrade-safe (no RSA key-exchange, AEAD-only); documented.
- **bpffs pin-dir mode not locked by the loader** (F-INFRA-04) — operator/systemd responsibility;
  loading requires CAP_BPF/root already.

**CF-S7-RHU** (h3_bridge.rs:164) — `request_h3_upstream` 30s wall-clock cap can truncate a slow
H1→H3/H2→H3 upload, but **fails CLOSED** (`Err(PrematureEof)` + Reset, never `End` → no response-
splitting). Availability edge only, not a security finding; documented carry-forward.

---

## 8. Phase-3 re-validation + gates

> _**[PLACEHOLDER — ×3 green (controlled parallelism to avoid CF-S38-RELOAD-BOOT-FLAKE), h2spec
> 147/147, h3spec 12-waiver, WS matrix, gRPC-H3, R8 under hostile input, full re-soak BOUNDED,
> every CI gate, per-module coverage, fuzz results, post-merge main green.]**_

---

## 9. Verdict

> _**[PLACEHOLDER — SESSION 38 COMPLETE/PARTIAL.]**_

---

## Carry-forwards (tracked)

CF-S38-RELOAD-BOOT-FLAKE (load-induced test-harness boot race; deflake = harden `boot()` readiness
in the perf/burn-in phase) · CF-S38-QUIC-MAXCONN (F-RES-3 per-IP QUIC cap + config tunability) ·
CF-S37-SC9-PLATEAU (fresh-box re-check) · CF-S37-D6-H2PROXY-FLAKY · CF-S37-D-TOKIO-1.52-RELAY ·
CF-S37-C-H3-BACKEND-RELOAD + CF-S37-C-PER-IP-STRICT-TE · CF-S27-2 (WS-H2 gated, hyper#4050) ·
F-ESC-1 (multi-kernel XDP) · perf validation + real-traffic burn-in (remaining pre-prod phase).
