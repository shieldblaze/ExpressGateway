# Round 2 — `code` Cross-Review

Reviewer: `code`.
Inputs reviewed:
- `audit/reliability/round-2-review.md` (REL-2-01 .. REL-2-15)
- `audit/ebpf/round-2-review.md` (EBPF-2-01 .. EBPF-2-09)
- `audit/protocol/round-2-review.md` (PROTO-2-01 .. PROTO-2-15)
- `audit/security/round-2-review.md` — **not present at close of `code`
  Round 2.** Sec is the last teammate to publish; verdicts on
  sec-owned findings are deferred to Round 6 reconciliation.

Verdicts:
- **AGREED** — finding is correct as stated; code endorses location,
  severity, and recommendation.
- **DISPUTED** — code disagrees on a substantive point (location,
  severity direction, fix shape). Explicit counter-position given.
- **ESCALATE-SEVERITY** — code believes the severity should rise.
- **DEFER-SEVERITY** — code believes the severity should drop.

Findings purely inside a teammate's lane with no code-side bearing
are listed as "noted (out of lane)".

---

## A. `code` vs `rel`

### REL-2-01 — Doc/code drift umbrella
Verdict: **AGREED**. The drift set is real and matches what `code`
found in Round 1 §6 (Q-CODE-1-04/05). `code` co-owns the *wiring*
piece via CODE-2-03 (CancellationToken plumbing), CODE-2-13 (machete
unused deps for `lb-controlplane` + `lb-health`), and CODE-2-12
(`arc-swap` dead workspace dep). No dispute.

### REL-2-02 — SIGTERM is not a drain (critical)
Verdict: **AGREED**. This is the rel-side companion to CODE-2-03.
Severity `critical` matches. Lead's pre-resolved decision on the
10 s drain budget is correctly captured in REL-2-02 recommendation
step 1. `code`'s slice is the `CancellationToken`/`TaskTracker`
plumbing across the 33-spawn fleet from Round 1 §2.2; details in
CODE-2-03 Recommendation §1–§3.

One additive note: REL-2-02 §3 proposes a `JoinSet` per listener;
`code` recommends `tokio_util::task::TaskTracker` instead because it
lets a `wait()` call run *after* all `JoinHandle`s are dropped without
having to keep the set alive. Functional equivalent; preference logged
for Round 3 plan.

### REL-2-03 — TLS cert rotation fictional (critical)
Verdict: **AGREED**. The `arc-swap = "1"` workspace declaration with
zero call sites is exactly what `code` flagged in Round 1 §3.4 and
CODE-2-12. `sec` owns the validation correctness (key match, expiry,
chain depth). No dispute.

### REL-2-04 — `/healthz` unconditional 200 (high)
Verdict: **AGREED**. Out of `code`'s primary lane (lb-observability
admin endpoint), but the implementation requires a per-listener
`Arc<AtomicBool>` `accepting` flag set/cleared by `code`'s
CancellationToken plumbing in CODE-2-03. Joint owner via that path.

### REL-2-05 — `HealthChecker` and `ConfigManager` dead from binary (high)
Verdict: **AGREED + co-owned**. `code`'s CODE-2-13 records the same
finding from the `cargo machete` angle. REL-2-05 carries the right
recommendation set (wire `HealthChecker` into the picker, wire SIGHUP
to `ConfigManager`). The `lb-cp-client` keep-or-delete decision is
correctly deferred to Round 3.

### REL-2-06 — Logs are plain text despite docs claiming JSON (medium)
Verdict: **AGREED**. No code-side dispute. Pure config change in the
binary's `tracing_subscriber::fmt()` init.

### REL-2-07 — No distributed tracing (high)
Verdict: **AGREED**. Adds a new workspace dep (`opentelemetry`,
`tracing-opentelemetry`); `code` flags that the OTel API surface
changes MSRV considerations — pin a version that holds MSRV 1.85.
Otherwise no dispute.

