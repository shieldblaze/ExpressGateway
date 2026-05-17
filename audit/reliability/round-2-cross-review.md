# Round 2 — Reliability Cross-Review (`rel`)

Reviewer: `rel` (SRE / observability / lifecycle).
Inputs available at write-time: `audit/ebpf/round-2-review.md`.
Pending at write-time (not yet on disk): `audit/security/round-2-*.md`,
`audit/code/round-2-*.md`, `audit/protocol/round-2-*.md`.

Verdicts:

- **AGREED** — finding stands as written; severity reasonable.
- **DISPUTED** — disagree on technical content; reason below.
- **ESCALATE-SEVERITY** — agree on facts, want a higher severity.
- **DOWNGRADE-SEVERITY** — agree on facts, want a lower severity.
- **PENDING** — file not yet on disk; placeholder for the
  round-2-late pass.

---

## On `audit/ebpf/round-2-review.md`

### EBPF-2-01 — BPF ELF has no `license` section
Verdict: **AGREED**.
Severity (high): correct given aya-obj 0.2.1's `"GPL"` default makes
this a latent-startup bug rather than an active one. The "future aya
upgrade flips this to SEV-1" framing is exactly the kind of soft
landmine SRE wants explicit.
No reliability concerns to add — the recommendation (explicit
`link_section`) is also the minimal-risk fix.

### EBPF-2-02 — `set_license` does not exist in aya 0.13.1
Verdict: **AGREED**.
Severity (medium): right call — this is a brief-correction finding,
not a code defect. Important to record so the audit isn't reopened
in Round 3.

### EBPF-2-03 — CONNTRACK / CONNTRACK_V6 are `HashMap`, not `LRU_HASH`
Verdict: **AGREED with cross-ref to REL-2-12**.
Severity (high): correct. The attack model write-up is the cleanest
articulation of T4 I've seen across all four files.
Reliability dependency: REL-2-12 owns the userspace
`xdp_conntrack_full_total` counter + the alert rule
(`xdp_conntrack_entries_current / map_capacity > 0.85 for 5m`). Both
findings refer to each other; no work duplicated. The alert can't fire
until the metric exists, and the metric is part of EBPF-2-08.
Suggested Round-3 ordering: EBPF-2-08 (STATS export) → REL-2-12
(metric + alert) → EBPF-2-03 (LRU fix). Once LRU lands, the alert
threshold shifts from "fast-path bypass imminent" to "memory pressure
warning" — the alert stays useful, the operational meaning improves.

### EBPF-2-04 — XDP attach hard-coded to SKB mode, no fallback
Verdict: **AGREED**.
Severity (high): correct. The 10×–50× perf gap on real NICs is a
production blocker for any deployment that claims Katran-class.
Reliability slot: the `xdp_attached_mode{mode}` gauge in
recommendation point 2 is exactly the surface I want for the
runbook's "did we get the fast path?" alert. Strong preference for
**Option A (fallback ladder)** + the metric — Option B (operator
knob) is good complement but the auto-fallback covers more deployment
shapes. Add: `xdp_attach_attempts_total{mode,result}` should be a
histogram or counter pair, not a single counter (need result-label
cardinality `success|eopnotsupp|einval|other`).

### EBPF-2-05 — No map pinning; every restart starts cold CONNTRACK
Verdict: **AGREED, with one reliability-side amendment**.
Severity (high): correct. The reload-zero-drop test claim in our own
runbook (RUNBOOK.md and `tests/reload_zero_drop.rs`) only works
because CI doesn't run with XDP attached; in prod every restart is a
flow massacre.
Reliability amendment: the recommendation should land in **coordinated
order** with REL-2-02 (SIGTERM drain). Specifically, the new
drain sequence (REL-2-02 step 2(d)) must NOT detach the BPF program
before the userspace control plane has flushed its last
batch-insert. Order: `/readyz=503` → drain L7 → drain userspace XDP
inserter → THEN drop `XdpLoader` (which detaches). If we drop
`XdpLoader` first, the kernel forwarding stops mid-drain and the
remaining L7 traffic gets unrouted packets.
Also: the bpffs perms (recommendation point 6) overlap with what
`sec` will probably surface in their round-2 file. Cross-team
coordination needed on the bpffs mount and the directory mode.

### EBPF-2-06 — Dropped `XdpLinkId` is safe in aya 0.13.1
Verdict: **AGREED**.
Severity (low): correct given the registry-source confirmation. The
proposed regression test is the right level of paranoia — it catches
any aya upgrade that quietly changes ownership semantics, without
imposing test-on-prod-hardware costs today.

### EBPF-2-07 — No verifier-log matrix captured
Verdict: **AGREED**.
Severity (medium): correct. This is exactly the kind of CI-image
work the Round-1 synthesis §D flagged.
Reliability angle: the kernel-version matrix (5.15.x / 6.1.x / 6.6.x)
must match the kernel floor declared in DEPLOYMENT.md:27. If we
ever bump that floor, REL-2-01's doc-truthfulness umbrella needs an
update too (the runbook references kernel features assuming a
particular floor). Suggest adding a single source-of-truth file
`docs/kernel-floor.toml` that both `xtask xdp-verify` and the
runbook consume.

### EBPF-2-08 — STATS per-CPU array never exported
Verdict: **AGREED, joint owner with REL-2-13**.
Severity (medium): correct, BUT this is the prerequisite for
REL-2-12's alert firing — until this lands, the
`xdp_conntrack_full_total` metric I want has no kernel-side source.
Suggested Round-3 ordering puts this **before** EBPF-2-03's LRU
change so the alert can prove it would have fired pre-LRU and
doesn't false-fire post-LRU.
Metric naming: I want `xdp_packets_total{action="..."}` (not
`result=`) to match the convention I'll use in REL-2-13. Pick one
label key and use it in both files. I'll defer to `ebpf` if `result`
reads better — but it must be consistent across all `xdp_*`
families.

