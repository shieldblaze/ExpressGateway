# Round 8 — cross-review by `div-l4`

Reviewer: `div-l4` (task #53), 2026-05-14. Stance: adversarial — push back where
the L7/OPS finding doesn't actually touch the L4/XDP fast path, escalate where
the L7/OPS author under-rated the L4 blast radius, and surface cross-cuts the
authors didn't tag.

Lens specific to this reviewer:

- Our XDP program emits **XDP_TX** (not XDP_REDIRECT) and **XDP_PASS** for any
  L7-port-matched flow (`ebpf/src/main.rs:443-450`, `:654-661`). QUIC is *not*
  redirected by XDP — UDP destined to a port not in `L7_PORTS` is conntracked
  and TX'd; UDP destined to a port in `L7_PORTS` is `XDP_PASS` to userspace.
- `STATS` is a 32-slot `PerCpuArray<u64>`; userspace iterates via
  `stats_export.rs` and produces fixed-label gauges
  (`xdp_attach_mode`, `STAT_*`) — closed-set labels.
- Conntrack is 1M (v4) / 512k (v6) pure LRU, no state machine.

---

## L7 findings

### ROUND8-L7-01 (premature 101 Switching Protocols) — CONFIRM (no L4 cross-cut)

The WS upgrade is a fully userspace L7 concern (hyper `OnUpgrade` + `tokio::spawn`
in `h1_proxy::handle_ws_upgrade`). XDP fast path never sees the 101 — by the time
hyper is parsing the upgrade headers we are at `XDP_PASS` (port matches
`L7_PORTS`). No L4 redirect of the WS bytes — the kernel TCP stack delivers them
to our L7 listener.

CONFIRM the severity (`high`) and the recommendation. The Pingora/Envoy
references the author cites are the right shape.

Tracing tie-in: if `tracing_propagation` were wired (see OPS-06), the premature
101 path would be much more debuggable post-mortem. That's an OPS-06 argument,
not a separate L4 argument.

### ROUND8-L7-02 (chunk-size hex `+` / leading whitespace) — CONFIRM (no L4 cross-cut)

H1 parser, fully userspace. XDP does not parse TCP payloads. CONFIRM `high`.

### ROUND8-L7-03 (empty header name) — CONFIRM (no L4 cross-cut)

H1 parser. No L4 cross-cut.

### ROUND8-L7-04 (XFF clobber) — CONFIRM (no L4 cross-cut)

Header-map manipulation in `h1_proxy`. No L4 cross-cut.

### ROUND8-L7-05 (header underscores) — CONFIRM (no L4 cross-cut)

H1 parser. No L4 cross-cut.

### ROUND8-L7-06 (keepalive_requests cap) — CONFIRM (no L4 cross-cut)

H1/H2 connection-lifecycle counter. The conntrack analogue is the per-flow
keep-alive (TCP), which is L4 — but that is fine because the kernel manages
TCP keepalive; XDP only observes packets.

Sub-note: ROUND8-L7-06's "TLS session staleness" concern is bounded by
`crates/lb-tls/`'s cert rotation lock — REL-2-03 closed correctly. No L4
exposure.

### ROUND8-L7-07 (no H2 frame-level recv timeout) — CONFIRM (no L4 cross-cut)

Per-stream tokio timer; hyper layer. No XDP interaction.

### ROUND8-L7-08 (H2 upstream timeout drops without RST_STREAM(CANCEL)) — CONFIRM (no L4 cross-cut)

H2 client pool. No L4 cross-cut.

### ROUND8-L7-09 (authority comma / control chars) — CONFIRM (no L4 cross-cut)

H1/H2/H3 header validation. The XDP fast path does not parse beyond L4 ports,
so authority is not on the L4 surface. CONFIRM medium.

### ROUND8-L7-10 (body over-read → connection non-reusable) — CONFIRM (no L4 cross-cut)

L7 conn-pool reuse policy. No L4 cross-cut.

### ROUND8-L7-11 (lb-h2 `decode_frame` ignores PADDED) — CONFIRM (no L4 cross-cut)

Frame decoder in lb-h2 (test-only today). No L4 cross-cut.

### ROUND8-L7-12 (no consolidated H2 glitches counter) — CONFIRM (no L4 cross-cut)

Operator UX for H2 abuse counters. The XDP STAT_* counters already have the
opposite shape (one PerCpuArray slot per signal, fixed at compile time) —
arguably worth referencing in the recommendation as the "we already have
a per-signal counter taxonomy elsewhere; collapse-or-keep consistent"
point. Not a defect, marginal cross-cut at most.

### ROUND8-L7-13 (path normalisation) — CONFIRM (no L4 cross-cut)

URI manipulation. No L4 cross-cut.

### ROUND8-L7-14 (proptest seeds missing) — CONFIRM (cross-cut to L4)

CROSS-CUT note: `lb-l4-xdp/src/sim.rs` userspace simulator has no
property-based test corpus for the parser-equivalent path. The four
parser-CVE shapes in this finding are H1-specific; the analogue for
L4 is malformed-Ethernet / oversize-IPv6-extension / IPv4-fragment
inputs. ROUND8-L4-08 (fragment handling) is the L4 sibling and is
already filed.

Recommend the L7 finding's recommendation #4 (CI gate `PROPTEST_CASES=200000`)
explicitly include `crates/lb-l4-xdp/tests/sim_parser.rs` if one exists, or
add a `tests/xdp_fuzz_corpus.rs` to mirror.

### ROUND8-L7-15 (edge-defaults parity gap) — CONFIRM (no L4 cross-cut)

`config/default.toml` exposes no XDP defaults today either, but XDP attach
is gated on systemd capability + binary feature flag, not config file — the
L7 author's recommendation to enumerate `[runtime]` defaults does not extend
to XDP. No additional L4 action.

---

## OPS findings

### ROUND8-OPS-01 (FD-passing claim aspirational) — CONFIRM, with L4 caveat

CROSS-CUT: the README's "Zero-downtime reload" claim has a *worse* shape on
the XDP side than on the TCP side. When XDP is attached, a binary swap that
inherits the listening FD still cannot inherit the BPF program — the new
binary would either:
  a) leave the old program attached (and serve userspace L7 from new binary,
     L4 fast path from old binary's BPF object — a recipe for ownership
     drift), or
  b) detach + re-attach, briefly leaving the NIC with no XDP program (drop
     window).

