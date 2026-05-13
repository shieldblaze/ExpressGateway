# Round 2 — `proto` Cross-Review

Reviewer: `proto` (HTTP/1.1, HTTP/2, HTTP/3, QUIC, TLS, WS, gRPC).
Files read (all five Round-2 reviews now on disk):

- `audit/CROSS-REVIEW-SYNTHESIS-r1.md` (lead, orientation)
- `audit/security/round-2-findings.md` (`SEC-2-01..16`) +
  `audit/security/round-2-cross-review.md`
- `audit/code/round-2-review.md` (`CODE-2-01..15`) +
  `audit/code/round-2-cross-review.md`
- `audit/ebpf/round-2-review.md` (`EBPF-2-01..09`) +
  `audit/ebpf/round-2-cross-review.md`
- `audit/reliability/round-2-review.md` (`REL-2-01..15`) +
  `audit/reliability/round-2-cross-review.md`
- `audit/protocol/round-2-review.md` (own findings, `PROTO-2-01..15`)

Verdict legend (per audit charter):

- **AGREED** — confirm with one additional line of evidence.
- **DISPUTED** — challenge with counter-evidence; propose alternative.
- **ESCALATE-SEVERITY** — bump severity, justify with concrete
  attack/failure scenario.
- **DOWNGRADE-SEVERITY** — bump down, justify against the proposed
  fix's blast radius.
- **OUT-OF-LANE** — no protocol-correctness cross-cut; ack only.

Findings purely inside another teammate's lane and without a wire-
format / RFC bearing are recorded as **OUT-OF-LANE** with one line.

---

## A. `proto` vs `sec`

### SEC-2-01 ↔ PROTO-2-10 — SmuggleDetector dead on hot path
Verdict: **AGREED** (high). Convergent with `code` CODE-2-01 and lead
T1. The dep-graph fix is `code`'s; the RFC-coverage matrix is mine
(PROTO-2-10). `sec`'s SEC-2-15 derives the hyper-1.9.0 coverage table
independently and reaches the same ~70 % estimate — see §A.7 of this
file for the proto-side delta against `sec`'s table.

Additional evidence: hyper 1.9.0 `proto/h1/role.rs::Server::parse_msg`
drops the `Content-Length` header when both CL and `TE: chunked` are
present and forwards only the chunked encoding to the next layer. The
outbound request synthesized by the gateway therefore loses the
original CL, but the *upstream* may have been issued the original CL
via a separate code path if the gateway is in proxy-as-translator mode
(H2→H1). That is the exact TE-CL desync hyper does not catch and that
`PROTO-2-10` row "TE-final-token != chunked" + SEC-2-15's "TE: gzip,
chunked" both target.

### SEC-2-02 ↔ EBPF-2-03 ↔ REL-2-12 — CONNTRACK / CONNTRACK_V6 are non-LRU
Verdict: **OUT-OF-LANE** (L4 fast-path, no wire-format cross-cut).
Severity high (concur).

### SEC-2-03 ↔ REL-2-02 — Slowloris / SlowPost / TLS-accept slowloris
Verdict: **AGREED** (medium). Wiring fix lives in `lb-l7` (header /
body read loops) — see PROTO-2-03 note: the 100-Continue policy
interacts here because an Expect-100 header parked indefinitely on a
slow upstream amplifies slowloris. The slowloris detector and the
1xx forwarding policy must compose: when the gateway is *holding the
request body waiting for upstream 100*, the slowloris read-side timer
must continue to tick against the client (the client is still slow
even though the proxy is the one stalling).

### SEC-2-04 ↔ REL-2-09 ↔ CODE-2-05 — No per-IP / per-listener cap
Verdict: **OUT-OF-LANE** (lifecycle / accept-loop). Severity high
(concur).

### SEC-2-05 — 0-RTT replay window under unique-token spray
Verdict: **OUT-OF-LANE**, but one protocol note: the
`ZeroRttReplayGuard` only matters once `LB_QUIC_ALPN` is corrected to
`b"h3"` (PROTO-2-02). Until then no real H3 client speaks 0-RTT to
the listener — the QUIC attack surface that SEC-2-05 describes is
theoretical, as `sec` themselves note in their cross-review §G.2.

### SEC-2-06 — Admin HTTP listener has no auth / no TLS
Verdict: **OUT-OF-LANE**, ack medium. Coordinate with REL-2-04 for
the `/livez` / `/readyz` split.

### SEC-2-07 — Supply-chain CI hygiene
Verdict: **OUT-OF-LANE**, ack medium.

### SEC-2-08 — TLS private-key file permissions
Verdict: **OUT-OF-LANE**, ack low.

### SEC-2-09 ↔ EBPF-2-09 ↔ CODE-2-07 — `unsafe impl Pod` padding
Verdict: **OUT-OF-LANE** (XDP / userspace mirror struct). Ack low
today; ack high once control-plane wiring lands.

