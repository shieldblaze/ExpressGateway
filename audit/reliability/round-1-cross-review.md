# Round 1 — Cross-Review notes (`rel`)

Round 1 is discovery; full adversarial cross-review happens in round 2.
This file pre-stages the disagreements I expect, so they can be resolved
quickly once findings land.

## Anticipated overlaps

| With | Topic | My posture |
|------|-------|------------|
| `sec` | `/healthz` auth posture | Agree it can stay unauth on loopback; I will not push for mTLS in round 2. |
| `sec` | TLS ticket-key persistence | If `sec` recommends persisting the ticket key on disk for restart-resumability, I will support iff the file gets `0600` + tmpfs option for replicas-as-cattle environments. |
| `code` | "Unbounded TCP spawn" (F-17) | We will both want this finding. Suggest `code` owns the implementation; I own the runbook + saturation metric. |
| `code` | `accept` tight loop on EMFILE | Same — joint finding. I will tolerate `code` owning the fix and leave `rel` on the metric (`accept_errors_total{kind}`). |
| `ebpf` | XDP cleanup on panic | If `ebpf` says aya already handles Drop-time detach, I withdraw F-20's residual concern. |
| `proto` | GOAWAY on SIGTERM | I claim no current emission; if `proto` finds an emission path I missed, I withdraw the H2 portion of H7. |
| `proto` | Connection draining ordering | Whoever lands GOAWAY also lands the `/readyz` 503 flip; I want the order to be `/readyz=503` → `Connection: close` / GOAWAY → 30 s grace → abort. |

## Likely disagreements

1. **Drain budget.** `code` may argue 2.5 s is fine because it matches
   the existing test fixture. I'll argue the *unit file claims 30 s*; we
   either honour that or shrink it explicitly. Compromise: 10 s default,
   configurable via `[runtime].drain_timeout_ms`.
2. **`/healthz` body.** Some teams want `application/json` health
   payloads; I prefer plain text + structured fields. Low stakes.
3. **`spawn_blocking` for connect.** I see this as a sharp edge
   (blocking-pool starvation). `code` may argue it matches Pingora.
   Counter: Pingora does this *inside* a bounded executor; we do it on
   the global tokio blocking pool.

## Things I will NOT contest

- The `ArcSwap` / `TlsStore` claim was almost certainly aspirational, not
  malicious. I will phrase round-2 findings as "doc/code drift", not
  "deceptive doc".
- The audit's owners-only-write rule on `crates/lb-l4-xdp/`: that's
  `ebpf`'s house.

## Cross-talk artifacts

The SendMessage / shared-task tool surface is not loaded into this
session. I am leaving teammate-addressed summaries as
`audit/reliability/msg-to-<name>.md` files for the team-lead to relay.
If the SendMessage tool surfaces in round 2, I'll re-send them through
the official channel.
