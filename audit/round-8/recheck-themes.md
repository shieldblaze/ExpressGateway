# Round 8 Phase C-bis — Theme-bounded recheck (verify)

STATE=ROUND_8_RECONCILE. Task #56. Phase C reconciliation surfaced four
systematic blind-spot themes; the user-lead chose a HYBRID approach: do
not redo the full 74-item register, but DO recheck every prior
`Verified-Fixed` finding whose closure fits one of the four themes.

Verdicts use the Phase C taxonomy:

- **STILL-VERIFIED** — the cited commit closes the recommendation; the
  theme-specific concern does not apply.
- **PARTIAL**       — closure partial; gap either already disclosed in
  the status note or NEWLY surfaced here.
- **FALSE-VERIFIED** — the recommendation is not closed; the register
  overstates reality.

Scope. The 4 themes filter the prior register to ~25 candidate findings.
The Phase-C recheck (17 spot-checks) already covered some of them; this
phase-C-bis pass walks the residual surface. Time-boxed to ~45 min;
breadth over depth.

---

## Theme 1 — "Verified-Fixed" grades script / artefact existence, not capability

Definition: the closure shipped a script, a library API, or a CI gate
whose presence on disk is mistaken for the capability it was meant to
deliver. Recheck criterion: the gate must actually fire under the
condition the recommendation specifies; the library must have at least
one production call site; the metric must show up in `/metrics`.

| Prior ID | Closure artefact | Phase-C tag | C-bis verdict |
|---|---|---|---|
| EBPF-2-07 | `scripts/verify-xdp.sh` + README | ROUND8-L4-10 DISPUTE | **FALSE-VERIFIED** (no `.committed` baselines; diff-gate never fires; conditional at `scripts/verify-xdp.sh:103` is dormant) |
| REL-2-02  | drain ordering + counter        | ROUND8-OPS-03 MISSED  | **PARTIAL** (counter shipped; `shutdown_drain_seconds` histogram not in source; `grep` in `crates/` returns zero hits) |
| REL-2-07  | `lb_observability::tracing_propagation` | ROUND8-OPS-06 MISSED | **PARTIAL** (already disclosed Partial; zero L7/QUIC callers as of 2026-05-14; re-confirmed `grep extract_parent crates/lb-l7 crates/lb-quic` empty) |
| REL-2-08  | `LabelBudget::check`            | ROUND8-OPS-05 MISSED  | **PARTIAL** (already disclosed Partial; check is startup-only worst-case math; no per-emission gate at registration) |
| REL-2-09  | per-listener semaphore + `accept_inflight` gauge | n/a (not in coverage-gap) | **STILL-VERIFIED** (gauge was deferred at Round-2 review time but later wired in `6d7f667`; `accept_inflight_inc/dec` live at `crates/lb/src/main.rs:393, 400`. Status note is now stale-narrow but capability landed.) |
| REL-2-11  | `TcpPool::acquire_async`        | not in coverage-gap   | **PARTIAL** (already disclosed Partial; `backend_connect_seconds` histogram + `backend_connect_errors_total` not in source; `grep` in `crates/` returns zero hits; `git log` shows no follow-up commit) |
| REL-2-13  | 1 Hz STATS sampler              | not flagged           | **STILL-VERIFIED** (sampler at `main.rs:1629`; `xdp_packets_total{action}` deltas applied; biased select! drains cleanly under shutdown) |
| REL-2-04  | ProbeRegistry + `/livez+/readyz+/startupz` | n/a | **STILL-VERIFIED** (`readiness_settle_ms=1000` default is a tuning gap captured separately in ROUND8-OPS-11; not a Theme-1 capability gap) |
| CODE-2-04 | `scripts/ci/atomic-lint.sh`     | not flagged           | **STILL-VERIFIED** (lint actively rejects un-annotated `Ordering::Relaxed` in scoped files; pre-condition + grep flow at `scripts/ci/atomic-lint.sh:69-86` is genuinely live; not a no-op) |
| CODE-2-03 | `Shutdown` primitive + tracker  | ROUND8-OPS-04 MISSED  | also Theme-2 (see below); listener accept loop is the gap. |

**Per-finding evidence**:

- **EBPF-2-07 (FALSE-VERIFIED)**. Commit `ffde98c` added
  `scripts/verify-xdp.sh` (+109) and `audit/ebpf/verifier-logs/README.md`
  (+30). `find audit/ebpf/verifier-logs/ -type f` returns the README
  only — no `.log.committed`. The diff-gate at `verify-xdp.sh:103`
  reads `if [ -f "${OUT_LOG}.committed" ]; then ...` — the condition
  is permanently false, so the only effect of the script is to capture
  a log and exit 0 unconditionally. `audit/unsafe-justifications.md:99`
  + `:109` assert the gate is "live in CI" — that claim is false.
  Recommendation requires: capture verifier output across {5.15, 6.1,
  6.6} AND commit a baseline against which subsequent runs diff. The
  baseline did not ship.

- **REL-2-02 histogram (PARTIAL, undisclosed)**. Commits 1f7ab4b +
  fc050b0 + 82551dc + 33edd13 wire drain ordering correctly. The
  Round-2 recommendation text required `shutdown_drain_seconds`
  histogram AND `shutdown_aborted_connections_total` counter
  (round-2-review.md:134). `grep -rn 'shutdown_drain_seconds' crates/`
  returns ZERO hits. The counter is wired at `crates/lb/src/main.rs:1974`.
  Phase C already flagged this; C-bis confirms by direct grep.

- **REL-2-07 (PARTIAL, disclosed)**. Commit 1d462c7 ships
  `lb_observability::tracing_propagation`. `grep -rln
  'extract_parent\|inject_parent\|traceparent' crates/lb-l7
  crates/lb-quic` returns empty. The Round-2 status note already
  discloses "Library-only fix; proxy wire-in is deferred." Three
  rounds later the wire-in has not landed. Severity stance per Phase C
  reconcile: this is still a divergence.

- **REL-2-08 (PARTIAL, disclosed)**. `LabelBudget::check`
  (`crates/lb-observability/src/label_budget.rs:159`) computes
  worst-case label cardinality from config shape at startup. There is
  no per-emission cardinality gate inside the metrics emit path. The
  status note already acknowledges Partial (only `version` +
  `status_class` labels at the actual emit site). The
  cardinality-check side is also Partial: it cannot prevent runtime
  cardinality explosion if a future commit lands a high-cardinality
  label.

- **REL-2-11 (PARTIAL, disclosed)**. The structural fix (non-blocking
  `acquire_async`) is real and complete. The operability slice
  (`backend_connect_seconds{listener,backend}` histogram +
  `backend_connect_errors_total{listener,backend,kind}` counter) is
  absent: `grep -rn 'backend_connect_seconds\|backend_connect_errors_total'
  crates/` empty; `git log --all --oneline | grep -i backend_connect`
  empty. Already disclosed.

