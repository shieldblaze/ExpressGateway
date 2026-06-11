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
- [`../features.md`](../features.md) — the front×back protocol matrix (supported / gated / waived).
- [`../known-limitations.md`](../known-limitations.md) — bounded, documented operator-facing constraints.
- [`../arch/`](../arch/) — developer/architecture documentation.

## Contributing

See the root [`CONTRIBUTING.md`](../../CONTRIBUTING.md) for how to build, test,
and run the gates locally, and [`../arch/DEV-SETUP.md`](../arch/DEV-SETUP.md) for
the detailed per-task box setup.
