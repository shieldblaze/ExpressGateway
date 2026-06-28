# Extending ExpressGateway

A practical orientation for a new engineer making a change: the build/test/gate
dev loop, three worked "how do I add X" maps into the real code, and where the
project's prior reasoning (the audit trail) lives.

This page is the connective tissue. It does **not** restate the canonical setup
and rules — read those once and keep them open:

- [`../../CONTRIBUTING.md`](../../CONTRIBUTING.md) — the rules: panic-free library
  invariant, the audit-trail expectation, PR expectations, commit hygiene.
- [`DEV-SETUP.md`](DEV-SETUP.md) — clone → build → test → run-the-gates → run-a-soak,
  the toolchain, system dependencies, and box sizing per task.
- [`../architecture.md`](../architecture.md) — the crate map and dependency graph
  (which crate owns what).
- [`overview.md`](overview.md) — the request-path narrative and the
  "parsing is delegated" model.

## The dev loop

Build the binary (it is named **`expressgateway`** and takes the config path as a
positional argument — there is no `--config` flag), then run it:

```bash
cargo build --release -p lb --bin expressgateway
./target/release/expressgateway config/default.toml
```

Run the tests — the default suite while iterating, the full session gate before
you push:

```bash
cargo test --workspace                                  # fast inner loop
cargo test --workspace --all-features --no-fail-fast    # the session gate (mirrors CI)
```

`--all-features` enables `test-gauges`, which the bounded-memory (R8) integration
tests read; it is off by default. `--no-fail-fast` gives the full failure set.
On a low-RAM box, cap parallelism (`CARGO_BUILD_JOBS=4`) or the `--all-features`
compile can OOM — see [`DEV-SETUP.md`](DEV-SETUP.md) for box sizing and a shared
`CARGO_TARGET_DIR`.

### The gates, and where they live

CI is a set of thin wrappers you can run locally before pushing. Each gate, what
it checks, and how to run it:

| Gate | Run it locally | What it checks |
|------|----------------|----------------|
| Format | `cargo fmt --all -- --check` | rustfmt is clean |
| Lint + panic-freedom | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | no warnings; the `#![deny(...)]` panic-free lints (no `unwrap`/`expect`/`panic!`/indexing) hold |
| Tests | `cargo test --workspace --all-features --no-fail-fast` | the workspace suite + required-tests manifest |
| Doc-lint | `bash scripts/ci/doc-lint.sh` | operator docs are free of stale patterns; the "audit-of-audit" Verified-Fixed claims resolve |
| Coverage | `bash scripts/ci/coverage-check.sh <lcov>` | per-module hot-path line coverage ≥ 80% |
| h3spec | `bash scripts/ci/h3spec-check.sh <h3spec> <host> <port>` | HTTP/3 conformance against the named-waiver list |
| Container smoke | `IMAGE=expressgateway:ci bash scripts/ci/docker-smoke.sh` | the image builds, boots, and serves a request |

Two gates run in CI without a local wrapper script:

- **h2spec** (HTTP/2 conformance) runs in the `Conformance` job of
  [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml): it stands up a TLS
  listener (ALPN `h2`), downloads a pinned `h2spec`, and runs it `--strict`
  against `127.0.0.1:8443`. To reproduce locally, run a `h1s` listener and point
  `h2spec -t -k -h 127.0.0.1 -p <port> --strict` at it.
- **Panic-freedom** is enforced three ways: the `#![deny(...)]` block at the top
  of every `crates/*/src/lib.rs`, a dedicated CI job, and an `awk` grep in
  `scripts/halting-gate.sh` that scans `crates/` for panicking constructs outside
  `#[cfg(test)]`. The release profile is `panic = "abort"`, so a stray panic is a
  hard outage — which is why the libraries forbid it by construction. Background:
  [`../decisions/ADR-0010-panic-free-enforcement.md`](../decisions/ADR-0010-panic-free-enforcement.md).

The soak and the multi-kernel XDP matrix are **not** run on hosted CI — they need
dedicated hardware. The release soak gate (`scripts/release-soak.sh`) provisions
its own box; see [`DEV-SETUP.md`](DEV-SETUP.md) "Release soak gate".

## How to extend — three worked maps

These are orientation maps into the real code, not copy-paste tutorials. Each
points at the files you will touch and the test that proves the change.

### 1. Add or modify a protocol-bridge cell

The 9-cell HTTP matrix is a `Bridge` per front×back pair. The shape:

- The neutral types and the trait live in `crates/lb-l7/src/lib.rs`:
  `BridgeRequest` / `BridgeResponse` (protocol-neutral: method, URI, a
  `Vec<(String, String)>` header list that may carry `:`-pseudo-headers, a
  `Bytes` body, and trailers), the `Bridge` trait (`bridge_request` /
  `bridge_response` / `source_protocol` / `dest_protocol`), and the
  `create_bridge(source, dest)` factory that maps each `Protocol` pair to its
  cell.
- Each cell is one file, `crates/lb-l7/src/h{1,2,3}_to_h{1,2,3}.rs`, implementing
  `Bridge` for `H?ToH?Bridge`. The request and response transforms are where
  pseudo-header insertion/removal, scheme handling, and trailer threading happen.
- Every cell calls the shared `check_header_count` (the `MAX_HEADERS = 256` cap)
  in both directions — keep that invariant when you edit one.

