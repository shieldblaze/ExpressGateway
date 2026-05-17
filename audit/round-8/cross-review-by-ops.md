# Cross-review by `div-ops` — Round 8 Phase B

Author: `div-ops`, 2026-05-14. Reviews the 15 L7 + 12 L4 findings filed
by `div-l7` and `div-l4`. The lens: can an on-call engineer at 03:00
**diagnose** the underlying defect from `/metrics`, traces, or
structured logs as they exist today? If not, that gap is itself a
cross-cut and is flagged here.

Stance vocabulary (per template):

- **CONFIRM** — the finding stands as written; ops cannot help.
- **CHALLENGE** — disputed severity, scope, or root cause.
- **ESCALATE-SEVERITY** — finding is correct but undersold *because*
  ops-blindness amplifies the defect.
- **CROSS-CUT** — the finding correctly describes the data-plane
  defect, but it is undiagnosable until a metric / log / trace lands
  (one of OPS-01..12).

---

## L7 cross-review (15 findings)

### ROUND8-L7-01 — Premature 101 (WebSocket upgrade)

**ESCALATE-SEVERITY + CROSS-CUT.**

The data-plane defect is rated High by `div-l7`. Pingora and Envoy both
shipped at Critical (CVSS 9.3 / GHSA-rj35-4m94-77jh). The reason
`div-l7` discounted was "the smuggled-second-request angle requires
hyper's `OnUpgrade` to surface buffered bytes; the unconditional
101-emit + dial-fail DoS is the easier exploitation." That discount is
acceptable for the worst-case attack but ignores the *operability*
amplifier:

- **Combined with OPS-06 (no L7 tracing callsites)**: when a flood of
  WS handshakes goes to an upstream that rejects them, the on-call
  engineer sees an inexplicable storm of "101 returned + connection
  torn down" lines in connection-level logs with **no per-request
  span** to correlate which client, which path, which upstream. The
  RUNBOOK has no `LbWebSocketUpstreamReject` alert (none of `LbWS*`
  alerts exist).
- **No `lb_ws_upstream_handshake_failed_total{listener,reason}`
  counter** exists today (`grep ws.*total crates/` returns nothing).
  The user-observable signal is `lb_active_connections` ticking up,
  then `connection_aborted` ticking. Neither flags WS specifically.

The page-without-a-span shape this creates is **the** combined-impact
scenario the brief explicitly flagged. Escalate severity to Critical
on combined criteria.

**Decision needed**: own the fix (re-order dial-before-101) AND add
both a `lb_ws_upstream_handshake_total{result}` counter and the
`lb.l7.ws.upgrade` span; bundle as one PR.

### ROUND8-L7-02 — Chunk-size hex parser (`+`, whitespace, no cap)

**CONFIRM.** No ops cross-cut. A smuggling primitive is detected (or
not) at the proxy layer; counters under `lb_smuggle_*` exist for
TE/CL conflict but not for malformed chunk-size lexemes specifically.
Adding `lb_chunked_lexer_rejected_total{reason}` alongside the fix is
nice-to-have but not blocking. Defer to the fix PR.

### ROUND8-L7-03 — Empty header name accepted

**CONFIRM.** No ops cross-cut. The smuggle detector's per-header miss
on empty names would be observable as "two upstreams report different
header sets for the same flow"; that is hard to attribute without
distributed tracing (OPS-06), but adding tracing isn't required to
*fix* this — fix at parse time.

### ROUND8-L7-04 — `append_xff` clobbers multi-value XFF

**ESCALATE-SEVERITY + CROSS-CUT.**

Today XFF appending is silent. The post-fix world is also silent if we
don't emit a metric. Specifically:

- **Reference (Envoy GHSA-ghc4-35x6-crw5)**: the bug class was an RBAC
  bypass downstream of *some* consumer that iterates and another that
  joins. Operators with multi-tier deployments are blind to this
  *today* — the truncation happens in a `format!` call.
- The recommendation in the finding says "audit every other
  `headers.get(...)` site in `lb-l7` and `lb-security`" — that's the
  audit, not an observability check. No metric exists for "request
  arrived with N>1 XFF entries"; operators cannot tell whether they
  are at risk from this class.
- Severity should escalate to **High** on the combined operability
  criterion: a multi-tenant deployment that depends on XFF integrity
  for ACL has zero signal that we collapsed the chain.

