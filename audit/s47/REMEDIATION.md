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

## Tier 2b — XDP / eBPF (all of `ebpf-xdp.md`)

The L4 accelerator is a special case of Tier 3 severe enough to list separately:
the data plane is implemented and the control plane is not, so the feature is not
merely unwired, it is unconfigurable.

| ID | What |
|---|---|
| EBPF-S47-01 | **HIGH.** `conntrack_map`, `conntrack_v6_map`, `acl_trie`, `insert_acl_deny`, `publish_backends_v4`, `set_new_flow_cap`, `install_stats_export` — all defined in `loader.rs`, all with ZERO production callers. |
| EBPF-S47-02 | **HIGH.** Map pinning is unreachable from the binary, despite EBPF-2-05 being closed "Verified-Fixed". |
| EBPF-S47-03 | Every XDP metric is inert, and the shipped default config fires a RUNBOOK alert forever. |
| EBPF-S47-04 | The SYN-flood new-flow rate cap can never fire; its documented fallback is dead code. |
| EBPF-S47-05 | The deny ACL is bypassable by IP fragmentation, and absent entirely on IPv6. |
| EBPF-S47-06 | Partial rewrite on the TCP error path: L3 is mutated before a bounds check that can fail. |
| EBPF-S47-07/08/09 | Nothing rebuilds or validates the committed object against its source; the claimed 5.15/6.1/6.6 verifier matrix is placeholder files; the "proof" tests re-implement the logic they claim to prove instead of running the BPF program. |
| EBPF-S47-13/14/15/16 | UDP checksum of zero emitted as "no checksum"; no TTL/hop-limit decrement on DNAT-forward; `L7_PORTS` byte-order contract disagrees with its ADR; conntrack eviction on an unvalidated RST. |

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

## Fixed in this session (do not re-queue)

Beyond the six in `INDEX.md`: `REL-02` (the shipped systemd unit was `Type=notify`
with no `sd_notify` anywhere — systemd SIGKILLed a healthy gateway every 90 s and
restart-looped forever) and `REL-05` (`ExecReload` sent SIGUSR1, the CERT reload, so
`systemctl reload` applied no config change). Both are packaging-only. The same
commit adds doc-lint tier-1b, which compares the shipped unit against
`DEPLOYMENT.md` directive by directive — the enforcement the unit's own header had
claimed since ROUND8-OPS-07 without it existing.

`REL-01` (admin listener cancelled before `run_drain`, so `/readyz` never serves 503
and `/livez` goes dark mid-drain) is NOT fixed and is a Tier 1 candidate: it defeats
three documented runbook alerts and invites an orchestrator to kill a draining pod.

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

---

## S47-CI-01 — the S46 hang-as-failure guard could never fire (fixed), and it immediately surfaced a CF-S44-class hang (open)

