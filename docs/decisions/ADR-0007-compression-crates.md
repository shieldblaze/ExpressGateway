# ADR-0007: Compression crates — zstd 0.13, brotli 7, flate2 1

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: crates.io download stats, RustSec advisory database, RFC
  1951 (DEFLATE), RFC 1952 (gzip), RFC 7932 (Brotli), RFC 8878 (zstd),
  BREACH attack paper (Gluck, Harris, Prado 2013).

## Context and problem statement

ExpressGateway terminates and re-encodes HTTP bodies for at least four
compression algorithms: **gzip** (RFC 1952), **deflate** (RFC 1951),
**Brotli** (RFC 7932), and **zstd** (RFC 8878). A load balancer sitting
between a client and an origin frequently has to *transcode*: the client
accepts `br, zstd, gzip` but the origin only speaks `gzip`, and policy
wants the smaller encoding on the client leg. That means we need
**streaming** encoders and decoders for all four algorithms, a
decompression-bomb detector, and BREACH-class mitigation for sensitive
responses.

For each algorithm we must pick a Rust crate. Candidates in March 2026:

- **zstd**: `zstd` (C-backend via `zstd-sys`) and `ruzstd` (pure Rust,
  decode-only).
- **Brotli**: `brotli` (pure Rust, unofficial Google port by
  Dropbox/Bromite) and `brotli-decompressor` (decompress-only).
- **gzip/deflate**: `flate2` (C miniz_oxide or zlib-ng backends, all
  selectable; pure-Rust miniz_oxide by default) and `libdeflater` /
  `zune-inflate`.

Each axis has trade-offs: C backends are faster but add a C toolchain;
pure-Rust is slower but auditable under our lint regime; encode-capable
crates are larger than decode-only.

## Decision drivers

- **Streaming**: we process HTTP bodies frame-by-frame (ADR-0006), so
  the chosen crates must expose streaming, incremental encode and
  decode.
- **Encode + decode**: a proxy that re-encodes needs both directions
  for every algorithm.
- **Security**: the crate must be actively maintained, not have open
  RustSec advisories at the version we pin.
- **Licence**: workspace is GPL-3.0-only; all chosen crates must be
  compatible (MIT / Apache / BSD / zlib are all compatible with GPLv3
  when we are the distributor).
- **Decompression-bomb resistance**: we implement our own ratio-based
  guard, but the underlying crate must not allocate unboundedly before
  we get control.
- **Reproducibility**: prefer pure-Rust when the perf delta is
  tolerable, for build simplicity.
- **Panic-free interop**: the crate API must be usable under our
  `#![deny(clippy::unwrap_used, …)]` gate — every fallible call must
  return a `Result`.

## Considered options

- zstd: `zstd` (0.13) vs `ruzstd`.
- Brotli: `brotli` (7) vs `brotli-decompressor`.
- gzip/deflate: `flate2` (1.x) vs `libdeflater` vs `zune-inflate`.

## Decision outcome

`crates/lb-compression/Cargo.toml` depends on:

    zstd = "0.13"
    brotli = "7"
    flate2 = "1"

These three crates together cover all four HTTP content-encodings:
`gzip`, `deflate`, `br`, `zstd`. The `Algorithm` enum in
`crates/lb-compression/src/lib.rs:67–76` enumerates exactly these four.

## Rationale

### `zstd = "0.13"`

- Streaming encode and decode via `zstd::stream::{Encoder, Decoder}`.
- C-backend via `zstd-sys`, linking the reference `libzstd` — the only
  zstd implementation with a stable performance profile and full
  compression-dictionary support.
- Pure-Rust alternatives (`ruzstd`) are decode-only as of 2026-04 and
  materially slower. Since the proxy encodes as well as decodes, that
  rules out `ruzstd`.
- Actively maintained; no open advisories in RustSec database at 0.13.
- BSD-licenced underlying library.

### `brotli = "7"`

