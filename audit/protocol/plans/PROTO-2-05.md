# Plan for PROTO-2-05 — `h3i` HTTP/3 conformance harness

Finding-ref:    PROTO-2-05 (medium, Open)
Files touched:
  - `crates/lb/Cargo.toml` or `Cargo.toml` (workspace) — add
    `h3i` to `[dev-dependencies]`
  - `tests/h3spec.rs` (new — `h3i` driver)
  - `docs/conformance/h3i-cases.md` (new — documented case-set)
  - `tests/fixtures/h3i/*.qlog` (new, if any case requires a
    pre-recorded transport-log fixture)

Approach:
**Choice: `h3i` over `h3spec`.** `h3i` is Rust and lives in the
same Cloudflare `quiche` workspace already depended on (the
workspace currently uses `quiche = "0.28"`; `h3i` shipped its first
crates.io release alongside quiche 0.21+ and is at the same crate
version cadence as quiche). `h3spec` is a Go binary and would
duplicate the CI image bake just for one test target. `h3i`'s
case-set is also the larger of the two (Cloudflare runs the
upstream interop tests against it).

Dependency add (workspace root `Cargo.toml`):

```toml
[workspace.dependencies]
h3i = "0.7"   # exact pinned minor; cadence-aligned with quiche 0.28
```

and in `crates/lb-quic/Cargo.toml` (or a dedicated test crate if
the workspace prefers):

```toml
[dev-dependencies]
h3i.workspace = true
```

`h3i` exposes a programmatic API (`h3i::client::sync_client` and the
`Action` enum) to script frame sequences and assert on responses.
Use the synchronous variant in tests for deterministic ordering.

Driver shape (`tests/h3spec.rs`):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn h3i_conformance_case_set() {
    let (gateway_addr, _gw) = spawn_h3_listener().await;
    for case in CASES {
        let client = h3i::client::ClientBuilder::new()
            .with_host_port(gateway_addr.to_string())
            .with_idle_timeout(5_000)
            .build()
            .expect("h3i client");
        let report = client.run(case.actions()).expect("h3i run");
        case.assert(&report);
    }
}
```

Initial case-set (documented in `docs/conformance/h3i-cases.md`):

1. **Settings echo** — server SETTINGS frame is sent on the
   server-initiated control stream within 1 RTT; QPACK
   blocked-streams and max-table-capacity fields are present.
2. **HEADERS oversize rejection** — send a HEADERS frame whose
   QPACK-decoded size exceeds `SETTINGS_MAX_FIELD_SECTION_SIZE`;
   server responds with `H3_EXCESSIVE_LOAD` or rejects the stream.
3. **GREASE frame tolerance** — send a GREASE frame on the
   request stream; server ignores per RFC 9114 §7.2.8.
4. **MAX_PUSH_ID enforcement** — server respects the
   `MAX_PUSH_ID` value (today the gateway doesn't push, so any
   client-side PUSH_PROMISE is a protocol error — assert
   `H3_ID_ERROR`).
5. **QPACK encoder-stream blocking** — opening a HEADERS frame
   that references a not-yet-acknowledged dynamic-table entry
   causes the server to block, then to unblock on
   `INSERT_COUNT_INCREMENT`.
6. **GOAWAY on graceful shutdown** — PROTO-2-11 cross-cut. With
   gateway draining (SIGTERM), assert a `GOAWAY` frame appears on
   the server control stream with a stream-id that is the largest
   in-flight stream the server accepted.
7. **Invalid pseudo-header order** — client sends `:method` after
   a regular header; server responds with stream error
   `H3_MESSAGE_ERROR` per RFC 9114 §4.1.1.

Case 6 is gated on PROTO-2-02 (real `h3` ALPN) **and** PROTO-2-11
(GOAWAY emission); the test file is structured so it compiles and
the other cases run even if the GOAWAY case is `#[ignore]`-d while
PROTO-2-11 lands.

CI image: `h3i` is a pure-Rust crate; no extra image bake beyond
the dev-dependency. The H3 listener spawned in-process uses the
fixed loopback bind from `tests/common.rs`.

Proof:
  - Test: `tests/h3spec.rs::h3i_conformance_case_set`
    Invariant: every case in the initial case-set reports `Ok`
    against the live H3 listener.
  - Sub-tests (each case file-local `#[test]`): one per case so a
    single failure pinpoints the failing scenario.

Risk / blast radius:
  - `h3i` 0.7 is the API the plan targets; if the workspace later
    bumps to a newer minor with an API break, the test harness
    breaks. Pinning to exact `=0.7.x` mitigates; lead may relax in
    Round 6.
  - Some `h3i` cases require a longer-lived loopback listener;
    test runtime ~30 s for the full case-set on a 4-core runner.
    Under CI's existing `--test-threads=1` configuration that is
    additive to the WS Autobahn run.
  - Cases will fail until PROTO-2-02 lands (the listener
    advertises wrong ALPN today). The whole file is gated behind
    `#[cfg_attr(not(feature = "h3-ready"), ignore)]` until then,
    flipped on once PROTO-2-02 ships.

Cross-ref:    depends on PROTO-2-02 (h3 ALPN); composes with
              PROTO-2-11 (GOAWAY case); closes PROTO-2-05;
              addresses PROTO-2-06 option (b) for H3.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
