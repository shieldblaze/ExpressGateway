# S47 — remediation plan for the findings NOT yet fixed

Ordered by (exploitability x blast radius), not by severity label. Everything
here has a `file:line` and a reproduction in the linked report; this file is the
work queue, not the evidence.

"LIVE" means the code is reachable from the shipped binary. Several findings are
in library code with no production caller — real defects, but they cannot be
attacked today, so they sit below live ones of nominally higher severity.

---

## Tier 1 — live, remotely reachable, no operator action required

| ID | Report | What | Why it is Tier 1 |
|---|---|---|---|
| H2-03 | `rfc-h2` | Classic `CONNECT` (no `:protocol`) has no arm in `h2_proxy` and falls through to normal proxying, emitting `CONNECT host:443 HTTP/1.1` to the backend. | A reverse proxy that forwards CONNECT is a tunnel primitive. Raised in `audit/protocol/round-1-inventory.md:287` and never closed. Fix: reject non-extended CONNECT with 405 before dispatch. |
| H3-H1 / rt-smuggle §2 | `rfc-h3`, `rt-smuggle` | H3→H1 forwards the client's `content-length` without reconciling it against the actual DATA byte count. | Second desync primitive on the same cell as the CRITICAL already fixed. Fix: count DATA bytes and reject on mismatch before writing the H1 head. |
| RT-DOS-02 / D-01 | `rt-dos`, `divergence` | One client's abort tears down the *shared* upstream H2 connection, killing every other client's in-flight stream on it. | Cross-tenant blast radius from a single well-formed abort. Envoy/Pingora both isolate this. |
| IOP-01 | `cr-io-pool` | `Http2Pool` evicts by ADDRESS, not entry identity — a racing dialer or a stale error tears down a live connection and every request on it. | Same blast radius as above, different trigger; no attacker needed, just concurrency. |
| RT-DOS-01 | `rt-dos` | O(N)-per-packet flow-table scan on unauthenticated, spoofable input. | Quadratic work driven by spoofed UDP. Pairs with the §14.1 gate already landed. |
| BAL-01 | `cr-balancer-health` | The minimum-healthy floor is double-spent by half-open re-ejection: a 2-backend listener can reach ZERO admitted backends. | Ejection IS live. This converts a partial backend outage into a total one — the exact failure `max_ejection_percent` exists to prevent. |

## Tier 2 — live, but need an operator precondition or a specific topology

| ID | Report | What |
|---|---|---|
| CW-03 | `cr-config-wiring` | A TCP/TLS/H1/H1s listener that fails to bind dies silently and the process still reports Ready — traffic is routed to an instance serving nothing. |
| CW-04 | `cr-config-wiring` | `SO_REUSEPORT` / `SO_REUSEADDR` are applied AFTER `bind()`, so both are inert. Breaks zero-downtime restart and multi-process fan-in. |
| IOP-02 | `cr-io-pool` | H3-front → H1-backend has no upstream deadline anywhere and its task is never aborted: a live-but-silent backend leaks a task + fd + QUIC stream permanently. |
| T2 | `rfc-grpc-tls` | The H3 upstream pool dials with a retired `lb-quic` ALPN token — H3 upstreams fail ALPN against conforming backends. |
| WS-01 | `rfc-ws` | The WebSocket upgrade forks above the SNI↔Host (421) choke point, so an upgrade bypasses authority validation that a normal request gets. |
| WS-02 | `rfc-ws` | An established H1 WebSocket session escapes connection accounting — long-lived sessions are invisible to the connection cap. |
| WS-03 | `rfc-ws` | The upstream WS handshake is synthesized; every client header is dropped (auth headers, cookies, subprotocol). |
| BAL-04 | `cr-balancer-health` | Maglev / ring-hash tables are built from an un-canonicalised backend list, so two gateway instances with the same backend SET but different ORDER build different tables. Classic Katran/Maglev fleet-consistency bug; invisible to any single-instance test. |
| H3-H2 | `rfc-h3` | H3→H1 and H3→H2 legs have no deadline and response tasks are never aborted at actor teardown. |