### SEC-2-10 ↔ REL-2-02 — TLS-handshake slowloris on
`acceptor.accept().await`
Verdict: **AGREED** (medium). Same defense knob as SEC-2-03 from the
TLS side. Proto note: when the TLS-accept wraps in
`tokio::time::timeout`, the timeout cancellation MUST happen *before*
any rustls state is consumed — rustls 0.23 `ServerConnection::process_new_packets`
is not async-cancel-safe by guarantee; the easiest fix is to put the
timeout around the *entire* `acceptor.accept().await` (one future),
not the inner `read_tls` loop.

### SEC-2-11 — XDP `CAP_BPF` probe misses `CAP_SYS_ADMIN` fallback
Verdict: **OUT-OF-LANE**, ack low.

### SEC-2-12 — BPF ELF license / loader license string
Verdict: **OUT-OF-LANE**, ack low (after `ebpf`'s aya-obj default
confirmation in EBPF-2-01).

### SEC-2-13 — 0-RTT on TCP/TLS listener
Verdict: **AGREED** (info, closed-as-not-a-bug). `sec` walked the
`build_server_config` path and confirms `max_early_data_size = 0` —
rel F-19 withdrawn. Proto adds: the `#[cfg(test)]` invariant `sec`
proposes (assert `max_early_data_size == 0`) should also gate on
`rustls::ServerConfig::session_storage` being the default
no-op so the regression check is comprehensive — a future operator
swapping in a session-cache that also enables early data would
otherwise pass the single-field assertion.

### SEC-2-14 — `lb-compression::Decompressor` is unused
Verdict: **OUT-OF-LANE**, ack medium. Cross-ref proto Q-CODE-1-04
(decompression of upstream bodies for inspection is *not* in the
v1 proto scope; the crate is dead-code today and the v1 gateway is
a pass-through). If `code` or `sec` later wires it for gRPC
max-message-size enforcement, proto wants prior review of the
inflation path because RFC 9110 §8.4 has specific layering rules
when `Transfer-Encoding` and `Content-Encoding` interact with a
proxy that does *its own* decompression.

### SEC-2-15 — Hyper 1.9.0 smuggling defense matrix
Verdict: **AGREED**, with one merge-and-one-delta against PROTO-2-10
(see §A.7 below).

`sec`'s row-by-row analysis of hyper 1.9.0 server-side behaviour is
correct. The deltas vs. my PROTO-2-10 matrix:

| Variant | PROTO-2-10 says | SEC-2-15 says | Truth |
|---|---|---|---|
| CL + TE-chunked both present | hyper rejects 400 | hyper drops CL, keeps TE-chunked | **`sec` is correct.** Hyper 1.9.0 `proto/h1/role.rs::Server::parse_msg` drops CL when TE is chunked (RFC 9112 §6.1 allows this); my "rejects with 400" cell was wrong. The *outbound* request the gateway then synthesizes has no CL, but if `H2ToH1Bridge` re-adds CL from `:content-length` pseudo, smuggle. PROTO-2-10 to be corrected in Round 3 plan; severity unchanged (still high). |
| `TE: gzip, chunked` (chunked final) | catches | accepted by hyper | **AGREED — both correct.** Hyper accepts (chunked is the final encoding, which is RFC-compliant); the gap is that an upstream that does not understand gzip-as-content-coding may parse the body as the raw gzip stream and disagree on length. SmuggleDetector catches this; hyper does not. |
| Two CLs same value | catches | hyper coalesces and accepts | **`sec` is correct per RFC 9110 §8.6**; the residual smuggle is "intermediate hop strips one of the two, upstream sees one, gateway saw two but coalesced". SmuggleDetector's `check_duplicate_cl` still adds value because it can be configured to *reject* coalesced duplicates in strict mode. |
| H2 → H1 downgrade with `Transfer-Encoding` re-injected | catches via `check_h2_downgrade` | hyper does not see; gateway must strip | **AGREED.** This is PROTO-2-07's premise (Bridge trait hop-by-hop hygiene). The runtime helper `strip_hop_by_hop` covers the path today, but the trait-level gap is real and the gateway's defense rests on a single call-site discipline. |

Net: PROTO-2-10 high severity stands; the matrix needs one row
correction in Round 3 documentation.

### SEC-2-16 — Atomic-ordering hand-off list for `code`
Verdict: **OUT-OF-LANE**, ack info. No protocol gating atomic in the
proto-owned crates (`lb-h1`, `lb-h2`, `lb-h3`) — the H2 detectors
are plain `&mut self` per-connection structs (single-threaded by
construction); see `code` cross-review §D.

