# ExpressGateway — User Guide

Public, operator-facing documentation: how to configure, deploy, run, and
operate ExpressGateway. New here? Start with the narrative guides below, then
drop into the reference docs.

## Guides (start here)

| Doc | What it covers |
|-----|----------------|
| [overview.md](overview.md) | **What ExpressGateway is**, the problem it solves, where it fits, headline capabilities and the key limitations up front. |
| [getting-started.md](getting-started.md) | Quickstart: build → pick an example config → run → serve a request → check metrics. |
| [capabilities.md](capabilities.md) | The consolidated supported / gated / waived / deferred matrix — what works and what to know before you rely on it. |
| [comparison.md](comparison.md) | Factual positioning vs Envoy / Traefik / HAProxy / nginx, with honest tradeoffs. |
| [PERFORMANCE.md](PERFORMANCE.md) | The S39 measured performance baseline, with its conditions and caveats. |

## Tasks & recipes

| Doc | What it covers |
|-----|----------------|
| [cookbook.md](cookbook.md) | Complete, annotated configs for named scenarios — production HTTPS + HTTP/2, terminating gRPC, rolling out HTTP/3 via `alt_svc`, DoS hardening, QUIC Mode A — plus a combined-config capstone. |
| [troubleshooting.md](troubleshooting.md) | First-run FAQ + symptom-indexed fixes: config rejected, won't boot, 502-to-everything, gRPC status missing, WebSocket on H2, cert reload, XDP attach. |
| [deployment-patterns.md](deployment-patterns.md) | Topology & scaling: stateless horizontal scale-out behind an external L4/L3 LB, `SO_REUSEPORT` handover, single-node / k8s patterns — and the HA it does **not** provide. |
| [observability.md](observability.md) | What to actually monitor: golden signals, what healthy looks like, a starter Prometheus scrape + alert set. |

## Configuration & operation (reference)

| Doc | What it covers |
|-----|----------------|
| [CONFIG.md](CONFIG.md) | The full TOML configuration schema, reload semantics, worked examples. |
| [DEPLOYMENT.md](DEPLOYMENT.md) | Build-time prerequisites (cmake, clang, kernel floor), the systemd unit, capabilities, sysctls, the XDP toolchain caveat. |
| [RUNBOOK.md](RUNBOOK.md) | Operational procedures, every alert that can fire, the triage matrix, graceful-drain/restart. |
| [METRICS.md](METRICS.md) | Every Prometheus metric family exported, the label-cardinality budget, scrape configuration. |

## Related (root + arch)

- [`../../SECURITY.md`](../../SECURITY.md) — threat model, defenses, audit posture, disclosure policy.
- [`../../CHANGELOG.md`](../../CHANGELOG.md) — release-notes-format changelog.
- [`../features.md`](../features.md) — the front×back protocol matrix + load-balancing reality (supported / gated / waived / deferred).
- [`../known-limitations.md`](../known-limitations.md) — bounded, documented operator-facing constraints (with who-it-affects).
- [`../glossary.md`](../glossary.md) — definitions of the terms used across these docs (Mode A/B, 9-cell, CID, Maglev, …).
- [`../architecture.md`](../architecture.md) — the developer crate map + crate-dependency graph.
- [`../arch/`](../arch/) — developer/architecture documentation (incl. [`extending.md`](../arch/extending.md) — how to extend the codebase).

## Contributing

See the root [`CONTRIBUTING.md`](../../CONTRIBUTING.md) for how to build, test,
and run the gates locally, and [`../arch/DEV-SETUP.md`](../arch/DEV-SETUP.md) for
the detailed per-task box setup.