**Cross-cut work**: add `lb_xff_input_value_count` histogram (bucket
1, 2, 3, 5, 10) and `lb_xff_collapsed_total{listener}` counter
incremented when the pre-fix code would have clobbered. Lets operators
*see* the impact and triage existing deployments.

### ROUND8-L7-05 — No `headers_with_underscores` policy

**CONFIRM.** Ops can diagnose: once the policy lands, the 400 / drop
emits a `lb_http_requests_total{status_class="4xx"}` tick.
Add `lb_header_policy_action_total{policy,action,header_name}` if we
want per-header attribution, but not blocking.

### ROUND8-L7-06 — No `keepalive_requests` count cap

**CROSS-CUT.**

The finding recommends `lb_keepalive_terminated_by_count_cap_total{listener}`.
Confirmed. Two adjacent metrics would let the operator empirically
pick the cap before deploying it:

- `lb_keepalive_request_count_per_connection_close` histogram —
  observed request count per closed keepalive connection. Lets the
  operator see "p99 of well-behaved clients is 47 requests; cap at
  100 is safe" instead of guessing.
- `lb_keepalive_long_lived_connections{listener}` gauge — current
  connections with `request_count > N`. Lets the operator know
  whether their cap is currently biting.

Both should be part of the fix PR, not a follow-up.

### ROUND8-L7-07 — No frame-level H2 read timeout

**CROSS-CUT.**

When this lands, the operator needs to distinguish
"slowloris-class abuse" from "legitimate slow uploaders behind
satellite link". Today's H2 abuse counters (rapid-reset, continuation-
flood, hpack-bomb) are six independent thresholds (see L7-12); adding
a seventh without consolidating into the HAProxy `glitches` metric
makes the operator UX strictly worse.

Bundle L7-07 fix with L7-12 (`glitches-threshold`); land one
consolidated counter `lb_h2_glitches_per_connection{listener}` with
sub-counters by kind, including the new `recv_frame_timeout`.

### ROUND8-L7-08 — H2 upstream read-timeout drops without RST_STREAM(CANCEL)

**ESCALATE-SEVERITY + CROSS-CUT.**

The finding correctly identifies that the upstream's rapid-reset
detector may count *us* as the attacker. Today the operator's view of
this is:

- `lb_h2_upstream_pool_eviction_total{reason="timeout"}` is the only
  signal we have. We have **no** counter for upstream-emitted GOAWAY
  that lands on us. When an upstream Envoy sends us GOAWAY because we
  look like a rapid-reset peer, all we see is 5xx on the next request.
- No span exists for "the upstream H2 connection that just died" —
  combined with OPS-06 it is functionally undiagnosable.

Recommend escalation to **High**. Without a counter we will not know
this is happening until a customer reports rolling 5xx during high
load.

**Cross-cut**: add `lb_h2_upstream_goaway_received_total{error_code}`
and `lb_h2_upstream_cancel_emitted_total{reason}`. Fixes are
inexpensive; without them, the data-plane fix is invisible.

### ROUND8-L7-09 — Authority comma / control-char

**CONFIRM.** Lesson-not-yet-paid-for as `div-l7` writes. No ops
cross-cut today (no host-based routing). Re-flag at host-routing
landing.

### ROUND8-L7-10 — Body over-read does not mark non-reusable

**CONFIRM.** `div-l7` correctly notes the H1 take-and-discard pattern
makes today's behaviour safer than feared. Add a code comment per
the recommendation. No ops cross-cut.

### ROUND8-L7-11 — `lb-h2::frame` ignores PADDED

**CONFIRM.** Test-only today; the fix is a one-liner. No ops
cross-cut.

### ROUND8-L7-12 — N independent H2 detectors, no `glitches` consolidation

**ESCALATE-SEVERITY (operability).**

`div-l7` rated Low. From an ops standpoint this is **Medium**.
HAProxy added `glitches-threshold` specifically because operators
could not tune six knobs. Our six-detector layout forces every
on-call SOP to enumerate six runbooks:

- `RUNBOOK.md` doesn't have a "H2 abuse triage" section today.
  Writing one for six detectors is a 6-section task; writing one
  for `glitches` is one section.
- Six counters means six dashboards. The operator's pager doesn't
  know to look at all six.

Cross-link to L7-07: bundle the consolidation with the new
`recv_frame_timeout` so we add one metric, not two.

### ROUND8-L7-13 — No URI / path normalisation

