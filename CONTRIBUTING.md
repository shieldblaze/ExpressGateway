# Contributing to ExpressGateway

Thanks for contributing. ExpressGateway is a production-validated Rust L4
(XDP/eBPF) + L7 HTTP load balancer, and the bar for a change is correctness with
evidence. This guide is the short version; the canonical, detailed setup —
box-per-task requirements, toolchain, system dependencies, soak harness, disk
discipline — is [`docs/arch/DEV-SETUP.md`](docs/arch/DEV-SETUP.md). Read it once;
this page just points at it and states the rules.

## Build, test, run

Build the binary (it is named **`expressgateway`** and takes the config path as a
**positional argument** — there is no `--config` flag):

```bash
cargo build --release -p lb --bin expressgateway
./target/release/expressgateway config/default.toml
```

Run the tests:

```bash
cargo test --workspace                  # the default suite
cargo test --workspace --all-features --no-fail-fast   # the full session gate
```

`--all-features` enables `test-gauges`, which the bounded-memory (R8) integration
tests read; it is off by default. `--no-fail-fast` gives the full failure set
rather than first-fail truncation. On a low-RAM box, cap parallelism
(`CARGO_BUILD_JOBS=4`) or the `--all-features` compile can OOM — see
[`docs/arch/DEV-SETUP.md`](docs/arch/DEV-SETUP.md) for box sizing.

## Run the gates locally

The CI gates are thin wrappers you can run before pushing (mirrors
`.github/workflows/ci.yml`):

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
bash scripts/ci/doc-lint.sh                              # operator-doc + audit-of-audit gate
bash scripts/ci/coverage-check.sh <lcov>                 # per-module hot-path >= 80%
bash scripts/ci/h3spec-check.sh <h3spec> <host> <port>   # h3spec named-waiver gate
```

The full how-to (coverage, the h3spec/h2spec conformance gates, the Docker
smoke, and the release soak) lives in
[`docs/arch/DEV-SETUP.md`](docs/arch/DEV-SETUP.md). The soak and the multi-kernel
XDP matrix need dedicated hardware and are not run on hosted CI.

## The panic-free library rule (non-negotiable)

Every library crate forbids panicking constructs. Outside `#[cfg(test)]`, do
**not** use:

- `.unwrap()` / `.expect()`
- `panic!` / `todo!` / `unimplemented!` / `unreachable!`
- index/slice operations that can panic (`a[i]`, `&buf[..n]`) — use `.get()` /
  `.get_mut()` and `checked_*` arithmetic instead

This is enforced by `#![deny(...)]` at the top of every `crates/*/src/lib.rs`,
plus a CI panic-freedom job and a halting-gate grep — a violation turns CI red.
The release profile is `panic = "abort"`, so an unhandled panic is a hard
outage, which is why these are forbidden by construction rather than caught.
`lb-quic` keeps `indexing_slicing` denied **even in tests**, so use checked
access in `lb-quic` test code too. Background:
[`docs/decisions/ADR-0010-panic-free-enforcement.md`](docs/decisions/ADR-0010-panic-free-enforcement.md).

Public items also require docs (`missing_docs` is denied) — add a `///` comment
on new public types/functions.

## The audit trail (`audit/`)

`audit/` is the program's **permanent, intentional evidence trail** — session
reports, gate outputs, conformance/soak/perf data, security findings. It is kept
wholesale and is referenced by the doc-lint "audit-of-audit" gate. When you
make a non-obvious correctness claim in a PR, cite the evidence under `audit/`
(or add it). Do **not** relocate files out of `audit/`, and do not commit raw
gate-run scratch into it — the committed evidence is the curated report +
markers, not working dirs.

## Pull-request expectations

- **CI green.** fmt, clippy (`-D warnings`), the test suite, doc-lint, coverage,
  and the conformance gates must pass.
- **No AI/Claude attribution** in commit messages or PR bodies.
- **Scope commits explicitly** on a shared tree: `git commit -- <paths>` and
  verify with `git show --stat HEAD` before pushing. Never `git add -A` or
  `git stash` on a shared working tree.
- **Documentation PRs touch only docs.** A docs-only change must not modify
  production source (`crates/**`). It still has to pass doc-lint and keep every
  link resolving.
- **Claims must be grounded.** If you state a behavior, point at the file (and
  line where it helps) or the audit doc that proves it. An aspirational claim
  the code doesn't support is treated as a defect.

## Security

For the security model, threat model, and **responsible disclosure**, see
[`SECURITY.md`](SECURITY.md). Do not file publicly exploitable issues as normal
GitHub issues — follow the disclosure process there.

## Where things live

- [`docs/arch/`](docs/arch/) — developer/internals docs (overview, protocol
  model, QUIC modes, backpressure, security & conformance) +
  [`DEV-SETUP.md`](docs/arch/DEV-SETUP.md).
- [`docs/guide/`](docs/guide/) — operator docs (config, deployment, metrics,
  runbook).
- [`docs/decisions/`](docs/decisions/) — the ADRs.
- [`docs/features.md`](docs/features.md) /
  [`docs/known-limitations.md`](docs/known-limitations.md) — what's supported,
  gated, and waived.