### A.7 — PROTO-2-15 (SNI ↔ Host) — `sec` ESCALATES medium → high
Verdict: **CONCUR with the escalation** (see §G.4 below for the
detailed argument). Proto formally accepts SEC's escalation:
**PROTO-2-15 severity bumped from medium to high** for the
cross-review record. RFC-citation anchor: RFC 6066 §3 (SNI) +
RFC 9110 §7.4 (Host) + RFC 9113 §8.3.1 (`:authority`) — the
client's stated authority is normative at three layers and the
gateway must enforce consistency for any deployment where SNI
participates in routing or in a downstream authorisation key.

---

## B. `proto` vs `code`

### CODE-2-01 — `lb-l7` does not depend on `lb-security`
Verdict: **AGREED** (critical). Joint with PROTO-2-10, SEC-2-01,
SEC-2-03, lead T1. No proto-side dispute on severity or fix shape;
the dep-graph fix is `code`'s lane. Proto provides the per-detector
call-site placement: `SmuggleDetector::check_all` after hyper has
parsed headers and BEFORE any bridge selection — i.e. immediately
after the `parse_request` await in `h1_proxy.rs::proxy_request` and
the matching H2 entry. Reject 400 on positive detection; the
detection must consult the H2-vs-H1 origin so `check_h2_downgrade`
fires on the H2→H1 bridge path only.

