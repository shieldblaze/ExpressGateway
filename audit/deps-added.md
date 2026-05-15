# Dependencies added in Round 4 prod-readiness work

Per round-4 instructions every new dependency must be justified here.
Pure version bumps and dev-only deps that already appear in the
transitive graph are noted for traceability but do not introduce
new supply-chain surface.

| Crate     | Where                                          | Type        | Version  | Already-transitive? | Justification |
|-----------|------------------------------------------------|-------------|----------|---------------------|---------------|
| `object`  | `crates/lb-l4-xdp/Cargo.toml` (dev-only, Linux)| dev-dep     | `0.36`   | yes (via `aya-obj`) | EBPF-2-01 proof test parses the BPF ELF's section headers to assert `license` / `.BTF` / `.BTF.ext` presence + 64 KiB size budget. `object` is the no_std-friendly ELF parser the plan calls out (alternative was `goblin`; chose `object` because it already lives in the dep graph). Dev-only so it does not ship in release binaries. |
| `tokio-util` | `crates/lb-core/Cargo.toml`                 | dep         | workspace `0.7` | yes (via lb-quic, lb-io, lb-observability, lb) | CODE-2-03: `Shutdown { CancellationToken, TaskTracker }` must live in `lb-core` so every long-lived spawn site across the workspace can `use lb_core::Shutdown`. `default-features = false` + `features = ["rt"]` is the minimum slice covering both types. No new supply-chain surface — tokio-util is already in the graph via lb-quic / lb-io / lb-observability / lb. |
| `tokio`   | `crates/lb-core/Cargo.toml`                    | dep         | workspace `1`   | yes (via every async crate) | CODE-2-03: `Shutdown::drain` uses `tokio::time::timeout`. Already transitive — this only promotes it to a direct dep on lb-core. `dev-dependencies` adds `features = ["full","test-util"]` for the `tokio::test(start_paused = true)` paused-time runtime used in `tests/shutdown.rs`. |
| `proptest` | workspace `[workspace.dependencies]`; dev-dep in lb-h1 / lb-h2 / lb-h3 / lb-quic | dev-dep | `1.x` | no | CODE-2-11: property-based no-panic / round-trip assertions over the H1 / H2 / H3 / QUIC parsers. `default-features = false` + `features = ["std", "fork", "timeout"]` trims out the `proptest-derive` regex generators we do not use. Dev-only so it does not ship in release binaries. Gated behind a per-crate `proptest` Cargo feature so plain `cargo test` stays fast; CI runs `--features proptest` with `PROPTEST_CASES` raised to the audit-gate budget. |
| `loom`    | workspace `[workspace.dependencies]`; dev-dep in lb-balancer | dev-dep | `0.7` | no | CODE-2-11: model-checking concurrency harness. Declared only under `[target.'cfg(loom)'.dev-dependencies]` so it NEVER compiles into normal test builds — invocation requires `RUSTFLAGS="--cfg loom"`. Used in `crates/lb-balancer/tests/loom_atomic_counter.rs` to model the accept-site / scheduler atomic-counter race. |
| `tracing-subscriber` | `crates/lb-observability/Cargo.toml` | dep | workspace `0.3` | yes (via lb-h2, lb-h3, lb-l7, lb-quic, lb) | REL-2-06: central JSON/text subscriber init lives in `lb_observability::log`. Already transitive — this promotes it to a direct dep on lb-observability. Workspace pin already enables `json` + `env-filter` features, no feature widening. |
| `opentelemetry` / `opentelemetry-otlp` / `tracing-opentelemetry` | DEFERRED to Wave 2c | dep | n/a | no | REL-2-07: the plan called for the OTLP exporter to be installed in Wave 2a. We landed the W3C propagation helpers (`lb_observability::tracing_propagation`) + the `OtlpConfig` knob without pulling the OTel crates yet — the L7 wire-up sites that consume the exporter are owned by `proto` (h1/h2/h3 entry points). Adding the heavy OTel crate set ahead of those call sites buys nothing and increases build time / disk pressure. Wave 2c will land the exporter alongside the proto-side header injection.  This row will be filled in when the Cargo.toml edit lands. |
| `arc-swap` | `crates/lb-security/Cargo.toml` | dep | `1` | yes (via `prometheus`) | REL-2-03 cert rotation: `SharedTlsBundle = Arc<ArcSwap<TlsConfigBundle>>` is the lock-free swap surface every TLS listener reads at accept time. The SIGUSR1 handler in `crates/lb/src/main.rs` calls `lb_security::reload_tls_bundle` which `store`s a new bundle Arc; readers holding a previous snapshot keep their snapshot until the connection drops. Already transitive through `prometheus`, so no new supply-chain surface — promoting to a direct dep on lb-security where the type lives. |
| `rustls-pemfile` | `crates/lb-security/Cargo.toml` | dep | workspace `2` | yes (via lb) | REL-2-03 cert rotation: `TlsConfigBundle::load_from_paths` parses cert + key PEM on every reload. The helper now lives next to the data type it produces (lb-security), not at the binary call site, so SIGUSR1 reload and startup share one validated path. |
| `tracing` | `crates/lb-security/Cargo.toml` | dep | workspace `0.1` | yes (via every async crate in workspace) | REL-2-03 cert rotation: the reload helpers in `lb_security::ticket` emit structured tracing events through the same JSON subscriber `lb_observability::log` installs at boot. Already transitive through the lb dep chain. |
| `lb-observability` | `crates/lb-l7/Cargo.toml` | dep | path (`../lb-observability`) | partial (workspace member; lb-l7 had no edge) | ROUND8-OPS-06 / REL-2-07: the W3C trace-context codec (`lb_observability::tracing_propagation`) shipped library-only in `1d462c7` with **zero** L7 callsites — the audit register could not tell "library committed" from "library + callsite committed". This edge gives `crates/lb-l7/src/trace_ctx.rs` the `extract_parent` / `inject_into` / `TraceContext` / `span_name` surface so H1/H2 open a per-request span and inject a child `traceparent` onto the upstream (incl. the WS-upgrade dial). One-directional: `lb-observability` does NOT depend on lb-l7 (no cycle). No new external supply-chain surface — `lb-observability` is an internal workspace crate already compiled by the `lb` binary. |
| `tracing-subscriber` | `crates/lb-l7/Cargo.toml` (dev-only) | dev-dep | workspace `0.3` | yes (via lb-h2, lb-h3, lb-quic, lb) | ROUND8-OPS-06: `tests/round8_traceparent_propagation.rs` installs a custom `tracing_subscriber::Layer` to snapshot the request span's emitted fields and assert the inbound `trace_id` is carried + `http.status_code` is recorded. Dev-only; the workspace pin already enables `json` + `env-filter`, no feature widening. |
| `libc` | `crates/lb-l4-xdp/Cargo.toml` (Linux-only) | dep | workspace `0.2` | yes (via aya, tokio, hyper, every async crate) | ROUND8-L4-11: `bpffs::assert_bpffs` runs `libc::statfs(2)` on the pin directory before handing it to aya, replacing an opaque deep-aya `EbpfError::Map(InvalidPin)` with a typed `XdpLoaderError::PinPathNotBpffs` carrying an operator-actionable mount-command hint. Did NOT add `nix` (which the plan suggested) — libc was already in the workspace; one `statfs` call doesn't justify the larger `nix` surface area. |