**CONFIRM.** Documented choice (transparent forwarder). The
recommendation to write `audit/decisions/path-normalisation.md` is
correct; without it, on-call will misdiagnose a normalisation-related
backend ACL bypass as "our LB rewrote the URI" and waste hours.
Doc-decision is the ops contribution.

### ROUND8-L7-14 — Proptest does not seed CVE-class inputs

**CONFIRM.** Meta-finding. No ops cross-cut.

### ROUND8-L7-15 — Edge defaults parity

**CROSS-CUT.**

The recommendation includes `compile_time_defaults` drift-detection
test. That belongs to the same family as ops-09 (doc-lint): both are
"contract gates that catch silent drift". Suggest landing them in a
shared `cargo defaults-check` binary that imports the actual consts
and asserts:

- the value appears verbatim in `config/default.toml`
- the value appears verbatim in `RUNBOOK.md`
- the value appears verbatim in `CONFIG.md`

This is the "one source of truth, three doc gates" pattern.

---

## L4 cross-review (12 findings)

### ROUND8-L4-01 — Backend-index 0 / zero-IP valid

**CROSS-CUT.**

The finding recommends `STAT_BACKEND_UNPOPULATED = 10`. Confirmed.
Today, the operator's view of "a half-written conntrack entry caused
black-hole" is:

- `xdp_packets_total{result="ct_hit_v4"}` ticks — looks healthy.
- `xdp_packets_total{result="tx_v4"}` ticks — also looks healthy.
- The packets land on `0.0.0.0:0` and are lost upstream of any
  metric we publish.

A counter + a `WARN` log on first occurrence (rate-limited) is
non-optional. Adding only the BPF guard without the counter still
leaves the operator blind to *how often* it would have triggered if
the guard didn't exist (i.e. how often userspace is publishing
half-written entries). Bundle the counter with the BPF guard.

### ROUND8-L4-02 — Pure LRU conntrack, no TCP state awareness

**ESCALATE-SEVERITY + CROSS-CUT.**

`div-l4` rates High and notes "lesson-not-yet-paid-for". From an ops
standpoint this finding is **harder than it looks**:

- The L7-visible symptom of a successful replay-old-RST attack is
  *legitimate flows seeing fresh SYNs they did not initiate* — which
  L4 does not surface because by the time the LRU evicts, the flow
  is gone. Today we have **no** counter for "conntrack entry
  evicted while in ESTABLISHED state" (because we don't track
  state). The symptom for the operator is "5xx rate jumps for no
  reason; the TCP retransmit counter on the upstream rises".
- The brief explicitly asks: "what L7-visible symptom would expose
  it? Connection resets per second, latency spike, RST counter — flag
  if absent." All three are absent at L7 too. We do not export
  upstream-RST-received or upstream-retransmit metrics from
  `lb-l7/h1_proxy.rs`. The `xdp_conntrack_*` stats are static
  cardinality counts, not "rate of eviction of LRU entries that
  recently saw bidirectional traffic".

This is the canonical ops-amplifier shape. The data-plane fix is
acknowledged-deferred; the *observability for the deferred state*
needs to land **even if we defer the fix**.

**Cross-cut artefact**: `xdp_conntrack_lru_evicted_total` (already
acknowledged in L4-02 recs), plus an L7-side
`lb_upstream_tcp_rst_received_total{listener,backend}` counter so
the on-call has a SLO signal that maps to the attack.

### ROUND8-L4-03 — No SYN-flood / new-flow-rate cap

**CONFIRM + CROSS-CUT (same shape as L4-02).**

The finding's recommendation includes `STAT_NEW_FLOW_RATE_CAP`.
Confirmed. Same ops-cross-cut as L4-02: the operator needs to know
the cap is engaging, not just that the LB survived. Adding the
counter is non-optional with the fix.

### ROUND8-L4-04 — Non-atomic backend-table updates

**CROSS-CUT.**

Lesson-not-yet-paid-for at our scale. The L7-visible symptom of a
mid-update inconsistency is "5xx burst during operator-initiated
rebalance". Today's rebalance metric coverage:

- `xdp_packets_total{result="ct_hit_v4"}` is per-packet, not
  per-rebalance.
- No `xdp_backend_table_publish_duration_seconds` histogram exists.
- The userspace `HotSwapManager` (`crates/lb-l4-xdp/src/lib.rs`) is a
  simulator; it does not export its own progress metric.

