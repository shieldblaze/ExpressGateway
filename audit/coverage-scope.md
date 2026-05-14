# Audit — Coverage Scope (Round 7 Gate 7)

Per Round 7 charter, the modules below are designated hot-path code that
must hit **≥80% line coverage** in `cargo llvm-cov` for the production
readiness gate to pass. Coverage runs that exclude these modules are not
acceptable.

## Hot-path modules

| Crate            | Module path                  | Rationale                                        |
| ---------------- | ---------------------------- | ------------------------------------------------ |
| lb-l7            | `h1_proxy`                   | HTTP/1.1 ingress + framing                       |
| lb-l7            | `h2_proxy`                   | HTTP/2 ingress, multiplexed stream handling      |
| lb-l7            | `bridges::*`                 | Cross-protocol bridges (h1<->h2, h1<->h3, …)     |
| lb-l4-xdp        | `loader`                     | XDP program loader + map population              |
| lb-l4-xdp        | `stats_export`               | XDP metrics export path                          |
| lb-balancer      | `*` (all modules in crate)   | Backend selection, EWMA, slow-start, P2C         |
| lb-security      | `hooks`                      | Pre/post hook plumbing                           |
| lb-security      | `conn_gate`                  | Connection admission control                     |
| lb-security      | `watchdog`                   | Slowloris / idle timer enforcement               |
| lb-security      | `ticket`                     | TLS session-ticket rotation logic                |
| lb-security      | `smuggle`                    | Request smuggling defences (CL/TE, h2 downgrade) |
| lb-config        | `validate`                   | Config validation (refuses placeholders)         |
| lb-quic          | `conn_actor`                 | QUIC connection actor                            |
| lb-quic          | `listener`                   | QUIC listener (incl. retry & 0-RTT)              |
| lb-observability | `admin_http`                 | Admin/healthz/metrics surface                    |
| lb-observability | `metrics`                    | Prometheus metric registry                       |

## Coverage target

* Per-module **line coverage ≥ 80%**.
* Per-module **branch coverage ≥ 70%** (best-effort; not all paths are
  reachable via unit harness — see `audit/round-7/deferred-to-ci.md` for
  integration scenarios that close the remaining branches in CI).

## How to run

```sh
cargo llvm-cov --workspace --html --output-dir audit/round-7/coverage-html
```

Then inspect `audit/round-7/coverage-html/index.html` and confirm each
module above is ≥80%. CI emits a JSON summary; the gate fails if any
hot-path module drops below threshold.

## Local sandbox note

`cargo-llvm-cov` requires the `llvm-tools-preview` rustup component and
network access for first-run installs. The sandbox cannot reach the
internet for `cargo install`, so local execution is best-effort; the
authoritative coverage numbers come from CI (see `deferred-to-ci.md`).