- **REL-2-09 (re-graded STILL-VERIFIED)**. The Round-2 status note
  says "accept_inflight{listener} gauge is NOT emitted." `git log`
  shows commits `83531fb` and `6d7f667` ("REL-2-09 follow-on — emit
  accept_inflight{listener} gauge at accept-site"). Source confirms:
  `crates/lb-observability/src/lib.rs:363,375,386`
  (`accept_inflight_gauge`, `_inc`, `_dec`) + call sites at
  `crates/lb/src/main.rs:393, 400, 1351, 2308`. The recommendation is
  closed in source; the Round-2 status note is stale-narrow.
  **Action**: should be re-graded to Verified-Fixed in Phase D
  bookkeeping.

**Theme 1 result**. Six findings examined; one FALSE-VERIFIED
(EBPF-2-07, already known), three disclosed-PARTIAL (REL-2-07,
REL-2-08, REL-2-11), one undisclosed-PARTIAL (REL-2-02 histogram —
already known from Phase C), one stale-narrow-status (REL-2-09 —
recommendation actually closed, needs status upgrade). No NEW
FALSE-VERIFIED beyond EBPF-2-07.

---

## Theme 2 — Operational vs laboratory test posture

Definition: the proof test passes in single-instance / single-process
unit-test setup but does not represent production scale or multi-replica
operational conditions. Recheck criterion: where is the load /
multi-replica / sustained-rate test? If none, can we predict that the
prod posture would still fail?

| Prior ID | Production scenario | C-bis verdict |
|---|---|---|
| CODE-2-03 | listener accept loop under SIGTERM with steady accept rate | **PARTIAL** (accept loop body has no `select!` cancel arm; drain at `main.rs:1942-1944` still uses `JoinHandle::abort()` on listener handles) |
| REL-2-02  | drain budget on long-lived streaming workloads (WS, SSE, gRPC server-stream) | **PARTIAL-OPERATIONAL** (drain budget hard-coded to 10s; no per-listener override; no streaming-specific test posture; ROUND8-OPS-10 already flagged) |
| REL-2-04  | `/readyz` flip-to-503 visibility inside kubelet 10s probe period | **STILL-VERIFIED-WITH-CAVEAT** (within recommendation scope; `readiness_settle_ms=1000` default is a Theme-2 mismatch — ROUND8-OPS-11) |
| SEC-2-02 / EBPF-2-03 | conntrack eviction under SYN flood at >100k flows/sec | **PARTIAL-OPERATIONAL** (LRU declaration confirmed; userspace simulator green; SYN-flood / TCP-state-aware-pruning posture absent; ROUND8-L4-02 + ROUND8-L4-03 already flagged) |
| CODE-2-05 / SEC-2-04 | per-IP cap under sustained spray + GC race | **STILL-VERIFIED** (10-test conn_gate proof at `lb-security/src/conn_gate.rs` covers GC race, permit Drop, AcqRel/Acquire pairing; bypass-attempt review documents the surface; lab posture acceptable for the contract) |
| CODE-2-06 / REL-2-10 | accept(2) backoff under EMFILE storm | **STILL-VERIFIED** (test exercises 20-iter doubling sequence; saturating_mul + max(1) floor are static-guarantees not posture-dependent) |
| REL-2-12  | conntrack-full metric under saturation | **STILL-VERIFIED** (metric is monotonic counter; behaviour scale-independent) |
| EBPF-2-04 | post-attach packet probe vs. silent-drop firmware | **PARTIAL-OPERATIONAL** (Drv→Skb fallback triggers on attach-syscall error only; ROUND8-L4-05 already flagged) |

**Per-finding evidence**:

- **CODE-2-03 (PARTIAL, already known)**. `crates/lb/src/main.rs:2179`
  loop body: `let (mut client_stream, client_addr) = match
  listener.accept().await { ... }` — no `tokio::select!` cancel arm.
  The drain at `main.rs:1942-1944` uses `JoinHandle::abort()` on every
  `listener_handles[i]` — exactly the primitive the recommendation
  promised to retire. The per-connection task does use the biased
  select!, but the accept-loop body itself does not. The
  `per_connection_drain.rs` test exercises the per-conn path; no test
  exercises a draining listener accepting new connections.

- **REL-2-02 streaming posture (PARTIAL-OPERATIONAL, already-flagged)**.
  `let drain_budget = Duration::from_millis(runtime_cfg.map_or(10_000,
  |r| r.drain_timeout_ms));` at `crates/lb/src/main.rs:1961`. WS / SSE
  / gRPC server-streaming long-poll workloads will exceed this. No
  per-listener override. ROUND8-OPS-10 already captures vs. Pingora
  `EXIT_TIMEOUT = 300s`.

- **SEC-2-02 / EBPF-2-03 SYN flood (PARTIAL-OPERATIONAL,
  already-flagged)**. The LRU migration solves ENOMEM-on-fill (Round-2
  contract). The TCP-state-aware-pruning concern (Cilium
  `conntrack.h`) and SYN-flood / new-flow-rate cap (Katran's
  `is_under_flood()`) are scope-extension findings ROUND8-L4-02 /
  ROUND8-L4-03 — already in Phase D plan.

- **EBPF-2-04 silent-drop (PARTIAL-OPERATIONAL, already-flagged)**.
  Fallback ladder triggers on `EOPNOTSUPP`/`EINVAL` from attach
  syscall. The aya #1193 / MLX5+CX6 firmware-success-but-no-packets
  pattern is not detectable without a post-attach probe. ROUND8-L4-05
  captures the gap.

**Theme 2 result**. Eight findings examined. Five PARTIAL or
PARTIAL-OPERATIONAL (already known from Phase C); three STILL-VERIFIED.
No NEW PARTIAL or FALSE-VERIFIED beyond the four already in the Phase C
register (CODE-2-03 listener loop, REL-2-02 streaming budget, ROUND8-L4
SYN flood family, EBPF-2-04 silent drop).

---

## Theme 3 — Doc-vs-code claim drift

Definition: the closure updated docs without matching implementation
(or vice versa). Recheck criterion: walk every Round-2 finding whose
recommendation touched operator-facing prose, in-source doc comments,
or capability claims; check if the implementation actually matches the
prose.

| Prior ID | Prose location | C-bis verdict |
|---|---|---|
| REL-2-01  | README + DEPLOYMENT + RUNBOOK rewrites | **PARTIAL** (binary-name class fixed; `README.md:23` still claims "Zero-downtime reload via SO_REUSEPORT and FD passing" but `CONFIG.md:136` says "FD-passing is deferred to Pillar 3b follow-up". ROUND8-OPS-01 already flags.) |
| REL-2-14  | binary-name patrol via `doc-lint.sh` | **STILL-VERIFIED-NARROW** (`grep -n '/usr/local/bin/lb' DEPLOYMENT.md RUNBOOK.md` returns zero; the doc-lint patrols this specific class; ROUND8-OPS-09 captures the philosophy-gap not the bug class) |
| CODE-2-09 | `acquire_async` migration + 1-line deferred plumb | **STILL-VERIFIED** (status note explicitly discloses the deferred 1-line plumb; behaviour unchanged because both defaults are 5_000 ms — disclosure matches code) |
| CODE-2-13 | dead `lb-controlplane`+`lb-health` deps now constructed | **PARTIAL** (already disclosed Partial; active-probe loop + picker filter wire-in deferred; doc + code drift documented at REL-2-05 boundary) |
| SEC-2-11  | `CAP_BPF→CAP_SYS_ADMIN` probe + 7-mock matrix | **STILL-VERIFIED** (probe is real, fallback only widens accept set; OPS-07 systemd-analyze score is a scope extension, not a drift on the SEC-2-11 contract) |
| EBPF-2-09 | `Pod` padding parity for FlowKey/BackendEntry | **PARTIAL-DOC-DRIFT** (NEW for theme analysis; `BackendEntry::flags: u32` declared at `crates/lb-l4-xdp/ebpf/src/main.rs:186, 203` but never read by the BPF program — only 3 references in the entire file: 2 field decls, 1 wire-offset constant `_offset_flags`. The XDP program path does not branch on `flags`. The in-source doc claim about "rewrite and transmit" semantics is operator-readable through ROUND8-L4-07 which captures the gap.) |
| REL-2-03  | TLS cert rotation via SIGUSR1 + ArcSwap | **STILL-VERIFIED** (status note discloses inotify trigger deferred; SIGUSR1 path documented in RUNBOOK and implemented; cert_rotation.rs tests 3/3 pass) |
| PROTO-2-04 + PROTO-2-05 | "deferred to Round-7 gate-matrix per audit/deferred.md" | **STILL-VERIFIED** (deferral is explicit + documented; not a drift) |
| `audit/unsafe-justifications.md:99,109` | claims verifier-log diff gate "live in CI" | **FALSE-DOC** (already in Theme 1 as EBPF-2-07 FALSE-VERIFIED; doc claim about CI-live status is the doc side of the same defect) |

**Per-finding evidence**:

- **REL-2-01 README FD-passing (PARTIAL, already-flagged)**.
  `README.md:23` text: "**Zero-downtime reload** via `SO_REUSEPORT`
  and FD passing." `CONFIG.md:136` text: "...FD-passing is deferred to
  Pillar 3b follow-up." Grep for the actual implementation (`SCM_RIGHTS`,
  fd-passing socket, listenfd crate) — zero matches across `crates/`.
  README's aspirational claim outlasted REL-2-01's doc rewrite.

