# ADR-0010: Panic-free enforcement — deny-lints in every crate, CI halting-gate check

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: Cloudflare November 2025 outage post-mortem, Rust Clippy
  documentation, "Fearless FFI" talks, internal incident reviews.

## Context and problem statement

In November 2025, Cloudflare experienced a multi-hour global outage whose
proximate cause was a panic in a Rust component deep inside their
request-handling path. The post-mortem identified the anti-pattern
explicitly: `unwrap()` on a value that was "known" to be present but
wasn't, under an adversarial or malformed input. The panic propagated up
the tokio task tree, the task was silently dropped, and because the
component was on the hot path, the entire load balancer tier failed.

ExpressGateway is the same kind of component, in the same language, in
the same position. If we inherit the same anti-patterns we inherit the
same failure mode. The industry-standard response is to forbid the
panic-producing constructs at compile time, per crate, via Clippy's
`deny`-level lints. But a lint that only lives in one crate's `lib.rs`
can drift: a new crate gets added without the block, a maintainer
silences a warning, a CI job skips the check. The lint must be:

1. **Declared** at the top of every library crate.
2. **Enforced** by CI — no green build without the check.
3. **Enforced** by the halting-gate — not merely a CI job, but a
   scripted precondition on declaring the project "complete".

The lint list must cover every idiom that turns "some future bad input"
into "process dies":

- `clippy::unwrap_used` — `.unwrap()` on `Result` / `Option`.
- `clippy::expect_used` — `.expect("...")` variant of the above.
- `clippy::panic` — explicit `panic!(…)` macro.
- `clippy::indexing_slicing` — `v[i]` when `i` may be out of range.
- `clippy::todo` — `todo!()` stub.
- `clippy::unimplemented` — `unimplemented!()` stub.
- `clippy::unreachable` — `unreachable!()` stub.
- `missing_docs` — bonus: undocumented public items.

## Decision drivers

- Cloudflare's 2025 outage and similar incidents across the industry.
- Rust idioms that *look* safe but panic on bad input: array
  indexing, iterator `.next().unwrap()`, string slicing on a non-ASCII
  boundary.
- The lint must be *structural*, not *cultural*: a new contributor
  cannot accidentally bypass it.
- Test code legitimately needs `unwrap()` for brevity; exempting tests
  only.
- CI must fail loudly on a violation; no "warning" middle ground.
- A single source of truth for the rule so it cannot drift between
  crates.

## Considered options

1. **Policy-only** ("please write panic-free code"), relying on code
   review.
2. **Per-crate `#![deny(...)]`** blocks, enforced only by the compiler.
3. **Per-crate `#![deny(...)]`** *plus* a CI clippy run with
   `-D warnings` *plus* a grep-based halting-gate check for escapes.
4. **Workspace-level lint table** (`[workspace.lints.clippy]` in
   `Cargo.toml`) with a single source of truth.

## Decision outcome

Option 3 — belt-and-braces:

1. Every library crate's `lib.rs` begins with:

        #![deny(
            clippy::unwrap_used,
            clippy::expect_used,
            clippy::panic,
            clippy::indexing_slicing,
            clippy::todo,
            clippy::unimplemented,
            clippy::unreachable,
            missing_docs
        )]
        #![warn(clippy::pedantic, clippy::nursery)]
        #![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

2. CI (`.github/workflows/ci.yml`) runs
   `cargo clippy --all-targets --all-features -- -D warnings`.
3. `scripts/halting-gate.sh` **check 3** uses a panic-pattern grep
   over `crates/` (excluding `tests/` paths and `#[cfg(test)]` modules,
   balanced-brace-aware via awk) that fails on any surviving
   `unwrap()`, `.expect(`, `panic!`, `todo!`, `unimplemented!`, or
   `unreachable!` in non-test code.

Option 4 (`[workspace.lints]`) was considered but rejected: the
per-crate block is self-documenting — a reader opens `lib.rs` and sees
the rule immediately. The redundancy is a feature, not a bug.

## Rationale

- **Compiler-enforced structure**: a `#![deny]` at crate root is an
  error, not a warning; `cargo build` fails. No human gate.
- **Test exemption via `cfg_attr`**: tests can use `.unwrap()` freely
  without weakening production code. Verified in
  `crates/lb-controlplane/src/lib.rs:17` and every other crate's
  `lib.rs`.
