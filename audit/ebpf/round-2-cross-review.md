# Round 2 — eBPF cross-review

Owner: `ebpf` (XDP / BPF correctness + portability).

Inputs read for this cross-review:

- `audit/CROSS-REVIEW-SYNTHESIS-r1.md` (team-lead)
- `audit/security/round-1-inventory.md` + `round-1-cross-review.md`
- `audit/code/round-1-inventory.md` + `round-1-cross-review.md`
- `audit/reliability/round-1-inventory.md` + `round-1-cross-review.md` +
  `msg-to-ebpf.md`
- `audit/protocol/round-1-inventory.md` + `round-1-cross-review.md`
- `audit/ebpf/round-2-review.md` (own findings)

State at time of writing: **no other teammate has landed
`round-2-*.md` yet**. The synthesis carries the convergent Round-1
themes, but the per-teammate finding IDs (`S-…`, `Q-CODE-2-…`,
`F-…`, `P-…`) are still pre-Round-2. This cross-review therefore
takes a tri-layer form:

1. **AGREED** — Round-1 themes that match my Round-2 findings
   exactly. Recorded so the lead can stitch the dependency graph
   without waiting for me to re-confirm.
2. **DISPUTED** — Round-1 claims where my closer reading shifts
   the assessment.
3. **ESCALATE-SEVERITY / DE-ESCALATE** — places where the
   evidence I produced this round moves the needle on someone
   else's existing finding.
4. **OPEN — needs other teammate's Round 2** — items where I
   cannot finalize a verdict until `sec` / `code` / `rel` /
   `proto` post Round-2 IDs.

Once the four other Round-2 files land I will append a stitched
`per-ID` block at the bottom; this file is meant to be re-readable
incrementally.

---

## 1. AGREED

### A-1. CONNTRACK / CONNTRACK_V6 must become `LRU_HASH`
(Round-1 sec S-2 ↔ ebpf §2-maps ↔ rel adversarial pressure)

- My EBPF-2-03 carries the kernel-side fix.
- `sec` S-2 carries the adversarial flow-spray model and the
  attack-evidence harness.
- `rel` carries the per-map saturation gauge + alert; this depends
  on EBPF-2-08 (STATS export) landing first or alongside.

No semantic disagreement. The three teammates' findings dovetail.

### A-2. SKB-mode hard-coded is a perf cliff that needs fallback or operator knob
(Round-1 ebpf §4 ↔ rel "Katran-class" note in synthesis T5)

- My EBPF-2-04 is the canonical statement.
- `rel`'s expected metric `xdp_attached_mode{mode=…}` is recorded
  there.
- Code-side dependency: `lb-config::RuntimeConfig` needs a new
  `xdp_mode` field. `code` should sign off on the config-shape
  change (additive, default-`"auto"`).

### A-3. No map pinning ⇒ cold CONNTRACK on every restart
(Round-1 ebpf §5 ↔ rel reload-zero-drop, F-08 family)

- My EBPF-2-05 carries the fix.
- bpffs perm posture coordinates with `sec` (new finding territory
  — synthesis T5 lists `sec` as consulted, not authoring; I am
  asking `sec` to author a sibling finding on the bpffs mount mode
  + uid:gid + SELinux/AppArmor labels for `/sys/fs/bpf/expressgateway/`).
- `rel`'s reload-zero-drop test must include a "BPF map state
  survives a process bounce" assertion that exercises an actually
  pinned map — today the test cannot meaningfully cover this
  because no map is pinned. EBPF-2-05 is therefore an unblocker
  for `rel`.

### A-4. Verifier-log matrix is missing
(Round-1 ebpf §3 + §7 ↔ rel kernel-floor decision)

- My EBPF-2-07 carries the CI work.
- `rel` Round-1 calls out the kernel-floor doc (DEPLOYMENT.md:27)
  could be lowered to 5.10 LTS; my finding does not chase that
  delta — it accepts 5.15 / 6.1 / 6.6 as the three log targets.
  If `rel` lowers the floor in Round 2, the verifier matrix
  should add 5.10.x.

### A-5. STATS map is never exported
(Round-1 ebpf §2 ↔ rel observability gap T7 ↔ rel msg-to-ebpf §1)

- My EBPF-2-08 plus `rel` msg-to-ebpf item 1: 5 s cadence is the
  default I'm proposing. `rel` asked the question; this is my
  answer.

### A-6. `unsafe impl Pod` constructors do not zero `pad`
(Round-1 code Q-CODE-1-? ↔ sec S-9 ↔ ebpf §5)

- My EBPF-2-09 covers the BPF-side parity check (BPF builds keys
  with zeroed padding; layouts match byte-for-byte) and confirms
  the bug is **userspace-only**.
- `code` owns the constructor fix (this is task T5b in the
  synthesis).
- `sec` S-9 closes once the constructor is in place.

### A-7. Aya 0.13 keeps the XDP link in `self.ebpf` — Round-1 worry is closed
(Round-1 ebpf §5 open question to `code`)

- Confirmed from `~/.cargo/registry/src/index.crates.io-…/aya-0.13.1/src/programs/xdp.rs:147-176`.
- `code` does not need to chase this — see EBPF-2-06 for the
  regression test that pins the behaviour.

---

## 2. DISPUTED

### D-1. Task brief says "call `EbpfLoader::set_license("GPL")`" — that method does not exist in aya 0.13

- Audit task #1 in my Round-2 brief asks for an explicit
  `EbpfLoader::set_license("GPL")` at `loader.rs:212`.
