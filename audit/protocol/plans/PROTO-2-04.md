# Plan for PROTO-2-04 — Real Autobahn fuzzingclient CI run

Finding-ref:    PROTO-2-04 (medium, Open)
Files touched:
  - `tests/ws_autobahn.rs` (replace `--help` stub with real harness)
  - `tests/fixtures/autobahn-fuzzingclient.json` (new — fuzzingclient
    spec config)
  - `tests/fixtures/autobahn-allowlist.json` (new — documented
    known-broken cases, initially empty)
  - `crates/lb/Cargo.toml` (no change; the test consumes the binary
    via `assert_cmd` or the existing test rig)
  - CI image bake — hand-off to `rel`: file an item under their
    `REL-2-01` doc/drift remediation umbrella to add `wstest` from
    `autobahntestsuite` (Python pip package) to the CI image. The
    request is a one-line `pip install autobahntestsuite` in the
    base image. Image-construction itself is rel-owned per synthesis
    §D; this plan supplies the consumer-side spec only.

Approach:
The current `tests/ws_autobahn.rs` probes `which wstest` and exits
OK with a TODO message even when wstest is available. Replace with
a real harness:

1. Bring up the gateway in WS-proxy mode against an in-test
   WebSocket-echo backend (use `tokio-tungstenite` for the echo
   server; loopback bind on a port chosen via `TcpListener::bind("127.0.0.1:0")`).

2. Write `autobahn-fuzzingclient.json` to a temp dir at test start:

```json
{
  "outdir": "./reports/clients",
  "servers": [
    {"agent": "expressgateway", "url": "ws://127.0.0.1:{PORT}/"}
  ],
  "cases": ["*"],
  "exclude-cases": [],
  "exclude-agent-cases": {}
}
```

   `{PORT}` substituted at runtime.

3. Spawn `wstest -m fuzzingclient -s <path-to-spec>` via
   `tokio::process::Command`; wait for completion with a 5-min hard
   timeout. Autobahn fuzzingclient acts as the **client** driving
   the gateway-fronted echo (so we exercise the gateway's
   WS-passthrough behaviour as a server).

4. Parse `./reports/clients/index.json`. For each case, the entry
   has shape `{"behavior": "OK"|"NON-STRICT"|"FAILED"|...,
   "behaviorClose": "..."}`. Pass criteria:
   - `behavior in {"OK", "NON-STRICT", "INFORMATIONAL"}`: pass
   - `behavior == "FAILED" | "WRONG CODE" | "UNCLEAN"`: fail,
     unless the case ID is in `autobahn-allowlist.json` (initially
     empty; lead approves additions case-by-case).

5. Assert ≥ 6 cases pass at the *gateway-as-server* role. Autobahn
   exposes ~520 cases; 6 is the minimum-acceptable smoke target the
   task brief specifies. The full target is "all non-allowlisted
   cases pass"; the test asserts both.

The harness also runs `wstest -m fuzzingserver` against the
gateway acting as a **client** to a fuzzing-server (gateway connects
upstream via the WS proxy; the upstream is `wstest -m fuzzingserver`).
This covers the second half of the Autobahn matrix. The task brief
explicitly says "server-or-client mode" — both directions land in
this single test, gated by `wstest`'s presence.

CI image bake (rel hand-off):
  - Add to base CI image: `python3 -m pip install autobahntestsuite`
    (pulls in twisted + txaio + autobahn; ~30 MB).
  - Add to PATH check in CI: `wstest --help` must succeed.
  - When `wstest` is **not** on PATH, the test compiles `#[ignore]`
    so local `cargo test` is unaffected. The CI runner sets
    `LB_AUTOBAHN_REQUIRED=1` to flip the ignore into a hard
    requirement.

Proof:
  - Test: `tests/ws_autobahn.rs::autobahn_fuzzingclient_against_gateway`
    Invariant: with `wstest` on PATH and gateway running, ≥ 6
    Autobahn client cases report `OK` or `NON-STRICT`; no
    `FAILED` cases outside the allowlist.
  - Test: `tests/ws_autobahn.rs::autobahn_fuzzingserver_with_gateway_client`
    Invariant: same shape, reversed direction.
  - Test (regression): `tests/ws_autobahn.rs::stub_replaced_when_wstest_missing`
    Invariant: when `LB_AUTOBAHN_REQUIRED=1` and `wstest` missing,
    test fails with a clear "wstest not on PATH" message (no silent
    skip).

Risk / blast radius:
  - CI runtime: full fuzzingclient run is ~3-4 min on a 4-core
    runner. Hard timeout 5 min; the test fails on overrun rather
    than hanging the build.
  - First real exposure of the WS path may surface real RFC 6455
    bugs in the gateway's frame loop. Each new failure either gets
    a code fix (lands as a separate finding in Round 4) or, with
    lead approval, an entry in `autobahn-allowlist.json` with a
    justification comment.
  - No source changes outside `tests/`; the WS proxy code is
    untouched by this plan. Bug-fixes from observed failures will
    be filed as new findings post-Round-4.

Cross-ref:    feeds rel REL-2-01 (CI image inventory); informs
              PROTO-2-13 (the close-handshake / extended-CONNECT
              advertise test); closes PROTO-2-04.
Owner:        proto
Lead-approval: approved 2026-05-13 team-lead
