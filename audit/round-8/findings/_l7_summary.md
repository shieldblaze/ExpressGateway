# L7 divergence audit — summary

Author: `div-l7`, Round-8, 2026-05-14.

## Coverage

- **Total candidate divergences examined**: 35 (15 findings + 20 resolved)
- **Findings opened**: 15 (`ROUND8-L7-01` through `ROUND8-L7-15`)
- **Resolved (different-but-fine)**: 20 entries in `audit/round-8/divergence/l7-resolved.md`

## Findings by severity

| Severity | Count | IDs |
|----------|-------|-----|
| Critical | 0     | — |
| High     | 2     | ROUND8-L7-01 (premature 101), ROUND8-L7-02 (chunk-size hex) |
| Medium   | 10    | L7-03 (empty header name), L7-04 (XFF clobber), L7-05 (underscores), L7-06 (keepalive_requests), L7-07 (h2_recv_timeout), L7-08 (h2 RST_STREAM(CANCEL)), L7-09 (authority comma), L7-10 (over-read pool), L7-13 (path normalisation), L7-15 (edge defaults) |
| Low      | 3     | L7-11 (lb-h2 PADDED — test-only today), L7-12 (no glitches counter), L7-14 (fuzz seed corpus) |
| Info     | 0     | — |

## Lane crossovers flagged

- **ROUND8-L7-14** (drain randomisation, hot-reload model) → div-ops scope, called out in resolved R-L7-14.
- **ROUND8-L7-13** (path normalisation impacting routing) → div-ops for any future host/path routing pillar.
- No L4 crossovers — QUIC routing is correctly DCID-based (R-L7-01 resolved).

## Top-3 surprises

### 1. Premature 101 Switching Protocols (ROUND8-L7-01) — `Pingora-class CVSS 9.3`

Our `handle_ws_upgrade` flips the wire to WebSocket protocol BEFORE the upstream WS handshake completes. Pingora paid for this exact bug at GHSA-xq2h-p299-vjwv (Critical 9.3) and Envoy at GHSA-rj35-4m94-77jh — independently, in the same release window. The previous audit (Rounds 1-7) has WebSocket coverage at WS-001 / WS-002 (rate-limit + per-frame timeout) but did not pin the "decide-protocol-switch-by-upstream" invariant. Rated High here rather than Critical because the smuggled-second-request angle requires hyper's `OnUpgrade` to surface buffered bytes; the unconditional 101-emit + dial-fail-then-tear-down DoS is the easier exploitation.

### 2. Chunk-size hex parser accepts `+` prefix + unbounded digits (ROUND8-L7-02)

`usize::from_str_radix("+5".trim(), 16) = Ok(5)`. Three big proxies took CVEs on the same primitive (nginx CVE-2013-2028 stack-overflow grade, hyper GHSA-5h46-h7hh-c6x9 overflow-class, HAProxy `h1_append_chunk_size` major-grade). Our chunked decoder calls `from_str_radix` after `.trim()` with no digit cap. The previous audit's smuggle work (SEC-2-15 matrix) covered TE-CL / CL-TE but not the underlying chunk-size lexer.

### 3. `append_xff` clobbers all but the first XFF value (ROUND8-L7-04)

`headers.get(&XFF_NAME)` returns only the first value; `headers.insert(...)` replaces all values. Multi-value XFF chains are silently truncated to one entry before the peer-IP is appended. This is *exactly* the Envoy GHSA-ghc4-35x6-crw5 shape `ref-l7` predicted (handoff prediction #1). The RBAC bypass class lands the moment a downstream policy iterates over `get_all("x-forwarded-for")` versus we who collapsed to one value. Trivial fix; surprising it shipped.

## Methodology notes

- Audited the H1 / H2 / H3 parsers, the L7 proxy entrypoints (`h1_proxy.rs`, `h2_proxy.rs`, `ws_proxy.rs`), the security hooks, the upstream pools (TCP, H2, QUIC), the sni/authority validators, the H2 security thresholds, and the QUIC router.
- Compared every prediction in `ref-l7`'s handoff Top-10 and cross-cutting #1–#5 against current code.
- Spot-checked prior-audit "Verified-Fixed" claims for SEC-2-15 (TE strict), PROTO-2-15 (SNI/authority), PROTO-2-16 (graceful shutdown) — all match their reference well enough to land in the resolved file, not in findings.
- Did NOT trust Round-7 closure for any item — re-read commits where my divergence overlapped.

## Items requiring Phase-C "previous-audit-MISSED" escalation

Three of the fifteen findings overlap topics that prior rounds *claimed* coverage on:

1. **ROUND8-L7-01 (premature 101)** — Rounds 4–6 had WS-001 / WS-002 audits. Neither pinned the protocol-switch ordering invariant. Recommend MISSED tag.
2. **ROUND8-L7-04 (XFF clobber)** — PROTO-2-08 (hop-by-hop strip) and adjacent header-handling audits did not exercise multi-value XFF preservation. Recommend MISSED tag.
3. **ROUND8-L7-02 (chunk-size hex)** — SEC-2-15 smuggle matrix audit covered TE/CL conflict but not the chunked lexer itself. Recommend MISSED tag.

The other 12 findings are either novel ref-l7 lessons (post-2025 CVEs / blog posts not on the previous audit's radar) or design-choice divergences that the previous audit chose not to surface.

## Where to look next (handoff to verify/reconcile)

- `audit/round-8/findings/ROUND8-L7-*.md` — the 15 findings.
- `audit/round-8/divergence/l7-resolved.md` — the 20 resolved divergences (work-evidence).
- `crates/lb-l7/src/h1_proxy.rs:879-1055` for the premature-101 path (the hottest finding).
- `crates/lb-h1/src/chunked.rs:127-158` for the chunk-size lexer.
- `crates/lb-l7/src/h1_proxy.rs:1123-1171` for XFF / Via append.
