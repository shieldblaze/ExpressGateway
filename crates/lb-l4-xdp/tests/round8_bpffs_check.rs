//! ROUND8-L4-11 proof: `bpffs::assert_bpffs` fail-fasts when the pin directory the loader is asked to use is NOT bpffs.

#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};

use lb_l4_xdp::bpffs::{BPF_FS_MAGIC, assert_bpffs};
use lb_l4_xdp::loader::XdpLoaderError;

#[test]
fn bpf_fs_magic_matches_uapi_constant() {
    assert_eq!(BPF_FS_MAGIC, 0xCAFE_4A11);
}

#[test]
fn rejects_tempdir_with_actionable_hint() {
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
        Err(XdpLoaderError::PinPathStatFailed { .. }) => {}
        Ok(()) => panic!("tempdir cannot be bpffs but assert_bpffs returned Ok"),
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn rejects_missing_path() {
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
    let p = Path::new(".");
    let r = assert_bpffs(p);
    assert!(
        !matches!(r, Ok(())),
        "CWD cannot be bpffs; assert_bpffs must not return Ok, got: {r:?}",
    );
}

/// Ignored: requires CAP_BPF + a real bpffs mount.
#[test]
#[ignore = "requires CAP_BPF and a bpffs mount; see CI privileged lane"]
fn accepts_real_bpffs() {
    let p = Path::new("/tmp/test-bpffs");
    assert!(p.exists(), "fixture: /tmp/test-bpffs must be mounted bpffs");
    let r = assert_bpffs(p);
    assert!(r.is_ok(), "real bpffs must be accepted: {r:?}");
}
