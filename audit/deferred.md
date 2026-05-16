# Deferred findings — production-readiness audit

Findings recorded here have been triaged out of the Round-3 fix
cadence with the team-lead's pre-emptive acknowledgement. Each entry
states the rationale; the user's final sign-off lands at FINAL.

---

## sec

### SEC-2-13 — 0-RTT on TCP/TLS listener (info)

**Status**: deferred — **closed-as-not-a-bug**.

`build_server_config` in `crates/lb-security/src/ticket.rs:319-338`
never assigns `max_early_data_size`; rustls 0.23.38 defaults this to
`0`, so 0-RTT is disabled by construction on every TCP/TLS listener.
There is no live attack surface today.

**Defence-in-depth follow-up** (will be folded into the SEC-2-01 fix
PR as a one-line additional test, no separate plan required): add
`#[cfg(test)] fn test_zero_rtt_disabled_invariant()` in `ticket.rs`
that builds the default config and asserts `max_early_data_size ==
0`. If a future change enables 0-RTT, this finding **must be
re-opened as critical** because the TCP path has no replay guard.

### SEC-2-15 — Hyper 1.9.0 smuggling-defence reference matrix (info)

**Status**: deferred — **reference material, not actionable on its
own**.

SEC-2-15 documents what hyper catches at the wire-decoder level vs.
what the gateway must guard against above hyper. The actionable
output of this analysis is **already folded into SEC-2-01's plan**
(the strict TE-codec policy and the `Transfer-Encoding: gzip,
chunked` rejection). No separate fix is needed.

### SEC-2-16 — Atomic-ordering hand-off list for `code` (info)

**Status**: deferred — **handed off to code under CODE-2-04**.

Per lead decision in synthesis §E.3, `code` owns the per-site atomic
ordering audit as a single workspace-wide plan; SEC-2-16 is the
input list. No separate sec plan is authored. Sec will review code's
CODE-2-04 plan when it lands.

---

(Other areas append their own deferred sections below.)

---

## proto (Wave-2b-2)

### PROTO-2-15 wiring side — SNI propagation from TLS-accept site (deferred)

**Status**: validator landed Wave-2b-2; **wiring deferred to Wave-2c**.

`crates/lb-l7/src/sni_authority.rs::check_sni_authority` + the 421
Misdirected Request renderer (`misdirected_response`) ship Wave-2b-2
with full unit-test coverage (`crates/lb-l7/tests/sni_authority_mismatch.rs`
— 7 tests pass). Threading the captured SNI from the
`tokio_rustls::TlsAcceptor::accept` future down into
`H1Proxy::serve_connection` / `H2Proxy::serve_connection`
requires a handler change in `crates/lb/src/main.rs` (the TLS-accept
site is built there). Wave-2b is forbidden from touching `main.rs`;
Wave-2c will:

  1. Capture `acceptor.accept(stream).get_ref().1.server_name()`
     from the rustls handshake result (or the
     `Acceptor::accept`-path equivalent) at the binary's TLS
     handler.
  2. Pass the captured SNI (`Option<String>`) into a new
     `with_sni: Option<String>` builder on each `H{1,2}Proxy`
     instance, or surface it via a request-extension propagated
     through the `serve_connection` IO wrapper.
  3. Inside `H1Proxy::handle` / `H2Proxy::handle`, after the
     PROTO-2-01 `check_authority_host_agreement` block, call
     `lb_l7::sni_authority::check_sni_authority(sni.as_deref(),
     authority_str)` and on `Err(_)` return
     `misdirected_response()` rendered as `Response<BoxBody<…>>`.

### PROTO-2-12 — trailer pass-through across cross-protocol bridges (Round-4 follow-on; H3 leg deferred)

**Status**: Round-4-Wave-2c follow-on lands the bridge-surface fix
for the H1↔H2 / H2↔H2 paths; **H3 leg of every cross-bridge
remains deferred** because `lb-quic::H3Request` /
`lb-quic::H3UpstreamResponse` carry no trailer field.

