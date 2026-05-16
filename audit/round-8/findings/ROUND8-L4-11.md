### ROUND8-L4-11 — Loader does not enforce `/sys/fs/bpf` mount-type before pinning, despite doc comment claiming otherwise

Reference: `audit/round-8/research/xdp-tutorial.md` lesson 6 + D2 (`mount -t bpf bpf /sys/fs/bpf/` is a prerequisite for pinning); handoff cross-cutting item 4 (mount-check + reload-unpin)
Our equivalent: `crates/lb-l4-xdp/src/loader.rs:58-62` (doc comment on `DEFAULT_PIN_DIR`), `:527-544` (`load_from_bytes_pinned` — no statfs)

Severity: medium
Status:   Verified-Fixed (verify, task#70, 2026-05-15) — pinned-load path calls bpffs::assert_bpffs (statfs BPF_FS_MAGIC) with fail-fast PinPathNotBpffs/PinPathStatFailed; doc-claims-but-not-enforced gap closed. Proof 4/4 (+1 ignored). See audit/round-8/verify/l4.md.

Divergence:
- xdp-tutorial: a loader either mounts bpffs or refuses to start with a clear error. Silent fallback to a non-bpffs path (e.g. a regular tmpfs) leaves *nothing-shared-with-userspace*; pinned maps land in a directory the kernel doesn't treat as bpffs, and other tools (`bpftool map show`, prometheus exporters) cannot see them.
- The handoff is explicit (cross-cutting item 4(a)): "`/sys/fs/bpf/expressgateway` exists and has fstype `bpf` (mountpoint, not just dir)".
- Our `DEFAULT_PIN_DIR` doc comment (`loader.rs:58-62`) says: "Caller is responsible for ensuring the directory exists with the correct mode/owner... see `crates/lb/src/xdp.rs` and the systemd unit". `crates/lb/src/xdp.rs` does **not** check mount type. The systemd unit might; that's an operations check, not a runtime guard.
- `load_from_bytes_pinned` calls `loader.map_pin_path(p)`. Aya creates the pin under `p`; if `p` is `/sys/fs/bpf/expressgateway` but `/sys/fs/bpf` is not mounted as `bpf`, aya gets an `EINVAL` from `bpf(BPF_OBJ_PIN)` deep inside the load path — error message is not actionable.

Impact:
- On a deploy where the systemd unit's `RequiresMountsFor=/sys/fs/bpf` is omitted, the loader fails with an opaque `EbpfError::Map(InvalidPin(...))` instead of "bpffs not mounted at /sys/fs/bpf; run `mount -t bpf bpffs /sys/fs/bpf`".
- For container deployments where the host's bpffs isn't bind-mounted into the container, same opaque failure.
- The CRD-style RBAC may dictate the dir exists with right perms (the doc emphasises that) but never check fstype.

Reproduction:
- On a clean Ubuntu container, `mkdir -p /sys/fs/bpf/expressgateway` without `mount -t bpf`. Run `XdpLoader::load_from_bytes_pinned(elf, Some(Path::new("/sys/fs/bpf/expressgateway")))`. Observe the opaque pin error.

Recommendation:
1. Before `loader.map_pin_path(p)`, call `nix::sys::statfs::statfs(p)?` (or read `/proc/self/mountinfo` and grep) and assert `f_type == 0xCAFE4A11` (`BPF_FS_MAGIC`). Return `XdpLoaderError::PinPathNotBpffs(PathBuf)` with a one-liner "run `mount -t bpf bpffs <path>`".
2. Add an integration test `bpffs_mount_check_fails_loudly` that creates a tmpfs dir and asserts the error is the new variant.
3. Update the doc comment on `DEFAULT_PIN_DIR` to say "mount-type is verified at runtime in `load_from_bytes_pinned`" once the check lands.
