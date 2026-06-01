# SESSION 27 HANDOFF — WebSockets-over-H2/H3, then gRPC-over-H3 (on quiche::h3)

**Branch base:** `main` (post-S26 promote — the H3 migration is LANDED on main; S26 merged
`feature/quiche-h3-migration-s23` `--no-ff`). Branch fresh from main; do NOT continue the S23 branch.
**main tip:** the S26 promote merge commit (see `git log main`). The whole S23→S26 migration is now
on main; the hand-rolled `lb-h3` crate is gone; the H3 front rides `quiche::h3`.

## STATE AT S27 START (S26 landed)
- **H3-front fully on `quiche::h3`** (E1 server ingress+egress S24, E2 upstream client S25);
  hand-rolled H3/QPACK layer fully removed from production + the `lb-h3` crate deleted (S26).
  Wire-test harnesses ride a TEST-ONLY `crates/lb-h3-testcodec` (relocated codec, byte-identical).
- h3spec: 7 of 9 carried findings closed by construction (#16-21, #24); #11-15/#22 pass; Huffman gained.
- Promoted to main; ×3 1442/0, BOUNDED H3-terminate re-soak, R8/R13 re-confirmed.

## S27 WORK — program-level, ~2 sessions to full spec
### 1. WebSockets-over-HTTP/2 (RFC 8441 — Extended CONNECT)
- The H2 stack already has a CONNECT-PROTOCOL test skeleton (PROTO-2-06/13). RFC 8441 adds
  `SETTINGS_ENABLE_CONNECT_PROTOCOL` + `:protocol` pseudo-header. Implement extended-CONNECT
  negotiation + the WebSocket-over-H2 tunnel relay (bidirectional byte stream over an H2 stream).
- Verify with `wstest` (Autobahn) — deferred since Round-7 (`audit/deferred.md` PROTO-2-04);
  needs the CI image to install `wstest`. F-ESC-1 (multi-kernel CI lane) is adjacent.

### 2. WebSockets-over-HTTP/3 (RFC 9220)
- RFC 9220 ports Extended CONNECT to H3. `quiche::h3` exposes the pieces (the h3_config has a
  noted "extended-CONNECT / WebSockets-over-H3 is an S26 item" comment in build_client_h3_config —
  now an S27 item). Implement `:protocol` over H3 + the datagram/stream tunnel.

### 3. gRPC-over-H3 conformance
- `lb-grpc` exists; extend it for gRPC-over-H3 (trailers-only responses, grpc-status in trailers
  — the H3 trailer path is already proven on the migrated stack). Conformance via grpc interop.

## CARRY-FORWARDS (from S26 + program)
- **CF-QUICHE-UPGRADE** (the consolidated quiche-0.28 limitations — ONE list): transport #1-10
  (CONNECTION_CLOSE suppression + transport-param/reserved-bit validation) + **QPACK #23/#25**
  (encoder/decoder uni-stream instruction validation — quiche reads-and-discards, no error variant,
  inert under static-only QPACK) + **CF-QUICHE-FRAME-COMPLETENESS** (§7.1 no-CL truncation). All
  close on a quiche bump that adds these — do NOT hand-roll them (that's what S23-S26 deleted).
  When bumping quiche: re-run h3spec (expect #23/#25 + several #1-10 to flip ✔) + re-tighten the
  §7.1 no-CL truncation test. MSRV pin caveat in root Cargo.toml (foundations/idna_adapter).
- **F-S26-1:** the production binary cannot config-wire an H3-terminate→backend relay
  (`spawn_quic` never calls `with_backends`/`with_h3_backend`/`with_h2_backend`; git-proven
  pre-existing). The migrated relay + §7.1 guard are library/harness-reachable; the soak covers
  ingress + inline-400 + F-MD-4 + no-backend-drop. If a real H3-terminate→backend deployment is
  wanted, wire `[[listeners.backends]]` into `quic_listener_params_from_config` (a product change,
  its own task) — then the soak can exercise the live relay.
- **CF-DEP-1** (Dependabot — owner; clear advisories now that the migration is on main),
  CF-FCAP1-FLAKE (pre-existing H2-timeout race, isolation-proven), N-1 (jumbo-MTU),
  Mode-A perf tiers + CF-S15-PASSTHROUGH-RETRY-ODCID (Mode A Retry limitation, v1 release-note).
- **CF-S22-QPACK-HUFFMAN:** CLOSED (Huffman gained in production via quiche).

## NOTES FOR S27
- Shared-tree multi-agent hygiene: use `git commit -- <explicit paths>` ALWAYS (S26 hit 2 benign
  index-collisions from `git commit` capturing co-located agents' staged changes; all caught).
- Disk: the full `--all-features` test build + an llvm-cov instrumented build are each ~20-45 GB on
  the shared `eg-target`; `cargo clean` between the ×3 gate and the release/cov builds (S26 did).
- The test-codec crate `lb-h3-testcodec` is the ONLY hand-rolled H3 frame codec left, and it is
  TEST-ONLY (no production linkage); keep it that way.