## Field meanings

- **Type**: `dep` = production dependency, `dev-dep` = test/bench-only.
- **Already-transitive?**: whether the crate already appeared in
  `cargo tree -p <crate>` before this change. `yes` means no new
  supply-chain surface — we're only pinning a direct view of an
  existing transitive crate.

Owner: `ebpf` for the first row; subsequent rows are owned by the
addition's plan owner.

## ROUND8-OPS-08 — SBOM regeneration provenance (deferred-with-rationale)

`audit/sbom.json` is generated by an in-tree `manual-fallback` tool
(`cargo metadata` -> CycloneDX 1.5 mapping). The Round-8 OPS-08
plan recommended switching to `cargo cyclonedx --format json` so
the SBOM's `metadata.tools[]` field accurately attributes
generation to the official CycloneDX project.

**Decision**: keep the manual-fallback for the current commit;
`cargo cyclonedx` is not installable on the sandbox runner
without network access and the time budget for Round 8 Wave 1
does not include the multi-minute install + regenerate cycle.
The CI gate that runs `cargo install cargo-cyclonedx --locked`,
then `cargo cyclonedx --format json --override-filename
audit/sbom.json`, then `git diff --exit-code -- audit/sbom.json`,
is the contract the follow-up wires in `.github/workflows/ci.yml`
(new `sbom` job).

**Acceptance for closure**: a follow-up commit replaces the
`metadata.tools[].vendor = "manual-fallback"` row with
`"CycloneDX"` and the CI gate above lands. Round-8 doc-lint
tier-2 will not flag this because OPS-08 is recorded here as
deferred-with-rationale rather than via a Verified-Fixed status
in any review file.