### EBPF-2-09 — Pod padding parity confirmed BPF-side; userspace fix is `code`'s
Verdict: **AGREED**.
Severity (medium): correct on this side; the per-team severity is
the ebpf-side confirmation, the userspace-side severity should be
`code`'s call.
Reliability angle: zero — this is a correctness bug with no
operability surface. Once `code` ships the constructor + the
`proptest` invariant, the conntrack hit-rate metric (`xdp_conntrack_hits_total`
from EBPF-2-08) will be the post-fix verification signal.

---

## On `audit/security/round-2-review.md`
Verdict: **PENDING** — file not on disk at the time of writing.
On materialisation, expect at minimum:
- A finding on TLS ticket-key persistence (Round-1 §S — open question
  from my msg-to-sec).
- The 0-RTT-on-TCP disagreement resolution per lead synthesis §B.1.
- A bpffs-perms finding paired with EBPF-2-05.
- Cert-validation joint-owner sign-off on REL-2-03.

I will issue verdicts in a `round-2-cross-review-late.md` if `sec`
publishes after this file.

---

## On `audit/code/round-2-review.md`
Verdict: **PENDING** — file not on disk at the time of writing.
On materialisation, expect at minimum:
- The `CancellationToken` plumbing finding (joint with REL-2-02).
- The `Semaphore` finding (joint with REL-2-09).
- The accept-backoff finding (joint with REL-2-10).
- The `spawn_blocking` alternative evaluation (joint with REL-2-11).
- The four `Pod` constructor zero-init fix (joint with EBPF-2-09).
- `cargo machete` evidence for `lb-controlplane` / `lb-health` /
  `lb-cp-client` (cross-ref REL-2-05).
- Panic-hook plumbing finding (joint with REL-2-15).
- `Ordering::Relaxed` audit per lead synthesis T6.

I will issue verdicts in a `round-2-cross-review-late.md` if `code`
publishes after this file.

---

## On `audit/protocol/round-2-review.md`
Verdict: **PENDING** — file not on disk at the time of writing.
On materialisation, expect at minimum:
- H2 GOAWAY + H3 CONNECTION_CLOSE emission during drain (joint with
  REL-2-02).
- `:authority` vs `Host` mismatch enforcement (RFC 9113 §8.3.1).
- `LB_QUIC_ALPN` byte-string fix (`b"h3"` not `b"lb-quic"`).
- 100-Continue policy + test.
- Detector wiring matrix (T1 partial owner).
- Bridge-trait hop-by-hop placement decision (synthesis §B.3).

I will issue verdicts in a `round-2-cross-review-late.md` if `proto`
publishes after this file.

---

## Cross-cutting reliability concerns (lead-only)

A handful of items don't sit cleanly inside any one teammate's file
but block the Round-3 plan. Recording them here so the lead has them
in one place:

1. **Drain ordering is multi-team.** REL-2-02 has 4 steps; each one
   touches a different teammate's surface area:
   - Step (a): admin listener flips `/readyz` — `rel` owns
     (REL-2-04).
   - Step (b): per-listener cancel — `code` owns the
     `CancellationToken` plumbing.
   - Step (c): per-protocol GOAWAY / CONNECTION_CLOSE — `proto`
     owns.
   - Step (c, XDP-coupled): userspace XDP inserter must drain
     before `XdpLoader` drops — `ebpf` owns the loader-drop
     ordering (per EBPF-2-05 amendment above).
   Round-3 needs a single coordinated PR or a strict
   sequence-of-PRs; piecemeal merge will leave the binary in a
   half-drained state on every deploy until the last PR lands.

2. **Metric naming convention.** REL-2-08 / REL-2-09 / REL-2-10 /
   REL-2-12 / REL-2-13 / EBPF-2-04 / EBPF-2-08 all introduce new
   metric families. Without a published naming convention they will
   drift. Suggest a `docs/metrics-conventions.md` that nails down:
   - Label keys (`listener`, `backend`, `route`, `kind`, `action`,
     `result`, `family`).
   - Counter suffix (`_total` for monotonic, no suffix for gauges,
     `_seconds`/`_bytes` for histograms with units).
   - Cardinality budgets per family.
   `rel` is happy to own this artefact in Round 3.

3. **Doc truthfulness is a sweep, not a finding.** REL-2-01 is an
   umbrella; the actual remediation is "every fictional flow either
   gets implemented or struck from the doc". That's a repeated
   action across REL-2-02, REL-2-03, REL-2-05, REL-2-06, REL-2-14
   and at least one finding from each other teammate (sec on
   cert/ticket, code on the unused-deps, proto on the conformance
   stub `ws_autobahn.rs`). Lead should consider whether the Round 3
   plan calls out a dedicated "doc-rewrite + CI doc-lint" workstream.

4. **Round 3 ordering hint** (reliability-priority): REL-2-02 →
   REL-2-04 → REL-2-09 → REL-2-10 → REL-2-15 → REL-2-03 →
   REL-2-05 → rest. Reasoning: REL-2-02 has the largest blast
   radius (every deploy) and unblocks REL-2-04's `/readyz` semantics.
   REL-2-09/10/15 share the listener-loop refactor that REL-2-02
   already opens. REL-2-03 + REL-2-05 share the SIGHUP + ArcSwap
   foundation. Everything else is independent.