- **Halting-gate grep check** catches escapes that slip past clippy
  (e.g. a macro expansion that the lint does not see, or a clippy
  false-negative):

      # scripts/halting-gate.sh, check 3 excerpt:
      panic_hits=$(find crates/ -name '*.rs' -not -path '*/tests/*' … |
        xargs awk '
          /^[[:space:]]*\/\// { next }
          /#\[cfg\(test\)\]/ { in_test=1; depth=0; next }
          in_test && /{/ { depth++; next }
          in_test && /}/ { depth--; if(depth<=0){in_test=0}; next }
          in_test { next }
          /unwrap\(\)|\.expect\(|panic!|todo!|unimplemented!|unreachable!/
            { print FILENAME":"NR": "$0 }
        ')

  The awk state machine balances braces to skip `#[cfg(test)]`
  modules correctly, which a naive grep cannot do.
- **Per-crate redundancy**: checked by `grep -c "deny" crates/*/src/lib.rs`:
  every one of the 17 library crates starts with the same deny block.
  Tested explicitly in the halting-gate by enumeration of required
  artifacts (`manifest/required-artifacts.txt` lists every crate's
  `Cargo.toml` and `src/lib.rs`).
- **`missing_docs`**: a public API without a doc comment is a
  correctness hazard (misuse via unclear semantics is a close cousin
  to panic on bad input). Including it in the deny list raises the
  documentation floor for free.
- **`#![warn(clippy::pedantic, clippy::nursery)]`** — additional Clippy
  tiers are warnings, not errors, because they have higher false-
  positive rates; the CI `-D warnings` escalates them to errors only
  in clippy's view of the code, not at crate-build time.
- **Main binary**: `crates/lb/src/main.rs` also carries the deny block.
  Notably, this is why `main()` is written as

      fn main() -> anyhow::Result<()> {
          let rt = tokio::runtime::Builder::new_multi_thread()
              .enable_all()
              .build()
              .context("failed to build tokio runtime")?;
          rt.block_on(async_main())
      }

  rather than `#[tokio::main]`. The macro expands to an internal
  `.unwrap()` on runtime construction, which violates the lint — see
  the code comment at `crates/lb/src/main.rs:49–53`:

      /// Builds a Tokio runtime manually (avoiding `#[tokio::main]` which
      /// generates an internal `.unwrap()`).

## Consequences

### Positive
- A panic-producing construct cannot enter the `crates/` tree without
  tripping at least two gates (clippy + halting-gate check 3).
- Every crate is self-auditable: the rule is literally at the top of
  the file.
- New crates inherit the rule by convention; adding a crate without the
  block fails halting-gate check 4 (required-artifacts) or, if the
  crate exists but is missing the block, surfaces as a clippy error.
- Error-handling discipline becomes idiomatic: Result-returning
  functions, `thiserror` enums per crate, `anyhow` at the binary
  boundary.

### Negative
- Adding a new fallible construct requires plumbing a new error
  variant; this is more typing than `.unwrap()`.
- Some patterns (e.g. `HashMap::entry(k).or_insert(v)` followed by
  mutation) must be rewritten to avoid `.get_mut(…).unwrap()`.
- Reviewers must know the exemption for `#[cfg(test)]`.

### Neutral
- The policy applies to binaries as well as libraries; one extra file
  (`crates/lb/src/main.rs`) to maintain the block in.

## Implementation notes

- Every library crate's `src/lib.rs` — the `#![deny(...)]` block.
  Verified by inspection across `crates/lb-{balancer, compression,
  config, controlplane, core, cp-client, grpc, h1, h2, h3, health,
  io, l4-xdp, l7, observability, quic, security}/src/lib.rs`.
- `crates/lb/src/main.rs` — binary carries the same block.
- `scripts/halting-gate.sh` — check 3 (panic grep) and check 2
  (clippy).
- `deny.toml` — licence/advisory deny list (adjacent concern;
  enforced by halting-gate check 6).
- `.github/workflows/ci.yml` — clippy run with `-D warnings`.

## Follow-ups / open questions

- Consider adding `clippy::float_arithmetic` to the deny list for
  timing-critical paths (currently not banned).
- Evaluate `clippy::cast_possible_truncation` — already warned by
  `pedantic`, sometimes legitimate with `#[allow]` (see
  `crates/lb-l4-xdp/src/lib.rs:210`).
- Machine-enforce the exemption: verify no `#![allow(clippy::unwrap_used)]`
  exists anywhere outside `#[cfg(test)]` contexts.

## Sources

- Cloudflare November 2025 incident post-mortem.
- Clippy documentation: `unwrap_used`, `expect_used`, `panic`,
  `indexing_slicing`.
- Internal: `scripts/halting-gate.sh` (check 3), every crate's
  `src/lib.rs` header, `crates/lb/src/main.rs`.
