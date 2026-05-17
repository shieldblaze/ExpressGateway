# Pre-existing H2 defect workstream (surfaced by S3 Phase 0)

Status: **NOT S3 work.** Two pre-existing, main-era / pre-S1 H2
correctness defects surfaced by running the full `--all-features`
workspace suite (which S1/S2 never did — 28 GB disk). Provenance
proven: `git log feature/h3-quic-s2..feature/h3-quic-s3 -- <files>`
is EMPTY for both; `tests/h2_security_live.rs` last touched by the
main base commit `ac58f613`, `tests/h2spec.rs` by pre-S1
`450b6e80`. NOT caused by any S3 commit (cargo runs test binaries
serially + aborts on first failure; the reworked H1 test was proven
not co-executing in the failing runs).

These were each, at different points this session, mislabeled
("environmental flake" / "documented load flake"). Verbatim
captured evidence proves both are **server-side wrong-frame
behaviours under load**, not CPU starvation (starvation → timeouts,
not affirmatively-wrong frames). They pass in isolation and fail
under full-workspace build+test load — the signature of a real
load/timing-sensitive correctness race.

---

## DEFECT 1 — Rapid-Reset: hard TCP close instead of HTTP/2 GOAWAY

**Priority: SECURITY (CVE-2023-44487 Rapid Reset mitigation).**

Test: `tests/h2_security_live.rs::rapid_reset_goaway` (asserts at
`:342`). Verbatim captured failure under full-workspace load:

```
rapid_reset: send_err=None conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
panicked at tests/h2_security_live.rs:342:5:
expected server-initiated GOAWAY after rapid-reset flood;
send_err=None, conn_res=Ok(Ok(Err(Error { kind: Io(Kind(BrokenPipe)) })))
```

Mechanism: after the Rapid-Reset stream flood, the gateway tears
down the **TCP transport** (peer FIN/RST → client `BrokenPipe` on
next write) **instead of** emitting a graceful HTTP/2
`GOAWAY(ENHANCE_YOUR_CALM / PROTOCOL_ERROR)`. `h2::Error` is a raw
`Io(BrokenPipe)`, so `is_go_away()` / `is_remote()` are both false;
the connection was killed at the transport layer, not the protocol
layer. Under load the mitigation path does **not** produce the
RFC-correct protocol signal.

Open question for the workstream (must be answered, not assumed):
is abrupt transport close an acceptable Rapid-Reset mitigation
(connection IS killed → DoS still mitigated) and only the test is
too strict about teardown *form*, OR does the mitigation path race
under scheduling pressure and lose the GOAWAY emit (a real
mitigation-correctness defect)? The CVE relevance makes this
security-priority — do not downgrade without proof. Suspect paths:
the H2 server GOAWAY-on-rapid-reset emit vs connection-drop ordering
under load (lb-h2 / lb-l7 H2 accept + the rapid-reset threshold
trip → teardown path).

## DEFECT 2 — Pseudo-header field in trailers not rejected

**PROTO-2-12 ADJACENT.** Test:
`tests/h2spec.rs::h2spec_generic_conformance` (panics `:199`).
Exactly ONE h2spec case fails (144 passed / 1 skipped / 1 failed,
stderr empty, exit 1):

```
8.1.2.1. Pseudo-Header Fields
  × 3: Sends a HEADERS frame that contains a pseudo-header field as trailers
    -> The endpoint MUST respond with a stream error of type PROTOCOL_ERROR.
       Expected: GOAWAY (PROTOCOL_ERROR) / RST_STREAM (PROTOCOL_ERROR) / Connection closed
         Actual: DATA Frame (length:2, flags:0x01, stream_id:1)
```

Mechanism: a request whose **trailing** HEADERS section contains a
pseudo-header field (RFC 9113 §8.1: trailers MUST NOT contain
pseudo-headers) is **not rejected** — the gateway proxies it and
returns a normal `DATA` frame (a 200-ish body) instead of a
`PROTOCOL_ERROR` stream error. Under full-workspace load this is
hit; in isolation the suite passed (timing-sensitive: the
proxy-forward appears to win a race against trailer validation).

PROTO-2-12 adjacency (owner-requested): the PROTO-2-12 trailer
work **forwards** request/response trailers across bridges —
`crates/lb-l7/src/h2_to_h1.rs:123` (req) / `:150` (resp),
`crates/lb-l7/src/h2_to_h2.rs:33`/`:51`, and the H3 equivalents
(`h3_to_h1.rs:65/92`, `h3_to_h3.rs`, `h3_bridge.rs:1149`). The
suspect: the trailer-forwarding path forwards the trailing field
section **without enforcing the RFC 9113 §8.1 / RFC 9114 §4.3
no-pseudo-header-in-trailers rule**, so a pseudo-header in trailers
is forwarded/accepted instead of triggering PROTOCOL_ERROR. The
next workstream MUST check whether the Round-9 trailer work (and
the S2 H3 PROTO-2-12 path in `crates/lb-quic/src/h3_bridge.rs`
`StreamRxBuf::feed_body` `BodyItem::Trailers`) has the **same
missing pseudo-header-in-trailers rejection** — if so this is a
cross-protocol (H2 *and* H3) validation gap with smuggling
relevance, not an isolated H2 conformance miss.

---

## Recommended handling

- Treat as a **separate defect-fix workstream**, not folded into
  the H3 response-streaming program. Both are pre-existing and
  out of the S3 headline scope.
- Do **not** exclude either from any green-gate by name (rejected
  by the program owner): excluding a CVE-2023-44487 mitigation
  defect by definition is unacceptable; "exclude by name" has been
  the wrong call on every test it was proposed for this session
  (each turned out real).
- Reproduce both under controlled CPU load in isolation (no full
  suite needed) to pin the exact race before fixing; author ≠
  verifier, same discipline.
- Re-run S1/S2's "verified" claims under `--all-features` — see the
  S2 verification-gap note in `s3-report.md` (S2 marked work
  verified that was never run under the full suite; this session
  proved that gap hid the H1-drain contract narrowness AND did not
  cover these two pre-existing H2 defects).