### REL-2-08 — Per-listener / per-backend RED labels missing (medium)
Verdict: **AGREED**. The per-request hook placement note (rel cites
`code` as joint owner) is well-aimed: per-request counter increment
needs to live inside `lb-l7` next to the response builder, not at
`crates/lb/src/main.rs:1186`. `code` owns that refactor.

### REL-2-09 — Unbounded `tokio::spawn` per accept (critical)
Verdict: **AGREED + escalation already aligned**. This is the metric/
alert/runbook side of `code`'s CODE-2-05 (`Semaphore` implementation).
Severity `critical` matches CODE-2-05. The owner split (rel = metric
+ runbook, code = semaphore) is the right one. No dispute.

### REL-2-10 — `accept(2)` tight-loop on EMFILE (critical)
Verdict: **AGREED**. Companion to CODE-2-06. Severity `critical`
matches. The recommendation alignment is exact: rel owns the
`accept_errors_total{kind}` metric + alert; code owns the
classification + backoff. No dispute.

### REL-2-11 — `spawn_blocking` for upstream connect (high)
Verdict: **AGREED**. `code` owes the analysis-of-alternatives in
CODE-2-09; this finding is the rel-side problem statement. The
75-second kernel-default SYN-blackhole timeout that REL-2-11 cites is
the strongest argument for switching to `tokio::net::TcpStream::connect`
+ explicit `tokio::time::timeout(...)`. CODE-2-09 already endorses
that as the preferred path. Joint owner; no dispute.

### REL-2-12 — CONNTRACK saturation unobserved (high)
Verdict: **AGREED** (out of `code` lane). The bpf-side fix is `ebpf`'s
EBPF-2-03 + EBPF-2-08; the metric is rel's. No code-side action.

### REL-2-13 — STATS map never exported (medium)
Verdict: **AGREED** (out of lane). Companion to EBPF-2-08.

### REL-2-14 — Binary name mismatch (low)
Verdict: **AGREED**. Documentation-only. Noted.

### REL-2-15 — No panic hook; `panic = "unwind"` (high)
Verdict: **ESCALATE-SEVERITY → critical**. `code` opens this as
CODE-2-02 at `critical`. The argument for escalation:

REL-2-15's framing ("silent task death, invisible bug") is correct
but under-states the *safety* risk. The 17 `unsafe` blocks in
`crates/lb-io/src/ring.rs` and the `unsafe impl aya::Pod` × 4 in
`crates/lb-l4-xdp/src/loader.rs` are not just unwind-unsafe in the
"observability gap" sense — they are unwind-unsafe in the
"continued-execution-may-be-UB" sense:
- A panic mid-`libc::close` (`ring.rs:345`) that gets caught by tokio
  leaves a process state that the next `unsafe` block can no longer
  assume — fd reuse races become real.
- A panic mid-BPF-map-`insert` leaves the kernel map in a state that
  the next userspace iteration cannot assume.

The Round-1 cross-review with `sec` (§B.2 F-22) called this out as a
joint reliability + security concern; `code` agrees with `sec`'s
framing. With `panic = "abort"` the process *dies* — systemd restarts
it via `Restart=on-failure`. That is the safer default for a
network-facing proxy with multiple unsafe boundaries.

Rel and code's recommendation lists are consistent on the hook + the
metric; the only divergence is `rel` lists the unwind-vs-abort choice
as a Round-3 decision and `code` recommends abort as the default. Lead
to arbitrate in Round 6.

---

## B. `code` vs `ebpf`

### EBPF-2-01 — BPF ELF has no `license` section / no BTF (high)
Verdict: **AGREED**. Out of `code`'s primary lane but the
`#[unsafe(link_section = "license")]` patch lives in the eBPF crate
which uses the same `unsafe_attributes` MSRV machinery as the rest of
the workspace. `code` notes for `ebpf`: the workspace builds on
1.85 — verify `unsafe(link_section = ...)` syntax is accepted at MSRV
or fall back to `#[link_section = "license"] #[no_mangle]`. (Both
work at 1.85; the `unsafe_attributes` lint group is informational
only.)

