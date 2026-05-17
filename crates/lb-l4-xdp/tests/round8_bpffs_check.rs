//! ROUND8-L4-11 proof: `bpffs::assert_bpffs` fail-fasts when the pin
//! directory the loader is asked to use is NOT bpffs. The runtime
//! check turns the deep-aya `EbpfError::Map(InvalidPin)` trail into a
//! typed `PinPathNotBpffs` with an operator-actionable hint.
//!
//! The kernel-touching test (`load_from_bytes_pinned_fails_fast_on_tmpfs`)
//! is `#[ignore]` because it requires CAP_BPF; the user-mode-only
//! tests run in CI on any Linux runner.
//!
//! Cross-ref: OPS-07 owns the systemd unit-file half of this bundle
//! (`RequiresMountsFor=/sys/fs/bpf` ensures the runtime check is
//! always satisfied in production).

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use lb_l4_xdp::bpffs::{BPF_FS_MAGIC, assert_bpffs};
use lb_l4_xdp::loader::XdpLoaderError;

#[test]
fn bpf_fs_magic_matches_uapi_constant() {
    // Sanity that the constant is what include/uapi/linux/magic.h
    // declares. A wrong magic here breaks the entire fail-fast.
    assert_eq!(BPF_FS_MAGIC, 0xCAFE_4A11);
}

#[test]
fn rejects_tempdir_with_actionable_hint() {
    // The CI runner's `std::env::temp_dir()` is never bpffs (it's
    // tmpfs or ext4). The fail-fast must mention the mount command
    // so the operator does not have to read the source to find it.
    let tmp = std::env::temp_dir();
    match assert_bpffs(&tmp) {
        Err(XdpLoaderError::PinPathNotBpffs {
            hint,
            found_magic,
            path,
        }) => {
            assert_eq!(path, tmp);
            assert_ne!(found_magic, BPF_FS_MAGIC);
            assert!(
                hint.contains("mount -t bpf"),
                "hint must surface the mount command for operators; got: {hint}",
            );
        }
        // Sandbox might refuse statfs; PinPathStatFailed is also
        // acceptable. The forbidden case is `Ok(())` — a false-
        // positive on a non-bpffs path would defeat the entire point.
        Err(XdpLoaderError::PinPathStatFailed { .. }) => {}
        Ok(()) => panic!("tempdir cannot be bpffs but assert_bpffs returned Ok"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn rejects_missing_path() {
    // The fail-fast for `/nonexistent` is `PinPathStatFailed` with
    // `NotFound` kind; the operator sees which exact path is missing.
    let p = PathBuf::from("/nonexistent/eg/bpffs-check-test");
    match assert_bpffs(&p) {
        Err(XdpLoaderError::PinPathStatFailed { source, path }) => {
            assert_eq!(path, p);
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected PinPathStatFailed(NotFound), got {other:?}"),
    }
}

#[test]
fn rejects_relative_root() {
    // Path::new(".") is the CWD. It is by definition NOT bpffs (no
    // test runs out of /sys/fs/bpf). Either NotBpffs or
    // PinPathStatFailed — both are correct fail-fast behaviours.
    let p = Path::new(".");
    let r = assert_bpffs(p);
    assert!(
        !matches!(r, Ok(())),
        "CWD cannot be bpffs; assert_bpffs must not return Ok, got: {r:?}",
    );
}

/// Ignored: requires CAP_BPF + a real bpffs mount. CI's privileged
/// lane runs `mount -t bpf bpffs /tmp/test-bpffs` before invoking the
/// `--ignored` suite. The assertion here is that the happy path
/// returns `Ok(())`.
#[test]
#[ignore = "requires CAP_BPF and a bpffs mount; see CI privileged lane"]
fn accepts_real_bpffs() {
    let p = Path::new("/tmp/test-bpffs");
    assert!(p.exists(), "fixture: /tmp/test-bpffs must be mounted bpffs");
    let r = assert_bpffs(p);
    assert!(r.is_ok(), "real bpffs must be accepted: {r:?}");
}
