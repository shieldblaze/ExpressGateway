# ADR-0002: HTTP/2 codec — in-house frame codec in lb-h2, no external `h2` crate

- Status: Accepted
- Date: 2026-04-22
- Deciders: ExpressGateway team
- Consulted: RFC 9113 (HTTP/2), RFC 7541 (HPACK), CVE-2023-44487 writeup
  (Rapid Reset), CVE-2024-24549 (CONTINUATION flood), hyperium/h2 issue
  tracker, Cloudflare 2023 post-mortem.

## Context and problem statement

An L7 load balancer must speak HTTP/2 fluently on both sides of the proxy.
Two plausible strategies exist in the Rust ecosystem:

1. Depend on the hyperium `h2` crate, which provides a full client/server
   state machine with flow control, HPACK, and stream multiplexing.
2. Own the codec: implement frame parsing, HPACK, and the security
   detectors in-house so every byte crossing the wire is auditable under our
   panic-free lint gate.

The tension is between velocity (h2 is mature, handles the 80 % case) and
control. An HTTP/2 *proxy* is not the same workload as an HTTP/2 *server*:
we forward frames largely unchanged, we care deeply about flood-class
attacks (Rapid Reset, CONTINUATION flood, HPACK bomb, 0-length
DATA spam), and we must be able to reject or throttle at the frame layer
without re-assembling full messages. The h2 crate is optimised for the
server/client use case; it re-encodes what it parses, and exposing
frame-level knobs to abuse-mitigation code requires forks or private-API
hacks.

A load balancer has also historically been the *target* of every major
HTTP/2 CVE of the past three years. If our codec handwaves a decision —
for example, what to do when 10 000 `RST_STREAM` frames arrive in a second
— we inherit the bug. Owning the codec means we can prove, line by line,
that no unbounded allocation, no re-entrant recursion, and no
`unwrap()`/`expect()` exists on the parsing path.

## Decision drivers

- Frame-level abuse mitigation (Rapid Reset, CONTINUATION flood, HPACK
  bomb) needs primitives the h2 crate does not expose.
- `#![deny(clippy::unwrap_used, …)]` compatibility: every dependency must
  be auditable or wrapped.
- Zero-copy: the proxy forwards frames; owning the codec lets us hand a
  `Bytes` payload directly to the upstream writer without decode+re-encode.
- Conformance: RFC 9113 §4–§6 is bounded and testable; the work is
  finite.
- Fuzzability: a hand-rolled codec is easier to target with focused
  fuzzers (we maintain 12 fuzz targets per MEMORY.md).
- Binary size and compile time.
- Licence compatibility (h2 is MIT — acceptable — but we would still carry
  its transitive tree).

## Considered options

1. Pull in `h2` (hyperium) and wrap with flood detectors sitting above the
   stream API.
2. Fork `h2` and add hooks for flood detection and raw-frame forwarding.
3. Implement the codec in-house in `lb-h2` (`frame.rs`, `hpack.rs`,
   `security.rs`, `error.rs`), exposing both high-level request/response
   types and raw `H2Frame` types.

## Decision outcome

Option 3. `lb-h2` is a self-contained HTTP/2 codec with no dependency on
the hyperium `h2` crate. Cargo.lock contains no `h2` package.

## Rationale

- Cargo.lock confirms: no `h2` crate, no `hyper` crate. `lb-h2`'s only
  non-trivial dependencies are `bytes`, `http`, and `thiserror`
  (`crates/lb-h2/Cargo.toml`).
- `crates/lb-h2/src/frame.rs` implements RFC 9113 frame encoding and
  decoding directly, with explicit constants for every frame type
  (`FRAME_DATA`, `FRAME_HEADERS`, `FRAME_PRIORITY`, `FRAME_RST_STREAM`,
  `FRAME_SETTINGS`, `FRAME_PUSH_PROMISE`, `FRAME_PING`, `FRAME_GOAWAY`,
  `FRAME_WINDOW_UPDATE`, `FRAME_CONTINUATION`) — all ten frame types
  covered.
- `crates/lb-h2/src/security.rs` exposes three detectors directly
  relevant to known CVEs:
  - `RapidResetDetector` — sliding-window `RST_STREAM` counter
    (CVE-2023-44487). The doc comment records why a naive fixed-window
    counter is inadequate.
  - `ContinuationFloodDetector` — caps the length of CONTINUATION runs
    (CVE-2024-24549).
  - `HpackBombDetector` — bounds HPACK decompression ratio.
- The h2 crate's public surface does not expose any of these hooks; adding
  them requires patching, and a patched dep is a maintenance liability.
- Our frame-by-frame forwarding design (ADR-0006) is enabled only because
  we own the codec: we parse the 9-byte frame header, *decide*, and
  forward the 9-byte header + payload as a single `Bytes` write.
- The clippy lint gate applies uniformly — `crates/lb-h2/src/lib.rs` head
  has the project-standard `#![deny(…)]` block, so our parsing code can be
  mechanically audited.
- HPACK is implemented separately (`crates/lb-h2/src/hpack.rs`) and can be
  fuzzed independently.

## Consequences

### Positive
- Every byte on the HTTP/2 wire path is covered by our lint gate and fuzz
  corpus.
- Raw frame forwarding is natural; no decode → internal model → re-encode
  round-trip.
- Security detectors are first-class types, not add-ons.
- No transitive dependency inflation.

### Negative
- More code to maintain. RFC 9113 has edge cases (settings ACK ordering,
  flow-control accounting, priority re-enumeration) that we have to keep
  in sync with the RFC.
- We reimplement HPACK; this is a known footgun. We mitigate via a fuzz
  target dedicated to `HpackDecoder`.
- Third-party conformance suites (e.g. h2spec) must be run against our
  codec manually.

### Neutral
- If h2 becomes plug-and-play for proxies in future (e.g. exposes
  frame-level hooks), we can revisit — but the security-detector payload
  stays ours.

## Implementation notes

- `crates/lb-h2/src/lib.rs` — module root, re-exports `H2Frame`,
  `decode_frame`, `encode_frame`, `DEFAULT_MAX_FRAME_SIZE`,
  `HpackDecoder`, `HpackEncoder`, `ContinuationFloodDetector`,
  `HpackBombDetector`, `RapidResetDetector`.
- `crates/lb-h2/src/frame.rs` — RFC 9113 §4 frame codec.
- `crates/lb-h2/src/hpack.rs` — RFC 7541 HPACK codec.
- `crates/lb-h2/src/security.rs` — flood detectors.
- `crates/lb-h2/src/error.rs` — `H2Error` with variants mapped to
  RFC 9113 error codes.
- Conformance harness: `tests/conformance_h2.rs`.

## Follow-ups / open questions

- Add full h2spec conformance run to CI.
- Priority frames (RFC 9218 extensible priorities) — currently we parse
  and re-emit; the scheduler wiring is deferred.
- SETTINGS_MAX_CONCURRENT_STREAMS negotiation as a policy knob in
  `lb-config`.

## Sources

- RFC 9113 — HTTP/2.
- RFC 7541 — HPACK.
- CVE-2023-44487 — HTTP/2 Rapid Reset.
- CVE-2024-24549 — HTTP/2 CONTINUATION flood.
- `Cargo.lock` — confirmation of no-h2-crate stance.
- Internal: `crates/lb-h2/src/{lib,frame,hpack,security,error}.rs`.