## Tier 3 — unwired controls (fix the wiring or delete the code, but stop documenting it as live)

`cr-config-wiring` holds the authoritative WIRED/UNWIRED table. The pattern
matters more than any single entry: six agents independently found controls that
exist, have passing unit tests, are cited as live, and have no production caller.

| ID | What |
|---|---|
| CW-05 / RT-DOS-03 / H2-04 | The H2 protocol-abuse glitch counter. `with_glitches` has one caller — a test. No config key exists, so `glitches_threshold` is always `None`, all five `record` sites are no-ops, `h2_glitches_total` is never registered, and the ENHANCE_YOUR_CALM drain cannot fire. **`audit/deferred.md:166-180` states it is "fully WIRED" with "the operator knob and Prometheus surface in place".** Found three times independently. |
| H2-12 | `SECURITY.md:49-50` documents SETTINGS-flood (100/10s) and PING-flood (50/10s) limits. Neither detector has a production call site, and neither hyper nor h2 implements such a limit. Both floods are memory-bounded but not rate-limited. |
| BAL-06 | least-connections / least-request / EWMA read a snapshot and atomics that have **no production writer**, so each returns index 0 for every pick. Library-only today (`docs/known-limitations.md` says the algorithms are not config-selectable), which is the only reason this is not Tier 1 — it becomes Tier 1 the moment a policy key is added. |
| BAL-05 | "EWMA" implements no exponentially-weighted moving average at all: no alpha, no time constant, no decay, no aging. |

## Tier 4 — correctness / conformance, low exploitability

Includes: `H2-01` (H2→H1 emits absolute-form request target), `H2-02` (`Host`
never replaced by `:authority`, RFC 9113 §8.3.1 MUST, and duplicate Host reaches
the H1 upstream), `H1-01`..`H1-05`, `H3-M1`..`H3-M6`, `QUIC-02`..`QUIC-08`,
`WS-04`..`WS-15`, `G1`..`G4`, `CW-06`..`CW-10`, `D-03`..`D-06`. See each report.

`QUIC-02` (Mode A rebinds the return path on unauthenticated packets) is rated
HIGH by its agent and is worth pulling forward if Mode A passthrough is deployed;
it is here only because Mode A is not the default listener type.

---

## Cross-cutting work items (not single findings)

1. **Re-verify the audit trail's own claims.** Three separate false premises
   reached operator-facing docs: the S38 TLS-key perm-check contrast (corrected
   in this session), the "all 9 cells proven clean" smuggling claim (which is
   why S47-SMG-01 survived), and the `deferred.md` "fully WIRED" glitch-counter
   entry. A claim in `SECURITY.md` or `audit/**` that a control is live should
   be treated as unverified until a production call path is shown.

2. **Test oracles that cannot fail.** `H3-M5` (both QPACK-bomb tests assert a
   test-only helper dead in production), `H2-11` (all five 1xx tests assert only
   `http::StatusCode` properties and touch no gateway code), `rfc-h2`'s finding
   that `h2_to_h1.rs` is test-only while `SECURITY.md` cites it as the
   downgrade-clean proof. Each should either exercise production or be deleted.

3. **`fuzz/Cargo.toml` pins `quiche 0.28` while the workspace ships `0.29`**
   (LEAD-DEP-4). The QUIC/H3 fuzz targets prove things about a codec that is not
   the one deployed.

4. **Coverage gate.** `h2_proxy.rs` sits at 79.87% against an 80% floor after the
   h2 0.4.19 bump. Attributed precisely: exactly 10 lines (2439-2453, the generic
   upstream-error arm and its F-CAP-1 413/400 classification) lost coverage
   because the newer h2 surfaces that path differently. All 1602 tests pass. Needs
   a test that drives a non-timeout `Http2PoolError`, since the standing rule is
   that no gate gets weakened.
