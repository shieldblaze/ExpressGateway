# SESSION 16 — handoff (post-S15)

> S15 shipped Mode A (QUIC passthrough, never-decrypt CID routing) to main.
> S16's headline is **Mode B** + closing S15's carry-forwards.

## Start here
1. `git checkout main && git pull --ff-only` — S15 A4 merge is the tip
   (`audit/quic/s15-final-report.md` exists on main = completion signal).
2. Read `s15-final-report.md` (one page) + `s15-design.md` §7 (Mode B sketch).
3. The disk-guard (`/home/ubuntu/Code/eg-disk-cleanup.sh`) + its lessons
   ([[disk-cleanup-loop-must-not-race-builds]]) are reusable — relaunch it
   detached at session start; build with `CARGO_INCREMENTAL=0`; only ONE
   agent runs `--workspace llvm-cov` (the verifier), builders self-check `-p`.

## S16 scope (in priority order)

1. **Mode B — raw QUIC stream/datagram proxy** (design §7). Unlike Mode A
   (CID-routing passthrough), Mode B terminates the QUIC transport and
   proxies application streams/datagrams. Reuse map from S15:
   - SHARED-1 `lb_quic::public_header::parse_public_header`
   - SHARED-2 `lb_quic::udp_dataplane::{UdpDataplane, TokioUdp}`
   - `lb_quic::passthrough::build_retry_packet` (1000-case differential-verified)
2. **CF-S15-PASSTHROUGH-FEATURE-GATING-DEEP** — the `quic-upstream` feature
   refactor so a passthrough-only binary links zero quiche termination
   symbols (A2 gate (ii)). ~42 use sites across lb-io/lb-l7/lb. Good
   stand-alone increment; closes the "NEVER-DECRYPTED LINKAGE" gate properly.
3. **CF-S15-PASSTHROUGH-RETRY-ODCID** — the QUIC PROXY-protocol sidecar
   (token-embedded ODCID + backend extract-on-verify) so LB-mints-Retry
   works with real-quiche backends without the `mint_retry=false` escape.
4. **Per-IP Retry rate-limit** (v1.1 ticket from A3).

## Program-level still unbuilt (post-matrix, from S14 handoff)
WebSockets-over-H2 (RFC 8441), WebSockets-over-H3 (RFC 9220),
gRPC-over-H3 conformance, full h3spec pass, chaos/soak suite.

## Process notes from S15
- The verifier agent repeatedly launched long jobs (llvm-cov, ×3 gate) via
  a raw `&` detached subshell, which doesn't wake it on completion → it
  registered idle and needed poking. Brief teammates to use the harness
  `run_in_background` + TaskOutput-block (which DOES notify on completion),
  never a raw `&` detach. [[feedback_auto_restart_stalled_agents]] still
  applies for genuine hangs.
- Disk on the 67G box is tight with the shared target dir: `debug/` (~33G)
  and `llvm-cov-target` (~17G) coexisting nearly ENOSPC'd twice. The two
  are mutually-exclusive working sets — emergency `rm debug/` is safe during
  an active llvm-cov run.