### EBPF-2-02 — `EbpfLoader::set_license` does not exist (medium)
Verdict: **AGREED + grateful**. `ebpf` corrected the team-lead's
brief at the source level by reading aya 0.13.1's `EbpfLoader` API.
`code` independently confirmed during CODE-2-10 evaluation that
`set_license` is not in aya 0.13.1's public surface. The fix path is
the ELF section (EBPF-2-01), not a loader call. No dispute.

### EBPF-2-03 — CONNTRACK is `HASH` not `LRU_HASH` (high)
Verdict: **AGREED** (out of code lane). Tri-owner per synthesis T4.

### EBPF-2-04 — SKB-mode hard-coded; no Drv probe / fallback (high)
Verdict: **AGREED** (out of code lane). The `[runtime].xdp_mode` knob
proposed is a `lb-config` change which `code` reviews at change time.
No dispute.

### EBPF-2-05 — No map pinning; cold CONNTRACK on every restart (high)
Verdict: **AGREED** (out of code lane). Coordinates with REL-2-02
(drain) since pinning is what allows zero-drop restart claims to be
truthful.

### EBPF-2-06 — Dropped `XdpLinkId` confirmed safe; needs regression test (low)
Verdict: **AGREED**. This is the matching finding to `code`'s
CODE-2-10. Same aya-source review. Same severity. Same recommendation
(integration test gated on `CAP_BPF`). `code`'s CODE-2-10 phrases it
as `info`; `ebpf` phrases it as `low`. Functionally indistinguishable;
the gap from `info` to `low` is whether absence of the test is
*blocking* the regression-coverage promise. **`code` defers to `ebpf`'s
`low` rating** — the test absence does matter for forward-compat.

### EBPF-2-07 — No verifier-log matrix (medium)
Verdict: **AGREED** (out of code lane; CI work).

### EBPF-2-08 — STATS PerCpuArray never exported (medium)
Verdict: **AGREED** (out of code lane).

### EBPF-2-09 — Pod padding parity (medium)
Verdict: **AGREED + identical fix**. `ebpf`'s EBPF-2-09 and `code`'s
CODE-2-07 are the same finding from the two sides of the boundary:

| Aspect | EBPF-2-09 (ebpf) | CODE-2-07 (code) |
|---|---|---|
| BPF-side struct layout match | confirmed byte-for-byte by ebpf | takes ebpf's word |
| Userspace constructor zero-init | identifies as fix | identifies as fix |
| Compile-time size assertion | recommends test | recommends `const _: () = assert!(...)` |
| Severity | medium | high |

**`code` ESCALATES-SEVERITY to high** for the userspace constructor
piece for the same reason `sec` S-9 marks it high: a single
hash-collision miss between userspace and kernel turns the entire XDP
fast path into a "first-packet-only" fast path with no signal —
exactly the failure mode T4 attacks expand on. The eBPF side
correctness is confirmed; the userspace side is one struct-literal
contributor away from silent breakage.

Action: `code` keeps CODE-2-07 at `high`; `ebpf` may keep EBPF-2-09 at
`medium` because the eBPF *side* of the fix is well-bounded. The
disagreement is about *which side* carries the severity; resolution
is "high on the userspace side" (CODE-2-07) and "medium on the eBPF
confirmation side" (EBPF-2-09). Joint owner via `sec` S-9.

---

## C. `code` vs `proto`

### PROTO-2-01 — H2 `:authority` vs `Host` mismatch (high)
Verdict: **AGREED**. `code` confirms the location and the precedence
behaviour. The `validate_authority` shared-helper question proto
raises is the same as the Bridge-trait hygiene question PROTO-2-07
asks: lead pre-approved deferring the placement question to Round 3.
Belt-and-braces wins for *both* fixes (one helper in
`lb-l7/src/util/`).

### PROTO-2-02 — `LB_QUIC_ALPN = b"lb-quic"` (critical)
Verdict: **AGREED + grateful**. This is a striking find. `code`
re-checked source at the cited line and confirms `build_config` is
the only `set_application_protos` site and that no override path
substitutes `b"h3"` for production. Severity `critical` is justified;
the H3 path is non-functional against any real H3 client. No dispute.