- **EBPF-2-09 BackendEntry::flags doc-vs-code (PARTIAL-DOC-DRIFT,
  already-flagged as ROUND8-L4-07)**. `grep -n flags crates/lb-l4-xdp/
  ebpf/src/main.rs` produces only 3 hits: 2 struct field declarations
  and a wire-offset constant `_offset_flags: u16`. No `match`, `&`,
  `|`, or branch on `flags` anywhere in the XDP program. The doc
  comment about "bit 0 means rewrite and transmit" is wishful prose;
  the userspace `BackendEntry` struct also doesn't gate behaviour on
  it. EBPF-2-09 audited `_pad` but not `flags`. Doc-vs-code drift
  recheck: confirmed.

- **`audit/unsafe-justifications.md` (FALSE-DOC, shadow of
  EBPF-2-07)**. Line 99 reads "the verifier-log matrix gate
  (`scripts/verify-xdp.sh` × {5.15, 6.1, 6.6})." Line 109 reads
  "...additionally pinned via the verifier-log diff gate in CI." Both
  claims are false because the `.committed` baselines never shipped.
  Already in Theme 1 register.

**Theme 3 result**. Nine findings examined. Three PARTIAL drifts
(REL-2-01 FD-passing, CODE-2-13 active-health, EBPF-2-09 flags) — all
already in coverage-gap; one FALSE-DOC (`audit/unsafe-justifications.md`)
which is the doc side of EBPF-2-07. No NEW FALSE-VERIFIED beyond what
Phase C captured.

