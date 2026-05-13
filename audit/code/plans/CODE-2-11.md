# Plan for CODE-2-11 — proptest + loom + miri bootstrap (workspace)
Finding-ref:     CODE-2-11 (high, Open)
Files touched:
  - `Cargo.toml`                                    (workspace `[workspace.dev-dependencies]`)
  - `crates/lb-h1/Cargo.toml` + `crates/lb-h1/tests/proptest_parser.rs`
  - `crates/lb-h2/Cargo.toml` + `crates/lb-h2/tests/proptest_hpack.rs`
  - `crates/lb-h2/tests/proptest_settings.rs`, `proptest_frames.rs`
  - `crates/lb-h3/Cargo.toml` + `crates/lb-h3/tests/proptest_qpack.rs`
  - `crates/lb-h3/tests/proptest_varint.rs`
  - `crates/lb-quic/Cargo.toml` + `crates/lb-quic/tests/proptest_retry_token.rs`
  - `crates/lb-quic/tests/proptest_header.rs`, `loom_router.rs`
  - `crates/lb-compression/Cargo.toml` + `crates/lb-compression/tests/proptest_decompress.rs`
  - `crates/lb-grpc/tests/proptest_frame.rs` + `proptest_deadline.rs`
  - `crates/lb-l7/tests/proptest_ws_frame.rs`
  - `crates/lb-core/tests/loom_backend.rs`
  - `crates/lb-io/tests/loom_pool.rs`
  - `crates/lb-h2/tests/loom_rapid_reset.rs`
  - `crates/lb-l4-xdp/tests/pod_layout.rs`           (from CODE-2-07)
  - `crates/lb-io/tests/miri_ring.rs`
  - `.github/workflows/ci.yml`                       (3 new jobs)

Approach:
Three orthogonal axes: proptest (input-space coverage), loom
(concurrency model coverage), miri (UB coverage). Each targets a
specific risk surface. CI gates added per-job.

### Proptest harnesses (≥100 000 cases per parser per audit gate)

Per-parser grammar generators and round-trip / no-panic assertions:

| Crate | Test | Input grammar | Budget |
|---|---|---|---|
| `lb-h1` | `tests/proptest_parser.rs` | Request-line + headers + chunked body. `Strategy = (method ∈ closed-set, target ∈ utf8_chunk(0..256), version ∈ "HTTP/1.1"\|"HTTP/1.0", headers ∈ vec(0..32, (header-name, header-value)), chunked ∈ vec(0..16, hex-size + bytes))`. | 200 000 cases |
| `lb-h2` | `tests/proptest_hpack.rs` | HPACK encode → decode round-trip + literal/indexed/never-indexed mix. Strategy generates a `Vec<(HeaderName, HeaderValue, IndexHint)>`. | 200 000 |
| `lb-h2` | `tests/proptest_settings.rs` | SETTINGS frame fuzz with valid & invalid identifiers; assert no panic. | 100 000 |
| `lb-h2` | `tests/proptest_frames.rs` | Frame-shape strategy: HEADERS / DATA / RST_STREAM / PRIORITY / WINDOW_UPDATE / PING / GOAWAY / PUSH_PROMISE / CONTINUATION sequences with stream IDs in 1..=65535. | 100 000 |
| `lb-h3` | `tests/proptest_qpack.rs` | QPACK static-table + dynamic-table interleavings; round-trip. | 200 000 |
| `lb-h3` | `tests/proptest_varint.rs` | QUIC varint encoding (1/2/4/8 byte). Round-trip; reject overlong forms. | 100 000 |
| `lb-quic` | `tests/proptest_retry_token.rs` | RFC 9000 retry-token encode/decode + AEAD tag. | 100 000 |
| `lb-quic` | `tests/proptest_header.rs` | Long / short header parse — CID length, version, packet type. | 100 000 |
| `lb-compression` | `tests/proptest_decompress.rs` | (gzip‖deflate‖brotli) body up to bomb-cap; assert size cap enforced, no panic. | 100 000 |
| `lb-grpc` | `tests/proptest_frame.rs` | gRPC length-prefixed frame: compressed-flag, length, payload. | 100 000 |
| `lb-grpc` | `tests/proptest_deadline.rs` | `grpc-timeout` header strategy: `\d+[HMSmun]`. | 100 000 |
| `lb-l7` | `tests/proptest_ws_frame.rs` | WS frame fuzz: opcode, mask bit, payload-length forms (7 / 7+16 / 7+64). | 100 000 |

