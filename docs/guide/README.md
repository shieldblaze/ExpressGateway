# ExpressGateway — User Guide

Public, operator-facing documentation: how to configure, deploy, run, and
operate ExpressGateway. Start at the root [`README.md`](../../README.md) for the
project overview and quickstart, then use the references below.

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

## Scaffolded for Session B

This guide tree is the home for the public-facing **what-is-this / getting-started /
usage / config-walkthrough** documentation. Session B writes those narrative
pages here (e.g. `getting-started.md`, `usage.md`, an Envoy/Traefik-style
"what is ExpressGateway"), linking the reference docs above.
