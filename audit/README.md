# Production Readiness Audit

Multi-round audit of ExpressGateway (Rust L7 LB + L4 XDP fast path).

## Layout

- `STATE` — current round and phase
- `security/` — owned by **sec** (security-reviewer)
- `code/` — owned by **code** (code-reviewer)
- `ebpf/` — owned by **ebpf** (kernel-ebpf-specialist)
- `reliability/` — owned by **rel** (reliability-engineer)
- `protocol/` — owned by **proto** (protocol-expert)
- `fuzz/` — fuzz corpora + coverage (shared, written only during Round 4+)
- `deferred.md` — accepted-risk register
- `deps-added.md` — dependency-addition justifications
- `unsafe-justifications.md` — every `unsafe` block, justified
- `coverage-scope.md` — modules that must hit ≥80% line coverage
- `sbom.json` — CycloneDX SBOM (final round)
- `FINAL_REPORT.md` — produced only in Round 7

## File ownership (review phases)

Each teammate writes ONLY to its own subdirectory. During fix phases, the
team-lead assigns a unique owner per finding and serializes any findings
that touch the same source file.

## Finding ID convention

`<AREA>-<ROUND>-<NN>` — e.g. `SEC-2-03`, `CODE-2-11`. IDs are stable across
rounds; status fields change.