To **modify** a cell, edit the matching `bridge_request` / `bridge_response`. To
**add** a behavior across all cells, prefer the shared helper in `lib.rs` so you
fix it once (the codebase has been bitten by single-cell fixes that missed the
other eight). The end-to-end proof is the matching integration test
`tests/bridging_h{1,2,3}_h{1,2,3}.rs`, which drives the public `lb_l7` API and
asserts header-cap, method/path/body preservation, and trailer handling — add or
extend the test for your case and confirm with the full suite, not just the one
file.

### 2. Add a backend protocol or type

The backend (upstream) protocol is a small enum plumbed from config to a pool:

- The enum is `UpstreamProto` (`crates/lb-l7/src/upstream.rs`); the binary maps
  config tokens to it in `parse_upstream_proto` (`crates/lb/src/main.rs`) — today
  `"tcp"`/`"h1"` → `Http1`, `"h2"` → `Http2`, `"h3"` → `Http3`, with an unknown
  token failing fast at startup with a clear message.
- The connection itself comes from a pool in `lb-io`: `TcpPool`
  (`crates/lb-io/src/pool.rs`) for H1/raw-TCP backends, `Http2Pool`
  (`http2_pool.rs`) for the hyper h2 client, and `QuicUpstreamPool`
  (`quic_pool.rs`) for the quiche client. Name resolution is `dns.rs`.

A new backend protocol therefore means: extend `UpstreamProto`, accept the token
in `parse_upstream_proto` **and** in `lb_config::validate_config` (so the schema
and the binary agree), add or reuse a pool in `lb-io`, and select that pool on the
upstream leg of the relevant bridge cells. The backend-protocol token is part of
`BackendConfig` in `crates/lb-config/src/lib.rs`.

### 3. Expose a load-balancing algorithm via config

This one is honest groundwork: the algorithms exist, but the selection knob does
**not**. It is a well-bounded contributor task, and the seam is clear.

What is already there:

- All eleven algorithms are implemented in `crates/lb-balancer/src/*.rs`, and the
  enum that names them, `LbPolicy`, lives in `crates/lb-core/src/policy.rs`.
- The live data path is hard-wired: the L7/TCP listeners build round-robin in the
  binary (`RoundRobinUpstreams::new` and `RoundRobin::new` in
  `crates/lb/src/main.rs`), and QUIC Mode A passthrough uses Maglev-by-CID in
  `crates/lb-quic/src/passthrough.rs`.

What is missing (the work):

1. **A config key.** There is no `policy` field on `ListenerConfig` /
   `BackendConfig` in `crates/lb-config/src/lib.rs`, and the schema is
   `#[serde(deny_unknown_fields)]` — so an operator literally cannot add
   `policy = "p2c"` today; it would be rejected at parse. Adding the field (and
   its validation) is step one.
2. **Selecting the picker.** `LbPolicy` is currently referenced nowhere outside
   `lb-core`. The binary would map the configured policy to the matching
   `lb-balancer` picker where it builds `RoundRobinUpstreams` / `RoundRobin`
   today, instead of always constructing round-robin.
3. **Two caveats to handle honestly.** The `ewma` policy needs a per-request
   backend-latency feed that the request path does not record yet (the setter is
   `#[cfg(test)]`-only), and the per-backend `weight` is parsed but ignored by
   the live pickers — a weighted policy must actually consume it.

Document the result in [`../features.md`](../features.md) (the canonical home for
load-balancing reality) and update
[`../known-limitations.md`](../known-limitations.md) when it lands.

## Where the audit trail lives

`audit/` is the program's permanent evidence trail — prior reasoning is recorded
there, not lost. When a comment or report cites a finding, that is where to read
the full story.

- [`../../audit/`](../../audit/) by area: `security/` (the S38 security audit —
  inventory, findings, cross-reviews), `perf/` (the S39 perf baseline + burn-in),
  `soak/` (per-session soak reports + verdicts), `reliability/` (operability
  reviews), `decisions/` (audit-level design decisions), and `release/` (CI/doc
  inventories and session reports). The honest deferred-features list is
  [`../../audit/deferred.md`](../../audit/deferred.md); the executive summary is
  [`../../audit/FINAL_REPORT.md`](../../audit/FINAL_REPORT.md).
- **Finding IDs.** You will see stable IDs in code comments and reports — `F-…`
  for a finding from an audit pass (e.g. `F-RES-1`) and `CF-…` for a carried
  finding tracked across sessions (e.g. `CF-S27-2`). To find the reasoning behind
  one, `grep -r <id> audit/`. When you make a non-obvious correctness claim in a
  PR, cite the evidence under `audit/` (or add it) — an ungrounded claim is
  treated as a defect.
- **ADRs.** The architecture decision records are in
  [`../decisions/`](../decisions/) (ADR-0001…0010 plus the eBPF toolchain split
  and the quinn→quiche migration).
- **The release gate's closed lists.** [`../../manifest/`](../../manifest/) holds
  `required-tests.txt` and `required-artifacts.txt`; `scripts/halting-gate.sh`
  greps the test output against them and they are sha256-locked, so a required
  test cannot silently disappear.

## MSRV and toolchain

- **Rust 1.88** is the MSRV and the pinned channel (`rust-toolchain.toml`). It is
  a **hard** requirement (quiche 0.29.1 + tokio-quiche 0.19). Do not downgrade.
- **BoringSSL build dependencies.** quiche links BoringSSL (via cmake) and
  bindgen needs libclang, so a from-source build needs `cmake`, `clang`,
  `libclang-dev`, `llvm`, `pkg-config`, and `iproute2`. The exact package list,
  the eBPF/nightly toolchains (only needed to *rebuild* the committed XDP ELF),
  and the systemd unit are in [`DEV-SETUP.md`](DEV-SETUP.md).