Recommendation: add `xdp_backend_table_publish_inconsistency_total`
counter that increments when a conntrack lookup hits an entry whose
generation differs from the per-VIP `BackendTable[vip].generation`
(once the design lands per L4-04 rec). Even before the design lands,
an interim signal `xdp_backend_update_syscalls_total{result}` per
syscall in the rebalance loop is a quick observability win.

### ROUND8-L4-05 — Drv→Skb attach fallback never runtime-probes XDP_TX

**ESCALATE-SEVERITY + CROSS-CUT.**

This is the canonical "metric lies, packet path is dead" failure
mode (the finding even names it). `div-l4` rates Medium. Combined
operability stance:

- `xdp_attach_mode = "drv"` is the only metric an operator has to
  confirm health. If the silent-drop class triggers, the metric is
  `drv` and the packet path is dead. Today there is **no second
  signal**.
- `lb_listener_packets_in_total` (L7-side) would *also* tick zero
  for the affected interface — but the operator has no way to
  correlate "interface foo has zero packets" with "XDP is silently
  dropping". They will assume the upstream LB stopped sending.

Escalate to **High**. The probe recommendation in L4-05 is the
only path; landing it without the
`xdp_attach_probe_failed_total{driver,fw}` counter is partial.

### ROUND8-L4-06 — `insert_acl_deny` accepts `prefix_len = 0`

**CONFIRM.** One-line fix. Ops contribution: `RUNBOOK.md` should
have a "ACL deny entry rejected at insert" alert path so that the
operator who hit the typo can see the error in the admin API
response — but that's a small follow-up.

### ROUND8-L4-07 — `BackendEntry::flags` is dead code

**CONFIRM.** Honesty fix. No ops cross-cut. Bundle with L4-01 per
`div-l4`'s priority list.

### ROUND8-L4-08 — IPv4 / IPv6 fragments silently passed

**CROSS-CUT.**

The finding correctly recommends `STAT_FRAGMENT` /
`STAT_V6_FRAGMENT`. The ops add: the operator needs a *log line*
the first time fragments are observed, rate-limited. Today the
counter alone is invisible without a dashboard panel. Add a
one-per-listener-per-minute structured log when the fragment
counter increments for the first time (`fragment_traffic_observed`
field, listener tag).

### ROUND8-L4-09 — `ptr_at` overflow guard

**CONFIRM.** Future-bug-shape. No ops cross-cut.

### ROUND8-L4-10 — Verifier-log no-op + EBPF-2-07 false-Verified-Fixed

**ESCALATE-SEVERITY + CROSS-CUT (AUDIT-OF-AUDIT).**

This is the L4 counterpart to OPS-09 (doc-lint too narrow). The
audit-of-audit point lands cleanly:

- `audit/unsafe-justifications.md:109` claims "validated by the
  kernel verifier on every load and additionally pinned via the
  verifier-log diff gate in CI" — that is false.
- `scripts/ci/doc-lint.sh` (OPS-09 scope) has six patterns, none
  of which would catch a false `Verified-Fixed` claim.

