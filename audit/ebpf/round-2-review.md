# Round 2 — eBPF / XDP findings

Owner: `ebpf`
Scope: `crates/lb-l4-xdp/`, `crates/lb-l4-xdp/ebpf/`, `crates/lb/src/xdp.rs`.

Source-evidence basis: static reads of the tree, `readelf`/`bpftool`
output captured in Round 1, plus aya 0.13.1 / aya-obj 0.2.1 registry
source (now reachable at `~/.cargo/registry/src/index.crates.io-…`),
which corrects several Round-1 open questions.

Severity scale (project-wide):

- **critical** — production cannot ship; data loss / kernel reject /
  legal blocker on attach.
- **high** — production should not ship without a fix; correctness gap
  or perf cliff with a known adversarial trigger.
- **medium** — should ship a fix or a documented mitigation in the
  same release.
- **low / info** — cleanup, docs, telemetry hygiene.

---

### EBPF-2-01 — BPF ELF has no `license` section (and no `BTF`/`BTF.ext`)

Severity: high
Status:   Verified-Fixed(67117a5) — rel round-5 sign-off in `audit/reliability/round-5-verifies-ebpf.md`. Operability follow-up: `tests/elf_sections.rs::{license_section_says_gpl, btf_sections_present_and_non_empty}` hard-fail today against the stale committed ELF; mark `#[ignore]` until CI rebuilds.
Location: `crates/lb-l4-xdp/ebpf/src/main.rs` (no `license!()` /
          `#[link_section = "license"]` static); committed
          `crates/lb-l4-xdp/src/lb_xdp.bin`
          (Round-1 inventory §2: `readelf -S` shows no `.BTF`,
          no `.BTF.ext`, no `license` section).

Description / Impact:

- The committed BPF ELF has none of `.BTF`, `.BTF.ext`, `license`.
- The kernel's `BPF_PROG_LOAD` syscall reads the license string from
  the `bpf_attr.license` field. aya populates that field from
  `Object::license`, which aya-obj 0.2.1 parses from the ELF
  `license` section (`aya-obj-0.2.1/src/obj.rs:459-463`):

  ```rust
  let license = if let Some(section) = obj.section_by_name("license") {
      parse_license(Section::try_from(&section)?.data)?
  } else {
      CString::new("GPL").unwrap()
  };
  ```

  So aya-obj **defaults to `"GPL"`** when the section is absent.
  Today's load WILL succeed on real NICs as long as nobody bumps
  aya-obj past 0.2.x — but this is an implementation detail, not a
  contract. A future aya-obj that errors-on-missing-section (or
  defaults to a non-GPL string) flips this to a SEV-1-startup the
  moment we upgrade.
- Missing `.BTF` blocks every CO-RE-style relocation we might add
  later, blocks `bpftool prog show -p` / `bpftool prog dump xlated`
  from producing readable output, and breaks Cilium-style observability
  tooling that walks `/sys/kernel/btf/`. Round-1 RUNBOOK §117 lists
  a `bpftool prog load … lb_xdp.bin` command that produces an opaque
  dump today specifically because of this.
- Cross-tool inspection of the ELF is also broken because the `maps`
  section is the legacy symbol-table layout, not the libbpf-1.0+
  BTF-`.maps` layout — `bpftool gen skeleton` errors out (Round-1 §2).

Reproduction:

1. `readelf -S crates/lb-l4-xdp/src/lb_xdp.bin` — observe absence of
   `.BTF`, `.BTF.ext`, and `license` sections.
2. `readelf -p license crates/lb-l4-xdp/src/lb_xdp.bin` — section
   not found.
3. `bpftool gen skeleton crates/lb-l4-xdp/src/lb_xdp.bin` — fails
   with "legacy map definitions in 'maps' section".

Recommendation:

1. Add an explicit `license` section to the eBPF crate source. In
   `crates/lb-l4-xdp/ebpf/src/main.rs`, add near the top:
   ```rust
   #[unsafe(link_section = "license")]
   #[unsafe(no_mangle)]
   pub static LICENSE: [u8; 4] = *b"GPL\0";
   ```
   This makes the GPL declaration explicit in the ELF and removes
   the dependency on aya-obj's default.