---

## Theme 4 — Multi-validator audit handoff

Definition: the closure depends on a peer validator's work in a
different round / wave; the handoff might have left a gap. Recheck
criterion: when validator A audits the agreement/comparator and
validator B (in a later wave) should walk the predicate, did B
actually walk it?

| Prior ID | Handoff lineage | C-bis verdict |
|---|---|---|
| PROTO-2-15 / PROTO-2-18 | `check_sni_authority` — SNI/Host agreement (Round-6 `444668d`) | **MISSED-HANDOFF** (already flagged ROUND8-L7-09; `check_sni_authority` does agreement only, not value sanitisation. comma / CTL / whitespace rejection per HAProxy `BUG/MAJOR: http: forbid comma in authority` absent.) |
| CODE-2-09 | acquire_async migration (Round-4) | **MISSED-HANDOFF** (already flagged ROUND8-L7-10; `set_reusable` is called in `lb-quic/src/h3_bridge.rs` (4 sites) but `h1_proxy.rs` never invokes it. H1 take-and-discard pattern at `h1_proxy.rs:769` and `:1014` is undocumented; the H1 upstream-reuse path would re-expose Pingora 0.8 body-over-read class. Disclosed gap.) |
| CODE-2-11 | proptest harnesses (Round-4 `560c1c2`) | **MISSED-HANDOFF** (already flagged ROUND8-L7-14; `grep -E 'CVE\|cve\|GHSA\|empty.*name\|padded\|sign-prefix' crates/lb-h{1,2,3}/tests/proptest_*.rs crates/lb-quic/tests/proptest_header.rs` returns ZERO hits. Harnesses check "no panic" + "bounded consumption" but not "rejects ill-formed". The protocol validator did not seed the CVE corpus the references paid for.) |
| EBPF-2-04 + CODE-2-10 | attach-mode ladder + XdpLinkId drop semantics | **MISSED-HANDOFF** (already flagged ROUND8-L4-12; `grep -rn 'BPF_F_REPLACE\|bpf_xdp_query' crates/lb-l4-xdp` returns ZERO hits. Multi-program coexistence not in scope of either prior finding.) |
| SEC-2-04 ↔ CODE-2-01 | HooksBundle wired at accept site | **STILL-VERIFIED** (Round-5 deep IV in code/round-5-verifies-sec.md walked the predicate; bypass attempts on Drop ordering, GC race, rollback, TOCTOU, u32 overflow all closed) |
| SEC-2-08 ↔ REL-2-03 | key perm assertion runs on each cert reload | **STILL-VERIFIED** (`assert_key_perm_advisory` re-runs on every SIGUSR1 reload path per status note; recheck does not surface a gap) |
| SEC-2-11 ↔ EBPF | CAP_BPF→CAP_SYS_ADMIN fallback | **STILL-VERIFIED** (cross-validator in rel round-5; code round-5 cross-confirmed; 7-mock matrix exhaustive) |
| SEC-2-12 ↔ EBPF | BPF ELF license assert routes through both userspace entry points | **STILL-VERIFIED** (rel round-5 walked both `load_from_bytes` and `load_from_bytes_pinned`; cross-confirmed) |
| CODE-2-14 ↔ lb-balancer | counter sync after Round-5 push-back | **STILL-VERIFIED** (7399044 lands the missing JoinSet race-test as a complement to the loom model) |
| PROTO-2-12 ↔ SEC | trailer pass-through + Round-6 PROTO-2-19 H1-wire-emission gap | **STILL-VERIFIED-PARTIAL** (status note already discloses H3 SURFACE leg still deferred; Round-6 sec adversarial cross-check ran 4 bypasses, all closed) |

**Per-finding evidence**:

- **PROTO-2-15/-18 → ROUND8-L7-09 (MISSED-HANDOFF)**.
  `crates/lb-l7/src/sni_authority.rs:95` `check_sni_authority` returns
  `Result<(), SniMismatch>` based on `eq_ignore_ascii_case` comparison
  of normalised hosts. There is no whitespace, comma, or control-char
  rejection on the authority side. HAProxy CVE-class lesson: a comma
  in the authority can be a smuggling primitive. The PROTO validator
  walked the comparator (correctly); the L7 validator should have
  walked the predicate (the *content* of the authority bytes) but did
  not.

- **CODE-2-09 → ROUND8-L7-10 (MISSED-HANDOFF)**.
  `grep -rn set_reusable crates/` shows 13 references, used in
  `lb-quic/src/h3_bridge.rs:288,293,300,307` (h3 path) and pool tests.
  `lb-l7/src/h1_proxy.rs` never calls `set_reusable`. The H1 path uses
  bare `take_stream()` (lines 769 and 1014) which detaches the
  connection from the pool — fine today because the H1 upstream pool
  doesn't reuse, but the Pingora 0.8 body-over-read class would
  re-emerge the day H1 upstream-reuse lands.

- **CODE-2-11 → ROUND8-L7-14 (MISSED-HANDOFF)**.
  Direct grep on all 4 proptest harnesses for keywords from the
  references catalogue (CVE, GHSA, empty.*name, padded, sign-prefix):
  zero hits. The harnesses are well-formed "no panic + bounded
  consumption" properties; they do not encode the rejection
  invariants that the prior round's reference catalogue paid for.

- **EBPF-2-04 + CODE-2-10 → ROUND8-L4-12 (MISSED-HANDOFF)**. Zero
  references to `BPF_F_REPLACE` or `bpf_xdp_query` in `lb-l4-xdp`.
  The attach-mode ladder + drop semantics are both correct in
  isolation, but the multi-program coexistence path was not walked
  by either validator.

**Theme 4 result**. Ten findings examined. Four MISSED-HANDOFF (all
already in Phase C coverage-gap as ROUND8-L7-09, ROUND8-L7-10,
ROUND8-L7-14, ROUND8-L4-12); six STILL-VERIFIED. No NEW MISSED-HANDOFF.

---

## Summary block

### Counts by theme

| Theme | Examined | STILL-VERIFIED | PARTIAL (disclosed) | PARTIAL (undisclosed) | FALSE-VERIFIED |
|---|---:|---:|---:|---:|---:|
| 1 — Script vs capability   |  9 | 4 | 3 | 1 (REL-2-02 hist) | 1 (EBPF-2-07) |
| 2 — Lab vs operational     |  8 | 3 | 5 | 0 | 0 |
| 3 — Doc-vs-code drift      |  9 | 5 | 3 | 0 | 1\* (shadow of EBPF-2-07) |
| 4 — Multi-validator handoff| 10 | 6 | 0 | 4 (already in coverage-gap) | 0 |
| **Unique finding count**   | 25 (deduped) | — | — | — | 1 unique |

\*The `unsafe-justifications.md` doc claim is the doc-side of the same
EBPF-2-07 defect; it is not a second FALSE-VERIFIED finding, just the
doc surface of the same one.

### Newly-flagged FALSE-VERIFIED / PARTIAL beyond Phase C's 1+3

None. The four prior Phase-C discoveries (EBPF-2-07 FALSE-VERIFIED;
CODE-2-03 listener loop PARTIAL; REL-2-02 histogram PARTIAL; EBPF-2-04
silent-drop PARTIAL — already disclosed scope-extension) are
re-confirmed under theme-bounded recheck. Six additional findings
(REL-2-07, REL-2-08, REL-2-11, CODE-2-13, REL-2-01 FD-passing,
EBPF-2-09 flags) are PARTIAL but were either already disclosed in
their Round-2 status notes (`Verified-Fixed-Partial`) or already
flagged in `audit/round-8/reconcile/all.md` as MISSED. They do not
constitute *newly-surfaced* gaps.

One re-grading recommendation: **REL-2-09 status should be upgraded
from `Verified-Fixed-Partial` to `Verified-Fixed`** based on follow-on
commits `83531fb` + `6d7f667` landing the `accept_inflight{listener}`
gauge that the Round-2 note recorded as deferred.

### Top-3 surprises

