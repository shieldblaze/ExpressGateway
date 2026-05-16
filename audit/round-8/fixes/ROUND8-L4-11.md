# Plan for ROUND8-L4-11 — bpffs mount-type check at runtime (paired with OPS-07 systemd hardening)

Finding-ref:     ROUND8-L4-11 (medium, Open)
Reference:       xdp-tutorial lessons 6 + 7 (`mount -t bpf bpf /sys/fs/bpf/` is a prerequisite for pinning); handoff cross-cutting item 4(a) ("`/sys/fs/bpf/expressgateway` exists and has fstype `bpf`").
Coverage-gap:    Theme 1 (EBPF-2-05 audited pinning paths existing; runtime fstype check never audited). Bundle B-4 with OPS-07: div-ops owns the systemd hardening half; this plan supplies the bpffs mount/perm contract that the systemd unit must satisfy.

Files touched:
  - `crates/lb-l4-xdp/src/loader.rs`                 (`load_from_bytes_pinned`: statfs the pin dir BEFORE `map_pin_path`; new error variant; doc comment correction)
  - `crates/lb-l4-xdp/src/bpffs.rs`                  (NEW — `assert_bpffs(&Path)` helper; constants for `BPF_FS_MAGIC` etc.)
  - `crates/lb-l4-xdp/Cargo.toml`                    (add `nix` with the `fs` feature for `statfs`; gated on `target_os = "linux"`)
  - `crates/lb-l4-xdp/tests/round8_bpffs_check.rs`   (NEW — proof; tests run with tmpfs to simulate the non-bpffs case)
  - `audit/round-8/fixes/ROUND8-OPS-07.md`           (cross-reference — the bpffs mount contract this plan owns is the input to OPS-07's `RequiresMountsFor=` directive in the systemd unit)

Approach (this plan owns the bpffs runtime contract; OPS-07 owns the unit-file hardening):

1. **`bpffs.rs` helper** (`crates/lb-l4-xdp/src/bpffs.rs`, NEW):
   ```rust
   use std::path::Path;
   use nix::sys::statfs::statfs;
   use crate::loader::XdpLoaderError;

   /// Magic for `BPF_FS_MAGIC` — kernel `include/uapi/linux/magic.h`.
   pub const BPF_FS_MAGIC: i64 = 0xCAFE4A11;

   /// Verify that `path` resolves to a directory backed by bpffs.
   /// Returns the resolved path on success or a typed error with
   /// an operator-actionable message.
   pub fn assert_bpffs(path: &Path) -> Result<(), XdpLoaderError> {
       let st = statfs(path).map_err(|e| {
           XdpLoaderError::PinPathStatFailed { path: path.to_path_buf(), source: e.into() }
       })?;
       let fs_type: i64 = st.filesystem_type().0 as i64;
       if fs_type != BPF_FS_MAGIC {
           return Err(XdpLoaderError::PinPathNotBpffs {
               path: path.to_path_buf(),
               found_magic: fs_type,
               hint: "run `mount -t bpf bpffs /sys/fs/bpf` (or set RequiresMountsFor=/sys/fs/bpf in the systemd unit)".into(),
           });
       }
       Ok(())
   }
   ```
   - `nix` crate is already in our `Cargo.lock`; add to
     `lb-l4-xdp` deps under `[target.'cfg(target_os = "linux")'.dependencies]`.

2. **Wire into `load_from_bytes_pinned`** (`crates/lb-l4-xdp/src/loader.rs:527-544`):
   ```rust
   pub fn load_from_bytes_pinned(
       bytes: &[u8],
       pin_dir: Option<&Path>,
   ) -> Result<Self, XdpLoaderError> {
       let mut loader = EbpfLoader::new();
       if let Some(p) = pin_dir {
           crate::bpffs::assert_bpffs(p)?;   // NEW — fail fast, before map_pin_path
           loader.map_pin_path(p);
       }
       // existing body
       loader.load(bytes).map_err(Into::into).map(|ebpf| Self { ebpf })
   }
   ```

3. **New error variants** (`crates/lb-l4-xdp/src/loader.rs`):
   ```rust
   #[error("pin path {path:?} is not bpffs (found magic 0x{found_magic:x}); {hint}")]
   PinPathNotBpffs { path: PathBuf, found_magic: i64, hint: String },
   #[error("statfs on pin path {path:?} failed: {source}")]
   PinPathStatFailed { path: PathBuf, #[source] source: io::Error },
   ```

4. **Doc comment correction** (`crates/lb-l4-xdp/src/loader.rs:58-62`):
   - Today: "Caller is responsible for ensuring the directory
     exists with the correct mode/owner... see `crates/lb/src/xdp.rs`
     and the systemd unit."
   - After: "Mount-type is verified at runtime in
     `load_from_bytes_pinned` (see `bpffs::assert_bpffs`). The
     directory itself must be created by the systemd unit with
     `0750 root:bpf` ownership and `RequiresMountsFor=/sys/fs/bpf`
     (see `packaging/expressgateway.service` and DEPLOYMENT.md §27)."

5. **bpffs/permission contract for OPS-07** (the divider between
   plans):
   - Mount: `tmpfs` -> `bpf` at `/sys/fs/bpf` via systemd's default
     bpffs mount unit. If the deploy host runs an older systemd
     (<252) that does not auto-mount bpffs, the LB unit must
     declare `RequiresMountsFor=/sys/fs/bpf`. Reading: this plan
     mandates the runtime check; OPS-07 mandates the unit-file
     directive so the host always satisfies it.
   - Pin sub-dir: `/sys/fs/bpf/expressgateway`, mode `0750`, owner
     `root:expressgateway` (or whatever uid:gid the unit's `User=` /
     `Group=` declares). The unit's `RuntimeDirectory=` is NOT used
     because bpffs is not a tmpfs.
   - Pre-start: the unit declares `ExecStartPre=+/bin/mkdir -p /sys/fs/bpf/expressgateway`
     and `ExecStartPre=+/bin/chmod 0750 /sys/fs/bpf/expressgateway`
     (the `+` prefix runs as root, before the unprivileged `ExecStart`).
   - Stop: `ExecStopPost=+/bin/rm -rf /sys/fs/bpf/expressgateway`
     is NOT used (we want pinned maps to survive a restart per
     ROUND8-L4-12 cross-ref). Cleanup is operator-explicit.
   - Capability: `CAP_BPF` plus `CAP_NET_ADMIN` on the unit; **not**
     `CAP_SYS_ADMIN` (SEC-2-11 closure stands).

6. **Proof tests** (`crates/lb-l4-xdp/tests/round8_bpffs_check.rs`,
   NEW):
   - `bpffs_check_rejects_tmpfs`: create a tempdir on the test
     runner (which is tmpfs/ext4, not bpffs); call
     `assert_bpffs(&tempdir)`; assert
     `Err(PinPathNotBpffs { found_magic, .. })` and that `found_magic != BPF_FS_MAGIC`.
   - `bpffs_check_rejects_missing_path`: `assert_bpffs(Path::new("/nonexistent"))`;
     assert `Err(PinPathStatFailed { source, .. })` and that
     `source.kind() == io::ErrorKind::NotFound`.
   - `bpffs_check_accepts_real_bpffs`: gated `#[ignore]` (needs CAP_BPF +
     a mounted bpffs); CI's `--ignored` job mounts
     `/tmp/test-bpffs` via `mount -t bpf bpffs /tmp/test-bpffs` and
     asserts `assert_bpffs` returns `Ok(())`.
   - `load_from_bytes_pinned_fails_fast_on_tmpfs`: ignored test —
     full `XdpLoader::load_from_bytes_pinned` with a tmpfs path,
     asserts the error is `PinPathNotBpffs` (not the deep aya
     EINVAL it would have produced before this fix).

Proof:

- `cargo test -p lb-l4-xdp --test round8_bpffs_check` (the
  non-ignored cases run in CI without privileges).
- The `--ignored` cases run in the existing privileged CI lane.
- After the patch, an operator who deploys without `RequiresMountsFor=`
  gets the operator-actionable error message at startup instead of
  the opaque `EbpfError::Map(InvalidPin)` from deep in aya.

Risk / blast radius:

- `statfs` on `/sys/fs/bpf/expressgateway` requires the path to be
  resolvable; the unit's `ExecStartPre=+/bin/mkdir -p` step makes
  this reliable. If the operator's unit lacks the pre-step, the
  loader's error message tells them what to do.
- `nix` crate brings in a small set of unsafe libc bindings; the
  `fs` feature is the minimum surface for `statfs`. Track in
  `audit/unsafe-justifications.md` if the crate's exposure widens.
- Container deployments: `nsenter`/podman without bpffs bind-mount
  fail loudly with the new error — that's the intended outcome.

Cross-ref:
- Bundle B-4 peer: OPS-07 (systemd unit hardening — the unit file
  consumes this plan's mount/perm contract).
- `audit/ebpf/round-2-review.md` EBPF-2-05 (pin-name stability):
  unchanged; this plan adds the runtime guard on top.
- ROUND8-L4-12 (XDP attach replace): the L4-12 plan describes the
  pin survival policy across restarts; bpffs mount must survive
  for L4-12's `bpf_xdp_query` reuse logic to work.

Owner:           div-l4 (bpffs runtime check); div-ops (systemd unit, via OPS-07)
Lead-approval: approved 2026-05-14 team-lead-r8
