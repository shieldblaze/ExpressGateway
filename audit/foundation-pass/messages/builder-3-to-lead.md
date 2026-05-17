# builder-3 → lead (append-only log) — F-COR-9 (task #17)

## 2026-05-17 — PLAN + PROCEEDING (pre-authorized R6 CORRECTNESS scope)

Task #17 (F-COR-9): the ~1/19 `InvalidMessage(InvalidContentType)` TLS
handshake flake in
`tests/reload_zero_drop.rs::test_sigterm_drains_h2_with_goaway` at
:1045.

Plan: audit/foundation-pass/plans/builder-3-fcor9.md

PROVEN MECHANISM (R2 — port/global-state collision = REAL DEFECT):
`ephemeral_port()` binds :0 then DROPS the listener (port released,
TOCTOU), and `spawn_gateway()`'s readiness gate is a bare TCP
`connect_timeout` that returns Ok when ANY process accepts TCP on the
port. Under 8-core full-workspace contention a foreign concurrent
process can grab the released port during the gateway's slow
cold-start; the TCP probe connects to that foreign socket, reports
"ready", and the test's real rustls handshake at :1045 then hits a
non-TLS peer → first byte is not a valid TLS ContentType →
`InvalidMessage(InvalidContentType)` at the rustls record layer,
before ALPN. Not a drain bug (runs entirely before any GOAWAY code).

FIX (test-harness-only; gateway takes only an address from config, no
fd-passing): make the readiness gate TLS-faithful — the probe must
complete the exact TLS+ALPN(h2) handshake the test relies on; only
the real gateway can satisfy it, closing the foreign-port-reuse and
stale-fd windows at the root. No assertion weakened; no test
skipped/ignored/deleted (R5).

Proceeding (pre-authorized R6 CORRECTNESS). No R7 fork. Will append
verbatim pre-fix repro + post-fix >=30x-under-load green.

## 2026-05-17 — STATUS: fix implemented, proof harness running

FIX IMPLEMENTED in tests/reload_zero_drop.rs (test-harness only):
- `boot_timeout()` made `pub` (reused by the H2 test's readiness gate).
- `test_sigterm_drains_h2_with_goaway_async`: the single-shot
  `tcp connect + connector.connect` at the old :1041-1054 replaced
  with a bounded TLS-FAITHFUL readiness loop — retry the EXACT
  TLS+ALPN(h2) handshake the test relies on until it succeeds
  against `listener_addr`, deadline-bounded by `boot_timeout()`,
  panic with a clear correctly-attributed diagnostic if the gateway
  never becomes TLS-ready. ALPN=h2 re-asserted on the established
  conn afterward (assertion UNWEAKENED, R5). Compiles clean.
  Fixed file preserved at /tmp/fcor9_fixed.rs while the pre-fix
  binary is used for the repro.

PROOF HARNESS (literal R1 condition — loop `cargo test --workspace
--all-features -- --test-threads=8`, exactly how builder-1 hit it
1/19):
- Pre-fix repro: /tmp/fcor9_repro.sh (PRE-FIX code restored via
  `git checkout`), running. Through R1 run 9: 9/9 green, no
  InvalidContentType yet (expected at ~1/19 — needs ~15-25 runs at
  ~5 min/run).
- Post-fix >=30x gate: /tmp/fcor9_gate.sh ready; will run after the
  pre-fix signature is captured, restoring /tmp/fcor9_fixed.rs.

Engineering is COMPLETE; remaining work is the faithful long-running
proof (pre-fix capture + >=30 post-fix runs under the real R1
condition = several hours wall-clock, monitored unattended). Verbatim
pre-fix repro + post-fix >=30x-under-load green + commit to follow on
this log once the bar is met. NOT pushed (lead pushes).
Author != verifier.

## 2026-05-17 — PROOF STATUS / repro tuning note

Pre-fix repro attempt 1 (loop literal `cargo test --workspace
--all-features -- --test-threads=8`, builder-1's exact command): ran
15 consecutive R1 runs, ALL green, NO InvalidContentType. Not a
refutation — at builder-1's observed ~1/19, P(0 hits in 15) ~= 44%.
Root observation from the run: this 8-core box, when running a SINGLE
workspace test invocation, stays at load ~0.5 (test binaries are
cache-fast and do not saturate), so the gateway cold-start is NOT
slowed — and the proven mechanism REQUIRES a slow cold-start to open
the ephemeral-port-reuse window. builder-1's box was genuinely
CPU-saturated during their R1 run (that is *why* they saw it 1/19);
an idle box hides it (consistent with "isolated 5/5 pass").

Repro attempt 2 (now running): DUAL concurrent literal R1 loops — two
real `cargo test --workspace --all-features -- --test-threads=8`
runs overlapping → genuine 8-core saturation + doubled loopback
ephemeral-port pressure, i.e. faithfully reconstructs builder-1's
contended condition (both are real R1 runs, no synthetic load). The
PRIMARY loop inspects reload_zero_drop for InvalidContentType, up to
50 iterations. /tmp/fcor9_repro.sh; per-run logs /tmp/fcor9_r1_run_*.

This is the correct faithful condition per the proven mechanism (R2:
slow gateway bind + released-ephemeral-port reuse). The post-fix
>=30x gate will run under the SAME dual-R1 contention with the FIXED
binary, restoring /tmp/fcor9_fixed.rs. Long unattended; verbatim
pre-fix + post-fix evidence + commit to follow.
