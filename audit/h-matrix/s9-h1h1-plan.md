# S9 — H1→H1 R8 Streaming Plan (DRAFT — S8 honest-stop head-start, NOT yet R8-approved)

- Author: `lead` (S8 honest-stop deliverable). **Status: DRAFT.** Authored under
  S8's honest-stop (Phase 2 budget judged unsafe to BUILD a 2nd cell to the bar
  after H2→H1 needed a full remediation round + disk at 21 GB). NOT yet
  lead-R8-approved. At S9 start: (a) refine against the live tree, (b)
  lead-R8-approve with the open questions (§6) answered — exactly as the M-D
  H2→H1 plan was.
- Base for S9: `main` after the S8 promote (H2→H1 / M-D BUILT, tip TBD at promote).
- Cell: **H1→H1** — HTTP/1.1 frontend ⇒ HTTP/1.1 backend. Plane: `lb-l7`
  (`crates/lb-l7/src/h1_proxy.rs`, `H1Proxy::proxy_request` @ ~1073).

---

## 1. Why H1→H1 is PARTIAL (it is NOT a buffering defect)

Unlike H2→H1 (which `Limited::collect()`-buffered), the H1→H1 path **already
streams**: `H1Proxy::proxy_request` passes the live `IncomingBody` straight into
`sender.send_request(req)` (`h1_proxy.rs:1111`) with NO `collect()`, NO
`Limited`, NO body cap. hyper's H1 client codec pipes the inbound body to the
upstream concurrently while `send_request` resolves on the response head. The
response is likewise streamed (`finalize_response` → `body.boxed()`,
`h1_proxy.rs:1342`). So H1→H1 "proxies on a wire" and the 3 functional e2e tests
(`tests/h1_proxy_e2e.rs`) pass.

It is PARTIAL because it lacks the R8 PROOFS, not because it buffers:
- **No non-vacuous, body-size-independent memory gauge.** hyper owns the
  request-body buffering internally; there is no instrumentable in-flight
  window we can assert a `retained ≤ ceiling ≪ body` bound against. (The S7/S8
  bar requires a real gauge + load-bearing inverted probe, not RSS, not a
  constant — see S8 F-MD-3.)