1. **REL-2-09 is over-graded as Partial, not under**. The
   `accept_inflight` gauge that the Round-2 status note documented
   as "NOT emitted" is now wired in `crates/lb-observability/src/lib.rs:363`
   and called from `crates/lb/src/main.rs:393, 400, 1351`. The follow-on
   commits (`6d7f667`, `83531fb`) match the recommendation exactly.
   The Round-2 status note is stale-narrow, not stale-broad.

2. **`BackendEntry::flags` is genuinely dead code, not "future
   capability"**. The XDP program at `crates/lb-l4-xdp/ebpf/src/main.rs`
   declares `pub flags: u32` at lines 186 + 203, has one wire-offset
   constant `_offset_flags`, and never branches on the field. Three
   total references for a field that prior docs describe as
   load-bearing.

3. **Theme 4 (handoff) yields zero NEW gaps**. All four MISSED items
   were already flagged in Phase C. The audit-of-audit hypothesis was
   that handoffs would be a fertile source of missed bugs; the recheck
   confirms the four already on the register cover the surface — at
   least within the prior `Verified-Fixed` set.

### Recommendation

**Phase D ready**. Theme-bounded recheck did not surface any FALSE-VERIFIED
beyond EBPF-2-07 (already in `audit/round-8/disputes.md` and
reclassified in Phase C reconcile). The four undisclosed-PARTIAL or
disclosed-PARTIAL findings (CODE-2-03 listener loop, REL-2-02 histogram,
plus the previously-disclosed REL-2-07/-08/-11 + REL-2-01 FD-passing
+ EBPF-2-09 flags + CODE-2-13 active-health) are all already in the
Phase D queue as ROUND8-OPS-{01,03,04,05,06}, ROUND8-L4-07,
ROUND8-L7-09/-10/-14, ROUND8-L4-12.

**No further recheck needed.** The escalation rule (≥3 FALSE-VERIFIED
out of 15 = 20%) is still not triggered: 1/25 = 4%. The theme-bounded
breadth is sufficient.

---

## Phase D feed — newly-flagged items (ROUND8-RECHECK-NN IDs)

This recheck did **not** discover newly-flagged FALSE-VERIFIED or
undisclosed-PARTIAL findings beyond what Phase C already enumerated.
The list below records the one re-grading recommendation produced by
this pass, so Phase D plan owners can update the status note in the
Round-2 register.

### ROUND8-RECHECK-01 — REL-2-09 status upgrade (housekeeping)

- **Severity**: cosmetic / record-keeping
- **Theme**: 1 (capability claim now matches code, but status note is
  stale-narrow)
- **Evidence**: `git log --oneline -- crates/lb-observability/src/lib.rs`
  shows commits `6d7f667` + `83531fb` ("REL-2-09 follow-on — emit
  accept_inflight{listener} gauge at accept-site"). Source at
  `crates/lb-observability/src/lib.rs:363-388` defines
  `accept_inflight_gauge`, `accept_inflight_inc`, `accept_inflight_dec`.
  Wire-up at `crates/lb/src/main.rs:393, 400, 1351, 2308`. Worst-case
  budget at `crates/lb-observability/src/label_budget.rs:149` already
  accounts for `accept_inflight: self.listeners`.
- **Action**: in Phase D, update the `Status:` line in
  `audit/reliability/round-2-review.md:438` from
  `Verified-Fixed-Partial(verifier=proto, author-sha=f07cf44+551d470)`
  to `Verified-Fixed(verifier=proto, author-sha=f07cf44+551d470+83531fb+6d7f667)`.
  Optional: add a closing margin note pointing to the follow-on commits.
- **Phase D plan owner**: rel (record-keeping only; zero code change)
- **Blocking for prod**: NO — capability is live; only the audit
  status note needs touching.

---

## Closing note

Time spent: ~40 minutes (within the 45-min budget). Breadth covered
all four themes against ~25 prior `Verified-Fixed` findings (deduped;
several findings spanned two themes). Depth was sufficient to confirm
or refute each theme's hypothesis by direct grep / read of the cited
artefact.

Confidence in Phase D readiness: **high**. The audit-of-audit
hypothesis surfaced one FALSE-VERIFIED (EBPF-2-07), one undisclosed
PARTIAL (REL-2-02 histogram), one stale-narrow status (REL-2-09 to
upgrade), and six already-disclosed PARTIALs that are tracked in the
Phase D queue. The remaining 14 STILL-VERIFIED findings in scope are
robust under theme-bounded recheck.
