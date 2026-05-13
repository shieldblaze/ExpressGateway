# Dependencies added in Round 4 prod-readiness work

Per round-4 instructions every new dependency must be justified here.
Pure version bumps and dev-only deps that already appear in the
transitive graph are noted for traceability but do not introduce
new supply-chain surface.

| Crate     | Where                                          | Type        | Version  | Already-transitive? | Justification |
|-----------|------------------------------------------------|-------------|----------|---------------------|---------------|
| `object`  | `crates/lb-l4-xdp/Cargo.toml` (dev-only, Linux)| dev-dep     | `0.36`   | yes (via `aya-obj`) | EBPF-2-01 proof test parses the BPF ELF's section headers to assert `license` / `.BTF` / `.BTF.ext` presence + 64 KiB size budget. `object` is the no_std-friendly ELF parser the plan calls out (alternative was `goblin`; chose `object` because it already lives in the dep graph). Dev-only so it does not ship in release binaries. |

## Field meanings

- **Type**: `dep` = production dependency, `dev-dep` = test/bench-only.
- **Already-transitive?**: whether the crate already appeared in
  `cargo tree -p <crate>` before this change. `yes` means no new
  supply-chain surface — we're only pinning a direct view of an
  existing transitive crate.

Owner: `ebpf` for the first row; subsequent rows are owned by the
addition's plan owner.