The combined finding is: **our CI gates do not catch lies in our
own audit register**. OPS-09's recommendation to add a
positive-assertion test ("every default appears verbatim in
RUNBOOK") generalises: every `Verified-Fixed(<sha>)` in
`audit/*/round-*-review.md` should have a CI gate that asserts
the artefact named in the recommendation block actually exists at
that sha. L4-10 is the trigger to write that gate.

Escalate to **High** (already High); cross-cut into OPS-09
explicitly.

### ROUND8-L4-11 — Loader doesn't enforce bpffs mount-type

**CONFIRM.** Already names the error path. Ops: the systemd unit
(`DEPLOYMENT.md:43-87`, see OPS-07) does **not** set
`RequiresMountsFor=/sys/fs/bpf`. Bundle the doc fix with the L4-11
loader fix so that when the loader emits
`PinPathNotBpffs(...)`, the unit also has the systemd-level
mitigation.

### ROUND8-L4-12 — XDP attach doesn't use `BPF_F_REPLACE`

**CROSS-CUT.**

Recommendation includes `xdp_attach_query_total{result}` metric.
Confirmed. Ops: in addition, the "previous process crashed leaving
pin alive" failure mode (named in the finding) is the same shape as
**OPS-04** (TCP listener accept loop relies on `JoinHandle::abort`).
Both are "clean detach on exit" failures. Bundle the runbook
documentation: "If a previous LB process crashed, the new process
will get EBUSY on attach; run `bpftool net detach` to clean up; OR
the new process will fail to drain cleanly because the accept loop
aborted mid-spawn — verify no orphan sockets on the previous pid via
`ss -tlnp`."

---

## Decisions for team-lead

1. **Combined-impact Critical: L7-01 + OPS-06.** Premature 101 is a
   pageable production defect today. Without OPS-06 (L7 tracing
   callsites), the on-call cannot triage. Both must land in one PR;
   landing only the data-plane fix leaves a fix that is invisible.
   **Escalate L7-01 severity to Critical on combined criteria.**

2. **AUDIT-OF-AUDIT cross-cut: L4-10 + OPS-09.** The verifier-log
   diff-gate is a permanent no-op AND the doc-lint script wouldn't
   catch the false `Verified-Fixed` claim. Both are symptoms of the
   same problem: our gates were written narrowly per the immediate
   bug, not as contract gates. Recommend a Round-8 deliverable:
   "every `Verified-Fixed(<sha>)` entry must point at a non-empty
   artefact named in the recommendation block, asserted by a CI
   gate." Owner: needs assignment.

3. **Ops-amplified severity escalations.** Three L7 findings and one
   L4 finding rise on the combined criterion:
   - L7-01 (High → Critical)
   - L7-04 (Medium → High) — XFF clobber silent today; multi-tier
     deployments blind to risk.
   - L7-08 (Medium → High) — H2 RST_STREAM(CANCEL) absence makes us
     look like the attacker; no metric for upstream GOAWAY.
   - L4-05 (Medium → High) — silent-drop on Drv mode; `xdp_attach_mode`
     metric is misleading.

4. **L7-12 reframe.** `div-l7` rates Low (operability-only). From the
   ops standpoint this is Medium (six runbooks vs one). Bundle the
   `glitches` consolidation with L7-07's new `recv_frame_timeout`
   detector so we add one metric, not two, and rewrite the H2-abuse
   runbook section once.

5. **Bundling proposals (reduce PR count).**
   - **L4-01 + L4-07** — `div-l4` already proposed; agreed. One commit.
   - **L7-07 + L7-12** — one consolidated H2 abuse counter PR.
   - **L4-11 + OPS-07** — bpffs mount-check landing requires systemd
     unit hardening at the same time; both touch the same operator
     surface.
   - **L4-12 + OPS-04** — clean-detach-on-exit story; runbook plus
     loader plus accept-loop cancel arm.

6. **Defer-with-instrumentation pattern.** L4-02 (TCP state pruning)
   and L4-03 (SYN flood cap) are correctly deferred to Pillar 4b-3.
   The defer must come with **observability landed now**: a counter
   that ticks under the attack class so on-call can attribute. ADR
   should pin: "if `xdp_conntrack_evicted_recent_total` rises, the
   deferred-pruning consequence is biting; advance Pillar 4b-3."

7. **L7-13 ADR is the unblocker for any future host-based routing.**
   Without an explicit `audit/decisions/path-normalisation.md`,
   anyone proposing path/host routing will be blocked re-litigating
   the question. Write the ADR now; cost is sub-day.

8. **L7-15 + OPS-09 contract gate.** Land `cargo defaults-check`
   that asserts every const in `lb-config` defaults appears verbatim
   in `config/default.toml`, `RUNBOOK.md`, and `CONFIG.md`. Single
   gate, three drift catchers. Closes the doc-as-code gap that
   round-7 left half-built.

---

## Self-reference

Cross-cut findings under my (`div-ops`) bundle that this review
explicitly leans on:

- **OPS-06** (tracing callsites missing) — amplifier for L7-01, L7-08.
- **OPS-09** (doc-lint too narrow) — amplifier for L4-10.
- **OPS-04** (accept-loop relies on `JoinHandle::abort`) — bundles
  with L4-12.
- **OPS-07** (systemd unit modern hardening) — bundles with L4-11.
- **OPS-03** (`shutdown_drain_seconds` missing) — independent of L4/L7
  findings but relevant to the "every fix has a counter" pattern this
  review advances.

No L7/L4 finding triggers a CHALLENGE from the ops standpoint; the
defects are real and the recommendations are sound. The amplifier
findings are about *whether the fix is observable*, not about whether
the fix is correct.
