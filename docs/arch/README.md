# ExpressGateway — Architecture & Developer Docs

Developer-facing documentation: internals, design decisions, and the
reference-system research that informed them.

> **Why some docs live one level up (`docs/…`) and not under `docs/arch/`.**
> Several developer references are cited **by path from production source,
> tests, config, and `manifest/required-artifacts.txt`** (e.g.
> `crates/lb-config/src/lib.rs` → `docs/edge-defaults.md`,
> `crates/lb-quic/src/lib.rs` → `docs/decisions/quinn-to-quiche-migration.md`).
> The S40 hygiene pass keeps them at their established paths so production
> source is **not** touched (the session's hard constraint). This index gives
> the clean developer-doc structure without moving the path-coupled files;
> a future session that is willing to touch source comments may consolidate them.

## Architecture & internals

| Doc | What it covers |
|-----|----------------|
| [`../architecture.md`](../architecture.md) | Crate graph, data-plane internals, the L4/L7 split. |
| [`../features.md`](../features.md) | The 9-cell front×back protocol matrix; supported / gated / waived. |
| [`../known-limitations.md`](../known-limitations.md) | Bounded, documented constraints (WS-H2 gating, gRPC-front requirement, named waivers). |
| [`../edge-defaults.md`](../edge-defaults.md) | The edge-default constant table (cross-referenced by `crates/lb-l7` + tests). |
| [`DEV-SETUP.md`](DEV-SETUP.md) | Clone → build → test → run-the-gates-locally → run-a-soak; box requirements per task. |

## Decision records (ADRs)

The architecture decision records live under [`../decisions/`](../decisions/):
ADR-0001..0010 plus `ebpf-toolchain-separation.md` and
`quinn-to-quiche-migration.md`. They capture the io_uring crate choice, the H2
codec strategy, quiche integration, the eBPF framework, BPF map schema, the
frame pipeline, compression crates, the control-plane protocol, graceful reload,
and panic-free enforcement.

## Reference-system research

Background studies that informed the design live under
[`../research/`](../research/): production load balancers (Pingora, Katran,
Envoy, NGINX, HAProxy, Cloudflare LB, AWS ALB/NLB), the protocol RFCs
(9000/9112/9113/9114, HPACK/QPACK), gRPC, compression RFCs, the DoS catalog,
tokio + io_uring, aya eBPF, and the cross-cutting-themes synthesis.

## The evidence trail (`audit/`)

`audit/` is the program's permanent, intentional evidence trail (session reports,
gate outputs, conformance/soak/perf data, security findings). It is **kept
wholesale** — `scripts/ci/doc-lint.sh` tier-2 (audit-of-audit) walks the
`audit/**/round-*-review.md` Verified-Fixed claims, and `audit/coverage-scope.md`
is the coverage charter. Do not relocate files out of `audit/`.
