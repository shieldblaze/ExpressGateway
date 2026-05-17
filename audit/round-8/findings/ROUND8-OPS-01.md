### ROUND8-OPS-01 — README claims "Zero-downtime reload via SO_REUSEPORT and FD passing"; FD-passing is unimplemented

Reference: `audit/round-8/research/pingora.md` lesson 14 (Pingora `Server::run_forever` SIGQUIT + FD passing over `upgrade_sock`, EXIT_TIMEOUT=300s) + handoff item "Hot reload semantics — listener handover model"; `audit/round-8/research/haproxy.md` architecture summary ("Hot reload via master-worker socket inheritance"); `audit/round-8/research/nginx.md` architecture summary ("Hot reload via master → worker handoff").
Our equivalent: `README.md:23` ("Zero-downtime reload via `SO_REUSEPORT` and FD passing."); `CONFIG.md:136` ("FD-passing is deferred to Pillar 3b follow-up."); `crates/lb/src/main.rs` — `grep "UnixListener\|FD passing\|upgrade_sock"` returns zero hits.

Severity: high
Status:   Verified-Fixed(verifier=verify, 66611b91+61e678a5)   <!-- grep -rniE 'zero.downtime|FD passing|SO_REUSEPORT.*fd' README/RUNBOOK/DEPLOYMENT returns nothing; doc-lint STALE_PATTERNS now guards FD-passing + zero-downtime-via-FD. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- Three production references (Pingora, HAProxy, nginx) implement hot reload by transferring listening FDs from the old process to the new one (Pingora) or by relying on the master forking children that inherit the listening socket (nginx, HAProxy). The user-observable invariant is: a SIGHUP / SIGQUIT replaces the binary without dropping the listen-side socket.
- Our README presents "Zero-downtime reload via `SO_REUSEPORT` and FD passing" as a shipped feature. CONFIG.md, on the same source tree, says FD-passing is deferred. The runbook at `RUNBOOK.md:113-122` says SIGHUP is "not yet wired" and that "Any TOML edit" requires a full restart.
- `SO_REUSEPORT` alone is *not* zero-downtime reload — it permits two processes to share a listen-port at the kernel level, but unless the old process explicitly drains and hands over, in-flight TCP connections served by the old process are still severed at shutdown. We set `SO_REUSEPORT` (see `listener_opts()` in `crates/lb/src/main.rs:404-417`) but never spawn a new process that inherits the FDs.

Impact:
- Doc-drift: a kind of bug REL-2-01 was supposed to extinguish. README is the first thing operators read and it advertises a capability we do not have. Following the README path, an operator runs `systemctl reload` expecting zero-downtime, gets the truth from `RUNBOOK.md` only after the deploy bites.
- Future-debt amplification: when FD-passing is actually implemented, the README claim is already "right" and nobody re-tests it. If the implementation lands buggy, no doc change forces a verifier to re-validate it.

Recommendation:
1. Strike the FD-passing claim from `README.md:23` until it is implemented and verified. Replace with: "Graceful drain on SIGTERM with bounded budget (`drain_timeout_ms`, default 10 s) — SO_REUSEPORT permits side-by-side replacement under a process supervisor that handles handover."
2. Extend `scripts/ci/doc-lint.sh` `STALE_PATTERNS` to fail on `FD.?passing` in README/DEPLOYMENT/RUNBOOK unless the marker `doc-lint-allow-fd-passing` is on the same line and the implementation is in scope (a follow-up linter rule that asserts both that the README does not claim what the source does not provide).
3. Track the FD-passing implementation as a discrete pillar follow-up with an acceptance test (`tests/reload_zero_drop.rs` extended to assert *the same* TCP socket FD survives a binary replacement).

Notes:
- The Cloudflare Oxy blog explicitly recommends using systemd socket activation to decouple the socket lifetime from the application lifetime. That model is a strict improvement on our current "rebind on every start" pattern and would let us truthfully claim zero-downtime restart with substantially less code.