Landed in this round:

  1. `BridgeRequest` / `BridgeResponse` (`crates/lb-l7/src/lib.rs`)
     each grew a `trailers: Vec<(String, String)>` field with a
     `Default` impl.
  2. All 9 bridge impls (`crates/lb-l7/src/h{1,2,3}_to_h{1,2,3}.rs`)
     forward the trailer list end-to-end.
  3. The H1 / H2 hot-path translation helpers (`h1_proxy::
     translate_h1_request_to_h2`, `h1_proxy::upstream_response_to_h1`,
     `h2_proxy::translate_h2_request_to_h2`, `h2_proxy::
     upstream_h2_response_to_h2`) capture trailers via
     `Collected::trailers()` at body-collect time and re-emit them
     via `StreamBody` + `Frame::trailers(HeaderMap)` (new helpers
     `build_body_with_trailers` / `build_h2_body_with_trailers`).
  4. `crates/lb-l7/tests/trailer_passthrough.rs` flipped from
     baseline-pinning to positive assertions: two suite tests
     iterate every (src, dst) pair and assert request / response
     trailers survive `bridge_request` / `bridge_response`.

Deferred — **H3 cross-bridge trailers**: `H3Request` /
`H3UpstreamResponse` in `lb-quic::h3_bridge` don't carry trailer
fields, so `collect_h1_request_to_h3_fieldlist`,
`collect_h2_request_to_h3_fieldlist`, `h3_response_to_h1`, and
`h3_response_to_h2` ship `trailers: Vec::new()` even though the
H1/H2 leg of the bridge is plumbed. Round-5 ticket: add a `trailers:
Vec<(String, String)>` field to `H3Request` / `H3UpstreamResponse`,
emit the matching `Frame::trailers` from the H3 client codec, and
flip the H3 leg in the proxy hot-path calls to forward
`translated.trailers`.

### PROTO-2-03 — explicit 1xx / 103 Early Hints forwarding (deferred)

**Status**: baseline pinned Wave-2b-2; **forwarding fix deferred to
Wave-2c**.

