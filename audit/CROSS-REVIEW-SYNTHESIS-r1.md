# Round 1 — Cross-Review Synthesis (team-lead)

The teammates did not have a `SendMessage` channel in their subagent
context this round, so they wrote per-teammate handoff blocks in their
own cross-review files. This is the lead-level synthesis: convergent
themes, scope-ownership map for Round 2 findings, and the small number
of genuine disagreements that need a decision.

Inputs:
- `audit/security/round-1-inventory.md` + cross-review (sec)
- `audit/code/round-1-inventory.md` + cross-review (code)
- `audit/ebpf/round-1-inventory.md` + cross-review (ebpf)
- `audit/reliability/round-1-inventory.md` + cross-review + msg-to-* (rel)
- `audit/protocol/round-1-inventory.md` + cross-review (proto)

## A. Convergent themes (≥2 teammates flagged independently)

| # | Theme | Flagged by | Round-2 owner |
|---|-------|------------|---------------|
| T1 | **Fictional defenses**: `SmuggleDetector`, `SlowlorisDetector`, `SlowPostDetector`, per-IP cap — all live in `lb-security` but `lb-l7` does NOT depend on it. Zero wire-up in the proxy hot path. | sec (S-1/S-3/S-4/S-10), code (Q-CODE-1-01), proto (#1), rel (F-17) | `sec` owns wire-up findings; `code` owns the dependency-graph fix; `proto` confirms hyper-only coverage gaps. |
| T2 | **SIGTERM is not a drain**: README claims 30 s graceful drain with `SO_REUSEPORT` + FD passing; reality is `JoinHandle::abort()` + 500 ms sleep. TCP/H1/H1s/metrics-sampler spawn fleet has no `CancellationToken`. No H2 `GOAWAY` emission. | rel (H7), code (Q-CODE-1-04/05), proto (consulted via rel) | `rel` owns SIGTERM finding + drain ordering; `code` owns CancellationToken plumbing; `proto` owns GOAWAY emission. |
| T3 | **TLS cert rotation is fictional**: docs claim `ArcSwap<TlsStore>` hot-swap; build site `main.rs:214,611` is restart-only. | rel (H2/F-06), sec (§5.1) | `rel` + `sec` joint. |
| T4 | **CONNTRACK is `HashMap` not `LRU_HASH`**: adversarial flood starves new flows. | sec (S-2), ebpf (§2 maps), rel | `ebpf` owns the map-type fix; `sec` owns the attack-evidence; `rel` owns the saturation metric + alert. |
| T5 | **XDP loader gaps**: no `.license` ELF section + `aya::EbpfLoader::set_license` never called (real-NIC attach risk depending on aya default); no `.BTF`; SKB-mode hard-coded with no Drv probe/fallback; no map pinning (cold CONNTRACK on restart); dropped `XdpLinkId` at `loader.rs:275` (needs aya-source confirm); 4 `unsafe impl aya::Pod` with non-zeroed padding. | ebpf, code (4 Pod impls), sec (Pod padding S-9) | `ebpf` owns ELF/license/pinning/SKB; `code` owns Pod constructor zero-init; `sec` confirms S-9 close. |
| T6 | **Every atomic is `Ordering::Relaxed`** (50 sites, 0 Acquire/Release/SeqCst). Acceptable for stats; dangerous when a counter gates enforcement (rapid-reset, per-IP rate limit, replay guard). | code (Q-CODE-1-02), sec | `code` owns the audit + reclassification per-site; `sec` signs off per detector. |
| T7 | **Operability**: `/healthz` unconditional 200 (no readiness/startup), logs are not JSON despite docs, no `traceparent` propagation anywhere, no per-listener/per-backend RED labels, unbounded `tokio::spawn` per accept, EMFILE tight-loop, `spawn_blocking` for upstream connect on global pool. | rel | `rel` owns observability findings; `code` co-owns spawn/EMFILE/spawn_blocking. |
| T8 | **Protocol correctness gaps**: H2 silently prefers `:authority` over `Host` instead of rejecting mismatch (RFC 9113 §8.3.1); `LB_QUIC_ALPN = b"lb-quic"` instead of `b"h3"`; no 1xx/100-Continue policy or test; `ws_autobahn.rs` is a `--help` stub; no `h3spec` harness; `tests/conformance_h{1,2,3}.rs` are codec round-trip unit tests, NOT server-conformance; H2/H3 same-pseudo-header `Bridge` impls don't strip hop-by-hop at trait level (runtime helper covers it). | proto, sec | `proto` owns all RFC-conformance findings; `code` owns the trait-level vs precondition fix decision. |
| T9 | **Test depth**: zero proptest / loom / miri usage in any crate; fuzz corpora 5–7 inputs per target; `cargo-audit`/`cargo-deny`/`cargo-geiger` absent from sandbox — need CI verification. `cargo-machete` flags 14 suspect-unused deps including `lb-controlplane` and `lb-health` in the binary → control-plane wiring confirmed missing. | code, sec | `code` owns proptest/loom/miri additions; `sec` owns supply-chain CI; lead notes control-plane wiring is a Round 3 question. |

Workspace baseline (corroborated by code): `cargo fmt --check` and
`cargo clippy --all-targets --all-features -- -D warnings` pass cleanly
on pinned MSRV 1.85. This is a real baseline, not aspirational.

## B. Genuine disagreements requiring resolution

1. **0-RTT on TCP listener** — `rel` F-19 claims a `ZeroRttReplayGuard`
   is missing on the TCP/TLS listener; `sec` argues 0-RTT is gated by
   `max_early_data_size` and is not enabled in `build_server_config`.
   **Resolution**: in Round 2, `sec` reads `lb-security::ticket::build_server_config`
   and confirms `max_early_data_size = 0`. If true → finding withdrawn.
   If false → S-5 reopens at SEV-1.

2. **Drain budget** — `rel` expects 10 s default configurable via
   `[runtime].drain_timeout_ms`; `code` may argue for the existing
   2.5 s fixture. **Lead decision (pre-emptive)**: 10 s default,
   configurable, must be a hard cap on the abort fallback. `rel` owns
   the finding.

3. **Bridge hop-by-hop stripping placement** — `proto` #7 asks `code`:
   fix at the `Bridge` trait level vs. document the precondition that
   the runtime helper is always called. **Resolution**: defer to
   Round-3 plan; both teammates surface options as a single finding
   in Round 2 (proto authors with code as co-reviewer).

4. **`spawn_blocking` for upstream connect** — `rel` flags as sharp
   edge (global blocking-pool starvation); `code` may argue Pingora
   parity. **Resolution**: Round 2 — `rel` authors the finding, `code`
   evaluates the alternative (bounded executor or non-blocking
   `tokio::net::TcpStream::connect`) for the Round 3 plan.

## C. Scope-only items confirmed shared / no dispute

- T4 CONNTRACK: tri-owner sec + ebpf + rel.
- T1 detector wiring: sec + code (dep graph) + proto (hyper-only-coverage matrix).
- T2 SIGTERM: rel + code + proto.
- T5 XDP loader: ebpf primary, code + sec consulted.
- T8 Bridge trait hop-by-hop: proto + code.

## D. Sandbox limitations recorded for CI-side verification

- `cargo-audit`, `cargo-deny`, `cargo-geiger` not present in sandbox.
- `h2spec`, `h3spec`, `wstest`, `bpftool`, `cargo-fuzz` not present.
- No host C toolchain by default; teammates installed `gcc/cmake/libclang-dev/libc6-dev`.
- No 1.85 toolchain → `rust-toolchain.toml` provided one; CI image
  should bake all of the above.

All of these become Round 7 gate items, not Round 2 findings — but
each finding that depends on one of them must specify the
verification target (CI image vs. local sandbox).

## E. Round-2 entry checklist

- STATE flips to `ROUND_FINDINGS`.
- Each teammate writes `audit/<area>/round-2-findings.md` using the
  finding template in their role definition.
- After all five land, every teammate must read all four others' and
  produce `audit/<area>/round-2-cross-review.md` with **AGREED /
  DISPUTED / ESCALATED-SEVERITY** verdicts per finding.
- Sec finalizes S-1 and S-9 severities now that code + proto are in.
- Lead-level disagreements above resolved in Round 2 cross-review.
