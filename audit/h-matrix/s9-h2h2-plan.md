# H2→H2 R8 Plan — Phase 2 candidate / S10 head-start (lead-drafted)

- Author: `lead` (S9). Drafted while H1→H1 verification runs. Becomes the Phase 2
  build spec if budget is safe, OR the S10 head-start if honest-stopped.
- Cell: **H2→H2** — HTTP/2 front ⇒ HTTP/2 backend. Plane `lb-l7`.

## Why H2→H2 is the right Phase 2 / next pick
It composes **two already-verified halves** with the lowest integration risk:
- **Front ingress = M-D** (the H2 bounded pump in `crates/lb-l7/src/h2_proxy.rs`
  `proxy_request`), BUILT + F-MD-1..4-hardened in S8.
- **Egress = M-B** (`lb_io::http2_pool::Http2Pool`), BUILT in S6 for H3→H2. Its
  request-body type `H2ReqBody = BoxBody<Bytes, Box<dyn Error + Send + Sync>>`
  was deliberately widened so a **streaming, bounded-incremental** body can error
  and **RST_STREAM the upstream** (a truncated request is never presented
  complete — the F-MD-4 discipline is already in M-B's type).
- The existing **H2→H1** cell is literally "M-D pump → H1 upstream sender." H2→H2
  swaps ONLY the egress: "M-D pump → M-B `Http2Pool::send_request`." That is the
  smallest possible delta over a BUILT cell.

(Contrast H1→H2: pairs the just-built H1 M-D-lite pump with M-B — also viable,
but reuses the NEWEST ingress rather than the most battle-tested one. Per the
session rule "front-ingress more directly reusable," M-D (H2) is the safer half.)

## Net-new (small): the M-D→M-B egress wiring
1. A new dispatch in `h2_proxy.rs` (or `H2Proxy`): when the picked backend is
   `UpstreamProto::H2`, route to an `proxy_h2_to_h2` path that runs the SAME M-D
   pump but feeds the resulting `StreamBody` into `Http2Pool::send_request`
   (`H2ReqBody`) instead of the http1 sender.
2. Error-type bridge: M-D's pump uses `PumpAbort`; M-B wants
   `Box<dyn Error + Send + Sync>`. `PumpAbort` already impls `Error`, so
   `.map_err(|e| Box::new(e) as _)` on the StreamBody — no new abort type, no edit
   to the gauge. The FIFO Err-before-verdict + is_end_stream gating carry over
   verbatim (M-D is the source of truth).
3. Response relay: M-B returns a streaming H2 response body; relay it through the
   existing H2 response path (mirror H3→H2's `upstream_response` handling).

## Carried-forward (mandatory — same traps)
- F-MD-1 (version/CL/TE framing): N/A in the same way (H2→H2 is H2 both sides),
  but STRIP hop-by-hop + set `:authority`/`:scheme` correctly for the H2 upstream
  (reuse the H3→H2 / `translate_*` request-build helpers).
- F-MD-2 (receiver-drop ≠ 413): M-D's drain-and-validate already handles it;
  ensure the M-B early-response (backend sends HEADERS before reading body) path
  relays the backend response, not a manufactured 413.
- F-MD-3 (non-vacuous gauge): reuse `H2_REQ_MAX_RETAINED_BODY_BYTES` (M-D's gauge)
  — the pump is M-D's, so the gauge is already correct and load-bearing.
- F-MD-4 (RST≠EOF): inbound H2 RST_STREAM mid-body → `PumpAbort` → M-B
  RST_STREAMs the H2 upstream (both halves already enforce this; the seam must
  not swallow the Err). **This is the Phase-0a regression test for the new cell**:
  real-wire client RST mid-body → upstream `complete=0`.

## Open questions to resolve at build-time (do NOT self-decide; mirror Q-pattern)
- **Q-HH-1**: dedup — does `proxy_h2_to_h2` share M-D's pump via a helper (now
  TEMPTING since both H2→H1 and H2→H2 use the identical H2 pump) or mirror? With
  TWO H2-ingress consumers this is the natural CF-DEDUP-1 trigger — but extracting
  edits the BUILT M-D cell (re-verify cost). Decide at approval.
- **Q-HH-2**: H2→H2 trailers end-to-end (inbound H2 trailers → M-B upstream
  trailers frame); confirm M-B forwards a trailers frame and the
  pseudo-header-in-trailers reject still fires.
- **Q-HH-3**: does M-B's per-upstream-connection stream multiplexing interact with
  the M-D single-stream pump assumptions (e.g. concurrent-stream cap, GOAWAY
  mid-stream)? Prove no regression to H3→H2 (M-B's existing consumer).

## BUILT bar (same as H1→H1, H2 flavour)
Real-wire both directions (genuine H2 client → real h2_proxy listener → router →
real H2 backend), binary bodies byte-identical; non-vacuous body-size-independent
memory + inverted probe; backpressure both legs; F-MD-4 RST-mid-body `complete=0`;
64 MiB cap → 413; ≥80% independent coverage on the seam's session sub-metric;
R3 (H3→H2 / H2→H1 / M-B consumers intact), fmt/clippy clean, R10 honored.

## Sequencing after H2→H2
Then **H1→H3 / H2→H3** (need M-C, the heaviest — H3 upstream via QUIC). H2→H2
done would make **6 of 9** BUILT.
