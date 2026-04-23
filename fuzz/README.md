# ExpressGateway fuzz targets

Continuous-fuzzing harnesses for the externally-facing wire protocol
parsers. Each target wraps a single public decode surface and feeds it
arbitrary bytes through [cargo-fuzz](https://rust-fuzz.github.io/book/)
/ libFuzzer. The only finding is a panic or abort inside the parser;
any `Err` return is fine by design.

## Why this crate lives outside the workspace

libFuzzer requires a nightly toolchain (`-Z sanitizer=address` plus the
`libclang_rt.fuzzer` runtime). The main workspace is pinned to stable
1.85 for MSRV, and we don't want the nightly pull to leak into
`cargo build --workspace`. The `fuzz/` crate therefore has its own
`rust-toolchain.toml` (nightly-2026-01-15, matching the eBPF crate so
the repo depends on exactly one nightly snapshot) and an empty
`[workspace]` table that opts it out of the root workspace. It still
references the `lb-*` crates through path dependencies, so any drift
in the fuzzed APIs surfaces immediately.

## Targets

| Target              | Surface                                                      |
|---------------------|--------------------------------------------------------------|
| `h1_parser`         | `lb_h1::parse_headers_with_limit(data, MAX_HEADER_BYTES)`    |
| `h2_frame`          | `lb_h2::decode_frame(data, DEFAULT_MAX_FRAME_SIZE)`          |
| `h3_frame`          | `lb_h3::decode_frame(data, DEFAULT_MAX_PAYLOAD_SIZE)`        |
| `quic_initial`      | `quiche::Header::from_slice(data, quiche::MAX_CONN_ID_LEN)`  |
| `tls_client_hello`  | `rustls::server::Acceptor::read_tls` + `accept`              |

Seed corpora (hand-crafted minimal valid inputs, 5-7 per target) live
under `corpus/<target>/`. They are committed so every run starts from
a warm corpus rather than a single-byte seed.

## Install

```bash
cargo install cargo-fuzz --locked
# The pinned nightly (declared in fuzz/rust-toolchain.toml) auto-installs
# on first invocation; no manual `rustup toolchain install` needed.
```

## Smoke run (2 minutes per target)

Used in CI / pre-merge to catch regressions in the parser surface:

```bash
cd fuzz
cargo fuzz run h1_parser -- -runs=1000 -max_total_time=120
```

Shape for all targets:

```bash
for target in h1_parser h2_frame h3_frame quic_initial tls_client_hello; do
    cargo fuzz run "$target" -- -runs=1000 -max_total_time=120 \
        2>&1 | tee "findings/${target}.smoke.txt"
done
```

Expected: all targets return `Done <N> runs` with exit code 0. Non-zero
exit = crash reproducer written under `fuzz/artifacts/<target>/` —
treat as a real bug, see Triage below.

## Production burn (>=1 h per target, post-ship)

The >=1 h burn per target required by the security review is explicitly
deferred to a post-ship harness because 5+ hours of wall-clock fuzzing
does not fit in a single session's budget, and findings go stale fast.
Run on dedicated hardware (not a dev laptop):

```bash
cd fuzz
for target in h1_parser h2_frame h3_frame quic_initial tls_client_hello; do
    cargo fuzz run "$target" -- -max_total_time=3600 \
        2>&1 | tee "findings/${target}.burn.$(date +%Y%m%d).txt"
done
```

Multi-core burn (one job per physical core):

```bash
cargo fuzz run h1_parser --jobs "$(nproc)" -- -max_total_time=3600
```

## Findings layout

```
fuzz/findings/
  <target>.smoke.txt            # last CI smoke summary
  <target>.burn.<date>.txt      # production burn summary
  <target>.crash.<sha1>         # reproducer bytes (raw binary)
  <target>.crash.md             # triage notes (stack, root cause, fix)
```

## Triage workflow when a target crashes

1. `cargo fuzz run <target> <artifact>` reproduces locally.
2. `cargo fuzz fmt <target> <artifact>` dumps the input in a
   human-readable form when the target uses `Arbitrary`.
3. Capture the stack trace from libFuzzer (it prints the ASan report on
   abort). File the fix as a new commit on the crate with the
   reproducer as a regression test. Do NOT "fix" the crash by tweaking
   the fuzz target — the crash is the finding.
4. After the fix lands, move the reproducer into `corpus/<target>/`
   so subsequent runs re-check the historical crash.

## Adding a new target

1. Create `fuzz_targets/<name>.rs` with the `fuzz_target!(|data: &[u8]| {
   ... })` shape used by the existing targets. No `unwrap`, no
   `expect`, no `panic!` — any panic is a fuzz finding.
2. Add a corresponding `[[bin]]` block to `Cargo.toml`.
3. Commit 3-10 minimal valid seeds to `corpus/<name>/`.
4. Run the 2-minute smoke and commit the summary to
   `findings/<name>.smoke.txt`.
