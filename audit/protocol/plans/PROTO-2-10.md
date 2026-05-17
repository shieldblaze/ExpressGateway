# Plan for PROTO-2-10 — SmuggleDetector wired into H1/H2 hot path

Finding-ref:    PROTO-2-10 (high, Open)
Files touched (proto's slice — the **config hook + detector matrix +
test plan**; the actual wire-up edits in `crates/lb-l7/src/h1_proxy.rs`
+ `h2_proxy.rs` are SEC-2-01's lane per synthesis §D):
  - `crates/lb-l7/src/h2_proxy.rs` (proto-owned; proto adds the
    matrix-aware *call shape* for H2→H1 downgrade detection — SEC
    plumbs the H1 entry, proto plumbs the H2 entry; the synthesis
    §D file-ownership column gives proto `h2_proxy.rs + bridges`)
  - `crates/lb-l7/src/h2_to_h1.rs` (proto-owned; check_h2_downgrade)
  - `crates/lb-controlplane/src/config.rs` (config hook —
    `[security.smuggle]` block)
  - `tests/smuggle_matrix_h1.rs`, `tests/smuggle_matrix_h2.rs`,
    `tests/smuggle_h2_to_h1_downgrade.rs` (new)

Approach:

**Config hook (proto-authored, applies workspace-wide).**

```toml
[security.smuggle]
enabled = true                       # default true; flips strict_mode
check_cl_te         = true
check_te_cl         = true
check_h2_downgrade  = true
check_duplicate_cl  = true
check_leading_zero_cl = true
on_detection = "reject"              # "reject" | "log_only"
```

`config.rs` parses these into a `SmuggleConfig` struct that
`H1Proxy::new` and `H2Proxy::new` consume. Default behaviour is
`enabled = true, on_detection = "reject"`. `log_only` exists for
canary rollouts where operators want to observe the false-positive
rate before flipping to reject.

**Detector-per-listener matrix.**

| Listener        | Inbound parser | Detectors that MUST run                                                                        |
|-----------------|----------------|------------------------------------------------------------------------------------------------|
| `plain-tcp` (L4) | n/a            | none (no L7 parse; bytes pass through unmodified)                                              |
| `tls`            | n/a            | none (TLS passthrough is L4-equivalent)                                                        |
| `h1` / `h1s`     | hyper H1       | `check_cl_te`, `check_te_cl`, `check_duplicate_cl`, `check_leading_zero_cl` (front-door H1)     |
| H2 server (via `h1s` ALPN upgrade or h2c) | hyper H2 | `check_h2_downgrade` (only fires when bridge is H2→H1)                                |
| `h3` (QUIC)      | quiche-h3      | `check_h2_downgrade` analogue for H3→H1 (PROTO-2-12 cross-cut; H3 forbids CL+TE same as H2)    |

Rationale for the matrix:
  - hyper's H1 parser catches some CL/TE shapes but not all (sec
    cross-review §A.7 confirmed: hyper drops CL when TE-chunked is
    present, so the *gateway internal* view is clean but the outbound
    request to an H1 upstream may re-introduce CL if the bridge does
    not strip it — that's the exact gap `check_te_cl` covers).
  - H2-front-door already forbids TE per RFC 9113 §8.2.2, so
    detection runs only on the H2→H1 downgrade *bridge* output, not
    on the H2 server inbound parse.
  - L4 listeners deliberately have no detectors — operators choosing
    L4 are opting out of L7 inspection.

**The wire-up that lives in SEC-2-01's edit:** SEC inserts the
`SmuggleDetector::check_all` call inside `H1Proxy::proxy_request`
immediately after `req = hyper::Request::from_parts(parts, body)`
and *before* bridge selection. proto's H2-side analogue lives in
`H2Proxy::proxy_request` at the matching point. Both call sites
read the shared `SmuggleConfig` and either return
`Response::builder().status(400)` or log-only per
`on_detection`.

**`H2ToH1Bridge::check_h2_downgrade` call site (proto's lane).**
Inside `H2ToH1Bridge::proxy`, after `strip_hop_by_hop` and before
serialising the H1 request to the upstream connector:

```rust
if cfg.smuggle.check_h2_downgrade {
    SmuggleDetector::check_h2_downgrade(&req)
        .map_err(|e| BridgeError::Smuggle(e))?;
}
```

This guards the path where an H2 client sent a `:method` /
`:path` / headers set that, when serialised to H1, would re-emerge
with a forbidden `Transfer-Encoding` (RFC 9113 §8.2.2 forbids TE
in H2 but a malicious H2 client could put `transfer-encoding:
chunked` in a regular header field; hyper's H2 server-side parse
may or may not have rejected it depending on hyper version; the
detector is the belt-and-braces).

**What hyper covers vs. what detectors cover (correcting the round-2
matrix per sec §A.7):**

| Variant | Hyper 1.9.0 H1 server | Detector |
|---|---|---|
| CL + TE-chunked both present | drops CL, keeps TE-chunked (NOT 400) | `check_cl_te` rejects |
| CL with multiple comma-separated values (`5,5`) | coalesces and accepts | `check_duplicate_cl` rejects (strict) |
| CL leading-zero / space-padded | rejects | `check_leading_zero_cl` rejects (redundant; defence-in-depth) |
| Duplicate TE final-token | partial | `check_te_cl` rejects |
| H2→H1 with TE re-injected | not seen by H2 parser | `check_h2_downgrade` |
| CRLF in header value | hyper rejects (httparse) | detector rejects |

Net: ~65% of cases overlap with hyper; the ~35% non-overlap is the
load-bearing reason the detectors must run.

Proof:
  - Test: `tests/smuggle_matrix_h1.rs::cl_te_desync_rejected`
    Invariant: raw TCP send `Content-Length: 5\r\nTransfer-Encoding:
    chunked\r\n\r\n5\r\nworld\r\n0\r\n\r\nGET /smuggled HTTP/1.1\r\n
    Host: x\r\n\r\n` to an H1 listener → gateway responds 400; the
    `/smuggled` portion never reaches upstream.
  - Test: `tests/smuggle_matrix_h1.rs::te_cl_desync_rejected`
    Invariant: TE: chunked first + CL: 5 → 400.
  - Test: `tests/smuggle_matrix_h1.rs::duplicate_cl_rejected`
    Invariant: two `Content-Length: 5` headers → 400 (strict mode).
  - Test: `tests/smuggle_matrix_h1.rs::leading_zero_cl_rejected`
    Invariant: `Content-Length: 05` → 400.
  - Test: `tests/smuggle_matrix_h2.rs::h2_te_header_rejected_by_hyper`
    Invariant: hyper-side already rejects (regression-only; asserts
    we don't double-handle).
  - Test: `tests/smuggle_h2_to_h1_downgrade.rs::te_smuggled_via_header_field_rejected`
    Invariant: H2 client sets a regular header
    `transfer-encoding: chunked`; gateway routes to an H1 upstream;
    the bridge detects and returns 400.
  - Test: `tests/smuggle_matrix_h1.rs::log_only_mode_does_not_reject`
    Invariant: with `on_detection = "log_only"`, a CL+TE request
    passes through but a log line with severity warn is emitted and
    the `smuggle_detected_total{kind="cl_te"}` counter increments.

Risk / blast radius:
  - False-positive risk: legitimate clients that send malformed
    framing newly fail. Mitigation: `log_only` mode for staged
    rollout (sec / rel co-author the canary doc); also the
    detectors only fire on shapes hyper itself would have rejected
    on a strict parse — these are not "borderline RFC" cases.
  - Per-request cost: the detectors do ~6 `HeaderMap::get`s and a
    couple of `&str` comparisons; bench-measured at <500 ns on the
    development workstation. Negligible vs. the hyper parse itself.
  - Cross-team coupling: SEC-2-01 plumbs the H1 call site; proto
    plumbs the H2 bridge call site. Synthesis §D allocates
    `crates/lb-l7/src/h1_proxy.rs` to sec and `h2_proxy.rs` /
    bridges to proto, so no edit conflict in Round 4.

Cross-ref:    joint with SEC-2-01 (call-site plumbing) and CODE-2-01
              (dep-graph fix); closes PROTO-2-10; corrects the
              round-2 matrix per sec cross-review §A.7.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