- **No backpressure proof** (no slow-client / slow-upstream causal-chain test).
- **No `MAX_REQUEST_BODY_BYTES` (64 MiB D1 cap) enforcement** on this path —
  an unbounded upload currently streams through uncapped (memory-safe because it
  streams, but inconsistent with H2→H1's cap).

## 2. The R8 approach — M-D-lite: interpose the verified M-D bounded pump

To get a CLEAN non-vacuous gauge + backpressure proof, interpose the **same
bounded in-flight pump M-D shipped in S8** on the H1 ingress, instead of handing
hyper the raw `IncomingBody`. H1 ingress is STRICTLY SIMPLER than H2:
- **No HPACK, no H2 framing, no validate-before-forward ordering** (no h2spec
  faces) → **no zero-dial requirement, no lookahead-for-validation regime.** The
  pump can forward-as-it-arrives immediately (Branch B only; Branch A's
  buffer-then-validate-before-dial is unnecessary). This removes M-D's hardest
  part.
- **Reuse the M-D machinery verbatim** where possible: the bounded `mpsc`
  (depth 8 × 8 KiB = 64 KiB window) → in-flight-counting `StreamBody` →
  hyper http1 sender; the real in-flight-bytes gauge (S8 F-MD-3 fix); the 64 MiB
  cap check; the drain-and-validate on receiver-drop (S8 F-MD-2 fix); the
  response-relay after terminal state.
- **Carry M-D's hard-won fixes forward (do NOT re-introduce the traps):**
  - **HTTP-version mis-frame trap (S8 F-MD-1 root cause):** when rebuilding the
    upstream `Request` around the `StreamBody`, set `parts.version = HTTP/1.1`
    and STRIP `content-length` + `transfer-encoding` so hyper frames the
    streaming body itself. (Inbound is already H1, but an inbound H1 request can
    carry `content-length`; an unknown-length StreamBody + stale `content-length`
    mis-frames identically to F-MD-1. Defense-in-depth, mandatory.)
  - **Receiver-drop ≠ 413** (S8 F-MD-2): a dropped upstream body receiver means
    the backend early-responded/finished, NOT "body too large." Only
    `forwarded_total > MAX_REQUEST_BODY_BYTES` is 413. Drain-and-validate, then
    relay the backend's early response.
  - **Non-vacuous gauge** (S8 F-MD-3): the in-flight counter must track REAL
    occupancy (increment on push, decrement when hyper pulls) — never a constant.

**Reuse strategy (Q-H1, decide at approval):** either (a) extract M-D's pump
into a shared `bounded_request_pump` helper called by both `h2_proxy` and
`h1_proxy` (DRY, but edits the just-verified M-D code → regression risk to a
BUILT cell, needs full M-D re-verify), or (b) mirror the (simpler, Branch-B-only)
pump in `h1_proxy::proxy_request` (duplication, but zero risk to the BUILT M-D
cell). Recommendation: **(b) for S9** (protect the BUILT cell; a later refactor
session can DRY it once both are BUILT), unless the verifier judges the
duplication itself a maintenance hazard. CF-DEDUP-1 applies.

## 3. Net-new = M-D-lite (ingress) + M-E (shared proof machinery)
- **M-D-lite**: the Branch-B-only bounded pump on the H1 `IncomingBody`
  (no lookahead/zero-dial regime). Smaller than M-D.
- **M-E**: the shared R8 proof harness — gauge (`H1_REQ_MAX_RETAINED_BODY_BYTES`,
  or reuse the `H2_REQ_*` gauge if the pump is shared per Q-H1; behind the lb-l7
  `test-gauges` feature added in S8), the stalled-backend non-vacuous memory
  test, the slow-client + slow-upstream backpressure tests with proven causal
  chains, and the inverted load-bearing probe.

## 4. Reuse (already shared, no rework)
`strip_hop_by_hop` (`h1_proxy.rs:1498`), `append_xff/append_via/set_xfp/set_xfh`,
`AltSvcConfig`, `HttpTimeouts`, `BackendPicker`, the `TcpPool::acquire_async →
take_stream() → http1::handshake` dial pattern, and the lb-l7 `test-gauges`
feature (added in S8). Smuggling defenses (`SmuggleDetector::check_all_mode` @
`h1_proxy.rs:961`, + hooks `inspect_request`) run in `handle_inner` BEFORE
`proxy_request` — the pump does NOT touch them; keep them intact.

## 5. Verification bar (BUILT) — the M-D template, H1 flavour
1. **Real-wire** BOTH directions: genuine H1 client → real h1_proxy listener →
   router → real H1 backend; binary (non-UTF-8) request AND response bodies;
   byte-identical. (Use a correct flow-control reader — see S8 verifier-harness
   bug: release per-chunk consumed length, not cumulative.)
2. **Non-vacuous memory** both directions: multi-MiB body through a stalled
   backend; `H1_REQ_MAX_RETAINED_BODY_BYTES` reflects REAL occupancy
   (lookahead-remainder + live in-flight) ≤ ~256 KiB ≪ body; inverted probe
   load-bearing (a whole-body-buffer variant MUST trip it).
3. **Backpressure** both directions, proven causal chain (channel fills → pump
   stops polling inbound → frontend TCP read window stalls → client paused;
   symmetric on the response leg).
4. **Smuggling parity**: client RST / half-close mid-body never seen as a
   complete request at the H1 upstream; upstream conn aborted, not pooled. The
   existing CL/TE smuggle rejections (`smuggle.rs`) still fire pre-pump.
5. **64 MiB cap**: a >64 MiB upload → 413 (NEW behavior on this path; confirm no
   existing e2e test regresses — `h1_proxy_e2e.rs` uses small bodies).
6. **≥80% independent canonical coverage** of the M-D-lite session sub-metric.
7. R3 no-regression (esp. the 3 `h1_proxy_e2e` tests, smuggle tests, keepalive
   cap tests), fmt/clippy clean, R10 gauge flag honored under
   `--workspace --all-features`.

## 6. Open questions to resolve at S9 R8-approval (do NOT self-decide)
- **Q-H1**: pump reuse — shared helper (DRY, risks the BUILT M-D cell) vs
  mirror-in-h1_proxy (safe duplication). §2 recommends mirror; confirm.
- **Q-H2**: does interposing the pump change H1 keep-alive / request-cap
  semantics (`round8_keepalive_count_cap`) or the take-and-discard single-use
  upstream contract (ROUND8-L7-10)? Prove no regression before building.
- **Q-H3**: H1 request **trailers** (chunked trailers) — does the pump forward
  them correctly, and is there an H1 analogue of the H2 pseudo-header-in-trailers
  rejection to preserve? Confirm against the live decoder behavior.
- **Q-H4**: the NEW 64 MiB cap on H1→H1 — is 413-on-exceed the desired product
  behavior here (parity with H2→H1), or should the uncapped-streaming status quo
  be preserved? (Mild product call; default = parity/cap.)

## 7. Size & sequencing
Size **S–M** (smaller than M-D: Branch-B-only, no validate-ordering). After
H1→H1: **H1→H2 / H2→H2** (need M-B, the H2 upstream connector), then
**H1→H3 / H2→H3** (need M-C, heaviest). H1→H1 + the BUILT H3 row + H2→H1 would
make **5 of 9** cells BUILT entering the M-B work.