### PROTO-2-03 — No 1xx / 100-Continue / 103 policy (medium)
Verdict: **AGREED** (out of code primary lane). Note: the
`BridgeResponse` representation change proto recommends (carry
`informationals: Vec<...>`) is a `code`-co-owned refactor of
`crates/lb-l7/src/lib.rs:48+`. Round 3 plan should bundle it with
PROTO-2-12 (trailers).

### PROTO-2-04 — `ws_autobahn.rs` is a `--help` stub (medium)
Verdict: **AGREED** (out of code lane; CI work).

### PROTO-2-05 — No `h3spec` / `h3i` harness (medium)
Verdict: **AGREED** (out of code lane).

### PROTO-2-06 — `conformance_h{1,2,3}.rs` are codec round-trip unit tests (low)
Verdict: **AGREED**. The rename option (a) should land in Round 3
plan as a quick win.

### PROTO-2-07 — `Bridge` trait does not strip hop-by-hop (low)
Verdict: **AGREED** (joint owner). `code` is the named co-reviewer.
Position for Round 3 plan:

- Option (a) **trait-level fix**. Cost: ~one `HeaderMap::retain` per
  request on bridge entry (already happens at runtime via the helper;
  doubling it is a measurable perf cost ~ tens of ns per request).
- Option (b) **documented precondition + `pub(crate)`**. Cost: zero
  perf; relies on caller discipline. Fragile when a future filter
  chain wants to call the bridge from anywhere new.

`code` recommends **(b) plus a compile-time guarantee**: wrap the
runtime helper in a newtype `StrippedRequest` that the bridge trait
consumes by type. The runtime path constructs `StrippedRequest` via
the helper; bridges that don't go through the helper can't compile.
Zero perf cost, type-system-enforced precondition. Lead arbitrates
in Round 6.

### PROTO-2-08 — `HOP_BY_HOP` lists spurious `trailers` (low)
Verdict: **AGREED**. Trivial cleanup; lands as part of CODE / proto
Round 3 plan.

### PROTO-2-09 — `ListenerMode` silent fallthrough to `PlainTcp` (medium)
Verdict: **AGREED + ESCALATE-SEVERITY → high**. Operator-facing silent
mis-binding of a listener that *was supposed to* terminate TLS instead
binding as plain TCP forwarding raw bytes to the upstream is a
security-equivalent failure for any deployment that uses TLS
termination at the gateway. The blast radius is the upstream
receiving raw internet bytes that ops thought were TLS-terminated.

`code` opens this as a follow-up under CODE-2 numbering only if proto
prefers severity `medium`; otherwise this stays under PROTO-2-09 with
the escalation. **Recommend high.** Lead arbitrates.

### PROTO-2-10 — SmuggleDetector unwired; hyper coverage gaps (high)
Verdict: **AGREED**. The dep-graph fix is `code`'s CODE-2-01; the
RFC-level coverage-matrix is proto's PROTO-2-10. The matrix proto
produced (10-row hyper-vs-detector table) is exactly what synthesis
T1 promised. No dispute.

### PROTO-2-11 — No GOAWAY / CONNECTION_CLOSE on SIGTERM (high)
Verdict: **AGREED**. Companion to CODE-2-03 (CancellationToken
plumbing). The fix shape proto recommends — hyper's
`graceful_shutdown()` call wired through a per-connection token — is
the same fix CODE-2-03 enables. No dispute.

### PROTO-2-12 — Trailer pass-through untested and broken (medium)
Verdict: **AGREED**. Joint owner with `code` via the `BridgeResponse`
extension (see PROTO-2-03 note above).

### PROTO-2-13 — `SETTINGS_ENABLE_CONNECT_PROTOCOL` not tested (low)
Verdict: **AGREED**. Test-only addition; no code change.

### PROTO-2-14 — TLS 1.2 enabled; no `tls13_only` knob (medium)
Verdict: **AGREED** (out of code primary lane). The config-knob plumbing
is a `lb-config` change `code` reviews at change time.

### PROTO-2-15 — SNI ↔ Host disagreement not enforced (medium)
Verdict: **AGREED** (out of code primary lane). Sibling of PROTO-2-01.