### CODE-2-02 ↔ REL-2-15 — `panic = "abort"` not set
Verdict: **AGREED**, **ESCALATE-SEVERITY** concur (rel's high →
code's critical). Proto adds a protocol-side argument:
`quiche::Connection` and `h3::client::Connection` carry partial-frame
state across `await` points (QPACK encoder stream, H3 SETTINGS
exchange). A panic mid-frame in `lb-quic/src/conn_actor.rs` or
`lb-quic/src/h3_bridge.rs` that gets caught by tokio leaves the
quiche state machine in a state where the next user-space call
re-enters with an invalid invariant — quiche then returns
`Error::InvalidState`, which the actor maps to "drop the connection"
without sending `CONNECTION_CLOSE` (PROTO-2-11). The client sees
what looks like packet loss. With `panic = "abort"` the process dies
and systemd brings it back; with unwind the process limps along
silently corrupting peer-visible state.

### CODE-2-03 ↔ REL-2-02 ↔ PROTO-2-11 — SIGTERM is not a drain
Verdict: **AGREED** (critical). Joint owner. Proto's slice is the
GOAWAY / CONNECTION_CLOSE emission (see §E.1 below for the drain
ordering and last-stream-id details).

### CODE-2-04 — Every atomic `Relaxed`
Verdict: **AGREED** (high). Per-detector list `sec` was supposed
to hand back is in SEC-2-16; the H2 detectors (`RapidResetDetector`,
`ContinuationFloodDetector`, `HpackBombDetector`,
`SettingsFloodDetector`, `PingFloodDetector`,
`ZeroWindowStallDetector`) are confirmed by `sec` and proto as
`&mut self` plain structs — no atomic needed. No proto-side
ordering changes required in `lb-h2::security`.

### CODE-2-05 — Unbounded `tokio::spawn` per accept
Verdict: **OUT-OF-LANE** (lifecycle). Severity critical (concur).

### CODE-2-06 — Accept-loop tight-loops on EMFILE
Verdict: **OUT-OF-LANE** (lifecycle). Severity critical (concur).

### CODE-2-07 ↔ EBPF-2-09 ↔ SEC-2-09 — Pod padding
Verdict: **OUT-OF-LANE** (XDP). Severity high (concur with `code`'s
escalation above `ebpf`'s medium).

### CODE-2-08 — Per-CID actor leaks DashMap entries on panic
Verdict: **AGREED** (high). Protocol cross-cut: a `quic_actor`
panic that fails to clean up the router map produces a connection-ID
that *new* QUIC packets cannot route to (mismatched DCID) and that
*existing* in-flight packets cannot drain (channel closed). The
client's behaviour is to retransmit with the same DCID; without
cleanup these retransmits are silently dropped until the connection
idle timeout fires (default 30 s in `lb-quic`). Per RFC 9000 §10.1
the client treats this as transport-level packet loss, not an
application-level signal — and retransmits will continue at the
client's PTO until the connection-level idle timer fires. The
`catch_unwind` wrap `code` recommends is correct; combined with
PROTO-2-11's CONNECTION_CLOSE emission, panics become loud.

### CODE-2-09 ↔ REL-2-11 — `spawn_blocking` for upstream connect
Verdict: **OUT-OF-LANE** (I/O lifecycle), ack high. Proto note: the
preferred alternative (`tokio::net::TcpStream::connect` +
`tokio::time::timeout`) is the right call for proto reasons too —
SIGTERM cancellation can then cut a cold-dial mid-3WHS, which is
required for PROTO-2-11's drain-ordering to actually terminate
in-flight upstream connects within budget.

### CODE-2-10 — `XdpLinkId` drop semantics
Verdict: **OUT-OF-LANE**, ack info.

### CODE-2-11 — Zero proptest / loom / miri
Verdict: **AGREED** (high) with proto-side amendment: the parser
list in CODE-2-11's recommendation §1 must include all of `lb-h1`,
`lb-h2`, `lb-h3`, `lb-grpc`, `lb-quic` (varint, retry-token, header
parse). Proto co-owns the proptest strategies for the codec
crates. The H2 / H3 fuzz corpora today are 5–7 inputs each; a
proptest strategy + a corpus expansion to ~1024 cases per parser
is the Round-3 ask.

### CODE-2-12 — `arc-swap` workspace dep unused
Verdict: **OUT-OF-LANE**, ack low.

### CODE-2-13 — `lb-controlplane` + `lb-health` unused
Verdict: **OUT-OF-LANE** (binary wiring), ack medium.

### CODE-2-14 — Duplicate backend-counter representations
Verdict: **OUT-OF-LANE** (scheduler / balancer scope), ack medium.
Cross-ref Q-CODE-1-07: proto's position is that `lb-balancer` should
be the canonical scheduler and `lb-l7::upstream::RoundRobinUpstreams`
should be deleted in favour of consuming `lb-balancer`. Round-3
plan decision.

### CODE-2-15 — `lb-h1` has no in-workspace consumer outside fuzz
Verdict: **AGREED** (low) and proto provides the answer to
Q-CODE-1-06: prefer **shadow-verify wiring**. `lb-h1`'s
`#![deny(clippy::unwrap_used, clippy::indexing_slicing)]` lint
posture is exactly what a defence-in-depth shadow parser needs;
running it next to hyper and emitting
`http_parser_divergence_total{kind=…}` is the cheapest way to
catch hyper-vs-attacker parser disagreement (the same family of
bugs the smuggling detectors target). The two parsers must NOT
make the routing decision jointly — hyper remains authoritative
on the wire; `lb-h1` is a tripwire only. Round-3 plan to own the
wiring decision.

---

## C. `proto` vs `ebpf`

Confirm: only `EBPF-2-09` (Pod parity) intersects proto-adjacent
work via the `code` constructor fix, and even there the BPF wire
format is unchanged. The remainder of the ebpf findings sit
inside the L4 fast path and the kernel-side data plane and have
no HTTP / QUIC / TLS wire-format implication.

### EBPF-2-01 — BPF ELF license section
Verdict: **OUT-OF-LANE**, ack high.

### EBPF-2-02 — `set_license` does not exist
Verdict: **OUT-OF-LANE**, ack medium.

### EBPF-2-03 — CONNTRACK is `HashMap` not `LRU_HASH`
Verdict: **OUT-OF-LANE**, ack high. No wire-format impact: the
CONNTRACK map carries 5-tuple → backend mapping; flow-spray
attacks degrade *new-connection rate* not *protocol correctness*.

### EBPF-2-04 — SKB-mode hard-coded
Verdict: **OUT-OF-LANE**, ack high.

### EBPF-2-05 — No map pinning
Verdict: **OUT-OF-LANE**, ack high.

### EBPF-2-06 — Dropped `XdpLinkId`
Verdict: **OUT-OF-LANE**, ack low.

### EBPF-2-07 — Verifier-log matrix
Verdict: **OUT-OF-LANE**, ack medium.

### EBPF-2-08 — STATS PerCpuArray never exported
Verdict: **OUT-OF-LANE**, ack medium.

### EBPF-2-09 — Pod parity (BPF-side confirmation)
Verdict: **AGREED**, **confirmation that wire format is unchanged.**

The four structs `FlowKey`, `FlowKeyV6`, `BackendEntry`,
`BackendEntryV6` are kernel BPF-map keys and values; they do not
appear on any network wire. Their layout is private to the XDP
data path. The `code` constructor fix (CODE-2-07) and `sec`'s S-9
hardening do not change any wire byte. The L4 fast-path is below
the gateway's HTTP/TLS layers; there is no protocol-side
regression risk from the constructor refactor.

Joint owner verdict: ebpf confirms, code fixes, sec hardens — proto
signs off that none of the work crosses the HTTP/2/3 wire format
boundary.

---

## D. `proto` vs `rel`

### REL-2-01 — Doc/code drift umbrella
Verdict: **AGREED** (high). Protocol-side drift items (the
`ws_autobahn.rs` stub, missing `h3spec` harness, mislabeled
`conformance_h{1,2,3}.rs`) are all in PROTO-2-04 / 05 / 06; rel's
umbrella correctly owns the doc-rewrite workstream.

### REL-2-02 — SIGTERM is not a drain (critical)
Verdict: **AGREED** (critical). See §E.1 for the GOAWAY error code +
last-stream-id specifics.

### REL-2-03 — TLS cert rotation fictional
Verdict: **AGREED** (critical). Protocol-side note: the cert-rotation
mechanism must use rustls's `ResolvesServerCert` trait, NOT a per-
accept rebuild of `TlsAcceptor`, because per-accept rebuild flips the
`ServerConfig` ALPN protocol list inside the live `Accept` future,
which rustls 0.23 does not guarantee is stable mid-handshake. The
`ArcSwap<Arc<dyn ResolvesServerCert>>` pattern is the correct shape
(rustls reads the resolver once per ClientHello at the right
moment).

### REL-2-04 — `/healthz` unconditional 200
Verdict: **AGREED** (high). Protocol-side: when `/readyz=503`
during drain, the readyz body should include `Connection: close`
on the response so upstream LB probes do not pool the admin
connection. (Minor; rel owns.)

### REL-2-05 — `HealthChecker` / `ConfigManager` dead
Verdict: **AGREED** (high). Protocol-side: when active health
checking lands, the probe protocol matters — H1 GET / vs H2
PING vs gRPC HealthCheck.Check have different failure modes. Proto
should review the probe-type-per-backend decision in Round-3 plan.

### REL-2-06 — Plain-text logs
Verdict: **OUT-OF-LANE**, ack medium.

### REL-2-07 — No distributed tracing / `traceparent` propagation
Verdict: **AGREED** (high) with **escalation-on-the-strip-policy**:
sec's cross-review §F.3 raises the trust-on-first-write concern.
Proto's position is that the W3C `traceparent` header is a
*propagated* header (not hop-by-hop per RFC 9110 §6.6.1), so the
default behaviour (forward verbatim) is RFC-compliant. The
strip-and-mint vs. pass-through decision is a deployment policy
choice; both must be implementable. Default for an *internet-facing*
listener should be strip-and-mint (else trust-context poisoning);
default for an *internal* listener should be pass-through. Round-3
plan should add a per-listener `[listener.*].trace_policy` knob.

### REL-2-08 — Per-listener / per-backend RED labels
Verdict: **OUT-OF-LANE**, ack medium.

### REL-2-09 — Unbounded `tokio::spawn` per accept
Verdict: **OUT-OF-LANE**, ack critical.

### REL-2-10 — `accept(2)` tight-loops on EMFILE
Verdict: **OUT-OF-LANE**, ack critical.

### REL-2-11 — `spawn_blocking` for upstream connect
Verdict: **AGREED** (high), see CODE-2-09 verdict above.

### REL-2-12 — CONNTRACK saturation unobserved
Verdict: **OUT-OF-LANE**, ack high.

### REL-2-13 — STATS map never exported
Verdict: **OUT-OF-LANE**, ack medium.

### REL-2-14 — Binary name mismatch
Verdict: **OUT-OF-LANE**, ack low.

### REL-2-15 — No panic hook; `panic = "unwind"`
Verdict: **AGREED**, **ESCALATE-SEVERITY** concur with `code`'s
critical (above rel's high). Same protocol-side argument as
CODE-2-02: a panic mid-frame in quiche / hyper-h2 leaves the peer
state machine in an unrecoverable place; abort + systemd restart
beats silent corruption.

---

## E. Drain ordering — the multi-team workstream

### E.1 — GOAWAY error code + last-stream-id for PROTO-2-11 / REL-2-02 step (c)
*(Lead asked proto to specify this.)*

**GOAWAY error code for graceful shutdown:**
- **HTTP/2 (RFC 9113 §6.8):** the correct code is `NO_ERROR (0x0)`.
  RFC 9113 §6.8: *"A server that is attempting to gracefully shut
  down a connection SHOULD send an initial GOAWAY frame with the
  last stream identifier set to 2³¹-1 and a NO_ERROR code."* The
  two-step protocol is: first GOAWAY with `last_stream_id = 2^31 - 1`
  (allows in-flight streams to complete and signals "no new
  streams beyond this point"); then after a settle period or when
  the in-flight set drains, a *second* GOAWAY with the actual
  highest processed `last_stream_id` and `NO_ERROR`. RFC explicitly
  prefers `NO_ERROR` over any `SHUTTING_DOWN`-shaped code; in fact
  no `SHUTTING_DOWN` code exists in the HTTP/2 error registry.
  Hyper's `http2::Connection::graceful_shutdown()` does exactly
  this two-step.
- **HTTP/3 (RFC 9114 §5.2):** the corresponding mechanism is the
  H3 `GOAWAY` frame on the control stream carrying a *stream
  identifier* (for client → server, it's a Push ID; for
  server → client, it's the max stream-id the server will
  process). H3 has no error-code in GOAWAY — the error-code lives
  in the QUIC `CONNECTION_CLOSE` frame that follows.
- **QUIC (RFC 9000 §10.2):** `CONNECTION_CLOSE` with application
  error `H3_NO_ERROR = 0x0100` (RFC 9114 §8.1). Quiche's
  `Connection::close(true, 0x100, b"shutdown")` is the exact
  call.

**Last-stream-id calculation for the second GOAWAY (and for H3
GOAWAY):**
- Per RFC 9113 §6.8: the last-stream-id is "the highest-numbered
  stream identifier for which the sender of the GOAWAY frame might
  have taken some action on or might yet take action on". The
  conservative correct value is the highest stream ID the server
  has ever begun processing for *this* connection. Hyper tracks
  this internally; `graceful_shutdown` queries it and emits the
  correct value. The gateway should NOT compute this itself —
  delegate to hyper.
- For H3 the analogue is the highest *request* stream ID accepted.
  Quiche/h3 expose this via the H3 `Connection::goaway_send` /
  `Connection::poll` API; again, do not compute manually.

**Drain ordering — final proto-recommended sequence:**

1. `/readyz` flips to 503 (rel owns). Wait 1 s for upstream LB
   probe (rel's settle window).
2. Per-listener `CancellationToken::cancel()` stops the accept loop
   (code owns).
3. Per H2 connection: `http2::Connection::graceful_shutdown()` (or
   the equivalent in the hyper server builder). This emits GOAWAY
   #1 (`last_stream_id = 2^31 - 1`, `NO_ERROR`) immediately; hyper
   internally waits for in-flight streams to complete or for the
   drain budget to expire and then emits GOAWAY #2 with the real
   last-stream-id.
4. Per H3 connection: send H3 `GOAWAY` on the control stream with
   `max_push_id` / `stream_id` per the H3 direction (RFC 9114
   §5.2); wait for in-flight requests to complete; then call
   `quiche::Connection::close(true, 0x100, b"shutdown")` to
   transmit `CONNECTION_CLOSE` with `H3_NO_ERROR = 0x0100`.
5. Per H1 connection: emit `Connection: close` on the *next*
   response (do not rewrite an in-flight response); after the
   current response completes, the connection closes naturally.
6. Per plain-TCP connection: half-close client→backend
   (`shutdown(SHUT_WR)`); drain backend→client; close.
7. After `[runtime].drain_timeout_ms` (default 10 s, lead-set
   hard cap), unconditional `JoinHandle::abort()` fallback fires.
   Track `shutdown_aborted_connections_total{protocol}` for ops
   visibility.

`ebpf`'s amendment in their cross-review §A-3 / EBPF-2-05 is
incorporated: the userspace XDP inserter must drain *before*
`XdpLoader::drop()` detaches the BPF program. Place this between
step 6 and step 7 (after all L7 listeners have stopped accepting,
but before the abort fallback).

`rel` already prefers `tokio_util::task::TaskTracker` over
`JoinSet` (CODE-2-03 vs REL-2-02 step 3). Proto concurs with
TaskTracker.

---

## F. The Bridge-trait hop-by-hop hygiene question (PROTO-2-07)

### F.1 — Recommendation: option (c), `StrippedRequest` newtype
*(Lead asked proto to record a preference; the choice is deferred
to Round-3 plan per lead synthesis §B.3.)*

`code`'s CODE-2-07 cross-review §C / CODE-2-15 area proposed a
third option I did not list in PROTO-2-07: a newtype
`StrippedRequest` consumed by the `Bridge` trait, where the only
way to construct a `StrippedRequest` is via the runtime helper.
This is **strictly better than my (a) trait-level fix or (b)
documented precondition** for three reasons:

1. **Zero perf cost.** The newtype is a `#[repr(transparent)]`
   wrapper over `Request<Body>`; no allocations, no extra
   `HeaderMap::retain` pass — the runtime helper does the strip
   exactly once and the type-system enforces every bridge
   consumes the stripped form.
2. **Compile-time guarantee.** Option (b)'s "rely on caller
   discipline" failure mode is impossible because the bridge
   signature requires `StrippedRequest` and there is no
   public constructor that bypasses the helper. A future filter
   chain or admin-plane caller that wants to use the bridge
   physically cannot skip the strip step.
3. **Survives refactor.** Option (a) (trait-level fix) leaves the
   strip happening twice on the runtime path — once in the
   helper, once in the bridge — which is observably correct but
   wasteful; a future contributor who notices the redundancy and
   "fixes" one of the two sites can recreate the gap. The
   newtype makes the strip's location load-bearing.

**Proto recommendation for Round-3 plan: option (c), the
`StrippedRequest` newtype.** Concrete shape:

```rust
// in lb-l7/src/util/hop.rs
#[repr(transparent)]
pub struct StrippedRequest<B>(http::Request<B>);

impl<B> StrippedRequest<B> {
    pub(crate) fn from_helper(req: http::Request<B>) -> Self {
        // Single canonical strip: HOP_BY_HOP + Connection-listed names
        // + Trailer-named-as-hop-by-hop (PROTO-2-12 cross-cut).
        strip_hop_by_hop(&mut req);
        Self(req)
    }
    pub fn parts(self) -> (http::request::Parts, B) { self.0.into_parts() }
    pub fn headers(&self) -> &http::HeaderMap { self.0.headers() }
    // … minimal read-only API …
}

// Bridge trait signature changes from:
//   async fn proxy(req: Request<B>, …) -> Response<…>
// to:
//   async fn proxy(req: StrippedRequest<B>, …) -> Response<…>
```

The `pub(crate)` constructor + the `strip_hop_by_hop` helper living
in `lb-l7::util::hop` means no out-of-crate caller can build a
`StrippedRequest` without going through the helper.

Same shape applies to PROTO-2-12's trailer pass-through: a
`StrippedTrailers` newtype enforces the trailer-side strip per
RFC 9110 §6.6.1.

Authoring credit: this is `code`'s idea (CODE-2-07 cross-review
§C / CODE-2-15 area). Proto endorses and recommends it as the
canonical Round-3 plan disposition.

Lead arbitrates in Round 6.

---

## G. Items where proto's findings are escalated by `sec`

### G.1 PROTO-2-09 — `code` and `sec` both ESCALATE medium → high
Verdict: **CONCUR** with the escalation.

Proto's PROTO-2-09 framed the silent fall-through to `PlainTcp` as
medium because the immediate observable effect is "client cannot
connect" (TLS handshake fails). `code`'s CODE-2-15 cross-review §C
and `sec`'s SEC-2-15 / SEC-2-09 area both ESCALATE this to high
with the same argument:

- An operator who writes `protocol = "https"` (typo; should be
  `"h1s"`) silently produces a *plain-TCP forwarder* that strips
  TLS from a listener the operator believed terminated TLS.
- A typo like `protocol = "h1S"` (case mistake) does the same.
- The blast radius is the upstream receiving raw internet bytes
  the operator thought were TLS-terminated. In any deployment
  where the upstream trusts the gateway to have terminated TLS
  (very common — that's the typical mTLS-fronted-gateway shape),
  this exposes the upstream to unauthenticated raw traffic.
- This is a category of operator-error mis-binding that I
  initially under-rated. RFC anchor: none — this is a config-
  validation / safe-defaults question, not an RFC-conformance
  one. The justification is operator-error frequency, not
  spec-deviation.

**Severity: HIGH.** Recommendation unchanged from PROTO-2-09:
hard-error on unknown protocol strings at config-load time,
require `protocol = "plain-tcp"` to be explicit, validate during
`Config::validate` before bind.

`sec` additionally proposes a `[runtime].strict_mode = true`
default that this finding feeds into (sec cross-review §H.1).
Proto supports the strict-mode proposal.

### G.2 PROTO-2-15 — `sec` ESCALATES medium → high
Verdict: **CONCUR** with the escalation.

`sec`'s cross-review §G.4 argues that SNI ↔ Host / `:authority`
disagreement in conjunction with SEC-2-01 (smuggling family) or
PROTO-2-01 (host disagreement family) opens a virtual-host
confusion vector. Proto agrees with the framing.

RFC anchor:
- RFC 6066 §3 (SNI): the SNI carries the *intended* destination
  authority; an intermediary that ignores the disagreement
  cannot make a correct routing decision for any deployment
  where multiple authorities share a listener.
- RFC 9110 §7.4 (Host): the Host header is the request's target
  authority for H1.
- RFC 9113 §8.3.1 (`:authority`): the H2 equivalent.
- RFC 9114 §4.3 (HTTP/3 control fields): the H3 equivalent.

Three different layers carry the "intended authority" signal and
the gateway today consults exactly one (the H1/H2 header). When
the deployment lands SNI-based cert selection (sec / rel both
anticipate this in their Round-2 reviews) the disagreement
becomes a *cert-authority* mis-issuance — the gateway can serve
cert A under SNI `a.example`, then route the request to backend
group B based on `Host: b.example`. Backend B's authorisation
policy may admit a request the gateway's cert posture should
have refused. This is host-confusion at the TLS layer + the HTTP
layer, an attack family that proto's PROTO-2-01 already calls
out for H2 alone.

**Severity: HIGH.** Recommendation unchanged from PROTO-2-15:
extract SNI in TLS-accept, propagate via Extensions, compare
against H1 Host / H2/H3 `:authority`, reject 421 Misdirected
Request on disagreement when listener is non-loopback. Gate
behind `[tls].enforce_sni_authority_match = true`.

Joint owner: proto + sec.

---

## H. Items proto independently escalates

### H.1 PROTO-2-12 (Trailers) — sec adds malicious-upstream smuggle case
Verdict: **PROTO holds at medium**; sec's add (cross-review §G.5) is
correct but the residual severity remains medium because the
attack requires a *malicious upstream* + an H1 client; in any
deployment where the operator controls the upstream, the
smuggle vector is moot.

If the deployment is "we proxy untrusted third-party origins"
(rare but real — CDN-fronting shapes), this finding escalates to
high. Defer the severity gate to Round-3 plan per deployment
profile.

### H.2 PROTO-2-13 — `SETTINGS_ENABLE_CONNECT_PROTOCOL` test
Verdict: low (proto-internal). No cross-team verdict.

### H.3 PROTO-2-14 — TLS 1.2 enabled, no `tls13_only` knob
Verdict: `sec` DOWNGRADES medium → low (cross-review §G.3). Proto
concurs with the downgrade. Recommendation: keep the config knob
addition as a low-priority hardening item; do not gate Round 3
on it.

---

## I. Cross-area severity bumps proto is asking the lead for

| ID | Was | Now | Source of bump |
|---|---|---|---|
| PROTO-2-09 | medium | **high** | `code` + `sec` escalation; proto concurs |
| PROTO-2-15 | medium | **high** | `sec` escalation; proto concurs |
| PROTO-2-14 | medium | **low** | `sec` downgrade; proto concurs |
| REL-2-15 / CODE-2-02 | high (rel) | **critical** (code) | code escalation; proto concurs on protocol grounds (quiche / h2 mid-frame state) |

No other proto-side severity bumps requested.

---

## J. RFC-citation questions for team lead

1. **GOAWAY error code on graceful shutdown.** Confirmed
   `NO_ERROR (0x0)` per RFC 9113 §6.8 for H2; `H3_NO_ERROR (0x0100)`
   in the QUIC `CONNECTION_CLOSE` for H3 per RFC 9114 §8.1. No
   `SHUTTING_DOWN`-named code exists in either registry. Already
   documented in §E.1. No lead decision needed unless lead disagrees.

2. **Last-stream-id calculation.** Delegate to hyper / quiche
   internals — do not compute manually. §E.1. No lead decision
   needed.

3. **Bridge hop-by-hop placement.** Recommend `StrippedRequest`
   newtype (option (c) from `code`'s cross-review). Lead pre-
   approved deferring the choice; this is the proto recommendation
   for the Round-3 plan. §F.1.

4. **PROTO-2-09 RFC anchor.** None — config-validation /
   operator-safety question, not an RFC question. Severity
   justified by operator-error frequency, not by spec deviation.
   §G.1.

5. **PROTO-2-15 RFC anchor.** RFC 6066 §3 + RFC 9110 §7.4 + RFC
   9113 §8.3.1 + RFC 9114 §4.3 — four-layer "intended authority"
   coherence. §G.2.

6. **SmuggleDetector wire-up placement.** Reject 400 in
   `H{1,2}Proxy::proxy_request` immediately after hyper parses
   the request and before bridge selection. §B / CODE-2-01.

---

## K. Verdict roll-up

Convergent findings touching protocol:
- 12 **AGREED** (no severity dispute): SEC-2-01, SEC-2-03, SEC-2-10,
  SEC-2-13, SEC-2-15, CODE-2-01, CODE-2-03, CODE-2-04, CODE-2-08,
  CODE-2-11, CODE-2-15, EBPF-2-09, REL-2-01, REL-2-02, REL-2-03,
  REL-2-04, REL-2-05, REL-2-07, REL-2-11. (counted by *distinct*
  finding IDs across the four other reviews where proto provided
  substantive evidence beyond ack-only)
- 4 **ESCALATE-SEVERITY** (proto concurs with bumps proposed by
  others): PROTO-2-09 (medium → high, from `code` + `sec`);
  PROTO-2-15 (medium → high, from `sec`); CODE-2-02 / REL-2-15
  (high → critical, from `code`).
- 1 **DOWNGRADE-SEVERITY** (proto concurs): PROTO-2-14
  (medium → low, from `sec`).
- 0 **DISPUTED**.
- Remaining items are **OUT-OF-LANE** acknowledgements.

No proto-originated DISPUTE.

---

## L. Items blocking Round-3 plan assignment

None from proto's perspective. Every PROTO-2-NN finding has:
- A concrete location with file:line evidence.
- A documented severity (after cross-review bumps).
- A recommendation (with deferred-choice items in PROTO-2-07 and
  PROTO-2-09 now resolved: PROTO-2-07 → newtype `StrippedRequest`;
  PROTO-2-09 → hard-error on unknown protocol + Config::validate
  enforcement).
- A cross-team owner identified where applicable (CODE-2-01,
  CODE-2-03, REL-2-02 share owners; PROTO-2-11 is joint with
  REL-2-02 step (c)).

Round-3 (planning) is unblocked from the proto side.

— `proto`, Round 2 cross-review.
