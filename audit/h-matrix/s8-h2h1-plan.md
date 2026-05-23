# S8 — H2 → H1 R8 Streaming Plan (DRAFT — honest-stop head-start, NOT yet R8-approved)

- Author: `lead` (S7 honest-stop deliverable). **Status: DRAFT.** This is the
  S8 head-start authored under S7's honest-stop (Phase 2 budget judged unsafe
  to BUILD a second cell to the bar). It is **NOT** yet lead-R8-approved.
  At S8 start it MUST be (a) refined by a builder against the live tree, then
  (b) lead-R8-approved with the open questions (§6) answered — exactly as the
  H3→H3 plan was (`s6-h3h3-plan.md`). No source change until then.
- Base for S8: `main` after the S7 promote (feature/h-matrix-s7 @ `0cd584e7`,
  H3→H3 BUILT).
- Cell: **H2→H1** — HTTP/2 frontend ⇒ HTTP/1.1 backend. Plane: `lb-l7`
  (`crates/lb-l7/src/h2_proxy.rs`), NOT the lb-quic H3 plane.

---

## 1. The defect (why H2→H1 is PARTIAL, not BUILT)

`h2_proxy::ProxyH2::proxy_request` (`h2_proxy.rs:1288`) fully **buffers** the
inbound request body before dialing the upstream:

```
let limited = http_body_util::Limited::new(body, MAX_REQUEST_BODY_BYTES);
let collected = limited.collect().await?;     // h2_proxy.rs:1314
```

This is bounded by the **TOTAL-BODY CAP** (`MAX_REQUEST_BODY_BYTES`), exactly
the anti-pattern R8 forbids: memory scales with body size, not with a fixed
in-flight window. There is no end-to-end backpressure and no memory proof.
Real-wire `tests/h2_proxy_e2e.rs` is 3/3 green but asserts only functional
correctness — no memory/backpressure assertion → **PARTIAL** (proxies on a
wire but buffers), never BUILT.

## 2. The central tension — R8 streaming vs. the h2spec validate-before-forward order

The `collect()` is **not** lazy buffering; the in-source rationale (F-COR-1
A2-2, `h2_proxy.rs:1297-1326`) is a real conformance fix:

> "fully RECEIVE + VALIDATE the inbound request body here, BEFORE dialing the
> upstream. `collect()` drives hyper/h2 protocol validation to completion and
> surfaces any stream/connection error … so a malformed request can never leak
> the backend response — the race window is removed structurally."

i.e. it closes the **validate-vs-forward race**: without it, a static backend's
`200` body could be relayed downstream *before* the malformed-request
rejection (trailers / content-length≠ΣDATA / stream-state / second-HEADERS /
flow-control) lands — h2spec then sees DATA instead of the mandated
RST_STREAM/GOAWAY (≥5 h2spec faces). The session brief states this as binding:
**"H2 ingress MUST preserve the validate-before-forward ordering h2spec
requires (do not forward a request upstream before H2 frame/header validation
completes)."**

So M-D may NOT simply replace `collect()` with a naive incremental pump — that
re-opens the race. The plan must reconcile **bounded-incremental forwarding**
(R8) with **validation completing before any downstream response relay**
(h2spec). This is the hard, net-new part and the reason H2→H1 is sequenced
after the H3 cells.

## 3. Net-new: M-D — bounded H2 INGRESS pump (lb-l7 plane)

M-D = stream the inbound `IncomingBody` with a fixed in-flight window + an R8
gauge, instead of `Limited::collect()`, **while preserving validate ordering**.
Design sketch (to be detailed + R8-approved at S8):

- **Header/frame validation first.** HEADERS pseudo-header + frame-structure
  validation (the parts hyper completes at stream-open / first poll) must
  complete and gate the dial. Forward the request line + headers to the H1
  upstream only after that passes.
- **Bounded DATA pump.** Stream request DATA incrementally from the H2
  `IncomingBody` (poll one frame at a time) into the H1 upstream writer, with
  an in-flight window = a fixed channel depth × chunk-max (mirror the H3 cells'
  `H3_BODY_CHANNEL_DEPTH=8 × 8 KiB` discipline → an `H2_REQ_*` analogue), NEVER
  buffering the whole body. Backpressure: when the H1 upstream write stalls,
  stop polling the H2 body ⇒ hyper withholds H2 flow-control `WINDOW_UPDATE` ⇒
  the H2 client is paused. (Mirror of the H3 cells' causal chain, expressed in
  hyper/h2 terms.)