**The guard.** `ci.yml`'s "Fail on a terminated test" step grepped
`(^|[[:space:]])(TIMEOUT|TMT)[[:space:]]*\[` against `nextest-run.log`. The workflow
sets `CARGO_TERM_COLOR: always`, so nextest actually writes:

    \e[31;1m     TIMEOUT\e[0m [1200.024s] (1608/1608) lb-quic::grpc_h3_e2e ...

The reset sequence sits between `TIMEOUT` and ` [`. `[[:space:]]*\[` cannot cross it
(ESC 0x1B is not in `[[:space:]]`), so the pattern could never match a real colorized
run. In run `33450259668` a test was terminated at 1200 s and the step printed **"No
terminated tests."** The job stayed green because the coverage command runs with
`--ignore-run-fail` — which `audit/ship/s46-report.md` already warns swallows a
terminate.

The step's own comment says the pattern "was R13-tested before being trusted (S46)".
It can only have been tested against ANSI-free fixtures. That is the same defect class
as CF-S44 itself: a control validated against an input differing from production in the
one way that mattered.

**Fixed** by normalising SGR sequences out of the log before matching. Proven against
the real captured line: the old pattern MISSES it, the new one FIRES on it, and neither
fires on a `PASS` line (no false red).

**What the working guard found — still open.**
`lb-quic::grpc_h3_e2e grpc_h3_trailer_survives_any_frame_granularity` hit the 1200 s
`terminate-after` in run `33450259668`.

Attribution, stated honestly from one observation:

- The `Test` job — same commit, same suite, no llvm-cov instrumentation — passed all
  1610 tests including this one. So it is instrumentation- and/or load-sensitive, not a
  deterministic failure.
- Run `33449106800` carried the identical H3 pseudo-header change and this test PASSED
  there (334 s, no timeout), so the S47-SMG-01 validator is not implicated by the
  evidence available.
- The one plausible S47-side contributor is the passthrough fixture padding (22 ->
  1200 bytes). It is weak: `BURST = 4096` datagrams is 4.9 MB over loopback, and the
  test binds 4096 sockets, which dominates its cost and was not changed.
- The likeliest reading is a recurrence of the CF-S44-class hang, which
  `.config/nextest.toml` states was never root-caused: *"This is CONTAINMENT, not a
  fix: the CF-S44 mechanism itself is still open (U5)."* The original hang was
  `grpc_h3_without_te_header_still_delivers_trailer` — a sibling in the same file and
  the same suite.

**Consequence to expect:** now that the guard works, a recurrence turns the Coverage
job RED instead of silently green. That is the intended behaviour and should not be
"fixed" by relaxing the guard. Do not re-run and move on — capture
`nextest-run.log` from the failing run, which is already archived by the workflow.


### S47-CI-01 addendum — a concrete lead on the CF-S44 mechanism (NOT yet proven)

Second occurrence, run `33452486817`, with the repaired guard doing its job:

    ##[error]A test was TERMINATED by the nextest timeout — a CF-S44 class HANG, not a flake.
         TIMEOUT [1200.028s] (1608/1608) lb-quic::grpc_h3_e2e grpc_h3_unary_echo_delivers_status_trailer
    ##[error]Process completed with exit code 1

**The victim is different every time, always in the same file:**

| Occasion | Hung test (all `lb-quic::grpc_h3_e2e`) |
| --- | --- |
| CF-S44 (S44/S46) | `grpc_h3_without_te_header_still_delivers_trailer` |
| run 33450259668 | `grpc_h3_trailer_survives_any_frame_granularity` |
| run 33452486817 | `grpc_h3_unary_echo_delivers_status_trailer` |

Three different tests, each terminated at position `(1608/1608)`. A deterministic
regression would hit the SAME test, so this is a property of the suite or something it
shares — and it further clears the S47 H3 validator, which was unchanged between the
two S47 runs that picked different victims.

**The lead.** `crates/lb-quic/tests/grpc_h3_e2e.rs:44`:

```rust
static SUITE_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
```

Its doc comment says it exists so the heavy real-wire tests serialize around a
"process-global `MAX_RETAINED_RESP_BYTES` gauge read", and that it is "Released on
unwind, so a panic cannot deadlock the suite."

**A `static` is per-process, and nextest runs each test in its own process.** So under
`cargo llvm-cov nextest` — the Coverage job — every test gets a private, uncontended
`SUITE_SERIAL` and the suite does not serialize at all. Under `cargo test` (the Test
job) the whole integration target is ONE process with threads, so the mutex does
serialize.

That maps exactly onto the observed split: the suite passes 1610/0 under `cargo test`
on two consecutive runs, and hangs under nextest.

**What is proven vs not.** Proven: the guard now fires; the hang is nextest-specific;
the victim is not fixed; `SUITE_SERIAL` cannot serialize across nextest processes.
NOT proven: that the missing serialization is what causes the hang. It is the strongest
available lead, and it is checkable — run the suite under `-P` with
`test-threads = 1` and `test-groups` serialization configured in
`.config/nextest.toml`, which is nextest's actual mechanism for this, and see whether
the hang disappears.

**Do not "fix" this by relaxing the guard or re-running.** `nextest-run.log` is archived
on every run and is the artifact to compare across occurrences.
