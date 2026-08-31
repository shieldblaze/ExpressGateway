# S47 — full-repo RFC conformance + security review

16 parallel Opus agents over ~51k lines of production Rust (18 crates), plus a
lead-side dependency and CI pass. Branch `review/s47-rfc-security`, based on
main @ `01915a77` (CI green, 16/16).

Every agent was briefed to read the ~46 sessions of prior audit evidence FIRST
(`docs/known-limitations.md`, `docs/features.md`, `SECURITY.md`,
`audit/deferred.md`, `audit/**`) and to mark anything already documented as
`ALREADY-KNOWN` rather than re-report it. No agent ran a build: the review box
is 2 vCPU / 7 GB RAM, so all verification runs in GitHub Actions.

## Reports

| Report | Scope |
|---|---|
| `rfc-h1.md` | HTTP/1.1 — RFC 9110 / 9112 |
| `rfc-h2.md` | HTTP/2 + HPACK — RFC 9113 / 7541 |
| `rfc-h3.md` | HTTP/3 + QPACK — RFC 9114 / 9204 |
| `rfc-quic.md` | QUIC transport — RFC 9000 / 9001 / 9002 |
| `rfc-ws.md` | WebSocket — RFC 6455 / 8441 / 9220 |
| `rfc-grpc-tls.md` | gRPC wire spec + TLS RFC 8446 |
| `rt-smuggle.md` | Request smuggling / desync, all 9 bridging cells |
| `rt-dos.md` | DoS, resource exhaustion, algorithmic complexity |
| `rt-unsafe.md` | `unsafe` soundness + panic reachability |
| `rt-crypto-auth.md` | Crypto, secrets, admin/control-plane trust boundary |
| `cr-balancer-health.md` | 12 LB algorithms + outlier ejection |
| `cr-config-wiring.md` | Config, SIGHUP reload, shutdown, binary wiring |
| `cr-io-pool.md` | Connection pooling, upstream IO, DNS |
| `rel-obs.md` | Observability, operability, failure modes |
| `ebpf-xdp.md` | XDP / eBPF L4 datapath |
| `divergence.md` | Divergence vs Pingora / Envoy / HAProxy / Katran |
| `lead-deps.md` | Lead: dependency advisories + the update pipeline |

## Fixed and CI-verified on this branch

| ID | Sev | What |
|---|---|---|
| S47-SMG-01 | **CRITICAL** | H3 `:method`/`:path` spliced unvalidated into a hand-built HTTP/1.1 request line — CRLF request splitting into an H1 backend. Found independently by `rt-smuggle` and `rfc-h3`. |
| S47-QUIC-1 | **HIGH** | No RFC 9000 §14.1 minimum-datagram gate — both QUIC listeners usable as UDP reflectors (11.5x H3-terminate, 5.6x Mode A), plus recv-task starvation. Found by `rfc-quic` and `rt-dos`. |
| LEAD-DEP-1 | **HIGH** | RUSTSEC-2026-0258 — vulnerable `h2` in the production H2 path; the panic it can raise is a whole-process abort under `panic = "abort"`. |
| S47-SEC-1 | MEDIUM | TLS private-key permission gate missing on the SIGUSR1 cert-reload path (present only at startup). |
| LEAD-DEP-2 | LOW | Yanked `chacha20 0.10.0` via `rand`. |
| LEAD-DEP-3 | MEDIUM | Dependabot grouped every update into one PR behind the held `tokio` bump, so security fixes could never land. Split by `applies-to`. |

## Cross-cutting themes

**1. "Implemented but unwired" is this codebase's dominant defect class.** Six
separate agents found controls that exist, have passing unit tests, are cited in
`SECURITY.md` or `audit/deferred.md` as live — and are never called by the
binary. `cr-config-wiring` produced the authoritative WIRED/UNWIRED table.
Notable: the H2 protocol-abuse glitch counter (`CW-05` / `RT-DOS-03` / `H2-04`,
three independent finds) is documented as "fully WIRED" in `audit/deferred.md`
and has no production call site.

**2. Prior audit evidence contains false premises that survived into SECURITY.md.**
`rt-crypto-auth` found S38 justified hardening the retry secret by contrast with
a TLS-key check that did not exist. `rt-smuggle` found the S38 "all 9 cells
proven clean" claim is what allowed the CRITICAL above to survive. `rfc-h2`
found half the cited evidence for "H2→H1 downgrade CLEAN" is dead code. The
lesson the project drew at S46 — *fix the instrument before theorising* —
applies to its own audit trail.

**3. Test oracles that cannot fail.** Multiple conformance and security tests
assert against test-only helpers dead in production (`H3-M5`: both QPACK-bomb
tests), or assert only library types and touch no gateway code (`H2-11`: all
five 1xx tests).

## Not re-reported

Documented and intentional, confirmed by the agents against prior art: gRPC over
an H1 front, library-only LB algorithms, EWMA unfed, WS-over-H2 gated off,
`lb-cp-client` stub-dead, Maglev-for-L4 deferred, the 10 justified `deny.toml`
advisory waivers (the dashmap `IterMut` waiver was re-verified sound — that API
is not used).