- **Validate-before-RESPONSE-relay invariant.** The downstream response relay
  must not begin until inbound request validation has either completed cleanly
  or produced an error that is mapped to RST/GOAWAY. Candidate mechanisms to
  evaluate (S8 open question Q-D1): (a) gate the *response head* relay on the
  request body stream having reached a validated terminal state (EOF or a
  surfaced protocol error) — this preserves the h2spec order WITHOUT buffering
  the body, because validation errors on an H2 `IncomingBody` surface as the
  stream yields `Err` incrementally, not only at `collect()`; (b) if (a) proves
  insufficient for a specific h2spec face, fall back to validating only the
  *framing/headers* eagerly while still streaming DATA, and document the
  residual face honestly. The plan FAILS R8 review if it reintroduces
  whole-body buffering to get validation ordering — that is the trap.

## 4. Reuse (verified, no rework)

- **H1 egress**: reuse the verified H3→H1 egress family — `write_h1_request`
  (streaming request → H1 upstream) + `stream_h1_response` (bounded H1 response
  → frontend). H2→H1's H1-backend leg is identical to H3→H1's; only the
  frontend ingress (M-D) is new. (Inventory step 3: "Reuses the proven
  `stream_h1_response`/`write_h1_request` egress.") **S8 open question Q-D2**:
  these live in the lb-quic `h3_bridge.rs` plane; confirm whether they are
  callable from the lb-l7 `h2_proxy.rs` plane or whether the lb-l7 H1 client
  path (`h1_proxy`/hyper H1 sender) is the actual reuse target. Resolve the
  plane boundary BEFORE building (avoids an I0.5-class cross-crate surprise).

## 5. Verification bar (the H3→H1/H3→H2/H3→H3 template, lb-l7 flavour)

To reach BUILT, H2→H1 must (independent verifier, author≠verifier):
1. **Real-wire**: genuine H2 client → real h2_proxy listener → router → H1
   upstream (real socket). Binary (non-UTF-8) request + response bodies.
2. **Non-vacuous memory** both directions: an R8 gauge (a real crate static,
   `--features`-gated) asserting `retained ≤ fixed-window-ceiling ≪ body size`
   for a multi-MiB body, body byte-identical; inverted probe proving the
   assertion is load-bearing.
3. **Backpressure** both directions with a proven causal chain (H2 flow-control
   `WINDOW_UPDATE` withheld under downstream stall).
4. **h2spec ordering preserved**: the malformed-request faces that motivated
   F-COR-1 A2-2 still produce RST_STREAM/GOAWAY (never a relayed backend body).
   Re-run the relevant `tests/h2_proxy_e2e.rs` + any h2spec-derived cases; add
   adversarial cases for the ≥5 faces if not already covered.
5. **Smuggling parity**: client RST mid-request never seen as a complete
   request at the H1 upstream.
6. **≥80% independent canonical coverage** of the M-D session code (the
   stricter session-code sub-metric, per the H3→H3 precedent).
7. R3 no-regression, fmt/clippy clean, R10 gauge flag honored.

## 6. Open questions to resolve at S8 R8-approval (do NOT self-decide)
- **Q-D1**: Does gating the response-head relay on a validated terminal state
  of the request body stream (§3 option a) preserve ALL the h2spec faces
  F-COR-1 A2-2 closed, WITHOUT whole-body buffering? Prove against the specific
  faces before approval.
- **Q-D2**: The plane boundary (§4) — is the verified H1 egress reusable from
  lb-l7, or does H2→H1 use the lb-l7 H1 sender? Settle to avoid an
  lb-io/cross-crate change (A1 escalation contract applies).
- **Q-D3**: Gauge naming/placement for the lb-l7 plane (the H3 cells' gauges
  are lb-quic statics) — define the `--features test-gauges`-equivalent for
  lb-l7 so R10 memory proofs are not silently skipped.
- **Q-D4**: In-flight window constants for the H2 ingress (depth × chunk) —
  pick to mirror the H3 ceiling discipline and document the ceiling formula.

## 7. Size & sequencing
Size **M** (per inventory). After H2→H1: **H1→H1** (S–M; body already streams
via hyper, net-new = R8 gauge + stalled-client memory test = M-D-lite + M-E),
then **H1→H2 / H2→H2** (need M-B), then **H1→H3 / H2→H3** (need M-C, heaviest).
