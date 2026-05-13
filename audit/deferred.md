# Deferred findings — production-readiness audit

Findings recorded here have been triaged out of the Round-3 fix
cadence with the team-lead's pre-emptive acknowledgement. Each entry
states the rationale; the user's final sign-off lands at FINAL.

---

## sec

### SEC-2-13 — 0-RTT on TCP/TLS listener (info)

**Status**: deferred — **closed-as-not-a-bug**.

`build_server_config` in `crates/lb-security/src/ticket.rs:319-338`
never assigns `max_early_data_size`; rustls 0.23.38 defaults this to
`0`, so 0-RTT is disabled by construction on every TCP/TLS listener.
There is no live attack surface today.

**Defence-in-depth follow-up** (will be folded into the SEC-2-01 fix
PR as a one-line additional test, no separate plan required): add
`#[cfg(test)] fn test_zero_rtt_disabled_invariant()` in `ticket.rs`
that builds the default config and asserts `max_early_data_size ==
0`. If a future change enables 0-RTT, this finding **must be
re-opened as critical** because the TCP path has no replay guard.

### SEC-2-15 — Hyper 1.9.0 smuggling-defence reference matrix (info)

**Status**: deferred — **reference material, not actionable on its
own**.

SEC-2-15 documents what hyper catches at the wire-decoder level vs.
what the gateway must guard against above hyper. The actionable
output of this analysis is **already folded into SEC-2-01's plan**
(the strict TE-codec policy and the `Transfer-Encoding: gzip,
chunked` rejection). No separate fix is needed.

### SEC-2-16 — Atomic-ordering hand-off list for `code` (info)

**Status**: deferred — **handed off to code under CODE-2-04**.

Per lead decision in synthesis §E.3, `code` owns the per-site atomic
ordering audit as a single workspace-wide plan; SEC-2-16 is the
input list. No separate sec plan is authored. Sec will review code's
CODE-2-04 plan when it lands.

---

(Other areas append their own deferred sections below.)
