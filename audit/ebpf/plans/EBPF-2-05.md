# Plan for EBPF-2-05 — Map pinning under `/sys/fs/bpf/expressgateway/` (mode 0750, owned by LB uid)
Finding-ref:     EBPF-2-05 (high, Open) — sec authors sibling finding on bpffs posture (synthesis §D / round-2 cross-review D-2)
Files touched:
  - `crates/lb-l4-xdp/src/loader.rs`        (`EbpfLoader::map_pin_path(..)`, schema-check on reuse, `xdp_pinned_map_reused` gauge)
  - `crates/lb-l4-xdp/ebpf/src/main.rs`     (verify `#[map(name = "…")]` names are stable & explicit — today they use defaults)
  - `crates/lb/src/xdp.rs`                  (ensure parent dir exists with correct mode/uid/gid before load; bpffs mount check)
  - `crates/lb-l4-xdp/tests/pinning.rs`     (NEW — proof test)
  - `DEPLOYMENT.md`, `RUNBOOK.md`           (operator docs — coordinate with rel)
  - `manifest/expressgateway.service`       (systemd unit — `RuntimeDirectory=` won't help here; need `ExecStartPre=` to mount bpffs and chown)

Approach:

1. **Pinned-path layout**:
   ```
   /sys/fs/bpf/                        # kernel bpffs mount (mode 0755, root:root, nosuid, nodev, noexec)
   /sys/fs/bpf/expressgateway/         # per-app subdir (mode 0750, owned by LB uid:gid)
       conntrack                       # CONNTRACK
       conntrack_v6                    # CONNTRACK_V6
       acl_deny_trie                   # ACL_DENY_TRIE
       l7_ports                        # L7_PORTS
       stats                           # STATS
   ```
   `0750` (rwxr-x---) on the parent dir means: LB uid reads+writes,
   audit group reads, world denied. This is the minimum that lets
   `bpftool` (run as the audit group) inspect maps without granting
   write access. The `sec` sibling finding may tighten this to `0700`
   if the threat model says no audit group exists.

2. **Bpffs mount posture (coordinated with `sec`)**. Most distros
   auto-mount `/sys/fs/bpf` via systemd or `mount -t bpf`. The
   `expressgateway` systemd unit gets an `ExecStartPre=` chain:
   ```
   ExecStartPre=/usr/bin/mountpoint -q /sys/fs/bpf || /usr/bin/mount -t bpf bpffs /sys/fs/bpf -o nosuid,nodev,noexec,mode=0755
   ExecStartPre=/usr/bin/install -d -m 0750 -o expressgateway -g expressgateway /sys/fs/bpf/expressgateway
   ```
   The mount options (`nosuid,nodev,noexec`) prevent a malicious pin
   from being chmod+x'd or setuid'd; `sec` signs off on this set.

3. **Aya wiring**. In `XdpLoader::load_from_bytes` (currently at
   `loader.rs:211-214`), add:
   ```rust
   let pin_dir = Path::new("/sys/fs/bpf/expressgateway");
   let mut loader = EbpfLoader::new();
   loader.map_pin_path(pin_dir);          // aya bpf.rs:243
   // optional: loader.allow_unsupported_maps(false);
   let mut ebpf = loader.load(elf_bytes)?;
   ```
   Aya's behaviour with `map_pin_path` set:
   - If the pin file exists AND its kernel-side type/key/value
     sizes match the ELF declaration: the map is reused (the
     userspace-side CONNTRACK survives the restart).
   - If it does not exist: the map is created and pinned.
   - If sizes mismatch: aya returns
     `MapError::InvalidPin{…}`. We catch this, log a hard warning,
     and refuse to start unless `--allow-pin-recreate` (new CLI
     flag) is passed. On `--allow-pin-recreate`, we `unlink` the
     stale pin files first, then retry.

4. **Explicit map names**. The eBPF crate today uses default `#[map]`
   names (uppercased identifier). aya pins with the same string.
   Add explicit lowercase pin names to decouple Rust identifier
   churn from on-disk artefact churn:
   ```rust
   #[map(name = "conntrack")]
   static CONNTRACK: LruHashMap<FlowKey, BackendEntry> = …;
   ```
   Apply to all five maps (CONNTRACK, CONNTRACK_V6, ACL_DENY_TRIE,
   L7_PORTS, STATS). Document the names in DEPLOYMENT.md.

5. **Telemetry**. New gauge (via `stats_export.rs`, the file
   created in EBPF-2-04):
   ```
   xdp_pinned_map_reused{name="conntrack"} 1   # 1 if reused from prior process, 0 if freshly created
   xdp_pinned_map_reused{name="conntrack_v6"} 0
   …
   ```
   This is the assertion that `rel`'s reload-zero-drop test needs
   to prove (cross-ref REL-2-02, REL-2-12).

Proof:

- Test name: `lb-l4-xdp/tests/pinning.rs::pin_survives_process_bounce`
- Test scaffold (needs CAP_BPF + a tmpfs-backed bpffs mount; CI
  configures via `mount -t bpf bpffs /tmp/eg-bpffs-${PID}` and
  passes the path via env var — the loader honours `EG_BPFFS_ROOT`
  override for tests):
  1. Fork-and-exec subprocess A: load XDP, insert known
     `FlowKey { src=10.0.0.1:42, dst=10.0.0.2:80, proto=TCP, pad=0 }`
     with `BackendEntry { backend_ip=10.1.0.1, … }`, exit.
  2. Assert pin file exists at
     `${EG_BPFFS_ROOT}/expressgateway/conntrack`.
  3. Fork-and-exec subprocess B: load XDP (same ELF). Read back
     the FlowKey. Assert the BackendEntry is intact byte-for-byte.
  4. Assert `current_pinned_map_reused("conntrack") == true`.
  5. Schema-mismatch case: hand-craft a stale pin with a different
     key size; assert subprocess B fails to start with
     `XdpLoaderError::PinSchemaMismatch`. Re-run with
     `--allow-pin-recreate`; assert success and the gauge value
     flips to `0` (recreated, not reused).
- Marked `#[ignore]` by default.

Risk / blast radius:

- **Bpffs not mounted** on minimal hosts. The `ExecStartPre=` line
  in the systemd unit mounts it; document this for non-systemd
  deployments in RUNBOOK.md.
- **Permission denial** if the LB process drops privileges before
  load. Mitigation: the load happens before the privilege drop in
  `crates/lb/src/main.rs` (the binary needs CAP_BPF/CAP_NET_ADMIN
  to load anyway).
- **Stale pin from an older schema** can deadlock startup. The
  `--allow-pin-recreate` escape hatch addresses this; document the
  upgrade procedure in RUNBOOK.md.
- **Shared bpffs across multiple LB instances** (multi-tenant
  host). The per-app subdir `/sys/fs/bpf/expressgateway/`
  namespaces us; sec sibling finding owns the per-tenant
  separation question if it ever applies.

Cross-ref:
- SEC sibling finding (per `sec`'s round-2 file) owns the bpffs
  mount-mode + uid:gid + SELinux/AppArmor label decision. This
  plan defers to that finding for the threat model; the values
  recorded here (`0750`, owned `expressgateway:expressgateway`,
  `nosuid,nodev,noexec`) are the proposed defaults pending sec's
  arbitration.
- REL-2-02 (reload-zero-drop): pinning is the unblocker. Once
  this plan lands, rel's reload-drop test can meaningfully
  exercise XDP state preservation.
- EBPF-2-08 (STATS export): STATS gets pinned too, so the metric
  collector continues working across restarts without losing
  counter history.

Owner:          ebpf
Lead-approval: approved 2026-05-13 team-lead
