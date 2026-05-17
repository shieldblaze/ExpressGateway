# PROTO-2-10 — Smuggle defence matrix

Maps every Content-Length / Transfer-Encoding variant covered by RFC
9112 §6.1 / RFC 9110 §8.6 / RFC 9113 §8.2.2 to:

  - **hyper** — what hyper 1.9.0 catches at the wire-decoder level
    (`hyper::server::conn::http1::Builder` and `http2::Builder`).
  - **detector default** — `lb_security::SmuggleDetector::check_all_mode`
    with `SmuggleMode::H1` (the production default unless the
    `[runtime].strict_te` flag is set).
  - **detector H1Strict** — same with `SmuggleMode::H1Strict`.

Reject status conventions:
  - "400" — caller sees `400 Bad Request` (the proxy renders
    `error_response(BAD_REQUEST, "request smuggling")`).
  - "wire-reject" — hyper closes the connection before the request
    reaches the proxy handler. The detector never sees the request.
  - "pass" — the variant is allowed by the named layer.

## Matrix

| # | Variant | hyper 1.9.0 | detector default (H1) | detector H1Strict |
|---|---------|-------------|-----------------------|-------------------|
| 1 | `Content-Length: 10` (single, well-formed) | pass | pass | pass |
| 2 | `Content-Length: 10` + `Content-Length: 10` (duplicate, same value) | pass | pass (RFC 9110 §8.6 allows identical) | pass |
| 3 | `Content-Length: 10` + `Content-Length: 20` (duplicate, differing) | wire-reject (hyper merges to invalid; closes) | 400 (`check_duplicate_cl`) | 400 |
| 4 | `Content-Length: 10` + `Transfer-Encoding: chunked` (CL+TE) | varies — hyper prefers TE, may close on H1 | 400 (`check_cl_te`) | 400 |
| 5 | `Transfer-Encoding: chunked` (final, only) | pass | pass | pass |
| 6 | `Transfer-Encoding: gzip` (no `chunked`) | wire-reject (final must be `chunked`) | 400 (`check_te_cl`) | 400 |
| 7 | `Transfer-Encoding: gzip, chunked` (chained, final chunked) | pass | pass (final is chunked) | **400** (strict: codec list ≠ `chunked`) |
| 8 | `Transfer-Encoding: chunked, gzip` (final NOT chunked) | wire-reject | 400 (`check_te_cl`) | 400 |
| 9 | `Transfer-Encoding: chunked` + `Content-Length` (CL+TE explicit) | hyper drops CL after TE per RFC 9112 §6.1 — risk on backend | 400 (`check_cl_te`) | 400 |
| 10 | `Transfer-Encoding: identity` | wire-reject | 400 (`check_te_cl`) | 400 |
| 11 | `Transfer-Encoding: ` (empty value) | wire-reject (parse error) | 400 (`check_te_cl` — final encoding empty ≠ chunked) | 400 |
| 12 | H2 request carrying `Transfer-Encoding: chunked` (forbidden by §8.2.2) | hyper-H2 rejects on parse | 400 (`check_h2_downgrade`) | 400 |
| 13 | H2 request carrying `Connection: keep-alive` (forbidden by §8.2.2) | hyper-H2 may pass (header-block validation lax) | 400 (`check_h2_downgrade`) | 400 |
| 14 | H2 request with `TE: trailers` (RFC 9113 §8.2.2 only allowed TE value) | pass | pass | pass |
| 15 | H2 request with `TE: gzip` (forbidden) | pass | 400 (`check_h2_downgrade`) | 400 |
| 16 | Pseudo-header (`:authority`) leaking through H2→H1 bridge | wire-reject from hyper-H2 server | 400 (`check_h2_downgrade`) | 400 |
| 17 | `Content-Length: 10 ` (trailing whitespace) | hyper trims → pass | pass (`first_value.trim()`) | pass |
| 18 | `Transfer-Encoding:  ,  chunked` (leading empty codec) | wire-reject | 400 (`check_te_cl` accepts; check_te_strict rejects empty) | 400 |

## Notes on hyper-shipping defects

The matrix exposes one cell where hyper-1.9.0 is **leakier than the
detector with default mode** (line 7: chained codec `gzip, chunked`).
This is consistent with hyper's RFC 9112 §6.1 reading — the final
encoding being `chunked` is the only invariant the wire-level parser
enforces. The detector's H1Strict mode is the gateway-level guard
for upstreams that mis-implement the codec-chain decode (almost all
non-Pingora upstreams in practice).

No PROTO-2-99-A escalation: every cell where hyper passes a variant
the detector also passes it, or the detector strictly rejects on top
of hyper's wire-level guard. The H1Strict mode collapses cell #7 into
a reject. Operators who run the gateway in front of mis-implementing
upstreams should enable `[runtime].strict_te = true`.

## Wire-up status

The detector runs on every inbound H1 / H2 request as of
Wave-2b commits `e00e85a` (SEC-2-01 hot-path wire-up) and `dc02517`
(CODE-2-01 trait shim). Mode selection per request is controlled by
`H1Proxy::with_smuggle_strict(bool)` (built from the
`[runtime].strict_te` knob in Wave-2c).

## Tests

`crates/lb-l7/tests/smuggle_matrix.rs` exercises cells #3, #4, #7
(both modes), #8, #9, #10, #12, #13, #15 — the rows that distinguish
the three columns. Cells covered by existing
`crates/lb-security/tests/` are not re-tested here.