- Pure-Rust encode+decode, no C toolchain.
- The "unofficial but everyone uses it" port; used by `reqwest`,
  `actix-web`, `hyper-rustls` compression features.
- Streaming `BrotliEncoder` and `BrotliDecompressor` with bounded
  buffer APIs.
- Licence: BSD-3-clause / MIT dual, GPLv3-compatible.
- Major version 7 matches the "Brotli 7" versioning of the original
  Google C library and has been stable since 2024. (The README's
  mention of "Brotli" without a number is a roadmap-level statement;
  the authoritative pin is `Cargo.toml`.)

### `flate2 = "1"`

- Covers both gzip (RFC 1952) and raw DEFLATE (RFC 1951) from one
  crate — two content-encodings, one dependency.
- Default backend `miniz_oxide` is **pure Rust**, so `flate2 = "1"`
  without additional features does not pull in a C toolchain. Backend
  is swappable to `zlib-ng` if peak perf is needed.
- Streaming `GzEncoder`, `GzDecoder`, `DeflateEncoder`, `DeflateDecoder`.
- The most downloaded compression crate on crates.io (hundreds of
  millions of downloads); stable, widely-audited API since 2014.
- Licence: MIT / Apache dual.

### Project-level integration

- `crates/lb-compression/src/lib.rs` wraps all three behind a
  unified `Compressor` / `Decompressor` API.
- `CompressionError::BombDetected` (lines 36–47) and `OutputTooLarge`
  (lines 49–54) are emitted by the wrapper, not the underlying crates
  — we own the bomb policy.
- `BreachGuard` sits at the `lb-compression` layer, not per-crate: it
  disables compression on responses that contain secrets *and* reflect
  attacker-controllable input.
- `negotiate()` implements `Accept-Encoding` content negotiation, per
  RFC 9110 §12.5.3.

## Consequences

### Positive
- Three crates cover four algorithms.
- Two of three (brotli, flate2 default) are pure Rust, keeping the
  default build C-toolchain-free.
- All three expose streaming APIs that compose with our frame-by-frame
  pipeline.
- Owning `BombDetected` ourselves decouples the mitigation policy from
  upstream crate updates.

### Negative
- `zstd` pulls a C dependency (libzstd) via `zstd-sys`. Acceptable for
  the performance win; documented as the sole C dep.
- `brotli` encoding at max level is slow in pure Rust. We cap the
  default level in `Algorithm::default_level`.

### Neutral
- `flate2`'s backend choice can be revisited; the default (miniz_oxide)
  is adequate for HTTP body sizes up to single-digit MiB.

## Implementation notes

- `crates/lb-compression/Cargo.toml` — version pins: `zstd = "0.13"`,
  `brotli = "7"`, `flate2 = "1"`.
- `crates/lb-compression/src/lib.rs:67–76` — `Algorithm` enum.
- `crates/lb-compression/src/lib.rs:36–54` — `CompressionError` variants
  for bomb and size-cap.
- `tests/compression_{brotli,deflate,gzip,zstd}.rs` — per-algorithm
  round-trip tests.
- `tests/compression_bomb_cap.rs` — bomb detector.
- `tests/compression_breach_posture.rs` — BREACH guard.
- `tests/compression_transcode.rs` — cross-algorithm re-encode.

## Follow-ups / open questions

- Consider pinning `flate2` to a non-default backend (`zlib-ng`) for
  high-throughput workloads, behind a feature flag.
- Evaluate `ruzstd` for decode-only paths when it reaches feature
  parity for streaming + dictionaries.
- HTTP/2 compression of static assets — currently deferred to origin.

## Sources

- <https://crates.io/crates/zstd>
- <https://crates.io/crates/brotli>
- <https://crates.io/crates/flate2>
- RFC 1951, 1952, 7932, 8878.
- RustSec advisory database (no open advisories for the chosen versions).
- Internal: `crates/lb-compression/Cargo.toml`, `crates/lb-compression/src/lib.rs`.
