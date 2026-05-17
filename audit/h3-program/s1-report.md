# H3/QUIC Path-1 — SESSION 1 Report

Branch: `feature/h3-quic-s1` (pushed to `origin/feature/h3-quic-s1`)
Date: 2026-05-16 | Lead: session lead | Team: inventory-eng, h3-builder-1, h3-builder-2, verifier
Base lineage: `feature/h3-green` (confirmed contains Round-9 H3 commit `b9a2c274`)
+ merged `origin/fix/round8-d1-d2-d5` (D-1/D-2/D-5 fixes) = merge commit `5b7d8658`.

## Verdict

**SESSION 1 COMPLETE** — both foundation deliverables (QUIC-listener crate-local
test coverage; H3→H1 backend bridge proven end-to-end) are built and
independently verified (author ≠ verifier for every increment and every fix).

## Commits (this session, on `origin/feature/h3-quic-s1`)

| Commit | Increment | Author | Verified by |
|--------|-----------|--------|-------------|
| `e0f18242` | Phase A inventory + dependency-ordered build-plan | inventory-eng | (audit artifact) |
| `25d8ad84` | B.1 — crate-local QUIC listener+router test coverage | h3-builder-1 | verifier |
| `162e5a59` | B.2 — H3→H1 bridge e2e + S1-B body seam (+S1-LINT-1 fix) | h3-builder-2 | verifier |

## What was BUILT and VERIFIED (with evidence)

### Phase A — QUIC/H3 inventory (`s1-inventory.md`, `build-plan.md`)
8-capability matrix BUILT/PARTIAL/ABSENT with file:line + test evidence.
Key verified facts: H3→H1/H2/H3 all BUILT for bodyless GET only; native QUIC
proxy PARTIAL; WS-over-H3 and gRPC-over-H3 ABSENT; quiche 0.28.0 +
tokio-quiche 0.18.0 integration BUILT. Program-wide blocker identified:
**no request-body/DATA-frame forwarding + no upstream trailer plumbing on
any H3 path** — sequenced as S2.

### B.1 — QUIC listener crate-local test coverage (the "0%" finding, closed)
- NEW `crates/lb-quic/tests/listener_lifecycle.rs` (6 tests): spawns the REAL
  `QuicListener::spawn` — UDP bind/`local_addr`, retry-secret 0600
  autogenerate + reload (verified by token minted by first signer), ALPN
  h3/h3-29 accept to `is_established()`, unknown-ALPN reject, clean shutdown
  (join within budget + port released).
- NEW `crates/lb-quic/tests/router_accept_path.rs` (3 tests): two concurrent
  distinct-CID clients → distinct per-CID actors; RETRY wire round-trip
  (`quiche::Type::Retry` packet observed then connection established);
  0-RTT replay-guard first-ok/second-err/distinct-key contract on the **exact
  shared `Arc<…ZeroRttReplayGuard>` the router holds**. Per-Initial byte-exact
  wire re-injection is **documented unreachable** without a router/listener
  source hook (quiche client API exposes no stable post-RETRY Initial
  boundary) — documented in code + report, **not faked, no weak assertion**.
- `crates/lb-quic/tests/proptest_header.rs`: removed the silent
  `#![cfg(feature="proptest")]` dead gate (`proptest` is an unconditional
  dev-dependency); 256-case sanity budget now runs by default; case logic
  byte-identical (verified by header-stripped diff).
- **Coverage (independently re-measured, `cargo llvm-cov -p lb-quic`):**
  `listener.rs` **0.00% → 80.09%** (173/216 lines), `router.rs`
  **48.47% → 90.39%** (442/489). Both meet the CI D6 `--fail-under-lines 80`
  gate. Crate-local active tests **14 → 24**.

### B.2 — H3→H1 backend bridge, proven end-to-end
- NEW `crates/lb-quic/tests/h3_h1_bridge_e2e.rs` (1 test): drives the REAL
  `QuicListener` (UDP → router → `conn_actor::poll_h3` →
  `h3_bridge::h3_to_h1_roundtrip`) to a real tokio HTTP/1.1 upstream;
  asserts response **`:status==201` + body bytes + `Host:` (`:authority`→
  `Host`) VERBATIM** — strictly stronger than round8's probe-hit count.
