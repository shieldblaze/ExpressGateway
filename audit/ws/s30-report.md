# SESSION 30 — REPORT: WS-over-H2 backpressure → CF-S27-2 feasibility finding

**Verdict: SESSION 30 COMPLETE — WS-H2 un-gate NOT-VIABLE this session.**
No contained in-repo fix exists; the H2 stack is untouched (zero source
change); WS-H2 stays GATED, honestly. CF-S27-2 carried forward as a
documented **hyper limitation** (not an ExpressGateway defect), with the
sharpened mechanism + the escalated fork. Owner decision: stay gated
(Exit-b); first step on the fork = file the hyper upstream bug.

This was a HARDENING session. The PROTOCOL SPEC remains COMPLETE and does
NOT regress.

---

## 1. What was asked
Close CF-S27-2 by giving the gated WS-over-H2 (RFC 8441) relay TRUE
window-aware backpressure so it can be un-gated on-by-default — IF that can
be done cleanly without regressing the mature, h2spec-passing H2 stack (R3).
Otherwise, prove the clean fix isn't feasible and keep WS-H2 gated (an
explicitly acceptable, honest outcome — R6/R11(b)).

## 2. Outcome
The clean fix is NOT feasible in-repo this session. The pre-authorized
direction ("window-gated writes on the raw `h2::SendStream`, bypassing
hyper's `Upgraded` for the WS-tunnel path ONLY") assumed a seam that **does
not exist** in hyper's public surface. The only ways to obtain the raw
`SendStream` are (a) owning the whole H2 connection codec = re-implementing
general H2 serving (the R3 crown-jewel risk / R7(a) STOP), or (b) forking
hyper (a dependency-governance decision / R7(b)). Neither is a contained
in-repo fix. WS-H2 stays gated.

Full analysis + line-level evidence: `audit/ws/s30-feasibility.md` (lead) and
`audit/ws/s30-feasibility-verifier.md` (independent verifier). Both readers
derived the same root cause and the same NOT-VIABLE verdict from primary
source, independently (author ≠ verifier, R5).

## 3. Phase 0 baseline (completed runs)
| Check | Result |
|-------|--------|
| Base tip | `main @ f5a04b2b` (S29) confirmed |
| Branch | `feature/ws-h2-backpressure-s30` pushed |
| Hygiene | 45 GB free, no S29 strays |
| Build | `cargo build --workspace --all-features` clean (exit 0) |
| **h2spec strict** | **`h2spec -S` exit 0 — 146 passed / 1 applicability-skip / 0 failed. Crown-jewel intact.** |

## 4. F-S27-2 reproduced — the R8 negative-control reference (R13(c))
`cargo test --test ws_r8_backpressure_plateau --release -- --include-ignored
--test-threads=1` (flood 2048 × 256 KiB = 512 MiB at a non-reading client;
plateau ceiling 256 msgs):

| transport | pushed / 2048 | in-flight | verdict |
|-----------|---------------|-----------|---------|
| **H1 (shipped)** | **20 / 2048** | ~5 MiB | BOUNDED — PASS |
| **H2 (gated)** | **2048 / 2048** | ~512 MiB | **UNBOUNDED — FAIL (the DoS)** |

The H2 path absorbs the entire flood into RAM at a stalled consumer. This is
the load-bearing control: a real fix must flip H2 to a flat plateau < 256,
volume-independent. The H1 result proves the relay *logic* is correct — the
gap is purely the H2 transport's missing window gating.

## 5. Root cause (primary source; both readers agree)
The relay forwards via `client_tx.send(msg).await` (ws_proxy.rs:392); over H2
the sink is tungstenite over hyper's `Upgraded` → `H2Upgraded`
(hyper-1.9.0 `proto/h2/upgrade.rs:39`, `pub(super)`).
1. `H2Upgraded::poll_write` gates only on an `mpsc::channel(1)`
   (upgrade.rs:21,205,222) — never on the H2 window.
2. `UpgradedSendStreamTask::tick` drains that mpsc into `send_data`
   UNCONDITIONALLY: a closed window makes `poll_capacity` return
   `Poll::Pending`, which is *swallowed* (`break 'capacity`, upgrade.rs:98),
   then `send_data` runs anyway (upgrade.rs:116-120). The task returns
   `Pending` only when the mpsc is empty (upgrade.rs:128) — never because the
   window closed. ⇒ the relay's `send().await` never parks over H2.
3. `h2::SendStream::send_data` buffers UNBOUNDED with no window (h2-0.4.13
   `share.rs:56-59`, `share.rs:326-331`; `prioritize.rs:218` pushes onto an
   uncapped `pending_send`).
4. hyper's `max_send_buffer_size` (64 KiB) only caps the *reported*
   `capacity()`, not the buffer; and the upgrade `tick` bypasses it anyway.
5. Not fixed in newer hyper — `upgrade.rs` is byte-identical in 1.10.1 (one
   cosmetic `.unwrap()`→`.expect()`). A lockfile bump does NOT help.

## 6. Why no contained seam (the feasibility wall)
- **No knob** bounds the upgraded stream (pt 4).
- **No downcast:** `H2Upgraded` is `pub(super)` (not nameable); and it holds
  only the mpsc `Sender`, not the `SendStream` (moved into hyper's private
  spawned task, server.rs:495-503).
- **No flow-controlled-body alternative:** hyper hard-routes any CONNECT
  through the upgrade fork — request body captured into private
  `ConnectParts.recv_stream` (server.rs:296), response body rejected for a
  2xx CONNECT (server.rs:478-504). (Hyper's NORMAL request/response body path
  DOES backpressure — which is why the H2→H1/H2/H3 cells pass R8 + h2spec —
  but a CONNECT cannot use it.)
- **Owning the codec is the only door**, and H2 multiplexes regular requests
  + CONNECT on one connection, so you can't peel one stream out of a
  hyper-owned connection — you'd re-host general H2 serving = R3 crown-jewel
  risk = R7(a) STOP.

## 7. H2-stack-intact evidence (R3)
**Zero source/config delta vs `main @ f5a04b2b`** — `git diff main...HEAD
-- '*.rs' '*.toml' '*.lock'` is EMPTY; the branch adds only `audit/ws/*.md`.
The spec-complete code is therefore byte-identical to S29's promoted,
×3-green main, so every spec invariant (9 cells, WS-H1/H3, gRPC-H3, both QUIC
modes, the migration, the soak scenarios) is preserved by construction. Two
direct corroborations this session: build clean (exit 0); **h2spec strict
exit 0** (146/147). The crown-jewel was never at risk because nothing was
touched.

## 8. Escalated fork + owner decision (R7)
Owner ruled **stay gated (Option 1)**. Rationale endorsed:
- **Option 3 (broad rearchitect)** — REJECTED: trades the h2spec 146/147
  crown-jewel for one gated feature; R3 forbids it.
- **Option 2 (vendored hyper fork)** — NOT this session, NOT discarded. It is
  the technically-correct fix (~10-20 lines, isolated to the upgrade path;
  general serving uses `PipeToSendStream`, unaffected), but it means carrying
  a maintained fork of the most security-critical dependency (hyper — the
  source of h2spec 146/147) until upstreamed. That is a deliberate
  supply-chain/governance decision, the opposite of the migration just done,
  and must not land under end-of-session momentum. **First step: file the
  hyper upstream bug** (the unconditional `send_data` on a closed window is a
  real latent hyper backpressure bug). If hyper fixes it upstream, the
  problem dissolves with zero fork burden. If the interim vendored fork is
  later deemed worth it, it gets its OWN session with the full un-gate + R8 +
  h2spec-re-verify + re-soak bar.

## 9. CF-S27-2 — carried, reframed
CF-S27-2 carries forward as a **documented hyper limitation**, NOT an
ExpressGateway defect: WS-over-H2 (RFC 8441) is gated off-by-default because
hyper's upgraded extended-CONNECT write path does not propagate H2
flow-control backpressure. ExpressGateway's own relay logic is correct
(proven by the bounded H1 path, same single-sourced `proxy_frames`). The
spec stays complete; WS-H1 (RFC 6455) + WS-H3 (RFC 9220) + gRPC-over-H3 all
ship with TRUE backpressure, so WS-H2-gated is a narrow, documented
limitation — not a functional gap. Recommended next action: the upstream
hyper issue (§8).

## 10. No-promote-of-fake-backpressure (R4/R11)
No fix was shipped; no source changed; no backpressure was claimed that
isn't real. The always-on `read_frame` write-timeout (Close 1008) remains a
TIME-bound mitigation even when ungated, explicitly NOT presented as the
window-gated fix. R4 (no asterisk) honored: the finding is "the clean fix
isn't feasible in-repo," fully documented with mechanism.

## Provenance (R15)
Every claim cites a completed run or vendored file:line. F-S27-2 repro:
finished 10.10s (H1 20/2048 PASS, H2 2048/2048 FAIL). h2spec strict: exit 0.
Code-delta-empty: `git diff main...HEAD`. Source: vendored under
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/`.