Each proptest declares `#![proptest_config(ProptestConfig { cases: NNN, max_global_rejects: NNN/10, .. })]`. Two invariants per generator:
1. **Round-trip** where applicable (parse(serialize(x)) == x).
2. **No panic / no unwrap_used** (`std::panic::catch_unwind` wrap;
   assert `Ok(_)`).

### Loom harnesses (concurrency model)

Three crates host loom tests. Cargo conditional dev-dep:
```toml
[target.'cfg(loom)'.dev-dependencies]
loom = "0.7"
```
Tests live in `tests/loom_*.rs` with `#![cfg(loom)]` at the top.

| Crate | Test | What it models |
|---|---|---|
| `lb-core` | `tests/loom_backend.rs` | Two threads: T1 `fetch_add(active_connections)` + scheduler observes, T2 saturating-decrement CAS at line 98–110. Asserts no underflow + scheduler-visible publication ordering. |
| `lb-io` | `tests/loom_pool.rs` | T1 increments `total` + pushes to queue; T2 reads `total.load(Acquire)` + reads queue. Asserts queue.len() ≥ total - cap. (gate semantics under CODE-2-04). |
| `lb-h2` | `tests/loom_rapid_reset.rs` | T1 increments reset accumulator; T2 reads accumulator vs threshold; assert no missed-publication of the byte that pushed over the line. |
| `lb-quic` | `tests/loom_router.rs` | T1 reaper sweep; T2 panicking actor. Asserts DashMap remove idempotency (paired with CODE-2-08). |

CI job: `cargo test --release --target x86_64-unknown-linux-gnu --cfg loom -- --test-threads=1`.
Each loom test caps `loom::model` exploration: `LOOM_MAX_PREEMPTIONS=3 LOOM_MAX_BRANCHES=4096`.

### Miri targets (UB coverage)

| Crate | Test | Why |
|---|---|---|
| `lb-l4-xdp` | `tests/pod_layout.rs` | Pod transmute round-trip; padding zeroed; size constants. (Plan from CODE-2-07.) |
| `lb-io` | `tests/miri_ring.rs` | ring-buffer head/tail wrap, slice-from-raw-parts validity — gated by `#[cfg(miri)]` so io_uring syscalls are bypassed; only the in-memory bookkeeping is exercised. |

CI job: `cargo +nightly miri test -p lb-l4-xdp -p lb-io --tests --
  -Zmiri-disable-isolation` (disable isolation needed for `Instant::now`).

### CI integration

Three jobs added to `.github/workflows/ci.yml`:
1. `proptest`: nightly runs the full 100k-cases budget; PR runs a
   reduced 5k-cases budget (env var `PROPTEST_CASES=5000`).
2. `loom`: `RUSTFLAGS="--cfg loom" cargo test --release` on every PR.
3. `miri`: nightly only (~30 min runtime).

Numeric input-budget summary (audit gate ≥100k per parser): 200k for
HPACK/QPACK/h1 (highest smuggling-risk), 100k for everything else.
Total CI minutes per nightly: ~45 min on a standard runner. PR-time
budget (`PROPTEST_CASES=5000`): ~3 min.

Proof:
- `cargo test --release --tests` runs proptest in reduced mode and
  must pass.
- Nightly CI lab job runs full budget. A reduction file
  `proptest-regressions/` per crate persists any minimised counter-
  examples — these become permanent unit tests.
- `cargo test --cfg loom --release` runs the four loom tests.
- `cargo +nightly miri test` runs the two miri tests.

Risk / blast radius:
- Adds CI minutes; budget audited.
- Loom tests can be flaky if model exploration is unbounded; capped
  per file via `LOOM_MAX_*`.
- Counterexamples from proptest run-1 may reveal bugs whose fixes
  fall outside this plan; those land as new CODE-3-NN tickets.
- proptest dep is dev-only; zero release-binary impact.

Cross-ref:    CODE-2-04 (loom proofs), CODE-2-07 (miri pod_layout),
              CODE-2-08 (loom router)
Owner:           code
Lead-approval: approved 2026-05-13 team-lead