---

## D. `code` vs `sec`

`sec` Round 2 findings file is **not present** at the time `code`
closes Round 2 (last check: `audit/security/round-2-review.md` does
not exist; only `round-1-*` is present). The per-detector atomic-
ordering requirements list that `sec` agreed to hand over for
CODE-2-04 Appendix A is therefore unfulfilled at this round. `code`
proceeded with a source-driven classification.

Items that will need `sec` ↔ `code` reconciliation in Round 6:
1. **CODE-2-04 Appendix A** — `sec` to confirm or override the
   per-detector Acquire/Release requirement for:
   - `lb-h2::security::RapidResetDetector`
   - `lb-h2::security::ContinuationFloodDetector`
   - `lb-h2::security::HpackBombDetector`
   - `lb-security::ZeroRttReplayGuard`
   - Forthcoming per-IP rate-limit bucket (from CODE-2-01).
2. **CODE-2-02 vs sec F-22 / panic_hook framing** — `code` escalates
   to `critical`; expecting `sec` will agree given the
   panic-mid-unsafe-block argument from `sec`'s Round 1 cross-review
   §B.2.
3. **CODE-2-07 Pod padding** — joint with sec S-9 and ebpf EBPF-2-09;
   `code` keeps `high` consistent with sec's S-9 framing.
4. **CODE-2-08 per-CID actor leak** — sec asked `code` (Q-1-09) to
   confirm acceptability; `code` says acceptable *iff* CODE-2-02
   lands (panic = abort).

---

## E. Severity distribution & cross-cutting blockers

### `code` Round 2 findings: 15 IDs
| Severity | IDs |
|---|---|
| critical | CODE-2-01, CODE-2-02, CODE-2-03, CODE-2-05, CODE-2-06 |
| high | CODE-2-04, CODE-2-07, CODE-2-08, CODE-2-09, CODE-2-11 |
| medium | CODE-2-13, CODE-2-14 |
| low | CODE-2-12, CODE-2-15 |
| info | CODE-2-10 |

### Blocking-for-prod (yes): 7 findings
CODE-2-01, CODE-2-02, CODE-2-03, CODE-2-04, CODE-2-05, CODE-2-06, CODE-2-07.

### Cross-team blocker convergence
Every `critical` and `high` `code` finding has at least one teammate
echo:
- CODE-2-01 ↔ PROTO-2-10 ↔ sec S-1/S-3/S-4 ↔ REL-2-?? (synthesis T1)
- CODE-2-02 ↔ REL-2-15 ↔ sec §B.2/F-22 (escalation)
- CODE-2-03 ↔ REL-2-02 ↔ PROTO-2-11 (synthesis T2)
- CODE-2-04 ↔ sec (per-detector list owed)
- CODE-2-05 ↔ REL-2-09 (synthesis T7)
- CODE-2-06 ↔ REL-2-10 (synthesis T7)
- CODE-2-07 ↔ EBPF-2-09 ↔ sec S-9 (synthesis T5b)
- CODE-2-09 ↔ REL-2-11 (synthesis disagreement #4)
- CODE-2-11 ↔ none (`code`-only proptest/loom/miri agenda)
- CODE-2-13 ↔ REL-2-05 (machete unused deps)

### Genuine disputes for Round 6 lead arbitration
1. **REL-2-15 / CODE-2-02 severity** — `code` says `critical`, `rel`
   says `high`. Argument: unwind across unsafe is a safety concern
   not just observability.
2. **PROTO-2-09 severity** — `code` argues for escalation
   `medium → high` because silent fall-through from `protocol = "https"`
   to plain-TCP is a category of mis-config that exposes upstream to
   the internet without TLS.
3. **PROTO-2-07 fix shape** — Bridge hop-by-hop hygiene. `code`
   proposes a newtype `StrippedRequest` to enforce the precondition
   at compile time, instead of either of proto's two options.

Round 3 (planning) is unblocked: every `critical` and `high` finding
has a concrete recommendation set. **No Round-3 blocker remaining
from the `code` side.**

— `code`, Round 2.