Investigation: hyper 1.9.0's H1 server auto-handles `Expect:
100-continue` transparently at the wire level, but `client::conn::http1::send_request().await`
resolves on the first non-1xx response — so 103 Early Hints frames
from the upstream are silently dropped. RFC 9110 §15.2 / RFC 8297
say MAY forward; production CDNs (Cloudflare, Fastly) forward.

Wave-2c will install an `OnInformational` callback on hyper's H1
client (and the equivalent stream-frame loop on the H2 / H3 paths)
to forward 1xx frames through to the inbound client.

`crates/lb-l7/tests/informational_responses.rs` (5 tests) pins the
status-class invariants today; Wave-2c will extend them to assert
the wire-level forwarding.

### PROTO-2-04 / PROTO-2-05 — wstest + h3spec integration (deferred to Round-7 gate-matrix CI image)

Both require CI image changes (installing `wstest`, `h3spec`); they
move with the rel-team CI image work. Round-4 follow-on confirms no
code change is required for these — they are pure CI/infra plumbing
and remain deferred until the gate-matrix work in Round 7 picks up
the conformance suites.

### PROTO-2-09 / PROTO-2-11 (H2 half) — `build_listener_mode` strict-protocol-validation, GOAWAY-on-SIGTERM

Both live in `crates/lb/src/main.rs` (forbidden to Wave-2b). Move
with Wave-2c.

## L7 (Round 8)

### ROUND8-L7-08 — Upstream H2 RST_STREAM(CANCEL) on application read timeout (deferred-with-rationale)

**Status**: deferred per lead-decision `R8-L-002` in
`audit/round-8/LEAD-DECISIONS.md`. hyper 1.x's `SendRequest` does
not expose an explicit `send_reset(CANCEL)` API; the practical
mitigations available today (drop-emits-CANCEL on future-drop;
eviction from the pool on timeout) are already wired in
`crates/lb-io/src/http2_pool.rs:206-209`. The Pingora 0.8.0 fix
shape (explicit CANCEL with reason context) requires the hyper-2.x
upgrade. Re-open when the hyper-2.x rebase lands.

### ROUND8-L7-07 FrameRecvTimeout timer sub-part — per-frame arrival watchdog

**Status**: partial — ONLY the `GlitchKind::FrameRecvTimeout`
*timer* sub-part is deferred. The COUNTER half (the actual HAProxy
`tune.h2.fe.glitches-threshold` pattern) is now fully WIRED:
`H2Proxy::with_glitches` creates one `GlitchesCounter` per H2
connection (`crates/lb-l7/src/h2_proxy.rs`,
`serve_connection_with_cancel_sni` → `GlitchConnState`); every H2
protocol-abuse event (underscore-policy reject, smuggle reject,
malformed authority [ROUND8-L7-09], `:authority`/Host disagreement,
SNI mismatch) records a weighted glitch, bumps `h2_glitches_total`,
and on threshold-crossing cancels the connection drain token →
the existing two-step GOAWAY path (logical ENHANCE_YOUR_CALM).
Proof: `crates/lb-l7/tests/round8_glitches_enforced.rs` (1/1 PASS —
drives abuse requests, asserts the connection drains at the
threshold and `h2_glitches_total` is non-zero).

What remains deferred is strictly the `tokio::time::Interval`
frame-arrival watchdog that would `record(FrameRecvTimeout, ..)`
on a stale inbound H2 frame. That requires reaching into hyper's
per-connection read context, which is not exposed in hyper 1.x's
`http2::Builder::serve_connection` surface (hyper pinned at 1.9.0,
`Cargo.lock`). The keep-alive PING + timeout already wired in
`H2SecurityThresholds::{keep_alive_interval, keep_alive_timeout}`
provides partial coverage for the holding-HEADERS-open slowloris
class (server-initiated PING, not gated on inbound frame
progress). The per-frame timer moves with the hyper-2.x upgrade;
the consolidated counter it would feed is already live, so the
operator knob and Prometheus surface are in place ahead of it.

### ROUND8-L4-08 — Fragmented datagrams: pass-to-kernel, no in-XDP reassembly

**Status**: by design, declared explicit per ROUND8-L4-08.

ExpressGateway XDP path does not reassemble fragments. IPv4 packets
with `MF=1` or fragment-offset `>0` (mask `0x3FFF` over the
big-endian `frag_off` field), and any IPv6 packet carrying a
Fragment Extension Header (`IPPROTO_FRAGMENT = 44`) take the
`XDP_PASS` branch with `STAT_V4_FRAGMENT` / `STAT_V6_FRAGMENT`
incremented. The kernel's network stack reassembles via the normal
path.

This matches Katran's documented design (lessons 2-3) and Cilium's
documented non-goal. Operators relying on L4 load-balancing for
fragmented flows should either raise the upstream MTU so
fragmentation does not occur on the wire, or accept the kernel-
stack latency for the small fraction of flows that fragment.

Counters: `xdp_packets_total{result="v4_fragment"|"v6_fragment"}`.

### ROUND8-L4-04 — Maglev consistent-hash backend selection in the XDP data plane

**Status**: deferred to Pillar 4b-3, with the atomicity guarantee
landed early (ROUND8-L4-04 Proposed-Fix).

ROUND8-L4-04 lands the load-bearing half of the Unimog / l4drop D1
lesson NOW: the `backends_v4` BPF map + `BackendTable` value layout
and `XdpLoader::publish_backends_v4` — a SINGLE `bpf_map_update_elem`
per VIP so a concurrent data-plane lookup never observes a
half-populated table, plus the Unimog lesson-3 daisy-chain
(`previous_entries`/`previous_count`) so in-flight flows during a
swap reach the previous backend. The atomic-publish + daisy-chain
algorithm is unit-tested (`tests/round8_atomic_backends.rs`).

Deferred to Pillar 4b-3: the verifier-heavy *data-plane read* side —
per-packet `BACKENDS_V4[vip]` lookup + consistent-hash
`entries[hash(5-tuple) % count]` selection + CT-remembered-generation
vs. current-generation compare driving the daisy-chain fallback.
Wiring it now forces a verifier-log re-capture for a code path no
production flow exercises yet (backend selection is still
control-plane-driven via CONNTRACK inserts). The eBPF program holds
a behaviorally-inert `backend_table_published(vip)` touch on the
CT-miss path so the map + BTF survive bpf-linker DCE and the
atomic-publication visibility is provable end-to-end; the
Pillar-4b-3 selection logic slots in at exactly that call site.

This also subsumes the long-standing "Maglev consistent hashing"
follow-up: the `MAX_BACKENDS_PER_VIP = 64` ceiling is the
verifier-tractable floor; a Maglev table larger than that, or
weighted backends, is the Pillar-4b-3 scope.

## D-5 container scan — CVE-2026-0861 (glibc) WAIVER (2026-05-16)

`.trivyignore` waives CVE-2026-0861 (libc6 2.36-9+deb12u13, Debian 12.13
distroless base). Reason: `FixedVersion=<none>`, Debian has not shipped
a patched libc6 — unremediable at the image layer. This is a documented
exception, NOT a code fix. RE-REVIEW: drop the `.trivyignore` line and
rebuild once `trivy image` shows a non-empty FixedVersion for this CVE.

## D-1 native XDP on ENA — fixes + remaining gap (2026-05-16)

FIXED (root cause of original Round-8 D-1/D-2 FAIL):
  * scripts/build-xdp.sh: removed broken `--target bpfel-unknown-none`
    from `rustup toolchain install` (no prebuilt std; `-Z build-std=core`
    handles core) — this is what silently skipped the ELF build.
  * scripts/build-xdp.sh: dropped `-Clink-arg=-g` (bpf-linker 0.10.3
    rejects `-g`; BTF comes from `-Cdebuginfo=2` + `--btf`).
  * scripts/build-xdp.sh: added post-build `llvm-objcopy --strip-debug`
    (keeps xdp/license/maps/.BTF) — ELF 181912 -> 35864 B, back under
    MAX_ELF_BYTES so build.rs re-embeds it.
  Result: rebuilt lb_xdp.bin HAS the `license` section; the production
  aya loader loads it and the kernel verifier ACCEPTS it (proven: skb
  attach on a dummy netdev succeeded). This resolves D-2's verifier
  question on this kernel via the production path.

ENA native-XDP DEPLOYMENT PREREQUISITES (discovered, must be in RUNBOOK):
  * ENA max XDP MTU is 3498; the AWS jumbo default (9001) must be
    lowered (`ip link set dev <if> mtu 3498`) or native attach fails:
    "current MTU larger than maximum allowed MTU".
  * ENA requires combined channel count <= half the max for native
    XDP (it allocates equal dedicated XDP TX queues):
    `ethtool -L <if> combined <=N/2>`. 8/8 fails; 4 works.

REMAINING GAP (not closed here):
  * With ELF fixed + MTU + channels corrected, legacy netlink DRV
    attach works on ENA, but the production loader's aya-0.13.1
    `bpf_link`-based DRV attach still fails ("bpf_link_create failed").
    The loader needs a netlink-XDP attach fallback when bpf_link is
    refused by a driver that supports netlink XDP. This is real loader
    work + requires the 3-kernel matrix (CI) to validate. D-1 native
    via the production code path therefore remains OPEN.

D-2 bpftool literal path: aya-ebpf 0.1 emits a legacy `maps` section
that libbpf-1.0+ (bpftool, `ip link obj`) refuses. Closing the literal
bpftool path needs an aya-ebpf major bump (BTF `.maps`) + matrix
re-validation. Verifier acceptance itself is proven via the loader.