- S1-B seam: `build_h1_request`/`h3_to_h1_roundtrip` thread
  `body: Option<&[u8]>`. `None` reproduces pre-seam bytes exactly (guard test
  `build_h1_request_none_body_is_byte_identical_to_legacy_bodyless`); the
  sole datapath caller (`conn_actor::poll_h3`) passes `None` → **zero
  on-wire change this session**. `#[ignore="S2: request-body forwarding"]`
  marks the unbuilt S2 target (masks no existing passing behavior).
- Process note: S1-LINT-1 (unused `Arc` import) + a `clippy::doc_lazy_
  continuation` defect from the same changeset were found by the verifier /
  surfaced by the lint gate, fixed by the author (h3-builder-2), and the
  delta independently re-verified. `cargo clippy -p lb-quic --all-targets
  -- -D warnings` is now clean (rc 0).

### Verification (author ≠ verifier, enforced)
Independent `verifier` reproduced from scratch: full `cargo test -p lb-quic`
green (round8 3/3 intact, 24 active + 1 ignored = the S2 marker only),
coverage numbers reproduced exactly, repo-root regression
`quic_listener_e2e_http3_get_through_proxy_to_h1_backend` still green, no
existing test weakened/skipped/deleted/ignored, the S2 `#[ignore]` masks no
existing behavior, and the lint-fix delta is doc/import-only with the
byte-identical `None` contract intact.

## What REMAINS (carried forward / known caveats)

1. **S2 binary-body design caveat (must fix in S2):** the seam's `Some(body)`
   arm builds the request via `String::from_utf8_lossy` — inert this session
   (no `Some` caller; only the ASCII `#[ignore]` marker exercises it), but
   S2 must move the body path to raw bytes (`Vec<u8>`) so non-UTF-8 request
   bodies are not corrupted and `Content-Length` cannot desync.
2. **D6 CI package list:** `prod-readiness-gates.yml:101` does not yet list
   `lb-quic`, so the ≥80 gate does not yet enforce on it in CI even though
   the numbers satisfy it locally. Separate TODO (not a B.1/B.2 blocker).
3. **Thin margin:** `listener.rs` 80.09% is 0.09 pp above the gate — S2+
   listener work should not regress it; consider raising coverage when the
   router/listener gains datapath body handling.
4. **0-RTT per-Initial wire replay** enforcement through the real listener
   remains proven only at the shared-guard-contract level; byte-exact wire
   re-injection needs a router/listener test hook — candidate for the S7
   chaos suite or a small S1-follow-up source seam.

## Updated build-plan for SESSION 2

`build-plan.md` stands. S1 is complete, so the strict serial gate is cleared:

**SESSION 2 = request-body / DATA-frame forwarding** (HARD prerequisite for
gRPC-over-H3, WS-over-H3, and full streaming):
- `conn_actor::poll_h3`: stop dispatching on first HEADERS; accumulate inbound
  H3 DATA frames until FIN; thread the collected body into `H3Request`.
- `h3_bridge`: fill the S1-B seam's `Some(body)` arm **using raw bytes, not
  `from_utf8_lossy`** (caveat #1); apply the same body plumbing to
  `h3_to_h2_roundtrip` and `h3_to_h3_roundtrip`; plumb request/response
  trailers (the `h3_bridge.rs:99` "3b.3b" TODO).
- Drop the `#[ignore]` on `s2_target_build_h1_request_with_body_…` and extend
  it to a real bodyful POST e2e for H3→H1 (then H3→H2, H3→H3): assert body
  integrity both directions, large-body / multi-DATA-frame / partial-flush.
- Exit S2: bodyful round-trip proven for all three bridges in crate-local +
  repo-root tests; the three "BUILT (GET only)" rows in `s1-inventory.md`
  upgrade to full-request semantics.
Then S3 streaming → S4 native QUIC proxy (∥ with S3) → S5 WS-over-H3 →
S6 gRPC-over-H3 → S7 chaos, exactly as dependency-ordered in `build-plan.md`.

## Housekeeping

Session cron `8d4fadeb` (every 30 min → `scripts/periodic-clean.sh`,
build-safe) is active; session-only, auto-expires in 7 days. It correctly
skipped `cargo clean` while builds were in flight during this session.
