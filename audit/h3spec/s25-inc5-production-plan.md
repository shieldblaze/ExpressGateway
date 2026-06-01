# S25 — INC-5 (PRODUCTION-ONLY, owner Option 3) implementation plan

**Goal:** get ALL PRODUCTION code off the hand-rolled `lb_h3` framing → quiche::h3.
lb_h3 survives ONLY as a test-only frame codec the wire harnesses still import (the
~20-harness rewrite + crate deletion + Phase-3 + promote → S26). main keeps the
S22-hardened stack (R11: lb_h3 crate not fully deleted ⇒ NO promote).

Author = lead; independent verifier confirms; full-workspace ×3 gate (R1/R15).

## Changes (compile-checked after each group)

### A. Migrate the inline-400 authority-reject to the DECODED Progressive egress
`conn_actor.rs::poll_h3` (~:1141): replace
`encode_h3_response(400) → request_tasks.push((sid, resp))` with a decoded producer
(mirrors the cell spawns): `(resp_tx, resp_rx)` → spawn `Head{400}+Body("bad request")
+End` → `resp_rx_by_stream.insert(sid)` + `stream_response.insert(sid, StreamTx::
progressive())`. The actor's INC-3 Progressive egress (`send_response`/`send_body`)
then frames+sends it via quiche::h3. (Same 400 + "bad request" body, byte-for-byte.)

### B. Delete the now-dead legacy raw-byte path (`request_tasks` sole user was A)
- `poll_h3` signature: drop the `request_tasks` param; call site (:348) drop the arg.
- `run_actor`: delete the `request_tasks` decl (:220), the `task_wait` future (:273-296),
  and the `finished = task_wait =>` select arm (:314-318).
- `StreamTx`: delete the `Buffered` variant + `StreamTx::new` + the `Buffered` egress arm
  (:651-687) + its `finished`/`is-pending` handling (:830, :836). The `Progressive` arm
  is the sole remaining egress (already the live path for every cell since INC-3).
- Drop the `encode_h3_response` import from `conn_actor.rs` (:41).

### C. Delete the now-dead `encode_h3_*` (no production caller after A+B)
`h3_bridge.rs`: delete `encode_h3_response`, `encode_h3_headers_frame`,
`encode_h3_headers_frame_full`, `encode_h3_data_frame`, `encode_h3_trailers_frame` +
their h3_bridge unit tests (`#[test]`s that call them). Other test files frame via
`lb_h3::encode_frame` DIRECTLY (not these wrappers), so they are unaffected.

### D. Delete the dead buffered upstream/roundtrip functions + their (moot) tests
- `request_h3_upstream` + `H3UpstreamResponse` (dead — only the `lib.rs` re-export);
  drop the re-exports.
- `h3_to_h1_roundtrip`, `h3_to_h1_stream` (test-only callers): delete + the tests that
  call them directly (`h3_h1_binary_body_e2e`, the `h3_to_h1_stream` cases in
  `h3_h1_stream_body_e2e` / `h3_h1_stream_body_errors_e2e`). The LIVE H3→H1 path
  (`h3_to_h1_stream_resp`, used by `conn_actor`) is unchanged and stays covered by
  `h3_h1_resp_stream_e2e` + `proto_translation` + the bridging tests.

### E. Drop the production lb_h3 dependency
After A-D, confirm `crates/lb-quic/src` has ZERO `lb_h3` usage (compiler / grep). Move
`lb_h3` from `[dependencies]` → `[dev-dependencies]` in `crates/lb-quic/Cargo.toml`
(tests still use it). Production is then 100% on quiche::h3.

## Gate
- `cargo clippy --all-targets --all-features -D warnings` + fmt clean.
- The 9 cells / Mode B / H1/H2 / security fixes (#11-15,#22) / R8 / F-MD-4 must NOT
  regress (R3) — full-workspace ×3 deterministic (R1/R15, completed runs only).
- Independent verifier confirms (author ≠ verifier): the inline-400 still returns 400 +
  "bad request" via quiche::h3; no behavioral change; production lb_h3-free.

## NOT done (honest PARTIAL → S26)
~20 test-harness rewrites off lb_h3's frame/QPACK codec; delete the lb_h3 crate;
Phase-3 (×3 + FRESH h3spec proving #16-21/#23-25 PASS by construction + re-soak with a
new H3-terminate scenario); PROMOTE. main keeps the S22-hardened stack until then.
