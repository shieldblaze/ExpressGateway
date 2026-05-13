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

## Field meanings

- **Type**: `dep` = production dependency, `dev-dep` = test/bench-only.
- **Already-transitive?**: whether the crate already appeared in
  `cargo tree -p <crate>` before this change. `yes` means no new
  supply-chain surface — we're only pinning a direct view of an
  existing transitive crate.

Owner: `ebpf` for the first row; subsequent rows are owned by the
addition's plan owner.
