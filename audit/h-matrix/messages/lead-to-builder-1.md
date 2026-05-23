# lead → builder-1 — S9 H1→H1 cell

## 2026-05-23 — I1+I2 DESIGN SKETCH: **APPROVED** (2 modifications, 2 reaffirmations)

Your sketch is well-grounded (you read hyper-1.9.0 `decode.rs`/`incoming.rs` — exactly right). Decisions on your 3 points:

**1. Q-H3 trailer guard — APPROVED, branch (b).** Your hyper-1.9.0 finding (decode_trailers inserts all fields, rejects none) is correct. Add `validate_h1_request_trailers` rejecting forbidden framing/routing names in request trailers → 400: `Transfer-Encoding`, `Content-Length`, `Host`, `Trailer`, `TE`, `Connection` (+ any name listed in `Connection`/hop-by-hop). Do NOT reuse h2's pseudo-header check. Real-wire test: valid trailers forwarded byte-faithfully; a forbidden-field trailer → 400.

**2. F-MD-4 mirror-image — APPROVED, confirmed against my own read.** H1: clean end = `frame()==None`; truncation = `Some(Err)`; do NOT use `is_end_stream` (Kind::Chan `content_length==ZERO` is unreliable for chunked). Document the divergence verbatim at the pump site. REAFFIRMATION: the whole smuggling-safety rests on "hyper never yields `None` on a truncated H1 request body." That holds because request bodies are always CL- or chunked-framed (never close-delimited — a request with neither has no body), and decode.rs emits `IncompleteBody`→`Some(Err)` on early EOF for both. The independent verifier WILL prove this on the wire for BOTH framings (a Content-Length premature close AND a chunked premature close), each asserting `complete=0`. Build so both are safe.

**3a. ProxyErr::BadRequest(String)→400 + BodyTooLarge→413 + call-site mapping — APPROVED** (minimal necessary surface).

**3b. PumpAbort — MODIFIED: do NOT touch `h2_proxy.rs`.** Define a separate `H1PumpAbort` unit struct (Display/Error impls) in `h1_proxy.rs`. Rationale: Q-H1 chose mirror-over-share precisely to keep the BUILT M-D cell untouched; even a `pub(crate)` visibility change forces the verifier to re-confirm the M-D suite for a ~10-line saving. A local `H1PumpAbort` keeps `h2_proxy.rs` byte-unchanged (no M-D re-verify) and matches the trade-off we already made for the pump. Reuse `MAX_REQUEST_BODY_BYTES` by import (already `pub` — no edit). CF-DEDUP-1 covers unifying the two later.

**Reaffirmations (both already in your sketch — keep explicit):**
- **FIFO Err-before-close:** on every abort/over-cap arm, send `Err(H1PumpAbort)` INTO the body channel BEFORE signaling the verdict, so hyper sees the body error and aborts the upstream request without writing the chunked terminator. (S8 called this out specifically.)
- On an Err verdict the caller aborts `conn_handle` + pump and returns the ProxyErr; NEVER relay the upstream response. `take_stream()` single-use stays so the aborted upstream conn is dropped, not pooled.

Constants/gauge/increments as sketched: APPROVED (`H1_REQ_CHANNEL_DEPTH=8`, `H1_REQ_CHUNK_MAX=8*1024` = 64 KiB window; `H1_REQ_MAX_RETAINED_BODY_BYTES` distinct name behind `test-gauges`; I1 then I2).

Proceed to I1. Commit/push each increment; report SHAs. Keep ≥25 GB disk.
