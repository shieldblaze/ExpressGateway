# Security & Conformance (developer view)

This is the "why is this safe and why is it correct" narrative for a contributor
or a skeptical reviewer. It is **not** the canonical security catalog — that is
[`../../SECURITY.md`](../../SECURITY.md) (threat model, full defenses table,
residual risks, disclosure). Here we cover the *engineering posture*: parser
delegation, the one hand-rolled parser and its fuzzing, panic-freedom
enforcement, the S38 audit verdict, and the conformance gates.

## The core strategy: delegate parsing to fuzzed libraries

Defense-in-depth starts with **not writing the wire parsers** — HTTP/1.1 + HTTP/2
on hyper/h2, HTTP/3 + QUIC on quiche (BoringSSL), TLS on rustls, WebSocket
framing on tungstenite. The full delegation map (and why the `lb-h1` / `lb-h2` /
`lb-h3-testcodec` crates are **not** a second, hand-rolled HTTP stack) is
documented once in
[`overview.md`](overview.md#the-one-thing-to-understand-first-parsing-is-delegated).
What this page covers is the surface ExpressGateway *does* own above those
codecs: the one hand-rolled parser and its fuzzing, panic-freedom, and the
conformance gates. Note that the security *filters* inspect *already-parsed*
headers — they are not parsers.

## The one hand-rolled production parser

There is exactly one hand-rolled parser on a production data path:
**`lb_quic::public_header`** (`crates/lb-quic/src/public_header.rs`), the Mode A
QUIC public-header reader that runs on **every datagram** of a passthrough flow.
It is hand-rolled because its whole point is to route **without decrypting** —
no off-the-shelf TLS-terminating parser can do that. It is engineered to be
unable to crash:

- The crate carries
  `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic,
  clippy::indexing_slicing)]` **including in tests** — so even a `&buf[..n]`
  slice in a test fails the gate. Slicing on the parse path goes through
  `.get()`-checked access; length arithmetic uses `checked_add`.
- It reads only cleartext public-header fields and never the encrypted payload,
  packet-number bytes, or header-protected reserved bits (the documented
  no-decrypt invariant).

**Fuzzing.** The Mode A parser survived **~670 M coverage-guided iterations with
0 crashes** (`audit/security/s38-report.md`). It is one target in a campaign of
**9 fuzz targets totalling ≈ 1.03 billion executed units — 0 crashes, 0 OOMs, 0
artifacts**. There were no crashing inputs, so there is no crash-regression
corpus to add. (`fuzz-smoke` is a CI gate; the full campaign runs on dedicated
hardware.)

## Panic-freedom is structural

A panic in a release build aborts (`panic = "abort"`), so an *unhandled* panic is
a hard outage. Panics are therefore forbidden **by construction** — every library
crate `#![deny]`s `unwrap`/`expect`/`panic`/indexing (with `lb-quic` keeping
`indexing_slicing` denied even in tests), the binary installs an aborting panic
hook (`init_panic_hook`), and CI turns red on any new offender. The full
mechanism and rationale live in
[`overview.md`](overview.md#error--panic-free-model) and
[`ADR-0010`](../decisions/ADR-0010-panic-free-enforcement.md). The security
consequence is the point here: there is **no wire-reachable panic** an attacker
can turn into a crash, and the S38 audit (below) found none.

## The S38 audit verdict

A four-auditor adversarial audit (parser / protocol / resource / infra) was run
against the full internet-facing, all-protocol deployment profile (S38). The
verdict:

> **0 Critical · 0 High · 1 Medium · 7 Low · 4 Info** — no auth bypass, no
> smuggling, no wire-reachable memory unsafety, no dependency-implicating
> finding.

The single **Medium** (`F-RES-1`) was **fixed this session**: hyper's H1
`header_read_timeout` was inert (no `.timer()` was wired, so the slowloris
header phase was bounded by the 60 s connection `total` instead of the intended
10 s `header` timeout); the fix wires the timer on the H1 builder in
`crates/lb-l7/src/h1_proxy.rs`. The Lows/Infos are tiered with proven
dispositions. The findings live under
[`../../audit/security/s38-findings.md`](../../audit/security/s38-findings.md)
(with `s38-findings-{parser,protocol,resource,infra}.md` and the threat model in
`s38-threat-model.md`). The canonical, prose catalog and residual-risk list is
[`../../SECURITY.md`](../../SECURITY.md).

## Conformance

### HTTP/2 — h2spec 147/147

The h2spec suite passes **147 of 147** examples (`tests/h2spec.rs`, a graceful
skip when the `h2spec` binary isn't on `PATH`). h2spec strict mode is gated in
CI.

### HTTP/3 — h3spec passes with 12 named waivers

The h3spec suite passes with a closed list of **12 named waivers** enforced by
[`../../scripts/ci/h3spec-check.sh`](../../scripts/ci/h3spec-check.sh)
(`CF-QUICHE-UPGRADE`). This is an **honesty gate**, not a blanket allowance:

- Each waiver is named individually with its spec reference (the script's
  `WAIVERS` array).
- The gate passes **iff** the set of h3spec failures is a *subset* of the waiver
  list **and** the suite actually ran (a minimum-examples floor guards against
  "couldn't connect, 0 ran").
- A **new** failure outside the list is `UNEXPECTED` → CI red. A waived case
  that starts *passing* (quiche fixed it) warns + suggests pruning, but does not
  fail.

What the 12 waivers are, and why they are inert:

1. **Ten QUIC transport-parameter / reserved-bit checks** quiche 0.29 does not
   enforce (e.g. "MUST send `TRANSPORT_PARAMETER_ERROR` if
   `original_destination_connection_id` is received", "MUST send
   `PROTOCOL_VIOLATION` if reserved bits in Short header are non-zero"). These
   are quiche-internal transport-layer deviations; the gateway has no hook to
   change quiche's behavior.
2. **Two QPACK encoder/decoder-stream instruction checks** (QPACK §4.1.3 dynamic
   table capacity, §4.4.3 Insert Count Increment) that quiche **reads and
   discards**. They are inert: quiche never allocates a dynamic table, so there
   is no amplification surface, and again there is no gateway hook.

None have a security impact; they are upstream-library deviations documented for
transparency. The gateway's *own* H3 behavior — pseudo-header validation, frame
sequencing, and message-error handling — passes. Operator-facing summary:
[`../known-limitations.md`](../known-limitations.md#h2spec--h3spec-named-waivers).

## See also

- [`../../SECURITY.md`](../../SECURITY.md) — canonical threat model + defenses
  table + residual risks + disclosure.
- [`backpressure.md`](backpressure.md) — the bounded-relay (R8) DoS posture.
- [`quic-modes.md`](quic-modes.md) — the Mode A no-decrypt parser in context.
- [`../../audit/security/`](../../audit/security/) — the S38 audit trail.