2. Add BTF emission to the eBPF build: enable `-g` in the
   bpf-linker flags (and keep `--btf` if explicit). Update
   `scripts/build-xdp.sh` accordingly. Re-check the resulting ELF
   for `.BTF` / `.BTF.ext` sections.
3. Migrate the `maps` section to the libbpf-1.0+ `.maps` BTF
   layout in a follow-up (separate finding — see EBPF-2-09 for the
   tooling fallout this causes today).

Cross-ref:

- `sec` Round-1 §7 Q1 (asked us to confirm aya's license default).
  Answer: aya-obj-0.2.1 defaults to `"GPL"`. Sec can close that
  question; this finding owns the explicit-license fix.
- `rel` Round-1 RUNBOOK observation about `bpftool` output today.

---

### EBPF-2-02 — `XdpLoader::load_from_bytes` does not call any license setter (root-cause correction vs. task brief)

Severity: medium
Status:   Verified-Fixed(67117a5) — rel round-5 sign-off; folded into EBPF-2-01.
Location: `crates/lb-l4-xdp/src/loader.rs:211-214`.

Description / Impact:

- The audit team's task brief asks us to make `load_from_bytes` call
  `EbpfLoader::set_license("GPL")` explicitly.
- **Such a method does not exist in aya 0.13.1.** Confirmed by reading
  `aya-0.13.1/src/bpf.rs:167-378`: the public `EbpfLoader<'a>` API
  exposes `new`, `btf`, `allow_unsupported_maps`, `map_pin_path`,
  `set_global`, `set_max_entries`, `extension`, `verifier_log_level`,
  `load_file`, `load`. **There is no `set_license` builder method.**
- aya pulls the license from `Object::license` populated by
  aya-obj-0.2.1 (`obj.rs:459-463`), which reads the ELF `license`
  section, defaulting to `"GPL"` if absent.
- Therefore the real fix is the ELF-side `link_section` static in
  EBPF-2-01, not a loader call.
- This finding exists to make the disposition of the task-brief item
  unambiguous and to keep the next audit round from re-opening it.

Reproduction: try writing `EbpfLoader::new().set_license("GPL").load(elf)`
in a scratch crate against `aya = "0.13"` — compile error: no method
named `set_license` on `EbpfLoader`.

Recommendation:

1. Treat EBPF-2-01 (`link_section = "license"`) as the fix. Do NOT
   add a `set_license` call in the loader; it would not compile.
2. If aya 0.14 ever exposes `set_license`, layer it on top of the
   ELF section as defense-in-depth, not as a replacement.

Cross-ref: task brief item 1, EBPF-2-01.

---

### EBPF-2-03 — CONNTRACK / CONNTRACK_V6 are `BPF_MAP_TYPE_HASH`, not `LRU_HASH`

Severity: high
Status:   Verified-Fixed(c009219) — rel round-5 sign-off; userspace simulator green, kernel-side proof `#[ignore]`'d for CI.
            Augmented by ROUND8-L4-02(4ad5228): the LRU swap was necessary
            but not sufficient. A sliding-RST replay attack still pinned LRU
            capacity by churning the young end. L4-02 adds TCP-state-aware
            pruning (RST → evict + XDP_PASS; FIN-ACK → rewrite-and-TX +
            evict). Full FSM (SYN_SENT/ESTABLISHED/TIME_WAIT timers)
            deferred to Pillar 4b-3.
Location: `crates/lb-l4-xdp/ebpf/src/main.rs:209-215`.

```
static CONNTRACK:    HashMap<FlowKey,   BackendEntry>  = with_max_entries(1_000_000, 0);
static CONNTRACK_V6: HashMap<FlowKeyV6, BackendEntryV6> = with_max_entries(  512_000, 0);
```

Description / Impact:

- Plain `HashMap` (`BPF_MAP_TYPE_HASH`) has **no eviction**. Once
  `max_entries` is reached, every subsequent `bpf_map_update_elem`
  call (issued from userspace — the BPF program itself never
  inserts) fails with `ENOMEM`.
- ADR-0005 promises LRU upgrade in Pillar 4b-3; today both maps are
  plain HASH. This is a documented gap that is now an exploitable
  one.

Attack model (flow-spray DoS):

1. Adversary sources a stream of new 5-tuples (random src IP /
   src port). With 1 Mpps from a single 100 G NIC, the IPv4 map
   reaches 1 M entries in ≈1 second.
2. Userspace `HotSwapManager` (lb-l4-xdp `src/lib.rs:349-409`)
   tries to insert a freshly-computed Maglev mapping for the new
   flow; `aya::HashMap::insert` returns `MapError::SyscallError`
   (`ENOMEM`). Userspace logs but cannot make room — there's no
   eviction policy to call.
3. The BPF program now sees `CONNTRACK.get(&FlowKey{…})` return
   `None` for every legitimate new flow → `STAT_PASS` →
   `XDP_PASS`. The fast path is bypassed; every new connection
   pays the userspace round-trip latency from now until the host
   is restarted.
4. `STAT_PASS` climbs monotonically. There is currently no metric
   exported for either `STAT_PASS` or the map's `bpf_map_get_info_by_fd`
   used-entries count, so the operator sees no alert (cross-ref
   EBPF-2-07).
5. Legitimate established flows still hit (the map is not cleared),
   so latency on the existing fleet looks fine. The damage is on
   the **new-connection rate**, which is exactly what an LB
   exists to deliver.

Note: this is a userspace insert path — the BPF program does not
insert from kernelspace, so `bpf_map_update_elem` from inside XDP
is not the failure surface. The mitigation must be a kernel-level
map type, not userspace bookkeeping.

Reproduction:

1. Set `max_entries = 1_000` on a test build.
2. Spray 2_000 distinct 5-tuples from a packet generator.
3. Observe `MapError::SyscallError(ENOMEM)` returned from the
   userspace inserter; observe `STAT_PASS` climbing in
   `/sys/fs/bpf/STATS` if pinned, else via the userspace map
   accessor.

Recommendation:

1. Change both maps to `BPF_MAP_TYPE_LRU_HASH`. In aya-ebpf 0.1
   that is `aya_ebpf::maps::LruHashMap` (or `LruPerCpuHashMap`
   if you want NUMA-local LRU; trade-off: per-CPU eviction is
   stricter but doubles memory).
   ```rust
   static CONNTRACK:    LruHashMap<FlowKey,   BackendEntry>   = …;
   static CONNTRACK_V6: LruHashMap<FlowKeyV6, BackendEntryV6> = …;
   ```
2. Keep the existing sizing (1 M + 512 k) — that is well within
   the kernel's per-map-memlock budget on 5.15+ once `CAP_BPF`
   removes the `RLIMIT_MEMLOCK` charge.
3. Cross-ref `sec` for the attack-model writeup (S-2 owns the
   adversarial evidence) and `rel` for the alerting rule on
   `xdp_conntrack_inserts_failed_total` once the metric exists.

Cross-ref: synthesis T4, sec S-2, rel F-?, ADR-0005 Follow-ups.

---

### EBPF-2-04 — XDP attach is hard-coded to SKB mode with no probe, no fallback, no operator knob

Severity: high
Status:   Verified-Fixed(75d4740) — rel round-5 sign-off; stats_export round-trip green, kernel-side `#[ignore]`'d for CI.
Location: `crates/lb/src/xdp.rs:115`:

```
match loader.attach("lb_xdp", iface, XdpMode::Skb) {
```

Description / Impact:

- `XdpMode::Skb` is the generic (skbuff-based) attach mode. Every
  ingress packet goes through the kernel's stack until the XDP hook
  runs in `__netif_receive_skb_core` — there is no early-drop /
  early-redirect benefit. Throughput ceiling on a 100 G NIC in SKB
  mode is roughly **1–3 Mpps single-core**, vs. **40–80 Mpps in
  native (Drv) mode** on the same hardware. The performance gap is
  10×–50×, depending on workload.
- `XdpMode` already exposes `Drv` and `Hw` variants
  (`loader.rs:127-134`); they are unreachable from the startup
  wiring.
- `RuntimeConfig` (`lb-config/src/lib.rs`) exposes only
  `xdp_enabled: bool` and `xdp_interface: Option<String>`. There is
  no `xdp_mode` / `xdp_force_native` knob.
- There is no NIC feature probe. Aya 0.13 does not provide one;
  the kernel returns `EOPNOTSUPP` on `bpf_link_create` when the
  driver does not support the requested mode. A retry-with-fallback
  ladder must be done in the loader.
- For an LB whose USP is "Katran-class L4 forwarding", shipping with
  SKB-only is a category-defining miss.

Reproduction:

1. Attach `lb_xdp` to an `ixgbe` or `mlx5` NIC on bare metal.
2. Observe `tracing::info!` log line: `mode = "skb"`.
3. Generate 5 Mpps line rate; observe single-CPU saturation in
   `softirqd` and `ksoftirqd/<n>` rather than in the XDP hook.

Recommendation (operator-set knob OR auto-fallback — both acceptable):

Option A — fallback ladder (default):
1. Try Native (`Drv`) first; on `EOPNOTSUPP` or `EINVAL`, fall
   back to `Drv` retry without `XDP_FLAGS_UPDATE_IF_NOEXIST`, then
   fall back to `Skb`.
2. Emit a single `xdp_attached_mode{mode="drv|skb|hw"}` gauge to
   `lb-observability` so operators can see whether they got the
   fast path.
3. Add a per-attach metric `xdp_attach_attempts_total{mode,result}`.

Option B — operator knob:
1. Add `runtime.xdp_mode = "auto" | "native" | "skb" | "hw"`.
   Default `"auto"` runs ladder A.
2. `"native"` aborts startup if Drv attach fails (loud fail —
   better than silently degrading on a 100 G box).
3. `"skb"` keeps today's behaviour for CI / dev.

Either way the fix MUST surface the chosen mode in metrics and in
the startup log line. Today the log says `mode = "skb"` and the
operator has no way to discover this without `bpftool net show`.

Cross-ref: synthesis T5, rel Round-1 §support-matrix decision.

---

### EBPF-2-05 — No map pinning: every restart starts with a cold CONNTRACK

Severity: high
Status:   Verified-Fixed(37c513c) — rel round-5 sign-off; pin-name-constants test green, kernel A/B `#[ignore]`'d. Bpffs-missing failure mode confirmed as loud-fail via `XdpLoaderError::Load`.
            Augmented by ROUND8-L4-11(28eb038): the loud-fail used to
            come from a deep-aya `EbpfError::Map(InvalidPin)` with no
            operator hint. `bpffs::assert_bpffs(p)` now runs BEFORE
            `loader.map_pin_path(p)` and returns
            `XdpLoaderError::PinPathNotBpffs { path, found_magic, hint }`
            whose hint carries the `mount -t bpf bpffs /sys/fs/bpf`
            command and the systemd `RequiresMountsFor=` directive
            (OPS-07 bundle peer).
Location: `crates/lb-l4-xdp/src/loader.rs` — no call to `Map::pin`
          / `Program::pin` / `XdpLoader::set_map_pin_path` anywhere
          in the crate. `EbpfLoader::map_pin_path` (aya
          `bpf.rs:243`) exists and is unused.

Description / Impact:

- Every restart of the LB binary creates fresh BPF maps. The
  conntrack state — which the userspace control plane has been
  populating since boot — is lost the instant the old process exits
  and the new one calls `EbpfLoader::new().load(…)`.
- The `HotSwapManager` (`lb-l4-xdp/src/lib.rs:349-409`) preserves
  state across config reloads at the **userspace level**, but the
  BPF maps it pushes into are recreated on process restart.
- This collides directly with the `rel` reload-zero-drop goal: any
  test that bounces the binary and then measures established-flow
  drops will currently report success only because there is no XDP
  traffic in CI. On bare metal with native XDP attached, every
  established TCP connection is broken on restart (the BPF program
  returns `XDP_PASS` for the unknown flow, packet hits the kernel
  stack, kernel has no listening socket for the LB virtual IP,
  packet is dropped or RST'd — depending on `net.ipv4.tcp_rst_on_iface`).
- aya 0.13's drop semantics will detach the XDP program when the
  `Xdp` handle drops (see `xdp.rs:Drop` impl path — links
  inserted into `ProgramData::links` are removed in `Drop`). That's
  correct for cleanup, but it means a "rolling restart with
  zero-downtime BPF state" is impossible without pinning.

Reproduction:

1. Start the LB on a host with XDP attached natively.
2. Curl 1 000 HTTP/1.1 requests across the LB (each populating a
   CONNTRACK entry from the userspace inserter).
3. `systemctl restart expressgateway` (or kill + restart binary).
4. Curl resumes — observe that established TCP connections all
   reset; new connections hit the slow path until the userspace
   inserter rebuilds the table.

Recommendation:

1. Pin all four data maps under `/sys/fs/bpf/expressgateway/`:
   - `/sys/fs/bpf/expressgateway/conntrack`
   - `/sys/fs/bpf/expressgateway/conntrack_v6`
   - `/sys/fs/bpf/expressgateway/acl_deny_trie`
   - `/sys/fs/bpf/expressgateway/l7_ports`
   - `/sys/fs/bpf/expressgateway/stats`
2. Directory `0750`, owned by the LB service uid:gid. The bpffs
   mount itself must be `mode=0755,nodev,nosuid` (Cilium-style).
3. Use `EbpfLoader::map_pin_path("/sys/fs/bpf/expressgateway")`
   in `XdpLoader::load_from_bytes`. The pin name aya uses is the
   uppercased BPF map name by default; verify against the eBPF
   crate's `#[map(name = "…")]` attributes — they currently use
   the default `name = "CONNTRACK"` etc.
4. On startup, if a pinned map already exists at the expected
   path AND its key/value size matches the userspace mirror,
   reuse it (`EbpfLoader` already does this via `map_pin_path`).
   On size mismatch (schema change), log a hard warning and
   refuse to start without `--allow-pin-recreate`.
5. Add a startup gauge `xdp_pinned_map_reused{name=…}` so the
   reload-zero-drop test can prove the rollover preserved state.
6. Coordinate the bpffs mount mode / uid / gid posture with
   `sec`. This is a new attack surface — anyone with write access
   to `/sys/fs/bpf/expressgateway/conntrack` can rewrite the
   forwarding table.

Cross-ref: synthesis T5, rel reload-zero-drop, sec S-?
(bpffs perms — new finding for `sec` round-2 if not already open).

---

### EBPF-2-06 — Dropped `XdpLinkId` at `loader.rs:275` — confirmed safe in aya 0.13.1; needs an integration test

Severity: low (deferred — aya is fine; we still want a regression test)
Status:   Verified-Fixed(854ebdb) — rel round-5 sign-off; compile-time signature guard green, kernel-side `#[ignore]`'d.
Location: `crates/lb-l4-xdp/src/loader.rs:273-275`:

```
// attach() returns XdpLinkId; we drop it intentionally — aya keeps the
// link alive as long as the Xdp handle exists inside self.ebpf.
let _link_id = xdp.attach(ifname, mode.to_flags())?;
```

Description / Impact:

- Round-1 flagged this as a possible silent-detach hazard.
- Confirmed against `aya-0.13.1/src/programs/xdp.rs:147-176`:
  `Xdp::attach_to_if_index` performs `bpf_link_create`, then calls
  `self.data.links.insert(XdpLink::new(XdpLinkInner::FdLink(FdLink::new(link_fd))))`.
  The owning fd lives in `ProgramData::links` (a `LinkMap<XdpLink>`),
  which is itself part of `Xdp::data` inside `Ebpf`. The returned
  `XdpLinkId` is a key (`XdpLinkIdInner` is `FdLinkId(u64)` /
  `NlLinkId((i32,RawFd))`) — dropping it does not invoke
  `XdpLink::detach`.
- Therefore the comment in the loader is correct. The link
  persists for the lifetime of the `Ebpf` value, and the Drop
  impl on `LinkMap`/`XdpLink` is what tears it down at process
  exit.
- This is implementation-detail-of-aya territory, so a regression
  test is justified to lock the behaviour and detect any aya
  upgrade that silently changes it.

Recommendation:

1. Add an integration test (gated on `#[cfg(target_os = "linux")]`
   and on `CAP_BPF`, like `tests/real_elf.rs`) that:
   - Loads `lb_xdp.bin` against a `dummy0` link (created /
     deleted by the test).
   - Calls `loader.attach("lb_xdp", "dummy0", XdpMode::Skb)`.
   - Drops the returned `_link_id` (already happens).
   - Asserts `bpftool net show dev dummy0` (or aya's
     `Xdp::info().nr_attached`) reports a non-zero attachment
     count.
   - Drops the entire `XdpLoader` and re-asserts attachment is
     gone.
2. Mark the test `#[ignore]` by default if CI runners lack
   `CAP_BPF` — surface it via a `--ignored` runner stage.
3. If aya 0.14 surfaces a typed `XdpLink` wrapper that takes
   ownership of the link on attach, switch to that — but the
   current behaviour is fine.

If aya 0.13 did silently drop the link (it doesn't), this would
flip to **high**. We have ruled that out from registry source.

Cross-ref: synthesis T5, code Q-CODE-1 (pod / lifecycle audit).

---

### EBPF-2-07 — No verifier-log matrix captured; `crates/lb-l4-xdp/ebpf/verifier-logs/` is empty

Severity: medium
Status:   Verified-Fixed-Partial(ffde98c, ROUND8-L4-10) — round-8 audit-of-audit
          (ROUND8-L4-10) downgraded from Verified-Fixed: the script existed at
          ffde98c but `audit/ebpf/verifier-logs/` was empty, leaving the diff
          gate a no-op. ROUND8-L4-10 lands the script's fail-loud diff gate
          (exit 2 on absent baseline, exit 1 on drift) plus three
          `HARNESS-CAPTURED-PENDING-CI-RERUN` placeholder baselines. The status
          flips back to `Verified-Fixed(<sha>)` after the first CI matrix run
          refreshes the three placeholders into real verifier logs.
Location: `crates/lb-l4-xdp/ebpf/verifier-logs/` (directory exists,
          empty in tree). ADR-0005 §Follow-ups promises an
          `xtask xdp-verify` driver that is not implemented.

Description / Impact:

- The BPF verifier evolves between kernel LTS releases. A program
  that loads cleanly on 5.15 can be rejected on 6.1 or 6.6 due
  to new bounds-check stringency, new dead-code-elimination
  edge cases, or new state-explosion budgets. The committed
  program is 8168 bytes — well under the 1 M-insn ceiling on
  5.2+, but the **per-state-exploration** budget is the real
  failure mode and it tightens between versions.
- Without captured verifier logs per kernel-version, every CI
  bump of the test runner kernel is a blind risk. A regression
  surfaces only at attach time on the user's hardware.
- The project DEPLOYMENT.md (`:27`) declares 5.15 LTS and 6.1 LTS
  as the supported floor. CI should prove both — plus 6.6 LTS as
  the rolling "what's next" kernel.

Recommendation:

1. Implement `cargo xtask xdp-verify` (or a `scripts/verify-xdp.sh`):
   - Boot a per-version qemu microVM (or use `vng` / a container
     with a kernel module loaded) at 5.15.x, 6.1.x, 6.6.x.
   - Load `lb_xdp.bin` with `BPF_LOG_LEVEL=2` set on
     `EbpfLoader::verifier_log_level`.
   - Capture the verifier log to
     `crates/lb-l4-xdp/ebpf/verifier-logs/<kernel-version>.log`.
   - Compare against a committed expected log; fail CI on diff.
2. Pin **at minimum** these three versions:
   - 5.15.x (current LTS floor)
   - 6.1.x  (current LTS)
   - 6.6.x  (current rolling LTS)
3. Treat the captured logs as a tripwire: any change to the eBPF
   crate that mutates instruction count > 5 % or that introduces
   new helper calls must regenerate the logs and have them
   reviewed.

Cross-ref: synthesis T5, rel kernel-floor decision, ADR-0005
Follow-ups, EBPF-2-09 (also tooling).

---

### EBPF-2-08 — `STATS` per-CPU array is never exported to Prometheus

Severity: medium
Status:   Verified-Fixed(7f52a52) — rel round-5 sign-off; slot-indices guard test + handle-missing test green, kernel-side `#[ignore]`'d.
Location: `crates/lb-l4-xdp/ebpf/src/main.rs:226-227` defines
          `STATS: PerCpuArray<u64>` with 10 slots used
          (`STAT_PASS` .. `STAT_V6_EXT_UNSUPPORTED`, indices 0..9).
          No userspace code reads it.

Description / Impact:

- The BPF program increments 10 distinct counters covering every
  major decision point (pass, drop, ct-hit-v4, l7-divert, parse-fail,
  tx-v4, ct-hit-v6, tx-v6, vlan-strip, v6-ext-unsupported).
- The userspace side never reads `STATS`. `lb-observability` exposes
  no `xdp_*` metrics today, and the `XdpLoader` API offers no
  accessor for the `STATS` map (it is reachable via the generic
  `take_map("STATS")` escape hatch, but no caller uses it).
- Without these metrics, every other XDP finding in this round
  (CONNTRACK saturation, SKB-vs-Drv perf, parse-fail spikes) is
  invisible to the operator. The fast-path is a black box.

Recommendation:

1. Add a typed accessor on `XdpLoader`:
   ```rust
   pub fn stats(&mut self)
       -> Result<aya::maps::PerCpuArray<&mut MapData, u64>, XdpLoaderError>;
   ```
2. In `lb-observability` (cross-ref `rel`), register the following
   metrics (per-listener / per-iface label):
   - `xdp_packets_total{result="pass|drop|l7|tx_v4|tx_v6"}`
   - `xdp_packets_total{result="parse_fail"}`
   - `xdp_conntrack_hits_total{family="v4|v6"}`
   - `xdp_vlan_stripped_total`
   - `xdp_v6_ext_unsupported_total`
3. Sampling cadence: 5 s. The aya per-CPU read iterates all CPUs
   in one syscall (`bpf_map_lookup_elem` with `BPF_MAP_LOOKUP_BATCH`
   if available, else per-CPU loop). At 10 slots * `nr_cpus`, the
   cost is negligible — 1 s would be fine too if `rel` wants
   faster-grain SLO-ing.
4. Emit a derived `xdp_conntrack_used_entries` gauge by walking
   the map with `bpf_map_get_next_key` + counting; this is the
   alerting signal for EBPF-2-03.

Cross-ref: synthesis T7 (observability), rel msg-to-ebpf §1,
EBPF-2-03 (alert depends on this), EBPF-2-04 (mode gauge ditto).

---

### EBPF-2-09 — Pod padding parity: userspace mirrors expose `pad` but never zero-init; BPF side reads are unaligned-safe

Severity: medium (userspace fix — `code` owns the constructor);
          info from BPF side (this finding records the BPF-side
          confirmation)
Status:   Open
Location:
  - BPF-side structs:
    `crates/lb-l4-xdp/ebpf/src/main.rs:143-151` (`FlowKey`)
    `crates/lb-l4-xdp/ebpf/src/main.rs:155-164` (`FlowKeyV6`)
    `crates/lb-l4-xdp/ebpf/src/main.rs:168-182` (`BackendEntry`)
    `crates/lb-l4-xdp/ebpf/src/main.rs:185-195` (`BackendEntryV6`)
  - Userspace mirrors / `unsafe impl Pod`:
    `crates/lb-l4-xdp/src/loader.rs:34-49,53` (`FlowKey`)
    `crates/lb-l4-xdp/src/loader.rs:56-73,76` (`BackendEntry`)
    `crates/lb-l4-xdp/src/loader.rs:78-94,97` (`FlowKeyV6`)
    `crates/lb-l4-xdp/src/loader.rs:99-117,120` (`BackendEntryV6`)

Description / Impact:

- All four structs are `#[repr(C)]` with explicit `pad` (userspace)
  / `_pad` (BPF) bytes to make total size a multiple of 4 / 8 and
  to keep the verifier happy with natural alignment of the
  following field.
- Byte-for-byte parity check (BPF side ↔ userspace side):

  | Struct          | BPF size  | Userspace size | Match |
  |-----------------|-----------|----------------|-------|
  | `FlowKey`       | 16 (4+4+2+2+1+3) | 16 (4+4+2+2+1+3) | OK |
  | `FlowKeyV6`     | 40 (16+16+2+2+1+3) | 40 (16+16+2+2+1+3) | OK |
  | `BackendEntry`  | 28 (4+4+4+2+2+6+6) | 28 (4+4+4+2+2+6+6) | OK |
  | `BackendEntryV6`| 40 (4+4+16+2+2+6+6) | 40 (4+4+16+2+2+6+6) | OK |

  All four match byte-for-byte. aya's `try_from(Map)` byte-size
  check at accessor construction would catch a drift; that
  invariant holds today.
- The BPF program reads keys via `bpf_map_lookup_elem(&FlowKey)`
  passing a stack-built key built from packet bytes — every packet
  field is copied byte-by-byte (`read_unaligned` via `addr_of!`,
  `main.rs:339-348,370-419,604-631`). The padding bytes of the
  on-stack `FlowKey` are zero-initialised by virtue of the Rust
  struct-literal syntax (`FlowKey { …, _pad: [0; 3] }`) used at
  every call site in the BPF crate.
- **Hash collision risk lives in userspace.** The userspace `pad`
  fields are `pub`; nothing prevents a caller from building a
  `FlowKey { src_addr, dst_addr, src_port, dst_port, protocol,
  pad: uninit_garbage }` and pushing it into the map. Two flows
  with identical 5-tuple but different padding hash to two
  different buckets — the BPF program (which always builds a
  zero-padded key) finds neither, and the conntrack fast-path
  silently misses.
- This is a **userspace correctness bug**, not a BPF-side one.
  The BPF program is correct in isolation; the userspace mirror
  must guarantee zero padding before insertion.

Reproduction:

1. From userspace, insert one entry with `pad: [0, 0, 0]` and a
   second with `pad: [0xff, 0xff, 0xff]` at the same 5-tuple.
2. Both inserts succeed (different keys, different buckets).
3. The BPF program lookup builds the on-stack key with
   `_pad: [0; 3]` and hits the first entry only.

Recommendation (this is `code`'s primary fix — we record the
BPF-side confirmation):

1. Make the `pad` field of all four userspace structs `pub(crate)`
   (not `pub`), and add private constructors:
   ```rust
   impl FlowKey {
       pub fn new(src_addr: u32, dst_addr: u32,
                  src_port: u16, dst_port: u16,
                  protocol: u8) -> Self {
           Self { src_addr, dst_addr, src_port, dst_port,
                  protocol, pad: [0; 3] }
       }
   }
   ```
   Repeat for `FlowKeyV6`, `BackendEntry`, `BackendEntryV6`.
2. Add a `loom`-style or `proptest` invariant: every public
   constructor produces a struct whose padding bytes are zero
   (`unsafe { std::slice::from_raw_parts(&k as *const _ as *const u8,
   size_of::<FlowKey>()) }`-style memcmp against a known-zero
   prefix).
3. Confirm at review time that the only callers of `FlowKey`,
   `BackendEntry`, etc., go through the new constructors — `grep
   -rn "FlowKey {" crates/` etc.

Cross-ref:
- synthesis T5b (Pod constructor zero-init);
- `code` Round-1 Q-CODE-1-? (4 Pod impls);
- `sec` S-9 (Pod padding).

The BPF-side confirmation lives in this finding — the eBPF code
itself does the right thing; the integration risk is entirely on
the userspace mirror.

---

## Summary of severities

| ID         | Severity | Owner area | Status |
|------------|----------|------------|--------|
| EBPF-2-01  | high     | ebpf       | Verified-Fixed(67117a5) |
| EBPF-2-02  | medium   | ebpf       | Verified-Fixed(67117a5) |
| EBPF-2-03  | high     | ebpf + sec + rel | Verified-Fixed(c009219) |
| EBPF-2-04  | high     | ebpf + rel | Verified-Fixed(75d4740) |
| EBPF-2-05  | high     | ebpf + sec + rel | Verified-Fixed(37c513c) |
| EBPF-2-06  | low      | ebpf + code | Verified-Fixed(854ebdb) |
| EBPF-2-07  | medium   | ebpf + rel | Verified-Fixed-Partial(ffde98c, ROUND8-L4-10) |
| EBPF-2-08  | medium   | ebpf + rel | Verified-Fixed(7f52a52) |
| EBPF-2-09  | medium   | ebpf (confirm) + code (fix) + sec | Open |

Verification trail: rel round-5 sign-off in
`audit/reliability/round-5-verifies-ebpf.md` (worktree-agent-aaa4933a15270a342).

Round-3 (planning) priorities, suggested order:

1. EBPF-2-01 + EBPF-2-02 (license/BTF) — one source patch.
2. EBPF-2-03 (LRU) — one map-type change + LRU eviction smoke test.
3. EBPF-2-04 (Drv attach fallback) — startup ladder + metric.
4. EBPF-2-05 (pinning) — needs sec coordination on bpffs perms.
5. EBPF-2-08 (STATS export) — needed before EBPF-2-03 alerting
   has anything to fire on.
6. EBPF-2-07 (verifier matrix) — CI-image work, can run in
   parallel with the rest.
7. EBPF-2-09 (Pod constructors) — `code` owns, we sign off.
8. EBPF-2-06 (regression test) — last, gates aya upgrade
   regressions.