- aya 0.13.1's `EbpfLoader` exposes `new`, `btf`,
  `allow_unsupported_maps`, `map_pin_path`, `set_global`,
  `set_max_entries`, `extension`, `verifier_log_level`,
  `load_file`, `load` — **no `set_license`**.
- The real fix is the ELF-side `link_section = "license"` static
  (EBPF-2-01). I have escalated this to the lead via EBPF-2-02.
- This is not a disagreement with another teammate; it is a
  correction to the task brief. Lead should accept the substitution.

### D-2. Synthesis lists `sec` as "consulted" on bpffs perms (T5) — I am asking `sec` to **author** a sibling finding

- The bpffs mount mode, uid:gid, SELinux/AppArmor label, and
  `noexec`/`nosuid`/`nodev` mount options are a security surface
  question, not a kernel-correctness one.
- My EBPF-2-05 recommends `0750` on the per-app dir and `0755` on
  the bpffs mount, but this is a default — `sec` must own the
  threat model (who reads CONNTRACK ⇒ flow visibility leak; who
  writes ⇒ traffic-redirect attack).
- Lead: please reassign T5/bpffs to `sec`-authoring with `ebpf`
  consulted.

---

## 3. ESCALATE / DE-ESCALATE SEVERITY

### E-1. ESCALATE — `sec` S-2 (CONNTRACK HASH) should match my EBPF-2-03 severity (high)

- Round-1 sec inventory lists "XDP map exhaustion" plausibility as
  "Medium" (sec §2.2 table).
- My Round-2 EBPF-2-03 escalates to **high** because:
  1. The attack rate-of-fire is concrete (1 Mpps → 1 s to fill).
  2. The damage is on the new-connection rate, which is the LB's
     core SLO.
  3. There is no alerting today (EBPF-2-08 gap).
- Recommendation: `sec` re-rates S-2 to **high** for Round 2 and
  the joint finding (sec + ebpf + rel) takes the higher rating.

### E-2. ESCALATE — synthesis T5 lumps "license + BTF + SKB + pin + Pod" as one bundle; they have **different severities**

- `license` ELF section missing (EBPF-2-01): **high** today,
  flips to **critical-startup** if aya-obj 0.2 ever changes its
  default. Documented one-line fix.
- `BTF` missing: **medium** (tooling, observability) — not a
  load-blocker.
- `SKB` hard-coded (EBPF-2-04): **high** (perf cliff).
- `Pin` missing (EBPF-2-05): **high** (correctness on restart).
- `XdpLinkId` dropped (EBPF-2-06): **low** (aya-confirmed safe).
- `Pod` constructors (EBPF-2-09): **medium** (correctness, but
  caller-controlled today — the userspace inserter is the only
  current caller and uses literal struct syntax that zero-inits).

The synthesis should not roll these up.

### E-3. DE-ESCALATE — `XdpLinkId` drop concern (EBPF-2-06)

- Round-1 cross-review (`ebpf` §5) put this on the watch list as
  potentially high.
- Confirmed safe against aya 0.13.1 source.
- Severity downgraded to **low** (regression-test only).

---

## 4. OPEN — pending other teammate's Round-2 files

The following will be revisited / finalized when the named files
land:

| Pending file                            | Items I need from it                          |
|-----------------------------------------|-----------------------------------------------|
| `audit/security/round-2-review.md`      | Final S-2 severity, S-9 closure on Pod, new bpffs-posture finding, CAP_SYS_ADMIN fallback decision (Round-1 §7 Q2 to `sec`). |
| `audit/code/round-2-review.md`          | Pod constructor fix ID, concurrent map-access decision (Round-1 §7 Q2 to `code`), aya upgrade gate. |
| `audit/reliability/round-2-review.md`   | XDP-mode metric label spec, kernel-floor decision (5.10 vs 5.15), reload-zero-drop test exercising XDP path, alert SLOs on STATS. |
| `audit/protocol/round-2-review.md`      | VLAN-strip-on-egress decision (Round-1 §7 Q1 to `proto`), IPv6 ext-header coverage stance, UDP/IPv6 zero-csum decision, first-packet latency confirmation. |

Once those files land I will append a per-ID verdict block:

```
### EBPF-2-03 ↔ SEC-2-??  — AGREED (severity:high)
### EBPF-2-09 ↔ CODE-2-?? — AGREED (severity:medium; code owns fix)
### EBPF-2-05 ↔ SEC-2-??  — AGREED (sec authors bpffs sibling finding)
### EBPF-2-08 ↔ REL-2-??  — AGREED (5s cadence; metric names per §A-5)
```

Until then, this file stands as my unilateral position and the
lead can stitch dependencies from it.

---

## 5. Concise position summary

- Two of my findings (EBPF-2-01, EBPF-2-02) clarify the
  license-section question that has been open since Round 1: aya
  0.13 has no `set_license` setter, the fix is an ELF-section
  static, and the default behaviour is currently safe but
  fragile.
- Three findings (EBPF-2-03, EBPF-2-04, EBPF-2-05) are the
  Katran-readiness gate: LRU conntrack, native-mode attach,
  pinned maps. None has any technical blocker; all three need
  cross-team coordination (sec for posture, rel for metrics,
  code for config-shape changes).
- Three findings (EBPF-2-06, EBPF-2-07, EBPF-2-08) are
  CI-and-observability hygiene. EBPF-2-08 is the unblocker for
  EBPF-2-03's alerting story.
- EBPF-2-09 records the BPF-side confirmation that the Pod
  parity bug is purely userspace; `code` owns the constructor
  fix.
- No new disagreement with another teammate is raised. The only
  dispute is with the audit-brief wording (D-1) and a scope
  reassignment request to `sec` (D-2).