ROUND8-L4-12 (no `BPF_F_REPLACE` on attach) is the L4 partner to OPS-01.
The combination is "doc claims zero-downtime reload; reality is the L7 path
*might* survive an FD-passing impl while the L4 path *cannot* unless we use
`BPF_F_REPLACE` + pinned object pass-through".

Recommend OPS-01 recommendation #1 (strike FD-passing claim) explicitly note
that the XDP fast path requires its own handover protocol beyond FD passing.
Cite ROUND8-L4-12.

### ROUND8-OPS-02 (drain has no jitter) — CONFIRM (no L4 cross-cut)

XDP fast path is stateless w.r.t. drain — the BPF program does not see SIGTERM
or the cancel token. On drain we either keep `XDP_TX`ing to backends (which
continues to work) or detach the program (entire fleet of fast-path flows
falls back to kernel L4 routing). The drain jitter problem is purely an L7
concern.

### ROUND8-OPS-03 (`shutdown_drain_seconds` histogram missing) — CONFIRM (no L4 cross-cut)

L7 metric. No L4 cross-cut.

### ROUND8-OPS-04 (TCP accept loop no cancel arm) — CONFIRM (no L4 cross-cut)

Listener loop is L7. XDP attach loop is a one-shot at startup, not an accept
loop. No L4 cross-cut.

### ROUND8-OPS-05 (`LabelBudget::check` startup-only) — CONFIRM + L4 vouches

CROSS-CUT note: L4 emits **closed-set labels only** — `stats_export.rs`
produces `xdp_attach_mode{mode="drv|skb|hw|none"}` + per-`STAT_*` named
gauges. No open-set label is ever passed from L4 into the registry. So
the runtime-injection failure mode the OPS author is worried about
(case 1 in their finding: someone passes `client_addr.to_string()` as a
label) cannot originate from L4 today.

That said, the OPS author's case 2 (`route` label is `""` today; will
explode when route extraction lands) is the more concerning one and is
purely L7. CONFIRM medium.

### ROUND8-OPS-06 (tracing propagation library has zero callsites) — CONFIRM, no L4 cross-cut

XDP fast path has nowhere to put a span — packets don't carry W3C
`traceparent` at L4 — and any flow that hits L7 will see the trace context
in headers regardless of whether we XDP_TX'd it or PASS'd. No L4 work
needed.

Lesson-not-yet-paid-for note: the L7 author asks "would tracing help debug
premature 101?". Yes — once OPS-06 ships, L7-01's premature-101 path would
have a `lb.l7.h1.ws_upgrade` span with attribute `upstream_handshake=
failed` and we could pivot on it. L4 has no equivalent need.

### ROUND8-OPS-07 (systemd unit missing modern hardening) — ESCALATE-SEVERITY (CROSS-CUT to L4)

OPS-07 is filed `medium`. I want to push back: the missing
`RestrictAddressFamilies` + `CAP_BPF` interaction is more pointed than
the OPS author calls out:

- `CAP_BPF` is in the unit's `CapabilityBoundingSet` (correct).
- The unit does NOT enumerate `AF_NETLINK` in `RestrictAddressFamilies`
  (because the unit doesn't have that directive at all).
- aya's loader uses `AF_NETLINK` (via libbpf) to attach XDP. The OPS
  author's draft `RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
  AF_NETLINK` is right, but the recommendation says "the last only when
  XDP is enabled". XDP is *always* enabled when our binary is the
  full-feature build — the systemd unit cannot probe binary features.

Cross-cut to ROUND8-L4-11 (bpffs mount-type check). Both are pre-flight
checks. Recommend OPS-07 #1 (add `packaging/expressgateway.service`)
also ship the bpffs mount unit dependency:

```ini
[Unit]
RequiresMountsFor=/sys/fs/bpf
After=sys-fs-bpf.mount
```

Severity not raised to `high` — but the missing dep is a deploy-time
opaque-failure shape, and the unit-vs-runtime-behaviour gap shows up
on the XDP path first.

### ROUND8-OPS-08 (SBOM provenance) — CONFIRM (no L4 cross-cut)

Supply-chain, no L4 surface.

### ROUND8-OPS-09 (doc-lint scope too narrow) — CONFIRM + cross-cut

CROSS-CUT: the L4-specific docs (`crates/lb-l4-xdp/README.md`,
`audit/ebpf/round-2-review.md`) have their own drift risk — e.g.
EBPF-2-07 was marked Verified-Fixed but no verifier-log snapshots
committed (see ROUND8-L4-10). A Rust-binary doc-lint (OPS-09 #3)
would catch the cross-doc drift between `audit/ebpf/` register
entries and the actual source.

Specifically: ROUND8-L4-07 (`BackendEntry::flags` is dead) has the
identical shape as OPS-09's "doc claims feature X works, code does
not implement X". Mention this in any consolidated fix.

### ROUND8-OPS-10 (drain budget 10s too short for streaming) — CONFIRM (no L4 cross-cut)

XDP fast path is per-packet not per-connection; drain budget does not
apply to it. The flow is: SIGTERM → L7 stops accepting → in-flight
streams drain over 10s → XDP keeps `XDP_TX`ing for any conntrack hit
during that window. The 10s is purely an L7-stream-completion budget.

### ROUND8-OPS-11 (readiness_settle_ms below kubelet period) — CONFIRM (no L4 cross-cut)

`/readyz` is an L7 endpoint on the admin listener, totally separate from
the XDP fast path. CONFIRM medium.

### ROUND8-OPS-12 (container image lacks RO rootfs / labels / HEALTHCHECK) — CONFIRM, with L4 caveat

CROSS-CUT: the container needs `/sys/fs/bpf` accessible (read-write,
mounted, with `bpf` fstype) for XDP attach. The OPS author's "use
`--read-only --tmpfs /tmp`" recommendation is correct but
incomplete — the container also needs:

- `--privileged` *or* `--cap-add=NET_ADMIN --cap-add=BPF
  --cap-add=NET_RAW`
- A bind mount or volume for `/sys/fs/bpf` with the `bpf` fstype
- Kernel ≥5.8 in the host (CAP_BPF availability)

Recommend OPS-12 also document the BPF mount + capabilities for the
XDP-enabled path; reference ROUND8-L4-11.

---

## Decisions for team-lead

1. **OPS-01 is partial-truth on the L4 side.** The README claim doesn't only
   mis-describe FD-passing for TCP listeners; the XDP attach has a *different*
   handover shape (`BPF_F_REPLACE`) that the readme also doesn't address.
   Treat ROUND8-L4-12 + ROUND8-OPS-01 as a paired fix or call out the L4
   delta in OPS-01's recommendation.

2. **OPS-07 should call out the `RequiresMountsFor=/sys/fs/bpf` /
   `After=sys-fs-bpf.mount` unit dependency.** Currently in `medium`. I do
   NOT recommend raising severity but I do recommend the
   `packaging/expressgateway.service` template that OPS-07 #1 prescribes
   include this dependency explicitly — otherwise a clean-boot host without
   bpffs mounted gets an opaque attach error.

3. **OPS-12 must document the BPF capability/mount requirements** for the
   XDP-enabled image path. The current Dockerfile review only audits the
   distroless base — it doesn't audit the runtime contract for the
   XDP feature.

4. **L7-14 (proptest seeds) is the right shape; extend it to the L4 sim.**
   Add `crates/lb-l4-xdp/tests/sim_fuzz_corpus.rs` with malformed
   Ethernet / IPv4-frag / IPv6-extension shapes. Pair-fix with
   ROUND8-L4-08.

5. **OPS-09 (doc-lint) gate philosophy applies to `audit/ebpf/`.** The
   Round-2 EBPF register has the same drift shape as `RUNBOOK.md` — a
   Rust-binary doc-lint should ingest the audit register table and
   assert each `Verified-Fixed(<sha>)` claim has at least one diff in
   the referenced sha touching the file the row describes. This catches
   the EBPF-2-07 false-Verified pattern (ROUND8-L4-10).

6. **No L4 cross-cut for L7-01..L7-13 individual findings.** The XDP fast
   path is correctly out of band for L7 protocol-layer divergences. The
   genuine cross-cuts are at the OPS lifecycle / packaging / metrics
   boundary, not the L7 protocol boundary.

7. **No severity escalations.** L7's and OPS's gradings hold on cross-review.
   The closest call is OPS-01 (already `high`) given the L4 amplification,
   but it's already at the right tier.

8. **No CHALLENGE/ESCALATE on L7's `MISSED` tagging** (L7-01, L7-02, L7-04
   recommend MISSED-by-prior-audits). Phase C reconciler will decide; from
   the L4 angle these are correctly characterised.
