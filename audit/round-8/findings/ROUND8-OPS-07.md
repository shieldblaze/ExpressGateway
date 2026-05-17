### ROUND8-OPS-07 — Systemd unit missing modern hardening directives (SystemCallFilter, RestrictAddressFamilies, ProtectKernel*); CAP_SYS_ADMIN absence verified

Reference: `audit/round-8/research/nginx.md` architecture summary ("A privileged master process forks unprivileged workers"); `audit/round-8/research/pingora.md` lesson 18 ("graceful shutdown is a state machine; making the entry function consume self prevents footguns"); generic systemd hardening reference (`systemd-analyze security`).
Our equivalent: `DEPLOYMENT.md:43-87` (the systemd unit *as documented*); no `packaging/expressgateway.service` file in the source tree (`find . -name "*.service"` returns nothing).

Severity: medium
Status:   Verified-Fixed(verifier=verify, 61e678a5)   <!-- packaging/expressgateway.service now a real file with SystemCallFilter/RestrictAddressFamilies/ProtectKernel*/ProtectControlGroups/ProtectProc; Type=notify, WatchdogSec deferred-with-comment (Wave 2, disclosed). systemd-analyze CI gate wired but unrunnable in sandbox. See audit/round-8/verify/ops.md. -->
Status (pre-verify):   Open

Divergence:
- The unit at `DEPLOYMENT.md:49-86` correctly sets `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome=true`, `PrivateTmp=true`, `PrivateDevices=true`, `RestrictSUIDSGID=true`, `RestrictRealtime=true`, `LockPersonality=true`, `MemoryDenyWriteExecute=true`, `ReadOnlyPaths=/`, `DevicePolicy=closed`. Good.
- It also correctly limits `CapabilityBoundingSet=CAP_NET_BIND_SERVICE CAP_NET_ADMIN CAP_BPF` — the Round-1 SEC-2-11 finding to drop `CAP_SYS_ADMIN` is reflected.
- **Missing hardening directives** the unit *should* have for a 2026-era systemd unit:
  - `SystemCallFilter=@system-service` (or a narrower allow-list). Catches a class of post-compromise primitives.
  - `SystemCallArchitectures=native`. Stops a 32-bit syscall trampoline if an attacker tries to invoke a syscall through compat ABI to bypass `SystemCallFilter`.
  - `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6 AF_NETLINK` (the last only when XDP is enabled; otherwise drop it).
  - `RestrictNamespaces=true`.
  - `ProtectKernelTunables=true`, `ProtectKernelModules=true`, `ProtectKernelLogs=true`.
  - `ProtectControlGroups=true`.
  - `ProtectProc=invisible` (and `ProcSubset=pid` when no Java-like JVM introspection needed).
  - `PrivateUsers=true` (often incompatible with CAP_BPF — but at least documented).
  - `UMask=0077`.
- **No actual file**: `DEPLOYMENT.md` documents the unit as a code block. The audit register entry REL-2-01 + REL-2-14 recommendations both called for shipping the unit as a real file at `packaging/expressgateway.service` so a smoke job could `systemd-analyze` it. That step is unstarted.

Impact:
- An operator following the runbook copies the code-block unit. The unit is *correct* but not maximally hardened. The delta between "what we documented" and "what `systemd-analyze security` would call green" is substantial — Envoy's reference container image and Pingora's deployment guide both bake the @system-service syscall filter in.
- Without `packaging/expressgateway.service` shipped as a file, no CI gate catches drift in the documented unit. If the next docs change accidentally drops `NoNewPrivileges=true`, doc-lint won't catch it.

Recommendation:
1. Add `packaging/expressgateway.service` with the full set above. Reference DEPLOYMENT.md from the file's preamble comment.
2. Add CI job `systemd-analyze-security`:
   ```bash
   systemd-analyze --no-pager security packaging/expressgateway.service
   ```
   Fail if score worse than 1.5 ("OK" tier). Run on the same matrix as `doc-lint`.
3. Update `DEPLOYMENT.md` to ship the unit *by reference* (link to `packaging/expressgateway.service`) instead of by inline code block. Keep the rationale text inline.
4. Document the `CAP_BPF` + `PrivateUsers=true` interaction in the file header; some kernels disable `CAP_BPF` inside a user namespace.
5. Add `[Service] WatchdogSec=15` and use `sd_notify(WATCHDOG=1)` from a 5-second tick — this gives systemd the ability to kill+restart on hung-but-not-crashed states (e.g. tokio runtime livelocked). HAProxy and nginx both support this.

Notes:
- Round 6 / 7 audited the capability matrix (correct) but did not score the unit against `systemd-analyze`. The unit is *defensible* against the Round-1 finding (SEC-2-11) but does not meet the 2026 baseline.
